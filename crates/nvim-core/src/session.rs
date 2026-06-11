//! Embedded Neovim session: spawn `nvim --embed`, attach the UI, forward redraw
//! batches, and expose non-blocking input/resize/mouse calls.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use nvim_rs::{create::tokio::new_child_cmd, Handler, Neovim, UiAttachOptions};
use rmpv::Value;
use tokio::process::{Child, ChildStdin, Command};
use tokio::runtime::Handle as RtHandle;
use tokio::task::JoinHandle;
use tokio_util::compat::Compat;

use crate::input::{MouseAction, MouseButton};

pub type Writer = Compat<ChildStdin>;
pub type Nvim = Neovim<Writer>;

/// Callback invoked (on a tokio thread) for every `redraw` notification.
pub type RedrawCallback = Arc<dyn Fn(Vec<Value>) + Send + Sync + 'static>;

/// Callback invoked (once, on a tokio thread) when the nvim process exits
/// (e.g. `:q` / `:qa`), so the host can close the window (§8).
pub type CloseCallback = Arc<dyn Fn() + Send + Sync + 'static>;

fn nvim_executable() -> PathBuf {
    if let Ok(neovim) = std::env::var("NEOVIM") {
        let path = PathBuf::from(&neovim);
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from("nvim")
}

fn nvim_embed_command(working_dir: Option<&PathBuf>) -> Command {
    let mut cmd = Command::new(nvim_executable());
    cmd.arg("--embed");
    // Tell neovim it's inside a true-color terminal so it skips DSR background-color
    // probing (the E1568 "Terminal did not respond" warning).
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    cmd
}

#[derive(Clone)]
struct NvimHandler {
    redraw: Arc<Mutex<Option<RedrawCallback>>>,
}

#[async_trait::async_trait]
impl Handler for NvimHandler {
    type Writer = Writer;

    async fn handle_notify(&self, name: String, args: Vec<Value>, _nvim: Nvim) {
        if name == "redraw" {
            if let Ok(guard) = self.redraw.lock() {
                if let Some(cb) = guard.as_ref() {
                    cb(args);
                }
            }
        }
    }
}

/// Neovim process started and RPC-ready, but UI not yet attached.
pub struct NvimBoot {
    pub nvim: Nvim,
    io: JoinHandle<Result<(), Box<nvim_rs::error::LoopError>>>,
    child: Child,
    redraw_slot: Arc<Mutex<Option<RedrawCallback>>>,
}

impl NvimBoot {
    /// Start `nvim --embed` and wait for the RPC handshake (no `ui_attach`).
    pub async fn boot(working_dir: Option<PathBuf>) -> Result<Self> {
        let redraw_slot = Arc::new(Mutex::new(None));
        let handler = NvimHandler {
            redraw: redraw_slot.clone(),
        };
        let mut cmd = nvim_embed_command(working_dir.as_ref());
        let (nvim, io, child) = new_child_cmd(&mut cmd, handler)
            .await
            .context("spawn nvim --embed")?;
        Ok(NvimBoot {
            nvim,
            io,
            child,
            redraw_slot,
        })
    }

    /// Wire callbacks, attach the UI, and return a live session.
    pub fn finish(
        self,
        rt: &RtHandle,
        cfg: SessionConfig,
        redraw: RedrawCallback,
        on_close: CloseCallback,
    ) -> Result<NvimSession> {
        rt.block_on(self.finish_async(cfg, redraw, on_close, rt))
    }

    /// Async variant for parallel startup (must not call `block_on` from inside).
    pub async fn finish_async(
        self,
        cfg: SessionConfig,
        redraw: RedrawCallback,
        on_close: CloseCallback,
        rt: &RtHandle,
    ) -> Result<NvimSession> {
        if let Ok(mut guard) = self.redraw_slot.lock() {
            *guard = Some(redraw.clone());
        }

        let exit_guard = Arc::new(AtomicBool::new(false));
        {
            let guard = exit_guard.clone();
            let oc = on_close.clone();
            let io = self.io;
            rt.spawn(async move {
                let _ = io.await;
                if !guard.load(Ordering::SeqCst) {
                    oc();
                }
            });
        }

        attach_ui_async(&self.nvim, &cfg)
            .await
            .context("nvim_ui_attach")?;

        Ok(NvimSession {
            nvim: self.nvim,
            rt: rt.clone(),
            child: self.child,
            cfg,
            redraw,
            on_close,
            exit_guard,
        })
    }
}

/// Current buffer and working directory shown in the host winbar.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WinbarInfo {
    pub file_name: String,
    pub project: String,
}

