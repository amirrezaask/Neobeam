//! Frame builder for terminal panes. Converts a `TermGrid` into `DrawLists`
//! using the same glyph atlas and GPU instance types as the nvim renderer.

use terminal_core::{TermCell, TermColor, TermCursorShape, Underline};

use crate::atlas::GlyphAtlas;
use crate::frame::{DrawBatch, DrawLists, GlyphInstance, RectInstance, ScissorRect};
use crate::term_scroll::TermScrollState;

pub struct TermFrameBuilder;

impl TermFrameBuilder {
    /// Build draw lists for one terminal pane.
    ///
    /// `origin` is the top-left corner of the pane in logical pixels.
    /// `queue` is needed by the atlas rasterizer.
    pub fn build(
        state: &TermScrollState,
        atlas: &mut GlyphAtlas,
        queue: &wgpu::Queue,
        origin: [f32; 2],
        cell_w: f32,
        cell_h: f32,
        focused: bool,
    ) -> DrawLists {
        let mut rects: Vec<RectInstance> = Vec::new();
        let mut glyphs: Vec<GlyphInstance> = Vec::new();

        let viewport = state.viewport();
        let default_fg = viewport.default_fg;
        let default_bg = viewport.default_bg;
        let clear = default_bg.to_f32_rgba(1.0);
        let scroll_y = state.scroll_offset_pixels(cell_h);
        let height = viewport.rows.len();
        let width = viewport.rows.first().map(Vec::len).unwrap_or(0);

        let cursor_active = viewport.display_offset == 0
            && viewport.cursor_visible
            && state.cursor_should_render();
        let cursor_opacity = state.cursor_opacity();
        let (cur_row, cur_col) = viewport.cursor;
        let cursor_shape = viewport.cursor_shape;

        for row in 0..=height {
            let Some(line) = state.line_at(row as isize) else {
                continue;
            };
            let y = origin[1] + scroll_y + row as f32 * cell_h;
            if y + cell_h <= origin[1] || y >= origin[1] + height as f32 * cell_h {
                continue;
            }
            for (col, cell) in line.iter().enumerate() {
                let under_cursor = cursor_active && row == cur_row && col == cur_col;
                let invert_glyph = under_cursor
                    && cursor_opacity > 0.01
                    && cursor_shape == TermCursorShape::Block
                    && focused;
                draw_cell(
                    &mut rects,
                    &mut glyphs,
                    cell,
                    default_bg,
                    atlas,
                    queue,
                    origin[0] + col as f32 * cell_w,
                    y,
                    cell_w,
                    cell_h,
                    invert_glyph,
                );
            }
        }

        if cursor_active && cursor_opacity > 0.01 {
            let [cx, cy] = state.cursor_pixel();
            let px = origin[0] + cx;
            let py = origin[1] + cy;
            draw_cursor_overlay(
                &mut rects,
                cursor_shape,
                default_fg,
                px,
                py,
                cell_w,
                cell_h,
                cursor_opacity,
                focused,
            );
        }

        let rect_count = rects.len() as u32;
        let glyph_count = glyphs.len() as u32;
        let scissor = Some(ScissorRect {
            x: origin[0].max(0.0) as u32,
            y: origin[1].max(0.0) as u32,
            w: (width as f32 * cell_w).max(0.0) as u32,
            h: (height as f32 * cell_h).max(0.0) as u32,
        });

        let mut batches = Vec::new();
        if rect_count > 0 {
            batches.push(DrawBatch::Rects {
                start: 0,
                count: rect_count,
                scissor,
            });
        }
        if glyph_count > 0 {
            batches.push(DrawBatch::Glyphs {
                start: 0,
                count: glyph_count,
                scissor,
            });
        }

        DrawLists {
            rects,
            glyphs,
            quads: Vec::new(),
            batches,
            clear,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_cell(
    rects: &mut Vec<RectInstance>,
    glyphs: &mut Vec<GlyphInstance>,
    cell: &TermCell,
    default_bg: TermColor,
    atlas: &mut GlyphAtlas,
    queue: &wgpu::Queue,
    x: f32,
    y: f32,
    cell_w: f32,
    cell_h: f32,
    invert_glyph: bool,
) {
    let (fg, bg) = if cell.inverse ^ cell.selected {
        (cell.bg, cell.fg)
    } else {
        (cell.fg, cell.bg)
    };

    let bg_rgba = bg.to_f32_rgba(1.0);
    if bg_rgba != default_bg.to_f32_rgba(1.0) {
        rects.push(RectInstance {
            pos: [x, y],
            size: [cell_w, cell_h],
            color: bg_rgba,
        });
    }

    let glyph_alpha = if cell.dim { 0.55 } else { 1.0 };
    let glyph_color = if invert_glyph {
        bg_rgba
    } else {
        fg.to_f32_rgba(glyph_alpha)
    };

    if !cell.hidden && !cell.spacer && cell.ch != ' ' && cell.ch != '\0' {
        for ch in std::iter::once(cell.ch).chain(cell.zerowidth.iter().copied()) {
            if let Some(info) = atlas.glyph(queue, ch) {
                glyphs.push(GlyphInstance {
                    pos: [x + info.left, y + info.top],
                    size: [info.width, info.height],
                    uv_min: info.uv_min,
                    uv_max: info.uv_max,
                    color: glyph_color,
                });
            }
        }
    }

    let decoration = cell.underline_color.unwrap_or(fg).to_f32_rgba(glyph_alpha);
    let thickness = (cell_h / 14.0).max(1.0);
    let underline_y = y + cell_h - thickness * 2.0;
    match cell.underline {
        Underline::None => {}
        Underline::Double => {
            rects.push(RectInstance {
                pos: [x, underline_y - thickness * 2.0],
                size: [cell_w, thickness],
                color: decoration,
            });
            rects.push(RectInstance {
                pos: [x, underline_y],
                size: [cell_w, thickness],
                color: decoration,
            });
        }
        Underline::Single | Underline::Curl | Underline::Dotted | Underline::Dashed => {
            rects.push(RectInstance {
                pos: [x, underline_y],
                size: [cell_w, thickness],
                color: decoration,
            });
        }
    }
    if cell.strikeout {
        rects.push(RectInstance {
            pos: [x, y + cell_h * 0.52],
            size: [if cell.wide { cell_w * 2.0 } else { cell_w }, thickness],
            color: fg.to_f32_rgba(glyph_alpha),
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_cursor_overlay(
    rects: &mut Vec<RectInstance>,
    shape: TermCursorShape,
    default_fg: TermColor,
    x: f32,
    y: f32,
    cell_w: f32,
    cell_h: f32,
    opacity: f32,
    focused: bool,
) {
    let thickness = (cell_h / 14.0).max(1.0);
    let fg = default_fg.to_f32_rgba(opacity * 0.9);

    match shape {
        TermCursorShape::Hidden => {}
        TermCursorShape::Beam => {
            let bar_w = thickness.max(2.0);
            rects.push(RectInstance {
                pos: [x, y],
                size: [bar_w, cell_h],
                color: fg,
            });
        }
        TermCursorShape::Underline => {
            rects.push(RectInstance {
                pos: [x, y + cell_h - thickness * 2.0],
                size: [cell_w, thickness],
                color: fg,
            });
        }
        TermCursorShape::Block if focused => {
            rects.push(RectInstance {
                pos: [x, y],
                size: [cell_w, cell_h],
                color: fg,
            });
        }
        TermCursorShape::Block | TermCursorShape::HollowBlock => {
            let outline = [
                ([x, y], [cell_w, thickness]),
                ([x, y + cell_h - thickness], [cell_w, thickness]),
                ([x, y], [thickness, cell_h]),
                ([x + cell_w - thickness, y], [thickness, cell_h]),
            ];
            for (pos, size) in outline {
                rects.push(RectInstance {
                    pos,
                    size,
                    color: fg,
                });
            }
        }
    }
}
