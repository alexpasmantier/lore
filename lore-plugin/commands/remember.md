---
description: Explicitly store something in long-term memory
argument-hint: <what to remember>
allowed-tools: ["mcp__plugin_lore_memory__store", "mcp__plugin_lore_memory__search", "mcp__plugin_lore_memory__read", "mcp__plugin_lore_memory__list_roots"]
---

# Store to Memory

The user wants to store knowledge in long-term memory: $ARGUMENTS

Follow these steps:

1. First, use `search` to check if related knowledge already exists
2. If matches exist, `read` them to understand what's already stored
3. Determine the appropriate depth level:
   - Depth 0 if this is a new broad concept
   - Depth 1+ for more specific knowledge under an existing root
4. If a relevant parent exists, use its ID as `parent_id`
5. Use `store` with:
   - `content`: The full knowledge text
   - `parent_id`: The parent fragment ID (or omit for new root)
   - `depth`: The appropriate level
6. Confirm what was stored and where it was placed in the hierarchy
