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
                .add_directive("lore_mcp=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    let db_path = std::env::var("LORE_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| lore_db::lore_home().join("memory.db"));

    tracing::info!("Starting lore MCP server with db: {}", db_path.display());

    let server = MemoryServer::new(db_path)?;
    let transport = rmcp::transport::stdio();
    let service = server.serve(transport).await?;
    service.waiting().await?;

    Ok(())
}
