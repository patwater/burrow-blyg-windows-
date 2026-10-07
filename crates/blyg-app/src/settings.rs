//! The app's handle on the config file (a GPUI global): load, validate,
//! write back changed keys, reload, and the one-time migration notice.
//!
//! The owner token store lives here too, so the first-run "Connect your
//! blyg" sheet can put the token in the Keychain and the URL in the config.

use std::path::PathBuf;
use std::sync::Arc;

use blyg_core::ConfigStore;
use blyg_core::config::theme::Registry;
use blyg_core::config::{Change, Diagnostic, Loaded, Severity, TokenStore};
use gpui_kit::{App, Global};

use crate::prefs::{self, Prefs};

pub struct AppConfig {
    pub store: ConfigStore,
    pub tokens: Arc<dyn TokenStore>,
    /// Shown once as a toast when the main window opens (migration).
    pub notice: Option<String>,
}

impl Global for AppConfig {}

pub fn init(store: ConfigStore, tokens: Arc<dyn TokenStore>, notice: Option<String>, cx: &mut App) {
    cx.set_global(AppConfig {
        store,
        tokens,
        notice,
    });
}

pub fn get(cx: &App) -> &AppConfig {
    cx.global::<AppConfig>()
}

/// Everything wrong with the loaded config: the core's parse/value
/// problems plus the app's own checks (fonts, hotkey, theme names), and
/// the problems in the theme files.
pub fn diagnostics(loaded: &Loaded, themes: &Registry) -> Vec<Diagnostic> {
    let mut out = loaded.diagnostics.clone();
    out.extend(themes.diagnostics.iter().cloned());
    for (file, line, key, v) in themes.user_fonts() {
        let list = if key == "font-writing" {
            prefs::WRITING_FONTS
        } else {
            prefs::UI_FONTS
        };
        if prefs::find(list, &v).is_none() {
            out.push(Diagnostic {
                file,
                line,
                severity: Severity::Warning,
                message: format!("{key}: unknown font `{v}`. See `blygger +list-fonts`"),
            });
        }
    }
    for e in &loaded.entries {
        let v = e.value.trim();
        if v.is_empty() {
            continue;
        }
        let problem = match e.key.as_str() {
            "font-family-writing" if prefs::find(prefs::WRITING_FONTS, v).is_none() => Some((
                Severity::Warning,
                format!(
                    "font-family-writing: unknown font `{v}`; using {}. See `blygger +list-fonts`",
                    prefs::WRITING_FONTS[0].label
                ),
            )),
            "font-family-ui" if prefs::find(prefs::UI_FONTS, v).is_none() => Some((
                Severity::Warning,
                format!(
                    "font-family-ui: unknown font `{v}`; using {}. See `blygger +list-fonts`",
                    prefs::UI_FONTS[0].label
                ),
            )),
            "capture-hotkey" if prefs::parse_hotkey(v).is_none() => Some((
                Severity::Error,
                format!("capture-hotkey: `{v}` isn't a key combination I understand"),
            )),
            "theme" | "theme-dark" if !themes.exists(&v.to_ascii_lowercase()) => Some((
                Severity::Warning,
                format!(
                    "{}: unknown theme `{v}`; using {}. See `blygger +list-themes`",
                    e.key,
                    if e.key == "theme" { "system" } else { "dark" }
                ),
            )),
            _ => None,
        };
        if let Some((severity, message)) = problem {
            out.push(Diagnostic {
                file: e.file.clone(),
                line: e.line,
                severity,
                message,
            });
        }
    }
    out.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    out
}

pub fn current_diagnostics(cx: &App) -> Vec<Diagnostic> {
    diagnostics(get(cx).store.loaded(), &crate::theme::themes(cx).registry)
}

