use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use clap::{Parser, Subcommand};
use lore_db::{LoreDb, Storage};

use lore_daemon::claude_client::ClaudeClient;
use lore_daemon::config::Config;
use lore_daemon::consolidation;
use lore_daemon::status::{self, DaemonState};
use lore_daemon::watcher::FileWatcher;

#[derive(Parser)]
#[command(
    name = "lore",
    about = "Persistent memory for AI agents"
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
    /// List root-level fragments
    Roots {
        /// Max roots to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Filter by keyword
        query: Option<String>,
    },
    /// Semantic search across fragments
    Query {
        /// Search text
        topic: String,
        /// Depth level to search (0=roots, 1=concepts, etc.)
        #[arg(short, long, default_value = "0")]
        depth: u32,
        /// Max results
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Show the subtree rooted at a fragment
    Explore {
        /// Fragment ID (or prefix)
        id: String,
        /// Max tree depth to show
        #[arg(short, long, default_value = "3")]
        depth: u32,
    },
    /// Show what's in the staging area
    Staged,
}

fn lore_home() -> PathBuf {
    lore_db::lore_home()
}

fn pid_file() -> PathBuf {
    lore_home().join("daemon.pid")
}

fn log_file() -> PathBuf {
    lore_home().join("daemon.log")
}

fn config_path(raw: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    let expanded = raw.replace('~', &home.to_string_lossy());
    PathBuf::from(expanded)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Fast-path: commands that don't need tracing, tokio, or the DB
    match &cli.command {
        Command::Status => return show_status(),
        Command::Stop => return stop_daemon(),
        Command::Logs { lines, follow } => return tail_logs(*lines, *follow),
        Command::Daemonize => {
            let config = Config::load(&config_path(&cli.config))?;
            return daemonize(config);
        }
        _ => {}
    }

    // Initialize tracing for commands that need it
    let env_filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive("lore_daemon=info".parse().unwrap())
        .add_directive("lore=info".parse().unwrap());

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
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    }

    let config = Config::load(&config_path(&cli.config))?;

    // Commands that need the DB but not tokio
    match &cli.command {
        Command::Roots { limit, query } => return cli_roots(config, *limit, query.as_deref()),
        Command::Explore { id, depth } => return cli_explore(config, id, *depth),
        Command::Staged => return cli_staged(config),
        _ => {}
    }

    // Commands that need the tokio runtime
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            match cli.command {
                Command::Start { .. } => run_foreground(config).await?,
                Command::Ingest => run_single_ingest(config).await?,
                Command::Consolidate => run_single_consolidation(config).await?,
                Command::Query {
                    topic,
                    depth,
                    limit,
                } => cli_query(config, &topic, depth, limit)?,
                _ => unreachable!(),
            }
            Ok::<(), Box<dyn std::error::Error>>(())
        })?;

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

    status::write_status(DaemonState::Idle);

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
    let consolidation_config = config.consolidation.clone();

    // Handle shutdown gracefully
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    shutdown_signal_handler(shutdown_tx);

    let mut ingestion_timer = tokio::time::interval(ingestion_interval);
    let mut consolidation_timer = tokio::time::interval(consolidation_interval);

    // Skip the first immediate tick for consolidation (let ingestion run first)
    consolidation_timer.tick().await;

    loop {
        tokio::select! {
            _ = ingestion_timer.tick() => {
                status::write_status(DaemonState::Ingesting);
                if let Err(e) = run_ingestion_pass(&db, &watcher) {
                    tracing::error!("Ingestion error: {}", e);
                }
                status::write_status(DaemonState::Idle);
            }
            _ = consolidation_timer.tick() => {
                status::write_status(DaemonState::Consolidating);
                if let Err(e) = consolidation::run_consolidation(
                    &db,
                    Some(&client),
                    &consolidation_config,
                ).await {
                    tracing::error!("Consolidation error: {}", e);
                }
                status::write_status(DaemonState::Idle);
            }
            _ = shutdown_rx.changed() => {
                tracing::info!("Shutting down...");
                break;
            }
        }
    }

    // Cleanup PID and status files
    let _ = std::fs::remove_file(&pid_path);
    status::clear_status();
    tracing::info!("Daemon stopped.");
    Ok(())
}

