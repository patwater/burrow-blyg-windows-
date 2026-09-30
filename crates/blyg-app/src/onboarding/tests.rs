//! Headless GPUI tests of onboarding and the tutorial: real keystrokes on
//! the test platform, the app's `SwitchBackend` in place, and a recording
//! backend standing in for the user's real blyg.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use blyg_core::config::MemoryTokenStore;
use blyg_core::*;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

use super::FlowStep;
use super::steps::STEPS;
use crate::app::{MainView, Sheet};
use crate::connection::{Connection, Mode, SwitchBackend};
use crate::fake::{FakeBackend, Timing};
use crate::prefs::Prefs;

const CONNECTED: &str = "# test config\nblyg-url = https://blyg.example.com\n";

/// The user's "real" backend: a FakeBackend with one post of its own, and
/// a log of every call that reaches it.
struct Recording {
    inner: FakeBackend,
    calls: Mutex<Vec<&'static str>>,
}

impl Recording {
    fn new() -> Arc<Recording> {
        let inner = FakeBackend::with_timing(Timing::instant()).without_media_cache();
        inner
            .create_draft(Kind::Fragment, "Only on the real blyg: harbour gulls")
            .unwrap();
        Arc::new(Recording {
            inner,
            calls: Mutex::new(vec![]),
        })
    }
    fn rec(&self, name: &'static str) {
        self.calls.lock().unwrap().push(name);
    }
    fn take(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

impl Backend for Recording {
    fn items(&self) -> Vec<Item> {
        self.rec("items");
        self.inner.items()
    }
    fn search(&self, q: &str) -> Vec<Item> {
        self.rec("search");
        self.inner.search(q)
    }
    fn item(&self, id: &LocalId) -> Option<Item> {
        self.rec("item");
        self.inner.item(id)
    }
    fn create_draft(&self, kind: Kind, md: &str) -> Result<LocalId> {
        self.rec("create_draft");
        self.inner.create_draft(kind, md)
    }
    fn save(&self, id: &LocalId, md: &str) -> Result<()> {
        self.rec("save");
        self.inner.save(id, md)
    }
    fn set_kind(&self, id: &LocalId, kind: Kind) -> Result<()> {
        self.rec("set_kind");
        self.inner.set_kind(id, kind)
    }
    fn save_with_provenance(
        &self,
        id: &LocalId,
        md: &str,
        scopes: &[Option<ScopeProvenance>],
    ) -> Result<()> {
        self.rec("save_with_provenance");
        self.inner.save_with_provenance(id, md, scopes)
    }
    fn sync_status(&self) -> SyncStatus {
        self.rec("sync_status");
        self.inner.sync_status()
    }
    fn base_url(&self) -> Option<String> {
        self.rec("base_url");
        self.inner.base_url()
    }
    fn set_event_sink(&self, sink: Box<dyn Fn(CoreEvent) + Send + Sync>) {
        self.rec("set_event_sink");
        self.inner.set_event_sink(sink)
    }
    fn resolve_conflict(&self, id: &LocalId, how: Resolution) -> Result<()> {
        self.rec("resolve_conflict");
        self.inner.resolve_conflict(id, how)
    }
    fn reading(&self) -> Vec<ReadingItem> {
        self.rec("reading");
        self.inner.reading()
    }
    fn subscriptions(&self) -> Vec<Subscription> {
        self.rec("subscriptions");
        self.inner.subscriptions()
    }
    fn create_scratch(&self, kind: Kind, md: &str) -> Result<LocalId> {
        self.rec("create_scratch");
        self.inner.create_scratch(kind, md)
    }
    fn promote(&self, id: &LocalId, to: Promote) -> Result<Promoted> {
        self.rec("promote");
        self.inner.promote(id, to)
    }
    fn publish(&self, id: &LocalId, note: Option<&str>) -> Result<PublishOutcome> {
        self.rec("publish");
        self.inner.publish(id, note)
    }
    fn withdraw(&self, id: &LocalId, note: Option<&str>) -> Result<u32> {
        self.rec("withdraw");
        self.inner.withdraw(id, note)
    }
    fn pin(&self, id: &LocalId, v: u32) -> Result<()> {
        self.rec("pin");
        self.inner.pin(id, v)
    }
    fn versions(&self, id: &LocalId) -> Result<Vec<Version>> {
        self.rec("versions");
        self.inner.versions(id)
    }
    fn restore(&self, id: &LocalId, v: u32) -> Result<()> {
        self.rec("restore");
        self.inner.restore(id, v)
    }
    fn delete_draft(&self, id: &LocalId) -> Result<()> {
        self.rec("delete_draft");
        self.inner.delete_draft(id)
    }
    fn upload_media(
        &self,
        bytes: Vec<u8>,
        mime: &str,
        item: Option<&LocalId>,
        alt: Option<&str>,
    ) -> Result<MediaRef> {
        self.rec("upload_media");
        self.inner.upload_media(bytes, mime, item, alt)
    }
    fn fork(&self, of: &RemoteRef) -> Result<LocalId> {
        self.rec("fork");
        self.inner.fork(of)
    }
    fn set_show_responses(&self, id: &LocalId, show: bool) -> Result<()> {
        self.rec("set_show_responses");
        self.inner.set_show_responses(id, show)
    }
    fn sync_now(&self) -> Result<()> {
        self.rec("sync_now");
        self.inner.sync_now()
    }
    fn preview_subscription(&self, url: &str) -> Result<SubscribePreview> {
        self.rec("preview_subscription");
        self.inner.preview_subscription(url)
    }
    fn subscribe(&self, url: &str, title: Option<&str>) -> Result<Subscription> {
        self.rec("subscribe");
        self.inner.subscribe(url, title)
    }
    fn unsubscribe(&self, sub: &str) -> Result<()> {
        self.rec("unsubscribe");
        self.inner.unsubscribe(sub)
    }
    fn set_subscription(&self, sub: &str, b: Option<bool>, t: Option<&str>) -> Result<()> {
        self.rec("set_subscription");
        self.inner.set_subscription(sub, b, t)
    }
    fn pause_subscription(&self, sub: &str, paused: bool) -> Result<()> {
        self.rec("pause_subscription");
        self.inner.pause_subscription(sub, paused)
    }
    fn signal(&self, sub: &str, id: &str, thumb: Option<i8>) -> Result<()> {
        self.rec("signal");
        self.inner.signal(sub, id, thumb)
    }
    fn mentions(&self) -> Result<Vec<Mention>> {
        self.rec("mentions");
        self.inner.mentions()
    }
    fn set_mention_hidden(&self, id: &str, hidden: bool) -> Result<()> {
        self.rec("set_mention_hidden");
        self.inner.set_mention_hidden(id, hidden)
    }
    fn settings(&self) -> Result<Settings> {
        self.rec("settings");
        self.inner.settings()
    }
    fn save_settings(&self, s: &Settings) -> Result<()> {
        self.rec("save_settings");
        self.inner.save_settings(s)
    }
    fn read_extensions_available(&self) -> bool {
        self.rec("read_extensions_available");
        self.inner.read_extensions_available()
    }
    fn provenance_available(&self) -> bool {
        self.rec("provenance_available");
        self.inner.provenance_available()
    }
    fn tracked_tk_provenance(
        &self,
        id: &LocalId,
    ) -> Option<(String, Vec<Option<ScopeProvenance>>)> {
        self.rec("tracked_tk_provenance");
        self.inner.tracked_tk_provenance(id)
    }
}

struct Env<'a> {
    view: Entity<MainView>,
    real: Arc<Recording>,
    switch: Arc<SwitchBackend>,
    dir: tempfile::TempDir,
    cx: &'a mut VisualTestContext,
}

/// The window as main.rs builds it: a `SwitchBackend` over the "real"
/// backend in the `Connection` global, with a data dir for `state.json`.
fn setup<'a>(cx: &'a mut TestAppContext, config: &str, onboarded: bool) -> Env<'a> {
    setup_mode(cx, config, onboarded, Mode::Fake)
}

