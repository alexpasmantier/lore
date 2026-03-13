//! Behavioral tests for the brain-inspired memory system.
//!
//! These tests verify the *system-level* expectations of how memory should work,
//! not individual function correctness. Each test describes a behavioral property
//! that mirrors how biological memory operates.

use lore_db::edge::{Edge, EdgeId, EdgeKind};
use lore_db::fragment::{now_unix, Fragment, FragmentId};
use lore_db::relevance::{compute_relevance, decay_rate_for_importance};
use lore_db::storage::Storage;
use lore_db::LoreDb;

// ──── Test Helpers ────

/// Create an in-memory database for testing (no embedder — fast, no model download).
fn test_db() -> LoreDb {
    let storage = Storage::open_memory().unwrap();
    LoreDb::new_without_embeddings(storage)
}

/// Insert a fragment with a specific embedding, importance, and age.
#[allow(clippy::too_many_arguments)]
fn insert_fragment(
    db: &LoreDb,
    content: &str,
    depth: u32,
    embedding: Vec<f32>,
    importance: f32,
    age_days: i64,
    access_count: u32,
    parent: Option<FragmentId>,
) -> FragmentId {
    let now = now_unix();
    let created = now - age_days * 86400;
    let mut frag = Fragment::new_with_importance(content.to_string(), depth, importance);
    frag.embedding = embedding;
    frag.created_at = created;
    frag.last_accessed = created;
    frag.last_reinforced = created;
    frag.access_count = access_count;
    // Recompute relevance based on actual age
    frag.relevance_score = compute_relevance(
        frag.importance,
        frag.access_count,
        frag.decay_rate,
        frag.last_reinforced,
        now,
    );
    db.storage().insert_fragment(&frag).unwrap();

    if let Some(parent_id) = parent {
        let edge = Edge {
            id: EdgeId::new(),
            source: parent_id,
            target: frag.id,
            kind: EdgeKind::Hierarchical,
            weight: 1.0,
            created_at: now,
        };
        db.storage().insert_edge(&edge).unwrap();
    }

    frag.id
}

/// Link two fragments with an edge.
fn link(db: &LoreDb, source: FragmentId, target: FragmentId, kind: EdgeKind, weight: f32) {
    db.link(source, target, kind, weight).unwrap();
}

// ════════════════════════════════════════════════════════════════
// 1. DECAY: Memories fade over time without access
// ════════════════════════════════════════════════════════════════

#[test]
fn memories_decay_with_age() {
    // A memory that hasn't been accessed should have lower relevance
    // as time passes — modeling the Ebbinghaus forgetting curve.
    let day = 86400i64;
    let now = 1_000_000_000;
    let importance = 0.5;
    let decay_rate = decay_rate_for_importance(importance);

    let r_fresh = compute_relevance(importance, 0, decay_rate, now, now);
    let r_1week = compute_relevance(importance, 0, decay_rate, now - 7 * day, now);
    let r_1month = compute_relevance(importance, 0, decay_rate, now - 30 * day, now);
    let r_3months = compute_relevance(importance, 0, decay_rate, now - 90 * day, now);

    assert!(r_fresh > r_1week, "1 week old should be weaker than fresh");
    assert!(
        r_1week > r_1month,
        "1 month old should be weaker than 1 week"
    );
    assert!(
        r_1month > r_3months,
        "3 months old should be weaker than 1 month"
    );
}

#[test]
fn decay_is_exponential_not_linear() {
    // The forgetting curve should be exponential — rapid initial decay
    // that slows down, not a steady linear decline.
    let now = 1_000_000_000;
    let day = 86400i64;

    let r0 = compute_relevance(0.5, 0, 0.035, now, now);
    let r10 = compute_relevance(0.5, 0, 0.035, now - 10 * day, now);
    let r20 = compute_relevance(0.5, 0, 0.035, now - 20 * day, now);
    let r30 = compute_relevance(0.5, 0, 0.035, now - 30 * day, now);

    let drop_0_to_10 = r0 - r10;
    let drop_10_to_20 = r10 - r20;
    let drop_20_to_30 = r20 - r30;

    // Each successive interval should produce a smaller drop
    assert!(
        drop_0_to_10 > drop_10_to_20,
        "First 10 days should drop more than days 10-20"
    );
    assert!(
        drop_10_to_20 > drop_20_to_30,
        "Days 10-20 should drop more than days 20-30"
    );
}

