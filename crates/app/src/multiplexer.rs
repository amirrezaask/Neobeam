//! In-app tiling window manager: splits, draggable title bars, drop previews.

use std::collections::{HashMap, HashSet};

use editor_surface::Spring;
use imgui::{DrawListMut, Ui};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn from_array([x, y, w, h]: [f32; 4]) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, cx: f32, cy: f32) -> bool {
        cx >= self.x && cx < self.x + self.w && cy >= self.y && cy < self.y + self.h
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WinId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    Editor,
    GitClient,
    FileList,
    GrepResults,
}

impl ViewKind {
    pub fn default_title(self) -> &'static str {
        match self {
            ViewKind::Editor => "Editor",
            ViewKind::GitClient => "Git",
            ViewKind::FileList => "Files",
            ViewKind::GrepResults => "Search Results",
        }
    }

    pub fn captures_input(self) -> bool {
        matches!(
            self,
            ViewKind::GitClient | ViewKind::FileList | ViewKind::GrepResults
        )
    }
}

#[derive(Clone, Debug)]
pub struct TilingWindow {
    pub id: WinId,
    pub view: ViewKind,
    pub title: String,
}

#[derive(Clone, Copy, Debug)]
pub enum SplitDir {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug)]
pub enum LayoutNode {
    Leaf(WinId),
    Split {
        dir: SplitDir,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
        ratio: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropZone {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

#[derive(Clone, Copy, Debug)]
pub struct DragState {
    pub win_id: WinId,
    pub start: (f32, f32),
    pub current: (f32, f32),
}

#[derive(Clone, Copy, Debug)]
pub struct DropPreview {
    pub target_win: WinId,
    pub zone: DropZone,
    pub rect: Rect,
}

#[derive(Clone, Debug)]
pub struct AnimRect {
    pub springs: [Spring; 4],
    pub target: Rect,
}

pub struct TilingManager {
    pub root: LayoutNode,
    pub windows: Vec<TilingWindow>,
    next_id: usize,
    pub drag: Option<DragState>,
    pub drop_preview: Option<DropPreview>,
    anim: HashMap<WinId, AnimRect>,
    pub title_bar_h: f32,
    pub focused: WinId,
    preview_alpha: f32,
    last_removed: Vec<WinId>,
    hidden: HashSet<WinId>,
}

const LAYOUT_ANIM_LEN: f32 = 0.25;
const FULLSCREEN_BTN_SIZE: f32 = 18.0;
const FULLSCREEN_BTN_MARGIN: f32 = 6.0;
const CLOSE_BTN_SIZE: f32 = 18.0;
const TITLE_BAR_BTN_GAP: f32 = 4.0;

impl TilingManager {
    pub fn new(title_bar_h: f32) -> Self {
        let id = WinId(0);
        Self {
            root: LayoutNode::Leaf(id),
            windows: vec![TilingWindow {
                id,
                view: ViewKind::Editor,
                title: ViewKind::Editor.default_title().into(),
            }],
            next_id: 1,
            drag: None,
            drop_preview: None,
            anim: HashMap::new(),
            title_bar_h,
            focused: id,
            preview_alpha: 0.0,
            last_removed: Vec::new(),
            hidden: HashSet::new(),
        }
    }

    pub fn is_hidden(&self, id: WinId) -> bool {
        self.hidden.contains(&id)
    }

    pub fn is_visible(&self, id: WinId) -> bool {
        self.windows.iter().any(|w| w.id == id) && !self.is_hidden(id)
    }

    pub fn find_hidden_by_view(&self, view: ViewKind) -> Option<WinId> {
        self.windows
            .iter()
            .find(|w| w.view == view && self.is_hidden(w.id))
            .map(|w| w.id)
    }

    pub fn visible_window_count(&self) -> usize {
        self.windows.iter().filter(|w| self.is_visible(w.id)).count()
    }

    pub fn can_close(&self, win_id: WinId) -> bool {
        self.windows
            .iter()
            .find(|w| w.id == win_id)
            .is_some_and(|w| w.view != ViewKind::Editor && self.is_visible(win_id))
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn focused_view(&self) -> ViewKind {
        self.windows
            .iter()
            .find(|w| w.id == self.focused && self.is_visible(w.id))
            .or_else(|| self.windows.iter().find(|w| self.is_visible(w.id)))
            .map(|w| w.view)
            .unwrap_or(ViewKind::Editor)
    }

    pub fn has_view(&self, view: ViewKind) -> bool {
        self.windows.iter().any(|w| w.view == view)
    }

    pub fn editor_win(&self) -> Option<WinId> {
        self.windows
            .iter()
            .find(|w| w.view == ViewKind::Editor && self.is_visible(w.id))
            .map(|w| w.id)
    }

    pub fn set_focus(&mut self, id: WinId) {
        if self.is_visible(id) {
            self.focused = id;
        }
    }

    pub fn open_or_focus(&mut self, view: ViewKind, area: Rect) {
        if let Some(w) = self
            .windows
            .iter()
            .find(|w| w.view == view && self.is_visible(w.id))
        {
            self.focused = w.id;
            return;
        }
        if let Some(w) = self
            .windows
            .iter()
            .find(|w| w.view == view && self.is_hidden(w.id))
        {
            self.show_window(w.id, area);
            return;
        }
        self.add_window(view, area);
    }

    /// Switch to `view` as the sole visible window, hiding all others.
    /// This gives page-switch semantics: clicking Git/Editor in the activity
    /// bar replaces whatever is on screen rather than adding a split.
    pub fn switch_to_page(&mut self, view: ViewKind, area: Rect) {
        let old_rects = self.compute_rects(area);

        // Find or create the target window.
        let win_id = if let Some(w) = self.windows.iter().find(|w| w.view == view) {
            w.id
        } else {
            let id = WinId(self.next_id);
            self.next_id += 1;
            self.windows.push(TilingWindow {
                id,
                view,
                title: view.default_title().into(),
            });
            id
        };

        // Push every other visible window into the hidden set without
        // going through hide_window (which guards against hiding the editor).
        let to_hide: Vec<WinId> = self
            .windows
            .iter()
            .filter(|w| w.id != win_id && self.is_visible(w.id))
            .map(|w| w.id)
            .collect();
        for id in to_hide {
            self.hidden.insert(id);
        }

        // Make the target the sole root leaf, removing it from hidden if needed.
        self.hidden.remove(&win_id);
        self.root = LayoutNode::Leaf(win_id);
        self.focused = win_id;

        let new_rects = self.compute_rects(area);
        self.begin_layout_anim(&old_rects, &new_rects);
    }

    pub fn hide_window(&mut self, win_id: WinId, area: Rect) {
        if !self.can_close(win_id) {
            return;
        }

        let old_rects = self.compute_rects(area);
        if remove_leaf(&mut self.root, win_id).is_none() {
            return;
        }

        self.hidden.insert(win_id);
        self.drag = None;
        self.drop_preview = None;

        if !self.is_visible(self.focused) {
            if let Some(w) = self.windows.iter().find(|w| self.is_visible(w.id)) {
                self.focused = w.id;
            }
        }

        let new_rects = self.compute_rects(area);
        self.begin_layout_anim(&old_rects, &new_rects);
        self.preview_alpha = 0.0;
    }

    pub fn show_window(&mut self, win_id: WinId, area: Rect) {
        if !self.windows.iter().any(|w| w.id == win_id) {
            return;
        }
        if self.is_visible(win_id) {
            self.focused = win_id;
            return;
        }

        let old_rects = self.compute_rects(area);
        self.hidden.remove(&win_id);

        match &self.root {
            LayoutNode::Leaf(existing) => {
                let existing_id = *existing;
                self.root = LayoutNode::Split {
                    dir: SplitDir::Horizontal,
                    first: Box::new(LayoutNode::Leaf(existing_id)),
                    second: Box::new(LayoutNode::Leaf(win_id)),
                    ratio: 0.5,
                };
            }
            LayoutNode::Split { .. } => {
                let anchor = self.visible_anchor();
                insert_at_target(
                    &mut self.root,
                    anchor,
                    LayoutNode::Leaf(win_id),
                    SplitDir::Horizontal,
                    false,
                );
            }
        }

        let new_rects = self.compute_rects(area);
        self.begin_layout_anim(&old_rects, &new_rects);
        self.focused = win_id;
    }

    pub fn add_view_window(&mut self, view: ViewKind, area: Rect) -> WinId {
        self.add_window(view, area)
    }

    pub fn take_removed_windows(&mut self) -> Vec<WinId> {
        std::mem::take(&mut self.last_removed)
    }

    pub fn add_window(&mut self, view: ViewKind, area: Rect) -> WinId {
        if view == ViewKind::Editor && self.has_view(ViewKind::Editor) {
            let id = self.editor_win().expect("editor window");
            self.focused = id;
            return id;
        }

        let id = WinId(self.next_id);
        self.next_id += 1;
        self.windows.push(TilingWindow {
            id,
            view,
            title: view.default_title().into(),
        });

        let old_rects = self.compute_rects(area);

        match &self.root {
            LayoutNode::Leaf(existing) => {
                let existing_id = *existing;
                self.root = LayoutNode::Split {
                    dir: SplitDir::Horizontal,
                    first: Box::new(LayoutNode::Leaf(existing_id)),
                    second: Box::new(LayoutNode::Leaf(id)),
                    ratio: 0.5,
                };
            }
            LayoutNode::Split { .. } => {
                let anchor = self.visible_anchor();
                self.insert_split_at(anchor, id, SplitDir::Horizontal, false);
            }
        }

        let new_rects = self.compute_rects(area);
        self.begin_layout_anim(&old_rects, &new_rects);
        self.focused = id;
        id
    }

    pub fn compute_rects(&self, area: Rect) -> HashMap<WinId, Rect> {
        let mut out = HashMap::new();
        self.compute_node(&self.root, area, &mut out);
        out
    }

    fn compute_node(&self, node: &LayoutNode, area: Rect, out: &mut HashMap<WinId, Rect>) {
        match node {
            LayoutNode::Leaf(id) => {
                out.insert(*id, area);
            }
            LayoutNode::Split {
                dir,
                first,
                second,
                ratio,
            } => {
                match dir {
                    SplitDir::Horizontal => {
                        let w1 = area.w * ratio;
                        let w2 = area.w - w1;
                        self.compute_node(
                            first,
                            Rect {
                                x: area.x,
                                y: area.y,
                                w: w1,
                                h: area.h,
                            },
                            out,
                        );
                        self.compute_node(
                            second,
                            Rect {
                                x: area.x + w1,
                                y: area.y,
                                w: w2,
                                h: area.h,
                            },
                            out,
                        );
                    }
                    SplitDir::Vertical => {
                        let h1 = area.h * ratio;
                        let h2 = area.h - h1;
                        self.compute_node(
                            first,
                            Rect {
                                x: area.x,
                                y: area.y,
                                w: area.w,
                                h: h1,
                            },
                            out,
                        );
                        self.compute_node(
                            second,
                            Rect {
                                x: area.x,
                                y: area.y + h1,
                                w: area.w,
                                h: h2,
                            },
                            out,
                        );
                    }
                }
            }
        }
    }

    pub fn visual_rect(&self, id: WinId, computed: &HashMap<WinId, Rect>) -> Rect {
        let target = computed
            .get(&id)
            .copied()
            .unwrap_or(Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 });
        if let Some(anim) = self.anim.get(&id) {
            Rect {
                x: target.x + anim.springs[0].position,
                y: target.y + anim.springs[1].position,
                w: (target.w + anim.springs[2].position).max(1.0),
                h: (target.h + anim.springs[3].position).max(1.0),
            }
        } else {
            target
        }
    }

    pub fn title_bar_rect(&self, win_rect: Rect) -> Rect {
        Rect {
            x: win_rect.x,
            y: win_rect.y,
            w: win_rect.w,
            h: self.title_bar_h.min(win_rect.h),
        }
    }

    pub fn content_rect(&self, win_rect: Rect) -> Rect {
        let tb = self.title_bar_h.min(win_rect.h);
        Rect {
            x: win_rect.x,
            y: win_rect.y + tb,
            w: win_rect.w,
            h: (win_rect.h - tb).max(1.0),
        }
    }

    pub fn update_anim(&mut self, dt: f32) -> bool {
        let mut animating = false;
        for anim in self.anim.values_mut() {
            for spring in &mut anim.springs {
                if spring.update(dt, LAYOUT_ANIM_LEN) {
                    animating = true;
                }
            }
        }

        let target_alpha = if self.drop_preview.is_some() { 1.0 } else { 0.0 };
        let rate = 8.0;
        if self.preview_alpha < target_alpha {
            self.preview_alpha = (self.preview_alpha + rate * dt).min(target_alpha);
        } else if self.preview_alpha > target_alpha {
            self.preview_alpha = (self.preview_alpha - rate * dt).max(target_alpha);
        }
        if (self.preview_alpha - target_alpha).abs() > 0.01 {
            animating = true;
        }

        animating
    }

    pub fn preview_alpha(&self) -> f32 {
        self.preview_alpha
    }

    pub fn begin_drag(&mut self, win_id: WinId, cursor: (f32, f32)) {
        self.drag = Some(DragState {
            win_id,
            start: cursor,
            current: cursor,
        });
        self.drop_preview = None;
    }

    pub fn update_drag(&mut self, cursor: (f32, f32), area: Rect) {
        let drag_id = self.drag.as_ref().map(|d| d.win_id);
        let Some(drag_id) = drag_id else {
            return;
        };
        if let Some(drag) = &mut self.drag {
            drag.current = cursor;
        }

        let rects = self.compute_rects(area);
        if let Some((target, zone)) = self.find_drop_zone(cursor, &rects, drag_id) {
            let target_rect = self.visual_rect(target, &rects);
            let preview_rect = preview_rect_for_zone(target_rect, zone);
            self.drop_preview = Some(DropPreview {
                target_win: target,
                zone,
                rect: preview_rect,
            });
        } else {
            self.drop_preview = None;
        }
    }

    pub fn cancel_drag(&mut self) {
        self.drag = None;
        self.drop_preview = None;
    }

    pub fn commit_drop(&mut self, area: Rect) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        let Some(preview) = self.drop_preview.take() else {
            return;
        };

        if drag.win_id == preview.target_win {
            self.preview_alpha = 0.0;
            return;
        }

        let old_rects = self.compute_rects(area);

        match preview.zone {
            DropZone::Center => self.swap_views(drag.win_id, preview.target_win),
            zone => {
                let dir = match zone {
                    DropZone::Left | DropZone::Right => SplitDir::Horizontal,
                    DropZone::Top | DropZone::Bottom => SplitDir::Vertical,
                    DropZone::Center => unreachable!(),
                };
                let new_first = matches!(zone, DropZone::Left | DropZone::Top);
                if let Some(extracted) = remove_leaf(&mut self.root, drag.win_id) {
                    insert_at_target(
                        &mut self.root,
                        preview.target_win,
                        extracted,
                        dir,
                        new_first,
                    );
                }
            }
        }

        let new_rects = self.compute_rects(area);
        self.begin_layout_anim(&old_rects, &new_rects);
        self.preview_alpha = 0.0;
    }

    pub fn can_fullscreen(&self) -> bool {
        self.visible_window_count() > 1
    }

    pub fn fullscreen(&mut self, win_id: WinId, area: Rect) {
        if !self.windows.iter().any(|w| w.id == win_id) {
            return;
        }
        if self.windows.len() <= 1 && matches!(self.root, LayoutNode::Leaf(id) if id == win_id) {
            return;
        }

        let old_rects = self.compute_rects(area);

        self.last_removed = self
            .windows
            .iter()
            .filter(|w| w.id != win_id)
            .map(|w| w.id)
            .collect();

        self.windows.retain(|w| w.id == win_id);
        self.hidden.retain(|id| *id == win_id);
        self.root = LayoutNode::Leaf(win_id);
        self.focused = win_id;
        self.drag = None;
        self.drop_preview = None;
        self.anim.retain(|id, _| *id == win_id);

        let new_rects = self.compute_rects(area);
        self.begin_layout_anim(&old_rects, &new_rects);
        self.preview_alpha = 0.0;
    }

    pub fn hit_close_button(
        &self,
        cursor: (f32, f32),
        rects: &HashMap<WinId, Rect>,
    ) -> Option<WinId> {
        let (cx, cy) = cursor;
        for win in self.visible_windows().rev() {
            let tb = self.title_bar_rect(self.visual_rect(win.id, rects));
            if !self.can_close(win.id) {
                continue;
            }
            let btn = close_button_rect(tb, self.can_fullscreen());
            if btn.contains(cx, cy) {
                return Some(win.id);
            }
        }
        None
    }

    pub fn hit_fullscreen_button(
        &self,
        cursor: (f32, f32),
        rects: &HashMap<WinId, Rect>,
    ) -> Option<WinId> {
        if !self.can_fullscreen() {
            return None;
        }
        let (cx, cy) = cursor;
        for win in self.visible_windows().rev() {
            let tb = self.title_bar_rect(self.visual_rect(win.id, rects));
            let btn = fullscreen_button_rect(tb);
            if btn.contains(cx, cy) {
                return Some(win.id);
            }
        }
        None
    }

    pub fn hit_title_bar(
        &self,
        cursor: (f32, f32),
        rects: &HashMap<WinId, Rect>,
    ) -> Option<WinId> {
        let (cx, cy) = cursor;
        let show_fullscreen = self.can_fullscreen();
        for win in self.visible_windows().rev() {
            let r = self.visual_rect(win.id, rects);
            let tb = self.title_bar_rect(r);
            if show_fullscreen {
                let btn = fullscreen_button_rect(tb);
                if btn.contains(cx, cy) {
                    continue;
                }
            }
            if self.can_close(win.id) {
                let btn = close_button_rect(tb, show_fullscreen);
                if btn.contains(cx, cy) {
                    continue;
                }
            }
            if cx >= tb.x && cx < tb.x + tb.w && cy >= tb.y && cy < tb.y + tb.h {
                return Some(win.id);
            }
        }
        None
    }

    pub fn hit_window_content(
        &self,
        cursor: (f32, f32),
        rects: &HashMap<WinId, Rect>,
    ) -> Option<WinId> {
        let (cx, cy) = cursor;
        for win in self.visible_windows().rev() {
            let r = self.content_rect(self.visual_rect(win.id, rects));
            if r.contains(cx, cy) {
                return Some(win.id);
            }
        }
        None
    }

    pub fn cursor_in_any_title_bar(
        &self,
        cursor: (f32, f32),
        rects: &HashMap<WinId, Rect>,
    ) -> bool {
        self.hit_title_bar(cursor, rects).is_some()
    }

    pub fn find_drop_zone(
        &self,
        cursor: (f32, f32),
        rects: &HashMap<WinId, Rect>,
        drag_id: WinId,
    ) -> Option<(WinId, DropZone)> {
        let (cx, cy) = cursor;
        for win in self.visible_windows() {
            if win.id == drag_id {
                continue;
            }
            let r = self.visual_rect(win.id, rects);
            if !r.contains(cx, cy) {
                continue;
            }
            return Some((win.id, classify_zone(cx, cy, r)));
        }
        None
    }

    pub fn draw_chrome(
        &self,
        ui: &Ui,
        rects: &HashMap<WinId, Rect>,
        editor_winbar: Option<(&str, &str)>,
    ) {
        let draw = ui.get_background_draw_list();

        let show_fullscreen = self.can_fullscreen();
        for win in self.visible_windows() {
            let visual = self.visual_rect(win.id, rects);
            let tb = self.title_bar_rect(visual);
            let focused = win.id == self.focused;
            let title = if win.view == ViewKind::Editor {
                match editor_winbar {
                    Some((file, project)) => {
                        let file_w = if project.is_empty() {
                            0.0
                        } else {
                            ui.calc_text_size(file)[0]
                        };
                        TitleBarText::Winbar {
                            file,
                            project,
                            file_w,
                        }
                    }
                    None => TitleBarText::Plain(&win.title),
                }
            } else {
                TitleBarText::Plain(&win.title)
            };
            draw_title_bar(
                &draw,
                tb,
                title,
                focused,
                self.is_dragging(),
                self.can_close(win.id),
                show_fullscreen,
            );
        }

        if let Some(preview) = &self.drop_preview {
            let alpha = self.preview_alpha;
            if alpha > 0.01 {
                draw_drop_preview(&draw, preview.rect, alpha);
            }
        }
    }

    fn begin_layout_anim(&mut self, old_rects: &HashMap<WinId, Rect>, new_rects: &HashMap<WinId, Rect>) {
        for (id, new_rect) in new_rects {
            let old = old_rects.get(id).copied().unwrap_or(*new_rect);
            let entry = self.anim.entry(*id).or_insert_with(|| AnimRect {
                springs: [Spring::new(), Spring::new(), Spring::new(), Spring::new()],
                target: *new_rect,
            });
            entry.target = *new_rect;
            entry.springs[0].position += old.x - new_rect.x;
            entry.springs[1].position += old.y - new_rect.y;
            entry.springs[2].position += old.w - new_rect.w;
            entry.springs[3].position += old.h - new_rect.h;
        }
    }

    fn swap_views(&mut self, a: WinId, b: WinId) {
        let ia = self.windows.iter().position(|w| w.id == a);
        let ib = self.windows.iter().position(|w| w.id == b);
        if let (Some(ia), Some(ib)) = (ia, ib) {
            if ia == ib {
                return;
            }
            let view_b = self.windows[ib].view;
            let title_b = self.windows[ib].title.clone();
            self.windows[ib].view = self.windows[ia].view;
            self.windows[ib].title = self.windows[ia].title.clone();
            self.windows[ia].view = view_b;
            self.windows[ia].title = title_b;
        }
    }

    fn visible_windows(&self) -> impl DoubleEndedIterator<Item = &TilingWindow> {
        self.windows.iter().filter(|w| self.is_visible(w.id))
    }

    fn visible_anchor(&self) -> WinId {
        if self.is_visible(self.focused) {
            self.focused
        } else {
            self.windows
                .iter()
                .find(|w| self.is_visible(w.id))
                .map(|w| w.id)
                .unwrap_or(self.focused)
        }
    }

    fn insert_split_at(&mut self, target: WinId, new_id: WinId, dir: SplitDir, new_first: bool) {
        insert_at_target(
            &mut self.root,
            target,
            LayoutNode::Leaf(new_id),
            dir,
            new_first,
        );
    }
}

fn classify_zone(cx: f32, cy: f32, r: Rect) -> DropZone {
    if cx < r.x + r.w * 0.25 {
        DropZone::Left
    } else if cx > r.x + r.w * 0.75 {
        DropZone::Right
    } else if cy < r.y + r.h * 0.25 {
        DropZone::Top
    } else if cy > r.y + r.h * 0.75 {
        DropZone::Bottom
    } else {
        DropZone::Center
    }
}

pub fn preview_rect_for_zone(target: Rect, zone: DropZone) -> Rect {
    match zone {
        DropZone::Left => Rect {
            x: target.x,
            y: target.y,
            w: target.w * 0.5,
            h: target.h,
        },
        DropZone::Right => Rect {
            x: target.x + target.w * 0.5,
            y: target.y,
            w: target.w * 0.5,
            h: target.h,
        },
        DropZone::Top => Rect {
            x: target.x,
            y: target.y,
            w: target.w,
            h: target.h * 0.5,
        },
        DropZone::Bottom => Rect {
            x: target.x,
            y: target.y + target.h * 0.5,
            w: target.w,
            h: target.h * 0.5,
        },
        DropZone::Center => target,
    }
}

fn is_leaf_with_id(node: &LayoutNode, id: WinId) -> bool {
    matches!(node, LayoutNode::Leaf(wid) if *wid == id)
}

fn remove_leaf(node: &mut LayoutNode, id: WinId) -> Option<LayoutNode> {
    match node {
        LayoutNode::Leaf(_) => None,
        LayoutNode::Split { first, second, .. } => {
            if is_leaf_with_id(first, id) {
                let extracted = match first.as_ref() {
                    LayoutNode::Leaf(w) => LayoutNode::Leaf(*w),
                    _ => unreachable!(),
                };
                *node = *second.clone();
                return Some(extracted);
            }
            if let Some(extracted) = remove_leaf(first, id) {
                return Some(extracted);
            }
            if is_leaf_with_id(second, id) {
                let extracted = match second.as_ref() {
                    LayoutNode::Leaf(w) => LayoutNode::Leaf(*w),
                    _ => unreachable!(),
                };
                *node = *first.clone();
                return Some(extracted);
            }
            remove_leaf(second, id)
        }
    }
}

fn insert_at_target(
    node: &mut LayoutNode,
    target: WinId,
    new_leaf: LayoutNode,
    dir: SplitDir,
    new_first: bool,
) -> bool {
    match node {
        LayoutNode::Leaf(wid) if *wid == target => {
            let existing = std::mem::replace(node, LayoutNode::Leaf(WinId(usize::MAX)));
            let (first, second) = if new_first {
                (new_leaf, existing)
            } else {
                (existing, new_leaf)
            };
            *node = LayoutNode::Split {
                dir,
                first: Box::new(first),
                second: Box::new(second),
                ratio: 0.5,
            };
            true
        }
        LayoutNode::Leaf(_) => false,
        LayoutNode::Split { first, second, .. } => {
            insert_at_target(first, target, new_leaf.clone(), dir, new_first)
                || insert_at_target(second, target, new_leaf, dir, new_first)
        }
    }
}

fn fullscreen_button_rect(title_bar: Rect) -> Rect {
    let size = FULLSCREEN_BTN_SIZE.min(title_bar.h - 4.0);
    Rect {
        x: title_bar.x + title_bar.w - FULLSCREEN_BTN_MARGIN - size,
        y: title_bar.y + (title_bar.h - size) * 0.5,
        w: size,
        h: size,
    }
}

fn close_button_rect(title_bar: Rect, show_fullscreen: bool) -> Rect {
    let size = CLOSE_BTN_SIZE.min(title_bar.h - 4.0);
    let mut right = title_bar.x + title_bar.w - FULLSCREEN_BTN_MARGIN;
    if show_fullscreen {
        right -= FULLSCREEN_BTN_SIZE + TITLE_BAR_BTN_GAP;
    }
    Rect {
        x: right - size,
        y: title_bar.y + (title_bar.h - size) * 0.5,
        w: size,
        h: size,
    }
}

enum TitleBarText<'a> {
    Plain(&'a str),
    Winbar {
        file: &'a str,
        project: &'a str,
        file_w: f32,
    },
}

fn draw_title_bar(
    draw: &DrawListMut<'_>,
    rect: Rect,
    title: TitleBarText<'_>,
    focused: bool,
    dragging: bool,
    show_close: bool,
    show_fullscreen: bool,
) {
    let bg = if dragging {
        [0.22, 0.24, 0.30, 1.0]
    } else if focused {
        [0.18, 0.20, 0.26, 1.0]
    } else {
        [0.14, 0.15, 0.19, 1.0]
    };
    let min = [rect.x, rect.y];
    let max = [rect.x + rect.w, rect.y + rect.h];
    draw.add_rect(min, max, bg).filled(true).rounding(0.0).build();

    let sep_y = rect.y + rect.h - 1.0;
    draw.add_rect(
        [rect.x, sep_y],
        [rect.x + rect.w, sep_y + 1.0],
        [0.35, 0.38, 0.45, 1.0],
    )
    .filled(true)
    .rounding(0.0)
    .build();

    let text_color = if focused {
        [0.92, 0.93, 0.96, 1.0]
    } else {
        [0.65, 0.67, 0.72, 1.0]
    };
    let muted_color = if focused {
        [0.55, 0.57, 0.62, 1.0]
    } else {
        [0.45, 0.47, 0.52, 1.0]
    };
    let text_x = rect.x + 10.0;
    let text_y = rect.y + (rect.h - 14.0) * 0.5;
    match title {
        TitleBarText::Plain(label) => {
            draw.add_text([text_x, text_y], text_color, label);
        }
        TitleBarText::Winbar {
            file,
            project,
            file_w,
        } => {
            draw.add_text([text_x, text_y], text_color, file);
            if !project.is_empty() {
                let project_x = text_x + file_w + 16.0;
                draw.add_text([project_x, text_y], muted_color, project);
            }
        }
    }

    if show_close {
        draw_close_button(draw, close_button_rect(rect, show_fullscreen), focused);
    }
    if show_fullscreen {
        draw_fullscreen_button(draw, fullscreen_button_rect(rect), focused);
    }
}

fn draw_close_button(draw: &DrawListMut<'_>, btn: Rect, focused: bool) {
    let hover_bg = [0.32, 0.18, 0.18, 1.0];
    let icon_color = if focused {
        [0.92, 0.55, 0.55, 1.0]
    } else {
        [0.62, 0.45, 0.45, 1.0]
    };
    draw.add_rect([btn.x, btn.y], [btn.x + btn.w, btn.y + btn.h], hover_bg)
        .filled(true)
        .rounding(3.0)
        .build();

    let cx = btn.x + btn.w * 0.5;
    let cy = btn.y + btn.h * 0.5;
    let half = btn.w * 0.18;
    draw.add_line([cx - half, cy - half], [cx + half, cy + half], icon_color)
        .thickness(1.5)
        .build();
    draw.add_line([cx + half, cy - half], [cx - half, cy + half], icon_color)
        .thickness(1.5)
        .build();
}

fn draw_fullscreen_button(draw: &DrawListMut<'_>, btn: Rect, focused: bool) {
    let hover_bg = [0.28, 0.30, 0.38, 1.0];
    let icon_color = if focused {
        [0.85, 0.87, 0.92, 1.0]
    } else {
        [0.55, 0.57, 0.62, 1.0]
    };
    draw.add_rect([btn.x, btn.y], [btn.x + btn.w, btn.y + btn.h], hover_bg)
        .filled(true)
        .rounding(3.0)
        .build();

    let pad = 4.0;
    let inner = Rect {
        x: btn.x + pad,
        y: btn.y + pad,
        w: btn.w - pad * 2.0,
        h: btn.h - pad * 2.0,
    };
    draw.add_rect(
        [inner.x, inner.y],
        [inner.x + inner.w, inner.y + inner.h],
        icon_color,
    )
    .filled(false)
    .thickness(1.5)
    .rounding(1.0)
    .build();
}

fn draw_drop_preview(draw: &DrawListMut<'_>, rect: Rect, alpha: f32) {
    let fill = [0.20, 0.45, 0.85, 0.25 * alpha];
    let border = [0.35, 0.60, 1.0, 0.85 * alpha];
    let min = [rect.x, rect.y];
    let max = [rect.x + rect.w, rect.y + rect.h];
    draw.add_rect(min, max, fill)
        .filled(true)
        .rounding(2.0)
        .build();
    draw.add_rect(min, max, border)
        .filled(false)
        .thickness(2.0)
        .rounding(2.0)
        .build();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_single_editor_window() {
        let mgr = TilingManager::new(28.0);
        assert_eq!(mgr.windows.len(), 1);
        assert_eq!(mgr.windows[0].view, ViewKind::Editor);
    }

    #[test]
    fn compute_rects_single_fills_area() {
        let mgr = TilingManager::new(28.0);
        let area = Rect {
            x: 0.0,
            y: 32.0,
            w: 800.0,
            h: 600.0,
        };
        let rects = mgr.compute_rects(area);
        assert_eq!(rects.len(), 1);
        let r = rects[&WinId(0)];
        assert!((r.w - 800.0).abs() < 0.01);
        assert!((r.h - 600.0).abs() < 0.01);
    }

    #[test]
    fn add_git_window_splits() {
        let mut mgr = TilingManager::new(28.0);
        let area = Rect {
            x: 0.0,
            y: 32.0,
            w: 800.0,
            h: 600.0,
        };
        mgr.add_window(ViewKind::GitClient, area);
        assert_eq!(mgr.windows.len(), 2);
        let rects = mgr.compute_rects(area);
        assert_eq!(rects.len(), 2);
    }

    #[test]
    fn hide_window_preserves_entry() {
        let mut mgr = TilingManager::new(28.0);
        let area = Rect {
            x: 0.0,
            y: 32.0,
            w: 800.0,
            h: 600.0,
        };
        mgr.add_window(ViewKind::GitClient, area);
        let git_id = mgr
            .windows
            .iter()
            .find(|w| w.view == ViewKind::GitClient)
            .map(|w| w.id)
            .unwrap();

        mgr.hide_window(git_id, area);

        assert!(mgr.is_hidden(git_id));
        assert_eq!(mgr.windows.len(), 2);
        assert_eq!(mgr.visible_window_count(), 1);
        assert!(mgr.editor_win().is_some());
    }

    #[test]
    fn show_window_restores_hidden() {
        let mut mgr = TilingManager::new(28.0);
        let area = Rect {
            x: 0.0,
            y: 32.0,
            w: 800.0,
            h: 600.0,
        };
        mgr.add_window(ViewKind::GitClient, area);
        let git_id = mgr
            .windows
            .iter()
            .find(|w| w.view == ViewKind::GitClient)
            .map(|w| w.id)
            .unwrap();
        mgr.hide_window(git_id, area);
        mgr.show_window(git_id, area);

        assert!(!mgr.is_hidden(git_id));
        assert_eq!(mgr.visible_window_count(), 2);
    }

    #[test]
    fn fullscreen_collapses_split() {
        let mut mgr = TilingManager::new(28.0);
        let area = Rect {
            x: 0.0,
            y: 32.0,
            w: 800.0,
            h: 600.0,
        };
        mgr.add_window(ViewKind::GitClient, area);
        assert_eq!(mgr.windows.len(), 2);

        let git_id = mgr
            .windows
            .iter()
            .find(|w| w.view == ViewKind::GitClient)
            .map(|w| w.id)
            .unwrap();
        mgr.fullscreen(git_id, area);

        assert_eq!(mgr.windows.len(), 1);
        assert_eq!(mgr.windows[0].id, git_id);
        assert!(matches!(mgr.root, LayoutNode::Leaf(id) if id == git_id));
        let rects = mgr.compute_rects(area);
        assert_eq!(rects.len(), 1);
        assert!((rects[&git_id].w - 800.0).abs() < 0.01);
    }

    #[test]
    fn drop_zone_left() {
        let r = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        };
        assert_eq!(classify_zone(10.0, 50.0, r), DropZone::Left);
        assert_eq!(classify_zone(90.0, 50.0, r), DropZone::Right);
        assert_eq!(classify_zone(50.0, 10.0, r), DropZone::Top);
        assert_eq!(classify_zone(50.0, 90.0, r), DropZone::Bottom);
        assert_eq!(classify_zone(50.0, 50.0, r), DropZone::Center);
    }
}
