# lore

Shared, evolving knowledge across agents, sessions, and machines.

Lore extracts knowledge from past agent sessions and stores it in a database that reshapes itself over time — using mechanisms inspired by how biological memory works. Multiple agents can share the same knowledge base locally, or across machines through a central server.

## Install

### macOS

```sh
just bundle-macos
cp -r target/Lore.app ~/Applications/
```

Launch **Lore** from Spotlight. It runs as a menu bar icon, auto-managing the background daemon.

> **Note:** This does not install the Lore CLI (see [CLI only](#cli-only) below).

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

### Docker (central server)

```sh
just docker-build
docker run -d -p 8080:8080 -v lore-data:/data -e ANTHROPIC_API_KEY=... lore-server
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

# Remote
lore sync <url>         # push staged turns to central server
```

### Desktop app

The Lore tray icon lives in your menu bar / system tray. It automatically starts the daemon when launched and stops it on quit.

| State | Appearance |
|-------|------------|
| **Stopped** | Dim red |
| **Idle** | Bright red |
| **Ingesting** | Red, pulsing |
| **Consolidating** | Orange, pulsing |
| **Syncing** | Green, pulsing |

## Configuration

`~/.lore/config.toml`:

```toml
[ingestion]
poll_interval_secs = 30

[consolidation]
interval_secs = 7200
idle_threshold_secs = 300       # wait 5 min before digesting a session
max_turns_per_extraction = 200  # chunk large conversations
similarity_threshold = 0.8
merge_threshold = 0.85

[claude]
extraction_model = "claude-sonnet-4-20250514"   # knowledge extraction
compression_model = "claude-haiku-4-5-20251001" # recursive summarization

[database]
path = "~/.lore/memory.db"

[remote]                        # optional: enable central server sync
url = "http://server:8080"
sync_interval_secs = 60
```

## Documentation

- [Setup](docs/setup.md) — single machine, central server, and Docker deployment
- [Architecture](docs/architecture.md) — knowledge model, pipeline, MCP tools, memory dynamics

## Development

```sh
cargo build              # build all crates
cargo test               # 152 tests
cargo clippy --workspace # lint
```
