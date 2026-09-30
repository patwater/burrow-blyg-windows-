//! In-app updates (`auto-update = install | notify | off`, docs/SPEC.md §
//! Updates).
//!
//! At launch (after a short delay) and then about once a day, the app asks
//! GitHub for the latest release. A newer one is downloaded only from GitHub
//! over HTTPS, and installed only if `SHA256SUMS` carries a valid Ed25519
//! signature from the embedded release key, the zip matches its signed
//! SHA-256, and the extracted bundle is `org.blygger.desktop`, strictly
//! newer, and passes `codesign --verify`. It replaces the running bundle on
//! "Restart to update", or when the app quits.
//!
//! The pieces: `check` (release JSON, versions, throttling), `verify`
//! (signature and hashes), `net` (the one HTTP trait), `install` (staging,
//! bundle checks, swap and rollback). This file is the GPUI glue: a global
//! holding the current phase, the schedule, the menu action and the status
//! bar notice (drawn by `app/update_view`).

pub mod check;
pub mod install;
pub mod net;
pub mod verify;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use gpui_kit::{App, AppContext as _, AsyncApp, Global};
use semver::Version;

use crate::prefs::AutoUpdate;
use check::{Assets, CheckOutcome, Offer};
use install::{Staged, Tools};
use net::Http;

gpui_kit::actions!(blygger, [CheckForUpdates]);

/// Why an update step failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// Offline, rate limited, an unexpected response: try again later.
    Network(String),
    /// The release didn't pass a signature, hash or bundle check.
    Verify(String),
    /// Extracting or moving files failed.
    Install(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Network(s) | UpdateError::Verify(s) | UpdateError::Install(s) => {
                f.write_str(s)
            }
        }
    }
}

/// `SHA256SUMS` and its signature are tiny; the zip is ~30 MB today.
const MAX_SUMS: u64 = 64 * 1024;
const MAX_SIG: u64 = 1024;
const MAX_ZIP: u64 = 512 * 1024 * 1024;
const LAUNCH_DELAY: Duration = Duration::from_secs(15);

/// Everything a download needs, cloneable onto a background thread.
#[derive(Clone)]
pub struct Env {
    pub http: Arc<dyn Http>,
    pub tools: Arc<dyn Tools>,
    pub key: VerifyingKey,
    pub api_url: String,
    pub current: Version,
    /// Where staging goes when it's on the bundle's volume.
    pub tmp_root: PathBuf,
}

/// Download `SHA256SUMS`, its signature and the zip; verify all three; then
/// extract and check the bundle next to `target`.
pub fn download_and_stage(
    env: &Env,
    version: &Version,
    assets: &Assets,
    target: &Path,
) -> Result<Staged, UpdateError> {
    let sums = env.http.get(&assets.sums_url, MAX_SUMS)?;
    let sig = env.http.get(&assets.sig_url, MAX_SIG)?;
    verify::verify_sums_signature(&env.key, &sums, &sig)?;
    let sums = std::str::from_utf8(&sums)
        .map_err(|_| UpdateError::Verify("SHA256SUMS isn't text".into()))?;
    // Fail before the big download if the zip isn't listed.
    verify::expected_hash(sums, &assets.zip_name)?;
    let zip = env.http.get(&assets.zip_url, MAX_ZIP)?;
    verify::verify_hash(sums, &assets.zip_name, &zip)?;
    install::stage(
        &zip,
        target,
        &env.tmp_root,
        version,
        &env.current,
        &*env.tools,
    )
}

/// Where the updater is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Checking {
        manual: bool,
    },
    /// Newer release found; `blocked` says why it can't be installed from
    /// here (then the notice links to the release page instead).
    Available {
        offer: Offer,
        blocked: Option<String>,
    },
    Downloading {
        offer: Offer,
        /// The user asked for it (show progress); otherwise it's quiet.
        shown: bool,
    },
    Ready {
        offer: Offer,
        staged: Staged,
    },
}

/// What the status bar shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub action: Option<(&'static str, NoticeAction)>,
    /// (label, release page URL)
    pub link: Option<(&'static str, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeAction {
    Restart,
    Download,
}

