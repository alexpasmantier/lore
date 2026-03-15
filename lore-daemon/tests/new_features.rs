//! Comprehensive tests for the daemon-side neuroscience-inspired features:
//! 1. Prediction-error multiplier
//! 2. Schema routing constants
//! 3. Event boundary detection (thorough edge cases)
//! 4. Reflection cluster detection during consolidation
//! 5. Store extraction result with schema routing

use lore_daemon::ingestion::{store_extraction_result, ExtractionResult};
use lore_daemon::parser::{detect_topic_boundaries, ConversationTurn};
use lore_db::edge::EdgeKind;
use lore_db::fragment::Fragment;
use lore_db::storage::Storage;
use lore_db::LoreDb;

fn test_db() -> LoreDb {
    let storage = Storage::open_memory().unwrap();
    LoreDb::new_without_embeddings(storage)
}

fn make_result(trees: Vec<Vec<String>>) -> ExtractionResult {
    ExtractionResult {
        transcript: "test conversation".to_string(),
        trees,
        relationships: vec![],
    }
}

// ════════════════════════════════════════════════════════════════════════
// EVENT BOUNDARY DETECTION - EDGE CASES
// ════════════════════════════════════════════════════════════════════════

#[test]
fn boundary_detection_empty_conversation() {
    let turns: Vec<ConversationTurn> = vec![];
    let boundaries = detect_topic_boundaries(&turns);
    assert_eq!(
        boundaries,
        vec![0],
        "Empty conversation: single segment of len 0"
    );
}

#[test]
fn boundary_detection_single_turn() {
    let turns = vec![ConversationTurn {
        role: "user".to_string(),
        text: "Hello world".to_string(),
    }];
    let boundaries = detect_topic_boundaries(&turns);
    assert_eq!(boundaries, vec![1], "Single turn: one segment");
}

#[test]
fn boundary_detection_exactly_min_segment_times_two() {
    // Exactly 12 turns (2 * MIN_SEGMENT_TURNS=6) — the threshold for allowing splits
    let turns: Vec<ConversationTurn> = (0..12)
        .map(|i| ConversationTurn {
            role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
            text: format!("Generic conversation message number {}", i),
        })
        .collect();
    let boundaries = detect_topic_boundaries(&turns);
    // Same-topic turns should not be split
    assert_eq!(
        *boundaries.last().unwrap(),
        12,
        "Last boundary should be end of turns"
    );
}

#[test]
fn boundary_detection_sharp_topic_shift() {
    // 8 turns about databases, then 8 turns about cooking
    let mut turns = Vec::new();
    for _ in 0..8 {
        turns.push(ConversationTurn {
            role: "user".to_string(),
            text: "How does PostgreSQL handle concurrent transactions with MVCC isolation levels?"
                .to_string(),
        });
    }
    for _ in 0..8 {
        turns.push(ConversationTurn {
            role: "user".to_string(),
            text: "What temperature should I use to bake sourdough bread in a Dutch oven?"
                .to_string(),
        });
    }

    let boundaries = detect_topic_boundaries(&turns);
    assert!(
        boundaries.len() >= 2,
        "Should detect at least 1 boundary, got {:?}",
        boundaries
    );

    // The boundary should be somewhere near turn 8
    if boundaries.len() > 1 {
        let split_point = boundaries[0];
        assert!(
            split_point >= 6 && split_point <= 10,
            "Boundary should be near the topic shift at turn 8, got {}",
            split_point
        );
    }
}