fn setup_mode<'a>(
    cx: &'a mut TestAppContext,
    config: &str,
    onboarded: bool,
    mode: Mode,
) -> Env<'a> {
    let dir = tempfile::tempdir().unwrap();
    if onboarded {
        state::AppState::update(dir.path(), |s| s.onboarded = true).unwrap();
    }
    let real = Recording::new();
    let switch = SwitchBackend::new(real.clone(), mode);
    let config = config.to_string();
    let prefs = Prefs::from_config(ConfigStore::in_memory(&config).config());
    let data_dir = dir.path().to_path_buf();
    let sw = switch.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::app::bind_keys(cx);
        crate::settings::init(
            ConfigStore::in_memory(&config),
            Arc::new(MemoryTokenStore::default()),
            None,
            cx,
        );
        cx.set_global(Connection {
            switch: sw,
            data_dir,
            verify: |_, _| Ok(1),
        });
    });
    let backend: Arc<dyn Backend> = switch.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        MainView::new(backend, None, prefs, std::time::Instant::now(), window, cx)
    });
    cx.run_until_parked();
    Env {
        view,
        real,
        switch,
        dir,
        cx,
    }
}

fn config_text(cx: &mut VisualTestContext) -> String {
    cx.update(|_, cx| {
        crate::settings::get(cx)
            .store
            .text()
            .unwrap_or_default()
            .to_string()
    })
}

