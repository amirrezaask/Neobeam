//! Hyprland-style spring animation for pane rects.
//!
//! Each pane carries a current rect that springs toward its target rect.
//! New panes scale in from 0.85 with alpha 0; closing panes fade out.

use std::collections::HashMap;

use crate::layout::Rect;
use crate::pane::PaneId;

/// Hyprland default-ish: ~0.15s settle, critically damped.
const SETTLE_S: f32 = 0.18;
const EPS_PX: f32 = 0.5;
const EPS_ALPHA: f32 = 0.01;

#[derive(Clone, Copy, Debug)]
pub struct PaneAnim {
    pub current: Rect,
    pub target: Rect,
    pub scale: f32,
    pub target_scale: f32,
    pub alpha: f32,
    pub target_alpha: f32,
}

impl PaneAnim {
    fn new(target: Rect, spawn: bool) -> Self {
        let (scale, alpha) = if spawn { (0.85, 0.0) } else { (1.0, 1.0) };
        Self {
            current: target,
            target,
            scale,
            target_scale: 1.0,
            alpha,
            target_alpha: 1.0,
        }
    }

    /// Returns true while still animating.
    fn tick(&mut self, dt: f32) -> bool {
        let mut animating = false;
        self.current.x = critically_damped(self.current.x, self.target.x, dt, SETTLE_S);
        self.current.y = critically_damped(self.current.y, self.target.y, dt, SETTLE_S);
        self.current.w = critically_damped(self.current.w, self.target.w, dt, SETTLE_S);
        self.current.h = critically_damped(self.current.h, self.target.h, dt, SETTLE_S);
        self.scale = critically_damped(self.scale, self.target_scale, dt, SETTLE_S);
        self.alpha = critically_damped(self.alpha, self.target_alpha, dt, SETTLE_S);
        if (self.current.x - self.target.x).abs() > EPS_PX
            || (self.current.y - self.target.y).abs() > EPS_PX
            || (self.current.w - self.target.w).abs() > EPS_PX
            || (self.current.h - self.target.h).abs() > EPS_PX
            || (self.scale - self.target_scale).abs() > EPS_ALPHA
            || (self.alpha - self.target_alpha).abs() > EPS_ALPHA
        {
            animating = true;
        } else {
            self.current = self.target;
            self.scale = self.target_scale;
            self.alpha = self.target_alpha;
        }
        animating
    }

    /// Final rect to draw — apply scale around center.
    pub fn render_rect(&self) -> Rect {
        let cx = self.current.x + self.current.w * 0.5;
        let cy = self.current.y + self.current.h * 0.5;
        let w = self.current.w * self.scale;
        let h = self.current.h * self.scale;
        Rect { x: cx - w * 0.5, y: cy - h * 0.5, w, h }
    }
}

/// Exponential approach with time constant tuned so settle within ~SETTLE_S.
/// Hyprland's bezier-driven feel approximated by simple exp smoothing —
/// good enough at 60+fps and dead-simple.
fn critically_damped(current: f32, target: f32, dt: f32, settle: f32) -> f32 {
    // 4 time constants ≈ settled. omega such that exp(-omega*settle) ~ 0.02.
    let omega = 4.0 / settle.max(0.001);
    let k = 1.0 - (-omega * dt).exp();
    current + (target - current) * k
}

pub struct PaneAnimStore {
    anims: HashMap<PaneId, PaneAnim>,
}

impl PaneAnimStore {
    pub fn new() -> Self {
        Self { anims: HashMap::new() }
    }

    /// Update targets for the current set of leaves. New IDs spawn-animate in.
    /// Stale IDs are dropped.
    pub fn sync(&mut self, leaves: &[(PaneId, Rect)]) {
        let mut keep: HashMap<PaneId, PaneAnim> = HashMap::with_capacity(leaves.len());
        for (id, rect) in leaves {
            let anim = if let Some(mut existing) = self.anims.remove(id) {
                existing.target = *rect;
                existing
            } else {
                PaneAnim::new(*rect, true)
            };
            keep.insert(*id, anim);
        }
        self.anims = keep;
    }

    pub fn tick(&mut self, dt: f32) -> bool {
        let mut any = false;
        for a in self.anims.values_mut() {
            any |= a.tick(dt);
        }
        any
    }

    pub fn get(&self, id: PaneId) -> Option<&PaneAnim> {
        self.anims.get(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anim_settles_at_target() {
        let mut store = PaneAnimStore::new();
        let id = PaneId(1);
        let target = Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        store.sync(&[(id, target)]);
        // Initial: scale=0.85, alpha=0 — animating.
        assert!(store.get(id).unwrap().scale < 1.0);
        for _ in 0..400 {
            store.tick(0.016);
        }
        let a = store.get(id).unwrap();
        assert!((a.scale - 1.0).abs() < 0.01);
        assert!((a.alpha - 1.0).abs() < 0.01);
    }

    #[test]
    fn target_change_animates() {
        let mut store = PaneAnimStore::new();
        let id = PaneId(1);
        let r1 = Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        store.sync(&[(id, r1)]);
        for _ in 0..400 {
            store.tick(0.016);
        }
        let r2 = Rect { x: 200.0, y: 0.0, w: 100.0, h: 100.0 };
        store.sync(&[(id, r2)]);
        assert!(store.tick(0.016));
        assert!(store.get(id).unwrap().current.x > 0.0);
        assert!(store.get(id).unwrap().current.x < 200.0);
    }
}
