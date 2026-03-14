use lore_db::{EdgeKind, Fragment, FragmentId, LoreDb};

use crate::claude_client::ClaudeClient;
use crate::parser::{format_conversation_batch, ConversationTurn};

/// Minimum length for the root (most abstract) fragment.
const MIN_ROOT_LENGTH: usize = 150;

/// Each summarization level compresses by this factor.
const COMPRESSION_RATIO: usize = 3;

/// Session context passed to prompts.
pub struct SessionContext {
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
}

fn session_context_prefix(session: Option<&SessionContext>) -> String {
    let mut prefix = String::new();
    if let Some(ctx) = session {
        if let Some(ref cwd) = ctx.cwd {
            prefix.push_str(&format!("Context — project: {}", cwd));
        }
        if let Some(ref branch) = ctx.git_branch {
            prefix.push_str(&format!(", branch: {}", branch));
        }
        prefix.push_str("\n\n");
    }
    prefix
}

/// Result of extracting knowledge from a conversation.
pub struct ExtractionResult {
    /// The raw conversation transcript (stored as a standalone fragment).
    pub transcript: String,
    /// Independent knowledge trees, each a vec of levels from root (shortest) to leaf (longest).
    pub trees: Vec<Vec<String>>,
}

/// Extract knowledge from a conversation: extract insights, split into topics,
/// then recursively summarize each topic into an abstraction tree.
pub async fn extract_knowledge_trees(
    client: &ClaudeClient,
    turns: &[ConversationTurn],
    session: Option<&SessionContext>,
) -> Result<ExtractionResult, Box<dyn std::error::Error>> {
    let transcript = format_conversation_batch(turns);

    if transcript.trim().is_empty() {
        return Ok(ExtractionResult {
            transcript,
            trees: Vec::new(),
        });
    }

    // Step 1: Extract knowledge worth remembering
    let ctx = session_context_prefix(session);
    let extract_prompt = format!(
        "{ctx}Extract the knowledge worth remembering from this conversation. \
         Focus on: architectural decisions and rationale, non-obvious technical insights, \
         debugging breakthroughs, user preferences and corrections, project conventions. \
         Skip: routine code changes, standard API usage, greetings, tool call noise. \
         Write a document of the key insights, each as a self-contained paragraph. \
         Respond with ONLY the extracted knowledge, no preamble.\n\n{transcript}"
    );

    let extracted = client.complete(&extract_prompt).await?;
    let extracted = extracted.trim().to_string();

    if extracted.is_empty() {
        tracing::info!("No extractable knowledge found");
        return Ok(ExtractionResult {
            transcript,
            trees: Vec::new(),
        });
    }

    tracing::info!(
        "Extracted {} chars of knowledge from {} char conversation",
        extracted.len(),
        transcript.len()
    );

    // Step 2: Split into distinct topics
    let split_prompt = format!(
        "{ctx}The following is extracted knowledge from a conversation. \
         Split it into distinct, independent topics. Each topic should be self-contained \
         and cover one coherent subject area. \
         Output each topic separated by a line containing only '---'. \
         Respond with ONLY the topics, no preamble or numbering.\n\n{extracted}"
    );

    let split_response = client.complete(&split_prompt).await?;
    let topics: Vec<String> = split_response
        .split("\n---\n")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    tracing::info!("Split into {} topics", topics.len());

    // Step 3: Recursively summarize each topic into an abstraction tree
    let mut trees = Vec::new();

    for topic in &topics {
        let tree = compress_to_tree(client, topic, session).await?;
        if !tree.is_empty() {
            trees.push(tree);
        }
    }

    Ok(ExtractionResult { transcript, trees })
}

