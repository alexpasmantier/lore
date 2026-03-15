//! Integration tests with fixture conversations.
//!
//! Each scenario simulates a full lifecycle: parse conversations, store extracted
//! knowledge, simulate time passage, run consolidation, query, and verify that the
//! brain-inspired memory system behaves correctly at a system level.

use lore_daemon::config::ConsolidationConfig;
use lore_daemon::consolidation;
use lore_daemon::ingestion::{store_extraction_result, ExtractionResult};
use lore_daemon::parser::parse_jsonl_line;
use lore_db::edge::EdgeKind;
use lore_db::fragment::{now_unix, Fragment};
use lore_db::relevance::{compute_relevance, MIN_RELEVANCE_THRESHOLD};
use lore_db::storage::Storage;
use lore_db::LoreDb;

// ════════════════════════════════════════════════════════════════════════
// FIXTURE CONVERSATIONS (realistic Claude Code JSONL)
// ════════════════════════════════════════════════════════════════════════

/// Conversation about Rust error handling patterns — architectural knowledge.
const CONVERSATION_RUST_ERRORS: &str = r#"{"type":"user","message":{"role":"user","content":"What's the best way to handle errors in Rust? I'm torn between using anyhow everywhere vs thiserror."}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"For library code, use `thiserror` to define structured error types. For application code and scripts, `anyhow` is more ergonomic. The key architectural decision is: will callers need to match on specific error variants? If yes, use thiserror. If they just need to propagate and display, use anyhow."}]}}
{"type":"user","message":{"role":"user","content":"Makes sense. We're building a library that other teams will consume, so thiserror it is. Any gotchas?"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"The user is building a library for internal consumption. Key gotchas with thiserror: 1) Don't expose third-party error types in your public API (they become semver-coupled). 2) Use #[from] carefully - it can create ambiguous From impls. 3) Consider boxing large error variants to keep the error type small."},{"type":"text","text":"Three gotchas to watch for:\n\n1. Don't expose third-party errors in your public API — creates semver coupling.\n2. Be careful with #[from] — can cause ambiguous From impls.\n3. Box large error variants to avoid inflating Result size."}]}}"#;

/// Debugging session with tool calls that should be skipped, plus a user correction.
const CONVERSATION_DEBUGGING: &str = r#"{"type":"user","message":{"role":"user","content":"The server is panicking on startup with 'already borrowed: BorrowMutError'. Can you look at server.rs?"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tool_1","name":"read_file","input":{"path":"server.rs"}},{"type":"text","text":"I see the issue. You have a RefCell<Connection> being borrowed immutably for a query while another part of the code tries to borrow it mutably. The fix is to switch to Mutex for clearer semantics."}]}}
{"type":"user","message":{"role":"user","content":"Actually, don't use Mutex for single-threaded code. RefCell is fine, we just need to restructure the borrows."}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"You're right — Mutex adds unnecessary overhead for single-threaded code. Let me restructure the borrows instead."}]}}"#;

/// Conversation about async patterns — moderate importance.
#[allow(dead_code)]
const CONVERSATION_ASYNC_PATTERNS: &str = r#"{"type":"user","message":{"role":"user","content":"How should we handle graceful shutdown in our tokio application?"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Use tokio::signal::ctrl_c() combined with a watch channel. The main loop selects on both work and the shutdown signal. Finish current work items but stop accepting new ones."}]}}
{"type":"user","message":{"role":"user","content":"What about long-running tasks that can't be interrupted?"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"For long-running tasks, pass a CancellationToken and check it periodically. The task cooperatively yields when cancelled. Never use task::abort() — it can leave resources in an inconsistent state."}]}}"#;

/// Low-value chatter — should extract nothing.
const CONVERSATION_CHATTER: &str = r#"{"type":"user","message":{"role":"user","content":"Hey, how are you?"}}
{"type":"assistant","message":{"role":"assistant","content":"I'm ready to help! What are you working on today?"}}
{"type":"user","message":{"role":"user","content":"Nothing specific, just checking in."}}
{"type":"assistant","message":{"role":"assistant","content":"No problem! Let me know if you need anything."}}"#;

/// Conversation with only tool calls and no text — should yield zero turns.
const CONVERSATION_TOOL_ONLY: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"read_file","input":{"path":"foo.rs"}}]}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_result","tool_use_id":"t1","content":"file contents here"}]}}"#;

// ════════════════════════════════════════════════════════════════════════
// FIXTURE EXTRACTIONS (simulated Claude extraction output)
// ════════════════════════════════════════════════════════════════════════

fn make_result(trees: Vec<Vec<String>>) -> ExtractionResult {
    ExtractionResult {
        transcript: "test conversation transcript".to_string(),
        trees,
        relationships: vec![],
    }
}

