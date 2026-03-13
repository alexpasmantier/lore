use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lore_db::{LoreDb, Fragment, FragmentId, Storage, Tree};
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ──── Parameter types for MCP tools ────

#[derive(Deserialize, JsonSchema)]
pub struct QueryMemoryParams {
    /// Semantic search query — what to search for
    pub topic: String,
    /// Depth level filter: 0=topics, 1=concepts, 2=facts, 3+=details. Only returns fragments at this exact depth.
    #[serde(default = "default_depth")]
    pub depth: u32,
    /// Max results to return
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Deserialize, JsonSchema)]
pub struct ExploreMemoryParams {
    /// Semantic search query — finds the best-matching topic to root the tree
    pub topic: String,
    /// How many hierarchy levels to expand below the root (default: 2)
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,
    /// Max number of separate topic trees to return (default: 3)
    #[serde(default = "default_explore_limit")]
    pub limit: usize,
}

#[derive(Deserialize, JsonSchema)]
pub struct TraverseMemoryParams {
    /// The fragment ID to navigate from
    pub fragment_id: String,
    /// Direction: "children", "parent", or "associations"
    pub direction: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct StoreMemoryParams {
    /// The knowledge to store
    pub content: String,
    /// One-line summary
    pub summary: String,
    /// Parent fragment ID. If omitted for depth > 0, auto-assigns to the most
    /// semantically similar existing topic.
    pub parent_id: Option<String>,
    /// Zoom level (0=topic overview, 1=concept, 2=fact, 3=detail)
    #[serde(default = "default_store_depth")]
    pub depth: u32,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListTopicsParams {
    /// Max number of topics to return (default: 50)
    #[serde(default = "default_list_limit")]
    pub limit: usize,
    /// Number of topics to skip (for pagination)
    #[serde(default)]
    pub offset: usize,
    /// Optional keyword filter — only return topics whose summary or content matches
    pub query: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct DeleteMemoryParams {
    /// The fragment ID to delete
    pub fragment_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct UpdateMemoryParams {
    /// The fragment ID to update
    pub fragment_id: String,
    /// New content (replaces existing)
    pub content: String,
    /// New one-line summary (replaces existing)
    pub summary: String,
}

fn default_depth() -> u32 {
    0
}
fn default_limit() -> usize {
    10
}
fn default_max_depth() -> u32 {
    2
}
fn default_explore_limit() -> usize {
    3
}
fn default_store_depth() -> u32 {
    2
}
fn default_list_limit() -> usize {
    50
}

// ──── Response types ────

#[derive(Serialize)]
struct FragmentResponse {
    id: String,
    summary: String,
    content: String,
    depth: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    score: Option<f32>,
    relevance: f32,
    /// Parent fragment ID (if this fragment has a parent in the hierarchy)
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<String>,
    /// Parent fragment summary (breadcrumb for context)
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_summary: Option<String>,
}

#[derive(Serialize)]
struct TreeResponse {
    id: String,
    summary: String,
    content: String,
    depth: u32,
    relevance: f32,
    children: Vec<TreeResponse>,
}

#[derive(Serialize)]
struct TopicResponse {
    id: String,
    summary: String,
    content: String,
    child_count: usize,
    relevance: f32,
}

impl FragmentResponse {
    fn from_fragment_with_parent(f: &Fragment, score: Option<f32>, db: &LoreDb) -> Self {
        let parent = db.parent(f.id);
        Self {
            id: f.id.to_string(),
            summary: f.summary.clone(),
            content: f.content.clone(),
            depth: f.depth,
            score,
            relevance: f.relevance_score,
            parent_id: parent.as_ref().map(|p| p.id.to_string()),
            parent_summary: parent.as_ref().map(|p| p.summary.clone()),
        }
    }
}

impl TreeResponse {
    fn from_tree(tree: &Tree) -> Self {
        Self {
            id: tree.fragment.id.to_string(),
            summary: tree.fragment.summary.clone(),
            content: tree.fragment.content.clone(),
            depth: tree.fragment.depth,
            relevance: tree.fragment.relevance_score,
            children: tree.children.iter().map(TreeResponse::from_tree).collect(),
        }
    }
}

// ──── MCP Server ────

#[derive(Clone)]
pub struct MemoryServer {
    db: Arc<Mutex<LoreDb>>,
    tool_router: ToolRouter<Self>,
}

impl MemoryServer {
    pub fn new(db_path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let storage = if db_path.exists() {
            Storage::open(&db_path)?
        } else {
            // Create parent directory if needed
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            Storage::open(&db_path)?
        };

        let db = LoreDb::new(storage);

        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            tool_router: Self::tool_router(),
        })
    }

    #[cfg(test)]
    pub fn new_in_memory() -> Result<Self, Box<dyn std::error::Error>> {
        let storage = Storage::open_memory()?;
        let db = LoreDb::new_without_embeddings(storage);
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            tool_router: Self::tool_router(),
        })
    }

