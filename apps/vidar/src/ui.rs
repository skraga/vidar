//! The window (egui): Home / Test / Settings. It only exists while open; closing it
//! destroys the GPU context while the engine and tray keep running.
//!
//! Light and dark palettes; "auto" follows the Windows app theme.

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, StrokeKind,
};

use crate::worker::{
    Cmd, Handle, InstallState, RunState, TestPhase, Tone, Track, UpdatePhase, VariantView, View,
    TEST_OPTIONS,
};
use crate::UiCmd;

// ---------------------------------------------------------------- tokens

// Colours, spacing, radii, type sizes and control sizes come from `theme`; this file only
// composes them. Interactive widgets below are drawn by hand and each one has the same states:
// idle, hover, pressed, selected, disabled and keyboard focus (a ring that shows only after a
// key press, as in Windows 11).

use crate::theme::{
    self, alpha, anim, focus_ring, focus_ring_circle, motion, pal, radius, semi, size, space,
    tone_color, ty, Pal, DARK, LIGHT,
};

// ---------------------------------------------------------------- widgets

/// A rounded panel.
fn card<R>(ui: &mut egui::Ui, p: &Pal, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(size::HAIRLINE, p.border))
        .corner_radius(radius::LG)
        .inner_margin(egui::Margin::same(space::LG as i8))
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
            .font(semi(ty::CAPTION))
            .color(p.muted),
    );
}

/// Tooltip content: a bold title and a short explanation.
type Tip = (&'static str, &'static str);

fn tip_ui(ui: &mut egui::Ui, tip: Tip) {
    tip_ui_owned(ui, tip.0, tip.1);
}

/// [`tip_ui`] for text built at run time.
fn tip_ui_owned(ui: &mut egui::Ui, title: &str, body: &str) {
    ui.set_max_width(size::TOOLTIP_W);
    ui.label(RichText::new(title).font(semi(ty::BODY)));
    ui.label(RichText::new(body).size(ty::CAPTION));
}

trait TipExt {
    /// Explain a control: on hover, and for keyboard users while it has focus.
    fn tip(self, tip: Tip) -> Self;
    /// [`TipExt::tip`] with custom content.
    ///
    /// One tooltip at a time: the last input device decides. After a key press the focused
    /// control shows its tip and hover tips stay hidden; once the pointer moves, hover tips
    /// are back and the focus tip is hidden.
    fn tip_with(self, add: impl FnOnce(&mut egui::Ui)) -> Self;
}

impl TipExt for egui::Response {
    fn tip(self, tip: Tip) -> Self {
        self.tip_with(|ui| tip_ui(ui, tip))
    }

    fn tip_with(self, add: impl FnOnce(&mut egui::Ui)) -> Self {
        if !theme::keyboard_tips(&self.ctx) {
            return self.on_hover_ui(add);
        }
        // Not while a popup (the microphone list) is open, and with an id of its own so it
        // can never collide with that popup's.
        if self.has_focus()
            && theme::keyboard_active(&self.ctx)
            && !egui::Popup::is_any_open(&self.ctx)
        {
            egui::Popup::from_response(&self)
                .kind(egui::PopupKind::Tooltip)
                .id(self.id.with("vidar-focus-tip"))
                .show(add);
        }
        self
    }
}

/// Section title with a "?" that explains the whole group on hover or focus.
fn section_help(ui: &mut egui::Ui, p: &Pal, title: &str, help: Tip) {
    ui.horizontal(|ui| {
        section(ui, p, title);
        let (r, resp) =
            ui.allocate_exact_size(vec2(size::MIN_TARGET, size::MIN_TARGET), Sense::click());
        let c = if resp.hovered() || resp.has_focus() {
            p.text
        } else {
            p.muted
        };
        ui.painter()
            .circle_stroke(r.center(), 7.5, Stroke::new(1.25, c));
        ui.painter().text(
            r.center() + vec2(0.0, 0.5),
            Align2::CENTER_CENTER,
            "?",
            semi(ty::MICRO),
            c,
        );
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, format!("About {title}"))
        });
        focus_ring(ui, p, &resp, radius::LG);
        resp.tip(help);
    });
}

fn pointer_cursor(ui: &egui::Ui, resp: &egui::Response) {
    if resp.hovered() && ui.is_enabled() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

/// Switch. Returns (flipped, response) so callers can attach a tooltip.
/// Off: outlined grey track, thumb on the left. On: green track, thumb on the right.
fn switch_resp(ui: &mut egui::Ui, p: &Pal, on: bool, label: &str) -> (bool, egui::Response) {
    let (rect, resp) = ui.allocate_exact_size(
        vec2(size::SWITCH_W, size::SWITCH_H),
        if ui.is_enabled() {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let t = ui
        .ctx()
        .animate_bool_with_time(resp.id, on, anim(ui.ctx(), motion::BASE));
    let down = resp.is_pointer_button_down_on();
    let track = match (on, down, resp.hovered()) {
        (true, true, _) => p.accent_pressed,
        (true, false, true) => p.accent_hover,
        (true, false, false) => p.accent,
        (false, true, _) => p.muted,
        (false, false, true) => p.muted,
        (false, false, false) => p.ctrl_border,
    };
    let painter = ui.painter();
    painter.rect_filled(rect, rect.height() / 2.0, track);
    let pad = 4.0;
    let r = rect.height() / 2.0 - pad;
    let x = egui::lerp(rect.left() + r + pad..=rect.right() - r - pad, t);
    painter.circle_filled(
        pos2(x, rect.center().y),
        if down { r + 1.0 } else { r },
        p.surface.lerp_to_gamma(p.on_accent, t),
    );
    pointer_cursor(ui, &resp);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, true, on, label));
    focus_ring(ui, p, &resp, (size::SWITCH_H / 2.0) as u8);
    (resp.clicked(), resp)
}

/// A labelled row with a switch on the right. The label is part of the click target.
fn switch_row(ui: &mut egui::Ui, p: &Pal, title: &str, hint: &str, on: bool, tip: Tip) -> bool {
    let mut flipped = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(ui.available_width() - (size::SWITCH_W + space::LG));
            let label = ui
                .add(egui::Label::new(RichText::new(title).color(p.text)).sense(Sense::CLICK))
                .tip(tip);
            flipped |= label.clicked();
            if !hint.is_empty() {
                ui.label(RichText::new(hint).size(ty::CAPTION).color(p.muted));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (clicked, resp) = switch_resp(ui, p, on, title);
            resp.tip(tip);
            flipped |= clicked;
        });
    });
    flipped
}

/// Segmented control (a radio group drawn as a row of buttons). Returns the index clicked,
/// if any. `tips` (one per option, or empty) appear on hover and focus, also when the
/// control is disabled. Selected: raised, green outline. Hover and pressed: neutral wash.
fn segmented(
    ui: &mut egui::Ui,
    p: &Pal,
    options: &[&str],
    selected: Option<usize>,
    enabled: bool,
    tips: &[Tip],
) -> Option<usize> {
    let (h, pad) = (size::SEGMENT_H, size::SEGMENT_PAD);
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(rect, radius::MD, p.surface2);
    painter.rect_stroke(
        rect,
        radius::MD,
        Stroke::new(size::HAIRLINE, p.ctrl_border),
        StrokeKind::Inside,
    );
    let seg = (w - 2.0 * pad) / options.len() as f32;
    let mut clicked = None;
    for (i, label) in options.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(rect.left() + pad + seg * i as f32, rect.top() + pad),
            vec2(seg, h - 2.0 * pad),
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
        let hot = resp.hovered() && enabled;
        if active {
            painter.rect_filled(r, radius::SM, p.surface);
            painter.rect_stroke(
                r,
                radius::SM,
                Stroke::new(size::SELECTED, p.accent),
                StrokeKind::Inside,
            );
        } else if hot {
            let wash = if resp.is_pointer_button_down_on() {
                p.pressed
            } else {
                p.hover
            };
            painter.rect_filled(r, radius::SM, wash);
        }
        let color = if active || hot { p.text } else { p.muted };
        painter.text(
            r.center(),
            Align2::CENTER_CENTER,
            *label,
            if active {
                semi(ty::BODY)
            } else {
                FontId::proportional(ty::BODY)
            },
            color,
        );
        resp.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::RadioButton, enabled, active, *label)
        });
        focus_ring(ui, p, &resp, radius::SM);
        if resp.clicked() && !active {
            clicked = Some(i);
        }
        if enabled {
            pointer_cursor(ui, &resp);
        }
        if let Some(&tip) = tips.get(i) {
            resp.tip(tip);
        }
    }
    clicked
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The one main action of a card or dialog: filled green.
    Primary,
    /// Everything else: outlined neutral.
    Secondary,
    /// A destructive action offered next to others: outlined, red text.
    Danger,
    /// The confirm button of a destructive dialog: filled red.
    DangerSolid,
}

/// Button with every state drawn from the palette: idle, hover, pressed, disabled (when the
/// `ui` is disabled) and keyboard focus. `min_w` of `0.0` hugs the label.
fn button(ui: &mut egui::Ui, p: &Pal, kind: Kind, text: &str, min_w: f32) -> egui::Response {
    button_h(ui, p, kind, text, min_w, None)
}

/// [`button`] with an explicit height, to sit level with a taller neighbour.
fn button_h(
    ui: &mut egui::Ui,
    p: &Pal,
    kind: Kind,
    text: &str,
    min_w: f32,
    height: Option<f32>,
) -> egui::Response {
    let enabled = ui.is_enabled();
    let solid = matches!(kind, Kind::Primary | Kind::DangerSolid);
    let (font, h) = if solid {
        (semi(ty::BODY), size::BUTTON_H)
    } else {
        (FontId::proportional(ty::BODY), size::BUTTON_H_SM)
    };
    let h = height.unwrap_or(h);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);
    let w = (galley.size().x + 2.0 * space::LG).max(min_w);
    let (rect, resp) = ui.allocate_exact_size(
        vec2(w, h),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let (hot, down) = (resp.hovered() && enabled, resp.is_pointer_button_down_on());
    let pick = |rest: Color32, hover: Color32, pressed: Color32| {
        if down {
            pressed
        } else if hot {
            hover
        } else {
            rest
        }
    };
    let (fill, fg, stroke) = if !enabled {
        (p.surface2, p.muted, Stroke::new(size::HAIRLINE, p.border))
    } else {
        match kind {
            Kind::Primary => (
                pick(p.accent, p.accent_hover, p.accent_pressed),
                p.on_accent,
                Stroke::NONE,
            ),
            Kind::DangerSolid => (
                pick(p.danger, p.danger_hover, p.danger_pressed),
                p.on_danger,
                Stroke::NONE,
            ),
            Kind::Secondary | Kind::Danger => (
                pick(p.surface2, p.hover, p.pressed),
                if kind == Kind::Danger {
                    p.danger
                } else {
                    p.text
                },
                Stroke::new(
                    size::HAIRLINE,
                    if hot || down { p.muted } else { p.ctrl_border },
                ),
            ),
        }
    };
    let painter = ui.painter();
    painter.rect_filled(rect, radius::MD, fill);
    if stroke != Stroke::NONE {
        painter.rect_stroke(rect, radius::MD, stroke, StrokeKind::Inside);
    }
    painter.galley(rect.center() - galley.size() / 2.0, galley, fg);
    pointer_cursor(ui, &resp);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, text));
    focus_ring(ui, p, &resp, radius::MD);
    resp
}

/// Primary (filled) button.
fn primary(ui: &mut egui::Ui, p: &Pal, text: &str, width: f32) -> bool {
    button(ui, p, Kind::Primary, text, width).clicked()
}

/// Primary button with a tooltip.
fn primary_tip(ui: &mut egui::Ui, p: &Pal, text: &str, width: f32, tip: Tip) -> bool {
    button(ui, p, Kind::Primary, text, width).tip(tip).clicked()
}

/// Secondary (outlined) button.
fn secondary(ui: &mut egui::Ui, p: &Pal, text: &str) -> egui::Response {
    button(ui, p, Kind::Secondary, text, 0.0)
}

/// A standalone text link with a 24 px high target, underlined on hover and focus.
fn link(ui: &mut egui::Ui, p: &Pal, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        FontId::proportional(ty::CAPTION),
        Color32::PLACEHOLDER,
    );
    let (rect, resp) = ui.allocate_exact_size(
        vec2(galley.size().x, size::MIN_TARGET.max(galley.size().y)),
        Sense::click(),
    );
    let color = if resp.is_pointer_button_down_on() {
        p.text
    } else {
        p.blue
    };
    let at = rect.center() - galley.size() / 2.0;
    if resp.hovered() || resp.has_focus() {
        let y = at.y + galley.size().y;
        ui.painter()
            .hline(at.x..=at.x + galley.size().x, y, Stroke::new(1.0, color));
    }
    ui.painter().galley(at, galley, color);
    pointer_cursor(ui, &resp);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, text));
    focus_ring(ui, p, &resp, radius::SM);
    resp
}

/// A tinted notice: a status icon, then the content. The text stays in the normal text
/// colours (they have the contrast); the tint and the icon carry the tone.
fn banner<R>(ui: &mut egui::Ui, p: &Pal, tone: Tone, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let c = tone_color(p, tone);
    egui::Frame::new()
        .fill(c.gamma_multiply(alpha::TINT))
        .stroke(Stroke::new(size::HAIRLINE, c.gamma_multiply(0.45)))
        .corner_radius(radius::MD)
        .inner_margin(egui::Margin::same(space::SM as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = space::SM;
                tone_icon(ui, p, tone);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = space::XS;
                    add(ui)
                })
                .inner
            })
            .inner
        })
        .inner
}

