//! Comment-preserving write-back. Only the lines of the keys that changed are
//! touched; every other line (comments, blank lines, ordering, unknown keys)
//! is kept byte for byte. Keys the file doesn't mention yet are appended at
//! the end.

use super::parse::{parse_line, quote};

/// One change to a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Set a single value (replaces the last line for the key in place).
    Set(String),
    /// Replace every line for a repeatable key with these values, at the
    /// position of the first existing line. An empty list writes `key =`
    /// (an explicit empty list).
    List(Vec<String>),
    /// Remove every line for the key (back to the default).
    Remove,
}

/// The header of a config file the app creates.
#[cfg(not(windows))]
pub const HEADER: &str = "\
# Blygger configuration. One `key = value` per line; `#` starts a comment.
# See every option, with its docs and default:
#     blygger +show-config --default --docs
# Reload after editing with ⌘⇧, in the app. Secrets (tokens, API keys) are
# kept in the macOS Keychain, never here.
";

/// The header of a config file the app creates.
#[cfg(windows)]
pub const HEADER: &str = "\
# Blygger configuration. One `key = value` per line; `#` starts a comment.
# See every option, with its docs and default:
#     blygger +show-config --default --docs
# Reload after editing with Ctrl+Shift+, in the app. Secrets (tokens, API
# keys) are kept in Windows Credential Manager, never here.
";

fn line_for(key: &str, value: &str) -> String {
    format!("{key} = {}", quote(value))
}

/// Apply `changes` to the text of a config file.
pub fn apply(text: &str, changes: &[(&str, Change)]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut appended: Vec<String> = Vec::new();
    for (key, change) in changes {
        let hits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| matches!(parse_line(l), Ok(Some((k, _))) if k == *key))
            .map(|(i, _)| i)
            .collect();
        match change {
            Change::Set(v) => match hits.last() {
                Some(&i) => {
                    let indent: String =
                        lines[i].chars().take_while(|c| c.is_whitespace()).collect();
                    lines[i] = format!("{indent}{}", line_for(key, v));
                }
                None => appended.push(line_for(key, v)),
            },
            Change::Remove => {
                for &i in hits.iter().rev() {
                    lines.remove(i);
                }
            }
            Change::List(vs) => {
                let new: Vec<String> = if vs.is_empty() {
                    vec![format!("{key} =")]
                } else {
                    vs.iter().map(|v| line_for(key, v)).collect()
                };
                match hits.first() {
                    Some(&first) => {
                        for &i in hits.iter().rev() {
                            lines.remove(i);
                        }
                        for (n, l) in new.into_iter().enumerate() {
                            lines.insert(first + n, l);
                        }
                    }
                    None => appended.extend(new),
                }
            }
        }
    }
    if !appended.is_empty() {
        if lines.last().is_some_and(|l| !l.trim().is_empty()) {
            // Keep appended keys visually separate from a trailing block.
            if !lines
                .last()
                .is_some_and(|l| matches!(parse_line(l), Ok(Some(_))))
            {
                lines.push(String::new());
            }
        }
        lines.extend(appended);
    }
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Apply `changes` to the file at `path` atomically (temp file + rename).
/// A missing file is created with [`HEADER`].
pub fn write_file(path: &std::path::Path, changes: &[(&str, Change)]) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => format!("{HEADER}\n"),
        Err(e) => return Err(e),
    };
    let new = apply(&text, changes);
    atomic_write(path, &new)
}

pub fn atomic_write(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}
