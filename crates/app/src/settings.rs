//! Persisted settings in the OS user-config dir (AGENT_RUST_PORT.md §9).

use std::path::PathBuf;

use editor_surface::{parse_vfx_modes, AnimationConfig, VfxMode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub font_family: Option<String>,
    pub font_size: f32,
    pub line_height: f32,
    pub animations_enabled: bool,
    pub power_mode: bool,

    pub animation_length: f32,
    pub short_animation_length: f32,
    pub trail_size: f32,
    pub animate_in_insert_mode: bool,
    pub smooth_blink: bool,
    pub position_animation_length: f32,
    pub scroll_animation_length: f32,
    pub scroll_animation_far_lines: u32,
    /// 0 = off; 1 = default glow; up to 2 = stronger.
    pub cursor_glow: f32,
    pub enable_flashes: bool,
    pub enable_float_animation: bool,
    pub float_fade_speed: f32,
    pub vfx_modes: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            font_family: None,
            font_size: 16.0,
            line_height: 1.3,
            animations_enabled: true,
            power_mode: false,
            animation_length: 0.150,
            short_animation_length: 0.04,
            trail_size: 1.0,
            animate_in_insert_mode: true,
            smooth_blink: false,
            position_animation_length: 0.15,
            scroll_animation_length: 0.3,
            scroll_animation_far_lines: 1,
            cursor_glow: 1.0,
            enable_flashes: true,
            enable_float_animation: true,
            float_fade_speed: 24.0,
            vfx_modes: String::new(),
        }
    }
}

impl Settings {
    pub fn animation_config(&self) -> AnimationConfig {
        let mut cfg = AnimationConfig::default();
        cfg.animation_length = self.animation_length;
        cfg.short_animation_length = self.short_animation_length;
        cfg.trail_size = self.trail_size;
        cfg.animate_in_insert_mode = self.animate_in_insert_mode;
        cfg.smooth_blink = self.smooth_blink;
        cfg.position_animation_length = self.position_animation_length;
        cfg.scroll_animation_length = self.scroll_animation_length;
        cfg.scroll_animation_far_lines = self.scroll_animation_far_lines;
        if self.cursor_glow > 0.0 {
            let g = self.cursor_glow;
            cfg.enable_cursor_glow = true;
            cfg.cursor_glow_alpha = 0.20 * g;
            cfg.cursor_glow_radius = 20.0 * g;
            cfg.cursor_glow_layers = (20.0 * g).round().max(1.0) as i32;
        } else {
            cfg.enable_cursor_glow = false;
        }
        cfg.enable_flashes = self.enable_flashes;
        cfg.enable_float_animation = self.enable_float_animation;
        cfg.float_fade_speed = self.float_fade_speed;
        cfg.vfx_modes = parse_vfx_modes(&self.vfx_modes);
        cfg.enable_power_mode = self.power_mode;

        if !self.animations_enabled {
            cfg.enable_cursor_animation = false;
            cfg.enable_cursor_glow = false;
            cfg.enable_smooth_scroll = false;
            cfg.enable_flashes = false;
            cfg.enable_float_animation = false;
            cfg.enable_power_mode = false;
            cfg.vfx_modes = Vec::<VfxMode>::new();
        }
        if self.animation_length <= 0.0 {
            cfg.enable_cursor_animation = false;
        }
        if self.scroll_animation_length <= 0.0 {
            cfg.enable_smooth_scroll = false;
        }
        cfg
    }

    /// Apply `g:neovide_scroll_*` from nvim when those keys are absent from settings.json.
    pub fn apply_neovide_scroll_globals(&mut self, scroll: Option<f32>, far: Option<u32>) {
        if !Self::json_has_scroll_keys() {
            if let Some(v) = scroll {
                self.scroll_animation_length = v;
            }
            if let Some(v) = far {
                self.scroll_animation_far_lines = v;
            }
        }
    }

    fn json_has_scroll_keys() -> bool {
        let Some(path) = config_path() else {
            return false;
        };
        let Ok(s) = std::fs::read_to_string(&path) else {
            return false;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else {
            return false;
        };
        v.get("scroll_animation_length").is_some()
            || v.get("scroll_animation_far_lines").is_some()
    }
}

fn config_path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("dev", "nvim-ui", "nvim-ui")?;
    Some(dirs.config_dir().join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        let Some(path) = config_path() else { return Settings::default() };
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) {
        let Some(path) = config_path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}
