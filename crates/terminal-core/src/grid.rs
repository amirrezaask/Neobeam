//! Terminal cell grid: exposes cell data to the renderer without leaking
//! alacritty internals. The `TermGrid` type holds a shared `Term` and
//! provides an ergonomic accessor over renderable content.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as VteColor, CursorShape as VteCursorShape, NamedColor, Rgb};
use std::sync::Arc;

/// RGBA color resolved from a terminal cell's color attribute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl TermColor {
    pub fn to_f32_rgba(self, alpha: f32) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            alpha,
        ]
    }
}

/// Minimal cell data exposed to the renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermCell {
    pub ch: char,
    pub zerowidth: Vec<char>,
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
    pub hidden: bool,
    pub inverse: bool,
    pub strikeout: bool,
    pub wide: bool,
    pub spacer: bool,
    pub underline: Underline,
    pub underline_color: Option<TermColor>,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Underline {
    None,
    Single,
    Double,
    Curl,
    Dotted,
    Dashed,
}

pub type TermRow = Vec<TermCell>;

/// Terminal cursor shape (mirrors VTE / DECSCUSR).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TermCursorShape {
    #[default]
    Block,
    Underline,
    Beam,
    HollowBlock,
    Hidden,
}

impl TermCursorShape {
    pub fn from_vte(shape: VteCursorShape) -> Self {
        match shape {
            VteCursorShape::Block => Self::Block,
            VteCursorShape::Underline => Self::Underline,
            VteCursorShape::Beam => Self::Beam,
            VteCursorShape::HollowBlock => Self::HollowBlock,
            VteCursorShape::Hidden => Self::Hidden,
        }
    }
}

/// Consistent copy of all terminal state needed by the renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermViewport {
    pub rows: Vec<TermRow>,
    pub cursor: (usize, usize),
    pub cursor_visible: bool,
    pub cursor_shape: TermCursorShape,
    pub default_fg: TermColor,
    pub default_bg: TermColor,
    pub display_offset: usize,
}

/// Resolve a `vte::ansi::Color` to an RGB triple, consulting the color table.
pub fn resolve_color(
    color: VteColor,
    colors: &alacritty_terminal::term::color::Colors,
    is_fg: bool,
) -> TermColor {
    let rgb = match color {
        VteColor::Named(named) => colors[named].or_else(|| named_fallback(named)),
        VteColor::Spec(rgb) => Some(rgb),
        VteColor::Indexed(idx) => colors[idx as usize].or_else(|| indexed_fallback(idx)),
    };
    let Rgb { r, g, b } = rgb.unwrap_or(if is_fg {
        Rgb {
            r: 0xcc,
            g: 0xcc,
            b: 0xcc,
        }
    } else {
        Rgb {
            r: 0x1e,
            g: 0x1e,
            b: 0x2e,
        }
    });
    TermColor { r, g, b }
}

fn named_fallback(named: NamedColor) -> Option<Rgb> {
    Some(match named {
        NamedColor::Foreground | NamedColor::BrightForeground => Rgb {
            r: 0xcc,
            g: 0xcc,
            b: 0xcc,
        },
        NamedColor::Background => Rgb {
            r: 0x1e,
            g: 0x1e,
            b: 0x2e,
        },
        NamedColor::Black => Rgb {
            r: 0x18,
            g: 0x18,
            b: 0x18,
        },
        NamedColor::Red => Rgb {
            r: 0xcc,
            g: 0x55,
            b: 0x55,
        },
        NamedColor::Green => Rgb {
            r: 0x55,
            g: 0xaa,
            b: 0x55,
        },
        NamedColor::Yellow => Rgb {
            r: 0xaa,
            g: 0xaa,
            b: 0x55,
        },
        NamedColor::Blue => Rgb {
            r: 0x55,
            g: 0x55,
            b: 0xcc,
        },
        NamedColor::Magenta => Rgb {
            r: 0xaa,
            g: 0x55,
            b: 0xaa,
        },
        NamedColor::Cyan => Rgb {
            r: 0x55,
            g: 0xaa,
            b: 0xaa,
        },
        NamedColor::White => Rgb {
            r: 0xaa,
            g: 0xaa,
            b: 0xaa,
        },
        NamedColor::BrightBlack => Rgb {
            r: 0x55,
            g: 0x55,
            b: 0x55,
        },
        NamedColor::BrightRed => Rgb {
            r: 0xff,
            g: 0x55,
            b: 0x55,
        },
        NamedColor::BrightGreen => Rgb {
            r: 0x55,
            g: 0xff,
            b: 0x55,
        },
        NamedColor::BrightYellow => Rgb {
            r: 0xff,
            g: 0xff,
            b: 0x55,
        },
        NamedColor::BrightBlue => Rgb {
            r: 0x55,
            g: 0x55,
            b: 0xff,
        },
        NamedColor::BrightMagenta => Rgb {
            r: 0xff,
            g: 0x55,
            b: 0xff,
        },
        NamedColor::BrightCyan => Rgb {
            r: 0x55,
            g: 0xff,
            b: 0xff,
        },
        NamedColor::BrightWhite => Rgb {
            r: 0xff,
            g: 0xff,
            b: 0xff,
        },
        _ => return None,
    })
}