#[test]
fn batch_decay_recomputation_updates_all_fragments() {
    let db = test_db();
    let now = now_unix();

    // Insert fragments with different ages
    let id_fresh = insert_fragment(&db, "Fresh", 0, vec![], 0.5, 0, 0, None);
    let id_old = insert_fragment(&db, "Old", 0, vec![], 0.5, 30, 0, None);
    let id_ancient = insert_fragment(&db, "Ancient", 0, vec![], 0.5, 90, 0, None);

    // Run batch recomputation
    let updated = db.storage().recompute_all_relevance(now).unwrap();
    assert_eq!(updated, 3);

    let fresh = db.storage().get_fragment(id_fresh).unwrap().unwrap();
    let old = db.storage().get_fragment(id_old).unwrap().unwrap();
    let ancient = db.storage().get_fragment(id_ancient).unwrap().unwrap();

    assert!(
        fresh.relevance_score > old.relevance_score,
        "Fresh ({}) should outrank old ({})",
        fresh.relevance_score,
        old.relevance_score
    );
    assert!(
        old.relevance_score > ancient.relevance_score,
        "Old ({}) should outrank ancient ({})",
        old.relevance_score,
        ancient.relevance_score
    );
}

// ════════════════════════════════════════════════════════════════
// 2. REINFORCEMENT: Accessing a memory strengthens it
// ════════════════════════════════════════════════════════════════

#[test]
fn accessing_a_memory_increases_relevance() {
    let db = test_db();

    // Insert a 30-day old fragment
    let id = insert_fragment(&db, "Some knowledge", 0, vec![], 0.5, 30, 0, None);
    let before = db.storage().get_fragment(id).unwrap().unwrap();

    // Reinforce it (simulating an access)
    let now = now_unix();
    let new_relevance = compute_relevance(
        before.importance,
        before.access_count + 1,
        before.decay_rate,
        now,
        now,
    );
    db.storage()
        .reinforce_fragment(id, now, new_relevance)
        .unwrap();

    let after = db.storage().get_fragment(id).unwrap().unwrap();

    assert!(
        after.relevance_score > before.relevance_score,
        "Access should increase relevance: {} -> {}",
        before.relevance_score,
        after.relevance_score
    );
    assert!(after.access_count > before.access_count);
    assert!(after.last_reinforced >= now);
}

#[test]
fn repeated_access_shows_diminishing_returns() {
    // Like the spacing effect in learning — each additional retrieval
    // strengthens the memory, but with diminishing marginal returns
    // per access.
    let now = 1_000_000_000;

    let r_0 = compute_relevance(0.5, 0, 0.035, now, now);
    let r_1 = compute_relevance(0.5, 1, 0.035, now, now);
    let r_10 = compute_relevance(0.5, 10, 0.035, now, now);
    let r_100 = compute_relevance(0.5, 100, 0.035, now, now);

    // Marginal gain per access should decrease
    let marginal_at_1 = r_1 - r_0; // gain from the 1st access
    let marginal_at_10 = r_10 - compute_relevance(0.5, 9, 0.035, now, now); // gain from the 10th access
    let marginal_at_100 = r_100 - compute_relevance(0.5, 99, 0.035, now, now); // gain from the 100th access

    assert!(marginal_at_1 > 0.0, "First access should strengthen");
    assert!(r_100 > r_10, "More accesses should still strengthen");
    assert!(
        marginal_at_1 > marginal_at_10,
        "1st access gain ({:.6}) should exceed 10th ({:.6})",
        marginal_at_1,
        marginal_at_10,
    );
    assert!(
        marginal_at_10 > marginal_at_100,
        "10th access gain ({:.6}) should exceed 100th ({:.6})",
        marginal_at_10,
        marginal_at_100,
    );
}

