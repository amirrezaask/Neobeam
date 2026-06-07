//! Git Client page: ImGui filters + `similar`-powered diff view.
//!
//! All blocking git subprocess calls run in dedicated background threads so the
//! render/event thread is never stalled.  The two background workers are:
//!
//!  • Refresh thread  – polls `git status` once per second (or immediately when
//!    woken via `refresh_wake`) and sends the new file list back via a channel.
//!  • Diff thread     – receives a diff request, runs the heavy `git show` /
//!    `git apply --check` work, and sends the result back via a channel.
//!
//! `draw()` does only non-blocking `try_recv` calls and returns `true` whenever
//! new data arrived (so the caller knows to `request_redraw`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::time::Duration;

use crate::layout::Rect;
use imgui::{Condition, StyleColor, Ui, WindowFlags};
use similar::udiff::UnifiedDiffHunk;
use similar::{ChangeTag, DiffOp, InlineChange, TextDiff};

use crate::git_diff::{
    commit_staged, fetch_head_vs_worktree, hunk_is_staged, list_changed_files,
    push as git_push, repo_root, restore_hunk_worktree, stage_file, stage_hunk, unstage_file,
    unstage_hunk, ChangedFile, FileContent,
};

const MAX_DIFF_LINES: usize = 2000;
const COMMIT_AREA_HEIGHT: f32 = 88.0;

#[derive(Clone, Debug)]
pub struct DiffSegment {
    pub emphasized: bool,
    pub text: String,
    pub tag: ChangeTag,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub tag: ChangeTag,
    pub segments: Vec<DiffSegment>,
}

#[derive(Clone, Debug)]
struct DiffHunkInfo {
    index: usize,
    start_line: usize,
    end_line: usize,
    header: String,
    patch: String,
    staged: bool,
}

#[derive(Clone, Debug)]
struct DiffLines {
    lines: Vec<DiffLine>,
    hunk_starts: Vec<usize>,
    hunks: Vec<DiffHunkInfo>,
}

