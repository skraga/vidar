//! The window (egui): Home / Test / Settings. It only exists while open; closing it
//! destroys the GPU context while the engine and tray keep running.
//!
//! Light and dark palettes; "auto" follows the Windows app theme.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Rect, RichText, Sense, Stroke, StrokeKind,
};

use crate::worker::{
    Cmd, Handle, InstallState, RunState, TestPhase, Tone, Track, VariantView, View, TEST_OPTIONS,
};
use crate::UiCmd;

// ---------------------------------------------------------------- palette

#[derive(Clone, Copy)]
struct Pal {
    bg: Color32,
    surface: Color32,
    surface2: Color32,
    border: Color32,
    text: Color32,
    muted: Color32,
    accent: Color32,
    on_accent: Color32,
    blue: Color32,
    warn: Color32,
    danger: Color32,
    track_idle: Color32,
}

const DARK: Pal = Pal {
    bg: Color32::from_rgb(14, 17, 22),
    surface: Color32::from_rgb(22, 27, 34),
    surface2: Color32::from_rgb(31, 38, 48),
    border: Color32::from_rgb(42, 51, 64),
    text: Color32::from_rgb(230, 237, 243),
    muted: Color32::from_rgb(139, 152, 168),
    accent: Color32::from_rgb(45, 215, 115),
    on_accent: Color32::from_rgb(6, 24, 14),
    blue: Color32::from_rgb(88, 156, 255),
    warn: Color32::from_rgb(240, 180, 50),
    danger: Color32::from_rgb(239, 83, 80),
    track_idle: Color32::from_rgb(120, 134, 152),
};

const LIGHT: Pal = Pal {
    bg: Color32::from_rgb(237, 240, 244),
    surface: Color32::from_rgb(255, 255, 255),
    surface2: Color32::from_rgb(229, 233, 239),
    border: Color32::from_rgb(206, 212, 221),
    text: Color32::from_rgb(17, 24, 39),
    muted: Color32::from_rgb(91, 102, 117),
    accent: Color32::from_rgb(18, 161, 80),
    on_accent: Color32::from_rgb(255, 255, 255),
    blue: Color32::from_rgb(37, 99, 235),
    warn: Color32::from_rgb(183, 121, 31),
    danger: Color32::from_rgb(220, 38, 38),
    track_idle: Color32::from_rgb(148, 160, 176),
};

fn pal(ui: &egui::Ui) -> Pal {
    if ui.visuals().dark_mode {
        DARK
    } else {
        LIGHT
    }
}

fn tone_color(p: &Pal, t: Tone) -> Color32 {
    match t {
        Tone::Idle => p.muted,
        Tone::Busy => p.blue,
        Tone::Good => p.accent,
        Tone::Warn => p.warn,
        Tone::Bad => p.danger,
    }
}

fn visuals(p: &Pal, dark: bool) -> egui::Visuals {
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.extreme_bg_color = p.surface2;
    v.faint_bg_color = p.surface2;
    v.override_text_color = Some(p.text);
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0, p.on_accent);
    v.hyperlink_color = p.blue;
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.widgets.inactive.weak_bg_fill = p.surface2;
    v.widgets.inactive.bg_fill = p.surface2;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.hovered.weak_bg_fill = p.border;
    v.widgets.hovered.bg_fill = p.border;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(12);
    v
}

pub fn setup_style(ctx: &egui::Context, theme: &str) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../../assets/fonts/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "inter-semi".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../../assets/fonts/Inter-SemiBold.ttf"
        ))),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".into());
    fonts.families.insert(
        FontFamily::Name("semi".into()),
        vec!["inter-semi".into(), "inter".into()],
    );
    ctx.set_fonts(fonts);
    ctx.set_visuals_of(egui::Theme::Dark, visuals(&DARK, true));
    ctx.set_visuals_of(egui::Theme::Light, visuals(&LIGHT, false));
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.button_padding = vec2(12.0, 6.0);
        s.text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.0));
        s.text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(14.0));
        s.text_styles
            .insert(egui::TextStyle::Small, FontId::proportional(12.0));
    });
    apply_theme(ctx, theme);
}

fn apply_theme(ctx: &egui::Context, theme: &str) {
    ctx.set_theme(match theme {
        "light" => egui::ThemePreference::Light,
        "dark" => egui::ThemePreference::Dark,
        _ => egui::ThemePreference::System,
    });
}

fn semi(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semi".into()))
}

// ---------------------------------------------------------------- widgets

/// A rounded panel.
fn card<R>(ui: &mut egui::Ui, p: &Pal, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(12)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Small caps section title.
fn section(ui: &mut egui::Ui, p: &Pal, title: &str) {
    ui.label(
        RichText::new(title.to_uppercase())
            .font(semi(11.0))
            .color(p.muted),
    );
}

/// Tooltip content: a bold title and a short explanation.
type Tip = (&'static str, &'static str);

fn tip_ui(ui: &mut egui::Ui, tip: Tip) {
    ui.set_max_width(260.0);
    ui.label(RichText::new(tip.0).font(semi(13.0)));
    ui.label(RichText::new(tip.1).size(12.5));
}

/// Section title with a "?" that explains the whole group on hover.
fn section_help(ui: &mut egui::Ui, p: &Pal, title: &str, help: Tip) {
    ui.horizontal(|ui| {
        section(ui, p, title);
        let (r, resp) = ui.allocate_exact_size(vec2(15.0, 15.0), Sense::hover());
        let hovered = resp.hovered();
        let c = if hovered { p.text } else { p.muted };
        ui.painter()
            .circle_stroke(r.center(), 6.5, Stroke::new(1.2, c));
        ui.painter().text(
            r.center() + vec2(0.0, 0.5),
            Align2::CENTER_CENTER,
            "?",
            semi(10.0),
            c,
        );
        resp.on_hover_ui(|ui| tip_ui(ui, help));
    });
}

/// iOS-style switch. Returns (flipped, response) so callers can attach a tooltip.
fn switch_resp(ui: &mut egui::Ui, p: &Pal, on: bool) -> (bool, egui::Response) {
    let (rect, resp) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, on, 0.12);
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        rect.height() / 2.0,
        if on { p.accent } else { p.border },
    );
    let r = rect.height() / 2.0 - 3.0;
    let x = egui::lerp(rect.left() + r + 3.0..=rect.right() - r - 3.0, t);
    painter.circle_filled(pos2(x, rect.center().y), r, Color32::WHITE);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    (resp.clicked(), resp)
}

/// A labelled row with a switch on the right.
fn switch_row(ui: &mut egui::Ui, p: &Pal, title: &str, hint: &str, on: bool, tip: Tip) -> bool {
    let mut flipped = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(ui.available_width() - 50.0);
            ui.label(RichText::new(title).color(p.text))
                .on_hover_ui(|ui| tip_ui(ui, tip));
            if !hint.is_empty() {
                ui.label(RichText::new(hint).small().color(p.muted));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (clicked, resp) = switch_resp(ui, p, on);
            resp.on_hover_ui(|ui| tip_ui(ui, tip));
            flipped = clicked;
        });
    });
    flipped
}

/// Segmented control. Returns the index clicked, if any. `tips` (one per option, or empty)
/// appear on hover, also when the control is disabled.
fn segmented(
    ui: &mut egui::Ui,
    p: &Pal,
    options: &[&str],
    selected: Option<usize>,
    enabled: bool,
    tips: &[Tip],
) -> Option<usize> {
    let h = 32.0;
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 9.0, p.surface2);
    let seg = (w - 6.0) / options.len() as f32;
    let mut clicked = None;
    for (i, label) in options.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(rect.left() + 3.0 + seg * i as f32, rect.top() + 3.0),
            vec2(seg, h - 6.0),
        );
        let resp = ui.interact(
            r,
            ui.scope_id().with(("seg", options, i)),
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let active = selected == Some(i);
        if active {
            painter.rect_filled(r, 7.0, p.surface);
            painter.rect_stroke(r, 7.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
        } else if resp.hovered() && enabled {
            painter.rect_filled(r, 7.0, p.border.gamma_multiply(0.5));
        }
        let color = if !enabled {
            p.muted.gamma_multiply(0.6)
        } else if active {
            p.text
        } else {
            p.muted
        };
        painter.text(
            r.center(),
            Align2::CENTER_CENTER,
            *label,
            if active {
                semi(13.0)
            } else {
                FontId::proportional(13.0)
            },
            color,
        );
        if resp.clicked() && !active {
            clicked = Some(i);
        }
        if resp.hovered() && enabled {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if let Some(&tip) = tips.get(i) {
            resp.on_hover_ui(|ui| tip_ui(ui, tip));
        }
    }
    clicked
}

/// Primary button with a tooltip.
fn primary_tip(ui: &mut egui::Ui, p: &Pal, text: &str, width: f32, tip: Tip) -> bool {
    let b = egui::Button::new(RichText::new(text).font(semi(14.0)).color(p.on_accent))
        .fill(p.accent)
        .corner_radius(9)
        .min_size(vec2(width, 36.0));
    ui.add(b).on_hover_ui(|ui| tip_ui(ui, tip)).clicked()
}

// ---------------------------------------------------------------- explanations

// Writing rules for every tooltip and hint:
// - The tooltip title is the control's name exactly as shown.
// - The body says what it does, then when to change it. Plain words: no "gain",
//   "buffer", "high-pass" or "threshold".
// - The one-line hint under a control has no full stop.
// - Names: "noise cancellation" is the on/off feature, "mode" is Voice / Voice HD,
//   "strength" is Gentle / Balanced / Strong, "Vidar AI Microphone" is what apps pick.
// - Numbers match the Test tab: Voice about 30 ms, Voice HD about 60 ms.

const TIP_MIC: Tip = ("Microphone", "The microphone Vidar listens to. \"Windows default microphone\" follows whatever is selected in Windows Sound settings. Bluetooth headsets sound narrower while their microphone is in use.");
const TIP_AUTO_LEVEL: Tip = ("Auto level", "Slowly makes quiet speech louder, by up to 9 dB. It never makes loud speech louder and holds still while you are silent, so background noise is not raised. Turn it on if people say you are too quiet.");
const TIP_FINE: Tip = ("Fine-tune strength", "The same setting as Strength on the Home tab, in finer steps. Further right, Vidar must be more sure you are talking before it turns your microphone back up between words: less noise slips through, but very soft words may be cut. Further left keeps every word but lets more noise through in pauses.");
const TIP_LEVEL: Tip = ("How loud apps hear you", "Your voice after Vidar, as apps receive it. Talk normally for a few seconds. Aim for the green zone: in the grey zone people may struggle to hear you, in the red zone you may sound harsh or distorted. The white mark is where your speech usually sits.");
const TIP_RESET: Tip = (
    "Reset to recommended",
    "Boost off, Auto level off, Rumble filter on, strength Balanced.",
);
const TIP_AUTOSTART: Tip = (
    "Start with Windows",
    "Starts Vidar in the tray when you sign in. Noise cancellation stays off until you turn it on.",
);
const TIP_RAW: Tip = ("Raw microphone signal", "Gives Vidar your microphone before Windows and the microphone's driver apply their own effects, such as built-in noise suppression or automatic volume. Usually sounds more natural with Vidar; turn it off if your microphone sounds worse. Audio restarts briefly if Vidar is on.");
const TIP_INSTALL: Tip = ("Virtual microphone", "Vidar AI Microphone runs on the VB-CABLE driver by VB-Audio (donationware). Installing it asks Windows for permission once; your speakers and microphone stay as they are.");
const TIP_REINSTALL: Tip = ("Reinstall", "Installs the VB-Audio driver again. Use it if Vidar AI Microphone stopped working, for example after a Windows update.");
const TIP_UNINSTALL: Tip = ("Uninstall", "Turns Vidar off, gives your own microphone back to Windows and removes the VB-Audio driver. Apps that were set to Vidar AI Microphone switch to another microphone. Windows may ask to restart.");
const TIP_LOGS: Tip = (
    "Open logs",
    "Opens the folder with Vidar's log files. Attach the newest one when reporting a problem.",
);
const TIP_RECORD: Tip = ("Start recording", "Records your microphone until you press Stop, up to 20 seconds. Talk normally with your usual background noise. The recording stays in memory and is never saved to disk.");
const TIP_WAVE: Tip = ("Waveform", "Blue: your original recording. Green: the version you are hearing. The less green between your words, the more noise was removed. Click to jump there, drag to loop a part, double-click to clear the loop.");

const STRENGTH_HELP: Tip = (
    "Strength",
    "In every strength the AI removes noise from under your voice. Between words Vidar also turns your microphone down; strength sets how sure it must be that you are talking before it turns it back up.",
);
const STRENGTH_TIPS: [Tip; 3] = [
    ("Gentle", "Turns back up easily, so every soft word and breath is kept. Some quiet background can slip through between words. For quiet rooms and soft voices."),
    ("Balanced (recommended)", "Keeps fans, keyboards and room noise out between words while word endings stay natural. Right for most people."),
    ("Strong", "Stays down unless it clearly hears you. For loud rooms, mechanical keyboards or people talking nearby. Very soft words may be cut."),
];
const MODE_HELP: Tip = (
    "Mode",
    "Which AI model removes the noise. You can switch while noise cancellation is on.",
);
const MODE_TIPS: [Tip; 2] = [
    ("Voice", "Light AI model (RNNoise). About 30 ms of delay and a few percent of one processor core. For games, voice chat and laptops."),
    ("Voice HD", "Larger AI model (DPDFNet): cleaner, more natural voice in loud or echoing rooms. About 60 ms of delay and about five times the processor work of Voice. For calls, meetings and recordings."),
];
const BOOST_HELP: Tip = ("Boost", "Makes your voice louder for quiet microphones, after the noise is removed. Lower it if people say you are too loud or distorted.");
const BOOST_TIPS: [Tip; 4] = [
    (
        "Off",
        "No extra loudness. Right for most USB and headset microphones.",
    ),
    (
        "+5 dB",
        "A little louder. For microphones that are slightly quiet.",
    ),
    (
        "+10 dB",
        "Clearly louder. For laptop microphones and Bluetooth headsets.",
    ),
    (
        "+15 dB",
        "Much louder. For very quiet microphones; loud speech may distort.",
    ),
];
const TONE_HELP: Tip = ("Voice tone", "Shapes how your voice sounds to others, after the noise is removed. Pick a tone, or drag the points on the curve to make your own. Double-click a point to reset it.");
const TONE_PRESETS: [(&str, &str); 5] = [
    ("off", "Off"),
    ("warm", "Warm"),
    ("clear", "Clear"),
    ("broadcast", "Broadcast"),
    ("custom", "Custom"),
];
const TONE_TIPS: [Tip; 5] = [
    (
        "Off",
        "Your voice exactly as the microphone and the noise model deliver it.",
    ),
    (
        "Warm",
        "Fuller and softer: more body, less edge and hiss. Good for thin headset mics.",
    ),
    (
        "Clear",
        "Easier to understand: less boom and mud, more presence. Good for boomy or muffled mics.",
    ),
    (
        "Broadcast",
        "Radio-style: a little weight, less mud, a forward voice.",
    ),
    ("Custom", "Your own curve. Drag the points to change it."),
];
/// What each band does, for the point tooltips.
const TONE_BANDS: [(&str, &str); vidar_dsp::EQ_BANDS] = [
    (
        "Body",
        "100 Hz and below. Up: fuller, deeper. Down: less boom and rumble.",
    ),
    (
        "Mud",
        "Around 300 Hz. Down: cleaner, less boxy. Up: thicker.",
    ),
    (
        "Nasal",
        "Around 1 kHz. Down: less honky or nasal. Up: more cutting.",
    ),
    (
        "Presence",
        "Around 3 kHz. Up: clearer, more forward. Down: softer, less harsh.",
    ),
    (
        "Air",
        "8 kHz and above. Up: brighter, crisper. Down: less hiss and sharp S sounds.",
    ),
];

const THEME_TIPS: [Tip; 3] = [
    (
        "Auto",
        "Follows Windows (Settings > Personalization > Colors).",
    ),
    ("Light", "Always light."),
    ("Dark", "Always dark."),
];
const COMPARE_HELP: Tip = (
    "Compare",
    "Every tile is your recording with a different mode (row) and strength (column). Each tile shows how much quieter your pauses got and whether your voice was kept; ★ marks the best one for you and a dot the one in use. Delay and processor use depend only on the mode, so they appear next to its name.",
);
const METRIC_TIPS: [Tip; 4] = [
    ("Noise removed", "How much quieter your pauses became. Every 10 dB sounds about half as loud; 20 dB or more is very clean."),
    ("Voice", "How much your speech level changed. \"Voice intact\" means it stayed within 3 dB; a lower number means Vidar is also taking away some of your voice."),
    ("Delay", "Time from your mouth to the app. Under about 50 ms goes unnoticed in calls and games; Voice HD's 60 ms is fine for calls but may feel late if you hear yourself back."),
    ("Processor use", "How busy this mode keeps one processor core while running, measured on this PC from your recording. The AI model runs on a single core, so this shows how much room is left. Strength does not change it."),
];

fn option_name(o: &crate::worker::Option_) -> String {
    format!("{} - {}", o.mode, o.strength)
}

fn is_current(o: &crate::worker::Option_, v: &View) -> bool {
    o.engine == v.engine && (o.threshold - v.vad_threshold).abs() < 0.005
}

/// The option that removes the most noise without noticeably lowering the voice.
fn best_option(variants: &[VariantView]) -> Option<usize> {
    variants
        .iter()
        .enumerate()
        .filter(|(_, x)| x.unavailable.is_none())
        .map(|(i, x)| {
            // Each dB of voice loss beyond 1 dB costs as much as 4 dB of noise removal.
            let penalty = 4.0 * (-x.voice_change_db - 1.0).max(0.0);
            (i, x.noise_removed_db - penalty)
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// CPU share of one core as people read it: "~13% of 1 core", "<1% of 1 core".
fn cpu_text(pct: f64) -> String {
    if pct < 1.0 {
        "<1% of 1 core".into()
    } else {
        format!("~{:.0}% of 1 core", pct.round())
    }
}

/// Logical processors (what Task Manager divides by).
fn cpu_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// A small pill sized to its text. Drawn by hand: a `Frame` stretches to the full width in a
/// centred layout.
fn badge(ui: &mut egui::Ui, bg: Color32, fg: Color32, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), semi(10.5), fg);
    let pad = vec2(6.0, 1.0);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + 2.0 * pad, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 6, bg);
        ui.painter().galley(rect.min + pad, galley, fg);
    }
    resp
}

