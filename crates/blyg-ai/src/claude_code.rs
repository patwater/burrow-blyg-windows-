//! Local Claude Code bridge: the supported way to use a Claude Pro/Max
//! subscription. Runs the user's installed `claude` in print mode, text only.
//!
//! Flags (code.claude.com/docs/en/headless and `claude --help`, v2.1.282):
//! - `-p --output-format stream-json --verbose --include-partial-messages`:
//!   newline-delimited events; text arrives as
//!   `{"type":"stream_event","event":{"delta":{"type":"text_delta","text":…}}}`
//!   and the last line is `{"type":"result","is_error":…,"result":…}`.
//! - `--tools ""`: no built-in tools at all (no Bash, Read, Edit, Write…).
//! - `--strict-mcp-config` (with no `--mcp-config`): no MCP servers.
//! - `--permission-mode dontAsk`: anything that would prompt is denied.
//! - `--disable-slash-commands`, `--no-session-persistence`.
//! - `--system-prompt <prompt>`: replaces Claude Code's agent prompt.
//! - The prompt goes on stdin; the working dir is a fresh empty temp dir.
//!
//! Not `--bare`: bare mode ignores the subscription login, which is the
//! point of this bridge.

use std::process::Command;

use serde_json::Value;

use crate::cli::{self, CliLocator, ScratchDir};
use crate::error::{AiError, Result};
use crate::provider::{GenRequest, GenResult, Provider, ProviderKind};

pub const BINARY: &str = "claude";

#[derive(Debug, Clone, Default)]
pub struct LocalClaudeCode {
    locator: CliLocator,
    /// `None` = whatever Claude Code is configured to use.
    model: Option<String>,
}

impl LocalClaudeCode {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_locator(mut self, locator: CliLocator) -> Self {
        self.locator = locator;
        self
    }
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }
    pub fn binary(&self) -> Option<std::path::PathBuf> {
        self.locator.find(BINARY)
    }

    /// The argument list (exposed for tests).
    pub fn args(&self, req: &GenRequest) -> Vec<String> {
        let mut a: Vec<String> = [
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--tools",
            "",
            "--strict-mcp-config",
            "--permission-mode",
            "dontAsk",
            "--disable-slash-commands",
            "--no-session-persistence",
            "--system-prompt",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        a.push(req.system.clone());
        if let Some(m) = req.model.clone().or_else(|| self.model.clone()) {
            a.push("--model".into());
            a.push(m);
        }
        a
    }
}

/// npm's `claude.cmd` shim runs through cmd.exe, which can't pass the
/// multi-line system prompt as an argument: hand it over as a file.
pub(crate) fn system_prompt_as_file(
    mut args: Vec<String>,
    dir: &std::path::Path,
) -> Result<Vec<String>> {
    if let Some(i) = args.iter().position(|a| a == "--system-prompt")
        && i + 1 < args.len()
    {
        let path = dir.join(".system-prompt.md");
        std::fs::write(&path, &args[i + 1]).map_err(|e| AiError::Storage(e.to_string()))?;
        args[i] = "--system-prompt-file".into();
        args[i + 1] = path.display().to_string();
    }
    Ok(args)
}

impl Provider for LocalClaudeCode {
    fn kind(&self) -> ProviderKind {
        ProviderKind::LocalClaudeCode
    }

    fn generate(&self, req: GenRequest, on_delta: &mut dyn FnMut(&str)) -> Result<GenResult> {
        req.cancel.check()?;
        let bin = self
            .binary()
            .ok_or_else(|| AiError::CliNotFound("Claude Code (`claude`)".into()))?;
        let dir = ScratchDir::new()?;
        let mut args = self.args(&req);
        if cli::is_batch_shim(&bin) {
            args = system_prompt_as_file(args, dir.path())?;
        }
        let mut cmd = Command::new(&bin);
        cmd.args(args)
            .current_dir(dir.path())
            .env("PATH", self.locator.child_path(&bin));

        let mut streamed = String::new();
        let mut assistant_text = String::new();
        let mut model: Option<String> = None;
        let mut result: Option<(bool, String)> = None;
        let exit = cli::run_streaming("claude", cmd, &req.user, &req.cancel, |line| {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                return Ok(());
            };
            // Ignore anything from subagents (there shouldn't be any).
            if v.get("parent_tool_use_id").is_some_and(|p| !p.is_null()) {
                return Ok(());
            }
            match v.get("type").and_then(Value::as_str) {
                Some("system") => {
                    if let Some(m) = v.get("model").and_then(Value::as_str) {
                        model = Some(m.to_string());
                    }
                }
                Some("stream_event") => {
                    let ev = &v["event"];
                    if ev.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta")
                        && let Some(t) = ev.pointer("/delta/text").and_then(Value::as_str)
                    {
                        streamed.push_str(t);
                        on_delta(t);
                    }
                    if let Some(m) = ev.pointer("/message/model").and_then(Value::as_str) {
                        model = Some(m.to_string());
                    }
                }
                Some("assistant") => {
                    if let Some(m) = v.pointer("/message/model").and_then(Value::as_str) {
                        model = Some(m.to_string());
                    }
                    if let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) {
                        for b in blocks {
                            if b.get("type").and_then(Value::as_str) == Some("text")
                                && let Some(t) = b.get("text").and_then(Value::as_str)
                            {
                                assistant_text.push_str(t);
                            }
                        }
                    }
                }
                Some("result") => {
                    let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false)
                        || v.get("subtype")
                            .and_then(Value::as_str)
                            .is_some_and(|s| s != "success");
                    let text = v
                        .get("result")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    result = Some((is_error, text));
                }
                _ => {}
            }
            Ok(())
        })?;

        match result {
            Some((true, msg)) => Err(AiError::CliFailed {
                name: "claude".into(),
                code: exit.code,
                message: if msg.is_empty() {
                    cli::tail(&exit.stderr)
                } else {
                    msg
                },
            }),
            Some((false, text)) => {
                let text = if text.is_empty() {
                    if streamed.is_empty() {
                        assistant_text
                    } else {
                        streamed.clone()
                    }
                } else {
                    text
                };
                if streamed.is_empty() && !text.is_empty() {
                    on_delta(&text);
                }
                if text.is_empty() {
                    return Err(AiError::Provider("Claude Code returned no text".into()));
                }
                Ok(GenResult {
                    text,
                    model: model.unwrap_or_else(|| "claude-code".into()),
                })
            }
            None => Err(AiError::CliFailed {
                name: "claude".into(),
                code: exit.code,
                message: match cli::tail(&exit.stderr) {
                    s if s.is_empty() => "no result from Claude Code".into(),
                    s => s,
                },
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_shims_get_the_system_prompt_as_a_file() {
        assert!(cli::is_batch_shim(std::path::Path::new(
            r"C:\npm\claude.cmd"
        )));
        assert!(cli::is_batch_shim(std::path::Path::new("claude.BAT")));
        assert!(!cli::is_batch_shim(std::path::Path::new("claude.exe")));
        assert!(!cli::is_batch_shim(std::path::Path::new("claude")));

        let dir = tempfile::tempdir().unwrap();
        let args = vec![
            "-p".to_string(),
            "--system-prompt".to_string(),
            "line one\nline two".to_string(),
        ];
        let out = system_prompt_as_file(args, dir.path()).unwrap();
        assert_eq!(out[1], "--system-prompt-file");
        assert_eq!(
            std::fs::read_to_string(&out[2]).unwrap(),
            "line one\nline two"
        );
    }
}
