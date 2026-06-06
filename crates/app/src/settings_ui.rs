//! In-app settings panel: font family/size/line-height and animation toggles.

use editor_surface::{GlyphAtlas, GlyphInstance, RectInstance, list_monospace_fonts};
use wgpu::Queue;

use crate::settings::Settings;

const PANEL_W: f32 = 540.0;
const PANEL_H: f32 = 580.0;
const PAD: f32 = 20.0;
const ROW: f32 = 32.0;
const LIST_H: f32 = 96.0;
const ITEM_H: f32 = 26.0;
const SLIDER_H: f32 = 8.0;
const SLIDER_KNOB: f32 = 14.0;
const SECTION_GAP: f32 = 26.0;

type Rgba = [f32; 4];

const BACKDROP: Rgba = [0.04, 0.05, 0.07, 1.0];
const PANEL_BG: Rgba = [0.11, 0.12, 0.14, 1.0];
const PANEL_BORDER: Rgba = [0.35, 0.40, 0.48, 1.0];
const SECTION: Rgba = [0.55, 0.78, 1.0, 1.0];
const TEXT: Rgba = [0.92, 0.93, 0.95, 1.0];
const TEXT_DIM: Rgba = [0.58, 0.60, 0.66, 1.0];
const SURFACE: Rgba = [0.16, 0.17, 0.20, 1.0];
const SURFACE_HOVER: Rgba = [0.22, 0.24, 0.28, 1.0];
const ACCENT: Rgba = [0.22, 0.58, 0.98, 1.0];
const ACCENT_DIM: Rgba = [0.16, 0.38, 0.72, 1.0];
const SELECTED: Rgba = [0.18, 0.32, 0.52, 1.0];
const BTN: Rgba = [0.20, 0.22, 0.26, 1.0];
const BTN_HOVER: Rgba = [0.28, 0.30, 0.35, 1.0];
const BTN_PRIMARY: Rgba = [0.18, 0.48, 0.88, 1.0];
const BTN_PRIMARY_HOVER: Rgba = [0.24, 0.56, 0.96, 1.0];
const TOGGLE_OFF: Rgba = [0.28, 0.30, 0.34, 1.0];
const PREVIEW_BG: Rgba = [0.08, 0.09, 0.11, 1.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Close,
    FontItem(usize),
    FontSizeSlider,
    LineHeightSlider,
    AnimToggle,
    SmoothBlinkToggle,
    AnimLengthSlider,
    PowerToggle,
    Cancel,
    Apply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    FontSize,
    LineHeight,
    AnimLength,
}

pub struct SettingsUi {
    pub open: bool,
    pub draft: Settings,
    saved: Settings,
    fonts: Vec<String>,
    font_scroll: f32,
    mouse: (f32, f32),
    hover: Option<Hit>,
    drag: Option<Drag>,
    rects: Vec<RectInstance>,
    glyphs: Vec<GlyphInstance>,
}

struct Layout {
    ox: f32,
    oy: f32,
    font_section_y: f32,
    family_label_y: f32,
    list: [f32; 4],
    size_label_y: f32,
    size_slider: [f32; 4],
    lh_label_y: f32,
    lh_slider: [f32; 4],
    preview_label_y: f32,
    preview: [f32; 4],
    anim_section_y: f32,
    anim_toggle: [f32; 4],
    smooth_blink_toggle: [f32; 4],
    anim_length_label_y: f32,
    anim_length_slider: [f32; 4],
    power_toggle: [f32; 4],
    cancel_btn: [f32; 4],
    apply_btn: [f32; 4],
    close_btn: [f32; 4],
}

impl SettingsUi {
    pub fn new(settings: &Settings) -> Self {
        SettingsUi {
            open: false,
            draft: settings.clone(),
            saved: settings.clone(),
            fonts: list_monospace_fonts(),
            font_scroll: 0.0,
            mouse: (0.0, 0.0),
            hover: None,
            drag: None,
            rects: Vec::new(),
            glyphs: Vec::new(),
        }
    }

    pub fn toggle(&mut self, settings: &Settings) {
        if self.open {
            self.close(false);
        } else {
            self.saved = settings.clone();
            self.draft = settings.clone();
            self.font_scroll = 0.0;
            self.open = true;
        }
    }

