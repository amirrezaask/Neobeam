//! Fixed layout: optional left sidebar + main content area.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn from_array([x, y, w, h]: [f32; 4]) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, cx: f32, cy: f32) -> bool {
        cx >= self.x && cx < self.x + self.w && cy >= self.y && cy < self.y + self.h
    }
}

pub struct SimpleLayout {
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
    pub resizing: bool,
}

impl SimpleLayout {
    pub const DEFAULT_SIDEBAR_WIDTH: f32 = 300.0;
    pub const MIN_SIDEBAR_WIDTH: f32 = 200.0;
    pub const MAX_SIDEBAR_WIDTH: f32 = 600.0;
    pub const RESIZE_HANDLE_WIDTH: f32 = 4.0;
    pub const MIN_MAIN_WIDTH: f32 = 200.0;

    pub fn new() -> Self {
        Self {
            sidebar_visible: false,
            sidebar_width: Self::DEFAULT_SIDEBAR_WIDTH,
            resizing: false,
        }
    }

    pub fn show_sidebar(&mut self) {
        self.sidebar_visible = true;
    }

    pub fn hide_sidebar(&mut self) {
        self.sidebar_visible = false;
    }

    fn clamped_sidebar_width(&self, area: Rect) -> f32 {
        let max_w = (area.w - Self::MIN_MAIN_WIDTH - Self::RESIZE_HANDLE_WIDTH).max(0.0);
        self.sidebar_width.clamp(
            Self::MIN_SIDEBAR_WIDTH.min(max_w),
            max_w.max(Self::MIN_SIDEBAR_WIDTH).min(Self::MAX_SIDEBAR_WIDTH),
        )
    }

    /// Returns `(main_rect, sidebar_rect)`.
    pub fn compute(&self, area: Rect) -> (Rect, Option<Rect>) {
        if !self.sidebar_visible {
            return (area, None);
        }
        let sidebar_w = self.clamped_sidebar_width(area);
        let sidebar = Rect {
            x: area.x,
            y: area.y,
            w: sidebar_w,
            h: area.h,
        };
        let main = Rect {
            x: area.x + sidebar_w + Self::RESIZE_HANDLE_WIDTH,
            y: area.y,
            w: area.w - sidebar_w - Self::RESIZE_HANDLE_WIDTH,
            h: area.h,
        };
        (main, Some(sidebar))
    }

    pub fn resize_handle_rect(&self, area: Rect) -> Option<Rect> {
        if !self.sidebar_visible {
            return None;
        }
        let sidebar_w = self.clamped_sidebar_width(area);
        Some(Rect {
            x: area.x + sidebar_w,
            y: area.y,
            w: Self::RESIZE_HANDLE_WIDTH,
            h: area.h,
        })
    }

    pub fn update_resize(&mut self, cursor_x: f32, area: Rect) {
        let width = cursor_x - area.x;
        let max_w = (area.w - Self::MIN_MAIN_WIDTH - Self::RESIZE_HANDLE_WIDTH).max(0.0);
        self.sidebar_width = width.clamp(
            Self::MIN_SIDEBAR_WIDTH.min(max_w),
            max_w.max(Self::MIN_SIDEBAR_WIDTH).min(Self::MAX_SIDEBAR_WIDTH),
        );
    }
}