fn flow_step(e: &mut Env) -> Option<FlowStep> {
    e.view
        .read_with(e.cx, |v, _| v.onboarding.flow.as_ref().map(|f| f.step))
}

fn tour_step(e: &mut Env) -> Option<&'static str> {
    e.view.read_with(e.cx, |v, _| {
        v.onboarding.tutorial.as_ref().map(|t| STEPS[t.step].id)
    })
}

/// Let a step's ✓ pause run out.
fn settle(e: &mut Env) {
    e.cx.executor().advance_clock(Duration::from_secs(4));
    e.cx.run_until_parked();
}

fn real_is_back(e: &Env) -> bool {
    let cur = e.switch.current();
    std::ptr::addr_eq(Arc::as_ptr(&cur), Arc::as_ptr(&e.real))
}

#[gpui_kit::test]
fn first_launch_without_a_blyg_shows_onboarding(cx: &mut TestAppContext) {
    let mut e = setup(cx, "# fresh\n", false);
    assert_eq!(flow_step(&mut e), Some(FlowStep::Welcome));
    assert!(
        e.view.read_with(e.cx, |v, _| v.sheet.is_none()),
        "the Connect sheet waits for its step"
    );
    // ⏎ goes on to "Connect your blyg"; ⏎ there opens the existing sheet.
    e.cx.simulate_keystrokes("enter");
    e.cx.run_until_parked();
    assert_eq!(flow_step(&mut e), Some(FlowStep::Connect));
    e.cx.simulate_keystrokes("enter");
    e.cx.run_until_parked();
    assert!(
        e.view
            .read_with(e.cx, |v, _| matches!(v.sheet, Some(Sheet::Connect { .. })))
    );
    // esc on the sheet comes back to the step.
    e.cx.simulate_keystrokes("escape");
    e.cx.run_until_parked();
    assert_eq!(flow_step(&mut e), Some(FlowStep::Connect));
    assert!(e.view.read_with(e.cx, |v, _| v.sheet.is_none()));
}

#[gpui_kit::test]
fn connected_but_never_onboarded_skips_onboarding(cx: &mut TestAppContext) {
    // Set up before onboarding existed: don't make them walk through it.
    let mut e = setup(cx, CONNECTED, false);
    assert_eq!(flow_step(&mut e), None);
}

#[gpui_kit::test]
fn onboarded_and_connected_launches_straight_in(cx: &mut TestAppContext) {
    let mut e = setup(cx, CONNECTED, true);
    assert_eq!(flow_step(&mut e), None);
    assert_eq!(tour_step(&mut e), None);
}

