//! winit shell: owns the window, GPU renderer, embedded nvim session, grid
//! state and animation engine, and wires input/resize/redraw together.

mod app_page;
mod component_picker;
mod context_menu;
mod fuzzy_picker;
mod git_client;
mod git_diff;
mod imgui_layer;
mod imgui_theme;
mod layout;
mod menu_bar;
mod nvim_view;
mod pane;
mod pane_anim;
mod project;
mod project_picker;
mod settings;
mod shell_env;
mod terminal_view;

use std::io::IsTerminal;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use app_page::AppPage;
use arboard::Clipboard;
use component_picker::ComponentPicker;
use context_menu::{ContextMenu, ContextMenuAction, ContextMenuCommand};
use editor_surface::{AnimationState, ChromeLayout, Renderer};
use git_client::GitClient;
use imgui_layer::ImguiLayer;
use layout::{Rect, SimpleLayout};
use menu_bar::{MenuBar, MenuBarAction};
use nvim_core::grid::GridStateStore;
use nvim_core::input::{
    encode_key, mods_string, KeyInput, Mods, MouseAction, MouseButton as CoreButton, NamedKey,
};
use nvim_core::protocol::parse_redraw;
use nvim_core::session::{NvimBoot, NvimSession, SessionConfig, WinbarInfo};
use nvim_core::Value;
use pane::{FocusDir, PaneId, PaneKind, PaneTree, SplitDir, SplitId};
use pane_anim::PaneAnimStore;
use project::Project;
use project_picker::ProjectPicker;
use settings::{spawn_watcher, Settings};
use terminal_core::input::{encode as encode_term_key, TermKey};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey as WinitNamed};
use winit::window::{Window, WindowId};

#[derive(Debug)]
enum UserEvent {
    Redraw(Vec<Value>),
    SettingsReloaded,
    Exited,
    /// A background git thread finished work; schedule a redraw so the
    /// git-client panel can pick up the new data from its channels.
    GitRefreshed,
    /// Async winbar query finished (see `App::schedule_winbar_refresh`).
    WinbarUpdated(WinbarInfo),
    /// Async colorscheme query finished during startup.
    ColorschemeUpdated(Option<String>),
    /// Terminal content changed; schedule a redraw.
    TermRedraw,
}

struct State {
    window: Arc<Window>,
    renderer: Renderer,
    imgui: ImguiLayer,
    session: NvimSession,
    store: GridStateStore,
    anim: AnimationState,
    last_frame: Instant,
    frame_count: u64,
    #[cfg(debug_assertions)]
    fps: f32,
    mods: Mods,
    ime_active: bool,
    cursor_pos: (f64, f64),
    mouse_down: Option<CoreButton>,
    wheel_scroll_accum: f32,
    grid_cols: u32,
    grid_rows: u32,
    context_menu: ContextMenu,
    /// Window is created before the first paint; stay hidden until then.
    window_pending_show: bool,
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    rt: tokio::runtime::Runtime,
    settings: Settings,
    menu_bar: MenuBar,
    layout: SimpleLayout,
    pane_tree: PaneTree,
    pane_anims: PaneAnimStore,
    component_picker: ComponentPicker,
    pane_drag: Option<PaneDrag>,
    current_page: AppPage,
    git_client: GitClient,
    project_picker: ProjectPicker,
    /// Single source of truth for the current project used by all views.
    project: Option<Project>,
    /// True while a winbar RPC is in flight.
    winbar_refresh_pending: bool,
    /// Another flush arrived while a winbar RPC was in flight.
    winbar_refresh_dirty: bool,
    state: Option<State>,
    /// Background nvim boot started in `App::new`.
    nvim_boot: Option<tokio::task::JoinHandle<Result<NvimBoot>>>,
    startup_t0: Instant,
    /// Independent PTY session for every terminal pane.
    term_sessions: HashMap<PaneId, terminal_core::TermSession>,
}

#[derive(Clone, Copy, Debug)]
struct PaneDrag {
    split_id: SplitId,
    dir: SplitDir,
    parent_rect: Rect,
}

/// Gutter thickness in logical px — generous enough to grab without precision.
const GUTTER_THICKNESS: f32 = 8.0;

fn imgui_active(state: &State) -> bool {
    state.imgui.wants_mouse() || state.imgui.wants_keyboard()
}

fn editor_area(settings: &Settings, renderer: &Renderer) -> Rect {
    let (lw, lh) = renderer.logical_size();
    let layout = ChromeLayout::compute(lw, lh, &settings.chrome_layout_config());
    Rect::from_array(layout.editor_rect)
}

fn cursor_logical(state: &State) -> (f32, f32) {
    let scale = state.renderer.scale() as f32;
    (
        state.cursor_pos.0 as f32 / scale,
        state.cursor_pos.1 as f32 / scale,
    )
}

fn imgui_captures_input(state: &State, current_page: AppPage, project_picker_open: bool) -> bool {
    if current_page == AppPage::GitClient {
        return true;
    }
    project_picker_open || imgui_active(state)
}

fn is_pane_kind_imgui(kind: PaneKind) -> bool {
    matches!(
        kind,
        PaneKind::GitDiff | PaneKind::Settings | PaneKind::Terminal | PaneKind::Empty
    )
}

fn mouse_to_nvim_blocked(
    state: &State,
    current_page: AppPage,
    project_picker_open: bool,
    focused_kind: PaneKind,
    component_picker_open: bool,
) -> bool {
    if current_page != AppPage::Editor {
        return true;
    }
    if component_picker_open || is_pane_kind_imgui(focused_kind) {
        return true;
    }
    project_picker_open
        || state.context_menu.open
        || imgui_captures_input(state, current_page, project_picker_open)
}

