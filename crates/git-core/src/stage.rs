use std::path::Path;

use git2::{ApplyLocation, ApplyOptions, Diff, IndexAddOption};

use crate::repo::{GitError, Repo};

/// Stage a file (or directory) — equivalent to `git add -- <path>`.
pub fn stage_file(repo: &Repo, path: &str) -> Result<(), GitError> {
    repo.with(|r| {
        let mut index = r.index()?;
        index.add_all([path].iter(), IndexAddOption::DEFAULT, None)?;
        index.write()?;
        Ok(())
    })
}

/// Unstage a file — restore index entry to HEAD's version.
pub fn unstage_file(repo: &Repo, path: &str) -> Result<(), GitError> {
    repo.with(|r| {
        let head = match r.head() {
            Ok(h) => h,
            Err(_) => {
                // No HEAD yet (initial commit) → remove from index.
                let mut index = r.index()?;
                index.remove_path(Path::new(path))?;
                index.write()?;
                return Ok(());
            }
        };
        let obj = head.peel(git2::ObjectType::Commit)?;
        let commit = obj.as_commit().ok_or(GitError::Other("HEAD not commit".into()))?;
        r.reset_default(Some(commit.as_object()), [path])?;
        Ok(())
    })
}

/// Apply a unified-diff patch to the index (`git apply --cached`).
pub fn apply_to_index(repo: &Repo, patch_text: &str) -> Result<(), GitError> {
    repo.with(|r| {
        let diff = Diff::from_buffer(patch_text.as_bytes())?;
        let mut opts = ApplyOptions::new();
        r.apply(&diff, ApplyLocation::Index, Some(&mut opts))?;
        Ok(())
    })
}

/// Reverse-apply a unified-diff patch to the index (unstage a hunk).
pub fn revert_from_index(repo: &Repo, patch_text: &str) -> Result<(), GitError> {
    // git2's apply lacks a reverse flag; libgit2 itself does not expose one.
    // Workaround: invert the patch text (swap +/-, swap old/new ranges).
    let inverted = invert_patch(patch_text);
    apply_to_index(repo, &inverted)
}

fn invert_patch(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    for line in p.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("@@ ") {
            // Header @@ -a,b +c,d @@ → swap a,b with c,d.
            if let Some((ranges, tail)) = rest.split_once(" @@") {
                let parts: Vec<&str> = ranges.split_whitespace().collect();
                if parts.len() == 2 {
                    let old = parts[0].trim_start_matches('-');
                    let new = parts[1].trim_start_matches('+');
                    out.push_str(&format!("@@ -{new} +{old} @@{tail}"));
                    continue;
                }
            }
            out.push_str(line);
        } else if let Some(rest) = line.strip_prefix('+') {
            if !line.starts_with("+++") {
                out.push('-');
                out.push_str(rest);
                continue;
            }
            out.push_str(line);
        } else if let Some(rest) = line.strip_prefix('-') {
            if !line.starts_with("---") {
                out.push('+');
                out.push_str(rest);
                continue;
            }
            out.push_str(line);
        } else {
            out.push_str(line);
        }
    }
    out
}