/// The big round power button: the Vidar logo inside a circle of short bars. Off, the bars
/// are small grey ticks; on, they take the logo's colours and each one rises and falls with
/// your voice on its own. Returns true when clicked.
fn power_button(
    ui: &mut egui::Ui,
    p: &Pal,
    state: RunState,
    voice: f32,
    bands: &[f32; vidar_dsp::BANDS],
    ring: &[f32; RING_BARS],
    motion: RingMotion,
) -> bool {
    let d = 168.0;
    // Extra height so the voice ring and its pulses have room to grow outward.
    let (rect, area) = ui.allocate_exact_size(vec2(ui.available_width(), d + 86.0), Sense::hover());
    let c = rect.center();
    let resp = ui.interact(
        Rect::from_center_size(c, vec2(d, d)),
        area.id.with("power"),
        Sense::click(),
    );
    let active = state == RunState::Running;
    let busy = matches!(state, RunState::Starting | RunState::Stopping);
    let t = ui.ctx().animate_bool_with_time(resp.id, active, 0.4);
    let time = ui.input(|i| i.time) as f32;
    let painter = ui.painter();

    // The circle of bars.
    let dark = ui.visuals().dark_mode;
    draw_voice_ring(
        painter,
        p,
        c,
        d / 2.0 - 5.0,
        t,
        voice,
        ring,
        time,
        motion,
        dark,
    );
    if busy {
        let pts: Vec<_> = (0..=24)
            .map(|k| {
                let a = time * 4.0 + k as f32 / 24.0 * std::f32::consts::PI * 0.9;
                c + vec2(a.cos(), a.sin()) * (d / 2.0 - 4.0)
            })
            .collect();
        painter.add(egui::Shape::line(pts, Stroke::new(6.0, p.blue)));
        ui.ctx().request_repaint();
    }
    painter.circle_filled(
        c,
        d / 2.0 - 10.0,
        if resp.hovered() && !busy {
            p.surface2
        } else {
            p.surface
        },
    );
    painter.circle_stroke(c, d / 2.0 - 10.0, Stroke::new(1.0, p.border));

    // The Vidar logo in place of a power glyph.
    draw_logo(
        painter,
        p,
        c - vec2(0.0, 8.0),
        48.0,
        t,
        bands,
        busy.then_some(time),
        time,
    );
    let icon = if active { p.accent } else { p.muted };
    let label = match state {
        RunState::Running => "ON",
        RunState::Stopped => "OFF",
        RunState::Starting => "STARTING",
        RunState::Stopping => "STOPPING",
    };
    painter.text(
        c + vec2(0.0, 40.0),
        Align2::CENTER_CENTER,
        label,
        semi(12.0),
        icon,
    );
    if resp.hovered() && !busy {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let clicked = resp.clicked();
    resp.on_hover_ui(|ui| {
        tip_ui(
            ui,
            match state {
                RunState::Running => ("Turn noise cancellation off", "Stops Vidar and makes your own microphone the Windows default again. While Vidar is on, the logo and the ring show your cleaned voice live: low tones in the middle of the logo and at the top of the ring, high ones at the edges and the bottom."),
                RunState::Stopped => ("Turn noise cancellation on", "Vidar listens to your microphone, removes the noise and sends your clean voice to Vidar AI Microphone, which apps then use."),
                _ => ("Please wait", "Vidar is switching your microphone."),
            },
        )
    });
    clicked && state != RunState::Stopping
}

/// Radio dot for a selectable version; little moving bars while it is the one playing.
fn play_indicator(
    painter: &egui::Painter,
    ui: &egui::Ui,
    color: Color32,
    c: egui::Pos2,
    selected: bool,
    playing: bool,
    idle: Color32,
) {
    if playing {
        let time = ui.input(|i| i.time) as f32;
        for k in 0..3 {
            let h = 4.0 + 7.0 * (0.5 + 0.5 * (time * 7.0 + k as f32 * 1.9).sin());
            let x = c.x - 5.0 + k as f32 * 5.0;
            painter.line_segment(
                [pos2(x, c.y + 5.5), pos2(x, c.y + 5.5 - h)],
                Stroke::new(3.0, color),
            );
        }
        ui.ctx().request_repaint();
    } else if selected {
        painter.circle_stroke(c, 6.0, Stroke::new(1.5, color));
        painter.circle_filled(c, 3.2, color);
    } else if idle != Color32::TRANSPARENT {
        painter.circle_stroke(c, 6.0, Stroke::new(1.5, idle));
    }
}

/// [`tip_ui`] for text built at run time.
fn tip_ui_owned(ui: &mut egui::Ui, title: &str, body: &str) {
    ui.set_max_width(260.0);
    ui.label(RichText::new(title).font(semi(13.0)));
    ui.label(RichText::new(body).size(12.5));
}

/// The voice tone graph: the equaliser's response from 50 Hz to 16 kHz (±12 dB), with one
/// point per band and the band names and gains underneath. Anywhere in a band's column drags
/// its point up and down (snapping to 0.5 dB); double-click resets it.
/// Returns the new gains while dragging (`false`) and once more when the drag ends (`true`).
fn tone_curve(
    ui: &mut egui::Ui,
    p: &Pal,
    gains: [f64; vidar_dsp::EQ_BANDS],
    active: bool,
) -> Option<([f64; vidar_dsp::EQ_BANDS], bool)> {
    const F_MIN: f64 = 50.0;
    const F_MAX: f64 = 16_000.0;
    const PLOT_H: f32 = 156.0;
    const LABELS_H: f32 = 36.0;
    const AXIS_W: f32 = 30.0;
    let max_db = vidar_dsp::EQ_MAX_DB;
    // The view reaches a little past the ±12 dB the points can go, so the curve between
    // points is never cut off.
    let view_db = max_db + 3.0;
    let (outer, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), PLOT_H + LABELS_H),
        Sense::hover(),
    );
    let frame = Rect::from_min_max(
        pos2(outer.min.x + AXIS_W, outer.min.y),
        pos2(outer.max.x, outer.min.y + PLOT_H),
    );
    // Inner plot: keep the end points and their value bubbles off the frame's edges.
    let plot = frame.shrink2(vec2(18.0, 12.0));
    let painter = ui.painter_at(outer);
    let dim = |c: Color32| if active { c } else { c.gamma_multiply(0.55) };
    painter.rect_filled(frame, 10.0, p.surface2);

    let x_of =
        |f: f64| plot.left() + ((f / F_MIN).ln() / (F_MAX / F_MIN).ln()) as f32 * plot.width();
    let y_of = |db: f64| plot.center().y - (db / view_db) as f32 * plot.height() / 2.0;
    let db_of = |y: f32| {
        (f64::from(plot.center().y - y) / f64::from(plot.height() / 2.0) * view_db)
            .clamp(-max_db, max_db)
    };

    // Which band the pointer is over (or dragging), found before drawing so the column can
    // be highlighted under the curve.
    let ids: [egui::Id; vidar_dsp::EQ_BANDS] =
        std::array::from_fn(|i| ui.scope_id().with(("tone", i)));
    let xs: [f32; vidar_dsp::EQ_BANDS] = std::array::from_fn(|i| x_of(vidar_dsp::EQ_POINTS_HZ[i]));
    // Hit columns split the plot halfway between neighbouring points, so every spot on the
    // graph belongs to one band and the narrow high-frequency gaps are still easy to grab.
    let cols: [Rect; vidar_dsp::EQ_BANDS] = std::array::from_fn(|i| {
        let l = if i == 0 {
            frame.left()
        } else {
            (xs[i - 1] + xs[i]) / 2.0
        };
        let r = if i + 1 == vidar_dsp::EQ_BANDS {
            frame.right()
        } else {
            (xs[i] + xs[i + 1]) / 2.0
        };
        Rect::from_x_y_ranges(l..=r, outer.y_range())
    });
    let resps: [egui::Response; vidar_dsp::EQ_BANDS] =
        std::array::from_fn(|i| ui.interact(cols[i], ids[i], Sense::click_and_drag()));
    let hot: Option<usize> = resps
        .iter()
        .position(|r| r.dragged())
        .or_else(|| resps.iter().position(|r| r.hovered()));

    if let Some(h) = hot {
        let band = Rect::from_center_size(
            pos2(xs[h], frame.center().y),
            vec2(34.0, frame.height() - 8.0),
        );
        painter.rect_filled(band, 8.0, p.accent.gamma_multiply(0.07));
    }

    // Grid: a line every 3 dB (labelled every 6) and the usual log-frequency lines
    // (50, 60, ... 100, 200, ... 1k, 2k, ...), with 100 Hz, 1 kHz and 10 kHz labelled.
    let inner_x = frame.x_range().shrink(6.0);
    let inner_y = frame.y_range().shrink(6.0);
    let fine = p.border.gamma_multiply(0.22);
    let major = p.border.gamma_multiply(0.5);
    for k in -5..=5 {
        let db = f64::from(k) * 3.0;
        let y = y_of(db);
        let labelled = k % 2 == 0 && db.abs() <= max_db;
        let color = if k == 0 {
            p.border
        } else if labelled {
            major
        } else {
            fine
        };
        painter.hline(inner_x, y, Stroke::new(1.0, color));
        if labelled {
            painter.text(
                pos2(frame.left() - 6.0, y),
                Align2::RIGHT_CENTER,
                if k == 0 {
                    "0".to_string()
                } else {
                    format!("{db:+.0}")
                },
                FontId::proportional(10.0),
                p.muted.gamma_multiply(if k == 0 { 1.0 } else { 0.75 }),
            );
        }
    }
    for decade in [10.0, 100.0, 1_000.0, 10_000.0] {
        for m in 1..10 {
            let f = decade * f64::from(m);
            if !(F_MIN..=F_MAX).contains(&f) {
                continue;
            }
            let x = x_of(f);
            painter.vline(
                x,
                inner_y,
                Stroke::new(1.0, if m == 1 { major } else { fine }),
            );
            if m == 1 {
                painter.text(
                    pos2(x + 3.0, frame.top() + 4.0),
                    Align2::LEFT_TOP,
                    if f >= 1_000.0 {
                        format!("{}k", f / 1_000.0)
                    } else {
                        format!("{f}")
                    },
                    FontId::proportional(9.5),
                    p.muted.gamma_multiply(0.7),
                );
            }
        }
    }

    // The response curve, with a soft fill towards 0 dB.
    let mut eq = vidar_dsp::VoiceEq::new(48_000.0);
    eq.set_gains(gains);
    let line = if active {
        p.accent
    } else {
        p.muted.gamma_multiply(0.7)
    };
    let n = 128;
    let pts: Vec<egui::Pos2> = (0..=n)
        .map(|k| {
            let f = F_MIN * (F_MAX / F_MIN).powf(k as f64 / n as f64);
            pos2(x_of(f), y_of(eq.response_db(f).clamp(-max_db, max_db)))
        })
        .collect();
    if active {
        let zero = y_of(0.0);
        let fill = line.gamma_multiply(0.12);
        let mut mesh = egui::Mesh::default();
        let mut tri = |a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| {
            let base = mesh.vertices.len() as u32;
            for v in [a, b, c] {
                mesh.colored_vertex(v, fill);
            }
            mesh.add_triangle(base, base + 1, base + 2);
        };
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (za, zb) = (pos2(a.x, zero), pos2(b.x, zero));
            if (a.y - zero) * (b.y - zero) < 0.0 {
                // The segment crosses 0 dB: fill each side up to the crossing point.
                let t = (zero - a.y) / (b.y - a.y);
                let m = pos2(a.x + t * (b.x - a.x), zero);
                tri(a, m, za);
                tri(m, b, zb);
            } else {
                tri(a, b, zb);
                tri(a, zb, za);
            }
        }
        painter.add(mesh);
    }
    painter.add(egui::Shape::line(pts, Stroke::new(2.0, line)));

    // Points, band labels and interaction.
    let mut out = None;
    for (i, resp) in resps.into_iter().enumerate() {
        let x = xs[i];
        let is_hot = hot == Some(i);
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(if resp.dragged() {
                egui::CursorIcon::ResizeVertical
            } else {
                egui::CursorIcon::Grab
            });
        }
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let mut g = gains;
                g[i] = (db_of(pos.y) * 2.0).round() / 2.0;
                if g != gains {
                    out = Some((g, false));
                }
            }
        }
        if resp.drag_stopped() {
            out = Some((out.map_or(gains, |(g, _)| g), true));
        }
        if resp.double_clicked() {
            let mut g = gains;
            g[i] = 0.0;
            out = Some((g, true));
        }

        let g = out.map_or(gains[i], |(g, _): ([f64; vidar_dsp::EQ_BANDS], bool)| g[i]);
        let c = pos2(x, y_of(g));
        let r = if is_hot { 7.5 } else { 6.0 };
        let fill = if active { p.accent } else { p.muted };
        painter.circle_filled(c, r + 2.5, p.surface2);
        painter.circle_filled(c, r, fill);
        painter.circle_filled(
            c,
            r * 0.38,
            p.surface2.gamma_multiply(if active { 0.9 } else { 0.6 }),
        );
        if is_hot {
            painter.circle_stroke(c, r + 4.0, Stroke::new(1.5, p.accent.gamma_multiply(0.45)));
            // Value bubble on the side of the point with more room.
            let text = db_text(g);
            let galley = painter.layout_no_wrap(text, semi(11.0), p.on_accent);
            let size = galley.size() + vec2(12.0, 4.0);
            let dy = if c.y > plot.center().y {
                -(r + 8.0 + size.y / 2.0)
            } else {
                r + 8.0 + size.y / 2.0
            };
            let mut bubble = Rect::from_center_size(c + vec2(0.0, dy), size);
            bubble = bubble.translate(vec2(
                (frame.left() + 2.0 - bubble.left()).max(0.0)
                    + (frame.right() - 2.0 - bubble.right()).min(0.0),
                0.0,
            ));
            painter.rect_filled(bubble, 6.0, if active { p.accent } else { p.muted });
            painter.galley(bubble.center() - galley.size() / 2.0, galley, p.on_accent);
        }

        // Under the plot: what the band is, and its current gain.
        let (name, what) = TONE_BANDS[i];
        let name_color = if is_hot {
            p.text
        } else {
            dim(p.text.gamma_multiply(0.85))
        };
        painter.text(
            pos2(x, frame.bottom() + 11.0),
            Align2::CENTER_CENTER,
            name,
            semi(11.5),
            name_color,
        );
        let value_color = if g == 0.0 {
            dim(p.muted)
        } else {
            dim(p.accent)
        };
        painter.text(
            pos2(x, frame.bottom() + 26.0),
            Align2::CENTER_CENTER,
            db_text(g),
            FontId::proportional(10.5),
            value_color,
        );
        if resp.hovered() && !resp.dragged() {
            resp.on_hover_ui(|ui| tip_ui(ui, (name, what)));
        }
    }
    out
}

