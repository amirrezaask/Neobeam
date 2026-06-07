//! Map Neovim Normal / mode colors onto Dear ImGui style colors.

use imgui::{Context, StyleColor};
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
}

impl Rgb {
    const WHITE: Self = Rgb { r: 255, g: 255, b: 255 };
    const BLACK: Self = Rgb { r: 0, g: 0, b: 0 };
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
}
