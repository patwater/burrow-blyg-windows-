//! The Worker's allowlist sanitizer (`importer/sanitize.ts`, studio 0.32),
//! which its thread preview runs over the walked HTML (`previewTransclusions`)
//! and its public pages run over a thread's baked HTML. It is built on
//! HTMLRewriter, which is lol-html; this uses the same crate, so the output
//! is byte-identical: an untouched element keeps its source bytes, and a
//! removed attribute takes only its own bytes with it.
//!
//! - Listed tags keep listed attributes, `data-blyg-*`, an `<a href>` that is
//!   http(s), mailto or tel, and an `<img src>` that is http(s).
//! - Active, foreign, raw-text and RCDATA elements go with their content.
//! - Any other element is unwrapped: its text and listed children stay.
//!
//! One addition: `data-line`, the desktop preview's own source-line
//! attribute, is kept (the Worker's preview has none).

use lol_html::{RewriteStrSettings, element, rewrite_str};

const TAGS: &[&str] = &[
    "a",
    "abbr",
    "address",
    "article",
    "aside",
    "b",
    "bdi",
    "bdo",
    "blockquote",
    "br",
    "caption",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "details",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "main",
    "mark",
    "nav",
    "ol",
    "p",
    "pre",
    "q",
    "rp",
    "rt",
    "ruby",
    "s",
    "samp",
    "section",
    "small",
    "span",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "th",
    "thead",
    "time",
    "tr",
    "u",
    "ul",
    "var",
    "wbr",
];

const DROP: &[&str] = &[
    "script",
    "style",
    "template",
    "noscript",
    "textarea",
    "title",
    "xmp",
    "plaintext",
    "listing",
    "noembed",
    "noframes",
    "iframe",
    "frame",
    "frameset",
    "object",
    "embed",
    "applet",
    "svg",
    "math",
    "select",
    "head",
    "base",
    "link",
    "meta",
    "form",
];

const ATTRIBUTES: &[&str] = &[
    "alt", "class", "title", "width", "height", "colspan", "rowspan", "scope", "datetime", "open",
    "dir", "lang",
];

/// `safeUrl`: decode the attribute's entities, then resolve it as WHATWG
/// URL does against a placeholder base, and check the scheme.
fn safe_url(raw: &str, image: bool) -> bool {
    let decoded = html_escape::decode_html_entities(raw);
    let Ok(base) = url::Url::parse("https://import.invalid/") else {
        return false;
    };
    match base.join(&decoded) {
        Ok(u) => {
            let ok: &[&str] = if image {
                &["https", "http"]
            } else {
                &["https", "http", "mailto", "tel"]
            };
            ok.contains(&u.scheme())
        }
        Err(_) => false,
    }
}

/// `sanitizeHtml`.
pub(crate) fn sanitize_html(html: &str) -> String {
    // workerd treats `<esi:include>` as an ordinary tag (no compat flag).
    let settings = RewriteStrSettings::new()
        .with_enable_esi_tags(false)
        .append_element_content_handler(element!("*", |el| {
            let tag = el.tag_name();
            if DROP.contains(&tag.as_str()) {
                el.remove();
                return Ok(());
            }
            if !TAGS.contains(&tag.as_str()) {
                el.remove_and_keep_content();
                return Ok(());
            }
            let attrs: Vec<(String, String)> = el
                .attributes()
                .iter()
                .map(|a| (a.name(), a.value()))
                .collect();
            for (name, value) in attrs {
                let key = name.to_ascii_lowercase();
                let keep = if key == "href" && tag == "a" {
                    safe_url(&value, false)
                } else if key == "src" && tag == "img" {
                    safe_url(&value, true)
                } else {
                    ATTRIBUTES.contains(&key.as_str())
                        || key.starts_with("data-blyg-")
                        || key == "data-line"
                };
                if !keep {
                    el.remove_attribute(&name);
                }
            }
            Ok(())
        }));
    // The rewriter fails only on its memory limits; then nothing of the
    // input is trusted.
    rewrite_str(html, settings).unwrap_or_else(|_| crate::util::escape_html(html))
}

#[cfg(test)]
mod tests {
    use super::sanitize_html;

    #[test]
    fn allowlist() {
        assert_eq!(
            sanitize_html("<p onclick=\"x()\" class=c>a<script>alert(1)</script>b</p>"),
            "<p class=c>ab</p>"
        );
        assert_eq!(
            sanitize_html("<ol start=\"3\">\n<li>a</li></ol><button>b</button>"),
            "<ol>\n<li>a</li></ol>b"
        );
        assert_eq!(
            sanitize_html("<a href=\"java&#x09;script:x\">j</a><a href=\"tel:1\">t</a>"),
            "<a>j</a><a href=\"tel:1\">t</a>"
        );
        assert_eq!(
            sanitize_html("<img src=\"data:image/png;base64,AA\" alt=\"\" loading=\"lazy\">"),
            "<img alt=\"\">"
        );
        assert_eq!(
            sanitize_html(
                "<blockquote class=\"blyg-transclusion\" data-blyg-id=\"x\" data-ytid=\"y\" data-line=\"2\">q</blockquote>"
            ),
            "<blockquote class=\"blyg-transclusion\" data-blyg-id=\"x\" data-line=\"2\">q</blockquote>"
        );
    }
}
