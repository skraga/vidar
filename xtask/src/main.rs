//! `cargo xtask <task>` (alias in .cargo/config.toml).
//!
//!   fetch         RNNoise weights + default DPDFNet model, hash-checked
//!   fetch --all   also the larger dpdfnet8 model
//!   dist          release build + everything it needs, licences included, in dist\vidar
//!   installer     dist, then dist\vidar-<version>-setup.exe (Inno Setup 6) and its .sha256

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const DPDFNET_BASE: &str = "https://huggingface.co/Ceva-IP/DPDFNet/resolve/main/onnx";
const ORT_VERSION: &str = "1.32.0";
const ORT_ZIP_SHA256: &str = "aaa1d04fcf86bd31e84b90bc0cbde80e632bd0720dcc5d8ba05c4fe45c09d127";

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

/// DPDFNet models folder (not in git; filled by `cargo xtask fetch`).
pub fn models_dir() -> PathBuf {
    workspace_root().join("models")
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
fn fetch_onnxruntime() -> Result<()> {
    let out = workspace_root().join("third_party").join("onnxruntime");
    let dll = out.join("onnxruntime.dll");
    if dll.is_file() {
        println!("[OK] onnxruntime.dll already present");
        return Ok(());
    }
    let url = format!("https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/onnxruntime-win-x64-{ORT_VERSION}.zip");
    let body = download(&url)?;
    let actual = sha256_hex(&body);
    if actual != ORT_ZIP_SHA256 {
        return Err(format!("ONNX Runtime checksum mismatch: {actual} != {ORT_ZIP_SHA256}").into());
    }
    fs::create_dir_all(&out)?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(body))?;
    for name in ["onnxruntime.dll", "LICENSE", "ThirdPartyNotices.txt"] {
        let suffix = format!("/{name}");
        let entry = (0..zip.len())
            .find(|&i| matches!(zip.name_for_index(i), Some(Ok(n)) if n.ends_with(&suffix)));
        if let Some(i) = entry {
            let mut f = zip.by_index(i)?;
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes)?;
            fs::write(out.join(name), bytes)?;
        }
    }
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
        _ => {
            eprintln!("usage: cargo xtask fetch [--all] | dist [--all] | installer [--all]");
            std::process::exit(2);
        }
    }
}
