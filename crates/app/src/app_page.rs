//! Top-level application pages (extensible shell).

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
}
