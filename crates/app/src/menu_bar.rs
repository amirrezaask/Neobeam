//! Left activity bar (Dear ImGui): settings, page icons; winbar lives in the editor title bar.

use editor_surface::list_monospace_fonts;
use imgui::{Condition, StyleColor, StyleVar, Ui, WindowFlags};
use nvim_core::session::{NvimSession, WinbarInfo};

use crate::activity_icons;
use crate::app_page::AppPage;
use crate::settings::Settings;

const SETTINGS_POPUP: &str = "##settings_popup";

/// Pixel width of the vertical activity bar for a given editor font size.
pub fn activity_bar_width(font_size: f32) -> f32 {
    (font_size * 2.5 + 8.0).round().max(40.0)
}

fn activity_bar_bg_from_theme(theme_bg: [f32; 4]) -> [f32; 4] {
    [
        theme_bg[0] * 0.88,
        theme_bg[1] * 0.88,
        theme_bg[2] * 0.88,
        1.0,
    ]
}

fn blend_rgba(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    let u = 1.0 - t;
    [
        a[0] * u + b[0] * t,
        a[1] * u + b[1] * t,
        a[2] * u + b[2] * t,
        a[3] * u + b[3] * t,
    ]
}

/// Full-width activity bar item: bar-colored by default, highlight on hover/active.
fn draw_activity_item(
    ui: &Ui,
    id: &str,
    icon: &str,
    width: f32,
    height: f32,
    selected: bool,
) -> bool {
    let bar_bg = ui.style_color(StyleColor::WindowBg);
    let hover_bg = blend_rgba(bar_bg, ui.style_color(StyleColor::Text), 0.08);
    let active_bg = blend_rgba(bar_bg, ui.style_color(StyleColor::CheckMark), 0.14);

    let pos = ui.cursor_screen_pos();
    let clicked = ui.invisible_button(id, [width, height]);
    let hovered = ui.is_item_hovered();

    let bg = if selected {
        active_bg
    } else if hovered {
        hover_bg
    } else {
        bar_bg
    };

    let draw = ui.get_window_draw_list();
    let min = pos;
    let max = [pos[0] + width, pos[1] + height];
    draw.add_rect(min, max, bg).filled(true).rounding(0.0).build();

    if selected {
        let accent = ui.style_color(StyleColor::CheckMark);
        draw.add_rect([min[0], min[1]], [min[0] + 2.0, max[1]], accent)
            .filled(true)
            .rounding(0.0)
            .build();
    }

    let icon_size = ui.calc_text_size(icon);
    let text_x = min[0] + (width - icon_size[0]) * 0.5;
    let text_y = min[1] + (height - icon_size[1]) * 0.5;
    draw.add_text([text_x, text_y], ui.style_color(StyleColor::Text), icon);

    clicked
}

