# Engram: Brain-Inspired Persistent Memory for AI Agents

## Context

Claude Code agents currently have no long-term memory across conversations. Each session starts from scratch. The built-in file-based memory system (CLAUDE.md / MEMORY.md) is primitive — flat files, manually maintained, no semantic understanding, no consolidation.

**Engram** is a brain-inspired memory system that gives agents persistent, queryable, hierarchically-organized long-term memory. A background daemon continuously ingests conversations and distills them into a semantic knowledge graph. Agents access this graph via MCP tools, querying at various depths like the brain recalls at different levels of abstraction.

## Key Decisions

- **Ingestion**: Single-pass Claude API extraction for v1 (one call extracts full hierarchy). Multi-agent iterative refinement deferred to v2.
- **Embeddings**: Local model via `fastembed` crate (`all-MiniLM-L6-v2`, 384-dim). No API costs, works offline.
- **MCP implementation**: Use the `rmcp` crate (Rust MCP SDK) rather than hand-rolling JSON-RPC.

## Architecture Overview

```
┌──────────────────────────────────────────────────┐
│                  Claude Code Agent                │
│  (queries memory via MCP tools during work)       │
└──────────┬───────────────────────────────────────┘
           │ stdio (JSON-RPC)
           ▼
┌──────────────────────┐     ┌─────────────────────┐
│   engram-mcp         │     │   engram-daemon      │
│   (MCP Server)       │     │   (Background)       │
│                      │     │                      │
│ Tools:               │     │ ┌─────────────────┐  │
│  • query_memory      │     │ │ Ingestion        │  │
│  • explore_memory    │     │ │ (polls convos,   │  │
│  • traverse_memory   │     │ │  extracts via    │  │
│  • store_memory      │     │ │  Claude API)     │  │
│  • list_topics       │     │ ├─────────────────┤  │
│                      │     │ │ Consolidation    │  │
│                      │     │ │ (merges, links,  │  │
│                      │     │ │  prunes, decays) │  │
│                      │     │ └─────────────────┘  │
└──────────┬───────────┘     └──────────┬──────────┘
           │ read                       │ read/write
           ▼                            ▼
      ┌─────────────────────────────────────┐
      │          ~/.engram/memory.db         │
      │          (SQLite + WAL mode)         │
      │                                      │
      │  Fragments (nodes with embeddings)   │
      │  Edges (hierarchical + associative)  │
      │  Ingestion watermarks                │
      └─────────────────────────────────────┘
```

## Component 1: engram-db (Core Library)

The brain-inspired graph database. All other components depend on this.

### Data Model

**Fragment** — A unit of knowledge (like a neuron ensemble encoding a concept):
```rust
struct Fragment {
    id: FragmentId,           // UUID
    content: String,          // The knowledge text
    summary: String,          // One-line summary for tree browsing
    depth: u32,               // 0=topic, 1=concept, 2=fact, 3+=detail
    embedding: Vec<f32>,      // Semantic vector (384-dim, all-MiniLM-L6-v2)
    created_at: i64,          // Unix timestamp
    last_accessed: i64,       // For decay/reinforcement
    access_count: u32,        // Frequency of retrieval
    source_session: Option<String>, // Which conversation produced this
    superseded_by: Option<FragmentId>, // If newer knowledge replaces this
    metadata: HashMap<String, String>,
    // Brain-inspired fields (v2)
    importance: f32,          // [0.0, 1.0] salience (high=0.9, medium=0.5, low=0.2)
    relevance_score: f32,     // Pre-computed composite score (Ebbinghaus curve)
    decay_rate: f32,          // Per-day exponential decay constant (derived from importance)
    last_reinforced: i64,     // Unix timestamp of last reinforcement (reset on access)
}
```

**Edge** — A connection between fragments:
```rust
enum EdgeKind {
    Hierarchical,  // Parent→child (tree structure)
    Associative,   // Cross-branch semantic link
    Temporal,      // Time-ordered within a topic
    Supersedes,    // Newer fragment replaces older
}

struct Edge {
    id: EdgeId,
    source: FragmentId,
    target: FragmentId,
    kind: EdgeKind,
    weight: f32,       // Strength of connection (0.0–1.0)
    created_at: i64,
}
```

### Depth Layers (inspired by cortical hierarchy)

