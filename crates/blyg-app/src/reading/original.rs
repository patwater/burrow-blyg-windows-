//! Opening a quote's, stub's or fork's original post (issue #3): from the
//! quote box in the reader's WebView or the stream, and from the lineage
//! lines. A post held in the local reading store opens at once (at the
//! quoted version when that version is pinned); one that isn't is fetched
//! from its author's public item document (`Backend::public_item`, with the
//! token-less public client) and shown with a "Subscribe" action.

use gpui_kit::*;

use super::vm::{self, Key};
use super::{Load, Opened, View};
use crate::app::MainView;

/// The key prefix of a post shown without a subscription (fetched on
/// demand): `("ext:<origin>", id)`. Never a real subscription id.
pub const EXTERNAL: &str = "ext:";

pub fn external_key(origin: &str, id: &str) -> Key {
    (format!("{EXTERNAL}{origin}"), id.to_string())
}

pub fn is_external(key: &Key) -> bool {
    key.0.starts_with(EXTERNAL)
}

impl MainView {
    /// Open the post `id` on `origin` in the reading pane (the side pane in
    /// the stream, the detail in the Reader), at `version` when that version
    /// can be shown (current or pinned).
    pub(crate) fn open_original(
        &mut self,
        origin: String,
        id: String,
        version: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reading.view != View::Reading {
            self.show_view(View::Reading, window, cx);
        }
        let same_id = |r: &&blyg_core::ReadingItem| r.remote_id.eq_ignore_ascii_case(&id);
        let held = self
            .reading
            .rows
            .iter()
            .filter(same_id)
            .find(|r| blyg_core::profile::same_origin(&r.origin, &origin))
            .or_else(|| self.reading.rows.iter().find(same_id))
            .map(vm::key);
        if let Some(key) = held {
            self.reading.want_version = version;
            self.open_reading(key, window, cx);
            return;
        }
        let key = external_key(&origin, &id);
        if self.reading.opened.as_ref().is_some_and(|o| o.key == key) {
            return;
        }
        self.show_toast(
            format!("Fetching the original from {}…", vm::host(&origin)),
            None,
            cx,
        );
        let backend = self.backend.clone();
        let (o2, i2) = (origin.clone(), id.clone());
        let task =
            cx.background_spawn(async move { crate::guarded(|| backend.public_item(&o2, &i2)) });
        cx.spawn_in(window, async move |this, cx| {
            let r = task.await;
            let _ = this.update_in(cx, |v, window, cx| match r {
                Ok(p) => {
                    let shown = vm::shown(&p.versions);
                    let current = vm::current_ix(&shown);
                    let responses = v.backend.responses(&p.item.origin, &p.item.remote_id);
                    v.reading.opened = Some(Opened {
                        responses,
                        key: key.clone(),
                        item: p.item,
                        changelog: Load::Ready(p.versions),
                        shown: Load::Ready(shown.clone()),
                        diff_base: None,
                        ix: current,
                        dropdown: false,
                        pins: Default::default(),
                        diff_vs_now: false,
                    });
                    v.toast = None;
                    if let Some(i) =
                        version.and_then(|want| shown.iter().position(|s| s.version == want))
                        && Some(i) != current
                    {
                        v.select_version(i, cx);
                    }
                    window.focus(&v.reading.focus, cx);
                    cx.notify();
                }
                Err(e) => v.show_toast(format!("Couldn't fetch the original: {e}"), None, cx),
            });
        })
        .detach();
    }
}
