//! winit shell: owns the window, GPU renderer, embedded nvim session, grid
//! state and animation engine, and wires input/resize/redraw together
//! (AGENT_RUST_PORT.md §3, §7, §8).

mod imgui_layer;
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
use imgui_layer::ImguiLayer;
use settings::{spawn_watcher, Settings};
use settings_ui::{revert_draft, SettingsAction, SettingsUi};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey as WinitNamed};
use winit::window::{Window, WindowId};

/// Apply (or clear) a native blurred backdrop behind the transparent window.
///
/// Idempotent: any previously-applied effect view is removed first, so this is
/// safe to call repeatedly. On macOS we use `NSVisualEffectView` with
/// `BehindWindow` blending and then force the wgpu `CAMetalLayer` in front of it
/// (see `raise_metal_layer`), because the effect view and the Metal layer are
/// siblings whose z-order is otherwise ambiguous.
#[cfg(target_os = "macos")]
fn apply_window_blur(window: &Window, enabled: bool) {
    use window_vibrancy::{
        apply_vibrancy, clear_vibrancy, NSVisualEffectMaterial, NSVisualEffectState,
    };
    let _ = clear_vibrancy(window);
    if enabled {
        let _ = apply_vibrancy(
            window,
            NSVisualEffectMaterial::UnderWindowBackground,
            Some(NSVisualEffectState::Active),
            None,
        );
        raise_metal_layer(window);
    }
}

#[cfg(not(target_os = "macos"))]
fn apply_window_blur(_window: &Window, _enabled: bool) {}

