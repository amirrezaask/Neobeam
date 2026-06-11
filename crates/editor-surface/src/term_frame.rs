//! Frame builder for terminal panes. Converts a `TermGrid` into `DrawLists`
//! using the same glyph atlas and GPU instance types as the nvim renderer.

use terminal_core::session::TermListener;
use terminal_core::{TermGrid, Underline};

use crate::atlas::GlyphAtlas;
use crate::frame::{DrawBatch, DrawLists, GlyphInstance, RectInstance};

pub struct TermFrameBuilder;

impl TermFrameBuilder {
    /// Build draw lists for one terminal pane.
    ///
    /// `origin` is the top-left corner of the pane in logical pixels.
    /// `queue` is needed by the atlas rasterizer.
    pub fn build(
        grid: &TermGrid<TermListener>,
        atlas: &mut GlyphAtlas,
        queue: &wgpu::Queue,
        origin: [f32; 2],
        cell_w: f32,
        cell_h: f32,
    ) -> DrawLists {
        let mut rects: Vec<RectInstance> = Vec::new();
        let mut glyphs: Vec<GlyphInstance> = Vec::new();

        let (default_fg, default_bg) = grid.default_colors();
        let clear = default_bg.to_f32_rgba(1.0);

        grid.with_cells(|row, col, cell| {
            let x = origin[0] + col as f32 * cell_w;
            let y = origin[1] + row as f32 * cell_h;
            let (fg, bg) = if cell.inverse ^ cell.selected {
                (cell.bg, cell.fg)
            } else {
                (cell.fg, cell.bg)
            };

            let bg = bg.to_f32_rgba(1.0);
            // Only emit a bg rect when it differs from the default background
            // (cheap overdraw reduction).
            let default_bg_arr = default_bg.to_f32_rgba(1.0);
            if bg != default_bg_arr {
                rects.push(RectInstance {
                    pos: [x, y],
                    size: [cell_w, cell_h],
                    color: bg,
                });
            }

            let glyph_alpha = if cell.dim { 0.55 } else { 1.0 };
            if !cell.hidden && !cell.spacer && cell.ch != ' ' && cell.ch != '\0' {
                let mut chars = Vec::with_capacity(cell.zerowidth.len() + 1);
                chars.push(cell.ch);
                chars.extend(cell.zerowidth.iter().copied());
                for ch in chars {
                    if let Some(info) = atlas.glyph(queue, ch) {
                        let gx = x + info.left;
                        // GlyphInfo::top is already the logical offset from the
                        // cell top, including ascent and vertical centering.
                        let gy = y + info.top;
                        glyphs.push(GlyphInstance {
                            pos: [gx, gy],
                            size: [info.width, info.height],
                            uv_min: info.uv_min,
                            uv_max: info.uv_max,
                            color: fg.to_f32_rgba(glyph_alpha),
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
        });

        // Cursor block.
        let (cur_row, cur_col) = grid.cursor();
        let cx = origin[0] + cur_col as f32 * cell_w;
        let cy = origin[1] + cur_row as f32 * cell_h;
        let fg = default_fg.to_f32_rgba(0.8);
        rects.push(RectInstance {
            pos: [cx, cy],
            size: [cell_w, cell_h],
            color: fg,
        });

        let rect_count = rects.len() as u32;
        let glyph_count = glyphs.len() as u32;

        let mut batches = Vec::new();
        if rect_count > 0 {
            batches.push(DrawBatch::Rects {
                start: 0,
                count: rect_count,
                scissor: None,
            });
        }
        if glyph_count > 0 {
            batches.push(DrawBatch::Glyphs {
                start: 0,
                count: glyph_count,
                scissor: None,
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