fn indexed_fallback(idx: u8) -> Option<Rgb> {
    if idx < 16 {
        return None;
    }
    if idx >= 232 {
        let v = (8 + (idx as u32 - 232) * 10).min(255) as u8;
        return Some(Rgb { r: v, g: v, b: v });
    }
    let i = (idx - 16) as u32;
    Some(Rgb {
        r: ((i / 36) * 51) as u8,
        g: (((i / 6) % 6) * 51) as u8,
        b: ((i % 6) * 51) as u8,
    })
}

/// Thread-safe terminal grid — generic over the `EventListener` so it
/// matches whatever listener the session creates.
pub struct TermGrid<L: EventListener> {
    pub term: Arc<FairMutex<Term<L>>>,
    pub cols: u16,
    pub rows: u16,
}

impl<L: EventListener> TermGrid<L> {
    /// Snapshot the complete visible viewport while holding the terminal lock once.
    pub fn snapshot(&self) -> TermViewport {
        let term = self.term.lock();
        let content = term.renderable_content();
        let colors = content.colors;
        let display_offset = content.display_offset as i32;
        let default_fg = resolve_color(VteColor::Named(NamedColor::Foreground), colors, true);
        let default_bg = resolve_color(VteColor::Named(NamedColor::Background), colors, false);
        let cols = term.grid().columns();
        let row_count = term.grid().screen_lines();
        let blank = TermCell {
            ch: ' ',
            zerowidth: Vec::new(),
            fg: default_fg,
            bg: default_bg,
            bold: false,
            italic: false,
            dim: false,
            hidden: false,
            inverse: false,
            strikeout: false,
            wide: false,
            spacer: false,
            underline: Underline::None,
            underline_color: None,
            selected: false,
        };
        let mut rows = vec![vec![blank; cols]; row_count];

        for cell in content.display_iter {
            let row = cell.point.line.0 + display_offset;
            let col = cell.point.column.0;
            if row < 0 || row as usize >= rows.len() || col >= cols {
                continue;
            }
            let fg = resolve_color(cell.fg, colors, true);
            let bg = resolve_color(cell.bg, colors, false);
            rows[row as usize][col] = TermCell {
                ch: cell.c,
                zerowidth: cell.zerowidth().unwrap_or_default().to_vec(),
                fg,
                bg,
                bold: cell.flags.contains(Flags::BOLD),
                italic: cell.flags.contains(Flags::ITALIC),
                dim: cell.flags.contains(Flags::DIM),
                hidden: cell.flags.contains(Flags::HIDDEN),
                inverse: cell.flags.contains(Flags::INVERSE),
                strikeout: cell.flags.contains(Flags::STRIKEOUT),
                wide: cell.flags.contains(Flags::WIDE_CHAR),
                spacer: cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
                underline: if cell.flags.contains(Flags::DOUBLE_UNDERLINE) {
                    Underline::Double
                } else if cell.flags.contains(Flags::UNDERCURL) {
                    Underline::Curl
                } else if cell.flags.contains(Flags::DOTTED_UNDERLINE) {
                    Underline::Dotted
                } else if cell.flags.contains(Flags::DASHED_UNDERLINE) {
                    Underline::Dashed
                } else if cell.flags.contains(Flags::UNDERLINE) {
                    Underline::Single
                } else {
                    Underline::None
                },
                underline_color: cell
                    .underline_color()
                    .map(|color| resolve_color(color, colors, true)),
                selected: content
                    .selection
                    .is_some_and(|selection| selection.contains(cell.point)),
            };
        }

        let cursor_shape = TermCursorShape::from_vte(content.cursor.shape);
        TermViewport {
            rows,
            cursor: (
                content.cursor.point.line.0.max(0) as usize,
                content.cursor.point.column.0,
            ),
            cursor_visible: cursor_shape != TermCursorShape::Hidden,
            cursor_shape,
            default_fg,
            default_bg,
            display_offset: content.display_offset,
        }
    }

