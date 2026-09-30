//! Scratch notes in the main window (docs/SPEC.md § Scratch notes).
//!
//! A child module of `app` (so it can reach `MainView`'s fields) that keeps
//! the scratch-note logic out of `app.rs`:
//! - ⌘D (`MakeDraft`) promotes the selected scratch note to a server draft;
//! - ⌘⏎ on a scratch note uses the normal publish sheet and publishes by
//!   promotion (`publish_via`);
//! - the omnibar's create honours `new-note` (`create_note`).
//!
//! The list's `scratch` pill and the status bar's "scratch · only on this
//! Mac" come from `vm::pill` / `vm::version_label`.

use blyg_core::config::NewNote;
use blyg_core::{Promote, PublishOutcome, Status};

use super::*;

gpui_kit::actions!(
    blygger,
    [
        /// ⌘D: make the scratch note a draft on the blyg (the main window's
        /// selection, or the quick-capture panel's text).
        MakeDraft,
        /// ⌘S in quick capture: keep it (scratch, or a draft per `capture-default`).
        KeepCapture,
    ]
);

/// Publish `id`; a scratch note is promoted (created on the server, with its
/// kind picked by `promotion_kind`) and then published. When promotion made
/// it a thread, the outcome's warning says so.
pub(crate) fn publish_via(
    backend: &dyn Backend,
    id: &LocalId,
    note: Option<&str>,
) -> blyg_core::Result<PublishOutcome> {
    let Some(item) = backend.item(id).filter(|i| i.status == Status::Scratch) else {
        return backend.publish(id, note);
    };
    let p = backend.promote(
        id,
        Promote::Publish {
            note: note.map(str::to_string),
        },
    )?;
    let mut out = p
        .published
        .ok_or_else(|| CoreError::Other("not published".into()))?;
    if p.kind == Kind::Thread && item.kind == Kind::Fragment {
        let said = "published as a thread (over 1000 characters)";
        out.warning = Some(match out.warning.take() {
            Some(w) => format!("{said} · {w}"),
            None => said.to_string(),
        });
    }
    Ok(out)
}

/// The omnibar's create: a draft, or a scratch note with `new-note = scratch`.
/// Returns the new id and the toast to show.
pub(crate) fn create_note(
    backend: &dyn Backend,
    new_note: NewNote,
    seed: &str,
) -> blyg_core::Result<(LocalId, &'static str)> {
    match new_note {
        NewNote::Draft => backend
            .create_draft(Kind::Fragment, seed)
            .map(|id| (id, "New draft created")),
        NewNote::Scratch => backend
            .create_scratch(Kind::Fragment, seed)
            .map(|id| (id, "New scratch note · only on this Mac")),
    }
}

/// What the omnibar's ⏎ creates, for its placeholder.
pub(crate) fn new_note_noun(new_note: NewNote) -> &'static str {
    match new_note {
        NewNote::Draft => "draft",
        NewNote::Scratch => "scratch note",
    }
}

/// The toast after ⌘D.
fn made_draft_toast(kind: Kind) -> &'static str {
    match kind {
        Kind::Fragment => "Now a draft on your blyg",
        Kind::Thread => "Now a draft on your blyg · a thread",
    }
}

/// Insert `![](url)` at byte `cursor` on its own paragraph (like the upload
/// placeholder). Returns the new text and the caret after the image.
pub(crate) fn insert_image(text: &str, cursor: usize, url: &str) -> (String, usize) {
    let (with_placeholder, range) = vm::insert_placeholder(text, cursor);
    let md = vm::image_markdown(url);
    let mut out = with_placeholder;
    out.replace_range(range.clone(), &md);
    (out, range.start + md.len())
}

impl MainView {
    /// --- follow-ups --- A paste or drop into a scratch note: the image is
    /// copied into the data dir and referenced as `blyg-local:…`; nothing
    /// is uploaded until the note is promoted (⌘D / ⌘⏎). `false` when the
    /// current item isn't a scratch note (the normal upload runs).
    pub(super) fn scratch_keep_image(
        &mut self,
        bytes: &[u8],
        mime: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self
            .current
            .as_ref()
            .is_none_or(|c| c.status != Status::Scratch)
        {
            return false;
        }
        match self.backend.save_scratch_media(bytes, mime) {
            Ok(url) => {
                let (text, cursor) = {
                    let s = self.editor.read(cx);
                    (s.value().to_string(), s.cursor())
                };
                let (new_text, caret) = insert_image(&text, cursor, &url);
                self.splice_editor(&text, &new_text, Some(caret), window, cx);
                self.show_toast(
                    "Image kept on this Mac",
                    Some("It's uploaded when the note becomes a draft or is published".into()),
                    cx,
                );
            }
            Err(e) => self.show_toast(format!("Couldn't keep the image: {e}"), None, cx),
        }
        true
    }

