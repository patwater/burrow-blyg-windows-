//! Small bits of app state that aren't configuration (things the app
//! remembers, not things the user sets): `state.json` in the data dir.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const STATE_FILE: &str = "state.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    /// The app has been launched (and shown its first-run flow) before.
    pub launched_before: bool,
    /// The "Sign in with ChatGPT is unofficial" notice has been shown.
    pub chatgpt_notice_shown: bool,
    /// The first-run onboarding was finished or skipped (Settings › Help
    /// can show it again).
    pub onboarded: bool,
    /// The main window's last view mode (`write`, `split`, `studio`,
    /// `focus`): the full editor's ⌘1/⌘2/⌘3/⌘E, remembered per viewer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    /// When an update check last found nothing newer (Unix seconds), so a
    /// relaunch doesn't ask GitHub again within a day (`auto-update`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_update_check: Option<u64>,
}

impl AppState {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(STATE_FILE)
    }

    /// Missing or unreadable → defaults (state is never worth a crash).
    pub fn load(data_dir: &Path) -> AppState {
        std::fs::read_to_string(Self::path(data_dir))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        let s = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        crate::config::edit::atomic_write(&Self::path(data_dir), &s)
    }

    /// Load, change, save.
    pub fn update(data_dir: &Path, f: impl FnOnce(&mut AppState)) -> std::io::Result<AppState> {
        let mut s = Self::load(data_dir);
        f(&mut s);
        s.save(data_dir)?;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(AppState::load(dir.path()), AppState::default());
        AppState::update(dir.path(), |s| s.launched_before = true).unwrap();
        assert!(AppState::load(dir.path()).launched_before);
        std::fs::write(AppState::path(dir.path()), "{not json").unwrap();
        assert_eq!(AppState::load(dir.path()), AppState::default());
    }
}
