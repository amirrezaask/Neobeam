//! Terminal cell grid: exposes cell data to the renderer without leaking
//! alacritty internals. The `TermGrid` type holds a shared `Term` and
//! provides an ergonomic accessor over renderable content.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as VteColor, NamedColor, Rgb};
use std::sync::Arc;

/// RGBA color resolved from a terminal cell's color attribute.
#[derive(Clone, Copy, Debug)]
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
#[derive(Clone, Debug)]
pub struct TermCell {
    pub ch: char,
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
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
    /// Call `f` with every visible cell, in row-major order.
    pub fn with_cells<F>(&self, mut f: F)
    where
        F: FnMut(usize, usize, TermCell),
    {
        let term = self.term.lock();
        let content = term.renderable_content();
        let colors = content.colors;
        for cell in content.display_iter {
            let row = cell.point.line.0 as usize;
            let col = cell.point.column.0;
            let fg = resolve_color(cell.fg, colors, true);
            let bg = resolve_color(cell.bg, colors, false);
            use alacritty_terminal::term::cell::Flags;
            let tc = TermCell {
                ch: cell.c,
                fg,
                bg,
                bold: cell.flags.contains(Flags::BOLD),
                italic: cell.flags.contains(Flags::ITALIC),
                underline: cell
                    .flags
                    .intersects(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE),
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
}
