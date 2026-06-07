//! Dear ImGui integration: winit input + wgpu rendering into the editor surface.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use arboard::Clipboard;
use editor_surface::{Renderer, SYMBOLS_NERD_FONT};
use imgui::ClipboardBackend;
use imgui::{Context, FontConfig, FontGlyphRanges, FontSource, Ui};
use nvim_core::grid::GridStateStore;

use crate::imgui_theme::apply_nvim_theme;
use imgui_wgpu::{Renderer as ImguiRenderer, RendererConfig};
use imgui_winit_support::{HiDpiMode, WinitPlatform};
use winit::event::{Event, WindowEvent};
use winit::window::{Window, WindowId};

static JETBRAINS_MONO: &[u8] =
    include_bytes!("../assets/fonts/JetBrainsMono-VariableFont_wght.ttf");

struct ArboardClipboard(Clipboard);

impl ClipboardBackend for ArboardClipboard {
    fn get(&mut self) -> Option<String> {
        self.0.get_text().ok()
    }

    fn set(&mut self, value: &str) {
        let _ = self.0.set_text(value);
    }
}

pub struct ImguiLayer {
    ctx: Context,
    platform: WinitPlatform,
    renderer: ImguiRenderer,
    last_frame: Instant,
    frame_ready: bool,
    font_size_px: f32,
    hidpi: f32,
}

impl ImguiLayer {
    pub fn new(window: &Window, editor: &Renderer, font_size_px: f32) -> Self {
        let mut ctx = Context::create();
        ctx.set_ini_filename(None::<std::path::PathBuf>);
        if let Ok(clipboard) = Clipboard::new() {
            ctx.set_clipboard_backend(ArboardClipboard(clipboard));
        }

        let hidpi = window.scale_factor() as f32;
        ctx.io_mut().font_global_scale = (1.0 / hidpi) as f32;
        load_font(&mut ctx, font_size_px, hidpi);

        let mut platform = WinitPlatform::new(&mut ctx);
        platform.attach_window(ctx.io_mut(), window, HiDpiMode::Default);

        let mut imgui_renderer = ImguiRenderer::new(
            &mut ctx,
            editor.device(),
            editor.queue(),
            RendererConfig {
                texture_format: editor.surface_format(),
                ..RendererConfig::new_srgb()
            },
        );
        imgui_renderer.reload_font_texture(
            &mut ctx,
            editor.device(),
            editor.queue(),
        );

        ImguiLayer {
            ctx,
            platform,
            renderer: imgui_renderer,
            last_frame: Instant::now(),
            frame_ready: false,
            font_size_px,
            hidpi,
        }
    }

    pub fn handle_event(&mut self, window: &Window, window_id: WindowId, event: &WindowEvent) {
        self.platform.handle_event(
            self.ctx.io_mut(),
            window,
            &Event::<()>::WindowEvent {
                window_id,
                event: event.clone(),
            },
        );
    }

    pub fn wants_mouse(&self) -> bool {
        self.ctx.io().want_capture_mouse
    }

    pub fn wants_keyboard(&self) -> bool {
        self.ctx.io().want_capture_keyboard
    }

    pub fn mouse_pos(&self) -> [f32; 2] {
        self.ctx.io().mouse_pos
    }

    /// Rebuild the font atlas when the editor font size or HiDPI scale changes.
    pub fn sync_font_size(
        &mut self,
        window: &Window,
        font_size_px: f32,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let hidpi = window.scale_factor() as f32;
        self.ctx.io_mut().font_global_scale = (1.0 / hidpi) as f32;

        if (self.font_size_px - font_size_px).abs() < f32::EPSILON
            && (self.hidpi - hidpi).abs() < f32::EPSILON
        {
            return;
        }

        self.font_size_px = font_size_px;
        self.hidpi = hidpi;
        self.ctx.fonts().clear();
        load_font(&mut self.ctx, font_size_px, hidpi);
        self.renderer
            .reload_font_texture(&mut self.ctx, device, queue);
    }

    /// Build UI for the current frame. Pair with [`Self::draw_to_pass`] or
    /// [`Self::discard_frame`] on the same frame.
    pub fn prepare_ui(
        &mut self,
        window: &Arc<Window>,
        store: &GridStateStore,
        font_size_px: f32,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        build: impl FnOnce(&mut Ui) -> (),
    ) -> Result<()> {
        self.sync_font_size(window.as_ref(), font_size_px, device, queue);

        let now = Instant::now();
        self.ctx
            .io_mut()
            .update_delta_time(now - self.last_frame);
        self.last_frame = now;

        self.discard_frame();

        apply_nvim_theme(&mut self.ctx, store);

        self.platform
            .prepare_frame(self.ctx.io_mut(), window.as_ref())?;
        let ui = self.ctx.new_frame();
        build(ui);
        self.platform.prepare_render(ui, window.as_ref());
        self.frame_ready = true;
        Ok(())
    }

    pub fn draw_to_pass(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
    ) -> Result<()> {
        if !self.frame_ready {
            return Ok(());
        }
        self.frame_ready = false;
        let draw_data = self.ctx.render();
        self.renderer
            .render(draw_data, queue, device, pass)
            .map_err(|e| anyhow::anyhow!("imgui render failed: {e}"))?;
        Ok(())
    }

    /// Finish an imgui frame without drawing (e.g. when the surface frame was skipped).
    pub fn discard_frame(&mut self) {
        if self.frame_ready {
            let _ = self.ctx.render();
            self.frame_ready = false;
        }
    }
}

/// Symbols Nerd Font Mono ranges merged into the ImGui atlas for UI chrome icons.
/// Codicons (activity bar) + Font Awesome (legacy pickers/menus).
static NERD_ICON_GLYPH_RANGES: &[u32] = &[
    0xea60, 0xec1e, // codicons (VS Code activity bar)
    0xf000, 0xf2ff, // font awesome
    0,
];

fn load_font(ctx: &mut Context, font_size_px: f32, hidpi: f32) {
    let size = font_size_px * hidpi;
    let base = FontConfig {
        oversample_h: 2,
        oversample_v: 1,
        pixel_snap_h: true,
        ..Default::default()
    };
    ctx.fonts().add_font(&[
        FontSource::TtfData {
            data: JETBRAINS_MONO,
            size_pixels: size,
            config: Some(base.clone()),
        },
        FontSource::TtfData {
            data: SYMBOLS_NERD_FONT,
            size_pixels: size,
            config: Some(FontConfig {
                glyph_ranges: FontGlyphRanges::from_slice(NERD_ICON_GLYPH_RANGES),
                ..base
            }),
        },
    ]);
}
