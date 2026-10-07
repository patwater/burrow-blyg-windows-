//! Content blocking for the browser pane: uBlock Origin's default filter
//! lists, converted to WebKit content-blocker JSON (`WKContentRuleList`).
//!
//! - Sources: uBO's defaults ([`SOURCES`]), fetched from their canonical URLs
//!   about weekly ([`REFRESH_AFTER`]) and kept in `<data dir>/browser/lists/`.
//!   Until the first download, a small hand-written list ships in the app
//!   ([`BUNDLED`]).
//! - Conversion: Brave's `adblock` crate parses ABP / uBO syntax and emits
//!   content-blocker rules. What it can't express is dropped: scriptlets
//!   (`##+js`), `$redirect`, `$removeparam`, `$csp`, procedural cosmetics
//!   (`:has-text`, `:upward`, …) and full-regex rules WebKit can't compile.
//!   Cosmetic `##` rules become `css-display-none`.
//! - Exceptions: WebKit's `ignore-previous-rules` only reaches rules earlier
//!   in the *same* list, so every chunk ends with all the exceptions
//!   ([`split`]); `#@#` element-hiding exceptions are folded into the
//!   generic rule they undo ([`fold_unhide`]).
//! - WebKit refuses a list over [`WEBKIT_MAX_RULES`] rules, so the rules are
//!   split into several lists of at most [`CHUNK_RULES`].
//!
//! Everything here is plain Rust (tests run it); compiling and attaching the
//! lists is `super::rules` (macOS only).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use adblock::content_blocking::{CbRule, CbType};
use adblock::lists::{FilterFormat, FilterSet, ParseOptions};
use sha2::{Digest, Sha256};

/// Bump when the conversion changes, so cached compiled lists are rebuilt.
pub const CONVERTER_VERSION: u32 = 1;

/// WebKit's per-list rule limit (`ContentExtensionParser.cpp`,
/// `maxRuleCount`: 150 000 since Safari 15; it was 50 000 before).
pub const WEBKIT_MAX_RULES: usize = 150_000;

/// Rules per compiled list: well under the limit, so a list compiles in a
/// few seconds and the exceptions fit alongside.
pub const CHUNK_RULES: usize = 60_000;

/// Download the lists again after this long.
pub const REFRESH_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// A filter list's syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// ABP / uBlock Origin syntax.
    Standard,
    /// A hosts file (`0.0.0.0 ads.example`).
    Hosts,
}

/// One filter list uBlock Origin enables by default.
#[derive(Debug, Clone, Copy)]
pub struct Source {
    /// File name in the lists dir (and the cache key's label).
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    pub format: Format,
}

/// uBlock Origin's default lists (uBO `assets.json`). Not included: uBO's
/// URL-tracking-parameter rules, which are all `$removeparam` (WebKit
/// content blockers can't rewrite URLs), and "Block Outsider Intrusion into
/// LAN", which WebKit already refuses for public pages.
pub const SOURCES: &[Source] = &[
    Source {
        id: "ubo-filters",
        name: "uBlock filters – Ads",
        url: "https://ublockorigin.github.io/uAssets/filters/filters.min.txt",
        format: Format::Standard,
    },
    Source {
        id: "ubo-badware",
        name: "uBlock filters – Badware risks",
        url: "https://ublockorigin.github.io/uAssets/filters/badware.min.txt",
        format: Format::Standard,
    },
    Source {
        id: "ubo-privacy",
        name: "uBlock filters – Privacy",
        url: "https://ublockorigin.github.io/uAssets/filters/privacy.min.txt",
        format: Format::Standard,
    },
    Source {
        id: "ubo-quick-fixes",
        name: "uBlock filters – Quick fixes",
        url: "https://ublockorigin.github.io/uAssets/filters/quick-fixes.min.txt",
        format: Format::Standard,
    },
    Source {
        id: "ubo-unbreak",
        name: "uBlock filters – Unbreak",
        url: "https://ublockorigin.github.io/uAssets/filters/unbreak.min.txt",
        format: Format::Standard,
    },
    Source {
        id: "easylist",
        name: "EasyList",
        url: "https://easylist.to/easylist/easylist.txt",
        format: Format::Standard,
    },
    Source {
        id: "easyprivacy",
        name: "EasyPrivacy",
        url: "https://easylist.to/easylist/easyprivacy.txt",
        format: Format::Standard,
    },
    Source {
        id: "pgl",
        name: "Peter Lowe's Ad and tracking server list",
        url: "https://pgl.yoyo.org/adservers/serverlist.php?hostformat=hosts&showintro=1&mimetype=plaintext",
        format: Format::Hosts,
    },
];

