use crate::edge::{Edge, EdgeId, EdgeKind};
use crate::embedding::{cosine_similarity, Embedder};
use crate::fragment::{now_unix, Fragment, FragmentId, ScoredFragment, Tree};
use crate::relevance::{
    compute_relevance, ACTIVATION_SPREAD_FACTOR, MIN_RELEVANCE_THRESHOLD, SEMANTIC_WEIGHT,
};
use crate::storage::Storage;

/// The main query engine for the lore knowledge graph.
pub struct LoreDb {
    storage: Storage,
    embedder: Option<Embedder>,
}

impl LoreDb {
    /// Create a new LoreDb with embedding support.
    pub fn new(storage: Storage) -> Self {
        let embedder = match Embedder::new() {
            Ok(e) => Some(e),
            Err(err) => {
                tracing::warn!("Failed to initialize embedder: {err}. Semantic search disabled.");
                None
            }
        };
        Self { storage, embedder }
    }

    /// Create a new LoreDb without embedding support (for testing or read-only).
    pub fn new_without_embeddings(storage: Storage) -> Self {
        Self {
            storage,
            embedder: None,
        }
    }

    /// Get a reference to the underlying storage.
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    /// Get a reference to the embedder (if available).
    pub fn embedder(&self) -> Option<&Embedder> {
        self.embedder.as_ref()
    }

    /// Search by topic string across all depths.
    /// Returns the best match per tree (deduplicated) with breadcrumbs.
    /// Accessing fragments reinforces them (reconsolidation on recall).
    pub fn query(&self, topic: &str, limit: usize) -> Vec<ScoredFragment> {
        let query_embedding = match self.embed_text(topic) {
            Some(e) => e,
            None => return self.query_text_fallback(topic, limit),
        };

        // Get ALL fragments with embeddings (no depth filter)
        let fragments = match self.storage.get_fragments_with_embeddings(None) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };

