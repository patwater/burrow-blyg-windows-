//! Sign in with ChatGPT (browser PKCE, device code, refresh) and the Codex
//! Responses endpoint, against mock auth + ChatGPT servers.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use blyg_ai::chatgpt::{self, ChatGptAccount, ChatGptOAuth, Credentials};
use blyg_ai::*;
use blyg_core::config::{MemoryTokenStore, TokenStore};
use common::*;
use serde_json::json;

fn token_json(access: &str, refresh: &str) -> serde_json::Value {
    json!({"access_token": access, "refresh_token": refresh, "expires_in": 3600, "id_token": "x"})
}

fn codex_stream(text: &str) -> String {
    sse(&[
        (
            "response.created",
            json!({"type":"response.created","response":{"model":"gpt-6-astra"}}),
        ),
        (
            "response.output_text.delta",
            json!({"type":"response.output_text.delta","delta":text}),
        ),
        (
            "response.completed",
            json!({"type":"response.completed","response":{"status":"completed","model":"gpt-6-astra"}}),
        ),
    ])
}

fn oauth(base: &str) -> ChatGptOAuth {
    ChatGptOAuth {
        auth_base: base.to_string(),
        callback_port: 0,
        min_poll_interval: Duration::from_millis(10),
        ..Default::default()
    }
}

fn store_creds(store: &dyn TokenStore, access: &str, expires: u64) {
    Credentials {
        access: access.into(),
        refresh: "refresh-1".into(),
        expires,
        account_id: "acc_1".into(),
        invalid: false,
    }
    .save(store)
    .unwrap();
}

fn far_future() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 3_600_000
}

#[test]
fn streams_with_codex_headers_and_body() {
    let access = fake_jwt("acc_1", "a");
    let mock = Mock::start(|r| match r.route().as_str() {
        "POST /backend-api/codex/responses" => Resp::sse(codex_stream("Hi from ChatGPT")),
        _ => Resp::not_found(),
    });
    let store: Arc<dyn TokenStore> = Arc::new(MemoryTokenStore::default());
    store_creds(store.as_ref(), &access, far_future());
    let p = ChatGptAccount::new(store)
        .with_oauth(oauth(&mock.base))
        .with_base_url(format!("{}/backend-api", mock.base));
    let mut d = Deltas::default();
    let res = p
        .generate(GenRequest::new("SYS", "USER"), &mut |t| d.0.push(t.into()))
        .unwrap();
    assert_eq!(res.text, "Hi from ChatGPT");
    assert_eq!(d.joined(), "Hi from ChatGPT");

    let r = mock.last("POST /backend-api/codex/responses");
    assert_eq!(
        r.header("authorization"),
        Some(format!("Bearer {access}").as_str())
    );
    assert_eq!(r.header("chatgpt-account-id"), Some("acc_1"));
    assert_eq!(r.header("openai-beta"), Some("responses=experimental"));
    assert_eq!(r.header("originator"), Some(chatgpt::ORIGINATOR));
    assert_eq!(r.header("accept"), Some("text/event-stream"));
    let b = r.json();
    assert_eq!(b["store"], false);
    assert_eq!(b["stream"], true);
    assert_eq!(b["instructions"], "SYS");
    assert_eq!(b["model"], "gpt-6-astra");
    assert_eq!(b["input"][0]["role"], "user");
    assert_eq!(
        b["input"][0]["content"][0],
        json!({"type":"input_text","text":"USER"})
    );
}

#[test]
fn refreshes_an_expired_token_before_calling() {
    let new_access = fake_jwt("acc_1", "fresh");
    let na = new_access.clone();
    let mock = Mock::start(move |r| match r.route().as_str() {
        "POST /oauth/token" => Resp::json(200, token_json(&na, "refresh-2")),
        "POST /backend-api/codex/responses" => Resp::sse(codex_stream("ok")),
        _ => Resp::not_found(),
    });
    let store: Arc<dyn TokenStore> = Arc::new(MemoryTokenStore::default());
    store_creds(store.as_ref(), &fake_jwt("acc_1", "old"), 1); // long expired
    let p = ChatGptAccount::new(store.clone())
        .with_oauth(oauth(&mock.base))
        .with_base_url(format!("{}/backend-api", mock.base));
    assert_eq!(
        p.generate(GenRequest::new("s", "u"), &mut |_| {})
            .unwrap()
            .text,
        "ok"
    );

    let form = mock.last("POST /oauth/token").form();
    assert_eq!(form["grant_type"], "refresh_token");
    assert_eq!(form["refresh_token"], "refresh-1");
    assert_eq!(form["client_id"], chatgpt::CLIENT_ID);
    let r = mock.last("POST /backend-api/codex/responses");
    assert_eq!(
        r.header("authorization"),
        Some(format!("Bearer {new_access}").as_str())
    );
    let saved = Credentials::load(store.as_ref()).unwrap().unwrap();
    assert_eq!(saved.access, new_access);
    assert_eq!(saved.refresh, "refresh-2");
}

