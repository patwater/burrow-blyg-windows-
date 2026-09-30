//! The Reading screen's body: someone else's post shown the way their blyg
//! published it, in a WKWebView (the studio's `PreviewSurface` machinery).
//!
//! - The source of truth is the HTML the author's server published
//!   (`ReadingItem::content_html`, or a pin's `content_html`), with its
//!   transclusion snapshots already baked in (SPEC § Protocol philosophy 3).
//!   Only when there is no HTML (an item that carries only Markdown) is
//!   `content_md` rendered here, with `blyg-render` and a resolver over what's
//!   held locally for that origin.
//! - Attached images (the item document's `media[]`) follow the text, as the
//!   author's permalink page shows them.
//! - Everything is sanitized ([`super::sanitize`]) and shown under the strict
//!   reader CSP, with `<base href>` at the author's origin: relative
//!   `media/…` URLs are origin-relative in the protocol (§5.4).
//! - The header, notes, diff and actions around it stay native GPUI; only the
//!   body is the WebView. It also shows the current version in your own
//!   post's history (⌘Y).
//! - One WebView on screen at a time: this one only shows on the Reading
//!   screen and in ⌘Y, where the studio preview is suppressed; both hide
//!   under sheets, overlays and the version dropdown.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use blyg_core::{Backend, LocalId, RemoteMedia, SubscriptionKind};
use blyg_render::{RenderOpts, ShellOpts};
use gpui_kit::*;

use super::super::reading::View;
use super::resolver::{StoreResolver, render_kind};
use super::sanitize::sanitize;
use super::style_cache::BUILTIN_CSS;
use super::webview::{self, Factory, FactoryGlobal, SurfaceEvent};
use super::{MainView, Slot};
use crate::theme::Palette;

/// What the body shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// Published HTML (someone else's post or pin): shown as published.
    Html(String),
    /// Markdown only: rendered here with `blyg-render`.
    Markdown(String),
}

/// Where a Markdown body's quotes resolve from.
#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    /// Someone else's item: held reading items from its origin (and your own
    /// items, when the origin is your blyg).
    Remote { origin: String, id: String },
    /// Your own item's working copy (as the studio preview renders it).
    Own(LocalId),
}

/// One document for the reader, before sanitizing and rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    pub body: Body,
    pub kind: blyg_render::Kind,
    /// `<base href>`: the author's origin (with a trailing slash).
    pub base: Option<String>,
    pub media: Vec<RemoteMedia>,
    pub scope: Scope,
}

/// What the loaded page was made from (a new page when this changes).
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    doc: Doc,
    font_px: u32,
}

pub struct Reader {
    slot: Rc<RefCell<Slot>>,
    pub(super) events: async_channel::Sender<SurfaceEvent>,
    pub(super) events_rx: Option<async_channel::Receiver<SurfaceEvent>>,
    failed: Option<String>,
    shown: Option<Shown>,
    /// The body is on screen this frame (set before layout).
    active: bool,
    dark: Option<bool>,
    viewport_w: Pixels,
    /// Pages loaded (tests).
    #[cfg(test)]
    pub(crate) pages: Vec<String>,
}

impl Reader {
    pub(super) fn reclaim_keyboard(&self) {
        self.slot.borrow_mut().with(|s| s.reclaim_keyboard());
    }