    /// Call `f` with every visible cell, in row-major order.
    pub fn with_cells<F>(&self, mut f: F)
    where
        F: FnMut(usize, usize, TermCell),
    {
        let term = self.term.lock();
        let content = term.renderable_content();
        let colors = content.colors;
        let display_offset = content.display_offset as i32;
        for cell in content.display_iter {
            let row = (cell.point.line.0 + display_offset).max(0) as usize;
            let col = cell.point.column.0;
            let fg = resolve_color(cell.fg, colors, true);
            let bg = resolve_color(cell.bg, colors, false);
            let tc = TermCell {
                ch: cell.c,
                zerowidth: cell.zerowidth().unwrap_or_default().to_vec(),
                fg,
                bg,
                bold: cell.flags.contains(Flags::BOLD),
                italic: cell.flags.contains(Flags::ITALIC),
                dim: cell.flags.contains(Flags::DIM),
                hidden: cell.flags.contains(Flags::HIDDEN),
                inverse: cell.flags.contains(Flags::INVERSE),
                strikeout: cell.flags.contains(Flags::STRIKEOUT),
                wide: cell.flags.contains(Flags::WIDE_CHAR),
                spacer: cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
                underline: if cell.flags.contains(Flags::DOUBLE_UNDERLINE) {
                    Underline::Double
                } else if cell.flags.contains(Flags::UNDERCURL) {
                    Underline::Curl
                } else if cell.flags.contains(Flags::DOTTED_UNDERLINE) {
                    Underline::Dotted
                } else if cell.flags.contains(Flags::DASHED_UNDERLINE) {
                    Underline::Dashed
                } else if cell.flags.contains(Flags::UNDERLINE) {
                    Underline::Single
                } else {
                    Underline::None
                },
                underline_color: cell
                    .underline_color()
                    .map(|color| resolve_color(color, colors, true)),
                selected: content
                    .selection
                    .is_some_and(|selection| selection.contains(cell.point)),
            };
            f(row, col, tc);
        }
    }

    /// Cursor position (row, col).
    pub fn cursor(&self) -> (usize, usize) {
        let term = self.term.lock();
        let c = term.renderable_content().cursor;
        (c.point.line.0 as usize, c.point.column.0)
    }

    /// Default foreground and background colors.
    pub fn default_colors(&self) -> (TermColor, TermColor) {
        let term = self.term.lock();
        let content = term.renderable_content();
        let fg = resolve_color(
            VteColor::Named(NamedColor::Foreground),
            content.colors,
            true,
        );
        let bg = resolve_color(
            VteColor::Named(NamedColor::Background),
            content.colors,
            false,
        );
        (fg, bg)
    }

    pub fn mode_enabled(&self, mode: TermMode) -> bool {
        self.term.lock().mode().contains(mode)
    }

    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string()
    }

    pub fn start_selection(&self, row: usize, col: usize, ty: SelectionType) {
        let mut term = self.term.lock();
        let point = viewport_point(&term, row, col);
        term.selection = Some(Selection::new(ty, point, Side::Left));
    }

    pub fn update_selection(&self, row: usize, col: usize) {
        let mut term = self.term.lock();
        let point = viewport_point(&term, row, col);
        if let Some(selection) = term.selection.as_mut() {
            selection.update(point, Side::Right);
        }
    }

    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
    }
}

fn viewport_point<L: EventListener>(term: &Term<L>, row: usize, col: usize) -> Point {
    let max_row = term.grid().screen_lines().saturating_sub(1);
    let line = row.min(max_row) as i32 - term.grid().display_offset() as i32;
    let max_col = term.grid().columns().saturating_sub(1);
    Point::new(Line(line), Column(col.min(max_col)))
}