/// A status icon whose shape, not only its colour, says what it means: ring (idle), arc
/// (busy), tick (good), triangle (warning), cross (problem).
fn tone_icon(ui: &mut egui::Ui, p: &Pal, tone: Tone) {
    let (r, _) = ui.allocate_exact_size(vec2(16.0, 20.0), Sense::hover());
    let c = r.center();
    let color = tone_color(p, tone);
    let painter = ui.painter();
    let mark = |pts: Vec<egui::Pos2>, col: Color32| {
        painter.add(egui::Shape::line(pts, Stroke::new(1.75, col)));
    };
    match tone {
        Tone::Idle => {
            painter.circle_stroke(c, 5.5, Stroke::new(1.75, color));
        }
        Tone::Busy => {
            let pts = (0..=18)
                .map(|k| {
                    let a = k as f32 / 18.0 * std::f32::consts::PI * 1.5;
                    c + vec2(a.cos(), a.sin()) * 6.0
                })
                .collect();
            mark(pts, color);
        }
        Tone::Good => {
            painter.circle_filled(c, 7.0, color);
            mark(
                vec![
                    c + vec2(-3.2, 0.2),
                    c + vec2(-1.0, 2.6),
                    c + vec2(3.4, -2.6),
                ],
                p.on_accent,
            );
        }
        Tone::Warn => {
            painter.add(egui::Shape::convex_polygon(
                vec![c + vec2(0.0, -7.0), c + vec2(7.5, 6.0), c + vec2(-7.5, 6.0)],
                color,
                Stroke::NONE,
            ));
            mark(vec![c + vec2(0.0, -2.5), c + vec2(0.0, 1.5)], p.surface);
            painter.circle_filled(c + vec2(0.0, 4.0), 1.0, p.surface);
        }
        Tone::Bad => {
            painter.circle_filled(c, 7.0, color);
            mark(vec![c + vec2(-2.8, -2.8), c + vec2(2.8, 2.8)], p.on_danger);
            mark(vec![c + vec2(-2.8, 2.8), c + vec2(2.8, -2.8)], p.on_danger);
        }
    }
}

// ---------------------------------------------------------------- explanations

// Writing rules for every tooltip and hint:
// - The tooltip title is the control's name exactly as shown.
// - The body says what it does, then when to change it. Plain words: no "gain",
//   "buffer", "high-pass" or "threshold".
// - The one-line hint under a control has no full stop.
// - Names: "noise cancellation" is the on/off feature, "mode" is Voice / Voice HD /
//   Voice HD+, "Podcast voice" is the
//   style switch in Voice tone, "Vidar AI Microphone" is what apps pick.
// - Numbers match the Sound tab: Voice about 30 ms, Voice HD and Voice HD+ about 60 ms.

const TIP_MIC: Tip = ("Microphone", "The microphone Vidar listens to. \"Windows default microphone\" follows whatever is selected in Windows Sound settings. Bluetooth headsets sound narrower while their microphone is in use.");
const TIP_AUTO_LEVEL: Tip = ("Auto level", "Slowly makes quiet speech louder, by up to 9 dB. It never makes loud speech louder and holds still while you are silent, so background noise is not raised. Turn it on if people say you are too quiet.");
const TIP_MIC_LEVEL: Tip = ("Microphone level", "Your microphone's own volume in Windows, the same as Input volume in Sound settings. It acts before Vidar: raise it if you sound quiet, lower it if your voice distorts. Usually better than Boost, which only makes the cleaned voice louder. Other apps, such as Discord or Teams, may change it too; Vidar shows the current value.");
const TIP_LEVEL: Tip = ("How loud apps hear you", "Your voice after Vidar, as apps receive it. Talk normally for a few seconds. Aim for the green zone: in the grey zone people may struggle to hear you, in the red zone you may sound harsh or distorted. The white mark is where your speech usually sits.");
const TIP_RESET: Tip = (
    "Reset to recommended",
    "Boost off, Auto level off, Rumble filter on.",
);
const TIP_AUTOSTART: Tip = (
    "Start with Windows",
    "Starts Vidar in the tray when you sign in. Noise cancellation stays off until you turn it on.",
);
const CPU_HELP: Tip = ("When the processor is busy", "If your processor cannot keep up with the chosen mode, your voice would crackle or drop out. Vidar can switch to a lighter mode for a while instead, one step at a time: Voice HD+ to Voice HD, then to Voice, and if that is still too much, to the filter and gate alone, without the noise model. It says so on the Home tab, and tries the better mode again after a few minutes.");
const CPU_TIPS: [Tip; 3] = [
    (
        "Keep my mode",
        "Never switch. If the processor cannot keep up, your voice may crackle.",
    ),
    (
        "Down to Voice",
        "Voice HD+ and Voice HD may switch to a lighter mode, but noise removal always stays on.",
    ),
    (
        "As far as needed",
        "One mode lighter at a time, then the filter and gate alone if even Voice is too much. Recommended.",
    ),
];
const TIP_PAUSE: Tip = ("Pause when no app is listening", "While no app records from Vidar AI Microphone, Vidar stops cleaning (the noise model is the part that uses the processor) and starts again the moment an app opens the microphone. Turn it off if an app misses the start of what you say.");
const TIP_RAW: Tip = ("Raw microphone signal", "Gives Vidar your microphone before Windows and the microphone's driver apply their own effects, such as built-in noise suppression or automatic volume. Usually sounds more natural with Vidar; turn it off if your microphone sounds worse. Audio restarts briefly if Vidar is on.");
const TIP_INSTALL: Tip = ("Virtual microphone", "Vidar AI Microphone runs on the VB-CABLE driver by VB-Audio (donationware). Installing it asks Windows for permission once; your speakers and microphone stay as they are.");
const TIP_REINSTALL: Tip = ("Reinstall", "Installs the VB-Audio driver again. Use it if Vidar AI Microphone stopped working, for example after a Windows update.");
const TIP_UNINSTALL: Tip = ("Uninstall", "Turns Vidar off, gives your own microphone back to Windows and removes the VB-Audio driver. Apps that were set to Vidar AI Microphone switch to another microphone. Windows may ask to restart.");
const TIP_LOGS: Tip = (
    "Open logs",
    "Opens the folder with Vidar's log files. Attach the newest one when reporting a problem.",
);
const TIP_RECORD: Tip = ("Start recording", "Records your microphone until you press Stop, up to 20 seconds. Talk normally with your usual background noise. The recording stays in memory and is never saved to disk.");
const TIP_WAVE: Tip = ("Waveform", "Blue: your original recording. On top, the version you are hearing, coloured by how loud apps hear each moment: green is a good level, grey too quiet to hear well (as pauses should be), red too loud. The less of it between your words, the more noise was removed. Click to jump there, drag to loop a part, double-click to clear the loop. With the keyboard, Tab to the waveform, then Left and Right move the playhead.");

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
const TONE_HELP: Tip = ("Voice tone", "Shapes how your voice sounds to others, after the noise is removed. The equalizer corrects what your microphone does to your voice (Automatic learns it, or choose your kind of microphone); Podcast voice makes it close and even, like a podcast host. Fine-tune adds your own curve on top: drag its points, double-click one to reset it. With the keyboard, Tab to a point, then Up and Down change it and Delete resets it.");
const TIP_MIC_CARD: Tip = ("Microphone", "Most people never need to change these. Talk and watch the meter: change something only if it says so. Mode, Boost and Auto level are remembered for each microphone.");
/// Equalizer: Automatic and Off, then the corrections to choose from (keys of
/// [`vidar_dsp::EQ_PRESETS`], in [`vidar_config::MIC_FIXES`]): key, name, what it does.
const FIX_AUTO: (&str, &str, &str) = (
    "auto",
    "Automatic",
    "Learns what your microphone does to your voice and corrects it, like a studio microphone",
);
const FIX_OFF: (&str, &str, &str) = ("off", "Off", "Your microphone as it is, nothing corrected");
const FIX_CHOICES: [(&str, &str, &str); 9] = [
    (
        "laptop",
        "Laptop mic",
        "Less boxy room sound, more presence, a little less hiss",
    ),
    (
        "headset",
        "Headset",
        "Adds the lows a tinny headset misses, softens its harsh upper mids",
    ),
    (
        "close",
        "Close mic",
        "Takes away the boom of talking very close to the microphone",
    ),
    (
        "bright",
        "Dull mic",
        "Opens up a dull or muffled sound: cheap webcams, a mic under clothing",
    ),
    (
        "soft",
        "Harsh mic",
        "Softer S sounds and edge, the voice itself unchanged",
    ),
    ("warm", "Thin mic", "More body, less edge and hiss"),
    ("clear", "Boomy mic", "Less boom and mud, more presence"),
    (
        "nasal",
        "Nasal sound",
        "A focused cut around 1 kHz for a honky or nasal sound",
    ),
    (
        "gaming",
        "Game chat",
        "Cuts through game sound and voice chat: lean lows, strong presence",
    ),
];
const FIX_TIPS: [Tip; 3] = [
    ("Automatic", "Listens to your voice for a few seconds at a time and gently corrects what your microphone does to it: a boomy, boxy, dull or harsh microphone is moved towards the sound of a good studio microphone. Settles within about half a minute of talking, and starts over when chosen again. Adds no delay."),
    ("Choose", "Pick the kind of microphone or problem yourself, if you know it."),
    ("Off", "No equalizer: your microphone as it is."),
];
const TIP_PODCAST: Tip = ("Podcast voice", "Close and even, like a podcast host: a radio-style curve, plus a compressor that brings loud and soft words to the same close level, a de-esser for sharp S sounds and a touch of warmth, at a steady loudness. A style rather than a cleanup: it scores slightly lower than without it on quality measures. Adds no delay.");
/// Each band: its label under the graph, its name, and what it does (for the tooltips).
const TONE_BANDS: [(&str, &str, &str); vidar_dsp::EQ_BANDS] = [
    (
        "Low",
        "Body",
        "100 Hz and below. Up: fuller, deeper. Down: less boom and rumble.",
    ),
    (
        "200",
        "Warmth",
        "Around 200 Hz. Up: warmer, thicker. Down: thinner, less muddy.",
    ),
    (
        "500",
        "Boxiness",
        "Around 500 Hz. Down: less boxy, less 'small room'. Up: fuller midrange.",
    ),
    (
        "1k",
        "Nasal",
        "Around 1 kHz. Down: less honky or nasal. Up: more cutting.",
    ),
    (
        "2.5k",
        "Presence",
        "Around 2.5 kHz. Up: clearer, more forward. Down: softer, less harsh.",
    ),
    (
        "5k",
        "Sibilance",
        "Around 5 kHz. Down: softer S and T sounds. Up: crisper consonants.",
    ),
    (
        "High",
        "Air",
        "8 kHz and above. Up: brighter, more open. Down: less hiss.",
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
    "Every tile is your recording with a different mode. Each shows how much quieter your pauses got, whether your voice was kept, its delay and how much of the processor it uses; ★ marks the best one for you. Click a tile to hear it and use it.",
);
const METRIC_TIPS: [Tip; 4] = [
    ("Noise removed", "How much quieter your pauses became. Every 10 dB sounds about half as loud; 20 dB or more is very clean."),
    ("Voice", "How much your speech level changed. \"Voice intact\" means it stayed within 3 dB; a lower number means Vidar is also taking away some of your voice."),
    ("Delay", "Time from your mouth to the app. Under about 50 ms goes unnoticed in calls and games; Voice HD's 60 ms is fine for calls but may feel late if you hear yourself back."),
    ("Processor use", "The share of this PC's whole processor this mode uses while running, as Task Manager shows it, measured on this PC from your recording with each mode running alone. The AI model runs on a single core; the share of that core shows how much room is left."),
];

fn option_name(o: &crate::worker::Option_) -> String {
    o.mode.to_string()
}

fn is_current(o: &crate::worker::Option_, v: &View) -> bool {
    o.engine == v.engine
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

/// Share of the whole processor (all logical cores, as Task Manager shows it) for a load of
/// `per_core` percent of one core.
fn cpu_whole_text(per_core: f64) -> String {
    let whole = per_core / cpu_count() as f64;
    if whole < 0.1 {
        "<0.1% CPU".into()
    } else if whole < 10.0 {
        format!("~{whole:.1}% CPU")
    } else {
        format!("~{whole:.0}% CPU")
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
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), semi(ty::MICRO), fg);
    let pad = vec2(space::SM - 2.0, 2.0);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + 2.0 * pad, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, radius::SM, bg);
        ui.painter().galley(rect.min + pad, galley, fg);
    }
    resp
}

