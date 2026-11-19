//! Design tokens and egui styling for the Vidar window.
//!
//! Everything visual that is a *decision* lives here: the spacing scale, corner radii, type
//! scale, control sizes, motion and the light and dark colour palettes. `ui.rs` only composes
//! them, so a change of look is a change to this file.
//!
//! Guidelines followed:
//! - WCAG 2.2 AA: text 4.5:1 (1.4.3), UI components, focus indicators and graphics 3:1
//!   (1.4.11, 2.4.13), targets at least 24 px (2.5.8), state never by colour alone (1.4.1).
//!   The `tests` module checks every pairing below for both themes, so a palette edit that
//!   breaks contrast fails `cargo test`.
//! - Windows 11 / Fluent conventions: follow the OS light/dark setting, 4 px spacing grid,
//!   8 px control radius and 12 px card radius, Segoe-like sizes (12/14/16/20), a 2 px
//!   focus ring that appears for keyboard users only.
//! - Reduced motion: when Windows has "Animation effects" off, decorative motion stops.

use std::sync::Arc;

use eframe::egui::{
    self, vec2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke,
    StrokeKind,
};

// ---------------------------------------------------------------- scales

/// Spacing scale (px), a 4 px grid with a half step.
pub mod space {
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
}

/// Corner radii (px). Nested shapes use the outer radius minus the padding between them.
pub mod radius {
    /// Inner segment of a segmented control, small chips.
    pub const SM: u8 = 6;
    /// Buttons, inputs, tiles, graphs.
    pub const MD: u8 = 8;
    /// Cards.
    pub const LG: u8 = 12;
    /// Dialogs.
    pub const XL: u8 = 16;
}

/// Type scale (px). Nothing in the window is set smaller than `MICRO`, which is for dense
/// graph labels only; running text is `CAPTION` or larger.
pub mod ty {
    pub const MICRO: f32 = 11.0;
    pub const CAPTION: f32 = 12.0;
    pub const BODY: f32 = 14.0;
    pub const SUBTITLE: f32 = 16.0;
    pub const TITLE: f32 = 18.0;
    pub const DISPLAY: f32 = 22.0;
}

/// Control geometry (px).
pub mod size {
    /// Primary action button height.
    pub const BUTTON_H: f32 = 36.0;
    /// Secondary / inline button height.
    pub const BUTTON_H_SM: f32 = 32.0;
    pub const SEGMENT_H: f32 = 32.0;
    /// Padding between a segmented control's track and its segments.
    pub const SEGMENT_PAD: f32 = 3.0;
    pub const SWITCH_W: f32 = 40.0;
    pub const SWITCH_H: f32 = 24.0;
    /// Minimum pointer target (WCAG 2.5.8).
    pub const MIN_TARGET: f32 = 24.0;
    pub const TOOLTIP_W: f32 = 260.0;
    pub const DIALOG_W: f32 = 340.0;
    /// Hairline for card edges and dividers.
    pub const HAIRLINE: f32 = 1.0;
    /// Outline of a selected item.
    pub const SELECTED: f32 = 1.5;
    /// Keyboard focus ring thickness and its gap from the control.
    pub const FOCUS_W: f32 = 2.0;
    pub const FOCUS_GAP: f32 = 2.0;
}

/// Opacity steps for tints and hover washes.
pub mod alpha {
    /// Badge / banner background tint of a status colour.
    pub const TINT: f32 = 0.12;
    /// Background of a selected tile.
    pub const SELECTED: f32 = 0.14;
    /// Resting tile background (over `surface2`).
    pub const REST: f32 = 0.55;
}

/// Seconds. Zero when the OS asks for reduced motion.
pub mod motion {
    pub const FAST: f32 = 0.10;
    pub const BASE: f32 = 0.12;
    pub const SLOW: f32 = 0.40;
}

// Scales stay ordered and every control stays at least a minimum target high (checked at
// compile time).
const _: () = {
    assert!(space::XXS < space::XS && space::XS < space::SM && space::SM < space::MD);
    assert!(space::MD < space::LG && space::LG < space::XL);
    assert!(radius::SM < radius::MD && radius::MD < radius::LG && radius::LG < radius::XL);
    assert!(ty::MICRO < ty::CAPTION && ty::CAPTION < ty::BODY && ty::BODY < ty::SUBTITLE);
    assert!(size::SWITCH_H >= size::MIN_TARGET && size::SEGMENT_H >= size::MIN_TARGET);
    assert!(size::BUTTON_H_SM >= size::MIN_TARGET);
};

