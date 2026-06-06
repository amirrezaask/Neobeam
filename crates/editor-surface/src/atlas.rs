//! Shelf-packed glyph atlas backed by an R8 coverage texture.
//!
//! Glyphs are rasterized once at device-pixel size and cached by char. The whole
//! atlas is reset when DPI or font changes (AGENT_RUST_PORT.md §5.2, §13.7).

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use etagere::{size2, AtlasAllocator};
use fontdue::{Font, FontSettings};

use crate::nerd_glyphs::is_nerd_glyph;

static SYMBOLS_NERD_FONT: &[u8] =
    include_bytes!("../assets/fonts/SymbolsNerdFontMono-Regular.ttf");

const ATLAS_SIZE: u32 = 2048;
const PADDING: i32 = 1;

#[derive(Clone, Copy, Debug)]
pub struct GlyphInfo {
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    /// Logical-pixel placement relative to the cell's top-left baseline box.
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

pub struct GlyphAtlas {
    font: Font,
    /// Bundled Symbols Nerd Font Mono for icon fallback when the primary font
    /// lacks Nerd Font glyphs.
    symbols: Font,
    /// Logical font size in px.
    pub size_px: f32,
    pub scale: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    pub ascent: f32,
    pub line_height: f32,

    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    alloc: AtlasAllocator,
    map: HashMap<char, Option<GlyphInfo>>,
}

impl GlyphAtlas {
    pub fn new(
        device: &wgpu::Device,
        font_family: Option<&str>,
        size_px: f32,
        line_height: f32,
        scale: f32,
    ) -> Result<Self> {
        let font = load_font(font_family)?;
        let symbols = load_symbols_font()?;
        let (texture, view, sampler) = create_atlas_texture(device);
        let mut atlas = GlyphAtlas {
            font,
            symbols,
            size_px,
            scale,
            cell_w: 0.0,
            cell_h: 0.0,
            ascent: 0.0,
            line_height,
            texture,
            view,
            sampler,
            alloc: AtlasAllocator::new(size2(ATLAS_SIZE as i32, ATLAS_SIZE as i32)),
            map: HashMap::new(),
        };
        atlas.recompute_metrics();
        Ok(atlas)
    }

    fn recompute_metrics(&mut self) {
        let px = self.size_px * self.scale;
        let m = self.font.metrics('M', px);
        // advance is in device px; convert to logical.
        self.cell_w = (m.advance_width / self.scale).ceil().max(1.0);
        self.cell_h = (self.size_px * self.line_height).ceil().max(1.0);
        if let Some(lm) = self.font.horizontal_line_metrics(px) {
            self.ascent = lm.ascent / self.scale;
            // vertically center the text box within the (taller) cell.
            let text_h = (lm.ascent - lm.descent) / self.scale;
            let pad = ((self.cell_h - text_h) * 0.5).max(0.0);
            self.ascent += pad;
        } else {
            self.ascent = self.size_px;
        }
    }

    /// Reconfigure for a new font size / DPI; clears all cached glyphs.
    pub fn reconfigure(
        &mut self,
        device: &wgpu::Device,
        size_px: f32,
        line_height: f32,
        scale: f32,
    ) {
        self.size_px = size_px;
        self.line_height = line_height;
        self.scale = scale;
        self.clear(device);
        self.recompute_metrics();
    }

    /// Change font family, size, and line height; clears all cached glyphs.
    pub fn reconfigure_font(
        &mut self,
        device: &wgpu::Device,
        font_family: Option<&str>,
        size_px: f32,
        line_height: f32,
        scale: f32,
    ) -> Result<()> {
        self.font = load_font(font_family)?;
        self.size_px = size_px;
        self.line_height = line_height;
        self.scale = scale;
        self.clear(device);
        self.recompute_metrics();
        Ok(())
    }

    fn clear(&mut self, device: &wgpu::Device) {
        self.map.clear();
        self.alloc.clear();
        let (texture, view, sampler) = create_atlas_texture(device);
        self.texture = texture;
        self.view = view;
        self.sampler = sampler;
    }

    /// Get (rasterizing if needed) the atlas entry for `ch`. Returns `None` for
    /// whitespace / empty-coverage glyphs.
    pub fn glyph(&mut self, queue: &wgpu::Queue, ch: char) -> Option<GlyphInfo> {
        if let Some(info) = self.map.get(&ch) {
            return *info;
        }
        let info = self.rasterize(queue, ch);
        self.map.insert(ch, info);
        info
    }

    fn select_font(&self, ch: char) -> &Font {
        if font_has_glyph(&self.font, ch) {
            return &self.font;
        }
        if is_nerd_glyph(ch) && font_has_glyph(&self.symbols, ch) {
            return &self.symbols;
        }
        &self.font
    }

