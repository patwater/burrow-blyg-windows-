//! --- browser --- The in-app browser pane (docs/SPEC.md § In-app browser).
//!
//! A link clicked in a post opens here instead of the default browser: a
//! slide-over from the right (about two thirds of the window) over the
//! reading view, or the whole reading area with ⌘-click; ⌥-click goes to the
//! default browser. `open-links = browser` flips that. esc closes the pane
//! and it keeps its page if reopened (⇧⌘B, or the same link).
//!
//! - `surface`: its own WKWebView (no bridge to the app, its own cookie jar,
//!   http(s) only); `nav_policy` below is the URL/scheme policy.
//! - `blocklist` + `rules`: uBlock Origin's default lists as WebKit content
//!   blockers, per-site shield in `state.json`, `content-blocking` config.
//! - `view`: the GPUI chrome and the `MainView` hooks. Other screens open a
//!   link with [`MainView::open_link`] (click modifiers decide) or
//!   [`MainView::open_url_in_app`].
//!
//! The web view is made on first use and dropped (freeing its WebContent
//! process) [`TEARDOWN_AFTER`] after the pane closes.

pub mod blocklist;
#[cfg(all(target_os = "macos", not(test)))]
mod driver;
#[cfg(all(target_os = "macos", not(test)))]
mod rules;
pub mod surface;
mod view;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::time::Duration;

pub use view::{
    Browser, BrowserAddress, BrowserBack, BrowserForward, BrowserReload, ToggleBrowser,
};

/// How long a closed pane keeps its web view (and page) alive.
pub const TEARDOWN_AFTER: Duration = Duration::from_secs(180);

/// [`TEARDOWN_AFTER`], or `BLYGGER_BROWSER_TEARDOWN_MS` in debug builds
/// (smoke tests measure the freed process without waiting minutes).
pub fn teardown_after() -> Duration {
    if cfg!(debug_assertions)
        && let Some(ms) = std::env::var("BLYGGER_BROWSER_TEARDOWN_MS")
            .ok()
            .and_then(|v| v.parse().ok())
    {
        return Duration::from_millis(ms);
    }
    TEARDOWN_AFTER
}

/// How long the first page waits for the compiled block lists before it
/// loads without them.
pub const RULES_WAIT: Duration = Duration::from_millis(400);

/// How the pane opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenMode {
    /// From the right, over the reading view (plain click).
    #[default]
    Slide,
    /// The whole reading area (⌘-click).
    Full,
}

/// `open-links`: where a clicked link goes by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenLinks {
    /// The browser pane (⌥-click: the default browser).
    #[default]
    App,
    /// The default browser (⌥-click: the pane).
    Browser,
}

impl OpenLinks {
    pub fn from_value(v: Option<&str>) -> Self {
        match v {
            Some("browser") => OpenLinks::Browser,
            _ => OpenLinks::App,
        }
    }
}

/// What a click on a link does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkAction {
    Pane(OpenMode),
    DefaultBrowser,
}

/// The click's modifiers and `open-links` decide where a link opens.
pub fn link_action(pref: OpenLinks, cmd: bool, alt: bool) -> LinkAction {
    match (pref, cmd, alt) {
        (_, true, _) => LinkAction::Pane(OpenMode::Full),
        (OpenLinks::App, false, false) | (OpenLinks::Browser, false, true) => {
            LinkAction::Pane(OpenMode::Slide)
        }
        _ => LinkAction::DefaultBrowser,
    }
}

/// What the pane's web view may do with a URL it's asked to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nav {
    /// http(s), `about:blank` and `about:srcdoc` (iframes).
    Allow,
    /// `mailto:`: the system's mail handler.
    System(String),
    /// Everything else: `file:`, `data:`, `javascript:`, `blob:`, custom
    /// schemes.
    Deny,
}

pub fn nav_policy(url: &str) -> Nav {
    let lower = url.trim().to_ascii_lowercase();
    if lower == "about:blank" || lower == "about:srcdoc" {
        return Nav::Allow;
    }
    if lower.starts_with("mailto:") {
        return Nav::System(url.trim().to_string());
    }
    match url::Url::parse(url.trim()) {
        Ok(u) if matches!(u.scheme(), "http" | "https") && u.host().is_some() => Nav::Allow,
        _ => Nav::Deny,
    }
}

/// Only http(s) URLs with a host are shown in the pane.
pub fn is_web_url(url: &str) -> bool {
    nav_policy(url) == Nav::Allow && !url.trim().to_ascii_lowercase().starts_with("about:")
}

/// What the address field's text means: a URL to load (http(s) only),
/// adding `https://` to a bare host (`example.com/page`).
pub fn parse_address(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.chars().any(char::is_whitespace) {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return is_web_url(t).then(|| t.to_string());
    }
    // `localhost:8080` has a "scheme" as far as a URL parser is concerned;
    // anything else with `scheme:` (mailto:, file:, javascript:) isn't ours.
    let host_part = t.split(['/', '?', '#']).next().unwrap_or("");
    // `host:port` is fine; `mailto:ada@example.com` isn't a host.
    if let Some((_, port)) = host_part.split_once(':')
        && !(!port.is_empty() && port.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let looks_like_host = host_part.contains('.')
        || host_part.eq_ignore_ascii_case("localhost")
        || host_part.to_ascii_lowercase().starts_with("localhost:");
    if t.contains("://") || !looks_like_host {
        return None;
    }
    let with = format!("https://{t}");
    is_web_url(&with).then_some(with)
}

/// The host whose shield a page is under (`www.` kept: it's what the user
/// sees).
pub fn host_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    u.host_str().map(|h| h.to_ascii_lowercase())
}

/// Blocking applies to `url` unless it's off everywhere or for its host.
pub fn blocking_applies(
    global: bool,
    unblocked: &std::collections::BTreeSet<String>,
    url: &str,
) -> bool {
    global && host_of(url).is_none_or(|h| !unblocked.contains(&h))
}

/// `[title](url)` for the notes drawer (and, for now, the clipboard).
pub fn notes_link(title: &str, url: &str) -> String {
    let title = title.trim();
    let title = if title.is_empty() { url } else { title };
    let mut t = String::with_capacity(title.len());
    for c in title.chars() {
        match c {
            '[' | ']' | '\\' => {
                t.push('\\');
                t.push(c);
            }
            '\n' | '\r' => t.push(' '),
            c => t.push(c),
        }
    }
    let u = url
        .replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29");
    format!("[{t}]({u})")
}

/// What the address field shows when it isn't being edited: the title, or
/// the URL without its scheme.
pub fn display_label(title: &str, url: &str) -> String {
    let t = title.trim();
    if !t.is_empty() {
        return t.to_string();
    }
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_string()
}

/// The shared compiled lists: what the web view attaches, and a generation
/// that changes when they do.
pub mod rules_handle {
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    pub struct RuleLists {
        /// 0 until the first lists are ready.
        pub generation: u64,
        /// Rules across all lists.
        pub rules: usize,
        #[cfg(all(target_os = "macos", not(test)))]
        pub lists: Vec<super::rules::RuleList>,
    }

    impl RuleLists {
        pub fn ready(&self) -> bool {
            self.generation > 0
        }
    }

    pub type Handle = Rc<RefCell<RuleLists>>;
}
