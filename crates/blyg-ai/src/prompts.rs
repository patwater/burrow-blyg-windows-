//! Prompts: TK fill/regenerate with exact parity to the blyg server
//! (`apps/blyg/src/ai/provider.ts` + `tk-generate.ts` + `tk.ts`), and the
//! v1 helpers built on top of it.
//!
//! Every generating helper is expressed as a TK scope and goes through the
//! same system prompt and user-content layout the server uses, so the text
//! it inserts is `[TK]<instruction>[=]<output>[/TK]` and flows through
//! disclosure like any other TK. Proofread is the exception: it returns
//! suggestions, isn't generated prose, and is not disclosed.
//!
//! Offsets are byte offsets into Rust `&str`s (always on char boundaries).
//! The server's are UTF-16 indices; they describe the same positions.

use blyg_core::model::{FRAGMENT_LIMIT, ReadingItem, published_len, strip_tk};
use serde::Deserialize;

use crate::error::{AiError, Result};
use crate::provider::{CancelFlag, GenRequest, GenResult, Provider};

// ------------------------------------------------------------------ server parity

/// `provider.ts::SCOPE_MARK_START`.
pub const SCOPE_MARK_START: &str = "<<<TK-SCOPE>>>";
/// `provider.ts::SCOPE_MARK_END`.
pub const SCOPE_MARK_END: &str = "<<<END-TK-SCOPE>>>";

/// `provider.ts::SYSTEM_PROMPT`, byte for byte.
pub const SYSTEM_PROMPT: &str = concat!(
    "You are the generation engine behind a TK (\"to come\") instructed-generation ",
    "feature in a writing tool. The author has marked a span of their draft with an ",
    "instruction; you write the prose that replaces it. Output ONLY the replacement ",
    "text: no preamble, no meta-commentary, no code fences, no explanation of what ",
    "you did. The output becomes the literal body of the author's document, so match ",
    "the voice, register, and formatting conventions of the surrounding context."
);

/// `util.ts::ID_ALPHABET` (Crockford-ish base32, lowercase).
pub const ID_ALPHABET: &str = "0123456789abcdefghjkmnpqrstvwxyz";

/// `provider.ts` system prompt with the site's `ai_style_prompt` appended
/// (`${SYSTEM_PROMPT}\n\n${stylePrompt}` when non-empty).
pub fn tk_system_prompt(style_prompt: Option<&str>) -> String {
    match style_prompt {
        Some(s) if !s.is_empty() => format!("{SYSTEM_PROMPT}\n\n{s}"),
        _ => SYSTEM_PROMPT.to_string(),
    }
}

/// A source fragment (`![[id]]` inside the scope) resolved to its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub id: String,
    pub content_md: String,
}

/// `provider.ts::GenerateRequest` minus the style prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TkPromptInput {
    pub instruction: String,
    /// The span's current output when regenerating; `None` for a first generation.
    pub current_text: Option<String>,
    pub sources: Vec<Source>,
    /// Full working copy with the active scope wrapped by the scope markers.
    pub document_context: String,
}

/// `provider.ts::buildUserContent`, byte for byte.
pub fn tk_user_content(req: &TkPromptInput) -> String {
    let mut parts: Vec<String> = vec![format!("Instruction: {}", req.instruction)];
    if !req.sources.is_empty() {
        parts.push(
            "Source material the instruction may draw on (weave into the generated prose; \
             do not reproduce verbatim unless the instruction asks for a quote):"
                .to_string(),
        );
        for s in &req.sources {
            parts.push(format!("--- source {} ---\n{}", s.id, s.content_md));
        }
    }
    if let Some(cur) = &req.current_text {
        parts.push(format!(
            "Current draft of this span, to revise per the instruction above:\n{cur}"
        ));
    }
    parts.push(format!(
        "Full document for context, with the active span marked between {SCOPE_MARK_START} and \
         {SCOPE_MARK_END} (the markers are for your orientation only — never reproduce them):\n{}",
        req.document_context
    ));
    parts.join("\n\n")
}

