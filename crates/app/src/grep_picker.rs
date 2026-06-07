//! Grep picker: live in-project content search backed by ripgrep libraries.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use grep_regex::RegexMatcherBuilder;
use grep_searcher::SearcherBuilder;
use grep_searcher::{Searcher, Sink, SinkMatch};
use ignore::WalkBuilder;
use ignore::WalkState;
use imgui::{Condition, Key, StyleVar, Ui, WindowFlags};

use crate::fuzzy_picker::{
    picker_default_size, picker_initial_position, scroll_list_to_selection, title_bar_pin_button,
    visible_row_count,
    PICKER_MIN_HEIGHT, PICKER_MIN_WIDTH,
};

const WINDOW_TITLE: &str = "Search in Project";
const WINDOW_ID_SUFFIX: &str = "##grep_picker";
const INPUT_ID: &str = "##grep_query";
const FADE_IN_SPEED: f32 = 5.0;
const FADE_OUT_SPEED: f32 = 4.0;
const DEBOUNCE_SECS: f32 = 0.2;
const MIN_QUERY_LEN: usize = 2;
const MAX_RESULTS: usize = 500;
const MAX_LINE_PREVIEW: usize = 120;

#[derive(Clone, Debug)]
pub enum GrepPickerOutcome {
    None,
    Selected(GrepMatch),
    Pinned {
        results: Vec<GrepMatch>,
        query: String,
        project_root: PathBuf,
    },
}

#[derive(Clone, Debug)]
pub struct GrepMatch {
    pub path: PathBuf,
    pub line_number: u64,
    pub line_text: String,
}

pub struct GrepPicker {
    query: String,
    last_searched: String,
    results: Vec<GrepMatch>,
    selected: usize,
    scroll_anchor: Option<usize>,
    open: bool,
    closing: bool,
    alpha: f32,
    focus_input: bool,
    project_root: PathBuf,
    debounce_secs: f32,
    search_in_flight: bool,
    result_rx: flume::Receiver<Vec<GrepMatch>>,
    result_tx: flume::Sender<Vec<GrepMatch>>,
}