fn extraction_rust_errors() -> ExtractionResult {
    make_result(vec![vec![
        "Rust error handling architecture: For library code, use thiserror to define structured error types with \
         specific variants callers can match on. For application code, use anyhow \
         for ergonomic error propagation. The choice depends on whether callers \
         need to match on specific error variants."
            .to_string(),
        "thiserror gotchas for library authors: Three key gotchas: (1) Don't expose third-party error types in \
         public API — creates semver coupling. (2) Be careful with #[from] — \
         can create ambiguous From impls. (3) Box large error variants to \
         avoid inflating Result size."
            .to_string(),
    ]])
}

fn extraction_debugging() -> ExtractionResult {
    make_result(vec![vec![
        "RefCell vs Mutex usage: User correction: Don't use Mutex for single-threaded code. RefCell is \
         appropriate for single-threaded interior mutability. When encountering \
         BorrowMutError, restructure borrows to avoid overlapping immutable and \
         mutable borrows."
            .to_string(),
        "RefCell BorrowMutError debugging: Classic pitfall: RefCell runtime borrow checking can cause \
         BorrowMutError when immutable and mutable borrows overlap. Fix by \
         scoping borrows or dropping the immutable borrow before taking a \
         mutable one."
            .to_string(),
    ]])
}

fn extraction_async_patterns() -> ExtractionResult {
    make_result(vec![vec![
        "Tokio graceful shutdown pattern: Use tokio::signal::ctrl_c() with a watch channel for graceful shutdown. \
         Main loop selects on work and shutdown signal. Finish current work items \
         but stop accepting new ones."
            .to_string(),
        "CancellationToken for long tasks: For long-running tasks, pass a CancellationToken and check it \
         periodically. Never use task::abort() — it can leave resources \
         in an inconsistent state."
            .to_string(),
    ]])
}

fn extraction_chatter() -> ExtractionResult {
    make_result(vec![])
}

// ════════════════════════════════════════════════════════════════════════
// TEST HELPERS
// ════════════════════════════════════════════════════════════════════════

fn test_db() -> LoreDb {
    let storage = Storage::open_memory().unwrap();
    LoreDb::new_without_embeddings(storage)
}

fn default_consolidation_config() -> ConsolidationConfig {
    ConsolidationConfig::default()
}

/// Filter out low-importance transcript fragments from a list of roots.
/// Transcript fragments are stored at depth 0 with importance 0.1.
fn knowledge_roots(roots: Vec<Fragment>) -> Vec<Fragment> {
    roots
        .into_iter()
        .filter(|f| (f.importance - 0.1).abs() > 0.01)
        .collect()
}

/// Store extracted knowledge and return the DB for further assertions.
fn ingest_all(db: &LoreDb) {
    store_extraction_result(db, &extraction_rust_errors(), Some("session-rust-errors")).unwrap();
    store_extraction_result(db, &extraction_debugging(), Some("session-debugging")).unwrap();
    store_extraction_result(
        db,
        &extraction_async_patterns(),
        Some("session-async-patterns"),
    )
    .unwrap();
    store_extraction_result(db, &extraction_chatter(), Some("session-chatter")).unwrap();
}

/// Parse a fixture JSONL string and return the turns.
fn parse_fixture(jsonl: &str) -> Vec<lore_daemon::parser::ConversationTurn> {
    jsonl.lines().filter_map(parse_jsonl_line).collect()
}

// ════════════════════════════════════════════════════════════════════════
// SCENARIO 1: Conversation Parsing Fidelity
// ════════════════════════════════════════════════════════════════════════

#[test]
fn parsing_extracts_text_and_thinking_blocks() {
    let turns = parse_fixture(CONVERSATION_RUST_ERRORS);
    // 2 user messages + 2 assistant messages
    assert_eq!(
        turns.len(),
        4,
        "Should parse 4 turns from rust errors conversation"
    );
    assert_eq!(turns[0].role, "user");
    assert!(turns[0].text.contains("anyhow"));

    // The assistant's second message has a thinking block (>50 chars) + text block
    let last_assistant = &turns[3];
    assert_eq!(last_assistant.role, "assistant");
    assert!(
        last_assistant.text.contains("[Reasoning:"),
        "Thinking blocks >50 chars should be extracted"
    );
    assert!(
        last_assistant.text.contains("semver"),
        "Text blocks should be extracted"
    );
}

#[test]
fn parsing_skips_tool_calls() {
    let turns = parse_fixture(CONVERSATION_DEBUGGING);
    // User 1, Assistant (tool_use + text = text only), User 2, Assistant 2
    // The tool_use block is skipped, but the text block in the same message is kept
    assert_eq!(
        turns.len(),
        4,
        "Should parse 4 turns (tool blocks skipped, text kept)"
    );

    // The assistant's first message had tool_use + text — only text should appear
    let assistant_1 = &turns[1];
    assert!(assistant_1.text.contains("RefCell"));
    assert!(
        !assistant_1.text.contains("tool_use"),
        "Tool use content should not appear"
    );
}

