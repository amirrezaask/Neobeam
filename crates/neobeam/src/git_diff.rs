//! Git change discovery and file content fetch.
//!
//! Status + per-file content come from `git-core` (libgit2). Network ops
//! (push/pull/fetch) and patch apply remain `git` subprocess for now.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use git_core as gc;

#[derive(Clone, Debug)]
pub struct ChangedFile {
    pub path: String,
    pub status: char,
    pub staged: bool,
    pub unstaged: bool,
}

#[derive(Clone, Debug)]
pub enum FileContent {
    Text { old: String, new: String },
    Binary,
    Added { new: String },
    Deleted { old: String },
    Error(String),
}

pub fn repo_root(cwd: &Path) -> Option<PathBuf> {
    gc::discover(cwd).map(|p| {
        // discover returns workdir with trailing slash; strip for parity.
        let s = p.to_string_lossy();
        PathBuf::from(s.trim_end_matches(std::path::MAIN_SEPARATOR))
    })
}

pub fn stage_file(repo: &Path, path: &str) -> Result<(), String> {
    git_file_op(repo, &["add", "--", path], "stage file")
}

pub fn unstage_file(repo: &Path, path: &str) -> Result<(), String> {
    git_file_op(repo, &["restore", "--staged", "--", path], "unstage file")
}

/// Push the current branch to its configured upstream.
pub fn push(repo: &Path) -> Result<(), String> {
    git_file_op(repo, &["push"], "push")
}

/// Commit all staged changes with the given message.
pub fn commit_staged(repo: &Path, message: &str) -> Result<(), String> {
    let repo_str = repo
        .to_str()
        .ok_or_else(|| "invalid repository path".to_string())?;
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_str)
        .args(["commit", "-m", message])
        .output()
        .map_err(|e| format!("failed to commit: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            "git commit failed".to_string()
        } else {
            stderr
        })
    }
}

/// Apply a unified-diff hunk to the index (`git apply --cached`).
pub fn stage_hunk(repo: &Path, patch: &str) -> Result<(), String> {
    git_apply(
        repo,
        patch,
        &["apply", "--cached", "--recount", "--whitespace=nowarn"],
    )
}

/// Remove a hunk from the index (`git apply --cached -R`).
pub fn unstage_hunk(repo: &Path, patch: &str) -> Result<(), String> {
    git_apply(
        repo,
        patch,
        &[
            "apply",
            "--cached",
            "-R",
            "--recount",
            "--whitespace=nowarn",
        ],
    )
}

/// Returns true when the reverse patch applies cleanly to the index, meaning the
/// hunk change is already staged.
pub fn hunk_is_staged(repo: &Path, patch: &str) -> bool {
    git_apply_check(repo, patch, true)
}

fn git_apply_check(repo: &Path, patch: &str, reverse: bool) -> bool {
    let Some(repo_str) = repo.to_str() else {
        return false;
    };
    let mut args = vec![
        "apply",
        "--cached",
        "--check",
        "--recount",
        "--whitespace=nowarn",
    ];
    if reverse {
        args.push("-R");
    }
    let mut child = match Command::new("git")
        .arg("-C")
        .arg(repo_str)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(patch.as_bytes()).is_err() {
            return false;
        }
    }
    child
        .wait_with_output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Discard working-tree changes for a hunk (`git apply -R`).
pub fn restore_hunk_worktree(repo: &Path, patch: &str) -> Result<(), String> {
    git_apply(repo, patch, &["apply", "-R", "--whitespace=nowarn"])
}

fn git_apply(repo: &Path, patch: &str, args: &[&str]) -> Result<(), String> {
    let repo_str = repo
        .to_str()
        .ok_or_else(|| "invalid repository path".to_string())?;
    let action = args.join(" ");
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo_str)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to {action}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(patch.as_bytes())
            .map_err(|e| format!("failed to write patch: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("failed to {action}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("git {action} failed")
        } else {
            stderr
        })
    }
}

fn git_file_op(repo: &Path, args: &[&str], action: &str) -> Result<(), String> {
    let repo_str = repo
        .to_str()
        .ok_or_else(|| "invalid repository path".to_string())?;
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_str)
        .args(args)
        .output()
        .map_err(|e| format!("failed to {action}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("git failed to {action}")
        } else {
            stderr
        })
    }
}

pub fn list_changed_files(repo: &Path) -> Vec<ChangedFile> {
    let Ok(r) = gc::Repo::open(repo) else {
        return Vec::new();
    };
    let Ok(items) = gc::status::list_changed(&r) else {
        return Vec::new();
    };
    let mut out: Vec<ChangedFile> = items
        .into_iter()
        .map(|c| ChangedFile {
            path: c.path,
            status: c.status.glyph(),
            staged: c.staged,
            unstaged: c.unstaged,
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Fetch HEAD vs working-tree content for JetBrains-style diff display.
pub fn fetch_head_vs_worktree(repo: &Path, path: &str) -> FileContent {
    let r = match gc::Repo::open(repo) {
        Ok(r) => r,
        Err(_) => return FileContent::Error("not a git repository".into()),
    };
    let old = r.read_head_blob(path);
    let wt = repo.join(path);
    let new = std::fs::read(&wt).ok();

    match (old, new) {
        (None, None) => FileContent::Error("File not found in HEAD or working tree".into()),
        (Some(old_bytes), None) => {
            if is_probably_binary(&old_bytes) {
                FileContent::Binary
            } else {
                FileContent::Deleted {
                    old: String::from_utf8_lossy(&old_bytes).into_owned(),
                }
            }
        }
        (None, Some(new_bytes)) => {
            if is_probably_binary(&new_bytes) {
                FileContent::Binary
            } else {
                FileContent::Added {
                    new: String::from_utf8_lossy(&new_bytes).into_owned(),
                }
            }
        }
        (Some(old_bytes), Some(new_bytes)) => {
            if is_probably_binary(&old_bytes) || is_probably_binary(&new_bytes) {
                FileContent::Binary
            } else {
                FileContent::Text {
                    old: String::from_utf8_lossy(&old_bytes).into_owned(),
                    new: String::from_utf8_lossy(&new_bytes).into_owned(),
                }
            }
        }
    }
}

fn is_probably_binary(data: &[u8]) -> bool {
    data.iter().take(8192).any(|&b| b == 0)
}
