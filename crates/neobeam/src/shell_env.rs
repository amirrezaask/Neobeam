//! Resolve a login-shell `PATH` for GUI launches (Spotlight, Dock, Finder).
//!
//! macOS GUI apps inherit a minimal `PATH`, so tools like `nvim`, `fzf`, and
//! Homebrew `git` are invisible to child processes. We cache the login-shell
//! path in `~/.config/neobeam/shell-env.json` for the next GUI launch.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ShellEnvCache {
    path: String,
    shell: String,
}

pub fn ensure_login_path() {
    let cache_path = match cache_path() {
        Some(path) => path,
        None => return,
    };

    if path_looks_minimal() {
        if let Some(cached) = load_cache(&cache_path) {
            if cached_path_usable(&cached.path) {
                set_path(&cached.path);
                return;
            }
        }
        if let Some(path) = resolve_path_from_login_shell() {
            set_path(&path);
            save_cache(&cache_path, &path);
        }
        return;
    }

    if let Ok(path) = std::env::var("PATH") {
        if !path.is_empty() {
            save_cache(&cache_path, &path);
        }
    }
}

fn cache_path() -> Option<PathBuf> {
    crate::settings::config_path()
        .map(|p| p.parent().unwrap_or(Path::new(".")).join("shell-env.json"))
}

fn load_cache(path: &Path) -> Option<ShellEnvCache> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_cache(path: &Path, path_value: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let cache = ShellEnvCache {
        path: path_value.to_string(),
        shell: login_shell().to_string_lossy().into_owned(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&cache) {
        let _ = std::fs::write(path, json);
    }
}

fn set_path(path: &str) {
    // SAFETY: called once on the main thread before any threads spawn subprocesses.
    unsafe {
        std::env::set_var("PATH", path);
    }
}

fn path_looks_minimal() -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return true;
    };
    !path.contains("/opt/homebrew/bin")
        && !path.contains("/usr/local/bin")
        && !path.contains(".local/bin")
}

fn cached_path_usable(path: &str) -> bool {
    executable_on_path(path, "nvim").is_some()
}

fn executable_on_path(path: &str, name: &str) -> Option<PathBuf> {
    for dir in path.split(':').filter(|entry| !entry.is_empty()) {
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn login_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|shell| shell.is_file())
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"))
}

fn resolve_path_from_login_shell() -> Option<String> {
    let shell = login_shell();
    let output = Command::new(&shell)
        .args(["-l", "-c", "printf %s \"$PATH\""])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}