#[test]
fn parsing_handles_string_content() {
    let turns = parse_fixture(CONVERSATION_CHATTER);
    assert_eq!(turns.len(), 4, "String content messages should parse");
    assert_eq!(
        turns[1].text,
        "I'm ready to help! What are you working on today?"
    );
}

#[test]
fn parsing_yields_nothing_for_tool_only_messages() {
    let turns = parse_fixture(CONVERSATION_TOOL_ONLY);
    assert_eq!(
        turns.len(),
        0,
        "Messages with only tool blocks should be skipped"
    );
}

// ════════════════════════════════════════════════════════════════════════
// SCENARIO 2: Multi-Session Knowledge Accumulation
// ════════════════════════════════════════════════════════════════════════

#[test]
fn multi_session_creates_correct_topic_hierarchy() {
    let db = test_db();
    ingest_all(&db);

    // 3 knowledge roots at depth 0 (transcripts are at max depth, not 0)
    let all_roots = db.list_roots(None);
    assert_eq!(
        all_roots.len(),
        3,
        "Should have 3 depth-0 knowledge roots from 3 non-empty sessions"
    );
}

#[test]
fn children_are_stored_with_correct_depth() {
    let db = test_db();
    ingest_all(&db);

    let topics = db.list_roots(None);
    for topic in &topics {
        assert_eq!(topic.depth, 0, "Topics should be at depth 0");
        let children = db.children(topic.id);
        for child in &children {
            assert_eq!(child.depth, 1, "Direct children should be at depth 1");
        }
    }
}

#[test]
fn importance_is_set_correctly_from_extraction() {
    let db = test_db();
    store_extraction_result(&db, &extraction_rust_errors(), Some("test")).unwrap();

    let topics = knowledge_roots(db.list_roots(None));
    assert_eq!(topics.len(), 1);
    let topic = &topics[0];
    // Root (depth 0) gets "high" importance → 0.9
    assert!(
        (topic.importance - 0.9).abs() < 0.01,
        "Root importance should map to 0.9, got {}",
        topic.importance
    );

    let children = db.children(topic.id);
    assert_eq!(children.len(), 1);
    // Leaf in a 2-level tree gets "medium" importance → 0.5
    assert!(
        (children[0].importance - 0.5).abs() < 0.01,
        "Leaf importance should map to 0.5, got {}",
        children[0].importance
    );
}

#[test]
fn source_session_is_recorded() {
    let db = test_db();
    store_extraction_result(&db, &extraction_rust_errors(), Some("my-project-abc123")).unwrap();

    let topics = db.list_roots(None);
    assert_eq!(
        topics[0].source_session.as_deref(),
        Some("my-project-abc123"),
        "Source session should be recorded on fragments"
    );
}

#[test]
fn empty_extraction_stores_nothing() {
    let db = test_db();
    let count = store_extraction_result(&db, &extraction_chatter(), Some("chatter")).unwrap();
    assert_eq!(count, 0);
    assert_eq!(db.list_roots(None).len(), 0);
}

// ════════════════════════════════════════════════════════════════════════
// SCENARIO 3: Memory Lifecycle Over Simulated Months
// ════════════════════════════════════════════════════════════════════════

#[test]
fn high_importance_memories_survive_months() {
    let db = test_db();
    ingest_all(&db);
    let day = 86400i64;
    let now = now_unix();

    // Record initial relevance of the high-importance topic
    let topics_before = db.list_roots(None);
    let high_imp_topic = topics_before
        .iter()
        .find(|t| t.content.contains("Rust error handling architecture"))
        .unwrap();
    let initial_relevance = high_imp_topic.relevance_score;

    // Simulate 90 days passing
    let future = now + 90 * day;
    db.storage().recompute_all_relevance(future).unwrap();

    let topics_after = db.list_roots(None);
    let high_imp_after = topics_after
        .iter()
        .find(|t| t.content.contains("Rust error handling architecture"))
        .unwrap();

    // High importance (0.9) should still be above the visibility threshold
    assert!(
        high_imp_after.relevance_score > MIN_RELEVANCE_THRESHOLD,
        "High-importance memory should survive 90 days, got relevance {}",
        high_imp_after.relevance_score
    );

    // But it should have decayed
    assert!(
        high_imp_after.relevance_score < initial_relevance,
        "Memory should decay even with high importance"
    );
}

