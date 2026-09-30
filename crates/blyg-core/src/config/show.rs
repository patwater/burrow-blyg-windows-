//! `blygger +show-config`: print the effective config, or (with `--default
//! --docs`) every key with its default and documentation, straight from the
//! key table.

use super::keys::{KEYS, KeySpec};
use super::parse::{Config, quote};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShowOptions {
    /// Show the defaults instead of the loaded config.
    pub default: bool,
    /// Precede each key with its documentation.
    pub docs: bool,
    /// Only keys the user set (ignored with `default`).
    pub changes_only: bool,
}

impl Default for ShowOptions {
    fn default() -> Self {
        ShowOptions {
            default: false,
            docs: false,
            changes_only: true,
        }
    }
}

impl ShowOptions {
    /// Parse `--default`, `--docs`, `--changes-only[=true|false]`.
    pub fn from_args<'a>(args: impl IntoIterator<Item = &'a str>) -> Result<ShowOptions, String> {
        let mut o = ShowOptions::default();
        for a in args {
            match a {
                "--default" => o.default = true,
                "--docs" => o.docs = true,
                "--changes-only" | "--changes-only=true" => o.changes_only = true,
                "--changes-only=false" => o.changes_only = false,
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(o)
    }
}

fn value_lines(k: &KeySpec, values: &[String]) -> Vec<String> {
    if values.is_empty() {
        return vec![format!("{} =", k.name)];
    }
    values
        .iter()
        .map(|v| format!("{} = {}", k.name, quote(v)))
        .collect()
}

fn defaults(k: &KeySpec) -> Vec<String> {
    if k.repeatable {
        k.default_list.iter().map(|s| s.to_string()).collect()
    } else {
        k.default.map(|d| vec![d.to_string()]).unwrap_or_default()
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && line.len() + 1 + word.len() > width {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

pub fn show_config(cfg: &Config, o: ShowOptions) -> String {
    let mut out = String::new();
    for k in KEYS {
        let values = if o.default {
            defaults(k)
        } else if cfg.is_set(k.name) {
            if k.repeatable {
                cfg.list(k.name)
            } else {
                cfg.get(k.name)
                    .map(|v| vec![v.to_string()])
                    .unwrap_or_default()
            }
        } else if o.changes_only {
            continue;
        } else {
            defaults(k)
        };
        if o.docs {
            for l in wrap(k.docs, 76) {
                if l.is_empty() {
                    out.push_str("#\n");
                } else {
                    out.push_str(&format!("# {l}\n"));
                }
            }
        }
        for l in value_lines(k, &values) {
            out.push_str(&l);
            out.push('\n');
        }
        if o.docs {
            out.push('\n');
        }
    }
    out
}
