//! The main window's AI UI: ⌘G / ⇧⌘G, the helper palette, the shorten and
//! proofread proposals, the publish-without-disclosure warning, the
//! "generating… (codex)" status, and Settings › AI.
//!
//! This file is a child module of `app` (declared there with `#[path]`) so
//! it can use the window's state; `app.rs` only carries small hooks into it.
//! The UI-free logic is in `crate::ai`.
//!
//! Providers run on their own thread (they block: HTTP streams, CLI
//! children). esc sets the shared cancel flag, which kills a CLI child; a
//! result that arrives after esc or the timeout is thrown away.

use std::sync::Arc;

use blyg_ai::prompts::{self, Suggestion};
use blyg_ai::{
    AccountRow, CancelFlag, GenRequest, Provider, ProviderKind, ProviderStatus, ServerScope,
};
use blyg_core::{Item, Kind, LocalId, ReadingItem, ScopeProvenance};
use gpui_kit::base::input::{Escape, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{MainView, TITLEBAR_H};
use crate::ai::generate::{self, Under};
use crate::ai::palette::{self, Action};
use crate::ai::settings as ais;
use crate::ai::{self, AiGenerate, AiShorten, Failure, Picked};

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;

/// Per-window AI state (a field of `MainView`).
#[derive(Default)]
pub struct AiView {
    job: Option<Job>,
    job_seq: usize,
    overlay: Option<Overlay>,
    /// The next `after_edit` save of exactly this text carries this
    /// provenance (set just before a generated splice into the editor).
    pending: Option<(String, Vec<Option<ScopeProvenance>>)>,
    /// One-shot: the user chose "publish anyway" on the disclosure warning.
    publish_anyway: bool,
}

struct Job {
    id: usize,
    provider: ProviderKind,
    cancel: CancelFlag,
    /// "generating", "shortening", …
    verb: &'static str,
}

enum Overlay {
    Palette {
        entries: Vec<palette::Entry>,
        selected: usize,
        focus: FocusHandle,
    },
    Shorten {
        item: LocalId,
        before: String,
        after: String,
        model: String,
        len: usize,
        fits: bool,
        focus: FocusHandle,
    },
    Proofread {
        item: LocalId,
        text: String,
        suggestions: Vec<Suggestion>,
        focus: FocusHandle,
    },
    PublishWarning {
        focus: FocusHandle,
    },
    Settings(Box<SettingsPanel>),
}

struct SettingsPanel {
    focus: FocusHandle,
    rows: Vec<AccountRow>,
    anthropic_key: Entity<InputState>,
    openai_key: Entity<InputState>,
    cf_account: Entity<InputState>,
    cf_token: Entity<InputState>,
    model: Entity<InputState>,
    /// (text, is_error). Never contains a secret.
    message: Option<(String, bool)>,
    /// The "unofficial" note is showing, waiting for "Continue".
    chatgpt_notice: bool,
    /// A ChatGPT browser sign-in is running.
    chatgpt_running: Option<CancelFlag>,
}

/// What finished, carried from the provider thread back to the UI.
enum Done {
    Fill {
        item: LocalId,
        index: usize,
        instruction: String,
        sources: Vec<blyg_core::ProvenanceSource>,
        text: String,
        model: String,
    },
    /// The blyg generated and recorded provenance itself: pull.
    Server,
    Shorten {
        item: LocalId,
        before: String,
        outcome: prompts::ShortenOutcome,
    },
    Continue {
        item: LocalId,
        before: String,
        at: usize,
        out: prompts::HelperOutput,
    },
    Outline {
        out: prompts::HelperOutput,
    },
    /// The reply helper's text for the stub thread `item`.
    Reply {
        item: LocalId,
        out: prompts::HelperOutput,
    },
    Proofread {
        item: LocalId,
        text: String,
        suggestions: Vec<Suggestion>,
    },
}

/// How often the UI checks on a provider thread.
const POLL: std::time::Duration = std::time::Duration::from_millis(30);

fn prov(model: &str, sources: Vec<blyg_core::ProvenanceSource>) -> ScopeProvenance {
    ScopeProvenance {
        model: model.to_string(),
        sources,
        at: Some(generate::now_iso()),
    }
}

impl MainView {
    // ------------------------------------------------------------ hooks

    /// esc: cancel a running generation (or close an AI overlay). True if
    /// esc was used.
    pub(super) fn ai_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(job) = self.ai.job.take() {
            job.cancel.cancel();
            self.show_toast(
                "Generation cancelled",
                Some("Nothing was written".into()),
                cx,
            );
            cx.notify();
            return true;
        }
        if self.ai.overlay.is_some() {
            self.ai_close(window, cx);
            return true;
        }
        false
    }

    /// Save an edit from the editor. A generated splice carries its
    /// provenance; an edit that changes the set of TK scopes pushes the
    /// carried-over provenance with it (never a plain save that would let
    /// the server's position-keyed disclosure slide).
    pub(super) fn ai_save_edit(&mut self, id: &LocalId, text: &str) -> blyg_core::Result<()> {
        if let Some((t, scopes)) = self.ai.pending.take()
            && t == text
        {
            return self.backend.save_with_provenance(id, text, &scopes);
        }
        if let Some(item) = self.current.as_ref().filter(|c| &c.local_id == id)
            && let Some(scopes) =
                generate::edit_provenance(&*self.backend, item, &item.content_md, text)
        {
            return self.backend.save_with_provenance(id, text, &scopes);
        }
        self.backend.save(id, text)
    }

    /// Before the publish sheet: warn if this would publish generated text
    /// without disclosure. True = stop (the warning is showing).
    pub(super) fn ai_publish_guard(
        &mut self,
        item: &Item,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if std::mem::take(&mut self.ai.publish_anyway) {
            return false;
        }
        if !generate::publish_needs_warning(&*self.backend, item) {
            return false;
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.ai.overlay = Some(Overlay::PublishWarning { focus });
        cx.notify();
        true
    }

    pub(super) fn ai_generate(
        &mut self,
        _: &AiGenerate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.ai.overlay, Some(Overlay::Palette { .. })) {
            self.ai_close(window, cx);
            return;
        }
        if self.ai_busy(cx) {
            return;
        }
        let Some(item) = self.current.clone() else {
            self.show_toast("Open a post first, then ⌘G inside [TK]…[/TK]", None, cx);
            return;
        };
        let text = self.editor.read(cx).value().to_string();
        let caret = self.editor.read(cx).cursor();
        match generate::under_caret(&text, caret) {
            Under::Scope(index) => self.ai_fill(item, text, index, window, cx),
            Under::Malformed => self.show_toast(
                "This TK isn't closed yet",
                Some("End it with [/TK], then ⌘G again".into()),
                cx,
            ),
            Under::Nothing => self.ai_open_palette(&item, &text, window, cx),
        }
    }

    pub(super) fn ai_shorten(
        &mut self,
        _: &AiShorten,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ai_run(Action::Shorten, window, cx);
    }

    /// The status bar's "generating… (codex) · esc cancels".
    pub(super) fn render_ai_status(&self) -> Option<AnyElement> {
        let job = self.ai.job.as_ref()?;
        let p = self.palette;
        Some(
            div()
                .id("ai-status")
                .flex()
                .items_center()
                .gap(px(6.))
                .text_color(p.accent)
                .child(
                    div()
                        .size(px(7.))
                        .rounded_full()
                        .bg(p.accent)
                        .with_animation(
                            "ai-pulse",
                            Animation::new(std::time::Duration::from_millis(1100)).repeat(),
                            |d, t| d.opacity(1.0 - 0.75 * (1.0 - (2.0 * t - 1.0).abs())),
                        ),
                )
                .child(format!(
                    "{}… ({}) · esc cancels",
                    job.verb,
                    job.provider.config_name()
                ))
                .into_any_element(),
        )
    }

    /// "AI reply" (the reading screen's current version, and the ⌘G
    /// palette row): a stub thread that transcludes `item` (stubs are the
    /// reply shape), then the reply helper fills a TK scope under the quote.
    /// The draft opens for review; nothing publishes until ⌘⏎. `item` is
    /// the reading row, i.e. the current version: an unpinned past version
    /// of someone else's post never reaches the prompt.
    pub(super) fn ai_reply_to(
        &mut self,
        item: ReadingItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if item.state == "tombstone" {
            self.show_toast("This post was withdrawn", None, cx);
            return;
        }
        if self.ai_busy(cx) {
            return;
        }
        // Pick the provider first: no empty stub when AI isn't set up.
        let Some(picked) = self.ai_pick(cx) else {
            return;
        };
        let of = blyg_core::RemoteRef {
            origin: item.origin.clone(),
            id: item.remote_id.clone(),
            version: item.version,
        };
        let body = super::reading::vm::stub_body(&item.remote_id);
        let stub = match self.backend.create_stub(&of, &body) {
            Ok(id) => id,
            Err(e) => {
                self.show_toast(format!("Couldn't start a reply: {e}"), None, cx);
                return;
            }
        };
        if self.ai.overlay.is_some() {
            self.ai_close(window, cx);
        }
        self.open_new_draft(&stub, window, cx);
        let style = ai::style_prompt(cx);
        self.ai_spawn(
            picked,
            "drafting a reply",
            move |p, cancel| {
                prompts::reply_draft(p, &item, style.as_deref(), cancel, &mut |_| {})
                    .map(|out| Done::Reply { item: stub, out })
            },
            window,
            cx,
        );
    }

    /// The reading item the palette's "Reply to a reading item" answers:
    /// the one open in Reading (its current version), unless withdrawn.
    fn ai_reply_target(&self) -> Option<ReadingItem> {
        self.reading
            .opened
            .as_ref()
            .map(|o| o.item.clone())
            .filter(|i| i.state != "tombstone")
    }

    /// `BLYGGER_DEMO=ai-…` steps (see demo.rs): the same calls a keystroke
    /// makes, for screenshots and smoke tests without synthetic OS input.
    pub(super) fn ai_demo_step(
        &mut self,
        scenario: &str,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (scenario, n) {
            ("ai-fill" | "ai-palette" | "ai-shorten", 0) => {
                self.open(&LocalId("01J9QK3".into()), window, cx);
                let old = self.editor.read(cx).value().to_string();
                let new = if scenario == "ai-fill" {
                    format!("{old} [TK]one short sentence about fog, in my voice[/TK]")
                } else if scenario == "ai-shorten" {
                    format!(
                        "{old} {}",
                        "The log records wind, swell and visibility, hour after hour. ".repeat(18)
                    )
                } else {
                    old.clone()
                };
                let caret = if scenario == "ai-fill" {
                    new.len() - 5
                } else {
                    new.len()
                };
                self.splice_editor(&old, &new, Some(caret), window, cx);
            }
            ("ai-fill" | "ai-palette", 1) => self.ai_generate(&AiGenerate, window, cx),
            ("ai-shorten", 1) => self.ai_shorten(&AiShorten, window, cx),
            ("ai-settings", 0) => self.ai_open_settings(window, cx),
            _ => {}
        }
    }

    // ------------------------------------------------------------ running

    fn ai_busy(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(job) = &self.ai.job {
            let who = job.provider.config_name();
            self.show_toast(
                format!("Still {}… ({who})", job.verb),
                Some("esc cancels it".into()),
                cx,
            );
            return true;
        }
        false
    }

    fn ai_pick(&mut self, cx: &mut Context<Self>) -> Option<Picked> {
        match ai::pick(cx) {
            Ok(p) => Some(p),
            Err((e, kind)) => {
                self.ai_fail(Failure::Ai(e), kind, cx);
                None
            }
        }
    }

    fn ai_fail(&mut self, f: Failure, kind: Option<ProviderKind>, cx: &mut Context<Self>) {
        let (head, sub) = ai::failure_toast(&f, kind);
        if timing() {
            println!("ai failed: {head} · {sub}");
        }
        self.show_toast(head, Some(sub.into()), cx);
    }

    /// Run `work` on its own thread with the picked provider; `Done` comes
    /// back to the UI unless esc or the timeout got there first.
    fn ai_spawn(
        &mut self,
        picked: Picked,
        verb: &'static str,
        work: impl FnOnce(&dyn Provider, &CancelFlag) -> blyg_ai::Result<Done> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ai.job_seq += 1;
        let id = self.ai.job_seq;
        let cancel = CancelFlag::new();
        let kind = picked.kind;
        self.ai.job = Some(Job {
            id,
            provider: kind,
            cancel: cancel.clone(),
            verb,
        });
        cx.notify();

        let (tx, rx) = async_channel::bounded(1);
        let provider: Arc<dyn Provider> = picked.provider;
        let spawned = std::thread::Builder::new()
            .name("blygger-ai".into())
            .spawn(move || {
                let r = work(provider.as_ref(), &cancel);
                let _ = tx.try_send(r);
            });
        if let Err(e) = spawned {
            self.ai.job = None;
            self.show_toast(format!("Couldn't start the AI helper: {e}"), None, cx);
            return;
        }
        // Poll rather than await the channel: the provider thread never has
        // to wake the UI executor (which GPUI's deterministic test
        // scheduler forbids), and a cancelled job just stops polling.
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let result = rx.try_recv();
                let alive = this.update_in(cx, |v, window, cx| {
                    if v.ai.job.as_ref().map(|j| j.id) != Some(id) {
                        return false; // cancelled or timed out: drop it
                    }
                    let Ok(result) = result else {
                        return true;
                    };
                    v.ai.job = None;
                    match result {
                        Ok(done) => v.ai_done(done, kind, window, cx),
                        Err(blyg_ai::AiError::Cancelled) => {}
                        Err(e) => v.ai_fail(Failure::Ai(e), Some(kind), cx),
                    }
                    cx.notify();
                    false
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        let limit = ai::timeout(cx);
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(limit).await;
            let _ = this.update_in(cx, |v, _, cx| {
                if v.ai.job.as_ref().map(|j| j.id) == Some(id)
                    && let Some(job) = v.ai.job.take()
                {
                    job.cancel.cancel();
                    v.ai_fail(Failure::TimedOut(limit), Some(kind), cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn ai_fill(
        &mut self,
        item: Item,
        text: String,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picked) = self.ai_pick(cx) else {
            return;
        };
        if picked.kind == ProviderKind::BlygServer {
            return self.ai_fill_on_server(picked, item, index, window, cx);
        }
        let style = ai::style_prompt(cx);
        let items = self.backend.items();
        let reading = self.backend.reading();
        let job = match generate::prepare_fill(&text, index, &items, &reading, style.as_deref()) {
            Ok(j) => j,
            Err(e) => return self.ai_fail(Failure::Ai(e), Some(picked.kind), cx),
        };
        let id = item.local_id.clone();
        self.ai_spawn(
            picked,
            "generating",
            move |p, cancel| {
                let r = p.generate(job.prompt.request(cancel), &mut |_| {})?;
                Ok(Done::Fill {
                    item: id,
                    index: job.index,
                    instruction: job.instruction,
                    sources: job.sources,
                    text: r.text,
                    model: r.model,
                })
            },
            window,
            cx,
        );
    }

    /// The blyg builds the prompt, splices and records provenance itself
    /// (`POST /api/items/:id/generate`); afterwards the item is re-pulled.
    fn ai_fill_on_server(
        &mut self,
        picked: Picked,
        item: Item,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sid) = item.server_id.clone().filter(|_| !item.pending_sync) else {
            self.show_toast(
                "The blyg doesn't have this version yet",
                Some("Wait for it to sync, then ⌘G again".into()),
                cx,
            );
            return;
        };
        let backend = self.backend.clone();
        self.ai_spawn(
            picked,
            "generating on the blyg",
            move |p, cancel| {
                let mut req = GenRequest::new("", "").with_cancel(cancel.clone());
                req.server_scope = Some(ServerScope {
                    item_id: sid.0,
                    scope: index,
                });
                p.generate(req, &mut |_| {})?;
                backend
                    .sync_now()
                    .map_err(|e| blyg_ai::AiError::Provider(e.to_string()))?;
                Ok(Done::Server)
            },
            window,
            cx,
        );
    }

    fn ai_run(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        if action == Action::Reply {
            match self.ai_reply_target() {
                Some(r) => self.ai_reply_to(r, window, cx),
                None => self.show_toast(palette::REPLY_NEEDS_READING, None, cx),
            }
            return;
        }
        let Some(item) = self.current.clone() else {
            self.show_toast("Open a post first", None, cx);
            return;
        };
        let text = self.editor.read(cx).value().to_string();
        let caret = self.editor.read(cx).cursor();
        if action == Action::NewGap {
            let at = caret.min(text.len());
            let new = format!("{}[TK][/TK]{}", &text[..at], &text[at..]);
            self.ai_close(window, cx);
            self.splice_editor(&text, &new, Some(at + 4), window, cx);
            return;
        }
        if let Some(why) = palette::entries(item.kind, text.trim().is_empty(), false)
            .into_iter()
            .find(|e| e.action == action)
            .and_then(|e| e.disabled)
        {
            self.show_toast(why, None, cx);
            return;
        }
        if self.ai_busy(cx) {
            return;
        }
        self.ai_close(window, cx);
        let Some(picked) = self.ai_pick(cx) else {
            return;
        };
        let style = ai::style_prompt(cx);
        let id = item.local_id.clone();
        match action {
            Action::Shorten => self.ai_spawn(
                picked,
                "shortening",
                move |p, cancel| {
                    let outcome =
                        prompts::shorten_to_fit(p, &text, style.as_deref(), cancel, &mut |_| {})?;
                    Ok(Done::Shorten {
                        item: id,
                        before: text,
                        outcome,
                    })
                },
                window,
                cx,
            ),
            Action::Continue => self.ai_spawn(
                picked,
                "continuing",
                move |p, cancel| {
                    let out = prompts::continue_thought(
                        p,
                        &text,
                        caret,
                        style.as_deref(),
                        cancel,
                        &mut |_| {},
                    )?;
                    Ok(Done::Continue {
                        item: id,
                        before: text,
                        at: caret,
                        out,
                    })
                },
                window,
                cx,
            ),
            Action::Outline => self.ai_spawn(
                picked,
                "outlining",
                move |p, cancel| {
                    prompts::outline_thread(p, &text, style.as_deref(), cancel, &mut |_| {})
                        .map(|out| Done::Outline { out })
                },
                window,
                cx,
            ),
            Action::Proofread => self.ai_spawn(
                picked,
                "proofreading",
                move |p, cancel| {
                    let suggestions = prompts::proofread(p, &text, cancel)?;
                    Ok(Done::Proofread {
                        item: id,
                        text,
                        suggestions,
                    })
                },
                window,
                cx,
            ),
            Action::Reply | Action::NewGap => {}
        }
    }

    /// The text of `id` right now: the editor's if it's open, else stored.
    fn ai_text_of(&self, id: &LocalId, cx: &App) -> Option<String> {
        if self.current.as_ref().is_some_and(|c| &c.local_id == id) {
            Some(self.editor.read(cx).value().to_string())
        } else {
            self.backend.item(id).map(|i| i.content_md)
        }
    }

    /// Write generated text with its provenance: through the editor (one
    /// undoable splice) when the item is open, else straight to the backend.
    #[allow(clippy::too_many_arguments)]
    fn ai_write(
        &mut self,
        id: &LocalId,
        old: &str,
        new: String,
        scopes: Vec<Option<ScopeProvenance>>,
        caret: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.current.as_ref().is_some_and(|c| &c.local_id == id) {
            self.ai.pending = Some((new.clone(), scopes));
            self.splice_editor(old, &new, caret, window, cx);
            self.ai.pending = None;
        } else if let Err(e) = self.backend.save_with_provenance(id, &new, &scopes) {
            self.show_toast(format!("Couldn't save the generated text: {e}"), None, cx);
        } else {
            self.requery(window, cx);
        }
    }

    fn ai_done(
        &mut self,
        done: Done,
        kind: ProviderKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if timing()
            && let Done::Fill { text, model, .. } = &done
        {
            println!(
                "ai fill ok provider={} model={model} chars={}",
                kind.config_name(),
                text.chars().count()
            );
        }
        let changed = |cx: &mut Context<Self>, v: &mut MainView| {
            v.show_toast(
                "The post changed while the AI was working",
                Some("Nothing was written; try again".into()),
                cx,
            );
        };
        match done {
            Done::Fill {
                item,
                index,
                instruction,
                sources,
                text,
                model,
            } => {
                let Some(old) = self.ai_text_of(&item, cx) else {
                    return;
                };
                let Some(new) = generate::apply_fill(&old, index, &instruction, &text) else {
                    return changed(cx, self);
                };
                let tracked = self.backend.tracked_tk_provenance(&item);
                let Some(scopes) =
                    generate::provenance_with(tracked, &new, index, prov(&model, sources))
                else {
                    return changed(cx, self);
                };
                self.ai_write(&item, &old, new, scopes, None, window, cx);
                self.show_toast(
                    format!("Generated with {model}"),
                    Some(format!("{} · disclosed when published", kind.label()).into()),
                    cx,
                );
            }
            Done::Server => {
                self.requery(window, cx);
                if let Some(cur) = self.current.clone() {
                    self.current = self.backend.item(&cur.local_id);
                    self.load_current_into_editor(window, cx);
                }
                self.show_toast(
                    "Generated on your blyg",
                    Some("It recorded the provenance itself".into()),
                    cx,
                );
            }
            Done::Shorten {
                item,
                before,
                outcome,
            } => {
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                self.ai.overlay = Some(Overlay::Shorten {
                    item,
                    before,
                    after: outcome.result.insert,
                    model: outcome.result.model,
                    len: outcome.len,
                    fits: outcome.fits,
                    focus,
                });
            }
            Done::Continue {
                item,
                before,
                at,
                out,
            } => {
                let Some(old) = self.ai_text_of(&item, cx).filter(|t| *t == before) else {
                    return changed(cx, self);
                };
                let Some((new, index)) = generate::insert_scope(&old, at, &out.insert) else {
                    return changed(cx, self);
                };
                let tracked = self.backend.tracked_tk_provenance(&item);
                let Some(scopes) =
                    generate::provenance_with(tracked, &new, index, prov(&out.model, vec![]))
                else {
                    return changed(cx, self);
                };
                let end = new.len() - (old.len() - at.min(old.len()));
                self.ai_write(&item, &old, new, scopes, Some(end), window, cx);
            }
            Done::Outline { out } => self.ai_new_draft(
                Kind::Thread,
                out,
                "Outline drafted as a new thread",
                window,
                cx,
            ),
            Done::Reply { item, out } => {
                // Under the quote, after whatever was typed meanwhile.
                let Some(old) = self.ai_text_of(&item, cx) else {
                    return;
                };
                let Some((new, index)) = generate::insert_scope(&old, old.len(), &out.insert)
                else {
                    return changed(cx, self);
                };
                let tracked = self.backend.tracked_tk_provenance(&item);
                let Some(scopes) =
                    generate::provenance_with(tracked, &new, index, prov(&out.model, vec![]))
                else {
                    return changed(cx, self);
                };
                let end = new.len();
                self.ai_write(&item, &old, new, scopes, Some(end), window, cx);
                self.show_toast(
                    "Reply drafted · review it, then ⌘⏎ publishes",
                    Some("Generated text is disclosed when published".into()),
                    cx,
                );
            }
            Done::Proofread {
                item,
                text,
                suggestions,
            } => {
                if suggestions.is_empty() {
                    self.show_toast("No typos or grammar slips found", None, cx);
                    return;
                }
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                self.ai.overlay = Some(Overlay::Proofread {
                    item,
                    text,
                    suggestions,
                    focus,
                });
            }
        }
        cx.notify();
    }

    fn ai_new_draft(
        &mut self,
        kind: Kind,
        out: prompts::HelperOutput,
        toast: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let saved = self.backend.create_draft(kind, "").and_then(|id| {
            let scopes = generate::provenance_with(None, &out.insert, 0, prov(&out.model, vec![]))
                .unwrap_or_default();
            self.backend
                .save_with_provenance(&id, &out.insert, &scopes)
                .map(|()| id)
        });
        match saved {
            Ok(id) => {
                let results = self.backend.search("");
                self.set_query_text("", window, cx);
                self.list.set_query("", results);
                self.open(&id, window, cx);
                self.show_toast(
                    toast,
                    Some("Generated text is disclosed when published".into()),
                    cx,
                );
            }
            Err(e) => self.show_toast(format!("Couldn't create the draft: {e}"), None, cx),
        }
    }

    // ------------------------------------------------------------ overlays

    fn ai_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Settings(s)) = &self.ai.overlay
            && let Some(c) = &s.chatgpt_running
        {
            c.cancel();
        }
        self.ai.overlay = None;
        self.focus_after_sheet(window, cx);
        cx.notify();
    }

    fn ai_open_palette(
        &mut self,
        item: &Item,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reading_open = self.ai_reply_target().is_some();
        let entries = palette::entries(item.kind, text.trim().is_empty(), reading_open);
        let selected = palette::first_enabled(&entries);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.ai.overlay = Some(Overlay::Palette {
            entries,
            selected,
            focus,
        });
        cx.notify();
    }

    fn ai_palette_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(Overlay::Palette {
            entries, selected, ..
        }) = self.ai.overlay.as_mut()
        else {
            return false;
        };
        match key {
            "down" => *selected = palette::step(entries, *selected, 1),
            "up" => *selected = palette::step(entries, *selected, -1),
            "enter" => {
                let e = entries[*selected].clone();
                if e.enabled() {
                    self.ai_run(e.action, window, cx);
                }
            }
            "escape" => self.ai_close(window, cx),
            k => {
                let Some(n) = k
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=entries.len()).contains(n))
                else {
                    return false;
                };
                let e = entries[n - 1].clone();
                match e.disabled {
                    None => self.ai_run(e.action, window, cx),
                    Some(why) => self.show_toast(why, None, cx),
                }
            }
        }
        cx.notify();
        true
    }

    fn ai_accept_shorten(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Shorten {
            item,
            before,
            after,
            model,
            ..
        }) = self.ai.overlay.take()
        else {
            return;
        };
        self.focus_after_sheet(window, cx);
        let Some(old) = self.ai_text_of(&item, cx).filter(|t| *t == before) else {
            self.show_toast("The post changed since; shorten it again", None, cx);
            return;
        };
        let Some(scopes) = generate::provenance_with(None, &after, 0, prov(&model, vec![])) else {
            return;
        };
        self.ai_write(&item, &old, after, scopes, None, window, cx);
        self.show_toast(
            "Shortened",
            Some("Disclosed as generated when published".into()),
            cx,
        );
    }

    fn ai_accept_proofread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Proofread {
            item,
            text,
            suggestions,
            ..
        }) = self.ai.overlay.take()
        else {
            return;
        };
        self.focus_after_sheet(window, cx);
        let Some(old) = self.ai_text_of(&item, cx).filter(|t| *t == text) else {
            self.show_toast("The post changed since; proofread it again", None, cx);
            return;
        };
        // Not generated prose: a plain edit, no provenance.
        let new = prompts::apply_suggestions(&old, &suggestions);
        if self.current.as_ref().is_some_and(|c| c.local_id == item) {
            self.splice_editor(&old, &new, None, window, cx);
        } else if let Err(e) = self.backend.save(&item, &new) {
            self.show_toast(format!("Couldn't save: {e}"), None, cx);
            return;
        }
        let n = suggestions.len();
        self.show_toast(
            format!("{n} fix{} applied", if n == 1 { "" } else { "es" }),
            None,
            cx,
        );
    }

    fn ai_publish_anyway(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ai.overlay = None;
        self.ai.publish_anyway = true;
        self.focus_after_sheet(window, cx);
        self.publish(window, cx);
    }

    // ------------------------------------------------------------ Settings › AI

    /// The "AI" row in the Settings sheet: a summary and the way in.
    pub(super) fn render_ai_settings_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let enabled = crate::settings::get(cx).store.config().ai_enabled();
        let summary = if enabled.is_empty() {
            "Off · nothing is enabled or signed in".to_string()
        } else {
            format!("On: {}", enabled.join(", "))
        };
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
                    .child("AI"),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(chip(p, "ai-open-settings", "AI accounts…", false).on_click(
                        cx.listener(|this, _, window, cx| this.ai_open_settings(window, cx)),
                    ))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(p.muted)
                            .truncate()
                            .child(summary),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn ai_open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = None;
        let input =
            |masked: bool, ph: &'static str, window: &mut Window, cx: &mut Context<Self>| {
                cx.new(|cx| InputState::new(window, cx).masked(masked).placeholder(ph))
            };
        let anthropic_key = input(true, "Paste an Anthropic API key, then ⏎", window, cx);
        let openai_key = input(true, "Paste an OpenAI API key, then ⏎", window, cx);
        let cf_account = input(false, "Cloudflare account ID", window, cx);
        let cf_token = input(true, "Workers AI API token, then ⏎", window, cx);
        let model = input(false, "the provider's default", window, cx);
        let subscribe = |e: &Entity<InputState>,
                         f: fn(&mut MainView, &mut Window, &mut Context<MainView>),
                         window: &mut Window,
                         cx: &mut Context<Self>| {
            cx.subscribe_in(e, window, move |this, _, ev, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    f(this, window, cx);
                }
            })
        };
        let subs = vec![
            subscribe(
                &anthropic_key,
                |v, w, cx| v.ai_save_key(ProviderKind::AnthropicApi, w, cx),
                window,
                cx,
            ),
            subscribe(
                &openai_key,
                |v, w, cx| v.ai_save_key(ProviderKind::OpenaiApi, w, cx),
                window,
                cx,
            ),
            subscribe(
                &cf_account,
                |v, w, cx| v.ai_save_cloudflare(w, cx),
                window,
                cx,
            ),
            subscribe(
                &cf_token,
                |v, w, cx| v.ai_save_cloudflare(w, cx),
                window,
                cx,
            ),
            subscribe(&model, |v, w, cx| v.ai_save_model(w, cx), window, cx),
        ];
        self._subs.extend(subs);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let rows = ais::rows(cx);
        let current_model = rows
            .iter()
            .find(|r| r.is_default)
            .and_then(|r| r.model.clone())
            .unwrap_or_default();
        model.update(cx, |s, cx| s.set_value(current_model, window, cx));
        self.ai.overlay = Some(Overlay::Settings(Box::new(SettingsPanel {
            focus,
            rows,
            anthropic_key,
            openai_key,
            cf_account,
            cf_token,
            model,
            message: None,
            chatgpt_notice: false,
            chatgpt_running: None,
        })));
        cx.notify();
    }

    fn ai_settings_result(&mut self, r: Result<(), String>, ok: String, cx: &mut Context<Self>) {
        let rows = ais::rows(cx);
        if let Some(Overlay::Settings(s)) = self.ai.overlay.as_mut() {
            s.rows = rows;
            s.message = Some(match r {
                Ok(()) => (ok, false),
                Err(e) => (e, true),
            });
        }
        cx.notify();
    }

    /// Save a pasted API key to the Keychain. The field is cleared at once
    /// whatever happens, and the key is never shown, logged or echoed.
    fn ai_save_key(&mut self, kind: ProviderKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Settings(s)) = self.ai.overlay.as_ref() else {
            return;
        };
        let input = match kind {
            ProviderKind::AnthropicApi => s.anthropic_key.clone(),
            ProviderKind::OpenaiApi => s.openai_key.clone(),
            _ => return,
        };
        let key = input.read(cx).value().to_string();
        input.update(cx, |s, cx| s.set_value("", window, cx));
        let r = ais::save_api_key(kind, &key, cx);
        drop(key);
        self.ai_settings_result(r, format!("{} saved in your Keychain", kind.label()), cx);
    }

    fn ai_save_cloudflare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Settings(s)) = self.ai.overlay.as_ref() else {
            return;
        };
        let (acct_in, token_in) = (s.cf_account.clone(), s.cf_token.clone());
        let account = acct_in.read(cx).value().to_string();
        let token = token_in.read(cx).value().to_string();
        if account.trim().is_empty() || token.trim().is_empty() {
            // Wait for both; ⏎ in the first field moves on.
            if account.trim().is_empty() {
                acct_in.update(cx, |s, cx| s.focus(window, cx));
            } else {
                token_in.update(cx, |s, cx| s.focus(window, cx));
            }
            return;
        }
        token_in.update(cx, |s, cx| s.set_value("", window, cx));
        let r = ais::save_cloudflare(&account, &token, cx);
        if r.is_ok() {
            acct_in.update(cx, |s, cx| s.set_value("", window, cx));
        }
        self.ai_settings_result(
            r,
            "Cloudflare Workers AI saved (token in your Keychain)".into(),
            cx,
        );
    }

    fn ai_save_model(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Settings(s)) = self.ai.overlay.as_ref() else {
            return;
        };
        let Some(kind) = s.rows.iter().find(|r| r.is_default).map(|r| r.kind) else {
            self.ai_settings_result(Err("Pick a provider first".into()), String::new(), cx);
            return;
        };
        let model = s.model.read(cx).value().to_string();
        let r = ais::set_model(kind, &model, cx);
        let ok = if model.trim().is_empty() {
            format!("{} uses its default model", kind.label())
        } else {
            format!("{} now uses {}", kind.label(), model.trim())
        };
        self.ai_settings_result(r, ok, cx);
    }

    fn ai_toggle(&mut self, kind: ProviderKind, on: bool, cx: &mut Context<Self>) {
        let r = ais::set_enabled(kind, on, cx);
        let ok = format!(
            "{} switched {}",
            kind.label(),
            if on { "on" } else { "off" }
        );
        self.ai_settings_result(r, ok, cx);
    }

    fn ai_use(&mut self, kind: ProviderKind, window: &mut Window, cx: &mut Context<Self>) {
        let r = ais::set_default(kind, cx);
        let ok = format!("⌘G now uses {}", kind.label());
        self.ai_settings_result(r, ok, cx);
        let model = crate::ai::with_accounts(cx, |a| blyg_ai::accounts::model_in(a.config(), kind))
            .unwrap_or_default();
        if let Some(Overlay::Settings(s)) = self.ai.overlay.as_ref() {
            let m = s.model.clone();
            m.update(cx, |s, cx| s.set_value(model, window, cx));
        }
    }

    fn ai_remove(&mut self, kind: ProviderKind, cx: &mut Context<Self>) {
        let r = ais::remove(kind, cx);
        self.ai_settings_result(r, format!("{} removed", kind.label()), cx);
    }

    fn ai_data_dir(cx: &App) -> Option<std::path::PathBuf> {
        cx.try_global::<crate::connection::Connection>()
            .map(|c| c.data_dir.clone())
    }

    fn ai_chatgpt_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let data_dir = Self::ai_data_dir(cx);
        let Some(Overlay::Settings(s)) = self.ai.overlay.as_mut() else {
            return;
        };
        if s.chatgpt_running.is_some() {
            return;
        }
        if !s.chatgpt_notice && !ais::notice_shown(data_dir.as_deref()) {
            s.chatgpt_notice = true;
            cx.notify();
            return;
        }
        s.chatgpt_notice = false;
        ais::mark_notice_shown(data_dir.as_deref());
        let oauth = crate::ai::with_accounts(cx, |a| a.endpoints().chatgpt_oauth.clone());
        let login = match oauth.start_browser_login() {
            Ok(l) => l,
            Err(e) => {
                self.ai_settings_result(Err(ai::redact(&e.to_string())), String::new(), cx);
                return;
            }
        };
        if let Err(e) = blyg_ai::open_in_browser(&login.url) {
            self.ai_settings_result(Err(e.to_string()), String::new(), cx);
            return;
        }
        let cancel = CancelFlag::new();
        if let Some(Overlay::Settings(s)) = self.ai.overlay.as_mut() {
            s.chatgpt_running = Some(cancel.clone());
            s.message = Some((
                "Approve Blygger in the browser window that opened…".into(),
                false,
            ));
        }
        cx.notify();
        let (tx, rx) = async_channel::bounded(1);
        let _ = std::thread::Builder::new()
            .name("blygger-chatgpt-sign-in".into())
            .spawn(move || {
                let r = oauth.finish_browser_login(
                    &login,
                    &cancel,
                    std::time::Duration::from_secs(300),
                );
                let _ = tx.try_send(r);
            });
        cx.spawn_in(window, async move |this, cx| {
            let r = loop {
                cx.background_executor().timer(POLL).await;
                match rx.try_recv() {
                    Ok(r) => break r,
                    Err(async_channel::TryRecvError::Empty) => continue,
                    Err(async_channel::TryRecvError::Closed) => return,
                }
            };
            let _ = this.update_in(cx, |v, _, cx| {
                if let Some(Overlay::Settings(s)) = v.ai.overlay.as_mut() {
                    s.chatgpt_running = None;
                }
                let r = match r {
                    Ok(creds) => {
                        crate::ai::with_accounts(cx, |a| a.store_chatgpt_credentials(creds))
                            .map_err(|e| ai::redact(&e.to_string()))
                    }
                    Err(blyg_ai::AiError::Cancelled) => Err("Sign-in cancelled".into()),
                    Err(e) => Err(ai::redact(&e.to_string())),
                };
                v.ai_settings_result(r, "Signed in with ChatGPT".into(), cx);
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ rendering

    /// Every AI overlay (palette, proposals, warning, Settings › AI).
    pub(super) fn render_ai_overlay(
        &self,
        ui_font: &SharedString,
        body_font: &SharedString,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let overlay = self.ai.overlay.as_ref()?;
        let p = self.palette;
        let (width, content): (f32, AnyElement) = match overlay {
            Overlay::Palette {
                entries,
                selected,
                focus,
            } => (440., self.render_palette(entries, *selected, focus, cx)),
            Overlay::Shorten {
                before,
                after,
                len,
                fits,
                focus,
                ..
            } => {
                let (mine, theirs) =
                    crate::vm::word_diff(&blyg_core::strip_tk(before), &blyg_core::strip_tk(after));
                let head = if *fits {
                    format!("Shortened to {len} characters")
                } else {
                    format!("Still {len} characters: over 1000 after two tries")
                };
                (
                    620.,
                    div()
                        .track_focus(focus)
                        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                            match ev.keystroke.key.as_str() {
                                "enter" => this.ai_accept_shorten(window, cx),
                                "escape" => this.ai_close(window, cx),
                                _ => return,
                            }
                            cx.stop_propagation();
                        }))
                        .child(heading(head))
                        .child(
                            div()
                                .flex()
                                .gap(px(10.))
                                .my(px(8.))
                                .child(diff_col(p, body_font, "NOW", &mine))
                                .child(diff_col(p, body_font, "SHORTENED (GENERATED)", &theirs)),
                        )
                        .child(keys(p, &[("⏎", "accept"), ("esc", "keep mine")]))
                        .into_any_element(),
                )
            }
            Overlay::Proofread {
                text,
                suggestions,
                focus,
                ..
            } => {
                let _ = text;
                let list = suggestions.iter().take(12).enumerate().map(|(i, s)| {
                    div()
                        .id(("pf", i))
                        .flex()
                        .gap(px(8.))
                        .py(px(3.))
                        .child(
                            div()
                                .text_color(p.over)
                                .line_through()
                                .child(s.original.clone()),
                        )
                        .child("→")
                        .child(div().text_color(p.green).child(s.replacement.clone()))
                        .child(div().text_color(p.muted).child(s.reason.clone()))
                });
                (
                    520.,
                    div()
                        .track_focus(focus)
                        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                            match ev.keystroke.key.as_str() {
                                "enter" => this.ai_accept_proofread(window, cx),
                                "escape" => this.ai_close(window, cx),
                                _ => return,
                            }
                            cx.stop_propagation();
                        }))
                        .child(heading(format!(
                            "{} suggestion{} · typos and grammar only",
                            suggestions.len(),
                            if suggestions.len() == 1 { "" } else { "s" }
                        )))
                        .child(div().font_family(body_font.clone()).children(list))
                        .child(
                            div()
                                .mt(px(6.))
                                .text_size(px(11.5))
                                .text_color(p.muted)
                                .child(
                                    "Proofreading isn't generated prose, so it isn't disclosed.",
                                ),
                        )
                        .child(keys(p, &[("⏎", "apply all"), ("esc", "ignore")]))
                        .into_any_element(),
                )
            }
            Overlay::PublishWarning { focus } => (
                460.,
                div()
                    .track_focus(focus)
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        match ev.keystroke.key.as_str() {
                            // Cancel is the default: ⏎ and esc both cancel.
                            "enter" | "escape" => this.ai_close(window, cx),
                            "p" => this.ai_publish_anyway(window, cx),
                            _ => return,
                        }
                        cx.stop_propagation();
                    }))
                    .child(heading("This will publish without AI disclosure".into()))
                    .child(div().text_color(p.muted).line_height(relative(1.45)).child(
                        "This post has text generated in Blygger, but your blyg doesn't have the \
                         provenance extension (it answered 404), so readers won't see that it \
                         was generated. See docs/SERVER.md.",
                    ))
                    .child(
                        div()
                            .mt(px(12.))
                            .flex()
                            .gap(px(8.))
                            .child(chip(p, "ai-warn-cancel", "Cancel  ⏎", true).on_click(
                                cx.listener(|this, _, window, cx| this.ai_close(window, cx)),
                            ))
                            .child(
                                chip(p, "ai-warn-publish", "Publish anyway  P", false).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.ai_publish_anyway(window, cx)
                                    }),
                                ),
                            ),
                    )
                    .into_any_element(),
            ),
            Overlay::Settings(s) => (560., self.render_ai_settings(s, ui_font, cx)),
        };
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
                        .id("ai-sheet")
                        .occlude()
                        .w(px(width))
                        .max_w(relative(0.92))
                        .max_h(relative(0.86))
                        .overflow_y_scroll()
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
                        .text_color(p.ink)
                        .child(content),
                )
                .into_any_element(),
        )
    }

    fn render_palette(
        &self,
        entries: &[palette::Entry],
        selected: usize,
        focus: &FocusHandle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        let rows = entries.iter().enumerate().map(|(i, e)| {
            let on = i == selected;
            let action = e.action;
            let enabled = e.enabled();
            div()
                .id(("ai-pal", i))
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(6.))
                .when(on, |d| d.bg(p.sel))
                .when(!enabled, |d| d.opacity(0.45))
                .when(enabled, |d| d.cursor_pointer())
                .on_click(cx.listener(move |this, _, window, cx| {
                    if enabled {
                        this.ai_run(action, window, cx);
                    }
                }))
                .child(
                    div()
                        .w(px(14.))
                        .text_color(p.muted)
                        .child(format!("{}", i + 1)),
                )
                .child(
                    div().flex_1().min_w_0().child(div().child(e.label)).child(
                        div()
                            .text_size(px(11.5))
                            .text_color(p.muted)
                            .truncate()
                            .child(e.disabled.unwrap_or(e.detail)),
                    ),
                )
                .children(e.key.map(|k| div().text_color(p.muted).child(k)))
        });
        div()
            .track_focus(focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if this.ai_palette_key(ev.keystroke.key.as_str(), window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(heading("AI helpers".into()))
            .child(div().flex().flex_col().children(rows))
            .child(div().mt(px(8.)).text_size(px(11.5)).text_color(p.muted).child(
                "Inside [TK]instruction[/TK], ⌘G fills it. Generated text is disclosed when published.",
            ))
            .child(keys(p, &[("↑↓", "choose"), ("⏎", "run"), ("esc", "close")]))
            .into_any_element()
    }

    fn render_ai_settings(
        &self,
        s: &SettingsPanel,
        ui_font: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        let input_box = |id: &'static str, e: &Entity<InputState>| {
            div()
                .id(id)
                .flex_1()
                .min_w_0()
                .px(px(9.))
                .py(px(4.))
                .rounded(px(7.))
                .border_1()
                .border_color(p.line)
                .font_family(ui_font.clone())
                .text_size(px(12.5))
                .child(gpui_kit::base::input::Input::new(e))
        };
        for e in [
            &s.anthropic_key,
            &s.openai_key,
            &s.cf_account,
            &s.cf_token,
            &s.model,
        ] {
            e.update(cx, |st, _| {
                st.set_editor_style(gpui_kit::base::input::InputEditorStyle {
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
        let rows = s.rows.iter().map(|row| {
            let kind = row.kind;
            let (word, tone) = ais::status_label(row);
            let tone_color = match tone {
                ais::Tone::Good => p.green,
                ais::Tone::Neutral => p.muted,
                ais::Tone::Bad => p.over,
            };
            let signed_in = matches!(
                row.status,
                ProviderStatus::SignedIn | ProviderStatus::TokenExpired
            );
            let ready = row.enabled && row.status.is_ready();
            let name: SharedString = kind.config_name().into();
            let mut controls = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .mt(px(4.));
            if ais::toggles_freely(kind) {
                let installed = !matches!(row.status, ProviderStatus::CliNotFound);
                let on = row.enabled;
                controls = controls.when(installed || on, |d| {
                    d.child(
                        chip(
                            p,
                            format!("ai-toggle-{name}"),
                            if on { "Switch off" } else { "Switch on" },
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.ai_toggle(kind, !on, cx))),
                    )
                });
            } else if signed_in {
                controls = controls
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(p.muted)
                            .child(match kind {
                                ProviderKind::ChatgptAccount => "signed in",
                                ProviderKind::CloudflareWorkersAi => "token saved",
                                _ => "key saved",
                            }),
                    )
                    .child(
                        chip(
                            p,
                            format!("ai-remove-{name}"),
                            if kind == ProviderKind::ChatgptAccount {
                                "Sign out"
                            } else {
                                "Remove"
                            },
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.ai_remove(kind, cx))),
                    );
            } else {
                controls = match kind {
                    ProviderKind::AnthropicApi => {
                        controls.child(input_box("ai-key-anthropic", &s.anthropic_key))
                    }
                    ProviderKind::OpenaiApi => {
                        controls.child(input_box("ai-key-openai", &s.openai_key))
                    }
                    ProviderKind::CloudflareWorkersAi => controls
                        .child(input_box("ai-cf-account", &s.cf_account))
                        .child(input_box("ai-cf-token", &s.cf_token)),
                    ProviderKind::ChatgptAccount => {
                        if s.chatgpt_running.is_some() {
                            controls
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(p.muted)
                                        .child("Waiting for the browser…"),
                                )
                                .child(chip(p, "ai-chatgpt-cancel", "Cancel", false).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(Overlay::Settings(s)) = this.ai.overlay.as_mut()
                                            && let Some(c) = s.chatgpt_running.take()
                                        {
                                            c.cancel();
                                        }
                                        cx.notify();
                                    }),
                                ))
                        } else {
                            controls.child(
                                chip(
                                    p,
                                    "ai-chatgpt",
                                    if s.chatgpt_notice {
                                        "Continue to the browser"
                                    } else {
                                        "Sign in with ChatGPT"
                                    },
                                    false,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| this.ai_chatgpt_sign_in(window, cx),
                                )),
                            )
                        }
                    }
                    _ => controls,
                };
            }
            if ready && !row.is_default {
                controls = controls.child(
                    chip(p, format!("ai-use-{name}"), "Use for ⌘G", false).on_click(
                        cx.listener(move |this, _, window, cx| this.ai_use(kind, window, cx)),
                    ),
                );
            }
            div()
                .id(SharedString::from(format!("ai-row-{name}")))
                .py(px(7.))
                .border_b_1()
                .border_color(p.line)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().font_weight(FontWeight::MEDIUM).child(kind.label()))
                        .child(div().text_size(px(11.)).text_color(tone_color).child(word))
                        .when(row.is_default && ready, |d| {
                            d.child(
                                div()
                                    .text_size(px(11.))
                                    .px(px(6.))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(p.accent)
                                    .text_color(p.accent)
                                    .child("⌘G uses this"),
                            )
                        }),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(p.muted)
                        .child(ais::how(kind)),
                )
                .when(
                    kind == ProviderKind::ChatgptAccount && s.chatgpt_notice,
                    |d| {
                        d.child(
                            div()
                                .mt(px(4.))
                                .p(px(8.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(p.warn)
                                .text_size(px(11.5))
                                .line_height(relative(1.4))
                                .child(ais::CHATGPT_NOTICE),
                        )
                    },
                )
                .child(controls)
        });
        let default = s.rows.iter().find(|r| r.is_default).map(|r| r.kind);
        div()
            .track_focus(&s.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.ai_close(window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                this.ai_close(window, cx);
            }))
            .child(heading("Settings › AI".into()))
            .child(div().text_color(p.muted).text_size(px(12.)).line_height(relative(1.45)).child(
                "AI stays off until you switch a provider on or sign in to one. Then ⌘G inside \
                 [TK]instruction[/TK] writes the gap, and the text is disclosed as generated when \
                 you publish. Keys go straight to your macOS Keychain and are never shown again. \
                 There's no claude.ai login: to use a Claude plan, switch on Claude Code.",
            ))
            .child(div().flex().flex_col().mt(px(6.)).children(rows))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .mt(px(10.))
                    .child(
                        div()
                            .w(px(150.))
                            .flex_none()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.muted)
                            .child(match default {
                                Some(k) => format!("MODEL · {}", k.config_name().to_uppercase()),
                                None => "MODEL".to_string(),
                            }),
                    )
                    .child(input_box("ai-model", &s.model)),
            )
            .when_some(s.message.clone(), |d, (m, is_err)| {
                d.child(
                    div()
                        .id("ai-settings-message")
                        .mt(px(8.))
                        .text_size(px(12.))
                        .text_color(if is_err { p.over } else { p.green })
                        .child(m),
                )
            })
            .child(keys(p, &[("⏎", "save a field"), ("esc", "close")]))
            .into_any_element()
    }
}