/// The files and folder whose changes reload the config: every config
/// file that could be loaded, and the themes folder with its files.
pub fn watched(cx: &App) -> Vec<PathBuf> {
    let Some(c) = cx.try_global::<AppConfig>() else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = c.store.loaded().files.clone();
    v.push(c.store.primary().to_path_buf());
    if let Some(d) = c.store.themes_dir() {
        v.push(d);
    }
    v
}

/// A fingerprint of `paths`: their modification times and sizes, and those
/// of the files directly inside any that are folders (at most 256).
pub fn watch_stamp(paths: &[PathBuf]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let stamp = |p: &std::path::Path, h: &mut std::collections::hash_map::DefaultHasher| {
        p.hash(h);
        if let Ok(m) = std::fs::metadata(p) {
            m.len().hash(h);
            m.modified().ok().hash(h);
        }
    };
    for p in paths {
        stamp(p, &mut h);
        if p.is_dir()
            && let Ok(rd) = std::fs::read_dir(p)
        {
            let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).take(256).collect();
            entries.sort();
            for e in entries {
                stamp(&e, &mut h);
            }
        }
    }
    h.finish()
}

pub fn prefs(cx: &App) -> Prefs {
    Prefs::from_config(get(cx).store.config())
}

pub fn blyg_url(cx: &App) -> Option<String> {
    get(cx).store.config().blyg_url().map(str::to_string)
}

/// Write changed keys back to the config file (comments and order kept).
pub fn write(changes: &[(&str, Change)], cx: &mut App) -> Result<(), String> {
    if changes.is_empty() {
        return Ok(());
    }
    cx.global_mut::<AppConfig>()
        .store
        .set(changes)
        .map_err(|e| format!("Couldn't save the config file: {e}"))
}

pub fn reload(cx: &mut App) {
    cx.global_mut::<AppConfig>().store.reload();
}

/// Create the config file if needed (with a commented header) and open it
/// in the default text editor.
pub fn open_config_file(cx: &App) -> Result<PathBuf, String> {
    let path = get(cx)
        .store
        .ensure_primary_exists()
        .map_err(|e| format!("Couldn't create the config file: {e}"))?;
    // The file has no extension, so Windows has no default app for it:
    // Notepad is always there.
    #[cfg(windows)]
    let mut cmd = std::process::Command::new("notepad.exe");
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = std::process::Command::new("/usr/bin/open");
        c.arg("-t");
        c
    };
    cmd.arg(&path)
        .spawn()
        .map_err(|e| format!("Couldn't open the config file: {e}"))?;
    Ok(path)
}

/// Check a "Connect your blyg" URL: http(s) with a host. Returns it
/// normalised (no trailing slash).
pub fn validate_blyg_url(url: &str) -> Result<String, String> {
    let spec = blyg_core::config::keys::spec("blyg-url").expect("blyg-url key");
    match blyg_core::config::parse::validate(spec, url.trim()) {
        Ok((Some(v), _)) => Ok(v),
        Ok((None, _)) => Err("Enter your blyg's address, e.g. https://blyg.example.com".into()),
        Err(e) => Err(e),
    }
}

/// Store the credential (password or token) in the Keychain, then point the
/// config at the blyg.
pub fn connect(
    url: &str,
    cred: &blyg_core::api::auth::Credential,
    cx: &mut App,
) -> Result<String, String> {
    use blyg_core::api::auth::Credential;
    let url = validate_blyg_url(url)?;
    let empty = match cred {
        Credential::Token(t) => t.trim().is_empty(),
        Credential::Password(p) => p.is_empty(),
        Credential::OAuth(s) => s.grant().access.is_empty(),
    };
    if empty {
        return Err("Enter the studio password or the owner token".into());
    }
    blyg_core::config::save_credential(get(cx).tokens.as_ref(), &url, cred)
        .map_err(|e| format!("Couldn't save it in the Keychain: {e}"))?;
    write(&[("blyg-url", Change::Set(url.clone()))], cx)?;
    Ok(url)
}

