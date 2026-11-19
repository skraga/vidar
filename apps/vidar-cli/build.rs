//! Embeds the Vidar icon and version information into vidar-cli.exe. Windows shows them in
//! the UAC prompt when the CLI installs the cable driver, and code signing requires them.
fn main() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = out.join("vidar.ico");
    std::fs::write(&ico, vidar_brand::ico(true)).expect("write icon");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(ico.to_str().unwrap());
        res.set("ProductName", "Vidar");
        res.set("FileDescription", "Vidar command line");
        res.set("LegalCopyright", "MIT License");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the icon resource: {e}");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
