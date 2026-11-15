//! Renaming the VB-Cable endpoints and setting their icon ("Vidar AI Microphone" / "Vidar Internal"),
//! and putting the original look back.
//!
//! Only the endpoint description is changed; the interface part "(VB-Audio Virtual Cable)"
//! comes from the driver and stays, which is also how Vidar keeps recognising the cable.
//! The originals are saved to a small backup file before anything is changed.

use std::path::Path;

use crate::model::{Endpoint, Flow};
use crate::router::{cable_capture, cable_render, AudioSystem};
use crate::sys::WinAudio;
use crate::Error;

/// The look to apply.
#[derive(Debug, Clone)]
pub struct Brand {
    pub capture_name: String,
    pub render_name: String,
    /// Icon for both endpoints: `"C:\Program Files\Vidar\vidar.ico,0"`.
    pub icon: Option<String>,
}

/// Name apps see for the clean microphone.
pub const CAPTURE_NAME: &str = "Vidar AI Microphone";
/// Name of the cable's playback side, which only Vidar writes to.
pub const RENDER_NAME: &str = "Vidar Internal";
/// Capture names earlier versions gave the cable, recognised as Vidar's own.
pub const LEGACY_CAPTURE_NAMES: [&str; 1] = ["Vidar Mic"];

impl Default for Brand {
    fn default() -> Self {
        Self {
            capture_name: CAPTURE_NAME.into(),
            render_name: RENDER_NAME.into(),
            icon: None,
        }
    }
}

/// Values found on an endpoint before branding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Original {
    pub id: String,
    pub description: String,
    pub icon: Option<String>,
}

/// Stock names from the VB-Cable driver, used when no backup exists.
const STOCK_CAPTURE: &str = "CABLE Output";
const STOCK_RENDER: &str = "CABLE Input";

fn save_backup(path: &Path, originals: &[Original]) -> Result<(), Error> {
    let mut text = String::from(
        "# Vidar: VB-Cable endpoint look before branding (id \\t description \\t icon)\n",
    );
    for o in originals {
        text.push_str(&format!(
            "{}\t{}\t{}\n",
            o.id,
            o.description,
            o.icon.as_deref().unwrap_or("")
        ));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::Other(e.to_string()))?;
    }
    std::fs::write(path, text).map_err(|e| Error::Other(e.to_string()))
}

fn load_backup(path: &Path) -> Vec<Original> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let mut parts = l.splitn(3, '\t');
            let id = parts.next()?.to_string();
            let description = parts.next()?.to_string();
            let icon = parts.next().map(str::to_string).filter(|s| !s.is_empty());
            Some(Original {
                id,
                description,
                icon,
            })
        })
        .collect()
}

/// Every active VB-Cable endpoint, both directions.
fn all_cable_endpoints(sys: &WinAudio) -> Vec<Endpoint> {
    [Flow::Capture, Flow::Render]
        .into_iter()
        .flat_map(|f| sys.endpoints(f))
        .filter(|e| e.is_vb_cable())
        .collect()
}

fn cable_pair(sys: &WinAudio) -> Result<(Endpoint, Endpoint), Error> {
    let cap = cable_capture(sys, None)
        .ok_or_else(|| Error::Other("VB-Cable recording endpoint not found".into()))?;
    let ren = cable_render(sys, None)
        .ok_or_else(|| Error::Other("VB-Cable playback endpoint not found".into()))?;
    Ok((cap, ren))
}

/// File next to the branding backup listing the cable endpoints Vidar disabled.
fn disabled_list(backup: &Path) -> std::path::PathBuf {
    backup.with_file_name("endpoint-disabled.txt")
}

/// The endpoint to keep in each direction. VB-Cable's playback side comes as "CABLE Input"
/// (shown as speakers) and "CABLE In 16ch" (a line-level jack); both feed the same cable.
/// Keep one that is not presented as speakers, so it looks least like a place to send
/// sound; otherwise the first listed.
fn keeper<'a>(sys: &WinAudio, eps: &'a [Endpoint]) -> Option<&'a Endpoint> {
    eps.iter()
        .find(|e| !sys.is_speaker(&e.id))
        .or_else(|| eps.first())
}

/// Disable every cable endpoint except one per direction, so the cable shows up once in
/// the device lists. The IDs are recorded so [`unbrand`] can enable them again.
fn disable_extras(sys: &WinAudio, backup: &Path) -> Result<(), Error> {
    let mut disabled: Vec<String> = std::fs::read_to_string(disabled_list(backup))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    let before = disabled.len();
    for flow in [Flow::Capture, Flow::Render] {
        let eps: Vec<Endpoint> = all_cable_endpoints(sys)
            .into_iter()
            .filter(|e| e.flow == flow)
            .collect();
        let Some(keep) = keeper(sys, &eps).map(|e| e.id.clone()) else {
            continue;
        };
        for ep in eps.iter().filter(|e| e.id != keep) {
            sys.set_enabled(&ep.id, false)?;
            if !disabled.contains(&ep.id) {
                disabled.push(ep.id.clone());
            }
        }
    }
    if disabled.len() != before {
        // Best effort: without the list, unbranding cannot re-enable them, but the cable still works.
        let _ = std::fs::write(
            disabled_list(backup),
            disabled.join(
                "
",
            ) + "
",
        );
    }
    Ok(())
}