/// The fallback list shipped in the app (hand-written for Blygger, MIT like
/// the rest of the code): common ad and tracker hosts, so blocking works
/// before the first download.
pub const BUNDLED: &str = include_str!("bundled.txt");

/// What a conversion produced.
#[derive(Debug, Default)]
pub struct Converted {
    /// Blocking and hiding rules, in list order.
    pub rules: Vec<CbRule>,
    /// `ignore-previous-rules` exceptions (they go last in every chunk).
    pub exceptions: Vec<CbRule>,
    /// Filters that became rules.
    pub filters_used: usize,
    /// Rules dropped because WebKit couldn't compile them.
    pub dropped: usize,
}

/// `site#@#sel` undoes a generic `##sel` on `site`: WebKit can't cancel a
/// hiding rule from another rule's selector, so the generic rule gets
/// `~site` instead, and a bare `#@#sel` removes the generic rule. Every
/// other `#@#` line (scriptlet and procedural exceptions) is dropped.
pub fn fold_unhide(lists: &[&str]) -> Vec<String> {
    // selector → the sites it's un-hidden on (None: everywhere).
    let mut unhide: HashMap<&str, Option<BTreeSet<&str>>> = HashMap::new();
    for list in lists {
        for line in list.lines() {
            let line = line.trim();
            let Some((domains, sel)) = line.split_once("#@#") else {
                continue;
            };
            if line.starts_with('!') || sel.is_empty() || sel.starts_with('+') {
                continue;
            }
            let entry = unhide.entry(sel).or_insert_with(|| Some(BTreeSet::new()));
            if domains.is_empty() {
                *entry = None;
            } else if let Some(set) = entry {
                for d in domains.split(',') {
                    let d = d.trim();
                    // Negated or entity (`google.*`) sites can't be folded.
                    if !d.is_empty() && !d.starts_with('~') && !d.ends_with(".*") {
                        set.insert(d);
                    }
                }
            }
        }
    }
    lists
        .iter()
        .map(|list| {
            let mut out = String::with_capacity(list.len());
            for line in list.lines() {
                let t = line.trim();
                if t.contains("#@#") && !t.starts_with('!') {
                    continue;
                }
                if let Some(sel) = t.strip_prefix("##") {
                    match unhide.get(sel) {
                        Some(None) => continue,
                        Some(Some(sites)) if !sites.is_empty() => {
                            let not: Vec<String> = sites.iter().map(|s| format!("~{s}")).collect();
                            out.push_str(&not.join(","));
                            out.push_str(t);
                            out.push('\n');
                            continue;
                        }
                        _ => {}
                    }
                }
                out.push_str(line);
                out.push('\n');
            }
            out
        })
        .collect()
}