    /// After a promotion: its local images now have blyg URLs in the stored
    /// text; put them in the editor too (as an edit, keeping the caret), so
    /// the next keystroke doesn't save the old references back.
    pub(super) fn scratch_sync_editor(
        &mut self,
        id: &LocalId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.current.as_ref().map(|c| &c.local_id) != Some(id) {
            return;
        }
        let Some(fresh) = self.backend.item(id).map(|i| i.content_md) else {
            return;
        };
        let text = self.editor.read(cx).value().to_string();
        let (mine, theirs) = (
            blyg_core::scratch_media::refs(&text),
            blyg_core::scratch_media::refs(&fresh),
        );
        // Only when the store has swapped some of the editor's references.
        if text != fresh && theirs.len() < mine.len() && theirs.iter().all(|r| mine.contains(r)) {
            self.splice_editor(&text, &fresh, None, window, cx);
        }
    }

    /// ⌘D: promote the current scratch note to a draft (same item, no copy).
    pub(super) fn make_draft(
        &mut self,
        _: &MakeDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.current.clone() else {
            return;
        };
        // (The toolbar's Make draft shows the same reason, disabled.)
        if let Some(msg) = vm::make_draft_blocked(&item) {
            self.show_toast(msg, None, cx);
            return;
        }
        // Local images are uploaded first (network), so promote off the UI
        // thread; a note without any stays instant.
        let backend = self.backend.clone();
        let id = item.local_id.clone();
        let images = blyg_core::scratch_media::has_refs(&item.content_md);
        if images {
            self.show_toast("Uploading images…", None, cx);
        }
        let task = cx.background_spawn(async move { backend.promote(&id, Promote::Draft) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |v, window, cx| {
                match result {
                    Ok(p) => {
                        v.scratch_sync_editor(&item.local_id, window, cx);
                        v.requery(window, cx);
                        v.show_toast(made_draft_toast(p.kind), None, cx);
                    }
                    // Nothing changed: it's still a scratch note.
                    Err(e) => v.show_toast(
                        format!("Couldn't make a draft: {e}"),
                        images.then(|| "It's still a scratch note, only on this Mac".into()),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use blyg_core::config::MemoryTokenStore;
    use blyg_core::{Backend, ConfigStore, Kind, LocalId, Status};
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};

    use crate::app::{MainView, Mode, Sheet};
    use crate::fake::{FakeBackend, Timing};
    use crate::prefs::Prefs;
    use crate::vm;

    const CONNECTED: &str = "# test config\nblyg-url = https://blyg.example.com\n";

    fn setup<'a>(
        cx: &'a mut TestAppContext,
        config: &str,
    ) -> (
        Entity<MainView>,
        Arc<FakeBackend>,
        &'a mut VisualTestContext,
    ) {
        let config = config.to_string();
        let prefs = Prefs::from_config(ConfigStore::in_memory(&config).config());
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::app::bind_keys(cx);
            crate::settings::init(
                ConfigStore::in_memory(&config),
                Arc::new(MemoryTokenStore::default()),
                None,
                cx,
            );
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
        (view, fake, cx)
    }

    fn toast(view: &Entity<MainView>, cx: &mut VisualTestContext) -> String {
        view.read_with(cx, |v, _| {
            v.toast
                .as_ref()
                .map(|t| t.text.to_string())
                .unwrap_or_default()
        })
    }

    /// A scratch note, listed first and opened in the editor.
    fn open_scratch(
        view: &Entity<MainView>,
        fake: &FakeBackend,
        text: &str,
        cx: &mut VisualTestContext,
    ) -> LocalId {
        let id = fake.create_scratch(Kind::Fragment, text).unwrap();
        cx.run_until_parked();
        // Find it by its first words (scratch notes are searchable), then open.
        let words: String = text
            .split_whitespace()
            .take(3)
            .collect::<Vec<_>>()
            .join(" ");
        cx.simulate_input(&words);
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        view.read_with(cx, |v, _| {
            assert_eq!(v.mode, Mode::Edit);
            assert_eq!(v.current.as_ref().map(|c| &c.local_id), Some(&id));
        });
        id
    }

    #[gpui_kit::test]
    fn scratch_rows_show_the_pill_and_status(cx: &mut TestAppContext) {
        let (view, fake, cx) = setup(cx, CONNECTED);
        let id = open_scratch(&view, &fake, "A note about moss on the north wall", cx);
        view.read_with(cx, |v, _| {
            let row = &v.list.results()[0];
            assert_eq!(row.local_id, id);
            assert_eq!(vm::pill(row), ("scratch".to_string(), false));
            assert!(!vm::has_unpublished_edits(row));
            let cur = v.current.as_ref().unwrap();
            assert_eq!(vm::version_label(cur), "scratch · only on this Mac");
        });
        // Editing it saves locally and never queues a push.
        cx.simulate_input(", green as a pond.");
        cx.run_until_parked();
        let it = fake.item(&id).unwrap();
        assert!(it.content_md.ends_with("green as a pond."));
        assert_eq!(it.status, Status::Scratch);
        assert!(!it.pending_sync);
    }

    #[gpui_kit::test]
    fn cmd_d_promotes_the_scratch_note_to_a_draft(cx: &mut TestAppContext) {
        let (view, fake, cx) = setup(cx, CONNECTED);
        let id = open_scratch(&view, &fake, "Kettle whistles in B flat", cx);
        let before = fake.items().len();

        cx.simulate_keystrokes("cmd-d");
        cx.run_until_parked();
        let it = fake.item(&id).expect("same id");
        assert_eq!(it.status, Status::Draft);
        assert_eq!(it.kind, Kind::Fragment);
        assert_eq!(fake.items().len(), before, "no duplicate");
        assert_eq!(toast(&view, cx), "Now a draft on your blyg");
        view.read_with(cx, |v, _| {
            assert_eq!(vm::pill(&v.list.results()[0]).0, "draft");
            assert_eq!(v.current.as_ref().unwrap().status, Status::Draft);
        });

        // Again: nothing to do, and no demotion.
        cx.simulate_keystrokes("cmd-d");
        cx.run_until_parked();
        assert_eq!(toast(&view, cx), "Already a draft");
        assert_eq!(fake.item(&id).unwrap().status, Status::Draft);
    }

    #[gpui_kit::test]
    fn cmd_enter_publishes_a_scratch_note_through_the_sheet(cx: &mut TestAppContext) {
        let (view, fake, cx) = setup(cx, CONNECTED);
        // Too long for a fragment: promotion makes it a thread, so no shake.
        let long = format!("Tide tables. {}", "Low water at noon. ".repeat(60));
        let id = open_scratch(&view, &fake, &long, cx);
        cx.simulate_keystrokes("cmd-enter");
        cx.run_until_parked();
        view.read_with(cx, |v, _| {
            assert!(matches!(v.sheet, Some(Sheet::Publish { .. })));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let it = fake.item(&id).unwrap();
        assert_eq!(
            (it.status, it.kind, it.version),
            (Status::Public, Kind::Thread, 1)
        );
        let t = toast(&view, cx);
        assert!(t.starts_with("Published v1"), "{t}");
        assert!(t.contains("as a thread"), "{t}");
    }

    #[gpui_kit::test]
    fn the_omnibar_creates_scratch_with_new_note_scratch(cx: &mut TestAppContext) {
        let (view, fake, cx) = setup(cx, &format!("{CONNECTED}new-note = scratch\n"));
        cx.simulate_input("rain on tin roofs");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let first = fake.items().remove(0);
        assert_eq!(first.content_md, "rain on tin roofs");
        assert_eq!(first.status, Status::Scratch);
        assert_eq!(toast(&view, cx), "New scratch note · only on this Mac");
    }

    #[gpui_kit::test]
    fn the_omnibar_creates_drafts_by_default(cx: &mut TestAppContext) {
        let (_view, fake, cx) = setup(cx, CONNECTED);
        cx.simulate_input("rain on tin roofs");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(fake.items()[0].status, Status::Draft);
    }

    // --- follow-ups --- images in scratch notes stay local.

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n a gull on a post";

    fn editor_text(view: &Entity<MainView>, cx: &mut VisualTestContext) -> String {
        view.read_with(cx, |v, cx| v.editor.read(cx).value().to_string())
    }

    /// Paste an image (the same path as ⌘V / a drop) into the open note.
    fn paste(view: &Entity<MainView>, cx: &mut VisualTestContext) {
        view.update_in(cx, |v, window, cx| {
            v.start_upload(PNG.to_vec(), "image/png".into(), window, cx)
        });
        cx.run_until_parked();
    }

    fn settle(cx: &mut VisualTestContext) {
        for _ in 0..20 {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[gpui_kit::test]
    fn an_image_pasted_into_scratch_is_kept_locally(cx: &mut TestAppContext) {
        let (view, fake, cx) = setup(cx, CONNECTED);
        let id = open_scratch(&view, &fake, "Gulls on the harbour wall", cx);
        paste(&view, cx);

        assert_eq!(fake.upload_count(), 0, "nothing is sent anywhere");
        let text = editor_text(&view, cx);
        let refs = blyg_core::scratch_media::refs(&text);
        assert_eq!(refs.len(), 1, "{text}");
        assert!(text.contains("![](blyg-local:"), "{text}");
        assert_eq!(fake.item(&id).unwrap().content_md, text, "saved locally");
        assert_eq!(fake.item(&id).unwrap().status, Status::Scratch);
        let file = fake
            .scratch_media_file(&format!("blyg-local:{}", refs[0]))
            .expect("the image is in the scratch media folder");
        assert_eq!(std::fs::read(file).unwrap(), PNG);
        assert_eq!(toast(&view, cx), "Image kept on this Mac");

        // The studio preview shows it (inline bytes; nothing is fetched).
        let item = fake.item(&id).unwrap();
        let html = crate::app::studio::render_doc(&*fake, Some(&item), &text).html;
        assert!(html.contains("src=\"data:image/png;base64,"), "{html}");
        assert!(!html.contains("blyg-local:"), "{html}");
    }

    #[gpui_kit::test]
    fn promotion_uploads_and_rewrites_local_images(cx: &mut TestAppContext) {
        let (view, fake, cx) = setup(cx, CONNECTED);
        let id = open_scratch(&view, &fake, "Low tide mudflats at noon", cx);
        paste(&view, cx);
        cx.simulate_keystrokes("cmd-d");
        settle(cx);

        assert_eq!(fake.upload_count(), 1);
        let it = fake.item(&id).unwrap();
        assert_eq!(it.status, Status::Draft);
        assert!(!it.content_md.contains("blyg-local:"), "{}", it.content_md);
        assert!(
            it.content_md
                .contains("![](https://blyg.example.com/media/"),
            "{}",
            it.content_md
        );
        assert_eq!(editor_text(&view, cx), it.content_md, "the editor follows");
        assert_eq!(toast(&view, cx), "Now a draft on your blyg");
    }

    #[gpui_kit::test]
    fn a_failed_upload_keeps_the_scratch_note(cx: &mut TestAppContext) {
        let (view, fake, cx) = setup(cx, CONNECTED);
        let id = open_scratch(&view, &fake, "Fog horn at the point", cx);
        paste(&view, cx);
        let before = editor_text(&view, cx);
        fake.set_fail_uploads(true);

        cx.simulate_keystrokes("cmd-d");
        settle(cx);
        let it = fake.item(&id).unwrap();
        assert_eq!(it.status, Status::Scratch, "no half-promoted state");
        assert_eq!(it.content_md, before);
        let t = view.read_with(cx, |v, _| {
            let t = v.toast.as_ref().unwrap();
            format!("{} | {}", t.text, t.sub.clone().unwrap_or_default())
        });
        assert!(t.starts_with("Couldn't make a draft:"), "{t}");
        assert!(t.contains("image couldn't be uploaded"), "{t}");
        assert!(t.contains("still a scratch note"), "{t}");

        // ⌘⏎ the same: it stays scratch and says why.
        cx.simulate_keystrokes("cmd-enter");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        settle(cx);
        assert_eq!(fake.item(&id).unwrap().status, Status::Scratch);
        assert!(
            toast(&view, cx).starts_with("Publish failed"),
            "{}",
            toast(&view, cx)
        );
        assert_eq!(editor_text(&view, cx), before);
    }

    #[test]
    fn images_go_on_their_own_paragraph() {
        let (t, caret) = super::insert_image("one two", 3, "blyg-local:x");
        assert_eq!(t, "one\n\n![](blyg-local:x)\n\ntwo");
        assert_eq!(&t[..caret], "one\n\n![](blyg-local:x)");
    }

    // A web address pasted over selected text links it (any note, not only
    // scratch ones).
    #[gpui_kit::test]
    fn pasting_a_link_over_a_selection_links_it(cx: &mut TestAppContext) {
        let (view, fake, cx) = setup(cx, CONNECTED);
        let id = open_scratch(&view, &fake, "Check the tide tables first", cx);
        let paste_over = |range: std::ops::Range<usize>, clip: &str, cx: &mut VisualTestContext| {
            view.update_in(cx, |v, window, cx| {
                v.editor.update(cx, |s, cx| s.set_selected_range(range, cx));
                window.focus(&gpui_kit::Focusable::focus_handle(&v.editor, cx), cx);
            });
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(clip.to_string()));
            cx.simulate_keystrokes("cmd-v");
            cx.run_until_parked();
        };
        paste_over(10..21, "https://example.org/tides", cx);
        let text = editor_text(&view, cx);
        assert_eq!(
            text,
            "Check the [tide tables](https://example.org/tides) first"
        );
        assert_eq!(fake.item(&id).unwrap().content_md, text, "saved");

        // Not an address: an ordinary paste replaces the selection.
        paste_over(0..5, "Read", cx);
        assert!(editor_text(&view, cx).starts_with("Read the [tide tables]"));
    }
}
