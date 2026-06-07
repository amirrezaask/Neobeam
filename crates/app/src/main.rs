//! winit shell: owns the window, GPU renderer, embedded nvim session, grid
//! state and animation engine, and wires input/resize/redraw together
//! (AGENT_RUST_PORT.md §3, §7, §8).

mod app_page;
mod context_menu;
mod file_picker;
mod fuzzy_picker;
mod grep_picker;
mod git_client;
mod git_diff;
mod imgui_layer;
mod imgui_theme;
mod menu_bar;
mod multiplexer;
mod project;
mod project_picker;
mod results_panel;
mod settings;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use app_page::AppPage;
use arboard::Clipboard;
use context_menu::{ContextMenu, ContextMenuAction, ContextMenuCommand};
use editor_surface::{AnimationState, ChromeLayout, Renderer};
use file_picker::{FilePicker, FilePickerOutcome};
use grep_picker::{GrepMatch, GrepPicker, GrepPickerOutcome};
use git_client::GitClient;
use imgui_layer::ImguiLayer;
use menu_bar::{MenuBar, MenuBarAction};
use multiplexer::{Rect, TilingManager, ViewKind, WinId};
use project::Project;
use project_picker::ProjectPicker;
use results_panel::{FileListPanel, GrepResultsPanel};
use nvim_core::grid::GridStateStore;
use nvim_core::input::{
    encode_key, mods_string, KeyInput, Mods, MouseAction, MouseButton as CoreButton, NamedKey,
};
use nvim_core::protocol::parse_redraw;
use nvim_core::session::{NvimSession, SessionConfig, WinbarInfo};
use nvim_core::Value;
use settings::{spawn_watcher, Settings};
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
    fps: f32,
    mods: Mods,
    ime_active: bool,
    cursor_pos: (f64, f64),
    mouse_down: Option<CoreButton>,
    wheel_scroll_accum: f32,
    grid_cols: u32,
    grid_rows: u32,
    context_menu: ContextMenu,
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    rt: tokio::runtime::Runtime,
    settings: Settings,
    menu_bar: MenuBar,
    tiling: TilingManager,
    git_client: GitClient,
    project_picker: ProjectPicker,
    file_picker: FilePicker,
    grep_picker: GrepPicker,
    /// Single source of truth for the current project used by all views.
    project: Option<Project>,
    /// True while a winbar RPC is in flight.
    winbar_refresh_pending: bool,
    /// Another flush arrived while a winbar RPC was in flight.
    winbar_refresh_dirty: bool,
    file_list_panels: Vec<(WinId, FileListPanel)>,
    grep_result_panels: Vec<(WinId, GrepResultsPanel)>,
    state: Option<State>,
}

fn imgui_active(state: &State) -> bool {
    state.imgui.wants_mouse() || state.imgui.wants_keyboard()
}

fn cursor_in_menu_bar(state: &State, settings: &Settings) -> bool {
    let scale = state.renderer.scale() as f64;
    let (lw, lh) = state.renderer.logical_size();
    let layout = ChromeLayout::compute(lw, lh, &settings.chrome_layout_config());
    state.cursor_pos.1 / scale < layout.editor_y as f64
}

fn app_page_to_view(page: AppPage) -> ViewKind {
    match page {
        AppPage::Editor => ViewKind::Editor,
        AppPage::GitClient => ViewKind::GitClient,
    }
}

