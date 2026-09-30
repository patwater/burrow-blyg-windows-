//! A port of linkify-it 5 (the link detector markdown-it's `linkify: true`
//! uses) with its default options: `fuzzyLink: true`, `fuzzyEmail: true`,
//! `fuzzyIP: false`, and the default schemas `http:`, `https:`, `ftp:`, `//`
//! and `mailto:`.
//!
//! The regular expressions are linkify-it's `lib/re.mjs`, translated to
//! `fancy-regex` syntax. uc.micro's `Z`, `P` and `Cc` become `\p{Z}`, `\p{P}`
//! and `\p{Cc}`, and negative look-aheads over a single character were folded
//! into negated character classes (same language, much faster). Offsets are
//! byte offsets into the searched `&str`, where linkify-it's are UTF-16 code
//! units; only relative positions are ever used, so results are identical.

use fancy_regex::Regex;
use std::sync::OnceLock;

/// One detected link, as linkify-it's `Match` after `normalize()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Lower-cased schema: `""` for a fuzzy link, `"mailto:"` for a fuzzy email.
    pub schema: String,
    pub index: usize,
    pub last_index: usize,
    /// The matched text, verbatim.
    pub text: String,
    /// The link target: `text`, with `http://` or `mailto:` added when missing.
    pub url: String,
}

struct Re {
    schema_test: Regex,
    schema_search: Regex,
    schema_at_start: Regex,
    pretest: Regex,
    host_fuzzy_test: Regex,
    link_no_ip_fuzzy: Regex,
    email_fuzzy: Regex,
    http: Regex,
    no_http: Regex,
    mailto: Regex,
}

const TLDS_2CH: &str = "a[cdefgilmnoqrstuwxz]|b[abdefghijmnorstvwyz]|c[acdfghiklmnoruvwxyz]|d[ejkmoz]|e[cegrstu]|f[ijkmor]|g[abdefghilmnpqrstuwy]|h[kmnrtu]|i[delmnoqrst]|j[emop]|k[eghimnprwyz]|l[abcikrstuvy]|m[acdeghklmnopqrstuvwxyz]|n[acefgilopruz]|om|p[aefghklmnrstwy]|qa|r[eosuw]|s[abcdeghijklmnortuvxyz]|t[cdfghjklmnortvwz]|u[agksyz]|v[aceginu]|w[fs]|y[et]|z[amw]";
const TLDS_DEFAULT: &str =
    "biz|com|edu|gov|net|org|pro|web|xxx|aero|asia|coop|info|museum|name|shop|рф";

fn re() -> &'static Re {
    static RE: OnceLock<Re> = OnceLock::new();
    RE.get_or_init(build)
}

