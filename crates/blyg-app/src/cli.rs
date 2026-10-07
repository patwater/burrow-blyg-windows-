//! Ghostty-style `+actions`. `blygger` with no `+action` starts the app.
//!
//! - `blygger +show-config [--default] [--docs] [--changes-only=false]`
//! - `blygger +validate-config`
//! - `blygger +list-fonts`
//! - `blygger +list-themes`
//! - `blygger +copy-theme <built-in> [name]`
//! - `blygger +list-keybinds`
//! - `blygger +version`
//! - `blygger +help`

use std::process::ExitCode;

use blyg_core::ConfigStore;
use blyg_core::config::show::{ShowOptions, show_config};

use crate::prefs::{UI_FONTS, WRITING_FONTS};

/// What an action printed and how it exits.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub stdout: String,
    pub stderr: String,
    pub code: u8,
}

/// `None` when the arguments don't name a `+action` (launch the app).
pub fn run(args: &[String]) -> Option<ExitCode> {
    let first = args.iter().find(|a| a.starts_with('+'))?;
    let rest: Vec<&str> = args
        .iter()
        .skip_while(|a| *a != first)
        .skip(1)
        .map(String::as_str)
        .collect();
    attach_parent_console();
    let out = exec(first, &rest, &mut ConfigStore::discover);
    // The docs name keys and places as macOS does; Windows respells them.
    print!("{}", crate::keymap::hint_owned(out.stdout));
    eprint!("{}", crate::keymap::hint_owned(out.stderr));
    Some(ExitCode::from(out.code))
}

/// A release build on Windows is a GUI program with no console of its own;
/// print to the terminal that ran `blygger +action`, if there is one.
fn attach_parent_console() {
    #[cfg(target_os = "windows")]
    {
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        unsafe extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }
        // SAFETY: a plain Win32 call; failure (no parent console, or one
        // already attached in a debug build) just leaves output unseen.
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
}

const HELP: &str = "\
Usage: blygger [+action [options]]

With no action, Burrow starts. Actions:
  +show-config            the effective config (only what you changed)
      --default           show the defaults instead
      --docs              include each option's documentation
      --changes-only=false  show every option, not just the ones you set
  +validate-config        check the config file(s) for problems
  +list-fonts             fonts for font-family-writing / font-family-ui
  +list-themes            every theme, built in and yours
  +copy-theme <built-in> [name]
                          copy a built-in theme into the themes folder to edit
  +list-keybinds          every keyboard shortcut, and the ones reserved
  +version                print the version
  +help                   this help

Discover every option with: blygger +show-config --default --docs
";

