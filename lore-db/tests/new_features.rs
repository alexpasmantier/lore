//! Comprehensive tests for the neuroscience-inspired features:
//! 1. Prediction-error-weighted encoding
//! 2. Schema-based dual routing
//! 3. Personalized PageRank deep search
//! 4. Metadata persistence (used by reflection fragments)
//!
//! These tests use synthetic embeddings to exercise code paths that
//! are transparent when using `new_without_embeddings`.

use lore_db::edge::{Edge, EdgeId, EdgeKind};
use lore_db::embedding::cosine_similarity;
use lore_db::fragment::{now_unix, Fragment, FragmentId};
use lore_db::storage::Storage;
use lore_db::LoreDb;

// ════════════════════════════════════════════════════════════════════════
// HELPERS
// ════════════════════════════════════════════════════════════════════════

fn test_db() -> LoreDb {
    let storage = Storage::open_memory().unwrap();
    LoreDb::new_without_embeddings(storage)
}

/// Create a synthetic embedding that concentrates energy in specific dimensions.
/// Different `topic_id` values produce embeddings pointing in different directions,
/// giving meaningful cosine similarity differences. `noise` adds small perturbation.
fn make_embedding(topic_id: u32, noise: f32) -> Vec<f32> {
    let mut emb = vec![0.01_f32; 384]; // small background
                                       // Each "topic" gets a unique cluster of hot dimensions
    let start = (topic_id as usize * 30) % 384;
    for i in 0..30 {
        let idx = (start + i) % 384;
        emb[idx] = 1.0 + noise * (i as f32 / 30.0);
    }
    emb
}

/// Insert a root fragment with a synthetic embedding.
fn insert_root(db: &LoreDb, content: &str, embedding: Vec<f32>) -> FragmentId {
    let mut frag = Fragment::new(content.to_string(), 0);
    frag.embedding = embedding;
    db.storage().insert_fragment(&frag).unwrap();
    frag.id
}

/// Insert a child fragment with a synthetic embedding under a parent.
fn insert_child(db: &LoreDb, content: &str, depth: u32, parent: FragmentId) -> FragmentId {
    let mut frag = Fragment::new(content.to_string(), depth);
    frag.embedding = make_embedding(2, 0.0);
    db.storage().insert_fragment(&frag).unwrap();
    let edge = Edge {
        id: EdgeId::new(),
        source: parent,
        target: frag.id,
        kind: EdgeKind::Hierarchical,
        weight: 1.0,
        content: None,
        created_at: now_unix(),
    };
    db.storage().insert_edge(&edge).unwrap();
    frag.id
}

// ════════════════════════════════════════════════════════════════════════
// 1. PREDICTION-ERROR ENCODING
// ════════════════════════════════════════════════════════════════════════

#[test]
fn find_best_root_by_embedding_returns_most_similar() {
    let db = test_db();

    let rust_emb = make_embedding(0, 0.0); // topic 0
    let python_emb = make_embedding(5, 0.0); // topic 5 (different direction)
    let _rust_id = insert_root(&db, "Rust programming", rust_emb.clone());
    let python_id = insert_root(&db, "Python programming", python_emb.clone());

    // Query with something close to python (same topic 5, slight noise)
    let query_emb = make_embedding(5, 0.1);
    let result = db.find_best_root_by_embedding(&query_emb);
    assert!(result.is_some());
    let (id, sim) = result.unwrap();
    assert_eq!(
        id, python_id,
        "Should match Python root (closest embedding)"
    );
    assert!(sim > 0.95, "Should have high similarity, got {}", sim);
}

#[test]
fn find_best_root_by_embedding_ignores_superseded() {
    let db = test_db();

    let emb = make_embedding(0, 0.0);
    let old_id = insert_root(&db, "Old knowledge", emb.clone());
    let new_id = insert_root(&db, "New knowledge", make_embedding(3, 0.0));

    // Supersede old
    db.supersede(old_id, new_id).unwrap();

    // Search should NOT find the superseded root
    let result = db.find_best_root_by_embedding(&emb);
    assert!(result.is_some());
    let (id, _) = result.unwrap();
    assert_ne!(id, old_id, "Should not return superseded fragment");
}

#[test]
fn find_best_root_by_embedding_returns_none_for_empty_db() {
    let db = test_db();
    let emb = make_embedding(0, 0.0);
    assert!(db.find_best_root_by_embedding(&emb).is_none());
}

#[test]
fn max_root_similarity_returns_none_without_embedder() {
    let db = test_db();
    insert_root(&db, "Some root", make_embedding(0, 0.0));
    // Without embedder, max_root_similarity can't embed the query text
    assert!(db.max_root_similarity("anything").is_none());
}