    pub fn new() -> Reader {
        let (tx, rx) = async_channel::unbounded();
        Reader {
            slot: Rc::default(),
            events: tx,
            events_rx: Some(rx),
            failed: None,
            shown: None,
            active: false,
            dark: None,
            viewport_w: px(0.),
            #[cfg(test)]
            pages: Vec::new(),
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    #[cfg(test)]
    pub fn visible(&self) -> bool {
        self.slot.borrow().visible
    }

    /// Where a toast goes so this view doesn't cover it (to its left).
    pub fn toast_right(&self) -> Option<Pixels> {
        let slot = self.slot.borrow();
        match (slot.visible, slot.frame) {
            (true, Some(f)) => Some((self.viewport_w - f.origin.x).max(px(0.)) + px(14.)),
            _ => None,
        }
    }
}

/// `https://a.example/` for an origin with or without its trailing slash.
pub fn origin_base(origin: &str) -> Option<String> {
    let o = origin.trim();
    if !(o.starts_with("https://") || o.starts_with("http://")) {
        return None;
    }
    Some(format!("{}/", o.trim_end_matches('/')))
}

/// The HTML if there is any, else the Markdown, else nothing.
fn body_of(html: &str, md: &str) -> Option<Body> {
    if !html.trim().is_empty() {
        Some(Body::Html(html.to_string()))
    } else if !md.trim().is_empty() {
        Some(Body::Markdown(md.to_string()))
    } else {
        None
    }
}

/// Attached images as the permalink page lists them (after the text).
pub fn attachments_html(media: &[RemoteMedia]) -> String {
    if media.is_empty() {
        return String::new();
    }
    let imgs: String = media
        .iter()
        .map(|m| {
            format!(
                "<p><img src=\"{}\" alt=\"{}\" loading=\"lazy\"></p>\n",
                blyg_render::escape_html(&m.url),
                blyg_render::escape_html(m.alt.as_deref().unwrap_or(""))
            )
        })
        .collect();
    format!("<div class=\"blyg-attachments\">\n{imgs}</div>")
}

/// Literata (regular and italic), inlined: the WebView's own process can't
/// see the fonts the app registers with CoreText, and the CSP allows fonts
/// from `data:` only.
fn font_faces() -> &'static str {
    static CSS: OnceLock<String> = OnceLock::new();
    CSS.get_or_init(|| {
        let face = |style: &str, bytes: &[u8]| {
            format!(
                "@font-face {{ font-family: \"Literata\"; font-style: {style}; font-weight: 400; \
                 font-display: swap; src: url(data:font/ttf;base64,{}) format(\"truetype\"); }}\n",
                super::local_media::base64(bytes)
            )
        };
        let files = crate::fonts::LITERATA;
        format!("{}{}", face("normal", files[0]), face("italic", files[1]))
    })
}

/// The reader's additions on top of the app theme (`BUILTIN_CSS`, the Tufte
/// palette in light and dark): the fonts, the text size, a left-aligned
/// column under the native header, and generated spans left unstyled as the
/// public page leaves them (the tint is a studio authoring aid).
pub fn reader_css(font_px: f32) -> String {
    format!(
        "{faces}
body {{ padding: 4px 32px 44px; font-size: {font_px}px; }}
article {{ max-width: 38em; margin: 0; }}
.blyg-tk-gen {{ background: none; box-shadow: none; padding: 0; border-radius: 0; }}
blockquote.blyg-transclusion {{ font-family: inherit; }}
blockquote.blyg-transclusion[data-blyg-origin] {{ cursor: pointer; }}
blockquote.blyg-transclusion[data-blyg-origin]::after {{
  content: 'quoted from ' attr(data-blyg-origin) ' · click for profile';
  display: block; margin-top: .45em; opacity: .6;
  font: 11.5px Inter, -apple-system, system-ui, sans-serif;
}}
.blyg-attachments {{ margin-top: 1.2em; }}
.blyg-provenance, figcaption, .stub-cite {{ font-family: Inter, -apple-system, system-ui, sans-serif; }}
",
        faces = font_faces()
    )
}

/// The complete page for the reader: `content` (untrusted) sanitized, the
/// attachments after it, the item's `<article>`, the app theme, and the
/// reader CSP with `<base href>` at the author's origin.
pub fn reader_page(content: &str, doc: &Doc, font_px: f32) -> String {
    let clean = sanitize(&format!("{content}\n{}", attachments_html(&doc.media)));
    let body = blyg_render::article_html(doc.kind, &clean, None, None);
    let css = format!("{BUILTIN_CSS}\n{}", reader_css(font_px));
    blyg_render::page_shell_with(
        &css,
        &body,
        &ShellOpts {
            title: "Reading".into(),
            base_href: doc.base.clone(),
            reader: true,
        },
    )
}

/// Render a Markdown-only item of someone else's with `blyg-render`,
/// resolving quotes from what's held for its origin: reading items from the
/// same origin, plus your own items when that origin is your blyg.
pub fn render_remote_md(
    backend: &dyn Backend,
    origin: &str,
    id: &str,
    md: &str,
    kind: blyg_render::Kind,
) -> String {
    let mount = origin.trim_end_matches('/').to_string();
    let resolver = if md.contains("![[") {
        let rss: Vec<String> = backend
            .subscriptions()
            .into_iter()
            .filter(|s| s.kind == SubscriptionKind::Rss)
            .map(|s| s.id)
            .collect();
        let same = |a: &str| a.trim_end_matches('/') == mount;
        let reading = backend
            .reading()
            .into_iter()
            .filter(|r| same(&r.origin))
            .collect();
        let own = if backend.base_url().is_some_and(|b| same(&b)) {
            backend.items()
        } else {
            Vec::new()
        };
        StoreResolver::new(own, reading, &rss, &mount)
    } else {
        StoreResolver::empty(&mount)
    };
    let opts = RenderOpts {
        data_line: false,
        provenance: true,
        mount,
        self_id: Some(id.to_string()),
    };
    blyg_render::render_preview(md, kind, &resolver, &opts).html
}

// ================================================================ MainView hooks

impl MainView {
    /// What the reader body should show now, if anything.
    pub(crate) fn reader_doc(&self) -> Option<Doc> {
        match self.reading.view {
            View::Reading => {
                let o = self.reading.opened.as_ref()?;
                let item = &o.item;
                let base = origin_base(&item.origin);
                let kind = render_kind(item.kind);
                let scope = Scope::Remote {
                    origin: item.origin.clone(),
                    id: item.remote_id.clone(),
                };
                if let Some(v) = o.pinned_on_screen() {
                    if o.diff_vs_now {
                        return None; // the native diff instead
                    }
                    let pin = o.pins.get(&v.version)?.ready()?;
                    return Some(Doc {
                        body: body_of(&pin.content_html, &pin.content_md)?,
                        kind,
                        base,
                        media: Vec::new(), // a pin document has no media[]
                        scope,
                    });
                }
                let tombstone = item.state == "tombstone";
                if tombstone && item.pinned_version_retained.is_none() {
                    return None;
                }
                let media = if tombstone {
                    Vec::new()
                } else {
                    o.changelog
                        .ready()
                        .and_then(|log| log.iter().find(|v| v.current && v.version == item.version))
                        .map(|v| v.media.clone())
                        .unwrap_or_default()
                };
                Some(Doc {
                    body: body_of(&item.content_html, &item.content_md)?,
                    kind,
                    base,
                    media,
                    scope,
                })
            }
            View::Posts => {
                let own = self.reading.own.as_ref()?;
                let item = self.backend.item(&own.id)?;
                if own.sel.unwrap_or(item.version) != item.version {
                    return None;
                }
                Some(Doc {
                    body: Body::Markdown(item.content_md.clone()),
                    kind: render_kind(item.kind),
                    base: self.backend.base_url().as_deref().and_then(origin_base),
                    media: Vec::new(),
                    scope: Scope::Own(item.local_id.clone()),
                })
            }
            _ => None,
        }
    }

