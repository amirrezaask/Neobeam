//! Authoritative grid + highlight + cursor state, mutated by `UiEvent`s.
//!
//! v1 is single-grid (`ext_linegrid` only). The store still keys grids by id so
//! multigrid can be enabled later without restructuring (AGENT_RUST_PORT.md §1, §3).

use std::collections::HashMap;

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
    /// Animated pixel scroll offset seed (lines moved on last viewport event).
    pub scroll_delta: i64,
    /// Float placement, if this grid is a floating window.
    pub float: Option<FloatInfo>,
}

#[derive(Debug, Clone, Copy)]
pub struct FloatInfo {
    pub anchor: Anchor,
    pub anchor_grid: i64,
    pub anchor_row: f64,
    pub anchor_col: f64,
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
            float: None,
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
            // content moves up: copy top-to-bottom
            let mut dst = top;
            let mut src = top + rows;
            while src < bot {
                move_row(&mut self.cells, src, dst);
                dst += 1;
                src += 1;
            }
        } else {
            // content moves down: copy bottom-to-top
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

#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultColors {
    pub fg: u32,
    pub bg: u32,
    pub sp: u32,
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

pub struct GridStateStore {
    pub grids: HashMap<i64, Grid>,
    pub default_colors: DefaultColors,
    pub highlights: HashMap<u32, HlAttr>,
    pub cursor: Cursor,
    pub mode_infos: Vec<ModeInfo>,
    pub mode_idx: usize,
    pub busy: bool,
    /// Set true whenever a `Flush` is applied; the host presents a frame and clears it.
    pub dirty: bool,
    /// Per-grid scroll delta produced by the most recent flush (consumed by the animator).
    pub pending_scroll: Vec<(i64, i64)>,
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
            default_colors: DefaultColors::default(),
            highlights: HashMap::new(),
            cursor: Cursor::default(),
            mode_infos: Vec::new(),
            mode_idx: 0,
            busy: false,
            dirty: false,
            pending_scroll: Vec::new(),
        }
    }

    pub fn grid(&self, id: i64) -> Option<&Grid> {
        self.grids.get(&id)
    }

    /// The primary (global) grid for single-grid v1.
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

    pub fn apply(&mut self, ev: UiEvent) {
        match ev {
            UiEvent::GridResize { grid, width, height } => {
                self.grids
                    .entry(grid)
                    .and_modify(|g| g.resize(width, height))
                    .or_insert_with(|| Grid::new(width, height));
            }
            UiEvent::GridClear { grid } => {
                if let Some(g) = self.grids.get_mut(&grid) {
                    g.clear();
                }
            }
            UiEvent::GridDestroy { grid } | UiEvent::WinClose { grid } => {
                self.grids.remove(&grid);
            }
            UiEvent::WinHide { grid } => {
                if let Some(g) = self.grids.get_mut(&grid) {
                    g.float = None;
                }
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
            }
            UiEvent::DefaultColorsSet { fg, bg, sp } => {
                self.default_colors = DefaultColors { fg, bg, sp };
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
            UiEvent::WinViewport { grid, scroll_delta, .. } => {
                if scroll_delta != 0 {
                    self.pending_scroll.push((grid, scroll_delta));
                }
                if let Some(g) = self.grids.get_mut(&grid) {
                    g.scroll_delta = scroll_delta;
                }
            }
            UiEvent::WinFloatPos { grid, anchor, anchor_grid, anchor_row, anchor_col, z_index, .. } => {
                if let Some(g) = self.grids.get_mut(&grid) {
                    g.float = Some(FloatInfo { anchor, anchor_grid, anchor_row, anchor_col, z_index });
                }
            }
            UiEvent::OptionSet { .. } => {}
            UiEvent::Flush => {
                self.dirty = true;
            }
        }
    }

    /// Apply a whole batch; returns true if a flush occurred (frame should present).
    pub fn apply_batch(&mut self, events: impl IntoIterator<Item = UiEvent>) -> bool {
        self.pending_scroll.clear();
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