fn keyboard_to_nvim_blocked(
    state: &State,
    current_page: AppPage,
    project_picker_open: bool,
    focused_kind: PaneKind,
    component_picker_open: bool,
) -> bool {
    if current_page != AppPage::Editor {
        return true;
    }
    if component_picker_open || is_pane_kind_imgui(focused_kind) {
        return true;
    }
    project_picker_open
        || state.imgui.wants_keyboard()
        || imgui_captures_input(state, current_page, project_picker_open)
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>, initial_project: Option<Project>) -> Result<Self> {
        let startup_t0 = Instant::now();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let settings = Settings::load();
        settings.save_if_missing();
        tracing::info!(
            "startup: settings loaded (+{:.1}ms)",
            startup_t0.elapsed().as_secs_f64() * 1000.0
        );
        let working_dir = initial_project.as_ref().map(|p| p.path.clone());
        let nvim_boot = rt.spawn(async move { NvimBoot::boot(working_dir).await });
        tracing::info!(
            "startup: nvim prefetch started (+{:.1}ms)",
            startup_t0.elapsed().as_secs_f64() * 1000.0
        );
        let reload_proxy = proxy.clone();
        spawn_watcher(move || {
            let _ = reload_proxy.send_event(UserEvent::SettingsReloaded);
        });
        let git_proxy = proxy.clone();
        let mut git_client = GitClient::new(move || {
            let _ = git_proxy.send_event(UserEvent::GitRefreshed);
        });
        let mut menu_bar = MenuBar::new();
        if let Some(p) = &initial_project {
            git_client.set_project(Some(p.path.clone()));
            menu_bar.set_project_display(p.display());
        }
        Ok(App {
            proxy,
            rt,
            settings,
            menu_bar,
            layout: SimpleLayout::new(),
            pane_tree: PaneTree::new_with(PaneKind::Nvim),
            pane_anims: PaneAnimStore::new(),
            component_picker: ComponentPicker::new(),
            pane_drag: None,
            current_page: AppPage::Editor,
            git_client,
            project_picker: ProjectPicker::new(),
            project: initial_project,
            winbar_refresh_pending: false,
            winbar_refresh_dirty: false,
            state: None,
            nvim_boot: Some(nvim_boot),
            startup_t0,
            term_sessions: HashMap::new(),
        })
    }

    fn ensure_terminal_session(&mut self, pane_id: PaneId) {
        if self.term_sessions.contains_key(&pane_id) {
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let (cw, ch) = state.renderer.cell_size();
        let area = editor_area(&self.settings, &state.renderer);
        let rect = self
            .pane_tree
            .layout(area)
            .into_iter()
            .find(|leaf| leaf.id == pane_id)
            .map(|leaf| leaf.rect)
            .unwrap_or(area);
        let cols = ((rect.w / cw).floor() as u16).max(1);
        let rows = ((rect.h / ch).floor() as u16).max(1);
        let proxy = self.proxy.clone();
        let redraw = Arc::new(move || {
            let _ = proxy.send_event(UserEvent::TermRedraw);
        });
        match terminal_core::TermSession::spawn(cols, rows, None, redraw) {
            Ok(session) => {
                self.term_sessions.insert(pane_id, session);
            }
            Err(e) => tracing::error!("terminal spawn failed: {e:#}"),
        }
    }

    fn reconcile_terminal_sessions(&mut self) {
        let live: HashSet<PaneId> = self
            .pane_tree
            .layout(self.editor_area_logical())
            .into_iter()
            .filter(|leaf| leaf.kind == PaneKind::Terminal)
            .map(|leaf| leaf.id)
            .collect();
        let stale: Vec<PaneId> = self
            .term_sessions
            .keys()
            .copied()
            .filter(|id| !live.contains(id))
            .collect();
        for id in stale {
            if let Some(session) = self.term_sessions.remove(&id) {
                session.shutdown();
            }
            if let Some(state) = self.state.as_mut() {
                state.imgui.remove_terminal_texture(id.0);
            }
        }
        for id in live {
            self.ensure_terminal_session(id);
        }
    }

    /// Defer theme/winbar RPCs until after the first frame is scheduled.
    fn schedule_startup_metadata(&mut self) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let proxy = self.proxy.clone();
        state.session.fetch_colorscheme_async(move |name| {
            let _ = proxy.send_event(UserEvent::ColorschemeUpdated(name));
        });
        self.schedule_winbar_refresh();
    }

    /// Drop GPU/window state while the event loop is still running.
    ///
    /// macOS delivers window events after `exit()` if the `Window` outlives the
    /// loop; winit then logs "no handler was set" (winit#3915).
    fn teardown(&mut self, event_loop: &ActiveEventLoop) {
        for (_, session) in self.term_sessions.drain() {
            session.shutdown();
        }
        if let Some(mut state) = self.state.take() {
            state.session.kill();
        }
        event_loop.exit();
    }

    /// Coalesced async winbar refresh. Must not call nvim synchronously from
    /// the redraw hot path — that deadlocks when nvim is busy (e.g. fzf).
    fn schedule_winbar_refresh(&mut self) {
        if self.winbar_refresh_pending {
            self.winbar_refresh_dirty = true;
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        self.winbar_refresh_pending = true;
        let proxy = self.proxy.clone();
        state.session.fetch_winbar_info_async(move |info| {
            let _ = proxy.send_event(UserEvent::WinbarUpdated(info));
        });
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<State> {
        let t0 = self.startup_t0;

        let proxy = self.proxy.clone();
        let redraw = Arc::new(move |args: Vec<Value>| {
            let _ = proxy.send_event(UserEvent::Redraw(args));
        });
        let proxy_close = self.proxy.clone();
        let on_close = Arc::new(move || {
            let _ = proxy_close.send_event(UserEvent::Exited);
        });

        // Attach nvim before creating the window so no transparent shell is shown.
        let boot_handle = self
            .nvim_boot
            .take()
            .context("nvim boot task already consumed")?;
        let rt_handle = self.rt.handle().clone();
        let session_cfg = SessionConfig {
            working_dir: self.project.as_ref().map(|p| p.path.clone()),
            ..Default::default()
        };
        let t_nvim = Instant::now();
        let session = self.rt.block_on(async move {
            let boot = boot_handle.await.context("nvim boot task join")??;
            boot.finish_async(session_cfg, redraw, on_close, &rt_handle)
                .await
        })?;
        tracing::info!(
            "startup: nvim attached (+{:.1}ms, nvim={:.1}ms)",
            t0.elapsed().as_secs_f64() * 1000.0,
            t_nvim.elapsed().as_secs_f64() * 1000.0
        );

        let attrs = Window::default_attributes()
            .with_title("Neobeam")
            .with_visible(false);
        let window = Arc::new(event_loop.create_window(attrs)?);
        window.set_ime_allowed(true);
        tracing::info!(
            "startup: window created (+{:.1}ms)",
            t0.elapsed().as_secs_f64() * 1000.0
        );

        let t_gpu = Instant::now();
        let renderer = Renderer::new(
            window.clone(),
            self.settings.font_family.as_deref(),
            self.settings.font_size,
            self.settings.line_height,
        )?;
        tracing::info!(
            "startup: gpu renderer ready (+{:.1}ms, gpu={:.1}ms)",
            t0.elapsed().as_secs_f64() * 1000.0,
            t_gpu.elapsed().as_secs_f64() * 1000.0
        );

        let chrome_cfg = self.settings.chrome_layout_config();
        let (cw, ch) = renderer.cell_size();
        let (lw, lh) = renderer.logical_size();
        let chrome_layout = ChromeLayout::compute(lw, lh, &chrome_cfg);
        let (cols, rows) = chrome_layout.editor_grid_size(cw, ch);
        session.resize(cols, rows);
        tracing::info!(
            "nvim grid: {cols}x{rows} cells, cell={cw:.1}x{ch:.1}px, scale={}",
            renderer.scale()
        );

        let t_imgui = Instant::now();
        let anim = AnimationState::new(self.settings.animation_config());
        let mut imgui = ImguiLayer::new(&window, &renderer, self.settings.font_size);
        let phys_size = window.inner_size();
        imgui.register_nvim_texture(
            renderer.device(),
            phys_size.width.max(1),
            phys_size.height.max(1),
            renderer.surface_format(),
        );
        tracing::info!(
            "startup: imgui ready (+{:.1}ms, imgui={:.1}ms)",
            t0.elapsed().as_secs_f64() * 1000.0,
            t_imgui.elapsed().as_secs_f64() * 1000.0
        );

        Ok(State {
            window,
            renderer,
            imgui,
            session,
            store: GridStateStore::new(),
            anim,
            last_frame: Instant::now(),
            frame_count: 0,
            #[cfg(debug_assertions)]
            fps: 0.0,
            mods: Mods::default(),
            ime_active: false,
            cursor_pos: (0.0, 0.0),
            mouse_down: None,
            wheel_scroll_accum: 0.0,
            grid_cols: cols,
            grid_rows: rows,
            context_menu: ContextMenu::new(),
            window_pending_show: true,
        })
    }

    fn reload_settings_from_disk(&mut self) {
        let loaded = Settings::load();
        if loaded == self.settings {
            return;
        }
        tracing::info!("reloading settings from disk");
        self.settings = loaded;
        self.apply_font_and_zoom();
        if let Some(state) = self.state.as_ref() {
            state.window.request_redraw();
        }
    }

    fn apply_font_and_zoom(&mut self) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        if let Err(e) = state.renderer.apply_font(
            self.settings.font_family.as_deref(),
            self.settings.font_size,
            self.settings.line_height,
        ) {
            tracing::warn!("font apply failed: {e:#}");
        }
        state.anim.cfg = self.settings.animation_config();
        state.recompute_grid(&self.settings, &self.layout);
    }

    fn adjust_font_size(&mut self, delta: f32) {
        let new_size = (self.settings.font_size + delta).clamp(10.0, 32.0);
        if new_size == self.settings.font_size {
            return;
        }
        self.settings.font_size = new_size;
        self.settings.save();
        self.apply_font_and_zoom();
        if let Some(state) = self.state.as_ref() {
            state.window.request_redraw();
        }
    }

    fn handle_menu_action(&mut self, action: MenuBarAction) {
        match action {
            MenuBarAction::None => {}
            MenuBarAction::SettingsChanged => {
                self.settings.save();
                self.apply_font_and_zoom();
                if let Some(state) = self.state.as_ref() {
                    state.window.request_redraw();
                }
            }
            MenuBarAction::ThemeChanged(name) => {
                if let Some(state) = self.state.as_ref() {
                    state.session.set_colorscheme(&name);
                    self.menu_bar.selected_theme = Some(name);
                    state.window.request_redraw();
                }
            }
        }
    }

    fn handle_context_menu_action(&mut self, action: ContextMenuAction) {
        let ContextMenuAction::Dispatch { grid_pos, command } = action else {
            return;
        };
        let Some(state) = self.state.as_mut() else {
            return;
        };
        dispatch_context_command(&state.session, grid_pos, command);
        state.anim.notify_keystroke();
        state.window.request_redraw();
    }

    fn editor_area_logical(&self) -> Rect {
        let Some(state) = self.state.as_ref() else {
            return Rect::default();
        };
        editor_area(&self.settings, &state.renderer)
    }

    /// Switch all views to a new project. This is the single place that
    /// updates the session cwd, git client, and menu bar together.
    fn set_project(&mut self, path: PathBuf) {
        if let Err(e) = std::fs::create_dir_all(&path) {
            tracing::warn!("failed to create project dir {}: {e:#}", path.display());
        }
        let project = Project::new(path);

        // Update the nvim session cwd.
        let cd_result = self
            .state
            .as_mut()
            .map(|s| s.session.change_working_directory(project.path.clone()));
        if let Some(Err(e)) = cd_result {
            tracing::warn!("nvim_set_current_dir failed: {e:#}");
        }

        // Update git client and menu bar synchronously — no async roundtrip.
        self.git_client.set_project(Some(project.path.clone()));
        self.menu_bar.set_project_display(project.display());
        self.project = Some(project);

        tracing::info!(
            "project: {}",
            self.project
                .as_ref()
                .map(|p| p.display())
                .unwrap_or_default()
        );
        self.schedule_winbar_refresh();
        if let Some(state) = self.state.as_ref() {
            state.window.request_redraw();
        }
    }
}