/// `BLYGGER_TIMING`: print what AI did (automation; never any secret).
fn timing() -> bool {
    std::env::var_os("BLYGGER_TIMING").is_some()
}

fn heading(s: String) -> Div {
    div()
        .mb(px(8.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(13.))
        .child(s)
}

fn chip(
    p: crate::theme::Palette,
    id: impl Into<SharedString>,
    label: &'static str,
    primary: bool,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .px(px(10.))
        .py(px(3.))
        .rounded_full()
        .border_1()
        .cursor_pointer()
        .text_size(px(12.))
        .border_color(if primary { p.accent } else { p.line })
        .when(primary, |d| d.text_color(p.accent))
        .hover(|s| s.border_color(p.accent))
        .child(label)
}

fn keys(p: crate::theme::Palette, items: &[(&'static str, &'static str)]) -> Div {
    div()
        .mt(px(12.))
        .flex()
        .gap(px(14.))
        .text_color(p.muted)
        .text_size(px(11.5))
        .children(items.iter().map(|(k, label)| {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .child(
                    div()
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(5.))
                        .border_1()
                        .border_b_2()
                        .border_color(p.line)
                        .text_color(p.ink)
                        .child(*k),
                )
                .child(*label)
        }))
}

fn diff_col(
    p: crate::theme::Palette,
    body_font: &SharedString,
    label: &'static str,
    runs: &[(crate::vm::DiffOp, String)],
) -> impl IntoElement {
    use crate::vm::DiffOp;
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
        .max_h(px(300.))
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
}

impl AiView {
    /// An AI sheet or palette is showing (the preview webview must hide:
    /// a native view would sit on top of it).
    pub fn has_overlay(&self) -> bool {
        self.overlay.is_some()
    }
}
