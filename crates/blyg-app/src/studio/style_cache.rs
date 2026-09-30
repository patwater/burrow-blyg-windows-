//! The preview's theme: the blyg's own public `/style.css`.
//!
//! Fetched once per connection with blyg-core's token-less `PublicClient`
//! (a plain unauthenticated GET, so the owner token can't be sent), cached in
//! `<data dir>/style-cache/`, and used from there offline. Without either
//! (fake mode, never connected, fetch failed with no cache) the preview uses
//! [`BUILTIN_CSS`]. `blyg-render`'s `page_shell` adds the studio additions
//! (`studio_css`, which includes `embed_css`) on top.

use std::path::{Path, PathBuf};

/// A theme bigger than this is not a stylesheet.
const MAX_BYTES: u64 = 1024 * 1024;

/// Where the CSS in use came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Fetched from the blyg just now.
    Blyg,
    /// The last copy fetched from this blyg.
    Cached,
    /// Nothing from the blyg: the app's own.
    Builtin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub css: String,
    pub source: Source,
}

impl Theme {
    pub fn builtin() -> Theme {
        Theme {
            css: BUILTIN_CSS.to_string(),
            source: Source::Builtin,
        }
    }
}

/// An unauthenticated GET. Implemented by `PublicClient`; tests use it with a
/// local mock server, or fake it outright.
pub trait Fetch: Send + Sync {
    fn get(&self, url: &str) -> Result<(Vec<u8>, Option<String>), String>;
}

impl Fetch for blyg_core::api::public::PublicClient {
    fn get(&self, url: &str) -> Result<(Vec<u8>, Option<String>), String> {
        self.get_bytes(url, MAX_BYTES).map_err(|e| e.to_string())
    }
}

/// `{base}/style.css` (the blyg may be mounted under a path).
pub fn style_url(base_url: &str) -> Option<String> {
    blyg_core::origin_url(base_url, "style.css")
}

pub fn cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("style-cache")
}

/// One file per blyg origin.
pub fn cache_file(data_dir: &Path, base_url: &str) -> PathBuf {
    let hash = blyg_core::content_hash(base_url.trim_end_matches('/'));
    let hash = hash.trim_start_matches("sha256:");
    cache_dir(data_dir).join(format!("{}.css", &hash[..16.min(hash.len())]))
}

/// The cached copy for this blyg, if any.
pub fn cached(data_dir: &Path, base_url: &str) -> Option<String> {
    std::fs::read_to_string(cache_file(data_dir, base_url))
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// What to show before (or instead of) a fetch: the cache, else the built-in.
pub fn initial(data_dir: Option<&Path>, base_url: Option<&str>) -> Theme {
    match (data_dir, base_url) {
        (Some(d), Some(b)) => cached(d, b).map_or_else(Theme::builtin, |css| Theme {
            css,
            source: Source::Cached,
        }),
        _ => Theme::builtin(),
    }
}

/// Fetch `/style.css`, keep a copy, and return it. On failure, the cached
/// copy (offline) or the built-in CSS.
pub fn refresh(fetch: &dyn Fetch, data_dir: Option<&Path>, base_url: &str) -> Theme {
    match fetch_css(fetch, base_url) {
        Ok(css) => {
            if let Some(d) = data_dir {
                let path = cache_file(d, base_url);
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&path, &css);
            }
            Theme {
                css,
                source: Source::Blyg,
            }
        }
        Err(_) => initial(data_dir, Some(base_url)),
    }
}

fn fetch_css(fetch: &dyn Fetch, base_url: &str) -> Result<String, String> {
    let url = style_url(base_url).ok_or_else(|| format!("not a blyg URL: {base_url}"))?;
    let (bytes, mime) = fetch.get(&url)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("too big for a stylesheet".into());
    }
    let css = String::from_utf8(bytes).map_err(|_| "not UTF-8".to_string())?;
    // A 200 with an HTML page (a catch-all route) is not a stylesheet.
    let html_type = mime.as_deref().is_some_and(|m| m.contains("html"));
    let looks_html = css.trim_start().starts_with('<');
    if html_type || looks_html || css.trim().is_empty() {
        return Err("not a stylesheet".into());
    }
    Ok(css)
}

