---
name: lore-memory
description: >-
  This skill should be used when the agent is working on a task that could
  benefit from long-term memory, prior conversation context, or stored knowledge.
  Triggers when the user mentions "remember", "recall", "what do you know about",
  "previous conversation", "long-term memory", or when the task involves a
  codebase or topic that may have been discussed before.
version: 0.1.0
---

# Lore: Long-Term Memory

You have access to a persistent long-term memory system called **Lore**. Knowledge is organized as interconnected abstraction trees — broad concepts at the top, conversation-specific details deeper down, with associative edges linking related fragments across trees.

## How Memory Behaves

Memory is dynamic — it behaves more like biological memory than a static database:
- **Memories decay over time**: Relevance fades following the Ebbinghaus forgetting curve. Old, unaccessed memories rank lower and eventually become invisible.
- **Accessing memories reinforces them**: When you read a memory, it gets reinforced — its decay timer resets and connected memories get a small activation boost.
- **Importance matters**: High-importance memories (architectural decisions, user corrections) decay much slower and maintain a minimum relevance floor.
- **Results are ranked by relevance**: Search results blend semantic similarity (70%) with temporal relevance (30%).
- **Novel knowledge persists longer**: Surprising information that doesn't match existing knowledge is encoded with higher importance, making it decay slower.
- **Deep search finds hidden connections**: `search(deep: true)` traverses the knowledge graph to find fragments connected by associative edges, even if they don't match the query directly.

## Available Tools

### `search`
Semantic search across memory. Returns **IDs and scores only** — no content. Use `read` to get content of specific results. Pass `parent_id` to restrict search to descendants of a fragment. Set `deep: true` to run graph traversal (Personalized PageRank) that discovers non-obvious connections across associative edges — useful when direct semantic search misses related knowledge.

### `read`
Read the full content of a fragment, plus its structural connections (parent ID, children IDs, association IDs) for navigation.

### `list_roots`
List root-level fragments (the broadest knowledge areas). Returns IDs and child counts only.

### `store`
Store a piece of knowledge. Provide content, optional parent ID, and depth (0=broad concept, higher=more specific).

### `delete`
Remove a fragment and all its edges.

### `update`
Update the content of an existing fragment. The embedding is recomputed automatically.

## Workflow: Search → Read → Narrow → Read

Each step is lightweight — content is only loaded when you explicitly read.

1. `search(query)` — find relevant fragments (IDs only)
2. `read(id)` — read content, see children/associations
3. `search(query, parent_id=id)` — narrow search within a subtree
4. Repeat until you have the detail you need.

## When to Use Memory

**Proactively check memory when:**
- Starting work on a codebase or topic that may have prior context
- The user references previous conversations or decisions
- You need to understand project conventions or architecture

**Store to memory when:**
- You learn important project context, decisions, or conventions
- The user explicitly asks you to remember something
- You discover architectural patterns or gotchas worth preserving

## Best Practices

1. **Start broad, drill deep**: Search globally first, then use `parent_id` to narrow
2. **Check before storing**: Search to avoid duplicating existing knowledge
3. **Connect to existing roots**: When storing, find the right parent rather than creating orphans
