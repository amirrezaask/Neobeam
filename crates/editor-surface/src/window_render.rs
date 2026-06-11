//! Per-window scrollback buffer, scroll spring, and position easing.

use std::collections::HashMap;
use std::ops::Range;

use nvim_core::grid::{Cell, Grid, GridStateStore, ViewportMargins, WindowMeta};

use crate::animation::AnimationConfig;
use crate::ring_buffer::RingBuffer;
use crate::spring::{ease_out_expo, ease_point, Spring};

pub type GridLine = Vec<Cell>;

#[derive(Debug, Clone, Copy, Default)]
pub struct GridPos {
    pub row: f32,
    pub col: f32,
}

pub struct WindowRenderState {
    pub grid_id: i64,
    pub grid_size: (u32, u32),
    pub hidden: bool,
    pub is_float: bool,

    actual_lines: Vec<Option<GridLine>>,
    scrollback_lines: RingBuffer<Option<GridLine>>,
    scroll_delta: isize,

    grid_start: GridPos,
    pub grid_current: GridPos,
    grid_destination: GridPos,
    position_t: f32,

    pub scroll_animation: Spring,

    /// Rows pinned to the top/bottom of the window (winbar, statusline, borders).
    pub top_margin: u32,
    pub bottom_margin: u32,
}

impl WindowRenderState {
    pub fn new(grid_id: i64) -> Self {
        Self {
            grid_id,
            grid_size: (0, 0),
            hidden: false,
            is_float: false,
            actual_lines: Vec::new(),
            scrollback_lines: RingBuffer::new(0, None),
            scroll_delta: 0,
            grid_start: GridPos::default(),
            grid_current: GridPos::default(),
            grid_destination: GridPos::default(),
            position_t: 2.0,
            scroll_animation: Spring::new(),
            top_margin: 0,
            bottom_margin: 0,
        }
    }

    fn sync_lines_from_grid(&mut self, grid: &Grid) {
        let h = grid.height as usize;
        self.actual_lines.resize(h, None);
        for row in 0..grid.height {
            let mut line = Vec::with_capacity(grid.width as usize);
            for col in 0..grid.width {
                line.push(grid.cell(row, col).cloned().unwrap_or_default());
            }
            self.actual_lines[row as usize] = Some(line);
        }
    }

    fn inner_line_slice(&self) -> Range<usize> {
        let height = self.grid_size.1 as usize;
        if height == 0 || self.top_margin + self.bottom_margin == 0 {
            return 0..height;
        }
        let top = self.top_margin as usize;
        let end = height.saturating_sub(self.bottom_margin as usize);
        if top >= end {
            // Margins don't fit (e.g. before the first grid_resize); scroll everything.
            return 0..height;
        }
        top..end
    }

    pub fn sync_from_store(
        &mut self,
        store: &GridStateStore,
        win: Option<&WindowMeta>,
        grid: &Grid,
    ) {
        self.grid_size = (grid.width, grid.height);
        self.is_float = win.map(|w| w.is_float).unwrap_or(false);
        self.hidden = false;

        let ViewportMargins { top, bottom, .. } = store.viewport_margins(self.grid_id);
        self.top_margin = top;
        self.bottom_margin = bottom;

        let dest = if let Some(w) = win {
            GridPos {
                row: w.row as f32,
                col: w.col as f32,
            }
        } else {
            GridPos::default()
        };

        if self.grid_destination.row != dest.row || self.grid_destination.col != dest.col {
            if self.grid_start.row.abs() > f32::EPSILON || self.grid_start.col.abs() > f32::EPSILON
            {
                self.position_t = 0.0;
                self.grid_start = self.grid_current;
            } else {
                self.position_t = 2.0;
                self.grid_start = dest;
            }
            self.grid_destination = dest;
        }

        let h = grid.height as usize;
        let height_changed = h != self.actual_lines.len();
        let inner_range = self.inner_line_slice();
        if height_changed {
            self.scroll_animation.reset();
            let inner_size = inner_range.len().max(1);
            self.scrollback_lines.resize(2 * inner_size, None);
            self.sync_lines_from_grid(grid);
            self.scrollback_lines
                .clone_from_iter(self.actual_lines[inner_range].iter());
        } else {
            self.sync_lines_from_grid(grid);
        }
    }