    fn with_db<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&LoreDb) -> R,
    {
        let db = self.db.lock().unwrap();
        f(&db)
    }
}

#[tool_router]
impl MemoryServer {
    /// Flat semantic search across memory. Returns individual fragments ranked by
    /// relevance to the query, filtered to a single depth level.
    /// Use depth 0 for topic-level overviews, depth 1 for concepts, depth 2+ for details.
    /// Unlike explore_memory, this does NOT return hierarchical trees — just a flat ranked list.
    /// Best for: broad searches when you don't know which topic contains the answer.
    #[tool(name = "query_memory")]
    async fn query_memory(&self, Parameters(params): Parameters<QueryMemoryParams>) -> String {
        self.with_db(|db| {
            let results = db.query(&params.topic, params.depth, params.limit);

            if results.is_empty() {
                return format!(
                    "No memories found for topic '{}' at depth {}.",
                    params.topic, params.depth
                );
            }

            let response: Vec<FragmentResponse> = results
                .iter()
                .map(|sf| {
                    FragmentResponse::from_fragment_with_parent(&sf.fragment, Some(sf.score), db)
                })
                .collect();

            serde_json::to_string_pretty(&response)
                .unwrap_or_else(|_| "Error serializing results".to_string())
        })
    }

    /// Hierarchical tree view of a knowledge area. Finds the best-matching topic
    /// and returns it with all its children expanded up to max_depth levels.
    /// Unlike query_memory (flat list at one depth), this fans out the full subtree.
    /// Best for: drilling into a known topic to see everything stored under it.
    #[tool(name = "explore_memory")]
    async fn explore_memory(&self, Parameters(params): Parameters<ExploreMemoryParams>) -> String {
        self.with_db(|db| {
            let trees = db.explore(&params.topic, params.max_depth, params.limit);

            if trees.is_empty() {
                return format!("No knowledge trees found for topic '{}'.", params.topic);
            }

            let response: Vec<TreeResponse> = trees.iter().map(TreeResponse::from_tree).collect();

            serde_json::to_string_pretty(&response)
                .unwrap_or_else(|_| "Error serializing results".to_string())
        })
    }

    /// Navigate from a specific memory fragment. Get its children (drill deeper),
    /// parent (zoom out), or associated fragments (lateral connections).
    #[tool(name = "traverse_memory")]
    async fn traverse_memory(
        &self,
        Parameters(params): Parameters<TraverseMemoryParams>,
    ) -> String {
        let id = match FragmentId::parse(&params.fragment_id) {
            Ok(id) => id,
            Err(_) => return format!("Invalid fragment ID: {}", params.fragment_id),
        };

        self.with_db(|db| {
            let fragments = match params.direction.as_str() {
                "children" => db.children(id),
                "parent" => db.parent(id).into_iter().collect(),
                "associations" => db.associations(id),
                other => {
                    return format!(
                        "Invalid direction '{}'. Use: children, parent, associations",
                        other
                    )
                }
            };

            if fragments.is_empty() {
                return format!(
                    "No {} found for fragment {}.",
                    params.direction, params.fragment_id
                );
            }

            let response: Vec<FragmentResponse> = fragments
                .iter()
                .map(|f| FragmentResponse::from_fragment_with_parent(f, None, db))
                .collect();

            serde_json::to_string_pretty(&response)
                .unwrap_or_else(|_| "Error serializing results".to_string())
        })
    }

