use std::path::PathBuf;

use clap::Parser;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpService,
};

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

    let session_manager = LocalSessionManager::default();
    let service = StreamableHttpService::new(
        move || {
            MemoryServer::new(db_path.clone())
                .map_err(|e| std::io::Error::other(e.to_string()))
        },
        session_manager.into(),
        Default::default(),
    );

    let app = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", cli.port)).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