/// The big round power button: the Vidar logo on a round face. Round the inside of the edge
/// runs a ring of 96 thin digital bars (see [`VoiceRing`]) that grow inward toward the logo.
/// Off, the bars are single grey ticks; on, they take the logo's colours and each one spikes
/// with your voice on its own. Returns true when clicked.
fn power_button(
    ui: &mut egui::Ui,
    p: &Pal,
    state: RunState,
    bands: &[f32; vidar_dsp::BANDS],
    ring: &VoiceRing,
    motion: RingMotion,
) -> bool {
    /// Radius of the button face.
    const FACE: f32 = 92.0;
    /// Room outside the face for the edge ring, the spinner and the focus ring.
    const MARGIN: f32 = 10.0;
    // Margin and face above the centre; below it the face, then the label.
    let (rect, area) = ui.allocate_exact_size(
        vec2(ui.available_width(), 2.0 * FACE + MARGIN + 32.0),
        Sense::hover(),
    );
    let c = pos2(rect.center().x, rect.top() + FACE + MARGIN);
    let resp = ui.interact(
        Rect::from_center_size(c, vec2(2.0 * FACE, 2.0 * FACE)),
        area.id.with("power"),
        Sense::click(),
    );
    // Stopping keeps the "on" look until it has stopped.
    let lit = matches!(state, RunState::Running | RunState::Stopping);
    let busy = matches!(state, RunState::Starting | RunState::Stopping);
    let t = ui
        .ctx()
        .animate_bool_with_time(resp.id, lit, anim(ui.ctx(), motion::SLOW));
    let time = ui.input(|i| i.time) as f32;
    let dark = ui.visuals().dark_mode;
    let painter = ui.painter();

    // The face: idle, hover and pressed washes; the ring carries the on/off colour.
    let face = if busy {
        p.surface
    } else if resp.is_pointer_button_down_on() {
        p.pressed
    } else if resp.hovered() {
        p.surface2
    } else {
        p.surface
    };
    painter.circle_filled(c, FACE, face);
    draw_voice_ring(painter, p, c, FACE, t, dark, ring, motion);

    // Starting turns clockwise, stopping counter-clockwise; a still arc when animations are off.
    let mut outer = FACE + 4.0;
    if busy {
        let dir = if state == RunState::Starting {
            1.0
        } else {
            -1.0
        };
        let (a0, sweep) = if motion.calm {
            (-FRAC_PI_2, PI * 1.5)
        } else {
            (time * 4.0 * dir, PI * 1.5 * 0.6)
        };
        let r = FACE + 8.0;
        let pts: Vec<_> = (0..=32)
            .map(|k| {
                let a = a0 + k as f32 / 32.0 * sweep;
                c + vec2(a.cos(), a.sin()) * r
            })
            .collect();
        painter.add(egui::Shape::line(pts, Stroke::new(5.0, p.blue)));
        outer = r + 2.5;
        if !motion.calm {
            ui.ctx().request_repaint();
        }
    }
    focus_ring_circle(ui, p, &resp, c, outer);

    // The Vidar logo in place of a power glyph.
    let painter = ui.painter();
    draw_logo(
        painter,
        p,
        c,
        52.0,
        t,
        bands,
        (busy && !motion.calm).then_some(time),
        time,
    );
    let label_color = if t > 0.5 && !busy { p.accent } else { p.muted };
    let label = match state {
        RunState::Running => "ON",
        RunState::Stopped => "OFF",
        RunState::Starting => "STARTING",
        RunState::Stopping => "STOPPING",
    };
    painter.text(
        c + vec2(0.0, FACE + 24.0),
        Align2::CENTER_CENTER,
        label,
        semi(ty::CAPTION),
        label_color,
    );
    if resp.hovered() && !busy {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let clicked = resp.clicked();
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Switch, !busy, lit, "Noise cancellation")
    });
    resp.tip(match state {
        RunState::Running => ("Turn noise cancellation off", "Stops Vidar and makes your own microphone the Windows default again. While Vidar is on, the logo and the ring show your cleaned voice live: low tones in the middle of the logo and at the top of the ring, high ones at the edges and the bottom."),
        RunState::Stopped => ("Turn noise cancellation on", "Vidar listens to your microphone, removes the noise and sends your clean voice to Vidar AI Microphone, which apps then use."),
        _ => ("Please wait", "Vidar is switching your microphone."),
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

/// Id of tone graph point `i`. Fixed (not tied to a parent ui) so `test_keys` can tell whether
/// a point has keyboard focus without waiting for the graph to be drawn.
fn tone_point_id(i: usize) -> egui::Id {
    egui::Id::unique(("vidar-tone-point", i))
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
    painter.rect_filled(frame, radius::MD, p.surface2);

    let x_of =
        |f: f64| plot.left() + ((f / F_MIN).ln() / (F_MAX / F_MIN).ln()) as f32 * plot.width();
    let y_of = |db: f64| plot.center().y - (db / view_db) as f32 * plot.height() / 2.0;
    let db_of = |y: f32| {
        (f64::from(plot.center().y - y) / f64::from(plot.height() / 2.0) * view_db)
            .clamp(-max_db, max_db)
    };

    // Which band the pointer is over (or dragging), found before drawing so the column can
    // be highlighted under the curve.
    let ids: [egui::Id; vidar_dsp::EQ_BANDS] = std::array::from_fn(tone_point_id);
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
    let any_focus = resps.iter().any(|r| r.has_focus());
    // The gains as last sent, until the engine's view has caught up with them: key repeats
    // can come faster than that, and each step must start from the latest value.
    let now = ui.input(|i| i.time);
    let pending_id = egui::Id::unique("vidar-tone-pending");
    let unsaved_id = egui::Id::unique("vidar-tone-unsaved");
    let pending: Option<([f64; vidar_dsp::EQ_BANDS], f64)> = ui.data(|d| d.get_temp(pending_id));
    let mut unsaved = ui.data(|d| d.get_temp::<bool>(unsaved_id)).unwrap_or(false);
    let mut cur = match pending {
        Some((g, at)) if g != gains && now - at < 1.0 => g,
        _ => gains,
    };
    let hot: Option<usize> = resps
        .iter()
        .position(|r| r.dragged())
        .or_else(|| resps.iter().position(|r| r.hovered()));

    if let Some(h) = hot {
        let band = Rect::from_center_size(
            pos2(xs[h], frame.center().y),
            vec2(34.0, frame.height() - 8.0),
        );
        painter.rect_filled(band, radius::MD, p.accent.gamma_multiply(0.07));
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
                FontId::proportional(ty::MICRO),
                p.muted,
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
                    FontId::proportional(ty::MICRO),
                    p.muted,
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
        let focused = resp.has_focus() && theme::keyboard_active(ui.ctx());
        let is_hot = hot == Some(i) || focused;
        if resp.has_focus() {
            // Keyboard: Up and Down move the focused point in 0.5 dB steps, Delete resets it.
            // The tone graph owns the vertical arrows while a point has focus.
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    resp.id,
                    egui::EventFilter {
                        vertical_arrows: true,
                        ..Default::default()
                    },
                )
            });
            let (steps, reset, released) = ui.input_mut(|inp| {
                let (mut steps, mut reset, mut released) = (0_i32, false, false);
                for ev in &inp.events {
                    if let egui::Event::Key {
                        key,
                        pressed,
                        modifiers,
                        ..
                    } = ev
                    {
                        match (key, pressed) {
                            (egui::Key::ArrowUp, true) if modifiers.is_none() => steps += 1,
                            (egui::Key::ArrowDown, true) if modifiers.is_none() => steps -= 1,
                            (egui::Key::Delete | egui::Key::Backspace, true) => reset = true,
                            (
                                egui::Key::ArrowUp
                                | egui::Key::ArrowDown
                                | egui::Key::Delete
                                | egui::Key::Backspace,
                                false,
                            ) => released = true,
                            _ => {}
                        }
                    }
                }
                for k in [
                    egui::Key::ArrowUp,
                    egui::Key::ArrowDown,
                    egui::Key::Delete,
                    egui::Key::Backspace,
                ] {
                    inp.consume_key(egui::Modifiers::NONE, k);
                }
                (steps, reset, released)
            });
            let after = if reset {
                0.0
            } else {
                (cur[i] + 0.5 * f64::from(steps)).clamp(-max_db, max_db)
            };
            // Only a real change is sent: while a key repeats, without saving each step;
            // the value is saved once when the key is released.
            if after != cur[i] {
                cur[i] = after;
                out = Some((cur, false));
                unsaved = true;
            }
            if (released || reset) && unsaved {
                out = Some((cur, true));
                unsaved = false;
            }
        }
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(if resp.dragged() {
                egui::CursorIcon::ResizeVertical
            } else {
                egui::CursorIcon::Grab
            });
        }
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let val = (db_of(pos.y) * 2.0).round() / 2.0;
                if val != cur[i] {
                    cur[i] = val;
                    out = Some((cur, false));
                }
            }
        }
        if resp.drag_stopped() {
            out = Some((cur, true));
        }
        if resp.double_clicked() {
            cur[i] = 0.0;
            out = Some((cur, true));
        }

        let g = cur[i];
        let c = pos2(x, y_of(g));
        let r = if is_hot { 7.5 } else { 6.0 };
        let fill = if active { p.accent } else { p.muted };
        painter.circle_filled(c, r + 2.5, p.surface2);
        painter.circle_filled(c, r, fill);
        // The point's own halo sits at r + 2, so the ring lands at r + 5.
        focus_ring_circle(ui, p, &resp, c, r + 2.0);
        painter.circle_filled(
            c,
            r * 0.38,
            p.surface2.gamma_multiply(if active { 0.9 } else { 0.6 }),
        );
        if is_hot {
            painter.circle_stroke(c, r + 4.0, Stroke::new(1.5, p.accent.gamma_multiply(0.45)));
            // Value bubble on the side of the point with more room.
            let text = db_text(g);
            let galley = painter.layout_no_wrap(text, semi(ty::MICRO), p.on_accent);
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
            painter.rect_filled(bubble, radius::SM, if active { p.accent } else { p.muted });
            painter.galley(bubble.center() - galley.size() / 2.0, galley, p.on_accent);
        }

        // Under the plot: where the band is, and its current gain (the points sit close
        // together, so the band's name is in its tooltip).
        let (label, name, what) = TONE_BANDS[i];
        // Only the curve is dimmed while the tone is off; the text stays at full contrast.
        painter.text(
            pos2(x, frame.bottom() + 11.0),
            Align2::CENTER_CENTER,
            label,
            semi(ty::MICRO),
            p.text,
        );
        let value_color = if g == 0.0 || !active {
            p.muted
        } else {
            p.accent
        };
        painter.text(
            pos2(x, frame.bottom() + 26.0),
            Align2::CENTER_CENTER,
            db_short(g),
            FontId::proportional(ty::MICRO),
            value_color,
        );
        if !resp.dragged() {
            resp.tip((name, what));
        }
    }
    // Focus left the graph in the middle of a key edit: save it now.
    if unsaved && !any_focus && out.is_none() {
        out = Some((cur, true));
        unsaved = false;
    }
    ui.data_mut(|d| {
        if let Some((g, _)) = out {
            d.insert_temp(pending_id, (g, now));
        }
        d.insert_temp(unsaved_id, unsaved);
    });
    out
}

/// Gain under a tone graph point, without the unit: "0", "+3.5", "-2".
fn db_short(db: f64) -> String {
    if db == 0.0 {
        "0".into()
    } else if db.fract() == 0.0 {
        format!("{db:+.0}")
    } else {
        format!("{db:+.1}")
    }
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

/// A full-width slider without a number. Rail and handle at 3:1 against the card, with an
/// outlined handle; keyboard focus turns the outline into the focus colour.
fn styled_slider(
    ui: &mut egui::Ui,
    p: &Pal,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().slider_width = ui.available_width();
        let keyboard = theme::keyboard_active(ui.ctx());
        let w = &mut ui.visuals_mut().widgets;
        for state in [&mut w.inactive, &mut w.hovered] {
            state.bg_fill = p.ctrl_border;
            state.weak_bg_fill = p.ctrl_border;
            state.bg_stroke = Stroke::new(size::FOCUS_W, p.text);
        }
        w.active.bg_fill = p.ctrl_border;
        // The focus colour only for keyboard users; a mouse drag keeps the outline.
        w.active.bg_stroke = if keyboard {
            Stroke::new(size::FOCUS_W, p.focus)
        } else {
            Stroke::new(size::FOCUS_W, p.text)
        };
        ui.add(egui::Slider::new(value, range).show_value(false))
    })
    .inner
}

/// Mirrored peak envelope.
/// Speech level zones, dBFS RMS: below QUIET people may struggle to hear you, above LOUD you
/// may sound harsh. The meter and the waveform colours use the same zones.
const LEVEL_QUIET_DB: f32 = -30.0;
const LEVEL_LOUD_DB: f32 = -12.0;

/// The waveform of a cleaned version, each moment coloured by how loud apps hear it: grey
/// below the good zone, the accent colour in it, red above it. `offset_db` moves the levels
/// (a Boost change since the recording was made).
fn draw_wave_levels(
    painter: &egui::Painter,
    rect: Rect,
    peaks: &[f32],
    levels: &[f32],
    offset_db: f32,
    p: &Pal,
) {
    if peaks.is_empty() {
        return;
    }
    let step = rect.width() / peaks.len() as f32;
    let mid = rect.center().y;
    let half = rect.height() / 2.0;
    let gain = 10f32.powf(offset_db / 20.0);
    for (i, &pk) in peaks.iter().enumerate() {
        let x = rect.left() + (i as f32 + 0.5) * step;
        let h = ((pk * gain).min(1.0).sqrt() * half).max(0.75);
        let db = levels.get(i).copied().unwrap_or(-100.0) + offset_db;
        let c = if db > LEVEL_LOUD_DB {
            p.danger
        } else if db >= LEVEL_QUIET_DB {
            p.accent
        } else {
            p.track_idle
        };
        painter.line_segment(
            [pos2(x, mid - h), pos2(x, mid + h)],
            Stroke::new((step * 0.7).max(1.0), c),
        );
    }
}

