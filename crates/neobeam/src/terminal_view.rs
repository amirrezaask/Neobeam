use imgui::{Image, TextureId, Ui, WindowFlags};

use crate::layout::Rect;

pub struct TerminalView {
    pub texture_id: TextureId,
}

impl TerminalView {
    pub fn new(texture_id: TextureId) -> Self {
        Self { texture_id }
    }

    pub fn draw(
        &self,
        ui: &Ui,
        rect: Rect,
        alpha: f32,
        focused: bool,
        title: Option<&str>,
        exited: bool,
        pane_id: u32,
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
        let _pad = ui.push_style_var(imgui::StyleVar::WindowPadding([0.0, 0.0]));
        let _border = ui.push_style_var(imgui::StyleVar::WindowBorderSize(0.0));
        let _spacing = ui.push_style_var(imgui::StyleVar::ItemSpacing([0.0, 0.0]));
        let mut id_buf = [0u8; 24];
        let id_str = format_id(&mut id_buf, pane_id);
        ui.window(id_str)
            .position([rect.x, rect.y], imgui::Condition::Always)
            .size([rect.w, rect.h], imgui::Condition::Always)
            .flags(flags)
            .build(|| {
                // Texture is sized exactly to this pane — sample the full texture.
                Image::new(self.texture_id, [rect.w, rect.h])
                    .uv1([1.0, 1.0])
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

fn format_id<'a>(buf: &'a mut [u8; 24], pane_id: u32) -> &'a str {
    use std::io::Write as _;
    let mut c = std::io::Cursor::new(buf.as_mut());
    let _ = write!(c, "##term_{}\0", pane_id);
    let pos = c.position() as usize;
    std::str::from_utf8(&c.into_inner()[..pos.saturating_sub(1)]).unwrap_or("##term")
}