/// Enable the cable endpoints [`disable_extras`] turned off and forget the list.
fn enable_extras(sys: &WinAudio, backup: &Path) {
    let list = disabled_list(backup);
    for id in std::fs::read_to_string(&list)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        let _ = sys.set_enabled(id, true);
    }
    let _ = std::fs::remove_file(list);
}

/// True if the cable needs [`brand`]: a name or icon differs from `brand` (after a Windows
/// or driver update, or a rename by an earlier Vidar version), or a direction has more than
/// one active endpoint.
pub fn needs_brand(sys: &WinAudio, brand: &Brand) -> bool {
    let all = all_cable_endpoints(sys);
    let duplicates = [Flow::Capture, Flow::Render]
        .into_iter()
        .any(|f| all.iter().filter(|e| e.flow == f).count() > 1);
    duplicates
        || all.iter().any(|ep| {
            let name = match ep.flow {
                Flow::Capture => &brand.capture_name,
                Flow::Render => &brand.render_name,
            };
            ep.description != *name
                || brand
                    .icon
                    .as_ref()
                    .is_some_and(|i| sys.endpoint_icon(&ep.id).as_ref() != Some(i))
        })
}

/// Apply `brand` to both cable endpoints and disable the extra ones (see [`disable_extras`]).
/// The first time, the current look is saved to `backup`; later calls keep that first
/// backup so unbranding always returns to stock.
pub fn brand(sys: &WinAudio, brand: &Brand, backup: &Path) -> Result<(Endpoint, Endpoint), Error> {
    cable_pair(sys)?; // fail early if the cable is missing
    let all = all_cable_endpoints(sys);

    let mut originals = load_backup(backup);
    let mut changed = false;
    for ep in &all {
        if !originals.iter().any(|o| o.id == ep.id) {
            // If the endpoint already carries a Vidar name (backup lost), record the stock name.
            let ours = ep.description == brand.capture_name
                || ep.description == brand.render_name
                || LEGACY_CAPTURE_NAMES.contains(&ep.description.as_str());
            let description = if ours {
                stock_name(ep.flow).to_string()
            } else {
                ep.description.clone()
            };
            originals.push(Original {
                id: ep.id.clone(),
                description,
                icon: sys.endpoint_icon(&ep.id),
            });
            changed = true;
        }
    }
    if changed {
        // Best effort: without a backup, unbranding falls back to the driver's stock names.
        let _ = save_backup(backup, &originals);
    }

    // Some driver versions create more than one endpoint per direction; brand them all.
    for ep in &all {
        let name = match ep.flow {
            Flow::Capture => &brand.capture_name,
            Flow::Render => &brand.render_name,
        };
        sys.set_description(&ep.id, name)?;
        if let Some(icon) = &brand.icon {
            sys.set_icon(&ep.id, icon)?;
        }
    }
    disable_extras(sys, backup)?;
    cable_pair(sys)
}

fn stock_name(flow: Flow) -> &'static str {
    match flow {
        Flow::Capture => STOCK_CAPTURE,
        Flow::Render => STOCK_RENDER,
    }
}

/// Put the original names and icons back (from `backup`, else VB-Cable's stock names).
/// The backup file is removed afterwards.
pub fn unbrand(sys: &WinAudio, backup: &Path) -> Result<(Endpoint, Endpoint), Error> {
    enable_extras(sys, backup);
    let (cap, ren) = cable_pair(sys)?;
    let originals = load_backup(backup);
    for ep in &all_cable_endpoints(sys) {
        let original = originals.iter().find(|o| o.id == ep.id);
        let name = original
            .map(|o| o.description.as_str())
            .unwrap_or(stock_name(ep.flow));
        sys.set_description(&ep.id, name)?;
        if let Some(icon) = original.and_then(|o| o.icon.as_deref()) {
            sys.set_icon(&ep.id, icon)?;
        }
    }
    let _ = std::fs::remove_file(backup);
    let cap = sys.endpoint(&cap.id).unwrap_or(cap);
    let ren = sys.endpoint(&ren.id).unwrap_or(ren);
    Ok((cap, ren))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_round_trips() {
        let dir = std::env::temp_dir().join(format!("vidar-brand-test-{}", std::process::id()));
        let path = dir.join("branding.tsv");
        let items = vec![
            Original {
                id: "{0.0.1.00000000}.{a}".into(),
                description: "CABLE Output".into(),
                icon: None,
            },
            Original {
                id: "{0.0.0.00000000}.{b}".into(),
                description: "CABLE Input".into(),
                icon: Some("%windir%\\x.dll,-1".into()),
            },
        ];
        save_backup(&path, &items).unwrap();
        assert_eq!(load_backup(&path), items);
        let _ = std::fs::remove_dir_all(dir);
    }
}
