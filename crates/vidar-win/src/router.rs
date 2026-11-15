//! Switching the Windows default microphone to the cable and back. Playback defaults are
//! never touched.

use crate::model::{is_valid_endpoint_id, Endpoint, Flow};

/// The OS operations the router needs; implemented for real by `WinAudio`, and by fakes in tests.
pub trait AudioSystem {
    fn endpoints(&self, flow: Flow) -> Vec<Endpoint>;
    fn default_endpoint(&self, flow: Flow) -> Option<Endpoint>;
    fn endpoint(&self, id: &str) -> Option<Endpoint>;
    /// Make `id` the default for all three roles. True if at least one role succeeded.
    fn set_default(&self, id: &str) -> bool;
}

/// Routing state that must survive a crash (stored in the config file).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteState {
    pub is_swapped: bool,
    pub original_mic_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    CableMissing,
    SwitchFailed,
    /// Windows accepted the switch but reports a different default afterwards.
    NotConfirmed(String),
    NoPhysicalMic,
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteError::CableMissing => {
                write!(f, "the virtual cable's recording side was not found")
            }
            RouteError::SwitchFailed => {
                write!(f, "Windows refused to change the default microphone")
            }
            RouteError::NotConfirmed(now) => write!(f, "the default microphone is still '{now}'"),
            RouteError::NoPhysicalMic => write!(f, "no physical microphone to switch back to"),
        }
    }
}

/// The cable's recording endpoint: the remembered ID if still active, else found by type.
pub fn cable_capture<S: AudioSystem + ?Sized>(
    sys: &S,
    remembered: Option<&str>,
) -> Option<Endpoint> {
    if let Some(ep) = remembered
        .and_then(|id| sys.endpoint(id))
        .filter(|e| e.is_active() && e.flow == Flow::Capture && e.is_vb_cable())
    {
        return Some(ep);
    }
    sys.endpoints(Flow::Capture)
        .into_iter()
        .find(|e| e.is_active() && e.is_vb_cable())
}

/// The cable's playback endpoint (where the engine writes clean audio).
pub fn cable_render<S: AudioSystem + ?Sized>(
    sys: &S,
    remembered: Option<&str>,
) -> Option<Endpoint> {
    if let Some(ep) = remembered
        .and_then(|id| sys.endpoint(id))
        .filter(|e| e.is_active() && e.flow == Flow::Render && e.is_vb_cable())
    {
        return Some(ep);
    }
    sys.endpoints(Flow::Render)
        .into_iter()
        .find(|e| e.is_active() && e.is_vb_cable())
}

/// Best physical mic: external headsets and USB mics first, then anything not virtual.
pub fn best_physical_mic<S: AudioSystem + ?Sized>(sys: &S) -> Option<Endpoint> {
    best_physical(sys, Flow::Capture)
}

/// Best physical device of either direction (external first, then anything not virtual).
pub fn best_physical<S: AudioSystem + ?Sized>(sys: &S, flow: Flow) -> Option<Endpoint> {
    let devs: Vec<Endpoint> = sys
        .endpoints(flow)
        .into_iter()
        .filter(|e| e.is_active() && !e.is_virtual())
        .collect();
    devs.iter()
        .find(|e| e.looks_external())
        .or_else(|| devs.first())
        .cloned()
}

/// If the default device of `flow` is virtual (for example right after the cable driver
/// installed itself as default), switch it to the best physical one. Returns the endpoint
/// switched to, or `None` when nothing needed changing.
pub fn ensure_physical_default<S: AudioSystem + ?Sized>(
    sys: &S,
    flow: Flow,
) -> Result<Option<Endpoint>, RouteError> {
    if sys.default_endpoint(flow).is_some_and(|d| !d.is_virtual()) {
        return Ok(None);
    }
    let target = best_physical(sys, flow).ok_or(RouteError::NoPhysicalMic)?;
    if sys.set_default(&target.id) {
        Ok(Some(target))
    } else {
        Err(RouteError::SwitchFailed)
    }
}

/// Make `id` the default again if it still exists and is active (installer backup/restore).
pub fn restore_default<S: AudioSystem + ?Sized>(sys: &S, id: &str) -> bool {
    is_valid_endpoint_id(id)
        && sys.endpoint(id).is_some_and(|e| e.is_active())
        && sys.set_default(id)
}