#[test]
fn low_importance_leaf_fades_over_time() {
    let db = test_db();
    store_extraction_result(&db, &extraction_async_patterns(), Some("test")).unwrap();

    let day = 86400i64;
    let now = now_unix();

    // The leaf (depth 1) gets "medium" importance (0.5) in a 2-level tree
    let topics = knowledge_roots(db.list_roots(None));
    assert_eq!(topics.len(), 1);
    let children = db.children(topics[0].id);
    assert_eq!(children.len(), 1);
    let leaf = &children[0];
    assert!(
        (leaf.importance - 0.5).abs() < 0.01,
        "Leaf importance should be 0.5 (medium), got {}",
        leaf.importance
    );

    // After 180 days, the medium-importance leaf should have decayed significantly
    let future = now + 180 * day;
    db.storage().recompute_all_relevance(future).unwrap();

    let leaf_after = db.storage().get_fragment(leaf.id).unwrap().unwrap();

    // The high-importance root should still be much more visible than the leaf
    let root_after = db.storage().get_fragment(topics[0].id).unwrap().unwrap();
    assert!(
        root_after.relevance_score > leaf_after.relevance_score,
        "High importance root ({}) should outrank medium importance leaf ({}) after 180 days",
        root_after.relevance_score,
        leaf_after.relevance_score
    );
}

#[test]
fn access_rescues_a_fading_memory() {
    let db = test_db();
    ingest_all(&db);

    let day = 86400i64;
    let now = now_unix();

    // Let 60 days pass
    let t60 = now + 60 * day;
    db.storage().recompute_all_relevance(t60).unwrap();

    // Find the Tokio shutdown topic
    let topic = db
        .list_roots(None)
        .into_iter()
        .find(|t| t.content.contains("Tokio graceful shutdown pattern"))
        .unwrap();
    let relevance_at_60d = topic.relevance_score;

    // Simulate an access at day 60: reinforce it
    let new_rel = compute_relevance(
        topic.importance,
        topic.access_count + 1,
        topic.decay_rate,
        t60,
        t60,
    );
    db.storage()
        .reinforce_fragment(topic.id, t60, new_rel)
        .unwrap();
    let after_access = db.storage().get_fragment(topic.id).unwrap().unwrap();

    assert!(
        after_access.relevance_score > relevance_at_60d,
        "Access should boost fading memory: {} > {}",
        after_access.relevance_score,
        relevance_at_60d
    );

    // After another 30 days (day 90), the accessed memory decays from day 60, not day 0
    let t90 = now + 90 * day;
    db.storage().recompute_all_relevance(t90).unwrap();
    let at_90d = db.storage().get_fragment(topic.id).unwrap().unwrap();

    // It should have decayed only 30 days worth (since reinforcement reset the timer)
    let expected_if_no_access = compute_relevance(
        topic.importance,
        0,
        topic.decay_rate,
        now, // original creation
        t90,
    );
    assert!(
        at_90d.relevance_score > expected_if_no_access,
        "Accessed memory at day 90 ({}) should be stronger than if never accessed ({})",
        at_90d.relevance_score,
        expected_if_no_access
    );
}

// ════════════════════════════════════════════════════════════════════════
// SCENARIO 4: Reconsolidation Cascade Through Graph
// ════════════════════════════════════════════════════════════════════════

#[test]
fn query_reinforces_and_spreads_activation() {
    let db = test_db();
    ingest_all(&db);

    let day = 86400i64;
    let now = now_unix();

    // Age everything by 30 days so there's visible decay
    let t30 = now + 30 * day;
    db.storage().recompute_all_relevance(t30).unwrap();

    // Get the Rust error handling topic and its child
    let topics = db.list_roots(None);
    let rust_topic = topics
        .iter()
        .find(|t| t.content.contains("Rust error handling architecture"))
        .unwrap();
    let children = db.children(rust_topic.id);
    let child = &children[0]; // thiserror gotchas

    let child_relevance_before = db
        .storage()
        .get_fragment(child.id)
        .unwrap()
        .unwrap()
        .relevance_score;

    // Query for "rust error" — this should match and reinforce the topic
    let results = db.query("rust error handling", 10);
    assert!(!results.is_empty(), "Should find rust error handling topic");

    // The topic itself should be reinforced (access_count increased)
    let topic_after = db.storage().get_fragment(rust_topic.id).unwrap().unwrap();
    assert!(
        topic_after.access_count > rust_topic.access_count,
        "Query should increment access count"
    );

    // Child should get a spreading activation boost via hierarchical edge
    let child_after = db.storage().get_fragment(child.id).unwrap().unwrap();
    assert!(
        child_after.relevance_score > child_relevance_before,
        "Child should get spreading activation boost: {} > {}",
        child_after.relevance_score,
        child_relevance_before
    );
}