/// Does WebKit's `url-filter` parser accept this pattern? It takes a small
/// regex subset: literals, `.`, `[…]` sets, `(…)` groups, the quantifiers
/// `? * +`, `^` only at the start and `$` only at the end. No `|`, no
/// `{n}`, no class escapes like `\d`, no lookarounds, ASCII only.
pub fn webkit_url_filter_ok(pattern: &str) -> bool {
    if pattern.is_empty() || !pattern.is_ascii() {
        return false;
    }
    let b = pattern.as_bytes();
    let mut i = 0;
    let mut depth = 0i32;
    // Whether the previous token can take a quantifier.
    let mut atom = false;
    while i < b.len() {
        let c = b[i];
        match c {
            b'^' if i == 0 => atom = false,
            b'^' => return false,
            b'$' if i == b.len() - 1 => atom = false,
            b'$' => return false,
            b'|' | b'{' | b'}' => return false,
            b'\\' => {
                let Some(&n) = b.get(i + 1) else {
                    return false;
                };
                if n.is_ascii_alphanumeric() {
                    return false;
                }
                i += 1;
                atom = true;
            }
            b'(' => {
                if b.get(i + 1) == Some(&b'?') {
                    return false;
                }
                depth += 1;
                atom = false;
            }
            b')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
                atom = true;
            }
            b'[' => {
                // A set: up to the first unescaped `]` after its first char.
                let mut j = i + 1;
                if b.get(j) == Some(&b'^') {
                    j += 1;
                }
                let start = j;
                loop {
                    match b.get(j) {
                        None => return false,
                        Some(b'\\') => {
                            match b.get(j + 1) {
                                Some(n) if !n.is_ascii_alphanumeric() => {}
                                _ => return false,
                            }
                            j += 2;
                        }
                        Some(b']') if j > start => break,
                        Some(b'[') => return false,
                        Some(_) => j += 1,
                    }
                }
                i = j;
                atom = true;
            }
            b'?' | b'*' | b'+' => {
                if !atom {
                    return false;
                }
                // A quantifier can't itself be quantified (`a**`, `a+?`).
                atom = false;
            }
            c if c < 0x20 || c == 0x7f => return false,
            _ => atom = true,
        }
        i += 1;
    }
    depth == 0
}

/// WebKit reads `if-domain: ["example.com"]` as that host only; ABP means
/// the site and its subdomains, which WebKit spells `*example.com`.
fn star_domains(v: &mut Option<Vec<String>>) {
    if let Some(list) = v {
        for d in list.iter_mut() {
            if !d.starts_with('*') {
                d.insert(0, '*');
            }
        }
    }
}

/// uBO / ABP procedural pseudo-classes: not CSS, so WebKit can't use them
/// (`:has()` and `:not()` are real CSS and stay).
const PROCEDURAL: &[&str] = &[
    ":has-text(",
    ":-abp-contains(",
    ":-abp-has(",
    ":-abp-properties(",
    ":contains(",
    ":matches-attr(",
    ":matches-css(",
    ":matches-css-after(",
    ":matches-css-before(",
    ":matches-media(",
    ":matches-path(",
    ":matches-prop(",
    ":min-text-length(",
    ":nth-ancestor(",
    ":others(",
    ":remove(",
    ":remove-attr(",
    ":remove-class(",
    ":shadow(",
    ":style(",
    ":upward(",
    ":watch-attr(",
    ":xpath(",
    ":if(",
    ":if-not(",
];

/// A selector WebKit can use for `css-display-none`.
pub fn plain_selector(sel: &str) -> bool {
    let s = sel.trim();
    !s.is_empty() && s.is_ascii() && !PROCEDURAL.iter().any(|p| s.contains(p))
}

fn webkit_ok(rule: &CbRule) -> bool {
    let t = &rule.trigger;
    let domains_ok = [&t.if_domain, &t.unless_domain]
        .into_iter()
        .flatten()
        .flatten()
        .all(|d| !d.is_empty() && d.is_ascii() && *d == d.to_ascii_lowercase());
    let selector_ok = match rule.action.typ {
        CbType::CssDisplayNone => rule.action.selector.as_deref().is_some_and(plain_selector),
        _ => true,
    };
    domains_ok && selector_ok && webkit_url_filter_ok(&t.url_filter)
}

