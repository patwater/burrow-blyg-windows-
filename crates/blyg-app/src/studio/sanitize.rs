//! Someone else's published HTML, made safe to show (spec §14: "store
//! verbatim, sanitize at render").
//!
//! An allowlist ([`ammonia`], on html5ever): ordinary text markup, links,
//! images, tables and figures, plus the classes and `data-*` attributes the
//! protocol and the embeds use (`blyg-transclusion`, `data-blyg-*`,
//! `blyg-tk-gen`, `blyg-yt`, `data-ytid`). Everything else goes: scripts,
//! styles, event handlers, `javascript:` and other odd URLs, forms, objects,
//! embeds, SVG, and every `<iframe>` that isn't a youtube-nocookie embed.
//! Every image gets `referrerpolicy="no-referrer"`.
//!
//! The page's CSP (`blyg_render::reader_csp`) is the backstop, not the
//! first line: content never gets a script nonce.

use std::borrow::Cow;
use std::sync::OnceLock;

use ammonia::Builder;

const YOUTUBE_EMBEDS: [&str; 2] = [
    "https://www.youtube-nocookie.com/embed/",
    "https://youtube-nocookie.com/embed/",
];

/// `https://www.youtube-nocookie.com/embed/<id>[?query]`, nothing else.
pub fn is_youtube_embed(src: &str) -> bool {
    YOUTUBE_EMBEDS.iter().any(|prefix| {
        src.strip_prefix(prefix).is_some_and(|rest| {
            let (id, query) = rest.split_once('?').unwrap_or((rest, ""));
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                && query
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "=&_-.%".contains(c))
        })
    })
}

fn builder() -> &'static Builder<'static> {
    static B: OnceLock<Builder<'static>> = OnceLock::new();
    B.get_or_init(|| {
        let mut b = Builder::default();
        b.add_tags(["iframe"])
            // Dropped with everything inside them (not just the tags).
            .clean_content_tags(
                [
                    "script", "style", "template", "noscript", "title", "textarea", "select",
                    "object", "embed", "applet", "svg", "math", "frameset", "noembed",
                ]
                .into_iter()
                .collect(),
            )
            .rm_tags(["map", "area"])
            .add_generic_attributes(["class", "dir"])
            .add_generic_attribute_prefixes(["data-"])
            .add_tag_attributes("img", ["loading", "decoding", "referrerpolicy"])
            .add_tag_attributes(
                "iframe",
                [
                    "src",
                    "title",
                    "allow",
                    "allowfullscreen",
                    "width",
                    "height",
                    "loading",
                    "referrerpolicy",
                ],
            )
            .add_tag_attributes("li", ["value"])
            .add_tag_attributes("ol", ["reversed", "type"])
            .set_tag_attribute_value("img", "referrerpolicy", "no-referrer")
            .url_schemes(["http", "https", "mailto", "data"].into_iter().collect())
            .attribute_filter(|element, attribute, value| {
                let v = value.trim_start().to_ascii_lowercase();
                match (element, attribute) {
                    ("iframe", "src") => is_youtube_embed(value.trim()).then(|| value.into()),
                    // Inline images only; a data: link is a navigation.
                    ("img", "src") if v.starts_with("data:") => v
                        .starts_with("data:image/")
                        .then(|| value.into())
                        .filter(|_| !v.starts_with("data:image/svg")),
                    (_, "src" | "href" | "cite") if v.starts_with("data:") => None,
                    _ => Some(Cow::Borrowed(value)),
                }
            });
        b
    })
}

/// Clean `html` for the reading view.
pub fn sanitize(html: &str) -> String {
    drop_empty_iframes(&builder().clean(html).to_string())
}