pub fn exec(action: &str, args: &[&str], load: &mut dyn FnMut() -> ConfigStore) -> Outcome {
    let mut o = Outcome::default();
    match action {
        "+version" => {
            o.stdout = format!("blygger {}\n", env!("CARGO_PKG_VERSION"));
        }
        "+help" => o.stdout = HELP.to_string(),
        // --- browser --- internal (not in +help): the app runs this as a
        // child process to convert the block lists (browser/driver.rs).
        "+convert-blocklists" => {
            let out_dir = args.first().map(std::path::PathBuf::from);
            let unless = args.iter().find_map(|a| a.strip_prefix("--unless-key="));
            let data_dir = blyg_core::config::data_dir();
            match out_dir
                .map(|d| crate::app::browser::blocklist::convert_to_dir(&data_dir, &d, unless))
            {
                Some(Ok(r)) => o.stdout = serde_json::to_string(&r).unwrap_or_default() + "\n",
                Some(Err(e)) => {
                    o.stderr = format!("blygger +convert-blocklists: {e}\n");
                    o.code = 1;
                }
                None => {
                    o.stderr = "usage: blygger +convert-blocklists <out dir>\n".into();
                    o.code = 2;
                }
            }
        }
        "+list-keybinds" => o.stdout = crate::keymap::list(),
        "+show-config" => match ShowOptions::from_args(args.iter().copied()) {
            Ok(opts) => {
                let store = load();
                o.stdout = show_config(store.config(), opts);
                if !opts.default {
                    for d in crate::settings::diagnostics(store.loaded(), &themes_of(&store)) {
                        o.stderr.push_str(&format!("{d}\n"));
                    }
                }
            }
            Err(e) => {
                o.stderr = format!("blygger +show-config: {e}\n");
                o.code = 2;
            }
        },
        "+validate-config" => {
            let store = load();
            let diags = crate::settings::diagnostics(store.loaded(), &themes_of(&store));
            for d in &diags {
                o.stdout.push_str(&format!("{d}\n"));
            }
            if diags.iter().any(|d| d.is_error()) {
                o.code = 1;
            } else if diags.is_empty() {
                let files: Vec<String> = store
                    .loaded()
                    .files
                    .iter()
                    .map(|f| blyg_core::config::paths::tilde(f))
                    .collect();
                o.stdout = if files.is_empty() {
                    format!(
                        "No config file yet (it would be {}); using the defaults.\n",
                        blyg_core::config::paths::tilde(store.primary())
                    )
                } else {
                    format!("OK: {}\n", files.join(", "))
                };
            }
        }
        "+list-themes" => {
            let store = load();
            let themes = themes_of(&store);
            let all = crate::theme::Themes::new(themes.clone());
            for (family, list) in all.grouped() {
                o.stdout.push_str(&format!("{}:\n", family.label()));
                if family == blyg_core::config::theme::Family::Plain {
                    o.stdout.push_str(&format!(
                        "  {:<14} follows macOS: light or dark\n",
                        "system"
                    ));
                }
                for t in list {
                    let whose = if t.builtin { "built-in" } else { "yours" };
                    o.stdout
                        .push_str(&format!("  {:<14} {} ({whose})\n", t.id, t.name));
                }
            }
            if let Some(d) = &themes.dir {
                o.stdout.push_str(&format!(
                    "\nYour themes live in {}\n",
                    blyg_core::config::paths::tilde(d)
                ));
            }
        }
        "+copy-theme" => match copy_theme(args, &load()) {
            Ok(msg) => o.stdout = msg,
            Err((code, e)) => {
                o.stderr = format!("blygger +copy-theme: {e}\n");
                o.code = code;
            }
        },
        "+list-fonts" => {
            let list = |title: &str, fonts: &[crate::prefs::FontChoice], o: &mut Outcome| {
                o.stdout.push_str(&format!("{title}\n"));
                for f in fonts {
                    let note = if f.bundled.is_some() {
                        "bundled"
                    } else if cfg!(target_os = "windows") {
                        "Windows"
                    } else {
                        "macOS"
                    };
                    o.stdout.push_str(&format!("  {:<20} {note}\n", f.label));
                }
            };
            list("font-family-writing:", WRITING_FONTS, &mut o);
            o.stdout.push('\n');
            list("font-family-ui:", UI_FONTS, &mut o);
        }
        other => {
            o.stderr = format!("blygger: unknown action {other}\n\n{HELP}");
            o.code = 2;
        }
    }
    o
}

/// The built-in themes plus the ones in this config's themes folder.
fn themes_of(store: &ConfigStore) -> blyg_core::config::theme::Registry {
    use blyg_core::config::theme::Registry;
    match store.themes_dir() {
        Some(d) => Registry::load(&d),
        None => Registry::builtin(),
    }
}