// ---------------------------------------------------------------- palette

/// One colour theme. Every colour that is used for text or a control has been checked against
/// the surfaces it sits on (see the tests).
#[derive(Clone, Copy)]
pub struct Pal {
    /// Window background.
    pub bg: Color32,
    /// Cards and dialogs.
    pub surface: Color32,
    /// Inputs, tracks, tiles: one step above `surface`.
    pub surface2: Color32,
    /// Decorative hairlines (card edges, dividers). Not the only cue for a control.
    pub border: Color32,
    /// Outline of an interactive control, at least 3:1 against the surface it sits on.
    pub ctrl_border: Color32,
    /// Neutral control fill while hovered / pressed.
    pub hover: Color32,
    pub pressed: Color32,
    pub text: Color32,
    /// Secondary text; 4.5:1 on every surface.
    pub muted: Color32,
    /// Brand green: fills, selection, "on" and "good" states, and text on any surface.
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_pressed: Color32,
    pub on_accent: Color32,
    /// Informational / busy / recording-adjacent blue.
    pub blue: Color32,
    pub warn: Color32,
    pub danger: Color32,
    pub danger_hover: Color32,
    pub danger_pressed: Color32,
    pub on_danger: Color32,
    /// Neutral graphics: idle waveform, quiet zone of the level meter (3:1 on `surface2`).
    pub track_idle: Color32,
    /// Keyboard focus ring (3:1 on every surface).
    pub focus: Color32,
}

pub const DARK: Pal = Pal {
    bg: Color32::from_rgb(14, 17, 22),
    surface: Color32::from_rgb(22, 27, 34),
    surface2: Color32::from_rgb(31, 38, 48),
    border: Color32::from_rgb(42, 51, 64),
    ctrl_border: Color32::from_rgb(115, 129, 148),
    hover: Color32::from_rgb(45, 54, 66),
    pressed: Color32::from_rgb(38, 46, 58),
    text: Color32::from_rgb(230, 237, 243),
    muted: Color32::from_rgb(160, 173, 189),
    accent: Color32::from_rgb(45, 215, 115),
    accent_hover: Color32::from_rgb(88, 228, 144),
    accent_pressed: Color32::from_rgb(34, 188, 98),
    on_accent: Color32::from_rgb(6, 24, 14),
    blue: Color32::from_rgb(96, 165, 255),
    warn: Color32::from_rgb(245, 185, 60),
    danger: Color32::from_rgb(255, 120, 116),
    danger_hover: Color32::from_rgb(255, 138, 135),
    danger_pressed: Color32::from_rgb(232, 88, 85),
    on_danger: Color32::from_rgb(30, 8, 8),
    track_idle: Color32::from_rgb(120, 134, 152),
    focus: Color32::from_rgb(125, 185, 255),
};

pub const LIGHT: Pal = Pal {
    bg: Color32::from_rgb(237, 240, 244),
    surface: Color32::from_rgb(255, 255, 255),
    surface2: Color32::from_rgb(229, 233, 239),
    border: Color32::from_rgb(206, 212, 221),
    ctrl_border: Color32::from_rgb(112, 122, 137),
    hover: Color32::from_rgb(219, 225, 233),
    pressed: Color32::from_rgb(205, 212, 222),
    text: Color32::from_rgb(17, 24, 39),
    muted: Color32::from_rgb(79, 90, 105),
    accent: Color32::from_rgb(8, 106, 48),
    accent_hover: Color32::from_rgb(6, 90, 40),
    accent_pressed: Color32::from_rgb(4, 74, 33),
    on_accent: Color32::from_rgb(255, 255, 255),
    blue: Color32::from_rgb(29, 78, 200),
    warn: Color32::from_rgb(122, 70, 0),
    danger: Color32::from_rgb(168, 22, 22),
    danger_hover: Color32::from_rgb(146, 16, 16),
    danger_pressed: Color32::from_rgb(124, 12, 12),
    on_danger: Color32::from_rgb(255, 255, 255),
    track_idle: Color32::from_rgb(112, 122, 137),
    focus: Color32::from_rgb(0, 90, 200),
};

pub fn pal(ui: &egui::Ui) -> Pal {
    pal_for(ui.visuals().dark_mode)
}

pub fn pal_for(dark: bool) -> Pal {
    if dark {
        DARK
    } else {
        LIGHT
    }
}

