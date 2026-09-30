//! Anthropic, OpenAI, Cloudflare Workers AI and blyg-server providers against
//! in-process mock servers.

mod common;

use blyg_ai::anthropic::AnthropicApi;
use blyg_ai::blyg_server::BlygServer;
use blyg_ai::cloudflare::CloudflareWorkersAi;
use blyg_ai::openai::OpenAiApi;
use blyg_ai::*;
use common::*;
use serde_json::json;

fn req() -> GenRequest {
    GenRequest::new("SYS", "USER")
}

fn run(p: &dyn Provider, r: GenRequest) -> (Result<GenResult>, String) {
    let mut d = Deltas::default();
    let res = p.generate(r, &mut |t| d.0.push(t.to_string()));
    (res, d.joined())
}

// ------------------------------------------------------------------ Anthropic

fn anthropic_stream(stop: &str, extra: serde_json::Value) -> String {
    let mut delta = json!({"stop_reason": stop});
    if let Some(o) = extra.as_object() {
        for (k, v) in o {
            delta[k] = v.clone();
        }
    }
    sse(&[
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_1","model":"claude-opus-5","content":[]}}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"secret plan"}}),
        ),
        ("ping", json!({"type":"ping"})),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"It isn't only "}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"the minutes lost."}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":1}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":delta,"usage":{"output_tokens":12}}),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ])
}

#[test]
fn anthropic_streams_text_and_skips_thinking() {
    let mock = Mock::start(|r| match r.route().as_str() {
        "POST /v1/messages" => Resp::sse(anthropic_stream("end_turn", json!({}))),
        _ => Resp::not_found(),
    });
    let p = AnthropicApi::new("sk-ant-test").with_base_url(&mock.base);
    let (res, streamed) = run(&p, req());
    let res = res.unwrap();
    assert_eq!(res.text, "It isn't only the minutes lost.");
    assert_eq!(streamed, res.text);
    assert_eq!(res.model, "claude-opus-5");

    let r = mock.last("POST /v1/messages");
    assert_eq!(r.header("x-api-key"), Some("sk-ant-test"));
    assert_eq!(r.header("anthropic-version"), Some("2023-06-01"));
    let b = r.json();
    assert_eq!(b["model"], "claude-opus-5");
    assert_eq!(b["stream"], true);
    assert_eq!(b["thinking"], json!({"type": "adaptive"}));
    assert!(b["thinking"].get("budget_tokens").is_none());
    assert_eq!(b["system"], "SYS");
    assert_eq!(b["messages"], json!([{"role":"user","content":"USER"}]));
}

#[test]
fn anthropic_refusal_is_a_clean_error() {
    let mock = Mock::start(|_| {
        Resp::sse(anthropic_stream(
            "refusal",
            json!({"stop_details": {"type":"refusal","category":"cyber"}}),
        ))
    });
    let p = AnthropicApi::new("k").with_base_url(&mock.base);
    let (res, _) = run(&p, req());
    assert_eq!(
        res,
        Err(AiError::Refused(
            "provider declined the request (cyber)".into()
        ))
    );
}

#[test]
fn anthropic_401_and_stream_error() {
    let mock = Mock::start(|_| {
        Resp::json(
            401,
            json!({"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}),
        )
    });
    let p = AnthropicApi::new("bad").with_base_url(&mock.base);
    let (res, _) = run(&p, req());
    assert!(matches!(res, Err(AiError::Unauthorized { .. })), "{res:?}");

    let mock = Mock::start(|_| {
        Resp::sse(sse(&[
            (
                "message_start",
                json!({"type":"message_start","message":{"model":"claude-opus-5"}}),
            ),
            (
                "error",
                json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}),
            ),
        ]))
    });
    let p = AnthropicApi::new("k").with_base_url(&mock.base);
    let (res, _) = run(&p, req());
    assert_eq!(
        res,
        Err(AiError::Provider("provider error: Overloaded".into()))
    );
}