impl Phase {
    pub fn notice(&self) -> Option<Notice> {
        match self {
            Phase::Idle | Phase::Checking { .. } => None,
            Phase::Downloading { shown: false, .. } => None,
            Phase::Downloading { offer, shown: true } => Some(Notice {
                text: format!("Downloading Blygger {}…", offer.version),
                action: None,
                link: None,
            }),
            Phase::Available {
                offer,
                blocked: None,
            } => Some(Notice {
                text: format!("Blygger {} is available", offer.version),
                action: Some(("Download", NoticeAction::Download)),
                link: Some(("What's new", offer.page_url.clone())),
            }),
            Phase::Available {
                offer,
                blocked: Some(why),
            } => Some(Notice {
                text: format!("Blygger {} is available · {why}", offer.version),
                action: None,
                link: Some(("Release page", offer.page_url.clone())),
            }),
            Phase::Ready { offer, .. } => Some(Notice {
                text: format!("Blygger {} is ready", offer.version),
                action: Some(("Restart to update", NoticeAction::Restart)),
                link: Some(("What's new", offer.page_url.clone())),
            }),
        }
    }
}

/// The "can't install" explanation for the notice.
pub fn cant_update_in_place(why: &str) -> String {
    format!("Can't update in place: {why}; download from the release page")
}

/// Whether update checks may run at all: never in tests, fake mode, or with
/// `BLYGGER_NO_UPDATE=1`.
pub fn disabled_reason() -> Option<&'static str> {
    if cfg!(test) {
        return Some("test build");
    }
    if std::env::var_os("BLYGGER_FAKE").is_some() {
        return Some("BLYGGER_FAKE is set");
    }
    if std::env::var_os("BLYGGER_NO_UPDATE").is_some() {
        return Some("BLYGGER_NO_UPDATE is set");
    }
    None
}

pub struct Updater {
    pub phase: Phase,
    env: Env,
    /// The running bundle, or why it can't be updated in place.
    target: Result<PathBuf, String>,
    data_dir: PathBuf,
    /// Unix seconds when the next automatic check is due.
    next_due: u64,
}

impl Global for Updater {}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The real environment. Debug builds only: `BLYGGER_UPDATE_URL` points the
/// check at a local test server (plain HTTP to localhost allowed), and
/// `BLYGGER_UPDATE_PUBKEY` swaps in a test key. Release builds ignore both.
fn real_env() -> Env {
    let (api_url, key, allow_local) = debug_overrides();
    Env {
        http: Arc::new(net::UreqHttp::new(allow_local)),
        tools: Arc::new(install::System),
        key,
        api_url,
        current: check::current_version(),
        tmp_root: std::env::temp_dir(),
    }
}

#[cfg(debug_assertions)]
fn debug_overrides() -> (String, VerifyingKey, bool) {
    let url = std::env::var("BLYGGER_UPDATE_URL").ok();
    let key = std::env::var("BLYGGER_UPDATE_PUBKEY")
        .ok()
        .and_then(|k| verify::parse_public_key(&k).ok())
        .unwrap_or_else(verify::release_key);
    let allow_local = url.is_some();
    (
        url.unwrap_or_else(|| check::LATEST_URL.into()),
        key,
        allow_local,
    )
}

#[cfg(not(debug_assertions))]
fn debug_overrides() -> (String, VerifyingKey, bool) {
    (check::LATEST_URL.into(), verify::release_key(), false)
}

/// Install the global, the schedule, the actions and the quit hook. Does
/// nothing (beyond the menu action's explanation) in dev/test builds.
pub fn init(data_dir: PathBuf, cx: &mut App) {
    cx.on_action(|_: &CheckForUpdates, cx| check_now(cx));
    if disabled_reason().is_some() {
        return;
    }
    let env = real_env();
    let target = install::running_bundle(&*env.tools).map_err(|why| cant_update_in_place(&why));
    let last = blyg_core::state::AppState::load(&data_dir).last_update_check;
    cx.set_global(Updater {
        phase: Phase::Idle,
        env,
        target,
        data_dir,
        next_due: check::first_due(now_secs(), last),
    });
    cx.on_app_quit(|cx| {
        install_on_quit(cx);
        async {}
    })
    .detach();
    cx.spawn(async move |cx: &mut AsyncApp| {
        cx.background_executor().timer(LAUNCH_DELAY).await;
        loop {
            let wait = cx.update(tick);
            cx.background_executor().timer(wait).await;
        }
    })
    .detach();
}