/// What the engine's status tone looks like.
pub fn tone_color(p: &Pal, t: crate::worker::Tone) -> Color32 {
    use crate::worker::Tone;
    match t {
        Tone::Idle => p.muted,
        Tone::Busy => p.blue,
        Tone::Good => p.accent,
        Tone::Warn => p.warn,
        Tone::Bad => p.danger,
    }
}

// ---------------------------------------------------------------- egui style

fn visuals(p: &Pal, dark: bool) -> egui::Visuals {
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(size::HAIRLINE, p.border);
    v.window_corner_radius = CornerRadius::same(radius::XL);
    v.menu_corner_radius = CornerRadius::same(radius::MD);
    v.extreme_bg_color = p.surface2;
    v.faint_bg_color = p.surface2;
    v.override_text_color = Some(p.text);
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(size::HAIRLINE, p.on_accent);
    v.hyperlink_color = p.blue;
    v.slider_trailing_fill = true;

    let r = CornerRadius::same(radius::MD);
    let w = &mut v.widgets;
    // Static surfaces.
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(size::HAIRLINE, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    // Idle: outlined so the control is visible without relying on fill contrast.
    w.inactive.bg_fill = p.surface2;
    w.inactive.weak_bg_fill = p.surface2;
    w.inactive.bg_stroke = Stroke::new(size::HAIRLINE, p.ctrl_border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;
    // Hover: lighter wash, stronger outline.
    w.hovered.bg_fill = p.hover;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_stroke = Stroke::new(size::HAIRLINE, p.muted);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = r;
    // Pressed and keyboard-focused: darker wash. The stroke is the focus colour only after a
    // key press (see `track_input_modality`); a mouse press gets a plain outline.
    w.active.bg_fill = p.pressed;
    w.active.weak_bg_fill = p.pressed;
    w.active.bg_stroke = pressed_stroke(dark, false);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.corner_radius = r;
    // Open (combo box with its list showing).
    w.open.bg_fill = p.hover;
    w.open.weak_bg_fill = p.hover;
    w.open.bg_stroke = Stroke::new(size::HAIRLINE, p.muted);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    w.open.corner_radius = r;
    v
}

/// Outline of egui's built-in widgets (combo box, slider) in their "active" state, which egui
/// uses for both a pressed and a keyboard-focused widget: the focus colour for keyboard users
/// only.
fn pressed_stroke(dark: bool, keyboard: bool) -> Stroke {
    let p = pal_for(dark);
    if keyboard {
        Stroke::new(size::FOCUS_W, p.focus)
    } else {
        Stroke::new(size::HAIRLINE, p.muted)
    }
}

/// Load the fonts, install both palettes and apply the theme preference ("auto" follows the
/// Windows app theme).
pub fn install(ctx: &egui::Context, theme: &str) {
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
    let reduce = reduce_motion();
    ctx.all_styles_mut(|s| {
        s.visuals.widgets.active.bg_stroke = pressed_stroke(s.visuals.dark_mode, false);
        s.spacing.item_spacing = vec2(space::SM, space::SM);
        s.spacing.button_padding = vec2(space::MD, 6.0);
        s.spacing.interact_size.y = 20.0;
        s.spacing.slider_rail_height = 6.0;
        s.interaction.tooltip_delay = 0.4;
        if reduce {
            s.animation_time = 0.0;
        }
        use egui::TextStyle as T;
        for (style, id) in [
            (T::Small, FontId::proportional(ty::CAPTION)),
            (T::Body, FontId::proportional(ty::BODY)),
            (T::Button, FontId::proportional(ty::BODY)),
            (T::Heading, semi(ty::DISPLAY)),
            (T::Monospace, FontId::monospace(13.0)),
        ] {
            s.text_styles.insert(style, id);
        }
    });
    apply_theme(ctx, theme);
}

pub fn apply_theme(ctx: &egui::Context, theme: &str) {
    ctx.set_theme(match theme {
        "light" => egui::ThemePreference::Light,
        "dark" => egui::ThemePreference::Dark,
        _ => egui::ThemePreference::System,
    });
}

/// The semibold face at `size`.
pub fn semi(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semi".into()))
}

/// Seconds for an animation, or zero when the user turned animations off in Windows.
pub fn anim(ctx: &egui::Context, secs: f32) -> f32 {
    if calm(ctx) {
        0.0
    } else {
        secs
    }
}

/// Windows has "Animation effects" off. This is the single source of truth: [`install`] reads
/// the OS setting once and zeroes egui's animation time, and everything else asks here.
pub fn calm(ctx: &egui::Context) -> bool {
    ctx.global_style().animation_time == 0.0
}

/// True when Windows has "Animation effects" turned off (Settings > Accessibility > Visual
/// effects). Read once by [`install`]; use [`calm`] everywhere else.
fn reduce_motion() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    let mut on: i32 = 1;
    // SAFETY: `on` outlives the call and the action reads one BOOL (4 bytes).
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && on == 0
}

// ---------------------------------------------------------------- keyboard focus

fn kb_nav_id() -> egui::Id {
    egui::Id::unique("vidar-keyboard-navigation")
}

/// Call once per frame before building the UI. Remembers whether the last input was from the
/// keyboard, so the focus ring shows for keyboard users and not after a mouse click
/// (the `:focus-visible` rule).
pub fn track_input_modality(ctx: &egui::Context) {
    let seen = ctx.input(|i| {
        let mut last = None;
        for e in &i.events {
            match e {
                egui::Event::Key {
                    pressed: true,
                    key:
                        egui::Key::Tab
                        | egui::Key::ArrowLeft
                        | egui::Key::ArrowRight
                        | egui::Key::ArrowUp
                        | egui::Key::ArrowDown
                        | egui::Key::Space
                        | egui::Key::Enter,
                    ..
                } => last = Some(true),
                egui::Event::PointerButton { pressed: true, .. } => last = Some(false),
                _ => {}
            }
        }
        last
    });
    if let Some(kb) = seen {
        if kb != keyboard_active(ctx) {
            ctx.all_styles_mut(|s| {
                s.visuals.widgets.active.bg_stroke = pressed_stroke(s.visuals.dark_mode, kb);
            });
        }
        ctx.data_mut(|d| d.insert_temp(kb_nav_id(), kb));
    }
    // Focus tooltips follow the keyboard only until the pointer moves again.
    let tips = ctx.input(|i| {
        let mut last = None;
        for e in &i.events {
            match e {
                egui::Event::Key {
                    pressed: true, key, ..
                } if is_nav_key(*key) => {
                    last = Some(true);
                }
                egui::Event::PointerMoved(_) | egui::Event::PointerButton { pressed: true, .. } => {
                    last = Some(false);
                }
                _ => {}
            }
        }
        last
    });
    if let Some(t) = tips {
        ctx.data_mut(|d| d.insert_temp(kb_tips_id(), t));
    }
}

fn is_nav_key(k: egui::Key) -> bool {
    use egui::Key::*;
    matches!(
        k,
        Tab | ArrowLeft | ArrowRight | ArrowUp | ArrowDown | Space | Enter
    )
}

fn kb_tips_id() -> egui::Id {
    egui::Id::unique("vidar-keyboard-tips")
}

/// Should tooltips come from keyboard focus (true) or from the pointer (false)? Only one of
/// the two shows at a time: the last input device wins.
pub fn keyboard_tips(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(kb_tips_id()))
        .unwrap_or(false)
}