    /// Explicitly store a piece of knowledge in long-term memory. Provide the
    /// knowledge, a summary, an optional parent topic ID, and depth level.
    /// If no parent_id is given and depth > 0, automatically assigns to the most
    /// semantically similar existing topic.
    #[tool(name = "store_memory")]
    async fn store_memory(&self, Parameters(params): Parameters<StoreMemoryParams>) -> String {
        let explicit_parent = match params.parent_id {
            Some(ref pid) => match FragmentId::parse(pid) {
                Ok(id) => Some(id),
                Err(_) => return format!("Invalid parent ID: {}", pid),
            },
            None => None,
        };

        self.with_db(|db| {
            // Auto-classify: if depth > 0 and no explicit parent, find best matching topic
            let parent_id = if explicit_parent.is_some() {
                explicit_parent
            } else if params.depth > 0 {
                db.find_best_parent(&params.content, 0.3)
            } else {
                None
            };

            let auto_parented = explicit_parent.is_none() && parent_id.is_some();
            let parent_summary = parent_id.and_then(|pid| {
                db.parent(pid)
                    .or_else(|| db.storage().get_fragment(pid).ok().flatten())
                    .map(|f| f.summary.clone())
            });

            let fragment = Fragment::new(params.content, params.summary.clone(), params.depth);
            match db.insert(fragment, parent_id) {
                Ok(id) => {
                    let mut response = serde_json::json!({
                        "status": "stored",
                        "fragment_id": id.to_string(),
                        "summary": params.summary,
                        "depth": params.depth,
                    });
                    if let Some(pid) = parent_id {
                        response["parent_id"] = serde_json::json!(pid.to_string());
                    }
                    if let Some(ref ps) = parent_summary {
                        response["parent_summary"] = serde_json::json!(ps);
                    }
                    if auto_parented {
                        response["auto_parented"] = serde_json::json!(true);
                    }
                    serde_json::to_string_pretty(&response).unwrap()
                }
                Err(e) => format!("Failed to store memory: {}", e),
            }
        })
    }

    /// Table of contents: lists all top-level topics (depth 0) in memory.
    /// Use this first to see what knowledge domains exist before querying or exploring.
    /// Supports pagination (limit/offset) and keyword filtering. Sorted by relevance.
    #[tool(name = "list_topics")]
    async fn list_topics(&self, Parameters(params): Parameters<ListTopicsParams>) -> String {
        self.with_db(|db| {
            let topics = db.list_topics(params.query.as_deref());

            if topics.is_empty() {
                return if params.query.is_some() {
                    format!(
                        "No topics matching '{}' found.",
                        params.query.as_deref().unwrap_or("")
                    )
                } else {
                    "No topics in memory yet.".to_string()
                };
            }

            let total = topics.len();

            // Apply pagination
            let page: Vec<_> = topics
                .into_iter()
                .skip(params.offset)
                .take(params.limit)
                .collect();

            let response: Vec<TopicResponse> = page
                .iter()
                .map(|t| {
                    let child_count = db.children(t.id).len();
                    TopicResponse {
                        id: t.id.to_string(),
                        summary: t.summary.clone(),
                        content: t.content.clone(),
                        child_count,
                        relevance: t.relevance_score,
                    }
                })
                .collect();

            // Include pagination metadata
            let result = serde_json::json!({
                "total": total,
                "offset": params.offset,
                "limit": params.limit,
                "topics": response,
            });

            serde_json::to_string_pretty(&result)
                .unwrap_or_else(|_| "Error serializing results".to_string())
        })
    }

    /// Delete a memory fragment and all its edges. Use this to remove incorrect
    /// or outdated knowledge.
    #[tool(name = "delete_memory")]
    async fn delete_memory(&self, Parameters(params): Parameters<DeleteMemoryParams>) -> String {
        let id = match FragmentId::parse(&params.fragment_id) {
            Ok(id) => id,
            Err(_) => return format!("Invalid fragment ID: {}", params.fragment_id),
        };

        self.with_db(|db| match db.prune(id) {
            Ok(()) => {
                let response = serde_json::json!({
                    "status": "deleted",
                    "fragment_id": params.fragment_id,
                });
                serde_json::to_string_pretty(&response).unwrap()
            }
            Err(e) => format!("Failed to delete memory: {}", e),
        })
    }