#[derive(Clone, Debug)]
enum DiffContent {
    Lines(DiffLines),
    Binary,
    Message(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ViewMode {
    Inline,
    #[default]
    SideBySide,
}

impl ViewMode {
    const ALL: [ViewMode; 2] = [ViewMode::Inline, ViewMode::SideBySide];

    fn label(self) -> &'static str {
        match self {
            ViewMode::Inline => "Inline",
            ViewMode::SideBySide => "Side-by-side",
        }
    }

    fn index(self) -> usize {
        match self {
            ViewMode::Inline => 0,
            ViewMode::SideBySide => 1,
        }
    }

    fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or(ViewMode::Inline)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SelectedEntry {
    path: String,
}

struct CachedDiff {
    path: String,
    generation: u64,
    content: DiffContent,
}

// ---------------------------------------------------------------------------
// Background thread message types
// ---------------------------------------------------------------------------

/// Result delivered by the background refresh thread.
struct RefreshResult {
    repo_root: Option<PathBuf>,
    files: Vec<ChangedFile>,
}

/// Request sent to the background diff thread.
struct DiffRequest {
    repo: PathBuf,
    file: ChangedFile,
    generation: u64,
}

/// Result delivered by the background diff thread.
struct DiffResponse {
    path: String,
    generation: u64,
    content: DiffContent,
}

// ---------------------------------------------------------------------------

pub struct GitClient {
    sidebar_width: f32,
    view_mode: ViewMode,
    files: Vec<ChangedFile>,
    commit_message: String,
    selected: Option<SelectedEntry>,
    cached_diff: Option<CachedDiff>,
    error: Option<String>,
    repo_root: Option<PathBuf>,
    generation: u64,
    /// Current project path (absolute). Kept in sync with `shared_project`.
    project: Option<PathBuf>,
    current_hunk: usize,
    scroll_to_hunk: Option<usize>,

    /// Shared absolute project path for the background refresh thread.
    shared_project: Arc<Mutex<Option<PathBuf>>>,
    // Send () to wake the refresh thread early (e.g. after a git op).
    refresh_wake: mpsc::SyncSender<()>,
    // Receive file-list updates from the refresh thread.
    refresh_rx: mpsc::Receiver<RefreshResult>,

    // Send a diff request to the diff thread.
    diff_tx: mpsc::SyncSender<DiffRequest>,
    // Receive diff results from the diff thread.
    diff_rx: mpsc::Receiver<DiffResponse>,
    diff_pending: bool,
    last_content_rect: Option<Rect>,
}

impl GitClient {
    /// `on_change` is called from background threads whenever new data is
    /// ready; the caller should use it to wake the event loop (e.g. via
    /// `EventLoopProxy::send_event`).
    pub fn new(on_change: impl Fn() + Send + Sync + 'static) -> Self {
        let on_change = Arc::new(on_change);

        // ── Refresh thread ────────────────────────────────────────────────
        let shared_project: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));

        let (wake_tx, wake_rx) = mpsc::sync_channel::<()>(1);
        let (refresh_tx, refresh_rx) = mpsc::sync_channel::<RefreshResult>(1);

        let project_ref = shared_project.clone();
        let on_change_refresh = on_change.clone();
        std::thread::Builder::new()
            .name("git-refresh".into())
            .spawn(move || {
                loop {
                    // Wait up to 1 second for an early-wake signal, then
                    // refresh regardless.
                    let _ = wake_rx.recv_timeout(Duration::from_secs(1));

                    let path = project_ref.lock().unwrap().clone();
                    let Some(path) = path else {
                        continue;
                    };
                    let repo = repo_root(&path);
                    let files = repo
                        .as_ref()
                        .map(|r| list_changed_files(r))
                        .unwrap_or_default();
                    // Drop result if the main thread is not keeping up; we
                    // will refresh again soon anyway.
                    let _ = refresh_tx.try_send(RefreshResult { repo_root: repo, files });
                    on_change_refresh();
                }
            })
            .expect("git-refresh thread");

        // ── Diff thread ───────────────────────────────────────────────────
        let (diff_tx, diff_rx_thread) = mpsc::sync_channel::<DiffRequest>(1);
        let (diff_tx_thread, diff_rx) = mpsc::sync_channel::<DiffResponse>(1);

        let on_change_diff = on_change.clone();
        std::thread::Builder::new()
            .name("git-diff".into())
            .spawn(move || {
                while let Ok(req) = diff_rx_thread.recv() {
                    let content = load_diff_content(&req.repo, &req.file);
                    let _ = diff_tx_thread.try_send(DiffResponse {
                        path: req.file.path,
                        generation: req.generation,
                        content,
                    });
                    on_change_diff();
                }
            })
            .expect("git-diff thread");

        GitClient {
            sidebar_width: 220.0,
            view_mode: ViewMode::default(),
            files: Vec::new(),
            commit_message: String::new(),
            selected: None,
            cached_diff: None,
            error: None,
            repo_root: None,
            generation: 0,
            project: None,
            current_hunk: 0,
            scroll_to_hunk: None,
            shared_project,
            refresh_wake: wake_tx,
            refresh_rx,
            diff_tx,
            diff_rx,
            diff_pending: false,
            last_content_rect: None,
        }
    }

    /// Update the current project. Immediately wakes the refresh thread.
    pub fn set_project(&mut self, path: Option<PathBuf>) {
        if self.project == path {
            return;
        }
        self.project = path.clone();
        *self.shared_project.lock().unwrap() = path;
        self.files.clear();
        self.selected = None;
        self.cached_diff = None;
        self.repo_root = None;
        self.error = None;
        let _ = self.refresh_wake.try_send(());
    }