| Depth | Role | Example |
|-------|------|---------|
| 0 | **Topic** — broad knowledge domain | "Rust async programming" |
| 1 | **Concept** — key idea within topic | "tokio runtime model" |
| 2 | **Fact** — specific piece of knowledge | "tokio uses work-stealing scheduler" |
| 3+ | **Detail** — deep specifics, code, decisions | "use `#[tokio::main(flavor = "multi_thread")]` for CPU-bound" |

### SQLite Schema

```sql
CREATE TABLE fragments (
    id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    summary TEXT NOT NULL,
    depth INTEGER NOT NULL,
    embedding BLOB,            -- f32 array stored as bytes
    created_at INTEGER NOT NULL,
    last_accessed INTEGER NOT NULL,
    access_count INTEGER DEFAULT 0,
    source_session TEXT,
    superseded_by TEXT REFERENCES fragments(id),
    metadata TEXT,              -- JSON
    -- V2: Brain-inspired columns (added via ALTER TABLE migration)
    importance REAL DEFAULT 0.5,
    relevance_score REAL DEFAULT 1.0,
    decay_rate REAL DEFAULT 0.035,
    last_reinforced INTEGER DEFAULT 0
);

CREATE TABLE edges (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL REFERENCES fragments(id),
    target TEXT NOT NULL REFERENCES fragments(id),
    kind TEXT NOT NULL,          -- 'hierarchical', 'associative', 'temporal', 'supersedes'
    weight REAL DEFAULT 1.0,
    created_at INTEGER NOT NULL
);

CREATE TABLE watermarks (
    file_path TEXT PRIMARY KEY,
    byte_offset INTEGER NOT NULL,
    last_processed INTEGER NOT NULL
);

CREATE INDEX idx_fragments_depth ON fragments(depth);
CREATE INDEX idx_fragments_superseded ON fragments(superseded_by) WHERE superseded_by IS NOT NULL;
CREATE INDEX idx_fragments_relevance ON fragments(relevance_score) WHERE superseded_by IS NULL;
CREATE INDEX idx_edges_source ON edges(source);
CREATE INDEX idx_edges_target ON edges(target);
CREATE INDEX idx_edges_kind ON edges(kind);
```

### Query API

```rust
impl EngramDb {
    /// Search by topic string, return fragments at specified depth.
    /// Uses embedding similarity to find relevant branches, then returns nodes at target depth.
    fn query(&self, topic: &str, depth: u32, limit: usize) -> Vec<ScoredFragment>;

    /// Get children of a specific node (walk down the tree).
    fn children(&self, id: FragmentId) -> Vec<Fragment>;

    /// Get parent of a node (walk up the tree).
    fn parent(&self, id: FragmentId) -> Option<Fragment>;

    /// Return full subtree rooted at a node, up to max_depth levels deep.
    fn subtree(&self, id: FragmentId, max_depth: u32) -> Tree<Fragment>;

    /// Explore a topic: find the best matching L0 node, return its subtree.
    fn explore(&self, topic: &str, max_depth: u32) -> Vec<Tree<Fragment>>;

    /// Pure semantic search across all fragments.
    fn search_semantic(&self, embedding: &[f32], top_k: usize) -> Vec<ScoredFragment>;

    /// List all top-level topics (L0 nodes) with summaries.
    fn list_topics(&self) -> Vec<Fragment>;

    /// Insert a fragment and connect it to parent.
    fn insert(&mut self, fragment: Fragment, parent: Option<FragmentId>) -> FragmentId;

    /// Create an edge between two fragments.
    fn link(&mut self, source: FragmentId, target: FragmentId, kind: EdgeKind, weight: f32);

    /// Mark a fragment as superseded by another.
    fn supersede(&mut self, old: FragmentId, new: FragmentId);

    /// Delete a fragment and its edges.
    fn prune(&mut self, id: FragmentId);
}
```

### Embedding Strategy

Use **fastembed-rs** with the `all-MiniLM-L6-v2` model (384 dimensions):
- Runs locally, no API calls, fast (~1ms per embedding)
- Good enough for semantic similarity in this context
- Model auto-downloads on first use (~80MB)
- Cosine similarity computed in Rust (trivial with SIMD)

Similarity search: brute-force cosine similarity over all fragments at the target depth. For <100K fragments this is sub-millisecond. Can add HNSW index later if needed.

## Component 2: engram-mcp (MCP Server)

Stdio-based MCP server that agents use to access memory.

### MCP Protocol

Uses the `rmcp` crate to implement the MCP server over stdin/stdout. The crate handles:
- `initialize` → capabilities handshake
- `tools/list` → enumerate available tools
- `tools/call` → execute a tool

