//! Prompt parity with the blyg server, and the helpers (with a scripted
//! in-memory provider).

use std::collections::VecDeque;
use std::sync::Mutex;

use blyg_ai::prompts::*;
use blyg_ai::*;
use blyg_core::model::{Author, FRAGMENT_LIMIT, Kind, ReadingItem, published_len, strip_tk};
use serde_json::Value;

// ------------------------------------------------------------------ parity

/// Generated from the server's own TypeScript by `fixtures/gen_parity.mjs`.
const PARITY: &str = include_str!("fixtures/provider_ts_parity.json");

/// provider.ts SYSTEM_PROMPT, copied verbatim (the JS concatenation, joined).
const PROVIDER_TS_SYSTEM_PROMPT: &str = "You are the generation engine behind a TK (\"to come\") instructed-generation feature in a writing tool. The author has marked a span of their draft with an instruction; you write the prose that replaces it. Output ONLY the replacement text: no preamble, no meta-commentary, no code fences, no explanation of what you did. The output becomes the literal body of the author's document, so match the voice, register, and formatting conventions of the surrounding context.";

#[test]
fn system_prompt_and_markers_match_provider_ts() {
    assert_eq!(SYSTEM_PROMPT, PROVIDER_TS_SYSTEM_PROMPT);
    assert_eq!(SCOPE_MARK_START, "<<<TK-SCOPE>>>");
    assert_eq!(SCOPE_MARK_END, "<<<END-TK-SCOPE>>>");
    assert_eq!(tk_system_prompt(None), SYSTEM_PROMPT);
    assert_eq!(tk_system_prompt(Some("")), SYSTEM_PROMPT);
    assert_eq!(
        tk_system_prompt(Some("Plain words.")),
        format!("{SYSTEM_PROMPT}\n\nPlain words.")
    );
}

#[test]
fn tk_prompts_match_the_server_byte_for_byte() {
    let cases: Vec<Value> = serde_json::from_str(PARITY).unwrap();
    assert!(cases.len() >= 3);
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let md = c["content_md"].as_str().unwrap();
        let sources = c["sources"].as_object().unwrap().clone();
        let style = c["style"].as_str();
        let job = tk_job(
            md,
            c["scope"].as_u64().unwrap() as usize,
            &mut |id| sources.get(id).and_then(Value::as_str).map(str::to_string),
            style,
        )
        .unwrap();
        assert_eq!(
            job.prompt.system,
            c["expected_system"].as_str().unwrap(),
            "{name}: system"
        );
        assert_eq!(
            job.prompt.user,
            c["expected_user"].as_str().unwrap(),
            "{name}: user"
        );
        let ids: Vec<String> = serde_json::from_value(c["expected_source_ids"].clone()).unwrap();
        assert_eq!(job.source_ids, ids, "{name}: sources");
    }
}

#[test]
fn tk_job_errors_like_the_server() {
    let id = "0123456789abcdefghjkmnpqrs";
    let md = format!("[TK]use ![[{id}]][/TK]");
    let e = tk_job(&md, 0, &mut |_| None, None).unwrap_err();
    assert_eq!(e, AiError::Prompt(format!("unresolvable source {id}")));
    assert_eq!(
        tk_job(&md, 3, &mut |_| Some(String::new()), None).unwrap_err(),
        AiError::Prompt("unknown scope index".into())
    );
    assert!(
        matches!(tk_job("[TK]open", 0, &mut |_| None, None), Err(AiError::Prompt(m)) if m.contains("malformed"))
    );
}

// ------------------------------------------------------------------ helpers

/// Replies from a script, records requests.
struct Scripted {
    replies: Mutex<VecDeque<String>>,
    seen: Mutex<Vec<GenRequest>>,
}

impl Scripted {
    fn new(replies: &[&str]) -> Self {
        Scripted {
            replies: Mutex::new(replies.iter().map(|s| s.to_string()).collect()),
            seen: Mutex::default(),
        }
    }
    fn seen(&self) -> Vec<GenRequest> {
        self.seen.lock().unwrap().clone()
    }
}

