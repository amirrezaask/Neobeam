//! Layout helpers.

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

/// Trivial single-pane layout: the editor always fills the entire editor area.
pub struct SimpleLayout;

impl SimpleLayout {
    pub fn new() -> Self {
        Self
    }

    /// Returns the main content rect (the full editor area).
    pub fn compute(&self, area: Rect) -> Rect {
        area
    }
}
