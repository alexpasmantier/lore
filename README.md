# lore

Long-term memory for AI agents. Lore watches past conversations, extracts what was learned, and stores it in a persistent knowledge base that any agent can query via [MCP](https://modelcontextprotocol.io).

Knowledge accumulated in one session is available to every future session, across all projects. Relevance decays over time — frequently accessed knowledge stays fresh, unused knowledge fades.

The MCP server is stateless and reads from a shared SQLite database. Multiple agent sessions can query the same knowledge base on a single machine, or point at a central server for shared memory across a team.

## How it works

Lore runs as a background daemon that watches your conversation logs, stages new turns, and periodically digests them into a knowledge database. Agents query it through MCP tools using an iterative search→read workflow that keeps context lean.

Knowledge is organized as **interconnected abstraction trees** — broad concepts at the roots, conversation-specific details at the leaves, with associative edges linking related ideas across trees.

```
"Rust error handling"                     depth 0 — broad concept
├── "anyhow vs thiserror trade-offs"      depth 1 — narrower aspect
│   └── "anyhow for apps, thiserror..."   depth 2 — specific finding
└── "error propagation patterns"          depth 1
    └── "? operator with custom From..."  depth 2
```

### For agents (MCP)

Agents interact through 6 tools. Search returns IDs only — content is loaded on demand, so context stays minimal:

1. **`search(query, parent_id?)`** → ranked IDs (no content)
2. **`read(id)`** → content + children/association IDs
3. **`list_roots`** → top-level knowledge areas
4. **`store / update / delete`** → write operations

Workflow: search → read → search deeper → read → repeat until you have what you need.

### For humans (CLI)

```sh
lore roots              # what do I know?
lore query "error handling"  # semantic search
lore explore <id>       # show subtree
lore status             # daemon running?
lore staged             # conversations awaiting digestion
```

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│              Agent sessions (any number)                  │
└──────────┬───────────────────────────────────────────────┘
           │ MCP (stdio / SSE)
           ▼
┌──────────────────┐    ┌──────────────────┐    ┌──────────────────┐
│    lore-mcp      │    │      lore        │    │    lore-tray     │
│  (MCP server)    │    │  (CLI + daemon)  │    │  (desktop app)   │
└────────┬─────────┘    └────────┬─────────┘    └────────┬─────────┘
         │                       │                        │
         ▼                       ▼                        ▼
    ┌────────────────────────────────────────────────────────────┐
    │                    ~/.lore/memory.db                        │
    │              SQLite · WAL mode · Local embeddings           │
    └────────────────────────────────────────────────────────────┘
```

## Install

### macOS

```sh
just bundle-macos
cp -r target/Lore.app ~/Applications/
```

Launch **Lore** from Spotlight. It runs as a menu bar icon, auto-managing the background daemon.

### Linux

```sh
sudo apt install libgtk-3-dev libayatana-appindicator3-dev  # Debian/Ubuntu
just install-linux
```

### CLI only

```sh
cargo build --release -p lore-daemon
cp target/release/lore ~/.local/bin/
```

### MCP server

```sh
cargo build --release -p lore-mcp
cp target/release/lore-mcp ~/.local/bin/
claude mcp add --scope user memory -- lore-mcp
```

## Configuration

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

[database]
path = "~/.lore/memory.db"
```

## Development

```sh
cargo build              # build all crates
cargo test               # 105 tests
cargo clippy --workspace # lint
```