/// Switch the default mic to the cable. `persist` is called with the new state *before*
/// Windows is changed, so a crash in between still leads to a restore on next start.
pub fn swap_to_cable<S: AudioSystem + ?Sized>(
    sys: &S,
    state: &mut RouteState,
    cable_capture_id: Option<&str>,
    persist: &mut dyn FnMut(&RouteState),
) -> Result<Endpoint, RouteError> {
    let cable = cable_capture(sys, cable_capture_id).ok_or(RouteError::CableMissing)?;

    let current = sys.default_endpoint(Flow::Capture);
    if state.is_swapped && current.as_ref().is_some_and(|c| c.id == cable.id) {
        return Ok(cable);
    }

    // Never remember the cable (or another virtual device) as the user's own mic.
    match current.filter(|c| !c.is_virtual()) {
        Some(c) => state.original_mic_id = Some(c.id),
        None => {
            if state.original_mic_id.is_none() {
                state.original_mic_id = best_physical_mic(sys).map(|e| e.id);
            }
        }
    }

    state.is_swapped = true;
    persist(state);

    if !sys.set_default(&cable.id) {
        state.is_swapped = false;
        persist(state);
        return Err(RouteError::SwitchFailed);
    }

    match sys.default_endpoint(Flow::Capture) {
        Some(now) if now.id.eq_ignore_ascii_case(&cable.id) => Ok(cable),
        other => {
            state.is_swapped = false;
            persist(state);
            Err(RouteError::NotConfirmed(
                other.map(|e| e.name).unwrap_or_else(|| "none".into()),
            ))
        }
    }
}

/// Put the user's mic back (on stop, exit, crash recovery). Falls back to the best physical
/// mic when the original is gone (unplugged headset).
pub fn restore_original<S: AudioSystem + ?Sized>(
    sys: &S,
    state: &mut RouteState,
    persist: &mut dyn FnMut(&RouteState),
) -> Result<Option<Endpoint>, RouteError> {
    if !state.is_swapped {
        return Ok(None);
    }

    let original = state
        .original_mic_id
        .as_deref()
        .filter(|id| is_valid_endpoint_id(id))
        .and_then(|id| sys.endpoint(id))
        .filter(|e| e.is_active() && e.flow == Flow::Capture && !e.is_virtual());

    let mut candidates: Vec<Endpoint> = original.into_iter().collect();
    if let Some(best) = best_physical_mic(sys) {
        if !candidates.iter().any(|c| c.id == best.id) {
            candidates.push(best);
        }
    }
    if candidates.is_empty() {
        // Nothing physical to return to: clear the flag so we do not retry forever.
        state.is_swapped = false;
        persist(state);
        return Err(RouteError::NoPhysicalMic);
    }

    for target in candidates {
        if sys.set_default(&target.id) {
            state.is_swapped = false;
            persist(state);
            return Ok(Some(target));
        }
    }
    Err(RouteError::SwitchFailed)
}

/// At startup: if the last run left the cable as default mic, restore the user's mic.
/// Returns the restored endpoint when a repair happened.
pub fn audit_crash_recovery<S: AudioSystem + ?Sized>(
    sys: &S,
    state: &mut RouteState,
    persist: &mut dyn FnMut(&RouteState),
) -> Result<Option<Endpoint>, RouteError> {
    if !state.is_swapped {
        return Ok(None);
    }
    let on_cable = sys
        .default_endpoint(Flow::Capture)
        .is_some_and(|e| e.is_virtual());
    if on_cable {
        return restore_original(sys, state, persist);
    }
    // The user (or Windows) already moved off the cable: just clear the flag.
    state.is_swapped = false;
    persist(state);
    Ok(None)
}

#[cfg(test)]
pub(crate) mod fake {
    use std::cell::RefCell;

    use super::*;

    pub struct FakeAudio {
        pub endpoints: Vec<Endpoint>,
        pub default_capture: RefCell<Option<String>>,
        pub default_render: RefCell<Option<String>>,
        pub refuse: Vec<String>,
        /// Simulate Windows accepting the call but not changing the default.
        pub ignore_switch: bool,
    }

