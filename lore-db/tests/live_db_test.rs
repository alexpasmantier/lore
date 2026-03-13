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
    lore_db::lore_home().join("memory.db")
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
    let roots = db.list_roots(None);
    assert!(!roots.is_empty(), "Should have roots in the live database");

    // V2 columns should be readable
    let first = &roots[0];
    assert!(first.importance >= 0.0 && first.importance <= 1.0);
    assert!(first.relevance_score >= 0.0 && first.relevance_score <= 1.0);
    assert!(first.decay_rate > 0.0);
    println!(
        "First root: '{}' importance={:.2} relevance={:.4} decay_rate={:.4}",
        first.content, first.importance, first.relevance_score, first.decay_rate
    );
}

#[test]
#[ignore]
fn live_roots_sorted_by_relevance() {
    let db = open_live_db();
    let roots = db.list_roots(None);

    for i in 1..roots.len() {
        assert!(
            roots[i - 1].relevance_score >= roots[i].relevance_score,
            "Roots should be sorted by relevance descending: {} ({:.4}) vs {} ({:.4})",
            roots[i - 1].content,
            roots[i - 1].relevance_score,
            roots[i].content,
            roots[i].relevance_score,
        );
    }
    println!("All {} roots correctly sorted by relevance", roots.len());
}

#[test]
#[ignore]
fn live_decay_recomputation() {
    let db = open_live_db();
    let now = now_unix();

    let count = db.storage().recompute_all_relevance(now).unwrap();
    println!("Recomputed relevance for {} fragments", count);
    assert!(count > 0, "Should have recomputed at least some fragments");

    // After recomputation, roots should still be sorted
    let roots = db.list_roots(None);
    for i in 1..roots.len() {
        assert!(
            roots[i - 1].relevance_score >= roots[i].relevance_score,
            "Should remain sorted after recomputation"
        );
    }
}

#[test]
#[ignore]
fn live_reinforcement_on_access() {
    let db = open_live_db();
    let now = now_unix();

    // Pick a root to reinforce
    let roots = db.list_roots(None);
    let target = &roots[roots.len() - 1]; // pick the least relevant
    let before_rel = target.relevance_score;
    let before_access = target.access_count;

    println!(
        "Before: '{}' relevance={:.4} access_count={}",
        target.content, before_rel, before_access
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
        after.content, after.relevance_score, after.access_count, after.last_reinforced
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

    // Find a root with children
    let roots = db.list_roots(None);
    let root_with_children = roots.iter().find(|t| !db.children(t.id).is_empty());

    if let Some(root) = root_with_children {
        let children = db.children(root.id);
        let child = &children[0];
        let before = child.relevance_score;

        println!(
            "Boosting child '{}' of '{}' (relevance before: {:.4})",
            child.content, root.content, before
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
        println!("No roots with children found — skipping");
    }
}

#[test]
#[ignore]
fn live_print_relevance_distribution() {
    let db = open_live_db();
    let roots = db.list_roots(None);

    println!("\n=== Root Relevance Distribution ===\n");
    println!(
        "{:<60} {:>6} {:>5} {:>8}",
        "Summary", "Rel", "Acc", "Age(d)"
    );
    println!("{}", "-".repeat(85));

    let now = now_unix();
    for t in &roots {
        let age_days = (now - t.created_at) / 86400;
        println!(
            "{:<60} {:.3}  {:>4}  {:>5}",
            &t.content[..t.content.len().min(60)],
            t.relevance_score,
            t.access_count,
            age_days,
        );
    }

    println!("\nTotal roots: {}", roots.len());
}