    /// The content HTML for a document (untrusted until sanitized).
    fn reader_content(&self, doc: &Doc) -> String {
        match (&doc.body, &doc.scope) {
            (Body::Html(h), _) => h.clone(),
            (Body::Markdown(md), Scope::Remote { origin, id }) => {
                render_remote_md(&*self.backend, origin, id, md, doc.kind)
            }
            (Body::Markdown(md), Scope::Own(id)) => {
                let item = self.backend.item(id);
                super::render_doc(&*self.backend, item.as_ref(), md).html
            }
        }
    }

    /// Per frame, before layout (from `studio_frame`): decide what the body
    /// shows, create the WebView the first time, load a new page when the
    /// document changed, and hide it whenever it isn't on screen or something
    /// GPUI draws would be under it.
    pub(super) fn reader_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let want = self.reader_doc();
        self.studio.reader.active = want.is_some();
        self.studio.reader.viewport_w = window.viewport_size().width;
        let r = &mut self.studio.reader;
        if want.is_some() && r.failed.is_none() && r.slot.borrow().surface.is_none() {
            let factory: Factory = cx
                .try_global::<FactoryGlobal>()
                .map(|g| g.0.clone())
                .unwrap_or_else(webview::default_factory);
            match factory(window, r.events.clone()) {
                Ok(surface) => {
                    r.slot.borrow_mut().surface = Some(surface);
                    r.shown = None;
                }
                Err(msg) => {
                    eprintln!("blygger: {msg}");
                    r.failed = Some(msg);
                }
            }
        }
        let dark = self.palette.dark;
        if self.studio.reader.dark != Some(dark) {
            self.studio.reader.dark = Some(dark);
            self.studio
                .reader
                .slot
                .borrow_mut()
                .with(|s| s.set_dark(dark));
        }
        if let Some(doc) = want {
            let shown = Shown {
                font_px: self.prefs.font_size.round() as u32,
                doc,
            };
            let has_surface = self.studio.reader.slot.borrow().surface.is_some();
            if has_surface && self.studio.reader.shown.as_ref() != Some(&shown) {
                let content = self.reader_content(&shown.doc);
                let page = reader_page(&content, &shown.doc, shown.font_px as f32);
                self.studio.reader.slot.borrow_mut().with(|s| s.load(&page));
                #[cfg(test)]
                self.studio.reader.pages.push(page);
                self.studio.reader.shown = Some(shown);
            }
        }
        let dropdown = self.reading.opened.as_ref().is_some_and(|o| o.dropdown)
            && self.reading.view == View::Reading;
        let suppressed = self.sheet.is_some()
            || self.ai.has_overlay()
            || self.reading.sheet.is_some()
            || dropdown
            || self.onboarding.flow.is_some()
            || self.onboarding.tutorial.is_some()
            // Every GPUI overlay that can sit over the body belongs here (and
            // in `studio_frame`): a native view draws above all GPUI content.
            || self.profile_sheet_open();
        let active = self.studio.reader.active;
        let mut slot = self.studio.reader.slot.borrow_mut();
        slot.suppressed = suppressed;
        if !active || suppressed {
            slot.hide();
        }
    }

