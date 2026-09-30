//! One-time migration of the old per-crate TOML files (`config.toml` from
//! blyg-core, `ui.toml` from the app, `ai.toml` from blyg-ai) into the one
//! config file. Translated keys are appended to the config file under a
//! comment naming their source, along with the old file's own comments.
//! Each migrated file is renamed `*.migrated`, so this runs once. A file
//! that can't be parsed is left alone and reported.

use std::path::{Path, PathBuf};

use super::edit::{HEADER, atomic_write};
use super::keys::spec;
use super::parse::{fmt_num, parse_text, quote, validate};

/// Old files, in the order they're migrated.
pub const LEGACY_FILES: [&str; 3] = ["config.toml", "ui.toml", "ai.toml"];

/// Providers ai.toml treated as on unless it said otherwise.
const LEGACY_AI_TOML_ENABLED: [&str; 2] = ["claude-code", "codex"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// The config file the settings went into.
    pub target: PathBuf,
    /// The old files, now renamed `*.migrated`.
    pub migrated: Vec<PathBuf>,
    /// Files that couldn't be read or parsed (left in place).
    pub problems: Vec<String>,
}

/// Old ai.toml provider slugs → config names.
fn provider_name(slug: &str) -> Option<&'static str> {
    Some(match slug {
        "chatgpt-account" => "chatgpt",
        "openai-api" => "openai",
        "anthropic-api" => "anthropic",
        "local-claude-code" => "claude-code",
        "local-codex" => "codex",
        "blyg-server" => "server",
        "cloudflare-workers-ai" => "cloudflare",
        _ => return None,
    })
}

/// A line of the migrated block: a setting, or a comment.
enum Out {
    Set(&'static str, String),
    Comment(String),
}

fn scalar(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Float(f) => fmt_num(*f as f32),
        toml::Value::Integer(i) => i.to_string(),
        other => other.to_string(),
    }
}

/// `ui.toml` keys (also accepted in `config.toml`'s `[ui]` table).
fn ui_key(name: &str) -> Option<&'static str> {
    Some(match name {
        "writing_font" => "font-family-writing",
        "ui_font" => "font-family-ui",
        "font_size" => "font-size",
        "theme" => "theme",
        "hotkey" => "capture-hotkey",
        _ => return None,
    })
}

fn translate_ui(table: &toml::Table, prefix: &str, out: &mut Vec<Out>) {
    for (k, v) in table {
        match ui_key(k) {
            Some(key) => out.push(Out::Set(key, scalar(v))),
            None => out.push(Out::Comment(format!("not migrated: {prefix}{k} = {v}"))),
        }
    }
}

fn translate(name: &str, table: &toml::Table, out: &mut Vec<Out>, notice_shown: &mut bool) {
    match name {
        "ui.toml" => translate_ui(table, "", out),
        "config.toml" => {
            for (k, v) in table {
                match (k.as_str(), v) {
                    ("base_url", v) => out.push(Out::Set("blyg-url", scalar(v))),
                    ("ui", toml::Value::Table(t)) => translate_ui(t, "[ui] ", out),
                    (k, v) => out.push(Out::Comment(format!("not migrated: {k} = {v}"))),
                }
            }
        }
        "ai.toml" => {
            let default = table
                .get("default_provider")
                .and_then(|v| v.as_str())
                .and_then(provider_name);
            if let Some(d) = default {
                out.push(Out::Set("ai-provider", d.to_string()));
            }
            let default_enabled = spec("ai-enable").map(|s| s.default_list).unwrap_or(&[]);
            // ai.toml's own default had the local CLI bridges on; keep what
            // the user had rather than the (now opt-in) config default.
            let mut enabled: Vec<String> = LEGACY_AI_TOML_ENABLED
                .iter()
                .map(|s| s.to_string())
                .collect();
            if let Some(providers) = table.get("providers").and_then(|v| v.as_table()) {
                for (slug, settings) in providers {
                    let Some(p) = provider_name(slug) else {
                        out.push(Out::Comment(format!(
                            "not migrated: unknown provider {slug}"
                        )));
                        continue;
                    };
                    let Some(s) = settings.as_table() else {
                        continue;
                    };
                    match s.get("enabled").and_then(|v| v.as_bool()) {
                        Some(true) if !enabled.iter().any(|e| e == p) => enabled.push(p.into()),
                        Some(false) => enabled.retain(|e| e != p),
                        _ => {}
                    }
                    if let Some(m) = s.get("model").and_then(|v| v.as_str()) {
                        if Some(p) == default {
                            out.push(Out::Set("ai-model", m.to_string()));
                        } else {
                            out.push(Out::Set("ai-provider-model", format!("{p}={m}")));
                        }
                    }
                }
            }
            let mut sorted_default: Vec<&str> = default_enabled.to_vec();
            sorted_default.sort();
            let mut sorted: Vec<&str> = enabled.iter().map(String::as_str).collect();
            sorted.sort();
            if sorted != sorted_default {
                if enabled.is_empty() {
                    out.push(Out::Set("ai-enable", String::new()));
                }
                for e in enabled {
                    out.push(Out::Set("ai-enable", e));
                }
            }
            if let Some(id) = table.get("cloudflare_account_id").and_then(|v| v.as_str()) {
                out.push(Out::Set("cloudflare-account-id", id.to_string()));
            }
            if table
                .get("chatgpt_notice_shown")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                *notice_shown = true;
            }
            for k in table.keys() {
                if !matches!(
                    k.as_str(),
                    "default_provider"
                        | "providers"
                        | "cloudflare_account_id"
                        | "chatgpt_notice_shown"
                ) {
                    out.push(Out::Comment(format!("not migrated: {k}")));
                }
            }
        }
        _ => {}
    }
}