/// Gain as shown on the tone graph: "0 dB", "+3.5 dB", "-2 dB".
fn db_text(db: f64) -> String {
    if db == 0.0 {
        "0 dB".into()
    } else if db.fract() == 0.0 {
        format!("{db:+.0} dB")
    } else {
        format!("{db:+.1} dB")
    }
}

/// Primary (filled) button.
fn primary(ui: &mut egui::Ui, p: &Pal, text: &str, width: f32) -> bool {
    let b = egui::Button::new(RichText::new(text).font(semi(14.0)).color(p.on_accent))
        .fill(p.accent)
        .corner_radius(9)
        .min_size(vec2(width, 36.0));
    ui.add(b).clicked()
}

/// Secondary (outlined) button.
fn secondary(ui: &mut egui::Ui, p: &Pal, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(p.text))
            .fill(p.surface2)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(9)
            .min_size(vec2(0.0, 32.0)),
    )
}

/// Mirrored peak envelope.
fn draw_wave(painter: &egui::Painter, rect: Rect, peaks: &[f32], color: Color32) {
    if peaks.is_empty() {
        return;
    }
    let step = rect.width() / peaks.len() as f32;
    let mid = rect.center().y;
    let half = rect.height() / 2.0;
    for (i, &pk) in peaks.iter().enumerate() {
        let x = rect.left() + (i as f32 + 0.5) * step;
        // Square root keeps quiet noise visible next to speech.
        let h = (pk.sqrt() * half).max(0.75);
        painter.line_segment(
            [pos2(x, mid - h), pos2(x, mid + h)],
            Stroke::new((step * 0.7).max(1.0), color),
        );
    }
}

/// The Vidar logo (bars from `vidar_brand`) drawn as shapes so it can move.
///
/// - `on` (0..1) fades the bars from grey to the neon colours and adds their glow.
/// - `bands` (0..1 per voice band, low to high) turn the bars into a live, mirrored spectrum
///   of what apps hear: the centre bar is the lowest band, the outer bars the highest. Speech
///   is loudest low down, so talking keeps the logo's bell shape while it moves; when you are
///   quiet the bars settle into a smaller copy of the logo.
/// - `wave` is set while starting or stopping: a ripple runs through the bars.
#[allow(clippy::too_many_arguments)]
fn draw_logo(
    painter: &egui::Painter,
    p: &Pal,
    centre: egui::Pos2,
    half: f32,
    on: f32,
    bands: &[f32; vidar_dsp::BANDS],
    wave: Option<f32>,
    time: f32,
) {
    let w = vidar_brand::BAR_HALF_WIDTH * half;
    let mid = vidar_brand::BARS.len() / 2;
    let tallest = vidar_brand::BARS[mid].1;
    for (i, &(bx, half_h, bright)) in vidar_brand::BARS.iter().enumerate() {
        let mut h = half_h;
        if on > 0.0 {
            let band = bands[i.abs_diff(mid).min(vidar_dsp::BANDS - 1)];
            // Left and right twins differ slightly so the mirror does not look frozen.
            let jitter = 1.0 + 0.12 * band * (time * 9.0 + i as f32 * 2.3).sin();
            let shape = half_h / tallest;
            let quiet = half_h * (0.5 + 0.03 * (time * 1.6 + i as f32).sin());
            let live = quiet + band * jitter * (0.2 + 0.45 * shape);
            h += (live - h) * on;
        }
        if let Some(t) = wave {
            h *= 1.0 + 0.35 * (t * 6.0 - bx * 5.0).sin().max(0.0);
        }
        let h = h.clamp(0.02, 0.85) * half;
        let [r, g, b] = vidar_brand::neon(bx);
        let neon = Color32::from_rgb(r as u8, g as u8, b as u8);
        let col = p
            .muted
            .lerp_to_gamma(neon, on)
            .gamma_multiply((0.35 + 0.65 * bright) * (0.8 + 0.2 * on));
        let x = centre.x + bx * half;
        let bar = Rect::from_min_max(pos2(x - w, centre.y - h - w), pos2(x + w, centre.y + h + w));
        if on > 0.0 {
            for (grow, alpha) in [(3.0, 0.10), (1.6, 0.16)] {
                painter.rect_filled(
                    bar.expand(w * grow),
                    w * (1.0 + grow),
                    neon.gamma_multiply(alpha * on * bright),
                );
            }
        }
        painter.rect_filled(bar, w, col);
    }
}

/// Bars in the voice ring around the power button.
const RING_BARS: usize = 72;

/// A fixed pseudo-random number in 0..1 for bar `k`, slot `i` (stable from frame to frame).
fn bar_rand(k: usize, i: u32) -> f32 {
    let mut x = (k as u32).wrapping_mul(0x9E37_79B1) ^ i.wrapping_mul(0x85EB_CA77);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65_535.0
}

/// How long bar `k` wants to be (0..1) for the current voice bands.
///
/// Every bar listens to its own mix of two bands and has its own sensitivity, which drifts
/// slowly over time, so neighbours react differently to the same sound. The result is
/// compressed (it rises quickly and never quite reaches 1), so loud speech keeps the ring
/// lively instead of pinning every bar at full length.
fn ring_target(k: usize, bands: &[f32; vidar_dsp::BANDS], time: f32) -> f32 {
    let n = vidar_dsp::BANDS;
    // Lower bands carry most of the voice; favour them so most bars move when you talk.
    let b1 = ((bar_rand(k, 1).powf(1.6)) * n as f32) as usize % n;
    let b2 = ((bar_rand(k, 2)) * n as f32) as usize % n;
    let mix = 0.65 * bands[b1] + 0.35 * bands[b2];
    let (w1, w2) = (0.6 + 1.6 * bar_rand(k, 3), 0.3 + 0.9 * bar_rand(k, 4));
    let (p1, p2) = (bar_rand(k, 5) * 6.3, bar_rand(k, 6) * 6.3);
    let sensitivity = 0.55 + 0.45 * (time * w1 + p1).sin() * (time * w2 + p2).sin().abs();
    // About one bar in four is extra sensitive, so peaks stand out above their neighbours.
    let peak = if bar_rand(k, 7) > 0.75 {
        1.8
    } else {
        0.8 + 0.5 * bar_rand(k, 10)
    };
    1.0 - (-2.6 * mix * sensitivity * peak).exp()
}

/// Rise and fall times for bar `k`: every bar has its own, so they jump and settle at
/// different speeds.
fn ring_speed(k: usize) -> (f32, f32) {
    (0.02 + 0.05 * bar_rand(k, 8), 0.10 + 0.25 * bar_rand(k, 9))
}

/// Colour of the ring at angle `a`, as a position on the logo's gradient (-0.7..0.7).
///
/// Three colour waves travel round the ring at different speeds, two one way and one the
/// other, so the colours keep streaming, meeting and blending like Siri's instead of just
/// turning. `flow` moves them along (faster while you talk). It depends only on the angle, so
/// colour changes smoothly from bar to bar.
fn ring_hue(a: f32, flow: f32) -> f32 {
    let h = 0.45 * (a + flow).sin()
        + 0.35 * (2.0 * a - 1.7 * flow + 1.3).sin()
        + 0.20 * (3.0 * a + 2.6 * flow + 4.1).sin();
    (0.8 * h).clamp(-0.7, 0.7)
}

/// What moves the ring as a whole: how far the colours have flowed, the pulse clock, and the
/// average bar length.
#[derive(Clone, Copy)]
struct RingMotion {
    flow: f32,
    pulse: f32,
    loudness: f32,
}

