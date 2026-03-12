# Engram Development Guide

## Build & Test
- Build all: `cargo build`
- Test all: `cargo test` (27 tests across 3 crates)
- Test single crate: `cargo test -p engram-db`
- Run MCP server: `cargo run -p engram-mcp`
- Run daemon: `cargo run -p engram-daemon -- start`
- Single ingestion pass: `cargo run -p engram-daemon -- ingest`
- Check: `cargo clippy --workspace`
- Format: `cargo fmt --all`

## Architecture
- **engram-db**: Core library. Graph database with SQLite backend + fastembed embeddings (all-MiniLM-L6-v2, 384-dim).
- **engram-mcp**: MCP server (stdio JSON-RPC via `rmcp` crate). Exposes 5 tools: `query_memory`, `explore_memory`, `traverse_memory`, `store_memory`, `list_topics`.
- **engram-daemon**: Background process. Ingests conversations from `~/.claude/projects/`, extracts knowledge via Claude API, consolidates memory. Falls back to `claude -p` if no ANTHROPIC_API_KEY is set.
- **engram-plugin**: Claude Code plugin (static files, not a Rust crate). Contains `.mcp.json`, SKILL.md, and /recall + /remember commands.

## Installed State
- Binaries installed at `~/.local/bin/engram-mcp` and `~/.local/bin/engram-daemon`
- MCP server registered in `~/.claude/.mcp.json` (user-level, all sessions)
- Config at `~/.engram/config.toml`
- Database at `~/.engram/memory.db`
- To rebuild and reinstall: `cargo build --release -p engram-mcp -p engram-daemon && cp target/release/engram-{mcp,daemon} ~/.local/bin/`

## Conventions
- All timestamps are Unix seconds (i64).
- Fragment IDs are UUIDs stored as TEXT in SQLite.
- Embeddings are 384-dim f32 vectors (all-MiniLM-L6-v2).
- SQLite uses WAL mode for concurrent read/write.
- MCP server logs to stderr (stdout is JSON-RPC protocol).
- Database path default: `~/.engram/memory.db`, override with `ENGRAM_DB_PATH`.
- Daemon uses `claude -p` CLI fallback when no API key is available (removes CLAUDECODE env var to avoid nesting error).
- Subagent JSONL files (`subagents/` dirs) are skipped during ingestion — mostly tool call noise.

## Key Dependencies
- `rmcp` 1.2 — MCP server SDK. Uses `#[tool_router]` + `#[tool_handler]` macro pattern. Needs `schemars` 1.x (not 0.8).
- `fastembed` 5.x — Local embeddings. `embed()` takes `&mut self`, so wrapped in `Mutex`.
- `rusqlite` 0.32 — SQLite. Connection is not `Sync`, so `EngramDb` is wrapped in `Mutex` in the MCP server.
