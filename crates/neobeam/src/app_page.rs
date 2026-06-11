//! Top-level application pages.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppPage {
    #[default]
    Editor,
    GitClient,
    Settings,
}
