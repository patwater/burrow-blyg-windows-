//! Local Codex bridge: runs the user's installed `codex exec`, read-only.
//!
//! Flags (`codex exec --help`, codex-cli 0.155.1, and
//! learn.chatgpt.com/docs/non-interactive-mode):
//! - `exec --json`: JSONL events (`thread.started`, `turn.started`,
//!   `item.started|updated|completed` with `item.type == "agent_message"`
//!   carrying `text`, `turn.completed`, `turn.failed {error.message}`,
//!   `error {message}`).
//! - `--sandbox read-only`: model-run commands can't write anywhere.
//! - `--ephemeral`: no session files; `--skip-git-repo-check`: runs in a
//!   fresh empty temp dir (`-C`), which isn't a repo.
//! - `--color never`; `-m <model>` when set; prompt from stdin (`-`).
//!
//! Codex has no system-prompt flag, so the system prompt is prepended to
//! the stdin prompt under a heading.

use std::collections::HashMap;
use std::process::Command;

use serde_json::Value;

use crate::cli::{self, CliLocator, ScratchDir};
use crate::error::{AiError, Result};
use crate::provider::{GenRequest, GenResult, Provider, ProviderKind};

pub const BINARY: &str = "codex";

#[derive(Debug, Clone, Default)]
pub struct LocalCodex {
    locator: CliLocator,
    model: Option<String>,
}

impl LocalCodex {
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

    pub fn args(&self, req: &GenRequest, dir: &std::path::Path) -> Vec<String> {
        let mut a: Vec<String> = [
            "exec",
            "--json",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "--ephemeral",
            "--color",
            "never",
            "-C",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        a.push(dir.display().to_string());
        if let Some(m) = req.model.clone().or_else(|| self.model.clone()) {
            a.push("-m".into());
            a.push(m);
        }
        a.push("-".into());
        a
    }

    pub fn prompt(req: &GenRequest) -> String {
        format!(
            "# Instructions\n\n{}\n\nDo not run commands or edit files; reply with text only.\n\n# Task\n\n{}",
            req.system, req.user
        )
    }
}

impl Provider for LocalCodex {
    fn kind(&self) -> ProviderKind {
        ProviderKind::LocalCodex
    }

    fn generate(&self, req: GenRequest, on_delta: &mut dyn FnMut(&str)) -> Result<GenResult> {
        req.cancel.check()?;
        let bin = self
            .binary()
            .ok_or_else(|| AiError::CliNotFound("Codex (`codex`)".into()))?;
        let dir = ScratchDir::new()?;
        let mut cmd = Command::new(&bin);
        cmd.args(self.args(&req, dir.path()))
            .current_dir(dir.path())
            .env("PATH", self.locator.child_path(&bin));

        // Per agent_message item: what we've already emitted.
        let mut emitted: HashMap<String, String> = HashMap::new();
        let mut order: Vec<String> = vec![];
        let mut failure: Option<String> = None;
        let exit = cli::run_streaming("codex", cmd, &Self::prompt(&req), &req.cancel, |line| {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                return Ok(());
            };
            match v.get("type").and_then(Value::as_str) {
                Some("item.started" | "item.updated" | "item.completed") => {
                    let item = &v["item"];
                    if item.get("type").and_then(Value::as_str) != Some("agent_message") {
                        return Ok(());
                    }
                    let id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let text = item.get("text").and_then(Value::as_str).unwrap_or("");
                    let earlier_text = order
                        .iter()
                        .any(|o| o != &id && emitted.get(o).is_some_and(|t| !t.is_empty()));
                    let prev = emitted.entry(id.clone()).or_insert_with(|| {
                        order.push(id.clone());
                        String::new()
                    });
                    if let Some(rest) = text.strip_prefix(prev.as_str())
                        && !rest.is_empty()
                    {
                        if prev.is_empty() && earlier_text {
                            // A later message: separate it like the final join.
                            on_delta("\n\n");
                        }
                        on_delta(rest);
                    }
                    *prev = text.to_string();
                }
                Some("turn.failed") => {
                    failure = v
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or(Some("turn failed".into()));
                }
                Some("error") => {
                    failure = v
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or(Some("error".into()));
                }
                _ => {}
            }
            Ok(())
        })?;

        if let Some(message) = failure {
            return Err(AiError::CliFailed {
                name: "codex".into(),
                code: exit.code,
                message,
            });
        }
        let text = order
            .iter()
            .filter_map(|id| emitted.get(id))
            .filter(|t| !t.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n");
        if exit.code != Some(0) {
            return Err(AiError::CliFailed {
                name: "codex".into(),
                code: exit.code,
                message: cli::tail(&exit.stderr),
            });
        }
        if text.is_empty() {
            return Err(AiError::Provider("Codex returned no text".into()));
        }
        Ok(GenResult {
            text,
            model: req
                .model
                .clone()
                .or_else(|| self.model.clone())
                .unwrap_or_else(|| "codex".into()),
        })
    }
}
