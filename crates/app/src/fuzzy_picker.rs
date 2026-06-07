//! Reusable Dear ImGui fuzzy-finder popup with fade-in/out animations.

use imgui::{Condition, Key, StyleVar, Ui, WindowFlags};

use crate::multiplexer::Rect;

const WINDOW_ID_SUFFIX: &str = "##fuzzy_picker";
const INPUT_ID: &str = "##fuzzy_query";
const PICKER_WIDTH_FRACTION: f32 = 0.9;
const PICKER_HEIGHT_FRACTION: f32 = 0.6;
pub(crate) const PICKER_MIN_WIDTH: f32 = 400.0;
pub(crate) const PICKER_MIN_HEIGHT: f32 = 200.0;

/// Alpha ramp speed when opening (units/sec). Reaches 1.0 in ~200 ms.
const FADE_IN_SPEED: f32 = 5.0;
/// Alpha ramp speed when closing via Escape (units/sec). Reaches 0.0 in ~250 ms.
const FADE_OUT_SPEED: f32 = 4.0;

/// Subsequence fuzzy match with bonuses for contiguous runs and word boundaries.
pub fn fuzzy_score(query: &str, target: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }

    let query_chars: Vec<char> = query.to_lowercase().chars().collect();
    let target_lower: Vec<char> = target.to_lowercase().chars().collect();

    let mut score = 0i32;
    let mut q_idx = 0usize;
    let mut prev_match: Option<usize> = None;
    let mut consecutive = 0i32;

    for (t_idx, tc) in target_lower.iter().enumerate() {
        if q_idx < query_chars.len() && *tc == query_chars[q_idx] {
            score += 10;
            if let Some(prev) = prev_match {
                if t_idx == prev + 1 {
                    consecutive += 1;
                    score += consecutive * 5;
                } else {
                    consecutive = 0;
                }
            }
            if t_idx == 0
                || target_lower
                    .get(t_idx.saturating_sub(1))
                    .is_none_or(|c| !c.is_alphanumeric())
            {
                score += 15;
            }
            prev_match = Some(t_idx);
            q_idx += 1;
        }
    }

    if q_idx == query_chars.len() {
        Some(score)
    } else {
        None
    }
}

struct ScoredItem<T: Clone> {
    label: String,
    value: T,
    score: i32,
}

#[derive(Clone, Debug)]
pub enum PickerOutcome<T> {
    None,
    Selected(T),
    Pinned {
        items: Vec<(String, T)>,
        query: String,
    },
}

pub struct FuzzyPicker<T: Clone> {
    items: Vec<(String, T)>,
    query: String,
    filtered: Vec<ScoredItem<T>>,
    selected: usize,
    /// Last selection we auto-scrolled to; `None` forces a scroll on the next draw.
    scroll_anchor: Option<usize>,
    open: bool,
    /// True while fading out after Escape (open stays true until alpha hits 0).
    closing: bool,
    /// Current window alpha (0.0 = invisible, 1.0 = fully opaque).
    alpha: f32,
    focus_input: bool,
}

pub(crate) fn picker_default_size(ui: &Ui) -> [f32; 2] {
    let display = ui.io().display_size;
    [
        display[0] * PICKER_WIDTH_FRACTION,
        display[1] * PICKER_HEIGHT_FRACTION,
    ]
}

pub(crate) fn picker_initial_position(ui: &Ui) -> [f32; 2] {
    let display = ui.io().display_size;
    let [width, _] = picker_default_size(ui);
    [
        (display[0] - width) * 0.5,
        display[1] * 0.2,
    ]
}

pub(crate) fn visible_row_count(ui: &Ui) -> usize {
    let h = ui.content_region_avail()[1];
    (h / ui.text_line_height_with_spacing()).max(1.0) as usize
}

/// Scroll a child list so `selected` stays visible. Only runs when selection changes
/// so manual mouse-wheel scrolling is not overwritten every frame.
pub(crate) fn scroll_list_to_selection(
    ui: &Ui,
    selected: usize,
    anchor: &mut Option<usize>,
    max_visible: usize,
) {
    if anchor == &Some(selected) {
        return;
    }
    let scroll_y = (selected.saturating_sub(max_visible / 2) as f32)
        * ui.text_line_height_with_spacing();
    ui.set_scroll_y(scroll_y);
    *anchor = Some(selected);
}

