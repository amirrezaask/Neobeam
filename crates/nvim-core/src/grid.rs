//! Authoritative grid + window + highlight + cursor state, mutated by `UiEvent`s.

use std::collections::{HashMap, HashSet};

use crate::protocol::{Anchor, CursorShape, HlAttr, ModeInfo, UiEvent};

#[derive(Debug, Clone, Default)]
pub struct Cell {
    pub text: String,
    pub hl_id: u32,
    pub double_width: bool,
    pub double_width_continuation: bool,
}

#[derive(Debug, Clone)]
pub struct Grid {
    pub width: u32,
    pub height: u32,
    pub cells: Vec<Cell>,
    pub scroll_delta: i64,
    pub z_index: i64,
}

/// Resolved screen position for a normal or floating window.
#[derive(Debug, Clone, Copy)]
pub struct WindowMeta {
    pub grid_id: i64,
    pub row: i32,
    pub col: i32,
    pub width: u32,
    pub height: u32,
    pub is_float: bool,
    pub z_index: i64,
}

impl Grid {
    fn new(width: u32, height: u32) -> Self {
        let len = (width * height) as usize;
        Grid {
            width,
            height,
            cells: vec![Cell::default(); len],
            scroll_delta: 0,
            z_index: 0,
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        let mut new_cells = vec![Cell::default(); (width * height) as usize];
        let copy_h = self.height.min(height);
        let copy_w = self.width.min(width);
        for r in 0..copy_h {
            for c in 0..copy_w {
                let src = (r * self.width + c) as usize;
                let dst = (r * width + c) as usize;
                new_cells[dst] = self.cells[src].clone();
            }
        }
        self.width = width;
        self.height = height;
        self.cells = new_cells;
    }

    fn clear(&mut self) {
        for c in self.cells.iter_mut() {
            *c = Cell::default();
        }
    }

    #[inline]
    pub fn cell(&self, row: u32, col: u32) -> Option<&Cell> {
        if row < self.height && col < self.width {
            self.cells.get((row * self.width + col) as usize)
        } else {
            None
        }
    }

    fn scroll(&mut self, top: i64, bot: i64, left: i64, right: i64, rows: i64) {
        let w = self.width as i64;
        let top = top.clamp(0, self.height as i64);
        let bot = bot.clamp(0, self.height as i64);
        let left = left.clamp(0, w);
        let right = right.clamp(0, w);
        if rows == 0 || top >= bot || left >= right {
            return;
        }
        let move_row = |cells: &mut Vec<Cell>, from: i64, to: i64| {
            for c in left..right {
                let s = (from * w + c) as usize;
                let d = (to * w + c) as usize;
                cells[d] = cells[s].clone();
            }
        };
        if rows > 0 {
            let mut dst = top;
            let mut src = top + rows;
            while src < bot {
                move_row(&mut self.cells, src, dst);
                dst += 1;
                src += 1;
            }
        } else {
            let mut dst = bot - 1;
            let mut src = bot - 1 + rows;
            while src >= top {
                move_row(&mut self.cells, src, dst);
                dst -= 1;
                src -= 1;
            }
        }
    }
}

/// Resolve absolute row/col for a float from its anchor (§4.9).
pub fn resolve_float_position(anchor: Anchor, row: f64, col: f64, width: u32, height: u32) -> (i32, i32) {
    let row = row as i32;
    let col = col as i32;
    let h = height as i32;
    let w = width as i32;
    let r = match anchor {
        Anchor::SW | Anchor::SE => (row - h).max(0),
        _ => row.max(0),
    };
    let c = match anchor {
        Anchor::NE | Anchor::SE => (col - w).max(0),
        _ => col.max(0),
    };
    (r, c)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultColors {
    pub fg: u32,
    pub bg: u32,
    pub sp: u32,
    /// True when the colorscheme leaves the default background unset
    /// (`:hi Normal guibg=NONE`); the editor background should be transparent.
    pub bg_none: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    pub grid: i64,
    pub row: u32,
    pub col: u32,
}

impl Default for Cursor {
    fn default() -> Self {
        Cursor { grid: 1, row: 0, col: 0 }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ViewportState {
    pub topline: i64,
    pub botline: i64,
    pub scroll_delta: i64,
}

/// Non-scrollable chrome rows/cols on a window grid (winbar, float borders, …).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewportMargins {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

impl ViewportMargins {
    /// Clamp margins so at least one content row remains.
    pub fn clamp_for_height(self, height: u32) -> Self {
        if height == 0 {
            return ViewportMargins::default();
        }
        let top = self.top.min(height.saturating_sub(1));
        let bottom = self.bottom.min(height.saturating_sub(top).saturating_sub(1));
        ViewportMargins {
            top,
            bottom,
            left: self.left,
            right: self.right,
        }
    }
}

pub struct GridStateStore {
    pub grids: HashMap<i64, Grid>,
    pub windows: HashMap<i64, WindowMeta>,
    viewports: HashMap<i64, ViewportState>,
    viewport_margins: HashMap<i64, ViewportMargins>,
    pub default_colors: DefaultColors,
    pub highlights: HashMap<u32, HlAttr>,
    pub cursor: Cursor,
    pub mode_infos: Vec<ModeInfo>,
    pub mode_idx: usize,
    pub busy: bool,
    pub dirty: bool,
    pub pending_scroll: Vec<(i64, i64)>,
    grid_scroll_pending: HashMap<i64, i64>,
    viewport_grids_this_batch: HashSet<i64>,
}

impl Default for GridStateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl GridStateStore {
    pub fn new() -> Self {
        GridStateStore {
            grids: HashMap::new(),
            windows: HashMap::new(),
            viewports: HashMap::new(),
            viewport_margins: HashMap::new(),
            default_colors: DefaultColors::default(),
            highlights: HashMap::new(),
            cursor: Cursor::default(),
            mode_infos: Vec::new(),
            mode_idx: 0,
            busy: false,
            dirty: false,
            pending_scroll: Vec::new(),
            grid_scroll_pending: HashMap::new(),
            viewport_grids_this_batch: HashSet::new(),
        }
    }

    pub fn grid(&self, id: i64) -> Option<&Grid> {
        self.grids.get(&id)
    }

    pub fn window(&self, id: i64) -> Option<&WindowMeta> {
        self.windows.get(&id)
    }

    pub fn viewport_margins(&self, grid_id: i64) -> ViewportMargins {
        let height = self.grids.get(&grid_id).map(|g| g.height).unwrap_or(0);
        self.viewport_margins
            .get(&grid_id)
            .copied()
            .unwrap_or_default()
            .clamp_for_height(height)
    }

    pub fn primary(&self) -> Option<&Grid> {
        self.grids.get(&1)
    }

    pub fn highlight(&self, id: u32) -> Option<&HlAttr> {
        self.highlights.get(&id)
    }

    pub fn current_mode(&self) -> Option<&ModeInfo> {
        self.mode_infos.get(self.mode_idx)
    }

    pub fn cursor_shape(&self) -> CursorShape {
        self.current_mode().map(|m| m.cursor_shape).unwrap_or(CursorShape::Block)
    }

    /// Screen cell origin `(row, col)` for a grid, defaulting to `(0, 0)`.
    pub fn window_origin(&self, grid_id: i64) -> (i32, i32) {
        self.windows
            .get(&grid_id)
            .map(|w| (w.row, w.col))
            .unwrap_or((0, 0))
    }

    /// Active floating windows and their resolved screen positions.
    pub fn active_floats(&self) -> HashMap<i64, (i32, i32)> {
        self.windows
            .iter()
            .filter(|(_, w)| w.is_float)
            .map(|(&id, w)| (id, (w.row, w.col)))
            .collect()
    }

    pub fn apply(&mut self, ev: UiEvent) {
        match ev {
            UiEvent::GridResize { grid, width, height } => {
                self.grids
                    .entry(grid)
                    .and_modify(|g| g.resize(width, height))
                    .or_insert_with(|| Grid::new(width, height));
                if let Some(win) = self.windows.get_mut(&grid) {
                    win.width = width;
                    win.height = height;
                }
            }
            UiEvent::GridClear { grid } => {
                if let Some(g) = self.grids.get_mut(&grid) {
                    g.clear();
                }
            }
            UiEvent::GridDestroy { grid } => {
                self.grids.remove(&grid);
                self.windows.remove(&grid);
                self.viewport_margins.remove(&grid);
            }
            UiEvent::WinClose { grid } | UiEvent::WinHide { grid } => {
                self.windows.remove(&grid);
                self.viewport_margins.remove(&grid);
            }
            UiEvent::WinPos { grid, row, col, width, height } => {
                self.windows.insert(
                    grid,
                    WindowMeta {
                        grid_id: grid,
                        row,
                        col,
                        width,
                        height,
                        is_float: false,
                        z_index: 0,
                    },
                );
            }
            UiEvent::MsgSetPos { grid, row } => {
                let (width, height) = self
                    .grids
                    .get(&grid)
                    .map(|g| (g.width, g.height))
                    .unwrap_or((0, 0));
                let prev = self.windows.get(&grid).copied();
                self.windows.insert(
                    grid,
                    WindowMeta {
                        grid_id: grid,
                        row,
                        col: prev.map(|w| w.col).unwrap_or(0),
                        width: prev.map(|w| w.width).unwrap_or(width),
                        height: prev.map(|w| w.height).unwrap_or(height),
                        is_float: false,
                        z_index: prev.map(|w| w.z_index).unwrap_or(0),
                    },
                );
            }
            UiEvent::GridLine { grid, row, col_start, cells } => {
                if let Some(g) = self.grids.get_mut(&grid) {
                    let mut col = col_start;
                    for dc in cells {
                        if col >= g.width {
                            break;
                        }
                        let idx = (row * g.width + col) as usize;
                        if let Some(slot) = g.cells.get_mut(idx) {
                            slot.text = dc.text;
                            slot.hl_id = dc.hl_id;
                            slot.double_width = dc.double_width;
                            slot.double_width_continuation = dc.double_width_continuation;
                        }
                        col += 1;
                    }
                }
            }
            UiEvent::GridCursorGoto { grid, row, col } => {
                self.cursor = Cursor { grid, row, col };
            }
            UiEvent::GridScroll { grid, top, bot, left, right, rows, .. } => {
                if let Some(g) = self.grids.get_mut(&grid) {
                    g.scroll(top, bot, left, right, rows);
                }
                if rows != 0 {
                    *self.grid_scroll_pending.entry(grid).or_insert(0) += rows;
                }
            }
            UiEvent::DefaultColorsSet { fg, bg, sp, bg_none } => {
                // When the background is unset, keep a black value for color math
                // (reverse video, cursor glyph fallback) but flag it transparent.
                let bg = if bg_none { 0x000000 } else { bg };
                self.default_colors = DefaultColors { fg, bg, sp, bg_none };
            }
            UiEvent::HlAttrDefine { id, attr } => {
                self.highlights.insert(id, attr);
            }
            UiEvent::ModeInfoSet { mode_infos, .. } => {
                self.mode_infos = mode_infos;
            }
            UiEvent::ModeChange { mode_idx } => {
                self.mode_idx = mode_idx;
            }
            UiEvent::Busy(b) => self.busy = b,
            UiEvent::WinViewportMargins { grid, top, bottom, left, right } => {
                let target = if self.grids.contains_key(&grid) { grid } else { 1 };
                let height = self.grids.get(&target).map(|g| g.height).unwrap_or(0);
                self.viewport_margins.insert(
                    target,
                    ViewportMargins {
                        top,
                        bottom,
                        left,
                        right,
                    }
                    .clamp_for_height(height),
                );
            }
            UiEvent::WinViewport {
                grid,
                topline,
                botline,
                curline: _,
                curcol: _,
                line_count: _,
                scroll_delta,
            } => {
                // `win_viewport` targets the window's content grid. With
                // `ext_multigrid` enabled each window owns its grid, so `grid`
                // usually resolves directly. The fallback to grid 1 covers any
                // legacy/non-multigrid viewport handles without content.
                let target = if self.grids.contains_key(&grid) { grid } else { 1 };
                // Viewport is the authoritative scroll signal; once we have one
                // for the target grid, suppress the grid_scroll fallback so the
                // delta is never double-counted.
                self.viewport_grids_this_batch.insert(target);
                let mut delta = scroll_delta;
                if delta == 0 {
                    if let Some(prev) = self.viewports.get(&grid) {
                        delta = topline - prev.topline;
                    }
                }
                if delta != 0 {
                    self.pending_scroll.push((target, delta));
                }
                self.viewports.insert(
                    grid,
                    ViewportState {
                        topline,
                        botline,
                        scroll_delta: delta,
                    },
                );
                if let Some(g) = self.grids.get_mut(&target) {
                    g.scroll_delta = delta;
                }
            }
            UiEvent::WinFloatPos {
                grid,
                anchor,
                anchor_grid,
                anchor_row,
                anchor_col,
                z_index,
                ..
            } => {
                let old = self.windows.get(&grid);
                let g = self.grids.get(&grid);
                let width = old.map(|w| w.width).or_else(|| g.map(|g| g.width)).unwrap_or(0);
                let height = old.map(|w| w.height).or_else(|| g.map(|g| g.height)).unwrap_or(0);
                let (anchor_row_base, anchor_col_base) = self
                    .windows
                    .get(&anchor_grid)
                    .map(|w| (w.row, w.col))
                    .unwrap_or((0, 0));
                let (row, col) = resolve_float_position(
                    anchor,
                    anchor_row_base as f64 + anchor_row,
                    anchor_col_base as f64 + anchor_col,
                    width,
                    height,
                );
                self.windows.insert(
                    grid,
                    WindowMeta {
                        grid_id: grid,
                        row,
                        col,
                        width,
                        height,
                        is_float: true,
                        z_index,
                    },
                );
                if let Some(g) = self.grids.get_mut(&grid) {
                    g.z_index = z_index;
                }
            }
            UiEvent::OptionSet { .. } => {}
            UiEvent::Flush => {
                self.dirty = true;
            }
        }
    }

    /// Take scroll deltas produced by the last batch (consume once when seeding animation).
    pub fn take_pending_scroll(&mut self) -> Vec<(i64, i64)> {
        for (grid, rows) in std::mem::take(&mut self.grid_scroll_pending) {
            if rows != 0 && !self.viewport_grids_this_batch.contains(&grid) {
                self.pending_scroll.push((grid, rows));
            }
        }
        self.viewport_grids_this_batch.clear();
        std::mem::take(&mut self.pending_scroll)
    }

    pub fn apply_batch(&mut self, events: impl IntoIterator<Item = UiEvent>) -> bool {
        self.pending_scroll.clear();
        self.grid_scroll_pending.clear();
        self.viewport_grids_this_batch.clear();
        let mut flushed = false;
        for ev in events {
            if matches!(ev, UiEvent::Flush) {
                flushed = true;
            }
            self.apply(ev);
        }
        flushed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Anchor;

    #[test]
    fn float_anchor_se() {
        let (r, c) = resolve_float_position(Anchor::SE, 10.0, 20.0, 5, 3);
        assert_eq!(r, 7);
        assert_eq!(c, 15);
    }
}
