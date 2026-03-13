use lore_db::fragment::now_unix;
use lore_db::{cosine_similarity, EdgeKind, Fragment, FragmentId, LoreDb};

use crate::claude_client::ClaudeClient;
use crate::config::ConsolidationConfig;
use crate::ingestion;
use crate::parser::ConversationTurn;

/// Run all consolidation phases.
pub async fn run_consolidation(
    db: &LoreDb,
    client: Option<&ClaudeClient>,
    config: &ConsolidationConfig,
) -> Result<ConsolidationStats, Box<dyn std::error::Error>> {
    let mut stats = ConsolidationStats::default();
    let now = now_unix();

    tracing::info!("Starting consolidation...");

    // Phase 0: Digest staged conversations into knowledge fragments
    if let Some(client) = client {
        let (sessions, fragments) = phase0_digest_staged(db, client, config).await?;
        stats.sessions_digested = sessions;
        stats.fragments_extracted = fragments;
        tracing::info!(
            "Phase 0: Digested {} sessions, extracted {} fragments",
            sessions,
            fragments
        );
    }

    // Phase 1: Decay recomputation — the "sleep cycle"
    stats.relevance_updated = db.storage().recompute_all_relevance(now)?;
    tracing::info!(
        "Phase 1: Recomputed relevance for {} fragments",
        stats.relevance_updated
    );

    // Phase 2: Similarity detection + root merging
    let similar_pairs = phase1_similarity_detection(db, config.similarity_threshold);
    tracing::info!("Phase 2: Found {} similar root pairs", similar_pairs.len());

    stats.roots_merged = phase1_root_merging(db, &similar_pairs, config.merge_threshold)?;
    tracing::info!("Phase 2: Merged {} root pairs", stats.roots_merged);

    // Phase 3: Create associative links between related concepts
    let similar_pairs = if stats.roots_merged > 0 {
        phase1_similarity_detection(db, config.similarity_threshold)
    } else {
        similar_pairs
    };
    stats.links_created = phase2_link_creation(db, &similar_pairs)?;
    tracing::info!("Phase 3: Created {} associative links", stats.links_created);

    // Phase 4: Re-summarization of roots with modified children
    if let Some(client) = client {
        stats.roots_resummarized = phase3_resummarization(db, client).await?;
        tracing::info!("Phase 4: Re-summarized {} roots", stats.roots_resummarized);

        // Phase 5: Contradiction resolution
        stats.contradictions_resolved = phase4_contradiction_resolution(db, client).await?;
        tracing::info!(
            "Phase 5: Resolved {} contradictions",
            stats.contradictions_resolved
        );
    } else {
        tracing::info!("Phase 4-5: Skipped (no API key)");
    }

    // Phase 6: Edge pruning (with decay)
    stats.edges_pruned = phase5_pruning(db, config)?;
    tracing::info!("Phase 6: Pruned {} weak edges", stats.edges_pruned);

    // Phase 7: Fragment pruning by relevance — true forgetting
    stats.fragments_pruned = phase6_fragment_pruning(db, config, now)?;
    tracing::info!(
        "Phase 7: Pruned {} low-relevance fragments",
        stats.fragments_pruned
    );

    tracing::info!("Consolidation complete: {:?}", stats);
    Ok(stats)
}

#[derive(Debug, Default)]
pub struct ConsolidationStats {
    pub sessions_digested: usize,
    pub fragments_extracted: usize,
    pub relevance_updated: usize,
    pub roots_merged: usize,
    pub links_created: usize,
    pub roots_resummarized: usize,
    pub contradictions_resolved: usize,
    pub edges_pruned: usize,
    pub fragments_pruned: usize,
}

/// Max sessions to digest per consolidation run.
const MAX_SESSIONS_PER_CONSOLIDATION: usize = 10;