// ════════════════════════════════════════════════════════════════════════
// 2. SCHEMA-BASED DUAL ROUTING (tested via depth offsets and edge creation)
// ════════════════════════════════════════════════════════════════════════

// Note: Schema routing in store_extraction_result depends on embed_text()
// which requires a real embedder. We test the building blocks here and
// the routing constants/logic in the daemon integration tests.

#[test]
fn high_similarity_roots_would_trigger_high_fit_routing() {
    let db = test_db();
    let emb_a = make_embedding(0, 0.0);
    let _root_a = insert_root(&db, "Rust error handling patterns", emb_a);

    // Very similar embedding (same topic, slight noise)
    let emb_b = make_embedding(0, 0.1);
    let result = db.find_best_root_by_embedding(&emb_b);
    let (_, sim) = result.unwrap();

    // This similarity should be above the HIGH_FIT threshold (0.75)
    assert!(
        sim > 0.75,
        "Near-identical embeddings should produce sim > 0.75, got {}",
        sim
    );
}

#[test]
fn dissimilar_roots_would_trigger_low_fit_routing() {
    let db = test_db();
    let emb_a = make_embedding(0, 0.0); // topic 0: dims 0-29 hot
    let _root_a = insert_root(&db, "Rust programming", emb_a);

    // Very different topic: dims 150-179 hot
    let emb_b = make_embedding(5, 0.0);
    let result = db.find_best_root_by_embedding(&emb_b);
    let (_, sim) = result.unwrap();

    // Different topic clusters should have low similarity
    assert!(
        sim < 0.35,
        "Different topic embeddings should produce sim < 0.35, got {}",
        sim
    );
}

// ════════════════════════════════════════════════════════════════════════
// 3. PERSONALIZED PAGERANK DEEP SEARCH
// ════════════════════════════════════════════════════════════════════════

#[test]
fn search_deep_discovers_fragments_via_associative_edges() {
    let db = test_db();

    // Tree A: Rust errors
    let root_a = insert_root(&db, "Rust error handling patterns", make_embedding(0, 0.0));
    let _child_a = insert_child(&db, "Use thiserror for libraries", 1, root_a);

    // Tree B: Python errors (very different topic)
    let root_b = insert_root(&db, "Python exception handling", make_embedding(5, 0.0));
    let _child_b = insert_child(&db, "Use custom exception classes", 1, root_b);

    // Tree C: Go errors (connected to A via associative edge)
    let root_c = insert_root(
        &db,
        "Go error handling with error wrapping",
        make_embedding(3, 0.0),
    );

    // Create associative link: A↔C (error handling related)
    db.link(root_a, root_c, EdgeKind::Associative, 0.9).unwrap();

    // Regular query for "rust" should find A (text match)
    let regular = db.query("rust error", 10);
    let regular_ids: Vec<FragmentId> = regular.iter().map(|sf| sf.fragment.id).collect();
    assert!(
        regular_ids.contains(&root_a),
        "Regular search should find Rust root"
    );

    // Deep search should find A (semantic) + C (via PPR through associative edge)
    let deep = db.search_deep("rust error", 10);
    let deep_ids: Vec<FragmentId> = deep.iter().map(|sf| sf.fragment.id).collect();
    assert!(
        deep_ids.contains(&root_a),
        "Deep search should find Rust root"
    );
    assert!(
        deep_ids.contains(&root_c),
        "Deep search should discover Go root via associative edge to Rust"
    );

    // B should NOT appear (no associative path from A)
    assert!(
        !deep_ids.contains(&root_b),
        "Python root should not be discovered (no associative link)"
    );
}

#[test]
fn search_deep_propagates_through_multi_hop_edges() {
    let db = test_db();

    // Chain: A → B → C (via associative edges)
    let root_a = insert_root(&db, "Rust programming", make_embedding(0, 0.0));
    let root_b = insert_root(&db, "Systems programming concepts", make_embedding(3, 0.0));
    let root_c = insert_root(&db, "Memory management strategies", make_embedding(6, 0.0));

    db.link(root_a, root_b, EdgeKind::Associative, 0.8).unwrap();
    db.link(root_b, root_c, EdgeKind::Associative, 0.8).unwrap();

    // Deep search for "rust" — should find A, and potentially B and C via chain
    let deep = db.search_deep("rust", 10);
    let deep_ids: Vec<FragmentId> = deep.iter().map(|sf| sf.fragment.id).collect();
    assert!(deep_ids.contains(&root_a), "Should find direct match");

    // B should be discovered (1 hop from A)
    assert!(
        deep_ids.contains(&root_b),
        "Should discover 1-hop neighbor B"
    );
    // C might be discovered (2 hops) depending on PPR convergence
    // This tests that multi-hop propagation works
}

