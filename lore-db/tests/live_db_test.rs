//! Test against the real ~/.lore/memory.db to verify migration,
//! decay recomputation, reinforcement, and spreading activation
//! on actual production data.
//!
//! These tests are #[ignore]d by default since they require a real database.
//! Run with: cargo test -p lore-db --test live_db_test -- --ignored

use lore_db::fragment::now_unix;
use lore_db::relevance::compute_relevance;
use lore_db::{LoreDb, Storage};
use std::path::PathBuf;

fn live_db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap();
    PathBuf::from(format!("{}/.lore/memory.db", home))
}

fn open_live_db() -> LoreDb {
    let path = live_db_path();
    if !path.exists() {
        panic!("No live database at {:?}", path);
    }
    let storage = Storage::open(&path).unwrap();
    LoreDb::new_without_embeddings(storage)
}

#[test]
#[ignore]
fn live_schema_has_v2_columns() {
    let db = open_live_db();
    let topics = db.list_topics(None);
    assert!(
        !topics.is_empty(),
        "Should have topics in the live database"
    );

    // V2 columns should be readable
    let first = &topics[0];
    assert!(first.importance >= 0.0 && first.importance <= 1.0);
    assert!(first.relevance_score >= 0.0 && first.relevance_score <= 1.0);
    assert!(first.decay_rate > 0.0);
    println!(
        "First topic: '{}' importance={:.2} relevance={:.4} decay_rate={:.4}",
        first.summary, first.importance, first.relevance_score, first.decay_rate
    );
}

#[test]
#[ignore]
fn live_topics_sorted_by_relevance() {
    let db = open_live_db();
    let topics = db.list_topics(None);

    for i in 1..topics.len() {
        assert!(
            topics[i - 1].relevance_score >= topics[i].relevance_score,
            "Topics should be sorted by relevance descending: {} ({:.4}) vs {} ({:.4})",
            topics[i - 1].summary,
            topics[i - 1].relevance_score,
            topics[i].summary,
            topics[i].relevance_score,
        );
    }
    println!("All {} topics correctly sorted by relevance", topics.len());
}

#[test]
#[ignore]
fn live_decay_recomputation() {
    let db = open_live_db();
    let now = now_unix();

    let count = db.storage().recompute_all_relevance(now).unwrap();
    println!("Recomputed relevance for {} fragments", count);
    assert!(count > 0, "Should have recomputed at least some fragments");

    // After recomputation, topics should still be sorted
    let topics = db.list_topics(None);
    for i in 1..topics.len() {
        assert!(
            topics[i - 1].relevance_score >= topics[i].relevance_score,
            "Should remain sorted after recomputation"
        );
    }
}

#[test]
#[ignore]
fn live_reinforcement_on_access() {
    let db = open_live_db();
    let now = now_unix();

    // Pick a topic to reinforce
    let topics = db.list_topics(None);
    let target = &topics[topics.len() - 1]; // pick the least relevant
    let before_rel = target.relevance_score;
    let before_access = target.access_count;

    println!(
        "Before: '{}' relevance={:.4} access_count={}",
        target.summary, before_rel, before_access
    );

    // Reinforce it
    let new_relevance = compute_relevance(
        target.importance,
        target.access_count + 1,
        target.decay_rate,
        now,
        now,
    );
    db.storage()
        .reinforce_fragment(target.id, now, new_relevance)
        .unwrap();

    // Verify
    let after = db.storage().get_fragment(target.id).unwrap().unwrap();
    println!(
        "After:  '{}' relevance={:.4} access_count={} last_reinforced={}",
        after.summary, after.relevance_score, after.access_count, after.last_reinforced
    );

    assert!(
        after.relevance_score > before_rel,
        "Reinforcement should increase relevance: {:.4} -> {:.4}",
        before_rel,
        after.relevance_score
    );
    assert_eq!(after.access_count, before_access + 1);
    assert_eq!(after.last_reinforced, now);
}

#[test]
#[ignore]
fn live_spreading_activation() {
    let db = open_live_db();
    let now = now_unix();

    // Find a topic with children
    let topics = db.list_topics(None);
    let topic_with_children = topics.iter().find(|t| !db.children(t.id).is_empty());

    if let Some(topic) = topic_with_children {
        let children = db.children(topic.id);
        let child = &children[0];
        let before = child.relevance_score;

        println!(
            "Boosting child '{}' of '{}' (relevance before: {:.4})",
            child.summary, topic.summary, before
        );

        // Boost the child (spreading activation)
        db.storage().boost_relevance(child.id, 0.05, now).unwrap();

        let after = db.storage().get_fragment(child.id).unwrap().unwrap();
        println!("Relevance after boost: {:.4}", after.relevance_score);

        assert!(
            after.relevance_score >= before,
            "Boost should not decrease relevance"
        );
    } else {
        println!("No topics with children found — skipping");
    }
}

#[test]
#[ignore]
fn live_print_relevance_distribution() {
    let db = open_live_db();
    let topics = db.list_topics(None);

    println!("\n=== Topic Relevance Distribution ===\n");
    println!(
        "{:<60} {:>6} {:>5} {:>8}",
        "Summary", "Rel", "Acc", "Age(d)"
    );
    println!("{}", "-".repeat(85));

    let now = now_unix();
    for t in &topics {
        let age_days = (now - t.created_at) / 86400;
        println!(
            "{:<60} {:.3}  {:>4}  {:>5}",
            &t.summary[..t.summary.len().min(60)],
            t.relevance_score,
            t.access_count,
            age_days,
        );
    }

    println!("\nTotal topics: {}", topics.len());
}
