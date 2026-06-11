//! Tiling pane tree: leaves host components (nvim, git, settings, terminal),
//! splits divide the area horizontally or vertically. Inspired by Hyprland's
//! dynamic tiling — each leaf has a spring-animated rect for buttery resizes.

use crate::layout::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaneId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SplitId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDir {
    /// Children side by side (a left, b right). Split line is vertical.
    Horizontal,
    /// Children stacked (a top, b bottom). Split line is horizontal.
    Vertical,
}

/// Kind of content a leaf hosts. `Empty` is the placeholder shown right after
/// a split — the component picker fills it in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneKind {
    Empty,
    Nvim,
    GitDiff,
    Settings,
    Terminal,
}

#[derive(Clone, Debug)]
pub enum PaneNode {
    Leaf {
        id: PaneId,
        kind: PaneKind,
    },
    Split {
        id: SplitId,
        dir: SplitDir,
        /// Fraction of the area given to child `a` (0.0..=1.0).
        ratio: f32,
        a: Box<PaneNode>,
        b: Box<PaneNode>,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct SplitGutter {
    pub id: SplitId,
    pub dir: SplitDir,
    /// Thin strip rect centered on the split boundary (logical px).
    pub rect: Rect,
    /// Full area occupied by this split's two children (parent rect).
    pub parent_rect: Rect,
}

/// Result of laying out the tree against a target area.
#[derive(Clone, Debug)]
pub struct LeafLayout {
    pub id: PaneId,
    pub kind: PaneKind,
    pub rect: Rect,
}

pub struct PaneTree {
    pub root: PaneNode,
    pub focused: PaneId,
    next_id: u32,
    next_split_id: u32,
}

impl PaneTree {
    pub fn new_with(kind: PaneKind) -> Self {
        let id = PaneId(1);
        Self {
            root: PaneNode::Leaf { id, kind },
            focused: id,
            next_id: 2,
            next_split_id: 1,
        }
    }

    fn alloc_id(&mut self) -> PaneId {
        let id = PaneId(self.next_id);
        self.next_id += 1;
        id
    }

    fn alloc_split_id(&mut self) -> SplitId {
        let id = SplitId(self.next_split_id);
        self.next_split_id += 1;
        id
    }

    /// Flatten the tree into per-leaf rects for the given area.
    pub fn layout(&self, area: Rect) -> Vec<LeafLayout> {
        let mut out = Vec::new();
        Self::layout_node(&self.root, area, &mut out);
        out
    }

    /// Collect gutter rects for all splits. Gutter is a thin strip centered
    /// on the split boundary, used for mouse hit-testing during resize.
    pub fn gutters(&self, area: Rect, thickness: f32) -> Vec<SplitGutter> {
        let mut out = Vec::new();
        Self::gutters_node(&self.root, area, thickness, &mut out);
        out
    }

    fn gutters_node(node: &PaneNode, area: Rect, thickness: f32, out: &mut Vec<SplitGutter>) {
        let PaneNode::Split {
            id,
            dir,
            ratio,
            a,
            b,
        } = node
        else {
            return;
        };
        let r = ratio.clamp(0.05, 0.95);
        let (ra, rb, gutter) = match dir {
            SplitDir::Horizontal => {
                let wa = area.w * r;
                let g_x = area.x + wa - thickness * 0.5;
                (
                    Rect {
                        x: area.x,
                        y: area.y,
                        w: wa,
                        h: area.h,
                    },
                    Rect {
                        x: area.x + wa,
                        y: area.y,
                        w: area.w - wa,
                        h: area.h,
                    },
                    Rect {
                        x: g_x,
                        y: area.y,
                        w: thickness,
                        h: area.h,
                    },
                )
            }
            SplitDir::Vertical => {
                let ha = area.h * r;
                let g_y = area.y + ha - thickness * 0.5;
                (
                    Rect {
                        x: area.x,
                        y: area.y,
                        w: area.w,
                        h: ha,
                    },
                    Rect {
                        x: area.x,
                        y: area.y + ha,
                        w: area.w,
                        h: area.h - ha,
                    },
                    Rect {
                        x: area.x,
                        y: g_y,
                        w: area.w,
                        h: thickness,
                    },
                )
            }
        };
        out.push(SplitGutter {
            id: *id,
            dir: *dir,
            rect: gutter,
            parent_rect: area,
        });
        Self::gutters_node(a, ra, thickness, out);
        Self::gutters_node(b, rb, thickness, out);
    }

    /// Set ratio of a specific split. Clamped to [0.05, 0.95].
    pub fn set_split_ratio(&mut self, target: SplitId, new_ratio: f32) {
        Self::set_ratio_in(&mut self.root, target, new_ratio.clamp(0.05, 0.95));
    }

    fn set_ratio_in(node: &mut PaneNode, target: SplitId, new_ratio: f32) -> bool {
        match node {
            PaneNode::Leaf { .. } => false,
            PaneNode::Split {
                id, ratio, a, b, ..
            } => {
                if *id == target {
                    *ratio = new_ratio;
                    true
                } else {
                    Self::set_ratio_in(a, target, new_ratio)
                        || Self::set_ratio_in(b, target, new_ratio)
                }
            }
        }
    }

    fn layout_node(node: &PaneNode, area: Rect, out: &mut Vec<LeafLayout>) {
        match node {
            PaneNode::Leaf { id, kind } => out.push(LeafLayout {
                id: *id,
                kind: *kind,
                rect: area,
            }),
            PaneNode::Split {
                dir, ratio, a, b, ..
            } => {
                let r = ratio.clamp(0.05, 0.95);
                let (ra, rb) = match dir {
                    SplitDir::Horizontal => {
                        let wa = area.w * r;
                        (
                            Rect {
                                x: area.x,
                                y: area.y,
                                w: wa,
                                h: area.h,
                            },
                            Rect {
                                x: area.x + wa,
                                y: area.y,
                                w: area.w - wa,
                                h: area.h,
                            },
                        )
                    }
                    SplitDir::Vertical => {
                        let ha = area.h * r;
                        (
                            Rect {
                                x: area.x,
                                y: area.y,
                                w: area.w,
                                h: ha,
                            },
                            Rect {
                                x: area.x,
                                y: area.y + ha,
                                w: area.w,
                                h: area.h - ha,
                            },
                        )
                    }
                };
                Self::layout_node(a, ra, out);
                Self::layout_node(b, rb, out);
            }
        }
    }

    /// Split the focused leaf, inserting a new `Empty` sibling on the
    /// trailing side. Focus moves to the new leaf so the picker opens there.
    pub fn split_focused(&mut self, dir: SplitDir) -> PaneId {
        let new_id = self.alloc_id();
        let split_id = self.alloc_split_id();
        let target = self.focused;
        Self::split_in(&mut self.root, target, dir, new_id, split_id);
        self.focused = new_id;
        new_id
    }

    fn split_in(
        node: &mut PaneNode,
        target: PaneId,
        dir: SplitDir,
        new_id: PaneId,
        split_id: SplitId,
    ) -> bool {
        match node {
            PaneNode::Leaf { id, kind } if *id == target => {
                let existing = PaneNode::Leaf {
                    id: *id,
                    kind: *kind,
                };
                let inserted = PaneNode::Leaf {
                    id: new_id,
                    kind: PaneKind::Empty,
                };
                *node = PaneNode::Split {
                    id: split_id,
                    dir,
                    ratio: 0.5,
                    a: Box::new(existing),
                    b: Box::new(inserted),
                };
                true
            }
            PaneNode::Leaf { .. } => false,
            PaneNode::Split { a, b, .. } => {
                Self::split_in(a, target, dir, new_id, split_id)
                    || Self::split_in(b, target, dir, new_id, split_id)
            }
        }
    }

    /// Replace the kind of the focused leaf (used by the component picker).
    pub fn set_focused_kind(&mut self, kind: PaneKind) {
        Self::set_kind_in(&mut self.root, self.focused, kind);
    }

    fn set_kind_in(node: &mut PaneNode, target: PaneId, kind: PaneKind) -> bool {
        match node {
            PaneNode::Leaf { id, kind: k } if *id == target => {
                *k = kind;
                true
            }
            PaneNode::Leaf { .. } => false,
            PaneNode::Split { a, b, .. } => {
                Self::set_kind_in(a, target, kind) || Self::set_kind_in(b, target, kind)
            }
        }
    }

    /// Close the focused leaf, collapsing its parent split into the sibling.
    /// No-op if focused is the last remaining leaf.
    pub fn close_focused(&mut self) {
        let target = self.focused;
        let collapsed = Self::close_in(&mut self.root, target);
        if collapsed {
            // Pick a new focus: first leaf in iteration order.
            self.focused = Self::first_leaf_id(&self.root);
        }
    }

    fn close_in(node: &mut PaneNode, target: PaneId) -> bool {
        // We can't replace a leaf root with nothing — refuse if root is the target.
        if let PaneNode::Leaf { id, .. } = node {
            return *id == target && false; // root leaf — keep
        }
        // node is a Split here.
        let (dir, ratio) = match node {
            PaneNode::Split { dir, ratio, .. } => (*dir, *ratio),
            _ => unreachable!(),
        };
        // Direct-child match → replace this split with the other child.
        let child_match = match node {
            PaneNode::Split { a, b, .. } => {
                let a_hit = matches!(a.as_ref(), PaneNode::Leaf { id, .. } if *id == target);
                let b_hit = matches!(b.as_ref(), PaneNode::Leaf { id, .. } if *id == target);
                if a_hit {
                    Some(true)
                } else if b_hit {
                    Some(false)
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(a_was_target) = child_match {
            if let PaneNode::Split { a, b, .. } = std::mem::replace(
                node,
                PaneNode::Leaf {
                    id: PaneId(0),
                    kind: PaneKind::Empty,
                },
            ) {
                let keep = if a_was_target { *b } else { *a };
                *node = keep;
            }
            let _ = (dir, ratio);
            return true;
        }
        // Recurse.
        match node {
            PaneNode::Split { a, b, .. } => Self::close_in(a, target) || Self::close_in(b, target),
            _ => false,
        }
    }

    fn first_leaf_id(node: &PaneNode) -> PaneId {
        match node {
            PaneNode::Leaf { id, .. } => *id,
            PaneNode::Split { a, .. } => Self::first_leaf_id(a),
        }
    }

    /// Find leaf under a logical point. Used for mouse-follow focus.
    pub fn hit_test(&self, area: Rect, x: f32, y: f32) -> Option<PaneId> {
        self.layout(area)
            .into_iter()
            .find(|l| l.rect.contains(x, y))
            .map(|l| l.id)
    }

    /// Move focus geometrically (h/j/k/l). Picks the leaf whose center is
    /// closest in the requested direction.
    pub fn focus_dir(&mut self, area: Rect, dir: FocusDir) {
        let leaves = self.layout(area);
        let Some(cur) = leaves.iter().find(|l| l.id == self.focused) else {
            return;
        };
        let (cx, cy) = (cur.rect.x + cur.rect.w * 0.5, cur.rect.y + cur.rect.h * 0.5);
        let mut best: Option<(f32, PaneId)> = None;
        for l in &leaves {
            if l.id == cur.id {
                continue;
            }
            let (lx, ly) = (l.rect.x + l.rect.w * 0.5, l.rect.y + l.rect.h * 0.5);
            let (dx, dy) = (lx - cx, ly - cy);
            let aligned = match dir {
                FocusDir::Left => dx < 0.0 && dx.abs() > dy.abs(),
                FocusDir::Right => dx > 0.0 && dx.abs() > dy.abs(),
                FocusDir::Up => dy < 0.0 && dy.abs() > dx.abs(),
                FocusDir::Down => dy > 0.0 && dy.abs() > dx.abs(),
            };
            if !aligned {
                continue;
            }
            let d2 = dx * dx + dy * dy;
            if best.map_or(true, |(b, _)| d2 < b) {
                best = Some((d2, l.id));
            }
        }
        if let Some((_, id)) = best {
            self.focused = id;
        }
    }

    /// If any leaf already hosts `kind`, focus it. Otherwise split the
    /// focused leaf horizontally and set the new leaf to `kind`.
    pub fn focus_or_spawn(&mut self, kind: PaneKind) {
        if let Some(id) = Self::find_kind_id(&self.root, kind) {
            self.focused = id;
            return;
        }
        let new_id = self.split_focused(SplitDir::Horizontal);
        Self::set_kind_in(&mut self.root, new_id, kind);
    }

    fn find_kind_id(node: &PaneNode, kind: PaneKind) -> Option<PaneId> {
        match node {
            PaneNode::Leaf { id, kind: k } if *k == kind => Some(*id),
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { a, b, .. } => {
                Self::find_kind_id(a, kind).or_else(|| Self::find_kind_id(b, kind))
            }
        }
    }

    pub fn focused_kind(&self) -> PaneKind {
        Self::find_kind(&self.root, self.focused).unwrap_or(PaneKind::Empty)
    }

    /// Swap two complete leaves, preserving each pane's ID, kind, and backing
    /// resources while moving them to each other's layout slots.
    pub fn swap_leaves(&mut self, first: PaneId, second: PaneId) -> bool {
        if first == second {
            return false;
        }
        let Some(first_kind) = Self::find_kind(&self.root, first) else {
            return false;
        };
        let Some(second_kind) = Self::find_kind(&self.root, second) else {
            return false;
        };
        Self::swap_in(
            &mut self.root,
            first,
            first_kind,
            second,
            second_kind,
        );
        true
    }

    fn swap_in(
        node: &mut PaneNode,
        first: PaneId,
        first_kind: PaneKind,
        second: PaneId,
        second_kind: PaneKind,
    ) {
        match node {
            PaneNode::Leaf {
                id: leaf_id,
                kind: leaf_kind,
            } if *leaf_id == first => {
                *leaf_id = second;
                *leaf_kind = second_kind;
            }
            PaneNode::Leaf {
                id: leaf_id,
                kind: leaf_kind,
            } if *leaf_id == second => {
                *leaf_id = first;
                *leaf_kind = first_kind;
            }
            PaneNode::Leaf { .. } => {}
            PaneNode::Split { a, b, .. } => {
                Self::swap_in(a, first, first_kind, second, second_kind);
                Self::swap_in(b, first, first_kind, second, second_kind);
            }
        }
    }

    fn find_kind(node: &PaneNode, target: PaneId) -> Option<PaneKind> {
        match node {
            PaneNode::Leaf { id, kind } if *id == target => Some(*kind),
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { a, b, .. } => {
                Self::find_kind(a, target).or_else(|| Self::find_kind(b, target))
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum FocusDir {
    Left,
    Right,
    Up,
    Down,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            w: 1000.0,
            h: 800.0,
        }
    }

    #[test]
    fn single_leaf_layout_fills_area() {
        let t = PaneTree::new_with(PaneKind::Nvim);
        let leaves = t.layout(area());
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].rect, area());
    }

    #[test]
    fn split_horizontal_makes_two_leaves() {
        let mut t = PaneTree::new_with(PaneKind::Nvim);
        let new_id = t.split_focused(SplitDir::Horizontal);
        let leaves = t.layout(area());
        assert_eq!(leaves.len(), 2);
        assert_eq!(t.focused, new_id);
        assert_eq!(leaves[1].kind, PaneKind::Empty);
        assert!((leaves[0].rect.w - 500.0).abs() < 0.1);
    }

    #[test]
    fn close_collapses_split() {
        let mut t = PaneTree::new_with(PaneKind::Nvim);
        t.split_focused(SplitDir::Horizontal);
        t.close_focused();
        let leaves = t.layout(area());
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].kind, PaneKind::Nvim);
    }

    #[test]
    fn hit_test_finds_leaf() {
        let mut t = PaneTree::new_with(PaneKind::Nvim);
        t.split_focused(SplitDir::Horizontal);
        let leaves = t.layout(area());
        let left_id = leaves[0].id;
        let right_id = leaves[1].id;
        assert_eq!(t.hit_test(area(), 100.0, 100.0), Some(left_id));
        assert_eq!(t.hit_test(area(), 900.0, 100.0), Some(right_id));
    }

    #[test]
    fn focus_dir_moves_right() {
        let mut t = PaneTree::new_with(PaneKind::Nvim);
        let right = t.split_focused(SplitDir::Horizontal);
        // After split focus is on the new right leaf. Move left, then right.
        assert_eq!(t.focused, right);
        t.focus_dir(area(), FocusDir::Left);
        assert_ne!(t.focused, right);
        t.focus_dir(area(), FocusDir::Right);
        assert_eq!(t.focused, right);
    }

    #[test]
    fn swap_leaves_moves_complete_pane_identity() {
        let mut t = PaneTree::new_with(PaneKind::Nvim);
        let right = t.split_focused(SplitDir::Horizontal);
        t.set_focused_kind(PaneKind::Terminal);
        let before = t.layout(area());
        let left = before[0].id;

        assert!(t.swap_leaves(left, right));
        let after = t.layout(area());
        assert_eq!(after[0].id, right);
        assert_eq!(after[0].kind, PaneKind::Terminal);
        assert_eq!(after[1].id, left);
        assert_eq!(after[1].kind, PaneKind::Nvim);
    }
}
