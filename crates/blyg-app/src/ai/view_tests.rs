//! Headless tests of the AI UI: real keystrokes through GPUI's dispatch,
//! a FakeBackend, and a scripted provider (never a real AI or CLI).

use std::sync::Arc;
use std::time::Duration;

use blyg_ai::cli::CliLocator;
use blyg_ai::{AiError, Endpoints, Provider};
use blyg_core::config::{MemoryTokenStore, TokenStore};
use blyg_core::{Backend, ConfigStore, LocalId};
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

use super::{MainView, Overlay};
use crate::ai::AiGlobal;
use crate::ai::tests_support::FakeProvider;
use crate::app::Mode;
use crate::fake::{FakeBackend, Timing};
use crate::prefs::Prefs;

const CONFIG: &str = "# test config\nblyg-url = https://blyg.example.com\n";
/// The seeded draft that opens first.
const DRAFT: &str = "01J9QK3";
const SECRET: &str = "sk-test-0000aaaa1111bbbb2222cccc3333dddd";

struct Env<'a> {
    view: Entity<MainView>,
    fake: Arc<FakeBackend>,
    tokens: Arc<MemoryTokenStore>,
    cx: &'a mut VisualTestContext,
}

fn setup(cx: &mut TestAppContext, provider: Option<FakeProvider>) -> Env<'_> {
    setup_with(cx, CONFIG, provider)
}

fn setup_with<'a>(
    cx: &'a mut TestAppContext,
    config: &str,
    provider: Option<FakeProvider>,
) -> Env<'a> {
    let config = config.to_string();
    let prefs = Prefs::from_config(ConfigStore::in_memory(&config).config());
    let tokens = Arc::new(MemoryTokenStore::default());
    let t2 = tokens.clone();
    let empty = std::env::temp_dir().join("blygger-ai-tests-no-cli");
    let _ = std::fs::create_dir_all(&empty);
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::app::bind_keys(cx);
        crate::settings::init(ConfigStore::in_memory(&config), t2, None, cx);
        cx.set_global(AiGlobal {
            test_provider: provider.map(|p| Arc::new(p) as Arc<dyn Provider>),
            timeout: Some(Duration::from_secs(60)),
            endpoints: Some(Endpoints {
                cli: CliLocator::only(&empty),
                ..Endpoints::default()
            }),
        });
    });
    let fake = Arc::new(FakeBackend::with_timing(Timing::instant()).without_media_cache());
    let backend: Arc<dyn Backend> = fake.clone();
    let f2 = fake.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        MainView::new(
            backend,
            Some(f2),
            prefs,
            std::time::Instant::now(),
            window,
            cx,
        )
    });
    cx.run_until_parked();
    Env {
        view,
        fake,
        tokens,
        cx,
    }
}

