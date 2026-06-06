//! Frame builder: walk the grid state + animation state and emit flat instance
//! draw lists (AGENT_RUST_PORT.md §5.1). Draw order: rects then glyphs.

use bytemuck::{Pod, Zeroable};
use nvim_core::grid::GridStateStore;
use nvim_core::protocol::CursorShape;

use crate::animation::AnimationState;
use crate::atlas::GlyphAtlas;
use crate::box_glyphs;
use crate::color::{resolve_cell, rgb_to_rgba, Rgba};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct RectInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GlyphInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub color: [f32; 4],
}

#[derive(Default)]
pub struct DrawLists {
    pub rects: Vec<RectInstance>,
    pub glyphs: Vec<GlyphInstance>,
    pub clear: [f32; 4],
}

pub struct FrameBuilder;

impl FrameBuilder {
    pub fn build(
        store: &GridStateStore,
        anim: &AnimationState,
        atlas: &mut GlyphAtlas,
        queue: &wgpu::Queue,
    ) -> DrawLists {
        let mut lists = DrawLists {
            clear: rgb_to_rgba(store.default_colors.bg),
            ..Default::default()
        };
        let cell_w = atlas.cell_w;
        let cell_h = atlas.cell_h;
        let thickness = (atlas.size_px / 12.0).max(1.0);

        // Cursor block invert: skip the underlying glyph and redraw it inverted.
        let block_cursor = matches!(store.cursor_shape(), CursorShape::Block);
        let cursor_visible = anim.cursor_visible(store) && !store.busy;
        let skip_cell = if block_cursor && cursor_visible {
            Some((store.cursor.grid, store.cursor.row, store.cursor.col))
        } else {
            None
        };

        // Grids: primary first, floats after, sorted by z-index.
        let mut grid_ids: Vec<i64> = store.grids.keys().copied().collect();
        grid_ids.sort_by_key(|id| {
            let z = store.grid(*id).and_then(|g| g.float).map(|f| f.z_index).unwrap_or(-1);
            (z, *id)
        });

        for grid_id in grid_ids {
            let Some(grid) = store.grid(grid_id) else { continue };
            let off_y = anim.scroll_offset(grid_id);

            for row in 0..grid.height {
                for col in 0..grid.width {
                    let cell = match grid.cell(row, col) {
                        Some(c) => c,
                        None => continue,
                    };
                    if cell.double_width_continuation {
                        continue;
                    }
                    let width_mult = if cell.double_width { 2.0 } else { 1.0 };
                    let x = col as f32 * cell_w;
                    let y = row as f32 * cell_h + off_y;
                    let colors = resolve_cell(store, cell.hl_id);

                    // Background rect (only if non-default).
                    if !colors.bg_is_default {
                        lists.rects.push(RectInstance {
                            pos: [x, y],
                            size: [cell_w * width_mult, cell_h],
                            color: colors.bg,
                        });
                    }

                    let is_cursor_cell = skip_cell == Some((grid_id, row, col));
                    let glyph_color = if is_cursor_cell { anim.glyph_color } else { colors.fg };

                    if is_cursor_cell {
                        // Drawn below as the inverted cursor glyph; still need the
                        // cell rendered, so fall through to emit it with cursor color.
                    }

                    let ch = cell.text.chars().next();
                    if let Some(ch) = ch {
                        if ch != ' ' && ch != '\0' {
                            if box_glyphs::is_box_glyph(ch) {
                                for (rx, ry, rw, rh) in
                                    box_glyphs::box_rects(ch, x, y, cell_w * width_mult, cell_h, thickness)
                                {
                                    lists.rects.push(RectInstance {
                                        pos: [rx, ry],
                                        size: [rw, rh],
                                        color: glyph_color,
                                    });
                                }
                            } else if let Some(info) = atlas.glyph(queue, ch) {
                                lists.glyphs.push(GlyphInstance {
                                    pos: [x + info.left, y + info.top],
                                    size: [info.width, info.height],
                                    uv_min: info.uv_min,
                                    uv_max: info.uv_max,
                                    color: glyph_color,
                                });
                            }
                        }
                    }

                    // Decorations (underline/undercurl/strikethrough) as rects.
                    if let Some(attr) = store.highlight(cell.hl_id) {
                        let dec_color = colors.sp;
                        let lw = (thickness * 0.8).max(1.0);
                        if attr.underline || attr.undercurl || attr.underdouble {
                            lists.rects.push(RectInstance {
                                pos: [x, y + cell_h - lw],
                                size: [cell_w * width_mult, lw],
                                color: dec_color,
                            });
                            if attr.underdouble {
                                lists.rects.push(RectInstance {
                                    pos: [x, y + cell_h - lw * 3.0],
                                    size: [cell_w * width_mult, lw],
                                    color: dec_color,
                                });
                            }
                        }
                        if attr.strikethrough {
                            lists.rects.push(RectInstance {
                                pos: [x, y + cell_h * 0.5],
                                size: [cell_w * width_mult, lw],
                                color: dec_color,
                            });
                        }
                    }
                }
            }
        }

        Self::build_cursor_effects(&mut lists, store, anim, cursor_visible);
        Self::build_particles(&mut lists, anim);
        lists
    }