/// Start an automatic check if one is due; returns how long to sleep.
fn tick(cx: &mut App) -> Duration {
    let hour = Duration::from_secs(60 * 60);
    let Some(u) = cx.try_global::<Updater>() else {
        return hour;
    };
    let now = now_secs();
    if now < u.next_due {
        return Duration::from_secs(u.next_due - now).clamp(Duration::from_secs(60), hour);
    }
    let idle = matches!(u.phase, Phase::Idle | Phase::Available { .. });
    if idle && crate::settings::prefs(cx).auto_update != AutoUpdate::Off {
        start_check(false, cx);
    } else {
        cx.global_mut::<Updater>().next_due = now + check::INTERVAL_SECS;
    }
    hour
}

fn toast(text: impl Into<String>, cx: &mut App) {
    let text = text.into();
    if let Some(h) = crate::capture::main_window(cx) {
        let _ = h.update(cx, |v, _, cx| v.update_toast(text, cx));
    }
}

fn set_phase(phase: Phase, cx: &mut App) {
    cx.global_mut::<Updater>().phase = phase;
    cx.refresh_windows();
}

/// Blygger › Check for Updates…: always checks (whatever `auto-update`
/// says) and always answers.
pub fn check_now(cx: &mut App) {
    if let Some(why) = disabled_reason() {
        return toast(format!("Update checks are off ({why})"), cx);
    }
    let Some(phase) = cx.try_global::<Updater>().map(|u| u.phase.clone()) else {
        return;
    };
    match phase {
        Phase::Checking { .. } => toast("Already checking for updates…", cx),
        Phase::Downloading { offer, .. } => {
            // An automatic download becomes visible once asked about.
            let msg = format!("Downloading Blygger {}…", offer.version);
            set_phase(Phase::Downloading { offer, shown: true }, cx);
            toast(msg, cx);
        }
        Phase::Ready { offer, .. } => toast(
            format!("Blygger {} is ready · Restart to update", offer.version),
            cx,
        ),
        Phase::Idle | Phase::Available { .. } => start_check(true, cx),
    }
}

fn start_check(manual: bool, cx: &mut App) {
    let u = cx.global::<Updater>();
    let env = u.env.clone();
    set_phase(Phase::Checking { manual }, cx);
    let task =
        cx.background_spawn(async move { check::check(&*env.http, &env.api_url, &env.current) });
    cx.spawn(async move |cx: &mut AsyncApp| {
        let result = task.await;
        cx.update(|cx| on_checked(result, manual, cx));
    })
    .detach();
}

fn on_checked(result: Result<CheckOutcome, UpdateError>, manual: bool, cx: &mut App) {
    let now = now_secs();
    let mode = crate::settings::prefs(cx).auto_update;
    match result {
        Err(e) => {
            cx.global_mut::<Updater>().next_due = now + check::RETRY_SECS;
            set_phase(Phase::Idle, cx);
            eprintln!("blygger: update check failed: {e}");
            if manual {
                toast(format!("Couldn't check for updates: {e}"), cx);
            }
        }
        Ok(CheckOutcome::UpToDate { .. }) => {
            let u = cx.global_mut::<Updater>();
            u.next_due = now + check::INTERVAL_SECS;
            let _ = blyg_core::state::AppState::update(&u.data_dir, |s| {
                s.last_update_check = Some(now);
            });
            set_phase(Phase::Idle, cx);
            if manual {
                let msg = format!("You're up to date ({})", check::current_version());
                toast(msg, cx);
            }
        }
        Ok(CheckOutcome::Available(offer)) => {
            let u = cx.global_mut::<Updater>();
            // Not recorded in state.json: a relaunch looks again (and
            // quitting with an update ready installs it).
            u.next_due = now + check::INTERVAL_SECS;
            let blocked = blocked_reason(&u.target, &offer);
            let version = offer.version.clone();
            let install = blocked.is_none() && (manual || mode == AutoUpdate::Install);
            set_phase(Phase::Available { offer, blocked }, cx);
            if install {
                start_download(manual, cx);
            }
            if manual {
                toast(format!("Blygger {version} is available"), cx);
            }
        }
    }
}

/// Why `offer` can't be installed from here, if it can't.
pub fn blocked_reason(target: &Result<PathBuf, String>, offer: &Offer) -> Option<String> {
    match (target, &offer.assets) {
        (Err(why), _) => Some(why.clone()),
        (Ok(_), None) => Some(
            "this release isn't signed for in-app updates; download it from the release page"
                .to_string(),
        ),
        (Ok(_), Some(_)) => None,
    }
}

