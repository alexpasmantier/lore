use lore_db::{Fragment, FragmentId, LoreDb};

use crate::claude_client::ClaudeClient;
use crate::parser::{format_conversation_batch, ConversationTurn};

/// Minimum length for the root (most abstract) fragment.
const MIN_ROOT_LENGTH: usize = 150;

/// Each summarization level compresses by this factor.
const COMPRESSION_RATIO: usize = 3;

/// Session context passed to the extraction prompt.
pub struct SessionContext {
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
}

/// Map importance string to numeric value [0.0, 1.0].
fn importance_value(s: &str) -> f32 {
    match s.to_lowercase().as_str() {
        "high" | "critical" => 0.9,
        "medium" | "normal" => 0.5,
        "low" | "minor" => 0.2,
        _ => 0.5,
    }
}

/// Build a summarization prompt for one compression step.
fn build_summarize_prompt(
    text: &str,
    target_len: usize,
    session: Option<&SessionContext>,
) -> String {
    let mut prompt = String::new();

    if let Some(ctx) = session {
        if let Some(ref cwd) = ctx.cwd {
            prompt.push_str(&format!("Context — project: {}", cwd));
        }
        if let Some(ref branch) = ctx.git_branch {
            prompt.push_str(&format!(", branch: {}", branch));
        }
        prompt.push_str("\n\n");
    }

    prompt.push_str(&format!(
        "Summarize the following into approximately {} characters. \
         Preserve key insights, decisions, and technical details. \
         Write a self-contained summary readable on its own. \
         Respond with ONLY the summary, no preamble.\n\n{}",
        target_len, text
    ));

    prompt
}

/// Extract an abstraction tree from a conversation using recursive summarization.
///
/// Returns a vec of content strings from root (most abstract) to leaf (raw conversation),
/// or an empty vec if the conversation is too short to summarize.
pub async fn extract_abstraction_tree(
    client: &ClaudeClient,
    turns: &[ConversationTurn],
    session: Option<&SessionContext>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let conversation_text = format_conversation_batch(turns);

    if conversation_text.trim().is_empty() {
        return Ok(Vec::new());
    }

    // Build levels bottom-up: F0 = raw, F1 = summary of F0, etc.
    let mut levels = vec![conversation_text];
    let mut current = &levels[0];

    loop {
        let target_len = current.len() / COMPRESSION_RATIO;
        if target_len < MIN_ROOT_LENGTH {
            break;
        }

        let prompt = build_summarize_prompt(current, target_len, session);
        let summary = client.complete(&prompt).await?;
        let summary = summary.trim().to_string();

        if summary.is_empty() {
            break;
        }

        tracing::info!(
            "Compressed {} chars → {} chars (target {})",
            current.len(),
            summary.len(),
            target_len
        );

        levels.push(summary);
        current = levels.last().unwrap();
    }

    if levels.len() == 1 {
        // Conversation too short to summarize — just store as a single fragment
        return Ok(levels);
    }

    // Reverse: root (most abstract) first, leaf (raw conversation) last
    levels.reverse();
    Ok(levels)
}

/// Store an abstraction tree in the database. Returns the number of fragments stored.
///
/// `levels` is ordered root-first (most abstract) to leaf (raw conversation).
pub fn store_abstraction_tree(
    db: &LoreDb,
    levels: &[String],
    source_session: Option<&str>,
) -> Result<usize, Box<dyn std::error::Error>> {
    if levels.is_empty() {
        return Ok(0);
    }

    let total_levels = levels.len();
    let mut parent_id: Option<FragmentId> = None;

    for (i, content) in levels.iter().enumerate() {
        let depth = i as u32;

        // Root = high importance (distilled concept), leaf = low (raw conversation)
        let importance = if i == 0 {
            "high"
        } else if i == total_levels - 1 {
            "low"
        } else {
            "medium"
        };

        let imp = importance_value(importance);
        let mut frag = Fragment::new_with_importance(content.clone(), depth, imp);
        frag.source_session = source_session.map(String::from);
        let frag_id = db.insert(frag, parent_id)?;

        parent_id = Some(frag_id);
    }

    tracing::info!("Stored {} fragments ({} levels)", total_levels, total_levels);
    Ok(total_levels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_summarize_prompt_no_context() {
        let prompt = build_summarize_prompt("Some text here", 100, None);
        assert!(prompt.contains("approximately 100 characters"));
        assert!(prompt.contains("Some text here"));
        assert!(!prompt.contains("Context"));
    }

    #[test]
    fn test_build_summarize_prompt_with_context() {
        let ctx = SessionContext {
            cwd: Some("/Users/alex/code/lore".to_string()),
            git_branch: Some("main".to_string()),
        };
        let prompt = build_summarize_prompt("Some text", 200, Some(&ctx));
        assert!(prompt.contains("/Users/alex/code/lore"));
        assert!(prompt.contains("main"));
    }

    #[test]
    fn test_importance_values() {
        assert_eq!(importance_value("high"), 0.9);
        assert_eq!(importance_value("medium"), 0.5);
        assert_eq!(importance_value("low"), 0.2);
    }
}