We define a `MemoryServer` struct that implements `rmcp`'s `ServerHandler` trait, with `#[tool]` attribute macros on each tool method.

### Tools Exposed

**`query_memory`**
```json
{
  "name": "query_memory",
  "description": "Search long-term memory for knowledge about a topic. Returns fragments at the specified depth level (0=broad topics, 1=concepts, 2=facts, 3+=details). Start shallow and drill deeper as needed.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "topic": { "type": "string", "description": "What to search for" },
      "depth": { "type": "integer", "description": "Depth level (0=topics, 1=concepts, 2=facts, 3+=details)", "default": 1 },
      "limit": { "type": "integer", "description": "Max results", "default": 10 }
    },
    "required": ["topic"]
  }
}
```

**`explore_memory`**
```json
{
  "name": "explore_memory",
  "description": "Get a full subtree view of a knowledge area. Returns a hierarchical tree starting from the best matching topic, showing the structure of what is known.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "topic": { "type": "string" },
      "max_depth": { "type": "integer", "default": 2 }
    },
    "required": ["topic"]
  }
}
```

**`traverse_memory`**
```json
{
  "name": "traverse_memory",
  "description": "Navigate from a specific memory fragment. Get its children (drill deeper), parent (zoom out), or associated fragments (lateral connections).",
  "inputSchema": {
    "type": "object",
    "properties": {
      "fragment_id": { "type": "string" },
      "direction": { "type": "string", "enum": ["children", "parent", "associations"] }
    },
    "required": ["fragment_id", "direction"]
  }
}
```

**`store_memory`**
```json
{
  "name": "store_memory",
  "description": "Explicitly store a piece of knowledge in long-term memory. Provide the knowledge, a parent topic (or null for new topic), and depth level.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "content": { "type": "string" },
      "summary": { "type": "string" },
      "parent_id": { "type": ["string", "null"] },
      "depth": { "type": "integer", "default": 2 }
    },
    "required": ["content", "summary"]
  }
}
```

**`list_topics`**
```json
{
  "name": "list_topics",
  "description": "List all top-level knowledge domains in memory with their summaries and fragment counts.",
  "inputSchema": { "type": "object", "properties": {} }
}
```

### Implementation Notes

- Opens SQLite database in **read-only mode** (WAL allows concurrent readers)
- Exception: `store_memory` opens a brief write transaction
- Database path: `~/.engram/memory.db` (configurable via env `ENGRAM_DB_PATH`)
- Logging to stderr (MCP convention — stdout is protocol only)

## Component 3: engram-daemon (Background Process)

Long-running daemon with two concurrent subsystems.

### Subsystem A: Ingestion

**File watching:**
- Polls `~/.claude/projects/` recursively for `*.jsonl` files every 30 seconds (configurable)
- Maintains watermarks table: tracks (file_path, byte_offset) of what's been processed
- On each poll: seek to watermark offset, read new lines, process them

**Conversation parsing:**
- Each JSONL line has a `type` field: `"user"` or `"assistant"`
- User messages: `message.content` is either a string or array with `text` / `tool_result` blocks
- Assistant messages: `message.content` is an array with `text` / `tool_use` / `thinking` blocks
- Filter out: tool_use/tool_result noise, base64 signatures, pure tool calls
- Extract: user questions/instructions, assistant explanations/reasoning, key decisions

**Knowledge extraction pipeline:**
1. Batch new conversation turns (configurable batch size, e.g. 20 turns)
2. Send to Claude API with a structured extraction prompt:
   ```
   Extract knowledge from this conversation into a hierarchical structure.
   For each piece of knowledge, provide:
   - topic (existing or new L0 category)
   - concept (L1 grouping within topic)
   - facts (L2 specific knowledge items)
   - details (L3+ code examples, specific decisions, etc.)

   Also identify:
   - corrections (knowledge that supersedes previous understanding)
   - relationships (connections between different topics)

   Output as JSON.
   ```
3. Parse structured output, generate embeddings for each fragment
4. Insert into database with proper hierarchy and edges
5. Update watermark

**Idempotency:** Watermarks ensure no double-processing. If daemon restarts, it picks up where it left off.

### Subsystem B: Consolidation

Runs on a configurable interval (default: every 2 hours). Seven phases (like a sleep cycle):

