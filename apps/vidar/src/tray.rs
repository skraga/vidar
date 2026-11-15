//! Tray icon with its own Win32 message loop (it must keep working while the window is
//! closed), plus a hidden window that catches Windows shutdown / logoff so the default
//! microphone is restored even then.

use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tray_icon::menu::{CheckMenuItem, IconMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW,
    RegisterClassW, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT, WINDOW_EX_STYLE, WM_ENDSESSION,
    WM_QUERYENDSESSION, WM_QUIT, WNDCLASSW, WS_OVERLAPPED,
};

use crate::worker::{Cmd, Handle, RunState};
use crate::{Page, UiCmd};

/// Called on Windows shutdown/logoff: stops the engine and restores the mic, synchronously.
static ON_END_SESSION: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

pub fn set_end_session_hook(f: impl Fn() + Send + Sync + 'static) {
    let _ = ON_END_SESSION.set(Box::new(f));
}

unsafe extern "system" fn session_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION => {
            if wp.0 != 0 {
                if let Some(f) = ON_END_SESSION.get() {
                    f();
                }
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn create_session_window() {
    // SAFETY: registers a class and creates one hidden top-level window on this thread
    // (message-only windows do not receive WM_ENDSESSION).
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return;
        };
        let class = WNDCLASSW {
            lpfnWndProc: Some(session_wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("VidarSessionWatcher"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let _ = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("VidarSessionWatcher"),
            w!("Vidar"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        );
    }
}

fn icon(active: bool) -> Option<Icon> {
    let size = 32;
    let rgba = vidar_brand::render(size, active);
    Icon::from_rgba(rgba, size, size).ok()
}

fn menu_icon(active: bool) -> Option<tray_icon::menu::Icon> {
    let size = 16;
    let rgba = vidar_brand::render(size, active);
    tray_icon::menu::Icon::from_rgba(rgba, size, size).ok()
}

/// What the menu last showed, to update only on change.
#[derive(PartialEq, Clone)]
struct Shown {
    state: RunState,
    muted: bool,
}

/// Run the tray on the current thread until Exit is chosen. The menu holds only the quick
/// actions; everything else is in the window (left-click the icon).
pub fn run(handle: Handle, ui: Sender<UiCmd>) {
    create_session_window();

    let header = IconMenuItem::new("Vidar: off", false, menu_icon(false), None);
    let toggle = MenuItem::new("Turn noise cancellation on", true, None);
    let mute = CheckMenuItem::new("Mute microphone", true, false, None);

    let open = MenuItem::new("Open Vidar", true, None);
    let exit = MenuItem::new("Exit", true, None);

    let menu = Menu::new();
    for item in [
        &header as &dyn tray_icon::menu::IsMenuItem,
        &PredefinedMenuItem::separator(),
        &toggle,
        &mute,
        &PredefinedMenuItem::separator(),
        &open,
        &exit,
    ] {
        let _ = menu.append(item);
    }

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Vidar: off");
    if let Some(i) = icon(false) {
        builder = builder.with_icon(i);
    }
    let tray = match builder.build() {
        Ok(t) => t,
        Err(e) => {
            log::warn!("tray icon failed: {e}");
            return;
        }
    };

    let mut shown: Option<Shown> = None;
    let mut last_check = Instant::now() - Duration::from_secs(1);
    loop {
        // SAFETY: standard message pump for this thread's windows (tray + session watcher).
        unsafe {
            MsgWaitForMultipleObjects(None, false, 150, QS_ALLINPUT);
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                let _ = ui.send(UiCmd::Show(None));
            }
        }
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            let id = ev.id;
            if id == *toggle.id() {
                handle.send(Cmd::Toggle);
            } else if id == *mute.id() {
                handle.send(Cmd::SetMute(mute.is_checked()));
            } else if id == *open.id() {
                let _ = ui.send(UiCmd::Show(Some(Page::Home)));
            } else if id == *exit.id() {
                let _ = ui.send(UiCmd::Quit);
                return;
            }
        }

        if last_check.elapsed() < Duration::from_millis(150) {
            continue;
        }
        last_check = Instant::now();
        let v = handle.view();
        let now = Shown {
            state: v.state,
            muted: v.muted,
        };
        if shown.as_ref() == Some(&now) {
            continue;
        }
        let active = now.state == RunState::Running;
        let (head, tip) = match (now.state, now.muted) {
            (RunState::Running, true) => {
                ("Vidar: on, microphone muted", "Vidar: on, microphone muted")
            }
            (RunState::Running, false) => ("Vidar: on", "Vidar: on"),
            (RunState::Starting, _) => ("Vidar: starting...", "Vidar: starting..."),
            (RunState::Stopping, _) => ("Vidar: stopping...", "Vidar: stopping..."),
            (RunState::Stopped, _) => ("Vidar: off", "Vidar: off"),
        };
        let _ = tray.set_icon(icon(active));
        let _ = tray.set_tooltip(Some(tip));
        header.set_text(head);
        header.set_icon(menu_icon(active));
        toggle.set_text(if now.state == RunState::Stopped {
            "Turn noise cancellation on"
        } else {
            "Turn noise cancellation off"
        });
        toggle.set_enabled(!matches!(
            now.state,
            RunState::Starting | RunState::Stopping
        ));
        mute.set_checked(now.muted);
        mute.set_enabled(active);
        shown = Some(now);
    }
}
