//! Where things live.
//!
//! Config (plain text, user-edited), loaded in this order, later overriding
//! earlier:
//! 1. `$XDG_CONFIG_HOME/blygger/config` (default `~/.config/blygger/config`)
//! 2. `~/Library/Application Support/org.blygger.desktop/config`
//!
//! `BLYGGER_CONFIG=<file>` replaces both (tests, development, screenshots).
//!
//! App state (the SQLite db, caches, media) is not config: it lives in
//! `~/Library/Application Support/org.blygger.desktop/` (`BLYGGER_DATA_DIR`
//! overrides it).
//!
//! On Windows the second config location is `%APPDATA%\Blygger\config`, and
//! app state lives in `%LOCALAPPDATA%\Blygger\` (not roamed).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const APP_ID: &str = "org.blygger.desktop";
pub const CONFIG_FILE_NAME: &str = "config";
/// The pre-rename data dir, migrated on first launch.
pub const LEGACY_DATA_DIR_NAME: &str = "Blygger";

/// The set of files to load, and the one new keys are written to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFiles {
    /// Load order; later files override earlier ones.
    pub load: Vec<PathBuf>,
    /// Where keys no file mentions yet are written, and what "Open config
    /// file" opens: the last existing file in `load`, else the first.
    pub primary: PathBuf,
}

impl ConfigFiles {
    /// From the real environment.
    pub fn discover() -> ConfigFiles {
        Self::discover_with(&|k| std::env::var_os(k), &|p| p.exists())
    }

    /// Pure version for tests: `env` looks up variables, `exists` checks files.
    pub fn discover_with(
        env: &dyn Fn(&str) -> Option<OsString>,
        exists: &dyn Fn(&Path) -> bool,
    ) -> ConfigFiles {
        if let Some(p) = env("BLYGGER_CONFIG").filter(|p| !p.is_empty()) {
            return ConfigFiles::single(PathBuf::from(p));
        }
        let load = default_locations(env);
        let primary = load
            .iter()
            .rev()
            .find(|p| exists(p))
            .unwrap_or(&load[0])
            .clone();
        ConfigFiles { load, primary }
    }

    pub fn single(path: PathBuf) -> ConfigFiles {
        ConfigFiles {
            load: vec![path.clone()],
            primary: path,
        }
    }
}

fn home(env: &dyn Fn(&str) -> Option<OsString>) -> PathBuf {
    env("HOME")
        .filter(|h| !h.is_empty())
        // Windows has no HOME, as a rule.
        .or_else(|| env("USERPROFILE").filter(|h| cfg!(windows) && !h.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Where this platform keeps an app's files: `var` (`APPDATA` for config,
/// `LOCALAPPDATA` for state) on Windows, else `~/Library/Application Support`.
fn app_dir(env: &dyn Fn(&str) -> Option<OsString>, var: &str) -> PathBuf {
    if cfg!(windows)
        && let Some(d) = env(var).filter(|d| !d.is_empty())
    {
        return PathBuf::from(d).join("Blygger");
    }
    app_support(&home(env))
}

/// The two default config locations, in load order.
pub fn default_locations(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let home = home(env);
    let xdg = env("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        // Rooted, so `/xdg` counts on Windows too (where it has no drive).
        .filter(|p| p.has_root())
        .unwrap_or_else(|| home.join(".config"));
    vec![
        xdg.join("blygger").join(CONFIG_FILE_NAME),
        app_dir(env, "APPDATA").join(CONFIG_FILE_NAME),
    ]
}

fn app_support(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join(APP_ID)
}

/// `~/Library/Application Support/org.blygger.desktop/` (or `BLYGGER_DATA_DIR`).
pub fn data_dir() -> PathBuf {
    data_dir_with(&|k| std::env::var_os(k))
}

pub fn data_dir_with(env: &dyn Fn(&str) -> Option<OsString>) -> PathBuf {
    if let Some(p) = env("BLYGGER_DATA_DIR").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    app_dir(env, "LOCALAPPDATA")
}

/// `~/Library/Application Support/Blygger/` (before the rename).
pub fn legacy_data_dir() -> PathBuf {
    home(&|k| std::env::var_os(k))
        .join("Library")
        .join("Application Support")
        .join(LEGACY_DATA_DIR_NAME)
}

/// Move everything from the old data dir into the new one (a rename, not a
/// copy). Entries that already exist in the new dir are left where they
/// are. The old dir is removed once it's empty. Returns what was moved.
pub fn migrate_data_dir(old: &Path, new: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut moved = Vec::new();
    if !old.is_dir() || old == new {
        return Ok(moved);
    }
    std::fs::create_dir_all(new)?;
    for entry in std::fs::read_dir(old)? {
        let entry = entry?;
        let dest = new.join(entry.file_name());
        if dest.exists() {
            continue;
        }
        std::fs::rename(entry.path(), &dest)?;
        moved.push(dest);
    }
    // Only succeeds when empty; anything left behind stays put.
    let _ = std::fs::remove_dir(old);
    Ok(moved)
}

/// `$HOME`, or `%USERPROFILE%` on Windows (which has no HOME, as a rule).
pub fn home_var() -> Option<OsString> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|h| cfg!(windows) && !h.is_empty()))
}

/// Replace a leading `$HOME` with `~` for display.
pub fn tilde(p: &Path) -> String {
    if let Some(home) = home_var()
        && let Ok(rest) = p.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    p.display().to_string()
}
