use imgui::{Image, TextureId, Ui, WindowFlags};

use crate::layout::Rect;

pub struct NvimView {
    pub texture_id: TextureId,
    /// Physical pixel size of the offscreen texture.
    pub tex_w: u32,
    pub tex_h: u32,
}

impl NvimView {
    pub fn new(texture_id: TextureId, tex_w: u32, tex_h: u32) -> Self {
        Self { texture_id, tex_w, tex_h }
    }

    pub fn draw(&self, ui: &Ui, rect: Rect, scale: f32) {
        let flags = WindowFlags::NO_TITLE_BAR
            | WindowFlags::NO_RESIZE
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_SCROLLBAR
            | WindowFlags::NO_SCROLL_WITH_MOUSE
            | WindowFlags::NO_COLLAPSE
            | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
            | WindowFlags::NO_DECORATION
            | WindowFlags::NO_BACKGROUND
            | WindowFlags::NO_INPUTS;
        // UV maps logical rect size → fraction of full-window texture.
        let uv1 = [
            (rect.w * scale) / self.tex_w as f32,
            (rect.h * scale) / self.tex_h as f32,
        ];
        ui.window("##nvim_view")
            .position([rect.x, rect.y], imgui::Condition::Always)
            .size([rect.w, rect.h], imgui::Condition::Always)
            .flags(flags)
            .build(|| {
                Image::new(self.texture_id, [rect.w, rect.h])
                    .uv1(uv1)
                    .build(ui);
            });
    }
}
