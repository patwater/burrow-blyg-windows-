#![cfg(unix)] // Fake CLIs here are POSIX shell scripts on a `:`-separated PATH.

//! The local Claude Code / Codex bridges against fake `claude` / `codex`
//! shell scripts on a custom PATH. No real CLI is ever run.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use blyg_ai::claude_code::LocalClaudeCode;
use blyg_ai::cli::CliLocator;
use blyg_ai::codex::LocalCodex;
use blyg_ai::*;
use common::*;

/// The temp dir first, then just enough of the system for `cat` & co.
/// (Neither real CLI lives in /bin or /usr/bin.)
fn locator(dir: &Path) -> CliLocator {
    CliLocator {
        path_var: Some(format!("{}:/bin:/usr/bin", dir.display()).into()),
        home: None,
        skip_common_dirs: true,
    }
}

const RECORD: &str = r#"d=$(dirname "$0")
printf '%s\n' "$@" > "$d/args.txt"
cat > "$d/stdin.txt"
pwd > "$d/cwd.txt""#;

fn read(dir: &Path, f: &str) -> String {
    std::fs::read_to_string(dir.join(f)).unwrap_or_default()
}

fn claude(dir: &Path) -> LocalClaudeCode {
    LocalClaudeCode::new().with_locator(locator(dir))
}

fn codex(dir: &Path) -> LocalCodex {
    LocalCodex::new().with_locator(locator(dir))
}

#[test]
fn claude_streams_text_only_with_tools_disabled() {
    let t = tempfile::tempdir().unwrap();
    script(
        t.path(),
        "claude",
        &format!(
            r#"{RECORD}
echo '{{"type":"system","subtype":"init","model":"claude-opus-5","tools":[]}}'
echo '{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"thinking_delta","thinking":"hmm"}}}},"parent_tool_use_id":null}}'
echo '{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"Hello "}}}},"parent_tool_use_id":null}}'
echo 'not json at all'
echo '{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"there"}}}},"parent_tool_use_id":null}}'
echo '{{"type":"assistant","message":{{"model":"claude-opus-5","content":[{{"type":"text","text":"Hello there"}}]}},"parent_tool_use_id":null}}'
echo '{{"type":"result","subtype":"success","is_error":false,"result":"Hello there"}}'"#
        ),
    );
    let p = claude(t.path()).with_model(Some("opus".into()));
    let mut d = Deltas::default();
    let res = p
        .generate(
            GenRequest::new("THE SYSTEM PROMPT", "the user prompt\nline 2"),
            &mut |s| d.0.push(s.into()),
        )
        .unwrap();
    assert_eq!(res.text, "Hello there");
    assert_eq!(res.model, "claude-opus-5");
    assert_eq!(d.0, vec!["Hello ", "there"]);

    let args: Vec<String> = read(t.path(), "args.txt")
        .lines()
        .map(str::to_string)
        .collect();
    let has_pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
    assert_eq!(args[0], "-p");
    assert!(has_pair("--output-format", "stream-json"));
    assert!(args.contains(&"--verbose".into()));
    assert!(args.contains(&"--include-partial-messages".into()));
    assert!(has_pair("--tools", ""), "all tools disabled: {args:?}");
    assert!(has_pair("--permission-mode", "dontAsk"));
    assert!(args.contains(&"--strict-mcp-config".into()));
    assert!(args.contains(&"--no-session-persistence".into()));
    assert!(has_pair("--system-prompt", "THE SYSTEM PROMPT"));
    assert!(has_pair("--model", "opus"));
    assert!(
        !args
            .iter()
            .any(|a| a.contains("dangerously") || a == "--allowedTools")
    );
    assert_eq!(read(t.path(), "stdin.txt"), "the user prompt\nline 2");
    // Ran in a scratch dir, not ours, and cleaned it up.
    let cwd = read(t.path(), "cwd.txt");
    assert!(cwd.contains("blygger-ai-"), "{cwd}");
    assert!(!Path::new(cwd.trim()).exists());
}

#[test]
fn claude_result_without_partials_is_emitted_once() {
    let t = tempfile::tempdir().unwrap();
    script(
        t.path(),
        "claude",
        r#"cat >/dev/null
echo '{"type":"result","subtype":"success","is_error":false,"result":"whole answer"}'"#,
    );
    let mut d = Deltas::default();
    let res = claude(t.path())
        .generate(GenRequest::new("s", "u"), &mut |s| d.0.push(s.into()))
        .unwrap();
    assert_eq!(res.text, "whole answer");
    assert_eq!(d.0, vec!["whole answer"]);
    assert_eq!(res.model, "claude-code");
}