pub struct MenuBar {
    fonts: Vec<String>,
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
            fonts: list_monospace_fonts(),
            colorschemes: Vec::new(),
            colorschemes_loaded: false,
            selected_theme: None,
            winbar: WinbarInfo::default(),
            project_display: String::new(),
        }
    }

    pub fn init_theme(&mut self, session: &NvimSession) {
        self.selected_theme = session.current_colorscheme();
    }

    pub fn refresh_winbar(&mut self, session: &NvimSession) {
        self.winbar = session.fetch_winbar_info();
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

    pub fn winbar_file_name(&self) -> &str {
        &self.winbar.file_name
    }

    pub fn project_display(&self) -> &str {
        &self.project_display
    }

    pub fn draw(
        &mut self,
        ui: &Ui,
        settings: &mut Settings,
        session: &NvimSession,
        focused_page: AppPage,
        window_h: f32,
    ) -> MenuBarAction {
        let mut settings_changed = false;
        let mut theme_changed = None;
        let mut page_changed = None;

        let width = activity_bar_width(settings.font_size);
        let item_h = width;

        let flags = WindowFlags::NO_TITLE_BAR
            | WindowFlags::NO_RESIZE
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_COLLAPSE
            | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
            | WindowFlags::NO_NAV_FOCUS;

        let bar_bg = activity_bar_bg_from_theme(ui.style_color(StyleColor::WindowBg));
        let _pad = ui.push_style_var(StyleVar::WindowPadding([0.0, 0.0]));
        let _space = ui.push_style_var(StyleVar::ItemSpacing([0.0, 0.0]));
        let _bg = ui.push_style_color(StyleColor::WindowBg, bar_bg);

        ui.window("##activity_bar")
            .position([0.0, 0.0], Condition::Always)
            .size([width, window_h], Condition::Always)
            .flags(flags)
            .movable(false)
            .resizable(false)
            .build(|| {
                if draw_activity_item(
                    ui,
                    "##settings",
                    activity_icons::SETTINGS,
                    width,
                    item_h,
                    false,
                ) {
                    ui.open_popup(SETTINGS_POPUP);
                }
                if let Some(_popup) = ui.begin_popup(SETTINGS_POPUP) {
                    ui.menu("Font", || {
                        settings_changed |= self.draw_font_menu(ui, settings);
                    });

                    ui.menu("Theme", || {
                        if let Some(name) = self.draw_theme_menu(ui, session) {
                            self.selected_theme = Some(name.clone());
                            theme_changed = Some(name);
                        }
                    });

                    ui.menu("Animations", || {
                        settings_changed |= self.draw_animations_menu(ui, settings);
                    });
                }

                for page in [AppPage::Editor, AppPage::GitClient] {
                    let selected = focused_page == page;
                    if draw_activity_item(ui, page.label(), page.icon(), width, item_h, selected)
                        && !selected
                    {
                        page_changed = Some(page);
                    }
                }
            });

        if let Some(page) = page_changed {
            MenuBarAction::PageChanged(page)
        } else if let Some(name) = theme_changed {
            MenuBarAction::ThemeChanged(name)
        } else if settings_changed {
            MenuBarAction::SettingsChanged
        } else {
            MenuBarAction::None
        }
    }

    fn draw_font_menu(&mut self, ui: &Ui, settings: &mut Settings) -> bool {
        let mut changed = false;

        let is_default = settings.font_family.is_none();
        if ui
            .menu_item_config("System default")
            .selected(is_default)
            .build()
            && !is_default
        {
            settings.font_family = None;
            changed = true;
        }

        for name in &self.fonts {
            let selected = settings.font_family.as_deref() == Some(name.as_str());
            if ui.menu_item_config(name).selected(selected).build() && !selected {
                settings.font_family = Some(name.clone());
                changed = true;
            }
        }

        ui.separator();

        if ui.menu_item("Increase Font Size") {
            settings.font_size = (settings.font_size + 1.0).min(32.0);
            changed = true;
        }
        if ui.menu_item("Decrease Font Size") {
            settings.font_size = (settings.font_size - 1.0).max(10.0);
            changed = true;
        }

        ui.separator();

        let mut font_size = settings.font_size;
        if ui.slider("Size (px)", 10.0, 32.0, &mut font_size) {
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

    fn draw_theme_menu(&mut self, ui: &Ui, session: &NvimSession) -> Option<String> {
        if !self.colorschemes_loaded {
            self.colorschemes = session.fetch_colorschemes();
            self.colorschemes_loaded = true;
        }

        let mut selected = None;
        for name in &self.colorschemes {
            let is_selected = self.selected_theme.as_deref() == Some(name.as_str());
            if ui.menu_item_config(name).selected(is_selected).build() && !is_selected {
                selected = Some(name.clone());
            }
        }
        selected
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
                .display_format("%.0f")
                .build(&mut far_lines)
            {
                settings.scroll_animation_far_lines =
                    far_lines.round().clamp(0.0, 10.0) as u32;
                changed = true;
            }
            ui.text_disabled("0 = snap large jumps");

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
            let color = ui.push_style_color(StyleColor::Text, [0.55, 0.57, 0.62, 1.0]);
            ui.text_wrapped("Enable animations to configure motion effects.");
            color.pop();
        }

        changed
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuBarAction {
    None,
    SettingsChanged,
    ThemeChanged(String),
    PageChanged(AppPage),
}