/// Stage new conversation turns into the database for later digestion.
/// No Claude API calls — just file I/O and SQLite writes.
fn run_ingestion_pass(
    db: &LoreDb,
    watcher: &FileWatcher,
) -> Result<(), Box<dyn std::error::Error>> {
    let files = watcher.find_conversation_files();
    tracing::debug!("Found {} conversation files", files.len());

    for file_path in &files {
        let (turns, new_offset) = watcher.read_new_turns(file_path, db.storage())?;

        if turns.is_empty() {
            continue;
        }

        let file_str = file_path.to_string_lossy();
        let turn_tuples: Vec<(&str, &str)> = turns
            .iter()
            .map(|t| (t.role.as_str(), t.text.as_str()))
            .collect();

        let staged = db.storage().stage_turns(&file_str, &turn_tuples)?;
        db.storage().set_watermark(&file_str, new_offset)?;

        tracing::info!("Staged {} turns from {}", staged, file_path.display());
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
    let watcher = FileWatcher::new();

    status::write_status(status::DaemonState::Ingesting);
    let result = run_ingestion_pass(&db, &watcher);
    status::write_status(status::DaemonState::Idle);
    result?;
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

    status::write_status(status::DaemonState::Consolidating);
    let result = consolidation::run_consolidation(&db, Some(&client), &config.consolidation).await;
    status::write_status(status::DaemonState::Idle);
    result?;
    tracing::info!("Single consolidation pass complete.");
    Ok(())
}

fn daemonize(_config: Config) -> Result<(), Box<dyn std::error::Error>> {
    // Fork a child process that runs the daemon
    let exe = std::env::current_exe()?;
    let config_path = lore_home().join("config.toml");
    let log_path = log_file();

    let child = std::process::Command::new(exe)
        .args([
            "--config",
            &config_path.to_string_lossy(),
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
    status::clear_status();
    Ok(())
}

fn show_status() -> Result<(), Box<dyn std::error::Error>> {
    // Check status file first (covers both daemon and single-pass commands)
    if let Some(s) = status::read_status() {
        let pid = s.pid as i32;
        let alive = pid > 0 && unsafe { libc::kill(pid, 0) } == 0;
        if alive {
            let state = match s.state {
                DaemonState::Idle => "idle",
                DaemonState::Ingesting => "ingesting",
                DaemonState::Consolidating => "consolidating",
            };
            println!("Lore: running (PID: {}, state: {})", s.pid, state);
            return Ok(());
        }
    }

    // Check PID file as fallback (daemon may be starting up)
    let pid_path = pid_file();
    if pid_path.exists() {
        let pid_str = std::fs::read_to_string(&pid_path)?;
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            let alive = pid > 0 && unsafe { libc::kill(pid, 0) } == 0;
            if alive {
                println!("Lore: running (PID: {})", pid);
                return Ok(());
            }
            // Stale PID file
            let _ = std::fs::remove_file(&pid_path);
            status::clear_status();
        }
    }

    println!("Lore: not running");
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

    let status = std::process::Command::new("tail").args(&args).status()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// CLI query commands
// ---------------------------------------------------------------------------

fn open_db(config: &Config) -> Result<LoreDb, Box<dyn std::error::Error>> {
    let storage = Storage::open(&config.db_path())?;
    Ok(LoreDb::new(storage))
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

fn cli_roots(
    config: Config,
    limit: usize,
    query: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = open_db(&config)?;
    let mut roots = db.list_roots(query);
    roots.truncate(limit);

    if roots.is_empty() {
        println!("No roots found.");
        return Ok(());
    }

    println!("{:<38} {:>5} {:>5}  Content", "ID", "Rel", "Acc");
    println!("{}", "-".repeat(100));
    for t in &roots {
        let children = db.children(t.id).len();
        let content_preview = truncate(&t.content, 60);
        println!(
            "{:<38} {:.2}  {:>4}  {} {}",
            t.id,
            t.relevance_score,
            t.access_count,
            content_preview,
            if children > 0 {
                format!("[{children} children]")
            } else {
                String::new()
            }
        );
    }
    println!("\n{} roots", roots.len());
    Ok(())
}

fn cli_query(
    config: Config,
    topic: &str,
    depth: u32,
    limit: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = open_db(&config)?;
    let results = db.query(topic, depth, limit);

    if results.is_empty() {
        println!("No results for \"{}\" at depth {}.", topic, depth);
        return Ok(());
    }

    for (i, sf) in results.iter().enumerate() {
        let f = &sf.fragment;
        println!(
            "{}. [score={:.3} rel={:.3}] (depth={}) {}",
            i + 1,
            sf.score,
            f.relevance_score,
            f.depth,
            f.id
        );
        println!("   {}", f.content);
        if let Some(parent) = db.parent(f.id) {
            println!("   <- parent: {}", truncate(&parent.content, 60));
        }
        println!();
    }
    Ok(())
}

fn cli_explore(
    config: Config,
    id_prefix: &str,
    max_depth: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = open_db(&config)?;

    // Support ID prefix matching
    let id = if id_prefix.len() < 36 {
        let roots = db.list_roots(None);
        let all_frags: Vec<_> = roots
            .iter()
            .filter(|f| f.id.to_string().starts_with(id_prefix))
            .collect();
        match all_frags.len() {
            0 => {
                println!("No fragment found matching prefix \"{}\"", id_prefix);
                return Ok(());
            }
            1 => all_frags[0].id,
            n => {
                println!(
                    "Ambiguous prefix \"{}\" matches {} fragments:",
                    id_prefix, n
                );
                for f in &all_frags {
                    println!("  {} - {}", f.id, truncate(&f.content, 60));
                }
                return Ok(());
            }
        }
    } else {
        lore_db::FragmentId::parse(id_prefix)?
    };

    let tree = db.subtree(id, max_depth);
    match tree {
        Some(tree) => print_tree(&tree, 0),
        None => println!("Fragment {} not found.", id_prefix),
    }
    Ok(())
}

fn print_tree(tree: &lore_db::Tree, indent: usize) {
    let prefix = "  ".repeat(indent);
    let f = &tree.fragment;
    println!(
        "{}{} (depth={}, rel={:.2})",
        prefix, f.id, f.depth, f.relevance_score
    );
    println!("{}  {}", prefix, f.content);
    for child in &tree.children {
        print_tree(child, indent + 1);
    }
}

fn cli_staged(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let db = open_db(&config)?;
    let now = lore_db::fragment::now_unix();
    let sessions = db.storage().get_staged_sessions(0, now + 1)?;

    if sessions.is_empty() {
        println!("No staged conversations.");
        return Ok(());
    }

    println!("{:<70} {:>6} {:>8}", "Session", "Turns", "Age");
    println!("{}", "-".repeat(90));
    let mut total_turns = 0;
    for s in &sessions {
        let age_secs = now - s.last_staged;
        let age = if age_secs < 60 {
            format!("{age_secs}s")
        } else if age_secs < 3600 {
            format!("{}m", age_secs / 60)
        } else {
            format!("{}h", age_secs / 3600)
        };
        let name = std::path::Path::new(&s.file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&s.file_path);
        println!("{:<70} {:>6} {:>8}", name, s.turn_count, age);
        total_turns += s.turn_count;
    }
    println!("\n{} sessions, {} total turns", sessions.len(), total_turns);
    Ok(())
}

fn shutdown_signal_handler(shutdown_tx: tokio::sync::watch::Sender<bool>) {
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }

        let _ = shutdown_tx.send(true);
    });
}