    pub fn add_scroll_delta(&mut self, delta: i64) {
        self.scroll_delta += delta as isize;
    }

    pub fn flush(&mut self, cfg: &AnimationConfig) {
        if self.actual_lines.is_empty() {
            return;
        }

        let inner_range = self.inner_line_slice();
        let inner_size = inner_range.len();
        if inner_size == 0 {
            self.scroll_delta = 0;
            self.scroll_animation.reset();
            return;
        }

        if inner_size != self.scrollback_lines.len() / 2 {
            self.scrollback_lines.resize(2 * inner_size, None);
            self.scrollback_lines
                .clone_from_iter(self.actual_lines[inner_range.clone()].iter());
            self.scroll_delta = 0;
            self.scroll_animation.reset();
            return;
        }

        let scroll_delta = self.scroll_delta;
        self.scrollback_lines.rotate(scroll_delta);
        self.scrollback_lines
            .clone_from_iter(self.actual_lines[inner_range].iter());

        if scroll_delta != 0 && cfg.enable_smooth_scroll {
            let mut scroll_offset = self.scroll_animation.position;
            let max_delta = self.scrollback_lines.len().saturating_sub(inner_size);

            if scroll_delta.unsigned_abs() > max_delta {
                let far_lines = cfg.scroll_animation_far_lines.min(inner_size as u32) as isize;
                scroll_offset = -(far_lines * scroll_delta.signum()) as f32;
                let empty_lines = if scroll_delta > 0 {
                    -far_lines..0
                } else {
                    inner_size as isize..inner_size as isize + far_lines
                };
                for i in empty_lines {
                    self.scrollback_lines[i] = None;
                }
            } else {
                scroll_offset -= scroll_delta as f32;
                scroll_offset = scroll_offset.clamp(-(max_delta as f32), max_delta as f32);
            }
            self.scroll_animation.position = scroll_offset;
        } else if scroll_delta != 0 {
            self.scroll_animation.reset();
        }

        self.scroll_delta = 0;
    }

    pub fn animate(&mut self, cfg: &AnimationConfig, dt: f32) -> bool {
        let mut animating = false;

        if self.position_t > 1.0 - f32::EPSILON {
            self.position_t = 2.0;
        } else if cfg.enable_float_animation || !self.is_float {
            animating = true;
            let len = cfg.position_animation_length.max(0.001);
            self.position_t = (self.position_t + dt / len).min(1.0);
        }

        let prev = self.grid_current;
        let start = [self.grid_start.col, self.grid_start.row];
        let end = [self.grid_destination.col, self.grid_destination.row];
        let pos = ease_point(start, end, self.position_t, ease_out_expo);
        self.grid_current = GridPos {
            col: pos[0],
            row: pos[1],
        };
        animating |= (self.grid_current.col - prev.col).abs() > f32::EPSILON
            || (self.grid_current.row - prev.row).abs() > f32::EPSILON;

        if cfg.enable_smooth_scroll {
            let scrolling = self
                .scroll_animation
                .update(dt, cfg.scroll_animation_length);
            animating |= scrolling;
        } else {
            self.scroll_animation.reset();
        }

        animating
    }

    pub fn scroll_offset_pixels(&self, cell_h: f32) -> f32 {
        let scroll_offset_lines = self.scroll_animation.position.floor();
        let scroll_offset = scroll_offset_lines - self.scroll_animation.position;
        (scroll_offset * cell_h).round()
    }

    /// Scrollable content line at `inner_row` (0 = first line below top margin).
    pub fn line_at(&self, inner_row: isize) -> Option<&GridLine> {
        if self.scrollback_lines.is_empty() {
            return None;
        }
        let scroll_offset = self.scroll_animation.position.floor() as isize;
        self.scrollback_lines[scroll_offset + inner_row].as_ref()
    }