#[test]
fn associative_links_propagate_activation() {
    let db = test_db();
    ingest_all(&db);

    let day = 86400i64;
    let now = now_unix();

    // Age everything by 30 days so fragments have room to be boosted (not at 1.0)
    let t30 = now + 30 * day;
    db.storage().recompute_all_relevance(t30).unwrap();

    // Create an associative link between two topics
    let topics = db.list_roots(None);
    let rust_topic = topics
        .iter()
        .find(|t| t.content.contains("Rust error handling architecture"))
        .unwrap();
    let refcell_topic = topics
        .iter()
        .find(|t| t.content.contains("RefCell vs Mutex usage"))
        .unwrap();

    db.link(rust_topic.id, refcell_topic.id, EdgeKind::Associative, 0.7)
        .unwrap();

    let refcell_before = db
        .storage()
        .get_fragment(refcell_topic.id)
        .unwrap()
        .unwrap()
        .relevance_score;

    // Query for "rust error" — triggers reconsolidation on rust_topic
    let _ = db.query("rust error", 5);

    // RefCell topic should get a boost via the associative edge
    let refcell_after = db
        .storage()
        .get_fragment(refcell_topic.id)
        .unwrap()
        .unwrap();
    assert!(
        refcell_after.relevance_score > refcell_before,
        "Associated topic should get activation spread: {} > {}",
        refcell_after.relevance_score,
        refcell_before
    );
}

// ════════════════════════════════════════════════════════════════════════
// SCENARIO 6: Forgetting and Pruning Lifecycle
// ════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn consolidation_recomputes_relevance_for_all_fragments() {
    let db = test_db();
    ingest_all(&db);

    let config = default_consolidation_config();
    let stats = consolidation::run_consolidation(&db, None, None, &config)
        .await
        .unwrap();

    // Should have recomputed relevance for all fragments
    // 3 trees × 2 levels each = 6 knowledge fragments + 3 transcript fragments = 9 total
    assert_eq!(
        stats.relevance_updated, 9,
        "Should recompute all 9 fragments (6 knowledge + 3 transcripts)"
    );
}

#[tokio::test]
async fn old_low_importance_fragments_are_pruned_by_consolidation() {
    let db = test_db();
    let day = 86400i64;
    let now = now_unix();

    // Insert a low-importance child that's very old
    let topic = Fragment::new_with_importance("Topic that stays".to_string(), 0, 0.5);
    db.insert(topic.clone(), None).unwrap();

    let mut old_child = Fragment::new_with_importance(
        "Old trivial detail".to_string(),
        1,
        0.02, // near-zero importance → floor = 0.006, below pruning threshold of 0.02
    );
    old_child.created_at = now - 120 * day; // 120 days old
    old_child.last_accessed = now - 120 * day;
    old_child.last_reinforced = now - 120 * day;
    old_child.access_count = 0; // never accessed
    old_child.relevance_score = compute_relevance(
        old_child.importance,
        0,
        old_child.decay_rate,
        old_child.last_reinforced,
        now,
    );
    db.storage().insert_fragment(&old_child).unwrap();
    db.link(topic.id, old_child.id, EdgeKind::Hierarchical, 1.0)
        .unwrap();

    // Also insert a fresh child that should survive
    let fresh_child = Fragment::new_with_importance("Fresh important detail".to_string(), 1, 0.9);
    db.insert(fresh_child.clone(), Some(topic.id)).unwrap();

    // Verify the old child has very low relevance (floor = 0.02 * 0.3 = 0.006)
    assert!(
        old_child.relevance_score < 0.02,
        "120-day old near-zero-importance should have very low relevance: {}",
        old_child.relevance_score
    );

    // Run consolidation
    let config = default_consolidation_config();
    let stats = consolidation::run_consolidation(&db, None, None, &config)
        .await
        .unwrap();

    // The old trivial child should be pruned
    assert!(
        stats.fragments_pruned >= 1,
        "Should prune at least the old trivial fragment"
    );

    // Verify it's actually gone
    let remaining = db.storage().get_fragment(old_child.id).unwrap();
    assert!(
        remaining.is_none(),
        "Pruned fragment should be deleted from database"
    );

    // Fresh child should survive
    let fresh_remaining = db.storage().get_fragment(fresh_child.id).unwrap();
    assert!(
        fresh_remaining.is_some(),
        "Fresh child should survive pruning"
    );

    // Topic should survive (depth-0 never pruned)
    let topic_remaining = db.storage().get_fragment(topic.id).unwrap();
    assert!(topic_remaining.is_some(), "Depth-0 topics are never pruned");
}

