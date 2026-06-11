//! libgit2-backed git operations for neobeam.
//!
//! Provides repo discovery, status, diff, and stage/unstage. UI-agnostic —
//! callers wrap calls in a worker thread (libgit2 calls are blocking).

pub mod diff;
pub mod graph;
pub mod log;
pub mod repo;
pub mod stage;
pub mod status;

pub use diff::{DiffHunk, DiffLine, DiffLineKind, FileDiff};
pub use graph::{Graph, GraphEdge, GraphNode};
pub use log::{walk as walk_log, Commit};
pub use repo::{discover, GitError, Repo};
pub use status::{ChangedFile, FileStatus};
