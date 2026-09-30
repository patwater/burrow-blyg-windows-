//! The preview page around the rendered content: studio styles, the item's
//! `<article>` wrapper (as pages.ts publishes it), citation lines, and a
//! complete HTML document with a strict Content-Security-Policy.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

use crate::Kind;
use crate::embeds::{embed_css, preview_script};
use crate::util::escape_html;

/// Studio-only additions on top of the blyg's own theme: the generated-span
/// tint, the unresolved-quote marker, and the facade styles (adapted from
/// the Worker's studio `STUDIO_STYLE` preview rules and `EMBED_CSS`).
///
/// The tint is a studio authoring aid only: the public page deliberately
/// leaves `.blyg-tk-gen` unstyled.
pub fn studio_css() -> String {
    format!(
        "{}{}",
        r#"
/* ---- blyg studio preview additions ---------------------------------- */
.blyg-tk-gen { background: rgba(90,140,255,0.12); border-radius: 3px; box-shadow: 0 0 0 2px rgba(90,140,255,0.12); }
div.blyg-tk-gen { padding: 0.1rem 0.4rem; }
span.blyg-tk-gen { padding: 0.03rem 0.15rem; }
blockquote.blyg-transclusion.unresolved {
  border-left: 3px solid var(--alert, #b3261e); background: var(--alert-wash, rgba(179,38,30,0.08));
  color: var(--alert, #b3261e); font-style: italic;
}
@media (prefers-color-scheme: dark) {
  blockquote.blyg-transclusion.unresolved { --alert: #f0a19a; --alert-wash: rgba(240,161,154,0.12); }
}
.item-content img { max-width: 100%; }
"#,
        embed_css()
    )
}

/// Options for [`page_shell_with`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellOpts {
    /// The document title.
    pub title: String,
    /// Base URL for relative links and images (e.g. the blyg's mount,
    /// `https://blyg.example.com/blyg/`), emitted before the CSP.
    pub base_href: Option<String>,
    /// Someone else's published HTML (the Reading screen): the stricter
    /// [`reader_csp`] (images over https or `data:` only, no remote styles
    /// or fonts) and no `Referer` on anything the page loads.
    pub reader: bool,
}

fn nonce() -> String {
    // Two independently seeded SipHash states: OS-random keys per process
    // and per `RandomState`, mixed with the clock. Content never sees it.
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut out = String::with_capacity(32);
    for salt in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(t);
        h.write_u64(salt);
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// The preview page's Content-Security-Policy: images from anywhere, frames
/// only from youtube-nocookie.com, scripts only with `nonce`, nothing else
/// that could run or phone home.
pub fn csp(nonce: &str) -> String {
    format!(
        "default-src 'none'; img-src * data: blob:; style-src 'unsafe-inline' https:; font-src * data:; \
         frame-src https://www.youtube-nocookie.com https://youtube-nocookie.com; \
         script-src 'nonce-{nonce}'; connect-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'"
    )
}

/// The Reading screen's policy: like [`csp`], but images only over https
/// (or inline `data:`), and styles and fonts only from the page itself.
pub fn reader_csp(nonce: &str) -> String {
    format!(
        "default-src 'none'; img-src https: data:; style-src 'unsafe-inline'; font-src data:; \
         frame-src https://www.youtube-nocookie.com https://youtube-nocookie.com; \
         script-src 'nonce-{nonce}'; connect-src 'none'; media-src 'none'; object-src 'none'; \
         base-uri 'none'; form-action 'none'"
    )
}

/// A complete, safe preview document: the theme CSS (the blyg's own
/// `/style.css`), the studio additions, `body` verbatim, and the preview
/// script under a fresh CSP nonce. No script from content can run: content
/// has no nonce, and inline handlers and `javascript:` URLs are blocked.
pub fn page_shell(theme_css: &str, body: &str) -> String {
    page_shell_with(theme_css, body, &ShellOpts::default())
}

/// [`page_shell`] with a title and base URL.
pub fn page_shell_with(theme_css: &str, body: &str, opts: &ShellOpts) -> String {
    let nonce = nonce();
    let base = opts
        .base_href
        .as_ref()
        .map(|b| format!("<base href=\"{}\">\n", escape_html(b)))
        .unwrap_or_default();
    let (policy, referrer) = if opts.reader {
        (
            reader_csp(&nonce),
            "<meta name=\"referrer\" content=\"no-referrer\">\n",
        )
    } else {
        (csp(&nonce), "")
    };
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n{base}\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">\n{referrer}\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{title}</title>\n<style>{theme}</style>\n<style>{studio}</style>\n</head>\n<body>\n{body}\n\
         <script nonce=\"{nonce}\">{script}</script>\n</body>\n</html>\n",
        csp = escape_html(&policy),
        title = escape_html(&opts.title),
        theme = style_safe(theme_css),
        studio = style_safe(&studio_css()),
        script = preview_script(),
    )
}

/// Keep CSS from closing its `<style>` element early.
fn style_safe(css: &str) -> String {
    css.replace("</", "<\\/")
}

/// A citation's human half (the frozen `stub_cite` / `fork_cite`) plus the
/// pointer it describes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Citation {
    /// The URL the citation names (a stub's target, or a fork's pinned file).
    pub url: String,
    pub source: Option<String>,
    pub author: Option<String>,
    pub excerpt: Option<String>,
    /// The cited blyg item, when it is one.
    pub id: Option<String>,
    pub version: Option<u32>,
    /// ISO 8601 retrieval time.
    pub retrieved: Option<String>,
}

/// pages.ts `formatDate` (`en-US`, short month), from the date part of an ISO time.
fn format_date(iso: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut it = iso.get(..10).unwrap_or("").split('-');
    let (Some(y), Some(m), Some(d)) = (it.next(), it.next(), it.next()) else {
        return escape_html(iso);
    };
    match (m.parse::<usize>(), d.parse::<u32>()) {
        (Ok(m @ 1..=12), Ok(d)) => format!("{} {d}, {y}", MONTHS[m - 1]),
        _ => escape_html(iso),
    }
}

fn citation_html(label: &str, c: &Citation, compact: bool, item_part: Option<String>) -> String {
    let url = escape_html(&c.url);
    if compact {
        let who = c.source.as_ref().map_or_else(
            || url.clone(),
            |s| format!("<cite>{}</cite>", escape_html(s)),
        );
        return format!(
            "<p class=\"stub-cite compact\"><span class=\"label\">{label}</span> <a href=\"{url}\">{who} ↗</a></p>"
        );
    }
    let mut parts = Vec::new();
    if let Some(s) = &c.source {
        parts.push(format!("<cite>{}</cite>", escape_html(s)));
    }
    if let Some(a) = &c.author {
        parts.push(escape_html(a));
    }
    if let Some(e) = &c.excerpt {
        parts.push(format!("&ldquo;{}&rdquo;", escape_html(e)));
    }
    if let Some(p) = item_part {
        parts.push(p);
    }
    parts.push(format!("&lt;<a href=\"{url}\">{url}</a>&gt;"));
    if let Some(r) = &c.retrieved {
        parts.push(format!("retrieved {}", format_date(r)));
    }
    format!(
        "<p class=\"stub-cite\"><span class=\"label\">{label}</span><br>{}</p>",
        parts.join(" &middot; ")
    )
}

/// pages.ts `stubCitation`: what a stub thread responds to.
pub fn stub_citation_html(c: &Citation, compact: bool) -> String {
    let item = c.id.as_ref().map(|id| {
        format!(
            "item <code>{}</code>, v{}",
            escape_html(id),
            c.version.unwrap_or(0)
        )
    });
    citation_html("In response to", c, compact, item)
}

/// pages.ts `forkLineage`: the pinned version an item was forked from.
pub fn fork_lineage_html(c: &Citation, compact: bool) -> String {
    let item = c.id.as_ref().map(|id| {
        format!(
            "item <code>{}</code>, pinned v{}",
            escape_html(id),
            c.version.unwrap_or(0)
        )
    });
    citation_html("Forked from", c, compact, item)
}

/// The item's `<article>` as the permalink page publishes it (pages.ts
/// `renderFragment` / `threadBlock`), minus the meta lines: citations first,
/// then `.item-content` holding the rendered HTML.
/// An image attached to an item (a Worker `media` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// The row's `r2_key`, e.g. `media/abc123.webp`.
    pub key: String,
    /// The stored alt text.
    pub alt: Option<String>,
}

