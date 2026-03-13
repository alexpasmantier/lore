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

You have access to a persistent long-term memory system called **Lore**. This gives you memory that persists across conversations, organized as a hierarchical knowledge graph.

## How Memory is Organized

Knowledge is stored as **zoom-trees** where each node is a self-contained summary and children are drill-downs of their parent:
- **Depth 0 — Overviews**: Rich, self-contained paragraph summaries of a knowledge area
- **Depth 1+ — Drill-downs**: Progressively more detailed elaborations on the parent

Each node is readable on its own — children add detail, not just categorize. Fragments are also connected by **associative links** (cross-topic relationships) and **temporal links** (sequential reading order).

## How Memory Behaves

Memory is dynamic — it behaves more like biological memory than a static database:
- **Memories decay over time**: Relevance fades following the Ebbinghaus forgetting curve. Old, unaccessed memories rank lower and eventually become invisible.
- **Accessing memories reinforces them**: When you query and retrieve a memory, it gets reinforced — its decay timer resets and connected memories get a small activation boost.
- **Importance matters**: High-importance memories (architectural decisions, user corrections) decay much slower and maintain a minimum relevance floor. Low-importance memories fade quickly if not accessed.
- **Results are ranked by relevance**: Query results blend semantic similarity (70%) with temporal relevance (30%), so fresh or frequently-accessed memories rank higher.

## Available Tools

### `query_memory`
Search for knowledge about a topic at a specific depth. **Start at depth 0 or 1**, then drill deeper. Use `limit` to control how many results you get back (default 10) — use fewer for focused lookups, more for broad exploration.

### `explore_memory`
Get a full tree view of a knowledge area — shows the hierarchical structure. `limit` controls how many topic trees are returned (default 3).

### `traverse_memory`
Navigate from a specific fragment: get its children (drill deeper), parent (zoom out), or associations (lateral connections).

### `store_memory`
Explicitly store a piece of knowledge. Provide content, a summary, optional parent ID, and depth level.

### `list_topics`
See all top-level knowledge domains with summaries and child counts, sorted by relevance (most active/important first). Use `limit` to cap how many topics are returned.

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

1. **Start broad, drill deep**: Query at depth 0-1 first, then use `traverse_memory` to go deeper
2. **Check before storing**: Use `query_memory` to avoid duplicating existing knowledge
3. **Use meaningful summaries**: The summary field is used for tree browsing — make it descriptive
4. **Connect to existing topics**: When storing, find the right parent topic rather than creating orphans
