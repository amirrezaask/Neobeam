//! Frame-delta animation engine. Logical state (the protocol cursor) is kept
//! separate from the rendered/animated state (AGENT_RUST_PORT.md §6).

use std::collections::HashMap;

use nvim_core::grid::GridStateStore;
use nvim_core::protocol::CursorShape;

use crate::color::{resolve_cursor, Rgba};

const DT_MAX: f32 = 0.05;
const TRAIL_CAP: usize = 12;
const TRAIL_LIFE: f32 = 0.35;
const FLOAT_REUSE_ROW_TOLERANCE: i32 = 2;
const FLOAT_REUSE_COL_TOLERANCE: i32 = 12;

#[derive(Clone, Copy, Debug)]
pub struct AnimationConfig {
    pub enable_cursor_animation: bool,
    pub enable_cursor_trail: bool,
    pub enable_cursor_glow: bool,
    pub enable_cursor_squash_stretch: bool,
    pub enable_smooth_scroll: bool,
    pub enable_flashes: bool,
    pub enable_power_mode: bool,
    pub enable_float_animation: bool,

    pub cursor_speed: f32,
    pub cursor_snap_epsilon: f32,
    pub cursor_stretch_strength: f32,
    pub cursor_max_stretch: f32,
    pub cursor_glow_layers: i32,
    pub cursor_glow_radius: f32,
    pub cursor_glow_alpha: f32,
    pub scroll_smooth_time: f32,
    pub flash_duration: f32,
    pub particles_per_key: i32,
    pub particle_lifetime: f32,
    pub screen_shake_decay: f32,
    pub float_fade_speed: f32,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        AnimationConfig {
            enable_cursor_animation: true,
            enable_cursor_trail: true,
            enable_cursor_glow: true,
            enable_cursor_squash_stretch: true,
            enable_smooth_scroll: true,
            enable_flashes: true,
            enable_power_mode: false,
            enable_float_animation: true,
            cursor_speed: 30.0,
            cursor_snap_epsilon: 0.5,
            cursor_stretch_strength: 1.0,
            cursor_max_stretch: 3.0,
            cursor_glow_layers: 20,
            cursor_glow_radius: 20.0,
            cursor_glow_alpha: 0.20,
            scroll_smooth_time: 0.10,
            flash_duration: 0.35,
            particles_per_key: 30,
            particle_lifetime: 0.6,
            screen_shake_decay: 10.0,
            float_fade_speed: 18.0,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Copy)]
pub struct TrailSample {
    pub rect: Rect,
    pub age: f32,
}

#[derive(Clone, Copy)]
pub struct Flash {
    pub rect: Rect,
    pub age: f32,
}

#[derive(Clone, Copy)]
pub struct Particle {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub age: f32,
    pub color: Rgba,
}

#[derive(Clone, Copy)]
struct FloatAnimState {
    opacity: f32,
    target: f32,
    row: i32,
    col: i32,
}

pub struct AnimationState {
    pub cfg: AnimationConfig,

    target: Rect,
    render: Rect,
    prev: Rect,
    pub display: Rect,

    pub fill_color: Rgba,
    pub glyph_color: Rgba,

    pub trail: Vec<TrailSample>,
    pub flashes: Vec<Flash>,
    pub particles: Vec<Particle>,
    pub scroll: HashMap<i64, f32>,
    float_states: HashMap<i64, FloatAnimState>,

    pub shake: f32,
    shake_phase: u32,

    last_cell: (u32, u32),
    last_mode: usize,
    blink_t: f32,
    initialized: bool,
    rng: u32,
}