/// pages.ts `mediaHtml`: every attachment as a lazy image paragraph. Public
/// pages put this after `.item-content`; since the Worker's patch 8 the
/// studio preview shows it too (see [`preview_media`]). `src` is
/// HTML-escaped, where the Worker interpolates it raw.
pub fn media_html(media: &[Attachment], mount: &str) -> String {
    media
        .iter()
        .map(|m| {
            format!(
                "<p><img src=\"{}\" alt=\"{}\" loading=\"lazy\"></p>",
                escape_html(&format!("{mount}/{}", m.key)),
                escape_html(m.alt.as_deref().unwrap_or(""))
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// attachments.ts `previewMedia`: the studio preview's attachment strip,
/// placed right after the preview body.
pub fn preview_media(media: &[Attachment], mount: &str) -> String {
    format!(
        "<div id=\"preview-media\">{}</div>",
        media_html(media, mount)
    )
}

pub fn article_html(
    kind: Kind,
    content_html: &str,
    stub: Option<&Citation>,
    fork: Option<&Citation>,
) -> String {
    let fork = fork
        .map(|c| fork_lineage_html(c, false))
        .unwrap_or_default();
    match kind {
        Kind::Fragment => format!(
            "<article class=\"fragment\">\n{fork}\n<div class=\"item-content\">\n{content_html}\n</div>\n</article>"
        ),
        Kind::Thread => {
            let stub = stub
                .map(|c| stub_citation_html(c, false))
                .unwrap_or_default();
            format!(
                "<article class=\"thread\">\n{stub}\n{fork}\n<div class=\"item-content\">\n{content_html}\n</div>\n</article>"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_is_locked_down() {
        let page = page_shell("body{color:red}</style><script>x()</script>", "<p>hi</p>");
        assert!(page.contains("script-src &#39;nonce-"));
        assert!(page.contains("frame-src https://www.youtube-nocookie.com"));
        assert!(!page.contains("</style><script>x()"));
        // The theme's text stays inside <style> (raw text until `</style`).
        assert!(page.contains("<\\/style><script>x()<\\/script>"));
        assert_eq!(page.matches("</style>").count(), 2);
        assert_eq!(page.matches("<script nonce=").count(), 1);
        assert!(!page.contains("<meta name=\"referrer\""));
        let reader = page_shell_with(
            "",
            "<p>hi</p>",
            &ShellOpts {
                reader: true,
                base_href: Some("https://blyg.example.com/".into()),
                ..Default::default()
            },
        );
        assert!(reader.contains("img-src https: data:;"));
        assert!(!reader.contains("img-src *"));
        assert!(reader.contains("<meta name=\"referrer\" content=\"no-referrer\">"));
        assert!(reader.contains("<base href=\"https://blyg.example.com/\">"));
        assert!(reader.find("<base").unwrap() < reader.find("Content-Security-Policy").unwrap());
        let n1 = nonce();
        assert_eq!(n1.len(), 32);
        assert_ne!(n1, nonce());
    }

    #[test]
    fn citations() {
        let c = Citation {
            url: "https://blyg.example.com/f/abc/".into(),
            source: Some("Example Notes".into()),
            id: Some("abc".into()),
            version: Some(2),
            retrieved: Some("2026-07-01T10:00:00Z".into()),
            ..Default::default()
        };
        let html = stub_citation_html(&c, false);
        assert!(html.contains(
            "<cite>Example Notes</cite> &middot; item <code>abc</code>, v2 &middot; &lt;<a"
        ));
        assert!(html.ends_with("retrieved Jul 1, 2026</p>"));
    }
}
