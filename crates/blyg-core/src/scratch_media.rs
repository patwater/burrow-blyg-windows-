//! Images in scratch notes stay on this Mac (docs/SPEC.md § Scratch notes:
//! "never sent anywhere"). A pasted or dropped image is copied into
//! `<data dir>/scratch-media/<sha256>.<ext>` and the note refers to it as
//! `blyg-local:<sha256>.<ext>`. Promotion uploads each one (unattached, as
//! the paste flow does for drafts) and rewrites the references to the
//! blyg's absolute `<origin>/media/<id>.<ext>` before the item is created.
//!
//! Names are content hashes, so pasting the same image twice stores it once,
//! and a name can never escape the folder (see [`file`]).

use std::path::{Path, PathBuf};

use crate::backend::{CoreError, Result};

/// The URL scheme of a local image reference in markdown.
pub const SCHEME: &str = "blyg-local:";

/// The folder under the data dir.
pub const DIR: &str = "scratch-media";

/// `<data_dir>/scratch-media`.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR)
}

/// The file extension for an image MIME type (`None` = not an image we keep).
pub fn ext_for(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        _ => return None,
    })
}

/// The MIME type of a stored name, from its extension.
pub fn mime_for(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// A stored name is `<64 lowercase hex>.<ext>`: nothing else is accepted, so
/// a reference can't point outside the folder.
pub fn valid_name(name: &str) -> bool {
    let Some((hash, ext)) = name.split_once('.') else {
        return false;
    };
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && matches!(ext, "png" | "jpg" | "gif" | "webp" | "svg")
}

/// Copy `bytes` into `dir` (idempotent) and return the markdown URL,
/// `blyg-local:<sha256>.<ext>`. Local: no network.
pub fn store(dir: &Path, bytes: &[u8], mime: &str) -> Result<String> {
    let ext = ext_for(mime)
        .ok_or_else(|| CoreError::Other(format!("{mime} isn't an image type Blygger can keep")))?;
    let name = format!("{}.{ext}", crate::util::hex(&crate::util::sha256(bytes)));
    let path = dir.join(&name);
    if !path.exists() {
        let io = |e: std::io::Error| CoreError::Storage(e.to_string());
        std::fs::create_dir_all(dir).map_err(io)?;
        let tmp = path.with_extension("part");
        std::fs::write(&tmp, bytes).map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(io)?;
    }
    Ok(format!("{SCHEME}{name}"))
}

/// The stored name of a `blyg-local:` URL (`None` for anything else).
pub fn name_of(url: &str) -> Option<&str> {
    let name = url.strip_prefix(SCHEME)?.trim_start_matches('/');
    valid_name(name).then_some(name)
}

/// The file behind a `blyg-local:` URL, if it's a valid name and exists.
pub fn file(dir: &Path, url: &str) -> Option<PathBuf> {
    let path = dir.join(name_of(url)?);
    path.is_file().then_some(path)
}

/// Every distinct local image the text refers to, in order of appearance.
pub fn refs(md: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = md;
    while let Some(i) = rest.find(SCHEME) {
        let tail = &rest[i + SCHEME.len()..];
        let end = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
            .unwrap_or(tail.len());
        let name = &tail[..end];
        if valid_name(name) && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
        rest = &tail[end..];
    }
    out
}

/// Whether the text still refers to any local image.
pub fn has_refs(md: &str) -> bool {
    md.contains(SCHEME) && !refs(md).is_empty()
}

/// Replace `blyg-local:<name>` with its uploaded URL, for each `(name, url)`.
pub fn rewrite(md: &str, uploaded: &[(String, String)]) -> String {
    let mut out = md.to_string();
    for (name, url) in uploaded {
        out = out.replace(&format!("{SCHEME}{name}"), url);
    }
    out
}

/// An uploaded `media/…` URL made absolute against the blyg's public origin
/// (the Worker renders a relative one page-relative, which 404s under
/// `/f/<id>/`). Absolute URLs pass through.
pub fn absolute(url: &str, origin: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    format!(
        "{}/{}",
        origin.trim_end_matches('/'),
        url.trim_start_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_by_hash_and_finds_refs() {
        let d = tempfile::tempdir().unwrap();
        let url = store(d.path(), b"\x89PNG fake", "image/png").unwrap();
        assert!(
            url.starts_with("blyg-local:") && url.ends_with(".png"),
            "{url}"
        );
        assert_eq!(store(d.path(), b"\x89PNG fake", "image/png").unwrap(), url);
        let path = file(d.path(), &url).expect("stored");
        assert_eq!(std::fs::read(path).unwrap(), b"\x89PNG fake");
        assert!(store(d.path(), b"x", "text/plain").is_err());

        let name = name_of(&url).unwrap().to_string();
        let md = format!("a ![]({url}) b ![x]({url})\n![](blyg-local:../etc/passwd)");
        assert_eq!(refs(&md), vec![name.clone()]);
        assert!(has_refs(&md));
        let out = rewrite(
            &md,
            &[(name, "https://blyg.example.com/media/M1.png".into())],
        );
        assert!(!has_refs(&out));
        assert_eq!(
            out.matches("https://blyg.example.com/media/M1.png").count(),
            2
        );
        assert!(file(d.path(), "blyg-local:../etc/passwd").is_none());
    }

    #[test]
    fn absolute_urls() {
        assert_eq!(
            absolute("media/a.png", "https://blyg.example.com/"),
            "https://blyg.example.com/media/a.png"
        );
        assert_eq!(
            absolute("https://x.example/a.png", "o"),
            "https://x.example/a.png"
        );
    }
}
