use git2::{Status, StatusOptions};

use crate::repo::{GitError, Repo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileStatus {
    New,
    Modified,
    Deleted,
    Renamed,
    Typechange,
    Conflicted,
    Untracked,
}

impl FileStatus {
    pub fn glyph(self) -> char {
        match self {
            FileStatus::New => 'A',
            FileStatus::Modified => 'M',
            FileStatus::Deleted => 'D',
            FileStatus::Renamed => 'R',
            FileStatus::Typechange => 'T',
            FileStatus::Conflicted => 'U',
            FileStatus::Untracked => '?',
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChangedFile {
    pub path: String,
    pub status: FileStatus,
    pub staged: bool,
    pub unstaged: bool,
}

/// Walk repo status — equivalent to `git status --porcelain`.
pub fn list_changed(repo: &Repo) -> Result<Vec<ChangedFile>, GitError> {
    repo.with(|r| {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .renames_head_to_index(true)
            .renames_index_to_workdir(true);
        let statuses = r.statuses(Some(&mut opts))?;
        let mut out = Vec::with_capacity(statuses.len());
        for s in statuses.iter() {
            let st = s.status();
            let path = s.path().unwrap_or("").to_string();
            if path.is_empty() {
                continue;
            }
            let (kind, staged, unstaged) = classify(st);
            out.push(ChangedFile {
                path,
                status: kind,
                staged,
                unstaged,
            });
        }
        Ok(out)
    })
}

fn classify(s: Status) -> (FileStatus, bool, bool) {
    let staged_mask = Status::INDEX_NEW
        | Status::INDEX_MODIFIED
        | Status::INDEX_DELETED
        | Status::INDEX_RENAMED
        | Status::INDEX_TYPECHANGE;
    let unstaged_mask = Status::WT_NEW
        | Status::WT_MODIFIED
        | Status::WT_DELETED
        | Status::WT_RENAMED
        | Status::WT_TYPECHANGE;

    let staged = s.intersects(staged_mask);
    let unstaged = s.intersects(unstaged_mask);

    let kind = if s.is_conflicted() {
        FileStatus::Conflicted
    } else if s.intersects(Status::WT_NEW) && !staged {
        FileStatus::Untracked
    } else if s.intersects(Status::INDEX_NEW | Status::WT_NEW) {
        FileStatus::New
    } else if s.intersects(Status::INDEX_DELETED | Status::WT_DELETED) {
        FileStatus::Deleted
    } else if s.intersects(Status::INDEX_RENAMED | Status::WT_RENAMED) {
        FileStatus::Renamed
    } else if s.intersects(Status::INDEX_TYPECHANGE | Status::WT_TYPECHANGE) {
        FileStatus::Typechange
    } else {
        FileStatus::Modified
    };
    (kind, staged, unstaged)
}
