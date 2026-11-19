//! Updates: find the latest release on GitHub and, for a copy installed by setup, download
//! its installer, check it against the published SHA-256 and run it.
//!
//! The portable zip cannot update itself (it may live anywhere, even on a read-only drive);
//! it only points to the release page.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use sha2::{Digest, Sha256};

pub const REPO: &str = "skraga/vidar";
pub const RELEASES_PAGE: &str = "https://github.com/skraga/vidar/releases/latest";
/// Arguments for an update run of setup: progress only, no questions, start Vidar again.
pub const SETUP_ARGS: &str = "/SILENT /SUPPRESSMSGBOXES /NORESTART /RELAUNCH";

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// The latest stable release.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// "0.2.0", without the tag's "v".
    pub version: String,
    /// The release's page on GitHub.
    pub page_url: String,
    /// The setup for this PC and its `.sha256`, when the release has them: see
    /// [`setup_names`].
    pub setup: Option<Asset>,
    pub setup_sha256: Option<Asset>,
}

/// `major.minor.patch` of a stable version ("v0.2.0" or "0.2.0"); `None` for anything
/// else, pre-releases ("0.2.0-rc.1") included.
pub fn parse_version(s: &str) -> Option<[u64; 3]> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let mut parts = s.split('.');
    let v = [
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ];
    parts.next().is_none().then_some(v)
}

/// Is `candidate` a newer stable version than `current`?
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Only GitHub URLs are opened or downloaded, whatever the reply says.
fn github_url(url: &str) -> bool {
    ["https://github.com/", "https://api.github.com/"]
        .iter()
        .any(|p| url.starts_with(p))
}

/// Setup file names to look for, best first: `vidar-<version>-<x64|arm64>-setup.exe`, then
/// `vidar-<version>-setup.exe`, the x64 name used up to 0.3.0. An ARM64 PC prefers the
/// native ARM64 setup (also when this copy is the x64 build running emulated) and falls back
/// to x64, which Windows 11 on ARM runs too.
pub fn setup_names(version: &str, arm64: bool) -> Vec<String> {
    let mut names = Vec::with_capacity(3);
    if arm64 {
        names.push(format!("vidar-{version}-arm64-setup.exe"));
    }
    names.push(format!("vidar-{version}-x64-setup.exe"));
    names.push(format!("vidar-{version}-setup.exe"));
    names
}