    fn rasterize(&mut self, queue: &wgpu::Queue, ch: char) -> Option<GlyphInfo> {
        let px = self.size_px * self.scale;
        let font = self.select_font(ch);
        let (metrics, bitmap) = font.rasterize(ch, px);
        if metrics.width == 0 || metrics.height == 0 || bitmap.iter().all(|b| *b == 0) {
            return None;
        }
        let w = metrics.width as i32;
        let h = metrics.height as i32;
        let alloc = self
            .alloc
            .allocate(size2(w + PADDING * 2, h + PADDING * 2))?;
        let x = (alloc.rectangle.min.x + PADDING) as u32;
        let y = (alloc.rectangle.min.y + PADDING) as u32;

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &bitmap,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(metrics.width as u32),
                rows_per_image: Some(metrics.height as u32),
            },
            wgpu::Extent3d {
                width: metrics.width as u32,
                height: metrics.height as u32,
                depth_or_array_layers: 1,
            },
        );

        let atlas = ATLAS_SIZE as f32;
        let info = GlyphInfo {
            uv_min: [x as f32 / atlas, y as f32 / atlas],
            uv_max: [(x + metrics.width as u32) as f32 / atlas, (y + metrics.height as u32) as f32 / atlas],
            left: metrics.xmin as f32 / self.scale,
            top: self.ascent - (metrics.ymin as f32 + metrics.height as f32) / self.scale,
            width: metrics.width as f32 / self.scale,
            height: metrics.height as f32 / self.scale,
        };
        Some(info)
    }
}

fn create_atlas_texture(
    device: &wgpu::Device,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("glyph-atlas"),
        size: wgpu::Extent3d {
            width: ATLAS_SIZE,
            height: ATLAS_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("glyph-sampler"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    (texture, view, sampler)
}

fn font_has_glyph(font: &Font, ch: char) -> bool {
    font.lookup_glyph_index(ch) != 0
}

fn load_symbols_font() -> Result<Font> {
    Font::from_bytes(
        SYMBOLS_NERD_FONT,
        FontSettings {
            ..FontSettings::default()
        },
    )
    .map_err(|e| anyhow!("failed to parse Symbols Nerd Font Mono: {e:?}"))
}

fn load_font(family: Option<&str>) -> Result<Font> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();

    let query = fontdb::Query {
        families: &[match family {
            Some(name) => fontdb::Family::Name(name),
            None => fontdb::Family::Monospace,
        }],
        weight: fontdb::Weight::NORMAL,
        stretch: fontdb::Stretch::Normal,
        style: fontdb::Style::Normal,
    };

    let id = db
        .query(&query)
        .or_else(|| {
            db.query(&fontdb::Query {
                families: &[fontdb::Family::Monospace],
                ..query
            })
        })
        .ok_or_else(|| anyhow!("no monospace font found on this system"))?;

    let font = db
        .with_face_data(id, |data, index| {
            Font::from_bytes(
                data,
                FontSettings {
                    collection_index: index,
                    ..FontSettings::default()
                },
            )
            .ok()
        })
        .flatten()
        .ok_or_else(|| anyhow!("failed to parse selected font face"))?;
    Ok(font)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_font_loads_and_has_nerd_icons() {
        let symbols = load_symbols_font().expect("symbols font should parse");
        assert!(font_has_glyph(&symbols, '\u{E0B0}')); // powerline separator
        assert!(font_has_glyph(&symbols, '\u{E7A8}')); // devicon
    }

    #[test]
    fn symbols_font_rasterizes_powerline_glyph() {
        let symbols = load_symbols_font().expect("symbols font should parse");
        let (metrics, bitmap) = symbols.rasterize('\u{E0B0}', 16.0);
        assert!(metrics.width > 0 && metrics.height > 0);
        assert!(bitmap.iter().any(|b| *b > 0));
    }

    #[test]
    fn primary_font_falls_back_to_symbols_for_missing_nerd_glyph() {
        let primary = load_font(None).expect("system monospace should exist");
        let symbols = load_symbols_font().expect("symbols font should parse");
        let ch = '\u{E0B0}';

        if font_has_glyph(&primary, ch) {
            // Default monospace is already Nerd-patched on this system.
            return;
        }

        assert!(font_has_glyph(&symbols, ch));
        assert!(is_nerd_glyph(ch));

        let font = if font_has_glyph(&primary, ch) {
            &primary
        } else if is_nerd_glyph(ch) && font_has_glyph(&symbols, ch) {
            &symbols
        } else {
            &primary
        };
        assert!(std::ptr::eq(font, &symbols));

        let (metrics, bitmap) = font.rasterize(ch, 16.0);
        assert!(metrics.width > 0 && metrics.height > 0);
        assert!(bitmap.iter().any(|b| *b > 0));
    }
}