    /// Fixed chrome line from the live grid (winbar, statusline, float borders).
    pub fn border_line(&self, row: usize) -> Option<&GridLine> {
        self.actual_lines.get(row)?.as_ref()
    }

    pub fn inner_row_count(&self) -> u32 {
        self.grid_size
            .1
            .saturating_sub(self.top_margin + self.bottom_margin)
    }

    pub fn has_fixed_margins(&self) -> bool {
        self.top_margin + self.bottom_margin > 0
    }
}

pub struct WindowAnimStore {
    windows: HashMap<i64, WindowRenderState>,
}

impl WindowAnimStore {
    pub fn new() -> Self {
        Self {
            windows: HashMap::new(),
        }
    }

    pub fn sync_from_store(&mut self, store: &GridStateStore) {
        let mut seen = HashMap::new();
        for (&grid_id, grid) in &store.grids {
            if grid_id != 1 && store.window(grid_id).is_none() {
                continue;
            }
            let win = store.window(grid_id);
            seen.insert(grid_id, ());
            let w = self
                .windows
                .entry(grid_id)
                .or_insert_with(|| WindowRenderState::new(grid_id));
            w.sync_from_store(store, win, grid);
        }
        self.windows.retain(|id, _| seen.contains_key(id));
    }

    pub fn flush_all(&mut self, cfg: &AnimationConfig) {
        for w in self.windows.values_mut() {
            w.flush(cfg);
        }
    }

    pub fn animate_all(&mut self, cfg: &AnimationConfig, dt: f32) -> bool {
        self.windows.values_mut().any(|w| w.animate(cfg, dt))
    }

    pub fn apply_scroll_delta(&mut self, grid_id: i64, delta: i64) {
        if let Some(w) = self.windows.get_mut(&grid_id) {
            w.add_scroll_delta(delta);
        }
    }

    pub fn get(&self, grid_id: i64) -> Option<&WindowRenderState> {
        self.windows.get(&grid_id)
    }

    pub fn is_animating(&self, cfg: &AnimationConfig) -> bool {
        if !cfg.enable_smooth_scroll && !cfg.enable_float_animation {
            return false;
        }
        self.windows.values().any(|w| {
            w.scroll_animation.position.abs() > 0.01 || (w.position_t <= 1.0 && w.is_float)
        })
    }
}

impl Default for WindowAnimStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inner_line_slice_uses_grid_height_not_stale_actual_lines() {
        let mut w = WindowRenderState::new(2);
        w.grid_size = (53, 19);
        w.top_margin = 1;
        w.bottom_margin = 0;
        // actual_lines still empty (pre-first sync) — must not panic.
        assert_eq!(w.inner_line_slice(), 1..19);
    }

    #[test]
    fn inner_line_slice_falls_back_when_margins_overflow() {
        let mut w = WindowRenderState::new(2);
        w.grid_size = (80, 1);
        w.top_margin = 1;
        w.bottom_margin = 1;
        assert_eq!(w.inner_line_slice(), 0..1);
    }

    #[test]
    fn sync_from_store_does_not_panic_with_winbar_before_lines_exist() {
        use nvim_core::grid::GridStateStore;
        use nvim_core::protocol::UiEvent;

        let mut store = GridStateStore::new();
        store.apply(UiEvent::GridResize {
            grid: 2,
            width: 53,
            height: 19,
        });
        store.apply(UiEvent::WinViewportMargins {
            grid: 2,
            top: 1,
            bottom: 0,
            left: 0,
            right: 0,
        });

        let grid = store.grid(2).unwrap().clone();
        let mut w = WindowRenderState::new(2);
        w.sync_from_store(&store, None, &grid);
        assert_eq!(w.top_margin, 1);
        assert_eq!(w.actual_lines.len(), 19);
    }
}