    pub(super) fn reader_event(
        &mut self,
        ev: SurfaceEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            SurfaceEvent::Ready => {
                if std::env::var_os("BLYGGER_TIMING").is_some() {
                    self.studio.reader.slot.borrow_mut().with(|s| s.probe());
                }
            }
            // A click in the page: the keyboard goes back to the list.
            SurfaceEvent::JumpToLine(_) | SurfaceEvent::Refocus => {
                self.studio
                    .reader
                    .slot
                    .borrow_mut()
                    .with(|s| s.focus_parent());
                if self.reading.view == View::Reading {
                    window.focus(&self.reading.focus, cx);
                } else {
                    window.focus(&self.focus, cx);
                }
            }
            SurfaceEvent::OpenUrl(url) => cx.open_url(&url),
            SurfaceEvent::OpenOrigin(origin) => self.open_profile(origin, window, cx),
        }
    }

    /// The body: the WebView is placed over this element's bounds.
    pub(crate) fn reader_pane(&self, p: &Palette) -> AnyElement {
        let base = div()
            .id("reader-body")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
            .bg(p.bg);
        if let Some(msg) = self.studio.reader.failed.clone() {
            return base
                .flex()
                .items_center()
                .justify_center()
                .p(px(24.))
                .text_color(p.muted)
                .text_size(px(13.))
                .font_family("Inter")
                .child(msg)
                .into_any_element();
        }
        let slot = self.studio.reader.slot.clone();
        base.child(
            canvas(
                |bounds, _, _| bounds,
                move |_, bounds, _, _| slot.borrow_mut().place(bounds),
            )
            .size_full(),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Body, Doc, RemoteMedia, Scope, body_of, origin_base, reader_page};

    fn doc(html: &str, media: Vec<RemoteMedia>) -> Doc {
        Doc {
            body: Body::Html(html.into()),
            kind: blyg_render::Kind::Thread,
            base: origin_base("https://ada.blyg.example.com"),
            media,
            scope: Scope::Remote {
                origin: "https://ada.blyg.example.com/".into(),
                id: "01k2x".into(),
            },
        }
    }

    #[test]
    fn origin_bases() {
        assert_eq!(
            origin_base("https://ada.blyg.example.com").as_deref(),
            Some("https://ada.blyg.example.com/")
        );
        assert_eq!(
            origin_base("https://ada.blyg.example.com/blyg/").as_deref(),
            Some("https://ada.blyg.example.com/blyg/")
        );
        assert_eq!(origin_base("javascript:alert(1)"), None);
        assert_eq!(origin_base(""), None);
    }

    #[test]
    fn the_page_has_the_authors_base_and_the_reader_policy() {
        let d = doc(
            "<p>Hi <img src=\"media/a.png\"></p><script>alert(1)</script>",
            vec![RemoteMedia {
                url: "https://ada.blyg.example.com/media/b.png".into(),
                alt: Some("A \"heron\"".into()),
            }],
        );
        let page = reader_page(
            "<p>Hi <img src=\"media/a.png\"></p><script>alert(1)</script>",
            &d,
            18.,
        );
        assert!(page.contains("<base href=\"https://ada.blyg.example.com/\">"));
        assert!(page.contains("img-src https: data:;"), "reader CSP");
        assert!(page.contains("<article class=\"thread\">"));
        assert!(!page.contains("alert(1)"), "sanitized");
        assert!(
            page.contains("src=\"media/a.png\""),
            "relative, resolved by <base>"
        );
        assert!(page.contains("src=\"https://ada.blyg.example.com/media/b.png\""));
        assert!(page.contains("alt=\"A &quot;heron&quot;\""));
        assert_eq!(page.matches("referrerpolicy=\"no-referrer\"").count(), 2);
        assert!(page.contains("font-size: 18px"));
        assert!(page.contains("@font-face { font-family: \"Literata\""));
        // Exactly one script: ours, with the nonce.
        assert_eq!(page.matches("<script").count(), 1);
        assert!(page.contains("<script nonce="));
    }

    #[test]
    fn empty_bodies_are_nothing_and_markdown_is_the_fallback() {
        assert_eq!(body_of("  ", "\n"), None);
        assert_eq!(body_of("", "text"), Some(Body::Markdown("text".into())));
        assert_eq!(
            body_of("<p>x</p>", "x"),
            Some(Body::Html("<p>x</p>".into()))
        );
    }
}
