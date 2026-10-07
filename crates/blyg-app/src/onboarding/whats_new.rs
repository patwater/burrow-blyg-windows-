//! What changed, per release: the first launch after an update opens the
//! tutorial on a card listing it, with Take the tour, Start at what's new,
//! and Skip tutorial. Add an entry when a release adds something the tour
//! teaches, and set its version to the release's (CHANGELOG.md says when).

/// One release's news.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Release {
    pub version: &'static str,
    /// Short lines, plain text.
    pub items: &'static [&'static str],
    /// The tour step that teaches the first new thing (`steps::STEPS` id).
    pub first_step: Option<&'static str>,
}

pub const RELEASES: &[Release] = &[
    Release {
        version: "0.9.0",
        items: &[
            "Sign in with your browser: Burrow asks your studio for access and renews it.",
            "In the Reader, ⌘-click, ⇧-click and ⌘A pick posts; r and u mark them read or \
             unread.",
            "Reply starts a stub quoting the whole post; choose a passage in the stub with \
             quote a passage instead.",
            "[[ and ![[ open a full-text picker over your posts and the ones you read.",
            "Rename a subscription, or hand it back to its own name; ⇧⌘R checks every feed.",
        ],
        first_step: Some("reader"),
    },
    Release {
        version: "0.8.0",
        items: &[
            "Lineage: a glyph beside each post's name shows which kinds of post it draws on and \
         which draw on it (fork, reply, quote), never how many.",
            "⌘J opens a post's lineage: what it draws on above, its responses below. Walk it one \
         step at a time with ⏎ and ⌫.",
            "Space on the post in the middle opens its actions on a ring: fork, reply, quote, link \
         post, versions, open.",
            "One response per post: a reply that also quotes is listed once, as a reply, and a \
         reply to a passage says so.",
        ],
        first_step: Some("lineage"),
    },
];

fn parse(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.trim().trim_start_matches('v').split('.');
    let n = |s: Option<&str>| s.and_then(|s| s.parse().ok());
    Some((n(it.next())?, n(it.next())?, n(it.next()).unwrap_or(0)))
}

/// The releases after `seen` (none seen: all) up to `current`, newest
/// first.
pub fn since(seen: Option<&str>, current: &str) -> Vec<&'static Release> {
    let Some(now) = parse(current) else {
        return vec![];
    };
    let from = seen.and_then(parse).unwrap_or((0, 0, 0));
    let mut out: Vec<&Release> = RELEASES
        .iter()
        .filter(|r| parse(r.version).is_some_and(|v| v > from && v <= now))
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(parse(r.version)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn news_is_what_shipped_since_you_last_looked() {
        assert!(since(Some("0.8.0"), "0.8.0").is_empty());
        assert!(
            since(Some("0.7.0"), "0.7.0").is_empty(),
            "not before it ships"
        );
        assert_eq!(since(Some("0.7.0"), "0.8.0")[0].version, "0.8.0");
        assert_eq!(since(None, "0.8.1").len(), 1);
        assert!(since(Some("0.8.0"), "0.8.2").is_empty());
        assert_eq!(since(Some("0.8.0"), "0.9.0")[0].version, "0.9.0");
        let both: Vec<_> = since(Some("0.7.0"), "0.9.0")
            .iter()
            .map(|r| r.version)
            .collect();
        assert_eq!(both, ["0.9.0", "0.8.0"], "newest first");
        assert!(since(Some("junk"), "junk").is_empty());
    }

    #[test]
    fn every_entry_names_a_real_step() {
        for r in RELEASES {
            assert!(parse(r.version).is_some(), "{}", r.version);
            assert!(!r.items.is_empty());
            if let Some(id) = r.first_step {
                assert!(
                    super::super::steps::STEPS.iter().any(|s| s.id == id),
                    "{id}"
                );
            }
        }
    }
}