/// Read GitHub's `releases/latest` reply. `arm64`: the PC is ARM64.
pub fn parse_release(json: &str, arm64: bool) -> Result<Release, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("unexpected reply from GitHub: {e}"))?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return Err("no stable release yet".into());
    }
    let tag = v["tag_name"]
        .as_str()
        .ok_or("the release has no version tag")?;
    let [a, b, c] = parse_version(tag).ok_or_else(|| format!("unrecognised version {tag}"))?;
    let version = format!("{a}.{b}.{c}");
    let page_url = v["html_url"]
        .as_str()
        .filter(|u| github_url(u))
        .unwrap_or(RELEASES_PAGE)
        .to_string();
    let assets: Vec<Asset> = v["assets"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|x| {
                    Some(Asset {
                        name: x["name"].as_str()?.to_string(),
                        url: x["browser_download_url"]
                            .as_str()
                            .filter(|u| github_url(u))?
                            .to_string(),
                        size: x["size"].as_u64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let find = |name: &str| assets.iter().find(|a| a.name == name).cloned();
    // The setup and its checksum always come as a pair of the same name.
    let (setup, setup_sha256) = setup_names(&version, arm64)
        .iter()
        .find_map(|name| Some((find(name)?, find(&format!("{name}.sha256"))?)))
        .map_or((None, None), |(s, h)| (Some(s), Some(h)));
    Ok(Release {
        setup,
        setup_sha256,
        version,
        page_url,
    })
}

/// The hash in a `sha256sum`-style line ("<64 hex>  <file name>").
pub fn parse_sha256_file(text: &str) -> Option<String> {
    let hash = text.split_whitespace().next()?.to_ascii_lowercase();
    (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hash)
}

fn agent(timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(timeout)
        .timeout_connect(Some(Duration::from_secs(15)))
        // No personal details: just the program and its version.
        .user_agent(format!("Vidar/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn net_err(e: impl std::fmt::Display) -> String {
    format!("could not reach GitHub ({e})")
}

/// Ask GitHub for the latest stable release.
pub fn fetch_latest() -> Result<Release, String> {
    let body = agent(Some(Duration::from_secs(20)))
        .get(format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(net_err)?
        .into_body()
        .read_to_string()
        .map_err(net_err)?;
    parse_release(&body, vidar_win::native_arm64())
}

/// Was this copy installed by setup? Inno Setup keeps its uninstaller next to the program;
/// the portable zip has none.
pub fn installed() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("unins000.exe")))
        .is_some_and(|p| p.is_file())
}

/// Download the release's setup into the temp folder and check it against the published
/// SHA-256. `progress` gets 0..1. Returns the verified file.
pub fn download_setup(rel: &Release, progress: &mut dyn FnMut(f64)) -> Result<PathBuf, String> {
    let setup = rel
        .setup
        .as_ref()
        .ok_or("this release has no installer; download it from the release page")?;
    let sum = rel
        .setup_sha256
        .as_ref()
        .ok_or("this release has no installer checksum")?;
    let expected = agent(Some(Duration::from_secs(30)))
        .get(&sum.url)
        .call()
        .map_err(net_err)?
        .into_body()
        .read_to_string()
        .map_err(net_err)
        .and_then(|t| {
            parse_sha256_file(&t).ok_or_else(|| "the checksum file is not valid".into())
        })?;

    let mut reader = agent(None)
        .get(&setup.url)
        .call()
        .map_err(net_err)?
        .into_body()
        .into_with_config()
        .limit(500 * 1024 * 1024)
        .reader();
    let mut data = Vec::with_capacity(setup.size as usize);
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(net_err)?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        if setup.size > 0 {
            progress((data.len() as f64 / setup.size as f64).min(1.0));
        }
    }
    let actual: String = Sha256::digest(&data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != expected {
        return Err(
            "the downloaded installer does not match its checksum; nothing was installed".into(),
        );
    }
    let dir = std::env::temp_dir().join("Vidar-update");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(&setup.name);
    std::fs::write(&path, &data).map_err(|e| format!("could not save the installer: {e}"))?;
    Ok(path)
}

/// Run a verified setup elevated (one UAC prompt). Returns once it has started; setup then
/// asks this copy to quit, updates the files and starts Vidar again.
pub fn launch_setup(path: &std::path::Path) -> Result<(), String> {
    vidar_win::driver::launch_elevated(path, SETUP_ARGS).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically_and_skip_prereleases() {
        assert_eq!(parse_version("v0.2.0"), Some([0, 2, 0]));
        assert_eq!(parse_version("0.10.3"), Some([0, 10, 3]));
        assert_eq!(parse_version("v0.2.0-rc.1"), None);
        assert_eq!(parse_version("0.2"), None);
        assert_eq!(parse_version("0.2.0.1"), None);
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("v0.3.0-rc.1", "0.2.0"));
    }

    #[test]
    fn release_reply_yields_the_setup_and_its_checksum() {
        let json = r#"{
            "tag_name": "v0.2.0", "draft": false, "prerelease": false,
            "html_url": "https://github.com/skraga/vidar/releases/tag/v0.2.0",
            "assets": [
                {"name": "vidar-0.2.0-windows-x64.zip", "size": 10,
                 "browser_download_url": "https://github.com/skraga/vidar/releases/download/v0.2.0/vidar-0.2.0-windows-x64.zip"},
                {"name": "vidar-0.2.0-setup.exe", "size": 1234,
                 "browser_download_url": "https://github.com/skraga/vidar/releases/download/v0.2.0/vidar-0.2.0-setup.exe"},
                {"name": "vidar-0.2.0-setup.exe.sha256", "size": 90,
                 "browser_download_url": "https://github.com/skraga/vidar/releases/download/v0.2.0/vidar-0.2.0-setup.exe.sha256"}
            ]}"#;
        let r = parse_release(json, false).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.setup.as_ref().unwrap().size, 1234);
        assert!(r.setup_sha256.unwrap().url.ends_with(".sha256"));
        assert!(r.page_url.ends_with("/v0.2.0"));
        // A release without an ARM64 setup still updates an ARM64 PC, with the x64 one.
        let r = parse_release(json, true).unwrap();
        assert_eq!(r.setup.unwrap().name, "vidar-0.2.0-setup.exe");
    }

    fn release_with(version: &str, names: &[&str]) -> String {
        let assets: Vec<String> = names
            .iter()
            .map(|n| {
                format!(
                    r#"{{"name": "{n}", "size": 1, "browser_download_url": "https://github.com/skraga/vidar/releases/download/v{version}/{n}"}}"#
                )
            })
            .collect();
        format!(
            r#"{{"tag_name": "v{version}", "assets": [{}]}}"#,
            assets.join(",")
        )
    }

    /// Each PC gets its own architecture's setup by the current names, also when the
    /// release still carries the old x64 name for 0.2's updater.
    #[test]
    fn each_pc_gets_its_architectures_setup() {
        let json = release_with(
            "0.3.1",
            &[
                "vidar-0.3.1-x64-portable.zip",
                "vidar-0.3.1-x64-setup.exe",
                "vidar-0.3.1-x64-setup.exe.sha256",
                "vidar-0.3.1-arm64-setup.exe",
                "vidar-0.3.1-arm64-setup.exe.sha256",
                "vidar-0.3.1-setup.exe",
                "vidar-0.3.1-setup.exe.sha256",
            ],
        );
        let arm = parse_release(&json, true).unwrap();
        assert_eq!(arm.setup.unwrap().name, "vidar-0.3.1-arm64-setup.exe");
        assert_eq!(
            arm.setup_sha256.unwrap().name,
            "vidar-0.3.1-arm64-setup.exe.sha256"
        );
        let x64 = parse_release(&json, false).unwrap();
        assert_eq!(x64.setup.unwrap().name, "vidar-0.3.1-x64-setup.exe");
    }

    /// Releases up to 0.3.0 named the x64 setup without its architecture.
    #[test]
    fn old_release_names_still_work() {
        let json = release_with(
            "0.3.0",
            &[
                "vidar-0.3.0-setup.exe",
                "vidar-0.3.0-setup.exe.sha256",
                "vidar-0.3.0-arm64-setup.exe",
                "vidar-0.3.0-arm64-setup.exe.sha256",
            ],
        );
        let arm = parse_release(&json, true).unwrap();
        assert_eq!(arm.setup.unwrap().name, "vidar-0.3.0-arm64-setup.exe");
        let x64 = parse_release(&json, false).unwrap();
        assert_eq!(x64.setup.unwrap().name, "vidar-0.3.0-setup.exe");

        // A setup without its checksum is never chosen: the ARM64 PC falls back to x64.
        let json = release_with(
            "0.3.0",
            &[
                "vidar-0.3.0-setup.exe",
                "vidar-0.3.0-setup.exe.sha256",
                "vidar-0.3.0-arm64-setup.exe",
            ],
        );
        assert_eq!(
            parse_release(&json, true).unwrap().setup.unwrap().name,
            "vidar-0.3.0-setup.exe"
        );
    }

    #[test]
    fn foreign_urls_and_prereleases_are_ignored() {
        let json = r#"{"tag_name": "v0.2.0", "html_url": "https://evil.example/x",
            "assets": [{"name": "vidar-0.2.0-setup.exe", "size": 1,
                        "browser_download_url": "https://evil.example/setup.exe"}]}"#;
        let r = parse_release(json, false).unwrap();
        assert_eq!(r.page_url, RELEASES_PAGE);
        assert!(r.setup.is_none());
        assert!(parse_release(r#"{"tag_name": "v0.3.0", "prerelease": true}"#, false).is_err());
    }

    #[test]
    fn checksum_files_are_read_strictly() {
        let h = "AB".repeat(32);
        assert_eq!(
            parse_sha256_file(&format!("{h}  vidar-0.2.0-setup.exe\n")),
            Some("ab".repeat(32))
        );
        assert_eq!(parse_sha256_file("nothex  file"), None);
        assert_eq!(parse_sha256_file(""), None);
    }

    /// Talks to GitHub: `cargo test -p vidar -- --ignored live_latest_release`.
    #[test]
    #[ignore = "network"]
    fn live_latest_release() {
        let r = fetch_latest().expect("GitHub reachable");
        assert!(parse_version(&r.version).is_some(), "{r:?}");
        eprintln!("latest release: {r:?}");
    }
}
