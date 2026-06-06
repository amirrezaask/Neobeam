//! Editor right-click context menu (Dear ImGui popup → Neovim commands).

use imgui::{Condition, Ui};
use imgui::sys;

const POPUP_ID: &str = "editor_context";

#[derive(Clone, Debug)]
pub enum ContextMenuCommand {
    Input(String),
    Paste,
}

#[derive(Debug)]
pub enum ContextMenuAction {
    None,
    Dispatch {
        grid_pos: (i64, i64),
        command: ContextMenuCommand,
    },
}

enum MenuItemDef {
    Separator,
    Entry {
        label: &'static str,
        command: CommandDef,
    },
}

#[derive(Copy, Clone)]
enum CommandDef {
    Input(&'static str),
    Paste,
}

impl CommandDef {
    fn to_command(self) -> ContextMenuCommand {
        match self {
            CommandDef::Input(s) => ContextMenuCommand::Input(s.to_string()),
            CommandDef::Paste => ContextMenuCommand::Paste,
        }
    }
}

static MENU_ITEMS: &[MenuItemDef] = &[
    MenuItemDef::Entry {
        label: "Cut",
        command: CommandDef::Input("\"+x"),
    },
    MenuItemDef::Entry {
        label: "Copy",
        command: CommandDef::Input("\"+y"),
    },
    MenuItemDef::Entry {
        label: "Paste",
        command: CommandDef::Paste,
    },
    MenuItemDef::Separator,
    MenuItemDef::Entry {
        label: "Go to definition",
        command: CommandDef::Input("gd"),
    },
    MenuItemDef::Entry {
        label: "Go to declaration",
        command: CommandDef::Input("gD"),
    },
    MenuItemDef::Entry {
        label: "Go to references",
        command: CommandDef::Input(":lua vim.lsp.buf.references()<CR>"),
    },
    MenuItemDef::Separator,
    MenuItemDef::Entry {
        label: "Split vertically",
        command: CommandDef::Input(":vsplit<CR>"),
    },
    MenuItemDef::Entry {
        label: "Split horizontally",
        command: CommandDef::Input(":split<CR>"),
    },
];

pub struct ContextMenu {
    pub open: bool,
    screen_pos: [f32; 2],
    grid_pos: (i64, i64),
    pending_open: bool,
}

impl ContextMenu {
    pub fn new() -> Self {
        ContextMenu {
            open: false,
            screen_pos: [0.0, 0.0],
            grid_pos: (0, 0),
            pending_open: false,
        }
    }

    pub fn open_at(&mut self, screen_pos: [f32; 2], grid_pos: (i64, i64)) {
        self.open = true;
        self.pending_open = true;
        self.screen_pos = screen_pos;
        self.grid_pos = grid_pos;
    }

    pub fn draw(&mut self, ui: &Ui) -> ContextMenuAction {
        if !self.open {
            return ContextMenuAction::None;
        }

        if self.pending_open {
            ui.open_popup(POPUP_ID);
            self.pending_open = false;
        }

        set_popup_pos(self.screen_pos);

        if let Some(_popup) = ui.begin_popup(POPUP_ID) {
            for item in MENU_ITEMS {
                match item {
                    MenuItemDef::Separator => ui.separator(),
                    MenuItemDef::Entry { label, command } => {
                        if ui.menu_item(*label) {
                            ui.close_current_popup();
                            let grid_pos = self.grid_pos;
                            self.open = false;
                            return ContextMenuAction::Dispatch {
                                grid_pos,
                                command: (*command).to_command(),
                            };
                        }
                    }
                }
            }
            ContextMenuAction::None
        } else {
            self.open = false;
            ContextMenuAction::None
        }
    }
}

fn set_popup_pos(pos: [f32; 2]) {
    unsafe {
        sys::igSetNextWindowPos(
            pos.into(),
            Condition::Appearing as i32,
            [0.0, 0.0].into(),
        );
    }
}