/// Force the wgpu `CAMetalLayer` to render in front of the blur effect view by
/// giving it a higher `zPosition`. The blur view's backing layer stays at the
/// default `zPosition` of 0, so the editor content composites on top of it
/// while still letting the translucent background reveal the blur behind.
#[cfg(target_os = "macos")]
fn raise_metal_layer(window: &Window) {
    use objc2::runtime::NSObjectProtocol;
    use objc2::ClassType;
    use objc2_app_kit::NSView;
    use objc2_quartz_core::CAMetalLayer;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
    // SAFETY: winit hands us a valid `NSView` pointer for the window's content
    // view, and we only touch it on the main (event-loop) thread.
    unsafe {
        let view: &NSView = h.ns_view.cast().as_ref();
        let Some(layer) = view.layer() else { return };
        let Some(sublayers) = layer.sublayers() else { return };
        for sub in sublayers.iter() {
            if sub.isKindOfClass(CAMetalLayer::class()) {
                sub.setZPosition(1.0);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn raise_metal_layer(_window: &Window) {}

#[derive(Debug)]
enum UserEvent {
    Redraw(Vec<Value>),
    SettingsReloaded,
    Exited,
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
    blur_on: bool,
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
        let reload_proxy = proxy.clone();
        spawn_watcher(move || {
            let _ = reload_proxy.send_event(UserEvent::SettingsReloaded);
        });
        Ok(App {
            proxy,
            rt,
            settings,
            settings_ui,
            state: None,
        })
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<State> {
        let attrs = Window::default_attributes()
            .with_title("nvim-ui")
            .with_transparent(true);
        let window = Arc::new(event_loop.create_window(attrs)?);
        window.set_ime_allowed(true);
        apply_window_blur(&window, self.settings.window_blur);

        let mut renderer = Renderer::new(
            window.clone(),
            self.settings.font_family.as_deref(),
            self.settings.font_size,
            self.settings.line_height,
        )?;
        renderer.set_opacity(self.settings.window_opacity);
        // The Metal layer only exists after the renderer is created, so put it
        // in front of the (already-applied) blur backdrop now.
        if self.settings.window_blur {
            raise_metal_layer(&window);
        }

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
        let (scroll, far) = session.fetch_neovide_scroll_globals();
        self.settings.apply_neovide_scroll_globals(scroll, far);
        self.settings_ui.draft = self.settings.clone();
        tracing::info!("nvim attached: {cols}x{rows} cells, cell={cw:.1}x{ch:.1}px, scale={}", renderer.scale());

        let anim = AnimationState::new(self.settings.animation_config());
        let imgui = ImguiLayer::new(&window, &renderer);

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
            blur_on: self.settings.window_blur,
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
        state.renderer.set_opacity(d.window_opacity);
        // Only (re)apply the native blur when it actually toggles, so dragging
        // the opacity slider doesn't restack the effect view every frame.
        if d.window_blur != state.blur_on {
            apply_window_blur(&state.window, d.window_blur);
            state.blur_on = d.window_blur;
        }
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

    fn reload_settings_from_disk(&mut self) {
        if self.settings_ui.open {
            return;
        }
        let loaded = Settings::load();
        if loaded == self.settings {
            return;
        }
        tracing::info!("reloading settings from disk");
        self.settings = loaded;
        revert_draft(&mut self.settings_ui, &self.settings);
        self.apply_preview();
        if let Some(state) = self.state.as_ref() {
            state.window.request_redraw();
        }
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

    fn render(&mut self, settings_ui: &mut SettingsUi) -> (bool, SettingsAction) {
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
        let mut imgui_action = SettingsAction::None;
        if settings_ui.open {
            let window = self.window.clone();
            if let Err(e) = self.imgui.prepare_ui(&window, |ui| {
                imgui_action = settings_ui.draw(ui);
            }) {
                tracing::warn!("imgui frame failed: {e:#}");
            }
        }

        self.anim.update(dt, &self.store, cw, ch);

        let draw_imgui = settings_ui.open;
        let store = &self.store;
        let anim = &mut self.anim;
        let imgui = &mut self.imgui;
        let renderer = &mut self.renderer;
        if let Err(e) = renderer.render(
            store,
            anim,
            overlay.as_deref(),
            |device, queue, pass| {
                if draw_imgui {
                    if let Err(e) = imgui.draw_to_pass(device, queue, pass) {
                        tracing::warn!("imgui draw failed: {e:#}");
                    }
                }
            },
        ) {
            tracing::error!("render error: {e}");
        }
        if draw_imgui {
            imgui.discard_frame();
        }
        self.frame_count += 1;
        let needs_anim = self.anim.is_animating()
            || self.anim.is_blinking(&self.store)
            || self.anim.render_deadline().is_some();
        if settings_ui.open || needs_anim {
            self.window.request_redraw();
        }
        (needs_anim, imgui_action)
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
                    let scroll = state.store.take_pending_scroll();
                    state.anim.on_flush(&state.store, &scroll);
                    state.anim.sync_floats(&state.store.active_floats());
                    state.window.request_redraw();
                }
            }
            UserEvent::SettingsReloaded => {
                self.reload_settings_from_disk();
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
            if self.settings_ui.open {
                state.window.request_redraw();
            }
        }
        match event {
            WindowEvent::CloseRequested => {
                let Some(state) = self.state.as_mut() else { return };
                state.session.kill();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                let Some(state) = self.state.as_mut() else { return };
                let scale = state.window.scale_factor() as f32;
                state.renderer.resize(size.width, size.height, scale);
                state.recompute_grid();
                state.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let Some(state) = self.state.as_mut() else { return };
                let size = state.window.inner_size();
                let scale = state.window.scale_factor() as f32;
                state.renderer.resize(size.width, size.height, scale);
                state.recompute_grid();
                state.window.request_redraw();
            }
            WindowEvent::Focused(focused) => {
                let Some(state) = self.state.as_mut() else { return };
                state.session.set_focus(focused);
                state.anim.set_focus(focused);
            }
            WindowEvent::ModifiersChanged(m) => {
                let Some(state) = self.state.as_mut() else { return };
                let s = m.state();
                state.mods = Mods {
                    ctrl: s.control_key(),
                    alt: s.alt_key(),
                    meta: s.super_key(),
                    shift: s.shift_key(),
                };
            }
            WindowEvent::Ime(ime) => {
                let Some(state) = self.state.as_mut() else { return };
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
                        return;
                    }
                    if self.state.as_ref().is_some_and(|s| s.imgui.wants_keyboard()) {
                        return;
                    }
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
                let Some(state) = self.state.as_mut() else { return };
                state.cursor_pos = (position.x, position.y);
                if self.settings_ui.open && state.imgui.wants_mouse() {
                    return;
                }
                if let Some(btn) = state.mouse_down {
                    let (row, col) = state.hit_test();
                    state
                        .session
                        .mouse(btn, MouseAction::Drag, mods_string(state.mods), 0, row, col);
                }
            }
            WindowEvent::MouseInput { state: btn_state, button, .. } => {
                if self.settings_ui.open
                    && self.state.as_ref().is_some_and(|s| s.imgui.wants_mouse())
                {
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
                if self.settings_ui.open
                    && self.state.as_ref().is_some_and(|s| s.imgui.wants_mouse())
                {
                    return;
                }
                let Some(state) = self.state.as_mut() else { return };
                let (row, col) = state.hit_test();
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
                let (needs_anim, action) = {
                    let Some(state) = self.state.as_mut() else { return };
                    state.render(&mut self.settings_ui)
                };
                self.handle_settings_action(action);
                if needs_anim {
                    if let Some(deadline) = self.state.as_ref().and_then(|s| s.anim.render_deadline()) {
                        event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
                    } else {
                        event_loop.set_control_flow(ControlFlow::Poll);
                    }
                } else if self.settings_ui.open {
                    event_loop.set_control_flow(ControlFlow::Poll);
                } else {
                    event_loop.set_control_flow(ControlFlow::Wait);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = self.state.as_ref() else { return };
        if self.settings_ui.open
            || state.anim.is_animating()
            || state.anim.is_blinking(&state.store)
        {
            state.window.request_redraw();
            if let Some(deadline) = state.anim.render_deadline() {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            } else {
                event_loop.set_control_flow(ControlFlow::Poll);
            }
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
