//! Sticky top menu bar (Dear ImGui): font, theme, zoom.

use editor_surface::list_monospace_fonts;
use imgui::{Condition, StyleVar, Ui, WindowFlags};
use nvim_core::session::NvimSession;

use crate::settings::Settings;

/// Pixel height of the top toolbar for a given editor font size.
pub fn menu_bar_height(font_size: f32) -> f32 {
    (font_size * 1.4 + 10.0).round().max(32.0)
}

struct ToolbarLayout {
    combo_w: f32,
    btn_w: f32,
    gap: f32,
    row_y: f32,
}

fn toolbar_layout(ui: &Ui, font_size: f32, bar_h: f32) -> ToolbarLayout {
    let gap = (font_size * 0.5).round().max(6.0);
    let frame_h = ui.frame_height();
    ToolbarLayout {
        combo_w: (font_size * 10.0).round().max(140.0),
        btn_w: (font_size * 1.8).round().max(24.0),
        gap,
        row_y: ((bar_h - frame_h) * 0.5).max(2.0),
    }
}

pub struct MenuBar {
    fonts: Vec<String>,
    colorschemes: Vec<String>,
    colorschemes_loaded: bool,
    pub selected_theme: Option<String>,
}

impl MenuBar {
    pub fn new() -> Self {
        MenuBar {
            fonts: list_monospace_fonts(),
            colorschemes: Vec::new(),
            colorschemes_loaded: false,
            selected_theme: None,
        }
    }

    pub fn init_theme(&mut self, session: &NvimSession) {
        self.selected_theme = session.current_colorscheme();
    }

    pub fn draw(&mut self, ui: &Ui, settings: &Settings, session: &NvimSession) -> MenuBarAction {
        let mut action = MenuBarAction::None;
        let display = ui.io().display_size;
        let bar_h = menu_bar_height(settings.font_size);

        ui.window("##menu_toolbar")
            .position([0.0, 0.0], Condition::Always)
            .size([display[0], bar_h], Condition::Always)
            .flags(
                WindowFlags::NO_TITLE_BAR
                    | WindowFlags::NO_RESIZE
                    | WindowFlags::NO_MOVE
                    | WindowFlags::NO_SCROLLBAR
                    | WindowFlags::NO_COLLAPSE
                    | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS,
            )
            .build(|| {
                let layout = toolbar_layout(ui, settings.font_size, bar_h);
                let _pad = ui.push_style_var(StyleVar::WindowPadding([0.0, 0.0]));
                let mut x = 8.0f32;
                let y = layout.row_y;

                ui.set_cursor_pos([x, y]);
                ui.text("Font");
                x += ui.calc_text_size("Font")[0] + layout.gap;

                ui.set_cursor_pos([x, y]);
                ui.set_next_item_width(layout.combo_w);
                if let Some(family) = self.draw_font_combo(ui, settings) {
                    action = MenuBarAction::FontChanged { family };
                }
                x += layout.combo_w + layout.gap;

                ui.set_cursor_pos([x, y]);
                ui.text("Theme");
                x += ui.calc_text_size("Theme")[0] + layout.gap;

                ui.set_cursor_pos([x, y]);
                ui.set_next_item_width(layout.combo_w);
                if let Some(name) = self.draw_theme_combo(ui, session) {
                    self.selected_theme = Some(name.clone());
                    action = MenuBarAction::ThemeChanged(name);
                }
                x += layout.combo_w + layout.gap;

                ui.set_cursor_pos([x, y]);
                ui.text("Zoom");
                x += ui.calc_text_size("Zoom")[0] + layout.gap;

                ui.set_cursor_pos([x, y]);
                if ui.button_with_size(" - ", [layout.btn_w, 0.0]) {
                    action = MenuBarAction::ZoomOut;
                }
                x += layout.btn_w + 4.0;

                ui.set_cursor_pos([x, y]);
                if ui.button_with_size(" + ", [layout.btn_w, 0.0]) {
                    action = MenuBarAction::ZoomIn;
                }
            });

        action
    }

    fn draw_font_combo(&mut self, ui: &Ui, settings: &Settings) -> Option<Option<String>> {
        let mut family_idx = match &settings.font_family {
            None => 0usize,
            Some(name) => self
                .fonts
                .iter()
                .position(|f| f == name)
                .map(|i| i + 1)
                .unwrap_or(0),
        };

        let preview = settings
            .font_family
            .as_deref()
            .unwrap_or("System default");

        let mut changed = false;
        if let Some(_combo) = ui.begin_combo("##font_combo", preview) {
            if ui
                .selectable_config("System default")
                .selected(family_idx == 0)
                .build()
            {
                family_idx = 0;
                changed = true;
            }
            for (i, name) in self.fonts.iter().enumerate() {
                let idx = i + 1;
                if ui.selectable_config(name).selected(family_idx == idx).build() {
                    family_idx = idx;
                    changed = true;
                }
            }
        }

        if changed {
            Some(if family_idx == 0 {
                None
            } else {
                Some(self.fonts[family_idx - 1].clone())
            })
        } else {
            None
        }
    }

    fn draw_theme_combo(&mut self, ui: &Ui, session: &NvimSession) -> Option<String> {
        let preview = self
            .selected_theme
            .as_deref()
            .unwrap_or("Default");

        let mut selected: Option<String> = None;
        if let Some(_combo) = ui.begin_combo("##theme_combo", preview) {
            if !self.colorschemes_loaded {
                self.colorschemes = session.fetch_colorschemes();
                self.colorschemes_loaded = true;
            }
            for name in &self.colorschemes {
                let is_selected = self.selected_theme.as_deref() == Some(name.as_str());
                if ui.selectable_config(name).selected(is_selected).build() {
                    selected = Some(name.clone());
                }
            }
        }
        selected
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuBarAction {
    None,
    FontChanged { family: Option<String> },
    ThemeChanged(String),
    ZoomIn,
    ZoomOut,
}
