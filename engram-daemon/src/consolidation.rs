use std::future::Future;
use std::pin::Pin;

use engram_db::{cosine_similarity, EdgeKind, EngramDb, FragmentId};

use crate::claude_client::ClaudeClient;
use crate::config::ConsolidationConfig;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Run all consolidation phases.
pub async fn run_consolidation(
    db: &EngramDb,
    client: Option<&ClaudeClient>,
    config: &ConsolidationConfig,
) -> Result<ConsolidationStats, Box<dyn std::error::Error>> {
    let mut stats = ConsolidationStats::default();

    tracing::info!("Starting consolidation...");

    // Phase 1: Similarity detection + topic merging
    let similar_pairs = phase1_similarity_detection(db, config.similarity_threshold);
    tracing::info!("Phase 1: Found {} similar topic pairs", similar_pairs.len());

    // Merge highly similar topics (similarity > 0.9)
    stats.topics_merged = phase1_topic_merging(db, &similar_pairs)?;
    tracing::info!("Phase 1: Merged {} topic pairs", stats.topics_merged);

    // Phase 2: Create associative links between related concepts
    // Re-detect after merging since some pairs may have been merged
    let similar_pairs = if stats.topics_merged > 0 {
        phase1_similarity_detection(db, config.similarity_threshold)
    } else {
        similar_pairs
    };
    stats.links_created = phase2_link_creation(db, &similar_pairs)?;
    tracing::info!("Phase 2: Created {} associative links", stats.links_created);

    // Phase 3: Re-summarization of topics with modified children
    if let Some(client) = client {
        stats.topics_resummarized = phase3_resummarization(db, client).await?;
        tracing::info!(
            "Phase 3: Re-summarized {} topics",
            stats.topics_resummarized
        );

        // Phase 4: Contradiction resolution
        stats.contradictions_resolved = phase4_contradiction_resolution(db, client).await?;
        tracing::info!(
            "Phase 4: Resolved {} contradictions",
            stats.contradictions_resolved
        );
    } else {
        tracing::info!("Phase 3-4: Skipped (no API key)");
    }

    // Phase 5: Pruning
    stats.edges_pruned = phase5_pruning(db, config)?;
    tracing::info!("Phase 5: Pruned {} weak edges", stats.edges_pruned);

    tracing::info!("Consolidation complete: {:?}", stats);
    Ok(stats)
}

#[derive(Debug, Default)]
pub struct ConsolidationStats {
    pub topics_merged: usize,
    pub links_created: usize,
    pub topics_resummarized: usize,
    pub contradictions_resolved: usize,
    pub edges_pruned: usize,
}

/// Phase 1: Find pairs of L0 topics with high semantic similarity.
fn phase1_similarity_detection(
    db: &EngramDb,
    threshold: f32,
) -> Vec<(FragmentId, FragmentId, f32)> {
    let topics = db.list_topics();
    let mut pairs = Vec::new();

    for i in 0..topics.len() {
        if topics[i].embedding.is_empty() {
            continue;
        }
        for j in (i + 1)..topics.len() {
            if topics[j].embedding.is_empty() {
                continue;
            }
            let sim = cosine_similarity(&topics[i].embedding, &topics[j].embedding);
            if sim > threshold {
                pairs.push((topics[i].id, topics[j].id, sim));
            }
        }
    }

    pairs
}

/// Merge topic pairs with very high similarity (>0.9).
/// Picks the survivor (higher access_count), reparents victim's children, supersedes victim.
fn phase1_topic_merging(
    db: &EngramDb,
    similar_pairs: &[(FragmentId, FragmentId, f32)],
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut merged = 0;

    for &(id_a, id_b, sim) in similar_pairs {
        if sim <= 0.9 {
            continue;
        }

        // Load both fragments to determine survivor
        let frag_a = match db.storage().get_fragment(id_a)? {
            Some(f) if f.superseded_by.is_none() => f,
            _ => continue, // already merged or missing
        };
        let frag_b = match db.storage().get_fragment(id_b)? {
            Some(f) if f.superseded_by.is_none() => f,
            _ => continue,
        };

        let (survivor_id, victim_id) = if frag_a.access_count >= frag_b.access_count {
            (id_a, id_b)
        } else {
            (id_b, id_a)
        };

        // Reparent victim's children to survivor
        let victim_children = db.children(victim_id);
        for child in &victim_children {
            db.storage()
                .delete_edge_between(victim_id, child.id, EdgeKind::Hierarchical)?;
            db.link(survivor_id, child.id, EdgeKind::Hierarchical, 1.0)?;
        }

        // Supersede victim
        db.supersede(victim_id, survivor_id)?;
        merged += 1;

        tracing::info!(
            "Merged topic {} into {} (sim={:.3})",
            victim_id,
            survivor_id,
            sim
        );
    }

    Ok(merged)
}

