//! Generation on the user's own blyg Worker: `POST /api/items/:id/generate
//! {scope}` → `{text, model}` (apps/blyg/src/api.ts + tk-generate.ts).
//!
//! blyg-core's `Api` has no method for this endpoint, so this is a tiny
//! direct call with the owner token. The server builds its own prompt (with
//! its own key, model and `ai_style_prompt`), **splices the output into its
//! working copy and records provenance itself**, so the caller must re-pull
//! the item afterwards rather than splicing locally. Not streamed: the text
//! arrives as a single delta.

use blyg_core::CoreError;
use blyg_core::api::Api;
use blyg_core::api::auth::Credential;

use crate::error::{AiError, Result};
use crate::provider::{GenRequest, GenResult, Provider, ProviderKind};

/// Owns an owner-API client, so it signs in the same way the app does (a
/// token, or the studio password) and reaches the host-rooted `/api`.
pub struct BlygServer {
    api: Api,
}

impl std::fmt::Debug for BlygServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlygServer")
            .field("api", &self.api)
            .finish()
    }
}

impl BlygServer {
    pub fn new(base_url: &str, cred: impl Into<Credential>) -> Self {
        BlygServer {
            api: Api::new(base_url, cred),
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
        let (text, model, content_md) = match self.api.generate(&scope.item_id, scope.scope as u32)
        {
            // `content_md` (studio 0.27.2+) is the whole body as stored: the
            // caller puts it in the editor instead of pulling.
            Ok(r) => (r.text, r.model, r.content_md),
            Err(CoreError::NotFound) => {
                return Err(AiError::Provider(
                    "item not found on the server (or the server has no /generate)".into(),
                ));
            }
            Err(CoreError::Unauthorized) => {
                return Err(AiError::Unauthorized {
                    provider: "your blyg".into(),
                });
            }
            Err(CoreError::Offline) => {
                return Err(AiError::Provider("couldn't reach your blyg".into()));
            }
            Err(CoreError::Rejected {
                status, message, ..
            }) => {
                return Err(if message.contains("declined") {
                    AiError::Refused(message)
                } else {
                    AiError::Provider(format!("blyg server: {message} ({status})"))
                });
            }
            Err(e) => return Err(AiError::Provider(format!("blyg server: {e}"))),
        };
        req.cancel.check()?;
        on_delta(&text);
        Ok(GenResult {
            text,
            model,
            content_md,
        })
    }
}