/// Recursively compress a text into an abstraction tree.
/// Returns levels from root (most abstract) to leaf (original text).
async fn compress_to_tree(
    client: &ClaudeClient,
    text: &str,
    session: Option<&SessionContext>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut levels = vec![text.to_string()];
    let mut current = &levels[0];

    loop {
        let target_len = current.len() / COMPRESSION_RATIO;
        if target_len < MIN_ROOT_LENGTH {
            break;
        }

        let ctx = session_context_prefix(session);
        let prompt = format!(
            "{ctx}Summarize the following into approximately {target_len} characters. \
             Preserve key insights, decisions, and technical details. \
             Write a self-contained summary readable on its own. \
             Respond with ONLY the summary, no preamble.\n\n{current}"
        );

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

    // Reverse: root (most abstract) first, leaf (full extracted topic) last
    levels.reverse();
    Ok(levels)
}

/// Store the extraction result in the database. Returns the number of fragments stored.
///
/// Stores the raw transcript as a standalone fragment, then each knowledge tree
/// as a chain of fragments from root to leaf, linked to the transcript via
/// associative edges.
pub fn store_extraction_result(
    db: &LoreDb,
    result: &ExtractionResult,
    source_session: Option<&str>,
) -> Result<usize, Box<dyn std::error::Error>> {
    if result.trees.is_empty() {
        return Ok(0);
    }

    // Store raw transcript as a standalone low-importance fragment
    let mut transcript_frag = Fragment::new_with_importance(result.transcript.clone(), 0, 0.1);
    transcript_frag.source_session = source_session.map(String::from);
    let transcript_id = db.insert(transcript_frag, None)?;
    let mut count = 1;

    // Store each knowledge tree
    for tree_levels in &result.trees {
        if tree_levels.is_empty() {
            continue;
        }

        let total = tree_levels.len();
        let mut parent_id: Option<FragmentId> = None;
        let mut leaf_id = None;

        for (i, content) in tree_levels.iter().enumerate() {
            let depth = i as u32;

            // Root = high importance, leaf = medium, single-level = high
            let importance = if i == 0 {
                0.9
            } else if i == total - 1 {
                0.5
            } else {
                0.7
            };

            let mut frag = Fragment::new_with_importance(content.clone(), depth, importance);
            frag.source_session = source_session.map(String::from);
            let frag_id = db.insert(frag, parent_id)?;

            parent_id = Some(frag_id);
            leaf_id = Some(frag_id);
            count += 1;
        }

        // Link the leaf of this tree to the transcript
        if let Some(leaf) = leaf_id {
            let _ = db.link(leaf, transcript_id, EdgeKind::Associative, 1.0);
        }
    }

    tracing::info!(
        "Stored {} fragments ({} trees + transcript)",
        count,
        result.trees.len()
    );
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_context_prefix() {
        let ctx = SessionContext {
            cwd: Some("/code/lore".to_string()),
            git_branch: Some("main".to_string()),
        };
        let prefix = session_context_prefix(Some(&ctx));
        assert!(prefix.contains("/code/lore"));
        assert!(prefix.contains("main"));

        let empty = session_context_prefix(None);
        assert!(empty.is_empty());
    }

    #[test]
    fn test_store_extraction_result_empty() {
        let storage = lore_db::Storage::open_memory().unwrap();
        let db = LoreDb::new_without_embeddings(storage);
        let result = ExtractionResult {
            transcript: "hello".to_string(),
            trees: vec![],
        };
        let count = store_extraction_result(&db, &result, None).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_store_extraction_result_with_trees() {
        let storage = lore_db::Storage::open_memory().unwrap();
        let db = LoreDb::new_without_embeddings(storage);
        let result = ExtractionResult {
            transcript: "raw conversation here".to_string(),
            trees: vec![
                vec!["broad concept".to_string(), "detailed knowledge".to_string()],
                vec!["another concept".to_string()],
            ],
        };
        let count = store_extraction_result(&db, &result, Some("session-1")).unwrap();
        // 1 transcript + 2 (tree 1) + 1 (tree 2) = 4
        assert_eq!(count, 4);

        // Two roots at depth 0 (the knowledge roots) + one transcript at depth 0
        let roots = db.list_roots(None);
        // transcript + 2 tree roots = 3 depth-0 fragments
        assert_eq!(roots.len(), 3);
    }
}
