# Lore Development Guide

## Build & Test
- Build all: `cargo build`
- Test all: `cargo test` (105 tests across 3 crates)
- Test single crate: `cargo test -p lore-db`
- Run MCP server: `cargo run -p lore-mcp`
- Run daemon: `cargo run -p lore-daemon -- start` (binary is `lore`)
- Single ingestion pass: `cargo run -p lore-daemon -- ingest`
- CLI query: `cargo run -p lore-daemon -- roots`
- Check: `cargo clippy --workspace`
- Format: `cargo fmt --all`

## Architecture
- **lore-db**: Core library. Stores knowledge as interconnected abstraction trees in SQLite with fastembed embeddings (all-MiniLM-L6-v2, 384-dim).
- **lore-mcp**: MCP server (stdio JSON-RPC via `rmcp` crate). Exposes 6 tools: `search`, `read`, `list_roots`, `store`, `update`, `delete`.
- **lore-daemon**: Background process. Two-phase pipeline: ingestion stages raw conversation turns from `~/.claude/projects/` into SQLite (fast, no API calls); consolidation digests idle sessions with full context via Claude API, then runs 7 maintenance phases. Falls back to `claude -p` if no ANTHROPIC_API_KEY is set. Writes `~/.lore/daemon.status` (JSON) to broadcast current activity state.
- **lore-tray**: Desktop app (HAL 9000 style tray icon). Auto-starts daemon on launch, stops on quit. Monitors `~/.lore/daemon.status`. Packaged as macOS `.app` or Linux `.desktop`. Requires GTK3 + libappindicator on Linux.
- **lore-plugin**: Claude Code plugin (static files, not a Rust crate). Contains `.mcp.json`, SKILL.md, and /recall + /remember commands.

## Installed State
- Binaries installed at `~/.local/bin/lore`, `~/.local/bin/lore-mcp`, and `~/.local/bin/lore-tray`
- MCP server registered in `~/.claude/.mcp.json` (user-level, all sessions)
- Config at `~/.lore/config.toml`
- Database at `~/.lore/memory.db`
- Daemon status at `~/.lore/daemon.status` (JSON, written by daemon, read by tray and `lore status`)
- To rebuild and reinstall: `cargo build --release -p lore-mcp -p lore-daemon -p lore-tray && cp target/release/lore target/release/lore-{mcp,tray} ~/.local/bin/`
- macOS app bundle: `just bundle-macos` → `target/Lore.app`
- Linux desktop install: `just install-linux`

## Brain-Inspired Memory Model
- **Relevance scoring**: Ebbinghaus forgetting curve with reinforcement. `R = importance * strength * exp(-decay_rate * days) + importance * 0.3`. Strength grows logarithmically with access count.
- **Reconsolidation on recall**: Accessing a fragment reinforces it (resets decay timer) and spreads activation to neighbors.
- **Importance weighting**: Fragments are scored high/medium/low at extraction. Importance controls decay rate (high=slow, low=fast) and relevance floor.
- **Blended query ranking**: `score = 0.7 * semantic_similarity + 0.3 * relevance_score`. Stale fragments rank lower.
- **Forgetting**: Fragments below relevance threshold (0.05) are invisible to queries. During consolidation, truly forgotten fragments (relevance < 0.02, age > 60d, never accessed) are pruned.
- **Root merging**: Roots above `merge_threshold` (default 0.85) are merged during consolidation. The survivor is the more-accessed root; the victim's children are reparented.
- **Contradiction resolution**: Sibling pairs are batch-checked (up to 10 per API call) for contradictions. The older fragment is superseded.
- **Edge decay**: Associative edge weights decay 5% per consolidation cycle. Edges below 0.15 are pruned.
- **Temporal edges**: Sequential siblings in extracted knowledge are linked with temporal edges.
- **Interconnected abstraction trees**: Fragments form trees where depth 0 = broad concepts and deeper = closer to original conversation specifics. All fragments are the same type, differing only in abstraction level and content. Associative edges link related fragments across different trees.

## Conventions
- All timestamps are Unix seconds (i64).
- Fragment IDs are UUIDs stored as TEXT in SQLite.
- Embeddings are 384-dim f32 vectors (all-MiniLM-L6-v2).
- SQLite uses WAL mode for concurrent read/write.
- MCP server logs to stderr (stdout is JSON-RPC protocol).
- Database path default: `~/.lore/memory.db`, override with `LORE_DB_PATH`.
- Daemon uses `claude -p` CLI fallback when no API key is available (removes CLAUDECODE env var to avoid nesting error).
- Subagent JSONL files (`subagents/` dirs) are skipped during ingestion — mostly tool call noise.
- Fragment columns include `importance` (f32), `relevance_score` (f32), `decay_rate` (f32), `last_reinforced` (i64). Schema auto-migrates via `migrate_v2()`.
- Ingestion stages raw turns into `staged_turns` table (no Claude calls). Consolidation Phase 0 digests idle sessions (default 5 min threshold) with full conversation context. Large conversations are chunked at `max_turns_per_extraction` (default 200).
- Consolidation Phase 1 recomputes all relevance scores (sleep cycle). Phase 7 prunes forgotten fragments.
- Extraction prompt includes existing root content (200 char preview) and children content to reduce duplicate root creation.
- Cross-platform paths via `dirs` crate. Shared `lore_home()` helper in `lore-db`. Never use raw `$HOME`.
- MCP tools `search`, `list_roots` accept a `limit` parameter. `search` returns IDs/scores only; `read` returns content + structural IDs. Iterative walk: search → read → search(parent_id) → read.

## Key Dependencies
- `rmcp` 1.2 — MCP server SDK. Uses `#[tool_router]` + `#[tool_handler]` macro pattern. Needs `schemars` 1.x (not 0.8).
- `fastembed` 5.x — Local embeddings. `embed()` takes `&mut self`, so wrapped in `Mutex`.
- `rusqlite` 0.32 — SQLite. Connection is not `Sync`, so `LoreDb` is wrapped in `Mutex` in the MCP server.
- `tray-icon` 0.19 + `tao` 0.32 — Cross-platform system tray for `lore-tray`. Linux requires `libgtk-3-dev` and `libayatana-appindicator3-dev`.
