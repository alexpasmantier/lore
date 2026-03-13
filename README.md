# lore

Empirical memory for AI agents. Lore builds a centralized knowledge base from experience — it watches past conversations across all sessions and projects, extracts what was learned, and organizes it into a hierarchical graph that any agent can query. Knowledge accumulated in one context is available to every future agent. Over time, a background consolidation process merges duplicates, resolves contradictions, and lets unused knowledge fade.

Agents query the graph through [MCP](https://modelcontextprotocol.io) tools: semantic search at any zoom level, tree exploration to drill from high-level topics into specifics, lateral traversal across associative links, and direct storage for explicit knowledge. Queries blend semantic similarity with a time-decaying relevance score — frequently accessed and recent knowledge surfaces first, while stale fragments naturally fade.

## How it works

Knowledge is stored in a tree of increasing specificity. Each node is a self-contained summary; children elaborate on their parent:

| Depth | Role | Example |
|-------|------|---------|
| 0 | **Topic** | "Rust async programming" |
| 1 | **Concept** | "tokio runtime model" |
| 2 | **Fact** | "tokio uses work-stealing scheduler" |
| 3+ | **Detail** | "`#[tokio::main(flavor = \"multi_thread\")]` for CPU-bound" |

Queries start broad and drill deeper as needed.

### Relevance model

Fragments have a relevance score that decays exponentially over time (Ebbinghaus forgetting curve). Querying a fragment resets its decay timer and spreads a small activation boost to neighbors. Each additional access increases strength with diminishing returns.

At ingestion, fragments are classified as high, medium, or low importance. Importance controls the decay rate and sets a relevance floor — high-importance fragments never fully decay, even if never accessed.

Query results are ranked by `0.7 * semantic_similarity + 0.3 * relevance`, so stale fragments rank lower even when they're a good semantic match. Fragments below the visibility threshold (0.05) are excluded from results entirely.

## Architecture

```
┌─────────────────────────────────────────┐
│          Any agent / session            │
└──────────┬──────────────────────────────┘
           │ stdio (JSON-RPC)
           ▼
┌──────────────────┐    ┌──────────────────┐    ┌──────────────────┐
│    lore-mcp      │    │   lore-daemon    │◄───│    lore-tray     │
│   (MCP server)   │    │   (background)   │    │  (system tray)   │
│                  │    │                  │    │                  │
│  5 query/store   │    │  Ingestion loop  │    │  Monitor, start  │
│  tools for       │    │  Consolidation   │    │  stop & trigger  │
│  agents          │    │  (7 phases)      │    │  daemon actions  │
└────────┬─────────┘    └────────┬─────────┘    └────────┬─────────┘
         │ read                  │ read/write             │ reads
         ▼                       ▼                        ▼
    ┌───────────────────────────────────┐  ┌──────────────────────┐
    │        ~/.lore/memory.db          │  │ ~/.lore/daemon.status│
    │        (SQLite + WAL mode)        │  │      (JSON)          │
    │                                   │  └──────────────────────┘
    │  Fragments · Edges · Watermarks   │
    └───────────────────────────────────┘
```

**Crates:**

- **lore-db** — Core library. SQLite storage, local embeddings ([all-MiniLM-L6-v2](https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx), 384-dim via `fastembed`), relevance scoring, spreading activation.
- **lore-mcp** — MCP server over stdio (`rmcp`). Exposes the shared knowledge base to any connected agent.
- **lore-daemon** — Background process. Watches conversation logs across all projects (`~/.claude/projects/`), extracts knowledge via Claude API, runs 7-phase consolidation.
- **lore-tray** — System tray icon. Monitors the daemon and provides start/stop/trigger controls.
- **lore-plugin** — Claude Code plugin. `/recall` and `/remember` slash commands.

### Consolidation

Runs periodically (default: every 2 hours) and walks the entire graph:

| Phase | Name | What it does |
|-------|------|-------------|
| 0 | Relevance recomputation | Recomputes all relevance scores based on time decay |
| 1 | Topic merging | Merges near-duplicate topics (configurable threshold, default 0.85) |
| 2 | Associative linking | Creates cross-topic edges between related concepts |
| 3 | Re-summarization | Regenerates topic overviews when children have changed |
| 4 | Contradiction resolution | Batch-checks sibling pairs for contradictions, supersedes the older one |
| 5 | Edge pruning | Decays associative edge weights by 5%, prunes below 0.15 |
| 6 | Fragment pruning | Deletes fragments with negligible relevance and no access history |

## Install

```sh
cargo build --release -p lore-mcp -p lore-daemon -p lore-tray
cp target/release/lore-mcp target/release/lore-daemon target/release/lore-tray ~/.local/bin/
```

> **Linux prerequisites for lore-tray:** `sudo apt install libgtk-3-dev libayatana-appindicator3-dev`

Register the MCP server (user-level, all sessions):

```sh
claude mcp add --scope user memory -- lore-mcp
```

## Usage

### MCP tools

| Tool | Description |
|------|-------------|
| `query_memory` | Search at a given depth. `limit` controls result count (default 10). |
| `explore_memory` | Subtree view of a knowledge area. `limit` controls how many trees (default 3). |
| `traverse_memory` | Navigate children, parent, or associations of a fragment. |
| `store_memory` | Explicitly store a piece of knowledge. |
| `list_topics` | List top-level topics, sorted by relevance. Optional `limit`. |

### Daemon

```sh
lore-daemon start          # foreground
lore-daemon daemonize      # background
lore-daemon ingest         # single ingestion pass
lore-daemon consolidate    # single consolidation pass
lore-daemon status         # check if running
lore-daemon stop           # stop background daemon
```

### System tray

```sh
lore-tray
```

> **GNOME users:** The system tray requires the AppIndicator extension. On Ubuntu: `gnome-extensions enable ubuntu-appindicators@ubuntu.com`. On other GNOME distros: install and enable `gnome-shell-extension-appindicator`.

The tray icon reflects the daemon's current state:

| State | Appearance |
|-------|------------|
| **Stopped** | Dim red — the eye is barely glowing |
| **Idle** | Bright red — full intensity |
| **Ingesting** | Red, pulsing — breathing animation |
| **Consolidating** | Orange, pulsing — breathing animation |

Right-click the icon to access the context menu:

- **Start Daemon** / **Stop Daemon** — toggle the background daemon
- **Trigger Ingestion** — run a single ingestion pass on demand
- **Trigger Consolidation** — run a single consolidation pass on demand
- **View Logs** — opens `~/.lore/daemon.log` in a terminal

The tray polls `~/.lore/daemon.status` to stay in sync with the daemon.

### Configuration

`~/.lore/config.toml`:

```toml
[ingestion]
poll_interval_secs = 30
batch_size = 100
claude_model = "claude-sonnet-4-20250514"

[consolidation]
interval_secs = 7200
similarity_threshold = 0.8
merge_threshold = 0.85
min_relevance_prune = 0.02

[database]
path = "~/.lore/memory.db"
```

## Development

```sh
cargo build              # build all crates
cargo test               # 97 tests
cargo clippy --workspace # lint
cargo fmt --all          # format
```

Tests cover unit tests across all crates, behavioral tests for the relevance model (decay, reinforcement, spreading activation, importance, forgetting), and integration scenarios that run fixture conversations through the full pipeline.
