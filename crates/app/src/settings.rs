//! Persisted settings in the OS user-config dir (AGENT_RUST_PORT.md §9).

use std::path::PathBuf;

use editor_surface::AnimationConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub font_family: Option<String>,
    pub font_size: f32,
    pub line_height: f32,
    pub animations_enabled: bool,
    pub power_mode: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            font_family: None,
            font_size: 16.0,
            line_height: 1.3,
            animations_enabled: true,
            power_mode: false,
        }
    }
}

impl Settings {
    pub fn animation_config(&self) -> AnimationConfig {
        let mut cfg = AnimationConfig::default();
        if !self.animations_enabled {
            cfg.enable_cursor_animation = false;
            cfg.enable_cursor_trail = false;
            cfg.enable_cursor_glow = false;
            cfg.enable_cursor_squash_stretch = false;
            cfg.enable_smooth_scroll = false;
            cfg.enable_flashes = false;
            cfg.enable_float_animation = false;
            cfg.enable_power_mode = false;
        } else {
            cfg.enable_power_mode = self.power_mode;
        }
        cfg
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
