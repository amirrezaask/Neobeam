//! Git change discovery and file content fetch via `git` subprocess.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(path)
}

pub fn repo_root(cwd: &Path) -> Option<PathBuf> {
    let cwd = cwd.to_str()?;
    let output = Command::new("git")
        .args(["-C", cwd, "rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        None
    } else {
        Some(PathBuf::from(root))
    }
}

pub fn stage_file(repo: &Path, path: &str) -> Result<(), String> {
    git_file_op(repo, &["add", "--", path], "stage file")
}

pub fn unstage_file(repo: &Path, path: &str) -> Result<(), String> {
    git_file_op(repo, &["restore", "--staged", "--", path], "unstage file")
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
        &["apply", "--cached", "-R", "--recount", "--whitespace=nowarn"],
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
    let mut args = vec!["apply", "--cached", "--check", "--recount", "--whitespace=nowarn"];
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
    let mut map: HashMap<String, ChangedFile> = HashMap::new();

    for (status, path) in git_name_status(repo, true) {
        map.entry(path.clone())
            .and_modify(|f| {
                f.staged = true;
                f.status = status;
            })
            .or_insert(ChangedFile {
                path,
                status,
                staged: true,
                unstaged: false,
            });
    }

    for (status, path) in git_name_status(repo, false) {
        map.entry(path.clone())
            .and_modify(|f| {
                f.unstaged = true;
                if !f.staged {
                    f.status = status;
                }
            })
            .or_insert(ChangedFile {
                path,
                status,
                staged: false,
                unstaged: true,
            });
    }

    // Untracked (new) files don't show up in `git diff`; list them explicitly.
    for path in git_untracked_files(repo) {
        map.entry(path.clone())
            .and_modify(|f| {
                f.unstaged = true;
            })
            .or_insert(ChangedFile {
                path,
                status: 'A',
                staged: false,
                unstaged: true,
            });
    }

    let mut files: Vec<_> = map.into_values().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// Fetch HEAD vs working-tree content for JetBrains-style diff display.
pub fn fetch_head_vs_worktree(repo: &Path, path: &str) -> FileContent {
    let old = git_show_bytes(repo, &format!("HEAD:{path}"));
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

fn git_name_status(repo: &Path, cached: bool) -> Vec<(char, String)> {
    let Some(repo_str) = repo.to_str() else {
        return Vec::new();
    };
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo_str);
    if cached {
        cmd.args(["diff", "--cached", "--name-status"]);
    } else {
        cmd.args(["diff", "--name-status"]);
    }
    let Ok(output) = cmd.output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_name_status(&String::from_utf8_lossy(&output.stdout))
}

fn parse_name_status(text: &str) -> Vec<(char, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let status_part = parts.next().unwrap_or("");
        let path = if status_part.starts_with('R') || status_part.starts_with('C') {
            parts.nth(1).unwrap_or("").to_string()
        } else {
            parts.next().unwrap_or("").to_string()
        };
        let status = status_part.chars().next().unwrap_or('?');
        if !path.is_empty() {
            out.push((status, path));
        }
    }
    out
}

fn git_untracked_files(repo: &Path) -> Vec<String> {
    let Some(repo_str) = repo.to_str() else {
        return Vec::new();
    };
    let Ok(output) = Command::new("git")
        .args(["-C", repo_str, "ls-files", "--others", "--exclude-standard"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn git_show_bytes(repo: &Path, spec: &str) -> Option<Vec<u8>> {
    let repo_str = repo.to_str()?;
    let output = Command::new("git")
        .args(["-C", repo_str, "show", spec])
        .output()
        .ok()?;
    if output.status.success() {
        Some(output.stdout)
    } else {
        None
    }
}

fn is_probably_binary(data: &[u8]) -> bool {
    data.iter().take(8192).any(|&b| b == 0)
}
