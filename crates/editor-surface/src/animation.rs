//! Frame-delta animation engine with spring cursor, scroll, and VFX.

use std::collections::HashMap;

use nvim_core::grid::GridStateStore;
use nvim_core::protocol::CursorShape;

use crate::blink::{BlinkStatus, ShouldRender};
use crate::color::{resolve_cursor, Rgba};
use crate::cursor_vfx::{new_cursor_vfxs, CursorVfx, VfxMode};
use crate::frame::QuadInstance;
use crate::spring::Spring;
use crate::window_render::WindowAnimStore;

const DT_MAX: f32 = 0.05;
const DEFAULT_CELL_PERCENTAGE: f32 = 1.0 / 8.0;
const FLOAT_REUSE_ROW_TOLERANCE: i32 = 2;
const FLOAT_REUSE_COL_TOLERANCE: i32 = 12;

const STANDARD_CORNERS: &[(f32, f32); 4] = &[(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)];

#[derive(Clone, Debug)]
pub struct AnimationConfig {
    pub enable_cursor_animation: bool,
    pub enable_cursor_glow: bool,
    pub enable_smooth_scroll: bool,
    pub enable_flashes: bool,
    pub enable_power_mode: bool,
    pub enable_float_animation: bool,
    pub animate_in_insert_mode: bool,
    pub smooth_blink: bool,

    pub animation_length: f32,
    pub short_animation_length: f32,
    pub trail_size: f32,
    pub unfocused_outline_width: f32,

    pub position_animation_length: f32,
    pub scroll_animation_length: f32,
    pub scroll_animation_far_lines: u32,
    pub float_fade_speed: f32,
    pub flash_duration: f32,

    pub cursor_glow_layers: i32,
    pub cursor_glow_radius: f32,
    pub cursor_glow_alpha: f32,

    pub vfx_modes: Vec<VfxMode>,
    pub vfx_opacity: f32,
    pub vfx_particle_lifetime: f32,
    pub vfx_particle_highlight_lifetime: f32,
    pub vfx_particle_density: f32,
    pub vfx_particle_speed: f32,
    pub vfx_particle_phase: f32,
    pub vfx_particle_curl: f32,

    pub particles_per_key: i32,
    pub particle_lifetime: f32,
    pub screen_shake_decay: f32,