#[gpui_kit::test]
fn skip_lands_in_the_app(cx: &mut TestAppContext) {
    let mut e = setup_mode(cx, "# fresh\n", false, Mode::Disconnected);
    assert_eq!(flow_step(&mut e), Some(FlowStep::Welcome));
    e.cx.simulate_keystrokes("escape");
    e.cx.run_until_parked();
    assert_eq!(flow_step(&mut e), None);
    assert_eq!(tour_step(&mut e), None);
    assert!(state::AppState::load(e.dir.path()).onboarded, "remembered");
    assert!(
        !config_text(e.cx).contains("onboard"),
        "app state, not config"
    );
    // The omnibar has the keyboard: typing filters.
    e.cx.simulate_input("harbour");
    e.cx.run_until_parked();
    // (the seed's Harbour Road post and the real backend's own)
    assert_eq!(e.view.read_with(e.cx, |v, _| v.list.results().len()), 2);
}

#[gpui_kit::test]
fn sample_data_runs_the_session_on_the_fake(cx: &mut TestAppContext) {
    let mut e = setup_mode(cx, "# fresh\n", false, Mode::Disconnected);
    e.cx.simulate_keystrokes("enter");
    e.cx.simulate_keystrokes("s");
    e.cx.run_until_parked();
    assert_eq!(flow_step(&mut e), Some(FlowStep::Ai));
    assert!(!real_is_back(&e), "the switch holds sample data");
    assert_eq!(e.switch.mode(), Mode::Fake);
    assert!(e.view.read_with(e.cx, |v, _| v.list.results().len() >= 6));
    assert!(!config_text(e.cx).contains("blyg-url"));
}

#[gpui_kit::test]
fn buttons_or_keyboard_writes_the_config(cx: &mut TestAppContext) {
    let mut e = setup(cx, CONNECTED, true);
    // Straight to the step (as "Show onboarding again" would get there).
    e.view.update_in(e.cx, |v, window, cx| {
        v.open_onboarding(FlowStep::Buttons, window, cx)
    });
    e.cx.run_until_parked();
    assert_eq!(flow_step(&mut e), Some(FlowStep::Buttons));
    e.cx.simulate_keystrokes("2");
    e.cx.run_until_parked();
    assert!(
        config_text(e.cx).contains("show-buttons = false"),
        "{}",
        config_text(e.cx)
    );
    assert_eq!(flow_step(&mut e), Some(FlowStep::Tutorial));
    // Back, and choose buttons: the same line is rewritten in place.
    e.cx.simulate_keystrokes("left 1");
    e.cx.run_until_parked();
    let text = config_text(e.cx);
    assert!(text.contains("show-buttons = true"), "{text}");
    assert_eq!(text.matches("show-buttons").count(), 1, "{text}");
    // ⏎ on the last step starts the tour.
    e.cx.simulate_keystrokes("enter");
    e.cx.run_until_parked();
    assert_eq!(tour_step(&mut e), Some("search"));
    assert!(state::AppState::load(e.dir.path()).onboarded);
}

