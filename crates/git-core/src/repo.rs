use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use git2::Repository;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("not a git repository")]
    NotARepo,
    #[error("git2: {0}")]
    Git2(#[from] git2::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

/// Discover the workdir of a repo by walking up from `start`. Returns the
/// canonical workdir path (not .git).
pub fn discover(start: &Path) -> Option<PathBuf> {
    let repo = Repository::discover(start).ok()?;
    repo.workdir().map(|p| p.to_path_buf())
}

/// Thread-safe wrapper. `Repository` is `!Sync` so we hold it behind a Mutex.
#[derive(Clone)]
pub struct Repo {
    inner: Arc<Mutex<Repository>>,
    workdir: PathBuf,
}

impl Repo {
    pub fn open(path: &Path) -> Result<Self, GitError> {
        let repo = Repository::discover(path).map_err(|_| GitError::NotARepo)?;
        let workdir = repo
            .workdir()
            .ok_or(GitError::Other("bare repo unsupported".into()))?
            .to_path_buf();
        Ok(Self {
            inner: Arc::new(Mutex::new(repo)),
            workdir,
        })
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// Run a closure with locked access to the underlying `Repository`.
    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Repository) -> R,
    {
        let guard = self.inner.lock().expect("repo mutex poisoned");
        f(&guard)
    }

    /// Read raw bytes of `path` at HEAD. Returns `None` when HEAD does not
    /// exist (initial commit) or the entry is missing/not a blob.
    pub fn read_head_blob(&self, path: &str) -> Option<Vec<u8>> {
        self.with(|r| -> Option<Vec<u8>> {
            let head = r.head().ok()?;
            let tree = head.peel_to_tree().ok()?;
            let entry = tree.get_path(std::path::Path::new(path)).ok()?;
            let obj = entry.to_object(r).ok()?;
            let blob = obj.as_blob()?;
            Some(blob.content().to_vec())
        })
    }
}