#[tokio::test]
async fn pruned_fragments_children_are_reparented() {
    let db = test_db();
    let day = 86400i64;
    let now = now_unix();

    // Create: topic → middle_node → grandchild
    let topic = Fragment::new_with_importance("Root topic".to_string(), 0, 0.9);
    db.insert(topic.clone(), None).unwrap();

    let mut middle = Fragment::new_with_importance(
        "Middle node that will be pruned".to_string(),
        1,
        0.02, // near-zero importance → floor below pruning threshold
    );
    middle.created_at = now - 100 * day;
    middle.last_accessed = now - 100 * day;
    middle.last_reinforced = now - 100 * day;
    middle.access_count = 0;
    middle.relevance_score =
        compute_relevance(0.02, 0, middle.decay_rate, middle.last_reinforced, now);
    db.storage().insert_fragment(&middle).unwrap();
    db.link(topic.id, middle.id, EdgeKind::Hierarchical, 1.0)
        .unwrap();

    let grandchild = Fragment::new_with_importance(
        "Grandchild should be reparented".to_string(),
        2,
        0.9, // high importance so it survives
    );
    db.insert(grandchild.clone(), Some(middle.id)).unwrap();

    // Run consolidation — middle should be pruned
    let config = default_consolidation_config();
    consolidation::run_consolidation(&db, None, None, &config)
        .await
        .unwrap();

    // Middle should be gone
    assert!(db.storage().get_fragment(middle.id).unwrap().is_none());

    // Grandchild should be reparented to topic
    let topic_children = db.children(topic.id);
    assert!(
        topic_children.iter().any(|c| c.id == grandchild.id),
        "Grandchild should be reparented to the root topic"
    );
}

// ════════════════════════════════════════════════════════════════════════
// SCENARIO 7: Edge Decay Over Consolidation Cycles
// ════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn associative_edges_decay_over_multiple_consolidation_cycles() {
    let db = test_db();
    ingest_all(&db);

    // Create an associative link
    let topics = db.list_roots(None);
    let (a, b) = (topics[0].id, topics[1].id);
    db.link(a, b, EdgeKind::Associative, 1.0).unwrap();

    // Run consolidation 10 times
    let config = default_consolidation_config();
    for _ in 0..10 {
        consolidation::run_consolidation(&db, None, None, &config)
            .await
            .unwrap();
    }

    // Check the edge weight: 1.0 * 0.95^10 ≈ 0.5987
    let edges = db.storage().get_edges_for(a).unwrap();
    let assoc_edge = edges
        .iter()
        .find(|e| e.kind == EdgeKind::Associative)
        .unwrap();
    let expected = 0.95f32.powi(10);
    assert!(
        (assoc_edge.weight - expected).abs() < 0.02,
        "Edge should have decayed to ~{:.3}, got {:.3}",
        expected,
        assoc_edge.weight
    );
}

#[tokio::test]
async fn weak_associative_edges_get_pruned() {
    let db = test_db();
    ingest_all(&db);

    // Create a weak associative link that should get pruned quickly
    let topics = knowledge_roots(db.list_roots(None));
    let (a, b) = (topics[0].id, topics[1].id);
    db.link(a, b, EdgeKind::Associative, 0.2).unwrap();

    // Run consolidation — each cycle decays by 5%, and pruning threshold is 0.15
    // 0.2 * 0.95 = 0.19 (survives)
    // 0.19 * 0.95 = 0.1805 (survives)
    // 0.1805 * 0.95 = 0.1715 (survives)
    // 0.1715 * 0.95 = 0.1629 (survives)
    // 0.1629 * 0.95 = 0.1548 (survives)
    // 0.1548 * 0.95 = 0.1470 → pruned!
    let config = default_consolidation_config();
    let mut total_pruned = 0;
    for _ in 0..10 {
        let stats = consolidation::run_consolidation(&db, None, None, &config)
            .await
            .unwrap();
        total_pruned += stats.edges_pruned;
    }

    assert!(total_pruned >= 1, "Weak edge should eventually get pruned");

    // Verify the edge is gone
    let edges = db.storage().get_edges_for(a).unwrap();
    let assoc_edges: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Associative)
        .collect();
    assert_eq!(assoc_edges.len(), 0, "Pruned edge should be gone");
}

#[tokio::test]
async fn hierarchical_edges_are_immune_to_decay() {
    let db = test_db();
    ingest_all(&db);

    let topics = db.list_roots(None);
    let topic_with_children = topics
        .iter()
        .find(|t| !db.children(t.id).is_empty())
        .unwrap();
    let children_before = db.children(topic_with_children.id);

    // Run consolidation 20 times
    let config = default_consolidation_config();
    for _ in 0..20 {
        consolidation::run_consolidation(&db, None, None, &config)
            .await
            .unwrap();
    }

    // Hierarchical edges should be completely unaffected
    let children_after = db.children(topic_with_children.id);
    assert_eq!(
        children_before.len(),
        children_after.len(),
        "Hierarchical edges should never be decayed or pruned"
    );
}

