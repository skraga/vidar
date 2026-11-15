//! `vidar-cli endpoints ...`: endpoint tools for the installer: default device backup and
//! restore, cable checks, branding, driver install and the device report. Exit code 0 = success, 1 = failure / not found.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use vidar_win::branding::{self, Brand};
use vidar_win::driver;
use vidar_win::router::{
    best_physical, cable_capture, cable_render, ensure_physical_default, restore_default,
};
use vidar_win::{AudioSystem, Flow, WinAudio};

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List active recording and playback endpoints
    List,
    /// Device report (default devices, cable status)
    Report,
    /// Exit 0 if the VB-Audio Virtual Cable is installed and active
    CheckCable,
    /// Print "<id>|<name>" of the default recording device
    GetDefaultCapture,
    /// Print the cable's recording endpoint ID
    GetCableCapture,
    /// Make an endpoint the default (all roles)
    SetDefault { id: String },
    /// Disable an endpoint (hidden from every device list); --enable turns it back on
    Disable {
        id: String,
        #[arg(long)]
        enable: bool,
    },
    /// Save the default playback (or recording) device ID to FILE, unless it is virtual
    Backup {
        file: PathBuf,
        #[arg(long)]
        capture: bool,
    },
    /// Restore the default from FILE (or an endpoint ID); falls back to the best physical device
    Restore {
        file_or_id: String,
        #[arg(long)]
        capture: bool,
    },
    /// If the default playback (or recording) device is virtual, switch to a physical one
    EnsurePhysical {
        #[arg(long)]
        capture: bool,
    },
    /// Rename the cable to "Vidar AI Microphone" / "Vidar Internal" and set its icon
    Brand {
        /// Icon as "C:\path\vidar.ico,0"
        #[arg(long)]
        icon: Option<String>,
        #[arg(long)]
        capture_name: Option<String>,
        #[arg(long)]
        render_name: Option<String>,
        /// Where the original look is saved (default: %ProgramData%\Vidar\endpoint-branding.tsv)
        #[arg(long)]
        backup: Option<PathBuf>,
    },
    /// Put the cable's original names and icons back
    Unbrand {
        #[arg(long)]
        backup: Option<PathBuf>,
    },
    /// Install the bundled cable driver, keep the default devices, brand the cable
    InstallDriver {
        /// Folder with VBCABLE_Setup_x64.exe (default: drivers\vbcable next to the program)
        #[arg(long)]
        driver_dir: Option<PathBuf>,
        #[arg(long)]
        icon: Option<String>,
        #[arg(long)]
        no_brand: bool,
    },
    /// Unbrand and uninstall the cable driver
    UninstallDriver {
        #[arg(long)]
        driver_dir: Option<PathBuf>,
    },
}

fn flow(capture: bool) -> Flow {
    if capture {
        Flow::Capture
    } else {
        Flow::Render
    }
}