/// The circle of bars around the power button.
///
/// - Bars are wedges that fill their whole slice of the circle, so there are no gaps; they are
///   one mesh, so neighbours meet without seams.
/// - Colour is set at every wedge corner from its angle (`ring_hue`), so the gradient runs
///   smoothly across and between bars; each bar also brightens from its foot to its tip.
/// - Each bar's length is lightly blended with its neighbours, so the edge moves in smooth
///   waves while every bar still has its own motion.
/// - Pulsar: soft gradient rings keep rippling outwards and fading, quicker and brighter
///   while you talk.
///
/// `on` fades from a quiet grey ring to the coloured, moving one.
#[allow(clippy::too_many_arguments)]
fn draw_voice_ring(
    painter: &egui::Painter,
    p: &Pal,
    c: egui::Pos2,
    r: f32,
    on: f32,
    voice: f32,
    ring: &[f32; RING_BARS],
    time: f32,
    motion: RingMotion,
    dark: bool,
) {
    use egui::epaint::Mesh;
    use std::f32::consts::{PI, TAU};
    const REST: f32 = 4.0; // length of a quiet bar, px
    const GROW: f32 = 24.0; // extra length at full voice, px
    const PULSES: usize = 3;
    const PULSE_TRAVEL: f32 = 26.0; // how far a pulse ripples out, px
    const SEGMENTS: usize = 120; // smoothness of the pulse rings
    let n = RING_BARS;
    let slot = TAU / n as f32;
    let dir = |a: f32| vec2(a.cos(), a.sin());
    let grey = p.track_idle.gamma_multiply(0.7);
    let colour = |a: f32, strength: f32| -> Color32 {
        let [cr, cg, cb] = vidar_brand::neon(ring_hue(a, motion.flow));
        let neon = Color32::from_rgb(cr as u8, cg as u8, cb as u8);
        grey.lerp_to_gamma(neon, on)
            .gamma_multiply(strength.clamp(0.0, 1.0))
    };
    let clear = |a: f32| colour(a, 0.0);

    // Pulses first, so the bars sit on top of them.
    if on > 0.0 {
        let loud = motion.loudness.min(1.0);
        for i in 0..PULSES {
            let phase = (motion.pulse + i as f32 / PULSES as f32).fract();
            let fade = (1.0 - phase).powi(2) * (phase * 8.0).min(1.0);
            let alpha = fade * on * (0.12 + 0.45 * loud);
            let radius = r + REST + 4.0 + phase * PULSE_TRAVEL;
            let half_w = 1.5 + 4.0 * (1.0 - phase);
            let mut mesh = Mesh::default();
            for k in 0..=SEGMENTS {
                let a = -PI / 2.0 + k as f32 / SEGMENTS as f32 * TAU;
                let base = mesh.vertices.len() as u32;
                mesh.colored_vertex(c + dir(a) * (radius - half_w), clear(a));
                mesh.colored_vertex(c + dir(a) * radius, colour(a, alpha));
                mesh.colored_vertex(c + dir(a) * (radius + half_w), clear(a));
                if k > 0 {
                    let prev = base - 3;
                    for (x, y) in [(0, 1), (1, 2)] {
                        mesh.add_triangle(prev + x, prev + y, base + x);
                        mesh.add_triangle(prev + y, base + y, base + x);
                    }
                }
            }
            painter.add(egui::Shape::mesh(mesh));
        }
    }

    // Bars: one mesh of touching wedges, plus a soft fringe past each tip.
    let mut mesh = Mesh::default();
    let quad = |mesh: &mut Mesh, pts: [egui::Pos2; 4], cols: [Color32; 4]| {
        let base = mesh.vertices.len() as u32;
        for (pt, col) in pts.into_iter().zip(cols) {
            mesh.colored_vertex(pt, col);
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    };
    for k in 0..n {
        let len = 0.2 * ring[(k + n - 1) % n] + 0.6 * ring[k] + 0.2 * ring[(k + 1) % n];
        let ac = -PI / 2.0 + k as f32 * slot;
        let (a0, a1) = (ac - slot / 2.0, ac + slot / 2.0);
        // A slow shimmer runs round the ring while Vidar is on, so it is never frozen.
        let shimmer = 1.2 * (0.5 + 0.5 * (ac * 2.0 - time * 1.5).sin());
        // Waveform: two travelling waves raise big lobes in some places and keep others low,
        // drifting round with the colours. Silence keeps every bar short.
        let shape = (0.5 + 0.5 * (3.0 * ac + 1.1 * motion.flow).sin())
            * (0.6 + 0.4 * (7.0 * ac - 1.7 * motion.flow + 2.0).sin());
        let len = (len * (0.45 + 1.1 * shape)).min(1.0);
        let tip = r + REST + on * (shimmer + GROW * len);
        let bright = 0.6 + 0.4 * len.max(0.4 * voice);
        quad(
            &mut mesh,
            [
                c + dir(a0) * r,
                c + dir(a1) * r,
                c + dir(a1) * tip,
                c + dir(a0) * tip,
            ],
            [
                colour(a0, 0.55 * bright),
                colour(a1, 0.55 * bright),
                colour(a1, bright),
                colour(a0, bright),
            ],
        );
        // Fringe: fades out past the tip (a glow in the dark theme, a soft edge in light).
        let fringe = if dark { 2.0 + 6.0 * len * on } else { 1.2 };
        let edge = if dark { 0.45 * bright } else { 0.6 * bright };
        quad(
            &mut mesh,
            [
                c + dir(a0) * tip,
                c + dir(a1) * tip,
                c + dir(a1) * (tip + fringe),
                c + dir(a0) * (tip + fringe),
            ],
            [colour(a0, edge), colour(a1, edge), clear(a1), clear(a0)],
        );
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn modal_frame(p: &Pal) -> egui::Frame {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(14)
        .inner_margin(egui::Margin::same(18))
}

/// Match the Windows title bar to the theme (Windows 11; older versions ignore the colours).
fn paint_title_bar(frame: &eframe::Frame, p: &Pal, dark: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    let Ok(handle) = frame.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as *mut _);
    let colorref =
        |c: Color32| u32::from(c.r()) | (u32::from(c.g()) << 8) | (u32::from(c.b()) << 16);
    let dark_flag: i32 = i32::from(dark);
    let caption = colorref(p.bg);
    let text = colorref(p.text);
    // SAFETY: valid window handle owned by eframe; values are plain integers.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const dark_flag).cast(),
            4,
        );
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, (&raw const caption).cast(), 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_TEXT_COLOR, (&raw const text).cast(), 4);
    }
}

// ---------------------------------------------------------------- window

use crate::Page as Tab;

pub struct VidarWindow {
    handle: Handle,
    ui_rx: Arc<Mutex<Receiver<UiCmd>>>,
    quit: Arc<AtomicBool>,
    tab: Tab,
    theme: String,
    show_details: bool,
    show_help: bool,
    /// Asking "remove the virtual microphone?".
    confirm_uninstall: bool,
    fine: f32,
    fine_drag: bool,
    /// Loudness meter in Settings: the level right now and the speech level apps hear
    /// (dBFS, smoothed over a few seconds of talking; `None` until you have talked).
    level_now_db: f32,
    voice_db: Option<f32>,
    /// Seconds of speech heard so far; the advice waits for a couple of seconds of talking.
    voice_secs: f32,
    /// Version shown in the test waveform and heard on playback.
    selected: Track,
    /// Loop selection in the test recording (fractions 0..1), if any.
    sel: Option<(f64, f64)>,
    /// Where a drag on the waveform began.
    drag_from: Option<f64>,
    /// Playback start when stopped (fraction 0..1).
    cursor: f64,
    /// Theme the title bar was last painted for.
    titlebar_dark: Option<bool>,
    /// Smoothed voice-band levels (0..1) driving the logo bars.
    logo_bands: [f32; vidar_dsp::BANDS],
    /// Length (0..1) of each bar in the voice ring around the power button.
    ring: [f32; RING_BARS],
    /// How far the ring's colours have flowed; advances faster while you talk.
    ring_flow: f32,
    /// Clock for the pulses rippling out of the ring; runs faster while you talk.
    ring_pulse: f32,
}

/// Strength presets for the gate's open threshold.
const STRENGTH: [(&str, f64); 3] = [("Gentle", 0.55), ("Balanced", 0.70), ("Strong", 0.82)];

/// Longest voice-test recording.
const MAX_TEST_SECONDS: f64 = 20.0;

/// Media-player time: "0:04.2".
fn fmt_secs(s: f64) -> String {
    let s = s.max(0.0);
    let m = (s / 60.0).floor();
    format!("{m:.0}:{:04.1}", s - m * 60.0)
}

fn strength_index(t: f64) -> Option<usize> {
    STRENGTH.iter().position(|(_, v)| (v - t).abs() < 0.005)
}

fn short_cable_name(v: &View) -> String {
    v.cable_names
        .as_ref()
        .map(|(c, _)| c.split(" (").next().unwrap_or(c).to_string())
        .unwrap_or_else(|| "Vidar AI Microphone".into())
}