/// An `<iframe>` whose `src` the filter refused would be an empty frame:
/// remove it outright. Runs on ammonia's normalized output, where every
/// iframe is `<iframe …>…</iframe>` with double-quoted attributes.
fn drop_empty_iframes(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find("<iframe") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let open_end = tail.find('>').map_or(tail.len(), |i| i + 1);
        let close = tail
            .find("</iframe>")
            .map_or(tail.len(), |i| i + "</iframe>".len());
        let keeps = tail[..open_end].contains(" src=\"");
        if keeps {
            out.push_str(&tail[..close.max(open_end)]);
        }
        rest = &tail[close.max(open_end)..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(s: &str) -> String {
        sanitize(s)
    }

    #[test]
    fn scripts_and_handlers_go() {
        let out = clean(
            "<p onclick=\"alert(1)\" onmouseover='x()'>hi<script>alert(1)</script></p>\
             <img src=\"https://example.com/a.png\" onerror=\"alert(2)\">\
             <style>body{display:none}</style><SCRIPT SRC=//evil.example.com/x.js></SCRIPT>\
             <svg onload=alert(3)><circle/></svg><math><mi>x</mi></math>",
        );
        assert!(!out.to_lowercase().contains("script"), "{out}");
        assert!(
            !out.contains("onclick") && !out.contains("onmouseover"),
            "{out}"
        );
        assert!(!out.contains("onerror") && !out.contains("alert"), "{out}");
        assert!(
            !out.contains("display:none") && !out.contains("svg"),
            "{out}"
        );
        assert!(out.contains("<p>hi</p>"), "{out}");
        assert!(out.contains("src=\"https://example.com/a.png\""), "{out}");
    }

    #[test]
    fn javascript_and_data_links_go() {
        for bad in [
            "<a href=\"javascript:alert(1)\">x</a>",
            "<a href=\" JaVaScRiPt:alert(1)\">x</a>",
            "<a href=\"jav&#x09;ascript:alert(1)\">x</a>",
            "<a href=\"vbscript:msgbox(1)\">x</a>",
            "<a href=\"data:text/html,<script>alert(1)</script>\">x</a>",
            "<a href=\"file:///etc/passwd\">x</a>",
        ] {
            let out = clean(bad);
            assert!(!out.contains("href"), "{bad} → {out}");
            assert!(out.contains(">x</a>"), "{bad} → {out}");
        }
        let img = clean(
            "<img src=\"javascript:alert(1)\"><img src=\"data:image/svg+xml,<svg onload=alert(1)>\">",
        );
        assert!(!img.contains("src="), "{img}");
        // Inline raster images and ordinary links stay.
        let ok = clean(
            "<img src=\"data:image/png;base64,iVBORw0KGgo=\"><a href=\"https://example.com/x\">y</a>\
             <a href=\"f/01k2abc/\">rel</a><a href=\"mailto:someone@example.com\">m</a>",
        );
        assert!(
            ok.contains("src=\"data:image/png;base64,iVBORw0KGgo=\""),
            "{ok}"
        );
        assert!(ok.contains("href=\"https://example.com/x\""), "{ok}");
        assert!(ok.contains("href=\"f/01k2abc/\""), "{ok}");
        assert!(ok.contains("href=\"mailto:someone@example.com\""), "{ok}");
        assert!(ok.contains("rel=\"noopener noreferrer\""), "{ok}");
    }

    #[test]
    fn iframes_only_for_youtube_nocookie_embeds() {
        let out = clean(
            "<p>a</p><iframe src=\"https://evil.example.com/x\"></iframe>\
             <iframe src=\"https://www.youtube.com/embed/Qa1b2C3d4E5\"></iframe>\
             <iframe src=\"https://www.youtube-nocookie.com/embed/../../evil\"></iframe>\
             <iframe srcdoc=\"<script>alert(1)</script>\"></iframe><p>b</p>",
        );
        assert_eq!(out, "<p>a</p><p>b</p>");
        let yt = clean(
            "<iframe src=\"https://www.youtube-nocookie.com/embed/Qa1b2C3d4E5?autoplay=1\" \
             allow=\"autoplay; encrypted-media\" allowfullscreen onload=\"x()\"></iframe>",
        );
        assert!(
            yt.contains("src=\"https://www.youtube-nocookie.com/embed/Qa1b2C3d4E5?autoplay=1\""),
            "{yt}"
        );
        assert!(!yt.contains("onload"), "{yt}");
    }

    #[test]
    fn forms_objects_and_embeds_go() {
        let out = clean(
            "<form action=\"https://evil.example.com/\"><input name=q><button>Go</button></form>\
             <object data=\"x.swf\"><param name=a value=b>fallback</object>\
             <embed src=\"x.swf\"><base href=\"https://evil.example.com/\">\
             <meta http-equiv=\"refresh\" content=\"0;url=https://evil.example.com/\">\
             <link rel=stylesheet href=\"https://evil.example.com/x.css\">",
        );
        for bad in [
            "form", "input", "object", "embed", "base", "meta", "link", "evil",
        ] {
            assert!(!out.contains(bad), "{bad}: {out}");
        }
    }

    #[test]
    fn the_youtube_facade_and_protocol_markup_survive() {
        let facade = blyg_render::youtube_facade_html(
            "Qa1b2C3d4E5",
            "https://www.youtube.com/watch?v=Qa1b2C3d4E5",
        );
        let quote = "<blockquote class=\"blyg-transclusion\" data-blyg-id=\"01k2abc\" \
                     data-blyg-version=\"3\" data-blyg-origin=\"https://ada.blyg.example.com/\">\
                     <p>Quoted <span class=\"blyg-tk-gen\">words</span>.</p></blockquote>";
        let out = clean(&format!("{quote}{facade}"));
        assert!(out.contains("class=\"blyg-transclusion\""), "{out}");
        assert!(out.contains("data-blyg-id=\"01k2abc\""), "{out}");
        assert!(out.contains("data-blyg-version=\"3\""), "{out}");
        assert!(
            out.contains("data-blyg-origin=\"https://ada.blyg.example.com/\""),
            "{out}"
        );
        assert!(out.contains("<span class=\"blyg-tk-gen\">"), "{out}");
        assert!(
            out.contains("<figure class=\"blyg-yt\" data-ytid=\"Qa1b2C3d4E5\">"),
            "{out}"
        );
        assert!(out.contains("class=\"blyg-yt-poster\""), "{out}");
        assert!(
            out.contains("src=\"https://i.ytimg.com/vi/Qa1b2C3d4E5/hqdefault.jpg\""),
            "{out}"
        );
        assert!(out.contains("<figcaption>"), "{out}");
    }

    #[test]
    fn images_never_send_a_referrer() {
        let out = clean(
            "<p><img src=\"media/a.png\" alt=\"A\"></p>\
             <p><img src=\"https://cdn.example.com/b.jpg\" referrerpolicy=\"unsafe-url\"></p>",
        );
        assert_eq!(out.matches("<img").count(), 2, "{out}");
        assert_eq!(
            out.matches("referrerpolicy=\"no-referrer\"").count(),
            2,
            "{out}"
        );
        assert!(!out.contains("unsafe-url"), "{out}");
        assert!(out.contains("src=\"media/a.png\""), "relative kept: {out}");
    }

    #[test]
    fn no_ids_or_inline_styles() {
        // No DOM clobbering of the host script's globals, no CSS tricks.
        let out = clean("<p id=\"__blyg\" name=\"x\" style=\"position:fixed;inset:0\">a</p>");
        assert_eq!(out, "<p>a</p>");
    }
}
