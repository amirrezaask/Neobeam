//! Project picker: scans ~/dev for git repos and switches the session working directory.

use std::path::{Path, PathBuf};

use directories::UserDirs;
use imgui::Ui;

use crate::fuzzy_picker::FuzzyPicker;

const PROJECTS_DIR_NAME: &str = "dev";
const SCRATCH_LABEL: &str = "scratch  ~/scratch";
const MAX_SCAN_DEPTH: usize = 3;

pub struct ProjectPicker {
    picker: FuzzyPicker<PathBuf>,
}

impl ProjectPicker {
    pub fn new() -> Self {
        ProjectPicker {
            picker: FuzzyPicker::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.picker.is_open()
    }

    pub fn open(&mut self) {
        let items = scan_projects();
        self.picker.open(items);
    }

    pub fn draw(&mut self, ui: &Ui, dt: f32) -> Option<PathBuf> {
        self.picker.draw(ui, "Session", dt)
    }

    /// True while a fade animation (in or out) is in progress.
    pub fn is_animating(&self) -> bool {
        self.picker.is_animating()
    }
}

impl Default for ProjectPicker {
    fn default() -> Self {
        Self::new()
    }
}

fn projects_root() -> Option<PathBuf> {
    Some(UserDirs::new()?.home_dir().join(PROJECTS_DIR_NAME))
}

fn scratch_dir() -> Option<PathBuf> {
    UserDirs::new().map(|dirs| dirs.home_dir().join("scratch"))
}

fn scan_projects() -> Vec<(String, PathBuf)> {
    let mut items = Vec::new();

    if let Some(scratch) = scratch_dir() {
        items.push((SCRATCH_LABEL.to_string(), scratch));
    }

    let Some(root) = projects_root() else {
        return items;
    };
    if !root.is_dir() {
        return items;
    }

    let mut repos = Vec::new();
    collect_git_repos(&root, &root, 0, &mut repos);
    repos.sort_by(|a, b| a.0.cmp(&b.0));
    items.extend(repos);
    items
}

fn collect_git_repos(
    root: &Path,
    current: &Path,
    depth: usize,
    out: &mut Vec<(String, PathBuf)>,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }

    let git_dir = current.join(".git");
    if git_dir.is_dir() {
        let rel = current
            .strip_prefix(root)
            .unwrap_or(current)
            .to_string_lossy()
            .into_owned();
        if !rel.is_empty() {
            out.push((rel, current.to_path_buf()));
        }
        return;
    }

    let entries = match std::fs::read_dir(current) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    let mut children: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    children.sort();
    for child in children {
        if child.is_dir() {
            collect_git_repos(root, &child, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fuzzy_picker::fuzzy_score;

    #[test]
    fn scratch_label_is_searchable() {
        assert!(fuzzy_score("scr", super::SCRATCH_LABEL).is_some());
    }
}