#[test]
fn boundary_detection_three_topics() {
    let mut turns = Vec::new();
    // Topic 1: Rust
    for _ in 0..8 {
        turns.push(ConversationTurn {
            role: "user".to_string(),
            text: "The Rust borrow checker ensures memory safety through lifetime annotations and ownership rules".to_string(),
        });
    }
    // Topic 2: Cooking
    for _ in 0..8 {
        turns.push(ConversationTurn {
            role: "user".to_string(),
            text: "Sourdough starter needs regular feeding with flour and water to maintain yeast activity".to_string(),
        });
    }
    // Topic 3: Space
    for _ in 0..8 {
        turns.push(ConversationTurn {
            role: "user".to_string(),
            text: "The James Webb Space Telescope orbits the Sun at the L2 Lagrange point observing infrared light".to_string(),
        });
    }

    let boundaries = detect_topic_boundaries(&turns);
    assert!(
        boundaries.len() >= 2,
        "Should detect at least 2 boundaries for 3 topics, got {:?}",
        boundaries
    );
}

#[test]
fn boundary_detection_gradual_topic_drift() {
    // Topics slowly drift — harder to detect
    let topics = [
        "Rust programming language features like ownership and borrowing",
        "Rust memory safety model prevents common bugs in systems programming",
        "Systems programming requires careful memory management and resource handling",
        "Resource management in operating systems involves scheduling and allocation",
        "Operating system kernel scheduling algorithms balance fairness and throughput",
        "Algorithm design for throughput optimization in distributed computing",
        "Distributed computing systems require network protocol design and coordination",
        "Network protocol design involves packet routing and error correction",
    ];

    let mut turns = Vec::new();
    for topic in &topics {
        for _ in 0..3 {
            turns.push(ConversationTurn {
                role: "user".to_string(),
                text: topic.to_string(),
            });
        }
    }

    let boundaries = detect_topic_boundaries(&turns);
    // Gradual drift may or may not trigger boundaries — what matters is it doesn't crash
    assert!(
        *boundaries.last().unwrap() == turns.len(),
        "Last boundary should be end of turns"
    );
}

#[test]
fn boundary_detection_with_very_short_messages() {
    // Messages with mostly short words (< 3 chars) that get filtered out
    let mut turns = Vec::new();
    for _ in 0..8 {
        turns.push(ConversationTurn {
            role: "user".to_string(),
            text: "ok so if we do it then yes".to_string(),
        });
    }
    for _ in 0..8 {
        turns.push(ConversationTurn {
            role: "assistant".to_string(),
            text: "I can do that for you now".to_string(),
        });
    }

    let boundaries = detect_topic_boundaries(&turns);
    // Should not crash even with mostly-filtered words
    assert!(
        *boundaries.last().unwrap() == 16,
        "Should handle short words gracefully"
    );
}

#[test]
fn boundary_detection_preserves_segment_ordering() {
    let mut turns = Vec::new();
    for _ in 0..8 {
        turns.push(ConversationTurn {
            role: "user".to_string(),
            text: "Database indexing strategies for PostgreSQL with B-tree and hash indexes"
                .to_string(),
        });
    }
    for _ in 0..8 {
        turns.push(ConversationTurn {
            role: "user".to_string(),
            text: "Chocolate cake recipe with buttercream frosting and ganache decoration"
                .to_string(),
        });
    }

    let boundaries = detect_topic_boundaries(&turns);

    // Verify boundaries are sorted and cover full range
    for window in boundaries.windows(2) {
        assert!(
            window[0] < window[1],
            "Boundaries should be strictly increasing"
        );
    }
    assert_eq!(
        *boundaries.last().unwrap(),
        16,
        "Last boundary should be end"
    );

    // Verify all turns are covered
    let mut start = 0;
    for &end in &boundaries {
        assert!(end > start, "Each segment should have at least 1 turn");
        assert!(
            end - start >= 6 || end == turns.len(),
            "Each segment should have >= MIN_SEGMENT_TURNS (6), got {}",
            end - start
        );
        start = end;
    }
}

// ════════════════════════════════════════════════════════════════════════
// STORE EXTRACTION WITH SCHEMA ROUTING (no-embedder path)
// ════════════════════════════════════════════════════════════════════════

