//! `cargo xtask <task>` (alias in .cargo/config.toml).
//!
//!   fetch         RNNoise weights + default DPDFNet model, hash-checked
//!   fetch --all   also the larger dpdfnet8 model
//!   dist          release build + everything it needs, licences included, in dist\vidar
//!   installer     dist, then dist\vidar-<version>-setup.exe (Inno Setup 6) and its .sha256
//!   outdated      check every bundled or downloaded component for a newer upstream version

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const DPDFNET_BASE: &str = "https://huggingface.co/Ceva-IP/DPDFNet/resolve/main/onnx";
/// ONNX Runtime comes from Microsoft's NuGet package: the GitHub repository (and its release
/// zips) disappeared on 2026-11-18. The package holds the same Microsoft-signed DLL.
const ORT_VERSION: &str = "1.32.0";
const ORT_NUPKG_SHA256: &str = "a9150bc2992cbbb6722070da9910e1ffa989a8bfc67bdd5bb8e745ca755641db";
const ORT_NUGET: &str = "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime";
/// `VBCABLE_Driver_Pack<N>.zip` that drivers\vbcable was taken from.
const VBCABLE_PACK: u32 = 46;

const DPDFNET_MODELS: [(&str, &str); 2] = [
    (
        "dpdfnet2_48khz_hr",
        "7f0575a5cec0ba4ffd8f8bd657e06d007e4ccdd955d76faab922b9d3291dc14b",
    ),
    (
        "dpdfnet8_48khz_hr",
        "7b3afbb260a08fe9af3d16e3bda992971be1e7e951d1dee7c2d235f5c43f5631",
    ),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// DPDFNet models folder, third_party/dpdfnet (not in git; filled by `cargo xtask fetch`).
pub fn models_dir() -> PathBuf {
    workspace_root().join("third_party").join("dpdfnet")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn download(url: &str) -> Result<Vec<u8>> {
    println!("[..] {url}");
    let mut body = Vec::new();
    ureq::get(url)
        .call()?
        .into_body()
        .into_with_config()
        .limit(200 * 1024 * 1024)
        .reader()
        .read_to_end(&mut body)?;
    Ok(body)
}

fn fetch_rnnoise() -> Result<()> {
    let rn = workspace_root().join("third_party").join("rnnoise");
    let out = rn.join("model");
    if out.join("rnnoise_data.c").is_file() && out.join("rnnoise_data.h").is_file() {
        println!("[OK] RNNoise weights already present");
        return Ok(());
    }
    let hash = fs::read_to_string(rn.join("model_version"))?
        .trim()
        .to_string();
    let tarball = download(&format!(
        "https://media.xiph.org/rnnoise/models/rnnoise_data-{hash}.tar.gz"
    ))?;
    let actual = sha256_hex(&tarball);
    if actual != hash {
        return Err(format!("RNNoise model checksum mismatch: {actual} != {hash}").into());
    }
    fs::create_dir_all(&out)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(&tarball[..]));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if name == "rnnoise_data.c" || name == "rnnoise_data.h" {
            entry.unpack(out.join(&name))?;
        }
    }
    println!("[OK] RNNoise weights -> {}", out.display());
    Ok(())
}

/// Microsoft's official ONNX Runtime build (signed), extracted to third_party/onnxruntime.
/// `VERSION` there records which one, so bumping `ORT_VERSION` fetches it again.
fn fetch_onnxruntime() -> Result<()> {
    let out = workspace_root().join("third_party").join("onnxruntime");
    let stamp = out.join("VERSION");
    let have = fs::read_to_string(&stamp).unwrap_or_default();
    if out.join("onnxruntime.dll").is_file() && have.trim() == ORT_VERSION {
        println!("[OK] onnxruntime.dll {ORT_VERSION} already present");
        return Ok(());
    }
    let body = download(&format!(
        "{ORT_NUGET}/{ORT_VERSION}/microsoft.ml.onnxruntime.{ORT_VERSION}.nupkg"
    ))?;
    let actual = sha256_hex(&body);
    if actual != ORT_NUPKG_SHA256 {
        return Err(
            format!("ONNX Runtime checksum mismatch: {actual} != {ORT_NUPKG_SHA256}").into(),
        );
    }
    fs::create_dir_all(&out)?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(body))?;
    for (entry, name) in [
        ("runtimes/win-x64/native/onnxruntime.dll", "onnxruntime.dll"),
        ("LICENSE", "LICENSE"),
        ("ThirdPartyNotices.txt", "ThirdPartyNotices.txt"),
    ] {
        let mut bytes = Vec::new();
        zip.by_name(entry)?.read_to_end(&mut bytes)?;
        fs::write(out.join(name), bytes)?;
    }
    fs::write(&stamp, ORT_VERSION)?;
    println!("[OK] onnxruntime.dll {ORT_VERSION} -> {}", out.display());
    Ok(())
}