impl VidarWindow {
    pub fn new(
        handle: Handle,
        ui_rx: Arc<Mutex<Receiver<UiCmd>>>,
        quit: Arc<AtomicBool>,
        ctx: &egui::Context,
    ) -> Self {
        let v = handle.view();
        setup_style(ctx, &v.ui_theme);
        if let Ok(mut r) = handle.repaint.lock() {
            *r = Some(ctx.clone());
        }
        Self {
            handle,
            ui_rx,
            quit,
            tab: Tab::Home,
            theme: v.ui_theme.clone(),
            show_details: false,
            show_help: false,
            confirm_uninstall: false,
            fine: v.vad_threshold as f32,
            fine_drag: false,
            level_now_db: -100.0,
            voice_db: None,
            voice_secs: 0.0,
            selected: Track::Original,
            sel: None,
            drag_from: None,
            cursor: 0.0,
            titlebar_dark: None,
            logo_bands: [0.0; vidar_dsp::BANDS],
            ring: [0.0; RING_BARS],
            ring_flow: 0.0,
            ring_pulse: 0.0,
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        // The window caption already says "Vidar" and the logo is on the power button, so the
        // header is just the tab bar, centred.
        {
            ui.vertical_centered(|ui| {
                // A pill-shaped tab bar: a sunken track with the active tab raised on it.
                const TABS: [(Tab, &str, &str); 4] = [
                    (Tab::Home, "Home", "Turn noise cancellation on or off, and pick your microphone, mode and strength"),
                    (Tab::Test, "Test", "Record yourself once and compare every mode and strength"),
                    (Tab::Settings, "Settings", "Appearance, microphone options, apps and Windows, troubleshooting"),
                    (Tab::About, "About", "Version, licence and the components Vidar is built on"),
                ];
                let (seg_w, h, pad) = (76.0, 30.0, 3.0);
                let (track, _) = ui.allocate_exact_size(vec2(seg_w * TABS.len() as f32 + pad * 2.0, h), Sense::hover());
                let painter = ui.painter().clone();
                painter.rect_filled(track, h / 2.0, p.surface2);
                painter.rect_stroke(track, h / 2.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
                let busy_test = matches!(v.test.phase, TestPhase::Recording | TestPhase::Analyzing | TestPhase::Playing(_));
                for (i, (tab, name, tip)) in TABS.into_iter().enumerate() {
                    let r = Rect::from_min_size(pos2(track.left() + pad + seg_w * i as f32, track.top() + pad), vec2(seg_w, h - pad * 2.0));
                    let resp = ui.interact(r, ui.scope_id().with(("tab", i)), Sense::click());
                    let active = self.tab == tab;
                    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() && !active, 0.1);
                    if active {
                        painter.rect_filled(r, r.height() / 2.0, p.surface);
                        painter.rect_stroke(r, r.height() / 2.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
                    } else if hover > 0.0 {
                        painter.rect_filled(r, r.height() / 2.0, p.border.gamma_multiply(0.5 * hover));
                    }
                    let color = if active { p.text } else { p.muted.lerp_to_gamma(p.text, hover) };
                    let galley = painter.layout_no_wrap(name.to_string(), if active { semi(13.0) } else { FontId::proportional(13.0) }, color);
                    let text_pos = r.center() - galley.size() / 2.0;
                    let text_right = text_pos.x + galley.size().x;
                    painter.galley(text_pos, galley, color);
                    // Live dot on Test while it records, analyses or plays.
                    if tab == Tab::Test && busy_test {
                        let dot = if v.test.phase == TestPhase::Recording { p.danger } else { p.blue };
                        painter.circle_filled(pos2(text_right + 6.0, r.center().y - 4.0), 3.0, dot);
                    }
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.on_hover_ui(|ui| tip_ui(ui, (name, tip))).clicked() {
                        self.tab = tab;
                    }
                }
            });
        }
    }

    fn status_line(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        let color = tone_color(p, v.tone);
        ui.horizontal_wrapped(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(8.0, 16.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, color);
            ui.label(RichText::new(&v.status).color(if v.tone == Tone::Idle {
                p.muted
            } else {
                color
            }));
        });
        if let Some(d) = &v.detail {
            if ui
                .link(
                    RichText::new(if self.show_details {
                        "Hide details"
                    } else {
                        "Show details"
                    })
                    .small(),
                )
                .clicked()
            {
                self.show_details = !self.show_details;
            }
            if self.show_details {
                ui.label(RichText::new(d).small().monospace().color(p.muted));
            }
        }
    }

    // ------------------------------------------------------------ Home

    fn home(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        if !v.cable_installed && v.install == InstallState::Idle {
            card(ui, p, |ui| {
                ui.label(
                    RichText::new("One-time setup")
                        .font(semi(16.0))
                        .color(p.text),
                );
                ui.label(RichText::new("Apps get your clean voice from Vidar AI Microphone, which needs the VB-CABLE driver by VB-Audio (donationware).").color(p.muted));
                ui.add_space(4.0);
                if primary(ui, p, "Install virtual microphone", ui.available_width()) {
                    self.handle.send(Cmd::InstallDriver);
                }
                ui.label(RichText::new("Windows will ask for permission. Your speakers and microphone stay as they are.").small().color(p.muted));
            });
            ui.add_space(8.0);
        }

        // Read the engine's meters directly so the logo follows the voice frame by frame.
        let live = v
            .live
            .as_ref()
            .filter(|_| v.state == RunState::Running)
            .map(|m| m.snapshot());
        let voice = live.map_or(0.0, |m| m.speech_prob as f32);
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        let time = ui.input(|i| i.time) as f32;
        // Higher bands carry less energy in speech; tilt them up ~4 dB per octave so
        // "s" and "t" sounds reach the outer bars too.
        let raw: [f32; vidar_dsp::BANDS] = std::array::from_fn(|k| {
            live.map_or(0.0, |m| {
                (((m.bands[k] + 4.0 * k as f64 + 68.0) / 42.0).clamp(0.0, 1.0)) as f32
            })
        });
        for (s, &target) in self.logo_bands.iter_mut().zip(&raw) {
            // Fast attack, slower release, like a VU meter.
            let tau = if target > *s { 0.025 } else { 0.16 };
            *s += (target - *s) * (1.0 - (-dt / tau).exp());
        }
        for (k, s) in self.ring.iter_mut().enumerate() {
            let target = ring_target(k, &raw, time);
            let (attack, release) = ring_speed(k);
            let tau = if target > *s { attack } else { release };
            *s += (target - *s) * (1.0 - (-dt / tau).exp());
        }
        let voice = ui
            .ctx()
            .animate_value_with_time(ui.scope_id().with("voice"), voice, 0.15);
        // The colours drift slowly in silence and stream faster while you talk. Integrated
        // frame by frame, so a change of speed never makes them jump.
        let loudness = self.ring.iter().sum::<f32>() / RING_BARS as f32;
        let talking = voice.max(1.5 * loudness).min(1.0);
        self.ring_flow += dt * (0.4 + 2.6 * talking);
        self.ring_pulse += dt * (0.35 + 0.65 * talking);
        let motion = RingMotion {
            flow: self.ring_flow,
            pulse: self.ring_pulse,
            loudness,
        };
        if power_button(ui, p, v.state, voice, &self.logo_bands, &self.ring, motion) {
            if v.cable_installed || v.state != RunState::Stopped {
                self.handle.send(Cmd::Toggle);
            } else {
                self.handle.send(Cmd::InstallDriver);
            }
        }

        let (headline, sub) = match v.state {
            RunState::Running if v.muted => (
                "Microphone muted".to_string(),
                "Apps hear silence until you unmute".to_string(),
            ),
            RunState::Running => (
                "Noise cancellation is on".to_string(),
                format!("Apps hear \"{}\"", short_cable_name(v)),
            ),
            RunState::Starting => ("Starting...".to_string(), String::new()),
            RunState::Stopping => (
                "Stopping...".to_string(),
                "Giving your microphone back to Windows".to_string(),
            ),
            RunState::Stopped => (
                "Noise cancellation is off".to_string(),
                "Click the button to start".to_string(),
            ),
        };
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(headline).font(semi(17.0)).color(p.text));
            if !sub.is_empty() {
                ui.label(RichText::new(sub).color(p.muted));
            }
        });
        if matches!(v.tone, Tone::Warn | Tone::Bad) {
            ui.add_space(4.0);
            self.status_line(ui, p, v);
        }
        ui.add_space(10.0);

        let running = v.state != RunState::Stopped;
        card(ui, p, |ui| {
            section(ui, p, "Microphone");
            let current = v
                .inputs
                .iter()
                .find(|c| c.id == v.selected_input)
                .map(|c| c.label.clone())
                .unwrap_or_else(|| "Windows default microphone".into());
            ui.add_enabled_ui(!running && v.test.phase != TestPhase::Recording, |ui| {
                egui::ComboBox::from_id_salt("mic")
                    .width(ui.available_width())
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for c in &v.inputs {
                            let mut label = c.label.clone();
                            if c.is_default {
                                label.push_str("  (Windows default)");
                            }
                            if ui
                                .selectable_label(c.id == v.selected_input, label)
                                .clicked()
                            {
                                self.handle.send(Cmd::SelectInput(c.id.clone()));
                            }
                        }
                    })
                    .response
                    .on_hover_ui(|ui| tip_ui(ui, TIP_MIC));
            });
            if running {
                ui.label(
                    RichText::new("Turn noise cancellation off to switch microphones")
                        .small()
                        .color(p.muted),
                );
            } else if let Some(hint) = v
                .inputs
                .iter()
                .find(|c| c.id == v.selected_input)
                .and_then(|c| c.hint)
            {
                ui.label(RichText::new(hint).small().color(p.warn));
            }

            ui.add_space(6.0);
            section_help(ui, p, "Mode", MODE_HELP);
            let mode = usize::from(v.engine == "dpdfnet");
            if let Some(i) = segmented(ui, p, &["Voice", "Voice HD"], Some(mode), true, &MODE_TIPS)
            {
                self.handle.send(Cmd::SetEngine(
                    if i == 0 { "rnnoise" } else { "dpdfnet" }.into(),
                ));
            }
            let mode_hint = if mode == 0 {
                "Light and fast: about 30 ms delay"
            } else {
                "Cleanest voice: about 60 ms delay, more processor work"
            };
            ui.label(RichText::new(mode_hint).small().color(p.muted));
            ui.add_space(6.0);
            section_help(ui, p, "Strength", STRENGTH_HELP);
            let names: Vec<&str> = STRENGTH.iter().map(|s| s.0).collect();
            if let Some(i) = segmented(
                ui,
                p,
                &names,
                strength_index(v.vad_threshold),
                true,
                &STRENGTH_TIPS,
            ) {
                self.handle.send(Cmd::SetVad(STRENGTH[i].1));
            }
            // Same shape as the mode hint: "<what you get>: <the trade-off>".
            let strength_hint = match strength_index(v.vad_threshold) {
                Some(0) => "Keeps every soft word: some background between words".to_string(),
                Some(1) => "Right for most people: quiet pauses, natural word endings".to_string(),
                Some(_) => "Quietest pauses: very soft words may be cut".to_string(),
                None => format!("Custom ({:.2}): set in Settings", v.vad_threshold),
            };
            ui.label(RichText::new(strength_hint).small().color(p.muted));
        });
    }

    // ------------------------------------------------------------ Test

    /// The pinned part of the Test tab: record button, waveform and player. It stays in view
    /// while the comparison list below it scrolls.
    fn test_top(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        let t = &v.test;
        let recording = t.phase == TestPhase::Recording;
        let analyzing = t.phase == TestPhase::Analyzing;
        let playing = matches!(t.phase, TestPhase::Playing(_));
        let ready = !t.variants.is_empty() && !recording && !analyzing;
        if ready {
            self.test_keys(ui, v);
        }

        card(ui, p, |ui| {
            // The introduction only until there is something to compare.
            if t.variants.is_empty() {
                ui.label(
                    RichText::new("Find your best setting")
                        .font(semi(16.0))
                        .color(p.text),
                );
                ui.label(
                    RichText::new("Record yourself talking with your usual background noise. Vidar runs that recording through every mode and strength so you can listen and compare.")
                        .color(p.muted),
                );
                ui.add_space(6.0);
            }

            // Start / stop.
            let width = ui.available_width();
            if recording || analyzing {
                ui.horizontal(|ui| {
                    let cancel_w = 84.0;
                    let main_w = width - cancel_w - ui.spacing().item_spacing.x;
                    if recording {
                        let b = egui::Button::new(RichText::new("Stop and compare").font(semi(14.0)).color(Color32::WHITE))
                            .fill(p.danger)
                            .corner_radius(9)
                            .min_size(vec2(main_w, 36.0));
                        let enough = t.progress * t.max_seconds >= 2.0;
                        if ui.add_enabled(enough, b).on_hover_ui(|ui| tip_ui(ui, ("Stop and compare", "Ends the recording here and runs it through every mode and strength."))).on_disabled_hover_ui(|ui| tip_ui(ui, ("Stop and compare", "Keep talking: at least 2 seconds are needed."))).clicked() {
                            self.handle.send(Cmd::FinishTest);
                        }
                    } else {
                        ui.add_enabled(
                            false,
                            egui::Button::new(format!("Comparing {} of {}...", ((t.progress * TEST_OPTIONS.len() as f64) as usize + 1).min(TEST_OPTIONS.len()), TEST_OPTIONS.len()))
                                .corner_radius(9)
                                .min_size(vec2(main_w, 36.0)),
                        );
                    }
                    let c = egui::Button::new(RichText::new("Cancel").color(p.text)).fill(p.surface2).stroke(Stroke::new(1.0, p.border)).corner_radius(9).min_size(vec2(cancel_w, 36.0));
                    if ui.add(c).on_hover_ui(|ui| tip_ui(ui, ("Cancel", "Throws this recording away."))).clicked() {
                        self.handle.send(Cmd::StopTest);
                    }
                });
            } else if primary_tip(
                ui,
                p,
                if t.variants.is_empty() {
                    "Start recording"
                } else {
                    "Record again"
                },
                width,
                TIP_RECORD,
            ) {
                self.sel = None;
                self.cursor = 0.0;
                self.selected = Track::Original;
                self.handle.send(Cmd::RecordTest(MAX_TEST_SECONDS));
            }
            ui.add_space(8.0);

            self.waveform(ui, p, v, width);

            if ready {
                ui.add_space(4.0);
                self.transport(ui, p, v, playing);
            }
            if let Some(e) = &t.error {
                ui.label(RichText::new(e).color(p.danger));
            }
        });
    }

    /// The scrolling part of the Test tab: the comparison, then voice tone to shape the
    /// result while listening.
    fn test_list(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        let t = &v.test;
        let playing = matches!(t.phase, TestPhase::Playing(_));
        let ready = !t.variants.is_empty()
            && t.phase != TestPhase::Recording
            && t.phase != TestPhase::Analyzing;
        if !ready {
            ui.label(RichText::new(format!("Up to {MAX_TEST_SECONDS:.0} seconds. Plays on your speakers; nothing is saved to disk")).small().color(p.muted));
            ui.add_space(8.0);
            self.tone_card(ui, p, v);
            return;
        }

        let best = if t.speech_detected {
            best_option(&t.variants)
        } else {
            None
        };
        card(ui, p, |ui| {
            section_help(ui, p, "Compare", COMPARE_HELP);
            if !t.speech_detected {
                egui::Frame::new().fill(p.warn.gamma_multiply(0.15)).corner_radius(8).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("No speech found in this recording").font(semi(13.0)).color(p.text));
                    ui.label(RichText::new("You can still hear how each mode removes your background noise. To compare how your voice sounds, record again and talk normally.").small().color(p.muted));
                });
                ui.add_space(6.0);
            }
            self.compare_grid(ui, p, v, playing, best);
        });
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Click a tile to hear it. Space plays or stops, Up and Down switch versions",
            )
            .small()
            .color(p.muted),
        );
        ui.add_space(8.0);
        self.tone_card(ui, p, v);
    }

    /// Start playback of the selected version from the cursor (inside the loop, if any).
    fn play_from(&mut self, start: f64) {
        self.cursor = start;
        self.handle.send(Cmd::PlayTest {
            track: self.selected,
            start,
            range: self.sel,
        });
    }

    /// Choose the version heard. While playing it switches in place; otherwise it starts playing.
    fn choose(&mut self, track: Track, playing: bool) {
        self.selected = track;
        if playing {
            self.handle.send(Cmd::SelectTrack(track));
        } else {
            let start = self.sel.map_or(self.cursor, |(a, b)| {
                if (a..b).contains(&self.cursor) {
                    self.cursor
                } else {
                    a
                }
            });
            self.play_from(start);
        }
    }

    fn stop_playback(&mut self, v: &View) {
        // Stop works like pause: Play continues from here.
        self.cursor = v.test.progress;
        self.handle.send(Cmd::StopTest);
    }

    /// Space = play / stop, Up / Down = previous / next version.
    fn test_keys(&mut self, ui: &mut egui::Ui, v: &View) {
        let playing = matches!(v.test.phase, TestPhase::Playing(_));
        let (space, up, down) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::Space),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            )
        });
        if space {
            if playing {
                self.stop_playback(v);
            } else {
                let start = self.sel.map_or(self.cursor, |(a, b)| {
                    if (a..b).contains(&self.cursor) {
                        self.cursor
                    } else {
                        a
                    }
                });
                self.play_from(start);
            }
        }
        if up || down {
            // Original plus every version that could run.
            let order: Vec<Track> = std::iter::once(Track::Original)
                .chain(
                    v.test
                        .variants
                        .iter()
                        .enumerate()
                        .filter(|(_, x)| x.unavailable.is_none())
                        .map(|(i, _)| Track::Variant(i)),
                )
                .collect();
            let at = order
                .iter()
                .position(|&tr| tr == self.selected)
                .unwrap_or(0);
            let next = if up {
                at.saturating_sub(1)
            } else {
                (at + 1).min(order.len() - 1)
            };
            if next != at {
                self.selected = order[next];
                if playing {
                    self.handle.send(Cmd::SelectTrack(self.selected));
                }
            }
        }
    }

    /// The waveform: live while recording, then the chosen version over the original, with a
    /// draggable loop selection and a click-to-seek playhead.
    fn waveform(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View, width: f32) {
        let t = &v.test;
        let recording = t.phase == TestPhase::Recording;
        let analyzing = t.phase == TestPhase::Analyzing;
        let playing = matches!(t.phase, TestPhase::Playing(_));
        let ready = !t.variants.is_empty() && !recording && !analyzing;
        let (rect, resp) = ui.allocate_exact_size(
            vec2(width, 110.0),
            if ready {
                Sense::click_and_drag()
            } else {
                Sense::hover()
            },
        );
        let painter = ui.painter().clone();
        painter.rect_filled(rect, 10.0, p.surface2);
        let inner = rect.shrink2(vec2(8.0, 18.0));
        let frac = |x: f32| f64::from(((x - inner.left()) / inner.width()).clamp(0.0, 1.0));
        let x_of = |f: f64| inner.left() + inner.width() * f as f32;

        if recording {
            // The view spans at least 8 s, then squeezes so the whole take always fits.
            let elapsed = t.progress * t.max_seconds;
            let window = elapsed.max(8.0);
            let filled = inner.width() * (elapsed / window) as f32;
            let mid = inner.center().y;
            let cols = (filled / 2.0).floor() as usize;
            let n = t.live.len();
            if n > 0 && cols > 0 {
                for c in 0..cols {
                    let a = c * n / cols;
                    let b = ((c + 1) * n / cols).max(a + 1).min(n);
                    let lv = t.live[a.min(n - 1)..b]
                        .iter()
                        .fold(0.0f32, |m, &v| m.max(v));
                    let x = inner.left() + c as f32 * 2.0 + 1.0;
                    let h = (lv * inner.height() / 2.0).max(1.0);
                    painter.line_segment(
                        [pos2(x, mid - h), pos2(x, mid + h)],
                        Stroke::new(1.5, p.blue),
                    );
                }
            }
            let now = inner.left() + filled;
            painter.line_segment(
                [pos2(now, rect.top() + 4.0), pos2(now, rect.bottom() - 4.0)],
                Stroke::new(2.0, p.danger),
            );
            let blink = (ui.input(|i| i.time) * 2.0).fract() < 0.6;
            if blink {
                painter.circle_filled(pos2(rect.left() + 14.0, rect.top() + 12.0), 4.5, p.danger);
            }
            painter.text(
                rect.left_top() + vec2(24.0, 12.0),
                Align2::LEFT_CENTER,
                format!("REC  {}  - talk normally", fmt_secs(elapsed)),
                semi(12.0),
                p.text,
            );
            painter.text(
                rect.right_top() + vec2(-10.0, 12.0),
                Align2::RIGHT_CENTER,
                format!("max {}", fmt_secs(t.max_seconds)),
                FontId::proportional(11.0),
                p.muted,
            );
            ui.ctx().request_repaint();
            return;
        }
        if analyzing {
            if let Some(orig) = &t.original_peaks {
                draw_wave(&painter, inner, orig, p.track_idle);
            }
            painter.rect_filled(
                Rect::from_min_size(
                    rect.left_bottom() - vec2(0.0, 4.0),
                    vec2(rect.width() * t.progress as f32, 4.0),
                ),
                2.0,
                p.blue,
            );
            painter.text(
                rect.left_top() + vec2(10.0, 8.0),
                Align2::LEFT_TOP,
                "Processing your recording with every mode...",
                semi(12.0),
                p.text,
            );
            ui.ctx().request_repaint();
            return;
        }
        let Some(orig) = t.original_peaks.as_ref().filter(|_| ready) else {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Your recording appears here",
                FontId::proportional(13.0),
                p.muted,
            );
            return;
        };

        // --- interaction: drag = loop selection, click = seek, double-click = clear.
        let min_sel = 0.25 / t.seconds.max(0.1);
        if resp.drag_started() {
            self.drag_from = ui.input(|i| i.pointer.press_origin()).map(|o| frac(o.x));
        }
        if let (Some(from), Some(pos)) = (self.drag_from, resp.interact_pointer_pos()) {
            if resp.dragged() {
                let to = frac(pos.x);
                if (to - from).abs() >= min_sel {
                    self.sel = Some((from.min(to), from.max(to)));
                }
            }
        }
        if resp.drag_stopped() {
            self.drag_from = None;
            if let Some((a, _)) = self.sel {
                self.cursor = a;
                if playing {
                    self.play_from(a);
                }
            }
        }
        if resp.double_clicked() {
            self.sel = None;
            if playing {
                self.play_from(t.progress);
            }
        } else if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let f = frac(pos.x);
                if self.sel.is_some_and(|(a, b)| !(a..=b).contains(&f)) {
                    self.sel = None;
                }
                self.cursor = f;
                if playing {
                    self.play_from(f);
                }
            }
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }

        // --- drawing
        if let Some((a, b)) = self.sel {
            let r =
                Rect::from_x_y_ranges(x_of(a)..=x_of(b), rect.top() + 2.0..=rect.bottom() - 2.0);
            painter.rect_filled(r, 4.0, p.blue.gamma_multiply(0.16));
            for x in [r.left(), r.right()] {
                painter.line_segment(
                    [pos2(x, r.top()), pos2(x, r.bottom())],
                    Stroke::new(1.5, p.blue),
                );
            }
            painter.text(
                pos2(r.center().x, rect.bottom() - 9.0),
                Align2::CENTER_CENTER,
                format!(
                    "loop {} - {}",
                    fmt_secs(a * t.seconds),
                    fmt_secs(b * t.seconds)
                ),
                FontId::proportional(10.5),
                p.blue,
            );
        }
        match self.selected {
            Track::Original => draw_wave(&painter, inner, orig, p.blue),
            Track::Variant(i) => {
                draw_wave(&painter, inner, orig, p.blue.gamma_multiply(0.30));
                if let Some(var) = t.variants.get(i) {
                    draw_wave(&painter, inner, &var.peaks, p.accent);
                }
            }
        }
        // Hover guide.
        if let Some(h) = resp.hover_pos().filter(|_| self.drag_from.is_none()) {
            painter.line_segment(
                [pos2(h.x, rect.top() + 4.0), pos2(h.x, rect.bottom() - 4.0)],
                Stroke::new(1.0, p.muted.gamma_multiply(0.5)),
            );
        }
        // Playhead while playing, cursor otherwise.
        let head = if playing { t.progress } else { self.cursor };
        if playing || head > 0.0 {
            let x = x_of(head);
            painter.line_segment(
                [pos2(x, rect.top() + 4.0), pos2(x, rect.bottom() - 4.0)],
                Stroke::new(2.0, if playing { p.text } else { p.muted }),
            );
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(x - 4.0, rect.top() + 3.0),
                    pos2(x + 4.0, rect.top() + 3.0),
                    pos2(x, rect.top() + 8.0),
                ],
                if playing { p.text } else { p.muted },
                Stroke::NONE,
            ));
        }
        if playing {
            ui.ctx().request_repaint();
        }
        let caption = match self.selected {
            Track::Original => "Original".to_string(),
            Track::Variant(i) => t
                .variants
                .get(i)
                .map(|x| option_name(&x.option))
                .unwrap_or_default(),
        };
        painter.text(
            rect.left_top() + vec2(10.0, 8.0),
            Align2::LEFT_TOP,
            caption,
            semi(11.0),
            p.muted,
        );
        resp.on_hover_ui(|ui| tip_ui(ui, TIP_WAVE));
    }

    /// Play / stop, the time, and the loop with a way to clear it.
    fn transport(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View, playing: bool) {
        let t = &v.test;
        ui.horizontal(|ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
            let painter = ui.painter();
            let hovered = resp.hovered();
            painter.circle_filled(r.center(), 17.0, if hovered { p.accent.gamma_multiply(0.85) } else { p.accent });
            if playing {
                painter.rect_filled(Rect::from_center_size(r.center(), vec2(10.0, 10.0)), 2.0, p.on_accent);
            } else {
                let c = r.center() + vec2(1.5, 0.0);
                painter.add(egui::Shape::convex_polygon(vec![c + vec2(-4.5, -7.0), c + vec2(7.0, 0.0), c + vec2(-4.5, 7.0)], p.on_accent, Stroke::NONE));
            }
            if hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.on_hover_ui(|ui| tip_ui(ui, if playing { ("Stop", "Stops playback. Shortcut: Space.") } else { ("Play", "Plays the selected version. Shortcut: Space.") })).clicked() {
                if playing {
                    self.stop_playback(v);
                } else {
                    let start = self.sel.map_or(self.cursor, |(a, b)| if (a..b).contains(&self.cursor) { self.cursor } else { a });
                    self.play_from(start);
                }
            }
            let head = if playing { t.progress } else { self.cursor };
            // Current time bright, total muted, in the UI font.
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(RichText::new(fmt_secs(head * t.seconds)).font(semi(13.0)).color(p.text));
                ui.label(RichText::new(format!("  /  {}", fmt_secs(t.seconds))).font(FontId::proportional(13.0)).color(p.muted));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.sel.is_some()
                    && ui.link(RichText::new("Clear loop").small()).on_hover_ui(|ui| tip_ui(ui, ("Clear loop", "Plays the whole recording again. Double-clicking the waveform does the same."))).clicked() {
                        self.sel = None;
                        if playing {
                            self.play_from(t.progress);
                        }
                    }
            });
        });
    }

    /// The comparison: the original on its own row, then one row per mode with a tile per
    /// strength. Clicking a tile plays that version (or switches to it while playing).
    fn compare_grid(
        &mut self,
        ui: &mut egui::Ui,
        p: &Pal,
        v: &View,
        playing: bool,
        best: Option<usize>,
    ) {
        let t = &v.test;
        const LABEL_W: f32 = 92.0;
        const GAP: f32 = 6.0;
        const TILE_H: f32 = 52.0;

        // Original.
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
        let selected = self.selected == Track::Original;
        let painter = ui.painter().clone();
        painter.rect_filled(
            r,
            10.0,
            if selected {
                p.blue.gamma_multiply(0.14)
            } else if resp.hovered() {
                p.surface2
            } else {
                p.surface2.gamma_multiply(0.55)
            },
        );
        if selected {
            painter.rect_stroke(r, 10.0, Stroke::new(1.5, p.blue), StrokeKind::Inside);
        }
        let ind = pos2(r.left() + 18.0, r.center().y);
        play_indicator(
            &painter,
            ui,
            p.blue,
            ind,
            selected,
            selected && playing,
            p.border,
        );
        painter.text(
            pos2(r.left() + 34.0, r.center().y),
            Align2::LEFT_CENTER,
            "Original",
            semi(13.5),
            p.text,
        );
        painter.text(
            pos2(r.right() - 12.0, r.center().y),
            Align2::RIGHT_CENTER,
            "your microphone, before Vidar",
            FontId::proportional(11.5),
            p.muted,
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp
            .on_hover_ui(|ui| {
                tip_ui(
                    ui,
                    (
                        "Original",
                        "Your recording as the microphone delivered it, without noise removal or voice tone.",
                    ),
                )
            })
            .clicked()
            && !(selected && playing)
        {
            self.choose(Track::Original, playing);
        }
        ui.add_space(10.0);

        // Column headers.
        let w = ui.available_width();
        let tile_w = (w - LABEL_W - GAP * 2.0) / 3.0;
        let (hdr, _) = ui.allocate_exact_size(vec2(w, 16.0), Sense::hover());
        for (k, (name, _)) in STRENGTH.iter().enumerate() {
            let x = hdr.left() + LABEL_W + k as f32 * (tile_w + GAP) + tile_w / 2.0;
            ui.painter().text(
                pos2(x, hdr.center().y),
                Align2::CENTER_CENTER,
                *name,
                semi(11.5),
                p.muted,
            );
        }
        ui.add_space(4.0);

        // One row per mode.
        let mut modes: Vec<&'static str> = Vec::new();
        for var in &t.variants {
            if !modes.contains(&var.option.mode) {
                modes.push(var.option.mode);
            }
        }
        for mode in modes {
            let (row, _) = ui.allocate_exact_size(vec2(w, TILE_H), Sense::hover());
            let first = t
                .variants
                .iter()
                .find(|x| x.option.mode == mode && x.unavailable.is_none());
            ui.painter().text(
                pos2(row.left() + 2.0, row.center().y - 8.0),
                Align2::LEFT_CENTER,
                mode,
                semi(13.5),
                p.text,
            );
            let sub = first.map_or("not available".to_string(), |x| {
                format!(
                    "{:.0} ms · {}",
                    x.delay_ms,
                    cpu_text(x.cpu_percent).replace(" of 1 core", " CPU")
                )
            });
            ui.painter().text(
                pos2(row.left() + 2.0, row.center().y + 9.0),
                Align2::LEFT_CENTER,
                sub,
                FontId::proportional(10.5),
                p.muted,
            );
            if let Some(x) = first {
                let (cpu, delay) = (x.cpu_percent, x.delay_ms);
                ui.interact(
                    Rect::from_min_size(row.min, vec2(LABEL_W - 6.0, TILE_H)),
                    ui.scope_id().with(("mode", mode)),
                    Sense::hover(),
                )
                .on_hover_ui(|ui| {
                    tip_ui(ui, METRIC_TIPS[2]);
                    ui.label(RichText::new(format!("{mode}: about {delay:.0} ms.")).size(12.5));
                    ui.add_space(4.0);
                    tip_ui(ui, METRIC_TIPS[3]);
                    let n = cpu_count();
                    ui.label(
                        RichText::new(format!(
                            "{mode}: {}. This PC has {n} logical cores, so Task Manager would show about {:.1}%.",
                            cpu_text(cpu),
                            cpu / n as f64
                        ))
                        .size(12.5),
                    );
                });
            }
            for (k, (strength, _)) in STRENGTH.iter().enumerate() {
                let Some(i) = t
                    .variants
                    .iter()
                    .position(|x| x.option.mode == mode && x.option.strength == *strength)
                else {
                    continue;
                };
                let r = Rect::from_min_size(
                    pos2(row.left() + LABEL_W + k as f32 * (tile_w + GAP), row.top()),
                    vec2(tile_w, TILE_H),
                );
                self.compare_tile(ui, p, v, r, i, playing, best == Some(i));
            }
            ui.add_space(GAP);
        }

        // What is selected, and a way to use it.
        ui.add_space(4.0);
        ui.horizontal(|ui| match self.selected {
            Track::Variant(i)
                if t.variants.get(i).is_some_and(|x| x.unavailable.is_none()) =>
            {
                let var = &t.variants[i];
                ui.label(
                    RichText::new(option_name(&var.option))
                        .font(semi(13.0))
                        .color(p.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if is_current(&var.option, v) {
                        badge(ui, p.accent.gamma_multiply(0.18), p.accent, "In use");
                    } else if primary_tip(
                        ui,
                        p,
                        "Use this",
                        110.0,
                        (
                            "Use this",
                            "Switches Vidar to this mode and strength. Voice tone stays as set below.",
                        ),
                    ) {
                        self.handle.send(Cmd::ApplyOption(i));
                    }
                });
            }
            _ => {
                ui.label(
                    RichText::new("Pick a tile to hear your recording cleaned")
                        .small()
                        .color(p.muted),
                );
            }
        });
    }

    /// One tile in the comparison grid: noise removed, the voice verdict and badges.
    #[allow(clippy::too_many_arguments)]
    fn compare_tile(
        &mut self,
        ui: &mut egui::Ui,
        p: &Pal,
        v: &View,
        r: Rect,
        i: usize,
        playing: bool,
        best: bool,
    ) {
        let t = &v.test;
        let var = &t.variants[i];
        let track = Track::Variant(i);
        let available = var.unavailable.is_none();
        let selected = self.selected == track;
        let current = is_current(&var.option, v);
        let resp = ui.interact(
            r,
            ui.scope_id().with(("tile", i)),
            if available {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let hovered = resp.hovered() && available;
        let painter = ui.painter().clone();
        let fill = if selected {
            p.accent.gamma_multiply(0.14)
        } else if hovered {
            p.surface2
        } else {
            p.surface2.gamma_multiply(0.55)
        };
        painter.rect_filled(r, 10.0, fill);
        let stroke = if selected {
            Stroke::new(1.5, p.accent)
        } else if current {
            Stroke::new(1.0, p.accent.gamma_multiply(0.5))
        } else {
            Stroke::new(1.0, p.border.gamma_multiply(0.7))
        };
        painter.rect_stroke(r, 10.0, stroke, StrokeKind::Inside);

        if !available {
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                "not available",
                FontId::proportional(11.0),
                p.muted,
            );
            let why = var.unavailable.clone().unwrap_or_default();
            resp.on_hover_ui(|ui| tip_ui_owned(ui, "Not available", &why));
            return;
        }

        let headline = if var.noise_removed_db >= 60.0 {
            "-60+ dB".to_string()
        } else {
            format!("-{:.0} dB", var.noise_removed_db)
        };
        painter.text(
            pos2(r.center().x, r.top() + 19.0),
            Align2::CENTER_CENTER,
            headline,
            semi(15.0),
            p.text,
        );
        let (verdict, vcolor) = if !t.speech_detected {
            ("less noise".to_string(), p.muted)
        } else if var.voice_change_db < -3.0 {
            (format!("voice {:+.0} dB", var.voice_change_db), p.warn)
        } else {
            ("voice intact".to_string(), p.muted)
        };
        painter.text(
            pos2(r.center().x, r.top() + 37.0),
            Align2::CENTER_CENTER,
            verdict,
            FontId::proportional(10.5),
            vcolor,
        );

        // Corners: playing / selected on the left, best and in use on the right.
        play_indicator(
            &painter,
            ui,
            p.accent,
            r.left_top() + vec2(13.0, 13.0),
            selected,
            selected && playing,
            Color32::TRANSPARENT,
        );
        if best {
            let c = r.right_top() + vec2(-12.0, 12.0);
            painter.circle_filled(c, 7.5, p.accent);
            painter.text(
                c,
                Align2::CENTER_CENTER,
                "★",
                FontId::proportional(10.0),
                p.on_accent,
            );
        }
        if current {
            painter.circle_filled(r.right_bottom() + vec2(-10.0, -10.0), 3.5, p.accent);
        }

        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let speech = t.speech_detected;
        let resp = resp.on_hover_ui(|ui| {
            ui.set_max_width(280.0);
            ui.label(RichText::new(option_name(&var.option)).font(semi(13.0)));
            if best {
                ui.label(
                    RichText::new("★ Best for you: removes the most noise without making your voice noticeably quieter.")
                        .size(12.0)
                        .color(p.accent),
                );
            }
            if current {
                ui.label(
                    RichText::new("● The mode and strength you use now.")
                        .size(12.0)
                        .color(p.accent),
                );
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("Noise removed: {:.0} dB", var.noise_removed_db)).size(12.5),
            );
            ui.label(RichText::new(METRIC_TIPS[0].1).size(11.5).color(p.muted));
            if speech {
                ui.add_space(2.0);
                ui.label(
                    RichText::new(format!("Voice: {:+.1} dB", var.voice_change_db)).size(12.5),
                );
                ui.label(RichText::new(METRIC_TIPS[1].1).size(11.5).color(p.muted));
            }
        });
        if resp.clicked() && !(selected && playing) {
            self.choose(track, playing);
        }
    }

    /// The voice tone card. It sits on the Test tab so a tone can be tried on your recording:
    /// changes are heard right away, and they also apply to noise cancellation.
    fn tone_card(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        card(ui, p, |ui| {
            section_help(ui, p, "Voice tone", TONE_HELP);
            let sel = TONE_PRESETS.iter().position(|(k, _)| *k == v.eq_preset);
            let labels: Vec<&str> = TONE_PRESETS.iter().map(|(_, l)| *l).collect();
            if let Some(i) = segmented(ui, p, &labels, sel, true, &TONE_TIPS) {
                self.handle
                    .send(Cmd::SetEqPreset(TONE_PRESETS[i].0.to_string()));
            }
            ui.add_space(6.0);
            let gains = vidar_dsp::eq_preset(&v.eq_preset).unwrap_or(v.eq_custom_db);
            if let Some((g, done)) = tone_curve(ui, p, gains, v.eq_preset != "off") {
                self.handle.send(Cmd::SetEqCustom {
                    gains_db: g,
                    save: done,
                });
            }
            ui.horizontal(|ui| {
                let hint = if v.test.variants.is_empty() {
                    "Record yourself above to hear each change as you make it"
                } else {
                    "Changes are heard right away on the cleaned versions"
                };
                ui.label(RichText::new(hint).small().color(p.muted));
                if v.eq_preset == "custom" && v.eq_custom_db.iter().any(|&g| g != 0.0) {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.link(RichText::new("Flatten curve").small()).clicked() {
                            self.handle.send(Cmd::SetEqCustom {
                                gains_db: [0.0; vidar_dsp::EQ_BANDS],
                                save: true,
                            });
                        }
                    });
                }
            });
        });
    }

    // ------------------------------------------------------------ Settings

    /// "How loud apps hear you": a live bar with quiet / good / loud zones and one line of
    /// advice that names the control to change.
    fn loudness_meter(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        const LOW: f32 = -50.0; // left end of the bar, dBFS
        const QUIET: f32 = -30.0; // speech below this is hard to hear
        const LOUD: f32 = -12.0; // speech above this sounds harsh
        let live = v
            .live
            .as_ref()
            .filter(|_| v.state == RunState::Running)
            .map(|m| m.snapshot());
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        match live {
            Some(m) => {
                let now = m.out_rms_db as f32;
                let tau = if now > self.level_now_db { 0.03 } else { 0.25 };
                self.level_now_db += (now - self.level_now_db) * (1.0 - (-dt / tau).exp());
                // Average only while you talk, so pauses do not drag the verdict down.
                if m.speech_prob > 0.6 && now > -70.0 {
                    let k = 1.0 - (-dt / 2.0).exp();
                    self.voice_db = Some(self.voice_db.map_or(now, |d| d + (now - d) * k));
                    self.voice_secs += dt;
                }
            }
            None => {
                self.level_now_db = -100.0;
                self.voice_db = None;
                self.voice_secs = 0.0;
            }
        }

        ui.label(RichText::new("How loud apps hear you").color(p.text))
            .on_hover_ui(|ui| tip_ui(ui, TIP_LEVEL));
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
        resp.on_hover_ui(|ui| tip_ui(ui, TIP_LEVEL));
        let x_of = |db: f32| rect.left() + rect.width() * ((db - LOW) / -LOW).clamp(0.0, 1.0);
        let painter = ui.painter();
        painter.rect_filled(rect, 5.0, p.surface2);
        let (left_end, right_end) = (
            CornerRadius {
                nw: 5,
                sw: 5,
                ne: 0,
                se: 0,
            },
            CornerRadius {
                nw: 0,
                sw: 0,
                ne: 5,
                se: 5,
            },
        );
        for (a, b, c, corners) in [
            (LOW, QUIET, p.track_idle, left_end),
            (QUIET, LOUD, p.accent, CornerRadius::ZERO),
            (LOUD, 0.0, p.danger, right_end),
        ] {
            let r = Rect::from_x_y_ranges(x_of(a)..=x_of(b), rect.y_range());
            painter.rect_filled(r, corners, c.gamma_multiply(0.22));
        }
        if live.is_some() && self.level_now_db > LOW {
            let fill = Rect::from_x_y_ranges(rect.left()..=x_of(self.level_now_db), rect.y_range());
            let c = if self.level_now_db >= LOUD {
                p.danger
            } else if self.level_now_db >= QUIET {
                p.accent
            } else {
                p.track_idle
            };
            painter.rect_filled(fill, 5.0, c);
        }
        if let Some(d) = self.voice_db {
            // Where your speech usually sits.
            let x = x_of(d);
            painter.line_segment(
                [pos2(x, rect.top() - 3.0), pos2(x, rect.bottom() + 3.0)],
                Stroke::new(2.0, p.text),
            );
        }

        let heard = self.voice_db.filter(|_| self.voice_secs >= 2.0);
        let (text, color) = match (live, heard) {
            (None, _) => (
                "Turn noise cancellation on and talk to see your level.",
                p.muted,
            ),
            (Some(_), None) => ("Talk normally for a few seconds...", p.muted),
            (Some(_), Some(d)) if d < QUIET => (
                if !v.auto_level {
                    "A bit quiet. Turn on Auto level below."
                } else if v.mic_boost_db < 14.5 {
                    "Still quiet. Raise Boost one step."
                } else {
                    "Still quiet. Raise your microphone volume in Windows Sound settings, or move closer."
                },
                p.warn,
            ),
            (Some(_), Some(d)) if d > LOUD => (
                if v.mic_boost_db > 0.5 {
                    "Too loud. Lower Boost one step."
                } else {
                    "Too loud. Move back a little or lower your microphone volume in Windows."
                },
                p.danger,
            ),
            (Some(_), Some(_)) => ("Good level. Nothing to change.", p.accent),
        };
        ui.label(RichText::new(text).small().color(color));
    }

    /// The strength slider without numbers: what each end means, where the presets sit, and
    /// the name of the current position.
    fn fine_strength(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        const PRESETS: [(&str, f32); 3] = [("Gentle", 0.55), ("Balanced", 0.70), ("Strong", 0.82)];
        const MIN: f32 = 0.40;
        const MAX: f32 = 0.90;
        if !self.fine_drag {
            self.fine = v.vad_threshold as f32;
        }
        let f = self.fine;
        let now = match PRESETS.iter().find(|(_, x)| (f - x).abs() < 0.015) {
            Some((name, _)) => (*name).to_string(),
            None if f < PRESETS[0].1 => "Gentler than Gentle".to_string(),
            None if f > PRESETS[2].1 => "Stronger than Strong".to_string(),
            None => {
                let i = PRESETS.iter().rposition(|(_, x)| *x < f).unwrap_or(0);
                format!("Between {} and {}", PRESETS[i].0, PRESETS[i + 1].0)
            }
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new("Fine-tune strength").color(p.text))
                .on_hover_ui(|ui| tip_ui(ui, TIP_FINE));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(now).font(semi(13.0)).color(p.text));
            });
        });
        ui.spacing_mut().slider_width = ui.available_width();
        let r = ui.add(egui::Slider::new(&mut self.fine, MIN..=MAX).show_value(false));
        // Preset marks under the track, where the handle sits for each preset.
        let pad = r.rect.height() / 2.5;
        let (l, w) = (r.rect.left() + pad, r.rect.width() - 2.0 * pad);
        let x_of = |x: f32| l + w * (x - MIN) / (MAX - MIN);
        let (marks, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
        for (name, x) in PRESETS {
            let px = x_of(x);
            ui.painter().line_segment(
                [pos2(px, marks.top()), pos2(px, marks.top() + 3.0)],
                Stroke::new(1.0, p.muted),
            );
            ui.painter().text(
                pos2(px, marks.top() + 3.0),
                Align2::CENTER_TOP,
                name,
                FontId::proportional(11.0),
                p.muted,
            );
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Keeps every word").small().color(p.muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("Keeps out more noise").small().color(p.muted));
            });
        });
        let r = r.on_hover_ui(|ui| tip_ui(ui, TIP_FINE));
        self.fine_drag = r.dragged();
        if r.changed() {
            self.handle.send(Cmd::SetVad(f64::from(self.fine)));
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        card(ui, p, |ui| {
            section(ui, p, "Appearance");
            let idx = match self.theme.as_str() {
                "light" => 1,
                "dark" => 2,
                _ => 0,
            };
            if let Some(i) = segmented(
                ui,
                p,
                &["Auto", "Light", "Dark"],
                Some(idx),
                true,
                &THEME_TIPS,
            ) {
                let t = ["auto", "light", "dark"][i].to_string();
                apply_theme(ui.ctx(), &t);
                self.theme = t.clone();
                self.handle.send(Cmd::SetTheme(t));
            }
        });
        ui.add_space(8.0);

        card(ui, p, |ui| {
            section(ui, p, "Microphone");
            ui.label(
                RichText::new(
                    "Most people never need to change these. Talk and watch the meter: change something only if it says so.",
                )
                .small()
                .color(p.muted),
            );
            ui.add_space(2.0);
            self.loudness_meter(ui, p, v);
            ui.add_space(6.0);

            section_help(ui, p, "Boost", BOOST_HELP);
            let boosts = [0.0, 5.0, 10.0, 15.0];
            let sel = boosts.iter().position(|b| (b - v.mic_boost_db).abs() < 0.5);
            if let Some(i) = segmented(
                ui,
                p,
                &["Off", "+5 dB", "+10 dB", "+15 dB"],
                sel,
                true,
                &BOOST_TIPS,
            ) {
                self.handle.send(Cmd::SetBoost(boosts[i]));
            }
            ui.add_space(4.0);
            if switch_row(
                ui,
                p,
                "Auto level",
                "Too quiet? Try this before Boost",
                v.auto_level,
                TIP_AUTO_LEVEL,
            ) {
                self.handle.send(Cmd::SetAutoLevel(!v.auto_level));
            }
            ui.add_space(4.0);
            self.fine_strength(ui, p, v);

            let recommended = v.mic_boost_db.abs() < 0.5
                && !v.auto_level
                && (v.vad_threshold - 0.70).abs() < 0.005;
            if !recommended {
                ui.add_space(2.0);
                if ui
                    .link(RichText::new("Reset to recommended").small())
                    .on_hover_ui(|ui| tip_ui(ui, TIP_RESET))
                    .clicked()
                {
                    self.handle.send(Cmd::SetBoost(0.0));
                    if v.auto_level {
                        self.handle.send(Cmd::SetAutoLevel(false));
                    }
                    self.handle.send(Cmd::SetVad(0.70));
                }
            }
        });
        ui.add_space(8.0);

        card(ui, p, |ui| {
            section_help(ui, p, "Voice tone", TONE_HELP);
            ui.horizontal(|ui| {
                let name = TONE_PRESETS
                    .iter()
                    .find(|(k, _)| *k == v.eq_preset)
                    .map_or("Off", |(_, l)| *l);
                ui.label(RichText::new(name).font(semi(13.5)).color(p.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if secondary(ui, p, "Tune on Test")
                        .on_hover_ui(|ui| {
                            tip_ui(
                                ui,
                                (
                                    "Tune on Test",
                                    "Voice tone lives on the Test tab, next to your recording, so you hear each change while you make it.",
                                ),
                            )
                        })
                        .clicked()
                    {
                        self.tab = Tab::Test;
                    }
                });
            });
        });
        ui.add_space(8.0);

        card(ui, p, |ui| {
            section(ui, p, "Apps and Windows");
            if switch_row(
                ui,
                p,
                "Start with Windows",
                "Starts in the tray, noise cancellation off",
                v.start_with_windows,
                TIP_AUTOSTART,
            ) {
                self.handle
                    .send(Cmd::SetStartWithWindows(!v.start_with_windows));
            }
            ui.add_space(2.0);
            if ui
                .link(RichText::new("How to use Vidar in a specific app").small())
                .clicked()
            {
                self.show_help = true;
            }
        });
        ui.add_space(8.0);

        card(ui, p, |ui| {
            section_help(ui, p, "Virtual microphone", TIP_INSTALL);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(8.0, 16.0), Sense::hover());
                let (color, text) = if v.cable_installed {
                    (
                        p.accent,
                        format!("Installed: apps see \"{}\"", short_cable_name(v)),
                    )
                } else {
                    (
                        p.warn,
                        "Not installed: apps cannot get your clean voice yet".to_string(),
                    )
                };
                ui.painter().circle_filled(r.center(), 4.0, color);
                ui.label(RichText::new(text).color(p.text));
            });
            ui.label(
                RichText::new("Runs on VB-CABLE by VB-Audio, donationware (see About)")
                    .small()
                    .color(p.muted),
            );
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if v.cable_installed {
                    if secondary(ui, p, "Reinstall")
                        .on_hover_ui(|ui| tip_ui(ui, TIP_REINSTALL))
                        .clicked()
                    {
                        self.handle.send(Cmd::InstallDriver);
                    }
                    let b = egui::Button::new(RichText::new("Uninstall").color(p.danger))
                        .fill(p.surface2)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(9)
                        .min_size(vec2(0.0, 32.0));
                    if ui
                        .add(b)
                        .on_hover_ui(|ui| tip_ui(ui, TIP_UNINSTALL))
                        .clicked()
                    {
                        self.confirm_uninstall = true;
                    }
                } else if primary_tip(ui, p, "Install virtual microphone", 220.0, TIP_INSTALL) {
                    self.handle.send(Cmd::InstallDriver);
                }
            });
        });
        ui.add_space(8.0);

        card(ui, p, |ui| {
            section(ui, p, "Advanced");
            if switch_row(
                ui,
                p,
                "Raw microphone signal",
                "Skips the microphone's own effects",
                v.raw_capture,
                TIP_RAW,
            ) {
                self.handle.send(Cmd::SetRawCapture(!v.raw_capture));
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if secondary(ui, p, "Open logs")
                    .on_hover_ui(|ui| tip_ui(ui, TIP_LOGS))
                    .clicked()
                {
                    let _ = std::process::Command::new("explorer")
                        .arg(crate::logging::log_dir())
                        .spawn();
                }
            });
            if v.tone != Tone::Idle {
                ui.add_space(4.0);
                self.status_line(ui, p, v);
            }
        });
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            if ui
                .link(
                    RichText::new(concat!("Vidar ", env!("CARGO_PKG_VERSION"), " · About")).small(),
                )
                .clicked()
            {
                self.tab = Tab::About;
            }
        });
    }

    fn about(&mut self, ui: &mut egui::Ui, p: &Pal) {
        // Header: the logo in its "on" colours, the name and the version.
        ui.add_space(6.0);
        ui.vertical_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(96.0, 64.0), Sense::hover());
            draw_logo_static(ui.painter(), r.center(), 40.0);
            ui.add_space(2.0);
            ui.label(RichText::new("Vidar").font(semi(22.0)).color(p.text));
            ui.add_space(2.0);
            badge(
                ui,
                p.accent.gamma_multiply(0.18),
                p.accent,
                concat!("Version ", env!("CARGO_PKG_VERSION")),
            );
            ui.add_space(6.0);
            ui.label(RichText::new("AI noise cancellation for your microphone").color(p.muted));
        });
        ui.add_space(14.0);

        // VB-Audio's distribution terms ask that users can identify VB-CABLE, see where it comes
        // from and that it is donationware: https://vb-audio.com/Services/licensing.htm
        card(ui, p, |ui| {
            section(ui, p, "Virtual microphone");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("VB-CABLE").font(semi(15.0)).color(p.text));
                badge(ui, p.blue.gamma_multiply(0.18), p.blue, "Donationware");
            });
            ui.label(RichText::new("by VB-Audio Software").small().color(p.muted));
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Vidar AI Microphone and Vidar Internal are VB-CABLE devices under Vidar's names. All participations are welcome.",
                )
                .color(p.text),
            );
            ui.add_space(10.0);
            if primary(ui, p, "Support VB-Audio", 150.0) {
                open_url(VB_CABLE_URL);
            }
            ui.add_space(6.0);
            // The origin VB-Audio's distribution terms ask us to name, as text.
            ui.label(
                RichText::new(format!(
                    "From {VB_CABLE_SITE}. Companies and organisations need a VB-CABLE licence from VB-Audio."
                ))
                .small()
                .color(p.muted),
            );
        });
        ui.add_space(8.0);

        card(ui, p, |ui| {
            section(ui, p, "Noise models");
            ui.add_space(4.0);
            for (i, &(name, by, licence, url)) in CREDITS.iter().enumerate() {
                if i > 0 {
                    let y = ui.cursor().top();
                    let x = ui.max_rect().x_range();
                    ui.painter().hline(x, y, Stroke::new(1.0, p.border));
                }
                credit_row(ui, p, name, by, licence, url);
            }
        });
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("Vidar is MIT licensed. Other components and their licences: THIRD_PARTY_NOTICES.md")
                    .small()
                    .color(p.muted),
            );
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context, p: &Pal, v: &View) {
        if matches!(v.install, InstallState::Running | InstallState::Done(_)) {
            egui::Modal::new(egui::Id::unique("install")).frame(modal_frame(p)).show(ctx, |ui| {
                ui.set_width(340.0);
                ui.label(RichText::new("Virtual microphone").font(semi(16.0)).color(p.text));
                ui.add_space(6.0);
                match &v.install {
                    InstallState::Running => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(if v.uninstalling { "Removing... click 'Yes' if Windows asks for permission." } else { "Installing... click 'Yes' if Windows asks for permission." });
                        });
                    }
                    InstallState::Done(Ok(())) if v.uninstalling => {
                        ui.label(if v.cable_installed {
                            "Removed. Windows finishes the removal after a restart."
                        } else {
                            "Removed. Your own microphone is the Windows default again."
                        });
                    }
                    InstallState::Done(Err(e)) if v.uninstalling => {
                        ui.label(RichText::new(format!("Removal did not complete: {e}")).color(p.danger));
                    }
                    InstallState::Done(Ok(())) if v.cable_installed => {
                        ui.label(RichText::new("Ready. Apps can now use Vidar AI Microphone.").color(p.accent));
                    }
                    InstallState::Done(Ok(())) => {
                        ui.label("Setup finished. If Vidar AI Microphone does not appear within a few seconds, restart Windows once.");
                    }
                    InstallState::Done(Err(e)) => {
                        ui.label(RichText::new(format!("Installation did not complete: {e}")).color(p.danger));
                    }
                    InstallState::Idle => {}
                }
                if matches!(v.install, InstallState::Done(_)) {
                    ui.add_space(8.0);
                    if primary(ui, p, "Done", 100.0) {
                        self.handle.send(Cmd::DismissInstall);
                    }
                }
            });
        }
        if self.confirm_uninstall {
            let r = egui::Modal::new(egui::Id::unique("uninstall")).frame(modal_frame(p)).show(ctx, |ui| {
                ui.set_width(340.0);
                ui.label(RichText::new("Remove the virtual microphone?").font(semi(16.0)).color(p.text));
                ui.add_space(6.0);
                ui.label("Vidar turns off and your own microphone becomes the Windows default again. Apps can no longer get your clean voice until you install it again.");
                ui.label(RichText::new("Windows will ask for permission and may want a restart.").small().color(p.muted));
                ui.add_space(8.0);
                let mut answer = None;
                ui.horizontal(|ui| {
                    let b = egui::Button::new(RichText::new("Remove").font(semi(14.0)).color(Color32::WHITE)).fill(p.danger).corner_radius(9).min_size(vec2(100.0, 36.0));
                    if ui.add(b).clicked() {
                        answer = Some(true);
                    }
                    if secondary(ui, p, "Keep it").clicked() {
                        answer = Some(false);
                    }
                });
                answer
            });
            match r.inner {
                Some(true) => {
                    self.confirm_uninstall = false;
                    self.handle.send(Cmd::UninstallDriver);
                }
                Some(false) => self.confirm_uninstall = false,
                None if r.should_close() => self.confirm_uninstall = false,
                None => {}
            }
        }
        if self.show_help {
            let r = egui::Modal::new(egui::Id::unique("help")).frame(modal_frame(p)).show(ctx, |ui| {
                ui.set_width(340.0);
                ui.label(RichText::new("Using Vidar in apps").font(semi(16.0)).color(p.text));
                ui.add_space(6.0);
                ui.label("With \"Use Vidar AI Microphone as the default\" on, apps that use the Windows default microphone switch automatically.");
                ui.add_space(4.0);
                ui.label(RichText::new("If an app keeps its old microphone:").color(p.text));
                ui.label(RichText::new(format!("open its audio settings and choose \"{}\".", short_cable_name(v))).color(p.accent));
                ui.label(RichText::new("Keep your normal speakers or headphones for playback.").small().color(p.muted));
                ui.add_space(8.0);
                primary(ui, p, "Got it", 100.0)
            });
            if r.inner || r.should_close() {
                self.show_help = false;
            }
        }
    }
}

