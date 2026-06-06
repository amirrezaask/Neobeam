//! Embedded Neovim session: spawn `nvim --embed`, attach the UI, forward redraw
//! batches, and expose non-blocking input/resize/mouse calls.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use nvim_rs::{create::tokio::new_child_cmd, Handler, Neovim, UiAttachOptions};
use rmpv::Value;
use tokio::process::{Child, ChildStdin, Command};
use tokio::runtime::Handle as RtHandle;
use tokio_util::compat::Compat;

use crate::input::{MouseAction, MouseButton};

pub type Writer = Compat<ChildStdin>;
pub type Nvim = Neovim<Writer>;

/// Callback invoked (on a tokio thread) for every `redraw` notification.
pub type RedrawCallback = Arc<dyn Fn(Vec<Value>) + Send + Sync + 'static>;

/// Callback invoked (once, on a tokio thread) when the nvim process exits
/// (e.g. `:q` / `:qa`), so the host can close the window (§8).
pub type CloseCallback = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Clone)]
struct NvimHandler {
    redraw: RedrawCallback,
}

#[async_trait::async_trait]
impl Handler for NvimHandler {
    type Writer = Writer;

    async fn handle_notify(&self, name: String, args: Vec<Value>, _nvim: Nvim) {
        if name == "redraw" {
            (self.redraw)(args);
        }
    }
}

pub struct SessionConfig {
    pub cols: u32,
    pub rows: u32,
    pub ext_cmdline: bool,
    pub ext_popupmenu: bool,
    pub ext_messages: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            cols: 120,
            rows: 36,
            ext_cmdline: false,
            ext_popupmenu: false,
            ext_messages: false,
        }
    }
}

pub struct NvimSession {
    pub nvim: Nvim,
    rt: RtHandle,
    child: Child,
    cfg: SessionConfig,
    redraw: RedrawCallback,
    on_close: CloseCallback,
    /// When set, the exit watcher will not fire `on_close` (used during restart).
    exit_guard: Arc<AtomicBool>,
}

impl NvimSession {
    /// Spawn nvim and attach the UI. Runs the async setup on `rt`.
    pub fn spawn(
        rt: &RtHandle,
        cfg: SessionConfig,
        redraw: RedrawCallback,
        on_close: CloseCallback,
    ) -> Result<Self> {
        let handler = NvimHandler { redraw: redraw.clone() };
        let (nvim, io, child) = rt
            .block_on(async move {
                let mut cmd = Command::new("nvim");
                cmd.arg("--embed");
                new_child_cmd(&mut cmd, handler).await
            })
            .context("spawn nvim --embed")?;

        // Watch the RPC io loop: it resolves when nvim closes its stdio (exit).
        let exit_guard = Arc::new(AtomicBool::new(false));
        {
            let guard = exit_guard.clone();
            let oc = on_close.clone();
            rt.spawn(async move {
                let _ = io.await;
                if !guard.load(Ordering::SeqCst) {
                    oc();
                }
            });
        }

        let session = NvimSession {
            nvim,
            rt: rt.clone(),
            child,
            cfg,
            redraw,
            on_close,
            exit_guard,
        };
        session.attach()?;
        Ok(session)
    }

    fn attach(&self) -> Result<()> {
        let nvim = self.nvim.clone();
        let (cols, rows) = (self.cfg.cols as i64, self.cfg.rows as i64);
        let (cmdline, popup, msgs) =
            (self.cfg.ext_cmdline, self.cfg.ext_popupmenu, self.cfg.ext_messages);
        self.rt
            .block_on(async move {
                let mut opts = UiAttachOptions::new();
                opts.set_rgb(true)
                    .set_linegrid_external(true)
                    .set_multigrid_external(true)
                    .set_cmdline_external(cmdline)
                    .set_popupmenu_external(popup)
                    .set_messages_externa(msgs);
                nvim.ui_attach(cols, rows, &opts).await?;
                let _ = nvim.command("set noswapfile nobackup nowritebackup").await;
                Ok::<(), Box<nvim_rs::error::CallError>>(())
            })
            .context("nvim_ui_attach")?;
        Ok(())
    }

    /// Send a raw `nvim_input` string (already in Neovim key notation).
    pub fn input(&self, keys: String) {
        let nvim = self.nvim.clone();
        self.rt.spawn(async move {
            let _ = nvim.input(&keys).await;
        });
    }

    /// Paste committed text (clipboard / IME commit) via `nvim_paste`.
    pub fn paste(&self, text: String) {
        let nvim = self.nvim.clone();
        self.rt.spawn(async move {
            let _ = nvim.paste(&text, false, -1).await;
        });
    }

    pub fn resize(&self, cols: u32, rows: u32) {
        let nvim = self.nvim.clone();
        let (c, r) = (cols.max(1) as i64, rows.max(1) as i64);
        self.rt.spawn(async move {
            let _ = nvim.ui_try_resize(c, r).await;
        });
    }