/// `provider.ts::markDocument`: wrap `[start, end)` with the scope markers.
pub fn mark_document(content_md: &str, start: usize, end: usize) -> String {
    format!(
        "{}{SCOPE_MARK_START}{}{SCOPE_MARK_END}{}",
        &content_md[..start],
        &content_md[start..end],
        &content_md[end..]
    )
}

/// One TK scope (`tk.ts::TkScope`), byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TkScope {
    /// Index of the opening `[TK]`.
    pub start: usize,
    /// Index just after the closing `[/TK]`.
    pub end: usize,
    /// Trimmed instruction.
    pub instruction: String,
    /// Text between `[=]` and `[/TK]`; `None` if never generated.
    pub output: Option<String>,
    /// Deduplicated `![[id]]` refs anywhere in the scope, first-occurrence order.
    pub source_ids: Vec<String>,
    /// Where `output` begins (just after `[=]`), if the scope has `[=]`.
    pub output_start: Option<usize>,
    /// Alone in its own paragraph.
    pub block: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TkParseError {
    pub at: usize,
    pub reason: String,
}

fn extract_source_ids(scope_text: &str) -> Vec<String> {
    let mut ids: Vec<String> = vec![];
    let mut rest = scope_text;
    while let Some(i) = rest.find("![[") {
        let after = &rest[i + 3..];
        let candidate: String = after.chars().take(26).collect();
        let valid = candidate.chars().count() == 26
            && candidate.chars().all(|c| ID_ALPHABET.contains(c))
            && after[candidate.len()..].starts_with("]]");
        if valid && !ids.contains(&candidate) {
            ids.push(candidate);
        }
        rest = &rest[i + 3..];
    }
    ids
}

fn blank_before(s: &str) -> bool {
    // /(^|\n[ \t]*\n)[ \t]*$/
    let t = s.trim_end_matches([' ', '\t']);
    if t.is_empty() {
        return true;
    }
    let Some(t) = t.strip_suffix('\n') else {
        return false;
    };
    t.trim_end_matches([' ', '\t']).ends_with('\n')
}

fn blank_after(s: &str) -> bool {
    // /^[ \t]*(\n[ \t]*\n|$)/
    let t = s.trim_start_matches([' ', '\t']);
    if t.is_empty() {
        return true;
    }
    let Some(t) = t.strip_prefix('\n') else {
        return false;
    };
    t.trim_start_matches([' ', '\t']).starts_with('\n')
}

/// `tk.ts::parseScopes`: linear scan, no nesting, no bracket balancing.
pub fn parse_scopes(content_md: &str) -> (Vec<TkScope>, Vec<TkParseError>) {
    let mut scopes = vec![];
    let mut errors = vec![];
    let mut i = 0;
    while let Some(rel) = content_md[i..].find("[TK]") {
        let tk = i + rel;
        let Some(close_rel) = content_md[tk + 4..].find("[/TK]") else {
            errors.push(TkParseError {
                at: tk,
                reason: "unterminated scope (missing [/TK])".into(),
            });
            break;
        };
        let close = tk + 4 + close_rel;
        if let Some(n) = content_md[tk + 4..].find("[TK]").map(|r| tk + 4 + r)
            && n < close
        {
            errors.push(TkParseError {
                at: n,
                reason: "nested TK scopes are not supported".into(),
            });
            i = close + 5;
            continue;
        }
        let eq = content_md[tk + 4..]
            .find("[=]")
            .map(|r| tk + 4 + r)
            .filter(|&e| e < close);
        let instr_end = eq.unwrap_or(close);
        let end = close + 5;
        scopes.push(TkScope {
            start: tk,
            end,
            instruction: content_md[tk + 4..instr_end].trim().to_string(),
            output: eq.map(|e| content_md[e + 3..close].to_string()),
            source_ids: extract_source_ids(&content_md[tk..end]),
            output_start: eq.map(|e| e + 3),
            block: blank_before(&content_md[..tk]) && blank_after(&content_md[end..]),
        });
        i = end;
    }
    (scopes, errors)
}

/// `tk.ts::setScopeOutput`: replace one scope's output, inserting `[=]` if needed.
pub fn set_scope_output(content_md: &str, scope: &TkScope, new_output: &str) -> String {
    let close = scope.end - 5;
    match scope.output_start {
        Some(os) => format!("{}{new_output}{}", &content_md[..os], &content_md[close..]),
        None => format!(
            "{}[=]{new_output}{}",
            &content_md[..close],
            &content_md[close..]
        ),
    }
}