fn fetch_dpdfnet(all: bool) -> Result<()> {
    let dir = models_dir();
    fs::create_dir_all(&dir)?;
    for (i, (name, expected)) in DPDFNET_MODELS.iter().enumerate() {
        if i > 0 && !all {
            continue;
        }
        let target = dir.join(format!("{name}.onnx"));
        if target.is_file() && sha256_hex(&fs::read(&target)?) == *expected {
            println!("[OK] {name}.onnx already present");
            continue;
        }
        let body = download(&format!("{DPDFNET_BASE}/{name}.onnx?download=true"))?;
        let actual = sha256_hex(&body);
        if actual != *expected {
            return Err(format!("{name}.onnx checksum mismatch: {actual} != {expected}").into());
        }
        let tmp = target.with_extension("part");
        fs::write(&tmp, &body)?;
        fs::rename(&tmp, &target)?;
        println!("[OK] {name}.onnx");
    }
    Ok(())
}

/// Copies `src` into `dst`, creating `dst` and its subfolders.
fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Builds the release binaries and assembles a folder that can be zipped and shipped as is:
///
/// ```text
/// dist\vidar\
///   vidar.exe, vidar-cli.exe, onnxruntime.dll
///   models\*.onnx
///   drivers\vbcable\     VB-Audio's package, unmodified
///   LICENSE, THIRD_PARTY_NOTICES.md
///   licenses\            upstream licence texts the notices refer to, plus
///                        THIRD_PARTY_LICENSES.html (every Rust crate, from cargo-about)
/// ```
///
/// Every licence file is required: a missing one fails the build instead of shipping without it.
/// Needs cargo-about (`cargo install cargo-about --locked`).
fn dist(all_models: bool) -> Result<()> {
    let root = workspace_root();
    fetch_rnnoise()?;
    fetch_onnxruntime()?;
    fetch_dpdfnet(all_models)?;

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());

    // Licence texts of every Rust dependency (about.toml, about.hbs). Runs before the long
    // build so a missing cargo-about fails fast.
    // Written to a file (-o): cargo-about refuses a redirected stdout under PowerShell.
    let about_file = root.join("target").join("THIRD_PARTY_LICENSES.html");
    let _ = fs::remove_file(&about_file);
    let about = std::process::Command::new(&cargo)
        .current_dir(&root)
        .args(["about", "generate", "--workspace", "-o"])
        .arg(&about_file)
        .arg("about.hbs")
        .status()?;
    let crate_licenses = fs::read(&about_file).unwrap_or_default();
    if !about.success() || crate_licenses.is_empty() {
        return Err(
            "cargo about failed; install it with `cargo install cargo-about --locked --features cli`"
                .into(),
        );
    }

    let status = std::process::Command::new(&cargo)
        .current_dir(&root)
        .args([
            "build",
            "--release",
            "--locked",
            "-p",
            "vidar",
            "-p",
            "vidar-cli",
        ])
        .status()?;
    if !status.success() {
        return Err("release build failed".into());
    }

    let out = root.join("dist").join("vidar");
    if out.exists() {
        fs::remove_dir_all(&out)?;
    }
    fs::create_dir_all(out.join("licenses"))?;
    fs::create_dir_all(out.join("models"))?;

    let release = root.join("target").join("release");
    let ort = root.join("third_party").join("onnxruntime");
    let rnnoise = root.join("third_party").join("rnnoise");
    let vendored = root.join("third_party").join("licenses");
    // (source, path inside dist\vidar)
    let mut files: Vec<(PathBuf, String)> = vec![
        (release.join("vidar.exe"), "vidar.exe".into()),
        (release.join("vidar-cli.exe"), "vidar-cli.exe".into()),
        (ort.join("onnxruntime.dll"), "onnxruntime.dll".into()),
        (root.join("LICENSE"), "LICENSE".into()),
        (
            root.join("THIRD_PARTY_NOTICES.md"),
            "THIRD_PARTY_NOTICES.md".into(),
        ),
        (
            ort.join("LICENSE"),
            "licenses/onnxruntime-LICENSE.txt".into(),
        ),
        (
            ort.join("ThirdPartyNotices.txt"),
            "licenses/onnxruntime-ThirdPartyNotices.txt".into(),
        ),
        (
            rnnoise.join("COPYING"),
            "licenses/rnnoise-COPYING.txt".into(),
        ),
        (
            rnnoise.join("AUTHORS"),
            "licenses/rnnoise-AUTHORS.txt".into(),
        ),
        (
            vendored.join("Apache-2.0.txt"),
            "licenses/dpdfnet-Apache-2.0.txt".into(),
        ),
        (
            vendored.join("MPL-2.0.txt"),
            "licenses/symphonia-MPL-2.0.txt".into(),
        ),
        (
            root.join("assets").join("fonts").join("OFL.txt"),
            "licenses/inter-OFL.txt".into(),
        ),
    ];
    for name in [
        "egui-fonts-OFL.txt",
        "egui-fonts-UFL.txt",
        "egui-fonts-Hack.txt",
        "egui-fonts-emoji-icon-font-MIT.txt",
        "egui-fonts-egui-icons.txt",
    ] {
        files.push((vendored.join(name), format!("licenses/{name}")));
    }
    for (from, to) in &files {
        if !from.is_file() {
            return Err(format!("missing {}", from.display()).into());
        }
        fs::copy(from, out.join(to))?;
    }
    fs::write(
        out.join("licenses").join("THIRD_PARTY_LICENSES.html"),
        crate_licenses,
    )?;
    for (i, (name, _)) in DPDFNET_MODELS.iter().enumerate() {
        if i > 0 && !all_models {
            continue;
        }
        let model = format!("{name}.onnx");
        fs::copy(models_dir().join(&model), out.join("models").join(&model))?;
    }
    copy_dir(
        &root.join("drivers").join("vbcable"),
        &out.join("drivers").join("vbcable"),
    )?;

    println!("[OK] {}", out.display());
    Ok(())
}

