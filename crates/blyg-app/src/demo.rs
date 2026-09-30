//! `BLYGGER_DEMO=<scenario>`: scripted, in-process walkthroughs (like the
//! mock's ▶ demos) used for screenshots and smoke-testing without synthetic OS
//! input. They call the same methods a keystroke would.

use std::time::Duration;

use blyg_core::LocalId;
use gpui_kit::*;

use super::MainView;

impl MainView {
    pub fn run_demo(&mut self, scenario: &str, window: &mut Window, cx: &mut Context<Self>) {
        let scenario = scenario.to_string();
        cx.spawn_in(window, async move |this, cx| {
            let exec = cx.background_executor().clone();
            let step = |ms: u64| exec.timer(Duration::from_millis(ms));
            step(400).await;
            let _ = this.update_in(cx, |v, window, cx| v.demo_step(&scenario, 0, window, cx));
            step(900).await;
            let _ = this.update_in(cx, |v, window, cx| v.demo_step(&scenario, 1, window, cx));
            step(1200).await;
            let _ = this.update_in(cx, |v, window, cx| v.demo_step(&scenario, 2, window, cx));
        })
        .detach();
    }

    fn demo_type(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |s, cx| s.insert(text.to_string(), window, cx));
        self.after_edit(cx);
    }

    fn demo_open(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open(&LocalId(id.into()), window, cx);
    }

    fn demo_step(&mut self, scenario: &str, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        match (scenario, n) {
            // Live filter with highlighted matches, ↓ previews the next row.
            ("search", 0) => self.set_query_text("the", window, cx),
            ("search", 1) => self.move_selection(1, window, cx),
            // ⏎ with no match creates a draft seeded with the query.
            ("create", 0) => self.set_query_text("tide pools", window, cx),
            ("create", 1) => {
                let seed = self.list.trimmed_query().to_string();
                self.create_from(&seed, window, cx);
                self.demo_type(
                    " are small oceans that forget, twice a day, that they belong to a larger one.",
                    window,
                    cx,
                );
            }
            ("publish", 0) => {
                self.demo_open("01J9QK3", window, cx);
                self.demo_type(" So is a good notebook.", window, cx);
            }
            ("publish", 1) => {
                self.publish(window, cx);
                if let Some(super::Sheet::Publish { note, .. }) = &self.sheet {
                    note.update(cx, |s, cx| s.set_value("first version", window, cx));
                }
            }
            ("published", 0) => self.demo_open("01J9QK3", window, cx),
            ("published", 1) => {
                self.publish(window, cx);
                self.do_publish("first version".into(), window, cx);
            }
            ("long", 0) => {
                self.demo_open("01J9M2A", window, cx);
                let extra = " And the cost is not only time. Each extra field, each login screen, each preview that takes a second to render asks the same question: is this thought worth the trouble? Most thoughts, asked that way, say no.".repeat(5);
                self.demo_type(&extra, window, cx);
            }
            ("long", 1) => self.publish(window, cx),
            // --- full editor: ⌘3 on a thread with a quote, TK and a video ---
            ("studio", 0) => self.studio_demo(window, cx),
            ("studio-edit", 0) => self.studio_demo(window, cx),
            ("studio-edit", 1) => self.demo_type(" Tide tables.", window, cx),
            ("preview", 0) => self.demo_open("01J9K7T", window, cx),
            ("preview", 1) => self.toggle_preview(&super::TogglePreview, window, cx),
            ("conflict", 0) => {
                self.demo_open("01J9K7T", window, cx);
                self.editor
                    .update(cx, |s, cx| s.set_selected_range(41..41, cx));
                self.demo_type(" (A. Gardener)", window, cx);
            }
            ("conflict", 1) => {
                if let (Some(f), Some(cur)) = (&self.fake, &self.current) {
                    f.trigger_conflict(&cur.local_id);
                }
            }
            ("offline", 0) => {
                self.demo_open("01J9QK3", window, cx);
                if let Some(f) = &self.fake {
                    f.set_offline(true);
                }
            }
            ("offline", 1) => self.demo_type(" Written on a plane.", window, cx),
            ("settings", 0) => self.open_settings(&super::OpenSettings, window, cx),
            ("capture", 0) => crate::capture::toggle(cx),
            ("capture", 1) => crate::capture::demo_fill(
                "The best interface for writing is the one that is already open.",
                cx,
            ),
            ("image", 0) => {
                self.demo_open("01J9PX1", window, cx);
                // A small generated PNG stands in for a pasted screenshot.
                let png = demo_png();
                self.start_upload(png, "image/png".into(), window, cx);
            }
            ("image", 2) => self.toggle_preview(&super::TogglePreview, window, cx),
            ("edit", 0) => {
                self.set_query_text("harb", window, cx);
            }
            // Against a real (local) blyg: write a new fragment and publish it.
            ("live-publish", 0) => {
                self.set_query_text("Morning notes", window, cx);
                let seed = self.list.trimmed_query().to_string();
                self.create_from(&seed, window, cx);
                self.demo_type(
                    ": the kettle, the window, the first sentence that arrives before the coffee does.",
                    window,
                    cx,
                );
            }
            ("live-publish", 1) => {
                self.publish(window, cx);
                self.do_publish("first version".into(), window, cx);
            }
            // The newest post, in the editor (after the initial sync).
            ("live-open", 1) => {
                self.back_to_search(window, cx);
                if let Some(id) = self.list.results().first().map(|i| i.local_id.clone()) {
                    self.open(&id, window, cx);
                }
            }
            ("live-image", 0) => {
                if let Some(id) = self.list.results().first().map(|i| i.local_id.clone()) {
                    self.open(&id, window, cx);
                    let png = demo_png();
                    self.start_upload(png, "image/png".into(), window, cx);
                }
            }
            ("live-image", 2) => self.toggle_preview(&super::TogglePreview, window, cx),
            // The newest post in the preview: its images come over HTTP
            // through the disk cache (a fresh data dir has none cached).
            ("live-preview", 1) => {
                if let Some(id) = self.list.results().first().map(|i| i.local_id.clone()) {
                    self.open(&id, window, cx);
                }
            }
            ("live-preview", 2) => self.toggle_preview(&super::TogglePreview, window, cx),
            // The Connect sheet checking a wrong token against a real blyg
            // (BLYGGER_DEMO_URL; nothing is saved when the check fails).
            ("connect-check", 0) => {
                let url = std::env::var("BLYGGER_DEMO_URL").unwrap_or_default();
                if let Some(super::Sheet::Connect { url: u, token, .. }) = &self.sheet {
                    u.update(cx, |s, cx| s.set_value(url, window, cx));
                    token.update(cx, |s, cx| s.set_value("not-the-right-token", window, cx));
                }
                self.submit_connect(window, cx);
            }
            ("edit", 1) => {
                if let super::EnterAction::Open(id) = self.list.enter() {
                    self.open(&id, window, cx);
                }
                self.demo_type(" Nobody in the crowd noticed.", window, cx);
            }
            // --- AI --- (ai-fill, ai-palette, ai-settings, ai-shorten)
            (s, n) if s.starts_with("ai-") => self.ai_demo_step(s, n, window, cx),
            // --- reading & versions (reading/demo.rs) ---
            (s, n) if s.starts_with("rd-") => self.reading_demo(s, n, window, cx),
            // --- profiles --- (profiles/demo.rs)
            (s, n) if s.starts_with("pf-") => self.profile_demo(s, n, window, cx),
            // --- onboarding --- (ob-welcome, ob-connect, ob-ai, ob-buttons, ob-tour, tut-<step id>)
            (s, n) if s.starts_with("ob-") || s.starts_with("tut-") => {
                self.onboarding_demo(s, n, window, cx)
            }
            // --- buttons --- (tb-main, tb-long, tb-capture; toolbar.rs)
            (s, n) if s.starts_with("tb-") => self.toolbar_demo(s, n, window, cx),
            _ => {}
        }
    }
}