/// Did the user last use the keyboard to move around?
pub fn keyboard_active(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(kb_nav_id()))
        .unwrap_or(false)
}

/// Draw the focus ring around `resp` if it has keyboard focus, and scroll it into view.
/// `radius` is the control's own corner radius.
pub fn focus_ring(ui: &egui::Ui, p: &Pal, resp: &egui::Response, radius: u8) {
    if !resp.has_focus() || !keyboard_active(ui.ctx()) {
        return;
    }
    if resp.gained_focus() {
        resp.scroll_to_me(None);
    }
    let gap = size::FOCUS_GAP;
    // A little past the parent's clip rect so a ring on a control at the edge of a card
    // is not cut off.
    let painter = ui
        .painter()
        .with_clip_rect(ui.clip_rect().expand(gap + size::FOCUS_W));
    painter.rect_stroke(
        resp.rect.expand(gap),
        CornerRadius::same(radius.saturating_add(gap as u8)),
        Stroke::new(size::FOCUS_W, p.focus),
        StrokeKind::Outside,
    );
}

/// Focus ring around a circular control centred at `centre` with the given `radius`.
pub fn focus_ring_circle(
    ui: &egui::Ui,
    p: &Pal,
    resp: &egui::Response,
    centre: egui::Pos2,
    radius: f32,
) {
    if !resp.has_focus() || !keyboard_active(ui.ctx()) {
        return;
    }
    if resp.gained_focus() {
        resp.scroll_to_me(None);
    }
    let painter = ui.painter().with_clip_rect(
        ui.clip_rect()
            .expand(radius + size::FOCUS_GAP + size::FOCUS_W),
    );
    painter.circle_stroke(
        centre,
        radius + size::FOCUS_GAP + size::FOCUS_W / 2.0,
        Stroke::new(size::FOCUS_W, p.focus),
    );
}