        // Score by blended semantic similarity + relevance
        let mut scored: Vec<(Fragment, f32)> = fragments
            .into_iter()
            .filter(|f| !f.embedding.is_empty() && f.relevance_score > MIN_RELEVANCE_THRESHOLD)
            .map(|f| {
                let semantic = cosine_similarity(&query_embedding, &f.embedding);
                let score =
                    SEMANTIC_WEIGHT * semantic + (1.0 - SEMANTIC_WEIGHT) * f.relevance_score;
                (f, score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Dedup by tree: keep only the best-scoring fragment per root
        let mut seen_roots = std::collections::HashSet::new();
        let mut deduped = Vec::new();

        for (frag, score) in scored {
            let root_id = self.find_root(frag.id);
            if seen_roots.contains(&root_id) {
                continue;
            }
            seen_roots.insert(root_id);

            let breadcrumb = self.build_breadcrumb(frag.id);
            deduped.push(ScoredFragment {
                fragment: frag,
                score,
                breadcrumb,
            });

            if deduped.len() >= limit {
                break;
            }
        }

        // Reinforce accessed fragments
        for sf in &deduped {
            self.reinforce_on_access(sf.fragment.id);
        }

        deduped
    }

    /// Get children of a specific node (walk down the tree).
    pub fn children(&self, id: FragmentId) -> Vec<Fragment> {
        self.storage.get_children(id).unwrap_or_default()
    }

    /// Get parent of a node (walk up the tree).
    pub fn parent(&self, id: FragmentId) -> Option<Fragment> {
        self.storage.get_parent(id).unwrap_or(None)
    }

    /// Return full subtree rooted at a node, up to max_depth levels deep.
    pub fn subtree(&self, id: FragmentId, max_depth: u32) -> Option<Tree> {
        let fragment = self.storage.get_fragment(id).ok()??;
        Some(self.build_tree(fragment, max_depth))
    }

    /// Explore a topic: find the best matching L0 root nodes, return their subtrees.
    pub fn explore(&self, topic: &str, max_depth: u32, limit: usize) -> Vec<Tree> {
        // Find matching L0 roots
        let top_roots = self.query(topic, limit);

        top_roots
            .into_iter()
            .filter_map(|sf| self.subtree(sf.fragment.id, max_depth))
            .collect()
    }

    /// Pure semantic search across all fragments, blended with relevance.
    pub fn search_semantic(&self, embedding: &[f32], top_k: usize) -> Vec<ScoredFragment> {
        let fragments = match self.storage.get_fragments_with_embeddings(None) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };

        let mut scored: Vec<ScoredFragment> = fragments
            .into_iter()
            .filter(|f| !f.embedding.is_empty() && f.relevance_score > MIN_RELEVANCE_THRESHOLD)
            .map(|f| {
                let semantic = cosine_similarity(embedding, &f.embedding);
                let score =
                    SEMANTIC_WEIGHT * semantic + (1.0 - SEMANTIC_WEIGHT) * f.relevance_score;
                ScoredFragment {
                    fragment: f,
                    score,
                    breadcrumb: Vec::new(),
                }
            })
            .collect();

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(top_k);
        scored
    }

    /// List all root-level fragments (L0 nodes), sorted by relevance (most relevant first).
    /// Optionally filter by a keyword query (matched against content).
    pub fn list_roots(&self, filter: Option<&str>) -> Vec<Fragment> {
        let mut roots = self.storage.get_fragments_at_depth(0).unwrap_or_default();

        // Apply keyword filter if provided
        if let Some(query) = filter {
            let query_lower = query.to_lowercase();
            roots.retain(|t| t.content.to_lowercase().contains(&query_lower));
        }

        roots.sort_by(|a, b| {
            b.relevance_score
                .partial_cmp(&a.relevance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        roots
    }

    /// Insert a fragment, optionally generate its embedding, and connect it to parent.
    pub fn insert(
        &self,
        mut fragment: Fragment,
        parent: Option<FragmentId>,
    ) -> Result<FragmentId, Box<dyn std::error::Error>> {
        // Generate embedding if we have an embedder and the fragment doesn't have one
        if fragment.embedding.is_empty() {
            if let Some(embedding) = self.embed_text(&fragment.content) {
                fragment.embedding = embedding;
            }
        }

        let id = fragment.id;
        self.storage.insert_fragment(&fragment)?;

        // Create hierarchical edge to parent
        if let Some(parent_id) = parent {
            let edge = Edge {
                id: EdgeId::new(),
                source: parent_id,
                target: id,
                kind: EdgeKind::Hierarchical,
                weight: 1.0,
                content: None,
                created_at: now_unix(),
            };
            self.storage.insert_edge(&edge)?;
        }

        Ok(id)
    }

    /// Update a fragment's content, auto-embedding the new content.
    pub fn update(
        &self,
        id: FragmentId,
        new_content: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let embedding = self.embed_text(new_content);
        self.storage
            .update_fragment_content(id, new_content, embedding.as_deref())?;
        Ok(())
    }

    /// Create an edge between two fragments.
    pub fn link(
        &self,
        source: FragmentId,
        target: FragmentId,
        kind: EdgeKind,
        weight: f32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.link_with_content(source, target, kind, weight, None)
    }

    /// Create an edge with a relationship description.
    pub fn link_with_content(
        &self,
        source: FragmentId,
        target: FragmentId,
        kind: EdgeKind,
        weight: f32,
        content: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let edge = Edge {
            id: EdgeId::new(),
            source,
            target,
            kind,
            weight,
            content,
            created_at: now_unix(),
        };
        self.storage.insert_edge(&edge)?;
        Ok(())
    }

    /// Mark a fragment as superseded by another.
    pub fn supersede(
        &self,
        old: FragmentId,
        new: FragmentId,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.storage.supersede_fragment(old, new)?;
        // Create supersedes edge
        self.link(new, old, EdgeKind::Supersedes, 1.0)?;
        Ok(())
    }

    /// Delete a fragment and its edges.
    pub fn prune(&self, id: FragmentId) -> Result<(), Box<dyn std::error::Error>> {
        self.storage.delete_fragment(id)?;
        Ok(())
    }

    /// Get associations for a fragment.
    pub fn associations(&self, id: FragmentId) -> Vec<Fragment> {
        self.storage.get_associations(id).unwrap_or_default()
    }

    /// Find the most similar L0 root by pre-computed embedding, returning its ID and similarity.
    /// Returns None if no non-superseded roots with embeddings exist.
    pub fn find_best_root_by_embedding(&self, embedding: &[f32]) -> Option<(FragmentId, f32)> {
        let roots = self.storage.get_fragments_with_embeddings(Some(0)).ok()?;
        roots
            .iter()
            .filter(|r| !r.embedding.is_empty() && r.superseded_by.is_none())
            .map(|r| (r.id, cosine_similarity(embedding, &r.embedding)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// Find the maximum cosine similarity between content and existing L0 roots.
    /// Returns None only if the embedder is unavailable.
    /// Returns Some(0.0) if no roots exist (content is maximally novel).
    pub fn max_root_similarity(&self, content: &str) -> Option<f32> {
        let embedding = self.embed_text(content)?;
        let roots = self.storage.get_fragments_with_embeddings(Some(0)).ok()?;

        let max_sim = roots
            .iter()
            .filter(|r| !r.embedding.is_empty() && r.superseded_by.is_none())
            .map(|r| cosine_similarity(&embedding, &r.embedding))
            .reduce(f32::max);

        Some(max_sim.unwrap_or(0.0))
    }

    /// Find the best parent root (L0) for a fragment by embedding similarity.
    /// Returns the parent FragmentId if a sufficiently similar root exists (cosine > threshold).
    pub fn find_best_parent(&self, content: &str, threshold: f32) -> Option<FragmentId> {
        let query_embedding = self.embed_text(content)?;

        let roots = self.storage.get_fragments_with_embeddings(Some(0)).ok()?;

        let mut best: Option<(FragmentId, f32)> = None;
        for root in &roots {
            if root.embedding.is_empty() || root.superseded_by.is_some() {
                continue;
            }
            let sim = cosine_similarity(&query_embedding, &root.embedding);
            if sim > threshold && (best.is_none() || sim > best.unwrap().1) {
                best = Some((root.id, sim));
            }
        }

        best.map(|(id, _)| id)
    }

    // ──── Internal helpers ────

    /// Reinforce a fragment on access: update relevance score and spread activation
    /// to neighbors. This is the reconsolidation-on-recall mechanism.
    pub fn reinforce_on_access(&self, id: FragmentId) {
        let now = now_unix();

        // Load the fragment to compute new relevance
        if let Ok(Some(frag)) = self.storage.get_fragment(id) {
            let new_access_count = frag.access_count + 1;
            let new_relevance = compute_relevance(
                frag.importance,
                new_access_count,
                frag.decay_rate,
                now, // last_reinforced becomes now
                now,
            );
            let _ = self.storage.reinforce_fragment(id, now, new_relevance);

            // Spreading activation: boost immediate neighbors
            self.spread_activation(id, now);
        }
    }

    /// Spread a small activation boost to connected fragments.
    /// Models the brain's associative priming: accessing one memory
    /// slightly strengthens related memories.
    fn spread_activation(&self, id: FragmentId, now: i64) {
        if let Ok(edges) = self.storage.get_edges_for(id) {
            for edge in edges {
                let neighbor_id = if edge.source == id {
                    edge.target
                } else {
                    edge.source
                };
                let boost = ACTIVATION_SPREAD_FACTOR * edge.weight.min(1.0);
                let _ = self.storage.boost_relevance(neighbor_id, boost, now);
            }
        }
    }

    /// Generate an embedding for text, returning None if no embedder is available.
    pub fn embed_text(&self, text: &str) -> Option<Vec<f32>> {
        self.embedder.as_ref().and_then(|e| e.embed(text).ok())
    }

    /// Build a tree from a root fragment, recursively loading children up to max_depth.
    fn build_tree(&self, fragment: Fragment, remaining_depth: u32) -> Tree {
        let children = if remaining_depth > 0 {
            self.children(fragment.id)
                .into_iter()
                .map(|child| self.build_tree(child, remaining_depth - 1))
                .collect()
        } else {
            Vec::new()
        };
        Tree { fragment, children }
    }

    /// Walk up the tree to find the root fragment ID.
    fn find_root(&self, id: FragmentId) -> FragmentId {
        let mut current = id;
        for _ in 0..20 {
            // safety limit
            match self.parent(current) {
                Some(p) => current = p.id,
                None => break,
            }
        }
        current
    }

    /// Build breadcrumb: collect ancestor content from root down to (but not including) this fragment.
    fn build_breadcrumb(&self, id: FragmentId) -> Vec<String> {
        let mut ancestors = Vec::new();
        let mut current = id;
        for _ in 0..20 {
            match self.parent(current) {
                Some(p) => {
                    ancestors.push(p.content.clone());
                    current = p.id;
                }
                None => break,
            }
        }
        ancestors.reverse();
        ancestors
    }

    /// Text-based fallback query when embeddings are not available.
    fn query_text_fallback(&self, topic: &str, limit: usize) -> Vec<ScoredFragment> {
        // Search all depths
        let mut all_fragments = Vec::new();
        for depth in 0..10 {
            let frags = self
                .storage
                .get_fragments_at_depth(depth)
                .unwrap_or_default();
            if frags.is_empty() {
                break;
            }
            all_fragments.extend(frags);
        }

        let topic_lower = topic.to_lowercase();

        let mut scored: Vec<(Fragment, f32)> = all_fragments
            .into_iter()
            .filter(|f| f.relevance_score > MIN_RELEVANCE_THRESHOLD)
            .filter_map(|f| {
                let content_lower = f.content.to_lowercase();

                let text_score = if content_lower.contains(&topic_lower) {
                    0.8
                } else {
                    let words: Vec<&str> = topic_lower.split_whitespace().collect();
                    let matches = words.iter().filter(|w| content_lower.contains(*w)).count();
                    if matches > 0 {
                        0.3 + (0.4 * matches as f32 / words.len() as f32)
                    } else {
                        return None;
                    }
                };

                let score =
                    SEMANTIC_WEIGHT * text_score + (1.0 - SEMANTIC_WEIGHT) * f.relevance_score;
                Some((f, score))
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Dedup by tree
        let mut seen_roots = std::collections::HashSet::new();
        let mut deduped = Vec::new();

        for (frag, score) in scored {
            let root_id = self.find_root(frag.id);
            if seen_roots.contains(&root_id) {
                continue;
            }
            seen_roots.insert(root_id);

            let breadcrumb = self.build_breadcrumb(frag.id);
            deduped.push(ScoredFragment {
                fragment: frag,
                score,
                breadcrumb,
            });

            if deduped.len() >= limit {
                break;
            }
        }

        for sf in &deduped {
            self.reinforce_on_access(sf.fragment.id);
        }

        deduped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_db() -> LoreDb {
        let storage = Storage::open_memory().unwrap();
        // Use without embeddings for unit tests (fast, no model download)
        let db = LoreDb::new_without_embeddings(storage);

        // Create a small knowledge hierarchy
        let mut topic = Fragment::new("Rust programming language".to_string(), 0);
        topic.embedding = vec![0.1; 384];
        db.storage().insert_fragment(&topic).unwrap();

        let mut concept = Fragment::new("Rust async programming with tokio".to_string(), 1);
        concept.embedding = vec![0.2; 384];
        db.storage().insert_fragment(&concept).unwrap();

        let edge = Edge {
            id: EdgeId::new(),
            source: topic.id,
            target: concept.id,
            kind: EdgeKind::Hierarchical,
            weight: 1.0,
            content: None,
            created_at: now_unix(),
        };
        db.storage().insert_edge(&edge).unwrap();

        let mut fact = Fragment::new("Tokio uses a work-stealing scheduler".to_string(), 2);
        fact.embedding = vec![0.3; 384];
        db.storage().insert_fragment(&fact).unwrap();

        let edge2 = Edge {
            id: EdgeId::new(),
            source: concept.id,
            target: fact.id,
            kind: EdgeKind::Hierarchical,
            weight: 1.0,
            content: None,
            created_at: now_unix(),
        };
        db.storage().insert_edge(&edge2).unwrap();

        db
    }

    #[test]
    fn test_list_topics() {
        let db = make_test_db();
        let topics = db.list_roots(None);
        assert_eq!(topics.len(), 1);
        assert_eq!(topics[0].content, "Rust programming language");
    }

    #[test]
    fn test_children_and_parent() {
        let db = make_test_db();
        let topics = db.list_roots(None);
        let topic = &topics[0];

        let children = db.children(topic.id);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].content, "Rust async programming with tokio");

        let parent = db.parent(children[0].id).unwrap();
        assert_eq!(parent.id, topic.id);
    }

    #[test]
    fn test_subtree() {
        let db = make_test_db();
        let topics = db.list_roots(None);
        let tree = db.subtree(topics[0].id, 3).unwrap();

        assert_eq!(tree.fragment.content, "Rust programming language");
        assert_eq!(tree.children.len(), 1);
        assert_eq!(
            tree.children[0].fragment.content,
            "Rust async programming with tokio"
        );
        assert_eq!(tree.children[0].children.len(), 1);
        assert_eq!(
            tree.children[0].children[0].fragment.content,
            "Tokio uses a work-stealing scheduler"
        );
    }

    #[test]
    fn test_text_fallback_query() {
        let db = make_test_db();
        let results = db.query("rust", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fragment.content, "Rust programming language");
    }

    #[test]
    fn test_supersede() {
        let db = make_test_db();

        let old = Fragment::new("Old fact".to_string(), 2);
        db.storage().insert_fragment(&old).unwrap();

        let new = Fragment::new("New fact".to_string(), 2);
        db.storage().insert_fragment(&new).unwrap();

        db.supersede(old.id, new.id).unwrap();

        let loaded = db.storage().get_fragment(old.id).unwrap().unwrap();
        assert_eq!(loaded.superseded_by, Some(new.id));
    }

    #[test]
    fn test_prune() {
        let db = make_test_db();
        let topics = db.list_roots(None);
        assert_eq!(topics.len(), 1);

        // Get the topic's children first
        let children = db.children(topics[0].id);

        // Prune a leaf
        let grandchildren = db.children(children[0].id);
        db.prune(grandchildren[0].id).unwrap();

        let remaining = db.children(children[0].id);
        assert_eq!(remaining.len(), 0);
    }

    #[test]
    fn test_update_fragment() {
        let db = make_test_db();
        let topics = db.list_roots(None);
        let topic_id = topics[0].id;

        db.update(topic_id, "Updated Rust content").unwrap();

        let loaded = db.storage().get_fragment(topic_id).unwrap().unwrap();
        assert_eq!(loaded.content, "Updated Rust content");
    }

    #[test]
    fn test_max_root_similarity_without_embedder() {
        let db = make_test_db();
        // Without embedder, should return None
        assert!(db.max_root_similarity("rust programming").is_none());
    }

    #[test]
    fn test_find_best_root_by_embedding() {
        let db = make_test_db();
        // The test DB has a root with embedding vec![0.1; 384]

        // Identical embedding should find the root with sim ~1.0
        let result = db.find_best_root_by_embedding(&vec![0.1; 384]);
        assert!(result.is_some());
        let (id, sim) = result.unwrap();
        assert!(
            sim > 0.99,
            "Identical embedding sim should be ~1.0, got {}",
            sim
        );

        let root = db.storage().get_fragment(id).unwrap().unwrap();
        assert_eq!(root.content, "Rust programming language");

        // Empty DB should return None
        let empty_storage = Storage::open_memory().unwrap();
        let empty_db = LoreDb::new_without_embeddings(empty_storage);
        assert!(empty_db
            .find_best_root_by_embedding(&vec![0.1; 384])
            .is_none());
    }

    #[test]
    fn test_insert_with_parent() {
        let db = make_test_db();
        let topics = db.list_roots(None);

        let new_concept = Fragment::new("Rust ownership system".to_string(), 1);
        let new_id = db.insert(new_concept, Some(topics[0].id)).unwrap();

        let children = db.children(topics[0].id);
        assert_eq!(children.len(), 2);
        assert!(children.iter().any(|c| c.id == new_id));
    }
}
