use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use clap::{Parser, Subcommand};
use lore_db::{LoreDb, Storage};

use lore_daemon::claude_client::ClaudeClient;
use lore_daemon::config::Config;
use lore_daemon::watcher::FileWatcher;
use lore_daemon::{consolidation, ingestion, parser};

#[derive(Parser)]
#[command(
    name = "lore-daemon",
    about = "Background daemon for lore memory system"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Path to config file
    #[arg(long, default_value = "~/.lore/config.toml")]
    config: String,
}

#[derive(Subcommand)]
enum Command {
    /// Start the daemon in the foreground
    Start {
        /// Write logs to a file instead of stderr
        #[arg(long)]
        log_file: Option<PathBuf>,
    },
    /// Start the daemon in the background
    Daemonize,
    /// Stop a running daemon
    Stop,
    /// Show daemon status
    Status,
    /// Run a single ingestion pass (useful for testing)
    Ingest,
    /// Run a single consolidation pass
    Consolidate,
    /// Tail the daemon log file
    Logs {
        /// Number of lines to show initially
        #[arg(short, long, default_value = "50")]
        lines: usize,
        /// Follow the log output (like tail -f)
        #[arg(short, long)]
        follow: bool,
    },
}

fn pid_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".lore").join("daemon.pid")
}

fn log_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".lore").join("daemon.log")
}

fn config_path(raw: &str) -> PathBuf {
    let expanded = raw.replace('~', &std::env::var("HOME").unwrap_or_default());
    PathBuf::from(expanded)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let env_filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive("lore_daemon=info".parse().unwrap());

    // If `start --log-file` is used, write tracing to that file instead of stderr
    let log_file_path = match &cli.command {
        Command::Start { log_file } => log_file.clone(),
        _ => None,
    };

    if let Some(ref path) = log_file_path {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(Mutex::new(file))
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .init();
    }

    let config = Config::load(&config_path(&cli.config))?;

    match cli.command {
        Command::Start { .. } => run_foreground(config).await?,
        Command::Daemonize => daemonize(config)?,
        Command::Stop => stop_daemon()?,
        Command::Status => show_status()?,
        Command::Ingest => run_single_ingest(config).await?,
        Command::Consolidate => run_single_consolidation(config).await?,
        Command::Logs { lines, follow } => tail_logs(lines, follow)?,
    }

    Ok(())
}

async fn run_foreground(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    // Write PID file
    let pid = std::process::id();
    let pid_path = pid_file();
    if let Some(parent) = pid_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&pid_path, pid.to_string())?;

    tracing::info!("Lore daemon started (PID: {})", pid);
    tracing::info!("Database: {}", config.db_path().display());
    tracing::info!(
        "Poll interval: {}s, Consolidation interval: {}s",
        config.ingestion.poll_interval_secs,
        config.consolidation.interval_secs
    );

    // Open database
    let db_path = config.db_path();
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let storage = Storage::open(&db_path)?;
    let db = LoreDb::new(storage);

    // Create Claude client (API key if available, otherwise `claude -p` fallback)
    let client = ClaudeClient::auto(
        &config.claude.api_key_env,
        config.ingestion.claude_model.clone(),
    );

    let watcher = FileWatcher::new();

    // Run ingestion and consolidation loops concurrently
    let ingestion_interval = Duration::from_secs(config.ingestion.poll_interval_secs);
    let consolidation_interval = Duration::from_secs(config.consolidation.interval_secs);
    let batch_size = config.ingestion.batch_size;
    let consolidation_config = config.consolidation.clone();

    // Handle shutdown gracefully
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    ctrlc_handler(shutdown_tx);

    let mut ingestion_timer = tokio::time::interval(ingestion_interval);
    let mut consolidation_timer = tokio::time::interval(consolidation_interval);

    // Skip the first immediate tick for consolidation (let ingestion run first)
    consolidation_timer.tick().await;

    loop {
        tokio::select! {
            _ = ingestion_timer.tick() => {
                if let Err(e) = run_ingestion_pass(&db, &watcher, &client, batch_size).await {
                    tracing::error!("Ingestion error: {}", e);
                }
            }
            _ = consolidation_timer.tick() => {
                if let Err(e) = consolidation::run_consolidation(
                    &db,
                    Some(&client),
                    &consolidation_config,
                ).await {
                    tracing::error!("Consolidation error: {}", e);
                }
            }
            _ = shutdown_rx.changed() => {
                tracing::info!("Shutting down...");
                break;
            }
        }
    }

    // Cleanup PID file
    let _ = std::fs::remove_file(&pid_path);
    tracing::info!("Daemon stopped.");
    Ok(())
}