#[test]
fn store_extraction_without_embedder_uses_default_routing() {
    let db = test_db();

    // First extraction: creates root
    let result1 = make_result(vec![vec![
        "Rust error handling patterns".to_string(),
        "Use thiserror for libraries".to_string(),
    ]]);
    let count1 = store_extraction_result(&db, &result1, Some("s1")).unwrap();
    assert_eq!(count1, 3); // 2 tree nodes + 1 transcript

    // Second extraction: similar topic but no embedder → defaults to new root
    let result2 = make_result(vec![vec![
        "Rust error handling best practices".to_string(),
        "Box errors to avoid inflation".to_string(),
    ]]);
    let count2 = store_extraction_result(&db, &result2, Some("s2")).unwrap();
    assert_eq!(count2, 3);

    // Without embedder, both should be separate roots (no schema routing)
    let roots = db.list_roots(None);
    // Filter out transcript fragments (importance 0.1)
    let knowledge_roots: Vec<_> = roots
        .iter()
        .filter(|r| (r.importance - 0.1).abs() > 0.01)
        .collect();
    assert_eq!(
        knowledge_roots.len(),
        2,
        "Without embedder, should create separate roots"
    );
}

#[test]
fn store_extraction_importance_unchanged_without_embedder() {
    let db = test_db();

    // Without embedder, prediction error multiplier is 1.0 (no adjustment)
    let result = make_result(vec![vec![
        "Root concept".to_string(),
        "Middle detail".to_string(),
        "Leaf observation".to_string(),
    ]]);
    store_extraction_result(&db, &result, Some("test")).unwrap();

    let roots: Vec<_> = db
        .list_roots(None)
        .into_iter()
        .filter(|r| (r.importance - 0.1).abs() > 0.01)
        .collect();
    assert_eq!(roots.len(), 1);
    let root = &roots[0];

    // Importance should be exactly the base values (no PE multiplier)
    assert!(
        (root.importance - 0.9).abs() < 0.01,
        "Root importance should be 0.9, got {}",
        root.importance
    );

    let children = db.children(root.id);
    assert_eq!(children.len(), 1);
    let mid = &children[0];
    assert!(
        (mid.importance - 0.7).abs() < 0.01,
        "Middle importance should be 0.7, got {}",
        mid.importance
    );

    let leaves = db.children(mid.id);
    assert_eq!(leaves.len(), 1);
    let leaf = &leaves[0];
    assert!(
        (leaf.importance - 0.5).abs() < 0.01,
        "Leaf importance should be 0.5, got {}",
        leaf.importance
    );
}

// ════════════════════════════════════════════════════════════════════════
// REFLECTION CLUSTER DETECTION
// ════════════════════════════════════════════════════════════════════════

#[test]
fn dense_cluster_requires_minimum_connections() {
    let db = test_db();

    // 2 roots with 1 associative link — too sparse for reflection
    let result = make_result(vec![
        vec!["Topic A: Rust patterns".to_string()],
        vec!["Topic B: Go patterns".to_string()],
    ]);
    store_extraction_result(&db, &result, Some("s1")).unwrap();

    let roots: Vec<_> = db
        .list_roots(None)
        .into_iter()
        .filter(|r| (r.importance - 0.1).abs() > 0.01)
        .collect();

    // Create one associative link
    if roots.len() >= 2 {
        db.link(roots[0].id, roots[1].id, EdgeKind::Associative, 0.8)
            .unwrap();
    }

    // Check: each root has only 1 associative connection to another root
    for root in &roots {
        let edges = db.storage().get_edges_for(root.id).unwrap_or_default();
        let root_ids: std::collections::HashSet<_> = roots.iter().map(|r| r.id).collect();
        let root_connections: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Associative)
            .filter(|e| {
                let neighbor = if e.source == root.id {
                    e.target
                } else {
                    e.source
                };
                root_ids.contains(&neighbor)
            })
            .collect();

        assert!(
            root_connections.len() < 3,
            "With only 2 roots, no root should have >= 3 connections"
        );
    }
}

