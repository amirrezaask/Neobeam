//! PTY spawning and event loop management.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty::{self, Options, Shell};
use anyhow::Result;

use crate::grid::TermGrid;

/// Fired whenever the terminal content changes (called from a background thread).
pub type RedrawCallback = Arc<dyn Fn() + Send + Sync>;

/// Listener that fires the redraw callback on content changes.
#[derive(Clone)]
pub struct TermListener {
    cb: RedrawCallback,
    metadata: Arc<Mutex<TermMetadata>>,
    generation: Arc<AtomicU64>,
}

#[derive(Clone, Debug, Default)]
pub struct TermMetadata {
    pub title: Option<String>,
    pub bell_count: u64,
    pub exited: bool,
}

impl EventListener for TermListener {
    fn send_event(&self, event: Event) {
        let mut metadata = self.metadata.lock().expect("terminal metadata poisoned");
        match event {
            Event::Title(title) => metadata.title = Some(title),
            Event::ResetTitle => metadata.title = None,
            Event::Bell => metadata.bell_count += 1,
            Event::ChildExit(_) | Event::Exit => metadata.exited = true,
            _ => {}
        }
        drop(metadata);
        self.generation.fetch_add(1, Ordering::Relaxed);
        (self.cb)();
    }
}

/// Manages the PTY session for one terminal pane.
pub struct TermSession {
    sender: EventLoopSender,
    cols: u16,
    rows: u16,
    metadata: Arc<Mutex<TermMetadata>>,
    generation: Arc<AtomicU64>,
    /// Shared terminal state; hand this to the renderer.
    pub grid: Arc<TermGrid<TermListener>>,
}

impl TermSession {
    /// Spawn a shell and start the PTY reader thread.
    pub fn spawn(
        cols: u16,
        rows: u16,
        cell_w: u16,
        cell_h: u16,
        shell: Option<String>,
        cb: RedrawCallback,
    ) -> Result<Self> {
        tty::setup_env();

        let window_size = WindowSize {
            num_cols: cols,
            num_lines: rows,
            cell_width: cell_w,
            cell_height: cell_h,
        };

        let options = Options {
            shell: shell.map(|s| Shell::new(s, vec![])),
            working_directory: std::env::current_dir().ok(),
            drain_on_exit: true,
            env: std::collections::HashMap::new(),
        };

        let pty = tty::new(&options, window_size, 0)?;

        let metadata = Arc::new(Mutex::new(TermMetadata::default()));
        let generation = Arc::new(AtomicU64::new(0));
        let listener = TermListener {
            cb,
            metadata: metadata.clone(),
            generation: generation.clone(),
        };
        let size = TermSize::new(cols as usize, rows as usize);
        let term = Term::new(Config::default(), &size, listener.clone());
        let term = Arc::new(FairMutex::new(term));

        let grid = Arc::new(TermGrid { term: term.clone() });

        let event_loop = EventLoop::new(term, listener, pty, true, false)?;
        let sender = event_loop.channel();
        event_loop.spawn();

        Ok(Self {
            sender,
            cols,
            rows,
            metadata,
            generation,
            grid,
        })
    }

    /// Monotonic counter bumped on PTY output, scroll, and resize.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// Send bytes to the PTY (keyboard input, paste, etc.).
    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.sender.send(Msg::Input(bytes.into()));
    }

    /// Paste text using the terminal's active bracketed-paste mode.
    pub fn paste(&self, text: String) {
        let mut bytes = Vec::with_capacity(text.len() + 12);
        if self.grid.mode_enabled(TermMode::BRACKETED_PASTE) {
            bytes.extend_from_slice(b"\x1b[200~");
            // Prevent pasted content from terminating bracketed paste early.
            bytes.extend_from_slice(text.replace("\x1b[201~", "").as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
        } else {
            bytes.extend_from_slice(text.as_bytes());
        }
        self.write(bytes);
    }

    pub fn app_cursor_mode(&self) -> bool {
        self.grid.mode_enabled(TermMode::APP_CURSOR)
    }

    pub fn scroll(&self, lines: i32) {
        self.grid.scroll(lines);
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn selection_text(&self) -> Option<String> {
        self.grid.selection_text()
    }

    pub fn start_selection(&self, row: usize, col: usize, semantic: bool) {
        let ty = if semantic {
            SelectionType::Semantic
        } else {
            SelectionType::Simple
        };
        self.grid.start_selection(row, col, ty);
    }

    pub fn update_selection(&self, row: usize, col: usize) {
        self.grid.update_selection(row, col);
    }

    pub fn clear_selection(&self) {
        self.grid.clear_selection();
    }

    pub fn metadata(&self) -> TermMetadata {
        self.metadata
            .lock()
            .expect("terminal metadata poisoned")
            .clone()
    }

    /// Resize the PTY and terminal grid.
    pub fn resize(&mut self, cols: u16, rows: u16, cell_w: u16, cell_h: u16) {
        if self.cols == cols && self.rows == rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.generation.fetch_add(1, Ordering::Relaxed);
        // alacritty's event loop only forwards resize to the PTY (SIGWINCH).
        // The Term's grid must be resized directly, or snapshot keeps returning
        // the spawn dimensions forever.
        {
            let size = TermSize::new(cols as usize, rows as usize);
            let mut term = self.grid.term.lock();
            term.resize(size);
        }
        let _ = self.sender.send(Msg::Resize(WindowSize {
            num_cols: cols,
            num_lines: rows,
            cell_width: cell_w,
            cell_height: cell_h,
        }));
    }

    /// Shut down the event loop.
    pub fn shutdown(&self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}