/// A prompt ready for any provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub system: String,
    pub user: String,
}

impl Prompt {
    pub fn request(&self, cancel: &CancelFlag) -> GenRequest {
        GenRequest::new(&self.system, &self.user).with_cancel(cancel.clone())
    }
}

/// A TK fill/regenerate job for scope `index` of `content_md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TkJob {
    pub prompt: Prompt,
    pub scope: TkScope,
    pub index: usize,
    /// The sources used, for provenance (`{id, version}` is the caller's to record).
    pub source_ids: Vec<String>,
}

/// Build the prompt for one scope exactly as `tk-generate.ts::runGenerateScope`
/// does. `resolve(id)` returns a local published fragment's `content_md`.
pub fn tk_job(
    content_md: &str,
    index: usize,
    resolve: &mut dyn FnMut(&str) -> Option<String>,
    style_prompt: Option<&str>,
) -> Result<TkJob> {
    let (scopes, errors) = parse_scopes(content_md);
    if let Some(e) = errors.first() {
        return Err(AiError::Prompt(format!(
            "working copy has malformed TK scopes: {}",
            e.reason
        )));
    }
    let scope = scopes
        .get(index)
        .cloned()
        .ok_or_else(|| AiError::Prompt("unknown scope index".into()))?;
    let mut sources = vec![];
    for id in &scope.source_ids {
        let content =
            resolve(id).ok_or_else(|| AiError::Prompt(format!("unresolvable source {id}")))?;
        sources.push(Source {
            id: id.clone(),
            content_md: content,
        });
    }
    let input = TkPromptInput {
        instruction: scope.instruction.clone(),
        current_text: scope.output.clone(),
        sources,
        document_context: mark_document(content_md, scope.start, scope.end),
    };
    Ok(TkJob {
        prompt: Prompt {
            system: tk_system_prompt(style_prompt),
            user: tk_user_content(&input),
        },
        source_ids: scope.source_ids.clone(),
        scope,
        index,
    })
}

// ------------------------------------------------------------------ TK wrapping

/// Remove TK grammar tokens so text can't break out of (or nest inside) a scope.
fn defang(s: &str) -> String {
    s.replace("[/TK]", "")
        .replace("[TK]", "")
        .replace("[=]", "")
}

/// Wrap generated text as a TK scope: `[TK]<instruction>[=]<output>[/TK]`.
pub fn wrap_tk(instruction: &str, output: &str) -> String {
    format!(
        "[TK]{}[=]{}[/TK]",
        defang(instruction.trim()),
        defang(output)
    )
}

// ------------------------------------------------------------------ helpers

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HelperId {
    FillGap,
    Shorten,
    Continue,
    Outline,
    Proofread,
    Reply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Helper {
    pub id: HelperId,
    pub label: &'static str,
    /// Whether the helper's text is disclosed as generated (inserted as a TK).
    pub disclose: bool,
}

/// The v1 helpers (docs/SPEC.md "AI providers & sign-in").
pub const HELPERS: [Helper; 6] = [
    Helper {
        id: HelperId::FillGap,
        label: "Fill a gap (TK)",
        disclose: true,
    },
    Helper {
        id: HelperId::Shorten,
        label: "Shorten to fit 1000",
        disclose: true,
    },
    Helper {
        id: HelperId::Continue,
        label: "Continue this thought",
        disclose: true,
    },
    Helper {
        id: HelperId::Outline,
        label: "Outline a thread",
        disclose: true,
    },
    Helper {
        id: HelperId::Proofread,
        label: "Proofread",
        disclose: false,
    },
    Helper {
        id: HelperId::Reply,
        label: "Reply to a reading item",
        disclose: true,
    },
];

pub fn helper(id: HelperId) -> Helper {
    HELPERS
        .iter()
        .copied()
        .find(|h| h.id == id)
        .expect("every HelperId is in HELPERS")
}

/// What a generating helper produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperOutput {
    pub instruction: String,
    /// The bare generated text.
    pub output: String,
    /// `wrap_tk(instruction, output)`: what to insert.
    pub insert: String,
    pub model: String,
}

