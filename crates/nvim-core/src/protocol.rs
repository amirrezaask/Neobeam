//! Neovim UI redraw protocol: typed events + adapter from raw msgpack.
//!
//! The adapter normalizes a raw `redraw` notification (a `Vec<Value>` where each
//! element is `[event_name, arg_tuple, arg_tuple, ...]`) into a flat
//! `Vec<UiEvent>`. Unknown event names and unknown trailing args are ignored for
//! forward-compatibility (see AGENT_RUST_PORT.md §4.4).

use rmpv::Value;

/// A single decoded grid cell from a `grid_line` event.
#[derive(Debug, Clone)]
pub struct DecodedCell {
    pub text: String,
    pub hl_id: u32,
    pub double_width: bool,
    pub double_width_continuation: bool,
}

/// Raw highlight attributes from `hl_attr_define`.
#[derive(Debug, Clone, Default)]
pub struct HlAttr {
    pub foreground: Option<u32>,
    pub background: Option<u32>,
    pub special: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub undercurl: bool,
    pub underdouble: bool,
    pub strikethrough: bool,
    pub reverse: bool,
    pub blend: u8,
}

/// Per-mode cursor info from `mode_info_set`.
#[derive(Debug, Clone)]
pub struct ModeInfo {
    pub short_name: String,
    pub cursor_shape: CursorShape,
    pub cell_percentage: u8,
    pub blinkwait: u32,
    pub blinkon: u32,
    pub blinkoff: u32,
    pub attr_id: u32,
}

impl Default for ModeInfo {
    fn default() -> Self {
        Self {
            short_name: String::new(),
            cursor_shape: CursorShape::Block,
            cell_percentage: 100,
            blinkwait: 0,
            blinkon: 0,
            blinkoff: 0,
            attr_id: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Horizontal,
    Vertical,
}

impl CursorShape {
    fn parse(s: &str) -> Self {
        match s {
            "horizontal" => CursorShape::Horizontal,
            "vertical" => CursorShape::Vertical,
            _ => CursorShape::Block,
        }
    }
}

/// Anchor corner for floating windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    NW,
    NE,
    SW,
    SE,
}

/// Normalized UI events. Only the v1 set is modeled; anything else is dropped by
/// the adapter.
#[derive(Debug, Clone)]
pub enum UiEvent {
    GridResize { grid: i64, width: u32, height: u32 },
    GridClear { grid: i64 },
    GridLine { grid: i64, row: u32, col_start: u32, cells: Vec<DecodedCell> },
    GridCursorGoto { grid: i64, row: u32, col: u32 },
    GridScroll { grid: i64, top: i64, bot: i64, left: i64, right: i64, rows: i64, cols: i64 },
    GridDestroy { grid: i64 },
    DefaultColorsSet { fg: u32, bg: u32, sp: u32 },
    HlAttrDefine { id: u32, attr: HlAttr },
    ModeInfoSet { cursor_style_enabled: bool, mode_infos: Vec<ModeInfo> },
    ModeChange { mode_idx: usize },
    OptionSet { name: String, value: Value },
    Busy(bool),
    WinViewport { grid: i64, topline: i64, botline: i64, curline: i64, curcol: i64, line_count: i64, scroll_delta: i64 },
    WinFloatPos { grid: i64, anchor: Anchor, anchor_grid: i64, anchor_row: f64, anchor_col: f64, z_index: i64, focusable: bool },
    WinClose { grid: i64 },
    WinHide { grid: i64 },
    Flush,
}

#[inline]
fn as_u32(v: &Value) -> u32 {
    v.as_u64().map(|x| x as u32).or_else(|| v.as_i64().map(|x| x as u32)).unwrap_or(0)
}

#[inline]
fn as_i64(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_u64().map(|x| x as i64)).unwrap_or(0)
}

