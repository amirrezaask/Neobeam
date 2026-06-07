//! Top-level application pages (extensible shell).

use crate::activity_icons;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppPage {
    #[default]
    Editor,
    GitClient,
}

impl AppPage {
    pub fn label(self) -> &'static str {
        match self {
            AppPage::Editor => "Editor",
            AppPage::GitClient => "Git",
        }
    }

    /// VS Code codicon for the activity bar (Symbols Nerd Font Mono).
    pub fn icon(self) -> &'static str {
        match self {
            AppPage::Editor => activity_icons::EDITOR,
            AppPage::GitClient => activity_icons::GIT,
        }
    }
}