/// Forget cached themes (Disconnect › delete the local copy).
pub fn forget(data_dir: &Path) {
    let _ = std::fs::remove_dir_all(cache_dir(data_dir));
}

/// The app's own reading theme, for when the blyg's isn't available: the
/// Tufte palette of the app itself, following light/dark.
pub const BUILTIN_CSS: &str = r#"
:root { --paper: #fffff8; --ink: #111; --muted: #6b665d; --accent: #a4271b; --rule: #dcd6c9; --wash: rgba(17,17,17,0.04); color-scheme: light dark; }
@media (prefers-color-scheme: dark) {
  :root { --paper: #161513; --ink: #e4dfd3; --muted: #9a9489; --accent: #e0775a; --rule: #2f2d29; --wash: rgba(228,223,211,0.05); }
}
html { background: var(--paper); }
body { margin: 0; padding: 30px 38px 60px; background: var(--paper); color: var(--ink);
  font: 17px/1.7 Literata, "Iowan Old Style", Charter, Georgia, serif; -webkit-font-smoothing: antialiased; }
article { max-width: 36em; margin: 0 auto; }
a { color: var(--accent); text-decoration-thickness: 1px; text-underline-offset: 2px; }
h1, h2, h3, h4 { font-weight: 400; font-style: italic; line-height: 1.2; margin: 1.4em 0 0.5em; }
h1 { font-size: 1.8rem; } h2 { font-size: 1.5rem; } h3 { font-size: 1.25rem; }
p, ul, ol, pre, table, figure { margin: 0 0 0.9em; }
.item-content > :first-child { margin-top: 0; }
blockquote { margin: 1rem 0; padding: 0.6rem 0.9rem; border-left: 3px solid var(--rule); background: var(--wash); border-radius: 0 4px 4px 0; }
blockquote > :last-child { margin-bottom: 0; }
blockquote.blyg-transclusion { font-size: 0.94rem; }
.blyg-provenance, blockquote.blyg-transclusion > p:last-child small { color: var(--muted); font: 12px/1.4 Inter, -apple-system, system-ui, sans-serif; }
code, pre { font: 0.86em/1.5 "SF Mono", Menlo, monospace; }
pre { padding: 0.7em 0.9em; overflow-x: auto; background: var(--wash); border-radius: 4px; }
hr { border: 0; border-top: 1px solid var(--rule); margin: 1.6em 0; }
img { max-width: 100%; height: auto; }
table { border-collapse: collapse; } th, td { border-bottom: 1px solid var(--rule); padding: 0.2em 0.6em; text-align: left; }
figcaption { color: var(--muted); font: 12px/1.4 Inter, -apple-system, system-ui, sans-serif; margin-top: 4px; word-break: break-all; }
.stub-cite { color: var(--muted); font-size: 0.9rem; }
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    struct FakeFetch(
        Result<(Vec<u8>, Option<String>), String>,
        Mutex<Vec<String>>,
    );
    impl Fetch for FakeFetch {
        fn get(&self, url: &str) -> Result<(Vec<u8>, Option<String>), String> {
            self.1.lock().unwrap().push(url.to_string());
            self.0.clone()
        }
    }

    #[test]
    fn urls_and_files() {
        assert_eq!(
            style_url("https://blyg.example.com").as_deref(),
            Some("https://blyg.example.com/style.css")
        );
        assert_eq!(
            style_url("https://blyg.example.com/blyg").as_deref(),
            Some("https://blyg.example.com/blyg/style.css")
        );
        assert_eq!(style_url("not a url"), None);
        let d = Path::new("/data");
        assert_eq!(
            cache_file(d, "https://blyg.example.com/"),
            cache_file(d, "https://blyg.example.com")
        );
        assert_ne!(
            cache_file(d, "https://blyg.example.com"),
            cache_file(d, "https://other.example.org")
        );
    }

    #[test]
    fn fetched_css_is_cached_and_used_offline() {
        let dir = tempfile::tempdir().unwrap();
        let base = "https://blyg.example.com";
        assert_eq!(
            initial(Some(dir.path()), Some(base)).source,
            Source::Builtin
        );

        let ok = FakeFetch(
            Ok((b"body{color:teal}".to_vec(), Some("text/css".into()))),
            Mutex::default(),
        );
        let t = refresh(&ok, Some(dir.path()), base);
        assert_eq!(t.source, Source::Blyg);
        assert_eq!(t.css, "body{color:teal}");
        assert_eq!(
            ok.1.lock().unwrap().as_slice(),
            ["https://blyg.example.com/style.css"]
        );

        // Offline: the cached copy.
        let offline = FakeFetch(Err("offline".into()), Mutex::default());
        let t = refresh(&offline, Some(dir.path()), base);
        assert_eq!(t.source, Source::Cached);
        assert_eq!(t.css, "body{color:teal}");
        assert_eq!(initial(Some(dir.path()), Some(base)).source, Source::Cached);

        // Another blyg has no cache: built-in.
        let t = refresh(&offline, Some(dir.path()), "https://other.example.org");
        assert_eq!(t, Theme::builtin());

        // No data dir at all (tests, fake mode): built-in, nothing written.
        assert_eq!(refresh(&offline, None, base), Theme::builtin());

        forget(dir.path());
        assert_eq!(
            initial(Some(dir.path()), Some(base)).source,
            Source::Builtin
        );
    }

    #[test]
    fn an_html_page_is_not_a_theme() {
        let dir = tempfile::tempdir().unwrap();
        let base = "https://blyg.example.com";
        for (body, mime) in [
            ("<!doctype html><p>not found</p>", Some("text/html")),
            ("body{}", Some("text/html; charset=utf-8")),
            ("   ", Some("text/css")),
            ("<html>", None),
        ] {
            let f = FakeFetch(
                Ok((body.as_bytes().to_vec(), mime.map(str::to_string))),
                Mutex::default(),
            );
            assert_eq!(
                refresh(&f, Some(dir.path()), base),
                Theme::builtin(),
                "{body}"
            );
        }
        assert!(!cache_file(dir.path(), base).exists());
    }

    /// The real client against a local server: a plain GET for /style.css
    /// with no `authorization` header and no cookie.
    #[test]
    fn the_fetch_never_sends_a_token() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen2 = seen.clone();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut lines = Vec::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                lines.push(line.trim_end().to_string());
            }
            *seen2.lock().unwrap() = lines;
            let body = "body{color:olive}";
            let _ = write!(
                sock,
                "HTTP/1.1 200 OK\r\ncontent-type: text/css\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.flush();
            let mut rest = Vec::new();
            let _ = reader.read_to_end(&mut rest);
        });
        let dir = tempfile::tempdir().unwrap();
        let base = format!("http://127.0.0.1:{port}");
        let client = blyg_core::api::public::PublicClient::new();
        let t = refresh(&client, Some(dir.path()), &base);
        server.join().unwrap();
        assert_eq!(t.source, Source::Blyg);
        assert_eq!(t.css, "body{color:olive}");
        let lines = seen.lock().unwrap().clone();
        assert_eq!(lines[0], "GET /style.css HTTP/1.1");
        for l in &lines[1..] {
            let name = l.split(':').next().unwrap_or("").to_ascii_lowercase();
            assert!(
                !matches!(
                    name.as_str(),
                    "authorization" | "cookie" | "proxy-authorization"
                ),
                "sent {name}"
            );
        }
        assert!(cache_file(dir.path(), &base).exists());
    }
}