// ---------------------------------------------------------------- neon

/// The logo's gradient colour at `x` (-0.7..0.7): cyan, magenta, violet, amber.
pub fn neon(x: f32) -> Color32 {
    let [r, g, b] = vidar_brand::neon(x);
    Color32::from_rgb(r as u8, g as u8, b as u8)
}

/// The same hue darkened for a light background (brightest channel at 62 %), so thin bars
/// and strokes keep 3:1 against white.
pub fn neon_on_light(c: Color32) -> Color32 {
    let m = f32::from(c.r().max(c.g()).max(c.b()).max(1));
    let k = 0.62 * 255.0 / m;
    let ch = |v: u8| (f32::from(v) * k).round().min(255.0) as u8;
    Color32::from_rgb(ch(c.r()), ch(c.g()), ch(c.b()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // WCAG relative luminance and contrast ratio of opaque colours.
    /// WCAG relative luminance of an opaque colour.
    fn luminance(c: Color32) -> f32 {
        fn lin(v: u8) -> f32 {
            let s = f32::from(v) / 255.0;
            if s <= 0.039_28 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    /// WCAG contrast ratio between two opaque colours (1.0 to 21.0).
    fn contrast(a: Color32, b: Color32) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// `fg` at opacity `a` over `bg`, as egui composites it (in gamma space).
    fn over(fg: Color32, a: f32, bg: Color32) -> Color32 {
        let mix = |f: u8, b: u8| (f32::from(f) * a + f32::from(b) * (1.0 - a)).round() as u8;
        Color32::from_rgb(
            mix(fg.r(), bg.r()),
            mix(fg.g(), bg.g()),
            mix(fg.b(), bg.b()),
        )
    }

    fn themes() -> [(&'static str, Pal); 2] {
        [("dark", DARK), ("light", LIGHT)]
    }

    #[track_caller]
    fn need(name: &str, theme: &str, fg: Color32, bg: Color32, min: f32) {
        let c = contrast(fg, bg);
        assert!(c >= min, "{theme}: {name} is {c:.2}:1, needs {min}:1");
    }

    #[test]
    fn text_is_readable_on_every_surface() {
        for (t, p) in themes() {
            for (sn, s) in [
                ("bg", p.bg),
                ("surface", p.surface),
                ("surface2", p.surface2),
            ] {
                for (n, c) in [
                    ("text", p.text),
                    ("muted", p.muted),
                    ("accent", p.accent),
                    ("blue", p.blue),
                    ("warn", p.warn),
                    ("danger", p.danger),
                ] {
                    need(&format!("{n} on {sn}"), t, c, s, 4.5);
                }
            }
            // Text on tinted badges and banners.
            for (n, c) in [
                ("accent", p.accent),
                ("blue", p.blue),
                ("warn", p.warn),
                ("danger", p.danger),
                ("muted", p.muted),
            ] {
                for (sn, s) in [("surface", p.surface), ("surface2", p.surface2)] {
                    need(
                        &format!("{n} on its tint over {sn}"),
                        t,
                        c,
                        over(c, alpha::TINT, s),
                        4.5,
                    );
                }
            }
            for (n, c) in [
                ("accent", p.accent),
                ("blue", p.blue),
                ("warn", p.warn),
                ("danger", p.danger),
            ] {
                for (sn, s) in [("surface", p.surface), ("surface2", p.surface2)] {
                    need(
                        &format!("text on {n} banner over {sn}"),
                        t,
                        p.text,
                        over(c, alpha::TINT, s),
                        4.5,
                    );
                    need(
                        &format!("muted on {n} banner over {sn}"),
                        t,
                        p.muted,
                        over(c, alpha::TINT, s),
                        4.5,
                    );
                }
            }
            need(
                "text on selected tile",
                t,
                p.text,
                over(p.accent, alpha::SELECTED, p.surface2),
                4.5,
            );
            need(
                "muted on selected tile",
                t,
                p.muted,
                over(p.accent, alpha::SELECTED, p.surface2),
                4.5,
            );
            need(
                "muted on selected original tile",
                t,
                p.muted,
                over(p.blue, alpha::SELECTED, p.surface2),
                4.5,
            );
        }
    }

    #[test]
    fn buttons_are_readable_in_every_state() {
        for (t, p) in themes() {
            for (n, bg) in [
                ("accent", p.accent),
                ("accent hover", p.accent_hover),
                ("accent pressed", p.accent_pressed),
            ] {
                need(&format!("on_accent on {n}"), t, p.on_accent, bg, 4.5);
            }
            for (n, bg) in [
                ("danger", p.danger),
                ("danger hover", p.danger_hover),
                ("danger pressed", p.danger_pressed),
            ] {
                need(&format!("on_danger on {n}"), t, p.on_danger, bg, 4.5);
            }
            for (n, bg) in [
                ("surface2", p.surface2),
                ("hover", p.hover),
                ("pressed", p.pressed),
            ] {
                need(&format!("text on {n}"), t, p.text, bg, 4.5);
            }
            need("muted on hover", t, p.muted, p.hover, 4.5);
            // Text-coloured danger and accent labels on a secondary button.
            for (n, bg) in [
                ("surface2", p.surface2),
                ("hover", p.hover),
                ("pressed", p.pressed),
            ] {
                need(&format!("danger on {n}"), t, p.danger, bg, 4.5);
                need(&format!("text on {n}"), t, p.text, bg, 4.5);
                need(&format!("muted on {n}"), t, p.muted, bg, 4.5);
            }
            // Selected tile text while pressed is the neutral pressed wash.
            need("on_accent on accent", t, p.on_accent, p.accent, 4.5);
        }
    }

    #[test]
    fn controls_and_focus_are_visible() {
        for (t, p) in themes() {
            for (sn, s) in [
                ("bg", p.bg),
                ("surface", p.surface),
                ("surface2", p.surface2),
            ] {
                need(
                    &format!("control outline on {sn}"),
                    t,
                    p.ctrl_border,
                    s,
                    3.0,
                );
                need(&format!("focus ring on {sn}"), t, p.focus, s, 3.0);
                need(&format!("accent graphic on {sn}"), t, p.accent, s, 3.0);
                need(&format!("idle graphic on {sn}"), t, p.track_idle, s, 3.0);
                need(&format!("danger graphic on {sn}"), t, p.danger, s, 3.0);
            }
            // Switch: thumb against both track colours.
            need(
                "switch thumb on off-track",
                t,
                p.surface,
                p.ctrl_border,
                3.0,
            );
            need("switch thumb on on-track", t, p.on_accent, p.accent, 3.0);
            // A slider handle outlined in text colour on its rail.
            need("slider handle outline", t, p.text, p.ctrl_border, 3.0);
        }
    }

    #[test]
    fn tone_graph_text_is_readable_when_off() {
        // In the default "Off" state the curve is dimmed but its labels are not.
        for (t, p) in themes() {
            for (n, c) in [("band label", p.text), ("gain value", p.muted)] {
                need(n, t, c, p.surface2, 4.5);
            }
            need("accent gain value", t, p.accent, p.surface2, 4.5);
            // Value bubble (accent or muted fill with on_accent text).
            need("bubble text on accent", t, p.on_accent, p.accent, 4.5);
            need("bubble text on muted", t, p.on_accent, p.muted, 4.5);
            // The dimmed curve line is still a 3:1 graphic.
            need(
                "dimmed curve line",
                t,
                over(p.muted, 0.7, p.surface2),
                p.surface2,
                3.0,
            );
        }
    }

    #[test]
    fn power_button_bars_hold_contrast() {
        // Light: solid, darkened neon on the button face (white, and the hover wash).
        for face in [LIGHT.surface, LIGHT.surface2] {
            for k in 0..=28 {
                let x = -0.7 + 0.05 * k as f32;
                let c = neon_on_light(neon(x));
                need(&format!("light neon at {x:.2}"), "light", c, face, 3.0);
            }
        }
        // Resting bars and the edge of the face, both themes.
        for (t, p) in themes() {
            need("idle bar", t, p.track_idle, p.surface, 3.0);
            need("idle edge", t, p.ctrl_border, p.surface, 3.0);
        }
        // Dark: the neon itself on the dark face.
        for k in 0..=28 {
            let x = -0.7 + 0.05 * k as f32;
            need(
                &format!("dark neon at {x:.2}"),
                "dark",
                neon(x),
                DARK.surface,
                3.0,
            );
        }
    }
}
