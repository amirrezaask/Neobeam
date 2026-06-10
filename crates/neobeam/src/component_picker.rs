//! Modal picker: choose a component kind to fill the focused (Empty) pane.

use imgui::{Condition, Key, StyleVar, Ui, WindowFlags};

use crate::pane::PaneKind;

const ITEMS: &[(PaneKind, &str, &str)] = &[
    (PaneKind::Nvim, "Editor", "Neovim editor surface"),
    (PaneKind::GitDiff, "Git", "Git status and diff viewer"),
    (PaneKind::Settings, "Settings", "Neobeam settings"),
];

pub struct ComponentPicker {
    open: bool,
    selected: usize,
    /// Swallow key polls for the first frame after open; otherwise the
    /// Enter/whatever that triggered the open fires through immediately.
    swallow_frames: u8,
}

impl ComponentPicker {
    pub fn new() -> Self {
        Self { open: false, selected: 0, swallow_frames: 0 }
    }

    pub fn open(&mut self) {
        self.open = true;
        self.selected = 0;
        self.swallow_frames = 2;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Draw the picker; returns `Some(kind)` once a selection is committed.
    pub fn draw(&mut self, ui: &Ui) -> Option<PaneKind> {
        if !self.open {
            return None;
        }
        let swallow = self.swallow_frames > 0;
        if swallow {
            self.swallow_frames -= 1;
        }
        if !swallow && ui.is_key_pressed(Key::Escape) {
            self.open = false;
            return None;
        }
        if !swallow && (ui.is_key_pressed(Key::DownArrow) || ui.is_key_pressed(Key::Tab)) {
            self.selected = (self.selected + 1) % ITEMS.len();
        }
        if !swallow && ui.is_key_pressed(Key::UpArrow) {
            self.selected = (self.selected + ITEMS.len() - 1) % ITEMS.len();
        }
        let mut commit: Option<PaneKind> = None;
        if !swallow && ui.is_key_pressed(Key::Enter) {
            commit = Some(ITEMS[self.selected].0);
        }

        let display = ui.io().display_size;
        let w = 380.0_f32.min(display[0] * 0.7);
        let h = 220.0_f32.min(display[1] * 0.7);
        let pos = [(display[0] - w) * 0.5, (display[1] - h) * 0.5];

        let flags = WindowFlags::NO_TITLE_BAR
            | WindowFlags::NO_RESIZE
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_COLLAPSE
            | WindowFlags::NO_SCROLLBAR;

        let rounding = ui.push_style_var(StyleVar::WindowRounding(8.0));
        let pad = ui.push_style_var(StyleVar::WindowPadding([16.0, 14.0]));
        let mut clicked: Option<usize> = None;
        ui.window("##component_picker")
            .position(pos, Condition::Always)
            .size([w, h], Condition::Always)
            .flags(flags)
            .build(|| {
                ui.text_disabled("Pick component");
                ui.separator();
                ui.spacing();
                for (i, (_, name, desc)) in ITEMS.iter().enumerate() {
                    let selected = i == self.selected;
                    let label = format!("{name}##picker_item_{i}");
                    if ui.selectable_config(&label).selected(selected).build() {
                        clicked = Some(i);
                    }
                    ui.same_line();
                    ui.text_disabled(*desc);
                }
                ui.spacing();
                ui.separator();
                ui.text_disabled("Enter to pick · Esc to cancel · ↑↓/Tab to move");
            });
        drop(pad);
        drop(rounding);

        if let Some(i) = clicked {
            self.selected = i;
            commit = Some(ITEMS[i].0);
        }
        if let Some(k) = commit {
            self.open = false;
            return Some(k);
        }
        None
    }
}
