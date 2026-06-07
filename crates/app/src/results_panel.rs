//! Sidebar for pinned file-list and grep-result pickers.

use std::path::PathBuf;

use imgui::{Condition, Key, Ui, WindowFlags};

use crate::fuzzy_picker::{scroll_list_to_selection, visible_row_count, FuzzyPicker};
use crate::grep_picker::{format_match_label, GrepMatch};
use crate::layout::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarTab {
    Files,
    Grep,
}

pub enum SidebarAction {
    None,
    OpenFile(PathBuf),
    OpenGrepMatch(GrepMatch),
}

pub struct Sidebar {
    pub visible: bool,
    pub active_tab: SidebarTab,
    file_panel: FileListPanel,
    grep_panel: GrepResultsPanel,
}

impl Sidebar {
    pub fn new() -> Self {
        Self {
            visible: false,
            active_tab: SidebarTab::Files,
            file_panel: FileListPanel::empty(),
            grep_panel: GrepResultsPanel::empty(),
        }
    }

    pub fn pin_files(&mut self, items: Vec<(String, PathBuf)>, query: String) {
        self.visible = true;
        self.active_tab = SidebarTab::Files;
        self.file_panel.reload(items, query);
    }

    pub fn pin_grep(
        &mut self,
        results: Vec<GrepMatch>,
        query: String,
        project_root: PathBuf,
    ) {
        self.visible = true;
        self.active_tab = SidebarTab::Grep;
        self.grep_panel.reload(results, query, project_root);
    }

    pub fn draw(&mut self, ui: &Ui, rect: Rect, dt: f32) -> SidebarAction {
        if !self.visible {
            return SidebarAction::None;
        }

        let pos = [rect.x, rect.y];
        let size = [rect.w, rect.h];
        let flags = WindowFlags::NO_TITLE_BAR
            | WindowFlags::NO_RESIZE
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_COLLAPSE
            | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
            | WindowFlags::NO_NAV_FOCUS;

        let mut action = SidebarAction::None;

        ui.window("##sidebar")
            .position(pos, Condition::Always)
            .size(size, Condition::Always)
            .flags(flags)
            .movable(false)
            .resizable(false)
            .build(|| {
                if ui.button("Files") {
                    self.active_tab = SidebarTab::Files;
                }
                ui.same_line();
                if ui.button("Search") {
                    self.active_tab = SidebarTab::Grep;
                }
                ui.same_line();
                if ui.button("Close") {
                    self.visible = false;
                    return;
                }
                ui.separator();

                let content_rect = {
                    let avail = ui.content_region_avail();
                    Rect {
                        x: ui.cursor_pos()[0],
                        y: ui.cursor_pos()[1],
                        w: avail[0],
                        h: avail[1],
                    }
                };

                match self.active_tab {
                    SidebarTab::Files => {
                        if let Some(path) = self.file_panel.draw(ui, content_rect, dt) {
                            action = SidebarAction::OpenFile(path);
                        }
                    }
                    SidebarTab::Grep => {
                        if let Some(m) = self.grep_panel.draw(ui, content_rect) {
                            action = SidebarAction::OpenGrepMatch(m);
                        }
                    }
                }
            });

        action
    }
}

struct FileListPanel {
    picker: FuzzyPicker<PathBuf>,
}

impl FileListPanel {
    fn empty() -> Self {
        Self {
            picker: FuzzyPicker::new(),
        }
    }

    fn reload(&mut self, items: Vec<(String, PathBuf)>, query: String) {
        self.picker.load_pinned(items, query);
    }

    fn draw(&mut self, ui: &Ui, content_rect: Rect, _dt: f32) -> Option<PathBuf> {
        self.picker.draw_inline(ui, "##sidebar_file_list", content_rect)
    }
}

struct GrepResultsPanel {
    results: Vec<GrepMatch>,
    filtered: Vec<usize>,
    query: String,
    selected: usize,
    scroll_anchor: Option<usize>,
    project_root: PathBuf,
}

impl GrepResultsPanel {
    fn empty() -> Self {
        Self {
            results: Vec::new(),
            filtered: Vec::new(),
            query: String::new(),
            selected: 0,
            scroll_anchor: None,
            project_root: PathBuf::new(),
        }
    }

    fn reload(
        &mut self,
        results: Vec<GrepMatch>,
        query: String,
        project_root: PathBuf,
    ) {
        self.results = results;
        self.query = query;
        self.project_root = project_root;
        self.selected = 0;
        self.scroll_anchor = None;
        self.rebuild_filtered();
    }

    fn draw(&mut self, ui: &Ui, content_rect: Rect) -> Option<GrepMatch> {
        let mut confirmed = None;

        let mut query = self.query.clone();
        if ui.input_text("##grep_panel_query", &mut query).build() {
            self.query = query;
            self.selected = 0;
            self.rebuild_filtered();
        } else {
            self.query = query;
        }

        if ui.is_key_pressed(Key::Escape) {
            return None;
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
            return self
                .filtered
                .get(self.selected)
                .and_then(|idx| self.results.get(*idx).cloned());
        }

        let list_size = [
            content_rect.w,
            (content_rect.h - ui.cursor_pos()[1] + ui.window_pos()[1]).max(0.0),
        ];
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
                scroll_list_to_selection(ui, self.selected, &mut self.scroll_anchor, max_visible);

                for (idx, &result_idx) in self.filtered.iter().enumerate() {
                    let Some(item) = self.results.get(result_idx) else {
                        continue;
                    };
                    let label = format_match_label(&self.project_root, item);
                    let selected = idx == self.selected;
                    let clicked = ui.selectable_config(&label).selected(selected).build();
                    if clicked {
                        self.selected = idx;
                        confirmed = Some(item.clone());
                        return;
                    }
                }
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
