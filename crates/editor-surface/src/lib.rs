//! editor-surface: wgpu renderer, glyph atlas, frame builder, animation engine.

pub mod animation;
pub mod atlas;
pub mod blink;
pub mod box_glyphs;
pub mod chrome;
pub mod color;
pub mod cursor_vfx;
pub mod fonts;
pub mod frame;
pub mod nerd_glyphs;
pub mod renderer;
pub mod ring_buffer;
pub mod spring;
pub mod term_frame;
pub mod term_scroll;
pub mod window_render;

pub use animation::{AnimationConfig, AnimationState};
pub use atlas::GlyphAtlas;
pub use blink::ShouldRender;
pub use chrome::{ChromeLayout, ChromeLayoutConfig};
pub use cursor_vfx::{parse_vfx_modes, HighlightMode, TrailMode, VfxMode};
pub use fonts::{estimate_cell_size, list_monospace_fonts, load_editor_font, SYMBOLS_NERD_FONT};
pub use frame::{
    sync_float_cache, DrawLists, FloatCache, FloatCacheEntry, FrameBuilder, GlyphInstance,
    QuadInstance, RectInstance,
};
pub use renderer::Renderer;
pub use spring::Spring;
pub use term_scroll::{TermScrollState, TermScrollStore};
