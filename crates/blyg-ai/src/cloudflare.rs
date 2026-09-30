//! Cloudflare Workers AI through its OpenAI-compatible chat completions
//! endpoint (developers.cloudflare.com/workers-ai/configuration/open-ai-compatibility/):
//! `POST https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1/chat/completions`
//! with `Authorization: Bearer <api token>`, `stream: true` (SSE,
//! `choices[].delta.content`, terminated by `data: [DONE]`).
//!
//! Default model: `@cf/google/gemma-4-26b-a4b-it`
//! (developers.cloudflare.com/workers-ai/models/gemma-4-26b-a4b-it/), a
//! reasoning model (`chat_template_kwargs.enable_thinking` defaults to true).
//! The model page's output schema doesn't say where reasoning text appears,
//! so every known form is dropped: `delta.reasoning_content`,
//! `delta.reasoning`, and `<think>…</think>` inside `content`. Only the
//! final answer is inserted (leading whitespace after a dropped think block
//! is trimmed too).

use serde_json::{Value, json};

use crate::error::{AiError, Result};
use crate::http::{self, read_sse};
use crate::provider::{GenRequest, GenResult, ModelInfo, Provider, ProviderKind};

pub const DEFAULT_BASE_URL: &str = "https://api.cloudflare.com/client/v4";
pub const DEFAULT_MODEL: &str = "@cf/google/gemma-4-26b-a4b-it";
/// Keychain account (via `TokenStore`) for the API token.
pub const KEYCHAIN_ACCOUNT: &str = "blygger-ai.cloudflare-api-token";
pub const DEFAULT_MAX_TOKENS: u32 = 16_000;

pub struct CloudflareWorkersAi {
    account_id: String,
    api_token: String,
    base_url: String,
    model: String,
}

impl std::fmt::Debug for CloudflareWorkersAi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudflareWorkersAi")
            .field("account_id", &self.account_id)
            .field("model", &self.model)
            .field("api_token", &"<redacted>")
            .finish()
    }
}

impl CloudflareWorkersAi {
    pub fn new(account_id: impl Into<String>, api_token: impl Into<String>) -> Self {
        CloudflareWorkersAi {
            account_id: account_id.into(),
            api_token: api_token.into(),
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
        }
    }
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url = base.into().trim_end_matches('/').to_string();
        self
    }
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    fn account_url(&self, path: &str) -> String {
        let id: String = url::form_urlencoded::byte_serialize(self.account_id.as_bytes()).collect();
        format!("{}/accounts/{id}{path}", self.base_url)
    }

    pub fn body(&self, req: &GenRequest) -> Value {
        json!({
            "model": req.model.clone().unwrap_or_else(|| self.model.clone()),
            "messages": [
                { "role": "system", "content": req.system },
                { "role": "user", "content": req.user },
            ],
            "stream": true,
            "max_completion_tokens": req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        })
    }
}

impl Provider for CloudflareWorkersAi {
    fn kind(&self) -> ProviderKind {
        ProviderKind::CloudflareWorkersAi
    }

    fn generate(&self, req: GenRequest, on_delta: &mut dyn FnMut(&str)) -> Result<GenResult> {
        req.cancel.check()?;
        let res = http::agent()
            .post(&self.account_url("/ai/v1/chat/completions"))
            .set("authorization", &format!("Bearer {}", self.api_token))
            .set("content-type", "application/json")
            .set("accept", "text/event-stream")
            .send_json(self.body(&req))
            .map_err(|e| http::map_err("Cloudflare Workers AI", e))?;

        // The provenance string is the full @cf/… id we asked for; the
        // response's `model` field is only used if it's also an @cf id.
        let mut model = req.model.clone().unwrap_or_else(|| self.model.clone());
        let mut filter = ThinkFilter::default();
        let mut text = String::new();
        let mut refusal = String::new();
        read_sse(res.into_reader(), &req.cancel, |ev| {
            if ev.data.trim() == "[DONE]" {
                return Ok(false);
            }
            let Ok(v) = serde_json::from_str::<Value>(&ev.data) else {
                return Ok(true);
            };
            if let Some(e) = v
                .get("error")
                .or_else(|| v.get("errors").and_then(|e| e.get(0)))
            {
                let msg = e
                    .get("message")
                    .and_then(Value::as_str)
                    .or(e.as_str())
                    .unwrap_or("unknown error");
                return Err(AiError::Provider(format!("provider error: {msg}")));
            }
            if let Some(m) = v.get("model").and_then(Value::as_str)
                && m.starts_with("@cf/")
            {
                model = m.to_string();
            }
            let Some(choices) = v.get("choices").and_then(Value::as_array) else {
                return Ok(true);
            };
            for c in choices {
                let d = c.get("delta").or_else(|| c.get("message"));
                if let Some(t) = d.and_then(|d| d.get("content")).and_then(Value::as_str) {
                    let mut out = filter.push(t);
                    if text.is_empty() {
                        out = out.trim_start().to_string();
                    }
                    if !out.is_empty() {
                        text.push_str(&out);
                        on_delta(&out);
                    }
                }
                if let Some(r) = d.and_then(|d| d.get("refusal")).and_then(Value::as_str) {
                    refusal.push_str(r);
                }
            }
            Ok(true)
        })?;
        let mut tail = filter.finish();
        if text.is_empty() {
            tail = tail.trim_start().to_string();
        }
        if !tail.is_empty() {
            text.push_str(&tail);
            on_delta(&tail);
        }
        if text.trim().is_empty() && !refusal.is_empty() {
            return Err(AiError::Refused(format!(
                "provider declined the request: {refusal}"
            )));
        }
        if text.trim().is_empty() {
            return Err(AiError::Provider(
                "provider returned no text content".into(),
            ));
        }
        Ok(GenResult { text, model })
    }