#[test]
fn anthropic_lists_models_and_honours_cancel() {
    let mock = Mock::start(|r| match r.route().as_str() {
        "GET /v1/models" => Resp::json(
            200,
            json!({"data":[{"id":"claude-opus-5","display_name":"Claude Opus 5"}],"has_more":false}),
        ),
        _ => Resp::not_found(),
    });
    let p = AnthropicApi::new("k").with_base_url(&mock.base);
    let m = p.list_models().unwrap();
    assert_eq!(m[0].id, "claude-opus-5");
    assert_eq!(m[0].display_name.as_deref(), Some("Claude Opus 5"));

    let c = CancelFlag::new();
    c.cancel();
    let (res, _) = run(&p, req().with_cancel(c));
    assert_eq!(res, Err(AiError::Cancelled));
    assert_eq!(mock.count("POST /v1/messages"), 0);
}

// ------------------------------------------------------------------ OpenAI

#[test]
fn openai_streams_responses() {
    let mock = Mock::start(|_| {
        Resp::sse(sse(&[
            (
                "response.created",
                json!({"type":"response.created","response":{"model":"gpt-6-astra-2026-08-01"}}),
            ),
            (
                "response.output_text.delta",
                json!({"type":"response.output_text.delta","delta":"Hello"}),
            ),
            (
                "response.output_text.delta",
                json!({"type":"response.output_text.delta","delta":", world"}),
            ),
            (
                "response.completed",
                json!({"type":"response.completed","response":{"status":"completed","model":"gpt-6-astra-2026-08-01"}}),
            ),
        ]))
    });
    let p = OpenAiApi::new("sk-test").with_base_url(&mock.base);
    let (res, streamed) = run(&p, req());
    let res = res.unwrap();
    assert_eq!(res.text, "Hello, world");
    assert_eq!(streamed, "Hello, world");
    assert_eq!(res.model, "gpt-6-astra-2026-08-01");
    let r = mock.last("POST /responses");
    assert_eq!(r.header("authorization"), Some("Bearer sk-test"));
    let b = r.json();
    assert_eq!(b["model"], "gpt-6-astra");
    assert_eq!(b["instructions"], "SYS");
    assert_eq!(
        b["input"][0]["content"][0],
        json!({"type":"input_text","text":"USER"})
    );
    assert_eq!(b["stream"], true);
}

#[test]
fn openai_refusal_401_and_failure() {
    let mock = Mock::start(|_| {
        Resp::sse(sse(&[
            (
                "response.refusal.delta",
                json!({"type":"response.refusal.delta","delta":"I can't help with that."}),
            ),
            (
                "response.completed",
                json!({"type":"response.completed","response":{"status":"completed"}}),
            ),
        ]))
    });
    let p = OpenAiApi::new("k").with_base_url(&mock.base);
    assert!(matches!(run(&p, req()).0, Err(AiError::Refused(m)) if m.contains("can't help")));

    let mock = Mock::start(|_| Resp::json(401, json!({"error":{"message":"Incorrect API key"}})));
    let p = OpenAiApi::new("k").with_base_url(&mock.base);
    assert!(matches!(
        run(&p, req()).0,
        Err(AiError::Unauthorized { .. })
    ));

    let mock = Mock::start(|_| {
        Resp::sse(sse(&[(
            "response.failed",
            json!({"type":"response.failed","response":{"status":"failed","error":{"code":"server_error","message":"boom"}}}),
        )]))
    });
    let p = OpenAiApi::new("k").with_base_url(&mock.base);
    assert_eq!(
        run(&p, req()).0,
        Err(AiError::Provider("provider error: boom".into()))
    );
}

// ------------------------------------------------------------------ Cloudflare

fn cf_chunk(delta: serde_json::Value) -> String {
    format!(
        "data: {}\n\n",
        json!({"id":"c1","object":"chat.completion.chunk","model":"@cf/google/gemma-4-26b-a4b-it","choices":[{"index":0,"delta":delta}]})
    )
}