impl HelperOutput {
    fn new(instruction: String, r: GenResult) -> Self {
        let output = r.text.trim_end().to_string();
        HelperOutput {
            insert: wrap_tk(&instruction, &output),
            instruction,
            output,
            model: r.model,
        }
    }
}

/// A TK prompt for a fresh scope `[TK]instruction[/TK]` (or, with
/// `current`, `[TK]instruction[=]current[/TK]`) placed at `at` in `doc`.
fn helper_prompt(
    doc: &str,
    at: usize,
    replace_to: usize,
    instruction: &str,
    current: Option<&str>,
    sources: Vec<Source>,
    style: Option<&str>,
) -> Prompt {
    let scope_text = match current {
        Some(c) => format!("[TK]{instruction}[=]{c}[/TK]"),
        None => format!("[TK]{instruction}[/TK]"),
    };
    let full = format!("{}{scope_text}{}", &doc[..at], &doc[replace_to..]);
    let input = TkPromptInput {
        instruction: instruction.to_string(),
        current_text: current.map(str::to_string),
        sources,
        document_context: mark_document(&full, at, at + scope_text.len()),
    };
    Prompt {
        system: tk_system_prompt(style),
        user: tk_user_content(&input),
    }
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

pub fn shorten_instruction(target: usize, previous_len: Option<usize>) -> String {
    match previous_len {
        None => format!(
            "shorten to fit {FRAGMENT_LIMIT}: rewrite this in at most {target} characters, \
             in the same voice, keeping the key points"
        ),
        Some(n) => format!(
            "shorten to fit {FRAGMENT_LIMIT}: the last version was {n} characters, which is too long; \
             rewrite it in at most {target} characters, in the same voice, keeping only the key points"
        ),
    }
}

/// Character targets for the first try and the tighter retry.
pub const SHORTEN_TARGET: usize = 900;
pub const SHORTEN_RETRY_TARGET: usize = 700;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortenOutcome {
    pub result: HelperOutput,
    /// `published_len(result.insert)`.
    pub len: usize,
    pub fits: bool,
    /// 1, or 2 when the first try was over the limit.
    pub attempts: u32,
}

/// Shorten a fragment to fit 1000 (by `published_len`). The insert replaces
/// the whole working copy. Retries once, tighter, if the first try is over;
/// `fits` is false if the retry is still over.
pub fn shorten_to_fit(
    provider: &dyn Provider,
    text: &str,
    style: Option<&str>,
    cancel: &CancelFlag,
    on_delta: &mut dyn FnMut(&str),
) -> Result<ShortenOutcome> {
    let original = strip_tk(text);
    let instruction = shorten_instruction(SHORTEN_TARGET, None);
    let prompt = helper_prompt(
        &original,
        0,
        original.len(),
        &instruction,
        Some(&original),
        vec![],
        style,
    );
    let first = HelperOutput::new(
        instruction,
        provider.generate(prompt.request(cancel), on_delta)?,
    );
    let len = published_len(&first.insert);
    if len <= FRAGMENT_LIMIT {
        return Ok(ShortenOutcome {
            result: first,
            len,
            fits: true,
            attempts: 1,
        });
    }
    let instruction = shorten_instruction(SHORTEN_RETRY_TARGET, Some(len));
    let prompt = helper_prompt(
        &first.output,
        0,
        first.output.len(),
        &instruction,
        Some(&first.output),
        vec![],
        style,
    );
    let second = HelperOutput::new(
        instruction,
        provider.generate(prompt.request(cancel), on_delta)?,
    );
    let len = published_len(&second.insert);
    Ok(ShortenOutcome {
        result: second,
        len,
        fits: len <= FRAGMENT_LIMIT,
        attempts: 2,
    })
}

pub const CONTINUE_INSTRUCTION: &str = "continue this thought: write the next few sentences in my voice, picking up exactly where the text before this span leaves off";

/// Continue the text at byte offset `at` (usually the caret or the end).
pub fn continue_thought(
    provider: &dyn Provider,
    doc: &str,
    at: usize,
    style: Option<&str>,
    cancel: &CancelFlag,
    on_delta: &mut dyn FnMut(&str),
) -> Result<HelperOutput> {
    let at = floor_boundary(doc, at);
    let prompt = helper_prompt(doc, at, at, CONTINUE_INSTRUCTION, None, vec![], style);
    Ok(HelperOutput::new(
        CONTINUE_INSTRUCTION.to_string(),
        provider.generate(prompt.request(cancel), on_delta)?,
    ))
}

pub const OUTLINE_INSTRUCTION: &str = "outline a thread: turn this fragment into a thread skeleton, \
     a title line and then one short paragraph stub per section, each a sentence or two saying what it will cover";

/// Turn a fragment into a thread skeleton (the insert becomes the new thread's body).
pub fn outline_thread(
    provider: &dyn Provider,
    fragment: &str,
    style: Option<&str>,
    cancel: &CancelFlag,
    on_delta: &mut dyn FnMut(&str),
) -> Result<HelperOutput> {
    let f = strip_tk(fragment);
    let prompt = helper_prompt(&f, 0, f.len(), OUTLINE_INSTRUCTION, Some(&f), vec![], style);
    Ok(HelperOutput::new(
        OUTLINE_INSTRUCTION.to_string(),
        provider.generate(prompt.request(cancel), on_delta)?,
    ))
}

pub fn reply_instruction(item: &ReadingItem) -> String {
    let whom = item
        .author
        .as_ref()
        .and_then(|a| a.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| item.subscription_title.clone());
    let link = item.page.clone().unwrap_or_else(|| item.origin.clone());
    format!(
        "draft a short reply to {whom}'s post ({link}), responding to its main point in my voice"
    )
}

/// Draft a stub reply to a reading item. The post is passed as source
/// material; the insert is the body of a new stub.
pub fn reply_draft(
    provider: &dyn Provider,
    item: &ReadingItem,
    style: Option<&str>,
    cancel: &CancelFlag,
    on_delta: &mut dyn FnMut(&str),
) -> Result<HelperOutput> {
    let instruction = reply_instruction(item);
    let sources = vec![Source {
        id: item.remote_id.clone(),
        content_md: item.content_md.clone(),
    }];
    let prompt = helper_prompt("", 0, 0, &instruction, None, sources, style);
    Ok(HelperOutput::new(
        instruction,
        provider.generate(prompt.request(cancel), on_delta)?,
    ))
}

// ------------------------------------------------------------------ proofread

pub const PROOFREAD_SYSTEM_PROMPT: &str = concat!(
    "You are a careful proofreader. Find typos, misspellings and grammar mistakes in the ",
    "author's text, and nothing else: do not change style, tone, word choice, punctuation ",
    "preferences, Markdown, or anything between [TK] and [/TK] markers. ",
    "Reply with ONLY a JSON array, no prose and no code fences. Each element is ",
    "{\"original\": \"<exact substring of the text, a few words long so it is unambiguous>\", ",
    "\"replacement\": \"<the corrected substring>\", \"reason\": \"<a few words, e.g. typo, ",
    "subject-verb agreement>\"}, in the order they appear. Reply [] if there is nothing to fix."
);

/// One proofreading suggestion over the original text (byte offsets on char
/// boundaries; `&text[start..end] == original`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub start: usize,
    pub end: usize,
    pub original: String,
    pub replacement: String,
    pub reason: String,
}

