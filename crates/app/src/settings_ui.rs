//! In-app settings panel built with Dear ImGui.

use editor_surface::list_monospace_fonts;
use imgui::{Condition, StyleColor, TreeNodeFlags, Ui, WindowFlags};

use crate::settings::Settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Settings,
}

pub struct SettingsUi {
    pub open: bool,
    pub draft: Settings,
    saved: Settings,
    fonts: Vec<String>,
    page: Page,
}

impl SettingsUi {
    pub fn new(settings: &Settings) -> Self {
        SettingsUi {
            open: false,
            draft: settings.clone(),
            saved: settings.clone(),
            fonts: list_monospace_fonts(),
            page: Page::Settings,
        }
    }

    pub fn toggle(&mut self, settings: &Settings) {
        if self.open {
            self.close(false);
        } else {
            self.saved = settings.clone();
            self.draft = settings.clone();
            self.open = true;
        }
    }

    pub fn close(&mut self, revert: bool) -> bool {
        if !self.open {
            return false;
        }
        self.open = false;
        revert
    }

    pub fn draw(&mut self, ui: &Ui) -> SettingsAction {
        if !self.open {
            return SettingsAction::None;
        }

        let display = ui.io().display_size;
        let mut action = SettingsAction::None;
        let mut changed = false;

        ui.window("Settings")
            .size([560.0, 720.0], Condition::Always)
            .position(
                [display[0] * 0.5, display[1] * 0.5],
                Condition::Always,
            )
            .position_pivot([0.5, 0.5])
            .flags(WindowFlags::NO_COLLAPSE)
            .build(|| {
                if ui.button("Settings") {
                    self.page = Page::Settings;
                }
                ui.same_line();
                ui.text_disabled("(more pages can be added here)");
                ui.separator();

                ui.child_window("settings-scroll")
                    .size([0.0, 600.0])
                    .border(true)
                    .build(|| match self.page {
                        Page::Settings => {
                            changed |= self.draw_settings_page(ui, &mut action);
                        }
                    });

                ui.separator();
                if ui.button("Cancel") {
                    action = SettingsAction::CloseCancel;
                }
                ui.same_line();
                if ui.button("Apply") {
                    action = SettingsAction::Apply;
                }
            });

        if changed && action == SettingsAction::None {
            action = SettingsAction::Preview;
        }
        action
    }

    fn draw_settings_page(&mut self, ui: &Ui, action: &mut SettingsAction) -> bool {
        let mut changed = false;

        if ui.collapsing_header("Font", TreeNodeFlags::DEFAULT_OPEN) {
            changed |= self.draw_font_family_combo(ui);

            let mut font_size = self.draft.font_size;
            if ui.slider("Size (px)", 10.0, 32.0, &mut font_size) {
                self.draft.font_size = font_size.round();
                changed = true;
            }

            let mut line_height = self.draft.line_height;
            if ui
                .slider_config("Line height", 1.0, 2.0)
                .display_format("%.2f")
                .build(&mut line_height)
            {
                self.draft.line_height = (line_height * 100.0).round() / 100.0;
                changed = true;
            }

            if ui
                .slider_config("Mouse scroll sensitivity", 0.05, 1.0)
                .display_format("%.2f")
                .build(&mut self.draft.mouse_scroll_sensitivity)
            {
                self.draft.mouse_scroll_sensitivity =
                    (self.draft.mouse_scroll_sensitivity * 100.0).round() / 100.0;
                changed = true;
            }

            let family = self
                .draft
                .font_family
                .as_deref()
                .unwrap_or("System default");
            ui.text_wrapped(&format!("Preview family: {family}"));
        }

        if ui.collapsing_header("Animations", TreeNodeFlags::DEFAULT_OPEN) {
            if ui.checkbox("Enable animations", &mut self.draft.animations_enabled) {
                if !self.draft.animations_enabled {
                    self.draft.power_mode = false;
                }
                changed = true;
            }

            if self.draft.animations_enabled {
                if ui.checkbox("Power mode (shake + particles)", &mut self.draft.power_mode) {
                    changed = true;
                }
                if ui.checkbox("Smooth cursor blink", &mut self.draft.smooth_blink) {
                    changed = true;
                }

                if ui
                    .slider_config("Cursor animation length", 0.0, 0.35)
                    .display_format("%.2fs")
                    .build(&mut self.draft.animation_length)
                {
                    self.draft.animation_length =
                        (self.draft.animation_length * 100.0).round() / 100.0;
                    changed = true;
                }

                if ui
                    .slider_config("Cursor glow", 0.0, 2.0)
                    .display_format("%.2f")
                    .build(&mut self.draft.cursor_glow)
                {
                    self.draft.cursor_glow = (self.draft.cursor_glow * 100.0).round() / 100.0;
                    changed = true;
                }

                if ui
                    .slider_config("Scroll animation length", 0.0, 0.5)
                    .display_format("%.2fs")
                    .build(&mut self.draft.scroll_animation_length)
                {
                    self.draft.scroll_animation_length =
                        (self.draft.scroll_animation_length * 100.0).round() / 100.0;
                    changed = true;
                }

                let mut far_lines = self.draft.scroll_animation_far_lines as f32;
                if ui
                    .slider_config("Far scroll lines", 0.0, 10.0)
                    .display_format("%.0f")
                    .build(&mut far_lines)
                {
                    self.draft.scroll_animation_far_lines =
                        far_lines.round().clamp(0.0, 10.0) as u32;
                    changed = true;
                }
                ui.text_disabled("0 = snap large jumps");

                if ui.checkbox("Float fade + slide", &mut self.draft.enable_float_animation) {
                    changed = true;
                }

                if self.draft.enable_float_animation {
                    let mut speed = self.draft.float_fade_speed;
                    if ui.slider("Float fade speed", 6.0, 48.0, &mut speed) {
                        self.draft.float_fade_speed = speed.round().clamp(6.0, 48.0);
                        changed = true;
                    }
                }
            } else {
                let color = ui.push_style_color(StyleColor::Text, [0.55, 0.57, 0.62, 1.0]);
                ui.text_wrapped("Enable animations to configure motion effects.");
                color.pop();
            }
        }

        let _ = action;
        changed
    }

    fn draw_font_family_combo(&mut self, ui: &Ui) -> bool {
        let mut family_idx = match &self.draft.font_family {
            None => 0usize,
            Some(name) => self
                .fonts
                .iter()
                .position(|f| f == name)
                .map(|i| i + 1)
                .unwrap_or(0),
        };

        let preview = self
            .draft
            .font_family
            .as_deref()
            .unwrap_or("System default");

        let mut changed = false;
        if let Some(_combo) = ui.begin_combo("Family", preview) {
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
            self.draft.font_family = if family_idx == 0 {
                None
            } else {
                Some(self.fonts[family_idx - 1].clone())
            };
        }
        changed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    None,
    Preview,
    Apply,
    CloseCancel,
}

pub fn revert_draft(ui: &mut SettingsUi, settings: &Settings) {
    ui.draft = settings.clone();
}