#[gpui_kit::test]
fn tutorial_steps_advance_on_their_key_only(cx: &mut TestAppContext) {
    let mut e = setup(cx, CONNECTED, true);
    e.cx.dispatch_action(super::ShowTutorial);
    e.cx.run_until_parked();
    assert_eq!(tour_step(&mut e), Some("search"));

    // ⌘T isn't this step's key.
    e.cx.simulate_keystrokes("cmd-t");
    settle(&mut e);
    assert_eq!(tour_step(&mut e), Some("search"));
    // ↓ is.
    e.cx.simulate_keystrokes("down");
    e.cx.run_until_parked();
    assert!(
        e.view
            .read_with(e.cx, |v, _| v.onboarding.tutorial.as_ref().unwrap().done)
    );
    settle(&mut e);
    assert_eq!(tour_step(&mut e), Some("create"));

    // ⏎ with no match creates the seeded draft (in the sample data).
    e.cx.simulate_keystrokes("cmd-t");
    settle(&mut e);
    assert_eq!(tour_step(&mut e), Some("create"));
    e.cx.simulate_keystrokes("enter");
    settle(&mut e);
    assert_eq!(tour_step(&mut e), Some("autosave"));
    let (cur, fake) = e
        .view
        .read_with(e.cx, |v, _| (v.current.clone(), v.fake.clone()));
    assert!(cur.unwrap().content_md.starts_with("tide pools"));

    // Typing advances "Just write"; ↓ doesn't.
    e.cx.simulate_keystrokes("cmd-l down");
    settle(&mut e);
    assert_eq!(tour_step(&mut e), Some("autosave"));
    e.view.update_in(e.cx, |v, window, cx| {
        v.tutorial_enter(2, window, cx);
    });
    e.cx.simulate_input(" at low tide");
    settle(&mut e);
    assert_eq!(tour_step(&mut e), Some("kinds"));
    e.cx.simulate_keystrokes("cmd-t");
    settle(&mut e);
    assert_eq!(tour_step(&mut e), Some("capture"));
    // The scratch note is in the tutorial's fake; ⌘D makes it a draft.
    e.cx.simulate_keystrokes("cmd-d");
    settle(&mut e);
    assert_eq!(tour_step(&mut e), Some("publish"));
    let fake = fake.expect("the tutorial's fake");
    assert!(
        fake.items()
            .iter()
            .any(|i| i.content_md.contains("quick capture") && i.status == Status::Draft)
    );

    // Next and Back work at any time.
    e.cx.simulate_keystrokes("alt-cmd-right");
    e.cx.run_until_parked();
    assert_eq!(tour_step(&mut e), Some("publish-note"));
    e.cx.simulate_keystrokes("alt-cmd-left");
    e.cx.run_until_parked();
    assert_eq!(tour_step(&mut e), Some("publish"));
}

#[gpui_kit::test]
fn tutorial_ring_never_draws_over_a_picker(cx: &mut TestAppContext) {
    let mut e = setup(cx, CONNECTED, true);
    e.cx.dispatch_action(super::ShowTutorial);
    e.cx.run_until_parked();
    let quotes = STEPS.iter().position(|s| s.id == "quotes").unwrap();
    e.view.update_in(e.cx, |v, window, cx| {
        v.tutorial_enter(quotes, window, cx);
    });
    settle(&mut e);
    let region = STEPS[quotes].region;
    let ring = |e: &mut Env| {
        e.view
            .update_in(e.cx, |v, window, _| v.tutorial_ring(region, window))
    };
    assert!(ring(&mut e).is_some(), "the editor is ringed before ⌘K");
    e.cx.simulate_keystrokes("cmd-k");
    e.cx.run_until_parked();
    assert!(e.view.read_with(e.cx, |v, _| v.reading.sheet.is_some()));
    assert_eq!(ring(&mut e), None, "the picker covers the editor");
}

#[gpui_kit::test]
fn tutorial_on_launch_shows_it_on_start(cx: &mut TestAppContext) {
    let mut e = setup(cx, &format!("{CONNECTED}tutorial-on-launch = true\n"), true);
    assert_eq!(tour_step(&mut e), Some("search"));
    assert_eq!(flow_step(&mut e), None);
    // The checkbox writes the key back.
    e.view
        .update(e.cx, |v, cx| v.set_tutorial_on_launch(false, cx));
    assert!(config_text(e.cx).contains("tutorial-on-launch = false"));
}