/// Migrate any legacy files in `data_dir` into `target`. `Ok(None)` when
/// there was nothing to migrate.
pub fn migrate_legacy_files(
    data_dir: &Path,
    target: &Path,
) -> std::io::Result<Option<MigrationReport>> {
    let present: Vec<&str> = LEGACY_FILES
        .into_iter()
        .filter(|f| data_dir.join(f).is_file())
        .collect();
    if present.is_empty() {
        return Ok(None);
    }
    let mut report = MigrationReport {
        target: target.to_path_buf(),
        ..Default::default()
    };
    let mut text = match std::fs::read_to_string(target) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HEADER.to_string(),
        Err(e) => return Err(e),
    };
    let (existing, _) = parse_text(&text, target);
    let already: std::collections::HashSet<String> = existing.into_iter().map(|e| e.key).collect();

    let mut done = Vec::new();
    let mut notice_shown = false;
    for name in present {
        let path = data_dir.join(name);
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                report.problems.push(format!("{name}: {e}"));
                continue;
            }
        };
        let table: toml::Table = match toml::from_str(&src) {
            Ok(t) => t,
            Err(e) => {
                report
                    .problems
                    .push(format!("{name} wasn't migrated: {}", e.message()));
                continue;
            }
        };
        let mut out = Vec::new();
        translate(name, &table, &mut out, &mut notice_shown);

        let mut block = String::new();
        if !text.ends_with("\n\n") {
            block.push('\n');
        }
        block.push_str(&format!("# Migrated from {name}\n"));
        for c in src.lines().map(str::trim).filter(|l| l.starts_with('#')) {
            block.push_str(c);
            block.push('\n');
        }
        for o in out {
            match o {
                Out::Comment(c) => block.push_str(&format!("# {c}\n")),
                Out::Set(key, value) => {
                    let Some(s) = spec(key) else { continue };
                    match validate(s, &value) {
                        Ok((v, _)) => {
                            let v = v.unwrap_or_default();
                            let line = if v.is_empty() {
                                format!("{key} =")
                            } else {
                                format!("{key} = {}", quote(&v))
                            };
                            if already.contains(key) {
                                block.push_str(&format!("# already set above: {line}\n"));
                            } else {
                                block.push_str(&line);
                                block.push('\n');
                            }
                        }
                        Err(e) => block.push_str(&format!("# not migrated ({e}): {value}\n")),
                    }
                }
            }
        }
        text.push_str(&block);
        done.push(path);
    }
    if !done.is_empty() {
        atomic_write(target, &text)?;
        for path in done {
            let mut renamed = path.as_os_str().to_owned();
            renamed.push(".migrated");
            let renamed = PathBuf::from(renamed);
            std::fs::rename(&path, &renamed)?;
            report.migrated.push(renamed);
        }
    }
    if notice_shown {
        let _ = crate::state::AppState::update(data_dir, |s| s.chatgpt_notice_shown = true);
    }
    Ok(Some(report))
}
