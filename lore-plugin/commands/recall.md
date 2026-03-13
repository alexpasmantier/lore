---
description: Search your long-term memory for knowledge about a topic
argument-hint: <topic>
allowed-tools: ["mcp__plugin_lore_memory__search", "mcp__plugin_lore_memory__read", "mcp__plugin_lore_memory__list_roots"]
---

# Recall from Memory

The user wants to recall knowledge from long-term memory about: $ARGUMENTS

Follow these steps:

1. If no specific topic is given, use `list_roots` to see what's in memory, then `read` the most relevant ones
2. Otherwise, use `search` with the topic to find relevant fragments
3. Use `read` on the top results to get their content
4. If more detail is needed, use `search(query, parent_id=...)` to drill into children
5. Present the results in a clear, organized way
6. Note: reading memories reinforces them — frequently recalled knowledge stays fresh

Format the output as a readable summary, not raw JSON.
