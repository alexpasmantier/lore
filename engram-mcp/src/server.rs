use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use engram_db::{EngramDb, Fragment, FragmentId, Storage, Tree};
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ──── Parameter types for MCP tools ────

#[derive(Deserialize, JsonSchema)]
pub struct QueryMemoryParams {
    /// What to search for
    pub topic: String,
    /// Zoom level (0=overview, deeper=more detail)
    #[serde(default = "default_depth")]
    pub depth: u32,
    /// Max results to return
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Deserialize, JsonSchema)]
pub struct ExploreMemoryParams {
    /// The topic to explore
    pub topic: String,
    /// How many levels deep to show
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,
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
    /// Parent fragment ID (null for new top-level topic)
    pub parent_id: Option<String>,
    /// Zoom level (0=overview, deeper=more detail)
    #[serde(default = "default_store_depth")]
    pub depth: u32,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListTopicsParams {}

fn default_depth() -> u32 {
    0
}
fn default_limit() -> usize {
    10
}
fn default_max_depth() -> u32 {
    2
}
fn default_store_depth() -> u32 {
    2
}

// ──── Response types ────

#[derive(Serialize)]
struct FragmentResponse {
    id: String,
    summary: String,
    content: String,
    depth: u32,
    score: Option<f32>,
}

#[derive(Serialize)]
struct TreeResponse {
    id: String,
    summary: String,
    content: String,
    depth: u32,
    children: Vec<TreeResponse>,
}

#[derive(Serialize)]
struct TopicResponse {
    id: String,
    summary: String,
    content: String,
    child_count: usize,
}

impl FragmentResponse {
    fn from_fragment(f: &Fragment, score: Option<f32>) -> Self {
        Self {
            id: f.id.to_string(),
            summary: f.summary.clone(),
            content: f.content.clone(),
            depth: f.depth,
            score,
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
            children: tree.children.iter().map(TreeResponse::from_tree).collect(),
        }
    }
}

// ──── MCP Server ────

#[derive(Clone)]
pub struct MemoryServer {
    db: Arc<Mutex<EngramDb>>,
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

        let db = EngramDb::new(storage);

        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            tool_router: Self::tool_router(),
        })
    }

    #[cfg(test)]
    pub fn new_in_memory() -> Result<Self, Box<dyn std::error::Error>> {
        let storage = Storage::open_memory()?;
        let db = EngramDb::new_without_embeddings(storage);
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            tool_router: Self::tool_router(),
        })
    }

    fn with_db<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&EngramDb) -> R,
    {
        let db = self.db.lock().unwrap();
        f(&db)
    }
}

#[tool_router]
impl MemoryServer {
    /// Search long-term memory for knowledge about a topic. Returns fragments at
    /// the specified zoom level (0=rich overviews, deeper=more detail).
    /// Start at depth 0 for self-contained summaries, drill deeper as needed.
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
                .map(|sf| FragmentResponse::from_fragment(&sf.fragment, Some(sf.score)))
                .collect();

            serde_json::to_string_pretty(&response)
                .unwrap_or_else(|_| "Error serializing results".to_string())
        })
    }

    /// Get a zoom-tree view of a knowledge area. Returns a hierarchical tree
    /// starting from the best matching topic, with each level drilling deeper into detail.
    #[tool(name = "explore_memory")]
    async fn explore_memory(&self, Parameters(params): Parameters<ExploreMemoryParams>) -> String {
        self.with_db(|db| {
            let trees = db.explore(&params.topic, params.max_depth);

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
                .map(|f| FragmentResponse::from_fragment(f, None))
                .collect();

            serde_json::to_string_pretty(&response)
                .unwrap_or_else(|_| "Error serializing results".to_string())
        })
    }

    /// Explicitly store a piece of knowledge in long-term memory. Provide the
    /// knowledge, a summary, an optional parent topic ID, and depth level.
    #[tool(name = "store_memory")]
    async fn store_memory(&self, Parameters(params): Parameters<StoreMemoryParams>) -> String {
        let parent_id = match params.parent_id {
            Some(ref pid) => match FragmentId::parse(pid) {
                Ok(id) => Some(id),
                Err(_) => return format!("Invalid parent ID: {}", pid),
            },
            None => None,
        };

        self.with_db(|db| {
            let fragment = Fragment::new(params.content, params.summary.clone(), params.depth);
            match db.insert(fragment, parent_id) {
                Ok(id) => {
                    let response = serde_json::json!({
                        "status": "stored",
                        "fragment_id": id.to_string(),
                        "summary": params.summary,
                        "depth": params.depth,
                    });
                    serde_json::to_string_pretty(&response).unwrap()
                }
                Err(e) => format!("Failed to store memory: {}", e),
            }
        })
    }

    /// List all top-level knowledge domains in memory with their summaries
    /// and child counts.
    #[tool(name = "list_topics")]
    async fn list_topics(&self, Parameters(_params): Parameters<ListTopicsParams>) -> String {
        self.with_db(|db| {
            let topics = db.list_topics();

            if topics.is_empty() {
                return "No topics in memory yet.".to_string();
            }

            let response: Vec<TopicResponse> = topics
                .iter()
                .map(|t| {
                    let child_count = db.children(t.id).len();
                    TopicResponse {
                        id: t.id.to_string(),
                        summary: t.summary.clone(),
                        content: t.content.clone(),
                        child_count,
                    }
                })
                .collect();

            serde_json::to_string_pretty(&response)
                .unwrap_or_else(|_| "Error serializing results".to_string())
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Engram: Brain-inspired persistent memory for AI agents. \
                 Query knowledge at different zoom levels (0=overview, deeper=more detail). \
                 Start with list_topics or query_memory at depth 0, then drill deeper.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engram_db::{fragment::now_unix, Edge, EdgeId, EdgeKind};

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

        let result = server.list_topics(Parameters(ListTopicsParams {})).await;
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
            let topics = db.list_topics();
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
}
