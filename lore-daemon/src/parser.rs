use serde_json::Value;

/// A parsed conversation turn with role and extracted text content.
#[derive(Debug, Clone)]
pub struct ConversationTurn {
    pub role: String,
    pub text: String,
}

/// Metadata about a conversation session extracted from JSONL lines.
#[derive(Debug, Clone, Default)]
pub struct SessionMetadata {
    /// Working directory (project path)
    pub cwd: Option<String>,
    /// Git branch active during the conversation
    pub git_branch: Option<String>,
}

/// Extract session metadata (cwd, gitBranch) from a JSONL line.
/// Call on each line — returns Some on the first line that has the fields.
pub fn parse_session_metadata(line: &str) -> Option<SessionMetadata> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    let obj = value.as_object()?;

    let cwd = obj.get("cwd").and_then(|v| v.as_str()).map(String::from);
    let git_branch = obj
        .get("gitBranch")
        .and_then(|v| v.as_str())
        .map(String::from);

    if cwd.is_some() || git_branch.is_some() {
        Some(SessionMetadata { cwd, git_branch })
    } else {
        None
    }
}

/// Parse a JSONL line from a Claude Code conversation log.
/// Returns None if the line should be skipped (tool calls, empty, etc.)
pub fn parse_jsonl_line(line: &str) -> Option<ConversationTurn> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let value: Value = serde_json::from_str(line).ok()?;
    let obj = value.as_object()?;

    // Get the message object
    let message = obj.get("message")?.as_object()?;
    let role = message.get("role")?.as_str()?;

    // Extract text content from the message
    let content = message.get("content")?;
    let text = extract_text_content(content);

    if text.trim().is_empty() {
        return None;
    }

    Some(ConversationTurn {
        role: role.to_string(),
        text,
    })
}

/// Extract text from message content, which can be a string or an array of blocks.
fn extract_text_content(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => {
            let mut texts = Vec::new();
            for block in blocks {
                if let Some(obj) = block.as_object() {
                    let block_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match block_type {
                        "text" => {
                            if let Some(text) = obj.get("text").and_then(|t| t.as_str()) {
                                texts.push(text.to_string());
                            }
                        }
                        "thinking" => {
                            // Include thinking content as it often contains reasoning
                            if let Some(text) = obj.get("thinking").and_then(|t| t.as_str()) {
                                // Only include substantial thinking
                                if text.len() > 50 {
                                    texts.push(format!("[Reasoning: {}]", truncate(text, 500)));
                                }
                            }
                        }
                        // Skip tool_use, tool_result, image blocks, etc.
                        _ => {}
                    }
                }
            }
            texts.join("\n\n")
        }
        _ => String::new(),
    }
}

fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

/// Read session metadata from a JSONL file (scans first 5 lines).
pub fn read_session_metadata(file_path: &str) -> SessionMetadata {
    let file = match std::fs::File::open(file_path) {
        Ok(f) => f,
        Err(_) => return SessionMetadata::default(),
    };
    let reader = std::io::BufReader::new(file);
    use std::io::BufRead;
    for line in reader.lines().take(5).flatten() {
        if let Some(meta) = parse_session_metadata(&line) {
            return meta;
        }
    }
    SessionMetadata::default()
}

/// Format a batch of conversation turns into a string suitable for the extraction prompt.
pub fn format_conversation_batch(turns: &[ConversationTurn]) -> String {
    let mut output = String::new();
    for turn in turns {
        let role_label = match turn.role.as_str() {
            "user" => "User",
            "assistant" => "Assistant",
            _ => "System",
        };
        output.push_str(&format!("### {}\n{}\n\n", role_label, turn.text));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_user_message_string_content() {
        let line = r#"{"type":"user","message":{"role":"user","content":"How does tokio work?"}}"#;
        let turn = parse_jsonl_line(line).unwrap();
        assert_eq!(turn.role, "user");
        assert_eq!(turn.text, "How does tokio work?");
    }

    #[test]
    fn test_parse_assistant_message_array_content() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Tokio is an async runtime."}]}}"#;
        let turn = parse_jsonl_line(line).unwrap();
        assert_eq!(turn.role, "assistant");
        assert_eq!(turn.text, "Tokio is an async runtime.");
    }

    #[test]
    fn test_skip_tool_only_message() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"123","name":"read_file","input":{}}]}}"#;
        assert!(parse_jsonl_line(line).is_none());
    }

    #[test]
    fn test_skip_empty_line() {
        assert!(parse_jsonl_line("").is_none());
        assert!(parse_jsonl_line("  ").is_none());
    }

    #[test]
    fn test_format_batch() {
        let turns = vec![
            ConversationTurn {
                role: "user".to_string(),
                text: "Hello".to_string(),
            },
            ConversationTurn {
                role: "assistant".to_string(),
                text: "Hi there!".to_string(),
            },
        ];
        let formatted = format_conversation_batch(&turns);
        assert!(formatted.contains("### User\nHello"));
        assert!(formatted.contains("### Assistant\nHi there!"));
    }
}
