//! Frame builder: walk grid state + animation state into flat draw lists.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use nvim_core::grid::{Cell, Grid, GridStateStore, WindowMeta};
use nvim_core::protocol::CursorShape;

use crate::animation::AnimationState;
use crate::atlas::GlyphAtlas;
use crate::box_glyphs;
use crate::color::{resolve_cell, rgb_to_rgba, Rgba};

/// Vertical slide distance (px) for float fade-in/out.
const FLOAT_SLIDE_PX: f32 = 8.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct RectInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct QuadInstance {
    pub corners: [[f32; 2]; 4],
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

#[derive(Clone, Copy, Debug, Default)]
pub struct ScissorRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Copy, Debug)]
pub enum DrawBatch {
    Rects {
        start: u32,
        count: u32,
        scissor: Option<ScissorRect>,
    },
    Glyphs {
        start: u32,
        count: u32,
        scissor: Option<ScissorRect>,
    },
    Quads {
        start: u32,
        count: u32,
        scissor: Option<ScissorRect>,
    },
}

#[derive(Default)]
pub struct DrawLists {
    pub rects: Vec<RectInstance>,
    pub glyphs: Vec<GlyphInstance>,
    pub quads: Vec<QuadInstance>,
    pub batches: Vec<DrawBatch>,
    pub clear: [f32; 4],
}

/// Snapshot kept for float fade-out after the window/grid is removed.
#[derive(Clone)]
pub struct FloatCacheEntry {
    pub grid: Grid,
    pub window: WindowMeta,
}

pub type FloatCache = HashMap<i64, FloatCacheEntry>;

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
    slide_y: f32,
}

pub struct FrameBuilder;

