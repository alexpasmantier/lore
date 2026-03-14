use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use lore_db::Storage;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpService,
};
use serde::{Deserialize, Serialize};

use lore_mcp::server::MemoryServer;

#[derive(Parser)]
#[command(name = "lore-server", about = "HTTP server for lore memory system")]
struct Cli {
    /// Port to listen on
    #[arg(short, long, default_value = "8080")]
    port: u16,

    /// Path to the lore database
    #[arg(long)]
    db: Option<PathBuf>,
}

#[derive(Clone)]
struct AppState {
    storage: Arc<Mutex<Storage>>,
}

// ──── Push API types ────

#[derive(Deserialize)]
struct PushRequest {
    /// Client identifier (machine name, user, etc.)
    client_id: String,
    /// Staged turns to push
    turns: Vec<PushTurn>,
}

#[derive(Deserialize)]
struct PushTurn {
    /// Session file path (unique per conversation)
    session: String,
    role: String,
    text: String,
}

#[derive(Serialize)]
struct PushResponse {
    status: String,
    staged: usize,
}

#[derive(Serialize)]
struct StatusResponse {
    status: String,
    staged_sessions: usize,
    staged_turns: usize,
}

// ──── Handlers ────

async fn handle_push(
    State(state): State<AppState>,
    Json(req): Json<PushRequest>,
) -> Result<Json<PushResponse>, StatusCode> {
    let storage = state.storage.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut total = 0;
    // Group turns by session
    let mut sessions: std::collections::HashMap<&str, Vec<(&str, &str)>> =
        std::collections::HashMap::new();
    for turn in &req.turns {
        sessions
            .entry(&turn.session)
            .or_default()
            .push((&turn.role, &turn.text));
    }

    for (session, turns) in &sessions {
        // Prefix session with client_id to avoid collisions across machines
        let key = format!("{}:{}", req.client_id, session);
        let count = storage
            .stage_turns(&key, turns)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        total += count;
    }

    tracing::info!(
        "Pushed {} turns from client '{}' ({} sessions)",
        total,
        req.client_id,
        sessions.len()
    );

    Ok(Json(PushResponse {
        status: "ok".to_string(),
        staged: total,
    }))
}

async fn handle_status(State(state): State<AppState>) -> Result<Json<StatusResponse>, StatusCode> {
    let storage = state.storage.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let now = lore_db::fragment::now_unix();
    let sessions = storage
        .get_staged_sessions(0, now + 1)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let total_turns: usize = sessions.iter().map(|s| s.turn_count).sum();

    Ok(Json(StatusResponse {
        status: "ok".to_string(),
        staged_sessions: sessions.len(),
        staged_turns: total_turns,
    }))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("lore_server=info".parse().unwrap())
                .add_directive("lore_mcp=info".parse().unwrap()),
        )
        .init();

    let db_path = cli
        .db
        .unwrap_or_else(|| lore_db::lore_home().join("memory.db"));

    tracing::info!("Database: {}", db_path.display());
    tracing::info!("Listening on 0.0.0.0:{}", cli.port);

    // Shared storage for the push API
    let storage = Storage::open(&db_path)?;
    let state = AppState {
        storage: Arc::new(Mutex::new(storage)),
    };

    // MCP service (each session gets its own MemoryServer + LoreDb)
    let mcp_db_path = db_path.clone();
    let session_manager = LocalSessionManager::default();
    let mcp_service = StreamableHttpService::new(
        move || {
            MemoryServer::new(mcp_db_path.clone())
                .map_err(|e| std::io::Error::other(e.to_string()))
        },
        session_manager.into(),
        Default::default(),
    );

    let app = Router::new()
        .route("/push", post(handle_push))
        .route("/status", get(handle_status))
        .nest_service("/mcp", mcp_service)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", cli.port)).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
