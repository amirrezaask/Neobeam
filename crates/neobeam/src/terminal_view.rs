use imgui::{Image, TextureId, Ui, WindowFlags};

use crate::layout::Rect;

pub struct TerminalView {
    pub texture_id: TextureId,
    pub tex_w: u32,
    pub tex_h: u32,
}

impl TerminalView {
    pub fn new(texture_id: TextureId, tex_w: u32, tex_h: u32) -> Self {
        Self {
            texture_id,
            tex_w,
            tex_h,
        }
    }

    pub fn draw(
        &self,
        ui: &Ui,
        rect: Rect,
        scale: f32,
        alpha: f32,
        focused: bool,
        title: Option<&str>,
        exited: bool,
    ) {
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
        let uv1 = [
            (rect.w * scale) / self.tex_w as f32,
            (rect.h * scale) / self.tex_h as f32,
        ];
        let _pad = ui.push_style_var(imgui::StyleVar::WindowPadding([0.0, 0.0]));
        let _border = ui.push_style_var(imgui::StyleVar::WindowBorderSize(0.0));
        let _spacing = ui.push_style_var(imgui::StyleVar::ItemSpacing([0.0, 0.0]));
        let id = format!("##term_view_{}_{}", rect.x as i32, rect.y as i32);
        ui.window(&id)
            .position([rect.x, rect.y], imgui::Condition::Always)
            .size([rect.w, rect.h], imgui::Condition::Always)
            .flags(flags)
            .build(|| {
                Image::new(self.texture_id, [rect.w, rect.h])
                    .uv1(uv1)
                    .tint_col([1.0, 1.0, 1.0, alpha])
                    .build(ui);
            });

        if focused || exited {
            let label = if exited {
                "process exited"
            } else {
                title
                    .filter(|title| !title.is_empty())
                    .unwrap_or("terminal")
            };
            let draw = ui.get_foreground_draw_list();
            let text_size = ui.calc_text_size(label);
            let min = [rect.x + 8.0, rect.y + 7.0];
            let max = [min[0] + text_size[0] + 14.0, min[1] + text_size[1] + 8.0];
            draw.add_rect(min, max, [0.04, 0.05, 0.08, 0.82])
                .filled(true)
                .rounding(6.0)
                .build();
            draw.add_text(
                [min[0] + 7.0, min[1] + 4.0],
                if exited {
                    [0.95, 0.35, 0.30, alpha]
                } else {
                    [0.55, 0.82, 0.80, alpha]
                },
                label,
            );
        }
    }
}