/// Convert filter lists (ABP / uBO / hosts syntax) to content-blocker rules.
pub fn convert(lists: &[(Format, &str)]) -> Converted {
    let standard: Vec<&str> = lists
        .iter()
        .filter(|(f, _)| *f == Format::Standard)
        .map(|(_, t)| *t)
        .collect();
    let mut folded = fold_unhide(&standard).into_iter();
    let mut set = FilterSet::new(true);
    for (format, text) in lists {
        let (text, format) = match format {
            Format::Standard => (folded.next().unwrap_or_default(), FilterFormat::Standard),
            Format::Hosts => (text.to_string(), FilterFormat::Hosts),
        };
        set.add_filter_list(
            text,
            ParseOptions {
                format,
                ..ParseOptions::default()
            },
        );
    }
    let Ok((rules, used)) = set.into_content_blocking() else {
        return Converted::default();
    };
    let mut out = Converted {
        filters_used: used.len(),
        ..Converted::default()
    };
    let mut seen = std::collections::HashSet::new();
    for mut rule in rules {
        if rule.action.typ == CbType::CssDisplayNone {
            star_domains(&mut rule.trigger.if_domain);
            star_domains(&mut rule.trigger.unless_domain);
        }
        if !webkit_ok(&rule) {
            out.dropped += 1;
            continue;
        }
        // Lists overlap (EasyList and uBO share many rules).
        let key = serde_json::to_string(&rule).unwrap_or_default();
        if !seen.insert(key) {
            continue;
        }
        if rule.action.typ == CbType::IgnorePreviousRules {
            out.exceptions.push(rule);
        } else {
            out.rules.push(rule);
        }
    }
    out
}

/// Split into lists WebKit accepts: each is a run of rules followed by
/// every exception (an exception only reaches earlier rules in its own
/// list). Never returns an empty list; `chunk` is the rule budget per list.
pub fn split(conv: &Converted, chunk: usize) -> Vec<Vec<CbRule>> {
    let chunk = chunk.min(WEBKIT_MAX_RULES);
    let room = chunk.saturating_sub(conv.exceptions.len()).max(1000);
    let mut out: Vec<Vec<CbRule>> = conv
        .rules
        .chunks(room)
        .map(|c| {
            let mut v = c.to_vec();
            v.extend(conv.exceptions.iter().cloned());
            v
        })
        .collect();
    if out.is_empty() {
        // Exceptions alone do nothing; an empty list is still a valid list.
        out.push(Vec::new());
    }
    out
}

/// The encoded JSON for one list.
pub fn encode(rules: &[CbRule]) -> String {
    serde_json::to_string(rules).unwrap_or_else(|_| "[]".into())
}

/// The cache key for a set of list texts: a compiled list is reused while
/// the sources (and the converter) are unchanged.
pub fn content_key(lists: &[(Format, &str)]) -> String {
    let mut h = Sha256::new();
    h.update(CONVERTER_VERSION.to_le_bytes());
    h.update(CHUNK_RULES.to_le_bytes());
    for (f, t) in lists {
        h.update([*f as u8]);
        h.update((t.len() as u64).to_le_bytes());
        h.update(t.as_bytes());
    }
    let d = h.finalize();
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// A compiled list's identifier in the rule-list store.
pub fn identifier(key: &str, index: usize) -> String {
    format!("blyg-{key}-{index}")
}

// ================================================================ conversion run

/// What [`convert_to_dir`] did (printed as JSON by `blygger
/// +convert-blocklists`, which the app runs as a child process so the
/// conversion's few hundred MB are returned to the system when it exits).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ConvertReport {
    /// The sources' content key.
    pub key: String,
    /// The sources match `unless_key`: nothing was written.
    pub unchanged: bool,
    /// `<out_dir>/<i>.json` for i in 0..lists.
    pub lists: usize,
    pub rules: usize,
    pub filters_used: usize,
    pub dropped: usize,
    pub convert_ms: u64,
}

/// A list file [`convert_to_dir`] wrote.
pub fn list_file(out_dir: &Path, index: usize) -> PathBuf {
    out_dir.join(format!("{index}.json"))
}

