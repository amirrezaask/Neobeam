//! Map Neovim Normal / mode colors onto Dear ImGui style colors, and expose
//! semantic theme accessors so surfaces don't need to hardcode RGBA literals.

use imgui::{Context, StyleColor, Ui};
use nvim_core::grid::GridStateStore;

#[derive(Clone, Copy, Debug)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

impl Rgb {
    fn from_u32(c: u32) -> Self {
        Rgb {
            r: ((c >> 16) & 0xff) as u8,
            g: ((c >> 8) & 0xff) as u8,
            b: (c & 0xff) as u8,
        }
    }

    fn from_rgba(c: [f32; 4]) -> Self {
        Rgb {
            r: (c[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            g: (c[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            b: (c[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        }
    }

    fn to_rgba(self, alpha: f32) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            alpha,
        ]
    }

    fn luminance(self) -> f32 {
        (0.2126 * self.r as f32 + 0.7152 * self.g as f32 + 0.0722 * self.b as f32) / 255.0
    }

    fn blend(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let u = 1.0 - t;
        Rgb {
            r: (self.r as f32 * u + other.r as f32 * t).round() as u8,
            g: (self.g as f32 * u + other.g as f32 * t).round() as u8,
            b: (self.b as f32 * u + other.b as f32 * t).round() as u8,
        }
    }

    fn lighten(self, amount: f32) -> Self {
        self.blend(Rgb::WHITE, amount)
    }

    fn darken(self, amount: f32) -> Self {
        self.blend(Rgb::BLACK, amount)
    }

    fn rotate_hue(self, degrees: f32) -> Self {
        let (h, s, v) = rgb_to_hsv(self);
        let h = (h + degrees).rem_euclid(360.0);
        hsv_to_rgb(h, s, v)
    }
}

impl Rgb {
    const WHITE: Self = Rgb {
        r: 255,
        g: 255,
        b: 255,
    };
    const BLACK: Self = Rgb { r: 0, g: 0, b: 0 };
}

fn rgb_to_hsv(c: Rgb) -> (f32, f32, f32) {
    let r = c.r as f32 / 255.0;
    let g = c.g as f32 / 255.0;
    let b = c.b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgb {
    let c = v * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    Rgb {
        r: ((r1 + m).clamp(0.0, 1.0) * 255.0).round() as u8,
        g: ((g1 + m).clamp(0.0, 1.0) * 255.0).round() as u8,
        b: ((b1 + m).clamp(0.0, 1.0) * 255.0).round() as u8,
    }
}

struct NvimPalette {
    fg: Rgb,
    bg: Rgb,
    accent: Rgb,
}

fn resolve_bg(store: &GridStateStore) -> Rgb {
    let def = &store.default_colors;
    if !def.bg_none {
        return Rgb::from_u32(def.bg);
    }
    let fg = Rgb::from_u32(def.fg);
    if fg.luminance() > 0.55 {
        fg.blend(Rgb::BLACK, 0.92)
    } else {
        fg.darken(0.88)
    }
}

fn resolve_accent(store: &GridStateStore) -> Rgb {
    if let Some(mode) = store.current_mode() {
        if let Some(attr) = store.highlight(mode.attr_id) {
            if let Some(c) = attr.foreground {
                return Rgb::from_u32(c);
            }
            if let Some(c) = attr.background {
                return Rgb::from_u32(c);
            }
        }
    }
    Rgb::from_u32(store.default_colors.sp)
}

fn palette_from_store(store: &GridStateStore) -> NvimPalette {
    NvimPalette {
        fg: Rgb::from_u32(store.default_colors.fg),
        bg: resolve_bg(store),
        accent: resolve_accent(store),
    }
}

fn set_color(style: &mut imgui::Style, slot: StyleColor, rgb: Rgb, alpha: f32) {
    style.colors[slot as usize] = rgb.to_rgba(alpha);
}

/// Spacing and rounding constants applied globally to ImGui's style. Any
/// surface that needs the same look should consult these instead of hardcoding.
#[derive(Clone, Copy, Debug)]
pub struct StyleMetrics {
    pub window_padding: [f32; 2],
    pub frame_padding: [f32; 2],
    pub item_spacing: [f32; 2],
    pub item_inner_spacing: [f32; 2],
    pub window_rounding: f32,
    pub frame_rounding: f32,
    pub grab_rounding: f32,
    pub scrollbar_size: f32,
    pub scrollbar_rounding: f32,
    pub indent_spacing: f32,
    pub pane_focus_border: f32,
    pub gutter_line: f32,
}

pub const METRICS: StyleMetrics = StyleMetrics {
    window_padding: [14.0, 12.0],
    frame_padding: [8.0, 5.0],
    item_spacing: [8.0, 6.0],
    item_inner_spacing: [6.0, 4.0],
    window_rounding: 8.0,
    frame_rounding: 5.0,
    grab_rounding: 4.0,
    scrollbar_size: 12.0,
    scrollbar_rounding: 4.0,
    indent_spacing: 18.0,
    pane_focus_border: 1.0,
    gutter_line: 1.0,
};

/// Apply the active Neovim colorscheme to the global ImGui style.
pub fn apply_nvim_theme(ctx: &mut Context, store: &GridStateStore) {
    let p = palette_from_store(store);
    let style = ctx.style_mut();

    let bg = p.bg.to_rgba(1.0);
    let frame = p.bg.lighten(0.08).to_rgba(1.0);
    let frame_hov = p.bg.blend(p.accent, 0.18).to_rgba(1.0);
    let frame_act = p.bg.blend(p.accent, 0.32).to_rgba(1.0);
    let popup = p.bg.darken(0.04).to_rgba(0.98);
    let border = p.fg.to_rgba(0.28);
    let disabled = p.fg.to_rgba(0.45);
    let accent = p.accent.to_rgba(1.0);

    set_color(style, StyleColor::Text, p.fg, 1.0);
    set_color(style, StyleColor::TextDisabled, p.fg, 0.45);
    set_color(style, StyleColor::WindowBg, p.bg, 1.0);
    set_color(style, StyleColor::ChildBg, p.bg, 1.0);
    style.colors[StyleColor::PopupBg as usize] = popup;
    style.colors[StyleColor::Border as usize] = border;
    style.colors[StyleColor::BorderShadow as usize] = [0.0, 0.0, 0.0, 0.0];

    style.colors[StyleColor::FrameBg as usize] = frame;
    style.colors[StyleColor::FrameBgHovered as usize] = frame_hov;
    style.colors[StyleColor::FrameBgActive as usize] = frame_act;

    style.colors[StyleColor::TitleBg as usize] = bg;
    style.colors[StyleColor::TitleBgActive as usize] = frame_hov;
    style.colors[StyleColor::TitleBgCollapsed as usize] = bg;
    style.colors[StyleColor::MenuBarBg as usize] = bg;

    style.colors[StyleColor::ScrollbarBg as usize] = p.bg.to_rgba(0.4);
    style.colors[StyleColor::ScrollbarGrab as usize] = p.fg.to_rgba(0.35);
    style.colors[StyleColor::ScrollbarGrabHovered as usize] = p.fg.to_rgba(0.55);
    style.colors[StyleColor::ScrollbarGrabActive as usize] = accent;

    style.colors[StyleColor::CheckMark as usize] = accent;
    style.colors[StyleColor::SliderGrab as usize] = accent;
    style.colors[StyleColor::SliderGrabActive as usize] = p.accent.lighten(0.15).to_rgba(1.0);

    style.colors[StyleColor::Button as usize] = frame;
    style.colors[StyleColor::ButtonHovered as usize] = frame_hov;
    style.colors[StyleColor::ButtonActive as usize] = frame_act;

    style.colors[StyleColor::Header as usize] = frame;
    style.colors[StyleColor::HeaderHovered as usize] = frame_hov;
    style.colors[StyleColor::HeaderActive as usize] = frame_act;

    style.colors[StyleColor::Separator as usize] = border;
    style.colors[StyleColor::SeparatorHovered as usize] = p.fg.to_rgba(0.45);
    style.colors[StyleColor::SeparatorActive as usize] = accent;

    style.colors[StyleColor::ResizeGrip as usize] = p.fg.to_rgba(0.18);
    style.colors[StyleColor::ResizeGripHovered as usize] = p.fg.to_rgba(0.45);
    style.colors[StyleColor::ResizeGripActive as usize] = accent;

    style.colors[StyleColor::Tab as usize] = frame;
    style.colors[StyleColor::TabHovered as usize] = frame_hov;
    style.colors[StyleColor::TabActive as usize] = frame_act;
    style.colors[StyleColor::TabUnfocused as usize] = p.bg.to_rgba(0.9);
    style.colors[StyleColor::TabUnfocusedActive as usize] = frame;

    style.colors[StyleColor::TableHeaderBg as usize] = frame;
    style.colors[StyleColor::TableBorderStrong as usize] = border;
    style.colors[StyleColor::TableBorderLight as usize] = p.fg.to_rgba(0.15);
    style.colors[StyleColor::TableRowBg as usize] = [0.0, 0.0, 0.0, 0.0];
    style.colors[StyleColor::TableRowBgAlt as usize] = p.fg.to_rgba(0.04);

    style.colors[StyleColor::TextSelectedBg as usize] = p.accent.to_rgba(0.45);
    style.colors[StyleColor::NavHighlight as usize] = accent;
    style.colors[StyleColor::ModalWindowDimBg as usize] = p.bg.to_rgba(0.65);

    style.colors[StyleColor::TextDisabled as usize] = disabled;

    style.window_padding = METRICS.window_padding;
    style.frame_padding = METRICS.frame_padding;
    style.item_spacing = METRICS.item_spacing;
    style.item_inner_spacing = METRICS.item_inner_spacing;
    style.window_rounding = METRICS.window_rounding;
    style.child_rounding = METRICS.frame_rounding;
    style.popup_rounding = METRICS.frame_rounding;
    style.frame_rounding = METRICS.frame_rounding;
    style.grab_rounding = METRICS.grab_rounding;
    style.tab_rounding = METRICS.frame_rounding;
    style.scrollbar_size = METRICS.scrollbar_size;
    style.scrollbar_rounding = METRICS.scrollbar_rounding;
    style.indent_spacing = METRICS.indent_spacing;
}

// ────────────────────────────── theme accessors ──────────────────────────────

fn rgb_window_bg(ui: &Ui) -> Rgb {
    Rgb::from_rgba(ui.style_color(StyleColor::WindowBg))
}

fn rgb_text(ui: &Ui) -> Rgb {
    Rgb::from_rgba(ui.style_color(StyleColor::Text))
}

fn rgb_accent(ui: &Ui) -> Rgb {
    Rgb::from_rgba(ui.style_color(StyleColor::CheckMark))
}

fn is_dark(ui: &Ui) -> bool {
    rgb_window_bg(ui).luminance() < 0.5
}

pub fn section_label_color(ui: &Ui) -> [f32; 4] {
    rgb_text(ui).blend(rgb_window_bg(ui), 0.45).to_rgba(1.0)
}

pub fn error_text(ui: &Ui) -> [f32; 4] {
    let bg = rgb_window_bg(ui);
    if is_dark(ui) {
        Rgb {
            r: 255,
            g: 115,
            b: 115,
        }
        .to_rgba(1.0)
    } else {
        Rgb {
            r: 200,
            g: 40,
            b: 40,
        }
        .blend(bg, 0.05)
        .to_rgba(1.0)
    }
}

pub fn accent(ui: &Ui) -> [f32; 4] {
    rgb_accent(ui).to_rgba(1.0)
}

pub fn border(ui: &Ui) -> [f32; 4] {
    ui.style_color(StyleColor::Border)
}

/// Background tint for empty/placeholder panes — quiet, luminance-aware nudge
/// from the window background so it sits next to the editor without clashing.
pub fn pane_bg(ui: &Ui, focused: bool) -> [f32; 4] {
    let bg = rgb_window_bg(ui);
    let nudge = if focused { 0.04 } else { 0.10 };
    let toward = if is_dark(ui) { Rgb::WHITE } else { Rgb::BLACK };
    bg.blend(toward, nudge).to_rgba(1.0)
}

/// 1-px accent-tinted border drawn around the focused pane.
pub fn pane_focus_border_color(ui: &Ui) -> [f32; 4] {
    rgb_accent(ui).to_rgba(0.55)
}

// ──────────────────────────── diff colors ────────────────────────────

fn diff_pair(ui: &Ui, hue: f32) -> (Rgb, Rgb, Rgb) {
    // Returns (bg, text, emphasis) for a diff color family, derived from theme luminance.
    let bg_lum = rgb_window_bg(ui).luminance();
    let dark = bg_lum < 0.5;
    let (s_bg, v_bg, s_tx, v_tx, s_em, v_em) = if dark {
        (0.55, 0.30, 0.65, 0.95, 0.85, 1.0)
    } else {
        (0.35, 0.95, 0.75, 0.55, 0.95, 0.45)
    };
    let bg_alpha = if dark { 0.55 } else { 0.30 };
    let bg = hsv_to_rgb(hue, s_bg, v_bg);
    let text = hsv_to_rgb(hue, s_tx, v_tx);
    let emph = hsv_to_rgb(hue, s_em, v_em);
    // Pre-multiply caller is easier if we just expose alpha separately, but the
    // caller already builds [f32;4]s — encode alpha in the bg variant only.
    let _ = bg_alpha; // alpha is applied by the helpers below.
    (bg, text, emph)
}

fn diff_bg_alpha(ui: &Ui) -> f32 {
    if is_dark(ui) {
        0.55
    } else {
        0.30
    }
}

pub fn diff_delete_bg(ui: &Ui) -> [f32; 4] {
    let (bg, _, _) = diff_pair(ui, 0.0);
    bg.to_rgba(diff_bg_alpha(ui))
}

pub fn diff_delete_text(ui: &Ui) -> [f32; 4] {
    let (_, text, _) = diff_pair(ui, 0.0);
    text.to_rgba(1.0)
}

pub fn diff_delete_emphasis(ui: &Ui) -> [f32; 4] {
    let (_, _, em) = diff_pair(ui, 0.0);
    em.to_rgba(1.0)
}

pub fn diff_insert_bg(ui: &Ui) -> [f32; 4] {
    let (bg, _, _) = diff_pair(ui, 130.0);
    bg.to_rgba(diff_bg_alpha(ui))
}

pub fn diff_insert_text(ui: &Ui) -> [f32; 4] {
    let (_, text, _) = diff_pair(ui, 130.0);
    text.to_rgba(1.0)
}

pub fn diff_insert_emphasis(ui: &Ui) -> [f32; 4] {
    let (_, _, em) = diff_pair(ui, 130.0);
    em.to_rgba(1.0)
}

// ───────────────────────── git log lane palette ─────────────────────────

/// Eight distinct, theme-aware colors for the git log's commit graph lanes.
/// Hues are evenly spaced from the accent; saturation/value pick from theme luminance.
pub fn git_lane_color(ui: &Ui, idx: usize) -> [f32; 4] {
    let dark = is_dark(ui);
    let (h0, _, _) = rgb_to_hsv(rgb_accent(ui));
    let step = 360.0 / 8.0;
    let h = (h0 + step * (idx % 8) as f32).rem_euclid(360.0);
    let (s, v) = if dark { (0.70, 0.90) } else { (0.65, 0.65) };
    hsv_to_rgb(h, s, v).to_rgba(1.0)
}

/// Soft shadow color used behind log-graph dots.
pub fn git_lane_dot_shadow(ui: &Ui) -> [f32; 4] {
    let bg = rgb_window_bg(ui);
    let toward = if is_dark(ui) { Rgb::BLACK } else { Rgb::WHITE };
    bg.blend(toward, 0.55).to_rgba(0.6)
}

#[allow(dead_code)]
pub fn rotate_accent_hue(ui: &Ui, degrees: f32) -> [f32; 4] {
    rgb_accent(ui).rotate_hue(degrees).to_rgba(1.0)
}
