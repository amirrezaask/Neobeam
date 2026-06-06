//! Cursor visual effects: particle trails and point highlights (Neovide port).

use crate::animation::AnimationConfig;
use crate::color::Rgba;
use crate::frame::{QuadInstance, RectInstance};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HighlightMode {
    SonicBoom,
    Ripple,
    Wireframe,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrailMode {
    Railgun,
    Torpedo,
    PixieDust,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VfxMode {
    Highlight(HighlightMode),
    Trail(TrailMode),
    Disabled,
}

impl Default for VfxMode {
    fn default() -> Self {
        VfxMode::Disabled
    }
}

pub fn parse_vfx_modes(s: &str) -> Vec<VfxMode> {
    if s.is_empty() {
        return vec![];
    }
    s.split(',')
        .map(|part| match part.trim().to_lowercase().as_str() {
            "sonicboom" => VfxMode::Highlight(HighlightMode::SonicBoom),
            "ripple" => VfxMode::Highlight(HighlightMode::Ripple),
            "wireframe" => VfxMode::Highlight(HighlightMode::Wireframe),
            "railgun" => VfxMode::Trail(TrailMode::Railgun),
            "torpedo" => VfxMode::Trail(TrailMode::Torpedo),
            "pixiedust" => VfxMode::Trail(TrailMode::PixieDust),
            _ => VfxMode::Disabled,
        })
        .filter(|m| !matches!(m, VfxMode::Disabled))
        .collect()
}

pub trait CursorVfx {
    fn update(
        &mut self,
        cfg: &AnimationConfig,
        base_color: Rgba,
        dest: [f32; 2],
        cursor_w: f32,
        cursor_h: f32,
        immediate: bool,
        dt: f32,
    ) -> bool;
    fn restart(&mut self, position: [f32; 2]);
    fn cursor_jumped(&mut self, position: [f32; 2]);
    fn emit_rects(&self, cfg: &AnimationConfig, rects: &mut Vec<RectInstance>);
    fn emit_quads(&self, cfg: &AnimationConfig, quads: &mut Vec<QuadInstance>);
}

pub fn new_cursor_vfxs(modes: &[VfxMode]) -> Vec<Box<dyn CursorVfx>> {
    modes
        .iter()
        .filter_map(|mode| match mode {
            VfxMode::Highlight(m) => Some(Box::new(PointHighlight::new(m.clone())) as Box<dyn CursorVfx>),
            VfxMode::Trail(m) => Some(Box::new(ParticleTrail::new(m.clone())) as Box<dyn CursorVfx>),
            VfxMode::Disabled => None,
        })
        .collect()
}

pub struct PointHighlight {
    t: f32,
    center: [f32; 2],
    mode: HighlightMode,
}

impl PointHighlight {
    pub fn new(mode: HighlightMode) -> Self {
        Self {
            t: 0.0,
            center: [0.0, 0.0],
            mode,
        }
    }
}

impl CursorVfx for PointHighlight {
    fn update(
        &mut self,
        cfg: &AnimationConfig,
        _base_color: Rgba,
        dest: [f32; 2],
        _cursor_w: f32,
        _cursor_h: f32,
        _immediate: bool,
        dt: f32,
    ) -> bool {
        self.center = dest;
        if cfg.vfx_particle_highlight_lifetime > 0.0 {
            self.t = (self.t + dt * (1.0 / cfg.vfx_particle_highlight_lifetime)).min(1.0);
        } else {
            self.t = 1.0;
        }
        self.t < 1.0
    }

    fn restart(&mut self, position: [f32; 2]) {
        self.t = 0.0;
        self.center = position;
    }

    fn cursor_jumped(&mut self, position: [f32; 2]) {
        self.restart(position);
    }

    fn emit_rects(&self, cfg: &AnimationConfig, rects: &mut Vec<RectInstance>) {
        if (self.t - 1.0).abs() < f32::EPSILON {
            return;
        }
        let alpha = crate::spring::ease(
            crate::spring::ease_in_quad,
            cfg.vfx_opacity / 255.0,
            0.0,
            self.t,
        );
        let size = 3.0 * cfg.vfx_cursor_height;
        let radius = self.t * size;
        let hr = radius * 0.5;
        let x = self.center[0] - hr;
        let y = self.center[1] - hr;
        let color = with_alpha(cfg.vfx_base_color, alpha);

        match self.mode {
            HighlightMode::SonicBoom => {
                rects.push(RectInstance {
                    pos: [x, y],
                    size: [radius, radius],
                    color,
                });
            }
            HighlightMode::Ripple | HighlightMode::Wireframe => {
                let stroke = cfg.vfx_cursor_height * 0.2;
                rects.push(RectInstance {
                    pos: [x, y],
                    size: [radius, stroke],
                    color,
                });
                rects.push(RectInstance {
                    pos: [x, y + radius - stroke],
                    size: [radius, stroke],
                    color,
                });
                rects.push(RectInstance {
                    pos: [x, y],
                    size: [stroke, radius],
                    color,
                });
                rects.push(RectInstance {
                    pos: [x + radius - stroke, y],
                    size: [stroke, radius],
                    color,
                });
            }
        }
    }

    fn emit_quads(&self, _cfg: &AnimationConfig, _quads: &mut Vec<QuadInstance>) {}
}

#[derive(Clone)]
struct ParticleData {
    pos: [f32; 2],
    speed: [f32; 2],
    rotation_speed: f32,
    lifetime: f32,
    color: Rgba,
}

pub struct ParticleTrail {
    particles: Vec<ParticleData>,
    previous_dest: [f32; 2],
    trail_mode: TrailMode,
    rng: RngState,
    count_reminder: f32,
}

impl ParticleTrail {
    pub fn new(trail_mode: TrailMode) -> Self {
        Self {
            particles: vec![],
            previous_dest: [0.0, 0.0],
            trail_mode,
            rng: RngState::new(),
            count_reminder: 0.0,
        }
    }

    fn add_particle(
        &mut self,
        pos: [f32; 2],
        speed: [f32; 2],
        rotation_speed: f32,
        lifetime: f32,
        color: Rgba,
    ) {
        self.particles.push(ParticleData {
            pos,
            speed,
            rotation_speed,
            lifetime,
            color,
        });
    }

    fn remove_particle(&mut self, idx: usize) {
        let last = self.particles.len() - 1;
        self.particles[idx] = self.particles[last].clone();
        self.particles.pop();
    }
}

impl CursorVfx for ParticleTrail {
    fn update(
        &mut self,
        cfg: &AnimationConfig,
        base_color: Rgba,
        dest: [f32; 2],
        _cursor_w: f32,
        cursor_h: f32,
        immediate: bool,
        dt: f32,
    ) -> bool {
        let mut i = 0;
        while i < self.particles.len() {
            self.particles[i].lifetime -= dt;
            if self.particles[i].lifetime <= 0.0 {
                self.remove_particle(i);
            } else {
                i += 1;
            }
        }

        for p in &mut self.particles {
            p.pos[0] += p.speed[0] * dt;
            p.pos[1] += p.speed[1] * dt;
            p.speed = rotate_vec(p.speed, dt * p.rotation_speed);
        }

        if dest[0] != self.previous_dest[0] || dest[1] != self.previous_dest[1] {
            if !immediate {
                let travel = [dest[0] - self.previous_dest[0], dest[1] - self.previous_dest[1]];
                let travel_distance = (travel[0] * travel[0] + travel[1] * travel[1]).sqrt();

                let f_particle_count = ((travel_distance / cursor_h) * cfg.vfx_particle_density)
                    + self.count_reminder;
                let particle_count = f_particle_count as usize;
                self.count_reminder = f_particle_count - particle_count as f32;

                for i in 0..particle_count {
                    let t = ((i + 1) as f32) / (particle_count.max(1) as f32);

                    let speed = match self.trail_mode {
                        TrailMode::Railgun => {
                            let phase = t / std::f32::consts::PI
                                * cfg.vfx_particle_phase
                                * (travel_distance / cursor_h);
                            [
                                phase.sin() * 2.0 * cfg.vfx_particle_speed,
                                phase.cos() * 2.0 * cfg.vfx_particle_speed,
                            ]
                        }
                        TrailMode::Torpedo => {
                            let travel_dir = normalize(travel);
                            let particle_dir = sub(
                                self.rng.rand_dir_normalized(),
                                [travel_dir[0] * 1.5, travel_dir[1] * 1.5],
                            );
                            let d = normalize(particle_dir);
                            [d[0] * cfg.vfx_particle_speed, d[1] * cfg.vfx_particle_speed]
                        }
                        TrailMode::PixieDust => {
                            let base_dir = self.rng.rand_dir_normalized();
                            let dir = [base_dir[0] * 0.5, 0.4 + base_dir[1].abs()];
                            [
                                dir[0] * 3.0 * cfg.vfx_particle_speed,
                                dir[1] * 3.0 * cfg.vfx_particle_speed,
                            ]
                        }
                    };

                    let pos = match self.trail_mode {
                        TrailMode::Railgun => [
                            self.previous_dest[0] + travel[0] * t,
                            self.previous_dest[1] + travel[1] * t,
                        ],
                        TrailMode::PixieDust | TrailMode::Torpedo => [
                            self.previous_dest[0]
                                + travel[0] * self.rng.next_f32()
                                + 0.0,
                            self.previous_dest[1]
                                + travel[1] * self.rng.next_f32()
                                + cursor_h * 0.5,
                        ],
                    };

                    let rotation_speed = match self.trail_mode {
                        TrailMode::Railgun => std::f32::consts::PI * cfg.vfx_particle_curl,
                        TrailMode::PixieDust | TrailMode::Torpedo => {
                            (self.rng.next_f32() - 0.5)
                                * std::f32::consts::FRAC_PI_2
                                * cfg.vfx_particle_curl
                        }
                    };

                    self.add_particle(
                        pos,
                        speed,
                        rotation_speed,
                        t * cfg.vfx_particle_lifetime,
                        base_color,
                    );
                }
            }
            self.previous_dest = dest;
        }

        !self.particles.is_empty()
    }

    fn restart(&mut self, _position: [f32; 2]) {
        self.count_reminder = 0.0;
    }

    fn cursor_jumped(&mut self, _position: [f32; 2]) {}

    fn emit_rects(&self, cfg: &AnimationConfig, rects: &mut Vec<RectInstance>) {
        for p in &self.particles {
            let lifetime = p.lifetime / cfg.vfx_particle_lifetime;
            let alpha = (lifetime * cfg.vfx_opacity / 255.0).clamp(0.0, 1.0);
            let color = with_alpha(p.color, alpha);

            let radius = match self.trail_mode {
                TrailMode::Torpedo | TrailMode::Railgun => cfg.vfx_cursor_height * 0.5 * lifetime,
                TrailMode::PixieDust => cfg.vfx_cursor_height * 0.2,
            };
            let hr = radius * 0.5;
            rects.push(RectInstance {
                pos: [p.pos[0] - hr, p.pos[1] - hr],
                size: [radius, radius],
                color,
            });
        }
    }

    fn emit_quads(&self, _cfg: &AnimationConfig, _quads: &mut Vec<QuadInstance>) {}
}

struct RngState {
    state: u64,
    inc: u64,
}

impl RngState {
    fn new() -> Self {
        Self {
            state: 0x853C_49E6_748F_EA9Bu64,
            inc: (0xDA3E_39CB_94B5BDBu64 << 1) | 1,
        }
    }

    fn next(&mut self) -> u32 {
        let old_state = self.state;
        let new_state = old_state
            .wrapping_mul(6_364_136_223_846_793_005u64)
            .wrapping_add(self.inc);
        self.state = new_state;

        const ROTATE: u32 = 59;
        const XSHIFT: u32 = 18;
        const SPARE: u32 = 27;

        let rot = (old_state >> ROTATE) as u32;
        let xsh = (((old_state >> XSHIFT) ^ old_state) >> SPARE) as u32;
        xsh.rotate_right(rot)
    }

    fn next_f32(&mut self) -> f32 {
        let v = self.next();
        let float_bits = (v as f64).to_bits();
        let exponent = (float_bits >> 52) & ((1 << 11) - 1);
        let new_exponent = exponent.max(32) - 32;
        let new_bits = (new_exponent << 52) | (float_bits & 0x801F_FFFF_FFFF_FFFFu64);
        f64::from_bits(new_bits) as f32
    }

    fn rand_dir(&mut self) -> [f32; 2] {
        let x = self.next_f32();
        let y = self.next_f32();
        [x * 2.0 - 1.0, y * 2.0 - 1.0]
    }

    fn rand_dir_normalized(&mut self) -> [f32; 2] {
        normalize(self.rand_dir())
    }
}

fn rotate_vec(v: [f32; 2], rot: f32) -> [f32; 2] {
    let sin = rot.sin();
    let cos = rot.cos();
    [v[0] * cos - v[1] * sin, v[0] * sin + v[1] * cos]
}

fn normalize(v: [f32; 2]) -> [f32; 2] {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len < f32::EPSILON {
        [0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len]
    }
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn with_alpha(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], a.clamp(0.0, 1.0)]
}
