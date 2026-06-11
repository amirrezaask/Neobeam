//! Settings page and winbar state (Dear ImGui).

use editor_surface::list_monospace_fonts;
use imgui::{Condition, StyleColor, Ui, WindowFlags};
use nvim_core::session::{NvimSession, WinbarInfo};

use crate::imgui_theme::{section_label_color, METRICS};
use crate::layout::Rect;
use crate::settings::Settings;

fn section_header(ui: &Ui, label: &str) {
    let color = ui.push_style_color(StyleColor::Text, section_label_color(ui));
    ui.text(label);
    color.pop();
    ui.separator();
}

fn section_break(ui: &Ui) {
    ui.dummy([0.0, METRICS.item_spacing[1] * 2.0]);
}

pub struct MenuBar {
    fonts: Vec<String>,
    fonts_loaded: bool,
    colorschemes: Vec<String>,
    colorschemes_loaded: bool,
    pub selected_theme: Option<String>,
    winbar: WinbarInfo,
    /// Authoritative project display string, set directly by the app when the
    /// project changes (not derived from nvim's async getcwd).
    project_display: String,
}

impl MenuBar {
    pub fn new() -> Self {
        MenuBar {
            fonts: Vec::new(),
            fonts_loaded: false,
            colorschemes: Vec::new(),
            colorschemes_loaded: false,
            selected_theme: None,
            winbar: WinbarInfo::default(),
            project_display: String::new(),
        }
    }

    pub fn set_selected_theme(&mut self, theme: Option<String>) {
        self.selected_theme = theme;
    }

    fn ensure_fonts_loaded(&mut self) {
        if !self.fonts_loaded {
            self.fonts = list_monospace_fonts();
            self.fonts_loaded = true;
        }
    }

    pub fn set_winbar(&mut self, info: WinbarInfo) {
        if self.winbar != info {
            self.winbar = info;
        }
    }

    /// Set the project display string (e.g. `~/dev/my-project`).
    pub fn set_project_display(&mut self, display: String) {
        self.project_display = display;
    }

    /// Draw a flat single-page settings UI inside `rect`.
    pub fn draw_settings_page(
        &mut self,
        ui: &Ui,
        settings: &mut Settings,
        session: &NvimSession,
        rect: Rect,
    ) -> MenuBarAction {
        let mut settings_changed = false;
        let mut theme_changed = None;

        let flags = WindowFlags::NO_TITLE_BAR
            | WindowFlags::NO_RESIZE
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_COLLAPSE
            | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
            | WindowFlags::NO_NAV_FOCUS
            | WindowFlags::NO_SCROLLBAR;

        ui.window("##settings_page")
            .position([rect.x, rect.y], Condition::Always)
            .size([rect.w, rect.h], Condition::Always)
            .flags(flags)
            .movable(false)
            .resizable(false)
            .build(|| {
                section_header(ui, "FONT");
                settings_changed |= self.draw_font_section(ui, settings);

                section_break(ui);

                section_header(ui, "THEME");
                if let Some(name) = self.draw_theme_section(ui, session) {
                    self.selected_theme = Some(name.clone());
                    theme_changed = Some(name);
                }

                section_break(ui);

                section_header(ui, "ANIMATIONS");
                settings_changed |= self.draw_animations_menu(ui, settings);
            });

        if let Some(name) = theme_changed {
            MenuBarAction::ThemeChanged(name)
        } else if settings_changed {
            MenuBarAction::SettingsChanged
        } else {
            MenuBarAction::None
        }
    }

