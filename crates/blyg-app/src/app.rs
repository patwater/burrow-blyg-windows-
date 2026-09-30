//! The main window: omnibar, list, editor, status bar, sheets and toasts.
//! Behaviour decisions live in `vm`; this file wires them to GPUI.

use std::sync::Arc;
use std::time::Duration;

use blyg_core::{Backend, CoreError, CoreEvent, Item, Kind, LocalId, Resolution, SyncStatus};
use gpui_kit::base::input::{
    Enter, Escape, InputEvent, InputState, MoveDown, MoveUp, Paste, TextareaState,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::fake::FakeBackend;
use crate::prefs::{self, LayoutPref, Prefs, ThemePref};
use crate::theme::Palette;
use crate::vm::{self, DiffOp, EnterAction, ListModel, PublishDecision};

gpui_kit::actions!(
    blygger,
    [
        FocusSearch,
        Publish,
        ToggleKind,
        TogglePreview,
        OpenPermalink,
        OpenSettings,
        FontBigger,
        FontSmaller,
        FontReset,
        NewDraft,
        Quit,
        FakeToggleOffline,
        FakeConflict,
        ShowCapture,
        OpenConfigFile,
        ReloadConfig,
        Disconnect,
    ]
);

#[cfg(test)]
#[path = "ui_tests.rs"]
mod ui_tests;

#[path = "demo.rs"]
mod demo;

#[path = "scratch.rs"]
pub(crate) mod scratch;
// --- AI --- (the window's AI UI; logic in crate::ai)
#[path = "ai/view.rs"]
mod ai_view;
// --- full editor ---
#[path = "studio/mod.rs"]
pub(crate) mod studio;
// --- reading & versions: the screens live in reading/ (a child module so
// they can extend MainView); the hooks below are marked the same way.
#[path = "reading/mod.rs"]
pub(crate) mod reading;
// --- onboarding --- (first-run flow + the tutorial; hooks marked the same way)
#[path = "onboarding/mod.rs"]
pub(crate) mod onboarding;
// --- buttons --- the optional toolbar (show-buttons)
#[path = "toolbar.rs"]
pub(crate) mod toolbar;
// --- profiles --- (the profile sheet; hooks marked the same way)
#[path = "profiles/mod.rs"]
pub(crate) mod profiles;
// --- delete & withdraw --- (drafts are deleted, published posts withdrawn)
#[path = "discard.rs"]
pub(crate) mod discard;
// --- auto-update --- (the status-bar notice; logic in crate::update)
#[path = "update/view.rs"]
mod update_view;

pub const CONTEXT: &str = "Blygger";

/// Every binding lives in `keymap::table()`.
pub fn bind_keys(cx: &mut App) {
    crate::keymap::bind_keys(cx);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Search,
    Edit,
}

enum Sheet {
    Publish {
        id: LocalId,
        title: String,
        next_version: u32,
        note: Entity<InputState>,
    },
    Conflict {
        id: LocalId,
        mine: Vec<(DiffOp, String)>,
        theirs: Vec<(DiffOp, String)>,
        focus: FocusHandle,
    },
    Settings {
        hotkey: Entity<InputState>,
        hotkey_error: Option<String>,
        focus: FocusHandle,
    },
    /// First run (no `blyg-url`): URL + owner token, verified with
    /// `GET /api/items`, then → Keychain → config.
    Connect {
        url: Entity<InputState>,
        token: Entity<InputState>,
        error: Option<String>,
        /// The check is running.
        busy: bool,
    },
    /// Forget this blyg: `1` keeps the local copy, `2` deletes it too.
    Disconnect { host: String, focus: FocusHandle },
    // --- delete & withdraw --- (discard.rs)
    /// ⇧⌘⌫ on a draft or scratch note: `⏎` deletes it, `esc` keeps it.
    DeleteDraft {
        id: LocalId,
        title: String,
        noun: &'static str,
        focus: FocusHandle,
    },
    /// Post › Withdraw… on a published post, with an optional note.
    Withdraw {
        id: LocalId,
        title: String,
        note: Entity<InputState>,
    },
}

struct Toast {
    id: usize,
    text: SharedString,
    sub: Option<SharedString>,
    leaving: bool,
}

pub struct MainView {
    backend: Arc<dyn Backend>,
    fake: Option<Arc<FakeBackend>>,
    focus: FocusHandle,
    omni: Entity<InputState>,
    editor: Entity<TextareaState>,
    list: ListModel,
    list_scroll: UniformListScrollHandle,
    current: Option<Item>,
    mode: Mode,
    loading_editor: bool,
    /// The full editor: view mode and the live preview (studio/mod.rs).
    studio: studio::Studio,
    sheet: Option<Sheet>,
    sheet_gen: usize,
    toast: Option<Toast>,
    toast_seq: usize,
    shake_gen: usize,
    sync: SyncStatus,
    /// The blyg origin for `media/…` links (`Backend::base_url`).
    base_url: Option<String>,
    publishing: bool,
    /// The first pull after connecting (or launching) is running.
    loading: bool,
    /// Reading-list posts not yet read (reader-local, never social).
    unread: usize,
    pub prefs: Prefs,
    /// What the config file says (`prefs` minus unsaved dev overrides);
    /// changes are written back as the difference from this.
    persisted: Prefs,
    /// Problems in the config file, shown as a dismissible banner.
    config_problems: Vec<blyg_core::config::Diagnostic>,
    problems_dismissed: bool,
    /// `Blygger — <host>` from the config's `blyg-url`.
    title: String,
    /// The title last given to the native window (Window menu, Mission Control).
    native_title: String,
    palette: Palette,
    now: chrono::DateTime<chrono::Utc>,
    first_frame: Option<std::time::Instant>,
    /// --- AI --- ⌘G jobs, the helper palette, proposals, Settings › AI.
    ai: ai_view::AiView,
    // --- reading & versions ---
    reading: reading::State,
    // --- onboarding ---
    onboarding: onboarding::State,
    // --- profiles ---
    profiles: profiles::State,
    _tasks: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

impl MainView {
    pub fn new(
        backend: Arc<dyn Backend>,
        fake: Option<Arc<FakeBackend>>,
        prefs: Prefs,
        launched: std::time::Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let omni = cx.new(|cx| InputState::new(window, cx).placeholder("Search or start writing…"));
        let editor = cx.new(|cx| TextareaState::new(window, cx).soft_wrap(true));
        let palette = Palette::resolve(prefs.theme, window.appearance());

        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(&omni, window, Self::on_omni_event));
        subs.push(cx.subscribe_in(&editor, window, Self::on_editor_event));
        subs.push(cx.observe_window_appearance(window, |this, window, cx| {
            this.palette = Palette::resolve(this.prefs.theme, window.appearance());
            cx.notify();
        }));
        // A WebView can end up holding (or dropping) the keyboard while the
        // window is in the background; typing must work when it comes back.
        subs.push(cx.observe_window_activation(window, |this, window, _| {
            if window.is_window_active() {
                this.studio.reclaim_keyboard();
            }
        }));

        // Core events arrive on a background thread; hop them onto the UI.
        let (tx, rx) = async_channel::unbounded::<CoreEvent>();
        backend.set_event_sink(Box::new(move |ev| {
            let _ = tx.try_send(ev);
        }));
        let mut tasks = Vec::new();
        tasks.push(cx.spawn_in(window, async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                if this
                    .update_in(cx, |v, window, cx| v.on_core_event(ev, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        // Keep relative times fresh.
        tasks.push(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                if this
                    .update(cx, |v, cx| {
                        v.now = chrono::Utc::now();
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));

        let sync = backend.sync_status();
        let base_url = backend.base_url();
        let persisted = crate::settings::prefs(cx);
        let config_problems = crate::settings::current_diagnostics(cx);
        let blyg_url = crate::settings::blyg_url(cx);
        let title = vm::window_title(blyg_url.as_deref());
        let notice = cx.global_mut::<crate::settings::AppConfig>().notice.take();
        // --- reading & versions ---
        let reading = reading::State::new(&*backend, cx);
        let mut this = Self {
            reading,
            onboarding: onboarding::State::new(cx), // --- onboarding ---
            profiles: Default::default(),           // --- profiles ---
            backend,
            fake,
            focus: cx.focus_handle(),
            omni,
            editor,
            list: ListModel::default(),
            list_scroll: UniformListScrollHandle::new(),
            current: None,
            mode: Mode::Search,
            loading_editor: false,
            studio: studio::Studio::new(
                cx.try_global::<crate::connection::Connection>()
                    .map(|c| c.data_dir.clone()),
            ),
            sheet: None,
            sheet_gen: 0,
            toast: None,
            toast_seq: 0,
            shake_gen: 0,
            sync,
            base_url,
            publishing: false,
            loading: false,
            unread: 0,
            prefs,
            persisted,
            config_problems,
            problems_dismissed: false,
            title,
            native_title: String::new(),
            palette,
            now: chrono::Utc::now(),
            first_frame: Some(launched),
            ai: Default::default(), // --- AI ---
            _tasks: tasks,
            _subs: subs,
        };
        let results = this.backend.search("");
        this.list.set_query("", results);
        this.load_selected(window, cx);
        this.studio_init(window, cx);
        this.omni.update(cx, |s, cx| s.focus(window, cx));
        if let Some(n) = notice {
            this.show_toast(n, None, cx);
        }
        this.unread = this
            .backend
            .reading()
            .iter()
            .filter(|r| r.is_unread())
            .count();
        let mode = crate::connection::mode(cx);
        if mode == Some(crate::connection::Mode::Live) {
            this.initial_sync(None, window, cx);
        }
        // --- onboarding --- (first launch, or tutorial-on-launch)
        if !this.onboarding_launch(blyg_url.is_some(), window, cx)
            && (mode == Some(crate::connection::Mode::Disconnected)
                || (blyg_url.is_none() && mode != Some(crate::connection::Mode::Fake)))
        {
            this.open_connect(window, cx);
        }
        this
    }

    /// Pull everything once, showing "loading your posts…" until it's done;
    /// the list fills in from SQLite as the pull lands.
    fn initial_sync(
        &mut self,
        connected_to: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.loading = true;
        cx.notify();
        let backend = self.backend.clone();
        let task = cx.background_spawn(async move {
            let r = backend.sync_now();
            (r, backend.items().len())
        });
        cx.spawn_in(window, async move |this, cx| {
            let (result, n) = task.await;
            let _ = this.update_in(cx, |v, window, cx| {
                v.loading = false;
                v.requery(window, cx);
                if std::env::var_os("BLYGGER_TIMING").is_some() {
                    println!("initial-sync items={n} ok={}", result.is_ok());
                }
                match (result, connected_to) {
                    (Ok(()), Some(host)) => v.show_toast(
                        format!("Connected to {host}"),
                        Some(
                            format!(
                                "{n} post{} loaded · the token is in your Keychain",
                                if n == 1 { "" } else { "s" }
                            )
                            .into(),
                        ),
                        cx,
                    ),
                    (Ok(()), None) => {}
                    (Err(CoreError::Offline), _) => v.show_toast(
                        "Offline: showing what's on this Mac",
                        Some("Changes sync when the blyg is reachable".into()),
                        cx,
                    ),
                    (Err(e), _) => {
                        v.show_toast(format!("Couldn't load from the blyg: {e}"), None, cx)
                    }
                }
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ config

    /// Write whatever changed in `prefs` back to the config file.
    fn save_prefs(&mut self, cx: &mut Context<Self>) {
        let changes = self.prefs.changes_from(&self.persisted);
        match crate::settings::write(&changes, cx) {
            Ok(()) => {
                // Only the keys that changed were written, so a dev override
                // of another key (BLYGGER_THEME) stays unsaved.
                let mut p = self.persisted.clone();
                for (key, _) in &changes {
                    match *key {
                        "font-family-writing" => p.writing_font = self.prefs.writing_font.clone(),
                        "font-family-ui" => p.ui_font = self.prefs.ui_font.clone(),
                        "font-size" => p.font_size = self.prefs.font_size,
                        "theme" => p.theme = self.prefs.theme,
                        "layout" => p.layout = self.prefs.layout,
                        "capture-hotkey" => p.hotkey = self.prefs.hotkey.clone(),
                        "show-buttons" => p.show_buttons = self.prefs.show_buttons, // --- buttons ---
                        _ => {}
                    }
                }
                self.persisted = p;
                self.refresh_problems(cx);
            }
            Err(e) => self.show_toast(e, None, cx),
        }
    }

    fn refresh_problems(&mut self, cx: &mut Context<Self>) {
        let problems = crate::settings::current_diagnostics(cx);
        if problems != self.config_problems {
            self.problems_dismissed = false;
        }
        self.config_problems = problems;
        self.title = vm::window_title(crate::settings::blyg_url(cx).as_deref());
    }

    /// ⌘⇧, — re-read the config file and apply fonts, theme, layout and the
    /// hotkey without a restart.
    fn reload_config(&mut self, _: &ReloadConfig, window: &mut Window, cx: &mut Context<Self>) {
        crate::settings::reload(cx);
        let fresh = crate::settings::prefs(cx);
        let hotkey_changed = fresh.hotkey != self.prefs.hotkey;
        self.persisted = fresh.clone();
        self.prefs = fresh;
        let mut msg = "Config reloaded".to_string();
        if hotkey_changed && let Err(e) = crate::capture::set_hotkey(&self.prefs.hotkey, cx) {
            msg = format!("Config reloaded, but the hotkey didn't change: {e}");
        }
        self.problems_dismissed = false;
        self.refresh_problems(cx);
        if !self.config_problems.is_empty() {
            let n = self.config_problems.len();
            msg = format!("{msg} · {n} problem{}", if n == 1 { "" } else { "s" });
        }
        self.apply_prefs_live(window, cx);
        self.show_toast(msg, None, cx);
    }

    fn open_config_file(&mut self, _: &OpenConfigFile, _: &mut Window, cx: &mut Context<Self>) {
        self.open_config_file_now(cx);
    }

    fn open_config_file_now(&mut self, cx: &mut Context<Self>) {
        match crate::settings::open_config_file(cx) {
            Ok(path) => self.show_toast(
                format!("Opening {}", blyg_core::config::paths::tilde(&path)),
                Some("Save it, then ⌘⇧, reloads".into()),
                cx,
            ),
            Err(e) => self.show_toast(e, None, cx),
        }
    }

    fn open_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("https://blyg.example.com"));
        let token = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Owner token")
        });
        let t2 = token.clone();
        self._subs
            .push(cx.subscribe_in(&url, window, move |_, _, ev, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    t2.update(cx, |s, cx| s.focus(window, cx));
                }
            }));
        self._subs
            .push(cx.subscribe_in(&token, window, |this, _, ev, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.submit_connect(window, cx);
                }
            }));
        // A configured blyg without a token (e.g. after the Keychain entry
        // went away): offer its address.
        if let Some(u) = crate::settings::blyg_url(cx) {
            url.update(cx, |s, cx| s.set_value(u, window, cx));
            token.update(cx, |s, cx| s.focus(window, cx));
        } else {
            url.update(cx, |s, cx| s.focus(window, cx));
        }
        self.sheet_gen += 1;
        self.sheet = Some(Sheet::Connect {
            url,
            token,
            error: None,
            busy: false,
        });
        cx.notify();
    }

    fn submit_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Sheet::Connect {
            url, token, busy, ..
        }) = &self.sheet
        else {
            return;
        };
        if *busy {
            return;
        }
        let (u, t) = (
            url.read(cx).value().to_string(),
            token.read(cx).value().to_string(),
        );
        let checked = crate::settings::validate_blyg_url(&u).and_then(|u| {
            let t = t.trim().to_string();
            if t.is_empty() {
                Err("Paste the owner token from your blyg's settings".to_string())
            } else {
                Ok((u, t))
            }
        });
        let (u, t) = match checked {
            Ok(v) => v,
            Err(e) => {
                self.set_connect_state(Some(e), false, cx);
                return;
            }
        };
        // Verify before saving anything: GET /api/items with the token.
        self.set_connect_state(None, true, cx);
        let verify = crate::connection::verifier(cx);
        let task = cx.background_spawn({
            let (u, t) = (u.clone(), t.clone());
            async move { verify(&u, &t) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let verdict = task.await;
            let _ = this.update_in(cx, |v, window, cx| match verdict {
                Err(e) => v.set_connect_state(Some(e.to_string()), false, cx),
                Ok(_) => v.finish_connect(&u, &t, window, cx),
            });
        })
        .detach();
    }

    fn set_connect_state(&mut self, err: Option<String>, is_busy: bool, cx: &mut Context<Self>) {
        if let Some(Sheet::Connect { error, busy, .. }) = self.sheet.as_mut() {
            *error = err;
            *busy = is_busy;
        }
        cx.notify();
    }

    /// The check passed: save the token and URL, switch to the live
    /// backend, and load everything once.
    fn finish_connect(
        &mut self,
        url: &str,
        token: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let url = match crate::settings::connect(url, token, cx) {
            Ok(u) => u,
            Err(e) => return self.set_connect_state(Some(e), false, cx),
        };
        let host = vm::url_host(&url).unwrap_or(url.clone());
        match crate::connection::go_live(&url, cx) {
            Ok(true) => {
                self.sheet = None;
                self.base_url = self.backend.base_url();
                self.refresh_problems(cx);
                self.focus_after_sheet(window, cx);
                self.initial_sync(Some(host), window, cx);
            }
            Ok(false) => {
                // Fake mode (or headless tests): nothing to switch.
                self.sheet = None;
                self.refresh_problems(cx);
                self.focus_after_sheet(window, cx);
                self.show_toast(
                    format!("Connected to {host}"),
                    Some("The token is in your Keychain".into()),
                    cx,
                );
            }
            Err(e) => self.set_connect_state(
                Some(format!("Couldn't open the local database: {e}")),
                false,
                cx,
            ),
        }
        cx.notify();
    }

    fn on_disconnect(&mut self, _: &Disconnect, window: &mut Window, cx: &mut Context<Self>) {
        let Some(url) = crate::settings::blyg_url(cx) else {
            return self.show_toast("Not connected to a blyg", None, cx);
        };
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.sheet_gen += 1;
        self.sheet = Some(Sheet::Disconnect {
            host: vm::url_host(&url).unwrap_or(url),
            focus,
        });
        cx.notify();
    }

    /// Forget the token and `blyg-url`; optionally delete the local copy.
    fn disconnect_now(&mut self, delete_local: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = None;
        match crate::connection::disconnect(delete_local, cx) {
            Ok(host) => {
                self.current = None;
                self.base_url = None;
                self.loading = false;
                self.unread = 0;
                self.refresh_problems(cx);
                self.requery(window, cx);
                self.show_toast(
                    format!("Disconnected from {host}"),
                    Some(if delete_local {
                        "The token and the local copy are gone".into()
                    } else {
                        "The token is gone; your posts stay on this Mac".into()
                    }),
                    cx,
                );
                self.open_connect(window, cx);
            }
            Err(e) => self.show_toast(e, None, cx),
        }
        cx.notify();
    }

    // ------------------------------------------------------------ helpers

    fn set_editor_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.loading_editor = true;
        self.editor
            .update(cx, |s, cx| s.set_value(text.to_string(), window, cx));
        self.loading_editor = false;
    }

    /// Load the selected row into the editor (NV preview-as-you-move).
    fn load_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let q = self.list.trimmed_query().to_string();
        if self.list.wants_create() {
            self.current = None;
            self.set_editor_text("", window, cx);
            let ph = format!(
                "⏎ creates a new {}: “{q}”",
                scratch::new_note_noun(self.prefs.new_note)
            );
            self.editor
                .update(cx, |s, cx| s.set_placeholder(ph, window, cx));
        } else {
            self.current = self.list.selected_item().cloned();
            let text = self
                .current
                .as_ref()
                .map(|i| i.content_md.clone())
                .unwrap_or_default();
            self.set_editor_text(&text, window, cx);
            self.editor
                .update(cx, |s, cx| s.set_placeholder("", window, cx));
        }
        self.refresh_preview(true, cx);
        cx.notify();
    }

    /// Re-render the preview (after a typing pause unless `now`).
    fn refresh_preview(&mut self, now: bool, cx: &mut Context<Self>) {
        self.studio_refresh(now, cx);
    }

    fn requery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let results = self.backend.search(self.list.query());
        self.list.refresh(results);
        // Refresh the current item's metadata without clobbering the editor.
        if let Some(cur) = &self.current {
            match self.backend.item(&cur.local_id) {
                Some(fresh) => {
                    let editor_text = self.editor.read(cx).value();
                    if fresh.content_md != editor_text.as_ref() && self.mode == Mode::Search {
                        self.current = Some(fresh);
                        self.load_current_into_editor(window, cx);
                    } else {
                        self.current = Some(fresh);
                    }
                }
                None => self.load_selected(window, cx),
            }
        } else if self.mode == Mode::Search {
            self.load_selected(window, cx);
        }
        cx.notify();
    }

    fn load_current_into_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .current
            .as_ref()
            .map(|i| i.content_md.clone())
            .unwrap_or_default();
        self.set_editor_text(&text, window, cx);
        self.refresh_preview(true, cx);
    }

    fn open(&mut self, id: &LocalId, window: &mut Window, cx: &mut Context<Self>) {
        self.list.select(id);
        if self.current.as_ref().map(|c| &c.local_id) != Some(id) {
            self.current = self.backend.item(id);
            self.load_current_into_editor(window, cx);
        }
        self.mode = Mode::Edit;
        self.editor.update(cx, |s, cx| {
            s.focus(window, cx);
            let end = s.text().len();
            s.set_selected_range(end..end, cx);
        });
        cx.notify();
    }

    fn create_from(&mut self, seed: &str, window: &mut Window, cx: &mut Context<Self>) {
        match scratch::create_note(&*self.backend, self.prefs.new_note, seed) {
            Ok((id, created)) => {
                self.set_query_text("", window, cx);
                let results = self.backend.search("");
                self.list.set_query("", results);
                self.list.select(&id);
                self.current = None;
                self.open(&id, window, cx);
                self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
                self.show_toast(created, None, cx);
            }
            Err(e) => self.show_toast(
                format!(
                    "Couldn't create a {}: {e}",
                    scratch::new_note_noun(self.prefs.new_note)
                ),
                None,
                cx,
            ),
        }
    }

    pub fn refresh_from_backend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.requery(window, cx);
    }

    fn show_toast(
        &mut self,
        text: impl Into<SharedString>,
        sub: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.toast_seq += 1;
        let id = self.toast_seq;
        let ms = if sub.is_some() { 3200 } else { 2600 };
        self.toast = Some(Toast {
            id,
            text: text.into(),
            sub,
            leaving: false,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(ms))
                .await;
            let _ = this.update(cx, |v, cx| {
                if let Some(t) = v.toast.as_mut().filter(|t| t.id == id) {
                    t.leaving = true;
                    cx.notify();
                }
            });
            cx.background_executor()
                .timer(Duration::from_millis(220))
                .await;
            let _ = this.update(cx, |v, cx| {
                if v.toast.as_ref().is_some_and(|t| t.id == id) {
                    v.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn close_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = None;
        self.focus_after_sheet(window, cx);
        cx.notify();
    }

    fn focus_after_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Edit && self.current.is_some() {
            self.editor.update(cx, |s, cx| s.focus(window, cx));
        } else {
            self.omni.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    /// Apply `prefs` to the UI and write the changed keys to the config file.
    fn apply_prefs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_prefs_live(window, cx);
        self.save_prefs(cx);
    }

    fn apply_prefs_live(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::fonts::ensure(self.prefs.writing().bundled, cx);
        crate::fonts::ensure(self.prefs.ui().bundled, cx);
        self.palette = Palette::resolve(self.prefs.theme, window.appearance());
        crate::capture::prefs_changed(&self.prefs, cx);
        cx.notify();
    }

    // ------------------------------------------------------------ events

    fn on_omni_event(
        &mut self,
        _: &Entity<InputState>,
        ev: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            InputEvent::Change => self.query_changed(window, cx),
            InputEvent::Focus => {
                self.mode = Mode::Search;
                cx.notify();
            }
            InputEvent::PressEnter {
                secondary: true, ..
            } => self.publish(window, cx),
            InputEvent::PressEnter { .. } => match self.list.enter() {
                EnterAction::Open(id) => self.open(&id, window, cx),
                EnterAction::Create(seed) => self.create_from(&seed, window, cx),
                EnterAction::Nothing => {}
            },
            InputEvent::Blur => {}
        }
    }

    fn on_editor_event(
        &mut self,
        _: &Entity<TextareaState>,
        ev: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            InputEvent::Focus => {
                if self.mode == Mode::Search && self.list.wants_create() {
                    let seed = self.list.trimmed_query().to_string();
                    self.create_from(&seed, window, cx);
                } else {
                    self.mode = Mode::Edit;
                    cx.notify();
                }
            }
            InputEvent::Change if !self.loading_editor => {
                // Typing `![[` at the start of a line opens the quote picker.
                let typed = self.typed_transclusion(cx);
                self.after_edit(cx);
                if let Some(typed) = typed {
                    self.open_quote_picker_from_typing(typed, window, cx);
                }
            }
            _ => {}
        }
    }

    fn on_core_event(&mut self, ev: CoreEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            CoreEvent::ItemsChanged => {
                // --- follow-ups --- a queued promotion uploaded local images.
                if let Some(id) = self.current.as_ref().map(|c| c.local_id.clone()) {
                    self.scratch_sync_editor(&id, window, cx);
                }
                self.requery(window, cx)
            }
            CoreEvent::SyncStatus(s) => {
                self.sync = s;
                cx.notify();
            }
            CoreEvent::Conflict {
                local_id,
                mine,
                theirs,
            } => {
                let (m, t) = vm::word_diff(&mine, &theirs);
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                self.sheet_gen += 1;
                self.sheet = Some(Sheet::Conflict {
                    id: local_id,
                    mine: m,
                    theirs: t,
                    focus,
                });
                cx.notify();
            }
            CoreEvent::ReadingChanged => {
                self.unread = self
                    .backend
                    .reading()
                    .iter()
                    .filter(|r| r.is_unread())
                    .count();
                // --- reading & versions ---
                self.reading_changed(cx);
                cx.notify();
            }
            CoreEvent::Error(msg) => self.show_toast(msg, None, cx),
        }
    }

    // ------------------------------------------------------------ actions

    fn move_selection(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.list.move_selection(delta).is_some() {
            if let Some(ix) = self.list.selected_index() {
                self.list_scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            }
            self.load_selected(window, cx);
        }
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_reading(); // --- reading & versions ---
        self.back_to_search(window, cx);
    }

    fn back_to_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = Mode::Search;
        self.omni.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        cx.notify();
    }

    fn new_draft(&mut self, _: &NewDraft, window: &mut Window, cx: &mut Context<Self>) {
        self.set_query_text("", window, cx);
        self.back_to_search(window, cx);
    }

    fn on_publish(&mut self, _: &Publish, window: &mut Window, cx: &mut Context<Self>) {
        match &self.sheet {
            // ⌘⏎ in the publish sheet's note field publishes, like ⏎.
            Some(Sheet::Publish { note, .. }) => {
                let text = note.read(cx).value().to_string();
                self.do_publish(text, window, cx);
            }
            Some(Sheet::Connect { .. }) => self.submit_connect(window, cx),
            Some(_) => {}
            None => self.publish(window, cx),
        }
    }

    fn publish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.current.clone() else {
            return;
        };
        // --- AI --- warn before publishing generated text undisclosed.
        if self.ai_publish_guard(&item, window, cx) {
            return;
        }
        match vm::publish_decision(&item) {
            PublishDecision::Shake => {
                self.shake_gen += 1;
                self.show_toast(
                    "Too long to publish as a fragment. Press ⌘T to make it a thread.",
                    None,
                    cx,
                );
            }
            PublishDecision::Empty => self.show_toast("Nothing to publish yet", None, cx),
            PublishDecision::AlreadyPublished(v) => {
                self.show_toast(format!("Already published as v{v}"), None, cx)
            }
            PublishDecision::Sheet {
                title,
                next_version,
            } => {
                let note = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("Version note (optional), e.g. “fixed a typo”")
                });
                let sub = cx.subscribe_in(&note, window, |this, note, ev, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        let text = note.read(cx).value().to_string();
                        this.do_publish(text, window, cx);
                    }
                });
                self._subs.push(sub);
                note.update(cx, |s, cx| s.focus(window, cx));
                self.sheet_gen += 1;
                self.sheet = Some(Sheet::Publish {
                    id: item.local_id,
                    title,
                    next_version,
                    note,
                });
                cx.notify();
            }
        }
    }

    fn do_publish(&mut self, note: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Sheet::Publish { id, .. }) = self.sheet.take() else {
            return;
        };
        self.focus_after_sheet(window, cx);
        self.publishing = true;
        cx.notify();
        let backend = self.backend.clone();
        let note_opt = Some(note.trim().to_string()).filter(|n| !n.is_empty());
        let task = cx.background_spawn({
            let id = id.clone();
            let note = note_opt.clone();
            async move { scratch::publish_via(&*backend, &id, note.as_deref()) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |v, window, cx| {
                v.publishing = false;
                if std::env::var_os("BLYGGER_TIMING").is_some() {
                    match &result {
                        Ok(out) => println!("publish ok v{} {}", out.version, out.permalink),
                        Err(e) => println!("publish failed: {e}"),
                    }
                }
                match result {
                    Ok(out) => {
                        let head = match &note_opt {
                            Some(n) => format!("Published v{} · “{n}”", out.version),
                            None => format!("Published v{}", out.version),
                        };
                        let sub = format!("{} · ⌘O opens it", vm::short_permalink(&out.permalink));
                        let head = match out.warning {
                            Some(w) => format!("{head} · {w}"),
                            None => head,
                        };
                        v.show_toast(head, Some(sub.into()), cx);
                    }
                    Err(CoreError::Offline) => {
                        v.show_toast("Offline. Publish again when you're back online.", None, cx)
                    }
                    Err(e) => v.show_toast(format!("Publish failed: {e}"), None, cx),
                }
                // --- follow-ups --- promotion may have rewritten local images.
                v.scratch_sync_editor(&id, window, cx);
                v.requery(window, cx);
            });
        })
        .detach();
    }

    fn toggle_kind(&mut self, _: &ToggleKind, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_kind_now(window, cx);
    }

    fn toggle_kind_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.current.clone() else {
            return;
        };
        let kind = match item.kind {
            Kind::Fragment => Kind::Thread,
            Kind::Thread => Kind::Fragment,
        };
        match self.backend.set_kind(&item.local_id, kind) {
            Ok(()) => {
                self.requery(window, cx);
                self.show_toast(
                    match kind {
                        Kind::Thread => {
                            "Now a thread: no length limit, and it can quote other blygs"
                        }
                        Kind::Fragment => "Now a fragment: 1000 characters max",
                    },
                    None,
                    cx,
                );
            }
            Err(e) => self.show_toast(format!("Couldn't change kind: {e}"), None, cx),
        }
    }

    fn toggle_preview(&mut self, _: &TogglePreview, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_preview_now(window, cx);
    }

    /// ⌘E: the preview on or off in place; the editor keeps the keyboard.
    fn toggle_preview_now(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.studio_set_view(self.studio.view.toggle_preview(), cx);
    }

    fn open_permalink(&mut self, _: &OpenPermalink, _: &mut Window, cx: &mut Context<Self>) {
        match self.current.as_ref().and_then(|i| i.permalink.clone()) {
            Some(url) => {
                cx.open_url(&url);
                self.show_toast("Opening in your browser…", None, cx);
            }
            None => self.show_toast("Not published yet", None, cx),
        }
    }

    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.sheet, Some(Sheet::Settings { .. })) {
            self.close_sheet(window, cx);
            return;
        }
        let current = self.prefs.hotkey.clone();
        let hotkey = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(current)
                .placeholder("ctrl+alt+B")
        });
        let sub = cx.subscribe_in(&hotkey, window, |this, input, ev, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                let text = input.read(cx).value().to_string();
                this.apply_hotkey(text, window, cx);
            }
        });
        self._subs.push(sub);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.sheet_gen += 1;
        self.sheet = Some(Sheet::Settings {
            hotkey,
            hotkey_error: None,
            focus,
        });
        cx.notify();
    }

    fn apply_hotkey(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let clash = crate::keymap::clash_with_hotkey(&text).map(|k| {
            format!(
                "{} is already {} in Blygger",
                crate::keymap::glyphs(k.key),
                k.label
            )
        });
        let set = match clash {
            Some(e) => Err(e),
            None => crate::capture::set_hotkey(&text, cx),
        };
        let err = match set {
            Ok(()) => {
                self.prefs.hotkey = text.trim().to_string();
                self.apply_prefs(window, cx);
                self.show_toast(
                    format!(
                        "Quick capture is now {}",
                        prefs::hotkey_glyphs(&self.prefs.hotkey)
                    ),
                    None,
                    cx,
                );
                None
            }
            Err(e) => Some(e),
        };
        if let Some(Sheet::Settings {
            hotkey_error,
            focus,
            ..
        }) = self.sheet.as_mut()
        {
            *hotkey_error = err;
            if hotkey_error.is_none() {
                window.focus(focus, cx);
            }
        }
        cx.notify();
    }

    fn font_bigger(&mut self, _: &FontBigger, window: &mut Window, cx: &mut Context<Self>) {
        self.prefs.bigger();
        self.apply_prefs(window, cx);
    }
    fn font_smaller(&mut self, _: &FontSmaller, window: &mut Window, cx: &mut Context<Self>) {
        self.prefs.smaller();
        self.apply_prefs(window, cx);
    }
    fn font_reset(&mut self, _: &FontReset, window: &mut Window, cx: &mut Context<Self>) {
        self.prefs.reset_size();
        self.apply_prefs(window, cx);
    }

    fn fake_offline(&mut self, _: &FakeToggleOffline, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = &self.fake {
            let off = !f.is_offline();
            f.set_offline(off);
            self.show_toast(
                if off {
                    "Wi-Fi off (simulated). Keep writing: nothing is lost."
                } else {
                    "Back online"
                },
                None,
                cx,
            );
        }
    }

    fn fake_conflict(&mut self, _: &FakeConflict, _: &mut Window, _cx: &mut Context<Self>) {
        if let (Some(f), Some(cur)) = (&self.fake, &self.current) {
            f.trigger_conflict(&cur.local_id);
        }
    }

    fn resolve(&mut self, how: Resolution, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Sheet::Conflict { id, .. }) = self.sheet.take() else {
            return;
        };
        match self.backend.resolve_conflict(&id, how) {
            Ok(()) => {
                if self.current.as_ref().is_some_and(|c| c.local_id == id) {
                    self.current = self.backend.item(&id);
                    self.load_current_into_editor(window, cx);
                }
                self.requery(window, cx);
                self.show_toast(
                    match how {
                        Resolution::KeepMine => {
                            "Kept your version; the server copy is saved in version history"
                        }
                        Resolution::TakeServer => "Took the server's version",
                        Resolution::KeepBoth => "Kept both: your version is now a separate draft",
                    },
                    None,
                    cx,
                );
            }
            Err(e) => self.show_toast(format!("Couldn't resolve: {e}"), None, cx),
        }
        self.focus_after_sheet(window, cx);
        cx.notify();
    }

    // ------------------------------------------------------------ images

    /// A web address pasted over selected text links the text: `[text](url)`.
    fn paste_link(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.current.is_none() {
            return false;
        }
        let Some(clip) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            return false;
        };
        let (text, sel) = {
            let s = self.editor.read(cx);
            (s.value().to_string(), s.selected_range())
        };
        let Some((new, caret)) = vm::link_paste(&text, sel, &clip) else {
            return false;
        };
        self.splice_editor(&text, &new, Some(caret), window, cx);
        true
    }

    fn paste_image(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let image = item.entries().iter().find_map(|e| match e {
            ClipboardEntry::Image(img) => Some(img.clone()),
            _ => None,
        });
        if let Some(img) = image {
            let mime = img.format.mime_type();
            self.start_upload(img.bytes, mime.to_string(), window, cx);
            return true;
        }
        // Copied image files arrive as paths.
        let paths: Vec<std::path::PathBuf> = item
            .entries()
            .iter()
            .filter_map(|e| match e {
                ClipboardEntry::ExternalPaths(p) => Some(p.paths().to_vec()),
                _ => None,
            })
            .flatten()
            .filter(|p| vm::mime_for_path(p).is_some())
            .collect();
        if paths.is_empty() {
            return false;
        }
        self.upload_paths(paths, window, cx);
        true
    }

    fn upload_paths(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for p in paths {
            let Some(mime) = vm::mime_for_path(&p) else {
                continue;
            };
            match std::fs::read(&p) {
                Ok(bytes) => self.start_upload(bytes, mime.to_string(), window, cx),
                Err(e) => self.show_toast(format!("Couldn't read {}: {e}", p.display()), None, cx),
            }
        }
    }

    fn start_upload(
        &mut self,
        bytes: Vec<u8>,
        mime: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.current.as_ref().map(|c| c.local_id.clone()) else {
            self.show_toast("Open or create a post first", None, cx);
            return;
        };
        // --- follow-ups --- a scratch note keeps its images on this Mac.
        if self.scratch_keep_image(&bytes, &mime, window, cx) {
            return;
        }
        // Insert the placeholder at the caret, as an undoable edit.
        let (text, cursor) = {
            let s = self.editor.read(cx);
            (s.value().to_string(), s.cursor())
        };
        let (new_text, range) = vm::insert_placeholder(&text, cursor);
        self.splice_editor(&text, &new_text, Some(range.end), window, cx);
        let size_kb = bytes.len().div_ceil(1024);
        let backend = self.backend.clone();
        // Uploaded *unattached*: the Worker appends every attachment to the
        // public page, so an attached image that's also inline would show
        // twice. Inline, it shows once, where it was pasted.
        let task = cx.background_spawn(async move {
            let r = backend.upload_media(bytes.clone(), &mime, None, None);
            // The text gets the absolute URL: the Worker renders a relative
            // `media/…` page-relative, which breaks under /f/<id>/.
            r.map(|m| {
                let inline = crate::vm::absolute_url(&m.url, backend.base_url().as_deref())
                    .unwrap_or_else(|| m.url.clone());
                (m, inline)
            })
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |v, window, cx| {
                let editing_same = v.current.as_ref().is_some_and(|c| c.local_id == id);
                let text = if editing_same {
                    v.editor.read(cx).value().to_string()
                } else {
                    v.backend
                        .item(&id)
                        .map(|i| i.content_md)
                        .unwrap_or_default()
                };
                let replaced = match &result {
                    Ok((_, inline)) => vm::replace_placeholder(&text, inline),
                    Err(_) => vm::remove_placeholder(&text),
                };
                // The placeholder was deleted before the upload finished:
                // nothing refers to the file, so remove it again.
                if replaced.is_none()
                    && let Ok((m, _)) = &result
                {
                    let (b, url) = (v.backend.clone(), m.url.clone());
                    cx.background_spawn(async move {
                        let _ = b.delete_media(&url);
                    })
                    .detach();
                }
                if let Some(new_text) = replaced {
                    if editing_same {
                        v.splice_editor(&text, &new_text, None, window, cx);
                    } else {
                        let _ = v.backend.save(&id, &new_text);
                    }
                }
                match result {
                    Ok((m, _)) => {
                        let ext = m.url.rsplit('.').next().unwrap_or("");
                        v.show_toast(format!("Image uploaded ({size_kb} KB {ext})"), None, cx)
                    }
                    Err(e) => v.show_toast(format!("Upload failed: {e}"), None, cx),
                }
            });
        })
        .detach();
    }

    /// Apply `old -> new` to the editor as a minimal, undoable replacement,
    /// keeping scroll. The caret goes to `caret` if given, else stays put
    /// (shifted if the edit was before it). Then saves.
    fn splice_editor(
        &mut self,
        old: &str,
        new: &str,
        caret: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (range, insert) = vm::splice(old, new);
        let prev_cursor = self.editor.read(cx).cursor();
        self.editor.update(cx, |s, cx| {
            s.set_selected_range(range.clone(), cx);
            s.replace(insert.to_string(), window, cx);
            let c = caret.unwrap_or_else(|| vm::shift_cursor(prev_cursor, &range, insert.len()));
            let c = c.min(s.text().len());
            s.set_selected_range(c..c, cx);
        });
        self.after_edit(cx);
    }

    /// Save the editor's text to the backend and refresh derived state.
    fn after_edit(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.current.as_ref().map(|c| c.local_id.clone()) else {
            return;
        };
        let text = self.editor.read(cx).value();
        if let Err(e) = self.ai_save_edit(&id, &text) {
            self.show_toast(format!("Couldn't save: {e}"), None, cx);
        }
        if let Some(fresh) = self.backend.item(&id) {
            self.list.update_item(fresh.clone());
            self.current = Some(fresh);
        }
        self.refresh_preview(false, cx);
        cx.notify();
    }

    /// Set the omnibar text programmatically (set_value emits no event).
    fn set_query_text(&mut self, q: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.omni
            .update(cx, |s, cx| s.set_value(q.to_string(), window, cx));
        self.query_changed(window, cx);
    }

    fn query_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let q = self.omni.read(cx).value().to_string();
        if q == self.list.query() {
            return;
        }
        let results = self.backend.search(&q);
        self.list.set_query(&q, results);
        self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.mode = Mode::Search;
        self.load_selected(window, cx);
    }
}