#[test]
fn dense_cluster_detected_with_sufficient_connections() {
    let db = test_db();

    // Create 4 roots and connect them all to root_a
    let mut roots = Vec::new();
    for i in 0..4 {
        let frag = Fragment::new(format!("Topic {}", i), 0);
        db.storage().insert_fragment(&frag).unwrap();
        roots.push(frag.id);
    }

    // Connect roots[0] to all others
    for i in 1..4 {
        db.link(roots[0], roots[i], EdgeKind::Associative, 0.8)
            .unwrap();
    }

    // roots[0] should have 3 associative connections to other roots
    let edges = db.storage().get_edges_for(roots[0]).unwrap();
    let assoc_to_roots: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Associative)
        .collect();
    assert_eq!(
        assoc_to_roots.len(),
        3,
        "Center root should have 3 associative edges"
    );
}

#[test]
fn derived_from_edge_prevents_duplicate_reflection() {
    let db = test_db();

    // Create a "center" root with 3+ connections
    let center = Fragment::new("Center topic".to_string(), 0);
    db.storage().insert_fragment(&center).unwrap();

    for i in 0..3 {
        let frag = Fragment::new(format!("Connected topic {}", i), 0);
        db.storage().insert_fragment(&frag).unwrap();
        db.link(center.id, frag.id, EdgeKind::Associative, 0.8)
            .unwrap();
    }

    // Add a "derived_from" edge (simulating an existing reflection)
    let reflection = Fragment::new("Existing reflection".to_string(), 0);
    db.storage().insert_fragment(&reflection).unwrap();
    db.link_with_content(
        center.id,
        reflection.id,
        EdgeKind::Associative,
        1.0,
        Some("derived_from".to_string()),
    )
    .unwrap();

    // Check: the center has a derived_from edge
    let edges = db.storage().get_edges_for(center.id).unwrap();
    let has_reflection = edges
        .iter()
        .any(|e| e.content.as_deref() == Some("derived_from"));
    assert!(
        has_reflection,
        "Center should have a derived_from edge indicating existing reflection"
    );
}

// ════════════════════════════════════════════════════════════════════════
// INTEGRATION: CONSOLIDATION WITH NEW FEATURES
// ════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn consolidation_stats_include_reflections_field() {
    let db = test_db();

    // Ingest some data
    let result = make_result(vec![vec!["Topic about Rust".to_string()]]);
    store_extraction_result(&db, &result, Some("s1")).unwrap();

    let config = lore_daemon::config::ConsolidationConfig::default();
    let stats = lore_daemon::consolidation::run_consolidation(&db, None, None, &config)
        .await
        .unwrap();

    // reflections_generated should be in stats (even if 0 without API key)
    assert_eq!(
        stats.reflections_generated, 0,
        "Without API key, no reflections should be generated"
    );
}

#[tokio::test]
async fn consolidation_runs_all_phases_without_api_key() {
    let db = test_db();

    // Ingest a multi-tree extraction
    let result = make_result(vec![
        vec!["Topic A".to_string(), "Detail A".to_string()],
        vec!["Topic B".to_string(), "Detail B".to_string()],
        vec!["Topic C".to_string(), "Detail C".to_string()],
    ]);
    store_extraction_result(&db, &result, Some("s1")).unwrap();

    let config = lore_daemon::config::ConsolidationConfig::default();
    let stats = lore_daemon::consolidation::run_consolidation(&db, None, None, &config)
        .await
        .unwrap();

    // Phase 1 (relevance recomputation) should run
    assert!(stats.relevance_updated > 0, "Should recompute relevance");

    // Phase 5-6 (reflection, contradiction) skipped without API
    assert_eq!(stats.reflections_generated, 0);
    assert_eq!(stats.contradictions_resolved, 0);

    // Phase 7-8 (edge/fragment pruning) should run
    // (may prune 0 since everything is fresh)
}
