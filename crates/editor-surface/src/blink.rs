//! Cursor blink state machine (Neovide port).

use std::time::{Duration, Instant};

use nvim_core::grid::GridStateStore;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum BlinkState {
    Waiting,
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShouldRender {
    Wait,
    Immediately,
    Deadline(Instant),
}

pub struct BlinkStatus {
    state: BlinkState,
    transition_time: Instant,
    blinkwait: u32,
    blinkon: u32,
    blinkoff: u32,
    cursor_key: (i64, u32, u32),
}

impl BlinkStatus {
    pub fn new() -> Self {
        Self {
            state: BlinkState::Waiting,
            transition_time: Instant::now(),
            blinkwait: 0,
            blinkon: 0,
            blinkoff: 0,
            cursor_key: (0, u32::MAX, u32::MAX),
        }
    }

    fn is_static(&self) -> bool {
        self.blinkoff == 0 || self.blinkon == 0
    }

    fn delay(&self) -> Duration {
        let ms = match self.state {
            BlinkState::Waiting => self.blinkwait,
            BlinkState::Off => self.blinkoff,
            BlinkState::On => self.blinkon,
        };
        Duration::from_millis(ms as u64)
    }

    pub fn update(&mut self, store: &GridStateStore) -> ShouldRender {
        let now = Instant::now();
        let c = store.cursor;
        let key = (c.grid, c.row, c.col);
        let mode = store.current_mode();

        let (wait, on, off) = mode
            .map(|m| (m.blinkwait, m.blinkon, m.blinkoff))
            .unwrap_or((0, 0, 0));

        if key != self.cursor_key
            || wait != self.blinkwait
            || on != self.blinkon
            || off != self.blinkoff
        {
            self.cursor_key = key;
            self.blinkwait = wait;
            self.blinkon = on;
            self.blinkoff = off;
            if wait > 0 {
                self.state = BlinkState::Waiting;
            } else {
                self.state = BlinkState::On;
            }
            self.transition_time = now + self.delay();
        }

        if self.is_static() {
            self.state = BlinkState::Waiting;
            return ShouldRender::Wait;
        }

        if self.transition_time <= now {
            self.state = match self.state {
                BlinkState::Waiting => BlinkState::On,
                BlinkState::On => BlinkState::Off,
                BlinkState::Off => BlinkState::On,
            };
            self.transition_time += self.delay();
            if self.transition_time <= now {
                self.transition_time = now + self.delay();
            }
            return ShouldRender::Immediately;
        }

        ShouldRender::Deadline(self.transition_time)
    }

    /// Opacity for smooth blink: 0.0 transparent, 1.0 opaque.
    pub fn opacity(&self) -> f32 {
        let now = Instant::now();
        if self.state == BlinkState::Waiting {
            return 1.0;
        }
        let total = self.delay().as_secs_f32();
        if total <= 0.0 {
            return 1.0;
        }
        let remaining = (self.transition_time.saturating_duration_since(now)).as_secs_f32();
        match self.state {
            BlinkState::Waiting => 1.0,
            BlinkState::On => (remaining / total).clamp(0.0, 1.0),
            BlinkState::Off => (1.0 - remaining / total).clamp(0.0, 1.0),
        }
    }

    pub fn should_animate_smooth(&self) -> bool {
        matches!(self.state, BlinkState::On | BlinkState::Off)
    }

    pub fn should_render(&self) -> bool {
        match self.state {
            BlinkState::Off => false,
            BlinkState::On | BlinkState::Waiting => true,
        }
    }
}

impl Default for BlinkStatus {
    fn default() -> Self {
        Self::new()
    }
}
