//! WebKit's side of content blocking (macOS only): compiled
//! `WKContentRuleList`s in a `WKContentRuleListStore` under the app's data
//! dir. WebKit compiles on its own queue and memory-maps the result, so a
//! cached list is ready in milliseconds on the next launch.
//!
//! Every call here is made on the main thread (GPUI's foreground); WebKit
//! calls the completion handlers there too, and they hand their result to
//! an `async_channel` that the foreground task awaits.

use std::path::Path;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_foundation::{NSError, NSString, NSURL};
use objc2_web_kit::{WKContentRuleList, WKContentRuleListStore};

pub type RuleList = Retained<WKContentRuleList>;

/// The store for compiled lists in `dir` (created if missing).
pub fn open_store(dir: &Path) -> Option<Retained<WKContentRuleListStore>> {
    let mtm = MainThreadMarker::new()?;
    std::fs::create_dir_all(dir).ok()?;
    let path = NSString::from_str(&dir.to_string_lossy());
    let url = NSURL::fileURLWithPath_isDirectory(&path, true);
    // SAFETY: main thread; `url` is a file URL to a directory we own.
    unsafe { WKContentRuleListStore::storeWithURL(Some(&url), mtm) }
}

fn error_text(err: *mut NSError) -> String {
    // SAFETY: WebKit passes a valid NSError or nil.
    match unsafe { err.as_ref() } {
        Some(e) => e.localizedDescription().to_string(),
        None => "unknown error".into(),
    }
}

/// A list compiled earlier, if the store still has it.
pub async fn lookup(store: &WKContentRuleListStore, id: &str) -> Option<RuleList> {
    let (tx, rx) = async_channel::bounded::<Option<RuleList>>(1);
    let block = RcBlock::new(move |list: *mut WKContentRuleList, _err: *mut NSError| {
        // SAFETY: a valid list or nil; retained before WebKit lets go of it.
        let list = unsafe { Retained::retain(list) };
        let _ = tx.try_send(list);
    });
    // SAFETY: main thread; the block lives until WebKit calls it.
    unsafe {
        store.lookUpContentRuleListForIdentifier_completionHandler(
            Some(&NSString::from_str(id)),
            Some(&block),
        );
    }
    rx.recv().await.ok().flatten()
}

/// Compile `json` (content-blocker rules) under `id`, replacing any list
/// with that id. The error is WebKit's own sentence.
pub async fn compile(
    store: &WKContentRuleListStore,
    id: &str,
    json: &str,
) -> Result<RuleList, String> {
    let (tx, rx) = async_channel::bounded::<Result<RuleList, String>>(1);
    let block = RcBlock::new(move |list: *mut WKContentRuleList, err: *mut NSError| {
        // SAFETY: as in `lookup`.
        let out = match unsafe { Retained::retain(list) } {
            Some(l) => Ok(l),
            None => Err(error_text(err)),
        };
        let _ = tx.try_send(out);
    });
    // SAFETY: main thread; WebKit copies the string before returning.
    unsafe {
        store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
            Some(&NSString::from_str(id)),
            Some(&NSString::from_str(json)),
            Some(&block),
        );
    }
    rx.recv()
        .await
        .unwrap_or_else(|_| Err("the compiler went away".into()))
}

/// Forget a compiled list (fire and forget).
pub fn remove(store: &WKContentRuleListStore, id: &str) {
    let block = RcBlock::new(|_err: *mut NSError| {});
    // SAFETY: main thread.
    unsafe {
        store.removeContentRuleListForIdentifier_completionHandler(
            Some(&NSString::from_str(id)),
            Some(&block),
        );
    }
}

/// Every identifier in the store (to sweep lists no manifest names).
pub async fn identifiers(store: &WKContentRuleListStore) -> Vec<String> {
    let (tx, rx) = async_channel::bounded::<Vec<String>>(1);
    let block = RcBlock::new(move |ids: *mut objc2_foundation::NSArray<NSString>| {
        // SAFETY: a valid array or nil.
        let v = match unsafe { ids.as_ref() } {
            Some(a) => a.iter().map(|s| s.to_string()).collect(),
            None => Vec::new(),
        };
        let _ = tx.try_send(v);
    });
    // SAFETY: main thread.
    unsafe { store.getAvailableContentRuleListIdentifiers(Some(&block)) };
    rx.recv().await.unwrap_or_default()
}
