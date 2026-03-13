---
description: Explicitly store something in long-term memory
argument-hint: <what to remember>
allowed-tools: ["mcp__plugin_lore_memory__store_memory", "mcp__plugin_lore_memory__query_memory", "mcp__plugin_lore_memory__list_roots"]
---

# Store to Memory

The user wants to store knowledge in long-term memory: $ARGUMENTS

Follow these steps:

1. First, use `list_roots` and `query_memory` to check if related knowledge already exists
2. Determine the appropriate depth level:
   - Depth 0 if this is a new broad root
   - Depth 1 if this is a concept within an existing root
   - Depth 2 if this is a specific fact (most common)
   - Depth 3+ if this is a detailed code example or decision
3. If a relevant parent root/concept exists, use its ID as `parent_id`
4. Use `store_memory` with:
   - `content`: The full knowledge text
   - `summary`: A concise one-line description
   - `parent_id`: The parent fragment ID (or null for new root)
   - `depth`: The appropriate level
5. Confirm what was stored and where it was placed in the hierarchy