**Phase 0 — Relevance Recomputation (Sleep Cycle):**
- Recompute relevance scores for ALL fragments based on the Ebbinghaus forgetting curve
- Formula: `R = importance * strength * exp(-decay_rate * days_since_reinforcement) + importance * 0.3`
- Strength grows logarithmically with access count (spacing effect)

**Phase 1 — Similarity Detection + Topic Merging:**
- Load all L0 topic fragments, compute pairwise embedding cosine similarity
- Pairs with similarity > 0.9: merge (reparent victim's children to survivor, supersede victim)
- Pairs with similarity > 0.8: candidates for associative linking

**Phase 2 — Associative Link Creation:**
- For each similar topic pair, compare their L1 children
- Create `Associative` edges between cross-topic children with similarity > 0.7

**Phase 3 — Re-summarization (requires API key):**
- Topics with new children (created after last access) get re-summarized by Claude
- Produces a fresh overview paragraph that integrates new knowledge

**Phase 4 — Contradiction Resolution (requires API key):**
- For sibling fragments within the same parent, ask Claude if they contradict
- If yes, supersede the older fragment with the newer one

**Phase 5 — Edge Pruning:**
- Decay all `Associative` edge weights by 5% per consolidation cycle
- Prune edges that have decayed below 0.15 threshold
- `Hierarchical` edges are never decayed or pruned

**Phase 6 — Fragment Pruning (True Forgetting):**
- Tier 1: relevance < 0.02, never accessed, age > 60 days → delete
- Tier 2: relevance < 0.01, age > 90 days → delete regardless of access
- Before deletion, reparent children to the fragment's parent
- Depth-0 topics are never pruned (they just rank low)

### Daemon Process Management

- Runs as a standard background process (not a system daemon initially)
- Start: `engram-daemon start` (forks to background, writes PID to `~/.engram/daemon.pid`)
- Stop: `engram-daemon stop`
- Status: `engram-daemon status`
- Config file: `~/.engram/config.toml`
  ```toml
  [ingestion]
  poll_interval_secs = 30
  batch_size = 20
  claude_model = "claude-sonnet-4-20250514"

  [consolidation]
  interval_secs = 7200
  similarity_threshold = 0.8
  prune_age_days = 30

  [database]
  path = "~/.engram/memory.db"

  [claude]
  api_key_env = "ANTHROPIC_API_KEY"
  ```

## Component 4: engram-plugin (Claude Code Plugin)

### Plugin Structure

```
engram/engram-plugin/
├── .claude-plugin/
│   └── plugin.json
├── .mcp.json
├── skills/
│   └── engram-memory/
│       └── SKILL.md
└── commands/
    ├── remember.md
    └── recall.md
```

### plugin.json
```json
{
  "name": "engram",
  "description": "Brain-inspired persistent long-term memory for AI agents. Gives Claude persistent memory across conversations via a hierarchical knowledge graph.",
  "author": {
    "name": "Alex"
  }
}
```

### .mcp.json
```json
{
  "memory": {
    "command": "engram-mcp",
    "env": {
      "ENGRAM_DB_PATH": "${HOME}/.engram/memory.db"
    }
  }
}
```

The `engram-mcp` binary must be in `$PATH` (or use absolute path after `cargo install`).

### skills/engram-memory/SKILL.md

This is what gives every agent the "awareness" that it has persistent memory. The skill triggers automatically when the agent's task would benefit from long-term context.

```markdown
---
name: engram-memory
description: This skill should be used when the agent is working on a task that could
  benefit from long-term memory, prior conversation context, or stored knowledge. Triggers
  when the user mentions "remember", "recall", "what do you know about", "previous
  conversation", "long-term memory", or when the task involves a codebase or topic
  that may have been discussed before.
version: 0.1.0
---

# Engram: Long-Term Memory

You have access to a persistent long-term memory system. [instructions for the agent...]
```

### commands/recall.md
```markdown
---
description: Search your long-term memory for knowledge about a topic
argument-hint: <topic> [--depth N]
allowed-tools: ["mcp__plugin_engram_memory__*"]
---
# Recall from memory ... [prompt]
```

### commands/remember.md
```markdown
---
description: Explicitly store something in long-term memory
argument-hint: <what to remember>
allowed-tools: ["mcp__plugin_engram_memory__store_memory"]
---
# Store to memory ... [prompt]
```

## Rust Workspace Layout

```
/Users/alex/code/rust/engram/
├── Cargo.toml                  # Workspace manifest
├── CLAUDE.md                   # Dev instructions
├── engram-db/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs              # Public API, re-exports
│   │   ├── fragment.rs         # Fragment, FragmentId types
│   │   ├── edge.rs             # Edge, EdgeKind types
│   │   ├── relevance.rs        # Brain-inspired relevance scoring (Ebbinghaus curve)
│   │   ├── query.rs            # Query engine (search, traverse, explore, reconsolidation)
│   │   ├── embedding.rs        # Embedding generation + cosine similarity
│   │   └── storage.rs          # SQLite backend (create, read, write, migrate, v2 columns)
│   └── tests/
│       ├── brain_behavior.rs   # 30 behavioral tests (decay, reinforcement, importance, etc.)
│       └── live_db_test.rs     # 6 tests against real ~/.engram/memory.db (ignored by default)
├── engram-mcp/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs             # Entry point, stdio transport via rmcp
│       └── server.rs           # MemoryServer impl with #[tool] methods
├── engram-daemon/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs              # Public module exports (for integration tests)
│   │   ├── main.rs             # Entry point, CLI (start/stop/status)
│   │   ├── config.rs           # Config file parsing
│   │   ├── watcher.rs          # File polling + watermark tracking
│   │   ├── parser.rs           # Conversation JSONL parsing
│   │   ├── ingestion.rs        # Knowledge extraction + importance classification
│   │   ├── consolidation.rs    # Memory consolidation (7 phases)
│   │   └── claude_client.rs    # Claude API HTTP client + CLI fallback
│   └── tests/
│       └── scenarios.rs        # 27 integration tests with fixture conversations
└── engram-plugin/
    ├── .claude-plugin/
    │   └── plugin.json
    ├── .mcp.json
    ├── skills/
    │   └── engram-memory/
    │       └── SKILL.md
    └── commands/
        ├── remember.md
        └── recall.md
```

## Key Dependencies

```toml
# engram-db
rusqlite = { version = "0.32", features = ["bundled"] }
fastembed = "5"              # Local embeddings (all-MiniLM-L6-v2, 384-dim)
uuid = { version = "1", features = ["v4", "serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# engram-mcp
engram-db = { path = "../engram-db" }
rmcp = { version = "1.2", features = ["server", "transport-io"] }  # Needs schemars 1.x
tokio = { version = "1", features = ["full"] }
serde_json = "1"

# engram-daemon
engram-db = { path = "../engram-db" }
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["json"] }
toml = "0.8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
futures = "0.3"
```

## Build Order

All phases are complete. Current state:

### Phase 1: Foundation — engram-db ✓
- Types, SQLite storage with WAL mode, fastembed embeddings, query engine
- Brain-inspired relevance scoring (Ebbinghaus curve, importance weighting, decay rates)
- Reconsolidation on recall with spreading activation
- Schema v2 migration for brain-inspired columns
- 24 unit tests + 30 behavioral tests + 6 live DB tests

### Phase 2: Agent Interface — engram-mcp ✓
- `MemoryServer` with `rmcp` 1.2 SDK, 5 MCP tools
- Responses include relevance scores, topics sorted by relevance
- 5 unit tests

### Phase 3: Background Processing — engram-daemon ✓
- Config, JSONL parser, file watcher, Claude API client with CLI fallback
- Ingestion with importance classification (high/medium/low)
- Temporal edges between sequential siblings
- 7-phase consolidation (decay, merge, link, resummarize, contradict, prune edges, prune fragments)
- 11 unit tests + 27 integration scenario tests with fixture conversations

### Phase 4: Plugin — engram-plugin ✓
- plugin.json, .mcp.json, SKILL.md, /recall and /remember commands

## Test Coverage (97 tests)

- **engram-db unit tests (24):** Fragment/edge CRUD, embedding roundtrip, query types, watermarks
- **Brain behavior tests (30):** Decay, reinforcement, spreading activation, importance, forgetting, blended ranking, edge lifecycle, temporal edges, schema migration, reconsolidation, supersession, graph integrity, end-to-end lifecycles
- **Integration scenarios (27):** Fixture conversation parsing, multi-session accumulation, memory lifecycle over months, reconsolidation cascade, topic augmentation, forgetting/pruning, edge decay, full pipeline E2E
- **Live DB tests (6, ignored):** Schema verification, topic sorting, decay recomputation, reinforcement, spreading activation against real `~/.engram/memory.db`
- **MCP server tests (5):** Query, store, traverse, list topics
- **Daemon unit tests (11):** JSONL parsing, extraction prompt building, zoom-tree response parsing
