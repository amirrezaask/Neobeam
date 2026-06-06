//! Frame builder: walk grid state + animation state into flat draw lists.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use nvim_core::grid::{Grid, GridStateStore, WindowMeta};
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

/// Snapshot kept for float fade-out after the window/grid is removed.
#[derive(Clone)]
pub struct FloatCacheEntry {
    pub grid: Grid,
    pub window: WindowMeta,
}

pub type FloatCache = HashMap<i64, FloatCacheEntry>;

/// Refresh cached float grid snapshots for active floating windows.
pub fn sync_float_cache(store: &GridStateStore, cache: &mut FloatCache) {
    for (&id, win) in &store.windows {
        if !win.is_float {
            continue;
        }
        if let Some(grid) = store.grid(id) {
            cache.insert(
                id,
                FloatCacheEntry {
                    grid: grid.clone(),
                    window: *win,
                },
            );
        }
    }
}

struct DrawOpts {
    clip: bool,
    scroll: bool,
}

pub struct FrameBuilder;

impl FrameBuilder {
    pub fn build(
        store: &GridStateStore,
        anim: &AnimationState,
        atlas: &mut GlyphAtlas,
        queue: &wgpu::Queue,
        float_cache: &FloatCache,
    ) -> DrawLists {
        let mut lists = DrawLists {
            clear: rgb_to_rgba(store.default_colors.bg),
            ..Default::default()
        };
        let cell_w = atlas.cell_w;
        let cell_h = atlas.cell_h;
        let thickness = (atlas.size_px / 12.0).max(1.0);

        let block_cursor = matches!(store.cursor_shape(), CursorShape::Block);
        let cursor_visible = anim.cursor_visible(store) && !store.busy;
        let skip_cell = if block_cursor && cursor_visible {
            Some((store.cursor.grid, store.cursor.row, store.cursor.col))
        } else {
            None
        };

        let mut grid_ids: Vec<i64> = store.grids.keys().copied().collect();
        grid_ids.sort_by_key(|id| {
            let is_float = store.window(*id).map(|w| w.is_float).unwrap_or(false) as i64;
            let z = store.window(*id).map(|w| w.z_index).unwrap_or(0);
            (is_float, z, *id)
        });

        for grid_id in grid_ids {
            if grid_id != 1 && store.window(grid_id).is_none() {
                continue;
            }
            let Some(grid) = store.grid(grid_id) else { continue };
            let win = store.window(grid_id);
            let is_float = win.map(|w| w.is_float).unwrap_or(false);
            let opacity = if is_float {
                anim.float_opacity(grid_id)
            } else {
                1.0
            };
            if is_float && opacity <= 0.01 {
                continue;
            }
            draw_grid(
                &mut lists,
                store,
                grid_id,
                grid,
                win,
                opacity,
                DrawOpts {
                    clip: !is_float,
                    scroll: !is_float,
                },
                anim,
                atlas,
                queue,
                cell_w,
                cell_h,
                thickness,
                skip_cell,
            );
        }

        for grid_id in anim.fading_out_float_ids() {
            if store.window(grid_id).map(|w| w.is_float).unwrap_or(false) {
                continue;
            }
            let Some(cached) = float_cache.get(&grid_id) else { continue };
            let opacity = anim.float_opacity(grid_id);
            if opacity <= 0.01 {
                continue;
            }
            draw_grid(
                &mut lists,
                store,
                grid_id,
                &cached.grid,
                Some(&cached.window),
                opacity,
                DrawOpts {
                    clip: false,
                    scroll: false,
                },
                anim,
                atlas,
                queue,
                cell_w,
                cell_h,
                thickness,
                None,
            );
        }

        build_cursor_effects(&mut lists, store, anim, cursor_visible);
        build_particles(&mut lists, anim);
        lists
    }
}

