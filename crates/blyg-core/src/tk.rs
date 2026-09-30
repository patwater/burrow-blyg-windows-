//! TK scopes as the Worker sees them, and the client side of the
//! client-recorded provenance extension (docs/SPEC.md § Client-recorded
//! provenance).
//!
//! The Worker keys provenance by scope **position**: entry `i` of the cache
//! describes the `i`-th `[TK]…[/TK]` scope of the working copy. So whenever
//! the set of scopes changes, the text and the whole provenance array must be
//! pushed together (`PUT /api/items/:id/tk-provenance`), never a plain
//! `PUT /api/items/:id`, or disclosure lands on the wrong span. `remap`
//! carries per-scope provenance across an edit.

use crate::model::{ProvenanceSource, ScopeProvenance};

/// One `[TK]<instruction>[=]<output>[/TK]` scope (byte offsets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub start: usize,
    pub end: usize,
    /// Trimmed, as the Worker compares it.
    pub instruction: String,
    /// `None` until generated (no `[=]`).
    pub output: Option<String>,
    /// `![[id]]` refs anywhere in the scope, first occurrence first, deduplicated.
    pub source_ids: Vec<String>,
}

/// The scopes of `md`, or `None` when the grammar is malformed (unterminated
/// or nested), exactly where the Worker's `tk.ts::parseScopes` reports errors.
pub fn parse(md: &str) -> Option<Vec<Scope>> {
    let mut scopes = Vec::new();
    let mut i = 0;
    let mut ok = true;
    while let Some(rel) = md[i..].find("[TK]") {
        let tk = i + rel;
        let close_rel = md[tk + 4..].find("[/TK]")?; // unterminated
        let close = tk + 4 + close_rel;
        if md[tk + 4..close].contains("[TK]") {
            ok = false;
            i = close + 5;
            continue;
        }
        let body = &md[tk + 4..close];
        let (instruction, output) = match body.find("[=]") {
            Some(eq) => (&body[..eq], Some(body[eq + 3..].to_string())),
            None => (body, None),
        };
        let end = close + 5;
        scopes.push(Scope {
            start: tk,
            end,
            instruction: instruction.trim().to_string(),
            output,
            source_ids: source_ids(&md[tk..end]),
        });
        i = end;
    }
    ok.then_some(scopes)
}

/// The Worker's id alphabet (Crockford-ish base32, lowercase).
const ID_ALPHABET: &str = "0123456789abcdefghjkmnpqrstvwxyz";

fn source_ids(scope: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = scope;
    while let Some(p) = rest.find("![[") {
        let after = &rest[p + 3..];
        if after.len() >= 28
            && after.as_bytes()[26..28] == *b"]]"
            && after[..26].chars().all(|c| ID_ALPHABET.contains(c))
        {
            let id = after[..26].to_string();
            if !out.contains(&id) {
                out.push(id);
            }
            rest = &after[28..];
        } else {
            rest = after;
        }
    }
    out
}

/// Scope count, the number of provenance entries the Worker expects.
/// `None` when malformed.
pub fn scope_count(md: &str) -> Option<usize> {
    parse(md).map(|s| s.len())
}

/// Whether the scope *structure* (the sequence of scopes, by instruction and
/// generated-ness) differs between two texts. Malformed text never equals
/// anything.
pub fn structure_changed(old: &str, new: &str) -> bool {
    let key = |s: &Scope| (s.instruction.clone(), s.output.is_some());
    match (parse(old), parse(new)) {
        (Some(a), Some(b)) => {
            a.iter().map(key).collect::<Vec<_>>() != b.iter().map(key).collect::<Vec<_>>()
        }
        _ => true,
    }
}

/// Check a provenance array against a working copy the way the Worker will:
/// one entry per scope, and a non-null entry only on a generated scope, only
/// citing sources the scope references. Returns the reason on failure.
pub fn validate(md: &str, scopes: &[Option<ScopeProvenance>]) -> Result<(), String> {
    let parsed = parse(md).ok_or("the text has malformed TK scopes")?;
    if parsed.len() != scopes.len() {
        return Err(format!(
            "{} provenance entries for {} TK scopes",
            scopes.len(),
            parsed.len()
        ));
    }
    for (i, (s, p)) in parsed.iter().zip(scopes).enumerate() {
        let Some(p) = p else { continue };
        if s.output.is_none() {
            return Err(format!("scope {i} has provenance but no output"));
        }
        if p.model.trim().is_empty() {
            return Err(format!("scope {i}: model required"));
        }
        if let Some(src) = p.sources.iter().find(|x| !s.source_ids.contains(&x.id)) {
            return Err(format!("scope {i}: source {} isn't quoted in it", src.id));
        }
    }
    Ok(())
}