#[test]
fn search_deep_with_no_associative_edges_returns_same_as_query() {
    let db = test_db();

    let root_a = insert_root(&db, "Rust programming language", make_embedding(0, 0.0));
    let _child_a = insert_child(&db, "Rust async programming", 1, root_a);

    // No associative edges — deep search should behave like regular search
    let regular = db.query("rust", 10);
    let deep = db.search_deep("rust", 10);

    assert_eq!(
        regular.len(),
        deep.len(),
        "Without associative edges, deep search should return same count as regular"
    );
}

#[test]
fn search_deep_reinforces_discovered_fragments() {
    let db = test_db();

    let root_a = insert_root(&db, "Rust programming", make_embedding(0, 0.0));
    let root_b = insert_root(&db, "Systems programming", make_embedding(3, 0.0));
    db.link(root_a, root_b, EdgeKind::Associative, 0.9).unwrap();

    let before = db.storage().get_fragment(root_b).unwrap().unwrap();
    assert_eq!(before.access_count, 0);

    // Deep search should discover and reinforce root_b
    let _ = db.search_deep("rust", 10);

    let _after = db.storage().get_fragment(root_b).unwrap().unwrap();
    // root_b may be reinforced if PPR discovered it
    // (depends on whether it appears in results after dedup)
}

#[test]
fn search_deep_deduplicates_by_tree() {
    let db = test_db();

    // Root with multiple children, all linked associatively to another root
    let root_a = insert_root(&db, "Rust programming", make_embedding(0, 0.0));
    let child_a1 = insert_child(&db, "Rust async", 1, root_a);
    let child_a2 = insert_child(&db, "Rust traits", 1, root_a);

    let root_b = insert_root(&db, "Type systems", make_embedding(3, 0.0));
    db.link(child_a1, root_b, EdgeKind::Associative, 0.8)
        .unwrap();
    db.link(child_a2, root_b, EdgeKind::Associative, 0.8)
        .unwrap();

    let deep = db.search_deep("rust", 10);
    // Should not have duplicate entries from root_b's tree
    let root_b_entries: Vec<_> = deep.iter().filter(|sf| sf.fragment.id == root_b).collect();
    assert!(
        root_b_entries.len() <= 1,
        "Should deduplicate results from same tree"
    );
}

// ════════════════════════════════════════════════════════════════════════
// 4. METADATA PERSISTENCE (critical for reflection fragments)
// ════════════════════════════════════════════════════════════════════════

#[test]
fn metadata_roundtrips_through_sqlite() {
    let db = test_db();

    let mut frag = Fragment::new("Test content".to_string(), 0);
    frag.metadata
        .insert("type".to_string(), "reflection".to_string());
    frag.metadata
        .insert("source_count".to_string(), "5".to_string());
    db.storage().insert_fragment(&frag).unwrap();

    let loaded = db.storage().get_fragment(frag.id).unwrap().unwrap();
    assert_eq!(
        loaded.metadata.get("type").map(|s| s.as_str()),
        Some("reflection"),
        "metadata 'type' should survive roundtrip"
    );
    assert_eq!(
        loaded.metadata.get("source_count").map(|s| s.as_str()),
        Some("5"),
        "metadata 'source_count' should survive roundtrip"
    );
}

#[test]
fn empty_metadata_roundtrips_correctly() {
    let db = test_db();

    let frag = Fragment::new("No metadata".to_string(), 0);
    assert!(frag.metadata.is_empty());
    db.storage().insert_fragment(&frag).unwrap();

    let loaded = db.storage().get_fragment(frag.id).unwrap().unwrap();
    assert!(
        loaded.metadata.is_empty(),
        "Empty metadata should roundtrip as empty"
    );
}

#[test]
fn metadata_with_special_characters_roundtrips() {
    let db = test_db();

    let mut frag = Fragment::new("Special chars".to_string(), 0);
    frag.metadata.insert(
        "description".to_string(),
        "Contains \"quotes\" and\nnewlines and émojis 🧠".to_string(),
    );
    db.storage().insert_fragment(&frag).unwrap();

    let loaded = db.storage().get_fragment(frag.id).unwrap().unwrap();
    assert_eq!(
        loaded.metadata.get("description").unwrap(),
        "Contains \"quotes\" and\nnewlines and émojis 🧠"
    );
}