    pub fn close(&mut self, revert: bool) -> bool {
        if !self.open {
            return false;
        }
        self.open = false;
        self.drag = None;
        revert
    }

    pub fn overlay(&self) -> Option<(&[RectInstance], &[GlyphInstance])> {
        if self.open {
            Some((&self.rects, &self.glyphs))
        } else {
            None
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn handle_mouse_move(&mut self, x: f32, y: f32, lw: f32, lh: f32) {
        if !self.open {
            return;
        }
        self.mouse = (x, y);
        let layout = layout(lw, lh);
        if self.drag.is_some() {
            self.apply_drag(&layout);
            return;
        }
        self.hover = self.hit_test(x, y, &layout);
    }

    pub fn handle_mouse_down(&mut self, x: f32, y: f32, lw: f32, lh: f32) -> SettingsAction {
        if !self.open {
            return SettingsAction::None;
        }
        self.mouse = (x, y);
        let layout = layout(lw, lh);
        match self.hit_test(x, y, &layout) {
            Some(Hit::Close) => SettingsAction::CloseCancel,
            Some(Hit::Cancel) => SettingsAction::CloseCancel,
            Some(Hit::Apply) => SettingsAction::Apply,
            Some(Hit::FontItem(i)) => {
                self.draft.font_family = if i == 0 {
                    None
                } else {
                    Some(self.fonts[i - 1].clone())
                };
                SettingsAction::Preview
            }
            Some(Hit::FontSizeSlider) => {
                self.drag = Some(Drag::FontSize);
                self.set_slider(Drag::FontSize, x, &layout);
                SettingsAction::Preview
            }
            Some(Hit::LineHeightSlider) => {
                self.drag = Some(Drag::LineHeight);
                self.set_slider(Drag::LineHeight, x, &layout);
                SettingsAction::Preview
            }
            Some(Hit::AnimToggle) => {
                self.draft.animations_enabled = !self.draft.animations_enabled;
                if !self.draft.animations_enabled {
                    self.draft.power_mode = false;
                }
                SettingsAction::Preview
            }
            Some(Hit::SmoothBlinkToggle) if self.draft.animations_enabled => {
                self.draft.smooth_blink = !self.draft.smooth_blink;
                SettingsAction::Preview
            }
            Some(Hit::AnimLengthSlider) if self.draft.animations_enabled => {
                self.drag = Some(Drag::AnimLength);
                self.set_slider(Drag::AnimLength, x, &layout);
                SettingsAction::Preview
            }
            Some(Hit::PowerToggle) if self.draft.animations_enabled => {
                self.draft.power_mode = !self.draft.power_mode;
                SettingsAction::Preview
            }
            None if !point_in_panel(x, y, &layout) => SettingsAction::CloseCancel,
            _ => SettingsAction::None,
        }
    }

    pub fn handle_mouse_up(&mut self) -> SettingsAction {
        if self.drag.take().is_some() {
            SettingsAction::Preview
        } else {
            SettingsAction::None
        }
    }

    pub fn handle_wheel(&mut self, delta_y: f32, lw: f32, lh: f32) {
        if !self.open {
            return;
        }
        let layout = layout(lw, lh);
        let list = [layout.ox + PAD, layout.list[1], PANEL_W - PAD * 2.0, LIST_H];
        if !in_rect(self.mouse.0, self.mouse.1, list) {
            return;
        }
        let total = (self.fonts.len() + 1) as f32 * ITEM_H;
        let max_scroll = (total - LIST_H).max(0.0);
        self.font_scroll = (self.font_scroll - delta_y * ITEM_H * 0.5).clamp(0.0, max_scroll);
    }

    pub fn rebuild(
        &mut self,
        atlas: &mut GlyphAtlas,
        queue: &Queue,
        lw: f32,
        lh: f32,
    ) {
        self.rects.clear();
        self.glyphs.clear();
        if !self.open {
            return;
        }
        let layout = layout(lw, lh);
        let mut rects = Vec::new();
        let mut glyphs = Vec::new();
        let mut p = Painter {
            rects: &mut rects,
            glyphs: &mut glyphs,
            atlas,
            queue,
        };

        p.rect([0.0, 0.0, lw, lh], BACKDROP);

        let panel = [layout.ox, layout.oy, PANEL_W, PANEL_H];
        // Double-fill the panel so editor pixels never bleed through blended edges.
        p.rect(panel, PANEL_BG);
        p.rect(panel, PANEL_BG);
        p.rect_outline(panel, PANEL_BORDER);

        p.text(PAD + layout.ox, layout.oy + 14.0, "Settings", TEXT, 1.0);
        let close = layout.close_btn;
        let close_hover = self.hover == Some(Hit::Close);
        p.rect(close, if close_hover { BTN_HOVER } else { BTN });
        p.text_centered(close, "×", TEXT);

        p.section_label(PAD + layout.ox, layout.font_section_y, "Font");
        p.label(PAD + layout.ox, layout.family_label_y, "Family", TEXT_DIM);
        self.draw_font_list(&mut p, &layout);

        p.label(PAD + layout.ox, layout.size_label_y, "Size", TEXT_DIM);
        let size_val = format!("{:.0} px", self.draft.font_size);
        p.text_right(
            layout.ox + PANEL_W - PAD,
            layout.size_label_y,
            &size_val,
            TEXT,
            1.0,
        );
        self.draw_slider(
            &mut p,
            layout.size_slider,
            10.0,
            32.0,
            self.draft.font_size,
            self.hover == Some(Hit::FontSizeSlider) || self.drag == Some(Drag::FontSize),
        );

        p.label(PAD + layout.ox, layout.lh_label_y, "Line height", TEXT_DIM);
        let lh_val = format!("{:.2}", self.draft.line_height);
        p.text_right(
            layout.ox + PANEL_W - PAD,
            layout.lh_label_y,
            &lh_val,
            TEXT,
            1.0,
        );
        self.draw_slider(
            &mut p,
            layout.lh_slider,
            1.0,
            2.0,
            self.draft.line_height,
            self.hover == Some(Hit::LineHeightSlider) || self.drag == Some(Drag::LineHeight),
        );

        p.label(PAD + layout.ox, layout.preview_label_y, "Preview", TEXT_DIM);
        p.rect(layout.preview, PREVIEW_BG);
        p.rect_outline(layout.preview, PANEL_BORDER);
        let family = self
            .draft
            .font_family
            .as_deref()
            .unwrap_or("System default");
        let preview_text = format!("{family} — The quick brown fox jumps");
        p.text_in_rect(layout.preview, &preview_text, TEXT, 1.0, 10.0);

        p.section_label(PAD + layout.ox, layout.anim_section_y, "Animations");
        self.draw_toggle(
            &mut p,
            layout.anim_toggle,
            "Enable animations",
            self.draft.animations_enabled,
            self.hover == Some(Hit::AnimToggle),
            true,
        );
        self.draw_toggle(
            &mut p,
            layout.power_toggle,
            "Power mode (shake + particles)",
            self.draft.power_mode,
            self.hover == Some(Hit::PowerToggle),
            self.draft.animations_enabled,
        );
        self.draw_toggle(
            &mut p,
            layout.smooth_blink_toggle,
            "Smooth cursor blink",
            self.draft.smooth_blink,
            self.hover == Some(Hit::SmoothBlinkToggle),
            self.draft.animations_enabled,
        );
        p.label(
            PAD + layout.ox,
            layout.anim_length_label_y,
            &format!("Cursor animation length: {:.2}s", self.draft.animation_length),
            TEXT_DIM,
        );
        self.draw_slider(
            &mut p,
            layout.anim_length_slider,
            0.04,
            0.35,
            self.draft.animation_length,
            self.hover == Some(Hit::AnimLengthSlider) || self.drag == Some(Drag::AnimLength),
        );

        let cancel_hover = self.hover == Some(Hit::Cancel);
        p.rect(
            layout.cancel_btn,
            if cancel_hover { BTN_HOVER } else { BTN },
        );
        p.text_centered(layout.cancel_btn, "Cancel", TEXT);

        let apply_hover = self.hover == Some(Hit::Apply);
        p.rect(
            layout.apply_btn,
            if apply_hover {
                BTN_PRIMARY_HOVER
            } else {
                BTN_PRIMARY
            },
        );
        p.text_centered(layout.apply_btn, "Apply", TEXT);

        self.rects = rects;
        self.glyphs = glyphs;
    }

    fn draw_font_list(&self, p: &mut Painter<'_>, layout: &Layout) {
        let list = layout.list;
        p.rect(list, SURFACE);
        p.rect_outline(list, PANEL_BORDER);

        let items: Vec<(Option<String>, bool)> = std::iter::once((None, self.draft.font_family.is_none()))
            .chain(self.fonts.iter().map(|f| {
                (
                    Some(f.clone()),
                    self.draft.font_family.as_deref() == Some(f.as_str()),
                )
            }))
            .collect();

        let mut item_y = list[1] - self.font_scroll;
        for (i, (_, selected)) in items.iter().enumerate() {
            let row = [list[0], item_y, list[2], ITEM_H];
            let visible = row[1] + row[3] > list[1] && row[1] < list[1] + list[3];
            if visible {
                let hover = self.hover == Some(Hit::FontItem(i));
                let bg = if *selected {
                    SELECTED
                } else if hover {
                    SURFACE_HOVER
                } else {
                    [0.0, 0.0, 0.0, 0.0]
                };
                if bg[3] > 0.0 {
                    // Clip row highlight to the list viewport.
                    let clip_y = row[1].max(list[1]);
                    let clip_h = (row[1] + row[3]).min(list[1] + list[3]) - clip_y;
                    if clip_h > 0.0 {
                        p.rect([row[0], clip_y, row[2], clip_h], bg);
                    }
                }
                let label = if i == 0 {
                    "System default".to_string()
                } else {
                    self.fonts[i - 1].clone()
                };
                let text_row = [row[0], row[1], row[2], row[3]];
                if let Some(clip) = intersect_clip(text_row, list) {
                    p.text_in_rect(
                        clip,
                        &label,
                        if *selected { TEXT } else { TEXT_DIM },
                        1.0,
                        10.0,
                    );
                }
            }
            item_y += ITEM_H;
        }
    }

    fn draw_slider(
        &self,
        p: &mut Painter<'_>,
        track: [f32; 4],
        min: f32,
        max: f32,
        value: f32,
        active: bool,
    ) {
        p.rect(track, SURFACE);
        let t = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let fill = [track[0], track[1], track[2] * t, track[3]];
        if fill[2] > 0.5 {
            p.rect(fill, if active { ACCENT } else { ACCENT_DIM });
        }
        let knob_x = track[0] + track[2] * t - SLIDER_KNOB * 0.5;
        let knob_y = track[1] + track[3] * 0.5 - SLIDER_KNOB * 0.5;
        p.rect([knob_x, knob_y, SLIDER_KNOB, SLIDER_KNOB], if active { ACCENT } else { TEXT });
    }

    fn draw_toggle(
        &self,
        p: &mut Painter<'_>,
        row: [f32; 4],
        label: &str,
        on: bool,
        hover: bool,
        enabled: bool,
    ) {
        let label_rect = [row[0], row[1], row[2] - 52.0, row[3]];
        p.text_in_rect(
            label_rect,
            label,
            if enabled { TEXT } else { TEXT_DIM },
            1.0,
            0.0,
        );
        let sw = [row[0] + row[2] - 44.0, row[1] + 5.0, 44.0, 22.0];
        let track = if !enabled {
            [0.22, 0.22, 0.24, 1.0]
        } else if on {
            ACCENT
        } else if hover {
            SURFACE_HOVER
        } else {
            TOGGLE_OFF
        };
        p.rect(sw, track);
        let knob_x = if on { sw[0] + sw[2] - 20.0 } else { sw[0] + 2.0 };
        p.rect([knob_x, sw[1] + 2.0, 18.0, 18.0], TEXT);
    }

    fn hit_test(&self, x: f32, y: f32, layout: &Layout) -> Option<Hit> {
        if in_rect(x, y, layout.close_btn) {
            return Some(Hit::Close);
        }
        if in_rect(x, y, layout.cancel_btn) {
            return Some(Hit::Cancel);
        }
        if in_rect(x, y, layout.apply_btn) {
            return Some(Hit::Apply);
        }
        if in_rect(x, y, layout.size_slider) {
            return Some(Hit::FontSizeSlider);
        }
        if in_rect(x, y, layout.lh_slider) {
            return Some(Hit::LineHeightSlider);
        }
        if in_rect(x, y, layout.anim_toggle) {
            return Some(Hit::AnimToggle);
        }
        if in_rect(x, y, layout.smooth_blink_toggle) && self.draft.animations_enabled {
            return Some(Hit::SmoothBlinkToggle);
        }
        if in_rect(x, y, layout.anim_length_slider) && self.draft.animations_enabled {
            return Some(Hit::AnimLengthSlider);
        }
        if in_rect(x, y, layout.power_toggle) && self.draft.animations_enabled {
            return Some(Hit::PowerToggle);
        }

        let list = layout.list;
        if in_rect(x, y, list) {
            let rel_y = y - list[1] + self.font_scroll;
            if rel_y >= 0.0 {
                let idx = (rel_y / ITEM_H).floor() as usize;
                let max = self.fonts.len() + 1;
                if idx < max {
                    return Some(Hit::FontItem(idx));
                }
            }
        }
        None
    }

    fn apply_drag(&mut self, layout: &Layout) {
        if let Some(drag) = self.drag {
            self.set_slider(drag, self.mouse.0, layout);
        }
    }

    fn set_slider(&mut self, drag: Drag, x: f32, layout: &Layout) {
        let (track, min, max, round) = match drag {
            Drag::FontSize => (layout.size_slider, 10.0_f32, 32.0, true),
            Drag::LineHeight => (layout.lh_slider, 1.0, 2.0, false),
            Drag::AnimLength => (layout.anim_length_slider, 0.04, 0.35, false),
        };
        let t = ((x - track[0]) / track[2]).clamp(0.0, 1.0);
        let v = min + t * (max - min);
        match drag {
            Drag::FontSize => self.draft.font_size = if round { v.round() } else { v },
            Drag::LineHeight => self.draft.line_height = (v * 100.0).round() / 100.0,
            Drag::AnimLength => {
                self.draft.animation_length = (v * 100.0).round() / 100.0;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    None,
    Preview,
    Apply,
    CloseCancel,
}

pub fn revert_draft(ui: &mut SettingsUi, settings: &Settings) {
    ui.draft = settings.clone();
}

fn layout(lw: f32, lh: f32) -> Layout {
    let ox = ((lw - PANEL_W) * 0.5).max(0.0);
    let oy = ((lh - PANEL_H) * 0.5).max(0.0);
    let content_w = PANEL_W - PAD * 2.0;

    let mut y = oy + 48.0;
    let font_section_y = y;
    y += 22.0;
    let family_label_y = y;
    y += 18.0;
    let list = [ox + PAD, y, content_w, LIST_H];
    y += LIST_H + 12.0;
    let size_label_y = y;
    y += 18.0;
    let size_slider = [ox + PAD, y, content_w, SLIDER_H];
    y += 24.0;
    let lh_label_y = y;
    y += 18.0;
    let lh_slider = [ox + PAD, y, content_w, SLIDER_H];
    y += 24.0;
    let preview_label_y = y;
    y += 18.0;
    let preview = [ox + PAD, y, content_w, 32.0];
    y += preview[3] + 16.0;
    let anim_section_y = y;
    y += SECTION_GAP;
    let anim_toggle = [ox + PAD, y, content_w, ROW];
    y += ROW + 4.0;
    let power_toggle = [ox + PAD, y, content_w, ROW];
    y += ROW + 4.0;
    let smooth_blink_toggle = [ox + PAD, y, content_w, ROW];
    y += ROW + 8.0;
    let anim_length_label_y = y;
    y += 18.0;
    let anim_length_slider = [ox + PAD, y, content_w, SLIDER_H];
    let footer_y = oy + PANEL_H - PAD - 34.0;

    Layout {
        ox,
        oy,
        font_section_y,
        family_label_y,
        list,
        size_label_y,
        size_slider,
        lh_label_y,
        lh_slider,
        preview_label_y,
        preview,
        anim_section_y,
        anim_toggle,
        smooth_blink_toggle,
        anim_length_label_y,
        anim_length_slider,
        power_toggle,
        cancel_btn: [ox + PAD, footer_y, 100.0, 34.0],
        apply_btn: [ox + PANEL_W - PAD - 100.0, footer_y, 100.0, 34.0],
        close_btn: [ox + PANEL_W - PAD - 28.0, oy + 10.0, 28.0, 28.0],
    }
}

fn point_in_panel(x: f32, y: f32, layout: &Layout) -> bool {
    in_rect(x, y, [layout.ox, layout.oy, PANEL_W, PANEL_H])
}

fn in_rect(x: f32, y: f32, r: [f32; 4]) -> bool {
    x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3]
}

fn intersect_clip(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    if x1 <= x0 || y1 <= y0 {
        None
    } else {
        Some([x0, y0, x1 - x0, y1 - y0])
    }
}

struct Painter<'a> {
    rects: &'a mut Vec<RectInstance>,
    glyphs: &'a mut Vec<GlyphInstance>,
    atlas: &'a mut GlyphAtlas,
    queue: &'a Queue,
}

impl Painter<'_> {
    fn rect(&mut self, r: [f32; 4], color: Rgba) {
        self.rects.push(RectInstance {
            pos: [r[0], r[1]],
            size: [r[2], r[3]],
            color,
        });
    }

    fn rect_outline(&mut self, r: [f32; 4], color: Rgba) {
        let [x, y, w, h] = r;
        let t = 1.0;
        self.rect([x, y, w, t], color);
        self.rect([x, y + h - t, w, t], color);
        self.rect([x, y, t, h], color);
        self.rect([x + w - t, y, t, h], color);
    }

    fn text(&mut self, x: f32, y: f32, text: &str, color: Rgba, scale: f32) {
        let adv = self.atlas.cell_w * scale;
        let mut px = x;
        for c in text.chars() {
            if c == ' ' {
                px += adv;
                continue;
            }
            if let Some(info) = self.atlas.glyph(self.queue, c) {
                let s = scale;
                self.glyphs.push(GlyphInstance {
                    pos: [px + info.left * s, y + info.top * s],
                    size: [info.width * s, info.height * s],
                    uv_min: info.uv_min,
                    uv_max: info.uv_max,
                    color,
                });
            }
            px += adv;
        }
    }

    fn text_w(&self, text: &str, scale: f32) -> f32 {
        text.chars().count() as f32 * self.atlas.cell_w * scale
    }

    fn text_right(&mut self, right_x: f32, y: f32, text: &str, color: Rgba, scale: f32) {
        let w = self.text_w(text, scale);
        self.text(right_x - w, y, text, color, scale);
    }

    fn text_in_rect(&mut self, rect: [f32; 4], text: &str, color: Rgba, scale: f32, pad: f32) {
        let adv = self.atlas.cell_w * scale;
        let y = rect[1] + (rect[3] - self.atlas.cell_h * scale) * 0.5;
        let mut px = rect[0] + pad;
        let max_x = rect[0] + rect[2] - pad;
        for c in text.chars() {
            if px + adv > max_x {
                break;
            }
            if c != ' ' {
                if let Some(info) = self.atlas.glyph(self.queue, c) {
                    let glyph_rect = [
                        px + info.left * scale,
                        y + info.top * scale,
                        info.width * scale,
                        info.height * scale,
                    ];
                    if intersect_clip(
                        glyph_rect,
                        [rect[0], rect[1], rect[2], rect[3]],
                    )
                    .is_some()
                    {
                        self.glyphs.push(GlyphInstance {
                            pos: [glyph_rect[0], glyph_rect[1]],
                            size: [glyph_rect[2], glyph_rect[3]],
                            uv_min: info.uv_min,
                            uv_max: info.uv_max,
                            color,
                        });
                    }
                }
            }
            px += adv;
        }
    }

    fn text_centered(&mut self, r: [f32; 4], text: &str, color: Rgba) {
        let w = self.text_w(text, 1.0);
        let pad = ((r[2] - w) * 0.5).max(0.0);
        self.text_in_rect(r, text, color, 1.0, pad);
    }

    fn label(&mut self, x: f32, y: f32, text: &str, color: Rgba) {
        self.text(x, y, text, color, 1.0);
    }

    fn section_label(&mut self, x: f32, y: f32, text: &str) {
        self.text(x, y, text, SECTION, 1.0);
    }
}