/// Carry provenance recorded against `old_md` over to `new_md`.
///
/// Scopes are matched in order by instruction (the first unused old scope
/// with the same instruction). A matched scope keeps its provenance unless it
/// is no longer generated, or its output was rewritten entirely by hand (no
/// word left in common with the generated text). Sources the new scope no
/// longer quotes are dropped. Unmatched scopes get `None`. A malformed
/// `new_md` yields `None` (can't be keyed yet).
pub fn remap(
    old_md: &str,
    old: &[Option<ScopeProvenance>],
    new_md: &str,
) -> Option<Vec<Option<ScopeProvenance>>> {
    let new_scopes = parse(new_md)?;
    let old_scopes = parse(old_md).unwrap_or_default();
    let mut used = vec![false; old_scopes.len()];
    let out = new_scopes
        .iter()
        .map(|ns| {
            let (i, os) = old_scopes
                .iter()
                .enumerate()
                .find(|(i, os)| !used[*i] && os.instruction == ns.instruction)?;
            used[i] = true;
            let p = old.get(i)?.as_ref()?;
            let (Some(new_out), Some(old_out)) = (&ns.output, &os.output) else {
                return None;
            };
            if new_out != old_out && !shares_a_word(new_out, old_out) {
                return None;
            }
            let sources: Vec<ProvenanceSource> = p
                .sources
                .iter()
                .filter(|s| ns.source_ids.contains(&s.id))
                .cloned()
                .collect();
            Some(ScopeProvenance {
                sources,
                ..p.clone()
            })
        })
        .collect();
    Some(out)
}

fn shares_a_word(a: &str, b: &str) -> bool {
    let words = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() > 2)
            .map(str::to_lowercase)
            .collect()
    };
    let wb = words(b);
    words(a).iter().any(|w| wb.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(model: &str) -> Option<ScopeProvenance> {
        Some(ScopeProvenance {
            model: model.into(),
            sources: vec![],
            at: None,
        })
    }

    #[test]
    fn parses_like_the_worker() {
        let s = parse("a [TK] say hi [=]hello[/TK] b [TK]later[/TK]").unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].instruction, "say hi");
        assert_eq!(s[0].output.as_deref(), Some("hello"));
        assert_eq!(s[1].output, None);
        assert_eq!(parse("x [TK]open"), None, "unterminated");
        assert_eq!(parse("[TK]a [TK]b[/TK]"), None, "nested");
        assert_eq!(parse("no scopes").unwrap(), vec![]);
        let ids = parse(
            "[TK]use ![[0123456789abcdefghjkmnpqrs]] and ![[0123456789abcdefghjkmnpqrs]][=]x[/TK]",
        )
        .unwrap();
        assert_eq!(ids[0].source_ids, vec!["0123456789abcdefghjkmnpqrs"]);
    }

    #[test]
    fn structure_changes() {
        let a = "x [TK]one[=]1[/TK] y [TK]two[=]2[/TK]";
        assert!(!structure_changed(
            a,
            "edited x [TK]one[=]1 more[/TK] y [TK]two[=]2[/TK]"
        ));
        assert!(structure_changed(a, "[TK]two[=]2[/TK]"), "removed one");
        assert!(structure_changed(
            a,
            "[TK]zero[=]0[/TK] x [TK]one[=]1[/TK] y [TK]two[=]2[/TK]"
        ));
        assert!(
            structure_changed(a, "x [TK]one[/TK] y [TK]two[=]2[/TK]"),
            "ungenerated"
        );
        assert!(structure_changed(a, "[TK]broken"));
    }

    #[test]
    fn remap_follows_scopes_by_position_change() {
        let old = "[TK]one[=]first output[/TK] and [TK]two[=]second output[/TK]";
        let prov = vec![p("m1"), p("m2")];
        // A scope inserted in front: provenance shifts with its scope.
        let new =
            "[TK]zero[=]new[/TK] [TK]one[=]first output[/TK] and [TK]two[=]second output[/TK]";
        assert_eq!(
            remap(old, &prov, new).unwrap(),
            vec![None, p("m1"), p("m2")]
        );
        // One removed.
        let new = "[TK]two[=]second output[/TK]";
        assert_eq!(remap(old, &prov, new).unwrap(), vec![p("m2")]);
        // Lightly edited output keeps it; rewritten entirely by hand drops it.
        let new = "[TK]one[=]first output, tidied[/TK] and [TK]two[=]all mine now[/TK]";
        assert_eq!(remap(old, &prov, new).unwrap(), vec![p("m1"), None]);
        // Malformed new text can't be keyed.
        assert_eq!(remap(old, &prov, "[TK]one"), None);
    }

    #[test]
    fn validates_like_the_worker() {
        let md = "[TK]one[=]x[/TK] [TK]two[/TK]";
        assert!(validate(md, &[p("m"), None]).is_ok());
        assert!(validate(md, &[p("m")]).is_err(), "length");
        assert!(validate(md, &[None, p("m")]).is_err(), "no output");
        assert!(validate("[TK]x", &[]).is_err(), "malformed");
        let src = Some(ScopeProvenance {
            model: "m".into(),
            sources: vec![ProvenanceSource {
                id: "0123456789abcdefghjkmnpqrs".into(),
                version: None,
            }],
            at: None,
        });
        assert!(
            validate("[TK]one[=]x[/TK]", &[src]).is_err(),
            "unquoted source"
        );
    }
}
