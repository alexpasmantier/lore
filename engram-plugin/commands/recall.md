---
description: Search your long-term memory for knowledge about a topic
argument-hint: <topic> [--depth N]
allowed-tools: ["mcp__plugin_engram_memory__query_memory", "mcp__plugin_engram_memory__explore_memory", "mcp__plugin_engram_memory__traverse_memory", "mcp__plugin_engram_memory__list_topics"]
---

# Recall from Memory

The user wants to recall knowledge from long-term memory about: $ARGUMENTS

Follow these steps:

1. If no specific topic is given, use `list_topics` to show what's in memory (sorted by relevance — most active first)
2. Otherwise, use `query_memory` with the topic at depth 0 (overview level) to find relevant knowledge
3. If matches are found, use `explore_memory` to show the full knowledge tree for the best match
4. Present the results in a clear, organized way — show the hierarchy from topic → concepts → facts
5. If the user specified `--depth N`, query at that depth instead of 0
6. Note: querying memories reinforces them — frequently recalled knowledge stays fresh

Format the output as a readable summary, not raw JSON. Group related knowledge together.
