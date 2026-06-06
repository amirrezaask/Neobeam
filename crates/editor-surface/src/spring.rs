//! Easing helpers and critically-damped spring animation (Neovide port).

#[allow(dead_code)]
pub fn ease_linear(t: f32) -> f32 {
    t
}

#[allow(dead_code)]
pub fn ease_in_quad(t: f32) -> f32 {
    t * t
}

#[allow(dead_code)]
pub fn ease_out_quad(t: f32) -> f32 {
    -t * (t - 2.0)
}

#[allow(dead_code)]
pub fn ease_in_out_quad(t: f32) -> f32 {
    if t < 0.5 {
        2.0 * t * t
    } else {
        let n = t * 2.0 - 1.0;
        -0.5 * (n * (n - 2.0) - 1.0)
    }
}

#[allow(dead_code)]
pub fn ease_in_cubic(t: f32) -> f32 {
    t * t * t
}

#[allow(dead_code)]
pub fn ease_out_cubic(t: f32) -> f32 {
    let n = t - 1.0;
    n * n * n + 1.0
}

#[allow(dead_code)]
pub fn ease_in_out_cubic(t: f32) -> f32 {
    let n = 2.0 * t;
    if n < 1.0 {
        0.5 * n * n * n
    } else {
        let n = n - 2.0;
        0.5 * (n * n * n + 2.0)
    }
}

#[allow(dead_code)]
pub fn ease_in_expo(t: f32) -> f32 {
    if t == 0.0 {
        0.0
    } else {
        2.0f32.powf(10.0 * (t - 1.0))
    }
}

pub fn ease_out_expo(t: f32) -> f32 {
    if (t - 1.0).abs() < f32::EPSILON {
        1.0
    } else {
        1.0 - 2.0f32.powf(-10.0 * t)
    }
}

pub fn lerp(start: f32, end: f32, t: f32) -> f32 {
    start + (end - start) * t
}

pub fn ease(ease_func: fn(f32) -> f32, start: f32, end: f32, t: f32) -> f32 {
    lerp(start, end, ease_func(t))
}

pub fn ease_point(start: [f32; 2], end: [f32; 2], t: f32, ease_func: fn(f32) -> f32) -> [f32; 2] {
    [
        ease(ease_func, start[0], end[0], t),
        ease(ease_func, start[1], end[1], t),
    ]
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Spring {
    pub position: f32,
    velocity: f32,
}

impl Spring {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` while the spring is still animating.
    pub fn update(&mut self, dt: f32, animation_length: f32) -> bool {
        if animation_length <= dt {
            self.reset();
            return false;
        }
        if self.position == 0.0 {
            return false;
        }

        let zeta = 1.0;
        let omega = 4.0 / (zeta * animation_length);

        let a = self.position;
        let b = self.position * omega + self.velocity;

        let c = (-omega * dt).exp();

        self.position = (a + b * dt) * c;
        self.velocity = c * (-a * omega - b * dt * omega + b);

        if self.position.abs() < 0.01 {
            self.reset();
            false
        } else {
            true
        }
    }

    pub fn reset(&mut self) {
        self.position = 0.0;
        self.velocity = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_reaches_rest() {
        let mut s = Spring { position: 10.0, velocity: 0.0 };
        for _ in 0..500 {
            s.update(0.016, 0.15);
        }
        assert_eq!(s.position, 0.0);
    }

    #[test]
    fn ease_out_expo_endpoints() {
        assert_eq!(ease_out_expo(1.0), 1.0);
        assert!((ease_out_expo(0.0) - 0.0).abs() < f32::EPSILON);
    }
}