pub fn proofread_prompt(text: &str) -> Prompt {
    Prompt {
        system: PROOFREAD_SYSTEM_PROMPT.to_string(),
        user: format!("Text to proofread:\n\n{text}"),
    }
}

#[derive(Deserialize)]
struct RawSuggestion {
    original: String,
    replacement: String,
    #[serde(default)]
    reason: String,
}

/// Parse a model reply into suggestions located in `text`. Unlocatable,
/// no-op or overlapping suggestions are dropped.
pub fn parse_suggestions(text: &str, reply: &str) -> Result<Vec<Suggestion>> {
    let r = reply.trim();
    let (Some(a), Some(b)) = (r.find('['), r.rfind(']')) else {
        return Err(AiError::Provider(
            "proofread reply was not a JSON array".into(),
        ));
    };
    if b < a {
        return Err(AiError::Provider(
            "proofread reply was not a JSON array".into(),
        ));
    }
    let raw: Vec<RawSuggestion> = serde_json::from_str(&r[a..=b])
        .map_err(|e| AiError::Provider(format!("proofread reply was not valid JSON: {e}")))?;
    let mut out: Vec<Suggestion> = vec![];
    let mut cursor = 0;
    for s in raw {
        if s.original.is_empty() || s.original == s.replacement {
            continue;
        }
        // In order first; fall back to anywhere.
        let found = text[cursor..]
            .find(&s.original)
            .map(|i| cursor + i)
            .or_else(|| text.find(&s.original));
        let Some(start) = found else { continue };
        let end = start + s.original.len();
        if out.iter().any(|o| start < o.end && o.start < end) {
            continue;
        }
        cursor = end;
        out.push(Suggestion {
            start,
            end,
            original: s.original,
            replacement: s.replacement,
            reason: s.reason,
        });
    }
    out.sort_by_key(|s| s.start);
    Ok(out)
}

