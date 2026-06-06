//! Dear ImGui integration: winit input + wgpu rendering into the editor surface.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use editor_surface::Renderer;
use imgui::{Context, FontConfig, FontSource, Ui};
use imgui_wgpu::{Renderer as ImguiRenderer, RendererConfig};
use imgui_winit_support::{HiDpiMode, WinitPlatform};
use winit::event::{Event, WindowEvent};
use winit::window::{Window, WindowId};

static JETBRAINS_MONO: &[u8] =
    include_bytes!("../assets/fonts/JetBrainsMono-VariableFont_wght.ttf");

const SETTINGS_UI_FONT_SIZE: f32 = 15.0;

pub struct ImguiLayer {
    ctx: Context,
    platform: WinitPlatform,
    renderer: ImguiRenderer,
    last_frame: Instant,
    frame_ready: bool,
}

impl ImguiLayer {
    pub fn new(window: &Window, editor: &Renderer) -> Self {
        let mut ctx = Context::create();
        ctx.set_ini_filename(None::<std::path::PathBuf>);

        let hidpi = window.scale_factor();
        ctx.io_mut().font_global_scale = (1.0 / hidpi) as f32;

        ctx.fonts().add_font(&[FontSource::TtfData {
            data: JETBRAINS_MONO,
            size_pixels: SETTINGS_UI_FONT_SIZE * hidpi as f32,
            config: Some(FontConfig {
                oversample_h: 2,
                oversample_v: 1,
                pixel_snap_h: true,
                ..Default::default()
            }),
        }]);

        let mut platform = WinitPlatform::new(&mut ctx);
        platform.attach_window(ctx.io_mut(), window, HiDpiMode::Default);

        let renderer = ImguiRenderer::new(
            &mut ctx,
            editor.device(),
            editor.queue(),
            RendererConfig {
                texture_format: editor.surface_format(),
                ..RendererConfig::new_srgb()
            },
        );

        ImguiLayer {
            ctx,
            platform,
            renderer,
            last_frame: Instant::now(),
            frame_ready: false,
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

    /// Build UI for the current frame. Pair with [`Self::draw_to_pass`] or
    /// [`Self::discard_frame`] on the same frame.
    pub fn prepare_ui(
        &mut self,
        window: &Arc<Window>,
        build: impl FnOnce(&mut Ui) -> (),
    ) -> Result<()> {
        let now = Instant::now();
        self.ctx
            .io_mut()
            .update_delta_time(now - self.last_frame);
        self.last_frame = now;

        self.discard_frame();

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
