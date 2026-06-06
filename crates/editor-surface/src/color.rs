//! Highlight/color resolution (AGENT_RUST_PORT.md §4.8).

use nvim_core::grid::GridStateStore;

pub type Rgba = [f32; 4];

#[inline]
pub fn rgb_to_rgba(c: u32) -> Rgba {
    let r = ((c >> 16) & 0xff) as f32 / 255.0;
    let g = ((c >> 8) & 0xff) as f32 / 255.0;
    let b = (c & 0xff) as f32 / 255.0;
    [r, g, b, 1.0]
}

pub struct CellColors {
    pub fg: Rgba,
    pub bg: Rgba,
    pub sp: Rgba,
    /// True if this cell's background differs from the default (needs a bg rect).
    pub bg_is_default: bool,
}

/// Resolve fg/bg/sp for a cell with highlight id `hl_id`.
pub fn resolve_cell(store: &GridStateStore, hl_id: u32) -> CellColors {
    let def = store.default_colors;
    let attr = store.highlight(hl_id);

    let mut fg = attr.and_then(|a| a.foreground).unwrap_or(def.fg);
    let mut bg = attr.and_then(|a| a.background).unwrap_or(def.bg);
    let sp = attr.and_then(|a| a.special).unwrap_or(def.sp);

    let reverse = attr.map(|a| a.reverse).unwrap_or(false);
    if reverse {
        std::mem::swap(&mut fg, &mut bg);
    }

    let bg_is_default = !reverse
        && attr.and_then(|a| a.background).is_none();

    CellColors {
        fg: rgb_to_rgba(fg),
        bg: rgb_to_rgba(bg),
        sp: rgb_to_rgba(sp),
        bg_is_default,
    }
}

/// Cursor fill (rect) and glyph colors for the active mode.
pub fn resolve_cursor(store: &GridStateStore) -> (Rgba, Rgba) {
    let mode = store.current_mode();
    let attr_id = mode.map(|m| m.attr_id).unwrap_or(0);

    // Underlying cell at the cursor (for the invert fallback).
    let under = store
        .grid(store.cursor.grid)
        .and_then(|g| g.cell(store.cursor.row, store.cursor.col))
        .map(|c| c.hl_id)
        .unwrap_or(0);
    let cell = resolve_cell(store, under);

    if attr_id != 0 {
        if let Some(a) = store.highlight(attr_id) {
            let fill = a.background.map(rgb_to_rgba).unwrap_or(cell.fg);
            let glyph = a.foreground.map(rgb_to_rgba).unwrap_or(cell.bg);
            return (fill, glyph);
        }
    }
    // Invert the underlying cell: fill = cell fg, glyph = cell bg.
    (cell.fg, cell.bg)
}