/// Phase 0: Digest staged conversation turns into knowledge fragments.
async fn phase0_digest_staged(
    db: &LoreDb,
    client: &ClaudeClient,
    config: &ConsolidationConfig,
) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let now = now_unix();
    let sessions = db
        .storage()
        .get_staged_sessions(config.idle_threshold_secs, now)?;

    if sessions.is_empty() {
        return Ok((0, 0));
    }

    let mut total_sessions = 0;
    let mut total_fragments = 0;

    for session in sessions.iter().take(MAX_SESSIONS_PER_CONSOLIDATION) {
        let staged_turns = db.storage().get_staged_turns(&session.file_path)?;
        if staged_turns.is_empty() {
            continue;
        }

        let turns: Vec<ConversationTurn> = staged_turns
            .iter()
            .map(|t| ConversationTurn {
                role: t.role.clone(),
                text: t.text.clone(),
            })
            .collect();

        tracing::info!("Digesting {} turns from {}", turns.len(), session.file_path);

        // Read session metadata from the JSONL file
        let meta = crate::parser::read_session_metadata(&session.file_path);
        let session_ctx = ingestion::SessionContext {
            cwd: meta.cwd,
            git_branch: meta.git_branch,
        };

        // Derive session ID from file path
        let session_id = std::path::Path::new(&session.file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());

        // Build existing root context
        let existing_roots: Vec<ingestion::ExistingRootContext> = db
            .list_roots(None)
            .into_iter()
            .map(|t| {
                let children_content = db.children(t.id).into_iter().map(|c| c.content).collect();
                ingestion::ExistingRootContext {
                    id: t.id.to_string(),
                    content: t.content.clone(),
                    children_content,
                }
            })
            .collect();

        // Chunk large conversations
        let chunks: Vec<&[ConversationTurn]> = if turns.len() > config.max_turns_per_extraction {
            turns.chunks(config.max_turns_per_extraction).collect()
        } else {
            vec![&turns]
        };

        for chunk in &chunks {
            match ingestion::extract_knowledge(client, chunk, &existing_roots, Some(&session_ctx))
                .await
            {
                Ok(knowledge) => {
                    match ingestion::store_knowledge(db, &knowledge, session_id.as_deref()) {
                        Ok(count) => {
                            total_fragments += count;
                            tracing::info!("Stored {} fragments", count);
                        }
                        Err(e) => tracing::error!("Storage failed (continuing): {}", e),
                    }
                }
                Err(e) => tracing::error!("Extraction failed (continuing): {}", e),
            }
        }

        db.storage().delete_staged_turns(&session.file_path)?;
        total_sessions += 1;
    }

    Ok((total_sessions, total_fragments))
}

/// Phase 1: Find pairs of L0 roots with high semantic similarity.
fn phase1_similarity_detection(db: &LoreDb, threshold: f32) -> Vec<(FragmentId, FragmentId, f32)> {
    let roots = db.list_roots(None);
    let mut pairs = Vec::new();

    for i in 0..roots.len() {
        if roots[i].embedding.is_empty() {
            continue;
        }
        for j in (i + 1)..roots.len() {
            if roots[j].embedding.is_empty() {
                continue;
            }
            let sim = cosine_similarity(&roots[i].embedding, &roots[j].embedding);
            if sim > threshold {
                pairs.push((roots[i].id, roots[j].id, sim));
            }
        }
    }

    pairs
}

/// Merge root pairs above the merge threshold.
/// Picks the survivor (higher access_count), reparents victim's children, supersedes victim.
fn phase1_root_merging(
    db: &LoreDb,
    similar_pairs: &[(FragmentId, FragmentId, f32)],
    merge_threshold: f32,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut merged = 0;

    for &(id_a, id_b, sim) in similar_pairs {
        if sim <= merge_threshold {
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
            "Merged root {} into {} (sim={:.3})",
            victim_id,
            survivor_id,
            sim
        );
    }

    Ok(merged)
}

