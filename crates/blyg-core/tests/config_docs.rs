//! The documentation site's configuration reference
//! (`site/src/generated/config-keys.md`) is generated from the key table in
//! `src/config/keys.rs`, so it can't drift from the defaults.
//!
//! This test fails when the checked-in file is out of date. Regenerate it with
//! `scripts/gen-docs.sh` (which runs this test with `BLYGGER_UPDATE_DOCS=1`).

use std::path::PathBuf;

use blyg_core::config::keys::{KEYS, KeySpec, ValueKind};

fn doc_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../site/src/generated/config-keys.md")
}

fn code(s: &str) -> String {
    format!("`{s}`")
}

fn values(k: &KeySpec) -> String {
    match k.kind {
        ValueKind::Text => "text".into(),
        ValueKind::Url => "an `http(s)://` address".into(),
        ValueKind::Bool => "`true` or `false`".into(),
        ValueKind::Number { min, max } => format!("a number from {min} to {max}"),
        ValueKind::Choice(opts) => opts.iter().map(|o| code(o)).collect::<Vec<_>>().join(", "),
        ValueKind::Hotkey => "a key combination, such as `ctrl+alt+b`".into(),
        ValueKind::Path => "a file path (`~/` is expanded; a leading `?` makes it optional)".into(),
        ValueKind::ProviderModel => "`provider=model`".into(),
        ValueKind::ThemeName => "`system`, `light`, `dark`, a built-in theme (`cutaway`, `kumiko`, `shola`, `fortress`, `portolan`, `aizome`, `saltspace`, `konkan`) or a theme file's name (see Themes)".into(),
    }
}

fn default(k: &KeySpec) -> String {
    if k.repeatable {
        if k.default_list.is_empty() {
            "none".into()
        } else {
            k.default_list
                .iter()
                .map(|d| code(d))
                .collect::<Vec<_>>()
                .join(", ")
        }
    } else {
        k.default.map(code).unwrap_or_else(|| "unset".into())
    }
}

fn render() -> String {
    let mut out = String::from(
        "<!-- Generated from crates/blyg-core/src/config/keys.rs by scripts/gen-docs.sh.\n     \
         Don't edit it by hand: cargo test fails when it's out of date. -->\n",
    );
    for k in KEYS {
        out.push_str(&format!("\n### `{}`\n\n", k.name));
        out.push_str(&format!(
            "- **Values:** {}\n- **Default:** {}\n",
            values(k),
            default(k)
        ));
        if k.repeatable {
            out.push_str("- **Repeatable:** yes, one value per line\n");
        }
        out.push('\n');
        for para in k.docs.split('\n') {
            out.push_str(&para.trim().replace('<', "&lt;"));
            out.push_str("\n\n");
        }
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

#[test]
fn config_reference_is_up_to_date() {
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
        "{} is out of date with src/config/keys.rs. Run scripts/gen-docs.sh and commit the result.",
        path.display()
    );
}
