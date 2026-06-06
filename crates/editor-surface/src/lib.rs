//! editor-surface: wgpu renderer, glyph atlas, frame builder, animation engine.

pub mod animation;
pub mod atlas;
pub mod box_glyphs;
pub mod color;
pub mod frame;
pub mod renderer;

pub use animation::{AnimationConfig, AnimationState};
pub use atlas::GlyphAtlas;
pub use frame::{DrawLists, FrameBuilder};
pub use renderer::Renderer;