impl State {
    fn hit_test_in_rect(&self, content_rect: Rect) -> (i64, i64) {
        let scale = self.renderer.scale() as f64;
        let (cw, ch) = self.renderer.cell_size();
        let x = self.cursor_pos.0 / scale - content_rect.x as f64;
        let y = self.cursor_pos.1 / scale - content_rect.y as f64;
        let col = (x / cw as f64).floor() as i64;
        let max_row = (content_rect.h / ch).floor() as i64;
        let row = (y / ch as f64).floor() as i64;
        (row.max(0).min(max_row), col.max(0))
    }

    fn editor_content_rect(&self, settings: &Settings, layout: &SimpleLayout) -> Rect {
        let area = editor_area(settings, &self.renderer);
        layout.compute(area)
    }

    fn hit_test(&self, settings: &Settings, layout: &SimpleLayout) -> (i64, i64) {
        let content = self.editor_content_rect(settings, layout);
        self.hit_test_in_rect(content)
    }

    fn hit_test_editor(&self, settings: &Settings, layout: &SimpleLayout) -> Option<(i64, i64)> {
        let content = self.editor_content_rect(settings, layout);
        let cursor = cursor_logical(self);
        if content.contains(cursor.0, cursor.1) {
            Some(self.hit_test_in_rect(content))
        } else {
            None
        }
    }

    fn recompute_grid(&mut self, settings: &Settings, layout: &SimpleLayout) {
        let content = self.editor_content_rect(settings, layout);
        self.recompute_grid_for(settings, content);
    }

    fn recompute_grid_for(&mut self, _settings: &Settings, content: Rect) {
        let (cw, ch) = self.renderer.cell_size();
        let cols = ((content.w / cw).floor() as u32).max(1);
        let rows = ((content.h / ch).floor() as u32).max(1);
        if cols != self.grid_cols || rows != self.grid_rows {
            self.grid_cols = cols;
            self.grid_rows = rows;
            self.session.resize(cols, rows);
        }
    }