impl GrepPicker {
    pub fn new() -> Self {
        let (result_tx, result_rx) = flume::unbounded();
        GrepPicker {
            query: String::new(),
            last_searched: String::new(),
            results: Vec::new(),
            selected: 0,
            scroll_anchor: None,
            open: false,
            closing: false,
            alpha: 0.0,
            focus_input: false,
            project_root: PathBuf::new(),
            debounce_secs: 0.0,
            search_in_flight: false,
            result_rx,
            result_tx,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn is_animating(&self) -> bool {
        self.open && (self.alpha < 1.0 || self.closing)
    }

    pub fn open(&mut self, project_root: &Path) {
        self.project_root = project_root.to_path_buf();
        self.query.clear();
        self.last_searched.clear();
        self.results.clear();
        self.selected = 0;
        self.scroll_anchor = None;
        self.open = true;
        self.closing = false;
        self.alpha = 0.0;
        self.focus_input = true;
        self.debounce_secs = 0.0;
        self.search_in_flight = false;
        while self.result_rx.try_recv().is_ok() {}
    }

    pub fn draw(&mut self, ui: &Ui, dt: f32) -> GrepPickerOutcome {
        if !self.open {
            return GrepPickerOutcome::None;
        }

        self.poll_results();
        self.tick_search(dt);

        if self.closing {
            self.alpha = (self.alpha - dt * FADE_OUT_SPEED).max(0.0);
            if self.alpha <= 0.0 {
                self.close_immediate();
                return GrepPickerOutcome::None;
            }
        } else {
            self.alpha = (self.alpha + dt * FADE_IN_SPEED).min(1.0);
        }

        let window_name = format!("{WINDOW_TITLE}{WINDOW_ID_SUFFIX}");
        let pos = picker_initial_position(ui);
        let size = picker_default_size(ui);

        let mut outcome = GrepPickerOutcome::None;
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
                let content_start = ui.cursor_screen_pos();
                if title_bar_pin_button(ui) {
                    outcome = GrepPickerOutcome::Pinned {
                        results: self.results.clone(),
                        query: self.query.clone(),
                        project_root: self.project_root.clone(),
                    };
                    self.close_immediate();
                    return;
                }
                ui.set_cursor_screen_pos(content_start);

                if self.focus_input {
                    ui.set_keyboard_focus_here();
                    self.focus_input = false;
                }

                let prev_query = self.query.clone();
                let mut query = self.query.clone();
                if ui.input_text(INPUT_ID, &mut query).build() {
                    self.query = query;
                    if self.query != prev_query {
                        self.selected = 0;
                        self.debounce_secs = 0.0;
                    }
                } else {
                    self.query = query;
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
                if go_down && self.selected + 1 < self.results.len() {
                    self.selected += 1;
                }
                if ui.is_key_pressed(Key::Enter) && ui.io().key_ctrl {
                    outcome = GrepPickerOutcome::Pinned {
                        results: self.results.clone(),
                        query: self.query.clone(),
                        project_root: self.project_root.clone(),
                    };
                    self.close_immediate();
                    return;
                }
                if ui.is_key_pressed(Key::Enter) && !self.results.is_empty() {
                    if let Some(value) = self.confirm_selection() {
                        outcome = GrepPickerOutcome::Selected(value);
                    }
                    return;
                }

                let status = if self.search_in_flight {
                    "Searching..."
                } else if self.query.len() < MIN_QUERY_LEN {
                    "Type at least 2 characters to search"
                } else if self.results.is_empty() {
                    "No matches"
                } else {
                    ""
                };

                let list_size = ui.content_region_avail();
                ui.child_window("##grep_list")
                    .size(list_size)
                    .border(true)
                    .build(|| {
                        if !status.is_empty() {
                            ui.text_disabled(status);
                            return;
                        }

                        let max_visible = visible_row_count(ui);
                        scroll_list_to_selection(
                            ui,
                            self.selected,
                            &mut self.scroll_anchor,
                            max_visible,
                        );

                        for (idx, item) in self.results.iter().enumerate() {
                            let label = format_match_label(&self.project_root, item);
                            let selected = idx == self.selected;
                            let clicked =
                                ui.selectable_config(&label).selected(selected).build();
                            if clicked {
                                self.selected = idx;
                                if let Some(value) = self.confirm_selection() {
                                    outcome = GrepPickerOutcome::Selected(value);
                                }
                                return;
                            }
                        }
                    });
            });

        outcome
    }

    fn poll_results(&mut self) {
        while let Ok(results) = self.result_rx.try_recv() {
            self.search_in_flight = false;
            self.results = results;
            if self.selected >= self.results.len() {
                self.selected = self.results.len().saturating_sub(1);
            }
        }
    }

    fn tick_search(&mut self, dt: f32) {
        if self.query == self.last_searched {
            return;
        }

        self.debounce_secs += dt;
        if self.debounce_secs < DEBOUNCE_SECS || self.search_in_flight {
            return;
        }

        if self.query.len() < MIN_QUERY_LEN {
            self.last_searched = self.query.clone();
            self.results.clear();
            self.selected = 0;
            return;
        }

        self.last_searched = self.query.clone();
        self.search_in_flight = true;
        self.results.clear();
        self.selected = 0;

        let root = self.project_root.clone();
        let query = self.query.clone();
        let tx = self.result_tx.clone();
        std::thread::spawn(move || {
            let results = grep_search(&root, &query);
            let _ = tx.send(results);
        });
    }

    fn close_immediate(&mut self) {
        self.open = false;
        self.closing = false;
        self.alpha = 0.0;
        self.query.clear();
        self.last_searched.clear();
        self.results.clear();
        self.selected = 0;
        self.scroll_anchor = None;
        self.focus_input = false;
        self.debounce_secs = 0.0;
        self.search_in_flight = false;
    }

    fn begin_close(&mut self) {
        self.closing = true;
    }

    fn confirm_selection(&mut self) -> Option<GrepMatch> {
        let value = self.results.get(self.selected).cloned()?;
        self.close_immediate();
        Some(value)
    }
}

impl Default for GrepPicker {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn format_match_label(root: &Path, item: &GrepMatch) -> String {
    let rel = item
        .path
        .strip_prefix(root)
        .unwrap_or(&item.path)
        .to_string_lossy();
    format!("{rel}:{}  {}", item.line_number, item.line_text)
}

fn truncate_line(text: String) -> String {
    if text.chars().count() <= MAX_LINE_PREVIEW {
        return text;
    }
    text.chars().take(MAX_LINE_PREVIEW).collect::<String>() + "..."
}

fn grep_search(root: &Path, query: &str) -> Vec<GrepMatch> {
    let matcher = match RegexMatcherBuilder::new()
        .case_insensitive(true)
        .build(query)
    {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };

    let results = Arc::new(Mutex::new(Vec::<GrepMatch>::new()));
    let root = root.to_path_buf();

    WalkBuilder::new(&root)
        .hidden(true)
        .build_parallel()
        .run(|| {
            let matcher = matcher.clone();
            let results = Arc::clone(&results);

            Box::new(move |entry| {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => return WalkState::Continue,
                };

                if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                    return WalkState::Continue;
                }

                {
                    let guard = results.lock().expect("grep results lock");
                    if guard.len() >= MAX_RESULTS {
                        return WalkState::Quit;
                    }
                }

                let path = entry.into_path();
                let search_path = path.clone();
                let mut searcher = SearcherBuilder::new().line_number(true).build();
                let mut sink = MatchSink {
                    results: Arc::clone(&results),
                    path,
                    stopped: false,
                };

                if searcher
                    .search_path(&matcher, &search_path, &mut sink)
                    .is_err()
                {
                    return WalkState::Continue;
                }

                if sink.stopped {
                    WalkState::Quit
                } else {
                    WalkState::Continue
                }
            })
        });

    let mut matches = results.lock().expect("grep results lock").clone();
    matches.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.line_number.cmp(&b.line_number))
    });
    matches
}

struct MatchSink {
    results: Arc<Mutex<Vec<GrepMatch>>>,
    path: PathBuf,
    stopped: bool,
}

impl Sink for MatchSink {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let mut guard = self.results.lock().expect("grep results lock");
        if guard.len() >= MAX_RESULTS {
            self.stopped = true;
            return Ok(false);
        }

        let line_number = mat.line_number().unwrap_or(0);
        let line_text = truncate_line(
            String::from_utf8_lossy(mat.bytes()).trim().to_owned(),
        );

        guard.push(GrepMatch {
            path: self.path.clone(),
            line_number,
            line_text,
        });

        Ok(guard.len() < MAX_RESULTS)
    }
}
