# lore

Empirical memory for AI agents. Lore builds a centralized knowledge base from experience — it watches past conversations across all sessions and projects, extracts what was learned, and organizes it into a centralized database that any agent can query. Knowledge accumulated in one context is available to every future agent. Over time, a background consolidation process merges duplicates, resolves contradictions, and lets unused knowledge fade.

Agents query the database through [MCP](https://modelcontextprotocol.io) tools using an iterative search→read workflow. Queries blend semantic similarity with a time-decaying relevance score — frequently accessed and recent knowledge surfaces first, while stale fragments naturally fade.

## How it works

Knowledge is organized as interconnected abstraction trees. Higher nodes capture general concepts; deeper nodes stay closer to the specifics of the original conversation. Associative edges link related fragments across different trees. Every node is a self-contained piece of knowledge.

| Depth | Abstraction | Example |
|-------|-------------|---------|
| 0 | Broad concept | "Rust async programming" |
| 1 | Narrower aspect | "tokio runtime model and trade-offs" |
| 2 | Specific finding | "work-stealing scheduler causes issues with CPU-bound tasks" |
| 3+ | Concrete detail | "`#[tokio::main(flavor = \"multi_thread\")]` needed for CPU-bound" |

### Relevance model

Fragments have a relevance score that decays exponentially over time (Ebbinghaus forgetting curve). Reading a fragment resets its decay timer and spreads a small activation boost to neighbors. Each additional access increases strength with diminishing returns.

During extraction, fragments are classified as high, medium, or low importance. Importance controls the decay rate and sets a relevance floor — high-importance fragments never fully decay, even if never accessed.

Query results are ranked by `0.7 * semantic_similarity + 0.3 * relevance`, so stale fragments rank lower even when they're a good semantic match. Fragments below the visibility threshold (0.05) are excluded from results entirely.

## Architecture

```
┌─────────────────────────────────────────┐
│          Any agent / session            │
└──────────┬──────────────────────────────┘
           │ stdio (JSON-RPC)
           ▼
┌──────────────────┐    ┌──────────────────┐    ┌──────────────────┐
│    lore-mcp      │    │      lore        │    │    lore-tray     │
│   (MCP server)   │    │  (CLI + daemon)  │    │  (desktop app)   │
│                  │    │                  │    │                  │
│  6 tools for     │    │  Staging loop    │    │  Auto-manages    │
│  agents          │    │  Consolidation   │    │  daemon lifecycle│
│                  │    │  (8 phases)      │    │                  │
└────────┬─────────┘    └────────┬─────────┘    └────────┬─────────┘
         │ read                  │ read/write             │ reads
         ▼                       ▼                        ▼
    ┌───────────────────────────────────┐  ┌──────────────────────┐
    │        ~/.lore/memory.db          │  │ ~/.lore/daemon.status│
    │        (SQLite + WAL mode)        │  │      (JSON)          │
    │                                   │  └──────────────────────┘
    │  Fragments · Edges · Watermarks   │
    │  Staged turns                     │
    └───────────────────────────────────┘
```

**Crates:**

- **lore-db** — Core library. Stores knowledge as interconnected abstraction trees in SQLite with local embeddings ([all-MiniLM-L6-v2](https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx), 384-dim via `fastembed`).
- **lore-mcp** — MCP server over stdio (`rmcp`). Exposes the knowledge base to any connected agent.
- **lore-daemon** — CLI and background daemon. Stages conversation turns from `~/.claude/projects/`, digests them with full context during consolidation, and provides interactive query commands. Falls back to `claude -p` if no ANTHROPIC_API_KEY is set. Produces the `lore` binary.
- **lore-tray** — Desktop app (HAL 9000 style tray icon). Auto-starts and stops the daemon. Packaged as macOS `.app` or Linux `.desktop`.
- **lore-plugin** — Claude Code plugin. `/recall` and `/remember` slash commands.

### Two-phase pipeline

**Ingestion** runs every 30 seconds, reading new conversation turns from JSONL files and staging them in SQLite. This is instant — no API calls, no latency. Watermarks track progress per file. Session metadata (project path, git branch) is extracted from the JSONL and passed to the extraction prompt.

**Consolidation** runs periodically (default: every 2 hours) and walks the entire graph:

| Phase | Name | What it does |
|-------|------|-------------|
| 0 | Digestion | Extracts knowledge from idle staged conversations (full context) |
| 1 | Relevance recomputation | Recomputes all relevance scores based on time decay |
| 2 | Root merging | Merges near-duplicate roots (configurable threshold, default 0.85) |
| 3 | Associative linking | Creates cross-branch edges between related concepts |
| 4 | Re-summarization | Regenerates root overviews when children have changed |
| 5 | Contradiction resolution | Batch-checks sibling pairs for contradictions, supersedes the older one |
| 6 | Edge pruning | Decays associative edge weights by 5%, prunes below 0.15 |
| 7 | Fragment pruning | Deletes fragments with negligible relevance and no access history |

Phase 0 only digests sessions that have been idle for 5 minutes (configurable), so active conversations are left alone until they're complete. Large conversations are automatically chunked.

## Install

### macOS

```sh
just bundle-macos
cp -r target/Lore.app ~/Applications/
```

Launch **Lore** from Spotlight or Finder. The app runs as a menu bar icon — it auto-starts the daemon in the background and stops it on quit. To start on login, add it via **System Settings > General > Login Items**.

### Linux

```sh
sudo apt install libgtk-3-dev libayatana-appindicator3-dev  # Debian/Ubuntu
just install-linux
```

This installs the binaries to `~/.local/bin/` and registers a `.desktop` entry so **Lore** appears in your application launcher.

> **GNOME users:** The system tray requires the AppIndicator extension. On Ubuntu: `gnome-extensions enable ubuntu-appindicators@ubuntu.com`. On other GNOME distros: install and enable `gnome-shell-extension-appindicator`.

### Manual install

```sh
cargo build --release -p lore-mcp -p lore-daemon -p lore-tray
cp target/release/lore target/release/lore-{mcp,tray} ~/.local/bin/
```

### MCP server registration

Register the MCP server (user-level, all sessions):

```sh
claude mcp add --scope user memory -- lore-mcp
```

## Usage

### Desktop app

The Lore tray icon lives in your menu bar / system tray. It automatically starts the daemon when launched and stops it on quit. The icon reflects the daemon's current state:

| State | Appearance |
|-------|------------|
| **Stopped** | Dim red — the eye is barely glowing |
| **Idle** | Bright red — full intensity |
| **Ingesting** | Red, pulsing — breathing animation |
| **Consolidating** | Orange, pulsing — breathing animation |

The context menu shows the current version and status, and provides controls to:

- **Start / Stop Daemon** — toggle the background daemon
- **Trigger Ingestion** — stage new conversation turns immediately
- **Trigger Consolidation** — digest staged conversations and run all consolidation phases
- **View Logs** — opens `~/.lore/daemon.log`
- **Quit** — stop the daemon and exit

### MCP tools

Agents interact with lore through an iterative search→read workflow. Each step is lightweight — content is only loaded when explicitly read.

| Tool | Description |
|------|-------------|
| `search` | Semantic search. Returns IDs and scores only — no content. Optional `parent_id` to scope to a subtree. |
| `read` | Read a fragment's content + its children/association IDs for navigation. |
| `list_roots` | List root-level fragment IDs and child counts. |
| `store` | Store a piece of knowledge with content, optional parent, and depth. |
| `update` | Update a fragment's content (embedding recomputed). |
| `delete` | Remove a fragment and its edges. |

Workflow: `search` → `read` → `search(parent_id=...)` → `read` → repeat until sufficient detail.

### CLI

The `lore` command provides both daemon management and interactive queries:

```sh
# Daemon
lore start              # run daemon in foreground
lore daemonize          # run daemon in background
lore stop               # stop background daemon
lore status             # check if running
lore logs               # tail daemon log

# Data pipeline
lore ingest             # stage new conversation turns
lore consolidate        # digest staged turns + run consolidation

# Query
lore roots              # list root-level fragments
lore query "text"       # semantic search
lore explore <id>       # show subtree (supports ID prefix)
lore staged             # show staging area
```

### Configuration

`~/.lore/config.toml`:

```toml
[ingestion]
poll_interval_secs = 30
claude_model = "claude-sonnet-4-20250514"

[consolidation]
interval_secs = 7200
idle_threshold_secs = 300       # wait 5 min before digesting a session
max_turns_per_extraction = 200  # chunk large conversations
similarity_threshold = 0.8
merge_threshold = 0.85
min_relevance_prune = 0.02

[database]
path = "~/.lore/memory.db"
```

## Development

```sh
cargo build              # build all crates
cargo test               # 105 tests
cargo clippy --workspace # lint
cargo fmt --all          # format
```

Tests cover unit tests across all crates, behavioral tests for the relevance model (decay, reinforcement, spreading activation, importance, forgetting), and integration scenarios that run fixture conversations through the full pipeline.