/// Convert the lists on disk (or the bundled one) into content-blocker JSON
/// files in `out_dir`, unless their key is `unless_key`.
pub fn convert_to_dir(
    data_dir: &Path,
    out_dir: &Path,
    unless_key: Option<&str>,
) -> std::io::Result<ConvertReport> {
    let t0 = std::time::Instant::now();
    let sources = load_sources(data_dir);
    let refs: Vec<(Format, &str)> = sources.iter().map(|(f, t)| (*f, t.as_str())).collect();
    let key = content_key(&refs);
    if unless_key == Some(key.as_str()) {
        return Ok(ConvertReport {
            key,
            unchanged: true,
            ..ConvertReport::default()
        });
    }
    let conv = convert(&refs);
    let lists = split(&conv, CHUNK_RULES);
    std::fs::create_dir_all(out_dir)?;
    for (i, l) in lists.iter().enumerate() {
        std::fs::write(list_file(out_dir, i), encode(l))?;
    }
    Ok(ConvertReport {
        key,
        unchanged: false,
        lists: lists.len(),
        rules: lists.iter().map(Vec::len).sum(),
        filters_used: conv.filters_used,
        dropped: conv.dropped,
        convert_ms: t0.elapsed().as_millis() as u64,
    })
}

// ================================================================ on disk

/// `<data dir>/browser/`: the downloaded lists, the manifest, and WebKit's
/// compiled rule lists (`compiled/`).
pub fn browser_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("browser")
}

pub fn lists_dir(data_dir: &Path) -> PathBuf {
    browser_dir(data_dir).join("lists")
}

#[cfg_attr(test, allow(dead_code))]
pub fn compiled_dir(data_dir: &Path) -> PathBuf {
    browser_dir(data_dir).join("compiled")
}

/// What's compiled and when the lists were fetched.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Manifest {
    /// Unix seconds of the last successful download.
    pub fetched_at: Option<u64>,
    /// The content key of the compiled lists, and their identifiers in the
    /// rule-list store.
    pub key: Option<String>,
    pub ids: Vec<String>,
    /// Rules in all lists (for Settings and the log).
    pub rules: usize,
}

impl Manifest {
    fn path(data_dir: &Path) -> PathBuf {
        browser_dir(data_dir).join("manifest.json")
    }

    pub fn load(data_dir: &Path) -> Manifest {
        std::fs::read_to_string(Self::path(data_dir))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(browser_dir(data_dir))?;
        let s = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        blyg_core::config::edit::atomic_write(&Self::path(data_dir), &s)
    }
}

/// Whether the downloaded lists are due for a refresh.
pub fn due(fetched_at: Option<u64>, now: SystemTime) -> bool {
    let now = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match fetched_at {
        None => true,
        Some(t) => now.saturating_sub(t) >= REFRESH_AFTER.as_secs() || t > now + 86_400,
    }
}

/// The lists to convert: every downloaded source that's on disk, or the
/// bundled list when none is (plus the bundled list is always first, so
/// its hosts are blocked even if a download came back partial).
pub fn load_sources(data_dir: &Path) -> Vec<(Format, String)> {
    let dir = lists_dir(data_dir);
    let mut out = vec![(Format::Hosts, BUNDLED.to_string())];
    for s in SOURCES {
        if let Ok(t) = std::fs::read_to_string(dir.join(format!("{}.txt", s.id))) {
            out.push((s.format, t));
        }
    }
    out
}

/// Download every source (a background thread; never in tests). A list that
/// fails keeps its previous copy. Returns how many were fetched.
pub fn download_all(data_dir: &Path) -> usize {
    let dir = lists_dir(data_dir);
    if std::fs::create_dir_all(&dir).is_err() {
        return 0;
    }
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("Burrow/", env!("CARGO_PKG_VERSION")))
        .build();
    let mut ok = 0;
    for s in SOURCES {
        let text = match agent.get(s.url).call().map(|r| r.into_string()) {
            Ok(Ok(t)) => t,
            _ => {
                eprintln!("blygger: couldn't download the {} list", s.name);
                continue;
            }
        };
        // A captive portal or an error page isn't a filter list.
        if !plausible_list(&text, s.format) {
            eprintln!("blygger: the {} download isn't a filter list", s.name);
            continue;
        }
        let path = dir.join(format!("{}.txt", s.id));
        if blyg_core::config::edit::atomic_write(&path, &text).is_ok() {
            ok += 1;
        }
    }
    ok
}

