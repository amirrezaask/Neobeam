//! Terminal emulator core: PTY management, VTE parsing, and cell grid state.
//!
//! Mirrors the nvim-core pattern: `TermSession` owns the PTY + event loop,
//! `TermGrid` exposes the cell state to the renderer.

pub mod grid;
pub mod input;
pub mod session;

pub use grid::{
    TermCell, TermColor, TermCursorShape, TermGrid, TermRow, TermViewport, Underline,
};
pub use session::{RedrawCallback, TermListener, TermMetadata, TermSession};
