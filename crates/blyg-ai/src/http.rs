//! Shared HTTP plumbing: a ureq agent (same stack as blyg-core), error
//! mapping, and a Server-Sent Events reader that checks the cancel flag
//! between events. Secrets never go into error messages.

use std::io::{BufRead, BufReader, Read};
use std::time::Duration;

use serde_json::Value;

use crate::error::{AiError, Result};
use crate::provider::CancelFlag;

pub(crate) fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        // Reasoning models can think for a while before the first byte.
        .timeout_read(Duration::from_secs(300))
        .timeout_write(Duration::from_secs(60))
        .build()
}

/// Map a ureq failure. `provider` names the service for 401s.
pub(crate) fn map_err(provider: &str, e: ureq::Error) -> AiError {
    match e {
        ureq::Error::Transport(t) => AiError::Network(t.to_string()),
        ureq::Error::Status(401 | 403, _) => AiError::Unauthorized {
            provider: provider.to_string(),
        },
        ureq::Error::Status(code, r) => {
            let body = r.into_string().unwrap_or_default();
            AiError::Provider(format!(
                "{provider} request failed: {code} {}",
                error_message(&body)
            ))
        }
    }
}

/// Pull a human message out of a JSON error body (`{error:{message}}`,
/// `{error:"…"}`, `{errors:[{message}]}`), else the trimmed raw body.
pub(crate) fn error_message(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return body.trim().chars().take(500).collect();
    };
    let pick = |v: &Value| -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => o.get("message").and_then(Value::as_str).map(str::to_string),
            _ => None,
        }
    };
    v.get("error")
        .and_then(pick)
        .or_else(|| {
            v.get("errors")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(pick)
        })
        .or_else(|| v.get("detail").and_then(pick))
        .or_else(|| v.get("message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| body.trim().chars().take(500).collect())
}

pub(crate) fn read_json(
    provider: &str,
    res: std::result::Result<ureq::Response, ureq::Error>,
) -> Result<Value> {
    let r = res.map_err(|e| map_err(provider, e))?;
    let mut s = String::new();
    r.into_reader()
        .take(16 * 1024 * 1024)
        .read_to_string(&mut s)
        .map_err(|e| AiError::Network(e.to_string()))?;
    serde_json::from_str(&s)
        .map_err(|e| AiError::Provider(format!("{provider} returned invalid JSON: {e}")))
}

/// One SSE event: the `event:` name (may be empty) and its joined `data:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub event: String,
    pub data: String,
}

/// Read SSE events from `reader`, calling `on_event` for each. Stops when
/// the stream ends, `on_event` returns `Ok(false)`, or the flag is set.
pub(crate) fn read_sse<R: Read>(
    reader: R,
    cancel: &CancelFlag,
    mut on_event: impl FnMut(SseEvent) -> Result<bool>,
) -> Result<()> {
    let mut r = BufReader::new(reader);
    let mut event = String::new();
    let mut data: Vec<String> = Vec::new();
    let mut line = String::new();
    loop {
        cancel.check()?;
        line.clear();
        let n = r
            .read_line(&mut line)
            .map_err(|e| AiError::Network(e.to_string()))?;
        let eof = n == 0;
        let l = line.trim_end_matches(['\r', '\n']);
        if eof || l.is_empty() {
            if !data.is_empty() || !event.is_empty() {
                let ev = SseEvent {
                    event: std::mem::take(&mut event),
                    data: std::mem::take(&mut data).join("\n"),
                };
                cancel.check()?;
                if !on_event(ev)? {
                    return Ok(());
                }
            }
            if eof {
                return Ok(());
            }
            continue;
        }
        if l.starts_with(':') {
            continue; // comment / keep-alive
        }
        let (field, value) = match l.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (l, ""),
        };
        match field {
            "event" => event = value.to_string(),
            "data" => data.push(value.to_string()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_and_comments() {
        let s = ": ping\nevent: a\ndata: {\"x\":1}\n\ndata: line1\ndata: line2\n\ndata: tail";
        let mut got = vec![];
        read_sse(s.as_bytes(), &CancelFlag::new(), |e| {
            got.push(e);
            Ok(true)
        })
        .unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].event, "a");
        assert_eq!(got[0].data, "{\"x\":1}");
        assert_eq!(got[1].data, "line1\nline2");
        assert_eq!(got[2].data, "tail");
    }

    #[test]
    fn cancel_stops_reading() {
        let c = CancelFlag::new();
        c.cancel();
        let r = read_sse("data: x\n\n".as_bytes(), &c, |_| Ok(true));
        assert_eq!(r, Err(AiError::Cancelled));
    }

    #[test]
    fn error_messages() {
        assert_eq!(
            error_message(r#"{"error":{"message":"bad key"}}"#),
            "bad key"
        );
        assert_eq!(error_message(r#"{"errors":[{"message":"nope"}]}"#), "nope");
        assert_eq!(error_message(r#"{"error":"x"}"#), "x");
        assert_eq!(error_message("plain"), "plain");
    }
}
