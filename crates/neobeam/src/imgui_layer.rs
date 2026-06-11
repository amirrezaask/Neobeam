//! Dear ImGui integration: winit input + wgpu rendering into the editor surface.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use arboard::Clipboard;
use editor_surface::{Renderer, SYMBOLS_NERD_FONT};
use imgui::ClipboardBackend;
use imgui::{Context, FontConfig, FontGlyphRanges, FontSource, Ui};
use nvim_core::grid::GridStateStore;

use crate::imgui_theme::apply_nvim_theme;
use imgui_wgpu::{Renderer as ImguiRenderer, RendererConfig, Texture as ImguiTexture};
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
    nvim_texture_id: Option<imgui::TextureId>,
    /// Shared view so callers can render into the texture without borrowing
    /// the imgui texture map.
    nvim_texture_view: Option<Arc<wgpu::TextureView>>,
    nvim_texture_size: Option<(u32, u32)>,
    terminal_textures: HashMap<u32, TerminalTexture>,
}

struct TerminalTexture {
    id: imgui::TextureId,
    view: Arc<wgpu::TextureView>,
    size: (u32, u32),
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
        imgui_renderer.reload_font_texture(&mut ctx, editor.device(), editor.queue());

        ImguiLayer {
            ctx,
            platform,
            renderer: imgui_renderer,
            last_frame: Instant::now(),
            frame_ready: false,
            font_size_px,
            hidpi,
            nvim_texture_id: None,
            nvim_texture_view: None,
            nvim_texture_size: None,
            terminal_textures: HashMap::new(),
        }
    }

    /// Register (or re-register on resize) the nvim offscreen texture.
    /// Keeps an `Arc` clone of the view so callers can render into it without
    /// borrowing the imgui texture map at the same time.
    pub fn register_nvim_texture(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> imgui::TextureId {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let raw = Arc::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nvim-offscreen"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        }));
        let view = Arc::new(raw.create_view(&wgpu::TextureViewDescriptor::default()));

        let sampler_desc = wgpu::SamplerDescriptor {
            label: Some("nvim-offscreen-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        };
        let raw_cfg = imgui_wgpu::RawTextureConfig {
            label: Some("nvim-offscreen"),
            sampler_desc,
        };
        let texture = ImguiTexture::from_raw_parts(
            device,
            &self.renderer,
            raw,
            view.clone(),
            None,
            Some(&raw_cfg),
            size,
        );

        self.nvim_texture_view = Some(view);
        self.nvim_texture_size = Some((width, height));

        if let Some(old_id) = self.nvim_texture_id {
            self.renderer.textures.replace(old_id, texture);
            old_id
        } else {
            let id = self.renderer.textures.insert(texture);
            self.nvim_texture_id = Some(id);
            id
        }
    }

    /// TextureId for the nvim offscreen texture (for `ui.image()`).
    pub fn nvim_texture_id(&self) -> Option<imgui::TextureId> {
        self.nvim_texture_id
    }

    /// Cloned `Arc` to the offscreen view — safe to hold while also mutably
    /// borrowing the renderer, since it doesn't touch the imgui texture map.
    pub fn nvim_texture_view_arc(&self) -> Option<Arc<wgpu::TextureView>> {
        self.nvim_texture_view.clone()
    }

    /// Physical pixel dimensions of the registered nvim texture.
    pub fn nvim_texture_size(&self) -> Option<(u32, u32)> {
        self.nvim_texture_size
    }

    /// Register (or replace on resize) the terminal offscreen texture.
    pub fn register_terminal_texture(
        &mut self,
        pane_id: u32,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> imgui::TextureId {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let raw = Arc::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terminal-offscreen"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        }));
        let view = Arc::new(raw.create_view(&wgpu::TextureViewDescriptor::default()));
        let raw_cfg = imgui_wgpu::RawTextureConfig {
            label: Some("terminal-offscreen"),
            sampler_desc: wgpu::SamplerDescriptor {
                label: Some("terminal-offscreen-sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            },
        };
        let texture = ImguiTexture::from_raw_parts(
            device,
            &self.renderer,
            raw,
            view.clone(),
            None,
            Some(&raw_cfg),
            size,
        );

        let id = if let Some(old) = self.terminal_textures.get(&pane_id) {
            let old_id = old.id;
            self.renderer.textures.replace(old_id, texture);
            old_id
        } else {
            self.renderer.textures.insert(texture)
        };
        self.terminal_textures.insert(
            pane_id,
            TerminalTexture {
                id,
                view,
                size: (width, height),
            },
        );
        id
    }

    pub fn terminal_texture_id(&self, pane_id: u32) -> Option<imgui::TextureId> {
        self.terminal_textures
            .get(&pane_id)
            .map(|texture| texture.id)
    }

    pub fn terminal_texture_view_arc(&self, pane_id: u32) -> Option<Arc<wgpu::TextureView>> {
        self.terminal_textures
            .get(&pane_id)
            .map(|texture| texture.view.clone())
    }

    pub fn terminal_texture_size(&self, pane_id: u32) -> Option<(u32, u32)> {
        self.terminal_textures
            .get(&pane_id)
            .map(|texture| texture.size)
    }

    pub fn remove_terminal_texture(&mut self, pane_id: u32) {
        if let Some(texture) = self.terminal_textures.remove(&pane_id) {
            self.renderer.textures.remove(texture.id);
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
        self.ctx.io_mut().update_delta_time(now - self.last_frame);
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
/// Mirrors the ranges in editor_surface::nerd_glyphs::is_nerd_glyph.
static NERD_ICON_GLYPH_RANGES: &[u32] = &[
    0x23FB, 0x23FE, // IEC power symbols
    0x2500, 0x259F, // box drawing (patched)
    0x2630, 0x2630, // powerline extra
    0x2665, 0x2665,
    0x26A1, 0x26A1, // octicons
    0x276C, 0x2771, // heavy angle brackets
    0x2B58, 0x2B58, // IEC power
    0xE000, 0xE00A, // pomicons
    0xE0A0, 0xE0A2, // powerline
    0xE0A3, 0xE0A3, // powerline extra
    0xE0B0, 0xE0B3, // powerline
    0xE0B4, 0xE0C8, // powerline extra
    0xE0CA, 0xE0CA, // powerline extra
    0xE0CC, 0xE0D7, // powerline extra
    0xE200, 0xE2A9, // font awesome extension
    0xE300, 0xE3E3, // weather icons
    0xE5FA, 0xE6B7, // seti-ui + custom
    0xE700, 0xE8EF, // devicons
    0xEA60, 0xEC1E, // codicons
    0xED00, 0xEFCE, // font awesome (includes progress at EE00..EE0B)
    0xF000, 0xF2FF, // font awesome
    0xF300, 0xF381, // font logos
    0xF400, 0xFD46, // octicons + material design
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
