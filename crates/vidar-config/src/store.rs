//! Where the config lives and how it is read and written.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{Config, Error, CONFIG_VERSION};

const FILE_NAME: &str = "config.json";

/// Portable mode: a `config.json` next to the executable in a writable folder.
/// Otherwise `%APPDATA%\Vidar\config.json`.
pub fn default_path() -> PathBuf {
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        let local = dir.join(FILE_NAME);
        if local.is_file() && dir_writable(&dir) {
            return local;
        }
    }
    appdata().join("Vidar").join(FILE_NAME)
}

fn appdata() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".write_test_{}", std::process::id()));
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// How `load` produced the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    Loaded,
    /// No file yet: defaults.
    Created,
    /// The file was unreadable; defaults are used and the bad file was kept as `.bad`.
    Recovered(String),
}

/// Load `path`, or create it with defaults. Any result other than `Loaded` is saved
/// straight away.
pub fn load(path: &Path) -> (Config, LoadOutcome) {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Config>(&text) {
            Ok(mut cfg) => {
                cfg.sanitize();
                if cfg.version != CONFIG_VERSION {
                    cfg.version = CONFIG_VERSION.into();
                    let _ = save(path, &cfg);
                }
                (cfg, LoadOutcome::Loaded)
            }
            Err(e) => {
                let _ = fs::rename(path, path.with_extension("json.bad"));
                let cfg = Config::default();
                let _ = save(path, &cfg);
                (cfg, LoadOutcome::Recovered(e.to_string()))
            }
        },
        Err(_) => {
            let cfg = Config::default();
            let _ = save(path, &cfg);
            (cfg, LoadOutcome::Created)
        }
    }
}

/// Write atomically: a temp file in the same folder, then rename over the target.
/// Retries the rename briefly because antivirus scanners hold new files open.
pub fn save(path: &Path, cfg: &Config) -> Result<(), Error> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
    let result = (|| -> Result<(), Error> {
        let mut f = fs::File::create(&tmp)?;
        let text = serde_json::to_string_pretty(cfg)?;
        f.write_all(text.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        drop(f);
        let mut last = None;
        for attempt in 0..3u64 {
            match fs::rename(&tmp, path) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    last = Some(e);
                    std::thread::sleep(Duration::from_millis(50 * (attempt + 1)));
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(last.expect("loop ran").into())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Vidar").join(FILE_NAME);
        let cfg = Config {
            mic_boost_db: 10.0,
            input_device_name: Some("Mic".into()),
            ..Config::default()
        };
        save(&path, &cfg).unwrap();
        let (back, outcome) = load(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(back, cfg);
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "temp file must not remain");
    }

    #[test]
    fn corrupt_file_is_kept_aside_and_defaults_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        fs::write(&path, "{ not json").unwrap();
        let (cfg, outcome) = load(&path);
        assert!(matches!(outcome, LoadOutcome::Recovered(_)));
        assert_eq!(cfg, Config::default());
        assert!(path.with_extension("json.bad").is_file());
    }

    #[test]
    fn missing_everything_creates_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let (cfg, outcome) = load(&path);
        assert_eq!(outcome, LoadOutcome::Created);
        assert_eq!(cfg, Config::default());
        assert!(path.is_file());
    }
}