    fn render(
        &mut self,
        settings: &mut Settings,
        menu_bar: &mut MenuBar,
        layout: &mut SimpleLayout,
        pane_tree: &PaneTree,
        pane_anims: &mut PaneAnimStore,
        component_picker: &mut ComponentPicker,
        current_page: AppPage,
        git_client: &mut GitClient,
        project_picker: &mut ProjectPicker,
        term_sessions: &mut HashMap<PaneId, terminal_core::TermSession>,
    ) -> (
        bool,
        MenuBarAction,
        ContextMenuAction,
        Option<PathBuf>,
        Option<PaneKind>,
    ) {
        if self.window_pending_show {
            self.window_pending_show = false;
            self.window.set_visible(true);
        }

        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;

        // Only update from "active" frames; a huge dt means we woke from idle
        // (the gate stopped requesting frames), which isn't a real frame time.
        #[cfg(debug_assertions)]
        if dt > 0.0 && dt < 0.1 {
            let inst = 1.0 / dt;
            self.fps = if self.fps == 0.0 {
                inst
            } else {
                self.fps * 0.9 + inst * 0.1
            };
        }
        let overlay: Option<String> = {
            #[cfg(debug_assertions)]
            {
                Some(format!("{:>3.0} FPS  {:>4.1} ms", self.fps, dt * 1000.0))
            }
            #[cfg(not(debug_assertions))]
            {
                None
            }
        };

        let (cw, ch) = self.renderer.cell_size();
        let chrome_cfg = settings.chrome_layout_config();
        let chrome_layout = ChromeLayout::compute(
            self.renderer.logical_size().0,
            self.renderer.logical_size().1,
            &chrome_cfg,
        );
        let editor_area_rect = Rect::from_array(chrome_layout.editor_rect);
        let main_rect = layout.compute(editor_area_rect);
        let target_leaves = pane_tree.layout(editor_area_rect);
        let anim_targets: Vec<(pane::PaneId, Rect)> =
            target_leaves.iter().map(|l| (l.id, l.rect)).collect();
        pane_anims.sync(&anim_targets);
        let pane_anim_active = pane_anims.tick(dt);
        // Resolve animated rect per leaf (falls back to target if missing).
        let leaves: Vec<pane::LeafLayout> = target_leaves
            .iter()
            .map(|l| {
                let rect = pane_anims
                    .get(l.id)
                    .map(|a| a.render_rect())
                    .unwrap_or(l.rect);
                pane::LeafLayout {
                    id: l.id,
                    kind: l.kind,
                    rect,
                }
            })
            .collect();
        // Nvim grid follows the TARGET rect (not animated) so animation
        // doesn't fire grid_resize events on every frame.
        let nvim_leaf_rect = target_leaves
            .iter()
            .find(|l| l.kind == PaneKind::Nvim)
            .map(|l| l.rect)
            .unwrap_or(main_rect);
        let nvim_anim_rect = leaves
            .iter()
            .find(|l| l.kind == PaneKind::Nvim)
            .map(|l| l.rect)
            .unwrap_or(nvim_leaf_rect);
        self.recompute_grid_for(settings, nvim_leaf_rect);

        let cursor = cursor_logical(self);
        let cursor_in_editor =
            current_page == AppPage::Editor && nvim_leaf_rect.contains(cursor.0, cursor.1);
        let hide_cursor = !cursor_in_editor;

        self.anim.update(dt, &self.store, cw, ch);

        // Step 1: render GPU-backed pane contents into offscreen textures.
        // Render with offset [0,0] — the texture origin IS the nvim area origin.
        let mut nvim_clear_color = [0.0f32; 4];
        if current_page == AppPage::Editor {
            let editor_rect = [0.0, 0.0, nvim_anim_rect.w, nvim_anim_rect.h];
            if let Some(tex_view) = self.imgui.nvim_texture_view_arc() {
                let (tex_w, tex_h) = self.imgui.nvim_texture_size().unwrap_or((
                    self.renderer.logical_size().0 as u32,
                    self.renderer.logical_size().1 as u32,
                ));
                match self.renderer.render_to_view(
                    &self.store,
                    &mut self.anim,
                    overlay.as_deref(),
                    editor_rect,
                    hide_cursor,
                    &tex_view,
                    tex_w,
                    tex_h,
                ) {
                    Ok(clear) => nvim_clear_color = clear,
                    Err(e) => tracing::error!("nvim render error: {e}"),
                }
            }
        }
        if current_page == AppPage::Editor {
            let physical_size = self.window.inner_size();
            for leaf in target_leaves.iter().filter(|leaf| leaf.kind == PaneKind::Terminal) {
                let Some(session) = term_sessions.get_mut(&leaf.id) else {
                    continue;
                };
                let term_rect = leaf.rect;
                let cols = ((term_rect.w / cw).floor() as u16).max(1);
                let rows = ((term_rect.h / ch).floor() as u16).max(1);
                session.resize(cols, rows);
                let desired = (physical_size.width.max(1), physical_size.height.max(1));
                if self.imgui.terminal_texture_size(leaf.id.0) != Some(desired) {
                    self.imgui.register_terminal_texture(
                        leaf.id.0,
                        self.renderer.device(),
                        desired.0,
                        desired.1,
                        self.renderer.surface_format(),
                    );
                }
                let Some(tex_view) = self.imgui.terminal_texture_view_arc(leaf.id.0) else {
                    continue;
                };
                if let Err(e) =
                    self.renderer
                        .render_term_to_view(&session.grid, &tex_view, desired.0, desired.1)
                {
                    tracing::error!("terminal render error: {e}");
                }
            }
        }

        // Step 2: build ImGui frame (nvim shown as Image widget, plus all chrome).
        let nvim_texture_id = self.imgui.nvim_texture_id();
        let nvim_tex_size = self.imgui.nvim_texture_size().unwrap_or((1, 1));
        let terminal_textures: HashMap<PaneId, (imgui::TextureId, (u32, u32))> = target_leaves
            .iter()
            .filter(|leaf| leaf.kind == PaneKind::Terminal)
            .filter_map(|leaf| {
                Some((
                    leaf.id,
                    (
                        self.imgui.terminal_texture_id(leaf.id.0)?,
                        self.imgui.terminal_texture_size(leaf.id.0)?,
                    ),
                ))
            })
            .collect();
        let nvim_scale = self.renderer.scale();
        let mut menu_action = MenuBarAction::None;
        let mut context_action = ContextMenuAction::None;
        let mut project_selection = None;
        let mut component_selection: Option<PaneKind> = None;
        let mut git_wants_redraw = false;
        let window = self.window.clone();
        let session = &self.session;
        let store = &self.store;
        let device = self.renderer.device();
        let queue = self.renderer.queue();
        let context_menu = &mut self.context_menu;
        let focused_id = pane_tree.focused;
        if let Err(e) =
            self.imgui
                .prepare_ui(&window, store, settings.font_size, device, queue, |ui| {
                    if current_page == AppPage::Editor {
                        let mut focused_rect: Option<Rect> = None;
                        for leaf in &leaves {
                            let is_focused = leaf.id == focused_id;
                            if is_focused && leaves.len() > 1 {
                                focused_rect = Some(leaf.rect);
                            }
                            match leaf.kind {
                                PaneKind::Nvim => {
                                    if let Some(tid) = nvim_texture_id {
                                        let (tw, th) = nvim_tex_size;
                                        nvim_view::NvimView::new(tid, tw, th)
                                            .draw(ui, leaf.rect, nvim_scale);
                                    }
                                }
                                PaneKind::GitDiff => {
                                    git_wants_redraw |= git_client.draw(ui, leaf.rect);
                                }
                                PaneKind::Settings => {
                                    let settings_action = menu_bar
                                        .draw_settings_page(ui, settings, session, leaf.rect);
                                    if menu_action == MenuBarAction::None {
                                        menu_action = settings_action;
                                    }
                                }
                                PaneKind::Terminal => {
                                    if let Some((tid, (tw, th))) = terminal_textures.get(&leaf.id) {
                                        terminal_view::TerminalView::new(*tid, *tw, *th)
                                            .draw(ui, leaf.rect, nvim_scale);
                                    }
                                }
                                PaneKind::Empty => {
                                    draw_empty_pane(ui, leaf.rect, is_focused);
                                }
                            }
                        }
                        let gutters = pane_tree.gutters(editor_area_rect, GUTTER_THICKNESS);
                        let mouse = ui.io().mouse_pos;
                        draw_split_gutters(ui, &gutters, mouse);
                        if let Some(r) = focused_rect {
                            draw_pane_focus_border(ui, r);
                        }
                        context_action = context_menu.draw(ui);
                    }
                    if current_page == AppPage::GitClient {
                        git_wants_redraw |= git_client.draw(ui, main_rect);
                    }
                    if current_page == AppPage::Settings {
                        let settings_action =
                            menu_bar.draw_settings_page(ui, settings, session, main_rect);
                        if menu_action == MenuBarAction::None {
                            menu_action = settings_action;
                        }
                    }
                    project_selection = project_picker.draw(ui, dt);
                    component_selection = component_picker.draw(ui);
                })
        {
            tracing::warn!("imgui frame failed: {e:#}");
        }

        // Step 3: present — ImGui renders into the swapchain surface.
        let imgui = &mut self.imgui;
        let renderer = &mut self.renderer;
        if let Err(e) = renderer.present(nvim_clear_color, |device, queue, pass| {
            if let Err(e) = imgui.draw_to_pass(device, queue, pass) {
                tracing::warn!("imgui draw failed: {e:#}");
            }
        }) {
            tracing::error!("present error: {e}");
        }
        self.frame_count += 1;
        let needs_anim = self.anim.is_animating()
            || (!hide_cursor && self.anim.is_blinking(&self.store))
            || self.anim.render_deadline().is_some()
            || pane_anim_active;
        if self.context_menu.open
            || project_picker.is_open()
            || component_picker.is_open()
            || needs_anim
            || imgui_active(self)
            || git_wants_redraw
        {
            self.window.request_redraw();
        }
        (
            needs_anim,
            menu_action,
            context_action,
            project_selection,
            component_selection,
        )
    }
}

fn draw_empty_pane(ui: &imgui::Ui, rect: Rect, focused: bool) {
    use imgui::{Condition, WindowFlags};
    let flags = WindowFlags::NO_TITLE_BAR
        | WindowFlags::NO_RESIZE
        | WindowFlags::NO_MOVE
        | WindowFlags::NO_SCROLLBAR
        | WindowFlags::NO_COLLAPSE
        | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
        | WindowFlags::NO_DECORATION
        | WindowFlags::NO_INPUTS;
    let bg = crate::imgui_theme::pane_bg(ui, focused);
    let _bg = ui.push_style_color(imgui::StyleColor::WindowBg, bg);
    let id = format!("##empty_pane_{}_{}", rect.x as i32, rect.y as i32);
    ui.window(&id)
        .position([rect.x, rect.y], Condition::Always)
        .size([rect.w, rect.h], Condition::Always)
        .flags(flags)
        .build(|| {
            let title = "Empty pane";
            let hint = "Press ⌘T to pick a component";
            let ts_title = ui.calc_text_size(title);
            let ts_hint = ui.calc_text_size(hint);
            let total_h = ts_title[1] + ts_hint[1] + 6.0;
            let mut cy = rect.h * 0.5 - total_h * 0.5;
            ui.set_cursor_pos([rect.w * 0.5 - ts_title[0] * 0.5, cy]);
            ui.text_disabled(title);
            cy += ts_title[1] + 6.0;
            ui.set_cursor_pos([rect.w * 0.5 - ts_hint[0] * 0.5, cy]);
            ui.text_disabled(hint);
        });
}

