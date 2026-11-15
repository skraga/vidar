//! Microphone choices for the UI: labels and hints, devices identified by endpoint ID.

use vidar_win::{Endpoint, Flow};

/// One entry in the microphone picker.
#[derive(Debug, Clone, PartialEq)]
pub struct InputChoice {
    /// `None` for the "Windows default" entry.
    pub id: Option<String>,
    pub label: String,
    pub hint: Option<&'static str>,
    pub is_default: bool,
    pub bluetooth: bool,
}

/// Shorten long names for the picker (max 52 characters).
pub fn clean_label(name: &str) -> String {
    let name = name.trim();
    if name.chars().count() > 52 {
        let short: String = name.chars().take(49).collect();
        format!("{short}...")
    } else {
        name.to_string()
    }
}

#[derive(PartialEq)]
enum Role {
    Mic,
    Line,
    Mix,
}

fn role(name: &str) -> Option<Role> {
    let lower = name.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let mut found = Vec::new();
    if words.iter().any(|w| *w == "mic" || *w == "microphone") {
        found.push(Role::Mic);
    }
    if words.contains(&"line") {
        found.push(Role::Line);
    }
    if words.contains(&"mix") {
        found.push(Role::Mix);
    }
    if found.len() == 1 {
        found.pop()
    } else {
        None
    }
}

/// Advice for picks that are usually a mistake (Line In, Stereo Mix).
pub fn selection_hint(name: &str, others: &[&str]) -> Option<&'static str> {
    match role(name)? {
        Role::Line => {
            if others.iter().any(|o| role(o) == Some(Role::Mic)) {
                Some("Line In is for line-level sources (mixer, phone, console). For a microphone in the pink jack, choose the 'Microphone' entry instead.")
            } else {
                Some("Line In is for line-level sources (mixer, phone, console). A microphone normally needs the pink Mic jack.")
            }
        }
        Role::Mix => Some("Stereo Mix records what your PC plays, not your voice."),
        Role::Mic => None,
    }
}

/// Picker entries: "Windows default" first, then every active physical capture endpoint.
/// The cable and other virtual devices are left out (selecting them would loop audio).
pub fn input_choices(endpoints: &[Endpoint], default_id: Option<&str>) -> Vec<InputChoice> {
    let mics: Vec<&Endpoint> = endpoints
        .iter()
        .filter(|e| e.flow == Flow::Capture && e.is_active() && !e.is_virtual())
        .collect();
    let names: Vec<&str> = mics.iter().map(|e| e.name.as_str()).collect();

    let mut out = vec![InputChoice {
        id: None,
        label: "Windows default microphone".into(),
        hint: None,
        is_default: false,
        bluetooth: false,
    }];
    for e in mics {
        let others: Vec<&str> = names.iter().copied().filter(|n| *n != e.name).collect();
        out.push(InputChoice {
            id: Some(e.id.clone()),
            label: clean_label(&e.name),
            hint: selection_hint(&e.name, &others),
            is_default: default_id.is_some_and(|d| d.eq_ignore_ascii_case(&e.id)),
            bluetooth: e.bus.is_bluetooth(),
        });
    }
    out
}

/// Find the endpoint for a saved choice: by ID, then by name (IDs change when a USB device
/// moves ports), else `None` meaning "use the Windows default".
pub fn resolve_saved<'a>(
    endpoints: &'a [Endpoint],
    id: Option<&str>,
    name: Option<&str>,
) -> Option<&'a Endpoint> {
    let usable = |e: &&Endpoint| e.flow == Flow::Capture && e.is_active() && !e.is_virtual();
    if let Some(id) = id {
        if let Some(e) = endpoints
            .iter()
            .filter(usable)
            .find(|e| e.id.eq_ignore_ascii_case(id))
        {
            return Some(e);
        }
    }
    let name = name?.trim();
    endpoints
        .iter()
        .filter(usable)
        .find(|e| e.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vidar_win::{Bus, VB_CABLE_INTERFACE};

    fn ep(id: &str, flow: Flow, desc: &str, iface: &str, bus: Bus) -> Endpoint {
        Endpoint {
            id: id.into(),
            flow,
            name: format!("{desc} ({iface})"),
            description: desc.into(),
            interface_name: iface.into(),
            bus,
            state: 1,
        }
    }

    #[test]
    fn hints_for_line_in_and_stereo_mix() {
        assert!(
            selection_hint("Line In (Realtek)", &["Microphone (Realtek)"])
                .unwrap()
                .contains("'Microphone' entry")
        );
        assert!(selection_hint("Line In (Realtek)", &[])
            .unwrap()
            .contains("pink Mic jack"));
        assert!(selection_hint("Stereo Mix (Realtek)", &[])
            .unwrap()
            .contains("not your voice"));
        assert_eq!(selection_hint("Microphone (USB)", &[]), None);
        assert_eq!(
            selection_hint("Mic/Line In", &[]),
            None,
            "ambiguous names get no hint"
        );
    }

    #[test]
    fn choices_skip_virtual_devices_and_mark_default() {
        let eps = vec![
            ep(
                "{0.0.1.0}.{a}",
                Flow::Capture,
                "Microphone",
                "USB Audio",
                Bus::Usb,
            ),
            ep(
                "{0.0.1.0}.{c}",
                Flow::Capture,
                "Vidar AI Microphone",
                VB_CABLE_INTERFACE,
                Bus::Root,
            ),
            ep(
                "{0.0.0.0}.{s}",
                Flow::Render,
                "Speakers",
                "USB Audio",
                Bus::Usb,
            ),
        ];
        let c = input_choices(&eps, Some("{0.0.1.0}.{a}"));
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].id, None);
        assert!(c[1].is_default);
        assert_eq!(c[1].label, "Microphone (USB Audio)");
    }

    #[test]
    fn saved_mic_found_by_name_when_id_changed() {
        let eps = vec![ep(
            "{0.0.1.0}.{new}",
            Flow::Capture,
            "Headset",
            "HyperX",
            Bus::Usb,
        )];
        let found = resolve_saved(&eps, Some("{0.0.1.0}.{old}"), Some("Headset (HyperX)")).unwrap();
        assert_eq!(found.id, "{0.0.1.0}.{new}");
        assert!(resolve_saved(&eps, Some("{x}"), None).is_none());
    }

    #[test]
    fn long_labels_are_shortened() {
        let l = clean_label(&"x".repeat(80));
        assert_eq!(l.chars().count(), 52);
        assert!(l.ends_with("..."));
    }
}