/// The level over time as a faint area graph (-50 dBFS at the bottom, 0 at the top), each
/// moment in its zone's colour, with the edges of the good zone as dotted lines.
fn draw_level_graph(painter: &egui::Painter, rect: Rect, levels: &[f32], offset_db: f32, p: &Pal) {
    const LOW: f32 = -50.0;
    if levels.is_empty() {
        return;
    }
    let y_of = |db: f32| rect.bottom() - rect.height() * ((db - LOW) / -LOW).clamp(0.0, 1.0);
    let step = rect.width() / levels.len() as f32;
    for (i, &l) in levels.iter().enumerate() {
        let db = l + offset_db;
        if db <= LOW {
            continue;
        }
        let c = if db > LEVEL_LOUD_DB {
            p.danger
        } else if db >= LEVEL_QUIET_DB {
            p.accent
        } else {
            p.track_idle
        };
        let x = rect.left() + i as f32 * step;
        painter.rect_filled(
            Rect::from_x_y_ranges(x..=x + step, y_of(db)..=rect.bottom()),
            0.0,
            c.gamma_multiply(0.14),
        );
    }
    for (db, c) in [(LEVEL_QUIET_DB, p.accent), (LEVEL_LOUD_DB, p.danger)] {
        let y = y_of(db);
        let mut x = rect.left();
        while x < rect.right() {
            painter.line_segment(
                [pos2(x, y), pos2((x + 3.0).min(rect.right()), y)],
                Stroke::new(1.0, c.gamma_multiply(0.5)),
            );
            x += 7.0;
        }
    }
}

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

/// Bars in the voice ring inside the power button.
const RING_BARS: usize = 96;
/// Most LED segments a bar can light.
const RING_SEGMENTS: usize = 9;

/// The digital level ring: every bar samples its own frequency band, picks a random spike
/// about 30 times a second, jumps to it almost at once and falls back more slowly, with a
/// peak marker that hangs and then drops. There is no smoothing between neighbours, so it
/// reads like an LED level meter rather than a wave.
struct VoiceRing {
    /// xorshift32 state; the spikes only need to look random.
    rng: u32,
    /// Seconds since the spikes were last re-rolled.
    clock: f32,
    target: [f32; RING_BARS],
    level: [f32; RING_BARS],
    hold: [f32; RING_BARS],
    /// Average level of all bars, smoothed: how loud the voice is right now (0..1).
    loud: f32,
}

impl VoiceRing {
    fn new() -> Self {
        Self {
            rng: 0x9E37_79B9,
            clock: 0.0,
            target: [0.0; RING_BARS],
            level: [0.0; RING_BARS],
            hold: [0.0; RING_BARS],
            loud: 0.0,
        }
    }

    /// A random number in 0..1.
    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / 16_777_216.0
    }

    /// The voice band bar `k` listens to: low at the top of the ring, high at the bottom,
    /// mirrored left and right.
    fn band_of(k: usize) -> usize {
        let frac = ((k as f32 / RING_BARS as f32 + 0.5).fract() - 0.5).abs() * 2.0;
        ((frac * (vidar_dsp::BANDS - 1) as f32).round() as usize).min(vidar_dsp::BANDS - 1)
    }

    fn update(&mut self, dt: f32, bands: &[f32; vidar_dsp::BANDS]) {
        self.clock += dt;
        if self.clock > 1.0 / 30.0 {
            self.clock = 0.0;
            for k in 0..RING_BARS {
                let spike = self.rand().powf(2.2);
                self.target[k] = (bands[Self::band_of(k)] * (0.25 + 1.6 * spike) * 1.25).min(1.0);
            }
        }
        for k in 0..RING_BARS {
            let tau = if self.target[k] > self.level[k] {
                0.012
            } else {
                0.09
            };
            self.level[k] += (self.target[k] - self.level[k]) * (1.0 - (-dt / tau).exp());
            self.hold[k] = (self.hold[k] - dt * 0.7).max(self.level[k]);
        }
        let mean = self.level.iter().sum::<f32>() / RING_BARS as f32;
        self.loud += (mean - self.loud) * (1.0 - (-dt / 0.12).exp());
    }
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

/// What moves the ring as a whole.
#[derive(Clone, Copy)]
struct RingMotion {
    /// How far the colours have flowed.
    flow: f32,
    /// Clock of the edge's breathing, in turns.
    pulse: f32,
    /// Windows animation effects are off: no colour flow and no breathing edge. The bars and
    /// their peak markers still follow your voice, which is information rather than decoration.
    calm: bool,
}

