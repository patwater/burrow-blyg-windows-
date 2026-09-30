//! The embeds extension (the Worker's `embeds.ts`, "local patch 6"): the
//! YouTube click-to-load facade and the off-origin image fallback.
//!
//! The baked facade needs no script: it is a poster image linked to the video
//! plus a caption holding the original link. The preview page's own script
//! ([`preview_script`]) swaps the poster for a `youtube-nocookie.com` iframe on
//! click and replaces a failed off-origin image with a visible link.

use regex::Regex;
use std::sync::OnceLock;

use crate::util::{escape_html, js_trim};

/// JavaScript's `\s`, as a character-class body.
pub(crate) const JS_WS: &str = r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

/// The Worker's `YOUTUBE_RE` source (JavaScript syntax), used by the preview script.
const YOUTUBE_RE_JS: &str = r"^https?:\/\/(?:www\.|m\.|music\.)?(?:youtube\.com\/watch\?(?:[^#\s]*&)?v=|youtube\.com\/shorts\/|youtube\.com\/live\/|youtu\.be\/)([A-Za-z0-9_-]{6,})";

fn youtube_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"^https?://(?:www\.|m\.|music\.)?(?:youtube\.com/watch\?(?:[^#{JS_WS}]*&)?v=|youtube\.com/shorts/|youtube\.com/live/|youtu\.be/)([A-Za-z0-9_-]{{6,}})"
        ))
        .expect("youtube regex")
    })
}

/// The video id of a YouTube URL (`youtubeId` in embeds.ts).
pub fn youtube_id(url: &str) -> Option<String> {
    youtube_re()
        .captures(js_trim(url))
        .map(|c| c[1].to_string())
}

/// The baked facade markup, byte-identical to embeds.ts `youtubeFacadeHtml`.
pub fn youtube_facade_html(id: &str, href: &str) -> String {
    let i = escape_html(id);
    let h = escape_html(href);
    format!(
        "<figure class=\"blyg-yt\" data-ytid=\"{i}\">\
         <a class=\"blyg-yt-poster\" href=\"{h}\" aria-label=\"Play video\"><img src=\"https://i.ytimg.com/vi/{i}/hqdefault.jpg\" alt=\"\" width=\"480\" height=\"360\" loading=\"lazy\" referrerpolicy=\"no-referrer\"></a>\
         <figcaption><a href=\"{h}\">{h}</a></figcaption>\
         </figure>\n"
    )
}

/// Facade and image-fallback styles (embeds.ts `EMBED_CSS`, with fallbacks
/// for themes that do not define the reference palette's custom properties).
pub fn embed_css() -> &'static str {
    r#"