/// VB-CABLE's product page, with the donation and licence links.
const VB_CABLE_URL: &str = "https://vb-audio.com/Cable/";
/// The origin VB-Audio's distribution terms ask us to name.
const VB_CABLE_SITE: &str = "www.vb-cable.com";

/// (component, author, licence, homepage) shown under "Noise models" on the About tab.
const CREDITS: &[(&str, &str, &str, &str)] = &[
    (
        "RNNoise",
        "Xiph.Org, Mozilla, Amazon",
        "BSD-3",
        "https://github.com/xiph/rnnoise",
    ),
    (
        "DPDFNet",
        "Ceva",
        "Apache-2.0",
        "https://huggingface.co/Ceva-IP/DPDFNet",
    ),
];

/// One "Built with" entry: name and author on the left, licence on the right. The whole row
/// is a link to the project's homepage.
fn credit_row(ui: &mut egui::Ui, p: &Pal, name: &str, by: &str, licence: &str, url: &str) {
    let bg = ui.painter().add(egui::Shape::Noop);
    let inner = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(6, 7))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(name).font(semi(13.5)).color(p.text));
                    ui.label(RichText::new(by).small().color(p.muted));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    badge(ui, p.surface2, p.muted, licence);
                });
            });
        });
    let resp = ui.interact(
        inner.response.rect,
        ui.scope_id().with(("credit", name)),
        Sense::click(),
    );
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    if hover > 0.0 {
        ui.painter().set(
            bg,
            egui::Shape::rect_filled(inner.response.rect, 8.0, p.surface2.gamma_multiply(hover)),
        );
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.on_hover_text(url).clicked() {
        open_url(url);
    }
}