/// Phase 2: For each similar topic pair, create associative links between their children.
fn phase2_link_creation(
    db: &EngramDb,
    similar_pairs: &[(FragmentId, FragmentId, f32)],
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut links_created = 0;
    let cross_threshold = 0.7;

    for &(topic_a, topic_b, _) in similar_pairs {
        let children_a = db.children(topic_a);
        let children_b = db.children(topic_b);

        for ca in &children_a {
            if ca.embedding.is_empty() {
                continue;
            }
            for cb in &children_b {
                if cb.embedding.is_empty() {
                    continue;
                }
                let sim = cosine_similarity(&ca.embedding, &cb.embedding);
                if sim > cross_threshold {
                    db.link(ca.id, cb.id, EdgeKind::Associative, sim)?;
                    links_created += 1;
                }
            }
        }
    }

    Ok(links_created)
}

/// Phase 3: Re-summarize topics whose children have been modified since last access.
async fn phase3_resummarization(
    db: &EngramDb,
    client: &ClaudeClient,
) -> Result<usize, Box<dyn std::error::Error>> {
    let topics = db.list_topics();
    let mut resummarized = 0;

    for topic in &topics {
        let children = db.children(topic.id);
        if children.is_empty() {
            continue;
        }

        // Check if any children were created/modified after the topic was last accessed
        let has_new_children = children.iter().any(|c| c.created_at > topic.last_accessed);

        if !has_new_children {
            continue;
        }

        // Build a summary of children for Claude to synthesize
        let children_summaries: Vec<String> = children
            .iter()
            .map(|c| format!("- {}: {}", c.summary, c.content))
            .collect();

        let prompt = format!(
            "Given these sub-topics of \"{}\", write a self-contained overview paragraph \
             (3-5 sentences) that captures the key knowledge. Do not use bullet points or \
             lists — write flowing prose.\n\nSub-topics:\n{}\n\nRespond with ONLY the paragraph, \
             no explanation.",
            topic.summary,
            children_summaries.join("\n")
        );

        match client.complete(&prompt).await {
            Ok(new_content) => {
                let new_content = new_content.trim();
                if !new_content.is_empty() {
                    db.update(topic.id, new_content, &topic.summary)?;
                    resummarized += 1;
                }
            }
            Err(e) => {
                tracing::warn!("Failed to re-summarize topic '{}': {}", topic.summary, e);
            }
        }
    }

    Ok(resummarized)
}

/// Phase 4: Detect contradictions between sibling fragments within the same parent.
async fn phase4_contradiction_resolution(
    db: &EngramDb,
    client: &ClaudeClient,
) -> Result<usize, Box<dyn std::error::Error>> {
    let topics = db.list_topics();
    let mut resolved = 0;

    for topic in &topics {
        // Check children at each level for contradictions
        resolved += check_siblings_for_contradictions(db, client, topic.id).await?;
    }

    Ok(resolved)
}

/// Recursively check sibling fragments for contradictions.
fn check_siblings_for_contradictions<'a>(
    db: &'a EngramDb,
    client: &'a ClaudeClient,
    parent_id: FragmentId,
) -> BoxFuture<'a, Result<usize, Box<dyn std::error::Error>>> {
    Box::pin(async move {
        let children = db.children(parent_id);
        let mut resolved = 0;

        // Check pairs of siblings
        for i in 0..children.len() {
            for j in (i + 1)..children.len() {
                if !children[i].embedding.is_empty() && !children[j].embedding.is_empty() {
                    let sim = cosine_similarity(&children[i].embedding, &children[j].embedding);
                    if sim < 0.5 {
                        continue;
                    }
                }

                let prompt = format!(
                    "Do these two statements contradict each other? Answer only 'yes' or 'no'.\n\n\
                     Statement A: {}\n\nStatement B: {}",
                    children[i].content, children[j].content
                );

                match client.complete(&prompt).await {
                    Ok(response) => {
                        if response.trim().to_lowercase().starts_with("yes") {
                            let (old, new) = if children[i].created_at < children[j].created_at {
                                (&children[i], &children[j])
                            } else {
                                (&children[j], &children[i])
                            };

                            if let Err(e) = db.supersede(old.id, new.id) {
                                tracing::warn!("Failed to supersede: {}", e);
                            } else {
                                resolved += 1;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Claude API error during contradiction check: {}", e);
                    }
                }
            }
        }

        // Recurse into children
        for child in &children {
            resolved += check_siblings_for_contradictions(db, client, child.id).await?;
        }

        Ok(resolved)
    })
}

/// Phase 5: Prune weak edges.
fn phase5_pruning(
    db: &EngramDb,
    _config: &ConsolidationConfig,
) -> Result<usize, Box<dyn std::error::Error>> {
    let pruned = db.storage().delete_weak_edges(EdgeKind::Associative, 0.3)?;

    Ok(pruned)
}
