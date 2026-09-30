//! Anthropic Messages API with an API key, over raw HTTPS + SSE.
//!
//! Request shape (claude-api reference, 2026): `model: "claude-opus-5"`,
//! `thinking: {type: "adaptive"}` (never `budget_tokens`, which Opus 5
//! rejects), `stream: true`. A `stop_reason: "refusal"` is a clean
//! `AiError::Refused` with the same wording as the blyg server's
//! `provider.ts` ("provider declined the request (category)").

use serde_json::{Value, json};

use crate::error::{AiError, Result};
use crate::http::{self, read_sse};
use crate::provider::{GenRequest, GenResult, ModelInfo, Provider, ProviderKind};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const API_VERSION: &str = "2023-06-01";
pub const DEFAULT_MODEL: &str = "claude-opus-5";
/// Adaptive thinking spends from `max_tokens`, so this is higher than the
/// server's 4096 (which runs without thinking).
pub const DEFAULT_MAX_TOKENS: u32 = 16_000;

pub struct AnthropicApi {
    api_key: String,
    base_url: String,
    model: String,
}

impl std::fmt::Debug for AnthropicApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicApi")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl AnthropicApi {
    pub fn new(api_key: impl Into<String>) -> Self {
        AnthropicApi {
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

    fn request(&self, method: &str, path: &str) -> ureq::Request {
        http::agent()
            .request(method, &format!("{}{}", self.base_url, path))
            .set("x-api-key", &self.api_key)
            .set("anthropic-version", API_VERSION)
    }

    /// The JSON body for a request (exposed for tests).
    pub fn body(&self, req: &GenRequest) -> Value {
        json!({
            "model": req.model.clone().unwrap_or_else(|| self.model.clone()),
            "max_tokens": req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            "stream": true,
            "thinking": { "type": "adaptive" },
            "system": req.system,
            "messages": [{ "role": "user", "content": req.user }],
        })
    }
}

impl Provider for AnthropicApi {
    fn kind(&self) -> ProviderKind {
        ProviderKind::AnthropicApi
    }

    fn generate(&self, req: GenRequest, on_delta: &mut dyn FnMut(&str)) -> Result<GenResult> {
        req.cancel.check()?;
        let body = self.body(&req);
        let res = self
            .request("POST", "/v1/messages")
            .set("content-type", "application/json")
            .set("accept", "text/event-stream")
            .send_json(body)
            .map_err(|e| http::map_err("Anthropic", e))?;

        let mut text = String::new();
        let mut model = req.model.clone().unwrap_or_else(|| self.model.clone());
        let mut stop_reason: Option<String> = None;
        let mut category: Option<String> = None;
        read_sse(res.into_reader(), &req.cancel, |ev| {
            let Ok(v) = serde_json::from_str::<Value>(&ev.data) else {
                return Ok(true);
            };
            match v.get("type").and_then(Value::as_str).unwrap_or(&ev.event) {
                "message_start" => {
                    if let Some(m) = v.pointer("/message/model").and_then(Value::as_str) {
                        model = m.to_string();
                    }
                }
                "content_block_delta" => {
                    // Only text; thinking deltas are never inserted.
                    if v.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta")
                        && let Some(t) = v.pointer("/delta/text").and_then(Value::as_str)
                    {
                        text.push_str(t);
                        on_delta(t);
                    }
                }
                "message_delta" => {
                    if let Some(s) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                        stop_reason = Some(s.to_string());
                    }
                    let cat = v
                        .pointer("/delta/stop_details/category")
                        .or_else(|| v.pointer("/stop_details/category"))
                        .and_then(Value::as_str);
                    if let Some(c) = cat {
                        category = Some(c.to_string());
                    }
                }
                "message_stop" => return Ok(false),
                "error" => {
                    let msg = v
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error");
                    return Err(AiError::Provider(format!("provider error: {msg}")));
                }
                _ => {}
            }
            Ok(true)
        })?;

        if stop_reason.as_deref() == Some("refusal") {
            return Err(AiError::Refused(match category {
                Some(c) => format!("provider declined the request ({c})"),
                None => "provider declined the request".to_string(),
            }));
        }
        if text.is_empty() {
            return Err(AiError::Provider(
                "provider returned no text content".into(),
            ));
        }
        Ok(GenResult { text, model })
    }

    fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let v = http::read_json(
            "Anthropic",
            self.request("GET", "/v1/models?limit=100").call(),
        )?;
        Ok(v.get("data")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|m| {
                        Some(ModelInfo {
                            id: m.get("id")?.as_str()?.to_string(),
                            display_name: m
                                .get("display_name")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }
}