    impl AudioSystem for FakeAudio {
        fn endpoints(&self, flow: Flow) -> Vec<Endpoint> {
            self.endpoints
                .iter()
                .filter(|e| e.flow == flow)
                .cloned()
                .collect()
        }
        fn default_endpoint(&self, flow: Flow) -> Option<Endpoint> {
            let id = match flow {
                Flow::Capture => self.default_capture.borrow().clone()?,
                Flow::Render => self.default_render.borrow().clone()?,
            };
            self.endpoint(&id)
        }
        fn endpoint(&self, id: &str) -> Option<Endpoint> {
            self.endpoints.iter().find(|e| e.id == id).cloned()
        }
        fn set_default(&self, id: &str) -> bool {
            if self.refuse.iter().any(|r| r == id) || self.endpoint(id).is_none() {
                return false;
            }
            if !self.ignore_switch {
                let flow = self.endpoint(id).map(|e| e.flow);
                let slot = if flow == Some(Flow::Render) {
                    &self.default_render
                } else {
                    &self.default_capture
                };
                *slot.borrow_mut() = Some(id.to_string());
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeAudio;
    use super::*;
    use crate::model::{ep, Bus, VB_CABLE_INTERFACE};

    const REALTEK: &str = "{0.0.1.00000000}.{11111111-0000-0000-0000-000000000001}";
    const HEADSET: &str = "{0.0.1.00000000}.{11111111-0000-0000-0000-000000000002}";
    const CABLE: &str = "{0.0.1.00000000}.{11111111-0000-0000-0000-00000000000c}";
    const CABLE_RENDER: &str = "{0.0.0.00000000}.{11111111-0000-0000-0000-00000000000d}";

    fn system(default: &str) -> FakeAudio {
        FakeAudio {
            endpoints: vec![
                ep(
                    REALTEK,
                    Flow::Capture,
                    "Microphone Array",
                    "Realtek(R) Audio",
                    Bus::HdAudio,
                ),
                ep(
                    HEADSET,
                    Flow::Capture,
                    "Headset Microphone",
                    "HyperX Cloud",
                    Bus::Usb,
                ),
                ep(
                    CABLE,
                    Flow::Capture,
                    "Vidar AI Microphone",
                    VB_CABLE_INTERFACE,
                    Bus::Root,
                ),
                ep(
                    CABLE_RENDER,
                    Flow::Render,
                    "Vidar Internal",
                    VB_CABLE_INTERFACE,
                    Bus::Root,
                ),
            ],
            default_capture: std::cell::RefCell::new(Some(default.into())),
            default_render: std::cell::RefCell::new(Some(CABLE_RENDER.into())),
            refuse: vec![],
            ignore_switch: false,
        }
    }

    fn recorder() -> (
        std::rc::Rc<std::cell::RefCell<Vec<RouteState>>>,
        impl FnMut(&RouteState),
    ) {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let l = log.clone();
        (log, move |s: &RouteState| l.borrow_mut().push(s.clone()))
    }

    #[test]
    fn swap_remembers_user_mic_and_persists_first() {
        let sys = system(REALTEK);
        let mut st = RouteState::default();
        let (log, mut persist) = recorder();
        let cable = swap_to_cable(&sys, &mut st, None, &mut persist).unwrap();
        assert_eq!(cable.id, CABLE);
        assert_eq!(
            st,
            RouteState {
                is_swapped: true,
                original_mic_id: Some(REALTEK.into())
            }
        );
        assert!(log.borrow()[0].is_swapped, "state saved before switching");
        assert_eq!(sys.default_capture.borrow().as_deref(), Some(CABLE));
    }

    #[test]
    fn swap_never_records_the_cable_as_original() {
        let sys = system(CABLE);
        let mut st = RouteState::default();
        swap_to_cable(&sys, &mut st, None, &mut |_| {}).unwrap();
        assert_eq!(
            st.original_mic_id.as_deref(),
            Some(HEADSET),
            "falls back to the best physical mic"
        );
    }

    #[test]
    fn swap_reports_missing_cable() {
        let mut sys = system(REALTEK);
        sys.endpoints.retain(|e| !e.is_vb_cable());
        let mut st = RouteState::default();
        assert_eq!(
            swap_to_cable(&sys, &mut st, None, &mut |_| {}),
            Err(RouteError::CableMissing)
        );
        assert!(!st.is_swapped);
    }

    #[test]
    fn failed_or_unconfirmed_switch_rolls_back_flag() {
        let mut sys = system(REALTEK);
        sys.refuse = vec![CABLE.into()];
        let mut st = RouteState::default();
        assert_eq!(
            swap_to_cable(&sys, &mut st, None, &mut |_| {}),
            Err(RouteError::SwitchFailed)
        );
        assert!(!st.is_swapped);

        let mut sys = system(REALTEK);
        sys.ignore_switch = true;
        let mut st = RouteState::default();
        assert!(matches!(
            swap_to_cable(&sys, &mut st, None, &mut |_| {}),
            Err(RouteError::NotConfirmed(_))
        ));
        assert!(!st.is_swapped);
    }

    #[test]
    fn restore_returns_to_original() {
        let sys = system(CABLE);
        let mut st = RouteState {
            is_swapped: true,
            original_mic_id: Some(REALTEK.into()),
        };
        let back = restore_original(&sys, &mut st, &mut |_| {})
            .unwrap()
            .unwrap();
        assert_eq!(back.id, REALTEK);
        assert!(!st.is_swapped);
    }

    #[test]
    fn restore_falls_back_when_original_unplugged() {
        let sys = system(CABLE);
        let mut st = RouteState {
            is_swapped: true,
            original_mic_id: Some("{0.0.1.00000000}.{gone}".into()),
        };
        let back = restore_original(&sys, &mut st, &mut |_| {})
            .unwrap()
            .unwrap();
        assert_eq!(back.id, HEADSET);
    }

    #[test]
    fn restore_rejects_malformed_saved_id() {
        let sys = system(CABLE);
        let mut st = RouteState {
            is_swapped: true,
            original_mic_id: Some("CABLE Output".into()),
        };
        let back = restore_original(&sys, &mut st, &mut |_| {})
            .unwrap()
            .unwrap();
        assert_eq!(back.id, HEADSET);
    }

    #[test]
    fn restore_is_noop_when_not_swapped() {
        let sys = system(CABLE);
        let mut st = RouteState::default();
        assert_eq!(restore_original(&sys, &mut st, &mut |_| {}), Ok(None));
        assert_eq!(sys.default_capture.borrow().as_deref(), Some(CABLE));
    }

    #[test]
    fn crash_audit_repairs_only_when_still_on_cable() {
        let sys = system(CABLE);
        let mut st = RouteState {
            is_swapped: true,
            original_mic_id: Some(REALTEK.into()),
        };
        assert_eq!(
            audit_crash_recovery(&sys, &mut st, &mut |_| {})
                .unwrap()
                .unwrap()
                .id,
            REALTEK
        );

        let sys = system(HEADSET);
        let mut st = RouteState {
            is_swapped: true,
            original_mic_id: Some(REALTEK.into()),
        };
        assert_eq!(audit_crash_recovery(&sys, &mut st, &mut |_| {}), Ok(None));
        assert!(!st.is_swapped);
        assert_eq!(
            sys.default_capture.borrow().as_deref(),
            Some(HEADSET),
            "user's own choice is kept"
        );
    }

    #[test]
    fn ensure_physical_moves_off_the_cable_for_both_directions() {
        const SPEAKERS: &str = "{0.0.0.00000000}.{11111111-0000-0000-0000-000000000005}";
        let mut sys = system(CABLE);
        sys.endpoints.push(ep(
            SPEAKERS,
            Flow::Render,
            "Speakers",
            "Realtek(R) Audio",
            Bus::HdAudio,
        ));
        assert_eq!(
            ensure_physical_default(&sys, Flow::Capture)
                .unwrap()
                .unwrap()
                .id,
            HEADSET
        );
        assert_eq!(
            ensure_physical_default(&sys, Flow::Render)
                .unwrap()
                .unwrap()
                .id,
            SPEAKERS
        );
        assert_eq!(
            ensure_physical_default(&sys, Flow::Render),
            Ok(None),
            "already physical"
        );
        assert!(restore_default(&sys, REALTEK));
        assert!(!restore_default(&sys, "{0.0.1.00000000}.{gone}"));
    }

    #[test]
    fn remembered_cable_id_wins() {
        let sys = system(REALTEK);
        assert_eq!(cable_capture(&sys, Some(CABLE)).unwrap().id, CABLE);
        assert_eq!(cable_render(&sys, None).unwrap().id, CABLE_RENDER);
        assert_eq!(
            cable_capture(&sys, Some(REALTEK)).unwrap().id,
            CABLE,
            "a remembered ID that is not the cable is ignored"
        );
    }
}