// ════════════════════════════════════════════════════════════════════════
// SCENARIO 8: Full Pipeline End-to-End
// ════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn full_lifecycle_ingest_age_consolidate_query_repeat() {
    let db = test_db();
    let day = 86400i64;
    let now = now_unix();

    // ── Day 0: Ingest knowledge from multiple sessions ──
    ingest_all(&db);

    let initial_topics = knowledge_roots(db.list_roots(None));
    assert_eq!(initial_topics.len(), 3);

    // All knowledge fragments start with full relevance
    for topic in &initial_topics {
        assert!(
            topic.relevance_score > 0.5,
            "Fresh fragments should have high relevance"
        );
    }

    // ── Day 30: First consolidation cycle ──
    let t30 = now + 30 * day;
    db.storage().recompute_all_relevance(t30).unwrap();

    let topics_at_30d = knowledge_roots(db.list_roots(None));
    // All should have decayed but still be visible
    for topic in &topics_at_30d {
        assert!(
            topic.relevance_score > MIN_RELEVANCE_THRESHOLD,
            "At 30 days, all topics should still be visible: {} has {}",
            topic.content,
            topic.relevance_score
        );
    }

    // ── Day 30: Query "rust" — this reinforces matching memories ──
    let results = db.query("rust error", 10);
    assert!(!results.is_empty());

    // (The query above reinforced the Rust topic — its decay timer resets to now)

    // ── Day 60: Second round ──
    let t60 = now + 60 * day;
    db.storage().recompute_all_relevance(t60).unwrap();

    // The Rust topic was reinforced at day 30, so it decays from day 30, not day 0
    // Other topics decay from day 0 (60 days of decay)
    let topics_at_60d = knowledge_roots(db.list_roots(None));
    let rust_at_60 = topics_at_60d
        .iter()
        .find(|t| t.content.contains("Rust"))
        .unwrap();
    let tokio_at_60 = topics_at_60d
        .iter()
        .find(|t| t.content.contains("Tokio"))
        .unwrap();

    // Rust topic (recently accessed at day 30) should outrank
    // Tokio topic (never accessed, decaying since day 0)
    assert!(
        rust_at_60.relevance_score > tokio_at_60.relevance_score,
        "Recently accessed Rust topic ({:.3}) should outrank untouched Tokio topic ({:.3})",
        rust_at_60.relevance_score,
        tokio_at_60.relevance_score
    );

    // ── Day 90: Run full consolidation ──
    let t90 = now + 90 * day;
    db.storage().recompute_all_relevance(t90).unwrap();

    let config = default_consolidation_config();
    let stats = consolidation::run_consolidation(&db, None, None, &config)
        .await
        .unwrap();

    // Verify consolidation ran all phases
    assert!(stats.relevance_updated > 0, "Phase 0 should run");

    // High importance topics should still be visible even at 90 days
    let topics_at_90d = knowledge_roots(db.list_roots(None));
    let rust_at_90 = topics_at_90d
        .iter()
        .find(|t| t.content.contains("Rust"))
        .unwrap();
    assert!(
        rust_at_90.relevance_score > MIN_RELEVANCE_THRESHOLD,
        "High-importance, recently-accessed topic should survive 90 days"
    );

    // ── Verify the full ordering makes sense ──
    // Most relevant should be the recently-accessed, high-importance Rust topic
    assert!(
        topics_at_90d[0].content.contains("Rust") || topics_at_90d[0].content.contains("RefCell"),
        "Top-ranked topic at 90 days should be one of the high-importance ones, got: {}",
        topics_at_90d[0].content
    );
}