#[test]
fn reinforcement_resets_decay_timer() {
    let db = test_db();
    let now = now_unix();

    // Insert a 60-day old fragment
    let id = insert_fragment(&db, "Old memory", 0, vec![], 0.5, 60, 0, None);
    let before = db.storage().get_fragment(id).unwrap().unwrap();

    // Reinforce it now
    let new_relevance = compute_relevance(0.5, 1, before.decay_rate, now, now);
    db.storage()
        .reinforce_fragment(id, now, new_relevance)
        .unwrap();

    let after = db.storage().get_fragment(id).unwrap().unwrap();

    // The reinforced fragment should have last_reinforced == now,
    // so future decay starts from this moment, not from 60 days ago
    assert_eq!(after.last_reinforced, now);
    assert!(
        after.relevance_score > before.relevance_score,
        "Reinforcement should reset decay: {} -> {}",
        before.relevance_score,
        after.relevance_score
    );
}

// ════════════════════════════════════════════════════════════════
// 3. SPREADING ACTIVATION: Accessing one memory boosts neighbors
// ════════════════════════════════════════════════════════════════

#[test]
fn accessing_a_fragment_boosts_connected_neighbors() {
    let db = test_db();
    let now = now_unix();

    // Create a topic with two children and an association
    let topic_id = insert_fragment(&db, "Topic", 0, vec![], 0.5, 10, 0, None);
    let child_id = insert_fragment(&db, "Child", 1, vec![], 0.5, 10, 0, Some(topic_id));
    let assoc_id = insert_fragment(&db, "Associated", 0, vec![], 0.5, 10, 0, None);
    link(&db, topic_id, assoc_id, EdgeKind::Associative, 0.8);

    let child_before = db.storage().get_fragment(child_id).unwrap().unwrap();
    let assoc_before = db.storage().get_fragment(assoc_id).unwrap().unwrap();

    // Boost neighbors of the topic (simulating spreading activation)
    let boost = 0.1;
    db.storage()
        .boost_relevance(child_id, boost * 1.0, now)
        .unwrap();
    db.storage()
        .boost_relevance(assoc_id, boost * 0.8, now)
        .unwrap();

    let child_after = db.storage().get_fragment(child_id).unwrap().unwrap();
    let assoc_after = db.storage().get_fragment(assoc_id).unwrap().unwrap();

    assert!(
        child_after.relevance_score > child_before.relevance_score,
        "Child should get boosted"
    );
    assert!(
        assoc_after.relevance_score > assoc_before.relevance_score,
        "Association should get boosted"
    );
}

#[test]
fn boost_is_capped_at_one() {
    let db = test_db();
    let now = now_unix();

    // Fresh fragment starts with relevance_score = 1.0
    let id = insert_fragment(&db, "Fresh", 0, vec![], 1.0, 0, 10, None);

    // Try to boost past 1.0
    db.storage().boost_relevance(id, 0.5, now).unwrap();

    let after = db.storage().get_fragment(id).unwrap().unwrap();
    assert!(
        after.relevance_score <= 1.0,
        "Relevance should be capped at 1.0, got {}",
        after.relevance_score
    );
}

// ════════════════════════════════════════════════════════════════
// 4. IMPORTANCE: High-salience memories are more durable
// ════════════════════════════════════════════════════════════════

#[test]
fn important_memories_decay_slower() {
    let now = 1_000_000_000;
    let day = 86400i64;

    let high = decay_rate_for_importance(0.9);
    let low = decay_rate_for_importance(0.2);

    let r_high = compute_relevance(0.9, 0, high, now - 30 * day, now);
    let r_low = compute_relevance(0.2, 0, low, now - 30 * day, now);

    assert!(
        r_high > r_low,
        "Important memory ({}) should outrank unimportant ({}) after 30 days",
        r_high,
        r_low
    );
}