    /// Draw the git client UI.  Returns `true` when new data arrived from a
    /// background thread and a redraw should be scheduled.
    pub fn draw(&mut self, ui: &Ui, content_rect: Rect) -> bool {
        let mut wants_redraw = false;

        let size_changed = self.last_content_rect.is_none_or(|r| {
            (r.w - content_rect.w).abs() > 0.5 || (r.h - content_rect.h).abs() > 0.5
        });
        if size_changed {
            let max_sidebar = (content_rect.w * 0.55).max(120.0);
            self.sidebar_width = self.sidebar_width.min(max_sidebar);
            wants_redraw = true;
        }
        self.last_content_rect = Some(content_rect);

        // ── Poll background results (non-blocking) ────────────────────────

        if let Ok(result) = self.refresh_rx.try_recv() {
            match result.repo_root {
                Some(repo) => {
                    self.repo_root = Some(repo);
                    self.error = None;
                }
                None => {
                    self.error = Some("Not a git repository.".into());
                    self.files.clear();
                    self.selected = None;
                    self.repo_root = None;
                }
            }
            self.files = result.files;
            self.validate_selection();
            wants_redraw = true;
        }

        if let Ok(resp) = self.diff_rx.try_recv() {
            if self.selected.as_ref().is_some_and(|s| s.path == resp.path)
                && resp.generation == self.generation
            {
                self.cached_diff = Some(CachedDiff {
                    path: resp.path,
                    generation: resp.generation,
                    content: resp.content,
                });
            }
            self.diff_pending = false;
            wants_redraw = true;
        }

        // ── Draw UI ───────────────────────────────────────────────────────

        let pos = [content_rect.x, content_rect.y];
        let size = [content_rect.w, content_rect.h];
        let flags = WindowFlags::NO_TITLE_BAR
            | WindowFlags::NO_RESIZE
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_COLLAPSE
            | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
            | WindowFlags::NO_NAV_FOCUS;

        let window_id = "##git_client";
        ui.window(&window_id)
            .position(pos, Condition::Always)
            .size(size, Condition::Always)
            .flags(flags)
            .movable(false)
            .resizable(false)
            .build(|| {
                self.draw_toolbar(ui);
                ui.separator();
                if let Some(err) = &self.error {
                    ui.text_colored([1.0, 0.45, 0.45, 1.0], err);
                    return;
                }
                self.draw_body(ui);
            });

        wants_redraw
    }

    fn draw_toolbar(&mut self, ui: &Ui) {
        ui.text("View:");
        ui.same_line();
        let mut view_idx = self.view_mode.index();
        ui.set_next_item_width(130.0);
        if ui.combo(
            "##view_mode",
            &mut view_idx,
            &ViewMode::ALL,
            |m| std::borrow::Cow::Borrowed(m.label()),
        ) {
            self.view_mode = ViewMode::from_index(view_idx);
        }

        ui.same_line_with_spacing(0.0, 16.0);
        let hunk_count = self.hunk_count();
        let at_first = self.current_hunk == 0 || hunk_count == 0;
        let at_last = hunk_count == 0 || self.current_hunk + 1 >= hunk_count;
        if ui.button("<-") && !at_first {
            self.goto_hunk(self.current_hunk - 1);
        }
        ui.same_line();
        if ui.button("->") && !at_last {
            self.goto_hunk(self.current_hunk + 1);
        }
        if hunk_count > 0 {
            ui.same_line();
            ui.text_disabled(format!(
                "Hunk {}/{}",
                self.current_hunk + 1,
                hunk_count
            ));
        }

        ui.same_line_with_spacing(0.0, 16.0);
        if ui.button("Refresh") {
            self.trigger_refresh_now();
        }
        ui.same_line();
        if ui.button("Push") {
            self.push_changes();
        }
    }

    fn push_changes(&mut self) {
        let Some(repo) = self.repo_root.clone() else {
            return;
        };
        match git_push(&repo) {
            Ok(()) => self.error = None,
            Err(e) => self.error = Some(e),
        }
    }

    fn draw_body(&mut self, ui: &Ui) {
        let avail = ui.content_region_avail();
        let list_h = avail[1];
        let splitter_w = 4.0;
        let min_side = 80.0;

        ui.child_window("##file_list")
            .size([self.sidebar_width, list_h])
            .border(true)
            .build(|| {
                self.draw_file_sidebar(ui);
            });

        ui.same_line();

        // Invisible draggable splitter between sidebar and diff panel.
        let splitter_id = ui.push_id("##splitter");
        let cursor = ui.cursor_screen_pos();
        let _ = ui.invisible_button("##splitter_btn", [splitter_w, list_h]);
        if ui.is_item_active() {
            let delta = ui.io().mouse_delta[0];
            self.sidebar_width = (self.sidebar_width + delta)
                .max(min_side)
                .min(avail[0] - min_side - splitter_w);
        }
        if ui.is_item_hovered() || ui.is_item_active() {
            let draw = ui.get_window_draw_list();
            let col = ui.style_color(StyleColor::SeparatorActive);
            draw.add_line(
                cursor,
                [cursor[0], cursor[1] + list_h],
                col,
            ).thickness(splitter_w).build();
        }
        drop(splitter_id);

        ui.same_line();
        let item_spacing = unsafe { ui.style().item_spacing[0] };
        let diff_w = avail[0] - self.sidebar_width - splitter_w - item_spacing * 2.0;
        ui.child_window("##diff_view")
            .size([diff_w.max(min_side), list_h])
            .border(true)
            .horizontal_scrollbar(true)
            .build(|| {
                self.draw_diff_panel(ui);
            });
    }

