# engram

Brain-inspired persistent memory for AI agents. Memories decay over time following the Ebbinghaus forgetting curve, strengthen when accessed (reconsolidation), and are weighted by importance. A background daemon ingests conversations and distills knowledge into a semantic graph, while periodic consolidation merges related concepts, resolves contradictions, and prunes truly forgotten fragments — like sleep consolidation in the brain.

The resulting knowledge graph is exposed to agents via [MCP](https://modelcontextprotocol.io) tools.

## How it works

Engram organizes knowledge as a hierarchy of increasing specificity:

| Depth | Role | Example |
|-------|------|---------|
| 0 | **Topic** | "Rust async programming" |
| 1 | **Concept** | "tokio runtime model" |
| 2 | **Fact** | "tokio uses work-stealing scheduler" |
| 3+ | **Detail** | "`#[tokio::main(flavor = \"multi_thread\")]` for CPU-bound" |

Agents query this hierarchy at different zoom levels — start broad, drill deeper as needed.

### Brain-inspired properties

- **Forgetting curve**: Relevance decays exponentially over time. `R = importance * strength * exp(-decay_rate * days) + importance * 0.3`
- **Reconsolidation on recall**: Querying a memory reinforces it (resets decay timer) and spreads activation to connected neighbors
- **Importance weighting**: Fragments are classified high/medium/low at ingestion. High-importance memories decay slower and maintain a higher relevance floor
- **Blended ranking**: Query results are scored as `0.7 * semantic_similarity + 0.3 * relevance_score`, so stale memories rank lower even if semantically relevant
- **True forgetting**: Fragments below the relevance threshold (0.05) become invisible to queries. During consolidation, fragments with negligible relevance are permanently pruned

## Architecture

```
┌────────────────────────────────────────────────┐
│                 Claude Code Agent               │
│         (queries memory via MCP tools)          │
└──────────┬─────────────────────────────────────┘
           │ stdio (JSON-RPC)
           ▼
┌──────────────────────┐    ┌─────────────────────┐
│   engram-mcp         │    │   engram-daemon      │
│   (MCP Server)       │    │   (Background)       │
│                      │    │                      │
│  query_memory        │    │  Ingestion           │
│  explore_memory      │    │  (polls conversations│
│  traverse_memory     │    │   extracts knowledge │
│  store_memory        │    │   + importance via   │
│  list_topics         │    │   Claude API)        │
│                      │    │                      │
│  Results include     │    │  Consolidation       │
│  relevance scores    │    │  (7 phases: decay,   │
│                      │    │   merge, link, resum │
│                      │    │   contradict, prune  │
│                      │    │   edges, prune frags)│
└──────────┬───────────┘    └──────────┬──────────┘
           │ read                      │ read/write
           ▼                           ▼
      ┌─────────────────────────────────────┐
      │          ~/.engram/memory.db         │
      │          (SQLite + WAL mode)         │
      │                                      │
      │  Fragments (nodes with embeddings,   │
      │    importance, relevance, decay)     │
      │  Edges (hierarchical, associative,   │
      │    temporal, supersedes)             │
      └─────────────────────────────────────┘
```

**Three crates:**

- **engram-db** — Core graph database. SQLite backend with local embeddings ([all-MiniLM-L6-v2](https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx), 384-dim via `fastembed`).
- **engram-mcp** — MCP server over stdio. Exposes 5 tools for querying and storing knowledge.
- **engram-daemon** — Background process that ingests `~/.claude/projects/` conversation logs, extracts knowledge with importance classification via Claude API, and periodically consolidates the graph (recomputing relevance scores, merging near-duplicate topics, creating associative links, resolving contradictions, decaying edge weights, and pruning forgotten fragments).

Plus **engram-plugin** — a Claude Code plugin with `/recall` and `/remember` commands.

## Install

```sh
cargo build --release -p engram-mcp -p engram-daemon
cp target/release/engram-{mcp,daemon} ~/.local/bin/
```

Register the MCP server (user-level, all sessions):

```sh
claude mcp add --scope user memory -- engram-mcp
```

## Usage

### MCP tools (used by agents automatically)

| Tool | Description |
|------|-------------|
| `query_memory` | Semantic search at a given depth level. Results ranked by blended semantic + relevance score. Accessing results reinforces them. |
| `explore_memory` | Get a subtree view of a knowledge area |
| `traverse_memory` | Navigate children, parent, or associations of a fragment |
| `store_memory` | Explicitly store a piece of knowledge |
| `list_topics` | List all top-level knowledge domains, sorted by relevance |

### Daemon

```sh
engram-daemon start          # run in foreground
engram-daemon daemonize      # run in background
engram-daemon ingest         # single ingestion pass
engram-daemon consolidate    # single consolidation pass
engram-daemon status         # check if running
engram-daemon stop           # stop background daemon
```

Configuration lives at `~/.engram/config.toml`:

```toml
[ingestion]
poll_interval_secs = 30
batch_size = 100
claude_model = "claude-sonnet-4-20250514"

[consolidation]
interval_secs = 7200
similarity_threshold = 0.8
min_relevance_prune = 0.02    # fragments below this relevance may be pruned

[database]
path = "~/.engram/memory.db"
```

## Development

```sh
cargo build              # build all crates
cargo test               # run 97 tests across all crates
cargo clippy --workspace # lint
cargo fmt --all          # format
```

Tests include unit tests (24 engram-db, 11 daemon, 5 MCP), 30 behavioral tests validating brain-inspired properties (decay, reinforcement, spreading activation, importance, forgetting), and 27 integration scenario tests with fixture conversations covering the full lifecycle from ingestion through consolidation to querying.