    /// Runtime cursor height for VFX sizing (set each frame).
    pub vfx_cursor_height: f32,
    pub vfx_base_color: Rgba,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        AnimationConfig {
            enable_cursor_animation: true,
            enable_cursor_glow: true,
            enable_smooth_scroll: true,
            enable_flashes: true,
            enable_power_mode: false,
            enable_float_animation: true,
            animate_in_insert_mode: true,
            smooth_blink: false,
            animation_length: 0.150,
            short_animation_length: 0.04,
            trail_size: 1.0,
            unfocused_outline_width: 1.0 / 8.0,
            position_animation_length: 0.15,
            scroll_animation_length: 0.3,
            scroll_animation_far_lines: 1,
            float_fade_speed: 24.0,
            flash_duration: 0.35,
            cursor_glow_layers: 20,
            cursor_glow_radius: 20.0,
            cursor_glow_alpha: 0.20,
            vfx_modes: vec![],
            vfx_opacity: 200.0,
            vfx_particle_lifetime: 0.5,
            vfx_particle_highlight_lifetime: 0.2,
            vfx_particle_density: 0.7,
            vfx_particle_speed: 10.0,
            vfx_particle_phase: 1.5,
            vfx_particle_curl: 1.0,
            particles_per_key: 30,
            particle_lifetime: 0.6,
            screen_shake_decay: 10.0,
            vfx_cursor_height: 18.0,
            vfx_base_color: [1.0, 1.0, 1.0, 1.0],
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
pub struct Flash {
    pub rect: Rect,
    pub age: f32,
}

#[derive(Clone, Copy)]
struct FloatAnimState {
    opacity: f32,
    target: f32,
    row: i32,
    col: i32,
}

#[derive(Clone)]
struct Corner {
    current: [f32; 2],
    relative: [f32; 2],
    previous_destination: [f32; 2],
    animation_x: Spring,
    animation_y: Spring,
    animation_length: f32,
}

impl Corner {
    fn new() -> Self {
        Self {
            current: [0.0, 0.0],
            relative: [0.0, 0.0],
            previous_destination: [-1000.0, -1000.0],
            animation_x: Spring::new(),
            animation_y: Spring::new(),
            animation_length: 0.0,
        }
    }

    fn destination(&self, center: [f32; 2], cursor_w: f32, cursor_h: f32) -> [f32; 2] {
        [
            center[0] + self.relative[0] * cursor_w,
            center[1] + self.relative[1] * cursor_h,
        ]
    }

    fn update(
        &mut self,
        center: [f32; 2],
        cursor_w: f32,
        cursor_h: f32,
        dt: f32,
        immediate: bool,
    ) -> bool {
        let dest = self.destination(center, cursor_w, cursor_h);
        if dest[0] != self.previous_destination[0] || dest[1] != self.previous_destination[1] {
            let delta = [dest[0] - self.current[0], dest[1] - self.current[1]];
            self.animation_x.position = delta[0];
            self.animation_y.position = delta[1];
            self.previous_destination = dest;
        }

        if immediate {
            self.animation_x.reset();
            self.animation_y.reset();
            self.current = dest;
            return false;
        }

        let mut animating = self.animation_x.update(dt, self.animation_length);
        animating |= self.animation_y.update(dt, self.animation_length);
        self.current = [
            dest[0] - self.animation_x.position,
            dest[1] - self.animation_y.position,
        ];
        animating
    }

    fn jump(
        &mut self,
        cfg: &AnimationConfig,
        center: [f32; 2],
        cursor_w: f32,
        cursor_h: f32,
        rank: usize,
    ) {
        let dest = self.destination(center, cursor_w, cursor_h);
        let jump_vec = [
            (dest[0] - self.previous_destination[0]) / cursor_w,
            (dest[1] - self.previous_destination[1]) / cursor_h,
        ];

        self.animation_length = if jump_vec[0].abs() <= 2.001 && jump_vec[1].abs() <= 0.001 {
            cfg.animation_length.min(cfg.short_animation_length)
        } else {
            let leading = cfg.animation_length * (1.0 - cfg.trail_size).clamp(0.0, 1.0);
            let trailing = cfg.animation_length;
            match rank {
                2..=3 => leading,
                1 => (leading + trailing) / 2.0,
                0 => trailing,
                _ => trailing,
            }
        };
    }

    fn direction_alignment(&self, center: [f32; 2], cursor_w: f32, cursor_h: f32) -> f32 {
        let dest = self.destination(center, cursor_w, cursor_h);
        let corner_dir = normalize(self.relative);
        let travel = [dest[0] - self.current[0], dest[1] - self.current[1]];
        let travel_dir = normalize(travel);
        corner_dir[0] * travel_dir[0] + corner_dir[1] * travel_dir[1]
    }
}

fn normalize(v: [f32; 2]) -> [f32; 2] {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len < f32::EPSILON {
        [0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len]
    }
}

pub struct AnimationState {
    pub cfg: AnimationConfig,
    pub windows: WindowAnimStore,

    corners: [Corner; 4],
    destination: [f32; 2],
    cursor_w: f32,
    cursor_h: f32,
    jumped: bool,
    previous_cursor: Option<(i64, u32, u32)>,
    previous_shape: Option<CursorShape>,

    pub fill_color: Rgba,
    pub glyph_color: Rgba,

    pub flashes: Vec<Flash>,
    float_states: HashMap<i64, FloatAnimState>,

    pub shake: f32,
    blink: BlinkStatus,
    pub next_render: ShouldRender,

    last_cell: (u32, u32),
    last_mode: usize,

    cursor_vfxs: Vec<Box<dyn CursorVfx>>,
    previous_vfx_modes: Vec<VfxMode>,

    window_has_focus: bool,
    in_insert_mode: bool,
    vfx_animating: bool,

    rng: u32,
}

impl AnimationState {
    pub fn new(cfg: AnimationConfig) -> Self {
        let mut s = AnimationState {
            cfg: cfg.clone(),
            windows: WindowAnimStore::new(),
            corners: std::array::from_fn(|_| Corner::new()),
            destination: [0.0, 0.0],
            cursor_w: 1.0,
            cursor_h: 1.0,
            jumped: false,
            previous_cursor: None,
            previous_shape: None,
            fill_color: [1.0; 4],
            glyph_color: [0.0, 0.0, 0.0, 1.0],
            flashes: Vec::new(),
            float_states: HashMap::new(),
            shake: 0.0,
            blink: BlinkStatus::new(),
            next_render: ShouldRender::Wait,
            cursor_vfxs: new_cursor_vfxs(&cfg.vfx_modes),
            previous_vfx_modes: cfg.vfx_modes.clone(),
            window_has_focus: true,
            in_insert_mode: false,
            vfx_animating: false,
            last_cell: (u32::MAX, u32::MAX),
            last_mode: usize::MAX,
            rng: 0x1234_5678,
        };
        s.set_cursor_shape(CursorShape::Block, DEFAULT_CELL_PERCENTAGE);
        s
    }

    pub fn set_focus(&mut self, focused: bool) {
        self.window_has_focus = focused;
    }

    pub fn on_flush(&mut self, store: &GridStateStore, scroll_deltas: &[(i64, i64)]) {
        self.windows.sync_from_store(store);
        for &(grid, delta) in scroll_deltas {
            self.windows.apply_scroll_delta(grid, delta);
        }
        self.windows.flush_all(&self.cfg);
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
        let cx = self.destination[0];
        let cy = self.destination[1];
        for _ in 0..self.cfg.particles_per_key {
            let ang = self.rand() * std::f32::consts::TAU;
            let speed = 60.0 + self.rand() * 180.0;
            // Power-mode particles use the first trail VFX if present; otherwise skip.
            let _ = (cx, cy, ang, speed);
        }
    }

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
                // Only hand off opacity from floats that are fading out; never
                // inherit from a fully visible float (would skip fade-in).
                if state.target != 0.0 || state.opacity >= 0.99 {
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

    fn set_cursor_shape(&mut self, shape: CursorShape, cell_percentage: f32) {
        for (i, corner) in self.corners.iter_mut().enumerate() {
            let (x, y) = STANDARD_CORNERS[i];
            corner.relative = match shape {
                CursorShape::Block => [x, y],
                CursorShape::Vertical => [(x + 0.5) * cell_percentage - 0.5, y],
                CursorShape::Horizontal => {
                    [x, -((-y + 0.5) * cell_percentage - 0.5)]
                }
            };
        }
    }

    fn update_cursor_destination(
        &mut self,
        store: &GridStateStore,
        cell_w: f32,
        cell_h: f32,
    ) {
        let c = store.cursor;
        let (win_row, win_col) = store.window_origin(c.grid);
        let mut grid_row = win_row as f32 + c.row as f32;
        let grid_col = win_col as f32 + c.col as f32;

        if let Some(w) = self.windows.get(c.grid) {
            grid_row -= w.scroll_animation.position;
            let top = 0.0f32;
            let bottom = w.grid_size.1 as f32 - 1.0;
            grid_row = grid_row.max(w.grid_current.row + top).min(w.grid_current.row + bottom);
        }

        self.destination = [grid_col * cell_w, grid_row * cell_h];

        let mut cw = cell_w;
        if store.cursor_shape() == CursorShape::Block {
            if let Some(grid) = store.grid(c.grid) {
                if let Some(cell) = grid.cell(c.row, c.col) {
                    if cell.double_width {
                        cw *= 2.0;
                    }
                }
            }
        }
        self.cursor_w = cw;
        self.cursor_h = cell_h;
        self.cfg.vfx_cursor_height = cell_h;

        let new_pos = Some((c.grid, c.row, c.col));
        if new_pos != self.previous_cursor {
            self.previous_cursor = new_pos;
            self.jumped = true;
            let center = [
                self.destination[0] + self.cursor_w * 0.5,
                self.destination[1] + self.cursor_h * 0.5,
            ];
            for vfx in &mut self.cursor_vfxs {
                vfx.cursor_jumped(center);
            }
        }
    }

    pub fn update(&mut self, dt: f32, store: &GridStateStore, cell_w: f32, cell_h: f32) {
        let dt = dt.min(DT_MAX).max(0.0);

        let (fill, glyph) = resolve_cursor(store);
        self.fill_color = fill;
        self.glyph_color = glyph;
        self.cfg.vfx_base_color = fill;

        self.in_insert_mode = store
            .current_mode()
            .map(|m| m.short_name == "i")
            .unwrap_or(false);

        self.update_cursor_destination(store, cell_w, cell_h);

        let cell = (store.cursor.row, store.cursor.col);
        let moved = cell != self.last_cell;
        let mode_changed = store.mode_idx != self.last_mode;

        if moved {
            let drow = (cell.0 as i64 - self.last_cell.0 as i64).unsigned_abs();
            let dcol = (cell.1 as i64 - self.last_cell.1 as i64).unsigned_abs();
            if self.last_cell.0 != u32::MAX
                && self.cfg.enable_flashes
                && (drow >= 3 || dcol >= 20)
            {
                self.flashes.push(Flash {
                    rect: Rect {
                        x: self.destination[0],
                        y: self.destination[1],
                        w: self.cursor_w,
                        h: self.cursor_h,
                    },
                    age: 0.0,
                });
            }
        }
        if moved || mode_changed {
            // blink reset handled by BlinkStatus via cursor key change
        }
        self.last_cell = cell;
        self.last_mode = store.mode_idx;

        let shape = store.cursor_shape();
        if self.previous_shape.as_ref() != Some(&shape) {
            self.previous_shape = Some(shape);
            let pct = store
                .current_mode()
                .map(|m| m.cell_percentage as f32 / 100.0)
                .unwrap_or(DEFAULT_CELL_PERCENTAGE);
            self.set_cursor_shape(shape, pct);
            let center = [
                self.destination[0] + self.cursor_w * 0.5,
                self.destination[1] + self.cursor_h * 0.5,
            ];
            for vfx in &mut self.cursor_vfxs {
                vfx.restart(center);
            }
        }

        if self.cfg.vfx_modes != self.previous_vfx_modes {
            self.cursor_vfxs = new_cursor_vfxs(&self.cfg.vfx_modes);
            self.previous_vfx_modes = self.cfg.vfx_modes.clone();
        }

        let center = [
            self.destination[0] + self.cursor_w * 0.5,
            self.destination[1] + self.cursor_h * 0.5,
        ];

        let immediate = !self.cfg.enable_cursor_animation
            || (!self.cfg.animate_in_insert_mode && self.in_insert_mode);

        let mut animating = false;
        let mut vfx_active = false;
        if center[0] != 0.0 || center[1] != 0.0 {
            if self.jumped && self.cfg.enable_cursor_animation {
                let mut ranks: Vec<(usize, f32)> = self
                    .corners
                    .iter()
                    .enumerate()
                    .map(|(id, c)| (id, c.direction_alignment(center, self.cursor_w, self.cursor_h)))
                    .collect();
                ranks.sort_by(|a, b| {
                    a.1.partial_cmp(&b.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(a.0.cmp(&b.0))
                });
                let corner_ranks: Vec<usize> = {
                    let mut out = vec![0usize; 4];
                    for (rank, (id, _)) in ranks.iter().enumerate() {
                        out[*id] = rank;
                    }
                    out
                };
                for (id, corner) in self.corners.iter_mut().enumerate() {
                    corner.jump(&self.cfg, center, self.cursor_w, self.cursor_h, corner_ranks[id]);
                }
            }

            for corner in &mut self.corners {
                animating |= corner.update(center, self.cursor_w, self.cursor_h, dt, immediate);
            }

            for vfx in &mut self.cursor_vfxs {
                vfx_active |= vfx.update(
                    &self.cfg,
                    fill,
                    center,
                    self.cursor_w,
                    self.cursor_h,
                    immediate,
                    dt,
                );
            }
        }
        self.vfx_animating = vfx_active;
        self.jumped = false;

        animating |= self.windows.animate_all(&self.cfg, dt);

        for f in self.flashes.iter_mut() {
            f.age += dt;
        }
        self.flashes
            .retain(|f| f.age < self.cfg.flash_duration);

        if self.shake > 0.0 {
            self.shake -= self.cfg.screen_shake_decay * dt;
            if self.shake < 0.0 {
                self.shake = 0.0;
            }
            animating |= self.shake > 0.0;
        }

        self.update_floats(dt);
        animating |= self
            .float_states
            .values()
            .any(|s| (s.opacity - s.target).abs() > 0.01);

        self.next_render = self.blink.update(store);
        if self.cfg.smooth_blink && self.blink.should_animate_smooth() {
            animating = true;
        }

        let _ = animating;
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
        [dx, dy]
    }

    pub fn cursor_visible(&self) -> bool {
        if self.cfg.smooth_blink {
            self.blink.opacity() > 0.01
        } else {
            self.blink.should_render()
        }
    }

    pub fn cursor_opacity(&self) -> f32 {
        if self.cfg.smooth_blink {
            self.blink.opacity()
        } else if self.blink.should_render() {
            1.0
        } else {
            0.0
        }
    }

    pub fn is_blinking(&self, store: &GridStateStore) -> bool {
        store
            .current_mode()
            .map(|m| m.blinkon > 0 && m.blinkoff > 0)
            .unwrap_or(false)
    }

    pub fn is_animating(&self) -> bool {
        let cursor_moving = self.corners.iter().any(|c| {
            c.animation_x.position.abs() > 0.01 || c.animation_y.position.abs() > 0.01
        });
        cursor_moving
            || self.windows.is_animating(&self.cfg)
            || !self.flashes.is_empty()
            || self.shake > 0.0
            || self.vfx_animating
            || (self.cfg.enable_float_animation
                && self
                    .float_states
                    .values()
                    .any(|s| (s.opacity - s.target).abs() > 0.01))
    }

    pub fn cursor_quad(&self) -> QuadInstance {
        let o = self.cursor_opacity();
        let color = [
            self.fill_color[0],
            self.fill_color[1],
            self.fill_color[2],
            self.fill_color[3] * o,
        ];
        QuadInstance {
            corners: [
                self.corners[0].current,
                self.corners[1].current,
                self.corners[2].current,
                self.corners[3].current,
            ],
            color,
        }
    }

    pub fn cursor_outline_quads(&self, outline_width: f32) -> Vec<QuadInstance> {
        let o = self.cursor_opacity();
        let color = [
            self.fill_color[0],
            self.fill_color[1],
            self.fill_color[2],
            self.fill_color[3] * o,
        ];
        let offsets: [[f32; 2]; 4] = [
            [outline_width, outline_width],
            [-outline_width, outline_width],
            [-outline_width, -outline_width],
            [outline_width, -outline_width],
        ];
        let outer: [[f32; 2]; 4] = std::array::from_fn(|i| {
            [
                self.corners[i].current[0] + offsets[i][0],
                self.corners[i].current[1] + offsets[i][1],
            ]
        });
        let inner: [[f32; 2]; 4] = std::array::from_fn(|i| self.corners[i].current);
        // Approximate outline as outer quad only (inner clipped visually by drawing bg first).
        vec![QuadInstance {
            corners: outer,
            color,
        }, QuadInstance {
            corners: inner,
            color: [0.0, 0.0, 0.0, 0.0],
        }]
    }

    pub fn use_outline_cursor(&self, store: &GridStateStore) -> bool {
        !self.window_has_focus && store.cursor_shape() == CursorShape::Block
    }

    pub fn cursor_center(&self) -> [f32; 2] {
        [
            self.destination[0] + self.cursor_w * 0.5,
            self.destination[1] + self.cursor_h * 0.5,
        ]
    }

    pub fn emit_vfx_rects(&self, rects: &mut Vec<crate::frame::RectInstance>) {
        for vfx in &self.cursor_vfxs {
            vfx.emit_rects(&self.cfg, rects);
        }
    }

    pub fn emit_vfx_quads(&self, quads: &mut Vec<QuadInstance>) {
        for vfx in &self.cursor_vfxs {
            vfx.emit_quads(&self.cfg, quads);
        }
    }

    pub fn render_deadline(&self) -> Option<std::time::Instant> {
        match self.next_render {
            ShouldRender::Deadline(t) => Some(t),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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
