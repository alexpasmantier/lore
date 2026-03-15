# Architecture

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

    Centralized:   lore-server (/mcp, /push, /status)
                   ▲  clients push via lore sync
```

## Crates

- **lore-db** — Core library. Stores knowledge as interconnected abstraction trees in SQLite with local embeddings ([all-MiniLM-L6-v2](https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx), 384-dim via `fastembed`). Includes Personalized PageRank for deep graph search.
- **lore-mcp** — MCP server over stdio (`rmcp`). Exposes the knowledge base to any connected agent. Supports `search(deep: true)` for multi-hop graph discovery.
- **lore-daemon** — CLI and background daemon. Produces the `lore` binary. Stages conversation turns (including tool result context), digests them during consolidation with schema-based routing and prediction-error encoding, and provides interactive query commands.
- **lore-tray** — Desktop app (system tray icon). Auto-starts and stops the daemon. Packaged as macOS `.app` or Linux `.desktop`.
- **lore-server** — HTTP server for centralized deployments. MCP over SSE, push endpoint for remote clients, status endpoint. Deployable via Docker.
- **lore-explorer** — Desktop knowledge browser (egui). Interactive search→refine→drill workflow.
- **lore-plugin** — Claude Code plugin. `/recall` and `/remember` slash commands.

## Knowledge model

Knowledge is organized as **interconnected abstraction trees** — broad concepts at the roots, conversation-specific details at the leaves, with associative edges linking related ideas across trees.

```
"Rust error handling"                     depth 0 — broad concept
├── "anyhow vs thiserror trade-offs"      depth 1 — narrower aspect
│   └── "anyhow for apps, thiserror..."   depth 2 — specific finding
└── "error propagation patterns"          depth 1
    └── "? operator with custom From..."  depth 2
```

All fragments are the same type, differing only in depth (abstraction level). Associative edges create lateral connections between related fragments across different trees. Temporal edges preserve the reading order of sequential siblings.

## Two-phase pipeline

**Ingestion** runs every 30 seconds, reading new conversation turns from JSONL files and staging them in SQLite. This is instant — no API calls, no latency. Watermarks track progress per file. Session metadata (project path, git branch) is extracted from the JSONL and passed to the extraction prompt. Tool results (file contents, command outputs, agent summaries) are included alongside text messages, capturing the full project context of each conversation.

**Consolidation** runs periodically (default: every 2 hours):

| Phase | Name | What it does |
|-------|------|-------------|
| 0 | Digestion | Extracts knowledge from idle staged conversations. Long sessions are split at topic boundaries before extraction. Schema-based routing attaches congruent knowledge to existing trees; novel knowledge creates new roots with boosted importance (prediction-error encoding). |
| 1 | Relevance recomputation | Recomputes all relevance scores based on time decay |
| 2 | Root merging | Merges near-duplicate roots (configurable threshold, default 0.85) |
| 3 | Associative linking | Creates cross-branch edges between related concepts |
| 4 | Re-summarization | Regenerates root overviews when children have changed |
| 5 | Reflection | Identifies dense knowledge clusters (3+ connected roots) and generates higher-order synthesis insights not present in any individual fragment |
| 6 | Contradiction resolution | Batch-checks sibling pairs for contradictions, supersedes the older one |
| 7 | Edge pruning | Decays associative edge weights by 5%, prunes below 0.15 |
| 8 | Fragment pruning | Deletes fragments with negligible relevance and no access history |

Phase 0 only digests sessions that have been idle for 5 minutes (configurable), so active conversations are left alone until they're complete. Large conversations are automatically chunked.

## Relevance model

Fragments have a relevance score that decays exponentially over time (Ebbinghaus forgetting curve). Reading a fragment resets its decay timer and spreads a small activation boost to neighbors. Each additional access increases strength with diminishing returns.

During extraction, fragments are classified as high, medium, or low importance. Importance controls the decay rate and sets a relevance floor — high-importance fragments never fully decay, even if never accessed. Novel content that doesn't match existing knowledge gets boosted importance via prediction-error encoding, making surprising discoveries persist longer than routine restatements.

Query results are ranked by `0.7 * semantic_similarity + 0.3 * relevance`, so stale fragments rank lower even when they're a good semantic match. Fragments below the visibility threshold (0.05) are excluded from results entirely.

## Neuroscience-inspired mechanisms

Several features draw on established neuroscience research:

- **Schema-based routing** (Tse et al., 2007; Van Kesteren et al., 2012) — New knowledge is routed based on how well it fits existing schemas. High-fit content is rapidly assimilated into existing trees; low-fit content creates new roots for careful evaluation.
- **Prediction-error encoding** (Kumaran & Maguire, 2006) — Information that contradicts or extends existing knowledge encodes more strongly than routine restatements, mirroring how hippocampal activation scales with prediction error.
- **Event boundary detection** (Zacks et al., 2007) — Long conversations are split at topic boundaries before extraction, reflecting how biological memory consolidation is triggered at event boundaries.
- **Personalized PageRank** (HippoRAG, Gutiérrez et al., 2024) — Deep search propagates activation across associative edges to discover non-obvious multi-hop connections, mimicking hippocampal pattern completion.
- **Reflection** (Park et al., 2023) — Consolidation generates higher-order synthesis from dense knowledge clusters, producing insights not present in any individual source fragment.

## MCP tools

Agents interact through 6 tools using an iterative search→read workflow. Search returns IDs only — content is loaded on demand, so context stays minimal:

| Tool | Description |
|------|-------------|
| `search` | Semantic search. Returns IDs and scores only. Optional `parent_id` to scope to a subtree. Set `deep: true` to run Personalized PageRank across associative edges for multi-hop discovery. |
| `read` | Read a fragment's content + its children/association IDs for navigation. Reinforces the fragment on access. |
| `list_roots` | List root-level fragment IDs and child counts. |
| `store` | Store a piece of knowledge with content, optional parent, and depth. |
| `update` | Update a fragment's content (embedding recomputed). |
| `delete` | Remove a fragment and its edges. |

Workflow: `search` → `read` → `search(parent_id=...)` → `read` → repeat until sufficient detail.