    /// `GET /accounts/{id}/ai/models/search?task=Text Generation`, Gemma 4 first.
    fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let v = http::read_json(
            "Cloudflare Workers AI",
            http::agent()
                .get(&self.account_url("/ai/models/search"))
                .query("task", "Text Generation")
                .query("per_page", "100")
                .set("authorization", &format!("Bearer {}", self.api_token))
                .call(),
        )?;
        let mut out: Vec<ModelInfo> = v
            .get("result")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|m| {
                        Some(ModelInfo {
                            id: m.get("name")?.as_str()?.to_string(),
                            display_name: m
                                .get("description")
                                .and_then(Value::as_str)
                                .map(|d| d.chars().take(120).collect()),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|m| (m.id != DEFAULT_MODEL, m.id.clone()));
        Ok(out)
    }
}

/// Streaming filter that removes `<think>…</think>` spans, even when the
/// tags are split across deltas.
#[derive(Debug, Default)]
pub(crate) struct ThinkFilter {
    buf: String,
    in_think: bool,
}

const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";

impl ThinkFilter {
    /// Feed a chunk; returns the visible text that is now certain.
    pub fn push(&mut self, chunk: &str) -> String {
        self.buf.push_str(chunk);
        let mut out = String::new();
        loop {
            let tag = if self.in_think { CLOSE } else { OPEN };
            if let Some(i) = self.buf.find(tag) {
                if !self.in_think {
                    out.push_str(&self.buf[..i]);
                }
                self.buf.drain(..i + tag.len());
                self.in_think = !self.in_think;
                continue;
            }
            // Keep a possible partial tag at the end.
            let keep = partial_suffix(&self.buf, tag);
            let cut = self.buf.len() - keep;
            if !self.in_think {
                out.push_str(&self.buf[..cut]);
            }
            self.buf.drain(..cut);
            return out;
        }
    }

    /// End of stream: flush whatever visible text is left.
    pub fn finish(&mut self) -> String {
        let rest = std::mem::take(&mut self.buf);
        if self.in_think { String::new() } else { rest }
    }
}

/// Length of the longest suffix of `s` that is a proper prefix of `tag`.
fn partial_suffix(s: &str, tag: &str) -> usize {
    (1..tag.len())
        .rev()
        .find(|&n| {
            n <= s.len() && s.is_char_boundary(s.len() - n) && tag.starts_with(&s[s.len() - n..])
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&str]) -> String {
        let mut f = ThinkFilter::default();
        let mut s: String = chunks.iter().map(|c| f.push(c)).collect();
        s.push_str(&f.finish());
        s
    }

    #[test]
    fn strips_think_blocks_across_chunks() {
        assert_eq!(run(&["<think>hmm</think>Hello"]), "Hello");
        assert_eq!(
            run(&["<thi", "nk>plan", " more</th", "ink>An", "swer"]),
            "Answer"
        );
        assert_eq!(run(&["a < b and <", "b>"]), "a < b and <b>");
        assert_eq!(run(&["café <think>ünïcode</think>🙂"]), "café 🙂");
        assert_eq!(run(&["<think>never closed"]), "");
    }
}