fn ok(b: bool) -> ExitCode {
    if b {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn default_driver_dir() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_default();
    let installed = exe_dir.join("drivers").join("vbcable");
    if installed.is_dir() {
        return installed;
    }
    // Development builds.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../drivers/vbcable")
}

pub fn run(action: Action) -> ExitCode {
    let audio = match WinAudio::new() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[ERROR] {e}");
            return ExitCode::from(1);
        }
    };
    match action {
        Action::List => {
            for f in [Flow::Capture, Flow::Render] {
                let def = audio.default_endpoint(f).map(|e| e.id);
                for e in audio.endpoints(f) {
                    let mark = if def.as_deref() == Some(e.id.as_str()) {
                        "*"
                    } else {
                        " "
                    };
                    let kind = if e.is_vb_cable() {
                        "cable"
                    } else if e.is_virtual() {
                        "virtual"
                    } else {
                        "physical"
                    };
                    println!(
                        "{mark} {:<8} {:<9} {:<55} {}",
                        format!("{f:?}"),
                        kind,
                        e.name,
                        e.id
                    );
                }
            }
            ExitCode::SUCCESS
        }
        Action::Report => {
            let cap = cable_capture(&audio, None);
            let ren = cable_render(&audio, None);
            println!("\n=== Vidar audio device report ===");
            match audio.default_endpoint(Flow::Capture) {
                Some(d) => println!(
                    " Default microphone : {}{}",
                    d.name,
                    if d.is_virtual() { "  (virtual)" } else { "" }
                ),
                None => println!(" Default microphone : none"),
            }
            match audio.default_endpoint(Flow::Render) {
                Some(d) => println!(" Default playback   : {}", d.name),
                None => println!(" Default playback   : none"),
            }
            match (&cap, &ren) {
                (Some(c), Some(r)) => {
                    println!(" Virtual cable      : INSTALLED");
                    println!("   recording side   : {}", c.name);
                    println!("   playback side    : {}", r.name);
                }
                _ => println!(
                    " Virtual cable      : NOT FOUND (vidar-cli --install-driver installs it)"
                ),
            }
            println!(" Microphones:");
            for e in audio
                .endpoints(Flow::Capture)
                .iter()
                .filter(|e| !e.is_virtual())
            {
                println!("   - {} [{:?}]", e.name, e.bus);
            }
            println!();
            ExitCode::SUCCESS
        }
        Action::CheckCable => {
            let found = cable_render(&audio, None);
            match &found {
                Some(e) => println!("VB-Cable is installed and active: {}", e.name),
                None => println!("VB-Cable is NOT installed"),
            }
            ok(found.is_some())
        }
        Action::GetDefaultCapture => match audio.default_endpoint(Flow::Capture) {
            Some(d) => {
                println!("{}|{}", d.id, d.name);
                ExitCode::SUCCESS
            }
            None => ExitCode::from(1),
        },
        Action::GetCableCapture => match cable_capture(&audio, None) {
            Some(c) => {
                println!("{}", c.id);
                ExitCode::SUCCESS
            }
            None => ExitCode::from(1),
        },
        Action::SetDefault { id } => {
            let done = vidar_win::is_valid_endpoint_id(&id) && audio.set_default(id.trim());
            println!(
                "{}",
                if done {
                    "default set"
                } else {
                    "could not set default"
                }
            );
            ok(done)
        }
        Action::Disable { id, enable } => {
            let id = id.trim();
            match vidar_win::is_valid_endpoint_id(id).then(|| audio.set_enabled(id, enable)) {
                Some(Ok(())) => {
                    println!("{} {id}", if enable { "enabled" } else { "disabled" });
                    ok(true)
                }
                Some(Err(e)) => {
                    eprintln!("[ERROR] {e}");
                    ok(false)
                }
                None => {
                    eprintln!("[ERROR] not an endpoint ID: {id}");
                    ok(false)
                }
            }
        }
        Action::Backup { file, capture } => match audio
            .default_endpoint(flow(capture))
            .filter(|d| !d.is_virtual())
        {
            Some(d) => match std::fs::write(&file, &d.id) {
                Ok(()) => {
                    println!("Backed up default: {} ({})", d.name, d.id);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("[ERROR] {e}");
                    ExitCode::from(1)
                }
            },
            None => {
                println!("Default is virtual or missing; nothing backed up");
                ExitCode::from(1)
            }
        },
        Action::Restore {
            file_or_id,
            capture,
        } => {
            let id = std::fs::read_to_string(&file_or_id)
                .map(|s| s.trim().to_string())
                .unwrap_or(file_or_id);
            if restore_default(&audio, &id) {
                println!("Restored default: {id}");
                return ExitCode::SUCCESS;
            }
            ensure(&audio, flow(capture))
        }
        Action::EnsurePhysical { capture } => ensure(&audio, flow(capture)),
        Action::Brand {
            icon,
            capture_name,
            render_name,
            backup,
        } => {
            let mut b = Brand {
                icon,
                ..Brand::default()
            };
            if let Some(n) = capture_name {
                b.capture_name = n;
            }
            if let Some(n) = render_name {
                b.render_name = n;
            }
            let backup = backup.unwrap_or_else(driver::default_branding_backup);
            match branding::brand(&audio, &b, &backup) {
                Ok((c, r)) => {
                    println!("Cable now shows as:\n  {}\n  {}", c.name, r.name);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("[ERROR] branding failed: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Action::Unbrand { backup } => match branding::unbrand(
            &audio,
            &backup.unwrap_or_else(driver::default_branding_backup),
        ) {
            Ok((c, r)) => {
                println!("Cable restored to:\n  {}\n  {}", c.name, r.name);
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("[ERROR] {e}");
                ExitCode::from(1)
            }
        },
        Action::InstallDriver {
            driver_dir,
            icon,
            no_brand,
        } => {
            drop(audio);
            let dir = driver_dir.unwrap_or_else(default_driver_dir);
            let brand = Brand {
                icon,
                ..Brand::default()
            };
            let backup = driver::default_branding_backup();
            println!(
                "[INSTALL] Installing VB-Audio Virtual Cable (Windows will ask for permission)..."
            );
            match driver::install(&dir, (!no_brand).then_some((&brand, backup.as_path()))) {
                Ok(r) => {
                    println!("[INSTALL] Done.");
                    if let Some((c, rn)) = r.branded {
                        println!("          Cable shows as: {c} / {rn}");
                    }
                    if let Some(e) = r.brand_error {
                        println!("          Branding skipped: {e}");
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("[ERROR] {e}");
                    ExitCode::from(1)
                }
            }
        }
        Action::UninstallDriver { driver_dir } => {
            drop(audio);
            let dir = driver_dir.unwrap_or_else(default_driver_dir);
            ok(driver::uninstall(&dir, &driver::default_branding_backup()).is_ok())
        }
    }
}

fn ensure(audio: &WinAudio, f: Flow) -> ExitCode {
    match ensure_physical_default(audio, f) {
        Ok(Some(e)) => {
            println!("Default set to physical device: {}", e.name);
            ExitCode::SUCCESS
        }
        Ok(None) => {
            println!("Default is already a physical device");
            ExitCode::SUCCESS
        }
        Err(e) => {
            println!("{e}");
            // No physical device is not a hard failure for the installer.
            let _ = best_physical(audio, f);
            ExitCode::SUCCESS
        }
    }
}