fn draw_grid(
    lists: &mut DrawLists,
    store: &GridStateStore,
    grid_id: i64,
    grid: &Grid,
    win: Option<&WindowMeta>,
    opacity: f32,
    opts: DrawOpts,
    anim: &AnimationState,
    atlas: &mut GlyphAtlas,
    queue: &wgpu::Queue,
    cell_w: f32,
    cell_h: f32,
    thickness: f32,
    skip_cell: Option<(i64, u32, u32)>,
) {
    let win_col = win.map(|w| w.col).unwrap_or(0) as f32;
    let win_row = win.map(|w| w.row).unwrap_or(0) as f32;
    let grid_x = win_col * cell_w;
    let grid_y = win_row * cell_h;
    let grid_w = grid.width as f32 * cell_w;
    let grid_h = grid.height as f32 * cell_h;
    let scroll_off = if opts.scroll {
        anim.scroll_offset(grid_id)
    } else {
        0.0
    };

    // Opaque background for the grid viewport.
    lists.rects.push(RectInstance {
        pos: [grid_x, grid_y],
        size: [grid_w, grid_h],
        color: scale_alpha(rgb_to_rgba(store.default_colors.bg), opacity),
    });

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
            let x = grid_x + col as f32 * cell_w;
            let y = grid_y + row as f32 * cell_h + scroll_off;
            let cw = cell_w * width_mult;

            if opts.clip {
                if !rect_intersects(x, y, cw, cell_h, grid_x, grid_y, grid_w, grid_h) {
                    continue;
                }
            }

            let colors = resolve_cell(store, cell.hl_id);
            if !colors.bg_is_default {
                lists.rects.push(RectInstance {
                    pos: [x, y],
                    size: [cw, cell_h],
                    color: scale_alpha(colors.bg, opacity),
                });
            }

            let is_cursor_cell = skip_cell == Some((grid_id, row, col));
            let glyph_color = if is_cursor_cell {
                scale_alpha(anim.glyph_color, opacity)
            } else {
                scale_alpha(colors.fg, opacity)
            };

            if let Some(ch) = cell.text.chars().next() {
                if ch != ' ' && ch != '\0' {
                    if box_glyphs::is_box_glyph(ch) {
                        for (rx, ry, rw, rh) in
                            box_glyphs::box_rects(ch, x, y, cw, cell_h, thickness)
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

            if let Some(attr) = store.highlight(cell.hl_id) {
                let dec_color = scale_alpha(colors.sp, opacity);
                let lw = (thickness * 0.8).max(1.0);
                if attr.underline || attr.undercurl || attr.underdouble {
                    lists.rects.push(RectInstance {
                        pos: [x, y + cell_h - lw],
                        size: [cw, lw],
                        color: dec_color,
                    });
                    if attr.underdouble {
                        lists.rects.push(RectInstance {
                            pos: [x, y + cell_h - lw * 3.0],
                            size: [cw, lw],
                            color: dec_color,
                        });
                    }
                }
                if attr.strikethrough {
                    lists.rects.push(RectInstance {
                        pos: [x, y + cell_h * 0.5],
                        size: [cw, lw],
                        color: dec_color,
                    });
                }
            }
        }
    }
}

fn build_cursor_effects(
    lists: &mut DrawLists,
    store: &GridStateStore,
    anim: &AnimationState,
    cursor_visible: bool,
) {
    let scroll_off = anim.scroll_offset(store.cursor.grid);
    let fd = anim.cfg.flash_duration;
    for f in &anim.flashes {
        let a = (1.0 - f.age / fd) * 0.35;
        if a <= 0.0 {
            continue;
        }
        lists.rects.push(RectInstance {
            pos: [f.rect.x, f.rect.y + scroll_off],
            size: [f.rect.w, f.rect.h],
            color: with_alpha(anim.fill_color, a),
        });
    }

    for s in &anim.trail {
        let a = (1.0 - s.age / 0.35) * 0.35;
        if a <= 0.0 {
            continue;
        }
        lists.rects.push(RectInstance {
            pos: [s.rect.x, s.rect.y + scroll_off],
            size: [s.rect.w, s.rect.h],
            color: with_alpha(anim.fill_color, a),
        });
    }

    if !cursor_visible {
        return;
    }
    let cur = anim.render_cursor();

    if anim.cfg.enable_cursor_glow {
        let layers = anim.cfg.cursor_glow_layers.max(0);
        for layer in 0..layers {
            let f = layer as f32 / layers.max(1) as f32;
            let expand = anim.cfg.cursor_glow_radius * f;
            let a = anim.cfg.cursor_glow_alpha * (1.0 - f);
            lists.rects.push(RectInstance {
                pos: [cur.x - expand, cur.y - expand + scroll_off],
                size: [cur.w + expand * 2.0, cur.h + expand * 2.0],
                color: with_alpha(anim.fill_color, a),
            });
        }
    }

    lists.rects.push(RectInstance {
        pos: [cur.x, cur.y + scroll_off],
        size: [cur.w, cur.h],
        color: anim.fill_color,
    });
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

#[inline]
fn rect_intersects(x: f32, y: f32, w: f32, h: f32, cx: f32, cy: f32, cw: f32, ch: f32) -> bool {
    x + w > cx && x < cx + cw && y + h > cy && y < cy + ch
}

#[inline]
fn scale_alpha(c: Rgba, opacity: f32) -> Rgba {
    [c[0], c[1], c[2], c[3] * opacity.clamp(0.0, 1.0)]
}

#[inline]
fn with_alpha(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], a.clamp(0.0, 1.0)]
}