fn view_to_app_page(view: ViewKind) -> AppPage {
    match view {
        ViewKind::Editor | ViewKind::FileList | ViewKind::GrepResults => AppPage::Editor,
        ViewKind::GitClient => AppPage::GitClient,
    }
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

fn cursor_in_panel_content(state: &State, settings: &Settings, tiling: &TilingManager) -> bool {
    let area = editor_area(settings, &state.renderer);
    let rects = tiling.compute_rects(area);
    let cursor = cursor_logical(state);
    let Some(win_id) = tiling.hit_window_content(cursor, &rects) else {
        return false;
    };
    tiling
        .windows
        .iter()
        .find(|w| w.id == win_id)
        .is_some_and(|w| w.view.captures_input())
}

fn imgui_captures_input(
    state: &State,
    settings: &Settings,
    tiling: &TilingManager,
    project_picker_open: bool,
) -> bool {
    if tiling.is_dragging() {
        return true;
    }
    let area = editor_area(settings, &state.renderer);
    let rects = tiling.compute_rects(area);
    let cursor = cursor_logical(state);
    if tiling.cursor_in_any_title_bar(cursor, &rects) {
        return true;
    }
    if cursor_in_panel_content(state, settings, tiling) {
        return true;
    }
    project_picker_open
        || imgui_active(state)
        || cursor_in_menu_bar(state, settings)
}

fn mouse_to_nvim_blocked(
    state: &State,
    settings: &Settings,
    tiling: &TilingManager,
    project_picker_open: bool,
) -> bool {
    if tiling.is_dragging() {
        return true;
    }
    let area = editor_area(settings, &state.renderer);
    let rects = tiling.compute_rects(area);
    let cursor = cursor_logical(state);
    if tiling.cursor_in_any_title_bar(cursor, &rects) {
        return true;
    }
    if cursor_in_panel_content(state, settings, tiling) {
        return true;
    }
    project_picker_open
        || state.context_menu.open
        || imgui_captures_input(state, settings, tiling, project_picker_open)
}

fn keyboard_to_nvim_blocked(
    state: &State,
    settings: &Settings,
    tiling: &TilingManager,
    project_picker_open: bool,
) -> bool {
    if tiling.focused_view().captures_input() {
        return true;
    }
    project_picker_open
        || state.imgui.wants_keyboard()
        || imgui_captures_input(state, settings, tiling, project_picker_open)
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>, initial_project: Option<Project>) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let settings = Settings::load();
        settings.save();
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
            tiling: TilingManager::new(28.0),
            git_client,
            project_picker: ProjectPicker::new(),
            file_picker: FilePicker::new(),
            grep_picker: GrepPicker::new(),
            project: initial_project,
            winbar_refresh_pending: false,
            winbar_refresh_dirty: false,
            file_list_panels: Vec::new(),
            grep_result_panels: Vec::new(),
            state: None,
        })
    }

    fn cleanup_panels(&mut self) {
        let alive: std::collections::HashSet<WinId> =
            self.tiling.windows.iter().map(|w| w.id).collect();
        self.file_list_panels
            .retain(|(id, _)| alive.contains(id));
        self.grep_result_panels
            .retain(|(id, _)| alive.contains(id));
        let _ = self.tiling.take_removed_windows();
    }

    fn handle_pin_outcomes(
        &mut self,
        pin_file: Option<(Vec<(String, PathBuf)>, String)>,
        pin_grep: Option<(Vec<GrepMatch>, String, PathBuf)>,
    ) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let area = editor_area(&self.settings, &state.renderer);
        if let Some((items, query)) = pin_file {
            if let Some(win_id) = self.tiling.find_hidden_by_view(ViewKind::FileList) {
                self.tiling.show_window(win_id, area);
                if let Some((_, panel)) =
                    self.file_list_panels.iter_mut().find(|(id, _)| *id == win_id)
                {
                    panel.reload(items, query);
                }
            } else {
                let panel = FileListPanel::new(items, query);
                let win_id = self.tiling.add_view_window(ViewKind::FileList, area);
                self.file_list_panels.push((win_id, panel));
            }
            state.recompute_grid(&self.settings, &self.tiling);
            state.window.request_redraw();
        }
        if let Some((results, query, project_root)) = pin_grep {
            if let Some(win_id) = self.tiling.find_hidden_by_view(ViewKind::GrepResults) {
                self.tiling.show_window(win_id, area);
                if let Some((_, panel)) =
                    self.grep_result_panels.iter_mut().find(|(id, _)| *id == win_id)
                {
                    panel.reload(results, query, project_root);
                }
            } else {
                let panel = GrepResultsPanel::new(results, query, project_root);
                let win_id = self.tiling.add_view_window(ViewKind::GrepResults, area);
                self.grep_result_panels.push((win_id, panel));
            }
            state.recompute_grid(&self.settings, &self.tiling);
            state.window.request_redraw();
        }
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
        let attrs = Window::default_attributes()
            .with_title("nvim-ui")
            .with_transparent(true);
        let window = Arc::new(event_loop.create_window(attrs)?);
        window.set_ime_allowed(true);

        let renderer = Renderer::new(
            window.clone(),
            self.settings.font_family.as_deref(),
            self.settings.font_size,
            self.settings.line_height,
        )?;

        let (cw, ch) = renderer.cell_size();
        let (lw, lh) = renderer.logical_size();
        let chrome_cfg = self.settings.chrome_layout_config();
        let layout = ChromeLayout::compute(lw, lh, &chrome_cfg);
        let area = Rect::from_array(layout.editor_rect);
        let rects = self.tiling.compute_rects(area);
        let editor_id = self.tiling.editor_win().expect("default editor window");
        let content = self
            .tiling
            .content_rect(self.tiling.visual_rect(editor_id, &rects));
        let cols = ((content.w / cw).floor() as u32).max(1);
        let rows = ((content.h / ch).floor() as u32).max(1);

        let proxy = self.proxy.clone();
        let redraw = Arc::new(move |args: Vec<Value>| {
            let _ = proxy.send_event(UserEvent::Redraw(args));
        });
        let proxy_close = self.proxy.clone();
        let on_close = Arc::new(move || {
            let _ = proxy_close.send_event(UserEvent::Exited);
        });

        let session = NvimSession::spawn(
            self.rt.handle(),
            SessionConfig {
                cols,
                rows,
                working_dir: self.project.as_ref().map(|p| p.path.clone()),
                ..Default::default()
            },
            redraw,
            on_close,
        )?;
        let (scroll, far) = session.fetch_neovide_scroll_globals();
        self.settings.apply_neovide_scroll_globals(scroll, far);
        self.menu_bar.init_theme(&session);
        self.menu_bar.refresh_winbar(&session);
        tracing::info!(
            "nvim attached: {cols}x{rows} cells, cell={cw:.1}x{ch:.1}px, scale={}",
            renderer.scale()
        );

        let anim = AnimationState::new(self.settings.animation_config());
        let imgui = ImguiLayer::new(&window, &renderer, self.settings.font_size);

        Ok(State {
            window,
            renderer,
            imgui,
            session,
            store: GridStateStore::new(),
            anim,
            last_frame: Instant::now(),
            frame_count: 0,
            fps: 0.0,
            mods: Mods::default(),
            ime_active: false,
            cursor_pos: (0.0, 0.0),
            mouse_down: None,
            wheel_scroll_accum: 0.0,
            grid_cols: cols,
            grid_rows: rows,
            context_menu: ContextMenu::new(),
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
        state.recompute_grid(&self.settings, &self.tiling);
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
            MenuBarAction::PageChanged(page) => {
                if let Some(state) = self.state.as_mut() {
                    let area = editor_area(&self.settings, &state.renderer);
                    self.tiling.open_or_focus(app_page_to_view(page), area);
                    state.recompute_grid(&self.settings, &self.tiling);
                    state.window.request_redraw();
                }
                if page == AppPage::GitClient {
                    self.schedule_winbar_refresh();
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
            self.project.as_ref().map(|p| p.display()).unwrap_or_default()
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

    fn hit_test(&self, settings: &Settings, tiling: &TilingManager) -> (i64, i64) {
        let area = editor_area(settings, &self.renderer);
        let rects = tiling.compute_rects(area);
        let Some(editor_id) = tiling.editor_win() else {
            return (0, 0);
        };
        let visual = tiling.visual_rect(editor_id, &rects);
        let content = tiling.content_rect(visual);
        self.hit_test_in_rect(content)
    }

    fn hit_test_editor(&self, settings: &Settings, tiling: &TilingManager) -> Option<(i64, i64)> {
        let area = editor_area(settings, &self.renderer);
        let rects = tiling.compute_rects(area);
        let cursor = cursor_logical(self);
        let win_id = tiling.hit_window_content(cursor, &rects)?;
        let view = tiling
            .windows
            .iter()
            .find(|w| w.id == win_id)
            .map(|w| w.view)?;
        if view != ViewKind::Editor {
            return None;
        }
        let visual = tiling.visual_rect(win_id, &rects);
        let content = tiling.content_rect(visual);
        if content.contains(cursor.0, cursor.1) {
            Some(self.hit_test_in_rect(content))
        } else {
            None
        }
    }

    fn recompute_grid(&mut self, settings: &Settings, tiling: &TilingManager) {
        let area = editor_area(settings, &self.renderer);
        let rects = tiling.compute_rects(area);
        let Some(editor_id) = tiling.editor_win() else {
            return;
        };
        let visual = tiling.visual_rect(editor_id, &rects);
        let content = tiling.content_rect(visual);
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
        tiling: &mut TilingManager,
        git_client: &mut GitClient,
        project_picker: &mut ProjectPicker,
        file_picker: &mut FilePicker,
        grep_picker: &mut GrepPicker,
        file_list_panels: &mut [(WinId, FileListPanel)],
        grep_result_panels: &mut [(WinId, GrepResultsPanel)],
    ) -> (
        bool,
        MenuBarAction,
        ContextMenuAction,
        Option<PathBuf>,
        Option<(Vec<(String, PathBuf)>, String)>,
        Option<(Vec<GrepMatch>, String, PathBuf)>,
    ) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;

        // Only update from "active" frames; a huge dt means we woke from idle
        // (the gate stopped requesting frames), which isn't a real frame time.
        if dt > 0.0 && dt < 0.1 {
            let inst = 1.0 / dt;
            self.fps = if self.fps == 0.0 {
                inst
            } else {
                self.fps * 0.9 + inst * 0.1
            };
        }
        let overlay = Some(format!("{:>3.0} FPS  {:>4.1} ms", self.fps, dt * 1000.0));

        let (cw, ch) = self.renderer.cell_size();
        let chrome_cfg = settings.chrome_layout_config();
        let layout = ChromeLayout::compute(
            self.renderer.logical_size().0,
            self.renderer.logical_size().1,
            &chrome_cfg,
        );
        let editor_area_rect = Rect::from_array(layout.editor_rect);
        let window_rects = tiling.compute_rects(editor_area_rect);
        let tiling_animating = tiling.update_anim(dt);
        self.recompute_grid(settings, tiling);
        let focused_page = view_to_app_page(tiling.focused_view());

        let mut menu_action = MenuBarAction::None;
        let mut context_action = ContextMenuAction::None;
        let mut project_selection = None;
        let mut pin_file_list = None;
        let mut pin_grep_results = None;
        let mut git_wants_redraw = false;
        let window = self.window.clone();
        let session = &self.session;
        let store = &self.store;
        let device = self.renderer.device();
        let queue = self.renderer.queue();
        let context_menu = &mut self.context_menu;
        let git_windows: Vec<WinId> = tiling
            .windows
            .iter()
            .filter(|w| w.view == ViewKind::GitClient && tiling.is_visible(w.id))
            .map(|w| w.id)
            .collect();
        if let Err(e) = self.imgui.prepare_ui(
            &window,
            store,
            settings.font_size,
            device,
            queue,
            |ui| {
                menu_action = menu_bar.draw(ui, settings, session, focused_page);
                tiling.draw_chrome(ui, &window_rects);
                for win_id in &git_windows {
                    let visual = tiling.visual_rect(*win_id, &window_rects);
                    let content = tiling.content_rect(visual);
                    git_wants_redraw |= git_client.draw(ui, *win_id, content);
                }
                for (win_id, panel) in file_list_panels.iter_mut() {
                    if !tiling.is_visible(*win_id) {
                        continue;
                    }
                    let visual = tiling.visual_rect(*win_id, &window_rects);
                    let content = tiling.content_rect(visual);
                    if let Some(path) = panel.draw(ui, *win_id, content, dt) {
                        session.open_file(path);
                    }
                }
                for (win_id, panel) in grep_result_panels.iter_mut() {
                    if !tiling.is_visible(*win_id) {
                        continue;
                    }
                    let visual = tiling.visual_rect(*win_id, &window_rects);
                    let content = tiling.content_rect(visual);
                    if let Some(m) = panel.draw(ui, *win_id, content) {
                        session.open_file_at_line(m.path, m.line_number);
                    }
                }
                if tiling.focused_view() == ViewKind::Editor {
                    context_action = context_menu.draw(ui);
                }
                project_selection = project_picker.draw(ui, dt);
                match file_picker.draw(ui, dt) {
                    FilePickerOutcome::None => {}
                    FilePickerOutcome::Opened(path) => session.open_file(path),
                    FilePickerOutcome::Pinned { items, query } => {
                        pin_file_list = Some((items, query));
                    }
                }
                match grep_picker.draw(ui, dt) {
                    GrepPickerOutcome::None => {}
                    GrepPickerOutcome::Selected(m) => {
                        session.open_file_at_line(m.path, m.line_number);
                    }
                    GrepPickerOutcome::Pinned {
                        results,
                        query,
                        project_root,
                    } => {
                        pin_grep_results = Some((results, query, project_root));
                    }
                }
            },
        ) {
            tracing::warn!("imgui frame failed: {e:#}");
        }

        let cursor = cursor_logical(self);
        let cursor_in_editor = tiling
            .hit_window_content(cursor, &window_rects)
            .and_then(|id| {
                tiling
                    .windows
                    .iter()
                    .find(|w| w.id == id)
                    .map(|w| w.view == ViewKind::Editor)
            })
            .unwrap_or(false);
        let editor_render_rect = tiling.editor_win().map(|editor_id| {
            let visual = tiling.visual_rect(editor_id, &window_rects);
            let content = tiling.content_rect(visual);
            [content.x, content.y, content.w, content.h]
        });
        let hide_cursor = !cursor_in_editor || tiling.focused_view().captures_input();

        self.anim.update(dt, &self.store, cw, ch);

        let anim = &mut self.anim;
        let imgui = &mut self.imgui;
        let renderer = &mut self.renderer;
        if let Some(editor_rect) = editor_render_rect {
            if let Err(e) = renderer.render(
                store,
                anim,
                overlay.as_deref(),
                editor_rect,
                hide_cursor,
                |device, queue, pass| {
                    if let Err(e) = imgui.draw_to_pass(device, queue, pass) {
                        tracing::warn!("imgui draw failed: {e:#}");
                    }
                },
            ) {
                tracing::error!("render error: {e}");
            }
        } else if let Err(e) = renderer.render(
            store,
            anim,
            overlay.as_deref(),
            [0.0, layout.editor_y, 0.0, 0.0],
            true,
            |device, queue, pass| {
                if let Err(e) = imgui.draw_to_pass(device, queue, pass) {
                    tracing::warn!("imgui draw failed: {e:#}");
                }
            },
        ) {
            tracing::error!("render error: {e}");
        }
        self.frame_count += 1;
        let needs_anim = tiling_animating
            || self.anim.is_animating()
            || (!hide_cursor && self.anim.is_blinking(&self.store))
            || self.anim.render_deadline().is_some()
            || tiling.preview_alpha() > 0.01
            || tiling.is_dragging();
        if self.context_menu.open
            || project_picker.is_open()
            || file_picker.is_open()
            || grep_picker.is_open()
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
            pin_file_list,
            pin_grep_results,
        )
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
            UserEvent::Exited => {
                // nvim exited (e.g. `:q`); tear down and close the app.
                tracing::info!("nvim exited; closing");
                self.state = None;
                event_loop.exit();
            }
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
            let picker_open = self.project_picker.is_open()
                || self.file_picker.is_open()
                || self.grep_picker.is_open();
            let redraw = state.context_menu.open
                || picker_open
                || self.tiling.is_dragging()
                || imgui_captures_input(state, &self.settings, &self.tiling, picker_open);
            if redraw {
                state.window.request_redraw();
            }
        }
        let picker_open = self.project_picker.is_open()
            || self.file_picker.is_open()
            || self.grep_picker.is_open();
        match event {
            WindowEvent::CloseRequested => {
                let Some(state) = self.state.as_mut() else {
                    return;
                };
                state.session.kill();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                let Some(state) = self.state.as_mut() else {
                    return;
                };
                let scale = state.window.scale_factor() as f32;
                state.renderer.resize(size.width, size.height, scale);
                state.recompute_grid(&self.settings, &self.tiling);
                state.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let Some(state) = self.state.as_mut() else {
                    return;
                };
                let size = state.window.inner_size();
                let scale = state.window.scale_factor() as f32;
                state.renderer.resize(size.width, size.height, scale);
                state.recompute_grid(&self.settings, &self.tiling);
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
                if self.state.as_ref().is_some_and(|s| {
                    cursor_in_panel_content(s, &self.settings, &self.tiling)
                        || self.tiling.focused_view().captures_input()
                }) || picker_open
                {
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
                        state.session.paste(text);
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

                if self.state.as_ref().is_some_and(|s| {
                    is_font_increase_shortcut(&event.logical_key, s.mods)
                }) {
                    self.adjust_font_size(1.0);
                    return;
                }
                if self.state.as_ref().is_some_and(|s| {
                    is_font_decrease_shortcut(&event.logical_key, s.mods)
                }) {
                    self.adjust_font_size(-1.0);
                    return;
                }

                if self.tiling.focused_view().captures_input() {
                    return;
                }

                if self.state.as_ref().is_some_and(|s| {
                    is_project_picker_shortcut(&event.logical_key, s.mods)
                }) {
                    self.project_picker.open();
                    if let Some(state) = self.state.as_mut() {
                        state.window.request_redraw();
                    }
                    return;
                }

                if self.state.as_ref().is_some_and(|s| {
                    is_file_picker_shortcut(&event.logical_key, s.mods)
                }) {
                    let root = self
                        .project
                        .as_ref()
                        .map(|p| p.path.clone())
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    self.file_picker.open(&root);
                    if let Some(state) = self.state.as_mut() {
                        state.window.request_redraw();
                    }
                    return;
                }

                if self.state.as_ref().is_some_and(|s| {
                    is_grep_picker_shortcut(&event.logical_key, s.mods)
                }) {
                    let root = self
                        .project
                        .as_ref()
                        .map(|p| p.path.clone())
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    self.grep_picker.open(&root);
                    if let Some(state) = self.state.as_mut() {
                        state.window.request_redraw();
                    }
                    return;
                }

                let paste_shortcut = self
                    .state
                    .as_ref()
                    .is_some_and(|s| is_paste_shortcut(&event.logical_key, s.mods));
                if paste_shortcut {
                    let imgui_wants_kb = self.state.as_ref().is_some_and(|s| {
                        imgui_captures_input(s, &self.settings, &self.tiling, picker_open)
                            || keyboard_to_nvim_blocked(
                                s,
                                &self.settings,
                                &self.tiling,
                                picker_open,
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
                    keyboard_to_nvim_blocked(s, &self.settings, &self.tiling, picker_open)
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
                if cursor_in_menu_bar(state, &self.settings) || self.tiling.is_dragging() {
                    state.window.request_redraw();
                }
                if self.tiling.is_dragging() {
                    let area = editor_area(&self.settings, &state.renderer);
                    let cursor = cursor_logical(state);
                    self.tiling.update_drag(cursor, area);
                    state.window.request_redraw();
                    return;
                }
                if mouse_to_nvim_blocked(state, &self.settings, &self.tiling, picker_open) {
                    return;
                }
                if let Some(btn) = state.mouse_down {
                    let (row, col) = state.hit_test(&self.settings, &self.tiling);
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

                if button == MouseButton::Left {
                    let area = editor_area(&self.settings, &state.renderer);
                    let rects = self.tiling.compute_rects(area);
                    let cursor = cursor_logical(state);
                    match btn_state {
                        ElementState::Pressed => {
                            if let Some(win_id) = self.tiling.hit_close_button(cursor, &rects)
                            {
                                self.tiling.hide_window(win_id, area);
                                state.recompute_grid(&self.settings, &self.tiling);
                                state.window.request_redraw();
                                return;
                            }
                            if let Some(win_id) =
                                self.tiling.hit_fullscreen_button(cursor, &rects)
                            {
                                self.tiling.fullscreen(win_id, area);
                                state.recompute_grid(&self.settings, &self.tiling);
                                state.window.request_redraw();
                                return;
                            }
                            if let Some(win_id) = self.tiling.hit_title_bar(cursor, &rects) {
                                self.tiling.begin_drag(win_id, cursor);
                                state.window.request_redraw();
                                return;
                            }
                            if let Some(win_id) = self.tiling.hit_window_content(cursor, &rects)
                            {
                                self.tiling.set_focus(win_id);
                            }
                        }
                        ElementState::Released => {
                            if self.tiling.is_dragging() {
                                self.tiling.commit_drop(area);
                                state.recompute_grid(&self.settings, &self.tiling);
                                state.window.request_redraw();
                                return;
                            }
                        }
                    }
                }

                if mouse_to_nvim_blocked(state, &self.settings, &self.tiling, picker_open) {
                    state.window.request_redraw();
                    return;
                }

                if button == MouseButton::Right && btn_state == ElementState::Pressed {
                    if let Some(grid_pos) =
                        state.hit_test_editor(&self.settings, &self.tiling)
                    {
                        let screen_pos = state.imgui.mouse_pos();
                        state.context_menu.open_at(screen_pos, grid_pos);
                        state.window.request_redraw();
                        return;
                    }
                }

                let Some(btn) = map_button(button) else {
                    return;
                };
                let (row, col) = state.hit_test(&self.settings, &self.tiling);
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
                    mouse_to_nvim_blocked(s, &self.settings, &self.tiling, picker_open)
                }) {
                    return;
                }
                let Some(state) = self.state.as_mut() else {
                    return;
                };
                let (row, col) = state.hit_test(&self.settings, &self.tiling);
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
                let (
                    needs_anim,
                    menu_action,
                    context_action,
                    project_selection,
                    pin_file_list,
                    pin_grep_results,
                ) = {
                    let Some(state) = self.state.as_mut() else {
                        return;
                    };
                    state.render(
                        &mut self.settings,
                        &mut self.menu_bar,
                        &mut self.tiling,
                        &mut self.git_client,
                        &mut self.project_picker,
                        &mut self.file_picker,
                        &mut self.grep_picker,
                        &mut self.file_list_panels,
                        &mut self.grep_result_panels,
                    )
                };
                self.handle_menu_action(menu_action);
                self.handle_context_menu_action(context_action);
                if let Some(path) = project_selection {
                    self.set_project(path);
                }
                self.handle_pin_outcomes(pin_file_list, pin_grep_results);
                self.cleanup_panels();
                let picker_open = self.project_picker.is_open()
                    || self.file_picker.is_open()
                    || self.grep_picker.is_open();
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
                        || self.tiling.is_dragging()
                        || (self.tiling.focused_view() == ViewKind::Editor
                            && imgui_captures_input(
                                s,
                                &self.settings,
                                &self.tiling,
                                picker_open,
                            ))
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
        let picker_open = self.project_picker.is_open()
            || self.file_picker.is_open()
            || self.grep_picker.is_open();

        // For git windows the event loop should stay in Wait mode when idle.
        // Redraws are triggered by the GitRefreshed user-event that the
        // background threads send via the proxy.  Forcing Poll here would
        // spin the render loop and block the main thread with git subprocess
        // calls every frame.
        let needs_continuous = state.context_menu.open
            || picker_open
            || self.project_picker.is_animating()
            || self.file_picker.is_animating()
            || self.grep_picker.is_animating()
            || self.tiling.is_dragging()
            || self.tiling.preview_alpha() > 0.01
            || (self.tiling.focused_view() == ViewKind::Editor
                && imgui_captures_input(state, &self.settings, &self.tiling, picker_open))
            || state.anim.is_animating()
            || (self.tiling.focused_view() == ViewKind::Editor
                && state.anim.is_blinking(&state.store));

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

fn is_file_picker_shortcut(key: &Key, mods: Mods) -> bool {
    if mods.alt || mods.shift || mods.ctrl {
        return false;
    }
    matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("o")) && mods.meta
}

fn is_grep_picker_shortcut(key: &Key, mods: Mods) -> bool {
    if mods.alt || mods.ctrl {
        return false;
    }
    matches!(key, Key::Character(c) if c.eq_ignore_ascii_case("f")) && mods.meta && mods.shift
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

fn read_clipboard_text() -> Option<String> {
    let mut clipboard = Clipboard::new().ok()?;
    clipboard.get_text().ok().filter(|text| !text.is_empty())
}

/// When launched from a shell, re-exec in the background so the terminal prompt returns.
const DETACHED_ENV: &str = "NVIM_UI_DETACHED";

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
    if try_detach_from_terminal()? {
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let initial_project = parse_cli_project_dir()
        .or_else(|| std::env::current_dir().ok())
        .map(Project::new);
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy, initial_project)?;
    event_loop.run_app(&mut app)?;
    Ok(())
}
