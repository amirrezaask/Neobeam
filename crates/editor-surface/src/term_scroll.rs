//! Buffered spring animation for terminal viewport scrolling.

use std::collections::HashMap;

use terminal_core::{TermRow, TermViewport};

use crate::animation::AnimationConfig;
use crate::blink::TerminalBlink;
use crate::ring_buffer::RingBuffer;
use crate::spring::Spring;

pub struct TermScrollState {
    viewport: TermViewport,
    lines: RingBuffer<Option<TermRow>>,
    scroll_animation: Spring,
    cursor_spring_x: Spring,
    cursor_spring_y: Spring,
    cursor_pixel: [f32; 2],
    cursor_dest_pixel: [f32; 2],
    cursor_initialized: bool,
    blink: TerminalBlink,
}

impl TermScrollState {
    pub fn new(viewport: TermViewport) -> Self {
        let height = viewport.rows.len();
        let mut lines = RingBuffer::new(height * 2, None);
        let current: Vec<Option<TermRow>> = viewport.rows.iter().cloned().map(Some).collect();
        lines.clone_from_iter(current.iter());
        Self {
            viewport,
            lines,
            scroll_animation: Spring::new(),
            cursor_spring_x: Spring::new(),
            cursor_spring_y: Spring::new(),
            cursor_pixel: [0.0, 0.0],
            cursor_dest_pixel: [0.0, 0.0],
            cursor_initialized: false,
            blink: TerminalBlink::new(),
        }
    }

    pub fn sync(&mut self, viewport: TermViewport, cfg: &AnimationConfig) {
        let height = viewport.rows.len();
        if height == 0
            || height != self.viewport.rows.len()
            || viewport.rows.first().map(Vec::len) != self.viewport.rows.first().map(Vec::len)
        {
            *self = Self::new(viewport);
            return;
        }

        let delta = detect_scroll_delta(&self.viewport, &viewport);
        self.viewport = viewport;
        self.lines.rotate(delta);
        let current: Vec<Option<TermRow>> = self.viewport.rows.iter().cloned().map(Some).collect();
        self.lines.clone_from_iter(current.iter());

        if delta != 0 && cfg.enable_smooth_scroll {
            let max_delta = self.lines.len().saturating_sub(height) as isize;
            if delta.unsigned_abs() > max_delta as usize {
                let far = cfg.scroll_animation_far_lines.min(height as u32) as isize;
                self.scroll_animation.position = -(far * delta.signum()) as f32;
                let empty_lines = if delta > 0 {
                    -far..0
                } else {
                    height as isize..height as isize + far
                };
                for index in empty_lines {
                    self.lines[index] = None;
                }
            } else {
                self.scroll_animation.position = (self.scroll_animation.position - delta as f32)
                    .clamp(-(max_delta as f32), max_delta as f32);
            }
        } else if delta != 0 || !cfg.enable_smooth_scroll {
            self.scroll_animation.reset();
        }
    }

    pub fn animate(
        &mut self,
        cfg: &AnimationConfig,
        dt: f32,
        cell_w: f32,
        cell_h: f32,
    ) -> bool {
        let mut active = false;

        if cfg.enable_smooth_scroll {
            active |= self
                .scroll_animation
                .update(dt.min(0.05), cfg.scroll_animation_length);
        } else {
            self.scroll_animation.reset();
        }

        let (row, col) = self.viewport.cursor;
        let new_dest = [col as f32 * cell_w, row as f32 * cell_h];
        if !self.cursor_initialized {
            self.cursor_pixel = new_dest;
            self.cursor_dest_pixel = new_dest;
            self.cursor_initialized = true;
        } else if new_dest != self.cursor_dest_pixel {
            self.cursor_spring_x.position = new_dest[0] - self.cursor_pixel[0];
            self.cursor_spring_y.position = new_dest[1] - self.cursor_pixel[1];
            self.cursor_dest_pixel = new_dest;
        }

        if cfg.enable_cursor_animation {
            active |= self
                .cursor_spring_x
                .update(dt.min(0.05), cfg.position_animation_length);
            active |= self
                .cursor_spring_y
                .update(dt.min(0.05), cfg.position_animation_length);
        } else {
            self.cursor_spring_x.reset();
            self.cursor_spring_y.reset();
            self.cursor_pixel = new_dest;
        }

        if cfg.enable_cursor_animation {
            self.cursor_pixel = [
                self.cursor_dest_pixel[0] - self.cursor_spring_x.position,
                self.cursor_dest_pixel[1] - self.cursor_spring_y.position,
            ];
        }

        self.blink.update((row, col));
        active |= self.blink.blink_deadline().is_some();

        active
    }