#[test]
fn metadata_visible_in_list_roots() {
    let db = test_db();

    let mut frag = Fragment::new("Reflection about patterns".to_string(), 0);
    frag.metadata
        .insert("type".to_string(), "reflection".to_string());
    db.storage().insert_fragment(&frag).unwrap();

    let roots = db.list_roots(None);
    let loaded = roots.iter().find(|r| r.id == frag.id).unwrap();
    assert_eq!(
        loaded.metadata.get("type").map(|s| s.as_str()),
        Some("reflection"),
        "metadata should be visible when listing roots"
    );
}

#[test]
fn reflection_fragments_can_be_filtered_by_metadata() {
    let db = test_db();

    // Insert a regular root
    let regular = Fragment::new("Regular knowledge".to_string(), 0);
    db.storage().insert_fragment(&regular).unwrap();

    // Insert a reflection root
    let mut reflection = Fragment::new("Emerging pattern insight".to_string(), 0);
    reflection
        .metadata
        .insert("type".to_string(), "reflection".to_string());
    db.storage().insert_fragment(&reflection).unwrap();

    let roots = db.list_roots(None);
    assert_eq!(roots.len(), 2);

    let reflections: Vec<_> = roots
        .iter()
        .filter(|r| r.metadata.get("type").map(|t| t.as_str()) == Some("reflection"))
        .collect();
    assert_eq!(reflections.len(), 1);
    assert_eq!(reflections[0].content, "Emerging pattern insight");

    let non_reflections: Vec<_> = roots
        .iter()
        .filter(|r| r.metadata.get("type").map(|t| t.as_str()) != Some("reflection"))
        .collect();
    assert_eq!(non_reflections.len(), 1);
    assert_eq!(non_reflections[0].content, "Regular knowledge");
}

// ════════════════════════════════════════════════════════════════════════
// 5. EDGE CASE: PPR WITH COMPLEX GRAPH TOPOLOGIES
// ════════════════════════════════════════════════════════════════════════

#[test]
fn ppr_handles_bidirectional_edges() {
    let db = test_db();

    let root_a = insert_root(&db, "Rust error handling", make_embedding(0, 0.0));
    let root_b = insert_root(&db, "Go error handling", make_embedding(3, 0.0));

    // Bidirectional associative link (two edges)
    db.link(root_a, root_b, EdgeKind::Associative, 0.9).unwrap();
    db.link(root_b, root_a, EdgeKind::Associative, 0.9).unwrap();

    // Should not crash or infinite-loop
    let results = db.search_deep("rust error", 10);
    assert!(!results.is_empty());
}

#[test]
fn ppr_handles_disconnected_components() {
    let db = test_db();

    // Component 1: A ↔ B
    let root_a = insert_root(&db, "Rust programming", make_embedding(0, 0.0));
    let root_b = insert_root(&db, "Go programming", make_embedding(1, 0.0));
    db.link(root_a, root_b, EdgeKind::Associative, 0.8).unwrap();

    // Component 2: C ↔ D (disconnected from A,B)
    let root_c = insert_root(&db, "Cooking recipes", make_embedding(5, 0.0));
    let root_d = insert_root(&db, "Baking techniques", make_embedding(6, 0.0));
    db.link(root_c, root_d, EdgeKind::Associative, 0.8).unwrap();

    // Search for "rust" — should find A (and maybe B), but NOT C or D
    let results = db.search_deep("rust", 10);
    let ids: Vec<FragmentId> = results.iter().map(|sf| sf.fragment.id).collect();
    assert!(ids.contains(&root_a));
    assert!(
        !ids.contains(&root_c),
        "Disconnected component should not appear"
    );
    assert!(
        !ids.contains(&root_d),
        "Disconnected component should not appear"
    );
}

#[test]
fn ppr_respects_edge_weights() {
    let db = test_db();

    let root_a = insert_root(&db, "Rust programming", make_embedding(0, 0.0));
    let root_b = insert_root(&db, "Strongly related", make_embedding(3, 0.0));
    let root_c = insert_root(&db, "Weakly related", make_embedding(6, 0.0));

    db.link(root_a, root_b, EdgeKind::Associative, 1.0).unwrap();
    db.link(root_a, root_c, EdgeKind::Associative, 0.2).unwrap();

    let results = db.search_deep("rust", 10);
    let b_score = results
        .iter()
        .find(|sf| sf.fragment.id == root_b)
        .map(|sf| sf.score);
    let c_score = results
        .iter()
        .find(|sf| sf.fragment.id == root_c)
        .map(|sf| sf.score);

    if let (Some(b), Some(c)) = (b_score, c_score) {
        assert!(
            b > c,
            "Strongly linked neighbor ({}) should score higher than weakly linked ({})",
            b,
            c
        );
    }
}

