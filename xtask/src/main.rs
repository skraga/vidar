//! `cargo xtask <task>` (alias in .cargo/config.toml).
//!
//!   fetch         RNNoise weights + default DPDFNet model, hash-checked
//!   fetch --all   also the larger dpdfnet8 model

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
        let entry = (0..zip.len()).find(|&i| matches!(zip.name_for_index(i), Some(Ok(n)) if n.ends_with(&suffix)));
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

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("fetch") => {
            let all = args.iter().any(|a| a == "--all");
            fetch_rnnoise()?;
            fetch_onnxruntime()?;
            fetch_dpdfnet(all)?;
            Ok(())
        }
        _ => {
            eprintln!("usage: cargo xtask fetch [--all]");
            std::process::exit(2);
        }
    }
}