    fn draw_font_section(&mut self, ui: &Ui, settings: &mut Settings) -> bool {
        self.ensure_fonts_loaded();
        let mut changed = false;

        let mut font_labels: Vec<&str> = Vec::with_capacity(self.fonts.len() + 1);
        font_labels.push("System default");
        for name in &self.fonts {
            font_labels.push(name);
        }

        let mut font_idx = match &settings.font_family {
            None => 0,
            Some(name) => self
                .fonts
                .iter()
                .position(|f| f == name)
                .map(|i| i + 1)
                .unwrap_or(0),
        };

        ui.set_next_item_width(-1.0);
        if ui.combo_simple_string("Font", &mut font_idx, &font_labels) {
            settings.font_family = if font_idx == 0 {
                None
            } else {
                Some(self.fonts[font_idx - 1].clone())
            };
            changed = true;
        }

        let mut font_size = settings.font_size;
        if ui
            .slider_config("Size", 10.0, 32.0)
            .display_format("%.0f px")
            .build(&mut font_size)
        {
            settings.font_size = font_size.round();
            changed = true;
        }

        let mut line_height = settings.line_height;
        if ui
            .slider_config("Line height", 1.0, 2.0)
            .display_format("%.2f")
            .build(&mut line_height)
        {
            settings.line_height = (line_height * 100.0).round() / 100.0;
            changed = true;
        }

        if ui
            .slider_config("Mouse scroll sensitivity", 0.05, 1.0)
            .display_format("%.2f")
            .build(&mut settings.mouse_scroll_sensitivity)
        {
            settings.mouse_scroll_sensitivity =
                (settings.mouse_scroll_sensitivity * 100.0).round() / 100.0;
            changed = true;
        }

        changed
    }

    fn draw_theme_section(&mut self, ui: &Ui, session: &NvimSession) -> Option<String> {
        if !self.colorschemes_loaded {
            self.colorschemes = session.fetch_colorschemes();
            self.colorschemes_loaded = true;
        }

        if self.colorschemes.is_empty() {
            ui.text_disabled("No colorschemes available");
            return None;
        }

        let mut theme_idx = self
            .selected_theme
            .as_ref()
            .and_then(|name| self.colorschemes.iter().position(|s| s == name))
            .unwrap_or(0);

        ui.set_next_item_width(-1.0);
        if ui.combo_simple_string("Theme", &mut theme_idx, &self.colorschemes) {
            let name = self.colorschemes[theme_idx].clone();
            if self.selected_theme.as_deref() != Some(name.as_str()) {
                return Some(name);
            }
        }
        None
    }

    fn draw_animations_menu(&mut self, ui: &Ui, settings: &mut Settings) -> bool {
        let mut changed = false;

        if ui.checkbox("Enable animations", &mut settings.animations_enabled) {
            if !settings.animations_enabled {
                settings.power_mode = false;
            }
            changed = true;
        }

        if settings.animations_enabled {
            if ui.checkbox("Power mode (shake + particles)", &mut settings.power_mode) {
                changed = true;
            }
            if ui.checkbox("Smooth cursor blink", &mut settings.smooth_blink) {
                changed = true;
            }

            if ui
                .slider_config("Cursor animation length", 0.0, 0.35)
                .display_format("%.2fs")
                .build(&mut settings.animation_length)
            {
                settings.animation_length =
                    (settings.animation_length * 100.0).round() / 100.0;
                changed = true;
            }

            if ui
                .slider_config("Cursor glow", 0.0, 2.0)
                .display_format("%.2f")
                .build(&mut settings.cursor_glow)
            {
                settings.cursor_glow = (settings.cursor_glow * 100.0).round() / 100.0;
                changed = true;
            }

            if ui
                .slider_config("Scroll animation length", 0.0, 0.5)
                .display_format("%.2fs")
                .build(&mut settings.scroll_animation_length)
            {
                settings.scroll_animation_length =
                    (settings.scroll_animation_length * 100.0).round() / 100.0;
                changed = true;
            }

            let mut far_lines = settings.scroll_animation_far_lines as f32;
            if ui
                .slider_config("Far scroll lines", 0.0, 10.0)
                .display_format("%.0f lines")
                .build(&mut far_lines)
            {
                settings.scroll_animation_far_lines =
                    far_lines.round().clamp(0.0, 10.0) as u32;
                changed = true;
            }
            if ui.is_item_hovered() {
                ui.tooltip_text("0 = snap immediately on large jumps");
            }

            if ui.checkbox("Float fade + slide", &mut settings.enable_float_animation) {
                changed = true;
            }

            if settings.enable_float_animation {
                let mut speed = settings.float_fade_speed;
                if ui.slider("Float fade speed", 6.0, 48.0, &mut speed) {
                    settings.float_fade_speed = speed.round().clamp(6.0, 48.0);
                    changed = true;
                }
            }
        } else {
            ui.text_disabled("Enable animations to configure motion effects.");
        }

        changed
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuBarAction {
    None,
    SettingsChanged,
    ThemeChanged(String),
}
