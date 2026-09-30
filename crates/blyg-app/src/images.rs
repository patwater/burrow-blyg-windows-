//! Remote images for the preview (`media/…` on the blyg, or any http(s)
//! image in a post): fetched once, in the background, with blyg-core's
//! token-less public client, and kept in a disk cache under
//! `<data dir>/image-cache/`. Fresh uploads are put in the cache directly,
//! so they show without a round trip.
//!
//! The preview asks `state(url)`: `Ready` renders from the cached file,
//! `Loading` shows a placeholder and starts a fetch (the `on_loaded` hook
//! then asks the window to repaint), `Failed` shows the alt text.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Refuse anything bigger (the Worker's own upload cap is 5 MB).
const MAX_BYTES: u64 = 20 * 1024 * 1024;

pub fn cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("image-cache")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageState {
    Ready(PathBuf),
    Loading,
    Failed,
}

type Hook = Box<dyn Fn() + Send + Sync>;

struct Images {
    dir: PathBuf,
    inflight: Mutex<HashSet<String>>,
    failed: Mutex<HashSet<String>>,
    on_loaded: Mutex<Option<std::sync::Arc<Hook>>>,
}

static IMAGES: OnceLock<Images> = OnceLock::new();

/// Turn the loader on, caching under `<data_dir>/image-cache`. Until then
/// (headless tests) nothing is fetched or written: images show alt text.
pub fn init(data_dir: &Path) {
    let _ = IMAGES.set(Images {
        dir: cache_dir(data_dir),
        inflight: Mutex::new(HashSet::new()),
        failed: Mutex::new(HashSet::new()),
        on_loaded: Mutex::new(None),
    });
}

fn images() -> Option<&'static Images> {
    IMAGES.get()
}

/// Called (from a background thread) whenever an image lands in the cache.
pub fn set_on_loaded(f: impl Fn() + Send + Sync + 'static) {
    if let Some(im) = images() {
        *im.on_loaded.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(std::sync::Arc::new(Box::new(f)));
    }
}

/// `<dir>/<sha256 of the URL>.<ext>`.
pub fn cache_path(dir: &Path, url: &str) -> PathBuf {
    let hash = blyg_core::content_hash(url);
    let hash = hash.trim_start_matches("sha256:");
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let ext = path
        .rsplit('/')
        .next()
        .and_then(|f| f.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg"))
        .unwrap_or_else(|| "img".into());
    dir.join(format!("{hash}.{ext}"))
}

/// Where `url` stands; starts a background fetch the first time.
pub fn state(url: &str) -> ImageState {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return ImageState::Failed;
    }
    let Some(im) = images() else {
        return ImageState::Failed;
    };
    let path = cache_path(&im.dir, url);
    if path.exists() {
        return ImageState::Ready(path);
    }
    if im
        .failed
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains(url)
    {
        return ImageState::Failed;
    }
    if !im
        .inflight
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(url.to_string())
    {
        return ImageState::Loading;
    }
    let url = url.to_string();
    std::thread::spawn(move || {
        let ok = fetch_into(&url, &path).is_ok();
        im.inflight
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&url);
        if !ok {
            im.failed
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(url);
        }
        let hook = im
            .on_loaded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(h) = hook {
            h();
        }
    });
    ImageState::Loading
}

fn fetch_into(url: &str, path: &Path) -> std::io::Result<()> {
    let (bytes, mime) = blyg_core::api::public::PublicClient::new()
        .get_bytes(url, MAX_BYTES)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    if !mime.as_deref().unwrap_or("image/").starts_with("image/") {
        return Err(std::io::Error::other("not an image"));
    }
    write_atomic(path, &bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_paths_are_stable_and_typed() {
        let d = Path::new("/c");
        let a = cache_path(d, "https://blyg.example.com/media/abc.png");
        assert_eq!(a, cache_path(d, "https://blyg.example.com/media/abc.png"));
        assert_eq!(a.extension().unwrap(), "png");
        assert_ne!(a, cache_path(d, "https://blyg.example.com/media/abd.png"));
        assert_eq!(
            cache_path(d, "https://x.example/pic?size=2")
                .extension()
                .unwrap(),
            "img"
        );
        assert_eq!(
            state("media/abc.png"),
            ImageState::Failed,
            "relative: not fetchable"
        );
    }
}
