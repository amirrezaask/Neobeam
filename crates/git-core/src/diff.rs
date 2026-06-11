use std::path::Path;

use git2::{Diff, DiffFormat, DiffLineType, DiffOptions, Patch};

use crate::repo::{GitError, Repo};
use crate::status::ChangedFile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct DiffHunk {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, Default)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub binary: bool,
    pub hunks: Vec<DiffHunk>,
}

/// Build unified diff for one file. Combines staged + unstaged into a single
/// HEAD-vs-worktree view (matches the legacy panel's behavior).
pub fn head_vs_worktree(repo: &Repo, file: &ChangedFile) -> Result<FileDiff, GitError> {
    repo.with(|r| {
        let mut opts = DiffOptions::new();
        opts.pathspec(&file.path)
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true)
            .context_lines(3);

        // HEAD tree (may not exist on first commit).
        let head_tree = r.head().ok().and_then(|h| h.peel_to_tree().ok());
        let diff = match head_tree {
            Some(tree) => r.diff_tree_to_workdir_with_index(Some(&tree), Some(&mut opts))?,
            None => r.diff_tree_to_workdir_with_index(None, Some(&mut opts))?,
        };
        diff_to_file_diff(&diff, &file.path)
    })
}

/// Diff of staged-only changes for one file (HEAD vs index).
pub fn head_vs_index(repo: &Repo, path: &str) -> Result<FileDiff, GitError> {
    repo.with(|r| {
        let mut opts = DiffOptions::new();
        opts.pathspec(path).context_lines(3);
        let head_tree = r.head().ok().and_then(|h| h.peel_to_tree().ok());
        let diff = match head_tree {
            Some(tree) => r.diff_tree_to_index(Some(&tree), None, Some(&mut opts))?,
            None => r.diff_tree_to_index(None, None, Some(&mut opts))?,
        };
        diff_to_file_diff(&diff, path)
    })
}

/// Diff of unstaged-only changes for one file (index vs worktree).
pub fn index_vs_worktree(repo: &Repo, path: &str) -> Result<FileDiff, GitError> {
    repo.with(|r| {
        let mut opts = DiffOptions::new();
        opts.pathspec(path)
            .include_untracked(true)
            .show_untracked_content(true)
            .context_lines(3);
        let diff = r.diff_index_to_workdir(None, Some(&mut opts))?;
        diff_to_file_diff(&diff, path)
    })
}

fn diff_to_file_diff(diff: &Diff<'_>, target_path: &str) -> Result<FileDiff, GitError> {
    let mut out = FileDiff {
        path: target_path.to_string(),
        ..Default::default()
    };

    for delta_idx in 0..diff.deltas().len() {
        let Ok(patch_opt) = Patch::from_diff(diff, delta_idx) else {
            continue;
        };
        let Some(patch) = patch_opt else { continue };
        let delta = patch.delta();
        let new_path = delta.new_file().path().and_then(Path::to_str).unwrap_or("");
        let old_path = delta
            .old_file()
            .path()
            .and_then(Path::to_str)
            .map(str::to_string);
        if new_path != target_path && old_path.as_deref() != Some(target_path) {
            continue;
        }
        out.old_path = old_path;

        if delta.flags().is_binary() {
            out.binary = true;
            return Ok(out);
        }

        let n_hunks = patch.num_hunks();
        for h in 0..n_hunks {
            let Ok((hunk, _)) = patch.hunk(h) else {
                continue;
            };
            let header = std::str::from_utf8(hunk.header()).unwrap_or("").to_string();
            let mut hunk_lines = Vec::new();
            let n_lines = patch.num_lines_in_hunk(h).unwrap_or(0);
            for li in 0..n_lines {
                let Ok(line) = patch.line_in_hunk(h, li) else {
                    continue;
                };
                let kind = match line.origin_value() {
                    DiffLineType::Addition => DiffLineKind::Addition,
                    DiffLineType::Deletion => DiffLineKind::Deletion,
                    _ => DiffLineKind::Context,
                };
                let content = std::str::from_utf8(line.content())
                    .unwrap_or("")
                    .trim_end_matches('\n')
                    .to_string();
                hunk_lines.push(DiffLine {
                    kind,
                    old_lineno: line.old_lineno(),
                    new_lineno: line.new_lineno(),
                    content,
                });
            }
            out.hunks.push(DiffHunk {
                header,
                old_start: hunk.old_start(),
                old_lines: hunk.old_lines(),
                new_start: hunk.new_start(),
                new_lines: hunk.new_lines(),
                lines: hunk_lines,
            });
        }
        return Ok(out);
    }
    Ok(out)
}

/// Format a single hunk back into unified-diff text (for `git apply`).
pub fn hunk_to_patch(file: &FileDiff, hunk: &DiffHunk) -> String {
    let old_path = file.old_path.as_deref().unwrap_or(&file.path);
    let new_path = &file.path;
    let mut s = String::new();
    s.push_str(&format!("diff --git a/{old_path} b/{new_path}\n"));
    s.push_str(&format!("--- a/{old_path}\n"));
    s.push_str(&format!("+++ b/{new_path}\n"));
    s.push_str(&hunk.header);
    if !hunk.header.ends_with('\n') {
        s.push('\n');
    }
    for l in &hunk.lines {
        let prefix = match l.kind {
            DiffLineKind::Context => ' ',
            DiffLineKind::Addition => '+',
            DiffLineKind::Deletion => '-',
        };
        s.push(prefix);
        s.push_str(&l.content);
        s.push('\n');
    }
    s
}

/// Render the whole diff to a single unified-diff string (debug helper).
#[allow(dead_code)]
pub fn print_unified(repo: &Repo, file: &ChangedFile) -> Result<String, GitError> {
    repo.with(|r| {
        let mut opts = DiffOptions::new();
        opts.pathspec(&file.path).context_lines(3);
        let head_tree = r.head().ok().and_then(|h| h.peel_to_tree().ok());
        let diff = match head_tree {
            Some(tree) => r.diff_tree_to_workdir_with_index(Some(&tree), Some(&mut opts))?,
            None => r.diff_tree_to_workdir_with_index(None, Some(&mut opts))?,
        };
        let mut out = String::new();
        diff.print(DiffFormat::Patch, |_, _, line| {
            let prefix = match line.origin_value() {
                DiffLineType::Addition => "+",
                DiffLineType::Deletion => "-",
                DiffLineType::Context => " ",
                _ => "",
            };
            out.push_str(prefix);
            out.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
            true
        })?;
        Ok(out)
    })
}