/// A 600×300 sunset gradient PNG (uncompressed deflate), made at runtime.
fn demo_png() -> Vec<u8> {
    let (w, h) = (600u32, 300u32);
    let mut raw = Vec::with_capacity(((w * 3 + 1) * h) as usize);
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            let t = y as f32 / h as f32;
            let hill = (x as f32 / 90.0).sin() * 18.0 + 215.0;
            let (r, g, b) = if (y as f32) > hill {
                (0x3d, 0x40, 0x5b)
            } else {
                let dx = x as f32 - 430.0;
                let dy = y as f32 - 120.0;
                if dx * dx + dy * dy < 46.0 * 46.0 {
                    (0xff, 0xf4, 0xd6)
                } else {
                    (
                        (0xf3 as f32 * (1.0 - t) + 0xe0 as f32 * t) as u8,
                        (0xc6 as f32 * (1.0 - t) + 0x7a as f32 * t) as u8,
                        (0x8a as f32 * (1.0 - t) + 0x5f as f32 * t) as u8,
                    )
                }
            };
            raw.extend_from_slice(&[r, g, b]);
        }
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    // zlib stream with stored (uncompressed) deflate blocks.
    let mut z = vec![0x78, 0x01];
    for (i, block) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(last as u8);
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    png
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_in = kind.to_vec();
    crc_in.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}