#[derive(Clone)]
pub struct SessionConfig {
    pub cols: u32,
    pub rows: u32,
    pub ext_cmdline: bool,
    pub ext_popupmenu: bool,
    pub ext_messages: bool,
    /// Working directory for the embedded nvim process.
    pub working_dir: Option<PathBuf>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            cols: 120,
            rows: 36,
            ext_cmdline: false,
            ext_popupmenu: false,
            ext_messages: false,
            working_dir: None,
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
        let working_dir = cfg.working_dir.clone();
        let boot = rt
            .block_on(NvimBoot::boot(working_dir))
            .context("spawn nvim --embed")?;
        boot.finish(rt, cfg, redraw, on_close)
    }

    fn attach(&self) -> Result<()> {
        self.rt
            .block_on(attach_ui_async(&self.nvim, &self.cfg))
            .context("nvim_ui_attach")
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
                .call_function("getcompletion", vec![Value::from(""), Value::from("color")])
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
        self.rt
            .block_on(async move { current_colorscheme_async(&nvim).await })
    }

    /// Non-blocking colorscheme query; runs on the session runtime.
    pub fn fetch_colorscheme_async(&self, on_ready: impl FnOnce(Option<String>) + Send + 'static) {
        let nvim = self.nvim.clone();
        self.rt.spawn(async move {
            let name = current_colorscheme_async(&nvim).await;
            on_ready(name);
        });
    }

    /// Apply a colorscheme immediately.
    pub fn set_colorscheme(&self, name: &str) {
        let nvim = self.nvim.clone();
        let cmd = format!("colorscheme {name}");
        self.rt.spawn(async move {
            let _ = nvim.command(&cmd).await;
        });
    }

    /// Open a file in the embedded nvim buffer using `:edit`.
    /// Uses `fnameescape()` so paths with spaces and special characters work.
    pub fn open_file(&self, path: PathBuf) {
        let nvim = self.nvim.clone();
        self.rt.spawn(async move {
            let path_val = Value::from(path.to_string_lossy().as_ref());
            if let Ok(escaped) = nvim.call_function("fnameescape", vec![path_val]).await {
                if let Some(s) = escaped.as_str() {
                    let _ = nvim.command(&format!("e {s}")).await;
                }
            }
        });
    }

    /// Open a file at a specific line using `:edit +{line}`.
    pub fn open_file_at_line(&self, path: PathBuf, line: u64) {
        let nvim = self.nvim.clone();
        self.rt.spawn(async move {
            let path_val = Value::from(path.to_string_lossy().as_ref());
            if let Ok(escaped) = nvim.call_function("fnameescape", vec![path_val]).await {
                if let Some(s) = escaped.as_str() {
                    let _ = nvim.command(&format!("e +{line} {s}")).await;
                }
            }
        });
    }

    /// Change the embedded nvim working directory (`nvim_set_current_dir`).
    pub fn change_working_directory(&mut self, path: PathBuf) -> Result<()> {
        let dir = path.canonicalize().unwrap_or(path);
        let dir_str = dir.to_string_lossy().into_owned();
        let nvim = self.nvim.clone();
        self.rt
            .block_on(async move { nvim.set_current_dir(&dir_str).await })
            .context("nvim_set_current_dir")?;
        self.cfg.working_dir = Some(dir);
        Ok(())
    }

    /// Buffer file name and working directory for the host winbar.
    pub fn fetch_winbar_info(&self) -> WinbarInfo {
        let nvim = self.nvim.clone();
        self.rt
            .block_on(async move { fetch_winbar_info_async(&nvim).await })
    }

    /// Non-blocking winbar query; runs on the session runtime and invokes `on_ready` when done.
    pub fn fetch_winbar_info_async(&self, on_ready: impl FnOnce(WinbarInfo) + Send + 'static) {
        let nvim = self.nvim.clone();
        self.rt.spawn(async move {
            let info = fetch_winbar_info_async(&nvim).await;
            on_ready(info);
        });
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

        let working_dir = self.cfg.working_dir.clone();
        let boot = self
            .rt
            .block_on(NvimBoot::boot(working_dir))
            .context("respawn nvim --embed")?;
        if let Ok(mut guard) = boot.redraw_slot.lock() {
            *guard = Some(self.redraw.clone());
        }

        let exit_guard = Arc::new(AtomicBool::new(false));
        {
            let guard = exit_guard.clone();
            let oc = self.on_close.clone();
            let io = boot.io;
            self.rt.spawn(async move {
                let _ = io.await;
                if !guard.load(Ordering::SeqCst) {
                    oc();
                }
            });
        }

        self.nvim = boot.nvim;
        self.child = boot.child;
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

async fn attach_ui_async(
    nvim: &Nvim,
    cfg: &SessionConfig,
) -> Result<(), Box<nvim_rs::error::CallError>> {
    let (cols, rows) = (cfg.cols as i64, cfg.rows as i64);
    let (cmdline, popup, msgs) = (cfg.ext_cmdline, cfg.ext_popupmenu, cfg.ext_messages);
    let mut opts = UiAttachOptions::new();
    opts.set_rgb(true)
        .set_linegrid_external(true)
        .set_multigrid_external(true)
        .set_cmdline_external(cmdline)
        .set_popupmenu_external(popup)
        .set_messages_externa(msgs);
    nvim.ui_attach(cols, rows, &opts).await?;
    let _ = nvim.command("set noswapfile nobackup nowritebackup").await;
    Ok(())
}

async fn current_colorscheme_async(nvim: &Nvim) -> Option<String> {
    nvim.get_var("colors_name")
        .await
        .ok()
        .and_then(|v| value_as_string(&v))
}

async fn fetch_winbar_info_async(nvim: &Nvim) -> WinbarInfo {
    let result = nvim
        .exec_lua(
            r#"
            local name = vim.api.nvim_buf_get_name(0)
            if name == '' then
              name = '[No Name]'
            else
              name = vim.fn.fnamemodify(name, ':t')
            end
            local path = vim.fn.getcwd()
            local home = vim.env.HOME or vim.env.USERPROFILE
            if home and vim.startswith(path, home) then
              path = '~' .. path:sub(#home + 1)
            end
            return { name, path }
            "#,
            vec![],
        )
        .await;
    result
        .ok()
        .and_then(|v| winbar_info_from_value(&v))
        .unwrap_or_default()
}

fn winbar_info_from_value(v: &Value) -> Option<WinbarInfo> {
    let arr = v.as_array()?;
    if arr.len() < 2 {
        return None;
    }
    Some(WinbarInfo {
        file_name: value_as_string(&arr[0]).unwrap_or_else(|| "[No Name]".into()),
        project: value_as_string(&arr[1]).unwrap_or_default(),
    })
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