#[test]
fn important_memories_never_fully_vanish() {
    // The amygdala floor: high-importance memories have a minimum
    // relevance that persists even after very long periods.
    let now = 1_000_000_000;
    let day = 86400i64;

    // A year-old, never-accessed, high-importance memory
    let r = compute_relevance(1.0, 0, 0.01, now - 365 * day, now);
    assert!(
        r >= 0.25,
        "Critical memory should maintain a floor even after a year, got {}",
        r
    );

    // A year-old, never-accessed, low-importance memory
    let r_low = compute_relevance(0.1, 0, 0.07, now - 365 * day, now);
    assert!(
        r_low < 0.05,
        "Trivial memory should nearly vanish after a year, got {}",
        r_low
    );
}

#[test]
fn importance_determines_decay_rate() {
    let high = decay_rate_for_importance(1.0);
    let mid = decay_rate_for_importance(0.5);
    let low = decay_rate_for_importance(0.0);

    assert!(high < mid, "High importance should decay slower");
    assert!(mid < low, "Medium importance should decay slower than low");
    // Verify reasonable bounds
    assert!(high > 0.0, "Decay rate should be positive");
    assert!(low < 0.1, "Decay rate shouldn't be extreme");
}

#[test]
fn fragment_with_importance_gets_correct_decay_rate() {
    let frag = Fragment::new_with_importance("Important decision".to_string(), 0, 0.9);

    let expected_rate = decay_rate_for_importance(0.9);
    assert!(
        (frag.decay_rate - expected_rate).abs() < 1e-6,
        "new_with_importance should set decay_rate from importance"
    );
    assert!(
        (frag.importance - 0.9).abs() < 1e-6,
        "importance should be set correctly"
    );
}

// ════════════════════════════════════════════════════════════════
// 5. FORGETTING: Below-threshold memories become invisible
// ════════════════════════════════════════════════════════════════

#[test]
fn very_old_low_importance_fragments_are_candidates_for_pruning() {
    let db = test_db();
    let now = now_unix();

    // Insert fragments with various ages and importances
    let _fresh = insert_fragment(&db, "Fresh", 1, vec![], 0.5, 0, 0, None);
    let _old_important = insert_fragment(&db, "Old important", 1, vec![], 0.9, 90, 0, None);
    let old_trivial = insert_fragment(&db, "Old trivial", 1, vec![], 0.1, 90, 0, None);

    // Recompute relevance
    db.storage().recompute_all_relevance(now).unwrap();

    // Get low-relevance fragments (>60 days old, relevance < 0.05)
    let candidates = db
        .storage()
        .get_low_relevance_fragments(0.05, 60 * 86400, now)
        .unwrap();

    // Only the old trivial fragment should qualify
    assert!(
        candidates.iter().any(|f| f.id == old_trivial),
        "Old trivial fragment should be a pruning candidate"
    );
    assert!(
        !candidates.iter().any(|f| f.content == "Fresh"),
        "Fresh fragment should not be a candidate"
    );
}

#[test]
fn depth_zero_topics_are_never_pruning_candidates() {
    let db = test_db();
    let now = now_unix();

    // Insert an old, low-importance depth-0 root
    let _root = insert_fragment(&db, "Old topic", 0, vec![], 0.1, 120, 0, None);

    db.storage().recompute_all_relevance(now).unwrap();

    // get_low_relevance_fragments filters depth > 0
    let candidates = db
        .storage()
        .get_low_relevance_fragments(1.0, 0, now)
        .unwrap();
    assert!(
        candidates.iter().all(|f| f.depth > 0),
        "Depth-0 roots should never be pruning candidates"
    );
}

#[test]
fn superseded_fragments_are_excluded_from_pruning_candidates() {
    let db = test_db();
    let now = now_unix();

    let old_id = insert_fragment(&db, "Old version", 1, vec![], 0.5, 90, 0, None);
    let new_id = insert_fragment(&db, "New version", 1, vec![], 0.5, 0, 0, None);
    db.supersede(old_id, new_id).unwrap();

    let candidates = db
        .storage()
        .get_low_relevance_fragments(1.0, 0, now)
        .unwrap();
    assert!(
        !candidates.iter().any(|f| f.id == old_id),
        "Superseded fragments should be excluded from pruning"
    );
}

