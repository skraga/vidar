//! Vidar desktop app: tray + engine always on, window only while open.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod logging;
mod tray;
mod ui;
mod worker;

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use log::{error, info, warn};
use vidar_win::instance::{self, Acquire};
use vidar_win::router::{self, RouteState};

/// Requests for the main (window) thread.
/// A page of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Test,
    Settings,
    About,
}

pub enum UiCmd {
    /// Show the window; `Some` also switches to that page.
    Show(Option<Page>),
    Quit,
}

/// Last-resort repair: put the user's microphone back using the saved routing state.
fn emergency_restore(cfg_path: &std::path::Path) {
    let (cfg, _) = vidar_config::load(cfg_path);
    let mut state = RouteState {
        is_swapped: cfg.is_swapped,
        original_mic_id: cfg.original_mic_id.clone(),
    };
    if let Ok(audio) = vidar_win::WinAudio::new() {
        let mut cfg2 = cfg.clone();
        let _ = router::restore_original(&audio, &mut state, &mut |s| {
            cfg2.is_swapped = s.is_swapped;
            let _ = vidar_config::save(cfg_path, &cfg2);
        });
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--write-icon") {
        // Packaging helper: write vidar.ico.
        let path = args
            .get(i + 1)
            .cloned()
            .unwrap_or_else(|| "vidar.ico".into());
        let ok = std::fs::write(&path, vidar_brand::ico(true)).is_ok();
        std::process::exit(if ok { 0 } else { 1 });
    }
    let minimized = args.iter().any(|a| a == "--minimized");

    logging::init();
    info!("Vidar {} starting", env!("CARGO_PKG_VERSION"));

    let guard = match instance::acquire() {
        Acquire::First(g) => g,
        Acquire::AlreadyRunning => {
            info!("already running; asked the other copy to show itself");
            return;
        }
    };
    let guard = Arc::new(guard);

    let cfg_path = vidar_config::default_path();
    let (mut cfg, outcome) = vidar_config::load(&cfg_path);
    info!("config {}: {:?}", cfg_path.display(), outcome);

    // If the last run crashed while the cable was the default mic, fix that first.
    {
        let path = cfg_path.clone();
        let hook_path = cfg_path.clone();
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            error!("panic: {info}");
            // Audio calls need the multithreaded COM apartment; keep them off the UI thread.
            let p = hook_path.clone();
            let _ = std::thread::spawn(move || emergency_restore(&p)).join();
            default_hook(info);
        }));
        // Audio (COM multithreaded) work runs on a helper thread: the UI thread must stay
        // single-threaded for the window.
        let audit_cfg = cfg.clone();
        let swapped = std::thread::spawn(move || {
            let audio = vidar_win::WinAudio::new().ok()?;
            let mut state = RouteState {
                is_swapped: audit_cfg.is_swapped,
                original_mic_id: audit_cfg.original_mic_id.clone(),
            };
            match router::audit_crash_recovery(&audio, &mut state, &mut |_| {}) {
                Ok(Some(e)) => info!("crash recovery: microphone restored to {}", e.name),
                Ok(None) => {}
                Err(e) => warn!("crash recovery: {e}"),
            }
            Some(state.is_swapped)
        })
        .join()
        .ok()
        .flatten();
        if let Some(s) = swapped {
            cfg.is_swapped = s;
            let _ = vidar_config::save(&path, &cfg);
        }
    }
    cfg.start_with_windows = autostart::is_enabled();

    let (handle, worker) = worker::spawn(cfg_path.clone(), cfg);
    let worker = Arc::new(Mutex::new(Some(worker)));

    // Windows shutdown/logoff: stop and restore the mic before the process is killed.
    {
        let (h, w) = (handle.clone(), worker.clone());
        tray::set_end_session_hook(move || {
            h.send(worker::Cmd::Exit);
            if let Some(j) = w.lock().ok().and_then(|mut g| g.take()) {
                let _ = j.join();
            }
        });
    }

    let (ui_tx, ui_rx) = mpsc::channel::<UiCmd>();
    let ui_rx = Arc::new(Mutex::new(ui_rx));
    let wake = {
        let h = handle.clone();
        move || {
            if let Ok(c) = h.repaint.lock() {
                if let Some(c) = c.as_ref() {
                    c.request_repaint();
                }
            }
        }
    };

    // Tray thread.
    {
        let (h, tx, wake) = (handle.clone(), ui_tx.clone(), wake.clone());
        let tx2 = tx.clone();
        std::thread::Builder::new()
            .name("vidar-tray".into())
            .spawn(move || {
                tray::run(h, tx);
                let _ = tx2.send(UiCmd::Quit);
                wake();
            })
            .expect("tray thread");
    }

    // A second launch asks us to show the window.
    {
        let (g, tx, wake) = (guard.clone(), ui_tx.clone(), wake.clone());
        std::thread::Builder::new()
            .name("vidar-instance".into())
            .spawn(move || loop {
                if g.wait_show_request(1000) {
                    let _ = tx.send(UiCmd::Show(None));
                    wake();
                }
            })
            .expect("instance thread");
    }

    let quit = Arc::new(AtomicBool::new(false));
    if !minimized {
        ui::run_window(handle.clone(), ui_rx.clone(), quit.clone(), None);
    }
    while !quit.load(Relaxed) {
        let next = ui_rx
            .lock()
            .map(|rx| rx.recv_timeout(Duration::from_millis(500)));
        match next {
            Ok(Ok(UiCmd::Show(page))) => {
                ui::run_window(handle.clone(), ui_rx.clone(), quit.clone(), page)
            }
            Ok(Ok(UiCmd::Quit)) => break,
            Ok(Err(RecvTimeoutError::Timeout)) => {}
            _ => break,
        }
    }

    info!("exiting");
    handle.send(worker::Cmd::Exit);
    if let Some(j) = worker.lock().ok().and_then(|mut g| g.take()) {
        let _ = j.join();
    }
    log::logger().flush();
}
