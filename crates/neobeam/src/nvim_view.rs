use imgui::{Image, TextureId, Ui, WindowFlags};

use crate::layout::Rect;

pub struct NvimView {
    pub texture_id: TextureId,
}

impl NvimView {
    pub fn new(texture_id: TextureId) -> Self {
        Self { texture_id }
    }

    pub fn draw(&self, ui: &Ui, rect: Rect) {
        let flags = WindowFlags::NO_TITLE_BAR
            | WindowFlags::NO_RESIZE
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_SCROLLBAR
            | WindowFlags::NO_SCROLL_WITH_MOUSE
            | WindowFlags::NO_COLLAPSE
            | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
            | WindowFlags::NO_DECORATION
            | WindowFlags::NO_BACKGROUND
            | WindowFlags::NO_INPUTS;  // prevents ImGui claiming mouse/keyboard
        ui.window("##nvim_view")
            .position([rect.x, rect.y], imgui::Condition::Always)
            .size([rect.w, rect.h], imgui::Condition::Always)
            .flags(flags)
            .build(|| {
                Image::new(self.texture_id, [rect.w, rect.h]).build(ui);
            });
    }
}