#[test]
fn claude_error_result_and_nonzero_exit() {
    let t = tempfile::tempdir().unwrap();
    script(
        t.path(),
        "claude",
        r#"cat >/dev/null
echo '{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}'
exit 1"#,
    );
    let e = claude(t.path())
        .generate(GenRequest::new("s", "u"), &mut |_| {})
        .unwrap_err();
    assert_eq!(
        e,
        AiError::CliFailed {
            name: "claude".into(),
            code: Some(1),
            message: "Not logged in · Please run /login".into()
        }
    );

    script(
        t.path(),
        "claude",
        "cat >/dev/null\necho 'segfault-ish' >&2\nexit 3",
    );
    let e = claude(t.path())
        .generate(GenRequest::new("s", "u"), &mut |_| {})
        .unwrap_err();
    assert_eq!(
        e,
        AiError::CliFailed {
            name: "claude".into(),
            code: Some(3),
            message: "segfault-ish".into()
        }
    );
}

#[test]
fn missing_binaries() {
    let t = tempfile::tempdir().unwrap();
    let e = claude(t.path())
        .generate(GenRequest::new("s", "u"), &mut |_| {})
        .unwrap_err();
    assert!(matches!(e, AiError::CliNotFound(_)), "{e:?}");
    let e = codex(t.path())
        .generate(GenRequest::new("s", "u"), &mut |_| {})
        .unwrap_err();
    assert!(matches!(e, AiError::CliNotFound(_)), "{e:?}");
    // A non-executable file isn't a binary.
    std::fs::write(t.path().join("claude"), "#!/bin/sh\n").unwrap();
    assert!(claude(t.path()).binary().is_none());
}

#[test]
fn cancel_kills_the_cli() {
    let t = tempfile::tempdir().unwrap();
    script(t.path(), "claude", "cat >/dev/null\nexec sleep 10");
    let c = CancelFlag::new();
    let c2 = c.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        c2.cancel();
    });
    let start = Instant::now();
    let e = claude(t.path())
        .generate(GenRequest::new("s", "u").with_cancel(c), &mut |_| {})
        .unwrap_err();
    assert_eq!(e, AiError::Cancelled);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn codex_exec_read_only_json() {
    let t = tempfile::tempdir().unwrap();
    script(
        t.path(),
        "codex",
        &format!(
            r#"{RECORD}
echo '{{"type":"thread.started","thread_id":"0199a213"}}'
echo '{{"type":"turn.started"}}'
echo '{{"type":"item.completed","item":{{"id":"item_0","type":"reasoning","text":"thinking hard"}}}}'
echo '{{"type":"item.started","item":{{"id":"item_1","type":"agent_message","text":"Codex "}}}}'
echo '{{"type":"item.completed","item":{{"id":"item_1","type":"agent_message","text":"Codex says hi"}}}}'
echo '{{"type":"turn.completed","usage":{{"input_tokens":1,"output_tokens":2}}}}'"#
        ),
    );
    let mut d = Deltas::default();
    let res = codex(t.path())
        .with_model(Some("gpt-6-astra".into()))
        .generate(GenRequest::new("SYS", "USER"), &mut |s| d.0.push(s.into()))
        .unwrap();
    assert_eq!(res.text, "Codex says hi");
    assert_eq!(d.joined(), "Codex says hi");
    assert_eq!(res.model, "gpt-6-astra");

    let args: Vec<String> = read(t.path(), "args.txt")
        .lines()
        .map(str::to_string)
        .collect();
    let has_pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
    assert_eq!(args[0], "exec");
    assert!(args.contains(&"--json".into()));
    assert!(has_pair("--sandbox", "read-only"), "{args:?}");
    assert!(args.contains(&"--ephemeral".into()));
    assert!(args.contains(&"--skip-git-repo-check".into()));
    assert!(has_pair("-m", "gpt-6-astra"));
    assert_eq!(args.last().map(String::as_str), Some("-"));
    assert!(
        !args
            .iter()
            .any(|a| a.contains("dangerously") || a.contains("full-access"))
    );
    let stdin = read(t.path(), "stdin.txt");
    assert!(stdin.contains("SYS") && stdin.contains("USER"));
}

#[test]
fn codex_failures() {
    let t = tempfile::tempdir().unwrap();
    script(
        t.path(),
        "codex",
        r#"cat >/dev/null
echo '{"type":"turn.failed","error":{"message":"You are not logged in"}}'
exit 1"#,
    );
    let e = codex(t.path())
        .generate(GenRequest::new("s", "u"), &mut |_| {})
        .unwrap_err();
    assert_eq!(
        e,
        AiError::CliFailed {
            name: "codex".into(),
            code: Some(1),
            message: "You are not logged in".into()
        }
    );

    script(
        t.path(),
        "codex",
        "cat >/dev/null\necho 'bad flag' >&2\nexit 2",
    );
    let e = codex(t.path())
        .generate(GenRequest::new("s", "u"), &mut |_| {})
        .unwrap_err();
    assert!(
        matches!(e, AiError::CliFailed { code: Some(2), ref message, .. } if message == "bad flag"),
        "{e:?}"
    );
}