// ════════════════════════════════════════════════════════════════
// 6. BLENDED RANKING: Relevance modulates semantic search
// ════════════════════════════════════════════════════════════════

#[test]
fn relevance_modulates_text_query_ordering() {
    // When using the text fallback, results should still respect
    // the fragment ordering (list_roots sorts by relevance).
    let db = test_db();
    let now = now_unix();

    // Two topics that both match "rust"
    let _stale = insert_fragment(
        &db,
        "Rust is a systems language",
        0,
        vec![],
        0.2,
        90,
        0,
        None,
    );
    let _fresh = insert_fragment(&db, "Rust programming patterns", 0, vec![], 0.8, 1, 5, None);

    db.storage().recompute_all_relevance(now).unwrap();

    let roots = db.list_roots(None);
    assert_eq!(roots.len(), 2);
    assert!(
        roots[0].relevance_score >= roots[1].relevance_score,
        "Roots should be sorted by relevance: {} >= {}",
        roots[0].relevance_score,
        roots[1].relevance_score
    );
}

// ════════════════════════════════════════════════════════════════
// 7. EDGE BEHAVIOR: Connections strengthen and decay
// ════════════════════════════════════════════════════════════════

#[test]
fn edge_weights_decay_over_consolidation_cycles() {
    let db = test_db();

    let a = insert_fragment(&db, "A", 0, vec![], 0.5, 0, 0, None);
    let b = insert_fragment(&db, "B", 0, vec![], 0.5, 0, 0, None);
    link(&db, a, b, EdgeKind::Associative, 1.0);

    // Simulate 5 consolidation cycles of 5% decay each
    for _ in 0..5 {
        db.storage()
            .decay_edge_weights(EdgeKind::Associative, 0.95)
            .unwrap();
    }

    let edges = db.storage().get_edges_for(a).unwrap();
    let edge = edges
        .iter()
        .find(|e| e.kind == EdgeKind::Associative)
        .unwrap();

    // After 5 cycles of 5% decay: 1.0 * 0.95^5 ≈ 0.774
    assert!(
        (edge.weight - 0.7738).abs() < 0.01,
        "Edge should have decayed to ~0.774, got {}",
        edge.weight
    );
}

#[test]
fn weak_edges_are_pruned_at_threshold() {
    let db = test_db();

    let a = insert_fragment(&db, "A", 0, vec![], 0.5, 0, 0, None);
    let b = insert_fragment(&db, "B", 0, vec![], 0.5, 0, 0, None);
    let c = insert_fragment(&db, "C", 0, vec![], 0.5, 0, 0, None);

    link(&db, a, b, EdgeKind::Associative, 0.5); // Strong enough
    link(&db, a, c, EdgeKind::Associative, 0.1); // Below threshold

    let pruned = db
        .storage()
        .delete_weak_edges(EdgeKind::Associative, 0.15)
        .unwrap();
    assert_eq!(pruned, 1, "Should prune exactly the weak edge");

    let edges = db.storage().get_edges_for(a).unwrap();
    let assoc_edges: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Associative)
        .collect();
    assert_eq!(assoc_edges.len(), 1, "Only the strong edge should remain");
}

#[test]
fn hierarchical_edges_are_not_affected_by_associative_decay() {
    let db = test_db();

    let parent = insert_fragment(&db, "Parent", 0, vec![], 0.5, 0, 0, None);
    let _child = insert_fragment(&db, "Child", 1, vec![], 0.5, 0, 0, Some(parent));

    // Decay associative edges
    db.storage()
        .decay_edge_weights(EdgeKind::Associative, 0.5)
        .unwrap();

    // Hierarchical edges should be untouched
    let children = db.children(parent);
    assert_eq!(
        children.len(),
        1,
        "Hierarchical edges shouldn't be affected by associative decay"
    );
}

