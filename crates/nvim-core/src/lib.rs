//! nvim-core: RPC session, UI protocol decode, grid/window state, input encoding.
//!
//! This crate is windowing- and GPU-agnostic so it can be unit-tested in
//! isolation (AGENT_RUST_PORT.md §2, §11).

pub mod grid;
pub mod input;
pub mod protocol;
pub mod session;

pub use grid::{
    resolve_float_position, Cell, Cursor, DefaultColors, Grid, GridStateStore, ViewportMargins,
    WindowMeta,
};
pub use input::{encode_key, KeyInput, Mods, MouseAction, MouseButton, NamedKey};
pub use protocol::{parse_redraw, CursorShape, HlAttr, ModeInfo, UiEvent};
pub use session::{NvimBoot, NvimSession, RedrawCallback, SessionConfig, WinbarInfo};

pub use rmpv::Value;
