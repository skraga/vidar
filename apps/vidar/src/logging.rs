//! Minimal file logger: %APPDATA%\Vidar\logs\vidar.log, rotated at 1 MB (one old copy kept).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

struct FileLog {
    path: PathBuf,
    file: Mutex<Option<File>>,
}

const MAX_BYTES: u64 = 1024 * 1024;

impl log::Log for FileLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        // Our own messages at info; other crates (winit, eframe) only warnings.
        m.level()
            <= if m.target().starts_with("vidar") {
                log::Level::Info
            } else {
                log::Level::Warn
            }
    }

    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let Ok(mut guard) = self.file.lock() else {
            return;
        };
        if guard
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .is_some_and(|m| m.len() > MAX_BYTES)
        {
            *guard = None;
            let _ = std::fs::rename(&self.path, self.path.with_extension("old.log"));
        }
        if guard.is_none() {
            *guard = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .ok();
        }
        if let Some(f) = guard.as_mut() {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = writeln!(f, "{secs} {:<5} {}: {}", r.level(), r.target(), r.args());
        }
    }

    fn flush(&self) {
        if let Ok(mut g) = self.file.lock() {
            if let Some(f) = g.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

/// Folder for logs; created on demand.
pub fn log_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Vidar").join("logs")
}

pub fn init() {
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let logger = Box::leak(Box::new(FileLog {
        path: dir.join("vidar.log"),
        file: Mutex::new(None),
    }));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}