impl Provider for Scripted {
    fn kind(&self) -> ProviderKind {
        ProviderKind::AnthropicApi
    }
    fn generate(&self, req: GenRequest, on_delta: &mut dyn FnMut(&str)) -> Result<GenResult> {
        req.cancel.check()?;
        self.seen.lock().unwrap().push(req);
        let text = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("a scripted reply");
        on_delta(&text);
        Ok(GenResult {
            text,
            model: "test-model".into(),
        })
    }
}

#[test]
fn shorten_retries_once_when_over_the_limit() {
    let long = "word ".repeat(300); // 1500 chars
    let still_long = "é".repeat(1200); // over 1000 in UTF-16 too
    let short = "Each step asks whether the idea is worth it.";
    let p = Scripted::new(&[&still_long, short]);
    let mut streamed = String::new();
    let out = shorten_to_fit(&p, &long, Some("Plain."), &CancelFlag::new(), &mut |d| {
        streamed.push_str(d)
    })
    .unwrap();
    assert_eq!(out.attempts, 2);
    assert!(out.fits);
    assert_eq!(out.result.output, short);
    assert_eq!(out.len, published_len(&out.result.insert));
    assert!(out.len <= FRAGMENT_LIMIT);
    assert!(out.result.insert.starts_with("[TK]shorten to fit 1000"));
    assert_eq!(strip_tk(&out.result.insert), short);

    let seen = p.seen();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].user.contains("at most 900 characters"));
    assert!(
        seen[0].system.ends_with("\n\nPlain."),
        "style prompt applied"
    );
    assert!(
        seen[1].user.contains("was 1200 characters"),
        "{}",
        seen[1].user
    );
    assert!(seen[1].user.contains("at most 700 characters"));
    // The retry revises the too-long attempt.
    assert!(seen[1].user.contains(&format!(
        "Current draft of this span, to revise per the instruction above:\n{still_long}"
    )));
}

#[test]
fn shorten_fits_first_time_or_reports_failure() {
    let p = Scripted::new(&["short enough"]);
    let out = shorten_to_fit(&p, &"x".repeat(1100), None, &CancelFlag::new(), &mut |_| {}).unwrap();
    assert_eq!((out.attempts, out.fits), (1, true));

    let over = "y".repeat(1001);
    let p = Scripted::new(&[&over, &over]);
    let out = shorten_to_fit(&p, &"x".repeat(1100), None, &CancelFlag::new(), &mut |_| {}).unwrap();
    assert_eq!((out.attempts, out.fits, out.len), (2, false, 1001));
}

#[test]
fn shorten_counts_utf16_like_the_worker() {
    // 600 emoji = 600 chars but 1200 UTF-16 units: over the Worker's cap.
    let emoji = "🙂".repeat(600);
    let p = Scripted::new(&[&emoji, "ok"]);
    let out = shorten_to_fit(&p, &"x".repeat(1100), None, &CancelFlag::new(), &mut |_| {}).unwrap();
    assert_eq!(
        out.attempts, 2,
        "the first reply is over 1000 by published_len"
    );
}

