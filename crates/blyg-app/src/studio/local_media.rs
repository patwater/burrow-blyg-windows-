//! --- follow-ups --- Scratch-note images (`blyg-local:<sha256>.<ext>`,
//! `blyg_core::scratch_media`) in the previews. They live only on this Mac,
//! so the studio WebView gets them inline as `data:` URIs (its CSP allows
//! `data:` images, and nothing is served or fetched).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use blyg_core::scratch_media::{self, SCHEME};

/// Encoded images kept for re-renders (typing re-renders ~10×/s).
const CACHE_MAX: usize = 16;

fn cache() -> &'static Mutex<HashMap<PathBuf, Arc<String>>> {
    static C: OnceLock<Mutex<HashMap<PathBuf, Arc<String>>>> = OnceLock::new();
    C.get_or_init(Mutex::default)
}

/// `data:<mime>;base64,…` for a stored image (cached by path: names are
/// content hashes, so a path's bytes never change).
fn data_uri(path: &Path) -> Option<Arc<String>> {
    let mut c = cache().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(s) = c.get(path) {
        return Some(s.clone());
    }
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_str()?;
    let uri = Arc::new(format!(
        "data:{};base64,{}",
        scratch_media::mime_for(name),
        base64(&bytes)
    ));
    if c.len() >= CACHE_MAX {
        c.clear();
    }
    c.insert(path.to_path_buf(), uri.clone());
    Some(uri)
}

/// Swap every `src="blyg-local:…"` in rendered HTML for the image's bytes
/// (`find` maps the URL to its file). Unknown images are left as they are
/// (the page shows them broken, like any missing image).
pub fn inline_images(html: &str, find: impl Fn(&str) -> Option<PathBuf>) -> String {
    let needle = format!("src=\"{SCHEME}");
    if !html.contains(&needle) {
        return html.to_string();
    }
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find(&needle) {
        let start = i + "src=\"".len();
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end = tail.find('"').unwrap_or(tail.len());
        let url = &tail[..end];
        match find(url).and_then(|p| data_uri(&p)) {
            Some(uri) => out.push_str(&uri),
            None => out.push_str(url),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }

    #[test]
    fn local_images_are_inlined_as_data_uris() {
        let d = tempfile::tempdir().unwrap();
        let url = scratch_media::store(d.path(), b"PNGDATA", "image/png").unwrap();
        let html = format!(
            "<p><img src=\"{url}\" alt=\"\"></p><p><img src=\"https://blyg.example.com/media/a.png\" alt=\"\"></p>"
        );
        let out = inline_images(&html, |u| scratch_media::file(d.path(), u));
        assert!(!out.contains(SCHEME), "{out}");
        assert!(
            out.contains("src=\"data:image/png;base64,UE5HREFUQQ==\""),
            "{out}"
        );
        assert!(out.contains("https://blyg.example.com/media/a.png"));
        // Missing files stay as they are.
        let out = inline_images(&html, |_| None);
        assert_eq!(out, html);
    }
}