/// Overlay a 1-px accent-tinted border around the focused pane on the
/// foreground draw list. Quiet enough to coexist with nvim's own coloring.
fn draw_pane_focus_border(ui: &imgui::Ui, rect: Rect) {
    use crate::imgui_theme::{pane_focus_border_color, METRICS};
    let color = pane_focus_border_color(ui);
    let draw = ui.get_foreground_draw_list();
    draw.add_rect(
        [rect.x + 0.5, rect.y + 0.5],
        [rect.x + rect.w - 0.5, rect.y + rect.h - 0.5],
        color,
    )
    .rounding(METRICS.frame_rounding)
    .thickness(METRICS.pane_focus_border)
    .build();
}

/// Draw a thin line at the center of each split gutter. Lights up to the
/// accent color when the user is hovering it for resize.
fn draw_split_gutters(ui: &imgui::Ui, gutters: &[crate::pane::SplitGutter], mouse: [f32; 2]) {
    use crate::imgui_theme::{accent, border, METRICS};
    use crate::pane::SplitDir;
    if gutters.is_empty() {
        return;
    }
    let draw = ui.get_background_draw_list();
    let normal = border(ui);
    let hot = accent(ui);
    for g in gutters {
        let r = g.rect;
        let inside =
            mouse[0] >= r.x && mouse[0] <= r.x + r.w && mouse[1] >= r.y && mouse[1] <= r.y + r.h;
        let color = if inside { hot } else { normal };
        match g.dir {
            SplitDir::Horizontal => {
                let x = r.x + r.w * 0.5;
                draw.add_line([x, r.y], [x, r.y + r.h], color)
                    .thickness(METRICS.gutter_line)
                    .build();
            }
            SplitDir::Vertical => {
                let y = r.y + r.h * 0.5;
                draw.add_line([r.x, y], [r.x + r.w, y], color)
                    .thickness(METRICS.gutter_line)
                    .build();
            }
        }
    }
}

fn dispatch_context_command(
    session: &NvimSession,
    grid_pos: (i64, i64),
    command: ContextMenuCommand,
) {
    let (row, col) = grid_pos;
    session.mouse(
        CoreButton::Left,
        MouseAction::Press,
        String::new(),
        0,
        row,
        col,
    );
    session.mouse(
        CoreButton::Left,
        MouseAction::Release,
        String::new(),
        0,
        row,
        col,
    );
    match command {
        ContextMenuCommand::Input(keys) => session.input(keys),
        ContextMenuCommand::Paste => {
            if let Some(text) = read_clipboard_text() {
                session.paste(text);
            }
        }
    }
}

fn map_named(n: WinitNamed) -> Option<NamedKey> {
    Some(match n {
        WinitNamed::Enter => NamedKey::Enter,
        WinitNamed::Escape => NamedKey::Escape,
        WinitNamed::Backspace => NamedKey::Backspace,
        WinitNamed::Tab => NamedKey::Tab,
        WinitNamed::Space => NamedKey::Space,
        WinitNamed::ArrowUp => NamedKey::Up,
        WinitNamed::ArrowDown => NamedKey::Down,
        WinitNamed::ArrowLeft => NamedKey::Left,
        WinitNamed::ArrowRight => NamedKey::Right,
        WinitNamed::Delete => NamedKey::Delete,
        WinitNamed::Home => NamedKey::Home,
        WinitNamed::End => NamedKey::End,
        WinitNamed::PageUp => NamedKey::PageUp,
        WinitNamed::PageDown => NamedKey::PageDown,
        WinitNamed::Insert => NamedKey::Insert,
        WinitNamed::F1 => NamedKey::F(1),
        WinitNamed::F2 => NamedKey::F(2),
        WinitNamed::F3 => NamedKey::F(3),
        WinitNamed::F4 => NamedKey::F(4),
        WinitNamed::F5 => NamedKey::F(5),
        WinitNamed::F6 => NamedKey::F(6),
        WinitNamed::F7 => NamedKey::F(7),
        WinitNamed::F8 => NamedKey::F(8),
        WinitNamed::F9 => NamedKey::F(9),
        WinitNamed::F10 => NamedKey::F(10),
        WinitNamed::F11 => NamedKey::F(11),
        WinitNamed::F12 => NamedKey::F(12),
        _ => return None,
    })
}

fn map_term_key(key: &Key, shift: bool) -> Option<TermKey> {
    match key {
        Key::Character(text) => text.chars().next().map(TermKey::Char),
        Key::Named(WinitNamed::Space) => Some(TermKey::Char(' ')),
        Key::Named(WinitNamed::Enter) => Some(TermKey::Enter),
        Key::Named(WinitNamed::Backspace) => Some(TermKey::Backspace),
        Key::Named(WinitNamed::Delete) => Some(TermKey::Delete),
        Key::Named(WinitNamed::Escape) => Some(TermKey::Escape),
        Key::Named(WinitNamed::Tab) => Some(if shift {
            TermKey::BackTab
        } else {
            TermKey::Tab
        }),
        Key::Named(WinitNamed::ArrowUp) => Some(TermKey::Up),
        Key::Named(WinitNamed::ArrowDown) => Some(TermKey::Down),
        Key::Named(WinitNamed::ArrowLeft) => Some(TermKey::Left),
        Key::Named(WinitNamed::ArrowRight) => Some(TermKey::Right),
        Key::Named(WinitNamed::Home) => Some(TermKey::Home),
        Key::Named(WinitNamed::End) => Some(TermKey::End),
        Key::Named(WinitNamed::PageUp) => Some(TermKey::PageUp),
        Key::Named(WinitNamed::PageDown) => Some(TermKey::PageDown),
        Key::Named(WinitNamed::Insert) => Some(TermKey::Insert),
        Key::Named(WinitNamed::F1) => Some(TermKey::F(1)),
        Key::Named(WinitNamed::F2) => Some(TermKey::F(2)),
        Key::Named(WinitNamed::F3) => Some(TermKey::F(3)),
        Key::Named(WinitNamed::F4) => Some(TermKey::F(4)),
        Key::Named(WinitNamed::F5) => Some(TermKey::F(5)),
        Key::Named(WinitNamed::F6) => Some(TermKey::F(6)),
        Key::Named(WinitNamed::F7) => Some(TermKey::F(7)),
        Key::Named(WinitNamed::F8) => Some(TermKey::F(8)),
        Key::Named(WinitNamed::F9) => Some(TermKey::F(9)),
        Key::Named(WinitNamed::F10) => Some(TermKey::F(10)),
        Key::Named(WinitNamed::F11) => Some(TermKey::F(11)),
        Key::Named(WinitNamed::F12) => Some(TermKey::F(12)),
        _ => None,
    }
}