impl AnimationState {
    pub fn new(cfg: AnimationConfig) -> Self {
        AnimationState {
            cfg,
            target: Rect::default(),
            render: Rect::default(),
            prev: Rect::default(),
            display: Rect::default(),
            fill_color: [1.0; 4],
            glyph_color: [0.0, 0.0, 0.0, 1.0],
            trail: Vec::new(),
            flashes: Vec::new(),
            particles: Vec::new(),
            scroll: HashMap::new(),
            float_states: HashMap::new(),
            shake: 0.0,
            shake_phase: 0,
            last_cell: (u32::MAX, u32::MAX),
            last_mode: usize::MAX,
            blink_t: 0.0,
            initialized: false,
            rng: 0x1234_5678,
        }
    }

    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32 / u32::MAX as f32).fract()
    }

    pub fn notify_keystroke(&mut self) {
        if !self.cfg.enable_power_mode {
            return;
        }
        self.shake = (self.shake + 2.5).min(8.0);
        let cx = self.display.x + self.display.w * 0.5;
        let cy = self.display.y + self.display.h * 0.5;
        for _ in 0..self.cfg.particles_per_key {
            let ang = self.rand() * std::f32::consts::TAU;
            let speed = 60.0 + self.rand() * 180.0;
            self.particles.push(Particle {
                x: cx,
                y: cy,
                vx: ang.cos() * speed,
                vy: ang.sin() * speed,
                age: 0.0,
                color: self.fill_color,
            });
        }
    }

    /// Seed smooth scroll once per viewport change (§6.4).
    pub fn seed_scroll(&mut self, grid_id: i64, lines_moved: i64, cell_h: f32, visible_rows: u32) {
        if !self.cfg.enable_smooth_scroll || lines_moved == 0 {
            return;
        }
        let cap = visible_rows.max(1).min(40) as i64;
        if lines_moved.abs() > cap {
            self.scroll.remove(&grid_id);
            return;
        }
        self.scroll.insert(grid_id, -(lines_moved as f32) * cell_h);
    }

    /// Track active floats; inherit opacity from nearby fading-out popups (§4.9).
    pub fn sync_floats(&mut self, active: &HashMap<i64, (i32, i32)>) {
        if !self.cfg.enable_float_animation {
            self.float_states.clear();
            return;
        }

        for (&grid_id, &(row, col)) in active {
            if let Some(state) = self.float_states.get_mut(&grid_id) {
                state.target = 1.0;
                state.row = row;
                state.col = col;
                continue;
            }

            let mut inherited = 0.0f32;
            let mut inherit_from = None;
            for (&other_id, state) in &self.float_states {
                if active.contains_key(&other_id) {
                    continue;
                }
                if (state.row - row).abs() <= FLOAT_REUSE_ROW_TOLERANCE
                    && (state.col - col).abs() <= FLOAT_REUSE_COL_TOLERANCE
                    && state.opacity > inherited
                {
                    inherited = state.opacity;
                    inherit_from = Some(other_id);
                }
            }
            if let Some(id) = inherit_from {
                self.float_states.remove(&id);
            }
            self.float_states.insert(
                grid_id,
                FloatAnimState {
                    opacity: inherited,
                    target: 1.0,
                    row,
                    col,
                },
            );
        }

        let ids: Vec<i64> = self.float_states.keys().copied().collect();
        for id in ids {
            if !active.contains_key(&id) {
                if let Some(state) = self.float_states.get_mut(&id) {
                    state.target = 0.0;
                }
            }
        }
    }

    fn cursor_target(store: &GridStateStore, cell_w: f32, cell_h: f32) -> Rect {
        let c = store.cursor;
        let (win_row, win_col) = store.window_origin(c.grid);
        let x = (win_col as f32 + c.col as f32) * cell_w;
        let y = (win_row as f32 + c.row as f32) * cell_h;
        match store.cursor_shape() {
            CursorShape::Block => Rect { x, y, w: cell_w, h: cell_h },
            CursorShape::Vertical => {
                let pct = store.current_mode().map(|m| m.cell_percentage).unwrap_or(25) as f32 / 100.0;
                Rect { x, y, w: (cell_w * pct).max(1.0), h: cell_h }
            }
            CursorShape::Horizontal => {
                let pct = store.current_mode().map(|m| m.cell_percentage).unwrap_or(15) as f32 / 100.0;
                let h = (cell_h * pct).max(1.0);
                Rect { x, y: y + cell_h - h, w: cell_w, h }
            }
        }
    }

    pub fn update(&mut self, dt: f32, store: &GridStateStore, cell_w: f32, cell_h: f32) {
        let dt = dt.min(DT_MAX).max(0.0);

        let (fill, glyph) = resolve_cursor(store);
        self.fill_color = fill;
        self.glyph_color = glyph;
        self.target = Self::cursor_target(store, cell_w, cell_h);

        let cell = (store.cursor.row, store.cursor.col);
        let moved = cell != self.last_cell;
        let mode_changed = store.mode_idx != self.last_mode;

        if !self.initialized {
            self.render = self.target;
            self.prev = self.target;
            self.display = self.target;
            self.initialized = true;
        }

        if moved {
            let drow = (cell.0 as i64 - self.last_cell.0 as i64).unsigned_abs();
            let dcol = (cell.1 as i64 - self.last_cell.1 as i64).unsigned_abs();
            if self.last_cell.0 != u32::MAX {
                if self.cfg.enable_cursor_trail {
                    self.trail.push(TrailSample { rect: self.display, age: 0.0 });
                    if self.trail.len() > TRAIL_CAP {
                        self.trail.remove(0);
                    }
                }
                if self.cfg.enable_flashes && (drow >= 3 || dcol >= 20) {
                    self.flashes.push(Flash { rect: self.target, age: 0.0 });
                }
            }
        }
        if moved || mode_changed {
            self.blink_t = 0.0;
        }
        self.last_cell = cell;
        self.last_mode = store.mode_idx;

        self.prev = self.render;
        if self.cfg.enable_cursor_animation {
            let t = 1.0 - (-self.cfg.cursor_speed * dt).exp();
            self.render.x += (self.target.x - self.render.x) * t;
            self.render.y += (self.target.y - self.render.y) * t;
            self.render.w += (self.target.w - self.render.w) * t;
            self.render.h += (self.target.h - self.render.h) * t;
            self.snap_axes();
        } else {
            self.render = self.target;
        }

        self.display = self.render;
        if self.cfg.enable_cursor_squash_stretch && dt > 0.0 {
            let vx = (self.render.x - self.prev.x) / dt;
            let vy = (self.render.y - self.prev.y) / dt;
            let speed = (vx * vx + vy * vy).sqrt();
            let stretch = (1.0 + speed * 0.0025 * self.cfg.cursor_stretch_strength)
                .min(self.cfg.cursor_max_stretch);
            if stretch > 1.001 {
                let (mut w, mut h) = (self.render.w, self.render.h);
                if vx.abs() >= vy.abs() {
                    w *= stretch;
                    h /= stretch;
                } else {
                    h *= stretch;
                    w /= stretch;
                }
                let cx = self.render.x + self.render.w * 0.5;
                let cy = self.render.y + self.render.h * 0.5;
                self.display = Rect { x: cx - w * 0.5, y: cy - h * 0.5, w, h };
            }
        }

        if self.cfg.enable_cursor_trail {
            for s in self.trail.iter_mut() {
                s.age += dt;
            }
            self.trail.retain(|s| s.age < TRAIL_LIFE);
        } else {
            self.trail.clear();
        }

        if self.cfg.enable_smooth_scroll {
            let t = if self.cfg.scroll_smooth_time > 0.0 {
                1.0 - (-dt / self.cfg.scroll_smooth_time).exp()
            } else {
                1.0
            };
            self.scroll.retain(|_, off| {
                *off += (0.0 - *off) * t;
                off.abs() > 0.5
            });
        } else {
            self.scroll.clear();
        }

        for f in self.flashes.iter_mut() {
            f.age += dt;
        }
        let fd = self.cfg.flash_duration;
        self.flashes.retain(|f| f.age < fd);

        let grav = 420.0 * dt;
        let plife = self.cfg.particle_lifetime;
        for p in self.particles.iter_mut() {
            p.vy += grav;
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.age += dt;
        }
        self.particles.retain(|p| p.age < plife);
        if self.shake > 0.0 {
            self.shake -= self.cfg.screen_shake_decay * dt;
            if self.shake < 0.0 {
                self.shake = 0.0;
            }
        }

        self.blink_t += dt;
        self.update_floats(dt);
    }

    fn update_floats(&mut self, dt: f32) {
        if !self.cfg.enable_float_animation {
            self.float_states.clear();
            return;
        }
        let t = 1.0 - (-self.cfg.float_fade_speed * dt).exp();
        let mut remove = Vec::new();
        for (&id, state) in self.float_states.iter_mut() {
            state.opacity += (state.target - state.opacity) * t;
            if state.target == 0.0 && state.opacity < 0.01 {
                remove.push(id);
            } else if state.target == 1.0 && state.opacity > 0.99 {
                state.opacity = 1.0;
            }
        }
        for id in remove {
            self.float_states.remove(&id);
        }
    }

    fn snap_axes(&mut self) {
        let e = self.cfg.cursor_snap_epsilon;
        if (self.render.x - self.target.x).abs() < e {
            self.render.x = self.target.x;
        }
        if (self.render.y - self.target.y).abs() < e {
            self.render.y = self.target.y;
        }
        if (self.render.w - self.target.w).abs() < e {
            self.render.w = self.target.w;
        }
        if (self.render.h - self.target.h).abs() < e {
            self.render.h = self.target.h;
        }
    }

    pub fn scroll_offset(&self, grid: i64) -> f32 {
        self.scroll.get(&grid).copied().unwrap_or(0.0)
    }

    pub fn float_opacity(&self, grid_id: i64) -> f32 {
        if !self.cfg.enable_float_animation {
            return 1.0;
        }
        self.float_states.get(&grid_id).map(|s| s.opacity).unwrap_or(1.0)
    }

    pub fn fading_out_float_ids(&self) -> Vec<i64> {
        self.float_states
            .iter()
            .filter(|(_, s)| s.target == 0.0 && s.opacity > 0.01)
            .map(|(&id, _)| id)
            .collect()
    }

    pub fn shake_offset(&mut self) -> [f32; 2] {
        if self.shake <= 0.0 {
            return [0.0, 0.0];
        }
        let half = self.shake * 0.5;
        let dx = (self.rand() * 2.0 - 1.0) * half;
        let dy = (self.rand() * 2.0 - 1.0) * half;
        self.shake_phase = self.shake_phase.wrapping_add(1);
        [dx, dy]
    }

    pub fn cursor_visible(&self, store: &GridStateStore) -> bool {
        let Some(m) = store.current_mode() else { return true };
        let (wait, on, off) = (m.blinkwait as f32, m.blinkon as f32, m.blinkoff as f32);
        if on == 0.0 || off == 0.0 {
            return true;
        }
        let t = self.blink_t * 1000.0;
        if t < wait {
            return true;
        }
        let cycle = on + off;
        let p = (t - wait) % cycle;
        p < on
    }

    pub fn is_blinking(&self, store: &GridStateStore) -> bool {
        store
            .current_mode()
            .map(|m| m.blinkon > 0 && m.blinkoff > 0)
            .unwrap_or(false)
    }

    pub fn is_animating(&self) -> bool {
        let cur_far = (self.render.x - self.target.x).abs() > self.cfg.cursor_snap_epsilon
            || (self.render.y - self.target.y).abs() > self.cfg.cursor_snap_epsilon
            || (self.render.w - self.target.w).abs() > self.cfg.cursor_snap_epsilon
            || (self.render.h - self.target.h).abs() > self.cfg.cursor_snap_epsilon;
        cur_far
            || self.scroll.values().any(|o| o.abs() > 0.5)
            || !self.trail.is_empty()
            || !self.flashes.is_empty()
            || !self.particles.is_empty()
            || self.shake > 0.0
            || (self.cfg.enable_float_animation
                && self
                    .float_states
                    .values()
                    .any(|s| (s.opacity - s.target).abs() > 0.01))
    }

    pub fn render_cursor(&self) -> Rect {
        self.display
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_scroll_sets_offset_and_eases_to_zero() {
        let mut anim = AnimationState::new(AnimationConfig::default());
        anim.seed_scroll(1, 2, 18.0, 10);
        assert!((anim.scroll_offset(1) + 36.0).abs() < 0.01);

        for _ in 0..200 {
            anim.update(0.016, &GridStateStore::new(), 9.0, 18.0);
        }
        assert!(anim.scroll_offset(1).abs() < 0.5);
    }

    #[test]
    fn float_fades_in() {
        let mut anim = AnimationState::new(AnimationConfig::default());
        let mut active = HashMap::new();
        active.insert(5, (3, 10));
        anim.sync_floats(&active);
        assert!(anim.float_opacity(5) < 0.01);

        for _ in 0..100 {
            anim.update(0.016, &GridStateStore::new(), 9.0, 18.0);
        }
        assert!(anim.float_opacity(5) > 0.95);
    }
}