    pub fn set_focus(&self, gained: bool) {
        let nvim = self.nvim.clone();
        self.rt.spawn(async move {
            let _ = nvim.ui_set_focus(gained).await;
        });
    }

    pub fn mouse(
        &self,
        button: MouseButton,
        action: MouseAction,
        modifier: String,
        grid: i64,
        row: i64,
        col: i64,
    ) {
        let nvim = self.nvim.clone();
        let (b, a) = (button.nvim_button().to_string(), {
            // Wheel buttons carry direction in the action slot.
            match crate::input::wheel_action(button) {
                Some(dir) => dir.to_string(),
                None => action.nvim_action().to_string(),
            }
        });
        self.rt.spawn(async move {
            let _ = nvim.input_mouse(&b, &a, &modifier, grid, row, col).await;
        });
    }

    /// List installed colorschemes (same source as `:colorscheme <Tab>`).
    pub fn fetch_colorschemes(&self) -> Vec<String> {
        let nvim = self.nvim.clone();
        self.rt.block_on(async move {
            let result = nvim
                .call_function(
                    "getcompletion",
                    vec![Value::from(""), Value::from("color")],
                )
                .await;
            result
                .ok()
                .map(|v| value_as_string_list(&v))
                .unwrap_or_default()
        })
    }

    /// Read `g:colors_name` for the active colorscheme.
    pub fn current_colorscheme(&self) -> Option<String> {
        let nvim = self.nvim.clone();
        self.rt.block_on(async move {
            nvim.get_var("colors_name")
                .await
                .ok()
                .and_then(|v| value_as_string(&v))
        })
    }

    /// Apply a colorscheme immediately.
    pub fn set_colorscheme(&self, name: &str) {
        let nvim = self.nvim.clone();
        let cmd = format!("colorscheme {name}");
        self.rt.spawn(async move {
            let _ = nvim.command(&cmd).await;
        });
    }

    /// Read `g:neovide_scroll_animation_length` and `g:neovide_scroll_animation_far_lines` if set.
    pub fn fetch_neovide_scroll_globals(&self) -> (Option<f32>, Option<u32>) {
        let nvim = self.nvim.clone();
        self.rt
            .block_on(async move {
                let scroll = nvim
                    .get_var("neovide_scroll_animation_length")
                    .await
                    .ok()
                    .and_then(|v| value_as_f32(&v));
                let far = nvim
                    .get_var("neovide_scroll_animation_far_lines")
                    .await
                    .ok()
                    .and_then(|v| value_as_u32(&v));
                (scroll, far)
            })
    }

    /// Try a graceful quit; the window lifecycle handles forced kill.
    pub fn quit(&self) {
        let nvim = self.nvim.clone();
        self.rt.spawn(async move {
            let _ = nvim.command("quit").await;
        });
    }

    /// Force-terminate the child process (used on window close).
    pub fn kill(&mut self) {
        self.exit_guard.store(true, Ordering::SeqCst);
        let _ = self.child.start_kill();
    }

    /// Restart: kill the old child and spawn a fresh attached session in place.
    pub fn restart(&mut self) -> Result<()> {
        // Suppress the old watcher so the impending exit doesn't close the app.
        self.exit_guard.store(true, Ordering::SeqCst);
        self.kill();

        let handler = NvimHandler { redraw: self.redraw.clone() };
        let (nvim, io, child) = self
            .rt
            .block_on(async move {
                let mut cmd = Command::new("nvim");
                cmd.arg("--embed");
                new_child_cmd(&mut cmd, handler).await
            })
            .context("respawn nvim --embed")?;

        let exit_guard = Arc::new(AtomicBool::new(false));
        {
            let guard = exit_guard.clone();
            let oc = self.on_close.clone();
            self.rt.spawn(async move {
                let _ = io.await;
                if !guard.load(Ordering::SeqCst) {
                    oc();
                }
            });
        }

        self.nvim = nvim;
        self.child = child;
        self.exit_guard = exit_guard;
        self.attach()?;
        Ok(())
    }
}

impl Drop for NvimSession {
    fn drop(&mut self) {
        // Ensure closing the window terminates the child (§8).
        self.exit_guard.store(true, Ordering::SeqCst);
        let _ = self.child.start_kill();
    }
}

fn value_as_f32(v: &Value) -> Option<f32> {
    v.as_f64().map(|n| n as f32)
}

fn value_as_u32(v: &Value) -> Option<u32> {
    v.as_u64()
        .map(|n| n as u32)
        .or_else(|| v.as_i64().filter(|&n| n >= 0).map(|n| n as u32))
}

fn value_as_string(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.to_string())
}

fn value_as_string_list(v: &Value) -> Vec<String> {
    let mut names: Vec<String> = match v {
        Value::Array(arr) => arr.iter().filter_map(value_as_string).collect(),
        _ => Vec::new(),
    };
    names.sort();
    names.dedup();
    names
}