/// The notice's "Download" (notify mode): proceed as `install` would.
pub fn download_now(cx: &mut App) {
    if cx
        .try_global::<Updater>()
        .is_some_and(|u| matches!(u.phase, Phase::Available { blocked: None, .. }))
    {
        start_download(true, cx);
    }
}

fn start_download(shown: bool, cx: &mut App) {
    let u = cx.global::<Updater>();
    let (Phase::Available { offer, .. }, Ok(target)) = (&u.phase, &u.target) else {
        return;
    };
    let Some(assets) = offer.assets.clone() else {
        return;
    };
    let (offer, target, env) = (offer.clone(), target.clone(), u.env.clone());
    set_phase(
        Phase::Downloading {
            offer: offer.clone(),
            shown,
        },
        cx,
    );
    let version = offer.version.clone();
    let task =
        cx.background_spawn(async move { download_and_stage(&env, &version, &assets, &target) });
    cx.spawn(async move |cx: &mut AsyncApp| {
        let result = task.await;
        cx.update(|cx| on_downloaded(offer, result, cx));
    })
    .detach();
}

fn on_downloaded(offer: Offer, result: Result<Staged, UpdateError>, cx: &mut App) {
    let shown = matches!(
        cx.global::<Updater>().phase,
        Phase::Downloading { shown: true, .. }
    );
    let phase = match result {
        Ok(staged) => {
            eprintln!("blygger: update {} verified and ready", offer.version);
            Phase::Ready { offer, staged }
        }
        Err(UpdateError::Network(e)) => {
            eprintln!("blygger: update download failed: {e}");
            if shown {
                toast(format!("Couldn't download the update: {e}"), cx);
            }
            cx.global_mut::<Updater>().next_due = now_secs() + check::RETRY_SECS;
            Phase::Idle
        }
        Err(e) => {
            eprintln!("blygger: update refused: {e}");
            Phase::Available {
                offer,
                blocked: Some(format!(
                    "the update didn't pass its checks ({e}); download from the release page"
                )),
            }
        }
    };
    set_phase(phase, cx);
    #[cfg(debug_assertions)]
    smoke_hook(cx);
}

/// Debug builds only: `BLYGGER_UPDATE_SMOKE=restart|quit` acts on a ready
/// update by itself, for manual smoke tests against a local test server
/// (`BLYGGER_UPDATE_URL`) without any synthetic input.
#[cfg(debug_assertions)]
fn smoke_hook(cx: &mut App) {
    if !matches!(cx.global::<Updater>().phase, Phase::Ready { .. }) {
        return;
    }
    match std::env::var("BLYGGER_UPDATE_SMOKE").as_deref() {
        Ok("restart") => cx.defer(restart_to_update),
        Ok("quit") => cx.defer(|cx| cx.quit()),
        _ => {}
    }
}

/// "Restart to update": install the staged bundle now, then reopen it once
/// this process has exited.
pub fn restart_to_update(cx: &mut App) {
    let Some(u) = cx.try_global::<Updater>() else {
        return;
    };
    let (Phase::Ready { offer, staged }, Ok(target)) = (&u.phase, &u.target) else {
        return;
    };
    let (offer, staged, target) = (offer.clone(), staged.clone(), target.clone());
    match install::install_staged(&target, &staged, &u.env.current, &*u.env.tools) {
        Ok(()) => {
            set_phase(Phase::Idle, cx);
            if let Err(e) = install::relaunch_after_exit(&target) {
                eprintln!("blygger: couldn't schedule the relaunch: {e}");
            }
            cx.quit();
        }
        Err(e) => {
            eprintln!("blygger: update install failed: {e}");
            toast(format!("Couldn't install the update: {e}"), cx);
            set_phase(
                Phase::Available {
                    offer,
                    blocked: Some(cant_update_in_place(&e.to_string())),
                },
                cx,
            );
        }
    }
}

/// Quitting with an update staged installs it (no relaunch).
fn install_on_quit(cx: &mut App) {
    let Some(u) = cx.try_global::<Updater>() else {
        return;
    };
    if let (Phase::Ready { staged, .. }, Ok(target)) = (&u.phase, &u.target)
        && let Err(e) = install::install_staged(target, staged, &u.env.current, &*u.env.tools)
    {
        eprintln!("blygger: update install on quit failed: {e}");
    }
}

/// The status bar notice, if any (no global in tests and dev builds).
pub fn notice(cx: &App) -> Option<Notice> {
    cx.try_global::<Updater>().and_then(|u| u.phase.notice())
}
