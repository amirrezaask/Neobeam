//! Reusable Dear ImGui fuzzy-finder popup.

use imgui::{Condition, Key, Ui, WindowFlags};

const WINDOW_ID: &str = "##fuzzy_picker";
const INPUT_ID: &str = "##fuzzy_query";
const LIST_HEIGHT: f32 = 280.0;
const WINDOW_WIDTH: f32 = 520.0;
const MAX_VISIBLE: usize = 12;

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

pub struct FuzzyPicker<T: Clone> {
    items: Vec<(String, T)>,
    query: String,
    filtered: Vec<ScoredItem<T>>,
    selected: usize,
    open: bool,
    focus_input: bool,
}

impl<T: Clone> FuzzyPicker<T> {
    pub fn new() -> Self {
        FuzzyPicker {
            items: Vec::new(),
            query: String::new(),
            filtered: Vec::new(),
            selected: 0,
            open: false,
            focus_input: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, items: Vec<(String, T)>) {
        self.items = items;
        self.query.clear();
        self.selected = 0;
        self.open = true;
        self.focus_input = true;
        self.rebuild_filtered();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.filtered.clear();
        self.selected = 0;
        self.focus_input = false;
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

    fn confirm_selection(&mut self) -> Option<T> {
        let value = self.filtered.get(self.selected).map(|item| item.value.clone())?;
        self.close();
        Some(value)
    }

    pub fn draw(&mut self, ui: &Ui, title: &str) -> Option<T> {
        if !self.open {
            return None;
        }

        let display = ui.io().display_size;
        let pos = [
            (display[0] - WINDOW_WIDTH) * 0.5,
            display[1] * 0.2,
        ];

        let flags = WindowFlags::NO_TITLE_BAR
            | WindowFlags::NO_RESIZE
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_COLLAPSE
            | WindowFlags::ALWAYS_AUTO_RESIZE;

        let mut confirmed = None;
        ui.window(WINDOW_ID)
            .position(pos, Condition::Always)
            .size([WINDOW_WIDTH, 0.0], Condition::Always)
            .flags(flags)
            .movable(false)
            .resizable(false)
            .build(|| {
                ui.text(title);
                ui.separator();

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

                if ui.is_key_pressed(Key::Escape) {
                    self.close();
                    return;
                }

                if ui.is_key_pressed(Key::UpArrow) && self.selected > 0 {
                    self.selected -= 1;
                }
                if ui.is_key_pressed(Key::DownArrow) && self.selected + 1 < self.filtered.len() {
                    self.selected += 1;
                }
                if ui.is_key_pressed(Key::Enter) && !self.filtered.is_empty() {
                    confirmed = self.confirm_selection();
                    return;
                }

                ui.child_window("##fuzzy_list")
                    .size([WINDOW_WIDTH - 16.0, LIST_HEIGHT])
                    .border(true)
                    .build(|| {
                        if self.filtered.is_empty() {
                            ui.text_disabled("No matches");
                            return;
                        }

                        let scroll_y = (self.selected.saturating_sub(MAX_VISIBLE / 2) as f32)
                            * ui.text_line_height_with_spacing();
                        ui.set_scroll_y(scroll_y);

                        for (idx, item) in self.filtered.iter().enumerate() {
                            let selected = idx == self.selected;
                            let clicked = ui.selectable_config(&item.label).selected(selected).build();
                            if clicked {
                                self.selected = idx;
                                confirmed = self.confirm_selection();
                                return;
                            }
                        }
                    });
            });

        if !self.open {
            return None;
        }
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