/// The Vidar logo at rest in its full colours (the About header).
fn draw_logo_static(painter: &egui::Painter, centre: egui::Pos2, half: f32) {
    let w = vidar_brand::BAR_HALF_WIDTH * half;
    for &(bx, half_h, bright) in vidar_brand::BARS.iter() {
        let [r, g, b] = vidar_brand::neon(bx);
        let neon = Color32::from_rgb(r as u8, g as u8, b as u8);
        let h = half_h.clamp(0.02, 0.85) * half;
        let x = centre.x + bx * half;
        let bar = Rect::from_min_max(pos2(x - w, centre.y - h - w), pos2(x + w, centre.y + h + w));
        for (grow, alpha) in [(3.0, 0.10), (1.6, 0.16)] {
            painter.rect_filled(
                bar.expand(w * grow),
                w * (1.0 + grow),
                neon.gamma_multiply(alpha * bright),
            );
        }
        painter.rect_filled(bar, w, neon.gamma_multiply(0.35 + 0.65 * bright));
    }
}

/// Opens `url` in the default browser.
fn open_url(url: &str) {
    let _ = std::process::Command::new("explorer").arg(url).spawn();
}

impl eframe::App for VidarWindow {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let dark = ctx.theme() == egui::Theme::Dark;
        if self.titlebar_dark != Some(dark) {
            self.titlebar_dark = Some(dark);
            paint_title_bar(frame, if dark { &DARK } else { &LIGHT }, dark);
        }
        if let Ok(rx) = self.ui_rx.lock() {
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    UiCmd::Show(page) => {
                        if let Some(p) = page {
                            self.tab = p;
                        }
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    UiCmd::Quit => {
                        self.quit.store(true, Relaxed);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
        }
        // Only the Home tab animates with the live voice (the logo and the ring); other tabs and
        // a minimised window repaint on changes alone.
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        // Settings shows the live loudness meter, so it animates too.
        if matches!(self.tab, Tab::Home | Tab::Settings)
            && !minimized
            && self.handle.view().state == RunState::Running
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(20));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let v = self.handle.view();
        let p = pal(ui);
        egui::Frame::new()
            .fill(p.bg)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(ui.available_height());
                self.header(ui, &p, &v);
                ui.add_space(8.0);
                if self.tab == Tab::Test {
                    self.test_top(ui, &p, &v);
                    ui.add_space(8.0);
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        match self.tab {
                            Tab::Home => self.home(ui, &p, &v),
                            Tab::Test => self.test_list(ui, &p, &v),
                            Tab::Settings => self.settings(ui, &p, &v),
                            Tab::About => self.about(ui, &p),
                        }
                        ui.add_space(8.0);
                    });
            });
        let ctx = ui.ctx().clone();
        self.dialogs(&ctx, &p, &v);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        (if visuals.dark_mode { DARK.bg } else { LIGHT.bg }).to_normalized_gamma_f32()
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Ok(mut r) = self.handle.repaint.lock() {
            *r = None;
        }
    }
}

/// Open the window and block until it is closed.
pub fn run_window(
    handle: Handle,
    ui_rx: Arc<Mutex<Receiver<UiCmd>>>,
    quit: Arc<AtomicBool>,
    page: Option<Tab>,
) {
    let icon = egui::IconData {
        rgba: vidar_brand::render(64, true),
        width: 64,
        height: 64,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Vidar")
            .with_inner_size([420.0, 660.0])
            .with_min_inner_size([380.0, 520.0])
            .with_maximize_button(false)
            .with_icon(Arc::new(icon)),
        centered: true,
        ..Default::default()
    };
    let r = eframe::run_native(
        "Vidar",
        options,
        Box::new(move |cc| {
            let mut w = VidarWindow::new(handle, ui_rx, quit, &cc.egui_ctx);
            if let Some(p) = page {
                w.tab = p;
            }
            Ok(Box::new(w))
        }),
    );
    if let Err(e) = r {
        log::warn!("window failed: {e}");
    }
}