fn build() -> Re {
    let zpcc = r"\p{Z}|\p{P}|\p{Cc}";
    let zcc = r"\p{Z}|\p{Cc}";
    let sep = r"[><\x{ff5c}]";
    // (?:(?!text_separators|ZPCc)Any)
    let pseudo_letter = r"[^\p{Z}\p{P}\p{Cc}><\x{ff5c}]";
    let ip4 =
        r"(?:(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)";
    // (?:(?:(?!ZCc|[@/\[\]()]).){1,50}@)?
    let auth = r"(?:[^\p{Z}\p{Cc}@/\[\]()]{1,50}@)?";
    let port = r"(?::(?:6(?:[0-4]\d{3}|5(?:[0-4]\d{2}|5(?:[0-2]\d|3[0-5])))|[1-5]?\d{1,4}))?";
    let host_terminator = format!(r"(?=$|{sep}|{zpcc})(?!-|_|:\d|\.-|\.(?!$|{zpcc}))");
    let path = format!(
        concat!(
            r"(?:[/?#](?:",
            r#"[^\p{{Z}}\p{{Cc}}><\x{{ff5c}}()\[\]{{}}.,"'?!\-;]|"#,
            r"\[[^\p{{Z}}\p{{Cc}}\]]*\]|",
            r"\([^\p{{Z}}\p{{Cc}})]*\)|",
            r"\{{[^\p{{Z}}\p{{Cc}}}}]*\}}|",
            r#""[^\p{{Z}}\p{{Cc}}"]+"|"#,
            r"'[^\p{{Z}}\p{{Cc}}']+'|",
            r"'(?={pl}|[-])|",
            r"\.{{2,}}[a-zA-Z0-9%/&]|",
            r"\.(?!{zcc}|[.]|$)|",
            r"\-+|",
            r",(?!{zcc}|$)|",
            r";(?!{zcc}|$)|",
            r"\!+(?!{zcc}|[!]|$)|",
            r"\?(?!{zcc}|[?]|$)",
            r")+|\/)?"
        ),
        pl = pseudo_letter,
        zcc = zcc
    );
    let email_name = r#"[\-;:&=\+\$,\.a-zA-Z0-9_][\-;:&=\+\$,"\.a-zA-Z0-9_]{0,63}"#;
    let xn = r"xn--[a-z0-9\-]{1,59}";
    let domain_root = format!("(?:{xn}|{pseudo_letter}{{1,63}})");
    let domain = format!(
        "(?:{xn}|(?:{pl})|(?:{pl}(?:-|{pl}){{0,61}}{pl}))",
        pl = pseudo_letter
    );
    let host = format!(r"(?:(?:(?:{domain})\.)*{domain})");
    let tlds = format!("{TLDS_DEFAULT}|{TLDS_2CH}|{xn}");
    let host_fuzzy = format!(r"(?:{ip4}|(?:(?:(?:{domain})\.)+(?:{tlds})))");
    let host_no_ip_fuzzy = format!(r"(?:(?:(?:{domain})\.)+(?:{tlds}))");
    let host_strict = format!("{host}{host_terminator}");
    let host_fuzzy_strict = format!("{host_fuzzy}{host_terminator}");
    let host_port_strict = format!("{host}{port}{host_terminator}");
    let host_port_no_ip_fuzzy_strict = format!("{host_no_ip_fuzzy}{port}{host_terminator}");
    let host_fuzzy_test = format!(r"localhost|www\.|\.\d{{1,3}}\.|(?:\.(?:{tlds})(?:{zpcc}|>|$))");
    let email_fuzzy = format!(r#"(^|{sep}|"|\(|{zcc})({email_name}@{host_fuzzy_strict})"#);
    let link_no_ip_fuzzy = format!(
        r"(^|(?![.:/\-_@])(?:[$+<=>^`|\x{{ff5c}}]|{zpcc}))((?![$+<=>^`|\x{{ff5c}}]){host_port_no_ip_fuzzy_strict}{path})"
    );
    let schemas = "http:|https:|ftp:|//|mailto:";
    let schema_search = format!(r"(^|(?!_)(?:[><\x{{ff5c}}]|{zpcc}))({schemas})");

    let ci = |s: &str| Regex::new(&format!("(?i){s}")).expect("linkify regex");
    Re {
        schema_test: ci(&schema_search),
        schema_at_start: ci(&format!("^{schema_search}")),
        pretest: ci(&format!("({schema_search})|({host_fuzzy_test})|@")),
        schema_search: ci(&schema_search),
        host_fuzzy_test: ci(&host_fuzzy_test),
        link_no_ip_fuzzy: ci(&link_no_ip_fuzzy),
        email_fuzzy: ci(&email_fuzzy),
        http: ci(&format!(r"^//{auth}{host_port_strict}{path}")),
        no_http: ci(&format!(
            r"^{auth}(?:localhost|(?:(?:{domain})\.)+{domain_root}){port}{host_terminator}{path}"
        )),
        mailto: ci(&format!("^{email_name}@{host_strict}")),
    }
}

fn is_match(re: &Regex, s: &str) -> bool {
    re.is_match(s).unwrap_or(false)
}

fn match_len(re: &Regex, s: &str) -> usize {
    match re.find(s) {
        Ok(Some(m)) => m.end() - m.start(),
        _ => 0,
    }
}

/// `testSchemaAt`: the length of a valid link after the schema at `pos`, or 0.
fn test_schema_at(text: &str, schema: &str, pos: usize) -> usize {
    let r = re();
    let tail = &text[pos..];
    match schema.to_ascii_lowercase().as_str() {
        "http:" | "https:" | "ftp:" => match_len(&r.http, tail),
        "//" => {
            let len = match_len(&r.no_http, tail);
            if len > 0 && pos >= 3 {
                let b = text.as_bytes()[pos - 3];
                if b == b':' || b == b'/' {
                    return 0;
                }
            }
            len
        }
        "mailto:" => match_len(&r.mailto, tail),
        _ => 0,
    }
}

/// `LinkifyIt#pretest`: a quick, permissive check.
pub fn pretest(text: &str) -> bool {
    is_match(&re().pretest, text)
}

/// `LinkifyIt#test`.
pub fn test(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let r = re();
    if is_match(&r.schema_test, text) {
        let mut pos = 0;
        while pos <= text.len() {
            let Ok(Some(c)) = r.schema_search.captures_from_pos(text, pos) else {
                break;
            };
            let whole = c.get(0).expect("group 0");
            let end = whole.end();
            if test_schema_at(text, c.get(2).map_or("", |m| m.as_str()), end) > 0 {
                return true;
            }
            pos = if end > whole.start() { end } else { end + 1 };
        }
    }
    if is_match(&r.host_fuzzy_test, text) && is_match(&r.link_no_ip_fuzzy, text) {
        return true;
    }
    if text.contains('@') && is_match(&r.email_fuzzy, text) {
        return true;
    }
    false
}

struct Candidate {
    schema: String,
    index: usize,
    last_index: usize,
}

fn global(re: &Regex, text: &str, mut f: impl FnMut(&fancy_regex::Captures<'_, str>)) {
    let mut pos = 0;
    while pos <= text.len() {
        let Ok(Some(c)) = re.captures_from_pos(text, pos) else {
            break;
        };
        let whole = c.get(0).expect("group 0");
        f(&c);
        pos = if whole.end() > whole.start() {
            whole.end()
        } else {
            whole.end() + 1
        };
        while pos < text.len() && !text.is_char_boundary(pos) {
            pos += 1;
        }
    }
}

fn group_len(c: &fancy_regex::Captures<'_, str>, i: usize) -> usize {
    c.get(i).map_or(0, |m| m.end() - m.start())
}

fn make_match(text: &str, schema: &str, index: usize, last_index: usize) -> Match {
    let raw = &text[index..last_index];
    let schema = schema.to_lowercase();
    let mut url = raw.to_string();
    // LinkifyIt#normalize
    if schema.is_empty() {
        url = format!("http://{url}");
    }
    if schema == "mailto:" && !url.to_ascii_lowercase().starts_with("mailto:") {
        url = format!("mailto:{url}");
    }
    Match {
        schema,
        index,
        last_index,
        text: raw.to_string(),
        url,
    }
}

/// `LinkifyIt#match`: every link in `text`, left to right, non-overlapping.
pub fn match_links(text: &str) -> Vec<Match> {
    if text.is_empty() {
        return Vec::new();
    }
    let r = re();
    let mut schemed = Vec::new();
    let mut fuzzy_link = Vec::new();
    let mut fuzzy_email = Vec::new();

    if is_match(&r.schema_test, text) {
        global(&r.schema_search, text, |c| {
            let whole = c.get(0).expect("group 0");
            let schema = c.get(2).map_or("", |m| m.as_str());
            let len = test_schema_at(text, schema, whole.end());
            if len > 0 {
                schemed.push(Candidate {
                    schema: schema.to_string(),
                    index: whole.start() + group_len(c, 1),
                    last_index: whole.end() + len,
                });
            }
        });
    }
    global(&r.link_no_ip_fuzzy, text, |c| {
        let whole = c.get(0).expect("group 0");
        fuzzy_link.push(Candidate {
            schema: String::new(),
            index: whole.start() + group_len(c, 1),
            last_index: whole.end(),
        });
    });
    global(&r.email_fuzzy, text, |c| {
        let whole = c.get(0).expect("group 0");
        fuzzy_email.push(Candidate {
            schema: "mailto:".to_string(),
            index: whole.start() + group_len(c, 1),
            last_index: whole.end(),
        });
    });

    fn choose<'a>(a: Option<&'a Candidate>, b: Option<&'a Candidate>) -> Option<&'a Candidate> {
        match (a, b) {
            (None, b) => b,
            (a, None) => a,
            (Some(x), Some(y)) => {
                if x.index != y.index {
                    Some(if x.index < y.index { x } else { y })
                } else {
                    Some(if x.last_index >= y.last_index { x } else { y })
                }
            }
        }
    }

    let mut idx = [0usize; 3];
    let mut last = 0usize;
    let mut result = Vec::new();
    loop {
        let c0 = schemed.get(idx[0]);
        let c1 = fuzzy_email.get(idx[1]);
        let c2 = fuzzy_link.get(idx[2]);
        let Some(cand) = choose(choose(c0, c1), c2) else {
            break;
        };
        if c0.is_some_and(|c| std::ptr::eq(c, cand)) {
            idx[0] += 1;
        } else if c1.is_some_and(|c| std::ptr::eq(c, cand)) {
            idx[1] += 1;
        } else {
            idx[2] += 1;
        }
        if cand.index < last {
            continue;
        }
        result.push(make_match(text, &cand.schema, cand.index, cand.last_index));
        last = cand.last_index;
    }
    result
}

/// `LinkifyIt#matchAtStart`: a schema link starting exactly at offset 0.
pub fn match_at_start(text: &str) -> Option<Match> {
    if text.is_empty() {
        return None;
    }
    let r = re();
    let c = r.schema_at_start.captures(text).ok()??;
    let whole = c.get(0)?;
    let schema = c.get(2)?.as_str();
    let len = test_schema_at(text, schema, whole.end());
    if len == 0 {
        return None;
    }
    Some(make_match(
        text,
        schema,
        whole.start() + group_len(&c, 1),
        whole.end() + len,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(s: &str) -> Vec<String> {
        match_links(s).into_iter().map(|m| m.url).collect()
    }

    #[test]
    fn schema_and_fuzzy() {
        assert_eq!(
            urls("see https://example.org/a. ok"),
            ["https://example.org/a"]
        );
        assert_eq!(
            urls("www.example.org and example.com"),
            ["http://www.example.org", "http://example.com"]
        );
        assert_eq!(urls("mail a@example.com"), ["mailto:a@example.com"]);
        assert!(urls("file.txt v1.2.3").is_empty());
        assert!(test("x https://example.org"));
        assert!(!test("nothing here"));
        assert_eq!(
            match_at_start("https://example.org/x y").unwrap().url,
            "https://example.org/x"
        );
    }
}
