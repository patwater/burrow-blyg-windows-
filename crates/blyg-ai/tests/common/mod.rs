//! Test helpers: a tiny scripted HTTP server (std TcpListener, one thread per
//! connection, `Connection: close`), SSE builders, fake JWTs, and fake CLI
//! scripts. Nothing here touches the network or a real CLI.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Req {
    pub method: String,
    /// Path including the query string.
    pub path: String,
    /// Lower-cased header names.
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl Req {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
    pub fn form(&self) -> HashMap<String, String> {
        url::form_urlencoded::parse(self.body.as_bytes())
            .into_owned()
            .collect()
    }
    pub fn route(&self) -> String {
        format!(
            "{} {}",
            self.method,
            self.path.split('?').next().unwrap_or("")
        )
    }
}

#[derive(Debug, Clone)]
pub struct Resp {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

impl Resp {
    pub fn json(status: u16, v: Value) -> Resp {
        Resp {
            status,
            content_type: "application/json",
            body: v.to_string(),
        }
    }
    pub fn sse(body: String) -> Resp {
        Resp {
            status: 200,
            content_type: "text/event-stream",
            body,
        }
    }
    pub fn not_found() -> Resp {
        Resp::json(404, serde_json::json!({"error": "not found"}))
    }
}

type Handler = dyn Fn(&Req) -> Resp + Send + Sync;

pub struct Mock {
    pub base: String,
    log: Arc<Mutex<Vec<Req>>>,
}

impl Mock {
    pub fn start(handler: impl Fn(&Req) -> Resp + Send + Sync + 'static) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let log: Arc<Mutex<Vec<Req>>> = Arc::default();
        let handler: Arc<Handler> = Arc::new(handler);
        let log2 = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let handler = handler.clone();
                let log = log2.clone();
                std::thread::spawn(move || {
                    let mut r = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if r.read_line(&mut line).is_err() || line.is_empty() {
                        return;
                    }
                    let mut parts = line.split_whitespace();
                    let method = parts.next().unwrap_or("").to_string();
                    let path = parts.next().unwrap_or("").to_string();
                    let mut headers = HashMap::new();
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).is_err() {
                            return;
                        }
                        let h = h.trim_end();
                        if h.is_empty() {
                            break;
                        }
                        if let Some((k, v)) = h.split_once(':') {
                            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
                        }
                    }
                    let len: usize = headers
                        .get("content-length")
                        .and_then(|l| l.parse().ok())
                        .unwrap_or(0);
                    let mut body = vec![0u8; len];
                    let _ = r.read_exact(&mut body);
                    let req = Req {
                        method,
                        path,
                        headers,
                        body: String::from_utf8_lossy(&body).into_owned(),
                    };
                    log.lock().unwrap().push(req.clone());
                    let resp = handler(&req);
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        resp.status,
                        resp.content_type,
                        resp.body.len(),
                        resp.body
                    );
                    let _ = stream.flush();
                });
            }
        });
        Mock { base, log }
    }

    pub fn requests(&self) -> Vec<Req> {
        self.log.lock().unwrap().clone()
    }

    pub fn count(&self, route: &str) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.route() == route)
            .count()
    }

    pub fn last(&self, route: &str) -> Req {
        self.requests()
            .into_iter()
            .rev()
            .find(|r| r.route() == route)
            .unwrap_or_else(|| panic!("no request to {route}"))
    }
}

/// `event: <name>\ndata: <json>\n\n` per event.
pub fn sse(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(e, d)| {
            if e.is_empty() {
                format!("data: {d}\n\n")
            } else {
                format!("event: {e}\ndata: {d}\n\n")
            }
        })
        .collect()
}

/// A JWT-shaped token carrying a ChatGPT account id (unsigned; tests only).
pub fn fake_jwt(account: &str, nonce: &str) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": account },
            "nonce": nonce,
        })
        .to_string(),
    );
    format!("{header}.{payload}.sig")
}

/// Write an executable shell script `name` into `dir`.
pub fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

/// Collects streamed deltas.
#[derive(Default)]
pub struct Deltas(pub Vec<String>);

impl Deltas {
    pub fn joined(&self) -> String {
        self.0.concat()
    }
}
