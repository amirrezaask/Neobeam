//! Commit history walk.

use git2::{Oid, Sort};

use crate::repo::{GitError, Repo};

#[derive(Clone, Debug)]
pub struct Commit {
    pub id: String,
    pub short_id: String,
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    /// Unix seconds.
    pub time: i64,
    pub parents: Vec<String>,
}

/// Walk commit history from HEAD (topological + time order).
pub fn walk(repo: &Repo, limit: usize) -> Result<Vec<Commit>, GitError> {
    repo.with(|r| {
        let mut rw = r.revwalk()?;
        rw.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
        if r.head().is_ok() {
            rw.push_head()?;
        }
        // Also push all local + remote refs so branches that aren't HEAD show.
        // (Best-effort; ignore failures.)
        let _ = rw.push_glob("refs/heads/*");
        let _ = rw.push_glob("refs/remotes/*");

        let mut out = Vec::with_capacity(limit);
        for (i, oid) in rw.enumerate() {
            if i >= limit {
                break;
            }
            let oid: Oid = oid?;
            let c = r.find_commit(oid)?;
            let id = oid.to_string();
            let short_id = id[..7.min(id.len())].to_string();
            let summary = c.summary().unwrap_or("").to_string();
            let author = c.author();
            let author_name = author.name().unwrap_or("").to_string();
            let author_email = author.email().unwrap_or("").to_string();
            let time = c.time().seconds();
            let parents: Vec<String> =
                c.parent_ids().map(|p| p.to_string()).collect();
            out.push(Commit {
                id,
                short_id,
                summary,
                author_name,
                author_email,
                time,
                parents,
            });
        }
        Ok(out)
    })
}
