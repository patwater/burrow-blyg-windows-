//! The documentation site's shortcut table (`site/src/generated/keybindings.md`)
//! is generated from `keymap::table()`, the same table that binds the keys and
//! builds the menus and buttons, so it can't drift.
//!
//! This test fails when the checked-in file is out of date. Regenerate it with
//! `scripts/gen-docs.sh` (which runs it with `BLYGGER_UPDATE_DOCS=1`).

use std::path::PathBuf;

use crate::keymap::{Scope, glyphs, table};

fn doc_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../site/src/generated/keybindings.md")
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// `glyphs`, with arrow keys drawn as arrows.
fn key(k: &str) -> String {
    glyphs(k)
        .replace("RIGHT", "→")
        .replace("LEFT", "←")
        .replace("UP", "↑")
        .replace("DOWN", "↓")
}

fn render() -> String {
    let mut out = String::from(
        "<!-- Generated from crates/blyg-app/src/keymap.rs by scripts/gen-docs.sh.\n     \
         Don't edit it by hand: cargo test fails when it's out of date. -->\n",
    );
    let groups = [
        (Scope::Global, "Anywhere"),
        (Scope::Main, "In the main window"),
        (
            Scope::MainInput,
            "While typing in the editor or a text field",
        ),
        (Scope::Browser, "In the browser pane"),
    ];
    let rows = table();
    for (scope, heading) in groups {
        let keyed: Vec<_> = rows
            .iter()
            .filter(|k| k.scope == scope && !k.key.is_empty())
            // Developer-only keys (sample data) aren't for the user docs.
            .filter(|k| !k.label.contains("BLYGGER_FAKE"))
            .collect();
        if keyed.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "\n### {heading}\n\n| Key | What it does | Menu |\n|---|---|---|\n"
        ));
        for k in keyed {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                cell(&key(k.key)),
                cell(k.label),
                cell(k.menu.unwrap_or(""))
            ));
        }
    }
    let menu_only: Vec<_> = rows
        .iter()
        .filter(|k| k.key.is_empty() && k.menu.is_some())
        .collect();
    if !menu_only.is_empty() {
        out.push_str("\n### In the menus, without a key\n\n| What it does | Menu |\n|---|---|\n");
        for k in menu_only {
            out.push_str(&format!(
                "| {} | {} |\n",
                cell(k.label),
                cell(k.menu.unwrap_or(""))
            ));
        }
    }
    out
}

#[test]
fn shortcut_reference_is_up_to_date() {
    let want = render();
    let path = doc_path();
    if std::env::var_os("BLYGGER_UPDATE_DOCS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &want).unwrap();
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "{} is out of date with src/keymap.rs. Run scripts/gen-docs.sh and commit the result.",
        path.display()
    );
}