#[test]
fn a_401_triggers_one_refresh_and_retry() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let fresh = fake_jwt("acc_1", "fresh");
    let f2 = fresh.clone();
    let mock = Mock::start(move |r| match r.route().as_str() {
        "POST /oauth/token" => Resp::json(200, token_json(&f2, "refresh-2")),
        "POST /backend-api/codex/responses" => {
            c2.fetch_add(1, Ordering::SeqCst);
            if r.header("authorization") == Some(format!("Bearer {f2}").as_str()) {
                Resp::sse(codex_stream("after refresh"))
            } else {
                Resp::json(401, json!({"error":{"message":"token revoked"}}))
            }
        }
        _ => Resp::not_found(),
    });
    let store: Arc<dyn TokenStore> = Arc::new(MemoryTokenStore::default());
    store_creds(store.as_ref(), &fake_jwt("acc_1", "stale"), far_future());
    let p = ChatGptAccount::new(store)
        .with_oauth(oauth(&mock.base))
        .with_base_url(format!("{}/backend-api", mock.base));
    let res = p.generate(GenRequest::new("s", "u"), &mut |_| {}).unwrap();
    assert_eq!(res.text, "after refresh");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(mock.count("POST /oauth/token"), 1);
}

#[test]
fn rejected_refresh_marks_the_sign_in_expired() {
    let mock = Mock::start(|r| match r.route().as_str() {
        "POST /oauth/token" => Resp::json(400, json!({"error":"invalid_grant"})),
        _ => Resp::not_found(),
    });
    let store: Arc<dyn TokenStore> = Arc::new(MemoryTokenStore::default());
    store_creds(store.as_ref(), &fake_jwt("acc_1", "old"), 1);
    let p = ChatGptAccount::new(store.clone())
        .with_oauth(oauth(&mock.base))
        .with_base_url(format!("{}/backend-api", mock.base));
    let e = p
        .generate(GenRequest::new("s", "u"), &mut |_| {})
        .unwrap_err();
    assert!(matches!(e, AiError::TokenExpired(_)), "{e:?}");
    assert!(Credentials::load(store.as_ref()).unwrap().unwrap().invalid);
    // Next call fails fast without hitting the network.
    let e = p
        .generate(GenRequest::new("s", "u"), &mut |_| {})
        .unwrap_err();
    assert!(matches!(e, AiError::TokenExpired(_)));
    assert_eq!(mock.count("POST /oauth/token"), 1);
}

#[test]
fn usage_limit_is_friendly() {
    let mock = Mock::start(|_| {
        Resp::json(
            429,
            json!({"error":{"code":"usage_limit_reached","plan_type":"PLUS","message":"x"}}),
        )
    });
    let store: Arc<dyn TokenStore> = Arc::new(MemoryTokenStore::default());
    store_creds(store.as_ref(), &fake_jwt("acc_1", "a"), far_future());
    let p = ChatGptAccount::new(store).with_base_url(&mock.base);
    let e = p
        .generate(GenRequest::new("s", "u"), &mut |_| {})
        .unwrap_err();
    assert_eq!(
        e,
        AiError::Provider("You have hit your ChatGPT usage limit (plus plan).".into())
    );
}

