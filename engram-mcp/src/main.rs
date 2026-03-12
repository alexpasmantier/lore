mod server;

use rmcp::ServiceExt;
use std::path::PathBuf;

use server::MemoryServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Log to stderr (stdout is reserved for MCP JSON-RPC protocol)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("engram_mcp=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    let db_path = std::env::var("ENGRAM_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_path().join("memory.db"));

    tracing::info!("Starting engram MCP server with db: {}", db_path.display());

    let server = MemoryServer::new(db_path)?;
    let transport = rmcp::transport::stdio();
    let service = server.serve(transport).await?;
    service.waiting().await?;

    Ok(())
}

fn dirs_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".engram")
}