    /// Update the content and summary of an existing memory fragment.
    /// The embedding is automatically recomputed.
    #[tool(name = "update_memory")]
    async fn update_memory(&self, Parameters(params): Parameters<UpdateMemoryParams>) -> String {
        let id = match FragmentId::parse(&params.fragment_id) {
            Ok(id) => id,
            Err(_) => return format!("Invalid fragment ID: {}", params.fragment_id),
        };

        self.with_db(|db| match db.update(id, &params.content, &params.summary) {
            Ok(()) => {
                let response = serde_json::json!({
                    "status": "updated",
                    "fragment_id": params.fragment_id,
                    "summary": params.summary,
                });
                serde_json::to_string_pretty(&response).unwrap()
            }
            Err(e) => format!("Failed to update memory: {}", e),
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Lore: Persistent memory for AI agents.\n\
             Recommended workflow:\n\
             1. list_topics — see what knowledge domains exist (table of contents)\n\
             2. explore_memory — drill into a topic to see its full subtree (hierarchical)\n\
             3. query_memory — broad semantic search across all fragments at a given depth (flat ranked list)\n\
             4. traverse_memory — navigate from a specific fragment to its children, parent, or associations\n\n\
             Key distinction: query_memory returns a flat list filtered by depth level. \
             explore_memory returns a tree rooted at the best match with children expanded. \
             Use query_memory when you don't know where to look; use explore_memory when you want \
             to see everything under a known topic.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lore_db::{fragment::now_unix, Edge, EdgeId, EdgeKind};

    fn seed_test_db(server: &MemoryServer) {
        let db = server.db.lock().unwrap();

        let topic = Fragment::new(
            "Rust programming language".to_string(),
            "Rust".to_string(),
            0,
        );
        db.storage().insert_fragment(&topic).unwrap();

        let concept = Fragment::new(
            "Async programming in Rust using tokio".to_string(),
            "Async Rust".to_string(),
            1,
        );
        db.storage().insert_fragment(&concept).unwrap();

        let edge = Edge {
            id: EdgeId::new(),
            source: topic.id,
            target: concept.id,
            kind: EdgeKind::Hierarchical,
            weight: 1.0,
            created_at: now_unix(),
        };
        db.storage().insert_edge(&edge).unwrap();

        let fact = Fragment::new(
            "Tokio uses a work-stealing scheduler for task distribution".to_string(),
            "Work-stealing scheduler".to_string(),
            2,
        );
        db.storage().insert_fragment(&fact).unwrap();

        let edge2 = Edge {
            id: EdgeId::new(),
            source: concept.id,
            target: fact.id,
            kind: EdgeKind::Hierarchical,
            weight: 1.0,
            created_at: now_unix(),
        };
        db.storage().insert_edge(&edge2).unwrap();
    }

    #[tokio::test]
    async fn test_list_topics() {
        let server = MemoryServer::new_in_memory().unwrap();
        seed_test_db(&server);

        let result = server
            .list_topics(Parameters(ListTopicsParams {
                limit: 50,
                offset: 0,
                query: None,
            }))
            .await;
        assert!(result.contains("Rust"));
    }

    #[tokio::test]
    async fn test_query_memory() {
        let server = MemoryServer::new_in_memory().unwrap();
        seed_test_db(&server);

        let result = server
            .query_memory(Parameters(QueryMemoryParams {
                topic: "Rust".to_string(),
                depth: 0,
                limit: 10,
            }))
            .await;
        assert!(result.contains("Rust"));
    }

    #[tokio::test]
    async fn test_store_memory() {
        let server = MemoryServer::new_in_memory().unwrap();

        let result = server
            .store_memory(Parameters(StoreMemoryParams {
                content: "Python is a dynamic language".to_string(),
                summary: "Python".to_string(),
                parent_id: None,
                depth: 0,
            }))
            .await;
        assert!(result.contains("stored"));
        assert!(result.contains("fragment_id"));
    }

    #[tokio::test]
    async fn test_traverse_memory() {
        let server = MemoryServer::new_in_memory().unwrap();
        seed_test_db(&server);

        let topic_id = {
            let db = server.db.lock().unwrap();
            let topics = db.list_topics(None);
            topics[0].id.to_string()
        };

        let result = server
            .traverse_memory(Parameters(TraverseMemoryParams {
                fragment_id: topic_id,
                direction: "children".to_string(),
            }))
            .await;
        assert!(result.contains("Async Rust"));
    }

    #[tokio::test]
    async fn test_traverse_invalid_direction() {
        let server = MemoryServer::new_in_memory().unwrap();

        let result = server
            .traverse_memory(Parameters(TraverseMemoryParams {
                fragment_id: "00000000-0000-0000-0000-000000000000".to_string(),
                direction: "sideways".to_string(),
            }))
            .await;
        assert!(result.contains("Invalid direction"));
    }

    #[tokio::test]
    async fn test_list_topics_with_query_filter() {
        let server = MemoryServer::new_in_memory().unwrap();
        seed_test_db(&server);

        // Store a second topic
        {
            let db = server.db.lock().unwrap();
            let topic2 = Fragment::new(
                "Python programming language".to_string(),
                "Python".to_string(),
                0,
            );
            db.storage().insert_fragment(&topic2).unwrap();
        }

        // Filter by "Python" should only return Python
        let result = server
            .list_topics(Parameters(ListTopicsParams {
                limit: 50,
                offset: 0,
                query: Some("Python".to_string()),
            }))
            .await;
        assert!(result.contains("Python"));
        assert!(!result.contains("\"summary\": \"Rust\""));

        // Pagination metadata should be present
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["total"], 1);
    }

    #[tokio::test]
    async fn test_list_topics_pagination() {
        let server = MemoryServer::new_in_memory().unwrap();
        {
            let db = server.db.lock().unwrap();
            for i in 0..5 {
                let t = Fragment::new(
                    format!("Topic {i} content"),
                    format!("Topic {i}"),
                    0,
                );
                db.storage().insert_fragment(&t).unwrap();
            }
        }

        // Get first 2
        let result = server
            .list_topics(Parameters(ListTopicsParams {
                limit: 2,
                offset: 0,
                query: None,
            }))
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["total"], 5);
        assert_eq!(parsed["topics"].as_array().unwrap().len(), 2);

        // Get next 2
        let result = server
            .list_topics(Parameters(ListTopicsParams {
                limit: 2,
                offset: 2,
                query: None,
            }))
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["topics"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_delete_memory() {
        let server = MemoryServer::new_in_memory().unwrap();

        // Store, then delete
        let store_result = server
            .store_memory(Parameters(StoreMemoryParams {
                content: "Temporary fact".to_string(),
                summary: "Temp".to_string(),
                parent_id: None,
                depth: 0,
            }))
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&store_result).unwrap();
        let frag_id = parsed["fragment_id"].as_str().unwrap().to_string();

        let delete_result = server
            .delete_memory(Parameters(DeleteMemoryParams {
                fragment_id: frag_id.clone(),
            }))
            .await;
        assert!(delete_result.contains("deleted"));

        // Verify it's gone
        let list_result = server
            .list_topics(Parameters(ListTopicsParams {
                limit: 50,
                offset: 0,
                query: None,
            }))
            .await;
        assert!(!list_result.contains("Temp"));
    }

    #[tokio::test]
    async fn test_update_memory() {
        let server = MemoryServer::new_in_memory().unwrap();

        let store_result = server
            .store_memory(Parameters(StoreMemoryParams {
                content: "Original content".to_string(),
                summary: "Original".to_string(),
                parent_id: None,
                depth: 0,
            }))
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&store_result).unwrap();
        let frag_id = parsed["fragment_id"].as_str().unwrap().to_string();

        let update_result = server
            .update_memory(Parameters(UpdateMemoryParams {
                fragment_id: frag_id.clone(),
                content: "Updated content".to_string(),
                summary: "Updated".to_string(),
            }))
            .await;
        assert!(update_result.contains("updated"));

        // Verify content changed
        let list_result = server
            .list_topics(Parameters(ListTopicsParams {
                limit: 50,
                offset: 0,
                query: None,
            }))
            .await;
        assert!(list_result.contains("Updated"));
        assert!(!list_result.contains("Original"));
    }

    #[tokio::test]
    async fn test_query_returns_parent_breadcrumb() {
        let server = MemoryServer::new_in_memory().unwrap();
        seed_test_db(&server);

        // Query at depth 1 — should return "Async Rust" with parent "Rust"
        let result = server
            .query_memory(Parameters(QueryMemoryParams {
                topic: "async".to_string(),
                depth: 1,
                limit: 10,
            }))
            .await;
        assert!(result.contains("parent_summary"));
        assert!(result.contains("Rust"));
    }
}
