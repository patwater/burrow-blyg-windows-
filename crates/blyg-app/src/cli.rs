//! Ghostty-style `+actions`. `blygger` with no `+action` starts the app.
//!
//! - `blygger +show-config [--default] [--docs] [--changes-only=false]`
//! - `blygger +validate-config`
//! - `blygger +list-fonts`
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

With no action, Blygger starts. Actions:
  +show-config            the effective config (only what you changed)
      --default           show the defaults instead
      --docs              include each option's documentation
      --changes-only=false  show every option, not just the ones you set
  +validate-config        check the config file(s) for problems
  +list-fonts             fonts for font-family-writing / font-family-ui
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
        "+list-keybinds" => o.stdout = crate::keymap::list(),
        "+show-config" => match ShowOptions::from_args(args.iter().copied()) {
            Ok(opts) => {
                let store = load();
                o.stdout = show_config(store.config(), opts);
                if !opts.default {
                    for d in crate::settings::diagnostics(store.loaded()) {
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
            let diags = crate::settings::diagnostics(store.loaded());
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

#[cfg(test)]
mod tests {
    use super::*;

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
