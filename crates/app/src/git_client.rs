//! Git Client page: ImGui filters + `similar`-powered diff view.

use std::path::PathBuf;

use editor_surface::ChromeLayout;
use imgui::{Condition, StyleColor, Ui, WindowFlags};
use similar::{ChangeTag, TextDiff};

use crate::git_diff::{
    expand_tilde, fetch_file_content, list_changed_files, path_matches, repo_root, stage_file,
    unstage_file, ChangedFile, DiffSide, FileContent,
};

const MAX_DIFF_LINES: usize = 2000;
const FILE_LIST_WIDTH: f32 = 220.0;

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
struct DiffLines {
    lines: Vec<DiffLine>,
    hunk_starts: Vec<usize>,
}

#[derive(Clone, Debug)]
enum DiffContent {
    Lines(DiffLines),
    Binary,
    Message(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ViewMode {
    #[default]
    Inline,
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
    side: DiffSide,
}

struct CachedDiff {
    path: String,
    side: DiffSide,
    generation: u64,
    content: DiffContent,
}

pub struct GitClient {
    path_filter: String,
    view_mode: ViewMode,
    files: Vec<ChangedFile>,
    path_candidates: Vec<String>,
    show_suggestions: bool,
    suggestions_hovered: bool,
    selected: Option<SelectedEntry>,
    cached_diff: Option<CachedDiff>,
    error: Option<String>,
    repo_root: Option<PathBuf>,
    generation: u64,
    cwd: String,
    filters_dirty: bool,
    current_hunk: usize,
    scroll_to_hunk: Option<usize>,
}

impl GitClient {
    pub fn new() -> Self {
        GitClient {
            path_filter: String::new(),
            view_mode: ViewMode::default(),
            files: Vec::new(),
            path_candidates: Vec::new(),
            show_suggestions: false,
            suggestions_hovered: false,
            selected: None,
            cached_diff: None,
            error: None,
            repo_root: None,
            generation: 0,
            cwd: String::new(),
            filters_dirty: true,
            current_hunk: 0,
            scroll_to_hunk: None,
        }
    }

    pub fn draw(&mut self, ui: &Ui, project_path: &str, layout: &ChromeLayout) {
        if project_path != self.cwd {
            self.cwd = project_path.to_string();
            self.filters_dirty = true;
        }

        if self.filters_dirty {
            self.refresh_files();
            self.filters_dirty = false;
        }

        let pos = [0.0, layout.editor_y];
        let size = [layout.window_w, layout.editor_h];
        let flags = WindowFlags::NO_TITLE_BAR
            | WindowFlags::NO_RESIZE
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_COLLAPSE
            | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
            | WindowFlags::NO_NAV_FOCUS;

        ui.window("##git_client")
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
    }

    fn draw_toolbar(&mut self, ui: &Ui) {
        ui.text("Path:");
        ui.same_line();
        let mut filter = self.path_filter.clone();
        ui.set_next_item_width(240.0);
        if ui
            .input_text("##path_filter", &mut filter)
            .hint("crates/app or src/main.rs")
            .build()
        {
            self.path_filter = filter;
            self.filters_dirty = true;
            self.show_suggestions = true;
        }
        let input_active = ui.is_item_active();
        let input_min = ui.item_rect_min();
        let input_max = ui.item_rect_max();

        ui.same_line_with_spacing(0.0, 16.0);
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
            self.refresh_files();
        }

        if self.show_suggestions {
            self.draw_path_suggestions(ui, input_min, input_max, input_active);
        } else {
            self.suggestions_hovered = false;
        }
    }

    fn draw_path_suggestions(
        &mut self,
        ui: &Ui,
        input_min: [f32; 2],
        input_max: [f32; 2],
        input_active: bool,
    ) {
        let query = self.path_filter.trim().to_lowercase();
        let matches: Vec<String> = if query.is_empty() {
            self.path_candidates.iter().take(10).cloned().collect()
        } else {
            self.path_candidates
                .iter()
                .filter(|c| {
                    let lc = c.to_lowercase();
                    lc.contains(&query) && lc != query
                })
                .take(10)
                .cloned()
                .collect()
        };

        if matches.is_empty() {
            self.suggestions_hovered = false;
            if !input_active {
                self.show_suggestions = false;
            }
            return;
        }

        let row_h = ui.text_line_height_with_spacing();
        let height = (matches.len() as f32 * row_h + ui.text_line_height()).min(220.0);
        let width = (input_max[0] - input_min[0]).max(240.0);

        let flags = WindowFlags::NO_TITLE_BAR
            | WindowFlags::NO_RESIZE
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_COLLAPSE
            | WindowFlags::NO_SAVED_SETTINGS
            | WindowFlags::NO_FOCUS_ON_APPEARING
            | WindowFlags::NO_NAV_FOCUS;

        let mut chosen: Option<String> = None;
        let mut hovered = false;
        ui.window("##path_suggestions")
            .position([input_min[0], input_max[1] + 2.0], Condition::Always)
            .size([width, height], Condition::Always)
            .flags(flags)
            .build(|| {
                for cand in &matches {
                    if ui.selectable(cand) {
                        chosen = Some(cand.clone());
                    }
                }
                hovered =
                    ui.is_window_hovered_with_flags(imgui::WindowHoveredFlags::CHILD_WINDOWS);
            });

        self.suggestions_hovered = hovered;

        if let Some(cand) = chosen {
            self.path_filter = cand;
            self.filters_dirty = true;
            self.show_suggestions = false;
            self.suggestions_hovered = false;
        } else if !input_active && !hovered {
            self.show_suggestions = false;
        }
    }

    fn draw_body(&mut self, ui: &Ui) {
        let avail = ui.content_region_avail();
        let list_h = avail[1];

        ui.child_window("##file_list")
            .size([FILE_LIST_WIDTH, list_h])
            .border(true)
            .build(|| {
                self.draw_file_sidebar(ui);
            });

        ui.same_line();
        let diff_w = unsafe {
            avail[0] - FILE_LIST_WIDTH - ui.style().item_spacing[0]
        };
        ui.child_window("##diff_view")
            .size([diff_w, list_h])
            .border(true)
            .horizontal_scrollbar(true)
            .build(|| {
                self.draw_diff_panel(ui);
            });
    }

    fn draw_file_sidebar(&mut self, ui: &Ui) {
        let staged: Vec<ChangedFile> = self
            .files
            .iter()
            .filter(|f| f.staged)
            .cloned()
            .collect();
        let unstaged: Vec<ChangedFile> = self
            .files
            .iter()
            .filter(|f| f.unstaged)
            .cloned()
            .collect();

        let has_staged = !staged.is_empty();
        let has_unstaged = !unstaged.is_empty();

        if !has_staged && !has_unstaged {
            ui.text_disabled("No matching changes.");
            return;
        }

        if has_staged {
            ui.text("Staged");
            ui.separator();
            for file in &staged {
                self.draw_file_entry(ui, file, DiffSide::Staged);
            }
        }

        if has_staged && has_unstaged {
            ui.spacing();
        }

        if has_unstaged {
            ui.text("Unstaged");
            ui.separator();
            for file in &unstaged {
                self.draw_file_entry(ui, file, DiffSide::Unstaged);
            }
        }
    }

    fn draw_file_entry(&mut self, ui: &Ui, file: &ChangedFile, side: DiffSide) {
        let selected = self
            .selected
            .as_ref()
            .is_some_and(|s| s.path == file.path && s.side == side);
        let label = format!("{} {}", file.status, file.path);
        if ui.selectable_config(&label).selected(selected).build() && !selected {
            self.selected = Some(SelectedEntry {
                path: file.path.clone(),
                side,
            });
            self.cached_diff = None;
            self.current_hunk = 0;
            self.scroll_to_hunk = None;
        }

        if let Some(_popup) = ui.begin_popup_context_item() {
            self.draw_file_context_menu(ui, &file.path, side);
        }
    }

    fn draw_file_context_menu(&mut self, ui: &Ui, path: &str, side: DiffSide) {
        match side {
            DiffSide::Unstaged => {
                if ui.menu_item("Stage") {
                    self.stage_path(path);
                    ui.close_current_popup();
                }
            }
            DiffSide::Staged => {
                if ui.menu_item("Unstage") {
                    self.unstage_path(path);
                    ui.close_current_popup();
                }
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
                self.filters_dirty = true;
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
                self.filters_dirty = true;
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

        let side = sel.side;
        let needs_load = self.cached_diff.as_ref().is_none_or(|c| {
            c.path != file.path || c.side != side || c.generation != self.generation
        });
        if needs_load {
            self.load_diff(&file, side);
        }

        let side_label = match side {
            DiffSide::Staged => "staged",
            DiffSide::Unstaged => "unstaged",
        };
        ui.text(&format!("{}  [{side_label}]", file.path));
        ui.separator();

        let Some(cached) = &self.cached_diff else {
            ui.text_disabled("Loading…");
            return;
        };

        match &cached.content {
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
                let shown = &diff.lines[..diff.lines.len().min(MAX_DIFF_LINES)];
                let scroll_to = self.scroll_to_hunk.and_then(|h| {
                    diff.hunk_starts.get(h).copied()
                });
                match self.view_mode {
                    ViewMode::Inline => {
                        for (i, line) in shown.iter().enumerate() {
                            if scroll_to == Some(i) {
                                ui.set_scroll_here_y_with_ratio(0.0);
                                self.scroll_to_hunk = None;
                            }
                            draw_diff_line(ui, line, &colors);
                        }
                    }
                    ViewMode::SideBySide => {
                        draw_side_by_side(ui, shown, &colors, scroll_to, &mut self.scroll_to_hunk);
                    }
                }
            }
        }
    }

    fn refresh_files(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.cached_diff = None;

        let cwd = expand_tilde(&self.cwd);
        self.repo_root = repo_root(&cwd);
        let Some(repo) = self.repo_root.clone() else {
            self.error = Some("Not a git repository.".into());
            self.files.clear();
            self.selected = None;
            return;
        };
        self.error = None;

        let all = list_changed_files(&repo);
        self.path_candidates = build_path_candidates(&all);
        self.files = all
            .into_iter()
            .filter(|f| path_matches(&self.path_filter, &f.path))
            .collect();

        self.validate_selection();
    }

    fn validate_selection(&mut self) {
        let still_valid = self.selected.as_ref().is_some_and(|s| {
            self.files.iter().any(|f| {
                f.path == s.path
                    && match s.side {
                        DiffSide::Staged => f.staged,
                        DiffSide::Unstaged => f.unstaged,
                    }
            })
        });

        if still_valid {
            return;
        }

        self.selected = self
            .files
            .iter()
            .find(|f| f.staged)
            .map(|f| SelectedEntry {
                path: f.path.clone(),
                side: DiffSide::Staged,
            })
            .or_else(|| {
                self.files.iter().find(|f| f.unstaged).map(|f| SelectedEntry {
                    path: f.path.clone(),
                    side: DiffSide::Unstaged,
                })
            });
        self.cached_diff = None;
    }

    fn load_diff(&mut self, file: &ChangedFile, side: DiffSide) {
        let Some(repo) = self.repo_root.clone() else {
            return;
        };
        let content = match fetch_file_content(&repo, &file.path, side) {
            FileContent::Text { old, new } => {
                if old == new {
                    DiffContent::Message("No textual changes.".into())
                } else {
                    DiffContent::Lines(build_diff_lines(&old, &new))
                }
            }
            FileContent::Added { new } => DiffContent::Lines(build_diff_lines("", &new)),
            FileContent::Deleted { old } => DiffContent::Lines(build_diff_lines(&old, "")),
            FileContent::Binary => DiffContent::Binary,
            FileContent::Error(msg) => DiffContent::Message(msg),
        };
        self.current_hunk = 0;
        self.scroll_to_hunk = if matches!(&content, DiffContent::Lines(d) if !d.hunk_starts.is_empty()) {
            Some(0)
        } else {
            None
        };
        self.cached_diff = Some(CachedDiff {
            path: file.path.clone(),
            side,
            generation: self.generation,
            content,
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

/// Pair up delete/insert runs into aligned left/right rows; equal lines occupy
/// both columns on the same row.
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

    // Skip a line that has no content on this side (e.g. an insert in the
    // left column, or a delete in the right column).
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

/// Collect file paths plus their directory prefixes as autocomplete candidates.
fn build_path_candidates(files: &[ChangedFile]) -> Vec<String> {
    let mut set = std::collections::BTreeSet::new();
    for f in files {
        set.insert(f.path.clone());
        let mut acc = String::new();
        let mut parts: Vec<&str> = f.path.split('/').collect();
        parts.pop(); // drop the file name; keep directory prefixes only
        for p in parts {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(p);
            set.insert(acc.clone());
        }
    }
    set.into_iter().collect()
}

fn build_diff_lines(old: &str, new: &str) -> DiffLines {
    let diff = TextDiff::from_lines(old, new);
    let lines: Vec<DiffLine> = diff
        .iter_all_inline_changes()
        .map(|change| {
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
        })
        .collect();
    DiffLines {
        hunk_starts: find_hunk_starts(&lines),
        lines,
    }
}

/// Line indices where each contiguous changed region begins.
fn find_hunk_starts(lines: &[DiffLine]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut in_hunk = false;
    for (i, line) in lines.iter().enumerate() {
        let changed = !matches!(line.tag, ChangeTag::Equal);
        if changed && !in_hunk {
            starts.push(i);
            in_hunk = true;
        } else if !changed {
            in_hunk = false;
        }
    }
    starts
}