/// Migrate the old data dir and the old TOML files (first launch after the
/// switch to one config file). Returns the notice to show, if any.
pub fn migrate_on_launch(primary: &std::path::Path) -> Option<String> {
    use blyg_core::config::{migrate, paths};
    let data = paths::data_dir();
    if std::env::var_os("BLYGGER_DATA_DIR").is_none()
        && let Err(e) = paths::migrate_data_dir(&paths::legacy_data_dir(), &data)
    {
        eprintln!("blygger: couldn't move the old data folder: {e}");
    }
    match migrate::migrate_legacy_files(&data, primary) {
        Ok(Some(r)) if !r.migrated.is_empty() => Some(format!(
            "Settings moved to {} (old files renamed *.migrated)",
            paths::tilde(&r.target)
        )),
        Ok(Some(r)) => {
            for p in r.problems {
                eprintln!("blygger: {p}");
            }
            None
        }
        Ok(None) => None,
        Err(e) => {
            eprintln!("blygger: couldn't migrate the old settings: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_checks_fonts_and_hotkeys() {
        // A system font, as this platform names it (any case).
        let ui = if cfg!(target_os = "windows") {
            "segoe ui"
        } else {
            "sf pro"
        };
        let s = ConfigStore::in_memory(&format!(
            "font-family-writing = Comic Sans\nfont-family-ui = {ui}\ncapture-hotkey = ctrl+alt+nope\nbogus = 1\n",
        ));
        let d = diagnostics(s.loaded(), &Registry::builtin());
        let lines: Vec<(usize, Severity)> = d.iter().map(|d| (d.line, d.severity)).collect();
        assert_eq!(
            lines,
            vec![
                (1, Severity::Warning),
                (3, Severity::Error),
                (4, Severity::Warning)
            ],
            "{d:#?}"
        );
        assert!(d[0].message.contains("+list-fonts"));
    }

    #[test]
    fn unknown_themes_and_theme_fonts_are_reported() {
        let s = ConfigStore::in_memory("theme = cutaway\ntheme-dark = midnight\ntheme = sepia\n");
        let mut themes = Registry::builtin();
        themes.add_user(
            "mine",
            "font-writing = Comic Sans\nfont-ui = Menlo\ncolor-bg = nope\n",
            std::path::Path::new("/t/themes/mine"),
        );
        let d = diagnostics(s.loaded(), &themes);
        let got: Vec<(String, usize)> = d
            .iter()
            .map(|d| (d.file.display().to_string(), d.line))
            .collect();
        assert_eq!(
            got,
            vec![
                ("/t/themes/mine".to_string(), 1),
                ("/t/themes/mine".to_string(), 3),
                ("config".to_string(), 2),
                ("config".to_string(), 3),
            ],
            "{d:#?}"
        );
        assert!(d[0].message.contains("unknown font `Comic Sans`"));
        assert!(
            d[2].message
                .starts_with("theme-dark: unknown theme `midnight`")
        );
    }

    #[test]
    fn the_watch_stamp_changes_when_a_theme_file_does() {
        let dir = tempfile::tempdir().unwrap();
        let themes = dir.path().join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        let paths = vec![dir.path().join("config"), themes.clone()];
        let a = watch_stamp(&paths);
        assert_eq!(a, watch_stamp(&paths));
        std::fs::write(themes.join("mine"), "inherit = kumiko\n").unwrap();
        let b = watch_stamp(&paths);
        assert_ne!(a, b);
        std::fs::write(themes.join("mine"), "inherit = kumiko\nradius = 3\n").unwrap();
        assert_ne!(b, watch_stamp(&paths));
    }

    #[test]
    fn blyg_urls() {
        assert_eq!(
            validate_blyg_url(" https://blyg.example.com/ ").unwrap(),
            "https://blyg.example.com"
        );
        assert!(validate_blyg_url("").is_err());
        assert!(validate_blyg_url("blyg.example.com").is_err());
    }
}