fn map_button(b: MouseButton) -> Option<CoreButton> {
    Some(match b {
        MouseButton::Left => CoreButton::Left,
        MouseButton::Right => CoreButton::Right,
        MouseButton::Middle => CoreButton::Middle,
        _ => return None,
    })
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match self.init(event_loop) {
            Ok(state) => {
                state.window.request_redraw();
                self.state = Some(state);
                self.schedule_startup_metadata();
                tracing::info!(
                    "startup: init complete (+{:.1}ms)",
                    self.startup_t0.elapsed().as_secs_f64() * 1000.0
                );
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Err(e) => {
                eprintln!("fatal: failed to initialize: {e:#}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Redraw(args) => {
                let refresh_winbar = {
                    let Some(state) = self.state.as_mut() else {
                        return;
                    };
                    let events = parse_redraw(&args);
                    let flushed = state.store.apply_batch(events);
                    if flushed {
                        let scroll = state.store.take_pending_scroll();
                        state.anim.on_flush(&state.store, &scroll);
                        state.anim.sync_floats(&state.store.active_floats());
                        state.window.request_redraw();
                        true
                    } else {
                        false
                    }
                };
                if refresh_winbar {
                    self.schedule_winbar_refresh();
                }
            }
            UserEvent::SettingsReloaded => {
                self.reload_settings_from_disk();
            }
            UserEvent::GitRefreshed => {
                // A background git thread has new data ready; wake the render
                // loop so `git_client.draw()` can pick it up from the channel.
                if let Some(state) = self.state.as_ref() {
                    state.window.request_redraw();
                }
            }
            UserEvent::WinbarUpdated(info) => {
                self.winbar_refresh_pending = false;
                let dirty = self.winbar_refresh_dirty;
                self.winbar_refresh_dirty = false;
                self.menu_bar.set_winbar(info);
                if let Some(state) = self.state.as_ref() {
                    state.window.request_redraw();
                }
                if dirty {
                    self.schedule_winbar_refresh();
                }
            }
            UserEvent::ColorschemeUpdated(name) => {
                self.menu_bar.set_selected_theme(name);
                if let Some(state) = self.state.as_ref() {
                    state.window.request_redraw();
                }
            }
            UserEvent::Exited => {
                // nvim exited (e.g. `:q`); tear down and close the app.
                tracing::info!("nvim exited; closing");
                self.teardown(event_loop);
            }
            UserEvent::TermRedraw => {
                if let Some(state) = self.state.as_ref() {
                    state.window.request_redraw();
                }
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        for (_, session) in self.term_sessions.drain() {
            session.shutdown();
        }
        if let Some(mut state) = self.state.take() {
            state.session.kill();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if let Some(state) = self.state.as_mut() {
            state
                .imgui
                .handle_event(state.window.as_ref(), window_id, &event);
            let picker_open = self.project_picker.is_open();
            let redraw = state.context_menu.open
                || picker_open
                || imgui_captures_input(state, self.current_page, picker_open);
            if redraw {
                state.window.request_redraw();
            }
        }
        let picker_open = self.project_picker.is_open();
        let comp_open = self.component_picker.is_open();
        let focused_kind = self.pane_tree.focused_kind();
        match event {
            WindowEvent::CloseRequested => {
                self.teardown(event_loop);
            }
            WindowEvent::Resized(size) => {
                let Some(state) = self.state.as_mut() else {
                    return;
                };
                let scale = state.window.scale_factor() as f32;
                state.renderer.resize(size.width, size.height, scale);
                state.imgui.register_nvim_texture(
                    state.renderer.device(),
                    size.width.max(1),
                    size.height.max(1),
                    state.renderer.surface_format(),
                );
                state.recompute_grid(&self.settings, &self.layout);
                state.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let Some(state) = self.state.as_mut() else {
                    return;
                };
                let size = state.window.inner_size();
                let scale = state.window.scale_factor() as f32;
                state.renderer.resize(size.width, size.height, scale);
                state.imgui.register_nvim_texture(
                    state.renderer.device(),
                    size.width.max(1),
                    size.height.max(1),
                    state.renderer.surface_format(),
                );
                state.recompute_grid(&self.settings, &self.layout);
                state.window.request_redraw();
            }
            WindowEvent::Focused(focused) => {
                let Some(state) = self.state.as_mut() else {
                    return;
                };
                state.session.set_focus(focused);
                state.anim.set_focus(focused);
            }
            WindowEvent::ModifiersChanged(m) => {
                let Some(state) = self.state.as_mut() else {
                    return;
                };
                let s = m.state();
                state.mods = Mods {
                    ctrl: s.control_key(),
                    alt: s.alt_key(),
                    meta: s.super_key(),
                    shift: s.shift_key(),
                };
            }
            WindowEvent::Ime(ime) => {
                if focused_kind != PaneKind::Terminal && self.state.as_ref().is_some_and(|s| {
                    keyboard_to_nvim_blocked(
                        s,
                        self.current_page,
                        picker_open,
                        focused_kind,
                        comp_open,
                    )
                }) {
                    return;
                }
                let Some(state) = self.state.as_mut() else {
                    return;
                };
                match ime {
                    Ime::Enabled => state.ime_active = true,
                    Ime::Preedit(text, _) => state.ime_active = !text.is_empty(),
                    Ime::Commit(text) => {
                        state.ime_active = false;
                        if focused_kind == PaneKind::Terminal {
                            if let Some(session) = self.term_sessions.get(&self.pane_tree.focused) {
                                session.paste(text);
                            }
                        } else {
                        state.session.paste(text);
                    }
                    }
                    Ime::Disabled => state.ime_active = false,
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                let ime_active = self.state.as_ref().is_some_and(|s| s.ime_active);
                if !pressed || ime_active {
                    return;
                }

                if self
                    .state
                    .as_ref()
                    .is_some_and(|s| is_font_increase_shortcut(&event.logical_key, s.mods))
                {
                    self.adjust_font_size(1.0);
                    return;
                }
                if self
                    .state
                    .as_ref()
                    .is_some_and(|s| is_font_decrease_shortcut(&event.logical_key, s.mods))
                {
                    self.adjust_font_size(-1.0);
                    return;
                }

                // Global pickers must work even when a pinned results panel has focus.
                if self
                    .state
                    .as_ref()
                    .is_some_and(|s| is_project_picker_shortcut(&event.logical_key, s.mods))
                {
                    self.project_picker.open();
                    if let Some(state) = self.state.as_mut() {
                        state.window.request_redraw();
                    }
                    return;
                }

                if self
                    .state
                    .as_ref()
                    .is_some_and(|s| is_component_picker_shortcut(&event.logical_key, s.mods))
                {
                    self.component_picker.open();
                    if let Some(state) = self.state.as_mut() {
                        state.window.request_redraw();
                    }
                    return;
                }

                if self
                    .state
                    .as_ref()
                    .is_some_and(|s| is_pane_shortcut(&event.logical_key, s.mods).is_some())
                {
                    let cmd = self
                        .state
                        .as_ref()
                        .and_then(|s| is_pane_shortcut(&event.logical_key, s.mods));
                    if let Some(cmd) = cmd {
                        let area = self.editor_area_logical();
                        match cmd {
                            PaneCmd::Split(d) => {
                                self.pane_tree.split_focused(d);
                                self.component_picker.open();
                            }
                            PaneCmd::Close => {
                                self.pane_tree.close_focused();
                            }
                            PaneCmd::Focus(d) => {
                                self.pane_tree.focus_dir(area, d);
                            }
                        }
                        self.reconcile_terminal_sessions();
                        if let Some(state) = self.state.as_ref() {
                            state.window.request_redraw();
                        }
                    }
                    return;
                }

                if self.state.as_ref().is_some_and(|s| {
                    is_quick_component_shortcut(&event.logical_key, s.mods).is_some()
                }) {
                    let kind = self
                        .state
                        .as_ref()
                        .and_then(|s| is_quick_component_shortcut(&event.logical_key, s.mods));
                    if let Some(kind) = kind {
                        self.pane_tree.focus_or_spawn(kind);
                        self.reconcile_terminal_sessions();
                        if kind == PaneKind::GitDiff {
                            self.schedule_winbar_refresh();
                        }
                        if let Some(state) = self.state.as_ref() {
                            state.window.request_redraw();
                        }
                    }
                    return;
                }

                if focused_kind == PaneKind::Terminal && !picker_open && !comp_open {
                    let Some(state) = self.state.as_ref() else {
                        return;
                    };
                    if is_paste_shortcut(&event.logical_key, state.mods) {
                        if let (Some(text), Some(session)) =
                            (read_clipboard_text(), self.term_sessions.get(&self.pane_tree.focused))
                        {
                            session.paste(text);
                        }
                    } else if state.mods.meta {
                        // Super/Command shortcuts belong to the GUI, not the PTY.
                    } else if let (Some(key), Some(session)) = (
                        map_term_key(&event.logical_key, state.mods.shift),
                        self.term_sessions.get(&self.pane_tree.focused),
                    ) {
                        if let Some(bytes) = encode_term_key(
                            key,
                            state.mods.shift,
                            state.mods.ctrl,
                            state.mods.alt,
                            session.app_cursor_mode(),
                        ) {
                            session.write(bytes);
                        }
                    }
                    state.window.request_redraw();
                    return;
                }

                let paste_shortcut = self
                    .state
                    .as_ref()
                    .is_some_and(|s| is_paste_shortcut(&event.logical_key, s.mods));
                if paste_shortcut {
                    let imgui_wants_kb = self.state.as_ref().is_some_and(|s| {
                        keyboard_to_nvim_blocked(
                            s,
                            self.current_page,
                            picker_open,
                            focused_kind,
                            comp_open,
                        )
                    });
                    if !imgui_wants_kb {
                        if let Some(text) = read_clipboard_text() {
                            let Some(state) = self.state.as_mut() else {
                                return;
                            };
                            state.session.paste(text);
                            state.anim.notify_keystroke();
                            state.window.request_redraw();
                        }
                        return;
                    }
                }

                if self.state.as_ref().is_some_and(|s| {
                    keyboard_to_nvim_blocked(
                        s,
                        self.current_page,
                        picker_open,
                        focused_kind,
                        comp_open,
                    )
                }) {
                    return;
                }

                let Some(state) = self.state.as_mut() else {
                    return;
                };
                let key = match &event.logical_key {
                    Key::Named(n) => map_named(*n).map(KeyInput::Named),
                    Key::Character(s) => s.chars().next().map(KeyInput::Char),
                    _ => None,
                };
                if let Some(k) = key {
                    if let Some(encoded) = encode_key(&k, state.mods) {
                        state.session.input(encoded);
                        state.anim.notify_keystroke();
                        state.window.request_redraw();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let Some(state) = self.state.as_mut() else {
                    return;
                };
                state.cursor_pos = (position.x, position.y);
                let cursor = cursor_logical(state);
                let area = editor_area(&self.settings, &state.renderer);

                // Active split drag: update ratio, skip focus change.
                if let Some(drag) = self.pane_drag {
                    let p = drag.parent_rect;
                    let new_ratio = match drag.dir {
                        SplitDir::Horizontal => (cursor.0 - p.x) / p.w.max(1.0),
                        SplitDir::Vertical => (cursor.1 - p.y) / p.h.max(1.0),
                    };
                    self.pane_tree.set_split_ratio(drag.split_id, new_ratio);
                    state.window.request_redraw();
                    return;
                }

                // Hover over a gutter → set resize cursor.
                let gutters = self.pane_tree.gutters(area, GUTTER_THICKNESS);
                let hovered_gutter = gutters
                    .iter()
                    .find(|g| g.rect.contains(cursor.0, cursor.1))
                    .copied();
                if let Some(g) = hovered_gutter {
                    let icon = match g.dir {
                        SplitDir::Horizontal => winit::window::CursorIcon::EwResize,
                        SplitDir::Vertical => winit::window::CursorIcon::NsResize,
                    };
                    state.window.set_cursor(icon);
                } else {
                    state.window.set_cursor(winit::window::CursorIcon::Default);
                }

                // Mouse-follow focus: hovered pane becomes focused.
                if hovered_gutter.is_none() {
                    if let Some(id) = self.pane_tree.hit_test(area, cursor.0, cursor.1) {
                        if self.pane_tree.focused != id {
                            self.pane_tree.focused = id;
                            state.window.request_redraw();
                        }
                    }
                }
                if mouse_to_nvim_blocked(
                    state,
                    self.current_page,
                    picker_open,
                    focused_kind,
                    comp_open,
                ) {
                    return;
                }
                if let Some(btn) = state.mouse_down {
                    let (row, col) = state.hit_test(&self.settings, &self.layout);
                    state.session.mouse(
                        btn,
                        MouseAction::Drag,
                        mods_string(state.mods),
                        0,
                        row,
                        col,
                    );
                }
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                let Some(state) = self.state.as_mut() else {
                    return;
                };

                // Gutter drag handling — runs even when an imgui pane has focus.
                if button == MouseButton::Left {
                    let cursor = cursor_logical(state);
                    let area = editor_area(&self.settings, &state.renderer);
                    match btn_state {
                        ElementState::Pressed => {
                            let gutters = self.pane_tree.gutters(area, GUTTER_THICKNESS);
                            if let Some(g) =
                                gutters.iter().find(|g| g.rect.contains(cursor.0, cursor.1))
                            {
                                self.pane_drag = Some(PaneDrag {
                                    split_id: g.id,
                                    dir: g.dir,
                                    parent_rect: g.parent_rect,
                                });
                                state.window.request_redraw();
                                return;
                            }
                        }
                        ElementState::Released => {
                            if self.pane_drag.take().is_some() {
                                state.window.request_redraw();
                                return;
                            }
                        }
                    }
                }

                if mouse_to_nvim_blocked(
                    state,
                    self.current_page,
                    picker_open,
                    focused_kind,
                    comp_open,
                ) {
                    state.window.request_redraw();
                    return;
                }

                if button == MouseButton::Right && btn_state == ElementState::Pressed {
                    if let Some(grid_pos) = state.hit_test_editor(&self.settings, &self.layout) {
                        let screen_pos = state.imgui.mouse_pos();
                        state.context_menu.open_at(screen_pos, grid_pos);
                        state.window.request_redraw();
                        return;
                    }
                }

                let Some(btn) = map_button(button) else {
                    return;
                };
                let (row, col) = state.hit_test(&self.settings, &self.layout);
                match btn_state {
                    ElementState::Pressed => {
                        state.mouse_down = Some(btn);
                        state.session.mouse(
                            btn,
                            MouseAction::Press,
                            mods_string(state.mods),
                            0,
                            row,
                            col,
                        );
                    }
                    ElementState::Released => {
                        state.mouse_down = None;
                        state.session.mouse(
                            btn,
                            MouseAction::Release,
                            mods_string(state.mods),
                            0,
                            row,
                            col,
                        );
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.state.as_ref().is_some_and(|s| {
                    mouse_to_nvim_blocked(
                        s,
                        self.current_page,
                        picker_open,
                        focused_kind,
                        comp_open,
                    )
                }) {
                    return;
                }
                let Some(state) = self.state.as_mut() else {
                    return;
                };
                let (row, col) = state.hit_test(&self.settings, &self.layout);
                let (_, ch) = state.renderer.cell_size();
                let scale = self.settings.mouse_scroll_sensitivity;
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * scale,
                    MouseScrollDelta::PixelDelta(p) => (p.y as f32) / ch.max(1.0) * scale,
                };
                state.wheel_scroll_accum += lines;
                while state.wheel_scroll_accum >= 1.0 {
                    state.wheel_scroll_accum -= 1.0;
                    state.session.mouse(
                        CoreButton::WheelUp,
                        MouseAction::Press,
                        mods_string(state.mods),
                        0,
                        row,
                        col,
                    );
                }
                while state.wheel_scroll_accum <= -1.0 {
                    state.wheel_scroll_accum += 1.0;
                    state.session.mouse(
                        CoreButton::WheelDown,
                        MouseAction::Press,
                        mods_string(state.mods),
                        0,
                        row,
                        col,
                    );
                }
            }
            WindowEvent::RedrawRequested => {
                let (needs_anim, menu_action, context_action, project_selection, component_sel) = {
                    let Some(state) = self.state.as_mut() else {
                        return;
                    };
                    state.render(
                        &mut self.settings,
                        &mut self.menu_bar,
                        &mut self.layout,
                        &self.pane_tree,
                        &mut self.pane_anims,
                        &mut self.component_picker,
                        self.current_page,
                        &mut self.git_client,
                        &mut self.project_picker,
                        &mut self.term_sessions,
                    )
                };
                self.handle_menu_action(menu_action);
                self.handle_context_menu_action(context_action);
                if let Some(kind) = component_sel {
                    self.pane_tree.set_focused_kind(kind);
                    self.reconcile_terminal_sessions();
                    if let Some(state) = self.state.as_ref() {
                        state.window.request_redraw();
                    }
                }
                if let Some(path) = project_selection {
                    self.set_project(path);
                }
                let picker_open = self.project_picker.is_open();
                if needs_anim {
                    if let Some(deadline) =
                        self.state.as_ref().and_then(|s| s.anim.render_deadline())
                    {
                        event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
                    } else {
                        event_loop.set_control_flow(ControlFlow::Poll);
                    }
                } else if self.state.as_ref().is_some_and(|s| {
                    // Only poll at full speed for editor imgui interactions
                    // (context menu, project picker, hover states).  The git
                    // client drives its own redraws via the GitRefreshed event
                    // and must not force a continuous Poll loop.
                    s.context_menu.open
                        || picker_open
                        || (self.current_page == AppPage::Editor
                            && imgui_captures_input(s, self.current_page, picker_open))
                }) {
                    event_loop.set_control_flow(ControlFlow::Poll);
                } else {
                    event_loop.set_control_flow(ControlFlow::Wait);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let picker_open = self.project_picker.is_open();

        // For git windows the event loop should stay in Wait mode when idle.
        // Redraws are triggered by the GitRefreshed user-event that the
        // background threads send via the proxy.  Forcing Poll here would
        // spin the render loop and block the main thread with git subprocess
        // calls every frame.
        let needs_continuous = state.context_menu.open
            || picker_open
            || self.project_picker.is_animating()
            || (self.current_page == AppPage::Editor
                && imgui_captures_input(state, self.current_page, picker_open))
            || state.anim.is_animating()
            || (self.current_page == AppPage::Editor && state.anim.is_blinking(&state.store));

        if needs_continuous {
            state.window.request_redraw();
            if let Some(deadline) = state.anim.render_deadline() {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            } else {
                event_loop.set_control_flow(ControlFlow::Poll);
            }
        }
    }
}

fn is_font_increase_shortcut(key: &Key, mods: Mods) -> bool {
    if mods.alt || mods.ctrl {
        return false;
    }
    mods.meta && matches!(key, Key::Character(c) if c == "=" || c == "+")
}

fn is_font_decrease_shortcut(key: &Key, mods: Mods) -> bool {
    if mods.alt || mods.shift {
        return false;
    }
    mods.meta && matches!(key, Key::Character(c) if c == "-")
}

fn is_paste_shortcut(key: &Key, mods: Mods) -> bool {
    if mods.alt || mods.shift {
        return false;
    }
    matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("v")) && (mods.meta || mods.ctrl)
}

fn is_project_picker_shortcut(key: &Key, mods: Mods) -> bool {
    if mods.alt || mods.shift || mods.ctrl {
        return false;
    }
    matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("p")) && mods.meta
}

fn is_component_picker_shortcut(key: &Key, mods: Mods) -> bool {
    if mods.alt || mods.shift || mods.ctrl {
        return false;
    }
    matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("t")) && mods.meta
}

enum PaneCmd {
    Split(SplitDir),
    Close,
    Focus(FocusDir),
}

fn is_pane_shortcut(key: &Key, mods: Mods) -> Option<PaneCmd> {
    if mods.alt || mods.ctrl || !mods.meta {
        return None;
    }
    // Cmd+Enter = split right; Cmd+Shift+Enter = split down.
    if matches!(key, Key::Named(WinitNamed::Enter)) {
        return Some(if mods.shift {
            PaneCmd::Split(SplitDir::Vertical)
        } else {
            PaneCmd::Split(SplitDir::Horizontal)
        });
    }
    // Cmd+W = close focused pane (no shift).
    if !mods.shift && matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("w")) {
        return Some(PaneCmd::Close);
    }
    // Cmd+H/J/K/L = focus traversal (no shift).
    if !mods.shift {
        if let Key::Character(c) = key {
            let d = match c.as_str() {
                "h" | "H" => Some(FocusDir::Left),
                "j" | "J" => Some(FocusDir::Down),
                "k" | "K" => Some(FocusDir::Up),
                "l" | "L" => Some(FocusDir::Right),
                _ => None,
            };
            if let Some(d) = d {
                return Some(PaneCmd::Focus(d));
            }
        }
    }
    None
}

/// Cmd+3 → GitDiff, Cmd+, → Settings: spawn-or-focus a pane of that kind.
fn is_quick_component_shortcut(key: &Key, mods: Mods) -> Option<PaneKind> {
    if mods.alt || mods.shift || mods.ctrl || !mods.meta {
        return None;
    }
    match key {
        Key::Character(c) if c.as_str() == "3" => Some(PaneKind::GitDiff),
        Key::Character(c) if c.as_str() == "," => Some(PaneKind::Settings),
        Key::Character(c) if c.as_str() == "1" => Some(PaneKind::Nvim),
        _ => None,
    }
}

fn parse_cli_project_dir() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--project" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

/// Directory used when no `--project` flag is given.
///
/// macOS `.app` launches (Spotlight, Dock, Finder) start with cwd `/`; fall back to `~`.
fn resolve_initial_project_dir() -> Option<PathBuf> {
    if let Some(dir) = parse_cli_project_dir() {
        return Some(dir);
    }
    if let Ok(dir) = std::env::current_dir() {
        if dir.as_os_str() != "/" {
            return Some(dir);
        }
    }
    directories::UserDirs::new().map(|d| d.home_dir().to_path_buf())
}

fn read_clipboard_text() -> Option<String> {
    let mut clipboard = Clipboard::new().ok()?;
    clipboard.get_text().ok().filter(|text| !text.is_empty())
}

/// When launched from a shell, re-exec in the background so the terminal prompt returns.
const DETACHED_ENV: &str = "NEOBEAM_DETACHED";

fn try_detach_from_terminal() -> Result<bool> {
    if std::env::var_os(DETACHED_ENV).is_some() {
        return Ok(false);
    }
    // Keep `cargo run` attached so logs and errors stay on the terminal.
    if std::env::var_os("CARGO").is_some() {
        return Ok(false);
    }
    if !std::io::stdout().is_terminal() {
        return Ok(false);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--foreground" || a == "-f") {
        return Ok(false);
    }

    Command::new(std::env::current_exe()?)
        .args(args)
        .env(DETACHED_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(true)
}

fn main() -> Result<()> {
    shell_env::ensure_login_path();

    if try_detach_from_terminal()? {
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let initial_project = resolve_initial_project_dir().map(|dir| {
        let _ = std::env::set_current_dir(&dir);
        Project::new(dir)
    });
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy, initial_project)?;
    event_loop.run_app(&mut app)?;
    Ok(())
}