impl Focusable for MainView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// ================================================================ rendering

/// The list's size along the layout axis: a width beside the editor
/// (`layout = side`) or a height above it (`layout = stacked`).
#[derive(Debug, Clone, Copy)]
enum ListLength {
    Width(Pixels),
    Height(Pixels),
}

const TITLEBAR_H: f32 = 34.;
const OMNI_H: f32 = 46.;
const STATUS_H: f32 = 30.;

pub(crate) fn shake_offset(t: f32) -> f32 {
    // keyframes: 25% -6, 50% +6, 75% -3, 100% 0
    let k = [
        (0.0, 0.0),
        (0.25, -6.0),
        (0.5, 6.0),
        (0.75, -3.0),
        (1.0, 0.0),
    ];
    for w in k.windows(2) {
        let ((t0, v0), (t1, v1)) = (w[0], w[1]);
        if t <= t1 {
            let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
            return v0 + (v1 - v0) * f;
        }
    }
    0.0
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(t0) = self.first_frame.take() {
            window.on_next_frame(move |_, _| {
                if std::env::var_os("BLYGGER_TIMING").is_some() {
                    let epoch = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis())
                        .unwrap_or(0);
                    println!(
                        "first-frame {} ms after main(); epoch_ms={epoch}",
                        t0.elapsed().as_millis()
                    );
                }
            });
        }
        let p = self.palette;
        let size = self.prefs.font_size;
        let ui_font: SharedString = self.prefs.ui().family.into();
        let body_font: SharedString = self.prefs.writing().family.into();
        let onboarding = self.onboarding_frame(window, cx); // --- onboarding ---
        let profile_overlay = self.render_profile_overlay(cx); // --- profiles ---

        if self.native_title != self.title {
            window.set_window_title(&self.title);
            self.native_title = self.title.clone();
        }

        // --- full editor: which panes are on screen (⌘1/⌘2/⌘3/⌘E) ---
        self.studio_frame(window, cx);
        let show_list = self.studio.view.list_visible();
        let show_preview = self.studio.view.preview_visible();
        let viewport = window.viewport_size();
        let stacked = self.prefs.layout == LayoutPref::Stacked;
        let list_w = (viewport.width * if show_preview { 0.24 } else { 0.38 }).round();
        let rest = if stacked || !show_list {
            viewport.width
        } else {
            viewport.width - list_w - px(1.)
        };
        let pane_w = if show_preview {
            (rest / 2.).floor()
        } else {
            rest
        };
        // --- end full editor ---
        let list_len = if stacked {
            ListLength::Height((viewport.height * 0.34).round())
        } else {
            ListLength::Width(list_w)
        };
        let measure = px(size * 36.0);
        let side_pad = ((pane_w - measure) / 2.).max(px(34.));

        // Project the palette + measure onto the editor each frame (cheap: copies).
        self.editor.update(cx, |s, _| {
            s.set_editor_style(gpui_kit::base::input::InputEditorStyle {
                foreground: p.ink,
                muted_foreground: p.muted,
                background: p.bg,
                border: p.line,
                selection: p.text_selection,
                caret: p.accent,
                ..Default::default()
            });
            s.set_editor_paddings(Edges {
                top: px(26.),
                bottom: px(40.),
                left: side_pad,
                right: side_pad,
            });
        });
        let sheet_inputs: Vec<&Entity<InputState>> = match &self.sheet {
            Some(Sheet::Publish { note, .. }) => vec![note],
            Some(Sheet::Settings { hotkey, .. }) => vec![hotkey],
            Some(Sheet::Connect { url, token, .. }) => vec![url, token],
            Some(Sheet::Withdraw { note, .. }) => vec![note], // --- delete & withdraw ---
            _ => vec![],
        };
        for input in [&self.omni].into_iter().chain(sheet_inputs) {
            input.update(cx, |s, _| {
                s.set_editor_style(gpui_kit::base::input::InputEditorStyle {
                    foreground: p.ink,
                    muted_foreground: p.muted,
                    background: gpui_kit::transparent_black(),
                    border: p.line,
                    selection: p.text_selection,
                    caret: p.accent,
                    ..Default::default()
                });
            });
        }

        // --- buttons --- how the toolbar fits (None: show-buttons = false)
        let toolbar = self.toolbar_fit(window, cx);
        let show_title = toolbar.as_ref().is_none_or(|(fit, _)| fit.title());
        div()
            .id("blygger")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::focus_search))
            .on_action(cx.listener(Self::on_publish))
            .on_action(cx.listener(Self::toggle_kind))
            .on_action(cx.listener(Self::toggle_preview))
            .on_action(cx.listener(Self::open_permalink))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::font_bigger))
            .on_action(cx.listener(Self::font_smaller))
            .on_action(cx.listener(Self::font_reset))
            .on_action(cx.listener(Self::new_draft))
            .on_action(cx.listener(Self::fake_offline))
            .on_action(cx.listener(Self::fake_conflict))
            .on_action(cx.listener(Self::reload_config))
            .on_action(cx.listener(Self::open_config_file))
            .on_action(cx.listener(Self::on_disconnect))
            .on_action(cx.listener(Self::make_draft))
            .on_action(cx.listener(Self::ai_generate)) // --- AI ---
            .on_action(cx.listener(Self::ai_shorten)) // --- AI ---
            // --- full editor ---
            .on_action(cx.listener(Self::view_write))
            .on_action(cx.listener(Self::view_split))
            .on_action(cx.listener(Self::view_studio))
            .map(|d| self.reading_actions(d, cx)) // --- reading & versions ---
            .map(|d| self.onboarding_actions(d, cx)) // --- onboarding ---
            .map(|d| self.profile_actions(d, cx)) // --- profiles ---
            .map(|d| self.discard_actions(d, cx)) // --- delete & withdraw ---
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(p.bg)
            .text_color(p.ink)
            .font_family(ui_font.clone())
            .child(self.render_titlebar(&ui_font, show_title))
            .child(self.render_view_switcher(cx)) // --- reading & versions ---
            .children(self.render_toolbar(toolbar, &ui_font, cx)) // --- buttons ---
            .children(self.render_problems(cx))
            // --- reading & versions: other screens replace omnibar + list + editor
            .map(|d| match self.render_reading_body(&body_font, cx) {
                Some(body) => d.child(body),
                None => d.child(self.render_omnibar(&ui_font, size, cx)).child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .when(stacked, |d| d.flex_col())
                        .when(!stacked, |d| d.flex_row())
                        .when(show_list, |d| {
                            d.child(self.render_list(list_len, &ui_font, cx))
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .min_h_0()
                                .flex()
                                .flex_row()
                                .child(match self.render_own_versions(&body_font, size, cx) {
                                    Some(v) => v,
                                    None => self
                                        .render_editor_pane(&body_font, size, cx)
                                        .into_any_element(),
                                })
                                // --- full editor --- (not beside ⌘Y's history,
                                // whose body is the reader's WebView)
                                .when(show_preview && self.reading.own.is_none(), |d| {
                                    d.child(self.studio_pane(&p))
                                }),
                        ),
                ),
            })
            .child(self.render_status_bar(cx))
            .children(self.render_sheet(&ui_font, &body_font, cx))
            .children(self.render_ai_overlay(&ui_font, &body_font, cx)) // --- AI ---
            .children(self.render_reading_sheet(cx)) // --- reading & versions ---
            .children(profile_overlay) // --- profiles ---
            .children(self.render_toast())
            .children(onboarding) // --- onboarding ---
    }
}

