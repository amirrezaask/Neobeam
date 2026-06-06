//! winit shell: owns the window, GPU renderer, embedded nvim session, grid
//! state and animation engine, and wires input/resize/redraw together
//! (AGENT_RUST_PORT.md §3, §7, §8).

mod settings;
mod settings_ui;

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use editor_surface::{AnimationState, Renderer};
use nvim_core::grid::GridStateStore;
use nvim_core::input::{
    encode_key, mods_string, KeyInput, Mods, MouseAction, MouseButton as CoreButton, NamedKey,
};
use nvim_core::protocol::parse_redraw;
use nvim_core::session::{NvimSession, SessionConfig};
use nvim_core::Value;
use settings::Settings;
use settings_ui::{revert_draft, SettingsAction, SettingsUi};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey as WinitNamed};
use winit::window::{Window, WindowId};

#[derive(Debug)]
enum UserEvent {
    Redraw(Vec<Value>),
    Exited,
}

struct State {
    window: Arc<Window>,
    renderer: Renderer,
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
    grid_cols: u32,
    grid_rows: u32,
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    rt: tokio::runtime::Runtime,
    settings: Settings,
    settings_ui: SettingsUi,
    state: Option<State>,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let settings = Settings::load();
        settings.save(); // ensure a settings.json exists for the user to edit (§9)
        let settings_ui = SettingsUi::new(&settings);
        Ok(App {
            proxy,
            rt,
            settings,
            settings_ui,
            state: None,
        })
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<State> {
        let attrs = Window::default_attributes().with_title("nvim-ui");
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
        let cols = ((lw / cw).floor() as u32).max(1);
        let rows = ((lh / ch).floor() as u32).max(1);

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
            SessionConfig { cols, rows, ..Default::default() },
            redraw,
            on_close,
        )?;
        tracing::info!("nvim attached: {cols}x{rows} cells, cell={cw:.1}x{ch:.1}px, scale={}", renderer.scale());

        let anim = AnimationState::new(self.settings.animation_config());

        Ok(State {
            window,
            renderer,
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
            grid_cols: cols,
            grid_rows: rows,
        })
    }

    fn apply_preview(&mut self) {
        let Some(state) = self.state.as_mut() else { return };
        let d = self.settings_ui.draft.clone();
        if let Err(e) = state.renderer.apply_font(
            d.font_family.as_deref(),
            d.font_size,
            d.line_height,
        ) {
            tracing::warn!("font preview failed: {e:#}");
        }
        state.anim.cfg = d.animation_config();
        state.recompute_grid();
    }

    fn apply_settings(&mut self) {
        self.settings = self.settings_ui.draft.clone();
        self.settings.save();
        self.apply_preview();
    }

    fn cancel_settings(&mut self) {
        revert_draft(&mut self.settings_ui, &self.settings);
        self.apply_preview();
    }

    fn handle_settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::None => {}
            SettingsAction::Preview => {
                self.apply_preview();
                if let Some(state) = self.state.as_ref() {
                    state.window.request_redraw();
                }
            }
            SettingsAction::Apply => {
                self.apply_settings();
                self.settings_ui.open = false;
                if let Some(state) = self.state.as_ref() {
                    state.window.request_redraw();
                }
            }
            SettingsAction::CloseCancel => {
                self.cancel_settings();
                self.settings_ui.open = false;
                if let Some(state) = self.state.as_ref() {
                    state.window.request_redraw();
                }
            }
        }
    }
}

impl State {
    fn hit_test(&self) -> (i64, i64) {
        let scale = self.renderer.scale() as f64;
        let (cw, ch) = self.renderer.cell_size();
        let col = (self.cursor_pos.0 / scale / cw as f64).floor() as i64;
        let row = (self.cursor_pos.1 / scale / ch as f64).floor() as i64;
        (row.max(0), col.max(0))
    }

