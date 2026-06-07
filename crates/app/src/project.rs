//! A `Project` is the unit of work: an absolute directory path that all views
//! (editor session, git client, menu bar) share as their single source of truth.

use std::path::PathBuf;

use directories::UserDirs;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub path: PathBuf,
}

impl Project {
    pub fn new(path: PathBuf) -> Self {
        let path = path.canonicalize().unwrap_or(path);
        Project { path }
    }

    /// Returns a `~`-abbreviated path for display (e.g. `~/dev/nvim-ui-rs`).
    pub fn display(&self) -> String {
        let home = UserDirs::new().map(|d| d.home_dir().to_path_buf());
        if let Some(home) = home {
            if let Ok(rel) = self.path.strip_prefix(&home) {
                return format!("~/{}", rel.display());
            }
        }
        self.path.display().to_string()
    }
}
