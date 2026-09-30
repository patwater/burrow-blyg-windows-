//! Configuration: one plain-text, Ghostty-style config file (see [`parse`]
//! for the syntax, [`keys::KEYS`] for every key, [`paths`] for where it
//! lives), plus owner-token storage in the Keychain ([`tokens`]).
//!
//! Secrets never go in the config file. App state (db, caches, media) lives
//! in [`paths::data_dir`], not in config.

pub mod edit;
pub mod keys;
pub mod migrate;
pub mod parse;
pub mod paths;
pub mod show;
mod tokens;

use std::path::{Path, PathBuf};

pub use edit::Change;
pub use parse::{
    CaptureDefault, Config, Diagnostic, EditedPosts, Layout, Loaded, NewNote, Severity, Theme,
};
pub use paths::{APP_ID, ConfigFiles, data_dir};
#[cfg(feature = "keychain")]
pub use tokens::KeychainTokenStore;
pub use tokens::{KEYCHAIN_SERVICE, MemoryTokenStore, TokenStore, token_account};

enum Backing {
    Files(ConfigFiles),
    /// Tests and previews: a virtual file, edited by the same write-back code.
    Memory {
        path: PathBuf,
        text: String,
    },
}

struct MemorySource<'a>(&'a Path, &'a str);

impl parse::Source for MemorySource<'_> {
    fn read(&self, path: &Path) -> std::io::Result<Option<String>> {
        Ok((path == self.0).then(|| self.1.to_string()))
    }
}

/// The loaded config plus where it came from, so it can be reloaded and
/// written back.
pub struct ConfigStore {
    backing: Backing,
    loaded: Loaded,
}

impl ConfigStore {
    /// The real locations (honours `BLYGGER_CONFIG`).
    pub fn discover() -> ConfigStore {
        Self::open(ConfigFiles::discover())
    }

    pub fn open(files: ConfigFiles) -> ConfigStore {
        let mut s = ConfigStore {
            backing: Backing::Files(files),
            loaded: Loaded::default(),
        };
        s.reload();
        s
    }

    /// A store over an in-memory file (nothing touches the disk).
    pub fn in_memory(text: &str) -> ConfigStore {
        let mut s = ConfigStore {
            backing: Backing::Memory {
                path: PathBuf::from("config"),
                text: text.to_string(),
            },
            loaded: Loaded::default(),
        };
        s.reload();
        s
    }

    pub fn reload(&mut self) {
        self.loaded = match &self.backing {
            Backing::Files(f) => parse::load(&f.load, &parse::Disk),
            Backing::Memory { path, text } => {
                parse::load(std::slice::from_ref(path), &MemorySource(path, text))
            }
        };
    }

    pub fn config(&self) -> &Config {
        &self.loaded.config
    }

    pub fn loaded(&self) -> &Loaded {
        &self.loaded
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.loaded.diagnostics
    }

    /// The file "Open config file" opens and new keys go to.
    pub fn primary(&self) -> &Path {
        match &self.backing {
            Backing::Files(f) => &f.primary,
            Backing::Memory { path, .. } => path,
        }
    }

    /// The in-memory file's text (in-memory stores only).
    pub fn text(&self) -> Option<&str> {
        match &self.backing {
            Backing::Memory { text, .. } => Some(text),
            Backing::Files(_) => None,
        }
    }

    /// Write `changes` back, preserving comments and ordering. Each key is
    /// edited in the file that last set it (so the change takes effect);
    /// keys no file sets go to the primary file. Then reload.
    pub fn set(&mut self, changes: &[(&str, Change)]) -> std::io::Result<()> {
        match &mut self.backing {
            Backing::Memory { text, .. } => *text = edit::apply(text, changes),
            Backing::Files(f) => {
                let mut by_file: Vec<(PathBuf, Vec<(&str, Change)>)> = Vec::new();
                for (key, change) in changes {
                    let target = self
                        .loaded
                        .last_file_for(key)
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| f.primary.clone());
                    match by_file.iter_mut().find(|(p, _)| *p == target) {
                        Some((_, v)) => v.push((key, change.clone())),
                        None => by_file.push((target, vec![(key, change.clone())])),
                    }
                }
                for (path, changes) in by_file {
                    edit::write_file(&path, &changes)?;
                }
            }
        }
        self.reload();
        Ok(())
    }

    /// Make sure the primary file exists (creating it with a commented
    /// header) and return its path.
    pub fn ensure_primary_exists(&self) -> std::io::Result<PathBuf> {
        let p = self.primary().to_path_buf();
        if let Backing::Files(_) = self.backing
            && !p.exists()
        {
            edit::atomic_write(&p, edit::HEADER)?;
        }
        Ok(p)
    }
}
