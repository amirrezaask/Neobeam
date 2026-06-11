//! Commit-graph lane assignment.
//!
//! Given a topologically-ordered commit list, assign each commit a `lane`
//! (column index ≥ 0) and emit edges to its parents. Lanes are reused once
//! freed. Produces enough info for an SVG/bezier renderer.

use crate::log::Commit;

#[derive(Clone, Copy, Debug)]
pub struct GraphNode {
    /// Index into the input commit list.
    pub commit_idx: usize,
    /// Column for the dot.
    pub lane: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct GraphEdge {
    /// Row index of child commit.
    pub from_row: u32,
    pub from_lane: u32,
    /// Row index of parent commit.
    pub to_row: u32,
    pub to_lane: u32,
    /// Index of parent in commit's parents list (0 = first/main, ≥1 = merge).
    pub parent_index: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub width: u32,
}

/// Lane-assignment over a topologically-ordered commit list.
pub fn build(commits: &[Commit]) -> Graph {
    let mut g = Graph::default();
    if commits.is_empty() {
        return g;
    }

    // index commits by id for parent lookup
    let mut id_to_row: std::collections::HashMap<&str, u32> =
        std::collections::HashMap::with_capacity(commits.len());
    for (i, c) in commits.iter().enumerate() {
        id_to_row.insert(c.id.as_str(), i as u32);
    }

    // lanes[i] = Some(commit_id awaited on lane i) | None (free)
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut max_width: u32 = 0;

    for (row, c) in commits.iter().enumerate() {
        // Find this commit's lane (a parent slot reserved by an earlier child).
        let lane = match lanes
            .iter()
            .position(|l| l.as_deref() == Some(c.id.as_str()))
        {
            Some(p) => p,
            None => {
                // Root of a branch: take first free lane (or append).
                match lanes.iter().position(Option::is_none) {
                    Some(p) => p,
                    None => {
                        lanes.push(None);
                        lanes.len() - 1
                    }
                }
            }
        };

        // Free every lane currently holding this commit (only one expected,
        // but a fast-forward could leave dupes — clear all defensively).
        for slot in lanes.iter_mut() {
            if slot.as_deref() == Some(c.id.as_str()) {
                *slot = None;
            }
        }

        g.nodes.push(GraphNode {
            commit_idx: row,
            lane: lane as u32,
        });

        // Place parents.
        for (pi, parent_id) in c.parents.iter().enumerate() {
            let target_lane = if pi == 0 {
                // First parent stays on this lane.
                lane
            } else {
                // Merge parent: pick a free lane (or append).
                match lanes.iter().position(Option::is_none) {
                    Some(p) => p,
                    None => {
                        lanes.push(None);
                        lanes.len() - 1
                    }
                }
            };
            lanes[target_lane] = Some(parent_id.clone());

            if let Some(&prow) = id_to_row.get(parent_id.as_str()) {
                g.edges.push(GraphEdge {
                    from_row: row as u32,
                    from_lane: lane as u32,
                    to_row: prow,
                    to_lane: target_lane as u32,
                    parent_index: pi as u32,
                });
            }
        }

        max_width = max_width.max(lanes.len() as u32);
    }

    g.width = max_width;
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmt(id: &str, parents: &[&str]) -> Commit {
        Commit {
            id: id.into(),
            short_id: id[..1].into(),
            summary: id.into(),
            author_name: "x".into(),
            author_email: "x".into(),
            time: 0,
            parents: parents.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn linear_history_single_lane() {
        let cs = vec![cmt("C", &["B"]), cmt("B", &["A"]), cmt("A", &[])];
        let g = build(&cs);
        assert_eq!(g.width, 1);
        assert!(g.nodes.iter().all(|n| n.lane == 0));
        assert_eq!(g.edges.len(), 2);
    }

    #[test]
    fn merge_uses_extra_lane() {
        // D merges C (mainline) and B (sidebranch)
        let cs = vec![
            cmt("D", &["C", "B"]),
            cmt("C", &["A"]),
            cmt("B", &["A"]),
            cmt("A", &[]),
        ];
        let g = build(&cs);
        assert!(g.width >= 2);
        // D is on lane 0; one of B/C is on lane 1.
        assert_eq!(g.nodes[0].lane, 0);
    }
}