/// `+copy-theme <built-in> [name]`: write a built-in theme out as a file
/// in the themes folder (never over an existing one).
fn copy_theme(args: &[&str], store: &ConfigStore) -> Result<String, (u8, String)> {
    use blyg_core::config::theme::{BUILTIN, builtin_text, valid_name};
    let names = || {
        BUILTIN
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let usage = || {
        (
            2,
            format!(
                "usage: blygger +copy-theme <built-in> [name]\nbuilt-ins: {}",
                names()
            ),
        )
    };
    let from = args.first().ok_or_else(usage)?.to_ascii_lowercase();
    let text = builtin_text(&from).ok_or_else(|| {
        (
            2,
            format!("`{from}` isn't a built-in theme (one of {})", names()),
        )
    })?;
    let to = args
        .get(1)
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| from.clone());
    if !valid_name(&to) {
        return Err((
            2,
            format!("`{to}` isn't a theme name (use lowercase letters, digits and -)"),
        ));
    }
    let dir = store
        .themes_dir()
        .ok_or_else(|| (1, "no themes folder for this config".to_string()))?;
    let path = dir.join(&to);
    if path.exists() {
        return Err((
            1,
            format!(
                "{} already exists; pass another name",
                blyg_core::config::paths::tilde(&path)
            ),
        ));
    }
    let mut out = format!(
        "# Copied from the built-in {from} theme. Edit it and save: the app reloads it.\n\
         # Use it with `theme = {to}` in the config. The format: docs/THEMES.md.\n"
    );
    for line in text.lines() {
        if to != from && line.trim_start().starts_with("name ") {
            out.push_str(&format!("name = {to}\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(&path, out))
        .map_err(|e| (1, format!("couldn't write {}: {e}", path.display())))?;
    Ok(format!(
        "Wrote {}\nUse it with: theme = {to}\n",
        blyg_core::config::paths::tilde(&path)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on_disk(dir: &std::path::Path) -> impl FnMut() -> ConfigStore + '_ {
        move || ConfigStore::open(blyg_core::config::ConfigFiles::single(dir.join("config")))
    }

    #[test]
    fn copy_theme_round_trips_every_builtin() {
        use blyg_core::config::theme::{BUILTIN, Registry};
        let dir = tempfile::tempdir().unwrap();
        let builtins = Registry::builtin();
        for (id, _) in BUILTIN {
            let name = format!("my-{id}");
            let o = exec("+copy-theme", &[id, &name], &mut on_disk(dir.path()));
            assert_eq!(o.code, 0, "{}", o.stderr);
            assert!(o.stdout.contains(&format!("theme = {name}")));
        }
        // And one under its own name (it then replaces the built-in).
        assert_eq!(
            exec("+copy-theme", &["kumiko"], &mut on_disk(dir.path())).code,
            0
        );
        let loaded = Registry::load(&dir.path().join("themes"));
        assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
        for (id, _) in BUILTIN {
            let copy = loaded.resolve(&format!("my-{id}")).unwrap();
            let orig = builtins.resolve(id).unwrap();
            assert_eq!(
                copy.with_identity(&orig.id, &orig.name, true),
                orig,
                "{id} round-trips"
            );
        }
        assert!(!loaded.resolve("kumiko").unwrap().builtin);

        let again = exec("+copy-theme", &["kumiko"], &mut on_disk(dir.path()));
        assert_eq!(again.code, 1);
        assert!(again.stderr.contains("already exists"));
        assert_eq!(exec("+copy-theme", &[], &mut on_disk(dir.path())).code, 2);
        assert_eq!(
            exec("+copy-theme", &["sepia"], &mut on_disk(dir.path())).code,
            2
        );
        assert_eq!(
            exec(
                "+copy-theme",
                &["aizome", "Bad Name"],
                &mut on_disk(dir.path())
            )
            .code,
            2
        );

        let l = exec("+list-themes", &[], &mut on_disk(dir.path()));
        assert!(l.stdout.contains("Woody:\n  cutaway"), "{}", l.stdout);
        assert!(l.stdout.contains("my-konkan"), "{}", l.stdout);
        assert!(l.stdout.contains("(yours)"));
    }

    fn with(text: &'static str) -> impl FnMut() -> ConfigStore {
        move || ConfigStore::in_memory(text)
    }

    #[test]
    fn actions() {
        let v = exec("+version", &[], &mut with(""));
        assert!(v.stdout.starts_with("blygger "));
        assert_eq!(v.code, 0);

        let f = exec("+list-fonts", &[], &mut with(""));
        assert!(f.stdout.contains("font-family-writing:\n  Literata"));
        assert!(f.stdout.contains("font-family-ui:\n  Inter"));

        let bad = exec("+frobnicate", &[], &mut with(""));
        assert_eq!(bad.code, 2);
        assert!(bad.stderr.contains("unknown action +frobnicate"));
        assert!(run(&["-psn_0_123".into()]).is_none(), "no +action: launch");
    }

    #[test]
    fn show_config_prints_changes_and_docs() {
        let s = exec("+show-config", &[], &mut with("# hi\ntheme = dark\n"));
        assert_eq!(s.stdout, "theme = dark\n");
        let d = exec(
            "+show-config",
            &["--default", "--docs"],
            &mut with("theme = dark\n"),
        );
        assert!(d.stdout.contains("\ntheme = system\n"));
        assert!(d.stdout.contains("# Colour theme"));
        assert_eq!(exec("+show-config", &["--nope"], &mut with("")).code, 2);
    }

    #[test]
    fn validate_config_exit_codes() {
        let ok = exec("+validate-config", &[], &mut with("theme = dark\n"));
        assert_eq!(ok.code, 0);
        let warn = exec("+validate-config", &[], &mut with("fnot-size = 3\n"));
        assert_eq!(warn.code, 0);
        assert!(
            warn.stdout
                .contains("config:1: warning: unknown key `fnot-size`")
        );
        let err = exec(
            "+validate-config",
            &[],
            &mut with("theme = dark\ncapture-hotkey = ctrl+alt+nope\n"),
        );
        assert_eq!(err.code, 1);
        assert!(
            err.stdout.contains("config:2: error: capture-hotkey"),
            "{}",
            err.stdout
        );
    }
}