impl Env<'_> {
    fn text(&mut self) -> String {
        self.view
            .read_with(self.cx, |v, cx| v.editor.read(cx).value().to_string())
    }

    /// Open the first draft and put `text` in it, caret at the end.
    fn open_with(&mut self, text: &str) {
        self.cx.simulate_keystrokes("enter");
        self.cx.run_until_parked();
        assert_eq!(self.view.read_with(self.cx, |v, _| v.mode), Mode::Edit);
        self.view.update_in(self.cx, |v, window, cx| {
            let old = v.editor.read(cx).value().to_string();
            v.splice_editor(&old, text, Some(text.len()), window, cx);
        });
        self.cx.run_until_parked();
        assert_eq!(self.text(), text);
    }

    fn wait(&mut self, what: &str, f: impl Fn(&MainView) -> bool) {
        for _ in 0..600 {
            self.cx.executor().advance_clock(Duration::from_millis(30));
            self.cx.run_until_parked();
            if self.view.read_with(self.cx, |v, _| f(v)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    fn wait_idle(&mut self) {
        self.wait("the AI job to finish", |v| v.ai.job.is_none());
    }

    fn toast(&mut self) -> String {
        self.view.read_with(self.cx, |v, _| {
            v.toast
                .as_ref()
                .map(|t| {
                    format!(
                        "{} | {}",
                        t.text,
                        t.sub.as_ref().map(|s| s.to_string()).unwrap_or_default()
                    )
                })
                .unwrap_or_default()
        })
    }

    fn tracked(&self) -> Option<(String, Vec<Option<blyg_core::ScopeProvenance>>)> {
        self.fake.tracked_tk_provenance(&LocalId(DRAFT.into()))
    }
}

#[gpui_kit::test]
fn cmd_g_fills_the_scope_and_records_provenance(cx: &mut TestAppContext) {
    let mut e = setup(
        cx,
        Some(FakeProvider::replying(&[
            "Because the sea keeps its own time.",
        ])),
    );
    e.open_with("Weather again. [TK]say why, briefly[/TK]");
    // Caret just inside the scope.
    e.view.update_in(e.cx, |v, _, cx| {
        v.editor
            .update(cx, |s, cx| s.set_selected_range(20..20, cx));
    });
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-g"));
    e.wait_idle();
    assert_eq!(
        e.text(),
        "Weather again. [TK]say why, briefly[=]Because the sea keeps its own time.[/TK]"
    );
    let saves = e.fake.provenance_saves();
    assert_eq!(saves.last(), Some(&(LocalId(DRAFT.into()), 1)), "{saves:?}");
    let (keyed, scopes) = e.tracked().expect("tracked");
    assert_eq!(keyed, e.text());
    let p = scopes[0].as_ref().expect("generated scope has provenance");
    assert_eq!(p.model, "fake-model-1");
    assert!(p.at.is_some());
    assert!(e.toast().contains("fake-model-1"), "{}", e.toast());

    // Regenerate with ⌘G again: two scopes now, the first keeps its record.
    e.view.update_in(e.cx, |v, window, cx| {
        let old = v.editor.read(cx).value().to_string();
        let new = format!("{old} [TK]and end it[/TK]");
        let end = new.len();
        v.splice_editor(&old, &new, Some(end), window, cx);
    });
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-g"));
    e.wait_idle();
    assert_eq!(e.fake.provenance_saves().last().unwrap().1, 2);
    let (_, scopes) = e.tracked().unwrap();
    assert!(scopes.iter().all(Option::is_some), "{scopes:?}");

    // Rewriting a generated scope entirely by hand → null provenance.
    e.view.update_in(e.cx, |v, window, cx| {
        let old = v.editor.read(cx).value().to_string();
        let new = old.replacen(
            "Because the sea keeps its own time.",
            "Honestly nobody knows.",
            1,
        );
        v.splice_editor(&old, &new, None, window, cx);
    });
    let (_, scopes) = e.tracked().unwrap();
    assert!(scopes[0].is_none() && scopes[1].is_some(), "{scopes:?}");
}

#[gpui_kit::test]
fn esc_cancels_a_running_generation(cx: &mut TestAppContext) {
    let mut e = setup(cx, Some(FakeProvider::hanging()));
    e.open_with("[TK]write something[/TK]");
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-g"));
    e.view.read_with(e.cx, |v, _| {
        let job = v.ai.job.as_ref().expect("running");
        assert_eq!(job.verb, "generating");
        assert_eq!(job.provider.config_name(), "codex");
        assert!(v.render_ai_status().is_some());
    });
    e.cx.simulate_keystrokes("escape");
    e.cx.run_until_parked();
    e.view.read_with(e.cx, |v, _| {
        assert!(v.ai.job.is_none());
        assert_eq!(
            v.mode,
            Mode::Edit,
            "esc cancelled instead of leaving the editor"
        );
    });
    assert!(e.toast().contains("cancelled"));
    std::thread::sleep(Duration::from_millis(30));
    e.cx.executor().advance_clock(Duration::from_millis(100));
    e.cx.run_until_parked();
    assert_eq!(e.text(), "[TK]write something[/TK]", "nothing written");
    assert!(e.fake.provenance_saves().is_empty());
}

#[gpui_kit::test]
fn a_hung_provider_times_out(cx: &mut TestAppContext) {
    let mut e = setup(cx, Some(FakeProvider::hanging()));
    e.open_with("[TK]write something[/TK]");
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-g"));
    e.cx.executor().advance_clock(Duration::from_secs(61));
    e.cx.run_until_parked();
    assert!(e.view.read_with(e.cx, |v, _| v.ai.job.is_none()));
    assert!(e.toast().contains("took longer than 60 s"), "{}", e.toast());
}

#[gpui_kit::test]
fn shorten_proposes_then_accepts_or_rejects(cx: &mut TestAppContext) {
    let mut e = setup(cx, Some(FakeProvider::replying(&["Short and sweet."])));
    let long = "word ".repeat(260);
    e.open_with(long.trim_end());
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-shift-g"));
    e.wait("the proposal", |v| {
        matches!(v.ai.overlay, Some(Overlay::Shorten { .. }))
    });
    assert_eq!(e.text(), long.trim_end(), "nothing changes until accepted");
    // esc keeps the original.
    e.cx.simulate_keystrokes("escape");
    e.cx.run_until_parked();
    assert!(e.view.read_with(e.cx, |v, _| v.ai.overlay.is_none()));
    assert_eq!(e.text(), long.trim_end());

    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-shift-g"));
    e.wait("the proposal", |v| {
        matches!(v.ai.overlay, Some(Overlay::Shorten { .. }))
    });
    e.cx.simulate_keystrokes("enter");
    e.cx.run_until_parked();
    let t = e.text();
    assert!(
        t.starts_with("[TK]shorten to fit 1000") && t.ends_with("[=]Short and sweet.[/TK]"),
        "{t}"
    );
    assert_eq!(e.fake.provenance_saves().last().unwrap().1, 1);
    let (_, scopes) = e.tracked().unwrap();
    assert!(scopes[0].is_some());
}

#[gpui_kit::test]
fn proofread_is_applied_without_disclosure(cx: &mut TestAppContext) {
    let reply = r#"[{"original":"Teh cat","replacement":"The cat","reason":"typo"}]"#;
    let mut e = setup(cx, Some(FakeProvider::replying(&[reply])));
    e.open_with("Teh cat sat on the mat.");
    // No TK under the caret: ⌘G opens the helper palette.
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-g"));
    e.cx.run_until_parked();
    assert!(e.view.read_with(e.cx, |v, _| matches!(
        v.ai.overlay,
        Some(Overlay::Palette { .. })
    )));
    e.cx.simulate_keystrokes("5"); // Proofread
    e.wait("suggestions", |v| {
        matches!(v.ai.overlay, Some(Overlay::Proofread { .. }))
    });
    e.cx.simulate_keystrokes("enter");
    e.cx.run_until_parked();
    assert_eq!(e.text(), "The cat sat on the mat.");
    assert!(
        e.fake.provenance_saves().is_empty(),
        "proofread isn't disclosed"
    );
    assert!(e.tracked().is_none());
}

#[gpui_kit::test]
fn palette_inserts_a_gap_and_continues(cx: &mut TestAppContext) {
    let mut e = setup(
        cx,
        Some(FakeProvider::replying(&["And then the fog lifted."])),
    );
    e.open_with("The harbour was quiet.");
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-g"));
    e.cx.run_until_parked();
    e.cx.simulate_keystrokes("enter"); // first row: fill a gap here
    e.cx.run_until_parked();
    assert_eq!(e.text(), "The harbour was quiet.[TK][/TK]");
    // Undo that and continue the thought instead.
    e.view.update_in(e.cx, |v, window, cx| {
        let old = v.editor.read(cx).value().to_string();
        v.splice_editor(&old, "The harbour was quiet.", Some(22), window, cx);
    });
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-g"));
    e.cx.run_until_parked();
    e.cx.simulate_keystrokes("3"); // continue this thought
    e.wait_idle();
    let t = e.text();
    assert!(
        t.starts_with("The harbour was quiet. [TK]continue this thought"),
        "{t}"
    );
    assert!(t.ends_with("[=]And then the fog lifted.[/TK]"), "{t}");
    assert_eq!(e.fake.provenance_saves().last().unwrap().1, 1);
}

#[gpui_kit::test]
fn publishing_undisclosed_generated_text_warns_first(cx: &mut TestAppContext) {
    let mut e = setup(cx, Some(FakeProvider::replying(&["Out."])));
    e.fake.set_provenance_available(false);
    e.open_with("Hi. [TK]go on[/TK]");
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-g"));
    e.wait_idle();
    assert!(e.text().contains("[=]Out.[/TK]"));

    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-enter"));
    e.cx.run_until_parked();
    e.view.read_with(e.cx, |v, _| {
        assert!(matches!(v.ai.overlay, Some(Overlay::PublishWarning { .. })));
        assert!(v.sheet.is_none(), "no publish sheet behind the warning");
    });
    // ⏎ is Cancel (the default).
    e.cx.simulate_keystrokes("enter");
    e.cx.run_until_parked();
    e.view.read_with(e.cx, |v, _| {
        assert!(v.ai.overlay.is_none() && v.sheet.is_none());
    });
    assert_eq!(e.fake.item(&LocalId(DRAFT.into())).unwrap().version, 0);

    // P publishes anyway: on to the normal publish sheet.
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-enter"));
    e.cx.run_until_parked();
    e.cx.simulate_keystrokes("p");
    e.cx.run_until_parked();
    e.view.read_with(e.cx, |v, _| {
        assert!(v.ai.overlay.is_none());
        assert!(matches!(v.sheet, Some(crate::app::Sheet::Publish { .. })));
    });
}

#[gpui_kit::test]
fn publishing_with_the_extension_does_not_warn(cx: &mut TestAppContext) {
    let mut e = setup(cx, Some(FakeProvider::replying(&["Out."])));
    e.open_with("Hi. [TK]go on[/TK]");
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-g"));
    e.wait_idle();
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-enter"));
    e.cx.run_until_parked();
    e.view.read_with(e.cx, |v, _| {
        assert!(v.ai.overlay.is_none());
        assert!(matches!(v.sheet, Some(crate::app::Sheet::Publish { .. })));
    });
}

#[gpui_kit::test]
fn no_provider_says_how_to_set_one_up(cx: &mut TestAppContext) {
    let mut e = setup(cx, None);
    e.open_with("[TK]x[/TK]");
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-g"));
    e.cx.run_until_parked();
    let t = e.toast();
    assert!(
        t.contains("AI isn't ready") && t.contains("Settings › AI"),
        "{t}"
    );
}

#[gpui_kit::test]
fn an_enabled_cli_that_isnt_installed_says_so(cx: &mut TestAppContext) {
    let mut e = setup_with(cx, &format!("{CONFIG}ai-enable = codex\n"), None);
    e.open_with("[TK]x[/TK]");
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-g"));
    e.cx.run_until_parked();
    let t = e.toast();
    assert!(t.contains("isn't installed") && t.contains("codex"), "{t}");
}

#[gpui_kit::test]
fn a_failing_cli_says_what_to_do(cx: &mut TestAppContext) {
    let p = FakeProvider::failing(AiError::CliFailed {
        name: "codex".into(),
        code: Some(1),
        message: "not logged in".into(),
    });
    let mut e = setup(cx, Some(p));
    e.open_with("[TK]x[/TK]");
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-g"));
    e.wait_idle();
    let t = e.toast();
    assert!(t.contains("exit code 1") && t.contains("signed in"), "{t}");
    assert_eq!(e.text(), "[TK]x[/TK]");
}

#[gpui_kit::test]
fn api_keys_go_to_the_keychain_and_are_never_echoed(cx: &mut TestAppContext) {
    let mut e = setup(cx, None);
    e.view
        .update_in(e.cx, |v, window, cx| v.ai_open_settings(window, cx));
    e.cx.run_until_parked();
    let input = e.view.read_with(e.cx, |v, _| match &v.ai.overlay {
        Some(Overlay::Settings(s)) => s.anthropic_key.clone(),
        _ => panic!("settings open"),
    });
    e.view.update_in(e.cx, |_, window, cx| {
        input.update(cx, |s, cx| s.focus(window, cx));
    });
    e.cx.simulate_input(SECRET);
    e.cx.simulate_keystrokes("enter");
    e.cx.run_until_parked();

    assert_eq!(
        e.tokens
            .get(blyg_ai::accounts::ANTHROPIC_KEY_ACCOUNT)
            .unwrap()
            .as_deref(),
        Some(SECRET)
    );
    let (field, message, row) = e.view.read_with(e.cx, |v, cx| match &v.ai.overlay {
        Some(Overlay::Settings(s)) => (
            s.anthropic_key.read(cx).value().to_string(),
            s.message.clone(),
            s.rows
                .iter()
                .find(|r| r.kind == blyg_ai::ProviderKind::AnthropicApi)
                .cloned()
                .unwrap(),
        ),
        _ => panic!("settings open"),
    });
    assert_eq!(field, "", "the field is cleared at once");
    let (msg, is_err) = message.unwrap();
    assert!(!is_err && msg.contains("saved"), "{msg}");
    assert!(!msg.contains(SECRET) && !msg.contains("sk-test"));
    assert!(!e.toast().contains("sk-test"));
    assert!(row.enabled);
    assert_eq!(crate::ai::settings::status_label(&row).0, "signed in");
    let config = e.cx.update(|_, cx| {
        crate::settings::get(cx)
            .store
            .text()
            .unwrap_or_default()
            .to_string()
    });
    assert!(config.contains("ai-enable = anthropic"), "{config}");
    assert!(!config.contains("sk-test"), "never in the config file");

    // A bad paste is refused without repeating it.
    e.view.update_in(e.cx, |_, window, cx| {
        input.update(cx, |s, cx| {
            s.set_value("sk-test with spaces", window, cx);
            s.focus(window, cx);
        });
    });
    e.cx.simulate_keystrokes("enter");
    e.cx.run_until_parked();
    let msg = e.view.read_with(e.cx, |v, _| match &v.ai.overlay {
        Some(Overlay::Settings(s)) => s.message.clone().unwrap().0,
        _ => panic!(),
    });
    assert!(!msg.contains("sk-test"), "{msg}");

    // Remove forgets it and switches the provider off.
    e.view.update_in(e.cx, |v, _, cx| {
        v.ai_remove(blyg_ai::ProviderKind::AnthropicApi, cx)
    });
    assert_eq!(
        e.tokens
            .get(blyg_ai::accounts::ANTHROPIC_KEY_ACCOUNT)
            .unwrap(),
        None
    );
}

#[gpui_kit::test]
fn settings_switch_a_cli_on_and_pick_it(cx: &mut TestAppContext) {
    let e = setup(cx, None);
    e.view
        .update_in(e.cx, |v, window, cx| v.ai_open_settings(window, cx));
    e.view.update_in(e.cx, |v, window, cx| {
        v.ai_toggle(blyg_ai::ProviderKind::LocalCodex, true, cx);
        v.ai_use(blyg_ai::ProviderKind::LocalCodex, window, cx);
    });
    let config = e.cx.update(|_, cx| {
        crate::settings::get(cx)
            .store
            .text()
            .unwrap_or_default()
            .to_string()
    });
    assert!(config.contains("ai-enable = codex"), "{config}");
    assert!(config.contains("ai-provider = codex"), "{config}");
    // The CLI isn't installed here, so the row says so.
    let word = e.view.read_with(e.cx, |v, _| match &v.ai.overlay {
        Some(Overlay::Settings(s)) => {
            crate::ai::settings::status_label(
                s.rows
                    .iter()
                    .find(|r| r.kind == blyg_ai::ProviderKind::LocalCodex)
                    .unwrap(),
            )
            .0
        }
        _ => panic!(),
    });
    assert_eq!(word, "not installed");
}

// --- follow-ups --- AI reply to a reading item.

const RUE_TRUST: &str = crate::fake::reading_seed::RUE_TRUST;

impl Env<'_> {
    /// ⌘R, then open a reading row (its current version).
    fn open_reading(&mut self, remote_id: &str) {
        self.cx.simulate_keystrokes(&crate::keymap::keys("cmd-r"));
        self.cx.run_until_parked();
        let key = self.view.read_with(self.cx, |v, _| {
            v.reading_rows()
                .iter()
                .find(|r| r.remote_id == remote_id)
                .map(crate::app::reading::vm::key)
                .expect("row shown")
        });
        self.view
            .update_in(self.cx, |v, window, cx| v.open_reading(key, window, cx));
        self.settle();
    }

    fn settle(&mut self) {
        for _ in 0..20 {
            self.cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn stubs(&self) -> Vec<blyg_core::Item> {
        self.fake
            .items()
            .into_iter()
            .filter(|i| i.stub_of.is_some())
            .collect()
    }
}

#[gpui_kit::test]
fn ai_reply_drafts_a_stub_with_a_disclosed_scope(cx: &mut TestAppContext) {
    let mut e = setup(cx, Some(FakeProvider::replying(&["Trust compounds, too."])));
    e.open_reading(RUE_TRUST);
    let current = e
        .fake
        .reading()
        .into_iter()
        .find(|r| r.remote_id == RUE_TRUST)
        .unwrap();
    assert!(e.view.read_with(e.cx, |v, _| {
        v.pill_model().unwrap().actions.contains(&"AI reply")
    }));
    e.view.update_in(e.cx, |v, window, cx| {
        v.reading_action_for_test("AI reply", window, cx)
    });
    e.wait_idle();

    let stubs = e.stubs();
    assert_eq!(stubs.len(), 1);
    let stub = &stubs[0];
    assert_eq!(stub.stub_of.as_ref().unwrap().id, RUE_TRUST);
    assert_eq!(stub.stub_of.as_ref().unwrap().version, current.version);
    assert_eq!(
        stub.status,
        blyg_core::Status::Draft,
        "reviewed, not published"
    );
    assert_eq!(stub.version, 0);
    let text = e.text();
    assert!(text.starts_with(&format!("![[{RUE_TRUST}]]\n\n")), "{text}");
    assert!(text.contains("[=]Trust compounds, too.[/TK]"), "{text}");
    assert_eq!(stub.content_md, text);
    assert_eq!(
        e.view
            .read_with(e.cx, |v, _| v.current.as_ref().map(|c| c.local_id.clone())),
        Some(stub.local_id.clone()),
        "the stub is open for review"
    );
    // Provenance for the generated scope.
    let (_, scopes) = e.fake.tracked_tk_provenance(&stub.local_id).unwrap();
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0].as_ref().unwrap().model, "fake-model-1");
    assert!(e.toast().starts_with("Reply drafted"), "{}", e.toast());
}

#[gpui_kit::test]
fn ai_reply_is_never_offered_on_a_pinned_version(cx: &mut TestAppContext) {
    let mut e = setup(cx, Some(FakeProvider::replying(&["x"])));
    e.open_reading(RUE_TRUST);
    // Step back to the pinned v3.
    e.cx.simulate_keystrokes("left");
    e.settle();
    let actions = e
        .view
        .read_with(e.cx, |v, _| v.pill_model().unwrap().actions);
    assert!(actions.contains(&"Fork this pin"), "{actions:?}");
    assert!(!actions.contains(&"AI reply"), "{actions:?}");
    // Even if asked directly, nothing happens on a pinned view.
    e.view.update_in(e.cx, |v, window, cx| {
        v.reading_action_for_test("AI reply", window, cx)
    });
    e.cx.run_until_parked();
    assert!(e.stubs().is_empty());
    assert!(e.view.read_with(e.cx, |v, _| v.ai.job.is_none()));
}

#[gpui_kit::test]
fn the_palette_reply_row_follows_the_open_reading_item(cx: &mut TestAppContext) {
    let mut e = setup(cx, Some(FakeProvider::replying(&["Agreed, mostly."])));
    e.open_with("Notes on trust.");
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-g"));
    e.cx.run_until_parked();
    let reply_enabled = |e: &mut Env| {
        e.view.read_with(e.cx, |v, _| match &v.ai.overlay {
            Some(Overlay::Palette { entries, .. }) => entries
                .iter()
                .find(|x| x.action == crate::ai::palette::Action::Reply)
                .unwrap()
                .enabled(),
            _ => panic!("no palette"),
        })
    };
    assert!(!reply_enabled(&mut e), "nothing open in Reading yet");
    e.cx.simulate_keystrokes("escape");
    e.cx.run_until_parked();

    e.open_reading(RUE_TRUST);
    // Back to the draft, ⌘G: the row is on now.
    e.view.update_in(e.cx, |v, window, cx| {
        v.show_view(crate::app::reading::View::Posts, window, cx);
        v.open(&LocalId(DRAFT.into()), window, cx);
    });
    e.cx.run_until_parked();
    e.cx.simulate_keystrokes(&crate::keymap::keys("cmd-g"));
    e.cx.run_until_parked();
    assert!(reply_enabled(&mut e));
    e.cx.simulate_keystrokes("6"); // Reply to a reading item
    e.wait_idle();
    let stubs = e.stubs();
    assert_eq!(stubs.len(), 1);
    assert!(
        stubs[0].content_md.contains("[=]Agreed, mostly.[/TK]"),
        "{}",
        stubs[0].content_md
    );
}