impl<T: Clone> FuzzyPicker<T> {
    pub fn new() -> Self {
        FuzzyPicker {
            items: Vec::new(),
            query: String::new(),
            filtered: Vec::new(),
            selected: 0,
            scroll_anchor: None,
            open: false,
            closing: false,
            alpha: 0.0,
            focus_input: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Returns true while a fade-in or fade-out animation is in progress.
    /// Use this to keep the render loop polling during transitions.
    pub fn is_animating(&self) -> bool {
        self.open && (self.alpha < 1.0 || self.closing)
    }

    pub fn open(&mut self, items: Vec<(String, T)>) {
        self.load_pinned(items, String::new());
        self.open = true;
        self.closing = false;
        self.alpha = 0.0;
        self.focus_input = true;
    }

    pub fn load_pinned(&mut self, items: Vec<(String, T)>, query: String) {
        self.items = items;
        self.query = query;
        self.selected = 0;
        self.scroll_anchor = None;
        self.open = false;
        self.closing = false;
        self.alpha = 0.0;
        self.focus_input = false;
        self.rebuild_filtered();
    }

    fn close_immediate(&mut self) {
        self.open = false;
        self.closing = false;
        self.alpha = 0.0;
        self.query.clear();
        self.filtered.clear();
        self.selected = 0;
        self.scroll_anchor = None;
        self.focus_input = false;
    }

    /// Begin an animated close (fade-out). Used when the user presses Escape.
    fn begin_close(&mut self) {
        self.closing = true;
    }

    fn rebuild_filtered(&mut self) {
        let mut scored: Vec<ScoredItem<T>> = self
            .items
            .iter()
            .filter_map(|(label, value)| {
                fuzzy_score(&self.query, label).map(|score| ScoredItem {
                    label: label.clone(),
                    value: value.clone(),
                    score,
                })
            })
            .collect();
        scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.label.cmp(&b.label)));
        self.filtered = scored;
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    /// Confirm the current selection and close immediately (no fade-out).
    fn confirm_selection(&mut self) -> Option<T> {
        let value = self.filtered.get(self.selected).map(|item| item.value.clone())?;
        self.close_immediate();
        Some(value)
    }

    /// `dt` is the frame delta time in seconds, used to advance the fade animation.
    pub fn draw(&mut self, ui: &Ui, title: &str, dt: f32) -> PickerOutcome<T> {
        if !self.open {
            return PickerOutcome::None;
        }

        // Advance fade animation.
        if self.closing {
            self.alpha = (self.alpha - dt * FADE_OUT_SPEED).max(0.0);
            if self.alpha <= 0.0 {
                self.close_immediate();
                return PickerOutcome::None;
            }
        } else {
            self.alpha = (self.alpha + dt * FADE_IN_SPEED).min(1.0);
        }

        let window_name = format!("{title}{WINDOW_ID_SUFFIX}");
        let pos = picker_initial_position(ui);
        let size = picker_default_size(ui);

        let mut outcome = PickerOutcome::None;
        // Apply fade alpha to the entire popup.
        let _alpha_token = ui.push_style_var(StyleVar::Alpha(self.alpha));
        ui.window(&window_name)
            .position(pos, Condition::FirstUseEver)
            .size(size, Condition::FirstUseEver)
            .size_constraints(
                [PICKER_MIN_WIDTH, PICKER_MIN_HEIGHT],
                [f32::MAX, f32::MAX],
            )
            .flags(WindowFlags::NO_COLLAPSE)
            .title_bar(true)
            .movable(true)
            .resizable(true)
            .build(|| {
                if self.focus_input {
                    ui.set_keyboard_focus_here();
                    self.focus_input = false;
                }

                let mut query = self.query.clone();
                if ui.input_text(INPUT_ID, &mut query).build() {
                    self.query = query;
                    self.selected = 0;
                    self.rebuild_filtered();
                } else {
                    self.query = query;
                }

                ui.same_line();
                if ui.button("Pin##pin") {
                    outcome = PickerOutcome::Pinned {
                        items: self.items.clone(),
                        query: self.query.clone(),
                    };
                    self.close_immediate();
                    return;
                }

                if ui.is_key_pressed(Key::Escape) {
                    self.begin_close();
                    return;
                }

                let go_up = ui.is_key_pressed(Key::UpArrow)
                    || (ui.io().key_ctrl && ui.is_key_pressed(Key::P));
                let go_down = ui.is_key_pressed(Key::DownArrow)
                    || (ui.io().key_ctrl && ui.is_key_pressed(Key::N));

                if go_up && self.selected > 0 {
                    self.selected -= 1;
                }
                if go_down && self.selected + 1 < self.filtered.len() {
                    self.selected += 1;
                }
                if ui.is_key_pressed(Key::Enter) && ui.io().key_ctrl {
                    outcome = PickerOutcome::Pinned {
                        items: self.items.clone(),
                        query: self.query.clone(),
                    };
                    self.close_immediate();
                    return;
                }
                if ui.is_key_pressed(Key::Enter) && !self.filtered.is_empty() {
                    if let Some(value) = self.confirm_selection() {
                        outcome = PickerOutcome::Selected(value);
                    }
                    return;
                }

                let list_size = ui.content_region_avail();
                ui.child_window("##fuzzy_list")
                    .size(list_size)
                    .border(true)
                    .build(|| {
                        if self.filtered.is_empty() {
                            ui.text_disabled("No matches");
                            return;
                        }

                        let max_visible = visible_row_count(ui);
                        scroll_list_to_selection(
                            ui,
                            self.selected,
                            &mut self.scroll_anchor,
                            max_visible,
                        );

                        for (idx, item) in self.filtered.iter().enumerate() {
                            let selected = idx == self.selected;
                            let clicked =
                                ui.selectable_config(&item.label).selected(selected).build();
                            if clicked {
                                self.selected = idx;
                                if let Some(value) = self.confirm_selection() {
                                    outcome = PickerOutcome::Selected(value);
                                }
                                return;
                            }
                        }
                    });
            });

        outcome
    }

    /// Draw as an embedded panel inside a tiled window (no popup chrome).
    pub fn draw_inline(
        &mut self,
        ui: &Ui,
        window_label: &str,
        content_rect: Rect,
    ) -> Option<T> {
        let pos = [content_rect.x, content_rect.y];
        let size = [content_rect.w, content_rect.h];
        let flags = WindowFlags::NO_TITLE_BAR
            | WindowFlags::NO_RESIZE
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_COLLAPSE
            | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
            | WindowFlags::NO_NAV_FOCUS;

        let mut confirmed = None;
        ui.window(window_label)
            .position(pos, Condition::Always)
            .size(size, Condition::Always)
            .flags(flags)
            .movable(false)
            .resizable(false)
            .build(|| {
                let mut query = self.query.clone();
                if ui.input_text(INPUT_ID, &mut query).build() {
                    self.query = query;
                    self.selected = 0;
                    self.rebuild_filtered();
                } else {
                    self.query = query;
                }

                if ui.is_key_pressed(Key::Escape) {
                    return;
                }

                let go_up = ui.is_key_pressed(Key::UpArrow)
                    || (ui.io().key_ctrl && ui.is_key_pressed(Key::P));
                let go_down = ui.is_key_pressed(Key::DownArrow)
                    || (ui.io().key_ctrl && ui.is_key_pressed(Key::N));

                if go_up && self.selected > 0 {
                    self.selected -= 1;
                }
                if go_down && self.selected + 1 < self.filtered.len() {
                    self.selected += 1;
                }
                if ui.is_key_pressed(Key::Enter) && !self.filtered.is_empty() {
                    confirmed = self.filtered.get(self.selected).map(|item| item.value.clone());
                    return;
                }

                let list_size = ui.content_region_avail();
                ui.child_window("##fuzzy_list_inline")
                    .size(list_size)
                    .border(true)
                    .build(|| {
                        if self.filtered.is_empty() {
                            ui.text_disabled("No matches");
                            return;
                        }

                        let max_visible = visible_row_count(ui);
                        scroll_list_to_selection(
                            ui,
                            self.selected,
                            &mut self.scroll_anchor,
                            max_visible,
                        );

                        for (idx, item) in self.filtered.iter().enumerate() {
                            let selected = idx == self.selected;
                            let clicked =
                                ui.selectable_config(&item.label).selected(selected).build();
                            if clicked {
                                self.selected = idx;
                                confirmed =
                                    self.filtered.get(self.selected).map(|i| i.value.clone());
                                return;
                            }
                        }
                    });
            });

        confirmed
    }
}

impl<T: Clone> Default for FuzzyPicker<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::fuzzy_score;

    #[test]
    fn empty_query_matches_all() {
        assert_eq!(fuzzy_score("", "nvim-ui-rs"), Some(0));
    }

    #[test]
    fn subsequence_match() {
        assert!(fuzzy_score("nui", "nvim-ui-rs").is_some());
    }

    #[test]
    fn non_match_returns_none() {
        assert!(fuzzy_score("zzz", "nvim-ui-rs").is_none());
    }
}
