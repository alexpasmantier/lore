mod icon;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tao::event_loop::{ControlFlow, EventLoop};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

/// How often we poll the daemon status file when idle / stopped.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Frame interval for the pulse animation.
const FRAME_INTERVAL: Duration = Duration::from_millis(100);
/// Number of frames in one pulse cycle.
const PULSE_FRAMES: usize = 16;

// ---------------------------------------------------------------------------
// Daemon state (read from `~/.lore/daemon.status`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum TrayState {
    Stopped,
    Idle,
    Ingesting,
    Consolidating,
}

#[derive(Deserialize)]
struct StatusFile {
    state: String,
    pid: u32,
    #[allow(dead_code)]
    updated_at: i64,
}

fn lore_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".lore")
}

fn status_path() -> PathBuf {
    lore_home().join("daemon.status")
}

fn poll_state() -> TrayState {
    let content = match std::fs::read_to_string(status_path()) {
        Ok(c) => c,
        Err(_) => return TrayState::Stopped,
    };

    let status: StatusFile = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(_) => return TrayState::Stopped,
    };

    // Reject invalid PIDs: 0 would check our own process group via kill(2).
    if status.pid == 0 {
        return TrayState::Stopped;
    }

    // Verify the process is still alive.
    let pid = status.pid as i32;
    if pid <= 0 {
        // u32 > i32::MAX overflowed to negative — treat as invalid.
        return TrayState::Stopped;
    }
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    if !alive {
        return TrayState::Stopped;
    }

    match status.state.as_str() {
        "idle" => TrayState::Idle,
        "ingesting" => TrayState::Ingesting,
        "consolidating" => TrayState::Consolidating,
        _ => TrayState::Idle,
    }
}

// ---------------------------------------------------------------------------
// Daemon control helpers
// ---------------------------------------------------------------------------

fn daemon_binary() -> PathBuf {
    // 1. Check next to the tray binary (for .app bundles / co-located installs)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("lore-daemon");
            if sibling.exists() {
                return sibling;
            }
        }
    }
    // 2. Check ~/.local/bin/
    if let Some(home) = dirs::home_dir() {
        let local = home.join(".local/bin/lore-daemon");
        if local.exists() {
            return local;
        }
    }
    // 3. Fall back to PATH
    PathBuf::from("lore-daemon")
}

