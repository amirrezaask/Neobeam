//! Persistent tiled panels for pinned file-list and grep-result pickers.

use std::path::PathBuf;

use imgui::{Condition, Key, Ui, WindowFlags};

use crate::fuzzy_picker::{scroll_list_to_selection, visible_row_count, FuzzyPicker};
use crate::grep_picker::{format_match_label, GrepMatch};
use crate::multiplexer::{Rect, WinId};

pub struct FileListPanel {
    picker: FuzzyPicker<PathBuf>,
}

impl FileListPanel {
    pub fn new(items: Vec<(String, PathBuf)>, initial_query: String) -> Self {
        let mut picker = FuzzyPicker::new();
        picker.load_pinned(items, initial_query);
        Self { picker }
    }

    pub fn draw(
        &mut self,
        ui: &Ui,
        win_id: WinId,
        content_rect: Rect,
        _dt: f32,
    ) -> Option<PathBuf> {
        let label = format!("##file_list_{}", win_id.0);
        self.picker.draw_inline(ui, &label, content_rect)
    }
}

pub struct GrepResultsPanel {
    results: Vec<GrepMatch>,
    filtered: Vec<usize>,
    query: String,
    selected: usize,
    scroll_anchor: Option<usize>,
    project_root: PathBuf,
}

impl GrepResultsPanel {
    pub fn new(results: Vec<GrepMatch>, initial_query: String, project_root: PathBuf) -> Self {
        let mut panel = Self {
            results,
            filtered: Vec::new(),
            query: initial_query,
            selected: 0,
            scroll_anchor: None,
            project_root,
        };
        panel.rebuild_filtered();
        panel
    }

    pub fn draw(&mut self, ui: &Ui, win_id: WinId, content_rect: Rect) -> Option<GrepMatch> {
        let pos = [content_rect.x, content_rect.y];
        let size = [content_rect.w, content_rect.h];
        let flags = WindowFlags::NO_TITLE_BAR
            | WindowFlags::NO_RESIZE
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_COLLAPSE
            | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
            | WindowFlags::NO_NAV_FOCUS;

        let window_id = format!("##grep_results_{}", win_id.0);
        let mut confirmed = None;

        ui.window(&window_id)
            .position(pos, Condition::Always)
            .size(size, Condition::Always)
            .flags(flags)
            .movable(false)
            .resizable(false)
            .build(|| {
                let mut query = self.query.clone();
                if ui.input_text("##grep_panel_query", &mut query).build() {
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
                    confirmed = self
                        .filtered
                        .get(self.selected)
                        .and_then(|idx| self.results.get(*idx).cloned());
                    return;
                }

                let list_size = ui.content_region_avail();
                ui.child_window("##grep_results_list")
                    .size(list_size)
                    .border(true)
                    .build(|| {
                        if self.results.is_empty() {
                            ui.text_disabled("No results");
                            return;
                        }
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

                        for (idx, &result_idx) in self.filtered.iter().enumerate() {
                            let Some(item) = self.results.get(result_idx) else {
                                continue;
                            };
                            let label = format_match_label(&self.project_root, item);
                            let selected = idx == self.selected;
                            let clicked =
                                ui.selectable_config(&label).selected(selected).build();
                            if clicked {
                                self.selected = idx;
                                confirmed = Some(item.clone());
                                return;
                            }
                        }
                    });
            });

        confirmed
    }

    fn rebuild_filtered(&mut self) {
        if self.query.is_empty() {
            self.filtered = (0..self.results.len()).collect();
        } else {
            let q = self.query.to_lowercase();
            self.filtered = self
                .results
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    let label = format_match_label(&self.project_root, item).to_lowercase();
                    label.contains(&q)
                })
                .map(|(idx, _)| idx)
                .collect();
        }
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
        self.scroll_anchor = None;
    }
}