impl MainView {
    fn render_titlebar(&self, ui_font: &SharedString, show_title: bool) -> impl IntoElement {
        let p = self.palette;
        div()
            .id("titlebar")
            .window_control_area(WindowControlArea::Drag)
            .h(px(TITLEBAR_H))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.bar)
            .border_b_1()
            .border_color(p.line)
            // --- buttons --- the toolbar hides the title when it needs the room
            .when(show_title, |d| {
                d.child(
                    div()
                        .font_family(ui_font.clone())
                        .font_weight(FontWeight::MEDIUM)
                        .text_size(px(12.))
                        .text_color(p.muted)
                        .child(self.title.clone()),
                )
            })
    }

    /// Config file problems, with line numbers, in a dismissible banner.
    fn render_problems(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.config_problems.is_empty() || self.problems_dismissed {
            return None;
        }
        let p = self.palette;
        let n = self.config_problems.len();
        let errors = self.config_problems.iter().any(|d| d.is_error());
        const SHOWN: usize = 4;
        let lines = self
            .config_problems
            .iter()
            .take(SHOWN)
            .map(|d| div().truncate().child(d.short()));
        Some(
            div()
                .id("config-problems")
                .flex_none()
                .flex()
                .gap(px(10.))
                .px(px(14.))
                .py(px(7.))
                .border_b_1()
                .border_color(p.line)
                .bg(p.bar)
                .font_family("Inter")
                .text_size(px(11.5))
                .text_color(p.muted)
                .child(
                    div()
                        .flex_none()
                        .size(px(7.))
                        .mt(px(5.))
                        .rounded_full()
                        .bg(if errors { p.over } else { p.amber }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            div()
                                .text_color(p.ink)
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!(
                                    "The config file has {n} problem{} · ⌘⇧, reloads it after you fix {}",
                                    if n == 1 { "" } else { "s" },
                                    if n == 1 { "it" } else { "them" }
                                )),
                        )
                        .children(lines)
                        .when(n > SHOWN, |d| {
                            d.child(format!(
                                "…and {} more (blygger +validate-config lists them all)",
                                n - SHOWN
                            ))
                        }),
                )
                .child(
                    div()
                        .id("open-config")
                        .flex_none()
                        .cursor_pointer()
                        .hover(|s| s.text_color(p.ink))
                        .on_click(cx.listener(|this, _, _, cx| this.open_config_file_now(cx)))
                        .child("Open"),
                )
                .child(
                    div()
                        .id("dismiss-problems")
                        .flex_none()
                        .cursor_pointer()
                        .hover(|s| s.text_color(p.ink))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.problems_dismissed = true;
                            cx.notify();
                        }))
                        .child("✕"),
                )
                .into_any_element(),
        )
    }

    fn render_omnibar(
        &self,
        ui_font: &SharedString,
        size: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = self.palette;
        let hint = if self.mode == Mode::Search {
            self.list.hint()
        } else {
            String::new()
        };
        div()
            .id("omnibar")
            .h(px(OMNI_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(14.))
            .border_b_1()
            .border_color(p.line)
            .child(div().text_size(px(16.)).text_color(p.muted).child("⌕"))
            .child(
                div()
                    .flex_1()
                    .font_family(ui_font.clone())
                    .text_size(px(size - 1.))
                    .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                        cx.stop_propagation();
                        this.move_selection(-1, window, cx);
                    }))
                    .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                        cx.stop_propagation();
                        this.move_selection(1, window, cx);
                    }))
                    .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                        cx.stop_propagation();
                        this.set_query_text("", window, cx);
                    }))
                    .child(gpui_kit::base::input::Input::new(&self.omni)),
            )
            .when(!hint.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .font_family("Inter")
                        .text_size(px(12.))
                        .text_color(p.muted)
                        .child(hint),
                )
            })
    }

    fn render_list(
        &self,
        len: ListLength,
        ui_font: &SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = self.palette;
        let lsize = self.prefs.list_size();
        let count = self.list.results().len();
        let base = div()
            .flex_none()
            .border_color(p.line)
            .font_family(ui_font.clone());
        let base = match len {
            ListLength::Width(w) => base.w(w).h_full().border_r_1(),
            ListLength::Height(h) => base.h(h).w_full().border_b_1(),
        };
        if count == 0 {
            let q = self.list.trimmed_query().to_string();
            let msg = if q.is_empty() {
                "Nothing here yet. Type to start writing.".to_string()
            } else {
                format!("No matches. Press ⏎ to start “{q}”.")
            };
            return base.child(
                div()
                    .p(px(14.))
                    .italic()
                    .text_size(px(lsize))
                    .text_color(p.muted)
                    .child(msg),
            );
        }
        base.child(
            uniform_list(
                "items",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                    range
                        .map(|ix| this.render_row(ix, lsize, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.list_scroll)
            .size_full(),
        )
    }

    fn render_row(&self, ix: usize, lsize: f32, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let item = &self.list.results()[ix];
        let selected = self.list.selected() == Some(&item.local_id);
        let title: SharedString = item.title().to_string().into();
        let highlights: Vec<_> = vm::highlight_ranges(&title, self.list.query())
            .into_iter()
            .map(|r| {
                (
                    r,
                    HighlightStyle {
                        color: Some(p.accent),
                        font_weight: Some(FontWeight::SEMIBOLD),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let (pill_text, pill_pub) = vm::pill(item);
        let id = item.local_id.clone();
        div()
            .id(("row", ix))
            .px(px(14.))
            .py(px(6.))
            .when(selected, |d| d.bg(p.sel))
            .on_click(cx.listener(move |this, _, window, cx| this.open(&id, window, cx)))
            .child(
                div()
                    .text_size(px(lsize))
                    .line_height(relative(1.35))
                    .truncate()
                    .child(StyledText::new(title).with_highlights(highlights)),
            )
            .child(
                div()
                    .mt(px(2.))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .font_family("Inter")
                    .text_size(px(11.))
                    .text_color(p.muted)
                    .when(vm::has_unpublished_edits(item), |d| {
                        d.child(div().size(px(6.)).rounded_full().bg(p.accent))
                    })
                    .child(vm::kind_label(item.kind))
                    .child(
                        div()
                            .px(px(6.))
                            .rounded_full()
                            .border_1()
                            .line_height(px(15.))
                            .border_color(if pill_pub { p.accent } else { p.line })
                            .when(pill_pub, |d| d.text_color(p.accent))
                            .child(pill_text),
                    )
                    .child(vm::relative_time(&item.updated, self.now)),
            )
            .into_any_element()
    }

    fn render_editor_pane(
        &self,
        body_font: &SharedString,
        size: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("editor-pane")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .relative()
            .font_family(body_font.clone())
            .text_size(px(size))
            .line_height(relative(1.6))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                if this.sheet.is_none() {
                    cx.stop_propagation();
                    // --- AI --- esc cancels a running generation first.
                    if !this.ai_escape(window, cx) {
                        this.back_to_search(window, cx);
                    }
                }
            }))
            .capture_action(cx.listener(|this, a: &Enter, window, cx| {
                if a.secondary {
                    cx.stop_propagation();
                    this.publish(window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &Paste, window, cx| {
                if this.paste_link(window, cx) || this.paste_image(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                let imgs: Vec<_> = paths
                    .paths()
                    .iter()
                    .filter(|p| vm::mime_for_path(p).is_some())
                    .cloned()
                    .collect();
                if !imgs.is_empty() {
                    this.upload_paths(imgs, window, cx);
                }
            }))
            .child(
                div()
                    .size_full()
                    .child(gpui_kit::base::input::Textarea::new(&self.editor)),
            )
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let disconnected =
            crate::connection::mode(cx) == Some(crate::connection::Mode::Disconnected);
        let (sync_text, dot) = if disconnected {
            ("not connected".to_string(), vm::SyncDot::Grey)
        } else if self.loading && !self.publishing {
            ("loading your posts…".to_string(), vm::SyncDot::Busy)
        } else {
            vm::sync_label(self.sync, self.publishing)
        };
        let dot_color = match dot {
            vm::SyncDot::Green => p.green,
            vm::SyncDot::Amber => p.amber,
            vm::SyncDot::Busy => p.accent,
            vm::SyncDot::Grey => p.grey,
            vm::SyncDot::Red => p.over,
        };
        // --- follow-ups --- Reading / Mentions / Subscriptions show their
        // own status, not the current post's.
        let screen = reading::vm::screen_status(self.reading.view, self.unread);
        let item = self.current.as_ref().filter(|_| screen.post);
        let chars = item.map(|i| i.char_count()).unwrap_or(0);
        let kind = item.map(|i| i.kind).unwrap_or(Kind::Fragment);
        let (count, level) = vm::counter(kind, chars);
        let banner = vm::banner(kind, chars);

        let dot_el = div().size(px(7.)).rounded_full().bg(dot_color);
        let dot_el = if dot == vm::SyncDot::Busy {
            dot_el
                .with_animation(
                    "pulse",
                    Animation::new(Duration::from_millis(1400)).repeat(),
                    |d, t| {
                        let tri = 1.0 - (2.0 * t - 1.0).abs();
                        d.opacity(1.0 - 0.75 * tri)
                    },
                )
                .into_any_element()
        } else {
            dot_el.into_any_element()
        };

        let bar = div()
            .id("status")
            .h(px(STATUS_H))
            .flex_none()
            .relative()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .border_t_1()
            .border_color(p.line)
            .bg(p.bar)
            .font_family("Inter")
            .text_size(px(11.5))
            .text_color(p.muted)
            .when_some(item, |d, item| {
                d.child(
                    div()
                        .id("kind")
                        .cursor_pointer()
                        .hover(|s| s.text_color(p.ink))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_kind_now(window, cx)),
                        )
                        .child(vm::kind_label(item.kind)),
                )
                .child(
                    div()
                        .when(level == vm::Level::Warn, |d| d.text_color(p.warn))
                        .when(level == vm::Level::Over, |d| {
                            d.text_color(p.over).font_weight(FontWeight::SEMIBOLD)
                        })
                        .child(count),
                )
            })
            // --- full editor ---
            .children(self.studio_status(&p).filter(|_| screen.post))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .text_color(p.over)
                    .children(banner),
            )
            .children(screen.to_read.map(|t| div().id("to-read").child(t)))
            .children(self.render_update_notice(cx)) // --- auto-update ---
            .children(self.render_ai_status()) // --- AI ---
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(dot_el)
                    .child(sync_text),
            )
            .children(item.map(vm::version_label));

        if self.shake_gen > 0 {
            bar.with_animation(
                ("shake", self.shake_gen),
                Animation::new(Duration::from_millis(350)),
                |d, t| d.left(px(shake_offset(t))),
            )
            .into_any_element()
        } else {
            bar.into_any_element()
        }
    }

    fn render_toast(&self) -> Option<AnyElement> {
        let t = self.toast.as_ref()?;
        let p = self.palette;
        let leaving = t.leaving;
        let el = div()
            .absolute()
            // --- full editor: clear of the native preview ---
            .right(self.studio_toast_right())
            .bottom(px(STATUS_H + 12.))
            .max_w(px(420.))
            .px(px(12.))
            .py(px(9.))
            .rounded(px(8.))
            .bg(p.ink)
            .text_color(p.bg)
            .font_family("Inter")
            .text_size(px(12.5))
            .shadow_md()
            .child(t.text.clone())
            .when_some(t.sub.clone(), |d, sub| {
                d.child(div().opacity(0.7).child(sub))
            });
        Some(
            el.with_animation(
                ElementId::NamedInteger("toast".into(), (t.id * 2 + leaving as usize) as u64),
                Animation::new(Duration::from_millis(200)).with_easing(ease_out_quint()),
                move |d, t| {
                    let v = if leaving { 1.0 - t } else { t };
                    d.opacity(v).mb(px(-8.0 * (1.0 - v)))
                },
            )
            .into_any_element(),
        )
    }

    fn render_sheet(
        &self,
        ui_font: &SharedString,
        body_font: &SharedString,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let sheet = self.sheet.as_ref()?;
        let p = self.palette;
        let kbd = |k: &'static str| {
            div()
                .px(px(6.))
                .py(px(1.))
                .min_w(px(20.))
                .flex()
                .justify_center()
                .rounded(px(5.))
                .border_1()
                .border_b_2()
                .border_color(p.line)
                .text_color(p.ink)
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(11.5))
                .child(k)
        };
        let key_hint = |k: &'static str, label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .child(kbd(k))
                .child(label)
        };
        let keys_row = |items: Vec<Div>| {
            div()
                .mt(px(12.))
                .flex()
                .gap(px(14.))
                .text_color(p.muted)
                .children(items)
        };
        let heading = |s: String| {
            div()
                .mb(px(8.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(13.))
                .child(s)
        };
        let input_box = |el: AnyElement| {
            div()
                .px(px(10.))
                .py(px(8.))
                .rounded(px(7.))
                .border_1()
                .border_color(p.line)
                .text_size(px(14.))
                .font_family(ui_font.clone())
                .child(el)
        };

        let (width, content): (f32, AnyElement) = match sheet {
            Sheet::Publish {
                title,
                next_version,
                note,
                ..
            } => (
                460.,
                div()
                    .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                        cx.stop_propagation();
                        this.close_sheet(window, cx);
                    }))
                    .child(heading(format!("Publish “{title}” as v{next_version}")))
                    // --- full editor ---
                    .children(self.studio_publish_warning(cx).map(|w| {
                        div()
                            .mb(px(8.))
                            .text_size(px(12.5))
                            .text_color(p.warn)
                            .child(w)
                    }))
                    .child(input_box(
                        gpui_kit::base::input::Input::new(note).into_any_element(),
                    ))
                    .child(keys_row(vec![
                        key_hint("⏎", "publish"),
                        key_hint("esc", "cancel"),
                    ]))
                    .into_any_element(),
            ),
            Sheet::Conflict {
                mine,
                theirs,
                focus,
                ..
            } => {
                let col = |label: &'static str, runs: &Vec<(DiffOp, String)>| {
                    let text: String = runs.iter().map(|(_, s)| s.as_str()).collect();
                    let mut hl = Vec::new();
                    let mut at = 0;
                    for (op, s) in runs {
                        let r = at..at + s.len();
                        at += s.len();
                        match op {
                            DiffOp::Insert => hl.push((
                                r,
                                HighlightStyle {
                                    background_color: Some(p.ins_bg),
                                    ..Default::default()
                                },
                            )),
                            DiffOp::Delete => hl.push((
                                r,
                                HighlightStyle {
                                    background_color: Some(p.del_bg),
                                    strikethrough: Some(StrikethroughStyle {
                                        thickness: px(1.),
                                        color: Some(p.over),
                                    }),
                                    ..Default::default()
                                },
                            )),
                            DiffOp::Same => {}
                        }
                    }
                    div()
                        .id(label)
                        .flex_1()
                        .min_w_0()
                        .max_h(px(260.))
                        .overflow_y_scroll()
                        .p(px(8.))
                        .rounded(px(7.))
                        .border_1()
                        .border_color(p.line)
                        .child(
                            div()
                                .mb(px(4.))
                                .font_family("Inter")
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(10.5))
                                .text_color(p.muted)
                                .child(label),
                        )
                        .child(
                            div()
                                .font_family(body_font.clone())
                                .text_size(px(13.))
                                .line_height(relative(1.45))
                                .child(StyledText::new(text).with_highlights(hl)),
                        )
                };
                (
                    560.,
                    div()
                        .track_focus(focus)
                        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                            match ev.keystroke.key.as_str() {
                                "1" => this.resolve(Resolution::KeepMine, window, cx),
                                "2" => this.resolve(Resolution::TakeServer, window, cx),
                                "3" => this.resolve(Resolution::KeepBoth, window, cx),
                                "escape" => this.close_sheet(window, cx),
                                _ => return,
                            }
                            cx.stop_propagation();
                        }))
                        .child(heading("This post was also edited elsewhere".into()))
                        .child(
                            div()
                                .flex()
                                .gap(px(10.))
                                .my(px(8.))
                                .child(col("ON THIS MAC", mine))
                                .child(col("ON THE SERVER", theirs)),
                        )
                        .child(keys_row(vec![
                            key_hint("1", "keep mine"),
                            key_hint("2", "take server's"),
                            key_hint("3", "keep both"),
                        ]))
                        .into_any_element(),
                )
            }
            Sheet::Settings {
                hotkey,
                hotkey_error,
                focus,
            } => (
                480.,
                self.render_settings(hotkey, hotkey_error.as_deref(), focus, &kbd, cx),
            ),
            Sheet::Disconnect { host, focus } => (
                460.,
                div()
                    .track_focus(focus)
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        match ev.keystroke.key.as_str() {
                            "1" => this.disconnect_now(false, window, cx),
                            "2" => this.disconnect_now(true, window, cx),
                            "escape" => this.close_sheet(window, cx),
                            _ => return,
                        }
                        cx.stop_propagation();
                    }))
                    .child(heading(format!("Disconnect from {host}?")))
                    .child(div().text_color(p.muted).line_height(relative(1.45)).child(
                        "The owner token is removed from your Keychain and the address from \
                         your config file. Nothing changes on the blyg itself.",
                    ))
                    .child(keys_row(vec![
                        key_hint("1", "disconnect, keep the posts on this Mac"),
                        key_hint("2", "also delete the local copy"),
                    ]))
                    .child(keys_row(vec![key_hint("esc", "cancel")]))
                    .into_any_element(),
            ),
            Sheet::Connect {
                url,
                token,
                error,
                busy,
            } => {
                let label = |s: &'static str| {
                    div()
                        .mt(px(10.))
                        .mb(px(4.))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.muted)
                        .child(s)
                };
                (
                    480.,
                    div()
                        .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                            cx.stop_propagation();
                            this.close_sheet(window, cx);
                        }))
                        .child(heading("Connect your blyg".into()))
                        .child(div().text_color(p.muted).line_height(relative(1.45)).child(
                            "Blygger writes to your own blyg. Enter its address and its \
                                     owner token. The token is kept in your macOS Keychain; the \
                                     address goes in your config file.",
                        ))
                        .child(label("BLYG ADDRESS"))
                        .child(input_box(
                            gpui_kit::base::input::Input::new(url).into_any_element(),
                        ))
                        .child(label("OWNER TOKEN"))
                        .child(input_box(
                            gpui_kit::base::input::Input::new(token).into_any_element(),
                        ))
                        .when_some(error.clone(), |d, e| {
                            d.child(div().mt(px(8.)).text_color(p.over).child(e))
                        })
                        .when(*busy, |d| {
                            d.child(
                                div()
                                    .mt(px(8.))
                                    .text_color(p.muted)
                                    .child("Checking the address and token…"),
                            )
                        })
                        .child(keys_row(vec![
                            key_hint("⏎", "next · connect"),
                            key_hint("esc", "not now"),
                        ]))
                        .into_any_element(),
                )
            }
            // --- delete & withdraw --- (discard.rs): the keys are buttons too.
            Sheet::DeleteDraft {
                title, noun, focus, ..
            } => (
                440.,
                div()
                    .track_focus(focus)
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        match ev.keystroke.key.as_str() {
                            "enter" => this.confirm_delete(window, cx),
                            "escape" => this.close_sheet(window, cx),
                            _ => return,
                        }
                        cx.stop_propagation();
                    }))
                    .child(heading(format!("Delete this {noun}?")))
                    .child(
                        div()
                            .text_color(p.muted)
                            .line_height(relative(1.45))
                            .child(div().text_color(p.ink).child(format!("“{title}”")))
                            .child(discard::DELETE_NOTE),
                    )
                    .child(
                        keys_row(vec![])
                            .child(
                                key_hint("⏎", "delete")
                                    .id("sheet-delete")
                                    .debug_selector(|| "sheet-delete".into())
                                    .cursor_pointer()
                                    .text_color(p.over)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirm_delete(window, cx)
                                    })),
                            )
                            .child(
                                key_hint("esc", "cancel")
                                    .id("sheet-cancel")
                                    .debug_selector(|| "sheet-cancel".into())
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_sheet(window, cx)
                                    })),
                            ),
                    )
                    .into_any_element(),
            ),
            Sheet::Withdraw { title, note, .. } => {
                let note = note.clone();
                (
                    460.,
                    div()
                        .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                            cx.stop_propagation();
                            this.close_sheet(window, cx);
                        }))
                        .child(heading(format!("Withdraw “{title}”?")))
                        .child(
                            div()
                                .mb(px(8.))
                                .text_color(p.muted)
                                .line_height(relative(1.45))
                                .child(discard::WITHDRAW_NOTE),
                        )
                        .child(input_box(
                            gpui_kit::base::input::Input::new(&note).into_any_element(),
                        ))
                        .child(
                            keys_row(vec![])
                                .child(
                                    key_hint("⏎", "withdraw")
                                        .id("sheet-withdraw")
                                        .debug_selector(|| "sheet-withdraw".into())
                                        .cursor_pointer()
                                        .text_color(p.over)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            let text = note.read(cx).value().to_string();
                                            this.confirm_withdraw(text, window, cx)
                                        })),
                                )
                                .child(
                                    key_hint("esc", "cancel")
                                        .id("sheet-cancel")
                                        .debug_selector(|| "sheet-cancel".into())
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.close_sheet(window, cx)
                                        })),
                                ),
                        )
                        .into_any_element(),
                )
            }
        };

        let sheet_gen = self.sheet_gen;
        Some(
            div()
                .absolute()
                .top(px(TITLEBAR_H))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("sheet")
                        .occlude()
                        .w(px(width))
                        .max_w(relative(0.92))
                        .bg(p.bg)
                        .border_1()
                        .border_t_0()
                        .border_color(p.line)
                        .rounded_b(px(12.))
                        .shadow(vec![BoxShadow {
                            color: p.shadow,
                            offset: point(px(0.), px(18.)),
                            blur_radius: px(40.),
                            spread_radius: px(-12.),
                            inset: false,
                        }])
                        .px(px(18.))
                        .py(px(16.))
                        .font_family(ui_font.clone())
                        .text_size(px(13.))
                        .child(content)
                        .with_animation(
                            ("sheet-in", sheet_gen),
                            Animation::new(Duration::from_millis(220))
                                .with_easing(ease_out_quint()),
                            |d, t| d.mt(px(-240.0 * (1.0 - t))).opacity(t.min(1.0) * 0.4 + 0.6),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_settings(
        &self,
        hotkey: &Entity<InputState>,
        hotkey_error: Option<&str>,
        focus: &FocusHandle,
        kbd: &dyn Fn(&'static str) -> Div,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        let chip =
            |id: SharedString, label: SharedString, on: bool, family: Option<&'static str>| {
                div()
                    .id(id)
                    .px(px(10.))
                    .py(px(3.))
                    .rounded_full()
                    .border_1()
                    .cursor_pointer()
                    .border_color(if on { p.accent } else { p.line })
                    .when(on, |d| d.text_color(p.accent))
                    .when_some(family, |d, f| d.font_family(f))
                    .hover(|s| s.border_color(p.accent))
                    .child(label)
            };
        let row = |label: &'static str, content: AnyElement| {
            div()
                .flex()
                .gap(px(12.))
                .py(px(7.))
                .child(
                    div()
                        .w(px(92.))
                        .flex_none()
                        .pt(px(4.))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.muted)
                        .child(label),
                )
                .child(div().flex_1().flex().flex_wrap().gap(px(6.)).child(content))
        };

        let writing = crate::prefs::WRITING_FONTS
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let label = f.label;
                chip(
                    format!("wf{i}").into(),
                    label.into(),
                    self.prefs.writing().label == label,
                    Some(f.family),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.prefs.writing_font = label.to_string();
                    this.apply_prefs(window, cx);
                }))
                .into_any_element()
            });
        let ui = crate::prefs::UI_FONTS.iter().enumerate().map(|(i, f)| {
            let label = f.label;
            chip(
                format!("uf{i}").into(),
                label.into(),
                self.prefs.ui().label == label,
                Some(f.family),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.prefs.ui_font = label.to_string();
                this.apply_prefs(window, cx);
            }))
            .into_any_element()
        });
        let themes = ThemePref::ALL.iter().map(|t| {
            let t = *t;
            chip(
                format!("th{}", t.label()).into(),
                t.label().into(),
                self.prefs.theme == t,
                None,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.prefs.theme = t;
                this.apply_prefs(window, cx);
            }))
            .into_any_element()
        });
        let layouts = LayoutPref::ALL.iter().map(|l| {
            let l = *l;
            chip(
                format!("ly{}", l.value()).into(),
                l.label().into(),
                self.prefs.layout == l,
                None,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.prefs.layout = l;
                this.apply_prefs(window, cx);
            }))
            .into_any_element()
        });
        let config_path = blyg_core::config::paths::tilde(crate::settings::get(cx).store.primary());
        // Only fonts that are in use get registered at launch; the chips show
        // each face in itself, so make sure they're all available here.
        for f in crate::prefs::WRITING_FONTS {
            crate::fonts::ensure(f.bundled, cx);
        }

        div()
            // The rows (fonts, buttons, AI, help…) outgrow a short window.
            .id("settings-scroll")
            .max_h(px(560.))
            .overflow_y_scroll()
            .track_focus(focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close_sheet(window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                this.close_sheet(window, cx);
            }))
            .child(
                div()
                    .mb(px(6.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(13.))
                    .child("Settings"),
            )
            .child(row(
                "WRITING FONT",
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .children(writing)
                    .into_any_element(),
            ))
            .child(row(
                "INTERFACE FONT",
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .children(ui)
                    .into_any_element(),
            ))
            .child(row(
                "SIZE",
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        chip("sz-".into(), "−".into(), false, None).on_click(cx.listener(
                            |this, _, window, cx| {
                                this.prefs.smaller();
                                this.apply_prefs(window, cx);
                            },
                        )),
                    )
                    .child(
                        div()
                            .w(px(46.))
                            .flex()
                            .justify_center()
                            .child(format!("{} px", self.prefs.font_size)),
                    )
                    .child(
                        chip("sz+".into(), "+".into(), false, None).on_click(cx.listener(
                            |this, _, window, cx| {
                                this.prefs.bigger();
                                this.apply_prefs(window, cx);
                            },
                        )),
                    )
                    .child(div().ml(px(8.)).text_color(p.muted).child("⌘+  ⌘−  ⌘0"))
                    .into_any_element(),
            ))
            .child(row(
                "THEME",
                div().flex().gap(px(6.)).children(themes).into_any_element(),
            ))
            .child(row(
                "LAYOUT",
                div()
                    .flex()
                    .gap(px(6.))
                    .children(layouts)
                    .into_any_element(),
            ))
            .child(self.render_buttons_setting(cx)) // --- buttons ---
            .child(row(
                "QUICK CAPTURE",
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .id("hotkey-box")
                            .px(px(10.))
                            .py(px(5.))
                            .rounded(px(7.))
                            .border_1()
                            .border_color(if hotkey_error.is_some() {
                                p.over
                            } else {
                                p.line
                            })
                            .text_size(px(13.))
                            .child(gpui_kit::base::input::Input::new(hotkey)),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(if hotkey_error.is_some() {
                                p.over
                            } else {
                                p.muted
                            })
                            .child(match hotkey_error {
                                Some(e) => e.to_string(),
                                None => format!(
                                    "Now {} · type e.g. ctrl+alt+b or cmd+shift+space, then ⏎",
                                    prefs::hotkey_glyphs(&self.prefs.hotkey)
                                ),
                            }),
                    )
                    .into_any_element(),
            ))
            .child(self.render_ai_settings_row(cx)) // --- AI ---
            .child(row(
                "CONFIG FILE",
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                chip("open-config".into(), "Open config file".into(), false, None)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.open_config_file_now(cx)),
                                    ),
                            )
                            .child(
                                chip("reload-config".into(), "Reload  ⌘⇧,".into(), false, None)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.reload_config(&ReloadConfig, window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(11.5))
                            .text_color(p.muted)
                            .child(div().truncate().child(format!("Saved in {config_path}")))
                            .child(
                                div()
                                    .truncate()
                                    .child("Every option: blygger +show-config --default --docs"),
                            ),
                    )
                    .into_any_element(),
            ))
            // --- help ---
            .child(self.render_help_settings(cx))
            // --- end help ---
            .child(
                div()
                    .mt(px(10.))
                    .flex()
                    .gap(px(14.))
                    .text_color(p.muted)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .child(kbd("esc"))
                            .child("close"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .child(kbd("⌘,"))
                            .child("toggle"),
                    ),
            )
            .into_any_element()
    }
}
