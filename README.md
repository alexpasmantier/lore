# engram

Brain-inspired persistent memory for AI agents. A background daemon continuously ingests conversations, distills them into a semantic knowledge graph, and exposes that graph to agents via [MCP](https://modelcontextprotocol.io) tools.

## How it works

Engram organizes knowledge hierarchically, inspired by cortical layers:

| Depth | Role | Example |
|-------|------|---------|
| 0 | **Topic** | "Rust async programming" |
| 1 | **Concept** | "tokio runtime model" |
| 2 | **Fact** | "tokio uses work-stealing scheduler" |
| 3+ | **Detail** | "`#[tokio::main(flavor = \"multi_thread\")]` for CPU-bound" |

Agents query this hierarchy at different zoom levels — start broad, drill deeper as needed.

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
│  store_memory        │    │   via Claude API)    │
│  list_topics         │    │                      │
│                      │    │  Consolidation       │
│                      │    │  (merges, links,     │
│                      │    │   prunes, decays)    │
└──────────┬───────────┘    └──────────┬──────────┘
           │ read                      │ read/write
           ▼                           ▼
      ┌─────────────────────────────────────┐
      │          ~/.engram/memory.db         │
      │          (SQLite + WAL mode)         │
      │                                      │
      │  Fragments (nodes with embeddings)   │
      │  Edges (hierarchical + associative)  │
      └─────────────────────────────────────┘
```

**Three crates:**

- **engram-db** — Core graph database. SQLite backend with local embeddings ([all-MiniLM-L6-v2](https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx), 384-dim via `fastembed`).
- **engram-mcp** — MCP server over stdio. Exposes 5 tools for querying and storing knowledge.
- **engram-daemon** — Background process that ingests `~/.claude/projects/` conversation logs, extracts knowledge via Claude API, and periodically consolidates the graph.

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
| `query_memory` | Semantic search at a given depth level |
| `explore_memory` | Get a subtree view of a knowledge area |
| `traverse_memory` | Navigate children, parent, or associations of a fragment |
| `store_memory` | Explicitly store a piece of knowledge |
| `list_topics` | List all top-level knowledge domains |

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
batch_size = 20
claude_model = "claude-sonnet-4-20250514"

[consolidation]
interval_secs = 7200
similarity_threshold = 0.8

[database]
path = "~/.engram/memory.db"
```

## Development

```sh
cargo build              # build all crates
cargo test               # run all tests
cargo clippy --workspace # lint
cargo fmt --all          # format
```