#[tokio::test]
async fn knowledge_from_different_sessions_builds_unified_graph() {
    let db = test_db();
    let day = 86400i64;
    let now = now_unix();

    // Ingest from session 1
    store_extraction_result(&db, &extraction_rust_errors(), Some("session-1")).unwrap();

    // Ingest from session 2
    store_extraction_result(&db, &extraction_debugging(), Some("session-2")).unwrap();

    // Create an associative link between related knowledge topics
    let topics = knowledge_roots(db.list_roots(None));
    assert_eq!(topics.len(), 2);
    db.link(topics[0].id, topics[1].id, EdgeKind::Associative, 0.8)
        .unwrap();

    // Query one topic — should trigger spreading activation to the other
    let _ = db.query("error handling", 5);

    // Age by 30 days and consolidate
    let t30 = now + 30 * day;
    db.storage().recompute_all_relevance(t30).unwrap();
    let config = default_consolidation_config();
    consolidation::run_consolidation(&db, None, None, &config)
        .await
        .unwrap();

    // Both knowledge topics should still be present
    let topics_after = knowledge_roots(db.list_roots(None));
    assert_eq!(topics_after.len(), 2);

    // The graph should have hierarchical edges (topic→children) and
    // associative edge (topic↔topic)
    for topic in &topics_after {
        let edges = db.storage().get_edges_for(topic.id).unwrap();
        assert!(
            !edges.is_empty(),
            "Each topic should have edges (hierarchical and/or associative)"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════
// SCENARIO 9: Supersession and Knowledge Evolution
// ════════════════════════════════════════════════════════════════════════

#[test]
fn superseded_knowledge_is_invisible_but_retained() {
    let db = test_db();

    // Ingest initial knowledge
    store_extraction_result(&db, &extraction_rust_errors(), Some("v1")).unwrap();

    let topics_v1 = db.list_roots(None);
    let old_topic_id = topics_v1[0].id;

    // Ingest updated knowledge as a new tree (simulating contradiction resolution)
    let updated = make_result(vec![vec![
        "Rust error handling (revised): Use thiserror for all library AND binary crates for consistency. \
         Anyhow is no longer recommended due to opaque error types."
            .to_string(),
    ]]);
    store_extraction_result(&db, &updated, Some("v2")).unwrap();

    let all_topics = db.list_roots(None);
    let new_topic = all_topics
        .iter()
        .find(|t| t.content.contains("revised"))
        .unwrap();

    // Supersede old with new
    db.supersede(old_topic_id, new_topic.id).unwrap();

    // Old topic should not appear in list_roots
    let visible_topics = db.list_roots(None);
    assert!(
        !visible_topics.iter().any(|t| t.id == old_topic_id),
        "Superseded topic should be invisible"
    );

    // But it should still exist in storage
    let old_in_storage = db.storage().get_fragment(old_topic_id).unwrap();
    assert!(
        old_in_storage.is_some(),
        "Superseded topic should still exist in storage"
    );
    assert_eq!(
        old_in_storage.unwrap().superseded_by,
        Some(new_topic.id),
        "Should point to its successor"
    );
}

// ════════════════════════════════════════════════════════════════════════
// SCENARIO 10: Decay Rate Correctness Across Importance Levels
// ════════════════════════════════════════════════════════════════════════

#[test]
fn importance_levels_produce_correct_decay_behavior() {
    let db = test_db();
    let day = 86400i64;
    let now = now_unix();

    // Create a 3-level tree: root (high=0.9), middle (medium=0.7), leaf (0.5)
    let result = make_result(vec![vec![
        "Critical decision: A critical architectural decision.".to_string(),
        "Technical pattern: A useful technical pattern.".to_string(),
        "Routine observation: A routine observation.".to_string(),
    ]]);
    store_extraction_result(&db, &result, Some("test")).unwrap();

    let topics = knowledge_roots(db.list_roots(None));
    assert_eq!(topics.len(), 1);
    let high = &topics[0];
    assert!(high.content.contains("Critical decision"));

    let mid_children = db.children(high.id);
    assert_eq!(mid_children.len(), 1);
    let med = &mid_children[0];
    assert!(med.content.contains("Technical pattern"));

    let leaf_children = db.children(med.id);
    assert_eq!(leaf_children.len(), 1);
    let low = &leaf_children[0];
    assert!(low.content.contains("Routine observation"));

    // Verify decay rates: high importance decays slower
    assert!(
        high.decay_rate < med.decay_rate,
        "High importance should decay slower"
    );
    assert!(
        med.decay_rate < low.decay_rate,
        "Medium importance should decay slower than low"
    );

    // Verify importance values: 3-level tree → root=0.9, middle=0.7, leaf=0.5
    assert!((high.importance - 0.9).abs() < 0.01);
    assert!((med.importance - 0.7).abs() < 0.01);
    assert!((low.importance - 0.5).abs() < 0.01);

    // After 60 days, the ordering should be high > medium > low
    let t60 = now + 60 * day;
    db.storage().recompute_all_relevance(t60).unwrap();

    let high_60 = db.storage().get_fragment(high.id).unwrap().unwrap();
    let med_60 = db.storage().get_fragment(med.id).unwrap().unwrap();
    let low_60 = db.storage().get_fragment(low.id).unwrap().unwrap();

    assert!(
        high_60.relevance_score > med_60.relevance_score,
        "High ({:.3}) > Medium ({:.3}) after 60 days",
        high_60.relevance_score,
        med_60.relevance_score
    );
    assert!(
        med_60.relevance_score > low_60.relevance_score,
        "Medium ({:.3}) > Low ({:.3}) after 60 days",
        med_60.relevance_score,
        low_60.relevance_score
    );
}