/// The face's edge and the ring of digital bars just inside it.
///
/// - Edge: a ring stroked in the flowing ring colours; its width (1.5 to 4 px) and, in the
///   dark theme, its brightness breathe with how loud you are.
/// - Bars: thin, stepped like an LED meter (up to nine segments of 3.2 px), starting just
///   inside the edge and growing toward the logo, brighter towards the tip, each with a
///   peak-hold marker. One mesh for everything.
/// - Light theme: solid, darkened neon (and a solid grey when idle) so every mark keeps 3:1
///   against the white face. Dark theme: the neon as it is, fading in toward the tip.
///
/// `on` (0..1) fades from a quiet grey ring (one dim segment per bar) to the coloured one.
#[allow(clippy::too_many_arguments)]
fn draw_voice_ring(
    painter: &egui::Painter,
    p: &Pal,
    c: egui::Pos2,
    face: f32,
    on: f32,
    dark: bool,
    ring: &VoiceRing,
    motion: RingMotion,
) {
    use egui::epaint::Mesh;
    const SEG: f32 = 3.2; // length of one lit segment, px
    const GAP: f32 = 1.4; // dark gap between segments, px
    const EDGE_STEPS: usize = 120; // smoothness of the edge ring
    let dir = |a: f32| vec2(a.cos(), a.sin());
    let neon = |a: f32| {
        let n = theme::neon(ring_hue(a, motion.flow));
        if dark {
            n
        } else {
            theme::neon_on_light(n)
        }
    };

    let mut mesh = Mesh::default();
    mesh.vertices
        .reserve(EDGE_STEPS * 2 + RING_BARS * (RING_SEGMENTS + 1) * 4);

    // Edge: breathes with the voice.
    let breathe = if motion.calm {
        0.0
    } else {
        on * (0.25 + 1.8 * ring.loud).min(1.0) * (0.6 + 0.4 * (motion.pulse * TAU).sin())
    };
    let width = 1.5 + 2.5 * breathe;
    let bright = if dark {
        1.0 - on * (0.4 - 0.4 * breathe)
    } else {
        1.0
    };
    for k in 0..=EDGE_STEPS {
        let a = -FRAC_PI_2 + k as f32 / EDGE_STEPS as f32 * TAU;
        let col = p
            .ctrl_border
            .lerp_to_gamma(neon(a), on)
            .gamma_multiply(bright);
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(c + dir(a) * face, col);
        mesh.colored_vertex(c + dir(a) * (face + width), col);
        if k > 0 {
            let prev = base - 2;
            mesh.add_triangle(prev, prev + 1, base);
            mesh.add_triangle(prev + 1, base + 1, base);
        }
    }

    // Bars, growing inward from just inside the edge.
    let start = face - 5.0;
    let pitch = TAU / RING_BARS as f32;
    let half = pitch * 0.32;
    let block = |mesh: &mut Mesh, d0: egui::Vec2, d1: egui::Vec2, r0: f32, r1: f32, col| {
        let base = mesh.vertices.len() as u32;
        for pt in [c + d0 * r0, c + d1 * r0, c + d1 * r1, c + d0 * r1] {
            mesh.colored_vertex(pt, col);
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    };
    let top = (RING_SEGMENTS - 1) as f32;
    for k in 0..RING_BARS {
        let a = -FRAC_PI_2 + k as f32 * pitch;
        let (d0, d1) = (dir(a - half), dir(a + half));
        let col = p.track_idle.lerp_to_gamma(neon(a), on);
        let lit = 1 + (on * ring.level[k] * top).round() as usize;
        for s in 0..lit {
            let r0 = start - s as f32 * (SEG + GAP);
            let alpha = if !dark {
                1.0
            } else if s == 0 {
                0.75 + 0.15 * on
            } else {
                0.45 + 0.55 * s as f32 / top
            };
            block(&mut mesh, d0, d1, r0, r0 - SEG, col.gamma_multiply(alpha));
        }
        // Peak marker: hangs above the bar, then drops.
        let peak = (on * ring.hold[k] * top).round() as usize;
        if peak >= lit {
            let r0 = start - peak as f32 * (SEG + GAP);
            let alpha = if dark { 0.85 } else { 1.0 };
            block(&mut mesh, d0, d1, r0, r0 - 1.6, col.gamma_multiply(alpha));
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn modal_frame(p: &Pal) -> egui::Frame {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(size::HAIRLINE, p.border))
        .corner_radius(radius::XL)
        .inner_margin(egui::Margin::same(space::XL as i8))
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
    /// Sound tab: the voice tone curve editor and the rarer microphone options, folded away
    /// until asked for.
    curve_open: bool,
    mic_more: bool,
    /// The equalizer correction "Choose" picks when switched to (the last one chosen).
    last_fix: usize,
    /// Microphone level slider (Windows volume, 0..1) while it is dragged, and how many more
    /// seconds the "overloading" warning stays up.
    mic_level: f32,
    mic_drag: bool,
    clip_secs: f32,
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
    /// The digital level ring of the power button.
    ring: VoiceRing,
    /// How far the ring's colours have flowed; advances faster while you talk.
    ring_flow: f32,
    /// Clock of the edge's breathing, in turns; runs faster while you talk.
    ring_pulse: f32,
    /// Windows has animation effects off: stop decorative motion (see `theme::calm`).
    calm: bool,
    /// Keyboard focus was already placed in the open dialog: install done, uninstall, help.
    dialog_focused: [bool; 3],
}

/// Longest voice-test recording.
const MAX_TEST_SECONDS: f64 = 20.0;

/// Media-player time: "0:04.2".
fn fmt_secs(s: f64) -> String {
    let s = s.max(0.0);
    let m = (s / 60.0).floor();
    format!("{m:.0}:{:04.1}", s - m * 60.0)
}

/// Speech level apps would hear, from the test recording: the version selected (or, with the
/// original selected, the one in use), moved by any Boost change since it was made. Auto
/// level and the voice tone are not included.
fn recorded_level(track: Track, v: &View) -> Option<f64> {
    let vars = &v.test.variants;
    let var = match track {
        Track::Variant(i) => vars.get(i),
        Track::Original => None,
    }
    .or_else(|| vars.iter().find(|x| is_current(&x.option, v)))?;
    Some(var.speech_db? + v.mic_boost_db - var.boost_db)
}

/// What a player label names.
#[derive(Clone, Copy)]
enum LabelKind {
    Model,
    Tone,
    Extra,
}

/// Labels for the player: the model heard, the equalizer and any extra effect, e.g.
/// "Voice HD", "Laptop mic", "Podcast voice". The original plays without a tone.
fn selection_labels(track: Track, v: &View) -> Vec<(String, LabelKind)> {
    let Track::Variant(i) = track else {
        return vec![("Original".into(), LabelKind::Extra)];
    };
    let Some(var) = v.test.variants.get(i) else {
        return Vec::new();
    };
    let mut out = vec![(var.option.mode.to_string(), LabelKind::Model)];
    match v.mic_fix.as_str() {
        "auto" => out.push(("Auto tone".into(), LabelKind::Tone)),
        "off" => {}
        key => {
            if let Some((_, name, _)) = FIX_CHOICES.iter().find(|(k, _, _)| *k == key) {
                out.push(((*name).to_string(), LabelKind::Tone));
            }
        }
    }
    if v.studio_sound {
        out.push(("Podcast voice".into(), LabelKind::Extra));
    }
    if v.eq_custom_db.iter().any(|&g| g != 0.0) {
        out.push(("Fine-tuned".into(), LabelKind::Extra));
    }
    out
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
        theme::install(ctx, &v.ui_theme);
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
            curve_open: false,
            mic_more: false,
            last_fix: 0,
            mic_level: 0.0,
            mic_drag: false,
            clip_secs: 0.0,
            level_now_db: -100.0,
            voice_db: None,
            voice_secs: 0.0,
            selected: Track::Original,
            sel: None,
            drag_from: None,
            cursor: 0.0,
            titlebar_dark: None,
            logo_bands: [0.0; vidar_dsp::BANDS],
            ring: VoiceRing::new(),
            ring_flow: 0.0,
            ring_pulse: 0.0,
            calm: theme::calm(ctx),
            dialog_focused: [false; 3],
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        // The window caption already says "Vidar" and the logo is on the power button, so the
        // header is just the tab bar, centred.
        ui.vertical_centered(|ui| {
            // A pill-shaped tab bar: a sunken track with the active tab raised on it.
            const TABS: [(Tab, &str, &str); 4] = [
                (
                    Tab::Home,
                    "Home",
                    "Turn noise cancellation on or off, and pick your microphone",
                ),
                (
                    Tab::Test,
                    "Sound",
                    "How you sound: record yourself and compare every mode, voice tone, and your microphone's level and boost",
                ),
                (
                    Tab::Settings,
                    "Settings",
                    "Appearance, start with Windows, performance, the virtual microphone, troubleshooting",
                ),
                (
                    Tab::About,
                    "About",
                    "Version and updates, licence, and the components Vidar is built on",
                ),
            ];
            let (h, pad) = (size::SEGMENT_H, size::SEGMENT_PAD);
            // 80 px per tab, less when the window is narrow or zoomed in.
            let seg_w = ((ui.available_width() - pad * 2.0) / TABS.len() as f32).clamp(40.0, 80.0);
            let (track, _) = ui.allocate_exact_size(
                vec2(seg_w * TABS.len() as f32 + pad * 2.0, h),
                Sense::hover(),
            );
            let painter = ui.painter().clone();
            let pill = (h / 2.0) as u8;
            painter.rect_filled(track, pill, p.surface2);
            painter.rect_stroke(
                track,
                pill,
                Stroke::new(size::HAIRLINE, p.ctrl_border),
                StrokeKind::Inside,
            );
            let busy_test = matches!(
                v.test.phase,
                TestPhase::Recording | TestPhase::Analyzing | TestPhase::Playing(_)
            );
            for (i, (tab, name, tip)) in TABS.into_iter().enumerate() {
                let r = Rect::from_min_size(
                    pos2(track.left() + pad + seg_w * i as f32, track.top() + pad),
                    vec2(seg_w, h - pad * 2.0),
                );
                let inner_pill = (r.height() / 2.0) as u8;
                let resp = ui.interact(r, ui.scope_id().with(("tab", i)), Sense::click());
                let active = self.tab == tab;
                let hover = ui.ctx().animate_bool_with_time(
                    resp.id,
                    resp.hovered() && !active,
                    anim(ui.ctx(), motion::FAST),
                );
                if active {
                    painter.rect_filled(r, inner_pill, p.surface);
                    painter.rect_stroke(
                        r,
                        inner_pill,
                        Stroke::new(size::SELECTED, p.accent),
                        StrokeKind::Inside,
                    );
                } else if hover > 0.0 {
                    let wash = if resp.is_pointer_button_down_on() {
                        p.pressed
                    } else {
                        p.hover
                    };
                    painter.rect_filled(r, inner_pill, wash.gamma_multiply(hover));
                }
                let color = if active {
                    p.text
                } else {
                    p.muted.lerp_to_gamma(p.text, hover)
                };
                let galley = painter.layout_no_wrap(
                    name.to_string(),
                    if active {
                        semi(ty::BODY)
                    } else {
                        FontId::proportional(ty::BODY)
                    },
                    color,
                );
                let text_pos = r.center() - galley.size() / 2.0;
                let text_right = text_pos.x + galley.size().x;
                painter.galley(text_pos, galley, color);
                // Live dot on Sound while it records, analyses or plays.
                if tab == Tab::Test && busy_test {
                    let dot = if v.test.phase == TestPhase::Recording {
                        p.danger
                    } else {
                        p.blue
                    };
                    painter.circle_filled(pos2(text_right + 7.0, r.center().y - 4.0), 3.5, dot);
                }
                pointer_cursor(ui, &resp);
                resp.widget_info(|| {
                    egui::WidgetInfo::selected(egui::Role::Tab, true, active, name)
                });
                focus_ring(ui, p, &resp, inner_pill);
                if resp.tip((name, tip)).clicked() {
                    self.tab = tab;
                }
            }
        });
    }

    fn status_line(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        banner(ui, p, v.tone, |ui| {
            ui.label(RichText::new(&v.status).color(p.text));
            if let Some(d) = &v.detail {
                let label = if self.show_details {
                    "Hide details"
                } else {
                    "Show details"
                };
                if link(ui, p, label).clicked() {
                    self.show_details = !self.show_details;
                }
                if self.show_details {
                    ui.label(RichText::new(d).small().monospace().color(p.muted));
                }
            }
        });
    }

    // ------------------------------------------------------------ Home

    fn home(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        self.update_notice(ui, p, v);
        if !v.cable_installed && v.install == InstallState::Idle {
            card(ui, p, |ui| {
                ui.label(
                    RichText::new("One-time setup")
                        .font(semi(ty::SUBTITLE))
                        .color(p.text),
                );
                ui.label(RichText::new("Apps get your clean voice from Vidar AI Microphone, which needs the VB-CABLE driver by VB-Audio (donationware).").color(p.muted));
                ui.add_space(space::XS);
                if primary(ui, p, "Install virtual microphone", ui.available_width()) {
                    self.handle.send(Cmd::InstallDriver);
                }
                ui.label(RichText::new("Windows will ask for permission. Your speakers and microphone stay as they are.").small().color(p.muted));
            });
            ui.add_space(space::SM);
        }

        // Read the engine's meters directly so the logo follows the voice frame by frame.
        let live = v
            .live
            .as_ref()
            .filter(|_| v.state == RunState::Running)
            .map(|m| m.snapshot());
        let voice = live.map_or(0.0, |m| m.speech_prob as f32);
        let dt = ui.input(|i| i.stable_dt).min(0.1);
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
        self.ring.update(dt, &raw);
        let voice = ui
            .ctx()
            .animate_value_with_time(ui.scope_id().with("voice"), voice, 0.15);
        // The colours drift slowly in silence and stream faster while you talk. Integrated
        // frame by frame, so a change of speed never makes them jump.
        let talking = voice.max(1.5 * self.ring.loud).min(1.0);
        if !self.calm {
            self.ring_flow += dt * (0.4 + 2.6 * talking);
            self.ring_pulse += dt * (0.35 + 0.65 * talking);
        }
        let motion = RingMotion {
            flow: self.ring_flow,
            pulse: self.ring_pulse,
            calm: self.calm,
        };
        if power_button(ui, p, v.state, &self.logo_bands, &self.ring, motion) {
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
            RunState::Running if v.idle => (
                "Noise cancellation is on".to_string(),
                format!("Paused until an app uses \"{}\"", short_cable_name(v)),
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
            ui.label(RichText::new(headline).font(semi(ty::TITLE)).color(p.text));
            if !sub.is_empty() {
                ui.label(RichText::new(sub).color(p.muted));
            }
        });
        if matches!(v.tone, Tone::Warn | Tone::Bad) {
            ui.add_space(space::XS);
            self.status_line(ui, p, v);
        }
        ui.add_space(space::MD);

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
                    .tip(TIP_MIC);
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

            if let Some(note) = &v.cpu_note {
                ui.add_space(space::SM);
                ui.label(RichText::new(note).small().color(p.warn));
            }
        });
    }

    // ------------------------------------------------------------ Test

    /// The pinned part of the Sound tab: record button, waveform and player. It stays in view
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
                        .font(semi(ty::SUBTITLE))
                        .color(p.text),
                );
                ui.label(
                    RichText::new("Record yourself talking with your usual background noise. Vidar runs that recording through every mode so you can listen and compare.")
                        .color(p.muted),
                );
                ui.add_space(space::SM);
            }

            // Start / stop.
            let width = ui.available_width();
            if recording || analyzing {
                ui.horizontal(|ui| {
                    let cancel_w = 84.0; // wide enough for "Cancel"
                    let main_w = width - cancel_w - ui.spacing().item_spacing.x;
                    if recording {
                        let enough = t.progress * t.max_seconds >= 2.0;
                        let r = ui
                            .add_enabled_ui(enough, |ui| {
                                button(ui, p, Kind::DangerSolid, "Stop and compare", main_w)
                            })
                            .inner;
                        if r.tip((
                            "Stop and compare",
                            "Ends the recording here and runs it through every mode.",
                        ))
                        .on_disabled_hover_ui(|ui| {
                            tip_ui(
                                ui,
                                (
                                    "Stop and compare",
                                    "Keep talking: at least 2 seconds are needed.",
                                ),
                            )
                        })
                        .clicked()
                        {
                            self.handle.send(Cmd::FinishTest);
                        }
                    } else {
                        let label = format!(
                            "Comparing {} of {}...",
                            ((t.progress * TEST_OPTIONS.len() as f64) as usize + 1)
                                .min(TEST_OPTIONS.len()),
                            TEST_OPTIONS.len()
                        );
                        ui.add_enabled_ui(false, |ui| {
                            button(ui, p, Kind::Primary, &label, main_w);
                        });
                    }
                    if button_h(
                        ui,
                        p,
                        Kind::Secondary,
                        "Cancel",
                        cancel_w,
                        Some(size::BUTTON_H),
                    )
                    .tip(("Cancel", "Throws this recording away."))
                    .clicked()
                    {
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
            ui.add_space(space::SM);

            // The waveform, with the live level meter on its right.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::XS;
                self.waveform(ui, p, v, width - 10.0 - space::XS);
                self.level_meter_vertical(ui, p, v, 110.0);
            });

            if ready {
                ui.add_space(space::XS);
                self.transport(ui, p, v, playing);
            }
            if let Some(e) = &t.error {
                ui.label(RichText::new(e).color(p.danger));
            }
        });
    }

    /// The scrolling part of the Sound tab: the comparison, voice tone to shape the result
    /// while listening, then the microphone's own settings.
    fn test_list(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        let t = &v.test;
        let playing = matches!(t.phase, TestPhase::Playing(_));
        let ready = !t.variants.is_empty()
            && t.phase != TestPhase::Recording
            && t.phase != TestPhase::Analyzing;
        if !ready {
            ui.label(RichText::new(format!("Up to {MAX_TEST_SECONDS:.0} seconds. Plays on your speakers; nothing is saved to disk")).small().color(p.muted));
            ui.add_space(space::SM);
            self.tone_card(ui, p, v);
            ui.add_space(space::SM);
            self.microphone_card(ui, p, v);
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
                banner(ui, p, Tone::Warn, |ui| {
                    ui.label(
                        RichText::new("No speech found in this recording")
                            .font(semi(ty::BODY))
                            .color(p.text),
                    );
                    ui.label(RichText::new("You can still hear how each mode removes your background noise. To compare how your voice sounds, record again and talk normally.").small().color(p.muted));
                });
                ui.add_space(space::SM);
            }
            self.compare_grid(ui, p, v, playing, best);
        });
        ui.add_space(space::SM);
        ui.label(
            RichText::new(
                "Click a tile to hear it and use it. Space plays or stops, Up and Down switch versions",
            )
            .small()
            .color(p.muted),
        );
        ui.add_space(space::SM);
        self.tone_card(ui, p, v);
        ui.add_space(space::SM);
        self.microphone_card(ui, p, v);
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

    /// A dialog is open on top of the window.
    fn dialog_open(&self, v: &View) -> bool {
        self.confirm_uninstall
            || self.show_help
            || matches!(v.install, InstallState::Running | InstallState::Done(_))
    }

    /// Space = play / stop, Up / Down = previous / next version.
    fn test_keys(&mut self, ui: &mut egui::Ui, v: &View) {
        let playing = matches!(v.test.phase, TestPhase::Playing(_));
        // Space always plays or stops here, before a focused control can take it as a click,
        // except where it is needed: typing in a text field, an open list, a dialog.
        let ctx = ui.ctx().clone();
        let free =
            !ctx.text_edit_focused() && !egui::Popup::is_any_open(&ctx) && !self.dialog_open(v);
        // The tone graph uses Up and Down for its focused point.
        let tone_focus =
            ui.memory(|m| (0..vidar_dsp::EQ_BANDS).any(|i| m.has_focus(tone_point_id(i))));
        let (space, up, down) = ui.input_mut(|i| {
            if !free {
                return (false, false, false);
            }
            // A held key repeats; only the first press counts for Space.
            let space = i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Space,
                        pressed: true,
                        repeat: false,
                        ..
                    }
                )
            });
            i.consume_key(egui::Modifiers::NONE, egui::Key::Space);
            (
                space,
                !tone_focus && i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                !tone_focus && i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
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
                // As with a click: the version chosen last is the setting.
                if let Track::Variant(i) = self.selected {
                    if v.test
                        .variants
                        .get(i)
                        .is_some_and(|x| !is_current(&x.option, v))
                    {
                        self.handle.send(Cmd::ApplyOption(i));
                    }
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
        painter.rect_filled(rect, radius::MD, p.surface2);
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
                semi(ty::CAPTION),
                p.text,
            );
            painter.text(
                rect.right_top() + vec2(-10.0, 12.0),
                Align2::RIGHT_CENTER,
                format!("max {}", fmt_secs(t.max_seconds)),
                FontId::proportional(ty::MICRO),
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
                semi(ty::CAPTION),
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
                FontId::proportional(ty::BODY),
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
        // Keyboard: Left and Right move the playhead by 2 %. The waveform owns the horizontal
        // arrows while it has focus.
        if resp.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    resp.id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        ..Default::default()
                    },
                )
            });
            let step = ui.input_mut(|i| {
                let right = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight);
                let left = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft);
                0.02 * (f64::from(u8::from(right)) - f64::from(u8::from(left)))
            });
            if step != 0.0 {
                let at = if playing { t.progress } else { self.cursor };
                let to = (at + step).clamp(0.0, 1.0);
                self.cursor = to;
                if playing {
                    self.play_from(to);
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
            painter.rect_filled(r, radius::SM, p.blue.gamma_multiply(0.16));
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
                FontId::proportional(ty::MICRO),
                p.blue,
            );
        }
        // With Auto level on, the level over time behind the waveform, in the meter's colours,
        // with the good zone's edges as guide lines: you see Auto level lift the quiet parts.
        if let (true, Track::Variant(i)) = (v.auto_level, self.selected) {
            if let Some(var) = t.variants.get(i) {
                let offset = (v.mic_boost_db - var.boost_db) as f32;
                draw_level_graph(&painter, inner, &var.levels_db, offset, p);
            }
        }
        match self.selected {
            Track::Original => draw_wave(&painter, inner, orig, p.blue),
            Track::Variant(i) => {
                draw_wave(&painter, inner, orig, p.blue.gamma_multiply(0.30));
                if let Some(var) = t.variants.get(i) {
                    let offset = (v.mic_boost_db - var.boost_db) as f32;
                    draw_wave_levels(&painter, inner, &var.peaks, &var.levels_db, offset, p);
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
        // How loud apps hear your speech in this version, in the bottom corner, in the
        // waveform's colours.
        if let Some(d) = recorded_level(self.selected, v).filter(|_| ready) {
            let d = d as f32;
            let (word, c) = if d > LEVEL_LOUD_DB {
                ("Level: too loud", p.danger)
            } else if d >= LEVEL_QUIET_DB {
                ("Level: good", p.accent)
            } else {
                ("Level: quiet", p.warn)
            };
            let galley = painter.layout_no_wrap(word.to_string(), semi(ty::MICRO), c);
            let pad = vec2(space::SM - 2.0, 1.0);
            let size = galley.size() + 2.0 * pad;
            let chip = Rect::from_min_size(
                pos2(rect.right() - 8.0 - size.x, rect.bottom() - 3.0 - size.y),
                size,
            );
            painter.rect_filled(chip, radius::SM, p.surface2);
            painter.rect_filled(chip, radius::SM, c.gamma_multiply(alpha::TINT));
            painter.galley(chip.min + pad, galley, c);
        }
        // What is heard, as labels in the top corner: the model, the equalizer and any
        // extra effect.
        if ready {
            let mut x = rect.left() + 8.0;
            for (text, kind) in selection_labels(self.selected, v) {
                let c = match kind {
                    LabelKind::Model => p.accent,
                    LabelKind::Tone => p.blue,
                    LabelKind::Extra => p.muted,
                };
                let galley = painter.layout_no_wrap(text, semi(ty::MICRO), c);
                let pad = vec2(space::SM - 2.0, 1.0);
                let chip =
                    Rect::from_min_size(pos2(x, rect.top() + 3.0), galley.size() + 2.0 * pad);
                if chip.right() > rect.right() - 8.0 {
                    break;
                }
                painter.rect_filled(chip, radius::SM, p.surface2);
                painter.rect_filled(chip, radius::SM, c.gamma_multiply(alpha::TINT));
                painter.galley(chip.min + pad, galley, c);
                x = chip.right() + space::XS;
            }
        }
        focus_ring(ui, p, &resp, radius::MD);
        resp.tip(TIP_WAVE);
    }

    /// Play / stop, the time, and the loop with a way to clear it.
    fn transport(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View, playing: bool) {
        let t = &v.test;
        ui.horizontal(|ui| {
            let (r, resp) = ui.allocate_exact_size(
                vec2(size::BUTTON_H, size::BUTTON_H),
                Sense::click(),
            );
            let painter = ui.painter();
            let hovered = resp.hovered();
            let fill = if resp.is_pointer_button_down_on() {
                p.accent_pressed
            } else if hovered {
                p.accent_hover
            } else {
                p.accent
            };
            painter.circle_filled(r.center(), size::BUTTON_H / 2.0, fill);
            focus_ring_circle(ui, p, &resp, r.center(), size::BUTTON_H / 2.0);
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Button,
                    true,
                    if playing { "Stop" } else { "Play" },
                )
            });
            if playing {
                painter.rect_filled(Rect::from_center_size(r.center(), vec2(10.0, 10.0)), 2.0, p.on_accent);
            } else {
                let c = r.center() + vec2(1.5, 0.0);
                painter.add(egui::Shape::convex_polygon(vec![c + vec2(-4.5, -7.0), c + vec2(7.0, 0.0), c + vec2(-4.5, 7.0)], p.on_accent, Stroke::NONE));
            }
            if hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.tip(if playing { ("Stop", "Stops playback. Shortcut: Space.") } else { ("Play", "Plays the selected version. Shortcut: Space.") }).clicked() {
                if playing {
                    self.stop_playback(v);
                } else {
                    let start = self.sel.map_or(self.cursor, |(a, b)| if (a..b).contains(&self.cursor) { self.cursor } else { a });
                    self.play_from(start);
                }
            }
            let head = if playing { t.progress } else { self.cursor };
            // Current time bright, total muted, in the UI font.
            ui.add_space(space::XS);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(RichText::new(fmt_secs(head * t.seconds)).font(semi(ty::BODY)).color(p.text));
                ui.label(RichText::new(format!("  /  {}", fmt_secs(t.seconds))).font(FontId::proportional(ty::BODY)).color(p.muted));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.sel.is_some()
                    && link(ui, p, "Clear loop").tip(("Clear loop", "Plays the whole recording again. Double-clicking the waveform does the same.")).clicked() {
                        self.sel = None;
                        if playing {
                            self.play_from(t.progress);
                        }
                    }
            });
        });
    }

    /// The comparison: the original, then one tile per mode. Clicking a tile plays that
    /// version and makes its mode the setting.
    fn compare_grid(
        &mut self,
        ui: &mut egui::Ui,
        p: &Pal,
        v: &View,
        playing: bool,
        best: Option<usize>,
    ) {
        let t = &v.test;
        const GAP: f32 = 6.0;
        const TILE_H: f32 = 98.0;

        // Original.
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
        let selected = self.selected == Track::Original;
        let painter = ui.painter().clone();
        painter.rect_filled(
            r,
            radius::MD,
            if selected {
                p.blue.gamma_multiply(alpha::SELECTED)
            } else if resp.is_pointer_button_down_on() {
                p.pressed
            } else if resp.hovered() {
                p.hover
            } else {
                p.surface2.gamma_multiply(alpha::REST)
            },
        );
        painter.rect_stroke(
            r,
            radius::MD,
            if selected {
                Stroke::new(size::SELECTED, p.blue)
            } else {
                Stroke::new(size::HAIRLINE, p.ctrl_border)
            },
            StrokeKind::Inside,
        );
        resp.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, "Original")
        });
        focus_ring(ui, p, &resp, radius::MD);
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
            semi(ty::BODY),
            p.text,
        );
        painter.text(
            pos2(r.right() - 12.0, r.center().y),
            Align2::RIGHT_CENTER,
            "your microphone, before Vidar",
            FontId::proportional(ty::CAPTION),
            p.muted,
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp
            .tip((
                "Original",
                "Your recording as the microphone delivered it, without noise removal or voice tone.",
            ))
            .clicked()
            && !(selected && playing)
        {
            self.choose(Track::Original, playing);
        }
        ui.add_space(space::MD);

        // The modes side by side, one tile each.
        let w = ui.available_width();
        let n = t.variants.len().max(1);
        let tile_w = (w - GAP * (n as f32 - 1.0)) / n as f32;
        let (row, _) = ui.allocate_exact_size(vec2(w, TILE_H), Sense::hover());
        for i in 0..t.variants.len() {
            let r = Rect::from_min_size(
                pos2(row.left() + i as f32 * (tile_w + GAP), row.top()),
                vec2(tile_w, TILE_H),
            );
            self.compare_tile(ui, p, v, r, i, playing, best == Some(i));
        }
    }

    /// One tile in the comparison: the mode, noise removed, the voice verdict, its delay and
    /// processor use.
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
            p.accent.gamma_multiply(alpha::SELECTED)
        } else if hovered && resp.is_pointer_button_down_on() {
            p.pressed
        } else if hovered {
            p.hover
        } else {
            p.surface2.gamma_multiply(alpha::REST)
        };
        painter.rect_filled(r, radius::MD, fill);
        let stroke = if selected {
            Stroke::new(size::SELECTED, p.accent)
        } else {
            Stroke::new(size::HAIRLINE, p.ctrl_border)
        };
        painter.rect_stroke(r, radius::MD, stroke, StrokeKind::Inside);
        if available {
            resp.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::Role::RadioButton,
                    true,
                    selected,
                    option_name(&var.option),
                )
            });
            focus_ring(ui, p, &resp, radius::MD);
        }

        painter.text(
            pos2(r.center().x, r.top() + 16.0),
            Align2::CENTER_CENTER,
            var.option.mode,
            semi(ty::BODY),
            p.text,
        );
        if !available {
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                "not available",
                FontId::proportional(ty::MICRO),
                p.muted,
            );
            let why = var.unavailable.clone().unwrap_or_default();
            resp.tip_with(|ui| tip_ui_owned(ui, "Not available", &why));
            return;
        }

        let headline = if var.noise_removed_db >= 60.0 {
            "-60+ dB".to_string()
        } else {
            format!("-{:.0} dB", var.noise_removed_db)
        };
        painter.text(
            pos2(r.center().x, r.top() + 40.0),
            Align2::CENTER_CENTER,
            headline,
            semi(if r.width() < 64.0 {
                ty::BODY
            } else {
                ty::SUBTITLE
            }),
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
            pos2(r.center().x, r.top() + 59.0),
            Align2::CENTER_CENTER,
            verdict,
            FontId::proportional(ty::MICRO),
            vcolor,
        );
        for (line, y) in [
            (format!("{:.0} ms", var.delay_ms), 75.0),
            (cpu_whole_text(var.cpu_percent), 87.0),
        ] {
            painter.text(
                pos2(r.center().x, r.top() + y),
                Align2::CENTER_CENTER,
                line,
                FontId::proportional(ty::MICRO),
                p.muted,
            );
        }

        // Corners: playing / selected on the left, best on the right.
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
                FontId::proportional(ty::MICRO),
                p.on_accent,
            );
        }

        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let speech = t.speech_detected;
        let resp = resp.tip_with(|ui| {
            ui.set_max_width(280.0);
            ui.label(RichText::new(option_name(&var.option)).font(semi(ty::BODY)));
            if best {
                ui.label(
                    RichText::new("★ Best for you: removes the most noise without making your voice noticeably quieter.")
                        .size(ty::CAPTION)
                        .color(p.accent),
                );
            }
            ui.add_space(space::XS);
            ui.label(
                RichText::new(format!("Noise removed: {:.0} dB", var.noise_removed_db)).size(ty::CAPTION),
            );
            ui.label(RichText::new(METRIC_TIPS[0].1).size(ty::CAPTION).color(p.muted));
            if speech {
                ui.add_space(space::XXS);
                ui.label(
                    RichText::new(format!("Voice: {:+.1} dB", var.voice_change_db)).size(ty::CAPTION),
                );
                ui.label(RichText::new(METRIC_TIPS[1].1).size(ty::CAPTION).color(p.muted));
            }
            ui.add_space(space::XXS);
            ui.label(
                RichText::new(format!("Delay: about {:.0} ms", var.delay_ms)).size(ty::CAPTION),
            );
            ui.label(RichText::new(METRIC_TIPS[2].1).size(ty::CAPTION).color(p.muted));
            ui.add_space(space::XXS);
            ui.label(
                RichText::new(format!(
                    "Processor: {} ({} on this PC's {} logical cores)",
                    cpu_whole_text(var.cpu_percent),
                    cpu_text(var.cpu_percent),
                    cpu_count()
                ))
                .size(ty::CAPTION),
            );
            ui.label(RichText::new(METRIC_TIPS[3].1).size(ty::CAPTION).color(p.muted));
        });
        if resp.clicked() {
            // Choosing a tile is choosing that mode; it also plays it.
            if !is_current(&var.option, v) {
                self.handle.send(Cmd::ApplyOption(i));
            }
            if !(selected && playing) {
                self.choose(track, playing);
            }
        }
    }

    /// The voice tone card. It sits on the Sound tab so a tone can be tried on your recording:
    /// changes are heard right away, and they also apply to noise cancellation.
    fn tone_card(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        card(ui, p, |ui| {
            section_help(ui, p, "Voice tone", TONE_HELP);

            ui.label(
                RichText::new("Equalizer")
                    .font(semi(ty::BODY))
                    .color(p.text),
            );
            let chosen = FIX_CHOICES.iter().position(|(k, _, _)| *k == v.mic_fix);
            let mode = match v.mic_fix.as_str() {
                "auto" => 0,
                "off" => 2,
                _ => 1,
            };
            if let Some(i) = segmented(
                ui,
                p,
                &["Automatic", "Choose", "Off"],
                Some(mode),
                true,
                &FIX_TIPS,
            ) {
                let fix = match i {
                    0 => FIX_AUTO.0,
                    2 => FIX_OFF.0,
                    _ => FIX_CHOICES[chosen.unwrap_or(self.last_fix)].0,
                };
                self.handle.send(Cmd::SetMicFix(fix.to_string()));
            }
            if let Some(i) = chosen {
                self.last_fix = i;
                ui.add_space(space::XXS);
                egui::ComboBox::from_id_salt("mic_fix")
                    .width(ui.available_width())
                    .selected_text(FIX_CHOICES[i].1)
                    .show_ui(ui, |ui| {
                        for (j, (key, name, what)) in FIX_CHOICES.iter().enumerate() {
                            if ui
                                .selectable_label(i == j, *name)
                                .on_hover_text(*what)
                                .clicked()
                            {
                                self.handle.send(Cmd::SetMicFix((*key).to_string()));
                            }
                        }
                    });
            }
            let what = match mode {
                0 => FIX_AUTO.2,
                2 => FIX_OFF.2,
                _ => chosen.map_or("", |i| FIX_CHOICES[i].2),
            };
            ui.label(RichText::new(what).small().color(p.muted));
            ui.add_space(space::SM);

            if switch_row(
                ui,
                p,
                "Podcast voice",
                "Close and even, like a podcast host",
                v.studio_sound,
                TIP_PODCAST,
            ) {
                self.handle.send(Cmd::SetPodcast(!v.studio_sound));
            }
            ui.add_space(space::SM);

            // Your own curve, added on top.
            let tuned = v.eq_custom_db.iter().any(|&g| g != 0.0);
            ui.horizontal(|ui| {
                let label = match (self.curve_open, tuned) {
                    (true, _) => "Hide fine-tune",
                    (false, true) => "Fine-tune (changed)",
                    (false, false) => "Fine-tune",
                };
                if link(ui, p, label).clicked() {
                    self.curve_open = !self.curve_open;
                }
                if tuned {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if link(ui, p, "Reset fine-tune").clicked() {
                            self.handle.send(Cmd::SetEqCustom {
                                gains_db: [0.0; vidar_dsp::EQ_BANDS],
                                save: true,
                            });
                        }
                    });
                }
            });
            if self.curve_open {
                ui.add_space(space::XS);
                if let Some((g, done)) = tone_curve(ui, p, v.eq_custom_db, true) {
                    self.handle.send(Cmd::SetEqCustom {
                        gains_db: g,
                        save: done,
                    });
                }
            }
            if v.test.variants.is_empty() {
                ui.add_space(space::XXS);
                ui.label(
                    RichText::new("Record yourself above to hear each change as you make it")
                        .small()
                        .color(p.muted),
                );
            }
        });
    }

    // ------------------------------------------------------------ Settings

    /// "How loud apps hear you": a live bar with quiet / good / loud zones and one line of
    /// advice that names the control to change.
    /// Returns true when its advice names Boost, Auto level or Microphone level, so the card
    /// can show those controls.
    /// Follow the live level apps hear, once per frame. While Vidar is paused (no app
    /// listening) apps hear nothing, so it is estimated from the raw microphone and the gain
    /// Vidar would add; noise removal leaves speech at about the same level. Returns `None`
    /// when noise cancellation is off, else whether the level is such an estimate.
    fn update_live_level(&mut self, ui: &egui::Ui, v: &View) -> Option<bool> {
        let live = v
            .live
            .as_ref()
            .filter(|_| v.state == RunState::Running)
            .map(|m| m.snapshot());
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        // The gain Vidar adds after the model (output gain and Boost), for the estimates.
        let gain_db = 20.0 * 1.08f64.log10() + v.mic_boost_db;
        // While a test recording runs it measures the raw microphone itself, whether or not
        // noise cancellation is on: follow that.
        let recording = v.test.phase == TestPhase::Recording;
        let (now, talking, estimated) = if recording {
            let now = (v.test.level_db + gain_db) as f32;
            (now, now > -45.0, true)
        } else if let Some(m) = live {
            if v.idle {
                let now = (m.in_rms_db + gain_db) as f32;
                (now, now > -45.0, true)
            } else {
                let now = m.out_rms_db as f32;
                (now, m.speech_prob > 0.6 && now > -70.0, false)
            }
        } else {
            self.level_now_db = -100.0;
            self.voice_db = None;
            self.voice_secs = 0.0;
            return None;
        };
        let tau = if now > self.level_now_db { 0.03 } else { 0.25 };
        self.level_now_db += (now - self.level_now_db) * (1.0 - (-dt / tau).exp());
        // Average only while you talk, so pauses do not drag the verdict down.
        if talking {
            let k = 1.0 - (-dt / 2.0).exp();
            self.voice_db = Some(self.voice_db.map_or(now, |d| d + (now - d) * k));
            self.voice_secs += dt;
        }
        Some(estimated)
    }

    /// The live level as a slim vertical meter beside the waveform: grey, green and red zones
    /// from the bottom, the level now as a fill, and where your speech usually sits as a tick.
    fn level_meter_vertical(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View, height: f32) {
        const LOW: f32 = -50.0;
        let state = self.update_live_level(ui, v);
        let (rect, resp) = ui.allocate_exact_size(vec2(10.0, height), Sense::hover());
        let y_of = |db: f32| rect.bottom() - rect.height() * ((db - LOW) / -LOW).clamp(0.0, 1.0);
        let painter = ui.painter();
        painter.rect_filled(rect, 5.0, p.surface2);
        for (a, b, c) in [
            (LOW, LEVEL_QUIET_DB, p.track_idle),
            (LEVEL_QUIET_DB, LEVEL_LOUD_DB, p.accent),
            (LEVEL_LOUD_DB, 0.0, p.danger),
        ] {
            let r = Rect::from_x_y_ranges(rect.x_range(), y_of(b)..=y_of(a));
            painter.rect_filled(r, 2.0, c.gamma_multiply(0.22));
        }
        if state.is_some() && self.level_now_db > LOW {
            let c = if self.level_now_db >= LEVEL_LOUD_DB {
                p.danger
            } else if self.level_now_db >= LEVEL_QUIET_DB {
                p.accent
            } else {
                p.track_idle
            };
            let fill =
                Rect::from_x_y_ranges(rect.x_range(), y_of(self.level_now_db)..=rect.bottom());
            painter.rect_filled(fill, 5.0, c);
        }
        if let Some(d) = self.voice_db.filter(|_| state.is_some()) {
            let y = y_of(d);
            painter.line_segment(
                [pos2(rect.left() - 3.0, y), pos2(rect.right() + 3.0, y)],
                Stroke::new(2.0, p.text),
            );
        }
        let now = match state {
            None => "Turn noise cancellation on to see your level live.".to_string(),
            Some(true) if v.test.phase == TestPhase::Recording => {
                "Estimated from the recording: the microphone plus the gain Vidar adds.".to_string()
            }
            Some(true) => "Estimated while Vidar is paused (no app listening).".to_string(),
            Some(false) => "Live: what apps hear now.".to_string(),
        };
        resp.tip_with(|ui| {
            tip_ui(ui, TIP_LEVEL);
            ui.label(RichText::new(now).size(ty::CAPTION).color(p.muted));
        });
    }

    fn loudness_meter(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) -> bool {
        // The level itself is followed by the meter beside the waveform; this is its advice.
        let running = v.live.is_some() && v.state == RunState::Running;
        let live = running.then_some(());
        // Live while an app listens; otherwise from your test recording, if there is one:
        // the version you hear or use, moved by any Boost change since it was made.
        // While paused the level is an estimate, but still live.
        let live_now = live.is_some();
        let recorded = if live_now {
            None
        } else {
            recorded_level(self.selected, v)
        };
        ui.label(RichText::new("How loud apps hear you").color(p.text))
            .tip(TIP_LEVEL);
        let heard = if live_now {
            self.voice_db
                .filter(|_| self.voice_secs >= 2.0)
                .map(f64::from)
        } else {
            recorded
        };
        let from = if live_now && v.idle {
            "Estimated (no app listening): "
        } else if live_now {
            ""
        } else {
            "From your recording (colours in the waveform): "
        };
        let advice = |d: f64| -> (&'static str, Color32) {
            if d < f64::from(LEVEL_QUIET_DB) {
                (
                    if !v.auto_level {
                        "a bit quiet. Turn on Auto level below."
                    } else if v.mic_boost_db < 14.5 {
                        "still quiet. Raise Boost one step."
                    } else {
                        "still quiet. Raise Microphone level below, or move closer."
                    },
                    p.warn,
                )
            } else if d > f64::from(LEVEL_LOUD_DB) {
                (
                    if v.mic_boost_db > 0.5 {
                        "too loud. Lower Boost one step."
                    } else {
                        "too loud. Lower Microphone level below, or move back a little."
                    },
                    p.danger,
                )
            } else {
                ("good level. Nothing to change.", p.accent)
            }
        };
        let (text, color) = match heard {
            Some(d) => {
                let (a, c) = advice(d);
                let line = if from.is_empty() {
                    let mut t = a.to_string();
                    t[..1].make_ascii_uppercase();
                    t
                } else {
                    format!("{from}{a}")
                };
                (line, c)
            }
            None if live_now => ("Talk normally for a few seconds...".to_string(), p.muted),
            None if live.is_some() => (
                "Paused while no app uses Vidar AI Microphone. Open your call or recorder and talk, or record yourself above.".to_string(),
                p.muted,
            ),
            None => (
                "Record yourself above, or turn noise cancellation on and talk, to see your level.".to_string(),
                p.muted,
            ),
        };
        ui.label(RichText::new(text).small().color(color));
        matches!(heard, Some(d) if d < f64::from(LEVEL_QUIET_DB) || d > f64::from(LEVEL_LOUD_DB))
    }

    /// The microphone's own volume in Windows ("Input volume" in Sound settings), with a
    /// warning when Windows has it muted and when the raw signal clips.
    fn mic_level(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        let Some(vol) = v.mic_volume else {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Microphone level").color(p.text))
                    .tip(TIP_MIC_LEVEL);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new("Not adjustable for this microphone")
                            .small()
                            .color(p.muted),
                    );
                });
            });
            return;
        };
        if !self.mic_drag {
            self.mic_level = vol.level;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Microphone level").color(p.text))
                .tip(TIP_MIC_LEVEL);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{:.0}%", self.mic_level * 100.0))
                        .font(semi(ty::BODY))
                        .color(p.text),
                );
            });
        });
        let r = styled_slider(ui, p, &mut self.mic_level, 0.0..=1.0).tip(TIP_MIC_LEVEL);
        self.mic_drag = r.dragged();
        if r.changed() {
            self.handle.send(Cmd::SetMicVolume(self.mic_level));
        }

        // Clipping: the microphone's own signal, before Vidar, hit full scale. Shown for a
        // few seconds after the last clip so a single shout does not flicker past.
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        let peak = v
            .live
            .as_ref()
            .filter(|_| v.state == RunState::Running)
            .map_or(-100.0, |m| m.snapshot().in_peak_db);
        self.clip_secs = if peak >= -0.5 {
            4.0
        } else {
            (self.clip_secs - dt).max(0.0)
        };

        if vol.muted {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Muted in Windows: Vidar hears nothing.")
                        .small()
                        .color(p.danger),
                );
                if link(ui, p, "Unmute").clicked() {
                    self.handle.send(Cmd::SetMicMuted(false));
                }
            });
        } else if self.clip_secs > 0.0 {
            ui.label(
                RichText::new("Your microphone is overloading. Lower Microphone level.")
                    .small()
                    .color(p.danger),
            );
        } else {
            ui.label(
                RichText::new("The same as Input volume in Windows Sound settings")
                    .small()
                    .color(p.muted),
            );
        }
    }

    /// The microphone's own settings: what apps hear, its level, Boost and Auto level. On
    /// the Sound tab, under the recording, so a change can be checked by ear.
    fn microphone_card(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        card(ui, p, |ui| {
            section_help(ui, p, "Microphone", TIP_MIC_CARD);
            // When the meter's advice names one of the folded controls, show them.
            let advised = self.loudness_meter(ui, p, v);
            ui.add_space(space::XS);

            // Level, Boost and Auto level: needed only when the meter says so.
            let mut changed = Vec::new();
            if v.mic_boost_db.abs() >= 0.5 {
                changed.push(format!("Boost +{:.0} dB", v.mic_boost_db));
            }
            if v.auto_level {
                changed.push("Auto level on".to_string());
            }
            let label = if self.mic_more {
                "Fewer microphone options".to_string()
            } else if changed.is_empty() {
                "More microphone options".to_string()
            } else {
                format!("More microphone options ({})", changed.join(", "))
            };
            if !advised && link(ui, p, &label).clicked() {
                self.mic_more = !self.mic_more;
            }
            if self.mic_more || advised {
                ui.add_space(space::XS);
                self.mic_level(ui, p, v);
                ui.add_space(space::SM);
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
                ui.add_space(space::XS);
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
            }

            let recommended = v.mic_boost_db.abs() < 0.5
                && !v.auto_level
                && (v.vad_threshold - 0.70).abs() < 0.005;
            if !recommended {
                ui.add_space(space::XXS);
                if link(ui, p, "Reset to recommended").tip(TIP_RESET).clicked() {
                    self.handle.send(Cmd::SetBoost(0.0));
                    if v.auto_level {
                        self.handle.send(Cmd::SetAutoLevel(false));
                    }
                    self.handle.send(Cmd::SetVad(0.70));
                }
            }
        });
    }

    fn settings(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        card(ui, p, |ui| {
            section(ui, p, "General");
            ui.label(RichText::new("Appearance").color(p.text));
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
                theme::apply_theme(ui.ctx(), &t);
                self.theme = t.clone();
                self.handle.send(Cmd::SetTheme(t));
            }
            ui.add_space(space::SM);
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
            ui.add_space(space::XXS);
            if link(ui, p, "How to use Vidar in a specific app").clicked() {
                self.show_help = true;
            }
        });
        ui.add_space(space::SM);

        card(ui, p, |ui| {
            section(ui, p, "Performance");
            if switch_row(
                ui,
                p,
                "Pause when no app is listening",
                "Saves processor time and battery",
                v.pause_when_unused,
                TIP_PAUSE,
            ) {
                self.handle
                    .send(Cmd::SetPauseWhenUnused(!v.pause_when_unused));
            }
            ui.add_space(space::XS);
            section_help(ui, p, "When the processor is busy", CPU_HELP);
            const LIMITS: [&str; 3] = ["off", "voice", "any"];
            let sel = LIMITS.iter().position(|l| *l == v.cpu_fallback);
            if let Some(i) = segmented(
                ui,
                p,
                &["Keep my mode", "Down to Voice", "As far as needed"],
                sel,
                true,
                &CPU_TIPS,
            ) {
                self.handle.send(Cmd::SetCpuFallback(LIMITS[i].into()));
            }
        });
        ui.add_space(space::SM);

        card(ui, p, |ui| {
            section_help(ui, p, "Virtual microphone", TIP_INSTALL);
            ui.horizontal(|ui| {
                let (tone, text) = if v.cable_installed {
                    (
                        Tone::Good,
                        format!("Installed: apps see \"{}\"", short_cable_name(v)),
                    )
                } else {
                    (
                        Tone::Warn,
                        "Not installed: apps cannot get your clean voice yet".to_string(),
                    )
                };
                tone_icon(ui, p, tone);
                ui.label(RichText::new(text).color(p.text));
            });
            ui.label(
                RichText::new("Runs on VB-CABLE by VB-Audio, donationware (see About)")
                    .small()
                    .color(p.muted),
            );
            ui.add_space(space::XXS);
            ui.horizontal(|ui| {
                if v.cable_installed {
                    if secondary(ui, p, "Reinstall").tip(TIP_REINSTALL).clicked() {
                        self.handle.send(Cmd::InstallDriver);
                    }
                    if button(ui, p, Kind::Danger, "Uninstall", 0.0)
                        .tip(TIP_UNINSTALL)
                        .clicked()
                    {
                        self.confirm_uninstall = true;
                    }
                } else if primary_tip(ui, p, "Install virtual microphone", 220.0, TIP_INSTALL) {
                    self.handle.send(Cmd::InstallDriver);
                }
            });
        });
        ui.add_space(space::SM);

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
            ui.add_space(space::XS);
            ui.horizontal(|ui| {
                if secondary(ui, p, "Open logs").tip(TIP_LOGS).clicked() {
                    let _ = std::process::Command::new("explorer")
                        .arg(crate::logging::log_dir())
                        .spawn();
                }
            });
            if v.tone != Tone::Idle {
                ui.add_space(space::XS);
                self.status_line(ui, p, v);
            }
        });
    }

    fn about(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        // Header: the logo in its "on" colours, the name and the version.
        ui.add_space(space::SM);
        ui.vertical_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(96.0, 64.0), Sense::hover());
            draw_logo_static(ui.painter(), r.center(), 40.0);
            ui.add_space(space::XXS);
            ui.label(RichText::new("Vidar").font(semi(ty::DISPLAY)).color(p.text));
            ui.add_space(space::XXS);
            badge(
                ui,
                p.accent.gamma_multiply(alpha::TINT),
                p.accent,
                concat!("Version ", env!("CARGO_PKG_VERSION")),
            );
            ui.add_space(space::SM);
            ui.label(RichText::new("AI noise cancellation for your microphone").color(p.muted));
        });
        ui.add_space(space::LG);

        self.updates_card(ui, p, v);
        ui.add_space(space::SM);

        // VB-Audio's distribution terms ask that users can identify VB-CABLE, see where it comes
        // from and that it is donationware: https://vb-audio.com/Services/licensing.htm
        card(ui, p, |ui| {
            section(ui, p, "Virtual microphone");
            ui.add_space(space::XS);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("VB-CABLE")
                        .font(semi(ty::SUBTITLE))
                        .color(p.text),
                );
                badge(
                    ui,
                    p.blue.gamma_multiply(alpha::TINT),
                    p.blue,
                    "Donationware",
                );
            });
            ui.label(RichText::new("by VB-Audio Software").small().color(p.muted));
            ui.add_space(space::SM);
            ui.label(
                RichText::new(
                    "Vidar AI Microphone and Vidar Internal are VB-CABLE devices under Vidar's names. All participations are welcome.",
                )
                .color(p.text),
            );
            ui.add_space(space::MD);
            if primary(ui, p, "Support VB-Audio", 150.0) {
                open_url(VB_CABLE_URL);
            }
            ui.add_space(space::SM);
            // The origin VB-Audio's distribution terms ask us to name, as text.
            ui.label(
                RichText::new(format!(
                    "From {VB_CABLE_SITE}. Companies and organisations need a VB-CABLE licence from VB-Audio."
                ))
                .small()
                .color(p.muted),
            );
        });
        ui.add_space(space::SM);

        card(ui, p, |ui| {
            section(ui, p, "Noise models");
            ui.add_space(space::XS);
            for (i, &(name, by, licence, url)) in CREDITS.iter().enumerate() {
                if i > 0 {
                    let y = ui.cursor().top();
                    let x = ui.max_rect().x_range();
                    ui.painter()
                        .hline(x, y, Stroke::new(size::HAIRLINE, p.border));
                }
                credit_row(ui, p, name, by, licence, url);
            }
        });
        ui.add_space(space::MD);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("Vidar is MIT licensed. Other components and their licences: THIRD_PARTY_NOTICES.md")
                    .small()
                    .color(p.muted),
            );
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context, p: &Pal, v: &View) {
        // Each dialog puts keyboard focus on its main button once, when it opens.
        let done_open = matches!(v.install, InstallState::Done(_));
        for (flag, open) in
            self.dialog_focused
                .iter_mut()
                .zip([done_open, self.confirm_uninstall, self.show_help])
        {
            if !open {
                *flag = false;
            }
        }
        if matches!(v.install, InstallState::Running | InstallState::Done(_)) {
            egui::Modal::new(egui::Id::unique("install")).frame(modal_frame(p)).show(ctx, |ui| {
                ui.set_width(size::DIALOG_W);
                ui.label(RichText::new("Virtual microphone").font(semi(ty::SUBTITLE)).color(p.text));
                ui.add_space(space::SM);
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
                    ui.add_space(space::SM);
                    let done = button(ui, p, Kind::Primary, "Done", 100.0);
                    if !self.dialog_focused[0] {
                        done.request_focus();
                        self.dialog_focused[0] = true;
                    }
                    if done.clicked() {
                        self.handle.send(Cmd::DismissInstall);
                    }
                }
            });
        }
        if self.confirm_uninstall {
            let r = egui::Modal::new(egui::Id::unique("uninstall")).frame(modal_frame(p)).show(ctx, |ui| {
                ui.set_width(size::DIALOG_W);
                ui.label(RichText::new("Remove the virtual microphone?").font(semi(ty::SUBTITLE)).color(p.text));
                ui.add_space(space::SM);
                ui.label("Vidar turns off and your own microphone becomes the Windows default again. Apps can no longer get your clean voice until you install it again.");
                ui.label(RichText::new("Windows will ask for permission and may want a restart.").small().color(p.muted));
                ui.add_space(space::SM);
                let mut answer = None;
                ui.horizontal(|ui| {
                    if button(ui, p, Kind::DangerSolid, "Remove", 100.0).clicked() {
                        answer = Some(true);
                    }
                    // A destructive question starts on the safe answer.
                    let keep = button(ui, p, Kind::Secondary, "Keep it", 0.0);
                    if !self.dialog_focused[1] {
                        keep.request_focus();
                        self.dialog_focused[1] = true;
                    }
                    if keep.clicked() {
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
                ui.set_width(size::DIALOG_W);
                ui.label(RichText::new("Using Vidar in apps").font(semi(ty::SUBTITLE)).color(p.text));
                ui.add_space(space::SM);
                ui.label("With \"Use Vidar AI Microphone as the default\" on, apps that use the Windows default microphone switch automatically.");
                ui.add_space(space::XS);
                ui.label(RichText::new("If an app keeps its old microphone:").color(p.text));
                ui.label(RichText::new(format!("open its audio settings and choose \"{}\".", short_cable_name(v))).color(p.accent));
                ui.label(RichText::new("Keep your normal speakers or headphones for playback.").small().color(p.muted));
                ui.add_space(space::SM);
                let got_it = button(ui, p, Kind::Primary, "Got it", 100.0);
                if !self.dialog_focused[2] {
                    got_it.request_focus();
                    self.dialog_focused[2] = true;
                }
                got_it.clicked()
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
                    ui.label(RichText::new(name).font(semi(ty::BODY)).color(p.text));
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
    let hover =
        ui.ctx()
            .animate_bool_with_time(resp.id, resp.hovered(), anim(ui.ctx(), motion::FAST));
    focus_ring(ui, p, &resp, radius::MD);
    if hover > 0.0 {
        ui.painter().set(
            bg,
            egui::Shape::rect_filled(
                inner.response.rect,
                radius::MD,
                p.surface2.gamma_multiply(hover),
            ),
        );
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp
        .tip_with(|ui| {
            ui.set_max_width(size::TOOLTIP_W);
            ui.label(RichText::new(url).size(ty::CAPTION));
        })
        .clicked()
    {
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
        theme::track_input_modality(ctx);
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
        // The Sound tab shows the live loudness meter, so it animates too.
        if matches!(self.tab, Tab::Home | Tab::Test)
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
            .inner_margin(egui::Margin::symmetric(space::LG as i8, space::MD as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(ui.available_height());
                self.header(ui, &p, &v);
                ui.add_space(space::SM);
                if self.tab == Tab::Test {
                    self.test_top(ui, &p, &v);
                    ui.add_space(space::SM);
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        match self.tab {
                            Tab::Home => self.home(ui, &p, &v),
                            Tab::Test => self.test_list(ui, &p, &v),
                            Tab::Settings => self.settings(ui, &p, &v),
                            Tab::About => self.about(ui, &p, &v),
                        }
                        ui.add_space(space::SM);
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

// ------------------------------------------------------------ Updates
// One block: the Home notice and the Settings card.

const TIP_UPDATES: Tip = ("Check for updates", "When it starts and every 8 hours after that, Vidar asks GitHub for the latest version number. No personal data is sent. Installed copies can update themselves; the portable zip opens the download page.");

impl VidarWindow {
    /// Home: a short notice while a newer version is available.
    fn update_notice(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        let Some((version, page)) = v.update.available.clone() else {
            return;
        };
        card(ui, p, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("Vidar {version} is available"))
                        .font(semi(14.0))
                        .color(p.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.update_action(ui, p, v, &page);
                });
            });
            update_error(ui, p, v);
        });
        ui.add_space(8.0);
    }

    /// "Update now" for installed copies, "Download" for the portable zip, or progress.
    fn update_action(&self, ui: &mut egui::Ui, p: &Pal, v: &View, page: &str) {
        match v.update.phase {
            UpdatePhase::Downloading(f) => {
                ui.label(
                    RichText::new(format!("Downloading... {:.0}%", f * 100.0))
                        .small()
                        .color(p.muted),
                );
            }
            UpdatePhase::Launching => {
                ui.label(
                    RichText::new("Starting the installer...")
                        .small()
                        .color(p.muted),
                );
            }
            _ if v.update.installed => {
                if primary(ui, p, "Update now", 110.0) {
                    self.handle.send(Cmd::InstallUpdate);
                }
            }
            _ => {
                if secondary(ui, p, "Download").clicked() {
                    open_url(page);
                }
            }
        }
    }

    /// Settings: the automatic check, "Check now" and what the last check found.
    fn updates_card(&mut self, ui: &mut egui::Ui, p: &Pal, v: &View) {
        card(ui, p, |ui| {
            section(ui, p, "Updates");
            if switch_row(
                ui,
                p,
                "Check for updates",
                "When Vidar starts and every 8 hours, from GitHub",
                v.update.enabled,
                TIP_UPDATES,
            ) {
                self.handle.send(Cmd::SetCheckUpdates(!v.update.enabled));
            }
            ui.add_space(4.0);
            // The version itself is in the About header, right above this card.
            ui.horizontal(|ui| {
                let (text, color) = match (&v.update.available, v.update.phase) {
                    (_, UpdatePhase::Checking) => ("Checking...".to_string(), p.muted),
                    (Some((ver, _)), _) => (format!("Vidar {ver} is available"), p.accent),
                    (None, _) if v.update.up_to_date => {
                        ("You have the latest version".to_string(), p.text)
                    }
                    (None, _) => ("Not checked yet".to_string(), p.muted),
                };
                ui.label(RichText::new(text).color(color));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some((_, page)) = &v.update.available {
                        self.update_action(ui, p, v, page);
                    } else {
                        let busy = v.update.phase != UpdatePhase::Idle;
                        if ui
                            .add_enabled_ui(!busy, |ui| secondary(ui, p, "Check now"))
                            .inner
                            .clicked()
                        {
                            self.handle.send(Cmd::CheckUpdates);
                        }
                    }
                });
            });
            update_error(ui, p, v);
        });
    }
}

fn update_error(ui: &mut egui::Ui, p: &Pal, v: &View) {
    if let Some(e) = &v.update.error {
        ui.label(RichText::new(e).small().color(p.danger));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every equalizer choice names a real preset, every preset is offered (Broadcast through
    /// Podcast voice), and the list matches the settings file's.
    #[test]
    fn equalizer_choices_cover_the_presets() {
        let fixes: Vec<&str> = FIX_CHOICES.iter().map(|c| c.0).collect();
        assert_eq!(fixes, vidar_config::MIC_FIXES);
        for key in &fixes {
            assert!(vidar_dsp::eq_preset(key).is_some(), "{key} has no preset");
        }
        for (key, _) in vidar_dsp::EQ_PRESETS {
            assert!(
                matches!(key, "off" | "broadcast") || fixes.contains(&key),
                "preset {key} is offered nowhere"
            );
        }
    }
}