/// Max concurrent Claude calls during ingestion.
const INGESTION_CONCURRENCY: usize = 4;

async fn run_ingestion_pass(
    db: &LoreDb,
    watcher: &FileWatcher,
    client: &ClaudeClient,
    batch_size: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let files = watcher.find_conversation_files();
    tracing::debug!("Found {} conversation files", files.len());

    // Phase 1: Read all files and collect work items (no Claude calls yet)
    let existing_topics: Vec<ingestion::ExistingTopicContext> = db
        .list_topics()
        .into_iter()
        .map(|t| {
            let children_summaries = db
                .children(t.id)
                .into_iter()
                .map(|c| c.summary)
                .collect();
            ingestion::ExistingTopicContext {
                id: t.id.to_string(),
                summary: t.summary.clone(),
                content: t.content.clone(),
                children_summaries,
            }
        })
        .collect();

    struct WorkItem {
        file_path: PathBuf,
        session_id: Option<String>,
        chunks: Vec<Vec<parser::ConversationTurn>>,
        new_offset: i64,
    }

    let mut work_items = Vec::new();

    for file_path in &files {
        let (turns, new_offset) = watcher.read_new_turns(file_path, db.storage())?;

        if turns.is_empty() {
            continue;
        }

        tracing::info!(
            "Processing {} new turns from {}",
            turns.len(),
            file_path.display()
        );

        let session_id = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());

        let chunks: Vec<Vec<_>> = turns.chunks(batch_size).map(|c| c.to_vec()).collect();

        work_items.push(WorkItem {
            file_path: file_path.clone(),
            session_id,
            chunks,
            new_offset,
        });
    }

    if work_items.is_empty() {
        return Ok(());
    }

    // Phase 2: Fan out all Claude calls in parallel with concurrency limit
    // Collect (work_item_index, chunk_index, result) tuples
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(INGESTION_CONCURRENCY));
    let mut handles = Vec::new();

    for (wi_idx, item) in work_items.iter().enumerate() {
        for (chunk_idx, chunk) in item.chunks.iter().enumerate() {
            let sem = semaphore.clone();
            let chunk = chunk.clone();
            let topics = existing_topics.clone();
            // ClaudeClient is not Clone, so we need to call from the current task.
            // Instead, collect futures and use buffered execution.
            handles.push((wi_idx, chunk_idx, chunk, topics, sem));
        }
    }

    // Build futures and run with buffered concurrency
    use futures::stream::{self, StreamExt};

    let results: Vec<_> = stream::iter(handles.into_iter().map(
        |(wi_idx, chunk_idx, chunk, topics, sem)| async move {
            let _permit = sem.acquire().await.unwrap();
            let result = ingestion::extract_knowledge(client, &chunk, &topics).await;
            (wi_idx, chunk_idx, result)
        },
    ))
    .buffer_unordered(INGESTION_CONCURRENCY)
    .collect()
    .await;

    // Phase 3: Store results sequentially and update watermarks
    for (wi_idx, _chunk_idx, result) in &results {
        let item = &work_items[*wi_idx];
        match result {
            Ok(knowledge) => {
                match ingestion::store_knowledge(db, knowledge, item.session_id.as_deref()) {
                    Ok(count) => tracing::info!("Stored {} fragments from batch", count),
                    Err(e) => tracing::error!("Storage failed (continuing): {}", e),
                }
            }
            Err(e) => tracing::error!("Extraction failed (continuing): {}", e),
        }
    }

    // Update watermarks for all processed files
    for item in &work_items {
        db.storage()
            .set_watermark(&item.file_path.to_string_lossy(), item.new_offset)?;
    }

    Ok(())
}