impl FrameBuilder {
    pub fn build(
        store: &GridStateStore,
        anim: &AnimationState,
        atlas: &mut GlyphAtlas,
        queue: &wgpu::Queue,
        float_cache: &FloatCache,
        hide_cursor: bool,
    ) -> DrawLists {
        // `guibg=NONE` keeps the editor background transparent so the colorscheme
        // can show the desktop through; otherwise the background is fully opaque.
        let default_bg_alpha = if store.default_colors.bg_none {
            0.0
        } else {
            1.0
        };
        let mut lists = DrawLists {
            clear: rgb_to_rgba(store.default_colors.bg),
            ..Default::default()
        };
        lists.clear[3] = default_bg_alpha;
        let cell_w = atlas.cell_w;
        let cell_h = atlas.cell_h;
        let thickness = (atlas.size_px / 12.0).max(1.0);

        let block_cursor = matches!(store.cursor_shape(), CursorShape::Block);
        let cursor_visible = !hide_cursor && anim.cursor_visible() && !store.busy;
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
            let Some(grid) = store.grid(grid_id) else {
                continue;
            };
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
            let slide_y = if is_float && anim.cfg.enable_float_animation {
                (1.0 - opacity) * FLOAT_SLIDE_PX
            } else {
                0.0
            };
            draw_grid(
                &mut lists,
                store,
                grid_id,
                grid,
                win,
                opacity,
                default_bg_alpha,
                DrawOpts {
                    clip: !is_float,
                    scroll: !is_float,
                    slide_y,
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
            let Some(cached) = float_cache.get(&grid_id) else {
                continue;
            };
            let opacity = anim.float_opacity(grid_id);
            if opacity <= 0.01 {
                continue;
            }
            let slide_y = if anim.cfg.enable_float_animation {
                (1.0 - opacity) * FLOAT_SLIDE_PX
            } else {
                0.0
            };
            draw_grid(
                &mut lists,
                store,
                grid_id,
                &cached.grid,
                Some(&cached.window),
                opacity,
                default_bg_alpha,
                DrawOpts {
                    clip: false,
                    scroll: false,
                    slide_y,
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

        build_cursor_effects(&mut lists, store, anim, cursor_visible, cell_w, cell_h);
        lists
    }
}

fn push_rect_batch(lists: &mut DrawLists, scissor: Option<ScissorRect>, prev_len: usize) {
    let count = lists.rects.len() - prev_len;
    if count > 0 {
        lists.batches.push(DrawBatch::Rects {
            start: prev_len as u32,
            count: count as u32,
            scissor,
        });
    }
}

fn push_glyph_batch(lists: &mut DrawLists, scissor: Option<ScissorRect>, prev_len: usize) {
    let count = lists.glyphs.len() - prev_len;
    if count > 0 {
        lists.batches.push(DrawBatch::Glyphs {
            start: prev_len as u32,
            count: count as u32,
            scissor,
        });
    }
}

fn push_quad_batch(lists: &mut DrawLists, scissor: Option<ScissorRect>, prev_len: usize) {
    let count = lists.quads.len() - prev_len;
    if count > 0 {
        lists.batches.push(DrawBatch::Quads {
            start: prev_len as u32,
            count: count as u32,
            scissor,
        });
    }
}

fn draw_grid(
    lists: &mut DrawLists,
    store: &GridStateStore,
    grid_id: i64,
    grid: &Grid,
    win: Option<&WindowMeta>,
    opacity: f32,
    default_bg_alpha: f32,
    opts: DrawOpts,
    anim: &AnimationState,
    atlas: &mut GlyphAtlas,
    queue: &wgpu::Queue,
    cell_w: f32,
    cell_h: f32,
    thickness: f32,
    skip_cell: Option<(i64, u32, u32)>,
) {
    let win_state = anim.windows.get(grid_id);
    let (win_row, win_col) = if let Some(w) = win_state {
        (w.grid_current.row, w.grid_current.col)
    } else {
        (
            win.map(|w| w.row).unwrap_or(0) as f32,
            win.map(|w| w.col).unwrap_or(0) as f32,
        )
    };

    let grid_x = win_col * cell_w;
    let grid_y = win_row * cell_h + opts.slide_y;
    let grid_w = grid.width as f32 * cell_w;
    let grid_h = grid.height as f32 * cell_h;

    let scroll_off = if opts.scroll {
        win_state
            .map(|w| w.scroll_offset_pixels(cell_h))
            .unwrap_or(0.0)
    } else {
        0.0
    };

    let scissor = if opts.clip {
        Some(ScissorRect {
            x: grid_x.max(0.0) as u32,
            y: grid_y.max(0.0) as u32,
            w: grid_w.max(0.0) as u32,
            h: grid_h.max(0.0) as u32,
        })
    } else {
        None
    };

    let rect_start = lists.rects.len();
    let glyph_start = lists.glyphs.len();
    let cursor_glyph = anim.glyph_color;

    let bg_alpha = opacity * default_bg_alpha;
    if bg_alpha > 0.0 {
        lists.rects.push(RectInstance {
            pos: [grid_x, grid_y],
            size: [grid_w, grid_h],
            color: scale_alpha(rgb_to_rgba(store.default_colors.bg), bg_alpha),
        });
    }

    let use_scrollback = opts.scroll && anim.cfg.enable_smooth_scroll && win_state.is_some();

    let top_margin = win_state.map(|w| w.top_margin).unwrap_or(0);
    let bottom_margin = win_state.map(|w| w.bottom_margin).unwrap_or(0);
    let has_margins = top_margin + bottom_margin > 0;
    let top_inset = top_margin as f32 * cell_h;
    let bottom_inset = bottom_margin as f32 * cell_h;
    let inner_h = grid.height.saturating_sub(top_margin + bottom_margin) as isize;

    let inner_scissor = if opts.clip && has_margins {
        Some(ScissorRect {
            x: grid_x.max(0.0) as u32,
            y: (grid_y + top_inset).max(0.0) as u32,
            w: grid_w.max(0.0) as u32,
            h: (grid_h - top_inset - bottom_inset).max(0.0) as u32,
        })
    } else {
        scissor
    };

    if has_margins {
        if let Some(w) = win_state {
            // ---- Pinned chrome rows (winbar / statusline / float borders) ----
            for row in 0..top_margin {
                if let Some(line) = w.border_line(row as usize) {
                    draw_line(
                        lists,
                        store,
                        grid_id,
                        line,
                        grid_x,
                        grid_y + row as f32 * cell_h,
                        cell_w,
                        cell_h,
                        thickness,
                        opacity,
                        skip_cell,
                        row,
                        cursor_glyph,
                        atlas,
                        queue,
                    );
                }
            }
            let bottom_start = grid.height.saturating_sub(bottom_margin);
            for row in bottom_start..grid.height {
                if let Some(line) = w.border_line(row as usize) {
                    draw_line(
                        lists,
                        store,
                        grid_id,
                        line,
                        grid_x,
                        grid_y + row as f32 * cell_h,
                        cell_w,
                        cell_h,
                        thickness,
                        opacity,
                        skip_cell,
                        row,
                        cursor_glyph,
                        atlas,
                        queue,
                    );
                }
            }
            // Chrome region: rects (incl. grid background) THEN glyphs.
            push_rect_batch(lists, scissor, rect_start);
            push_glyph_batch(lists, scissor, glyph_start);

            // ---- Scrollable content (clipped to the inner region) ----
            let content_rect_start = lists.rects.len();
            let content_glyph_start = lists.glyphs.len();
            if use_scrollback {
                let scroll_offset_lines = w.scroll_animation.position.floor() as u32;
                for inner_row in 0..inner_h + 1 {
                    let Some(line) = w.line_at(inner_row) else {
                        continue;
                    };
                    let y = grid_y + top_inset + scroll_off + inner_row as f32 * cell_h;
                    let inner_y0 = grid_y + top_inset;
                    let inner_y1 = grid_y + grid_h - bottom_inset;
                    if opts.clip && (y + cell_h <= inner_y0 || y >= inner_y1) {
                        continue;
                    }
                    let grid_row = top_margin + scroll_offset_lines + inner_row as u32;
                    draw_line(
                        lists,
                        store,
                        grid_id,
                        line,
                        grid_x,
                        y,
                        cell_w,
                        cell_h,
                        thickness,
                        opacity,
                        skip_cell,
                        grid_row,
                        cursor_glyph,
                        atlas,
                        queue,
                    );
                }
            } else {
                for row in top_margin..bottom_start {
                    let inner_row = row - top_margin;
                    let y = grid_y + top_inset + inner_row as f32 * cell_h + scroll_off;
                    for col in 0..grid.width {
                        let cell = match grid.cell(row, col) {
                            Some(c) => c,
                            None => continue,
                        };
                        draw_cell(
                            lists,
                            store,
                            grid_id,
                            row,
                            col,
                            cell,
                            grid_x,
                            y,
                            cell_w,
                            cell_h,
                            thickness,
                            opacity,
                            opts.clip,
                            grid_x,
                            grid_y + top_inset,
                            grid_w,
                            grid_h - top_inset - bottom_inset,
                            skip_cell,
                            cursor_glyph,
                            atlas,
                            queue,
                        );
                    }
                }
            }
            push_rect_batch(lists, inner_scissor, content_rect_start);
            push_glyph_batch(lists, inner_scissor, content_glyph_start);
        }
    } else if use_scrollback {
        if let Some(w) = win_state {
            for inner_row in 0..grid.height as isize + 1 {
                let Some(line) = w.line_at(inner_row) else {
                    continue;
                };
                let y = grid_y + scroll_off + inner_row as f32 * cell_h;
                if opts.clip && (y + cell_h <= grid_y || y >= grid_y + grid_h) {
                    continue;
                }
                draw_line(
                    lists,
                    store,
                    grid_id,
                    line,
                    grid_x,
                    y,
                    cell_w,
                    cell_h,
                    thickness,
                    opacity,
                    skip_cell,
                    inner_row as u32,
                    cursor_glyph,
                    atlas,
                    queue,
                );
            }
        }
        push_rect_batch(lists, scissor, rect_start);
        push_glyph_batch(lists, scissor, glyph_start);
    } else {
        for row in 0..grid.height {
            for col in 0..grid.width {
                let cell = match grid.cell(row, col) {
                    Some(c) => c,
                    None => continue,
                };
                draw_cell(
                    lists,
                    store,
                    grid_id,
                    row,
                    col,
                    cell,
                    grid_x,
                    grid_y + row as f32 * cell_h + scroll_off,
                    cell_w,
                    cell_h,
                    thickness,
                    opacity,
                    opts.clip,
                    grid_x,
                    grid_y,
                    grid_w,
                    grid_h,
                    skip_cell,
                    cursor_glyph,
                    atlas,
                    queue,
                );
            }
        }
        push_rect_batch(lists, scissor, rect_start);
        push_glyph_batch(lists, scissor, glyph_start);
    }
}

fn draw_line(
    lists: &mut DrawLists,
    store: &GridStateStore,
    grid_id: i64,
    line: &[Cell],
    grid_x: f32,
    y: f32,
    cell_w: f32,
    cell_h: f32,
    thickness: f32,
    opacity: f32,
    skip_cell: Option<(i64, u32, u32)>,
    row: u32,
    cursor_glyph: Rgba,
    atlas: &mut GlyphAtlas,
    queue: &wgpu::Queue,
) {
    for (col, cell) in line.iter().enumerate() {
        draw_cell(
            lists,
            store,
            grid_id,
            row,
            col as u32,
            cell,
            grid_x,
            y,
            cell_w,
            cell_h,
            thickness,
            opacity,
            false,
            0.0,
            0.0,
            f32::MAX,
            f32::MAX,
            skip_cell,
            cursor_glyph,
            atlas,
            queue,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_cell(
    lists: &mut DrawLists,
    store: &GridStateStore,
    grid_id: i64,
    row: u32,
    col: u32,
    cell: &Cell,
    x: f32,
    y: f32,
    cell_w: f32,
    cell_h: f32,
    thickness: f32,
    opacity: f32,
    clip: bool,
    grid_x: f32,
    grid_y: f32,
    grid_w: f32,
    grid_h: f32,
    skip_cell: Option<(i64, u32, u32)>,
    cursor_glyph: Rgba,
    atlas: &mut GlyphAtlas,
    queue: &wgpu::Queue,
) {
    if cell.double_width_continuation {
        return;
    }
    // `x` is passed as the grid's left edge; offset by the column here.
    let x = x + col as f32 * cell_w;
    let width_mult = if cell.double_width { 2.0 } else { 1.0 };
    let cw = cell_w * width_mult;

    if clip {
        if !rect_intersects(x, y, cw, cell_h, grid_x, grid_y, grid_w, grid_h) {
            return;
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
        scale_alpha(cursor_glyph, opacity)
    } else {
        scale_alpha(colors.fg, opacity)
    };

    if let Some(ch) = cell.text.chars().next() {
        if ch != ' ' && ch != '\0' {
            if box_glyphs::is_box_glyph(ch) {
                for (rx, ry, rw, rh) in box_glyphs::box_rects(ch, x, y, cw, cell_h, thickness) {
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

fn build_cursor_effects(
    lists: &mut DrawLists,
    store: &GridStateStore,
    anim: &AnimationState,
    cursor_visible: bool,
    cell_w: f32,
    cell_h: f32,
) {
    let scroll_off = anim
        .windows
        .get(store.cursor.grid)
        .map(|w| w.scroll_offset_pixels(cell_h))
        .unwrap_or(0.0);
    let fd = anim.cfg.flash_duration;

    let rect_start = lists.rects.len();
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
    anim.emit_vfx_rects(&mut lists.rects);
    push_rect_batch(lists, None, rect_start);

    if !cursor_visible {
        return;
    }

    let mut quad = anim.cursor_quad();
    for c in &mut quad.corners {
        c[1] += scroll_off;
    }

    if anim.cfg.enable_cursor_glow {
        let rect_start = lists.rects.len();
        let layers = anim.cfg.cursor_glow_layers.max(0);
        let cx = (quad.corners[0][0] + quad.corners[2][0]) * 0.5;
        let cy = (quad.corners[0][1] + quad.corners[2][1]) * 0.5;
        let qw = (quad.corners[1][0] - quad.corners[0][0]).abs();
        let qh = (quad.corners[3][1] - quad.corners[0][1]).abs();
        for layer in 0..layers {
            let f = layer as f32 / layers.max(1) as f32;
            let expand = anim.cfg.cursor_glow_radius * f;
            let a = anim.cfg.cursor_glow_alpha * (1.0 - f);
            lists.rects.push(RectInstance {
                pos: [cx - qw * 0.5 - expand, cy - qh * 0.5 - expand],
                size: [qw + expand * 2.0, qh + expand * 2.0],
                color: with_alpha(anim.fill_color, a),
            });
        }
        push_rect_batch(lists, None, rect_start);
    }

    let quad_start = lists.quads.len();
    if anim.use_outline_cursor(store) {
        let outline = anim.cfg.unfocused_outline_width * cell_w;
        for mut q in anim.cursor_outline_quads(outline) {
            for c in &mut q.corners {
                c[1] += scroll_off;
            }
            lists.quads.push(q);
        }
    } else {
        lists.quads.push(quad);
    }
    anim.emit_vfx_quads(&mut lists.quads);
    push_quad_batch(lists, None, quad_start);
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