/// Proofread (typos and grammar only). Not disclosed; not a TK.
pub fn proofread(
    provider: &dyn Provider,
    text: &str,
    cancel: &CancelFlag,
) -> Result<Vec<Suggestion>> {
    let r = provider.generate(proofread_prompt(text).request(cancel), &mut |_| {})?;
    parse_suggestions(text, &r.text)
}

/// Apply suggestions (non-overlapping, any order) to `text`.
pub fn apply_suggestions(text: &str, suggestions: &[Suggestion]) -> String {
    let mut s: Vec<&Suggestion> = suggestions.iter().collect();
    s.sort_by_key(|x| std::cmp::Reverse(x.start));
    let mut out = text.to_string();
    for x in s {
        if x.end <= out.len() && out.is_char_boundary(x.start) && out.is_char_boundary(x.end) {
            out.replace_range(x.start..x.end, &x.replacement);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdefghjkmnpqrs";

    #[test]
    fn parses_like_tk_ts() {
        let md = format!("a [TK] say hi ![[{ID}]] [/TK] b\n\n[TK]x[=]out[/TK]");
        let (s, e) = parse_scopes(&md);
        assert!(e.is_empty());
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].instruction, format!("say hi ![[{ID}]]"));
        assert_eq!(s[0].output, None);
        assert_eq!(s[0].source_ids, vec![ID.to_string()]);
        assert!(!s[0].block);
        assert_eq!(s[1].output.as_deref(), Some("out"));
        assert!(s[1].block);
        assert_eq!(&md[s[1].start..s[1].end], "[TK]x[=]out[/TK]");
    }

    #[test]
    fn parse_errors() {
        let (s, e) = parse_scopes("[TK]a [TK]b[/TK] c");
        assert!(s.is_empty());
        assert_eq!(e[0].reason, "nested TK scopes are not supported");
        let (_, e) = parse_scopes("x [TK] never");
        assert_eq!(e[0].reason, "unterminated scope (missing [/TK])");
    }

    #[test]
    fn set_output_inserts_or_replaces() {
        let md = "é [TK]go[/TK] z";
        let (s, _) = parse_scopes(md);
        let md2 = set_scope_output(md, &s[0], "ok");
        assert_eq!(md2, "é [TK]go[=]ok[/TK] z");
        let (s, _) = parse_scopes(&md2);
        assert_eq!(
            set_scope_output(&md2, &s[0], "new"),
            "é [TK]go[=]new[/TK] z"
        );
    }

    #[test]
    fn wrap_defangs_tokens() {
        assert_eq!(wrap_tk(" do it ", "a [/TK] b"), "[TK]do it[=]a  b[/TK]");
        let w = wrap_tk("x", "out");
        assert_eq!(strip_tk(&w), "out");
    }

    #[test]
    fn proofread_is_not_disclosed() {
        assert!(!helper(HelperId::Proofread).disclose);
        assert!(
            HELPERS
                .iter()
                .filter(|h| h.id != HelperId::Proofread)
                .all(|h| h.disclose)
        );
    }
}