    pub fn is_scroll_animating(&self, cfg: &AnimationConfig) -> bool {
        cfg.enable_smooth_scroll && self.scroll_animation.position.abs() > 0.01
    }

    pub fn is_animating(&self, cfg: &AnimationConfig) -> bool {
        self.is_scroll_animating(cfg)
            || cfg.enable_cursor_animation
                && (self.cursor_spring_x.position.abs() > 0.01
                    || self.cursor_spring_y.position.abs() > 0.01)
            || self.blink.blink_deadline().is_some()
    }

    pub fn cursor_pixel(&self) -> [f32; 2] {
        self.cursor_pixel
    }

    pub fn cursor_opacity(&self) -> f32 {
        self.blink.opacity()
    }

    pub fn cursor_should_render(&self) -> bool {
        self.blink.should_render()
    }

    pub fn blink_deadline(&self) -> Option<std::time::Instant> {
        self.blink.blink_deadline()
    }

    pub fn viewport(&self) -> &TermViewport {
        &self.viewport
    }

    pub fn line_at(&self, row: isize) -> Option<&TermRow> {
        let offset = self.scroll_animation.position.floor() as isize;
        self.lines[offset + row].as_ref()
    }

    pub fn scroll_offset_pixels(&self, cell_h: f32) -> f32 {
        let whole = self.scroll_animation.position.floor();
        ((whole - self.scroll_animation.position) * cell_h).round()
    }
}

pub struct TermScrollStore {
    panes: HashMap<u32, TermScrollState>,
}

impl TermScrollStore {
    pub fn new() -> Self {
        Self {
            panes: HashMap::new(),
        }
    }

    pub fn sync(&mut self, pane_id: u32, viewport: TermViewport, cfg: &AnimationConfig) {
        match self.panes.get_mut(&pane_id) {
            Some(state) => state.sync(viewport, cfg),
            None => {
                self.panes.insert(pane_id, TermScrollState::new(viewport));
            }
        }
    }

    pub fn animate_all(
        &mut self,
        cfg: &AnimationConfig,
        dt: f32,
        cell_w: f32,
        cell_h: f32,
    ) -> bool {
        self.panes
            .values_mut()
            .fold(false, |active, state| state.animate(cfg, dt, cell_w, cell_h) || active)
    }

    pub fn is_animating(&self, cfg: &AnimationConfig) -> bool {
        self.panes.values().any(|state| state.is_animating(cfg))
    }

    pub fn get(&self, pane_id: u32) -> Option<&TermScrollState> {
        self.panes.get(&pane_id)
    }

    pub fn remove(&mut self, pane_id: u32) {
        self.panes.remove(&pane_id);
    }
}

impl Default for TermScrollStore {
    fn default() -> Self {
        Self::new()
    }
}

fn hash_row(row: &TermRow) -> u64 {
    let mut h = 0u64;
    for cell in row {
        h = h.wrapping_mul(31).wrapping_add(cell.ch as u64);
        h = h.wrapping_mul(31).wrapping_add(cell.fg.r as u64);
        h = h.wrapping_mul(31).wrapping_add(cell.bg.r as u64);
        h = h.wrapping_mul(31).wrapping_add(cell.inverse as u64);
    }
    h
}

fn detect_scroll_delta(old: &TermViewport, new: &TermViewport) -> isize {
    if old.rows.len() != new.rows.len() || old.rows.is_empty() {
        return 0;
    }
    if old.rows == new.rows {
        return 0;
    }
    if old.display_offset != new.display_offset {
        return old.display_offset as isize - new.display_offset as isize;
    }

    let height = old.rows.len() as isize;
    let old_hashes: Vec<u64> = old.rows.iter().map(hash_row).collect();
    let new_hashes: Vec<u64> = new.rows.iter().map(hash_row).collect();

    let mut best = (0isize, 0usize);
    for delta in -(height - 1)..height {
        if delta == 0 {
            continue;
        }
        let start = 0.max(-delta);
        let end = height.min(height - delta);
        let mut evidence = 0;
        let mut matches = 0;
        for row in start..end {
            let new_row = &new.rows[row as usize];
            let old_row = &old.rows[(row + delta) as usize];
            if row_has_content(new_row, new.default_bg) || row_has_content(old_row, old.default_bg)
            {
                evidence += 1;
                let hash_match = new_hashes[row as usize] == old_hashes[(row + delta) as usize];
                if hash_match && new_row == old_row {
                    matches += 1;
                }
            }
        }
        if evidence >= 2
            && matches >= 2
            && matches * 4 >= evidence * 3
            && (matches > best.1 || (matches == best.1 && delta.abs() < best.0.abs()))
        {
            best = (delta, matches);
        }
    }
    best.0
}