// ════════════════════════════════════════════════════════════════
// 8. TEMPORAL EDGES: Sequential knowledge is linked in time
// ════════════════════════════════════════════════════════════════

#[test]
fn temporal_edges_connect_sequential_siblings() {
    let db = test_db();
    let topic = insert_fragment(&db, "Topic", 0, vec![], 0.5, 0, 0, None);
    let child1 = insert_fragment(&db, "First", 1, vec![], 0.5, 0, 0, Some(topic));
    let child2 = insert_fragment(&db, "Second", 1, vec![], 0.5, 0, 0, Some(topic));
    let child3 = insert_fragment(&db, "Third", 1, vec![], 0.5, 0, 0, Some(topic));

    // Create temporal edges: 1→2→3
    link(&db, child1, child2, EdgeKind::Temporal, 1.0);
    link(&db, child2, child3, EdgeKind::Temporal, 1.0);

    // Verify temporal chain
    let edges1 = db.storage().get_edges_for(child1).unwrap();
    let temporal1: Vec<_> = edges1
        .iter()
        .filter(|e| e.kind == EdgeKind::Temporal && e.source == child1)
        .collect();
    assert_eq!(temporal1.len(), 1);
    assert_eq!(temporal1[0].target, child2);

    let edges2 = db.storage().get_edges_for(child2).unwrap();
    let temporal2: Vec<_> = edges2
        .iter()
        .filter(|e| e.kind == EdgeKind::Temporal && e.source == child2)
        .collect();
    assert_eq!(temporal2.len(), 1);
    assert_eq!(temporal2[0].target, child3);
}

// ════════════════════════════════════════════════════════════════
// 9. SCHEMA MIGRATION: Existing databases get new columns
// ════════════════════════════════════════════════════════════════

#[test]
fn new_fragments_have_correct_defaults() {
    let frag = Fragment::new("Test".to_string(), 0);
    assert!(
        (frag.importance - 0.5).abs() < 1e-6,
        "Default importance should be 0.5"
    );
    assert!(
        (frag.relevance_score - 1.0).abs() < 1e-6,
        "Default relevance should be 1.0"
    );
    assert!(
        (frag.decay_rate - 0.035).abs() < 1e-6,
        "Default decay_rate should be 0.035"
    );
    assert!(frag.last_reinforced > 0, "last_reinforced should be set");
}

#[test]
fn migration_v2_columns_are_readable() {
    // Verify that opening a fresh database gives us all V2 columns
    let storage = Storage::open_memory().unwrap();
    let db = LoreDb::new_without_embeddings(storage);

    let frag = Fragment::new_with_importance("Test".to_string(), 0, 0.9);
    db.storage().insert_fragment(&frag).unwrap();

    let loaded = db.storage().get_fragment(frag.id).unwrap().unwrap();
    assert!((loaded.importance - 0.9).abs() < 1e-6);
    assert!(loaded.relevance_score > 0.0);
    assert!(loaded.decay_rate > 0.0);
    assert!(loaded.last_reinforced > 0);
}

// ════════════════════════════════════════════════════════════════
// 10. RECONSOLIDATION: Full access cycle behavior
// ════════════════════════════════════════════════════════════════

#[test]
fn query_reinforces_returned_fragments() {
    // When we query and get results, those results should be
    // reinforced (reconsolidation on recall).
    let db = test_db();

    let mut frag = Fragment::new("Rust async patterns".to_string(), 0);
    frag.embedding = vec![0.1; 384];
    db.storage().insert_fragment(&frag).unwrap();
    let original_access = frag.access_count;

    // Query with text fallback (since we have no embedder)
    let results = db.query("rust", 0, 10);
    assert!(!results.is_empty());

    // The fragment in the DB should have been touched
    let after = db.storage().get_fragment(frag.id).unwrap().unwrap();
    assert!(
        after.access_count > original_access,
        "Query should increment access_count"
    );
}