/// A downloaded body looks like a filter list (not an HTML error page).
pub fn plausible_list(text: &str, format: Format) -> bool {
    let head = text.trim_start();
    if head.len() < 200 || head.starts_with('<') {
        return false;
    }
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    match format {
        Format::Standard => lines >= 50,
        Format::Hosts => lines >= 50 && text.contains('.'),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(list: &str) -> Converted {
        convert(&[(Format::Standard, list)])
    }

    #[test]
    fn network_rules_become_block_rules() {
        let c = conv("||ads.example.net^\n||track.example.org^$third-party\n");
        assert_eq!(c.rules.len(), 2, "{:?}", c.rules);
        assert!(c.rules.iter().all(|r| r.action.typ == CbType::Block));
        let json = encode(&c.rules);
        assert!(json.contains(r#""type":"block""#), "{json}");
        assert!(json.contains("ads\\\\.example\\\\.net"), "{json}");
        assert!(json.contains(r#""load-type":["third-party"]"#), "{json}");
        // The first-party document exception comes with any network rule.
        assert!(
            c.exceptions
                .iter()
                .any(|r| r.action.typ == CbType::IgnorePreviousRules)
        );
    }

    #[test]
    fn cosmetic_rules_become_css_display_none_for_the_site_and_subdomains() {
        let c = conv("##.ad-banner\nexample.com##.sponsored\n");
        let css: Vec<&CbRule> = c
            .rules
            .iter()
            .filter(|r| r.action.typ == CbType::CssDisplayNone)
            .collect();
        assert_eq!(css.len(), 2);
        let site = css
            .iter()
            .find(|r| r.action.selector.as_deref() == Some(".sponsored"))
            .unwrap();
        assert_eq!(site.trigger.if_domain, Some(vec!["*example.com".into()]));
    }

    #[test]
    fn exceptions_are_kept_and_ordered_last() {
        let c = conv("||ads.example.net^\n@@||ads.example.net/ok.js\n");
        assert_eq!(c.rules.len(), 1);
        assert!(c.exceptions.len() >= 2, "{:?}", c.exceptions);
        let lists = split(&c, 100_000);
        assert_eq!(lists.len(), 1);
        let l = &lists[0];
        assert_eq!(l[0].action.typ, CbType::Block);
        assert!(
            l[1..]
                .iter()
                .all(|r| r.action.typ == CbType::IgnorePreviousRules)
        );
    }

    #[test]
    fn unconvertible_filters_are_skipped() {
        // Scriptlets, redirects, removeparam and procedural cosmetics.
        let c = conv(
            "example.com##+js(nowebrtc)\n\
             ||ads.example.net/a.js$script,redirect=noopjs\n\
             ||example.com^$removeparam=utm_source\n\
             example.com##.x:has-text(Sponsored)\n\
             /banner[0-9]{3}/\n",
        );
        assert!(
            c.rules.is_empty(),
            "{:?}",
            c.rules
                .iter()
                .map(|r| serde_json::to_string(r).unwrap())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn procedural_selectors_are_not_css() {
        assert!(plain_selector(".ad-banner"));
        assert!(plain_selector("div:has(> .sponsored)"));
        assert!(plain_selector("a[href^=\"https://ads.example.net\"]"));
        assert!(!plain_selector(".x:has-text(Sponsored)"));
        assert!(!plain_selector("div:upward(2)"));
        assert!(!plain_selector(".x:style(display: none !important)"));
        assert!(!plain_selector(""));
    }

    #[test]
    fn hosts_lists_block_the_host() {
        let c = convert(&[(
            Format::Hosts,
            "# comment\n0.0.0.0 ads.example.net\n127.0.0.1 pixel.example.org\n",
        )]);
        assert_eq!(c.rules.len(), 2);
        assert!(c.rules.iter().all(|r| r.action.typ == CbType::Block));
    }

    #[test]
    fn unhide_is_folded_into_the_generic_rule() {
        let lists = fold_unhide(&[
            "##.ad\n##.promo\n##.gone\n",
            "example.com,shop.example.org#@#.ad\n#@#.gone\nexample.com#@#+js(x)\n",
        ]);
        assert_eq!(lists[0], "~example.com,~shop.example.org##.ad\n##.promo\n");
        assert_eq!(lists[1], "");
        let c = conv("##.ad\nexample.com#@#.ad\n");
        assert_eq!(c.rules.len(), 1);
        assert_eq!(
            c.rules[0].trigger.unless_domain,
            Some(vec!["*example.com".into()])
        );
    }

    #[test]
    fn duplicates_across_lists_are_dropped() {
        let c = convert(&[
            (Format::Standard, "||ads.example.net^\n"),
            (Format::Standard, "||ads.example.net^\n"),
        ]);
        assert_eq!(c.rules.len(), 1);
    }

    #[test]
    fn url_filters_webkit_can_parse() {
        for ok in [
            "^[^:]+:(//)?([^/]+\\.)?ads\\.example\\.net",
            ".*",
            "^https?://",
            "banner\\.gif$",
            "/ad[sx]?/.*\\.js",
            "[a-z]+\\-ad",
        ] {
            assert!(webkit_url_filter_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "a|b",
            "ad{2}",
            "\\d+",
            "(?=x)",
            "a^b",
            "a$b",
            "(unclosed",
            "closed)",
            "**",
            "a+*",
            "[abc",
            "caf\u{e9}",
        ] {
            assert!(!webkit_url_filter_ok(bad), "{bad}");
        }
    }

    #[test]
    fn lists_split_under_the_limit_with_every_exception() {
        let mut text = String::new();
        for i in 0..2500 {
            text.push_str(&format!("||ads{i}.example.net^\n"));
        }
        text.push_str("@@||ads1.example.net/ok^\n");
        let c = conv(&text);
        assert_eq!(c.rules.len(), 2500);
        let ex = c.exceptions.len();
        let lists = split(&c, 1000 + ex);
        assert_eq!(lists.len(), 3);
        for l in &lists {
            assert!(l.len() <= 1000 + ex);
            let n = l.len();
            assert!(
                l[n - ex..]
                    .iter()
                    .all(|r| r.action.typ == CbType::IgnorePreviousRules)
            );
        }
        assert_eq!(lists.iter().map(|l| l.len() - ex).sum::<usize>(), 2500);
        assert_eq!(split(&Converted::default(), 10).len(), 1);
        const { assert!(CHUNK_RULES < WEBKIT_MAX_RULES) };
    }

    #[test]
    fn the_bundled_list_converts() {
        let c = convert(&[(Format::Hosts, BUNDLED)]);
        assert!(c.rules.len() >= 200, "{}", c.rules.len());
        assert_eq!(c.dropped, 0);
        let hosts = BUNDLED
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .count();
        assert_eq!(c.rules.len(), hosts, "one rule per host, no duplicates");
    }

    #[test]
    fn cache_keys_follow_content() {
        let a = content_key(&[(Format::Hosts, "a.example")]);
        assert_eq!(a, content_key(&[(Format::Hosts, "a.example")]));
        assert_ne!(a, content_key(&[(Format::Hosts, "b.example")]));
        assert_ne!(a, content_key(&[(Format::Standard, "a.example")]));
        assert_eq!(a.len(), 16);
        assert_eq!(identifier(&a, 2), format!("blyg-{a}-2"));
    }

    #[test]
    fn refresh_is_weekly() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000_000);
        assert!(due(None, now));
        assert!(!due(Some(10_000_000 - 3600), now));
        assert!(due(Some(10_000_000 - 8 * 86_400), now));
        // A clock that went backwards a lot: fetch again.
        assert!(due(Some(10_000_000 + 3 * 86_400), now));
    }

    #[test]
    fn error_pages_are_not_lists() {
        assert!(!plausible_list(
            "<!doctype html><html>…</html>",
            Format::Standard
        ));
        assert!(!plausible_list("", Format::Hosts));
        let hosts: String = (0..60).map(|i| format!("0.0.0.0 a{i}.example\n")).collect();
        assert!(plausible_list(&hosts, Format::Hosts));
    }

    #[test]
    fn conversion_runs_write_one_file_per_list() {
        let data = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let r = convert_to_dir(data.path(), out.path(), None).unwrap();
        assert!(!r.unchanged);
        assert_eq!(r.lists, 1);
        let json = std::fs::read_to_string(list_file(out.path(), 0)).unwrap();
        let rules: Vec<CbRule> = serde_json::from_str(&json).unwrap();
        assert_eq!(rules.len(), r.rules);
        assert!(json.contains("doubleclick\\\\.net"), "the bundled list");
        // Same sources, same key: nothing to do.
        let again = convert_to_dir(data.path(), out.path(), Some(&r.key)).unwrap();
        assert!(again.unchanged);
        assert_eq!(again.key, r.key);
        // A downloaded list changes the key.
        std::fs::create_dir_all(lists_dir(data.path())).unwrap();
        std::fs::write(
            lists_dir(data.path()).join("easylist.txt"),
            "||ads.example.net^\n",
        )
        .unwrap();
        let more = convert_to_dir(data.path(), out.path(), Some(&r.key)).unwrap();
        assert!(!more.unchanged);
        assert_ne!(more.key, r.key);
        assert!(more.rules > r.rules);
    }

    #[test]
    fn manifest_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Manifest::load(dir.path()), Manifest::default());
        let m = Manifest {
            fetched_at: Some(5),
            key: Some("abcd".into()),
            ids: vec![identifier("abcd", 0), identifier("abcd", 1)],
            rules: 10,
        };
        m.save(dir.path()).unwrap();
        assert_eq!(Manifest::load(dir.path()), m);
        // Only the bundled list until something is downloaded.
        assert_eq!(load_sources(dir.path()).len(), 1);
    }

    /// Converts the real lists (downloaded, so `--ignored`): rule counts and
    /// time, for the report. `BLYGGER_LISTS_DIR` can point at a folder of
    /// already-downloaded `<id>.txt` files instead.
    #[test]
    #[ignore]
    fn real_lists_convert_under_the_limit() {
        let dir = match std::env::var_os("BLYGGER_LISTS_DIR") {
            Some(d) => PathBuf::from(d),
            None => {
                let t = tempfile::tempdir().unwrap().keep();
                assert!(download_all(&t) > 0, "no network?");
                lists_dir(&t)
            }
        };
        let mut texts = vec![(Format::Hosts, BUNDLED.to_string())];
        for s in SOURCES {
            if let Ok(t) = std::fs::read_to_string(dir.join(format!("{}.txt", s.id))) {
                texts.push((s.format, t));
            }
        }
        let refs: Vec<(Format, &str)> = texts.iter().map(|(f, t)| (*f, t.as_str())).collect();
        let t0 = std::time::Instant::now();
        let c = convert(&refs);
        let lists = split(&c, CHUNK_RULES);
        let bytes: usize = lists.iter().map(|l| encode(l).len()).sum();
        println!(
            "sources={} filters_used={} rules={} exceptions={} dropped={} lists={} json_bytes={} convert_ms={}",
            texts.len(),
            c.filters_used,
            c.rules.len(),
            c.exceptions.len(),
            c.dropped,
            lists.len(),
            bytes,
            t0.elapsed().as_millis()
        );
        for l in &lists {
            assert!(l.len() <= WEBKIT_MAX_RULES);
        }
    }
}