    fn recompute_grid(&mut self) {
        let (cw, ch) = self.renderer.cell_size();
        let (lw, lh) = self.renderer.logical_size();
        let cols = ((lw / cw).floor() as u32).max(1);
        let rows = ((lh / ch).floor() as u32).max(1);
        if cols != self.grid_cols || rows != self.grid_rows {
            self.grid_cols = cols;
            self.grid_rows = rows;
            self.session.resize(cols, rows);
        }
    }

    fn render(&mut self, settings_ui: &mut SettingsUi) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;

        // Only update from "active" frames; a huge dt means we woke from idle
        // (the gate stopped requesting frames), which isn't a real frame time.
        if dt > 0.0 && dt < 0.1 {
            let inst = 1.0 / dt;
            self.fps = if self.fps == 0.0 { inst } else { self.fps * 0.9 + inst * 0.1 };
        }
        let overlay = if settings_ui.open {
            None
        } else {
            Some(format!("{:>3.0} FPS  {:>4.1} ms", self.fps, dt * 1000.0))
        };

        let (cw, ch) = self.renderer.cell_size();
        let (lw, lh) = self.renderer.logical_size();
        self.renderer.with_atlas_queue(|atlas, queue| {
            settings_ui.rebuild(atlas, queue, lw, lh);
        });
        let ui = settings_ui.overlay();

