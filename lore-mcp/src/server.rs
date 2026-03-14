use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lore_db::{Fragment, FragmentId, LoreDb, Storage};
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ──── Parameter types for MCP tools ────

#[derive(Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Semantic search query
    pub query: String,
    /// Optional parent ID to restrict search to descendants of this fragment
    pub parent_id: Option<String>,
    /// Max results to return
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadParams {
    /// The fragment ID to read
    pub id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListRootsParams {
    /// Max number of roots to return (default: 20)
    #[serde(default = "default_list_limit")]
    pub limit: usize,
}

#[derive(Deserialize, JsonSchema)]
pub struct StoreMemoryParams {
    /// The knowledge to store
    pub content: String,
    /// Parent fragment ID. If omitted for depth > 0, auto-assigns to the most
    /// semantically similar existing root.
    pub parent_id: Option<String>,
    /// Abstraction level (0=broad concept, higher=more specific)
    #[serde(default = "default_store_depth")]
    pub depth: u32,
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
}

fn default_limit() -> usize {
    10
}
fn default_store_depth() -> u32 {
    1
}
fn default_list_limit() -> usize {
    20
}

// ──── Response types ────

#[derive(Serialize)]
struct SearchHit {
    id: String,
    score: f32,
    depth: u32,
}

#[derive(Serialize)]
struct RootHit {
    id: String,
    children_count: usize,
}

#[derive(Serialize)]
struct ReadResponse {
    id: String,
    content: String,
    depth: u32,
    relevance: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<String>,
    children: Vec<String>,
    associations: Vec<AssociationResponse>,
}

#[derive(Serialize)]
struct AssociationResponse {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    relationship: Option<String>,
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
    /// Semantic search across memory. Returns fragment IDs and scores — no content.
    /// Use `read` to get the content of specific results.
    /// If parent_id is provided, only searches within descendants of that fragment.
    #[tool(name = "search")]
    async fn search(&self, Parameters(params): Parameters<SearchParams>) -> String {
        let scope = match &params.parent_id {
            Some(pid) => match FragmentId::parse(pid) {
                Ok(id) => Some(id),
                Err(_) => return format!("Invalid parent ID: {}", pid),
            },
            None => None,
        };

        self.with_db(|db| {
            let results = if let Some(parent_id) = scope {
                // Search within children of the given parent
                let children = db.children(parent_id);
                if children.is_empty() {
                    return "No children found for this fragment.".to_string();
                }

                let query_embedding = match db.embed_text(&params.query) {
                    Some(e) => e,
                    None => {
                        // Text fallback: filter children by keyword match
                        let query_lower = params.query.to_lowercase();
                        let hits: Vec<SearchHit> = children
                            .iter()
                            .filter(|c| c.content.to_lowercase().contains(&query_lower))
                            .take(params.limit)
                            .map(|c| SearchHit {
                                id: c.id.to_string(),
                                score: c.relevance_score,
                                depth: c.depth,
                            })
                            .collect();
                        return serde_json::to_string_pretty(&hits)
                            .unwrap_or_else(|_| "[]".to_string());
                    }
                };

                let mut scored: Vec<_> = children
                    .into_iter()
                    .filter(|f| !f.embedding.is_empty())
                    .map(|f| {
                        let sim =
                            lore_db::cosine_similarity(&query_embedding, &f.embedding);
                        let score = 0.7 * sim + 0.3 * f.relevance_score;
                        (f, score)
                    })
                    .collect();
                scored.sort_by(|a, b| {
                    b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                });
                scored.truncate(params.limit);

                scored
                    .iter()
                    .map(|(f, score)| SearchHit {
                        id: f.id.to_string(),
                        score: *score,
                        depth: f.depth,
                    })
                    .collect::<Vec<_>>()
            } else {
                // Global search
                let scored = db.query(&params.query, 0, params.limit);
                scored
                    .iter()
                    .map(|sf| SearchHit {
                        id: sf.fragment.id.to_string(),
                        score: sf.score,
                        depth: sf.fragment.depth,
                    })
                    .collect::<Vec<_>>()
            };

            if results.is_empty() {
                return format!("No results for '{}'.", params.query);
            }

            serde_json::to_string_pretty(&results)
                .unwrap_or_else(|_| "Error serializing results".to_string())
        })
    }

    /// Read the full content of a specific fragment, plus its structural connections
    /// (parent ID, children IDs, association IDs) for navigation.
    #[tool(name = "read")]
    async fn read(&self, Parameters(params): Parameters<ReadParams>) -> String {
        let id = match FragmentId::parse(&params.id) {
            Ok(id) => id,
            Err(_) => return format!("Invalid fragment ID: {}", params.id),
        };

        self.with_db(|db| {
            let fragment = match db.storage().get_fragment(id) {
                Ok(Some(f)) => f,
                Ok(None) => return format!("Fragment {} not found.", params.id),
                Err(e) => return format!("Error reading fragment: {}", e),
            };

            // Reinforce on access
            db.reinforce_on_access(id);

            let parent_id = db.parent(id).map(|p| p.id.to_string());
            let children: Vec<String> =
                db.children(id).iter().map(|c| c.id.to_string()).collect();
            let assoc_edges = db.storage().get_edges_for(id).unwrap_or_default();
            let associations: Vec<AssociationResponse> = assoc_edges
                .iter()
                .filter(|e| e.kind == lore_db::EdgeKind::Associative)
                .map(|e| {
                    let other_id = if e.source == id { e.target } else { e.source };
                    AssociationResponse {
                        id: other_id.to_string(),
                        relationship: e.content.clone(),
                    }
                })
                .collect();

            let response = ReadResponse {
                id: fragment.id.to_string(),
                content: fragment.content,
                depth: fragment.depth,
                relevance: fragment.relevance_score,
                parent_id,
                children,
                associations,
            };

            serde_json::to_string_pretty(&response)
                .unwrap_or_else(|_| "Error serializing fragment".to_string())
        })
    }

    /// List root-level fragments (depth 0) — the broadest knowledge areas.
    /// Returns just IDs and child counts. Use `read` to see content.
    #[tool(name = "list_roots")]
    async fn list_roots(&self, Parameters(params): Parameters<ListRootsParams>) -> String {
        self.with_db(|db| {
            let roots = db.list_roots(None);

            if roots.is_empty() {
                return "No knowledge stored yet.".to_string();
            }

            let response: Vec<RootHit> = roots
                .iter()
                .take(params.limit)
                .map(|r| RootHit {
                    id: r.id.to_string(),
                    children_count: db.children(r.id).len(),
                })
                .collect();

            serde_json::to_string_pretty(&response)
                .unwrap_or_else(|_| "Error serializing roots".to_string())
        })
    }

    /// Store a piece of knowledge. Provide content, optional parent ID,
    /// and depth (0=broad concept, higher=more specific).
    #[tool(name = "store")]
    async fn store(&self, Parameters(params): Parameters<StoreMemoryParams>) -> String {
        let explicit_parent = match params.parent_id {
            Some(ref pid) => match FragmentId::parse(pid) {
                Ok(id) => Some(id),
                Err(_) => return format!("Invalid parent ID: {}", pid),
            },
            None => None,
        };

        self.with_db(|db| {
            let parent_id = if explicit_parent.is_some() {
                explicit_parent
            } else if params.depth > 0 {
                db.find_best_parent(&params.content, 0.3)
            } else {
                None
            };

            let fragment = Fragment::new(params.content, params.depth);
            match db.insert(fragment, parent_id) {
                Ok(id) => {
                    let mut response = serde_json::json!({
                        "status": "stored",
                        "id": id.to_string(),
                        "depth": params.depth,
                    });
                    if let Some(pid) = parent_id {
                        response["parent_id"] = serde_json::json!(pid.to_string());
                    }
                    serde_json::to_string_pretty(&response).unwrap()
                }
                Err(e) => format!("Failed to store: {}", e),
            }
        })
    }

    /// Delete a fragment and all its edges.
    #[tool(name = "delete")]
    async fn delete(&self, Parameters(params): Parameters<DeleteMemoryParams>) -> String {
        let id = match FragmentId::parse(&params.fragment_id) {
            Ok(id) => id,
            Err(_) => return format!("Invalid fragment ID: {}", params.fragment_id),
        };

        self.with_db(|db| match db.prune(id) {
            Ok(()) => {
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "deleted",
                    "id": params.fragment_id,
                }))
                .unwrap()
            }
            Err(e) => format!("Failed to delete: {}", e),
        })
    }

    /// Update the content of a fragment. The embedding is recomputed automatically.
    #[tool(name = "update")]
    async fn update(&self, Parameters(params): Parameters<UpdateMemoryParams>) -> String {
        let id = match FragmentId::parse(&params.fragment_id) {
            Ok(id) => id,
            Err(_) => return format!("Invalid fragment ID: {}", params.fragment_id),
        };

        self.with_db(|db| match db.update(id, &params.content) {
            Ok(()) => {
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "updated",
                    "id": params.fragment_id,
                }))
                .unwrap()
            }
            Err(e) => format!("Failed to update: {}", e),
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Lore: Persistent memory for AI agents. Knowledge is organized as interconnected \
             abstraction trees — broad concepts at the top, conversation-specific details deeper \
             down, with associative edges linking related fragments across trees.\n\n\
             Workflow: search → read → search deeper → read.\n\
             1. search(query) — find relevant fragments by semantic similarity (returns IDs only)\n\
             2. read(id) — read content + see children/associations for navigation\n\
             3. search(query, parent_id=id) — narrow search within a subtree\n\
             4. Repeat until you have the detail you need.\n\n\
             list_roots shows all top-level knowledge areas (IDs only).\n\
             Each step is lightweight — content is only loaded when you explicitly read.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lore_db::{fragment::now_unix, Edge, EdgeId, EdgeKind};

    fn seed_test_db(server: &MemoryServer) {
        let db = server.db.lock().unwrap();

        let root = Fragment::new("Rust programming language".to_string(), 0);
        db.storage().insert_fragment(&root).unwrap();

        let child = Fragment::new(
            "Async Rust: Async programming in Rust using tokio".to_string(),
            1,
        );
        db.storage().insert_fragment(&child).unwrap();

        let edge = Edge {
            id: EdgeId::new(),
            source: root.id,
            target: child.id,
            kind: EdgeKind::Hierarchical,
            weight: 1.0,
            content: None,
            created_at: now_unix(),
        };
        db.storage().insert_edge(&edge).unwrap();

        let leaf = Fragment::new(
            "Tokio uses a work-stealing scheduler for task distribution".to_string(),
            2,
        );
        db.storage().insert_fragment(&leaf).unwrap();

        let edge2 = Edge {
            id: EdgeId::new(),
            source: child.id,
            target: leaf.id,
            kind: EdgeKind::Hierarchical,
            weight: 1.0,
            content: None,
            created_at: now_unix(),
        };
        db.storage().insert_edge(&edge2).unwrap();
    }

    #[tokio::test]
    async fn test_list_roots() {
        let server = MemoryServer::new_in_memory().unwrap();
        seed_test_db(&server);

        let result = server
            .list_roots(Parameters(ListRootsParams { limit: 20 }))
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let roots = parsed.as_array().unwrap();
        assert_eq!(roots.len(), 1);
        assert!(roots[0]["id"].is_string());
        assert_eq!(roots[0]["children_count"], 1);
        // No content in list_roots response
        assert!(roots[0].get("content").is_none());
    }

    #[tokio::test]
    async fn test_search_global() {
        let server = MemoryServer::new_in_memory().unwrap();
        seed_test_db(&server);

        let result = server
            .search(Parameters(SearchParams {
                query: "Rust".to_string(),
                parent_id: None,
                limit: 10,
            }))
            .await;
        assert!(result.contains("score"));
        assert!(result.contains("depth"));
        // No content in search response
        assert!(!result.contains("Rust programming language"));
    }

    #[tokio::test]
    async fn test_search_scoped() {
        let server = MemoryServer::new_in_memory().unwrap();
        seed_test_db(&server);

        let root_id = {
            let db = server.db.lock().unwrap();
            db.list_roots(None)[0].id.to_string()
        };

        // Search within children of root — text fallback (no embeddings in test)
        let result = server
            .search(Parameters(SearchParams {
                query: "Async".to_string(),
                parent_id: Some(root_id),
                limit: 10,
            }))
            .await;
        assert!(result.contains("score"));
    }

    #[tokio::test]
    async fn test_read_fragment() {
        let server = MemoryServer::new_in_memory().unwrap();
        seed_test_db(&server);

        let root_id = {
            let db = server.db.lock().unwrap();
            db.list_roots(None)[0].id.to_string()
        };

        let result = server
            .read(Parameters(ReadParams {
                id: root_id.clone(),
            }))
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // Content is returned
        assert!(parsed["content"]
            .as_str()
            .unwrap()
            .contains("Rust programming"));
        assert_eq!(parsed["depth"], 0);
        // Children IDs are returned
        assert_eq!(parsed["children"].as_array().unwrap().len(), 1);
        // No parent for root
        assert!(parsed["parent_id"].is_null());
    }

    #[tokio::test]
    async fn test_read_returns_parent_and_children() {
        let server = MemoryServer::new_in_memory().unwrap();
        seed_test_db(&server);

        // Get the child (depth 1) ID
        let (root_id, child_id) = {
            let db = server.db.lock().unwrap();
            let roots = db.list_roots(None);
            let children = db.children(roots[0].id);
            (roots[0].id.to_string(), children[0].id.to_string())
        };

        let result = server
            .read(Parameters(ReadParams { id: child_id }))
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // Has parent
        assert_eq!(parsed["parent_id"].as_str().unwrap(), root_id);
        // Has child (the leaf)
        assert_eq!(parsed["children"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_store() {
        let server = MemoryServer::new_in_memory().unwrap();

        let result = server
            .store(Parameters(StoreMemoryParams {
                content: "Python is a dynamic language".to_string(),
                parent_id: None,
                depth: 0,
            }))
            .await;
        assert!(result.contains("stored"));
        assert!(result.contains("\"id\""));
    }

    #[tokio::test]
    async fn test_delete() {
        let server = MemoryServer::new_in_memory().unwrap();

        let store_result = server
            .store(Parameters(StoreMemoryParams {
                content: "Temporary".to_string(),
                parent_id: None,
                depth: 0,
            }))
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&store_result).unwrap();
        let frag_id = parsed["id"].as_str().unwrap().to_string();

        let result = server
            .delete(Parameters(DeleteMemoryParams {
                fragment_id: frag_id,
            }))
            .await;
        assert!(result.contains("deleted"));
    }

    #[tokio::test]
    async fn test_update() {
        let server = MemoryServer::new_in_memory().unwrap();

        let store_result = server
            .store(Parameters(StoreMemoryParams {
                content: "Original".to_string(),
                parent_id: None,
                depth: 0,
            }))
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&store_result).unwrap();
        let frag_id = parsed["id"].as_str().unwrap().to_string();

        let result = server
            .update(Parameters(UpdateMemoryParams {
                fragment_id: frag_id,
                content: "Updated".to_string(),
            }))
            .await;
        assert!(result.contains("updated"));
    }

    #[tokio::test]
    async fn test_read_not_found() {
        let server = MemoryServer::new_in_memory().unwrap();

        let result = server
            .read(Parameters(ReadParams {
                id: "00000000-0000-0000-0000-000000000000".to_string(),
            }))
            .await;
        assert!(result.contains("not found"));
    }
}