fn start_daemon() {
    let _ = std::process::Command::new(daemon_binary())
        .arg("daemonize")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn stop_daemon() {
    let _ = std::process::Command::new(daemon_binary())
        .arg("stop")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn trigger_ingest() {
    let _ = std::process::Command::new(daemon_binary())
        .arg("ingest")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn trigger_consolidate() {
    let _ = std::process::Command::new(daemon_binary())
        .arg("consolidate")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn view_logs() {
    let log_path = lore_home().join("daemon.log");
    let log_str = log_path.to_string_lossy().to_string();

    #[cfg(target_os = "macos")]
    {
        if std::process::Command::new("open")
            .arg(&log_str)
            .spawn()
            .is_ok()
        {
            return;
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Try common terminal emulators until one succeeds.
        let terminals: &[(&str, &[&str])] = &[
            ("x-terminal-emulator", &["-e"]),
            ("gnome-terminal", &["--"]),
            ("konsole", &["-e"]),
            ("xfce4-terminal", &["-e"]),
            ("alacritty", &["-e"]),
            ("kitty", &["--"]),
            ("xterm", &["-e"]),
        ];

        for (term, prefix) in terminals {
            let mut args: Vec<&str> = prefix.to_vec();
            args.extend(["tail", "-f", &log_str]);

            if std::process::Command::new(term).args(&args).spawn().is_ok() {
                return;
            }
        }
    }

    eprintln!("lore-tray: could not find a way to show logs");
}

// ---------------------------------------------------------------------------
// Icon helper
// ---------------------------------------------------------------------------

fn make_icon(brightness: f32, color: icon::IconColor) -> Icon {
    let rgba = icon::generate(icon::ICON_SIZE, brightness, color);
    Icon::from_rgba(rgba, icon::ICON_SIZE, icon::ICON_SIZE).expect("invalid icon data")
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let event_loop = EventLoop::new();

    // -- Build context menu ------------------------------------------------
    let header_item = MenuItem::new("Lore v0.1.0\t\tStopped", false, None);
    let sep0 = PredefinedMenuItem::separator();
    let start_item = MenuItem::new("Start Daemon", true, None);
    let stop_item = MenuItem::new("Stop Daemon", false, None);
    let sep1 = PredefinedMenuItem::separator();
    let ingest_item = MenuItem::new("Trigger Ingestion", false, None);
    let consolidate_item = MenuItem::new("Trigger Consolidation", false, None);
    let sep2 = PredefinedMenuItem::separator();
    let logs_item = MenuItem::new("View Logs", true, None);
    let sep3 = PredefinedMenuItem::separator();
    let quit_item = MenuItem::new("Quit", true, None);

    let menu = Menu::new();
    menu.append_items(&[
        &header_item,
        &sep0,
        &start_item,
        &stop_item,
        &sep1,
        &ingest_item,
        &consolidate_item,
        &sep2,
        &logs_item,
        &sep3,
        &quit_item,
    ])
    .expect("failed to build menu");

    // Clone IDs for event matching inside the closure.
    let start_id = start_item.id().clone();
    let stop_id = stop_item.id().clone();
    let ingest_id = ingest_item.id().clone();
    let consolidate_id = consolidate_item.id().clone();
    let logs_id = logs_item.id().clone();
    let quit_id = quit_item.id().clone();

    // -- Create the tray icon (initially in stopped state) -----------------
    let initial_icon = make_icon(0.15, icon::IconColor::Red);

    let tray = TrayIconBuilder::new()
        .with_icon(initial_icon)
        .with_menu(Box::new(menu))
        .with_tooltip("Lore - Stopped")
        .build()
        .expect("failed to create tray icon");

    // -- Auto-start daemon if not already running --------------------------
    if poll_state() == TrayState::Stopped {
        start_daemon();
    }

    // -- State tracking ----------------------------------------------------
    let mut state = TrayState::Stopped;
    let mut frame: usize = 0;
    let mut last_update = Instant::now()
        .checked_sub(POLL_INTERVAL)
        .unwrap_or_else(Instant::now);

    let menu_rx = MenuEvent::receiver();

    // -- Event loop --------------------------------------------------------
    event_loop.run(move |_event, _, control_flow| {
        let now = Instant::now();

        // Wake up at the right cadence for the current state.
        let interval = if matches!(state, TrayState::Ingesting | TrayState::Consolidating) {
            FRAME_INTERVAL
        } else {
            POLL_INTERVAL
        };
        *control_flow = ControlFlow::WaitUntil(now + interval);

        // -- Handle menu clicks --------------------------------------------
        while let Ok(event) = menu_rx.try_recv() {
            if event.id == start_id {
                start_daemon();
            } else if event.id == stop_id {
                stop_daemon();
            } else if event.id == ingest_id {
                trigger_ingest();
            } else if event.id == consolidate_id {
                trigger_consolidate();
            } else if event.id == logs_id {
                view_logs();
            } else if event.id == quit_id {
                // Stop the daemon before quitting
                if poll_state() != TrayState::Stopped {
                    stop_daemon();
                }
                *control_flow = ControlFlow::Exit;
                return;
            }
        }

        // -- Throttle visual updates to `interval` -------------------------
        if now.duration_since(last_update) < interval {
            return;
        }
        last_update = now;

        // -- Poll daemon state ---------------------------------------------
        let new_state = poll_state();
        let state_changed = new_state != state;

        if state_changed {
            state = new_state;
            frame = 0;

            // Update header label.
            let status = match state {
                TrayState::Stopped => "Stopped",
                TrayState::Idle => "Idle",
                TrayState::Ingesting => "Ingesting\u{2026}",
                TrayState::Consolidating => "Consolidating\u{2026}",
            };
            header_item.set_text(format!("Lore v0.1.0\t\t{status}"));

            // Toggle menu items.
            let running = !matches!(state, TrayState::Stopped);
            let busy = matches!(state, TrayState::Ingesting | TrayState::Consolidating);
            start_item.set_enabled(!running);
            stop_item.set_enabled(running);
            ingest_item.set_enabled(running && !busy);
            consolidate_item.set_enabled(running && !busy);

            // Update tooltip.
            let tip = match state {
                TrayState::Stopped => "Lore - Stopped",
                TrayState::Idle => "Lore - Running",
                TrayState::Ingesting => "Lore - Ingesting\u{2026}",
                TrayState::Consolidating => "Lore - Consolidating\u{2026}",
            };
            tray.set_tooltip(Some(tip)).ok();
        }

        // -- Update icon ---------------------------------------------------
        let animating = matches!(state, TrayState::Ingesting | TrayState::Consolidating);

        if state_changed || animating {
            let (brightness, color) = match state {
                TrayState::Stopped => (0.15_f32, icon::IconColor::Red),
                TrayState::Idle => (1.0, icon::IconColor::Red),
                TrayState::Ingesting => {
                    let t = frame as f32 / PULSE_FRAMES as f32 * std::f32::consts::TAU;
                    let b = 0.3 + 0.7 * (t.sin() + 1.0) / 2.0;
                    (b, icon::IconColor::Red)
                }
                TrayState::Consolidating => {
                    let t = frame as f32 / PULSE_FRAMES as f32 * std::f32::consts::TAU;
                    let b = 0.3 + 0.7 * (t.sin() + 1.0) / 2.0;
                    (b, icon::IconColor::Orange)
                }
            };

            let new_icon = make_icon(brightness, color);
            tray.set_icon(Some(new_icon)).ok();

            if animating {
                frame = (frame + 1) % PULSE_FRAMES;
            }
        }
    });
}