/// Phase 2: For each similar root pair, create associative links between their children.
fn phase2_link_creation(
    db: &LoreDb,
    similar_pairs: &[(FragmentId, FragmentId, f32)],
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut links_created = 0;
    let cross_threshold = 0.7;

    for &(root_a, root_b, _) in similar_pairs {
        let children_a = db.children(root_a);
        let children_b = db.children(root_b);

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

/// Phase 3: Re-summarize roots whose children have been modified since last access.
async fn phase3_resummarization(
    db: &LoreDb,
    client: &ClaudeClient,
) -> Result<usize, Box<dyn std::error::Error>> {
    let roots = db.list_roots(None);
    let mut resummarized = 0;

    for root in &roots {
        let children = db.children(root.id);
        if children.is_empty() {
            continue;
        }

        // Check if any children were created/modified after the root was last accessed
        let has_new_children = children.iter().any(|c| c.created_at > root.last_accessed);

        if !has_new_children {
            continue;
        }

        // Build a list of children content for Claude to synthesize
        let children_list: Vec<String> = children
            .iter()
            .map(|c| format!("- {}", c.content))
            .collect();

        let root_preview: String = root.content.chars().take(200).collect();
        let prompt = format!(
            "Given these child fragments of a knowledge node:\n\nCurrent content: \"{}\"\n\n\
             Children:\n{}\n\n\
             Write a self-contained overview paragraph (3-5 sentences) that captures the key \
             knowledge at a higher abstraction level. Do not use bullet points or lists — \
             write flowing prose.\n\nRespond with ONLY the paragraph, no explanation.",
            root_preview,
            children_list.join("\n")
        );

        match client.complete(&prompt).await {
            Ok(new_content) => {
                let new_content = new_content.trim();
                if !new_content.is_empty() {
                    db.update(root.id, new_content)?;
                    resummarized += 1;
                }
            }
            Err(e) => {
                tracing::warn!("Failed to re-summarize root: {}", e);
            }
        }
    }

    Ok(resummarized)
}

/// Phase 4: Detect contradictions between sibling fragments within the same parent.
/// Collects all candidate pairs first, then batch-checks them to minimize API calls.
async fn phase4_contradiction_resolution(
    db: &LoreDb,
    client: &ClaudeClient,
) -> Result<usize, Box<dyn std::error::Error>> {
    let roots = db.list_roots(None);

    // Collect all candidate pairs across the entire tree
    let mut candidates = Vec::new();
    for root in &roots {
        collect_contradiction_candidates(db, root.id, &mut candidates);
    }

    if candidates.is_empty() {
        return Ok(0);
    }

    tracing::info!(
        "Checking {} candidate pairs for contradictions",
        candidates.len()
    );

    // Process in batches
    let mut resolved = 0;
    for batch in candidates.chunks(CONTRADICTION_BATCH_SIZE) {
        resolved += check_contradiction_batch(db, client, batch).await?;
    }

    Ok(resolved)
}

/// A candidate contradiction pair to check.
struct ContradictionCandidate {
    id_a: FragmentId,
    id_b: FragmentId,
    content_a: String,
    content_b: String,
    created_a: i64,
    created_b: i64,
}

/// Collect all candidate contradiction pairs from the tree, then batch-check them.
fn collect_contradiction_candidates(
    db: &LoreDb,
    parent_id: FragmentId,
    candidates: &mut Vec<ContradictionCandidate>,
) {
    let children = db.children(parent_id);

    for i in 0..children.len() {
        for j in (i + 1)..children.len() {
            if !children[i].embedding.is_empty() && !children[j].embedding.is_empty() {
                let sim = cosine_similarity(&children[i].embedding, &children[j].embedding);
                if sim < 0.5 {
                    continue;
                }
            }

            candidates.push(ContradictionCandidate {
                id_a: children[i].id,
                id_b: children[j].id,
                content_a: children[i].content.clone(),
                content_b: children[j].content.clone(),
                created_a: children[i].created_at,
                created_b: children[j].created_at,
            });
        }
    }

    // Recurse into children
    for child in &children {
        collect_contradiction_candidates(db, child.id, candidates);
    }
}

/// Maximum pairs per batched contradiction-check API call.
const CONTRADICTION_BATCH_SIZE: usize = 10;

/// Check a batch of candidate pairs for contradictions in a single API call.
async fn check_contradiction_batch(
    db: &LoreDb,
    client: &ClaudeClient,
    batch: &[ContradictionCandidate],
) -> Result<usize, Box<dyn std::error::Error>> {
    if batch.is_empty() {
        return Ok(0);
    }

    // Single pair — use simple prompt
    if batch.len() == 1 {
        let c = &batch[0];
        let prompt = format!(
            "Do these two statements contradict each other? Answer only 'yes' or 'no'.\n\n\
             Statement A: {}\n\nStatement B: {}",
            c.content_a, c.content_b
        );

        return match client.complete(&prompt).await {
            Ok(response) => {
                if response.trim().to_lowercase().starts_with("yes") {
                    resolve_contradiction(db, c)
                } else {
                    Ok(0)
                }
            }
            Err(e) => {
                tracing::warn!("Claude API error during contradiction check: {}", e);
                Ok(0)
            }
        };
    }

    // Multi-pair batch — ask Claude to identify which pairs contradict
    let mut prompt = String::from(
        "For each numbered pair below, determine if the two statements contradict each other. \
         Respond with ONLY a JSON array of the pair numbers that contradict. \
         Example: [1, 3] means pairs 1 and 3 contradict. [] means none contradict.\n\n",
    );

    for (i, c) in batch.iter().enumerate() {
        prompt.push_str(&format!(
            "Pair {}:\n  A: {}\n  B: {}\n\n",
            i + 1,
            c.content_a,
            c.content_b
        ));
    }

    prompt.push_str("Respond with ONLY the JSON array, no explanation.");

    match client.complete(&prompt).await {
        Ok(response) => {
            let response = response.trim();
            // Parse the JSON array of contradicting pair numbers
            let indices: Vec<usize> = serde_json::from_str(response).unwrap_or_default();
            let mut resolved = 0;

            for idx in indices {
                if idx >= 1 && idx <= batch.len() {
                    match resolve_contradiction(db, &batch[idx - 1]) {
                        Ok(n) => resolved += n,
                        Err(e) => tracing::warn!("Failed to resolve contradiction: {}", e),
                    }
                }
            }

            Ok(resolved)
        }
        Err(e) => {
            tracing::warn!("Claude API error during batch contradiction check: {}", e);
            Ok(0)
        }
    }
}

/// Resolve a contradiction by superseding the older fragment with the newer one.
fn resolve_contradiction(
    db: &LoreDb,
    candidate: &ContradictionCandidate,
) -> Result<usize, Box<dyn std::error::Error>> {
    let (old_id, new_id) = if candidate.created_a < candidate.created_b {
        (candidate.id_a, candidate.id_b)
    } else {
        (candidate.id_b, candidate.id_a)
    };

    db.supersede(old_id, new_id)?;
    Ok(1)
}

/// Phase 5: Decay edge weights and prune weak edges.
fn phase5_pruning(
    db: &LoreDb,
    _config: &ConsolidationConfig,
) -> Result<usize, Box<dyn std::error::Error>> {
    // Decay all associative edge weights by 5% per consolidation cycle
    let _ = db.storage().decay_edge_weights(EdgeKind::Associative, 0.95);

    // Prune edges that have decayed below threshold
    let pruned = db
        .storage()
        .delete_weak_edges(EdgeKind::Associative, 0.15)?;

    Ok(pruned)
}

/// Phase 6: Prune fragments with negligible relevance — true forgetting.
///
/// Rules:
/// - Never prune depth-0 roots (they just rank low instead)
/// - Fragments with relevance < 0.02 and no accesses and age > 60 days: deleted
/// - Fragments with relevance < 0.01 and age > 90 days: deleted regardless
/// - Before deleting, reparent any children to the fragment's parent
fn phase6_fragment_pruning(
    db: &LoreDb,
    _config: &ConsolidationConfig,
    now: i64,
) -> Result<usize, Box<dyn std::error::Error>> {
    let day = 86400i64;
    let mut pruned = 0;

    // Tier 1: Very low relevance, never accessed, >60 days old
    let stale = db
        .storage()
        .get_low_relevance_fragments(0.02, 60 * day, now)?;
    for frag in &stale {
        if frag.access_count == 0 {
            reparent_and_prune(db, frag)?;
            pruned += 1;
        }
    }

    // Tier 2: Negligible relevance, >90 days old (regardless of access)
    let very_stale = db
        .storage()
        .get_low_relevance_fragments(0.01, 90 * day, now)?;
    for frag in &very_stale {
        reparent_and_prune(db, frag)?;
        pruned += 1;
    }

    Ok(pruned)
}

/// Reparent a fragment's children to its parent before pruning.
fn reparent_and_prune(db: &LoreDb, frag: &Fragment) -> Result<(), Box<dyn std::error::Error>> {
    let children = db.children(frag.id);
    if !children.is_empty() {
        // Find this fragment's parent to reparent children to
        if let Some(parent) = db.parent(frag.id) {
            for child in &children {
                db.storage()
                    .delete_edge_between(frag.id, child.id, EdgeKind::Hierarchical)?;
                db.link(parent.id, child.id, EdgeKind::Hierarchical, 1.0)?;
            }
        }
        // If no parent, children become orphaned — they'll get pruned later if irrelevant
    }

    db.prune(frag.id)?;
    tracing::debug!(
        "Pruned forgotten fragment: {} (relevance={:.3})",
        &frag.content[..frag.content.len().min(60)],
        frag.relevance_score
    );
    Ok(())
}
