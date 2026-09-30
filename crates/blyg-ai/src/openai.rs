//! OpenAI Responses API with an API key (`POST /v1/responses`, `stream: true`).
//!
//! Streaming events handled (developers.openai.com/api/docs/guides/streaming-responses):
//! `response.output_text.delta {delta}`, `response.refusal.delta {delta}`,
//! `response.completed|response.done {response}`, `response.incomplete`,
//! `response.failed {response.error}`, `error {message|error}`.
//! The same parser serves the ChatGPT (Codex) endpoint.

use std::io::Read;

use serde_json::{Value, json};

use crate::error::{AiError, Result};
use crate::http::{self, read_sse};
use crate::provider::{CancelFlag, GenRequest, GenResult, ModelInfo, Provider, ProviderKind};

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// "Start with GPT-6 Astra for complex reasoning and coding" —
/// developers.openai.com/api/docs/models (checked 2026-09-24).
pub const DEFAULT_MODEL: &str = "gpt-6-astra";

pub struct OpenAiApi {
    api_key: String,
    base_url: String,
    model: String,
}

impl std::fmt::Debug for OpenAiApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiApi")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl OpenAiApi {
    pub fn new(api_key: impl Into<String>) -> Self {
        OpenAiApi {
            api_key: api_key.into(),
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

    pub fn body(&self, req: &GenRequest) -> Value {
        let mut b = json!({
            "model": req.model.clone().unwrap_or_else(|| self.model.clone()),
            "instructions": req.system,
            "input": [{
                "role": "user",
                "content": [{ "type": "input_text", "text": req.user }],
            }],
            "stream": true,
            "store": false,
        });
        if let Some(n) = req.max_tokens {
            b["max_output_tokens"] = json!(n);
        }
        b
    }
}

impl Provider for OpenAiApi {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenaiApi
    }

    fn generate(&self, req: GenRequest, on_delta: &mut dyn FnMut(&str)) -> Result<GenResult> {
        req.cancel.check()?;
        let res = http::agent()
            .post(&format!("{}/responses", self.base_url))
            .set("authorization", &format!("Bearer {}", self.api_key))
            .set("content-type", "application/json")
            .set("accept", "text/event-stream")
            .send_json(self.body(&req))
            .map_err(|e| http::map_err("OpenAI", e))?;
        let model = req.model.clone().unwrap_or_else(|| self.model.clone());
        stream_responses(res.into_reader(), &req.cancel, on_delta, model)
    }

    fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let v = http::read_json(
            "OpenAI",
            http::agent()
                .get(&format!("{}/models", self.base_url))
                .set("authorization", &format!("Bearer {}", self.api_key))
                .call(),
        )?;
        let mut out: Vec<ModelInfo> = v
            .get("data")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|m| m.get("id")?.as_str().map(str::to_string))
                    .filter(|id| is_text_model(id))
                    .map(|id| ModelInfo {
                        id,
                        display_name: None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(out)
    }
}

/// Chat-capable ids only (drop embeddings, audio, image, moderation, …).
fn is_text_model(id: &str) -> bool {
    let chatty = id.starts_with("gpt-") || id.starts_with('o');
    let other = [
        "embedding",
        "audio",
        "realtime",
        "tts",
        "transcribe",
        "image",
        "moderation",
        "search",
    ];
    chatty && !other.iter().any(|o| id.contains(o))
}

/// Parse a Responses-API SSE stream into text. Shared with `chatgpt.rs`.
pub(crate) fn stream_responses<R: Read>(
    reader: R,
    cancel: &CancelFlag,
    on_delta: &mut dyn FnMut(&str),
    mut model: String,
) -> Result<GenResult> {
    let mut text = String::new();
    let mut refusal = String::new();
    let mut terminal = false;
    let mut incomplete: Option<String> = None;
    read_sse(reader, cancel, |ev| {
        if ev.data == "[DONE]" {
            return Ok(false);
        }
        let Ok(v) = serde_json::from_str::<Value>(&ev.data) else {
            return Ok(true);
        };
        let ty = v
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(&ev.event)
            .to_string();
        match ty.as_str() {
            "response.created" | "response.in_progress" => {
                if let Some(m) = v.pointer("/response/model").and_then(Value::as_str) {
                    model = m.to_string();
                }
            }
            "response.output_text.delta" => {
                if let Some(d) = v.get("delta").and_then(Value::as_str) {
                    text.push_str(d);
                    on_delta(d);
                }
            }
            "response.refusal.delta" => {
                if let Some(d) = v.get("delta").and_then(Value::as_str) {
                    refusal.push_str(d);
                }
            }
            "response.completed" | "response.done" | "response.incomplete" => {
                terminal = true;
                if let Some(m) = v.pointer("/response/model").and_then(Value::as_str) {
                    model = m.to_string();
                }
                let status = v.pointer("/response/status").and_then(Value::as_str);
                if ty == "response.incomplete" || status == Some("incomplete") {
                    incomplete = Some(
                        v.pointer("/response/incomplete_details/reason")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string(),
                    );
                }
                return Ok(false);
            }
            "response.failed" => {
                let msg = v
                    .pointer("/response/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("response failed");
                return Err(AiError::Provider(format!("provider error: {msg}")));
            }
            "error" => {
                let msg = v
                    .get("message")
                    .and_then(Value::as_str)
                    .or_else(|| v.pointer("/error/message").and_then(Value::as_str))
                    .unwrap_or("unknown error");
                return Err(AiError::Provider(format!("provider error: {msg}")));
            }
            _ => {}
        }
        Ok(true)
    })?;

    if text.is_empty() && !refusal.is_empty() {
        return Err(AiError::Refused(format!(
            "provider declined the request: {refusal}"
        )));
    }
    if text.is_empty() {
        return Err(AiError::Provider(match (terminal, incomplete) {
            (_, Some(r)) => format!("provider response incomplete: {r}"),
            (false, None) => "provider stream ended before the response completed".into(),
            (true, None) => "provider returned no text content".into(),
        }));
    }
    Ok(GenResult { text, model })
}