#[gpui_kit::test]
fn finishing_restores_the_real_backend(cx: &mut TestAppContext) {
    let mut e = setup(cx, CONNECTED, true);
    e.cx.simulate_input("harb");
    e.cx.run_until_parked();
    e.cx.dispatch_action(super::ShowTutorial);
    e.cx.run_until_parked();
    assert!(!real_is_back(&e));
    // From here on nothing may reach the real backend.
    e.real.take();

    // Publish in the tour: jump to the step, ⌘⏎, a note, ⏎.
    let publish = STEPS.iter().position(|s| s.id == "publish").unwrap();
    e.view
        .update_in(e.cx, |v, window, cx| v.tutorial_enter(publish, window, cx));
    e.cx.run_until_parked();
    e.cx.simulate_keystrokes("cmd-enter");
    settle(&mut e);
    assert_eq!(tour_step(&mut e), Some("publish-note"));
    e.cx.simulate_input("first version");
    e.cx.simulate_keystrokes("enter");
    settle(&mut e);
    let fake = e.view.read_with(e.cx, |v, _| v.fake.clone()).unwrap();
    assert!(
        fake.item(&LocalId(super::steps::DRAFT.into()))
            .is_some_and(|i| i.status == Status::Public),
        "published in the sample data"
    );
    // Walk the rest of the tour with its keys and Next.
    for _ in 0..STEPS.len() {
        e.cx.simulate_keystrokes("alt-cmd-right");
        e.cx.run_until_parked();
    }
    settle(&mut e);
    assert_eq!(tour_step(&mut e), None, "finished");
    let during: Vec<_> = e.real.take();
    // Finishing re-attaches the real backend (its event sink) and reads it.
    let first_other = during.iter().position(|c| *c == "set_event_sink");
    assert!(first_other.is_some(), "{during:?}");
    assert!(real_is_back(&e));
    assert_eq!(e.switch.mode(), Mode::Fake, "the mode it had before");
    e.view.read_with(e.cx, |v, _| {
        assert_eq!(v.list.query(), "harb", "the query came back");
        assert!(
            v.list
                .results()
                .iter()
                .all(|i| i.content_md.contains("arb"))
        );
        assert!(v.fake.is_none());
    });
    // The real backend never saw the tour's publish.
    assert!(
        e.real
            .inner
            .item(&LocalId(super::steps::DRAFT.into()))
            .is_some_and(|i| i.status == Status::Draft)
    );
}

#[gpui_kit::test]
fn no_call_reaches_the_real_backend_during_the_tour(cx: &mut TestAppContext) {
    let mut e = setup(cx, CONNECTED, true);
    e.cx.dispatch_action(super::ShowTutorial);
    e.cx.run_until_parked();
    e.real.take();
    for step in 0..STEPS.len() {
        e.view
            .update_in(e.cx, |v, window, cx| v.tutorial_enter(step, window, cx));
        e.cx.run_until_parked();
    }
    e.cx.simulate_keystrokes("cmd-3 cmd-1 cmd-r cmd-r cmd-y");
    settle(&mut e);
    assert_eq!(e.real.take(), Vec::<&str>::new());
    e.view
        .update_in(e.cx, |v, window, cx| v.finish_tutorial(window, cx));
    e.cx.run_until_parked();
    assert!(real_is_back(&e));
    assert!(
        !e.real.take().is_empty(),
        "the window reads the real backend again"
    );
}

#[gpui_kit::test]
fn replay_from_settings(cx: &mut TestAppContext) {
    let mut e = setup(cx, CONNECTED, true);
    e.cx.simulate_keystrokes("cmd-,");
    e.cx.run_until_parked();
    assert!(
        e.view
            .read_with(e.cx, |v, _| matches!(v.sheet, Some(Sheet::Settings { .. })))
    );
    e.view
        .update_in(e.cx, |v, window, cx| v.help_replay(window, cx));
    e.cx.run_until_parked();
    assert_eq!(tour_step(&mut e), Some("search"));
    assert!(e.view.read_with(e.cx, |v, _| v.sheet.is_none()));
    // "Show onboarding again" ends the tour and opens the flow.
    e.cx.simulate_keystrokes("cmd-,");
    e.cx.run_until_parked();
    e.view
        .update_in(e.cx, |v, window, cx| v.help_onboarding(window, cx));
    e.cx.run_until_parked();
    assert_eq!(tour_step(&mut e), None);
    assert_eq!(flow_step(&mut e), Some(FlowStep::Welcome));
    assert!(real_is_back(&e));
}