/// `[workspace.package] version` from the root Cargo.toml (xtask's own version is 0.0.0).
fn workspace_version() -> Result<String> {
    let toml = fs::read_to_string(workspace_root().join("Cargo.toml"))?;
    let mut in_package = false;
    for line in toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[workspace.package]";
        } else if in_package {
            if let Some(v) = line.strip_prefix("version") {
                let v = v
                    .trim_start()
                    .trim_start_matches('=')
                    .trim()
                    .trim_matches('"');
                if !v.is_empty() {
                    return Ok(v.to_string());
                }
            }
        }
    }
    Err("no version in [workspace.package] of Cargo.toml".into())
}

/// The Inno Setup 6 compiler: `ISCC` if set, the usual install folders, else PATH.
fn iscc() -> PathBuf {
    if let Some(p) = std::env::var_os("ISCC") {
        return PathBuf::from(p);
    }
    let candidates = [
        std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
        std::env::var_os("ProgramFiles").map(PathBuf::from),
        std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Programs")),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(|d| d.join("Inno Setup 6").join("ISCC.exe"))
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("ISCC"))
}

/// `dist`, then the setup program from installer\vidar.iss:
/// `dist\vidar-<version>-setup.exe` plus `<that>.sha256`, which the app's updater checks.
/// Needs Inno Setup 6 (`winget install JRSoftware.InnoSetup`).
fn installer(all_models: bool) -> Result<()> {
    dist(all_models)?;
    let root = workspace_root();
    let version = workspace_version()?;
    let out = root.join("dist");
    let source = out.join("vidar");

    // The setup's own icon, from the same drawing as the program's.
    let icon = out.join("vidar.ico");
    let status = std::process::Command::new(source.join("vidar.exe"))
        .arg("--write-icon")
        .arg(&icon)
        .status()?;
    if !status.success() || !icon.is_file() {
        return Err("vidar.exe --write-icon failed".into());
    }

    let compiler = iscc();
    println!("[..] {}", compiler.display());
    let status = std::process::Command::new(&compiler)
        .current_dir(root.join("installer"))
        .arg("/Q")
        .arg(format!("/DAppVersion={version}"))
        .arg(format!("/DSourceDir={}", source.display()))
        .arg(format!("/DOutputDir={}", out.display()))
        .arg(format!("/DIconFile={}", icon.display()))
        .arg("vidar.iss")
        .status()
        .map_err(|e| {
            format!(
                "cannot run {} ({e}); install Inno Setup 6 (winget install JRSoftware.InnoSetup) or set ISCC",
                compiler.display()
            )
        })?;
    if !status.success() {
        return Err("Inno Setup failed".into());
    }

    let name = format!("vidar-{version}-setup.exe");
    let setup = out.join(&name);
    let hash = sha256_hex(&fs::read(&setup)?);
    fs::write(
        out.join(format!("{name}.sha256")),
        format!("{hash}  {name}\n"),
    )?;
    println!("[OK] {}", setup.display());
    Ok(())
}