#[test]
fn browser_login_via_localhost_callback() {
    let access = fake_jwt("acc_9", "b");
    let a2 = access.clone();
    let mock = Mock::start(move |r| match r.route().as_str() {
        "POST /oauth/token" => Resp::json(200, token_json(&a2, "r-1")),
        _ => Resp::not_found(),
    });
    let o = oauth(&mock.base);
    let login = o.start_browser_login().unwrap();
    let url = url::Url::parse(&login.url).unwrap();
    assert!(
        login
            .url
            .starts_with(&format!("{}/oauth/authorize?", mock.base))
    );
    let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    let state = q["state"].clone();
    let redirect = url::Url::parse(&q["redirect_uri"]).unwrap();
    let port = redirect.port().unwrap();

    // The "browser": first a wrong-state hit (rejected), then the real one.
    let browser = std::thread::spawn(move || {
        let hit = |path: String| {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(s, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            out
        };
        std::thread::sleep(Duration::from_millis(50));
        let bad = hit("/auth/callback?code=nope&state=wrong".into());
        let good = hit(format!("/auth/callback?code=the-code&state={state}"));
        (bad, good)
    });
    let creds = o
        .finish_browser_login(&login, &CancelFlag::new(), Duration::from_secs(5))
        .unwrap();
    let (bad, good) = browser.join().unwrap();
    assert!(bad.starts_with("HTTP/1.1 400"), "{bad}");
    assert!(good.starts_with("HTTP/1.1 200"), "{good}");
    assert_eq!(creds.access, access);
    assert_eq!(creds.account_id, "acc_9");

    let form = mock.last("POST /oauth/token").form();
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["code"], "the-code");
    assert_eq!(form["client_id"], chatgpt::CLIENT_ID);
    assert_eq!(form["redirect_uri"], q["redirect_uri"]);
    // PKCE: the verifier sent hashes to the challenge we advertised.
    use base64::Engine;
    use sha2::Digest;
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(form["code_verifier"].as_bytes()));
    assert_eq!(challenge, q["code_challenge"]);
}

#[test]
fn browser_login_can_be_cancelled() {
    let o = oauth("http://127.0.0.1:9");
    let login = o.start_browser_login().unwrap();
    let c = CancelFlag::new();
    let c2 = c.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        c2.cancel();
    });
    let e = o
        .finish_browser_login(&login, &c, Duration::from_secs(10))
        .unwrap_err();
    assert_eq!(e, AiError::Cancelled);
}

#[test]
fn device_code_flow() {
    let polls = Arc::new(AtomicUsize::new(0));
    let p2 = polls.clone();
    let access = fake_jwt("acc_d", "d");
    let a2 = access.clone();
    let mock = Mock::start(move |r| match r.route().as_str() {
        "POST /api/accounts/deviceauth/usercode" => {
            assert_eq!(r.json()["client_id"], chatgpt::CLIENT_ID);
            Resp::json(
                200,
                json!({"device_auth_id":"dev-1","user_code":"ABCD-1234","interval":"0"}),
            )
        }
        "POST /api/accounts/deviceauth/token" => {
            assert_eq!(
                r.json(),
                json!({"device_auth_id":"dev-1","user_code":"ABCD-1234"})
            );
            match p2.fetch_add(1, Ordering::SeqCst) {
                0 => Resp::json(403, json!({"error":"pending"})),
                1 => Resp::json(
                    400,
                    json!({"error":{"code":"deviceauth_authorization_pending"}}),
                ),
                _ => Resp::json(
                    200,
                    json!({"authorization_code":"dev-code","code_verifier":"dev-verifier"}),
                ),
            }
        }
        "POST /oauth/token" => Resp::json(200, token_json(&a2, "r-d")),
        _ => Resp::not_found(),
    });
    let o = oauth(&mock.base);
    let dc = o.start_device().unwrap();
    assert_eq!(dc.user_code, "ABCD-1234");
    assert_eq!(dc.verification_uri, format!("{}/codex/device", mock.base));
    let creds = o.poll_device(&dc, &CancelFlag::new()).unwrap();
    assert_eq!(creds.access, access);
    assert_eq!(polls.load(Ordering::SeqCst), 3);
    let form = mock.last("POST /oauth/token").form();
    assert_eq!(form["code"], "dev-code");
    assert_eq!(form["code_verifier"], "dev-verifier");
    assert_eq!(
        form["redirect_uri"],
        format!("{}/deviceauth/callback", mock.base)
    );
}

#[test]
fn not_signed_in() {
    let p = ChatGptAccount::new(Arc::new(MemoryTokenStore::default()));
    assert!(matches!(
        p.generate(GenRequest::new("s", "u"), &mut |_| {}),
        Err(AiError::NotConfigured(_))
    ));
}
