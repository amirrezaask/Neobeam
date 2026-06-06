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
///
/// Matches Neovim's `mode_info_set` contract: when `attr_id` is 0, Normal fg/bg
/// are swapped; otherwise the mode highlight's bg/fg are used with swapped
/// Normal colors as fallback (see `:h ui-global`).
pub fn resolve_cursor(store: &GridStateStore) -> (Rgba, Rgba) {
    let def = store.default_colors;
    let default_fill = rgb_to_rgba(def.fg);
    let default_glyph = rgb_to_rgba(def.bg);

    let attr_id = store
        .current_mode()
        .map(|m| m.attr_id)
        .unwrap_or(0);

    if attr_id == 0 {
        return (default_fill, default_glyph);
    }

    let Some(attr) = store.highlight(attr_id) else {
        return (default_fill, default_glyph);
    };

    let fill = attr.background.map(rgb_to_rgba).unwrap_or(default_fill);
    let glyph = attr.foreground.map(rgb_to_rgba).unwrap_or(default_glyph);
    (fill, glyph)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nvim_core::protocol::{HlAttr, ModeInfo, UiEvent};

    fn store_with_mode(attr_id: u32, fg: u32, bg: u32) -> GridStateStore {
        let mut store = GridStateStore::new();
        store.apply(UiEvent::DefaultColorsSet {
            fg: 0xffffff,
            bg: 0x000000,
            sp: 0xff0000,
            bg_none: false,
        });
        if attr_id != 0 {
            store.apply(UiEvent::HlAttrDefine {
                id: attr_id,
                attr: HlAttr {
                    foreground: Some(fg),
                    background: Some(bg),
                    ..HlAttr::default()
                },
            });
        }
        store.apply(UiEvent::ModeInfoSet {
            cursor_style_enabled: true,
            mode_infos: vec![ModeInfo {
                short_name: "n".into(),
                attr_id,
                ..ModeInfo::default()
            }],
        });
        store.apply(UiEvent::ModeChange { mode_idx: 0 });
        store
    }

    #[test]
    fn cursor_attr_id_zero_swaps_normal_colors() {
        let store = store_with_mode(0, 0, 0);
        let (fill, glyph) = resolve_cursor(&store);
        assert_eq!(fill, rgb_to_rgba(0xffffff));
        assert_eq!(glyph, rgb_to_rgba(0x000000));
    }

    #[test]
    fn cursor_uses_mode_highlight_colors() {
        let store = store_with_mode(7, 0x00ff00, 0x0000ff);
        let (fill, glyph) = resolve_cursor(&store);
        assert_eq!(fill, rgb_to_rgba(0x0000ff));
        assert_eq!(glyph, rgb_to_rgba(0x00ff00));
    }

    #[test]
    fn cursor_highlight_missing_colors_fall_back_to_swapped_normal() {
        let mut store = store_with_mode(0, 0, 0);
        store.apply(UiEvent::HlAttrDefine {
            id: 3,
            attr: HlAttr::default(),
        });
        store.apply(UiEvent::ModeInfoSet {
            cursor_style_enabled: true,
            mode_infos: vec![ModeInfo {
                short_name: "n".into(),
                attr_id: 3,
                ..ModeInfo::default()
            }],
        });

        let (fill, glyph) = resolve_cursor(&store);
        assert_eq!(fill, rgb_to_rgba(0xffffff));
        assert_eq!(glyph, rgb_to_rgba(0x000000));
    }
}
