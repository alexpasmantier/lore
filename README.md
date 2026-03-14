# lore

Long-term memory for AI agents. Lore watches past conversations, extracts what was learned, and stores it in a persistent knowledge base that any agent can query. Knowledge accumulated in one session is available to every future session, across all projects.

Multiple agent sessions can query the same knowledge base on a single machine, or point at a central server for shared memory across a group of users, a team, a company.

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

## Usage

### CLI

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

### Desktop app

The Lore tray icon lives in your menu bar / system tray. It automatically starts the daemon when launched and stops it on quit.

| State | Appearance |
|-------|------------|
| **Stopped** | Dim red |
| **Idle** | Bright red |
| **Ingesting** | Red, pulsing |
| **Consolidating** | Orange, pulsing |

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

## Documentation

- [Architecture](docs/architecture.md) — knowledge model, pipeline, MCP tools, relevance model

## Development

```sh
cargo build              # build all crates
cargo test               # 105 tests
cargo clippy --workspace # lint
```
