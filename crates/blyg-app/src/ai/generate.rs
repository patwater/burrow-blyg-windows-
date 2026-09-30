//! TK generation, UI-free: find the scope under the caret, build its prompt
//! (blyg-ai's `prompts`, byte-for-byte parity with the Worker), splice the
//! output back, and work out the provenance array to save with it.
//!
//! Provenance is keyed by scope *position* (docs/SPEC.md § Client-recorded
//! provenance), so every write that changes the scopes goes through
//! `Backend::save_with_provenance` with one entry per scope.

use blyg_ai::AiError;
use blyg_ai::prompts::{self, Prompt};
use blyg_core::{Backend, Item, ProvenanceSource, ReadingItem, ScopeProvenance, tk};

/// What's under the caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Under {
    /// The caret is inside `[TK]…[/TK]` scope `index`.
    Scope(usize),
    /// The TK markup is broken (unterminated or nested): nothing can run.
    Malformed,
    /// No scope here.
    Nothing,
}

/// The TK scope that contains byte offset `caret` (its brackets included).
pub fn under_caret(md: &str, caret: usize) -> Under {
    match tk::parse(md) {
        None => {
            // An open `[TK]` before the caret with no `[/TK]` is "inside" a
            // broken scope; anything else is just text.
            let before = &md[..caret.min(md.len())];
            match (before.rfind("[TK]"), before.rfind("[/TK]")) {
                (Some(o), c) if c.is_none_or(|c| c < o) => Under::Malformed,
                _ => Under::Nothing,
            }
        }
        Some(scopes) => scopes
            .iter()
            .position(|s| s.start <= caret && caret <= s.end)
            .map_or(Under::Nothing, Under::Scope),
    }
}

/// A transcluded id (`![[id]]`) resolved from what's already held locally:
/// your own posts (by server id) and imported reading items. Never fetched.
pub fn resolve_source(
    items: &[Item],
    reading: &[ReadingItem],
    id: &str,
) -> Option<(String, Option<u32>)> {
    items
        .iter()
        .find(|i| i.server_id.as_ref().is_some_and(|s| s.0 == id))
        .map(|i| (i.content_md.clone(), (i.version > 0).then_some(i.version)))
        .or_else(|| {
            reading
                .iter()
                .find(|r| r.remote_id == id)
                .map(|r| (r.content_md.clone(), Some(r.version)))
        })
}

/// A fill/regenerate request, built on the UI thread and run elsewhere.
#[derive(Debug, Clone)]
pub struct FillJob {
    pub index: usize,
    pub instruction: String,
    pub prompt: Prompt,
    /// The sources the scope quotes, for provenance.
    pub sources: Vec<ProvenanceSource>,
}

pub fn prepare_fill(
    md: &str,
    index: usize,
    items: &[Item],
    reading: &[ReadingItem],
    style: Option<&str>,
) -> Result<FillJob, AiError> {
    let mut versions = Vec::new();
    let job = prompts::tk_job(
        md,
        index,
        &mut |id| {
            let (content, version) = resolve_source(items, reading, id)?;
            versions.push((id.to_string(), version));
            Some(content)
        },
        style,
    )?;
    let sources = job
        .source_ids
        .iter()
        .map(|id| ProvenanceSource {
            id: id.clone(),
            version: versions.iter().find(|(v, _)| v == id).and_then(|(_, v)| *v),
        })
        .collect();
    Ok(FillJob {
        index,
        instruction: job.scope.instruction.clone(),
        prompt: job.prompt,
        sources,
    })
}

/// Model output made safe to put inside a scope: TK tokens removed (so it
/// can't close or nest a scope) and trailing whitespace trimmed.
pub fn clean_output(s: &str) -> String {
    s.replace("[/TK]", "")
        .replace("[TK]", "")
        .replace("[=]", "")
        .trim()
        .to_string()
}