#[inline]
fn as_str(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// Parse a full `redraw` batch (the args of the notification) into typed events.
pub fn parse_redraw(args: &[Value]) -> Vec<UiEvent> {
    let mut out = Vec::with_capacity(args.len());
    for group in args {
        let Some(arr) = group.as_array() else { continue };
        let Some((name, tuples)) = arr.split_first() else { continue };
        let Some(name) = name.as_str() else { continue };
        for tuple in tuples {
            let Some(p) = tuple.as_array() else { continue };
            parse_event(name, p, &mut out);
        }
    }
    out
}

fn parse_event(name: &str, p: &[Value], out: &mut Vec<UiEvent>) {
    match name {
        "grid_resize" if p.len() >= 3 => out.push(UiEvent::GridResize {
            grid: as_i64(&p[0]),
            width: as_u32(&p[1]),
            height: as_u32(&p[2]),
        }),
        "grid_clear" if !p.is_empty() => out.push(UiEvent::GridClear { grid: as_i64(&p[0]) }),
        "grid_destroy" if !p.is_empty() => out.push(UiEvent::GridDestroy { grid: as_i64(&p[0]) }),
        "grid_cursor_goto" if p.len() >= 3 => out.push(UiEvent::GridCursorGoto {
            grid: as_i64(&p[0]),
            row: as_u32(&p[1]),
            col: as_u32(&p[2]),
        }),
        "grid_scroll" if p.len() >= 7 => out.push(UiEvent::GridScroll {
            grid: as_i64(&p[0]),
            top: as_i64(&p[1]),
            bot: as_i64(&p[2]),
            left: as_i64(&p[3]),
            right: as_i64(&p[4]),
            rows: as_i64(&p[5]),
            cols: as_i64(&p[6]),
        }),
        "grid_line" if p.len() >= 4 => {
            if let Some(cells) = parse_grid_line_cells(&p[3]) {
                out.push(UiEvent::GridLine {
                    grid: as_i64(&p[0]),
                    row: as_u32(&p[1]),
                    col_start: as_u32(&p[2]),
                    cells,
                });
            }
        }
        "default_colors_set" if p.len() >= 3 => out.push(UiEvent::DefaultColorsSet {
            fg: as_u32(&p[0]),
            bg: as_u32(&p[1]),
            sp: as_u32(&p[2]),
        }),
        "hl_attr_define" if p.len() >= 2 => out.push(UiEvent::HlAttrDefine {
            id: as_u32(&p[0]),
            attr: parse_hl_attr(&p[1]),
        }),
        "mode_info_set" if p.len() >= 2 => out.push(parse_mode_info_set(p)),
        "mode_change" if p.len() >= 2 => out.push(UiEvent::ModeChange { mode_idx: as_i64(&p[1]) as usize }),
        "option_set" if p.len() >= 2 => out.push(UiEvent::OptionSet {
            name: as_str(&p[0]).to_string(),
            value: p[1].clone(),
        }),
        "busy_start" => out.push(UiEvent::Busy(true)),
        "busy_stop" => out.push(UiEvent::Busy(false)),
        "win_viewport" if p.len() >= 6 => out.push(UiEvent::WinViewport {
            grid: as_i64(&p[0]),
            topline: as_i64(&p[2]),
            botline: as_i64(&p[3]),
            curline: as_i64(&p[4]),
            curcol: as_i64(&p[5]),
            line_count: p.get(6).map(as_i64).unwrap_or(0),
            scroll_delta: p.get(7).map(as_i64).unwrap_or(0),
        }),
        "win_float_pos" if p.len() >= 6 => out.push(UiEvent::WinFloatPos {
            grid: as_i64(&p[0]),
            anchor: match as_str(&p[2]) {
                "NE" => Anchor::NE,
                "SW" => Anchor::SW,
                "SE" => Anchor::SE,
                _ => Anchor::NW,
            },
            anchor_grid: as_i64(&p[3]),
            anchor_row: p[4].as_f64().unwrap_or(0.0),
            anchor_col: p[5].as_f64().unwrap_or(0.0),
            z_index: p.get(7).map(as_i64).unwrap_or(0),
            focusable: p.get(6).and_then(|v| v.as_bool()).unwrap_or(true),
        }),
        "win_close" if !p.is_empty() => out.push(UiEvent::WinClose { grid: as_i64(&p[0]) }),
        "win_hide" if !p.is_empty() => out.push(UiEvent::WinHide { grid: as_i64(&p[0]) }),
        "flush" => out.push(UiEvent::Flush),
        _ => {}
    }
}

/// Decode the cells array of a `grid_line`, expanding repeats, carrying hl ids
/// forward, and marking double-width continuations (§4.5).
fn parse_grid_line_cells(v: &Value) -> Option<Vec<DecodedCell>> {
    let arr = v.as_array()?;
    let mut cells: Vec<DecodedCell> = Vec::with_capacity(arr.len());
    let mut last_hl: u32 = 0;
    for cell in arr {
        let c = cell.as_array()?;
        if c.is_empty() {
            continue;
        }
        let text = as_str(&c[0]).to_string();
        // hl_id omitted -> carry forward
        let hl_id = if c.len() >= 2 { as_u32(&c[1]) } else { last_hl };
        last_hl = hl_id;
        let repeat = if c.len() >= 3 { as_u32(&c[2]).max(1) } else { 1 };

        // Empty text after a non-empty cell = double-width continuation.
        if text.is_empty() {
            if let Some(prev) = cells.last_mut() {
                prev.double_width = true;
            }
            cells.push(DecodedCell {
                text: String::new(),
                hl_id,
                double_width: false,
                double_width_continuation: true,
            });
            continue;
        }

        for _ in 0..repeat {
            cells.push(DecodedCell {
                text: text.clone(),
                hl_id,
                double_width: false,
                double_width_continuation: false,
            });
        }
    }
    Some(cells)
}

fn parse_hl_attr(v: &Value) -> HlAttr {
    let mut a = HlAttr::default();
    let Some(map) = v.as_map() else { return a };
    for (k, val) in map {
        match k.as_str().unwrap_or("") {
            "foreground" => a.foreground = val.as_u64().map(|x| x as u32),
            "background" => a.background = val.as_u64().map(|x| x as u32),
            "special" => a.special = val.as_u64().map(|x| x as u32),
            "bold" => a.bold = val.as_bool().unwrap_or(false),
            "italic" => a.italic = val.as_bool().unwrap_or(false),
            "underline" => a.underline = val.as_bool().unwrap_or(false),
            "undercurl" => a.undercurl = val.as_bool().unwrap_or(false),
            "underdouble" => a.underdouble = val.as_bool().unwrap_or(false),
            "strikethrough" => a.strikethrough = val.as_bool().unwrap_or(false),
            "reverse" => a.reverse = val.as_bool().unwrap_or(false),
            "blend" => a.blend = val.as_u64().unwrap_or(0) as u8,
            _ => {}
        }
    }
    a
}

fn parse_mode_info_set(p: &[Value]) -> UiEvent {
    let cursor_style_enabled = p[0].as_bool().unwrap_or(false);
    let mut modes = Vec::new();
    if let Some(arr) = p[1].as_array() {
        for m in arr {
            let mut info = ModeInfo::default();
            if let Some(map) = m.as_map() {
                for (k, v) in map {
                    match k.as_str().unwrap_or("") {
                        "name" | "short_name" if info.short_name.is_empty() => {
                            info.short_name = as_str(v).to_string()
                        }
                        "cursor_shape" => info.cursor_shape = CursorShape::parse(as_str(v)),
                        "cell_percentage" => info.cell_percentage = as_u32(v) as u8,
                        "blinkwait" => info.blinkwait = as_u32(v),
                        "blinkon" => info.blinkon = as_u32(v),
                        "blinkoff" => info.blinkoff = as_u32(v),
                        "attr_id" => info.attr_id = as_u32(v),
                        _ => {}
                    }
                }
            }
            modes.push(info);
        }
    }
    UiEvent::ModeInfoSet { cursor_style_enabled, mode_infos: modes }
}