#[test]
fn continue_outline_and_reply_are_tk_scopes() {
    let doc = "On friction: every step is a place the thought can die.";
    let p = Scripted::new(&["It isn't only the minutes lost.\n"]);
    let out = continue_thought(&p, doc, doc.len(), None, &CancelFlag::new(), &mut |_| {}).unwrap();
    assert_eq!(out.output, "It isn't only the minutes lost.");
    assert_eq!(
        out.insert,
        format!("[TK]{CONTINUE_INSTRUCTION}[=]It isn't only the minutes lost.[/TK]")
    );
    let u = &p.seen()[0].user;
    assert!(
        u.contains(&format!(
            "{doc}<<<TK-SCOPE>>>[TK]{CONTINUE_INSTRUCTION}[/TK]<<<END-TK-SCOPE>>>"
        )),
        "{u}"
    );
    assert!(
        !u.contains("Current draft"),
        "a first generation has no current text"
    );

    let p = Scripted::new(&["Title\n\nPart one…"]);
    let out = outline_thread(&p, doc, None, &CancelFlag::new(), &mut |_| {}).unwrap();
    assert!(out.insert.starts_with("[TK]outline a thread"));
    assert!(p.seen()[0].user.contains(&format!(
        "Current draft of this span, to revise per the instruction above:\n{doc}"
    )));

    let item = ReadingItem {
        subscription_id: "S".into(),
        remote_id: "R1".into(),
        subscription_title: "Their blyg".into(),
        origin: "https://them.example/".into(),
        kind: Kind::Fragment,
        state: "current".into(),
        version: 1,
        created: None,
        updated: None,
        observed_at: "2026-09-24T00:00:00Z".into(),
        content_md: "Tools should disappear.".into(),
        content_html: String::new(),
        author: Some(Author {
            name: Some("Sam".into()),
            url: None,
        }),
        page: Some("https://them.example/f/R1/".into()),
        thumb: None,
        hoppers: vec![],
        read_version: None,
        stub_of: None,
        forked_from: None,
        transclusions: vec![],
        pinned_version_retained: None,
    };
    let p = Scripted::new(&["Agreed, and…"]);
    let out = reply_draft(&p, &item, None, &CancelFlag::new(), &mut |_| {}).unwrap();
    assert!(
        out.instruction
            .contains("Sam's post (https://them.example/f/R1/)")
    );
    assert_eq!(strip_tk(&out.insert), "Agreed, and…");
    assert!(
        p.seen()[0]
            .user
            .contains("--- source R1 ---\nTools should disappear.")
    );
}

#[test]
fn helpers_respect_cancel() {
    let p = Scripted::new(&["x"]);
    let c = CancelFlag::new();
    c.cancel();
    assert_eq!(
        continue_thought(&p, "a", 1, None, &c, &mut |_| {}).unwrap_err(),
        AiError::Cancelled
    );
}

// ------------------------------------------------------------------ proofread

#[test]
fn proofread_offsets_are_valid_on_multibyte_text() {
    let text = "Café ☕ is naïve 🙂 teh best. Their going home. 日本語 recieve teh end.";
    let reply = r#"Here you go:
```json
[
  {"original": "teh best", "replacement": "the best", "reason": "typo"},
  {"original": "Their going", "replacement": "They're going", "reason": "grammar"},
  {"original": "recieve", "replacement": "receive", "reason": "spelling"},
  {"original": "teh end", "replacement": "the end", "reason": "typo"},
  {"original": "not in the text", "replacement": "x", "reason": "hallucinated"},
  {"original": "naïve", "replacement": "naïve", "reason": "no-op"}
]
```"#;
    let p = Scripted::new(&[reply]);
    let s = proofread(&p, text, &CancelFlag::new()).unwrap();
    assert_eq!(s.len(), 4);
    for x in &s {
        assert!(text.is_char_boundary(x.start) && text.is_char_boundary(x.end));
        assert_eq!(&text[x.start..x.end], x.original);
    }
    assert!(
        s.windows(2).all(|w| w[0].end <= w[1].start),
        "sorted, non-overlapping"
    );
    assert_eq!(s[0].reason, "typo");
    assert_eq!(
        apply_suggestions(text, &s),
        "Café ☕ is naïve 🙂 the best. They're going home. 日本語 receive the end."
    );
    // Proofread uses its own prompt, not the TK one, and isn't disclosed.
    assert_eq!(p.seen()[0].system, PROOFREAD_SYSTEM_PROMPT);
    assert!(!helper(HelperId::Proofread).disclose);
}

#[test]
fn proofread_rejects_non_json() {
    let p = Scripted::new(&["Looks fine to me!"]);
    assert!(matches!(
        proofread(&p, "text", &CancelFlag::new()),
        Err(AiError::Provider(_))
    ));
    let p = Scripted::new(&["[]"]);
    assert_eq!(proofread(&p, "text", &CancelFlag::new()).unwrap(), vec![]);
}
