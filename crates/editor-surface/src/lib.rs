//! editor-surface: wgpu renderer, glyph atlas, frame builder, animation engine.

pub mod animation;
pub mod atlas;
pub mod box_glyphs;
pub mod color;
pub mod fonts;
pub mod frame;
pub mod renderer;

pub use animation::{AnimationConfig, AnimationState};
pub use atlas::GlyphAtlas;
pub use fonts::list_monospace_fonts;
pub use frame::{
    DrawLists, FloatCache, FloatCacheEntry, FrameBuilder, GlyphInstance, RectInstance,
    sync_float_cache,
};
pub use renderer::Renderer;