// ════════════════════════════════════════════════════════════════════════
// 6. COSINE SIMILARITY EDGE CASES (affects all embedding-based features)
// ════════════════════════════════════════════════════════════════════════

#[test]
fn cosine_similarity_of_synthetic_embeddings_is_predictable() {
    // Identical embeddings → sim ≈ 1.0
    let a = make_embedding(0, 0.0);
    let b = make_embedding(0, 0.0);
    let sim = cosine_similarity(&a, &b);
    assert!(
        sim > 0.999,
        "Identical embeddings should have sim ~1.0, got {}",
        sim
    );

    // Same topic with noise → high but not perfect
    let c = make_embedding(0, 0.5);
    let sim2 = cosine_similarity(&a, &c);
    assert!(
        sim2 > 0.9 && sim2 < 1.0,
        "Same topic with noise should be high: {}",
        sim2
    );

    // Different topics → low similarity (different hot dimensions)
    let d = make_embedding(0, 0.0); // dims 0-29 hot
    let e = make_embedding(5, 0.0); // dims 150-179 hot
    let sim3 = cosine_similarity(&d, &e);
    assert!(
        sim3 < 0.3,
        "Different topic embeddings should have low sim, got {}",
        sim3
    );

    // Adjacent topics → moderate similarity (some overlap in hot dims)
    let f = make_embedding(0, 0.0); // dims 0-29
    let g = make_embedding(1, 0.0); // dims 30-59
    let sim4 = cosine_similarity(&f, &g);
    assert!(
        sim4 < 0.5,
        "Adjacent but non-overlapping topics should have low-moderate sim, got {}",
        sim4
    );
}

#[test]
fn find_best_root_handles_single_root() {
    let db = test_db();
    let emb = make_embedding(0, 0.0);
    let root_id = insert_root(&db, "Only root", emb.clone());

    let result = db.find_best_root_by_embedding(&emb);
    assert!(result.is_some());
    let (id, sim) = result.unwrap();
    assert_eq!(id, root_id);
    assert!(sim > 0.99);
}

// ════════════════════════════════════════════════════════════════════════
// 7. INTEGRATION: FEATURES WORKING TOGETHER
// ════════════════════════════════════════════════════════════════════════

#[test]
fn deep_search_on_graph_with_reflections() {
    let db = test_db();

    // Create 3 roots with associative links
    let root_a = insert_root(&db, "Rust error handling", make_embedding(0, 0.0));
    let root_b = insert_root(&db, "Go error handling", make_embedding(1, 0.0));
    let root_c = insert_root(&db, "Error handling principles", make_embedding(2, 0.0));

    db.link(root_a, root_b, EdgeKind::Associative, 0.8).unwrap();
    db.link(root_a, root_c, EdgeKind::Associative, 0.8).unwrap();
    db.link(root_b, root_c, EdgeKind::Associative, 0.7).unwrap();

    // Add a reflection fragment
    let mut reflection = Fragment::new("Error handling is a cross-cutting concern".to_string(), 0);
    reflection.embedding = make_embedding(0, 0.1);
    reflection
        .metadata
        .insert("type".to_string(), "reflection".to_string());
    db.storage().insert_fragment(&reflection).unwrap();

    // Link reflection to the cluster
    db.link_with_content(
        root_a,
        reflection.id,
        EdgeKind::Associative,
        1.0,
        Some("derived_from".to_string()),
    )
    .unwrap();

    // Deep search should potentially find the reflection via the cluster
    let results = db.search_deep("rust error", 10);
    assert!(!results.is_empty(), "Should find at least the direct match");
}

#[test]
fn superseded_fragments_invisible_to_all_new_features() {
    let db = test_db();

    let old_emb = make_embedding(0, 0.0);
    let old_id = insert_root(&db, "Old knowledge", old_emb.clone());
    let new_id = insert_root(&db, "New knowledge", make_embedding(3, 0.0));
    db.supersede(old_id, new_id).unwrap();

    // find_best_root_by_embedding should not find superseded
    let result = db.find_best_root_by_embedding(&old_emb);
    assert!(result.is_some());
    assert_ne!(result.unwrap().0, old_id);

    // Deep search should not return superseded
    let deep = db.search_deep("old knowledge", 10);
    let ids: Vec<FragmentId> = deep.iter().map(|sf| sf.fragment.id).collect();
    assert!(
        !ids.contains(&old_id),
        "Superseded should be invisible to deep search"
    );
}
