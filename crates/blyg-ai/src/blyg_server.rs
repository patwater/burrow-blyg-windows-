//! Generation on the user's own blyg Worker: `POST /api/items/:id/generate
//! {scope}` → `{text, model}` (apps/blyg/src/api.ts + tk-generate.ts).
//!
//! blyg-core's `Api` has no method for this endpoint, so this is a tiny
//! direct call with the owner token. The server builds its own prompt (with
//! its own key, model and `ai_style_prompt`), **splices the output into its
//! working copy and records provenance itself**, so the caller must re-pull
//! the item afterwards rather than splicing locally. Not streamed: the text
//! arrives as a single delta.

use serde_json::{Value, json};

use crate::error::{AiError, Result};
use crate::http;
use crate::provider::{GenRequest, GenResult, Provider, ProviderKind};

pub struct BlygServer {
    base_url: String,
    token: String,
}

impl std::fmt::Debug for BlygServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlygServer")
            .field("base_url", &self.base_url)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl BlygServer {
    pub fn new(base_url: &str, owner_token: impl Into<String>) -> Self {
        BlygServer {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: owner_token.into(),
        }
    }
}

impl Provider for BlygServer {
    fn kind(&self) -> ProviderKind {
        ProviderKind::BlygServer
    }

    fn generate(&self, req: GenRequest, on_delta: &mut dyn FnMut(&str)) -> Result<GenResult> {
        req.cancel.check()?;
        let Some(scope) = &req.server_scope else {
            return Err(AiError::NotConfigured(
                "the blyg server can only fill TK scopes in items that have synced".into(),
            ));
        };
        let id: String = url::form_urlencoded::byte_serialize(scope.item_id.as_bytes()).collect();
        let res = http::agent()
            .post(&format!("{}/api/items/{id}/generate", self.base_url))
            .set("authorization", &format!("Bearer {}", self.token))
            .set("accept", "application/json")
            .send_json(json!({ "scope": scope.scope }));
        let v = match res {
            Err(ureq::Error::Status(404, _)) => {
                return Err(AiError::Provider(
                    "item not found on the server (or the server has no /generate)".into(),
                ));
            }
            Err(ureq::Error::Status(401, _)) => {
                return Err(AiError::Unauthorized {
                    provider: "your blyg".into(),
                });
            }
            Err(ureq::Error::Status(code, r)) => {
                let body = r.into_string().unwrap_or_default();
                let msg = http::error_message(&body);
                return Err(if msg.contains("declined") {
                    AiError::Refused(msg)
                } else {
                    AiError::Provider(format!("blyg server: {msg} ({code})"))
                });
            }
            r => http::read_json("blyg server", r)?,
        };
        req.cancel.check()?;
        let text = v
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| AiError::Provider("blyg server returned no text".into()))?
            .to_string();
        let model = v
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        on_delta(&text);
        Ok(GenResult { text, model })
    }
}