// ════════════════════════════════════════════════════════════════
// 11. SUPERSESSION: Knowledge evolution
// ════════════════════════════════════════════════════════════════

#[test]
fn superseded_fragments_dont_appear_in_queries() {
    let db = test_db();

    let mut old = Fragment::new("Old info".to_string(), 0);
    old.embedding = vec![0.1; 384];
    db.storage().insert_fragment(&old).unwrap();

    let mut new = Fragment::new("Updated info".to_string(), 0);
    new.embedding = vec![0.1; 384];
    db.storage().insert_fragment(&new).unwrap();

    db.supersede(old.id, new.id).unwrap();

    let roots = db.list_roots(None);
    assert!(
        !roots.iter().any(|t| t.id == old.id),
        "Superseded fragment should not appear in list_roots"
    );
}

#[test]
fn superseded_fragments_dont_appear_in_children() {
    let db = test_db();

    let topic = insert_fragment(&db, "Topic", 0, vec![], 0.5, 0, 0, None);
    let old_child = insert_fragment(&db, "Old child", 1, vec![], 0.5, 0, 0, Some(topic));
    let new_child = insert_fragment(&db, "New child", 1, vec![], 0.5, 0, 0, Some(topic));

    db.supersede(old_child, new_child).unwrap();

    let children = db.children(topic);
    assert!(
        !children.iter().any(|c| c.id == old_child),
        "Superseded child should not appear in children list"
    );
    assert!(
        children.iter().any(|c| c.id == new_child),
        "New child should appear"
    );
}

// ════════════════════════════════════════════════════════════════
// 12. GRAPH INTEGRITY: Tree structure operations
// ════════════════════════════════════════════════════════════════

#[test]
fn pruning_a_node_removes_all_its_edges() {
    let db = test_db();

    let topic = insert_fragment(&db, "Topic", 0, vec![], 0.5, 0, 0, None);
    let child = insert_fragment(&db, "Child", 1, vec![], 0.5, 0, 0, Some(topic));
    let assoc = insert_fragment(&db, "Associated", 1, vec![], 0.5, 0, 0, None);
    link(&db, child, assoc, EdgeKind::Associative, 0.8);

    // Verify edges exist
    let edges_before = db.storage().get_edges_for(child).unwrap();
    assert!(
        edges_before.len() >= 2,
        "Should have hierarchical + associative edges"
    );

    // Prune the child
    db.prune(child).unwrap();

    // Verify all edges are gone
    let edges_after = db.storage().get_edges_for(child).unwrap();
    assert_eq!(edges_after.len(), 0, "All edges should be removed on prune");

    // Topic should have no children now
    let children = db.children(topic);
    assert_eq!(children.len(), 0);
}

#[test]
fn subtree_respects_max_depth() {
    let db = test_db();

    let l0 = insert_fragment(&db, "L0", 0, vec![], 0.5, 0, 0, None);
    let l1 = insert_fragment(&db, "L1", 1, vec![], 0.5, 0, 0, Some(l0));
    let l2 = insert_fragment(&db, "L2", 2, vec![], 0.5, 0, 0, Some(l1));
    let _l3 = insert_fragment(&db, "L3", 3, vec![], 0.5, 0, 0, Some(l2));

    let tree = db.subtree(l0, 2).unwrap();
    assert_eq!(tree.children.len(), 1); // L1
    assert_eq!(tree.children[0].children.len(), 1); // L2
    assert_eq!(tree.children[0].children[0].children.len(), 0); // L3 cut off
}

// ════════════════════════════════════════════════════════════════
// 13. END-TO-END: Complete lifecycle scenarios
// ════════════════════════════════════════════════════════════════