async fn run_single_ingest(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = config.db_path();
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let storage = Storage::open(&db_path)?;
    let db = LoreDb::new(storage);

    let client = ClaudeClient::auto(
        &config.claude.api_key_env,
        config.ingestion.claude_model.clone(),
    );

    let watcher = FileWatcher::new();

    run_ingestion_pass(&db, &watcher, &client, config.ingestion.batch_size).await?;
    tracing::info!("Single ingestion pass complete.");
    Ok(())
}

async fn run_single_consolidation(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let storage = Storage::open(&config.db_path())?;
    let db = LoreDb::new(storage);

    let client = ClaudeClient::auto(
        &config.claude.api_key_env,
        config.ingestion.claude_model.clone(),
    );

    consolidation::run_consolidation(&db, Some(&client), &config.consolidation).await?;
    tracing::info!("Single consolidation pass complete.");
    Ok(())
}

fn daemonize(_config: Config) -> Result<(), Box<dyn std::error::Error>> {
    // Fork a child process that runs the daemon
    let exe = std::env::current_exe()?;
    let home = std::env::var("HOME").unwrap_or_default();
    let config_path = format!("{}/.lore/config.toml", home);
    let log_path = log_file();

    let child = std::process::Command::new(exe)
        .args([
            "--config",
            &config_path,
            "start",
            "--log-file",
            &log_path.to_string_lossy(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    println!("Daemon started with PID: {}", child.id());
    println!("Logs: {}", log_path.display());
    Ok(())
}

fn stop_daemon() -> Result<(), Box<dyn std::error::Error>> {
    let pid_path = pid_file();
    if !pid_path.exists() {
        println!("No daemon running (no PID file found).");
        return Ok(());
    }

    let pid_str = std::fs::read_to_string(&pid_path)?;
    let pid: i32 = pid_str.trim().parse()?;

    // Send SIGTERM
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }

    println!("Sent stop signal to daemon (PID: {}).", pid);
    let _ = std::fs::remove_file(&pid_path);
    Ok(())
}

fn show_status() -> Result<(), Box<dyn std::error::Error>> {
    let pid_path = pid_file();
    if !pid_path.exists() {
        println!("Daemon: not running");
        return Ok(());
    }

    let pid_str = std::fs::read_to_string(&pid_path)?;
    let pid: i32 = pid_str.trim().parse()?;

    // Check if process is actually running
    let running = unsafe { libc::kill(pid, 0) } == 0;

    if running {
        println!("Daemon: running (PID: {})", pid);
    } else {
        println!("Daemon: stale PID file (PID: {} not running)", pid);
        let _ = std::fs::remove_file(&pid_path);
    }

    Ok(())
}

fn tail_logs(lines: usize, follow: bool) -> Result<(), Box<dyn std::error::Error>> {
    let path = log_file();
    if !path.exists() {
        println!("No log file found at {}", path.display());
        println!("Start the daemon with `lore-daemon daemonize` first.");
        return Ok(());
    }

    let mut args = vec!["-n".to_string(), lines.to_string()];
    if follow {
        args.push("-f".to_string());
    }
    args.push(path.to_string_lossy().into_owned());

    let status = std::process::Command::new("tail")
        .args(&args)
        .status()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn ctrlc_handler(shutdown_tx: tokio::sync::watch::Sender<bool>) {
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        let _ = shutdown_tx.send(true);
    });
}