    fn draw_file_sidebar(&mut self, ui: &Ui) {
        let avail = ui.content_region_avail();
        let list_h = (avail[1] - COMMIT_AREA_HEIGHT).max(40.0);

        ui.child_window("##file_list_scroll")
            .size([avail[0], list_h])
            .border(false)
            .build(|| {
                let files = self.files.clone();
                if files.is_empty() {
                    ui.text_disabled("No changes.");
                } else {
                    for file in &files {
                        self.draw_file_entry(ui, file);
                    }
                }
            });

        ui.separator();
        self.draw_commit_area(ui);
    }

    fn draw_commit_area(&mut self, ui: &Ui) {
        let width = ui.content_region_avail()[0];
        let msg_h = 52.0;

        ui.input_text_multiline("##commit_msg", &mut self.commit_message, [width, msg_h])
            .build();

        let has_staged = self.files.iter().any(|f| f.staged);
        let can_commit = has_staged && !self.commit_message.trim().is_empty();

        if ui.button("Commit") && can_commit {
            self.commit_staged_changes();
        }
    }

    fn commit_staged_changes(&mut self) {
        let Some(repo) = self.repo_root.clone() else {
            return;
        };
        let message = self.commit_message.trim().to_string();
        if message.is_empty() {
            return;
        }
        match commit_staged(&repo, &message) {
            Ok(()) => {
                self.error = None;
                self.commit_message.clear();
                self.cached_diff = None;
                self.generation = self.generation.wrapping_add(1);
                self.trigger_refresh_now();
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn draw_file_entry(&mut self, ui: &Ui, file: &ChangedFile) {
        let has_staged = file.staged;
        let mut checked = has_staged;
        ui.checkbox(&format!("##file_cb_{}", file.path), &mut checked);
        if checked != has_staged {
            if checked {
                self.stage_path(&file.path);
            } else {
                self.unstage_path(&file.path);
            }
        }

        ui.same_line();
        let selected = self
            .selected
            .as_ref()
            .is_some_and(|s| s.path == file.path);
        let label = format!("{} {}##file_sel_{}", file.status, file.path, file.path);
        if ui.selectable_config(&label).selected(selected).build() && !selected {
            self.selected = Some(SelectedEntry {
                path: file.path.clone(),
            });
            self.cached_diff = None;
            self.diff_pending = false;
            self.current_hunk = 0;
            self.scroll_to_hunk = None;
        }

        if let Some(_popup) = ui.begin_popup_context_item() {
            self.draw_file_context_menu(ui, file);
        }
    }

    fn draw_file_context_menu(&mut self, ui: &Ui, file: &ChangedFile) {
        if file.unstaged {
            if ui.menu_item("Stage") {
                self.stage_path(&file.path.clone());
                ui.close_current_popup();
            }
        }
        if file.staged {
            if ui.menu_item("Unstage") {
                self.unstage_path(&file.path.clone());
                ui.close_current_popup();
            }
        }
    }

    fn stage_path(&mut self, path: &str) {
        let Some(repo) = self.repo_root.clone() else {
            return;
        };
        match stage_file(&repo, path) {
            Ok(()) => {
                self.error = None;
                self.after_file_git_op(path);
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn unstage_path(&mut self, path: &str) {
        let Some(repo) = self.repo_root.clone() else {
            return;
        };
        match unstage_file(&repo, path) {
            Ok(()) => {
                self.error = None;
                self.after_file_git_op(path);
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn after_file_git_op(&mut self, path: &str) {
        self.trigger_refresh_now();
        if self.selected.as_ref().is_some_and(|s| s.path == path) {
            self.cached_diff = None;
            self.diff_pending = false;
        }
    }

    fn draw_hunked_diff(
        &mut self,
        ui: &Ui,
        diff: &DiffLines,
        path: &str,
        colors: &DiffColors,
    ) {
        let scroll_to_line = self.scroll_to_hunk.and_then(|h| {
            diff.hunks.get(h).map(|hk| hk.start_line)
        });

        for hunk in &diff.hunks {
            if hunk.start_line >= MAX_DIFF_LINES {
                break;
            }

            if scroll_to_line == Some(hunk.start_line) {
                ui.set_scroll_here_y_with_ratio(0.0);
                self.scroll_to_hunk = None;
            }

            self.draw_hunk_header(ui, path, hunk);

            let end = hunk.end_line.min(MAX_DIFF_LINES);
            let hunk_lines = &diff.lines[hunk.start_line..end];
            match self.view_mode {
                ViewMode::Inline => {
                    for line in hunk_lines {
                        draw_diff_line(ui, line, colors);
                    }
                }
                ViewMode::SideBySide => {
                    let mut noop_scroll = None;
                    draw_side_by_side(ui, hunk_lines, colors, None, &mut noop_scroll);
                }
            }
            ui.separator();
        }
    }

    fn draw_hunk_header(&mut self, ui: &Ui, path: &str, hunk: &DiffHunkInfo) {
        let mut staged = hunk.staged;
        ui.checkbox(&format!("##hunk_cb_{}", hunk.index), &mut staged);
        ui.same_line();

        let hdr_color = ui.style_color(StyleColor::TextDisabled);
        let hdr_label = format!("{}##hunk_hdr_{}", hunk.header.trim_end(), hunk.index);
        let _col = ui.push_style_color(StyleColor::Text, hdr_color);
        ui.selectable_config(&hdr_label)
            .span_all_columns(false)
            .build();

        if let Some(_popup) = ui.begin_popup_context_item() {
            self.draw_hunk_context_menu(ui, path, hunk);
        }

        if staged != hunk.staged {
            self.apply_hunk_staged(path, &hunk.patch, staged);
        }
    }

    fn draw_hunk_context_menu(&mut self, ui: &Ui, path: &str, hunk: &DiffHunkInfo) {
        let mut acted = false;
        if !hunk.staged && ui.menu_item("Stage") {
            self.apply_hunk_staged(path, &hunk.patch, true);
            acted = true;
        }
        if hunk.staged && ui.menu_item("Unstage") {
            self.apply_hunk_staged(path, &hunk.patch, false);
            acted = true;
        }
        if ui.menu_item("Restore") {
            self.restore_hunk(path, &hunk.patch, hunk.staged);
            acted = true;
        }
        if acted {
            ui.close_current_popup();
        }
    }

    fn apply_hunk_staged(&mut self, path: &str, patch: &str, staged: bool) {
        let Some(repo) = self.repo_root.clone() else {
            return;
        };
        let result = if staged {
            stage_hunk(&repo, patch)
        } else {
            unstage_hunk(&repo, patch)
        };
        self.after_hunk_git_op(path, result);
    }

    fn restore_hunk(&mut self, path: &str, patch: &str, is_staged: bool) {
        let Some(repo) = self.repo_root.clone() else {
            return;
        };
        let result = if is_staged {
            let _ = unstage_hunk(&repo, patch);
            restore_hunk_worktree(&repo, patch)
        } else {
            restore_hunk_worktree(&repo, patch)
        };
        self.after_hunk_git_op(path, result);
    }

    fn after_hunk_git_op(&mut self, path: &str, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.error = None;
                self.trigger_refresh_now();
                if self.selected.as_ref().is_some_and(|s| s.path == path) {
                    self.cached_diff = None;
                    self.diff_pending = false;
                }
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn draw_diff_panel(&mut self, ui: &Ui) {
        let Some(sel) = self.selected.clone() else {
            ui.text_disabled("Select a file to view its diff.");
            return;
        };
        let Some(file) = self.files.iter().find(|f| f.path == sel.path).cloned() else {
            return;
        };

        let needs_load = self.cached_diff.as_ref().is_none_or(|c| {
            c.path != file.path || c.generation != self.generation
        });
        if needs_load && !self.diff_pending {
            self.trigger_diff_load(&file);
        }

        ui.text(&format!("{} {}", file.status, file.path));
        ui.separator();

        if self.diff_pending && self.cached_diff.is_none() {
            ui.text_disabled("Loading…");
            return;
        }

        let content = self.cached_diff.as_ref().map(|c| c.content.clone());
        let Some(content) = content else {
            ui.text_disabled("Loading…");
            return;
        };

        match content {
            DiffContent::Binary => {
                ui.text_disabled("Binary file changed.");
            }
            DiffContent::Message(msg) => {
                ui.text_disabled(msg);
            }
            DiffContent::Lines(diff) => {
                if diff.lines.len() >= MAX_DIFF_LINES {
                    ui.text_disabled(format!(
                        "Showing first {MAX_DIFF_LINES} of {} changed lines.",
                        diff.lines.len()
                    ));
                    ui.separator();
                }
                let colors = diff_colors(ui);
                self.draw_hunked_diff(ui, &diff, &file.path, &colors);
            }
        }
    }

    /// Send a wake signal so the refresh thread runs immediately without
    /// waiting for the 1-second timeout.
    fn trigger_refresh_now(&self) {
        let _ = self.refresh_wake.try_send(());
    }

    /// Queue a diff load in the background thread.  Does nothing if a load is
    /// already in flight (the result from the previous request will arrive
    /// shortly via `diff_rx`).
    fn trigger_diff_load(&mut self, file: &ChangedFile) {
        let Some(repo) = self.repo_root.clone() else {
            return;
        };
        let req = DiffRequest {
            repo,
            file: file.clone(),
            generation: self.generation,
        };
        if self.diff_tx.try_send(req).is_ok() {
            self.diff_pending = true;
        }
    }

    fn validate_selection(&mut self) {
        let still_valid = self
            .selected
            .as_ref()
            .is_some_and(|s| self.files.iter().any(|f| f.path == s.path));

        if still_valid {
            return;
        }

        self.cached_diff = None;
        self.diff_pending = false;
        self.generation = self.generation.wrapping_add(1);
        self.selected = self.files.first().map(|f| SelectedEntry {
            path: f.path.clone(),
        });
    }

    fn hunk_count(&self) -> usize {
        match self.cached_diff.as_ref().map(|c| &c.content) {
            Some(DiffContent::Lines(d)) => d.hunk_starts.len(),
            _ => 0,
        }
    }

    fn goto_hunk(&mut self, index: usize) {
        self.current_hunk = index;
        self.scroll_to_hunk = Some(index);
    }
}

// ---------------------------------------------------------------------------
// Background diff loading (free functions, no `&self`)
// ---------------------------------------------------------------------------

fn load_diff_content(repo: &Path, file: &ChangedFile) -> DiffContent {
    match fetch_head_vs_worktree(repo, &file.path) {
        FileContent::Text { old, new } => {
            if old == new {
                DiffContent::Message("No textual changes.".into())
            } else {
                let mut diff = build_diff_lines(&old, &new, &file.path);
                apply_hunk_staged_states(repo, file, &mut diff);
                DiffContent::Lines(diff)
            }
        }
        FileContent::Added { new } => {
            let mut diff = build_diff_lines("", &new, &file.path);
            apply_hunk_staged_states(repo, file, &mut diff);
            DiffContent::Lines(diff)
        }
        FileContent::Deleted { old } => {
            let mut diff = build_diff_lines(&old, "", &file.path);
            apply_hunk_staged_states(repo, file, &mut diff);
            DiffContent::Lines(diff)
        }
        FileContent::Binary => DiffContent::Binary,
        FileContent::Error(msg) => DiffContent::Message(msg),
    }
}

fn apply_hunk_staged_states(repo: &Path, file: &ChangedFile, diff: &mut DiffLines) {
    if file.staged && !file.unstaged {
        for h in &mut diff.hunks {
            h.staged = true;
        }
    } else if !file.staged && file.unstaged {
        for h in &mut diff.hunks {
            h.staged = false;
        }
    } else {
        for h in &mut diff.hunks {
            h.staged = hunk_is_staged(repo, &h.patch);
        }
    }
}

// ---------------------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------------------

struct DiffColors {
    gutter: [f32; 4],
    equal_text: [f32; 4],
    delete_bg: [f32; 4],
    delete_text: [f32; 4],
    delete_emphasis: [f32; 4],
    insert_bg: [f32; 4],
    insert_text: [f32; 4],
    insert_emphasis: [f32; 4],
}

fn diff_colors(ui: &Ui) -> DiffColors {
    let style = unsafe { ui.style() };
    let text = style.colors[StyleColor::Text as usize];
    let disabled = style.colors[StyleColor::TextDisabled as usize];

    DiffColors {
        gutter: disabled,
        equal_text: text,
        delete_bg: [0.35, 0.15, 0.15, 0.55],
        delete_text: [0.95, 0.55, 0.55, 1.0],
        delete_emphasis: [1.0, 0.35, 0.35, 1.0],
        insert_bg: [0.12, 0.28, 0.15, 0.55],
        insert_text: [0.55, 0.9, 0.65, 1.0],
        insert_emphasis: [0.35, 1.0, 0.5, 1.0],
    }
}

fn draw_diff_line(ui: &Ui, line: &DiffLine, colors: &DiffColors) {
    let sign = match line.tag {
        ChangeTag::Delete => "-",
        ChangeTag::Insert => "+",
        ChangeTag::Equal => " ",
    };

    let old_gutter = line
        .old_line
        .map(|n| format!("{n:>5}"))
        .unwrap_or_else(|| "     ".into());
    let new_gutter = line
        .new_line
        .map(|n| format!("{n:>5}"))
        .unwrap_or_else(|| "     ".into());
    let gutter = format!("{sign} {old_gutter} {new_gutter} |");

    let (bg, default_text) = match line.tag {
        ChangeTag::Delete => (Some(colors.delete_bg), colors.delete_text),
        ChangeTag::Insert => (Some(colors.insert_bg), colors.insert_text),
        ChangeTag::Equal => (None, colors.equal_text),
    };

    if let Some(bg_color) = bg {
        let min = ui.cursor_screen_pos();
        let line_h = ui.text_line_height_with_spacing();
        let avail = ui.content_region_avail()[0];
        let max = [min[0] + avail, min[1] + line_h];
        let draw = ui.get_window_draw_list();
        draw.add_rect(min, max, bg_color)
            .filled(true)
            .rounding(0.0)
            .build();
    }

    ui.text_colored(colors.gutter, &gutter);
    ui.same_line();

    let mut first = true;
    for seg in &line.segments {
        if !first && !seg.text.is_empty() {
            ui.same_line();
        }
        first = false;

        let color = segment_color(seg, default_text, colors);
        ui.text_colored(color, &seg.text);
    }

    if line.segments.is_empty() {
        ui.new_line();
    }
}

#[derive(Clone, Copy)]
enum Half {
    Left,
    Right,
}

fn draw_side_by_side(
    ui: &Ui,
    lines: &[DiffLine],
    colors: &DiffColors,
    scroll_to_line: Option<usize>,
    scroll_done: &mut Option<usize>,
) {
    let rows = build_side_rows(lines);

    ui.columns(2, "##sxs_diff", true);
    for (left, right) in rows {
        let row_has_scroll_target = scroll_to_line.is_some_and(|target| {
            left == Some(target) || right == Some(target)
        });
        if row_has_scroll_target {
            ui.set_scroll_here_y_with_ratio(0.0);
            *scroll_done = None;
        }

        let col_w = ui.column_width(0);
        draw_diff_half(ui, left.map(|i| &lines[i]), col_w, colors, Half::Left);
        ui.next_column();

        let col_w = ui.column_width(1);
        draw_diff_half(ui, right.map(|i| &lines[i]), col_w, colors, Half::Right);
        ui.next_column();
    }
    ui.columns(1, "##sxs_reset", false);
}

fn build_side_rows(lines: &[DiffLine]) -> Vec<(Option<usize>, Option<usize>)> {
    let mut rows = Vec::new();
    let mut dels: Vec<usize> = Vec::new();
    let mut adds: Vec<usize> = Vec::new();

    fn flush(
        rows: &mut Vec<(Option<usize>, Option<usize>)>,
        dels: &mut Vec<usize>,
        adds: &mut Vec<usize>,
    ) {
        let n = dels.len().max(adds.len());
        for i in 0..n {
            rows.push((dels.get(i).copied(), adds.get(i).copied()));
        }
        dels.clear();
        adds.clear();
    }

    for (i, line) in lines.iter().enumerate() {
        match line.tag {
            ChangeTag::Delete => dels.push(i),
            ChangeTag::Insert => adds.push(i),
            ChangeTag::Equal => {
                flush(&mut rows, &mut dels, &mut adds);
                rows.push((Some(i), Some(i)));
            }
        }
    }
    flush(&mut rows, &mut dels, &mut adds);
    rows
}

fn draw_diff_half(
    ui: &Ui,
    line: Option<&DiffLine>,
    col_w: f32,
    colors: &DiffColors,
    half: Half,
) {
    let Some(line) = line else {
        ui.new_line();
        return;
    };

    let belongs = matches!(
        (half, line.tag),
        (Half::Left, ChangeTag::Delete)
            | (Half::Left, ChangeTag::Equal)
            | (Half::Right, ChangeTag::Insert)
            | (Half::Right, ChangeTag::Equal)
    );
    if !belongs {
        ui.new_line();
        return;
    }

    let (bg, default_text) = match line.tag {
        ChangeTag::Delete => (Some(colors.delete_bg), colors.delete_text),
        ChangeTag::Insert => (Some(colors.insert_bg), colors.insert_text),
        ChangeTag::Equal => (None, colors.equal_text),
    };

    if let Some(bg_color) = bg {
        let min = ui.cursor_screen_pos();
        let line_h = ui.text_line_height_with_spacing();
        let max = [min[0] + col_w, min[1] + line_h];
        let draw = ui.get_window_draw_list();
        draw.add_rect(min, max, bg_color).filled(true).build();
    }

    let (num, sign) = match half {
        Half::Left => (
            line.old_line,
            if matches!(line.tag, ChangeTag::Delete) { '-' } else { ' ' },
        ),
        Half::Right => (
            line.new_line,
            if matches!(line.tag, ChangeTag::Insert) { '+' } else { ' ' },
        ),
    };
    let num_str = num.map(|n| format!("{n:>5}")).unwrap_or_else(|| "     ".into());
    let gutter = format!("{sign} {num_str} |");

    ui.text_colored(colors.gutter, &gutter);
    ui.same_line();

    let mut first = true;
    for seg in &line.segments {
        if !first && !seg.text.is_empty() {
            ui.same_line();
        }
        first = false;
        let color = segment_color(seg, default_text, colors);
        ui.text_colored(color, seg.text.trim_end_matches('\n'));
    }

    if line.segments.is_empty() {
        ui.new_line();
    }
}

fn segment_color(seg: &DiffSegment, default: [f32; 4], colors: &DiffColors) -> [f32; 4] {
    if !seg.emphasized {
        return default;
    }
    match seg.tag {
        ChangeTag::Delete => colors.delete_emphasis,
        ChangeTag::Insert => colors.insert_emphasis,
        ChangeTag::Equal => default,
    }
}

fn build_diff_lines(old: &str, new: &str, path: &str) -> DiffLines {
    let diff = TextDiff::from_lines(old, new);
    let old_label = format!("a/{path}");
    let new_label = format!("b/{path}");

    let mut lines = Vec::new();
    let mut hunks = Vec::new();

    for (hunk_idx, ops) in diff.grouped_ops(3).into_iter().enumerate() {
        if !ops_has_changes(&ops) {
            continue;
        }

        let start = lines.len();
        let udiff_hunk = UnifiedDiffHunk::new(ops.clone(), &diff, true);
        let hunk_lines = build_lines_for_ops(&diff, &ops);
        lines.extend(hunk_lines);

        let patch = format!("--- {old_label}\n+++ {new_label}\n{udiff_hunk}");
        hunks.push(DiffHunkInfo {
            index: hunk_idx,
            start_line: start,
            end_line: lines.len(),
            header: udiff_hunk.header().to_string(),
            patch,
            staged: false,
        });
    }

    let hunk_starts: Vec<usize> = hunks.iter().map(|h| h.start_line).collect();
    DiffLines {
        lines,
        hunk_starts,
        hunks,
    }
}

fn ops_has_changes(ops: &[DiffOp]) -> bool {
    ops.iter().any(|op| !matches!(op, DiffOp::Equal { .. }))
}

fn build_lines_for_ops(diff: &TextDiff<'_, '_, str>, ops: &[DiffOp]) -> Vec<DiffLine> {
    ops.iter()
        .flat_map(|op| diff.iter_inline_changes(op))
        .map(inline_change_to_line)
        .collect()
}

fn inline_change_to_line(change: InlineChange<'_, str>) -> DiffLine {
    let segments: Vec<DiffSegment> = change
        .iter_strings_lossy()
        .map(|(emphasized, text)| DiffSegment {
            emphasized,
            text: text.into_owned(),
            tag: change.tag(),
        })
        .collect();
    DiffLine {
        old_line: change.old_index().map(|i| (i + 1) as u32),
        new_line: change.new_index().map(|i| (i + 1) as u32),
        tag: change.tag(),
        segments,
    }
}