fn get_text(url: &str) -> Result<String> {
    let mut req = ureq::get(url).header("User-Agent", "vidar-xtask");
    // Optional, only to lift GitHub's anonymous rate limit; sent to api.github.com only.
    if url.starts_with("https://api.github.com/") {
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            req = req.header("Authorization", &format!("Bearer {token}"));
        }
    }
    Ok(req.call()?.into_body().read_to_string()?)
}

fn get_json(url: &str) -> Result<serde_json::Value> {
    Ok(serde_json::from_str(&get_text(url)?)?)
}

/// `1.32.0` -> `[1, 32, 0]`; `None` for pre-releases such as `1.33.0-rc1`.
fn stable_version(v: &str) -> Option<Vec<u64>> {
    v.split('.').map(|p| p.parse().ok()).collect()
}

/// One line of the `outdated` report. Returns whether an update is available.
fn report(what: &str, check: impl FnOnce() -> Result<Option<String>>) -> bool {
    match check() {
        Ok(None) => {
            println!("[OK]  {what}");
            false
        }
        Ok(Some(news)) => {
            println!("[NEW] {what}: {news}");
            true
        }
        Err(e) => {
            println!("[??]  {what}: check failed: {e}");
            false
        }
    }
}

/// Compares every pinned component with its upstream and says what is newer. Changes nothing.
/// Exits with 1 when something is newer, so it can gate a scheduled job.
fn outdated() -> Result<bool> {
    let root = workspace_root();
    let mut newer = false;

    newer |= report(&format!("ONNX Runtime {ORT_VERSION}"), || {
        let index = get_json(&format!("{ORT_NUGET}/index.json"))?;
        let ours = stable_version(ORT_VERSION);
        let latest = index["versions"]
            .as_array()
            .ok_or("no versions in the NuGet index")?
            .iter()
            .filter_map(|v| v.as_str())
            .filter_map(|v| stable_version(v).map(|n| (n, v)))
            .max();
        Ok(match latest {
            Some((n, v)) if Some(&n) > ours.as_ref() => Some(format!(
                "{v} on NuGet. Bump ORT_VERSION and ORT_NUPKG_SHA256 in xtask/src/main.rs, and \
                 check that the `ort` crate's api-NN feature (crates/vidar-denoise/Cargo.toml) \
                 supports it"
            )),
            _ => None,
        })
    });

    let hf = || get_json("https://huggingface.co/api/models/Ceva-IP/DPDFNet/tree/main/onnx");
    let tree = hf();
    for (name, pinned) in DPDFNET_MODELS {
        newer |= report(&format!("DPDFNet {name}"), || {
            let tree = tree.as_ref().map_err(|e| e.to_string())?;
            let path = format!("onnx/{name}.onnx");
            let file = tree
                .as_array()
                .and_then(|a| a.iter().find(|f| f["path"] == path.as_str()))
                .ok_or("no longer on Hugging Face")?;
            let upstream = file["lfs"]["oid"].as_str().unwrap_or_default();
            Ok((upstream != pinned).then(|| {
                format!("changed on Hugging Face (SHA-256 {upstream}); update DPDFNET_MODELS")
            }))
        });
    }
    newer |= report("DPDFNet 48 kHz model list", || {
        let tree = tree.as_ref().map_err(|e| e.to_string())?;
        let new: Vec<&str> = tree
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| {
                f["path"]
                    .as_str()?
                    .strip_prefix("onnx/")?
                    .strip_suffix(".onnx")
            })
            .filter(|n| n.contains("48khz") && !DPDFNET_MODELS.iter().any(|(m, _)| m == n))
            .collect();
        Ok((!new.is_empty()).then(|| format!("new model(s): {}", new.join(", "))))
    });

    let rn = root.join("third_party").join("rnnoise");
    newer |= report("RNNoise source (third_party/rnnoise)", || {
        let ours = fs::read_to_string(rn.join("UPSTREAM_COMMIT"))?;
        let head = get_json("https://api.github.com/repos/xiph/rnnoise/commits/main")?;
        let sha = head["sha"].as_str().ok_or("no commit sha")?;
        Ok((sha != ours.trim()).then(|| {
            let date = head["commit"]["committer"]["date"].as_str().unwrap_or("?");
            format!(
                "xiph/rnnoise main is at {sha} ({date}); vendored {}",
                ours.trim()
            )
        }))
    });
    newer |= report("RNNoise weights", || {
        let ours = fs::read_to_string(rn.join("model_version"))?;
        let theirs = get_text("https://raw.githubusercontent.com/xiph/rnnoise/main/model_version")?;
        Ok((theirs.trim() != ours.trim()).then(|| {
            format!(
                "upstream model_version is {}; update it together with the source",
                theirs.trim()
            )
        }))
    });

    newer |= report(&format!("VB-CABLE Pack{VBCABLE_PACK}"), || {
        let page = get_text("https://vb-audio.com/Cable/")?;
        let latest = page
            .split("VBCABLE_Driver_Pack")
            .skip(1)
            .filter_map(|s| {
                let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
                digits.parse::<u32>().ok()
            })
            .max()
            .ok_or("no driver pack link on vb-audio.com/Cable")?;
        Ok((latest > VBCABLE_PACK).then(|| {
            format!(
                "Pack{latest} on vb-audio.com; replace drivers/vbcable with it unmodified and \
                 update VBCABLE_PACK and THIRD_PARTY_NOTICES.md"
            )
        }))
    });

    // Rust crates: what `cargo update` would change within the current version requirements.
    // (GitHub Actions pins are kept current by Dependabot.)
    println!("[..]  Rust crates (cargo update --dry-run):");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = std::process::Command::new(cargo)
        .current_dir(&root)
        .args(["update", "--dry-run"])
        .output()?;
    let text = String::from_utf8_lossy(&out.stderr);
    let changes: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("Updating ") && l.contains(" -> "))
        .collect();
    for line in &changes {
        println!("        {line}");
    }
    if changes.is_empty() {
        println!("        none within the current version requirements");
    }
    if let Some(note) = text.lines().find(|l| l.contains("behind latest")) {
        println!("        {}", note.trim().trim_start_matches("note: "));
    }
    newer |= !changes.is_empty();

    Ok(newer)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let all = args.iter().any(|a| a == "--all");
    match args.first().map(String::as_str) {
        Some("fetch") => {
            fetch_rnnoise()?;
            fetch_onnxruntime()?;
            fetch_dpdfnet(all)?;
            Ok(())
        }
        Some("dist") => dist(all),
        Some("installer") => installer(all),
        Some("outdated") => {
            if outdated()? {
                std::process::exit(1);
            }
            Ok(())
        }
        _ => {
            eprintln!(
                "usage: cargo xtask fetch [--all] | dist [--all] | installer [--all] | outdated"
            );
            std::process::exit(2);
        }
    }
}