.blyg-yt { margin: 1.25rem 0; }
.blyg-yt-poster, .blyg-yt iframe {
  position: relative; display: block; width: 100%; aspect-ratio: 16 / 9;
  border: 1px solid var(--rule, #dde3e5); border-radius: 2px; overflow: hidden; background: var(--paper-sunk, #eef1f3);
}
.blyg-yt iframe { border: 0; }
article .blyg-yt-poster img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; border-radius: 0; }
.blyg-yt-poster::before {
  content: ""; position: absolute; top: 50%; left: 50%; width: 3.6rem; height: 3.6rem; margin: -1.8rem 0 0 -1.8rem;
  border-radius: 50%; background: var(--pencil, #23608c); box-shadow: 0 2px 10px rgb(0 0 0 / 0.3); z-index: 1;
  transition: transform 0.15s ease;
}
.blyg-yt-poster::after {
  content: ""; position: absolute; top: 50%; left: 50%; margin: -0.7rem 0 0 -0.4rem; z-index: 2;
  border-style: solid; border-width: 0.7rem 0 0.7rem 1.15rem; border-color: transparent transparent transparent var(--paper, #fafbfb);
}
.blyg-yt-poster:hover::before { transform: scale(1.07); }
.blyg-yt figcaption { font: var(--apparatus, 0.8rem/1.4 system-ui, sans-serif); color: var(--ink-soft, #5c686b); margin-top: 0.35rem; overflow-wrap: anywhere; }
.blyg-yt figcaption a { color: var(--ink-soft, #5c686b); }
.blyg-img-fallback {
  display: inline-block; font: var(--apparatus, 0.8rem/1.4 system-ui, sans-serif); padding: 0.35rem 0.6rem;
  border: 1px dashed var(--rule, #dde3e5); border-radius: 2px; color: var(--pencil, #23608c); overflow-wrap: anywhere;
}
@media (prefers-reduced-motion: reduce) { .blyg-yt-poster::before { transition: none; } }
"#
}

/// The preview page's own script: embeds.ts `EMBED_SCRIPT` (click-to-play in a
/// `youtube-nocookie.com` iframe, client-side upgrade of bare YouTube links in
/// older quoted snapshots, and the failed-image link fallback). It is the only
/// script a preview page runs; [`crate::page_shell`] gives it the CSP nonce.
pub fn preview_script() -> String {
    // JSON-encode the regex source exactly as the Worker does (JSON.stringify).
    let json = format!(
        "\"{}\"",
        YOUTUBE_RE_JS.replace('\\', "\\\\").replace('"', "\\\"")
    );
    PREVIEW_SCRIPT.replace("__YOUTUBE_RE__", &json)
}

const PREVIEW_SCRIPT: &str = r#"
(function () {
  "use strict";
  var YT = new RegExp(__YOUTUBE_RE__);

  function facade(id, href) {
    var fig = document.createElement("figure");
    fig.className = "blyg-yt";
    fig.setAttribute("data-ytid", id);
    var a = document.createElement("a");
    a.className = "blyg-yt-poster";
    a.href = href;
    a.setAttribute("aria-label", "Play video");
    var img = document.createElement("img");
    img.src = "https://i.ytimg.com/vi/" + encodeURIComponent(id) + "/hqdefault.jpg";
    img.alt = "";
    img.width = 480; img.height = 360;
    img.loading = "lazy";
    img.referrerPolicy = "no-referrer";
    a.appendChild(img);
    var cap = document.createElement("figcaption");
    var link = document.createElement("a");
    link.href = href;
    link.textContent = href;
    cap.appendChild(link);
    fig.appendChild(a);
    fig.appendChild(cap);
    return fig;
  }

  // Quoted snapshots baked before the facade existed hold a bare <p><a>url</a></p>;
  // the public page upgrades them in the browser, so the preview does too.
  function upgrade(root) {
    if (!root.querySelectorAll) return;
    var ps = root.querySelectorAll("article p");
    for (var i = 0; i < ps.length; i++) {
      var p = ps[i];
      if (p.children.length !== 1 || p.children[0].tagName !== "A") continue;
      var a = p.children[0];
      var href = a.getAttribute("href") || "";
      var text = (a.textContent || "").trim();
      if (text !== href || (p.textContent || "").trim() !== text) continue;
      var m = YT.exec(href);
      if (m && p.parentNode) p.parentNode.replaceChild(facade(m[1], href), p);
    }
  }

  function play(poster) {
    var fig = poster.closest(".blyg-yt");
    var id = fig && fig.getAttribute("data-ytid");
    if (!id || !/^[A-Za-z0-9_-]+$/.test(id)) return false;
    var f = document.createElement("iframe");
    f.src = "https://www.youtube-nocookie.com/embed/" + encodeURIComponent(id) + "?autoplay=1&rel=0";
    f.title = "YouTube video";
    f.setAttribute("allow", "autoplay; encrypted-media; picture-in-picture");
    f.setAttribute("allowfullscreen", "");
    f.setAttribute("referrerpolicy", "strict-origin-when-cross-origin");
    poster.parentNode.replaceChild(f, poster);
    try { f.focus(); } catch (e) {}
    return true;
  }

  document.addEventListener("click", function (e) {
    var poster = e.target.closest && e.target.closest(".blyg-yt-poster");
    if (!poster || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0) return;
    if (play(poster)) e.preventDefault();
  });

  // Off-origin images that fail become a visible link to the image.
  function remote(img) {
    if (!img || img.tagName !== "IMG" || !img.closest("article") || img.closest(".blyg-yt")) return null;
    var src = img.getAttribute("src") || "";
    try {
      var u = new URL(src, document.baseURI);
      if (!/^https?:$/.test(u.protocol)) return null;
      if (/^https?:$/.test(location.protocol) && u.origin === location.origin) return null;
      return u;
    } catch (e) { return null; }
  }
  function fallback(img) {
    var u = remote(img);
    if (!u || !img.parentNode) return;
    var alt = (img.getAttribute("alt") || "").trim();
    var label = "Image" + (alt ? ": " + alt : "") + " ↗ (" + u.host + ")";
    var el;
    if (img.parentNode.closest("a")) { el = document.createElement("span"); }
    else { el = document.createElement("a"); el.href = u.href; el.rel = "noopener noreferrer"; }
    el.className = "blyg-img-fallback";
    el.title = "This image could not be loaded from " + u.host;
    el.textContent = label;
    img.parentNode.replaceChild(el, img);
  }
  document.addEventListener("error", function (e) { fallback(e.target); }, true);
  function sweep(root) {
    if (!root.querySelectorAll) return;
    var imgs = root.querySelectorAll("article img");
    for (var i = 0; i < imgs.length; i++) if (imgs[i].complete && imgs[i].naturalWidth === 0 && imgs[i].getAttribute("src")) fallback(imgs[i]);
  }

  function run(root) { upgrade(root); sweep(root); }
  run(document);
  if (window.MutationObserver) {
    new MutationObserver(function (records) {
      for (var i = 0; i < records.length; i++) {
        var t = records[i].target;
        if (t.nodeType === 1 && t.closest && t.closest("article")) { upgrade(t); sweep(t); }
      }
    }).observe(document.body, { childList: true, subtree: true });
  }
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert_eq!(
            youtube_id("https://youtu.be/Qa1b2C3d4E5").as_deref(),
            Some("Qa1b2C3d4E5")
        );
        assert_eq!(
            youtube_id("https://m.youtube.com/watch?feature=share&v=Zx9_abc-123&t=4").as_deref(),
            Some("Zx9_abc-123")
        );
        assert_eq!(youtube_id("https://example.org/watch?v=Zx9_abc-123"), None);
        assert!(preview_script().contains(r#"new RegExp("^https?:\\/\\/"#));
    }
}
