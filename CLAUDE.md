# Lore Development Guide

## Build & Test
- Build all: `cargo build`
- Test all: `cargo test` (97 tests across 3 crates)
- Test single crate: `cargo test -p lore-db`
- Run MCP server: `cargo run -p lore-mcp`
- Run daemon: `cargo run -p lore-daemon -- start`
- Single ingestion pass: `cargo run -p lore-daemon -- ingest`
- Check: `cargo clippy --workspace`
- Format: `cargo fmt --all`

## Architecture
- **lore-db**: Core library. Persistent memory database with SQLite backend + fastembed embeddings (all-MiniLM-L6-v2, 384-dim).
- **lore-mcp**: MCP server (stdio JSON-RPC via `rmcp` crate). Exposes 5 tools: `query_memory`, `explore_memory`, `traverse_memory`, `store_memory`, `list_topics`.
- **lore-daemon**: Background process. Ingests conversations from `~/.claude/projects/`, extracts knowledge via Claude API, consolidates memory. Falls back to `claude -p` if no ANTHROPIC_API_KEY is set.
- **lore-plugin**: Claude Code plugin (static files, not a Rust crate). Contains `.mcp.json`, SKILL.md, and /recall + /remember commands.

## Installed State
- Binaries installed at `~/.local/bin/lore-mcp` and `~/.local/bin/lore-daemon`
- MCP server registered in `~/.claude/.mcp.json` (user-level, all sessions)
- Config at `~/.lore/config.toml`
- Database at `~/.lore/memory.db`
- To rebuild and reinstall: `cargo build --release -p lore-mcp -p lore-daemon && cp target/release/lore-{mcp,daemon} ~/.local/bin/`

## Brain-Inspired Memory Model
- **Relevance scoring**: Ebbinghaus forgetting curve with reinforcement. `R = importance * strength * exp(-decay_rate * days) + importance * 0.3`. Strength grows logarithmically with access count.
- **Reconsolidation on recall**: Accessing a fragment reinforces it (resets decay timer) and spreads activation to neighbors.
- **Importance weighting**: Fragments are scored high/medium/low at ingestion. Importance controls decay rate (high=slow, low=fast) and relevance floor.
- **Blended query ranking**: `score = 0.7 * semantic_similarity + 0.3 * relevance_score`. Stale fragments rank lower.
- **Forgetting**: Fragments below relevance threshold (0.05) are invisible to queries. During consolidation, truly forgotten fragments (relevance < 0.02, age > 60d, never accessed) are pruned.
- **Edge decay**: Associative edge weights decay 5% per consolidation cycle. Edges below 0.15 are pruned.
- **Temporal edges**: Sequential siblings in extracted knowledge are linked with temporal edges.

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
- Consolidation Phase 0 recomputes all relevance scores (sleep cycle). Phase 6 prunes forgotten fragments.

## Key Dependencies
- `rmcp` 1.2 — MCP server SDK. Uses `#[tool_router]` + `#[tool_handler]` macro pattern. Needs `schemars` 1.x (not 0.8).
- `fastembed` 5.x — Local embeddings. `embed()` takes `&mut self`, so wrapped in `Mutex`.
- `rusqlite` 0.32 — SQLite. Connection is not `Sync`, so `LoreDb` is wrapped in `Mutex` in the MCP server.