fn row_has_content(row: &TermRow, default_bg: terminal_core::TermColor) -> bool {
    row.iter().any(|cell| {
        (cell.ch != ' ' && cell.ch != '\0')
            || cell.bg != default_bg
            || cell.inverse
            || cell.strikeout
            || cell.underline != terminal_core::Underline::None
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminal_core::{TermCell, TermColor, TermCursorShape, Underline};

    fn row(ch: char) -> TermRow {
        vec![TermCell {
            ch,
            zerowidth: Vec::new(),
            fg: TermColor { r: 1, g: 2, b: 3 },
            bg: TermColor { r: 4, g: 5, b: 6 },
            bold: false,
            italic: false,
            dim: false,
            hidden: false,
            inverse: false,
            strikeout: false,
            wide: false,
            spacer: false,
            underline: Underline::None,
            underline_color: None,
            selected: false,
        }]
    }

    fn viewport(chars: &str, offset: usize) -> TermViewport {
        TermViewport {
            rows: chars.chars().map(row).collect(),
            cursor: (0, 0),
            cursor_visible: true,
            cursor_shape: TermCursorShape::Block,
            default_fg: TermColor { r: 1, g: 2, b: 3 },
            default_bg: TermColor { r: 4, g: 5, b: 6 },
            display_offset: offset,
        }
    }

    #[test]
    fn detects_live_output_scroll_up() {
        assert_eq!(
            detect_scroll_delta(&viewport("abcd", 0), &viewport("bcde", 0)),
            1
        );
    }

    #[test]
    fn detects_history_navigation_from_display_offset() {
        assert_eq!(
            detect_scroll_delta(&viewport("defg", 0), &viewport("cdef", 1)),
            -1
        );
    }

    #[test]
    fn unchanged_scrollback_view_does_not_animate_new_output() {
        assert_eq!(
            detect_scroll_delta(&viewport("cdef", 1), &viewport("cdef", 2)),
            0
        );
    }

    #[test]
    fn unrelated_redraw_is_immediate() {
        assert_eq!(
            detect_scroll_delta(&viewport("abcd", 0), &viewport("wxyz", 0)),
            0
        );
    }

    #[test]
    fn edit_in_mostly_blank_viewport_is_not_scroll() {
        assert_eq!(
            detect_scroll_delta(&viewport("a   ", 0), &viewport("b   ", 0)),
            0
        );
    }

    #[test]
    fn detects_multiple_line_shift() {
        assert_eq!(
            detect_scroll_delta(&viewport("abcdef", 0), &viewport("cdefgh", 0)),
            2
        );
    }

    #[test]
    fn state_keeps_previous_line_while_scroll_animates() {
        let cfg = AnimationConfig::default();
        let mut state = TermScrollState::new(viewport("abcd", 0));
        state.sync(viewport("bcde", 0), &cfg);

        assert!(state.is_scroll_animating(&cfg));
        assert_eq!(state.line_at(0).unwrap()[0].ch, 'a');

        for _ in 0..120 {
            state.animate(&cfg, 1.0 / 60.0, 9.0, 18.0);
        }
        assert!(!state.is_scroll_animating(&cfg));
        assert_eq!(state.line_at(0).unwrap()[0].ch, 'b');
    }

    #[test]
    fn disabled_smooth_scroll_updates_immediately() {
        let mut cfg = AnimationConfig::default();
        cfg.enable_smooth_scroll = false;
        let mut state = TermScrollState::new(viewport("abcd", 0));
        state.sync(viewport("bcde", 0), &cfg);

        assert!(!state.is_scroll_animating(&cfg));
        assert_eq!(state.line_at(0).unwrap()[0].ch, 'b');
    }
}
