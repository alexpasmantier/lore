# Setup

## Single machine

All components run locally. The daemon ingests conversations, digests them, and agents query the same local database.

### 1. Build and install

```sh
just install
```

This installs `lore`, `lore-mcp`, `lore-tray`, `lore-server`, and `lore-explorer` to `~/.local/bin/`.

On macOS, you can also build the desktop app:

```sh
just bundle-macos
cp -r target/Lore.app ~/Applications/
```

### 2. Register the MCP server

```sh
claude mcp add --scope user memory -- lore-mcp
```

### 3. Start the daemon

Either launch the Lore desktop app (auto-starts the daemon), or:

```sh
lore daemonize
```

The daemon stages conversation turns every 30 seconds and runs consolidation every 2 hours. No further setup needed — it works out of the box using `claude -p` for extraction.

### 4. Verify

```sh
lore status       # should show "running"
lore roots        # list extracted knowledge (empty at first, populates after consolidation)
lore staged       # show conversations waiting to be digested
```

---

## Central server

Multiple client machines push conversation data to a central server. Agents on any machine query the shared knowledge base.

### Server setup

#### 1. Build the server

```sh
cargo build --release -p lore-server -p lore-daemon
cp target/release/lore-server target/release/lore ~/.local/bin/
```

#### 2. Start the server

```sh
lore-server --port 8080 --db /path/to/shared/memory.db
```

The server exposes:
- `/mcp` — MCP tools over streamable HTTP (for agents)
- `/push` — accepts staged turns from clients
- `/status` — health check

#### 3. Start the consolidation daemon on the server

The server needs a daemon to digest pushed turns into knowledge:

```sh
lore start --log-file /var/log/lore.log
```

This runs consolidation against all turns pushed by clients.

#### 4. Register the remote MCP server on each client

```sh
claude mcp add --scope user memory -- npx mcp-remote http://server:8080/mcp
```

Or configure your MCP client to connect to `http://server:8080/mcp` directly if it supports streamable HTTP transport.

### Client setup

#### 1. Build and install the CLI

```sh
cargo build --release -p lore-daemon
cp target/release/lore ~/.local/bin/
```

#### 2. Configure remote mode

Create `~/.lore/config.toml`:

```toml
[remote]
url = "http://server:8080"
sync_interval_secs = 60
```

#### 3. Start the client daemon

```sh
lore daemonize
```

The client daemon:
- Stages new conversation turns every 30 seconds (instant, no API calls)
- Syncs staged turns to the server every 60 seconds
- Does **not** run consolidation or require Claude API access
- Does a final sync on shutdown

#### 4. Verify

```sh
lore status       # should show "running"
lore staged       # should be empty after sync (turns are on the server)
```

To manually trigger a sync:

```sh
lore sync http://server:8080
```

---

## Docker deployment

The central server can also be deployed via Docker:

```sh
just docker-build
docker run -d \
  --name lore \
  -p 8080:8080 \
  -v lore-data:/data \
  -e ANTHROPIC_API_KEY=sk-... \
  lore-server
```

The container runs both `lore-server` (MCP over HTTP) and the consolidation daemon. Configuration is via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `ANTHROPIC_API_KEY` | — | Required for consolidation. Server runs without it but won't digest. |
| `LORE_CONSOLIDATION_INTERVAL` | `7200` | Seconds between consolidation cycles |
| `LORE_IDLE_THRESHOLD` | `300` | Seconds before a session is eligible for digestion |
| `LORE_EXTRACTION_MODEL` | `claude-sonnet-4-20250514` | Model for knowledge extraction |
| `LORE_COMPRESSION_MODEL` | `claude-haiku-4-5-20251001` | Model for recursive summarization |

Data is persisted at `/data/memory.db` inside the container.