        self.anim.update(dt, &self.store, cw, ch);
        if let Err(e) = self.renderer.render(
            &self.store,
            &mut self.anim,
            overlay.as_deref(),
            ui,
        ) {
            tracing::error!("render error: {e}");
        }
        self.frame_count += 1;
        if settings_ui.open
            || self.anim.is_animating()
            || self.anim.is_blinking(&self.store)
        {
            self.window.request_redraw();
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
                let Some(state) = self.state.as_mut() else { return };
                let events = parse_redraw(&args);
                let flushed = state.store.apply_batch(events);
                if flushed {
                    let (_, ch) = state.renderer.cell_size();
                    for (grid, delta) in state.store.take_pending_scroll() {
                        let visible = state.store.grid(grid).map(|g| g.height).unwrap_or(40);
                        state.anim.seed_scroll(grid, delta, ch, visible);
                    }
                    state.anim.sync_floats(&state.store.active_floats());
                    state.window.request_redraw();
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
        _id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => {
                state.session.kill();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                let scale = state.window.scale_factor() as f32;
                state.renderer.resize(size.width, size.height, scale);
                state.recompute_grid();
                state.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = state.window.inner_size();
                let scale = state.window.scale_factor() as f32;
                state.renderer.resize(size.width, size.height, scale);
                state.recompute_grid();
                state.window.request_redraw();
            }
            WindowEvent::Focused(focused) => {
                state.session.set_focus(focused);
            }
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                state.mods = Mods {
                    ctrl: s.control_key(),
                    alt: s.alt_key(),
                    meta: s.super_key(),
                    shift: s.shift_key(),
                };
            }
            WindowEvent::Ime(ime) => match ime {
                Ime::Enabled => state.ime_active = true,
                Ime::Preedit(text, _) => state.ime_active = !text.is_empty(),
                Ime::Commit(text) => {
                    state.ime_active = false;
                    state.session.paste(text);
                }
                Ime::Disabled => state.ime_active = false,
            },
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                let ime_active = self.state.as_ref().is_some_and(|s| s.ime_active);
                if !pressed || ime_active {
                    return;
                }

                let cmd_comma = self.state.as_ref().is_some_and(|s| {
                    matches!(&event.logical_key, Key::Character(c) if c.as_str() == "," && s.mods.meta)
                });
                if cmd_comma {
                    self.settings_ui.toggle(&self.settings);
                    if self.settings_ui.open {
                        self.apply_preview();
                    } else {
                        self.cancel_settings();
                    }
                    if let Some(state) = self.state.as_ref() {
                        state.window.request_redraw();
                    }
                    return;
                }

                if self.settings_ui.open {
                    if matches!(&event.logical_key, Key::Named(WinitNamed::Escape)) {
                        self.handle_settings_action(SettingsAction::CloseCancel);
                    }
                    return;
                }

                let Some(state) = self.state.as_mut() else { return };
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
                if let Some(state) = self.state.as_mut() {
                    state.cursor_pos = (position.x, position.y);
                }
                if self.settings_ui.open {
                    let scale = self
                        .state
                        .as_ref()
                        .map(|s| s.renderer.scale())
                        .unwrap_or(1.0);
                    let (lw, lh) = self
                        .state
                        .as_ref()
                        .map(|s| s.renderer.logical_size())
                        .unwrap_or((800.0, 600.0));
                    self.settings_ui.handle_mouse_move(
                        position.x as f32 / scale,
                        position.y as f32 / scale,
                        lw,
                        lh,
                    );
                    if self.settings_ui.is_dragging() {
                        self.apply_preview();
                    }
                    if let Some(state) = self.state.as_ref() {
                        state.window.request_redraw();
                    }
                    return;
                }
                let Some(state) = self.state.as_mut() else { return };
                if let Some(btn) = state.mouse_down {
                    let (row, col) = state.hit_test();
                    state
                        .session
                        .mouse(btn, MouseAction::Drag, mods_string(state.mods), 0, row, col);
                }
            }
            WindowEvent::MouseInput { state: btn_state, button, .. } => {
                if self.settings_ui.open && button == MouseButton::Left {
                    let (lx, ly, lw, lh) = {
                        let Some(state) = self.state.as_ref() else { return };
                        let scale = state.renderer.scale();
                        let (lw, lh) = state.renderer.logical_size();
                        (
                            state.cursor_pos.0 as f32 / scale,
                            state.cursor_pos.1 as f32 / scale,
                            lw,
                            lh,
                        )
                    };
                    let action = match btn_state {
                        ElementState::Pressed => {
                            self.settings_ui.handle_mouse_down(lx, ly, lw, lh)
                        }
                        ElementState::Released => self.settings_ui.handle_mouse_up(),
                    };
                    self.handle_settings_action(action);
                    return;
                }
                let Some(state) = self.state.as_mut() else { return };
                let Some(btn) = map_button(button) else { return };
                let (row, col) = state.hit_test();
                match btn_state {
                    ElementState::Pressed => {
                        state.mouse_down = Some(btn);
                        state
                            .session
                            .mouse(btn, MouseAction::Press, mods_string(state.mods), 0, row, col);
                    }
                    ElementState::Released => {
                        state.mouse_down = None;
                        state
                            .session
                            .mouse(btn, MouseAction::Release, mods_string(state.mods), 0, row, col);
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.settings_ui.open {
                    let (lw, lh) = state.renderer.logical_size();
                    let y = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y * 3.0,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32 / 20.0,
                    };
                    self.settings_ui.handle_wheel(y, lw, lh);
                    state.window.request_redraw();
                    return;
                }
                let (row, col) = state.hit_test();
                let (_, ch) = state.renderer.cell_size();
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y as f32) / ch.max(1.0),
                };
                let steps = lines.abs().round().max(1.0) as i32;
                let btn = if lines >= 0.0 {
                    CoreButton::WheelUp
                } else {
                    CoreButton::WheelDown
                };
                for _ in 0..steps {
                    state
                        .session
                        .mouse(btn, MouseAction::Press, mods_string(state.mods), 0, row, col);
                }
            }
            WindowEvent::RedrawRequested => {
                state.render(&mut self.settings_ui);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        let Some(state) = self.state.as_ref() else { return };
        if self.settings_ui.open
            || state.anim.is_animating()
            || state.anim.is_blinking(&state.store)
        {
            state.window.request_redraw();
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy)?;
    event_loop.run_app(&mut app)?;
    Ok(())
}