/// Write `output` into the scope the job was for, in the text as it is
/// *now* (the user may have typed elsewhere meanwhile). The scope is found
/// by its index and instruction; `None` if it's gone or was changed.
pub fn apply_fill(md_now: &str, index: usize, instruction: &str, output: &str) -> Option<String> {
    let (scopes, errors) = prompts::parse_scopes(md_now);
    if !errors.is_empty() {
        return None;
    }
    let scope = scopes
        .get(index)
        .filter(|s| s.instruction == instruction)
        .or_else(|| scopes.iter().find(|s| s.instruction == instruction))?;
    Some(prompts::set_scope_output(
        md_now,
        scope,
        &clean_output(output),
    ))
}

/// Insert a generated scope (`[TK]instruction[=]output[/TK]`) at byte
/// offset `at`. Returns the new text and the new scope's index.
pub fn insert_scope(md: &str, at: usize, insert: &str) -> Option<(String, usize)> {
    let mut at = at.min(md.len());
    while !md.is_char_boundary(at) {
        at -= 1;
    }
    let before = &md[..at];
    let sep = if before.is_empty() || before.ends_with(char::is_whitespace) {
        ""
    } else {
        " "
    };
    let new = format!("{before}{sep}{insert}{}", &md[at..]);
    let start = at + sep.len();
    let index = tk::parse(&new)?.iter().position(|s| s.start == start)?;
    Some((new, index))
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The provenance array for `new_md`: whatever was tracked (carried over
/// to the new text), with scope `index` set to `p`. `None` if `new_md` has
/// broken TK markup.
pub fn provenance_with(
    tracked: Option<(String, Vec<Option<ScopeProvenance>>)>,
    new_md: &str,
    index: usize,
    p: ScopeProvenance,
) -> Option<Vec<Option<ScopeProvenance>>> {
    let scopes = tk::parse(new_md)?;
    let n = scopes.len();
    let mut v = tracked
        .and_then(|(k, s)| tk::remap(&k, &s, new_md))
        .filter(|v| v.len() == n)
        .unwrap_or_else(|| vec![None; n]);
    if let Some(slot) = v.get_mut(index) {
        // Cite only what the scope still quotes (the Worker refuses others).
        let quoted = &scopes[index].source_ids;
        *slot = Some(ScopeProvenance {
            sources: p
                .sources
                .into_iter()
                .filter(|s| quoted.contains(&s.id))
                .collect(),
            ..p
        });
    }
    Some(v)
}

/// The tracked provenance, carried to `md`.
pub fn tracked_for(backend: &dyn Backend, item: &Item, md: &str) -> Vec<Option<ScopeProvenance>> {
    let n = tk::scope_count(md).unwrap_or(0);
    backend
        .tracked_tk_provenance(&item.local_id)
        .and_then(|(k, s)| tk::remap(&k, &s, md))
        .filter(|v| v.len() == n)
        .unwrap_or_else(|| vec![None; n])
}

/// Whether the item carries text generated in the app (a scope with
/// recorded provenance).
pub fn has_generated(backend: &dyn Backend, item: &Item) -> bool {
    tracked_for(backend, item, &item.content_md)
        .iter()
        .any(Option::is_some)
}

/// Publishing this item would drop its AI disclosure: it has app-generated
/// scopes and the server has no provenance extension (it answered 404).
pub fn publish_needs_warning(backend: &dyn Backend, item: &Item) -> bool {
    !backend.provenance_available() && has_generated(backend, item)
}

/// A plain edit that changed the set of TK scopes: the provenance array to
/// push with it (so the server's position-keyed disclosure can't slide
/// onto the wrong span). `None` when a plain save is enough: nothing
/// tracked, the scopes didn't change, or the markup is mid-typing.
pub fn edit_provenance(
    backend: &dyn Backend,
    item: &Item,
    old_md: &str,
    new_md: &str,
) -> Option<Vec<Option<ScopeProvenance>>> {
    if !tk::structure_changed(old_md, new_md) {
        return None;
    }
    let (k, s) = backend.tracked_tk_provenance(&item.local_id)?;
    if !s.iter().any(Option::is_some) {
        return None;
    }
    let v = tk::remap(&k, &s, new_md)?;
    tk::validate(new_md, &v).ok()?;
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdefghjkmnpqrs";

    fn prov(model: &str) -> ScopeProvenance {
        ScopeProvenance {
            model: model.into(),
            sources: vec![],
            at: None,
        }
    }

    #[test]
    fn finds_the_scope_under_the_caret() {
        let md = "a [TK]one[/TK] b [TK]two[=]x[/TK] c";
        assert_eq!(under_caret(md, 0), Under::Nothing);
        assert_eq!(under_caret(md, 4), Under::Scope(0));
        assert_eq!(under_caret(md, md.find("two").unwrap()), Under::Scope(1));
        assert_eq!(under_caret(md, md.len()), Under::Nothing);
        assert_eq!(under_caret("x [TK]open", 8), Under::Malformed);
        assert_eq!(under_caret("[TK]a[/TK] [TK]open", 2), Under::Nothing);
    }

    #[test]
    fn fills_and_regenerates_in_place() {
        let md = "Intro. [TK]say hi[/TK] End.";
        let job = prepare_fill(md, 0, &[], &[], Some("Plain words.")).unwrap();
        assert_eq!(job.instruction, "say hi");
        assert!(job.prompt.system.ends_with("\n\nPlain words."));
        assert!(job.prompt.user.contains("<<<TK-SCOPE>>>"));
        let out = apply_fill(md, 0, "say hi", " Hello [/TK] there. ").unwrap();
        assert_eq!(out, "Intro. [TK]say hi[=]Hello  there.[/TK] End.");
        let again = apply_fill(&out, 0, "say hi", "Hi.").unwrap();
        assert_eq!(again, "Intro. [TK]say hi[=]Hi.[/TK] End.");
        // The scope was edited away meanwhile: nothing is written.
        assert_eq!(apply_fill("Intro. End.", 0, "say hi", "Hi"), None);
    }

    #[test]
    fn unresolvable_sources_are_an_error() {
        let md = format!("[TK]quote ![[{ID}]][/TK]");
        let e = prepare_fill(&md, 0, &[], &[], None).unwrap_err();
        assert!(e.to_string().contains("unresolvable source"));
    }

    #[test]
    fn provenance_keeps_other_scopes_and_cites_quoted_sources() {
        let md = format!("[TK]a[=]x[/TK] [TK]b ![[{ID}]][=]y[/TK]");
        let tracked = Some((md.clone(), vec![Some(prov("m1")), None]));
        let p = ScopeProvenance {
            sources: vec![
                ProvenanceSource {
                    id: ID.into(),
                    version: Some(2),
                },
                ProvenanceSource {
                    id: "zzzzzzzzzzzzzzzzzzzzzzzzzz".into(),
                    version: None,
                },
            ],
            ..prov("m2")
        };
        let v = provenance_with(tracked, &md, 1, p).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].as_ref().unwrap().model, "m1");
        let s = v[1].as_ref().unwrap();
        assert_eq!(s.model, "m2");
        assert_eq!(s.sources.len(), 1);
        assert!(tk::validate(&md, &v).is_ok());
    }

    #[test]
    fn inserts_a_scope_at_the_caret() {
        let (md, i) = insert_scope("One. [TK]a[=]b[/TK] Two.", 24, "[TK]go[=]on[/TK]").unwrap();
        assert_eq!(md, "One. [TK]a[=]b[/TK] Two. [TK]go[=]on[/TK]");
        assert_eq!(i, 1);
        let (md, i) = insert_scope("", 0, "[TK]go[=]on[/TK]").unwrap();
        assert_eq!((md.as_str(), i), ("[TK]go[=]on[/TK]", 0));
    }
}