#[test]
fn lifecycle_fresh_memory_ages_and_can_be_rescued_by_access() {
    let db = test_db();
    let now = now_unix();
    let day = 86400i64;

    // Day 0: Insert a medium-importance memory
    let id = insert_fragment(
        &db,
        "Important architectural decision about async runtime",
        0,
        vec![],
        0.5,
        0,
        0,
        None,
    );

    // Simulate 30 days passing: recompute relevance as if 30 days later
    let future = now + 30 * day;
    db.storage().recompute_all_relevance(future).unwrap();
    let after_30d = db.storage().get_fragment(id).unwrap().unwrap();

    // Simulate 60 days passing
    let future = now + 60 * day;
    db.storage().recompute_all_relevance(future).unwrap();
    let after_60d = db.storage().get_fragment(id).unwrap().unwrap();

    assert!(
        after_60d.relevance_score < after_30d.relevance_score,
        "Should continue to decay"
    );

    // Now reinforce it (someone accessed it at day 60)
    let new_rel = compute_relevance(0.5, 1, after_60d.decay_rate, future, future);
    db.storage()
        .reinforce_fragment(id, future, new_rel)
        .unwrap();
    let after_rescue = db.storage().get_fragment(id).unwrap().unwrap();

    assert!(
        after_rescue.relevance_score > after_60d.relevance_score,
        "Reinforcement should rescue decayed memory: {} > {}",
        after_rescue.relevance_score,
        after_60d.relevance_score
    );
}

#[test]
fn lifecycle_important_vs_trivial_over_time() {
    let now = 1_000_000_000;
    let day = 86400i64;

    // Track both over 180 days
    let critical_rate = decay_rate_for_importance(0.9);
    let trivial_rate = decay_rate_for_importance(0.2);

    for days in [0, 7, 30, 60, 90, 180] {
        let t = now - days * day;
        let r_critical = compute_relevance(0.9, 0, critical_rate, t, now);
        let r_trivial = compute_relevance(0.2, 0, trivial_rate, t, now);

        assert!(
            r_critical > r_trivial,
            "At day {}: critical ({:.3}) should outrank trivial ({:.3})",
            days,
            r_critical,
            r_trivial
        );
    }

    // After 180 days, trivial should be nearly gone
    let r_trivial_180 = compute_relevance(0.2, 0, trivial_rate, now - 180 * day, now);
    assert!(
        r_trivial_180 < 0.08,
        "Trivial memory after 180 days should nearly vanish, got {}",
        r_trivial_180
    );

    // After 180 days, critical should still be substantial
    let r_critical_180 = compute_relevance(0.9, 0, critical_rate, now - 180 * day, now);
    assert!(
        r_critical_180 > 0.25,
        "Critical memory after 180 days should still be substantial, got {}",
        r_critical_180
    );
}

#[test]
fn lifecycle_knowledge_graph_with_mixed_ages() {
    // Build a realistic knowledge graph with topics of varying ages
    // and importance, then verify the list_roots ordering makes sense.
    let db = test_db();
    let now = now_unix();

    // Recent, important architectural decision
    let _id1 = insert_fragment(
        &db,
        "Use SQLite WAL mode for concurrent access",
        0,
        vec![],
        0.9,
        2,
        3,
        None,
    );

    // Old, frequently accessed pattern
    let _id2 = insert_fragment(
        &db,
        "Rust error handling with thiserror + anyhow",
        0,
        vec![],
        0.5,
        45,
        20,
        None,
    );

    // Old, never accessed trivial observation
    let _id3 = insert_fragment(
        &db,
        "The CI pipeline takes about 3 minutes",
        0,
        vec![],
        0.1,
        60,
        0,
        None,
    );

    // Recent, low importance greeting
    let _id4 = insert_fragment(
        &db,
        "User prefers concise responses",
        0,
        vec![],
        0.3,
        5,
        1,
        None,
    );

    db.storage().recompute_all_relevance(now).unwrap();

    let roots = db.list_roots(None);
    assert_eq!(roots.len(), 4);

    // The recent important decision should rank first
    assert_eq!(
        roots[0].content, "Use SQLite WAL mode for concurrent access",
        "Recent important memory should rank first"
    );

    // The old trivial observation should rank last
    assert_eq!(
        roots[3].content, "The CI pipeline takes about 3 minutes",
        "Old trivial memory should rank last"
    );
}