    fn build_cursor_effects(
        lists: &mut DrawLists,
        store: &GridStateStore,
        anim: &AnimationState,
        cursor_visible: bool,
    ) {
        // Flashes (drawn under everything cursor-related, on top of cells).
        let fd = anim.cfg.flash_duration;
        for f in &anim.flashes {
            let a = (1.0 - f.age / fd) * 0.35;
            if a <= 0.0 {
                continue;
            }
            lists.rects.push(RectInstance {
                pos: [f.rect.x, f.rect.y],
                size: [f.rect.w, f.rect.h],
                color: with_alpha(anim.fill_color, a),
            });
        }

        // Trail samples (behind the cursor).
        for s in &anim.trail {
            let a = (1.0 - s.age / 0.35) * 0.35;
            if a <= 0.0 {
                continue;
            }
            lists.rects.push(RectInstance {
                pos: [s.rect.x, s.rect.y],
                size: [s.rect.w, s.rect.h],
                color: with_alpha(anim.fill_color, a),
            });
        }

        if !cursor_visible {
            return;
        }
        let cur = anim.render_cursor();

        // Glow (concentric expanding rects behind the cursor).
        if anim.cfg.enable_cursor_glow {
            let layers = anim.cfg.cursor_glow_layers.max(0);
            for layer in 0..layers {
                let f = layer as f32 / layers.max(1) as f32;
                let expand = anim.cfg.cursor_glow_radius * f;
                let a = anim.cfg.cursor_glow_alpha * (1.0 - f);
                lists.rects.push(RectInstance {
                    pos: [cur.x - expand, cur.y - expand],
                    size: [cur.w + expand * 2.0, cur.h + expand * 2.0],
                    color: with_alpha(anim.fill_color, a),
                });
            }
        }

        // Cursor body.
        lists.rects.push(RectInstance {
            pos: [cur.x, cur.y],
            size: [cur.w, cur.h],
            color: anim.fill_color,
        });

        let _ = store;
    }

    fn build_particles(lists: &mut DrawLists, anim: &AnimationState) {
        let plife = anim.cfg.particle_lifetime;
        for p in &anim.particles {
            let a = 1.0 - p.age / plife;
            if a <= 0.0 {
                continue;
            }
            lists.rects.push(RectInstance {
                pos: [p.x - 1.5, p.y - 1.5],
                size: [3.0, 3.0],
                color: with_alpha(p.color, a),
            });
        }
    }
}

#[inline]
fn with_alpha(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], a.clamp(0.0, 1.0)]
}
