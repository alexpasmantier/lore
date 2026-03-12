---
description: Explicitly store something in long-term memory
argument-hint: <what to remember>
allowed-tools: ["mcp__plugin_engram_memory__store_memory", "mcp__plugin_engram_memory__query_memory", "mcp__plugin_engram_memory__list_topics"]
---

# Store to Memory

The user wants to store knowledge in long-term memory: $ARGUMENTS

Follow these steps:

1. First, use `list_topics` and `query_memory` to check if related knowledge already exists
2. Determine the appropriate depth level:
   - Depth 0 if this is a new broad topic
   - Depth 1 if this is a concept within an existing topic
   - Depth 2 if this is a specific fact (most common)
   - Depth 3+ if this is a detailed code example or decision
3. If a relevant parent topic/concept exists, use its ID as `parent_id`
4. Use `store_memory` with:
   - `content`: The full knowledge text
   - `summary`: A concise one-line description
   - `parent_id`: The parent fragment ID (or null for new topic)
   - `depth`: The appropriate level
5. Confirm what was stored and where it was placed in the hierarchy