#[test]
fn cloudflare_streams_and_strips_reasoning() {
    let body = [
        cf_chunk(json!({"role":"assistant","reasoning_content":"Let me think about friction."})),
        cf_chunk(json!({"reasoning":"more thoughts"})),
        cf_chunk(json!({"content":"<thi"})),
        cf_chunk(json!({"content":"nk>hidden plan</think>\n\n"})),
        cf_chunk(json!({"content":"Every step "})),
        cf_chunk(json!({"content":"asks if it's worth it."})),
        "data: [DONE]\n\n".to_string(),
    ]
    .concat();
    let mock = Mock::start(move |r| match r.route().as_str() {
        "POST /accounts/acc123/ai/v1/chat/completions" => Resp::sse(body.clone()),
        _ => Resp::not_found(),
    });
    let p = CloudflareWorkersAi::new("acc123", "cf-token").with_base_url(&mock.base);
    let (res, streamed) = run(&p, req());
    let res = res.unwrap();
    assert_eq!(res.text, "Every step asks if it's worth it.");
    assert_eq!(streamed, res.text);
    assert_eq!(res.model, "@cf/google/gemma-4-26b-a4b-it");

    let r = mock.last("POST /accounts/acc123/ai/v1/chat/completions");
    assert_eq!(r.header("authorization"), Some("Bearer cf-token"));
    let b = r.json();
    assert_eq!(b["model"], "@cf/google/gemma-4-26b-a4b-it");
    assert_eq!(b["stream"], true);
    assert_eq!(b["messages"][0], json!({"role":"system","content":"SYS"}));
    assert_eq!(b["messages"][1], json!({"role":"user","content":"USER"}));
}

#[test]
fn cloudflare_401_and_models() {
    let mock = Mock::start(|r| match r.route().as_str() {
        "GET /accounts/acc/ai/models/search" => Resp::json(
            200,
            json!({"success":true,"result":[
                {"name":"@cf/meta/llama-4-scout-17b-16e-instruct","description":"Llama","task":{"name":"Text Generation"}},
                {"name":"@cf/google/gemma-4-26b-a4b-it","description":"Gemma 4","task":{"name":"Text Generation"}}
            ]}),
        ),
        _ => Resp::json(
            401,
            json!({"success":false,"errors":[{"code":10000,"message":"Authentication error"}]}),
        ),
    });
    let p = CloudflareWorkersAi::new("acc", "bad").with_base_url(&mock.base);
    assert!(matches!(
        run(&p, req()).0,
        Err(AiError::Unauthorized { .. })
    ));
    let models = p.list_models().unwrap();
    assert_eq!(
        models[0].id, "@cf/google/gemma-4-26b-a4b-it",
        "Gemma 4 first"
    );
    assert_eq!(models.len(), 2);
    let r = mock.last("GET /accounts/acc/ai/models/search");
    assert!(
        r.path.contains("task=Text+Generation") || r.path.contains("task=Text%20Generation"),
        "{}",
        r.path
    );
}

// ------------------------------------------------------------------ blyg server

#[test]
fn blyg_server_generate() {
    let mock = Mock::start(|r| match r.route().as_str() {
        "POST /api/items/ITEM1/generate" => {
            if r.json()["scope"] == 2 {
                Resp::json(200, json!({"text":"server text","model":"claude-opus-5"}))
            } else {
                Resp::json(400, json!({"error":"unknown scope index"}))
            }
        }
        _ => Resp::not_found(),
    });
    let p = BlygServer::new(&mock.base, "owner-token");
    let mut r = req();
    r.server_scope = Some(ServerScope {
        item_id: "ITEM1".into(),
        scope: 2,
    });
    let (res, streamed) = run(&p, r.clone());
    assert_eq!(
        res.unwrap(),
        GenResult {
            text: "server text".into(),
            model: "claude-opus-5".into()
        }
    );
    assert_eq!(streamed, "server text");
    assert_eq!(
        mock.last("POST /api/items/ITEM1/generate")
            .header("authorization"),
        Some("Bearer owner-token")
    );

    r.server_scope.as_mut().unwrap().scope = 9;
    assert!(matches!(run(&p, r).0, Err(AiError::Provider(m)) if m.contains("unknown scope index")));
    assert!(matches!(run(&p, req()).0, Err(AiError::NotConfigured(_))));
}
