//! Persisted settings in `$XDG_CONFIG_HOME/neobeam/settings.json` (or `~/.config/...`).

use std::path::PathBuf;

use editor_surface::{parse_vfx_modes, AnimationConfig, ChromeLayoutConfig, VfxMode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Fraction of one editor line per wheel unit (1.0 = one line per notch).
    #[serde(default = "default_mouse_scroll_sensitivity")]
    pub mouse_scroll_sensitivity: f32,
}

fn default_mouse_scroll_sensitivity() -> f32 {
    0.35
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
            mouse_scroll_sensitivity: 0.35,
        }
    }
}

impl Settings {
    pub fn chrome_layout_config(&self) -> ChromeLayoutConfig {
        ChromeLayoutConfig {
            activity_bar_width: 0.0,
            ..ChromeLayoutConfig::default()
        }
    }

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
}

pub fn config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let dirs = directories::ProjectDirs::from("", "", "neobeam")?;
        return Some(dirs.config_dir().join("settings.json"));
    }
    #[cfg(not(windows))]
    {
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().join(".config")))?;
        Some(config_home.join("neobeam").join("settings.json"))
    }
}

impl Settings {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Settings::default();
        };
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

    /// Write defaults only when the config file does not exist yet.
    pub fn save_if_missing(&self) {
        let Some(path) = config_path() else { return };
        if path.exists() {
            return;
        }
        self.save();
    }
}

/// Watch `settings.json` and invoke `on_change` after writes settle (debounced).
pub fn spawn_watcher(mut on_change: impl FnMut() + Send + 'static) {
    let Some(path) = config_path() else { return };
    let Some(parent) = path.parent().map(|p| p.to_path_buf()) else {
        return;
    };
    let settings_path = path;

    std::thread::spawn(move || {
        use std::time::Duration;

        use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode, DebounceEventResult};

        let watch_target = settings_path.clone();
        let mut debouncer = match new_debouncer(
            Duration::from_millis(300),
            move |res: DebounceEventResult| {
                let Ok(events) = res else { return };
                if events.iter().any(|e| e.path == watch_target) {
                    on_change();
                }
            },
        ) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("settings watcher failed to start: {e}");
                return;
            }
        };

        if let Err(e) = debouncer
            .watcher()
            .watch(&parent, RecursiveMode::NonRecursive)
        {
            tracing::warn!("settings watcher failed to watch {}: {e}", parent.display());
            return;
        }

        tracing::info!("watching settings at {}", settings_path.display());
        loop {
            std::thread::park();
        }
    });
}
