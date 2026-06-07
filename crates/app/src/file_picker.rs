//! File picker: fuzzy-finds files in the current project directory.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use imgui::Ui;

use crate::fuzzy_picker::{FuzzyPicker, PickerOutcome};

/// Hard cap on the number of files collected to keep the picker responsive.
const MAX_FILES: usize = 50_000;

#[derive(Clone, Debug)]
pub enum FilePickerOutcome {
    None,
    Opened(PathBuf),
    Pinned {
        items: Vec<(String, PathBuf)>,
        query: String,
    },
}

pub struct FilePicker {
    picker: FuzzyPicker<PathBuf>,
}

impl FilePicker {
    pub fn new() -> Self {
        FilePicker {
            picker: FuzzyPicker::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.picker.is_open()
    }

    /// True while a fade animation (in or out) is in progress.
    pub fn is_animating(&self) -> bool {
        self.picker.is_animating()
    }

    /// Scan `project_root` for files and open the picker.
    pub fn open(&mut self, project_root: &Path) {
        let items = scan_files(project_root);
        self.picker.open(items);
    }

    pub fn draw(&mut self, ui: &Ui, dt: f32) -> FilePickerOutcome {
        match self.picker.draw(ui, "Open File", dt) {
            PickerOutcome::None => FilePickerOutcome::None,
            PickerOutcome::Selected(path) => FilePickerOutcome::Opened(path),
            PickerOutcome::Pinned { items, query } => FilePickerOutcome::Pinned { items, query },
        }
    }
}

impl Default for FilePicker {
    fn default() -> Self {
        Self::new()
    }
}

fn scan_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mut items = Vec::new();
    for entry in WalkBuilder::new(root).hidden(true).build() {
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
            let path = entry.into_path();
            let label = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            items.push((label, path));
            if items.len() >= MAX_FILES {
                break;
            }
        }
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items
}
