//! Chrome layout: reserve pixel space for external UI bars.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BarPlacement {
    #[default]
    Bottom,
    Top,
    Both,
}

impl BarPlacement {
    pub fn shows_bottom(self) -> bool {
        matches!(self, BarPlacement::Bottom | BarPlacement::Both)
    }

    pub fn shows_top(self) -> bool {
        matches!(self, BarPlacement::Top | BarPlacement::Both)
    }
}

#[derive(Debug, Clone)]
pub struct ChromeLayoutConfig {
    /// Width of the vertical activity bar on the left (VS Code-style).
    pub activity_bar_width: f32,
    pub custom_cmdline_enabled: bool,
    pub custom_bar_enabled: bool,
    pub bar_placement: BarPlacement,
    pub bar_height: f32,
    pub cmdline_height: f32,
    pub message_line_height: f32,
    pub chrome_font_size: f32,
    pub animations_enabled: bool,
}

impl Default for ChromeLayoutConfig {
    fn default() -> Self {
        ChromeLayoutConfig {
            activity_bar_width: 0.0,
            custom_cmdline_enabled: false,
            custom_bar_enabled: false,
            bar_placement: BarPlacement::Bottom,
            bar_height: 22.0,
            cmdline_height: 32.0,
            message_line_height: 24.0,
            chrome_font_size: 11.0,
            animations_enabled: true,
        }
    }
}

impl ChromeLayoutConfig {
    pub fn renders_anything(&self) -> bool {
        self.custom_cmdline_enabled || self.custom_bar_enabled
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ChromeLayout {
    pub window_w: f32,
    pub window_h: f32,
    pub editor_y: f32,
    pub editor_h: f32,
    /// Editor region as `[x, y, w, h]` in logical pixels.
    pub editor_rect: [f32; 4],
    pub activity_bar_width: f32,
    pub top_stack_h: f32,
    pub statusbar_y: f32,
    pub cmdline_y: f32,
    pub bottom_stack_h: f32,
}

impl ChromeLayout {
    pub fn compute(window_w: f32, window_h: f32, cfg: &ChromeLayoutConfig) -> Self {
        let bar_bottom = cfg.custom_bar_enabled && cfg.bar_placement.shows_bottom();
        let cmdline = cfg.custom_cmdline_enabled;

        let bar_h = if bar_bottom { cfg.bar_height } else { 0.0 };
        let cmdline_h = if cmdline { cfg.cmdline_height } else { 0.0 };
        let msg_h = 0.0;
        let bottom_stack = bar_h + cmdline_h + msg_h;
        let left_stack = cfg.activity_bar_width;
        let editor_w = (window_w - left_stack).max(1.0);
        let editor_h = (window_h - bottom_stack).max(1.0);

        ChromeLayout {
            window_w,
            window_h,
            editor_y: 0.0,
            editor_h,
            editor_rect: [left_stack, 0.0, editor_w, editor_h],
            activity_bar_width: left_stack,
            top_stack_h: 0.0,
            statusbar_y: window_h - bar_h - cmdline_h - msg_h,
            cmdline_y: window_h - cmdline_h - msg_h,
            bottom_stack_h: bottom_stack,
        }
    }

    pub fn editor_grid_size(&self, cell_w: f32, cell_h: f32) -> (u32, u32) {
        let cols = ((self.editor_rect[2] / cell_w).floor() as u32).max(1);
        let rows = ((self.editor_h / cell_h).floor() as u32).max(1);
        (cols, rows)
    }
}
