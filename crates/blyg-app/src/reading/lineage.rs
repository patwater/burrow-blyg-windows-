//! Lineage (⌘J): the post in a hexagon, what it draws on above, what draws
//! on it below, one step each way. ⏎ on a neighbour makes it the centre (⌫
//! walks back); Space on the centre opens the ring of the reader's actions
//! in fixed places (f r q l v o), each previewed before ⏎ does it. The glyph
//! on stream rows is the same map in miniature: which kinds, never how many
//! (spec rule 1). Model: `lineage_vm.rs`.

use blyg_core::profile::Relation;
use blyg_core::{RemoteRef, SubscriptionKind};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::lineage_vm::{self, Act, Kinds, Mark, Model, Sel};
use super::{RSheet, vm};
use crate::app::MainView;
use crate::theme::Palette;

/// The newest pinned version of the centre, for Fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pin {
    Unknown,
    Looking,
    Found(u32),
    None,
}

pub struct Sheet {
    pub model: Model,
    pub sel: Sel,
    /// The centres walked through, oldest first (⌫ goes back).
    pub hist: Vec<(String, String)>,
    pub ring: bool,
    pub act: Option<Act>,
    pub focus: FocusHandle,
    pub pin: Pin,
}

// The stage, in its own pixels.
const STAGE_W: f32 = 540.;
const STAGE_H: f32 = 380.;
const CX: f32 = 270.;
const CY: f32 = 190.;
const HEX_R: f32 = 50.;
const RING_IN: f32 = 58.;
const RING_OUT: f32 = 118.;
const CARD_H: f32 = 48.;
const UP_Y: f32 = 12.;
const DOWN_Y: f32 = STAGE_H - 12. - CARD_H;
/// Neighbours shown per row; the side list has them all.
const ROW_MAX: usize = 5;

/// The colour of a relation: fork amber, reply the accent, quote green.
pub(crate) fn rel_color(p: &Palette, rel: Relation) -> Hsla {
    match rel {
        Relation::Forks => p.amber,
        Relation::Stubs => p.accent,
        Relation::Quotes => p.green,
    }
}

/// Cards across a row: (x, width) each.
fn lay(n: usize) -> Vec<(f32, f32)> {
    if n == 0 {
        return vec![];
    }
    let gap = 10.;
    let w = ((STAGE_W - 16. - (n as f32 - 1.) * gap) / n as f32).min(170.);
    let total = n as f32 * w + (n as f32 - 1.) * gap;
    let x0 = CX - total / 2.;
    (0..n).map(|i| (x0 + i as f32 * (w + gap), w)).collect()
}

/// The slice of a row on screen: up to `ROW_MAX` around the selection.
fn window(n: usize, sel: Option<usize>) -> std::ops::Range<usize> {
    if n <= ROW_MAX {
        return 0..n;
    }
    let s = sel
        .unwrap_or(0)
        .saturating_sub(ROW_MAX / 2)
        .min(n - ROW_MAX);
    s..s + ROW_MAX
}

fn hex_points(cx: f32, cy: f32, r: f32) -> Vec<(f32, f32)> {
    (0..6)
        .map(|i| {
            let a = (i as f32 * 60.).to_radians();
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// The centre's title in the hexagon: at most two lines of about `width`
/// characters, broken between words, the second ending in "…" when the
/// title goes on.
fn hex_lines(title: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = vec![String::new()];
    for w in title.split_whitespace() {
        let cur = lines.last_mut().expect("one line");
        let fits = cur.is_empty() || cur.chars().count() + 1 + w.chars().count() <= width;
        if fits {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(w);
        } else if lines.len() < 2 {
            lines.push(w.to_string());
        } else {
            let cur = lines.last_mut().expect("two lines");
            cur.push('…');
            break;
        }
    }
    for l in &mut lines {
        if l.chars().count() > width + 1 {
            *l = l.chars().take(width).collect::<String>() + "…";
        }
    }
    lines.retain(|l| !l.is_empty());
    lines
}

impl MainView {
    // ------------------------------------------------------------ open / close

    /// ⌘J: the lineage of the post in front of you (the open reading post,
    /// the stream's selected one, or your own published post), or close it.
    pub(crate) fn toggle_lineage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.reading.sheet, Some(RSheet::Lineage(_))) {
            self.close_reading_sheet(window, cx);
            return;
        }
        match self.lineage_target() {
            Some((origin, id)) => self.open_lineage(origin, id, window, cx),
            None => self.show_toast(
                "No post to show the lineage of",
                Some("Select a post in Reading, or open one of your published posts".into()),
                cx,
            ),
        }
    }

    fn lineage_target(&self) -> Option<(String, String)> {
        if self.reading.view == super::View::Reading {
            if let Some(o) = &self.reading.opened {
                return Some((o.item.origin.clone(), o.item.remote_id.clone()));
            }
            let sel = self.reading.sel.as_ref()?;
            let r = self.reading.rows.iter().find(|r| &vm::key(r) == sel)?;
            return Some((r.origin.clone(), r.remote_id.clone()));
        }
        let base = self.base_url.clone()?;
        let item = self.backend.item(&self.current.as_ref()?.local_id)?;
        (item.version > 0)
            .then(|| item.server_id.map(|s| (base, s.0)))
            .flatten()
    }

    /// Your published posts' references and your blyg's origin, for the
    /// glyph's "drawn on by" side.
    pub(super) fn own_lineage(&self) -> (Vec<blyg_core::PostRef>, Option<String>) {
        let Some(base) = self.base_url.clone() else {
            return (vec![], None);
        };
        let rows = &self.reading.rows;
        let refs = self
            .backend
            .items()
            .iter()
            .filter(|i| i.version > 0 && i.status == blyg_core::Status::Public)
            .flat_map(|i| i.references(&base, rows))
            .collect();
        (refs, Some(base))
    }

    fn lineage_model(&self, origin: &str, id: &str) -> Model {
        let fresh;
        let rows = if self.reading.rows.is_empty() {
            fresh = self.backend.reading();
            &fresh
        } else {
            &self.reading.rows
        };
        let own = self.backend.items();
        let mentions = self
            .reading
            .mentions
            .ready()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let src = lineage_vm::Sources {
            rows,
            own: &own,
            own_origin: self.base_url.as_deref(),
            mentions,
        };
        lineage_vm::build(origin, id, &src, self.backend.responses(origin, id))
    }

    pub(crate) fn open_lineage(
        &mut self,
        origin: String,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        let sheet = Sheet {
            model: self.lineage_model(&origin, &id),
            sel: Sel::Centre,
            hist: vec![],
            ring: false,
            act: None,
            focus: focus.clone(),
            pin: Pin::Unknown,
        };
        self.open_reading_sheet(RSheet::Lineage(Box::new(sheet)), cx);
        window.focus(&focus, cx);
    }

    /// The lineage sheet is up (the tutorial watches for it).
    pub(crate) fn lineage_open(&self) -> bool {
        matches!(self.reading.sheet, Some(RSheet::Lineage(_)))
    }

    pub(crate) fn lineage_ring_open(&self) -> bool {
        matches!(&self.reading.sheet, Some(RSheet::Lineage(s)) if s.ring)
    }

    /// Open the ring on the centre (Space), for the tour's snapshots.
    pub(crate) fn lineage_open_ring(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.lineage_mut() {
            s.sel = Sel::Centre;
            s.ring = true;
            s.act = None;
        }
        self.lineage_look_for_pin(cx);
        cx.notify();
    }

    /// Preview `act` on the open ring (screenshots).
    pub(crate) fn lineage_preview(&mut self, act: Act, cx: &mut Context<Self>) {
        if let Some(s) = self.lineage_mut().filter(|s| s.ring) {
            s.act = Some(act);
        }
        cx.notify();
    }

    fn lineage_mut(&mut self) -> Option<&mut Sheet> {
        match self.reading.sheet.as_mut() {
            Some(RSheet::Lineage(s)) => Some(s),
            _ => None,
        }
    }

    fn lineage_recentre(&mut self, origin: String, id: String, back: bool) {
        let model = self.lineage_model(&origin, &id);
        if let Some(s) = self.lineage_mut() {
            if !back {
                let c = &s.model.centre;
                s.hist.push((c.origin.clone(), c.id.clone()));
            }
            s.model = model;
            s.sel = Sel::Centre;
            s.ring = false;
            s.act = None;
            s.pin = Pin::Unknown;
        }
    }

    // ------------------------------------------------------------ keys

    fn lineage_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.lineage_mut() else {
            return false;
        };
        let (ring, act, sel) = (s.ring, s.act, s.sel);
        let node = s.model.node(sel).map(|n| (n.origin.clone(), n.id.clone()));
        let centre = (s.model.centre.origin.clone(), s.model.centre.id.clone());
        if ring {
            match key {
                "escape" if act.is_some() => {
                    if let Some(s) = self.lineage_mut() {
                        s.act = None;
                    }
                }
                "escape" | "space" => {
                    if let Some(s) = self.lineage_mut() {
                        s.ring = false;
                        s.act = None;
                    }
                }
                "enter" => {
                    if let Some(a) = act {
                        self.lineage_do(a, window, cx);
                    }
                }
                k => match Act::of_key(k) {
                    Some(a) if act == Some(a) => self.lineage_do(a, window, cx),
                    Some(a) => {
                        if let Some(s) = self.lineage_mut() {
                            s.act = Some(a);
                        }
                    }
                    None => return false,
                },
            }
            cx.notify();
            return true;
        }
        match key {
            "up" | "down" | "left" | "right" => {
                if let Some(s) = self.lineage_mut() {
                    s.sel = s.model.step(s.sel, key);
                }
            }
            "enter" | "space" => match node {
                Some((o, i)) => self.lineage_recentre(o, i, false),
                None => {
                    if let Some(s) = self.lineage_mut() {
                        s.ring = true;
                        s.act = None;
                    }
                    self.lineage_look_for_pin(cx);
                }
            },
            "backspace" => {
                let prev = self.lineage_mut().and_then(|s| s.hist.pop());
                if let Some((o, i)) = prev {
                    self.lineage_recentre(o, i, true);
                }
            }
            "o" => {
                let (o, i) = node.unwrap_or(centre);
                self.close_reading_sheet(window, cx);
                self.open_original(o, i, None, window, cx);
            }
            "escape" => self.close_reading_sheet(window, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Find the centre's newest pin (for Fork), once.
    fn lineage_look_for_pin(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.lineage_mut() else { return };
        if s.pin != Pin::Unknown {
            return;
        }
        let Some(item) = s.model.centre.held.clone() else {
            s.pin = Pin::None;
            return;
        };
        s.pin = Pin::Looking;
        let backend = self.backend.clone();
        let (sub, rid) = (item.subscription_id.clone(), item.remote_id.clone());
        let task = cx.background_spawn(async move { backend.remote_versions(&sub, &rid) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |v, cx| {
                if let Some(s) = v.lineage_mut()
                    && s.model.centre.id == item.remote_id
                {
                    s.pin = r
                        .ok()
                        .and_then(|vs| vs.iter().filter(|v| v.pinned).map(|v| v.version).max())
                        .map_or(Pin::None, Pin::Found);
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ actions

    /// Why `act` can't be done on the centre, if it can't.
    fn lineage_unavailable(&self, s: &Sheet, act: Act) -> Option<&'static str> {
        let c = &s.model.centre;
        let blyg = c.held.as_ref().is_some_and(|r| {
            self.reading
                .subs
                .iter()
                .find(|x| x.id == r.subscription_id)
                .is_none_or(|x| x.kind == SubscriptionKind::Blyg)
        });
        if c.held.is_none() && !c.own {
            return match act {
                Act::Open => None,
                _ => Some("Not in your reading list. Open ↗ shows it in the reader first."),
            };
        }
        match act {
            Act::Fork if c.own => Some("It's yours: edit it instead."),
            Act::Fork if !blyg => Some("Feed posts can't be forked."),
            Act::Fork => match s.pin {
                Pin::Unknown | Pin::Looking => Some("Looking for a pinned version…"),
                Pin::None => Some(
                    "There's no pinned version to fork. The author pins the versions others \
                     may fork.",
                ),
                Pin::Found(_) => None,
            },
            Act::Reply if c.own => Some("It's yours: publish a new version, or quote it (Q)."),
            Act::Link if !c.own && !blyg => Some("Feed posts can't be linked with [[…]]."),
            _ => None,
        }
    }

    fn lineage_do(&mut self, act: Act, window: &mut Window, cx: &mut Context<Self>) {
        let Some(RSheet::Lineage(s)) = self.reading.sheet.as_ref() else {
            return;
        };
        if let Some(why) = self.lineage_unavailable(s, act) {
            self.show_toast(why, None, cx);
            return;
        }
        let c = s.model.centre.clone();
        let pin = s.pin;
        self.close_reading_sheet(window, cx);
        let own_item = || {
            self.backend.items().into_iter().find(|i| {
                i.server_id
                    .as_ref()
                    .is_some_and(|s| s.0.eq_ignore_ascii_case(&c.id))
            })
        };
        match (act, &c.held) {
            (Act::Fork, Some(item)) => {
                let Pin::Found(version) = pin else { return };
                let of = RemoteRef {
                    origin: item.origin.clone(),
                    id: item.remote_id.clone(),
                    version,
                };
                let backend = self.backend.clone();
                let task = cx.background_spawn(async move { backend.fork(&of) });
                cx.spawn_in(window, async move |this, cx| {
                    let r = task.await;
                    let _ = this.update_in(cx, |this, window, cx| match r {
                        Ok(id) => {
                            this.open_new_draft(&id, window, cx);
                            this.show_toast(format!("Forked 📌 v{version}"), None, cx);
                        }
                        Err(e) => this.show_toast(format!("Couldn't fork: {e}"), None, cx),
                    });
                })
                .detach();
            }
            (Act::Reply, Some(item)) => self.item_action(item.clone(), "Reply", window, cx),
            (Act::Quote, Some(item)) => self.item_action(item.clone(), "Quote", window, cx),
            (Act::Link, Some(item)) => self.item_action(item.clone(), "Link post", window, cx),
            (Act::Open, Some(item)) => self.item_action(item.clone(), "Open on web", window, cx),
            (Act::Versions, Some(_)) => {
                self.open_original(c.origin.clone(), c.id.clone(), None, window, cx);
                if let Some(o) = self.reading.opened.as_mut() {
                    o.dropdown = true;
                }
            }
            (Act::Quote, None) => self.quote_into_thread(format!("![[{}]]", c.id), window, cx),
            (Act::Link, None) => match self
                .backend
                .create_draft(blyg_core::Kind::Fragment, &vm::link_post_body(&c.id))
            {
                Ok(id) => self.open_new_draft(&id, window, cx),
                Err(e) => self.show_toast(format!("Couldn't start a post: {e}"), None, cx),
            },
            (Act::Versions, None) if c.own => {
                if let Some(i) = own_item() {
                    self.open(&i.local_id, window, cx);
                    self.toggle_versions(window, cx);
                }
            }
            (Act::Open, None) if c.own => match own_item().and_then(|i| i.permalink) {
                Some(u) => cx.open_url(&u),
                None => self.show_toast("No web address for this post yet", None, cx),
            },
            (Act::Open, None) => self.open_original(c.origin, c.id, None, window, cx),
            _ => {}
        }
    }

    // ------------------------------------------------------------ the sheet

    pub(super) fn lineage_sheet_keys(&self, d: Div, cx: &mut Context<Self>) -> Div {
        d.on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
            let k = &ev.keystroke;
            if k.modifiers.platform || k.modifiers.control || k.modifiers.alt {
                return;
            }
            if this.lineage_key(k.key.as_str(), window, cx) {
                cx.stop_propagation();
            }
        }))
    }

    pub(super) fn render_lineage_sheet(&self, s: &Sheet, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette.on_page();
        let crumb = s
            .hist
            .iter()
            .map(|(o, i)| {
                self.reading
                    .rows
                    .iter()
                    .find(|r| {
                        r.remote_id.eq_ignore_ascii_case(i)
                            && blyg_core::post_key(&r.origin, "").0 == blyg_core::post_key(o, "").0
                    })
                    .map(vm::post_title)
                    .unwrap_or_else(|| vm::host(o))
            })
            .chain(std::iter::once(s.model.centre.title.clone()))
            .collect::<Vec<_>>()
            .join("  ›  ");
        let head = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .mb(px(8.))
            .text_size(px(11.5))
            .text_color(p.muted)
            .child(div().flex_1().min_w_0().truncate().child(crumb))
            .child("Responses: what this Mac holds")
            .child("⌘J / esc closes");
        let body = div()
            .flex()
            .gap(px(14.))
            .child(self.render_lineage_stage(s, cx))
            .child(self.render_lineage_side(s));
        div()
            .track_focus(&s.focus)
            .map(|d| self.lineage_sheet_keys(d, cx))
            .child(head)
            .child(body)
            .into_any_element()
    }

    fn render_lineage_stage(&self, s: &Sheet, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette.on_page();
        let m = &s.model;
        let up_sel = match s.sel {
            Sel::Up(i) => Some(i),
            _ => None,
        };
        let down_sel = match s.sel {
            Sel::Down(i) => Some(i),
            _ => None,
        };
        let uw = window(m.ups.len(), up_sel);
        let dw = window(m.downs.len(), down_sel);
        let ghost = s
            .act
            .filter(|_| s.ring)
            .filter(|a| self.lineage_unavailable(s, *a).is_none())
            .and_then(|a| {
                let passage = a == Act::Reply && self.reading_has_selection();
                a.ghost(passage).map(|g| (a, g))
            });
        let up_pos = lay(uw.len());
        let down_pos = lay(dw.len() + usize::from(ghost.is_some()));

        // Edges, hexagon and ring, painted.
        struct Edge {
            from: (f32, f32),
            to: (f32, f32),
            color: Hsla,
            partial: bool,
            dashed: bool,
        }
        let mut edges: Vec<Edge> = Vec::new();
        let dim = if s.ring { 0.28 } else { 1.0 };
        for (k, i) in uw.clone().enumerate() {
            let n = &m.ups[i];
            let (x, w) = up_pos[k];
            edges.push(Edge {
                from: (x + w / 2., UP_Y + CARD_H),
                to: (CX, CY - HEX_R * 0.87),
                color: rel_color(&p, n.relation).opacity(dim),
                partial: n.partial,
                dashed: false,
            });
        }
        for (k, i) in dw.clone().enumerate() {
            let n = &m.downs[i];
            let (x, w) = down_pos[k];
            edges.push(Edge {
                from: (x + w / 2., DOWN_Y),
                to: (CX, CY + HEX_R * 0.87),
                color: rel_color(&p, n.relation).opacity(dim),
                partial: n.partial,
                dashed: false,
            });
        }
        if let Some((a, _)) = ghost {
            let (x, w) = down_pos[down_pos.len() - 1];
            edges.push(Edge {
                from: (x + w / 2., DOWN_Y),
                to: (CX, CY + HEX_R * 0.87),
                color: a.relation().map_or(p.muted, |r| rel_color(&p, r)),
                partial: a == Act::Reply && self.reading_has_selection(),
                dashed: true,
            });
        }
        let hex_line = if s.sel == Sel::Centre {
            p.accent
        } else {
            p.ink
        };
        let hex_fill = p.bg;
        let ring = s.ring.then(|| {
            Act::ALL
                .iter()
                .map(|a| {
                    let off = self.lineage_unavailable(s, *a).is_some();
                    let base = a.relation().map_or(p.muted, |r| rel_color(&p, r));
                    let o = if s.act == Some(*a) {
                        0.9
                    } else if off {
                        0.06
                    } else {
                        0.2
                    };
                    (a.angle(), base.opacity(o))
                })
                .collect::<Vec<_>>()
        });
        let bg = p.bg;
        let painter = canvas(
            |_, _, _| (),
            move |b, (), window, _| {
                let at = |x: f32, y: f32| point(b.origin.x + px(x), b.origin.y + px(y));
                for e in &edges {
                    let mut pb = PathBuilder::stroke(px(if e.partial { 2.2 } else { 1.7 }));
                    if e.partial {
                        pb = pb.dash_array(&[px(1.5), px(3.5)]);
                    } else if e.dashed {
                        pb = pb.dash_array(&[px(4.), px(3.)]);
                    }
                    let my = (e.from.1 + e.to.1) / 2.;
                    pb.move_to(at(e.from.0, e.from.1));
                    pb.cubic_bezier_to(at(e.to.0, e.to.1), at(e.from.0, my), at(e.to.0, my));
                    if let Ok(path) = pb.build() {
                        window.paint_path(path, e.color);
                    }
                }
                let pts = hex_points(CX, CY, HEX_R);
                let poly = |pb: &mut PathBuilder| {
                    pb.move_to(at(pts[0].0, pts[0].1));
                    for q in &pts[1..] {
                        pb.line_to(at(q.0, q.1));
                    }
                    pb.close();
                };
                let mut fillp = PathBuilder::fill();
                poly(&mut fillp);
                if let Ok(path) = fillp.build() {
                    window.paint_path(path, hex_fill);
                }
                let mut line = PathBuilder::stroke(px(1.6));
                poly(&mut line);
                if let Ok(path) = line.build() {
                    window.paint_path(path, hex_line);
                }
                if let Some(ring) = &ring {
                    for (angle, color) in ring {
                        let (a1, a2) = ((angle - 29.).to_radians(), (angle + 29.).to_radians());
                        let pt = |r: f32, t: f32| at(CX + r * t.cos(), CY + r * t.sin());
                        let mut pb = PathBuilder::fill();
                        pb.move_to(pt(RING_IN, a1));
                        pb.line_to(pt(RING_OUT, a1));
                        pb.arc_to(
                            point(px(RING_OUT), px(RING_OUT)),
                            px(0.),
                            false,
                            true,
                            pt(RING_OUT, a2),
                        );
                        pb.line_to(pt(RING_IN, a2));
                        pb.arc_to(
                            point(px(RING_IN), px(RING_IN)),
                            px(0.),
                            false,
                            false,
                            pt(RING_IN, a1),
                        );
                        pb.close();
                        if let Ok(path) = pb.build() {
                            window.paint_path(path, *color);
                        }
                        let mut edge = PathBuilder::stroke(px(2.));
                        edge.move_to(pt(RING_IN, a1));
                        edge.line_to(pt(RING_OUT, a1));
                        if let Ok(path) = edge.build() {
                            window.paint_path(path, bg);
                        }
                    }
                }
            },
        )
        .absolute()
        .inset_0();

        let card = |id: (&'static str, usize),
                    x: f32,
                    w: f32,
                    y: f32,
                    title: String,
                    sub: String,
                    rel: Option<Relation>,
                    selected: bool,
                    ghost: bool| {
            div()
                .id(id)
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(w))
                .h(px(CARD_H))
                .pl(px(11.))
                .pr(px(8.))
                .py(px(6.))
                .rounded(px(8.))
                .bg(if ghost { p.bg.opacity(0.0) } else { p.bg })
                .border_1()
                .when(ghost, |d| d.border_dashed())
                .border_color(if selected {
                    p.accent
                } else if ghost {
                    rel.map_or(p.muted, |r| rel_color(&p, r))
                } else {
                    p.line
                })
                .when(selected, |d| d.border_2())
                .when(s.ring && !ghost, |d| d.opacity(0.28))
                .child(
                    div()
                        .absolute()
                        .left(px(0.))
                        .top(px(8.))
                        .w(px(3.))
                        .h(px(CARD_H - 16.))
                        .rounded(px(1.5))
                        .when_some(rel.filter(|_| !ghost), |d, r| d.bg(rel_color(&p, r))),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.ink)
                        .child(title),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(10.5))
                        .text_color(p.muted)
                        .child(sub),
                )
        };

        let mut cards: Vec<AnyElement> = Vec::new();
        for (k, i) in uw.clone().enumerate() {
            let n = &m.ups[i];
            let (x, w) = up_pos[k];
            let sub = format!(
                "{} · {}",
                n.who,
                lineage_vm::relation_words(n.relation, n.partial, true)
            );
            cards.push(
                card(
                    ("lineage-up", i),
                    x,
                    w,
                    UP_Y,
                    n.title.clone(),
                    sub,
                    Some(n.relation),
                    s.sel == Sel::Up(i),
                    false,
                )
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.lineage_click(Sel::Up(i), window, cx)
                }))
                .into_any_element(),
            );
        }
        for (k, i) in dw.clone().enumerate() {
            let n = &m.downs[i];
            let (x, w) = down_pos[k];
            let sub = format!(
                "{} · {}",
                n.who,
                lineage_vm::relation_words(n.relation, n.partial, false)
            );
            cards.push(
                card(
                    ("lineage-down", i),
                    x,
                    w,
                    DOWN_Y,
                    n.title.clone(),
                    sub,
                    Some(n.relation),
                    s.sel == Sel::Down(i),
                    false,
                )
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.lineage_click(Sel::Down(i), window, cx)
                }))
                .into_any_element(),
            );
        }
        if let Some((a, (t, sub))) = ghost {
            let (x, w) = down_pos[down_pos.len() - 1];
            cards.push(
                card(
                    ("lineage-ghost", 0),
                    x,
                    w,
                    DOWN_Y,
                    t.into(),
                    sub.into(),
                    a.relation(),
                    false,
                    true,
                )
                .into_any_element(),
            );
        }
        let more = |left: bool, y: f32, id: &'static str| {
            div()
                .id(id)
                .absolute()
                .top(px(y + CARD_H / 2. - 8.))
                .when(left, |d| d.left(px(0.)))
                .when(!left, |d| d.right(px(0.)))
                .text_color(p.muted)
                .child(if left { "‹" } else { "›" })
        };
        if uw.start > 0 {
            cards.push(more(true, UP_Y, "lineage-up-more-l").into_any_element());
        }
        if uw.end < m.ups.len() {
            cards.push(more(false, UP_Y, "lineage-up-more-r").into_any_element());
        }
        if dw.start > 0 {
            cards.push(more(true, DOWN_Y, "lineage-down-more-l").into_any_element());
        }
        if dw.end < m.downs.len() {
            cards.push(more(false, DOWN_Y, "lineage-down-more-r").into_any_element());
        }
        let empty = |y: f32, text: &'static str| {
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(y + 16.))
                .flex()
                .justify_center()
                .italic()
                .text_size(px(11.5))
                .text_color(p.muted)
                .child(text)
        };
        let c = &m.centre;
        let hex_text = div()
            .id("lineage-centre")
            .absolute()
            .left(px(CX - HEX_R * 0.82))
            .top(px(CY - 26.))
            .w(px(HEX_R * 1.64))
            .h(px(52.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(s.ring, |d| d.opacity(0.28))
            .on_click(
                cx.listener(|this, _, window, cx| this.lineage_click(Sel::Centre, window, cx)),
            )
            .children(hex_lines(&c.title, 13).into_iter().map(|line| {
                div()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.ink)
                    .line_height(relative(1.2))
                    .whitespace_nowrap()
                    .child(line)
            }))
            .child(
                div()
                    .mt(px(2.))
                    .text_size(px(10.))
                    .text_color(p.muted)
                    .truncate()
                    .child(c.who.clone()),
            );
        let labels: Vec<AnyElement> = if s.ring {
            Act::ALL
                .iter()
                .map(|a| {
                    let t = a.angle().to_radians();
                    let mid = (RING_IN + RING_OUT) / 2.;
                    let (lx, ly) = (CX + mid * t.cos(), CY + mid * t.sin());
                    let on = s.act == Some(*a);
                    let off = self.lineage_unavailable(s, *a).is_some();
                    let act = *a;
                    div()
                        .id(("lineage-wedge", a.angle() as usize))
                        .absolute()
                        .left(px(lx - 40.))
                        .top(px(ly - 15.))
                        .w(px(80.))
                        .h(px(30.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let again = matches!(&this.reading.sheet, Some(RSheet::Lineage(s)) if s.act == Some(act));
                            if again {
                                this.lineage_do(act, window, cx);
                            } else {
                                this.lineage_key(act.key(), window, cx);
                            }
                        }))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if on {
                                    p.bg
                                } else if off {
                                    p.muted
                                } else {
                                    p.ink
                                })
                                .child(a.label()),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(if on { p.bg } else { p.muted })
                                .child(a.key().to_uppercase()),
                        )
                        .into_any_element()
                })
                .collect()
        } else {
            vec![]
        };
        div()
            .relative()
            .flex_none()
            .w(px(STAGE_W))
            .h(px(STAGE_H))
            .rounded(px(10.))
            .border_1()
            .border_color(p.line)
            .bg(p.sel.opacity(0.35))
            .overflow_hidden()
            .child(painter)
            .when(m.ups.is_empty(), |d| {
                d.child(empty(UP_Y, "draws on nothing: an original"))
            })
            .when(m.downs.is_empty() && ghost.is_none(), |d| {
                d.child(empty(DOWN_Y, "no responses this Mac has seen"))
            })
            .children(cards)
            .child(hex_text)
            .children(labels)
            .into_any_element()
    }

    fn lineage_click(&mut self, sel: Sel, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.lineage_mut() else { return };
        if s.ring {
            s.ring = false;
            s.act = None;
        } else if s.sel == sel {
            self.lineage_key("enter", window, cx);
        } else {
            s.sel = sel;
        }
        cx.notify();
    }

    /// The reading pane has a passage selected (Reply quotes just that).
    /// Whether Reply would quote a passage: never since studio 0.31 (Reply
    /// quotes the whole post; a passage is chosen in the stub editor).
    fn reading_has_selection(&self) -> bool {
        false
    }

    fn render_lineage_side(&self, s: &Sheet) -> AnyElement {
        let p = self.palette.on_page();
        let keys = |rows: Vec<(&'static str, &'static str)>| {
            div()
                .mt(px(10.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .text_size(px(11.5))
                .text_color(p.muted)
                .children(rows.into_iter().map(|(k, what)| {
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(div().w(px(56.)).flex_none().text_color(p.ink).child(k))
                        .child(div().flex_1().min_w_0().child(what))
                }))
        };
        let side = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .text_size(px(12.5))
            .line_height(relative(1.45));
        if s.ring {
            let Some(a) = s.act else {
                return side
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(14.))
                            .child("Actions"),
                    )
                    .child(
                        div().text_color(p.muted).child(
                            "A letter previews what it makes; ⏎ does it; esc closes the ring.",
                        ),
                    )
                    .child(keys(
                        Act::ALL
                            .iter()
                            .map(|a| {
                                (
                                    match a {
                                        Act::Fork => "F",
                                        Act::Reply => "R",
                                        Act::Quote => "Q",
                                        Act::Link => "L",
                                        Act::Versions => "V",
                                        Act::Open => "O",
                                    },
                                    a.one(),
                                )
                            })
                            .collect(),
                    ))
                    .into_any_element();
            };
            let passage = a == Act::Reply && self.reading_has_selection();
            let why = self.lineage_unavailable(s, a);
            let facts = a.facts(passage);
            return side
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(14.))
                        .child(
                            div()
                                .w(px(10.))
                                .h(px(10.))
                                .rounded(px(3.))
                                .bg(a.relation().map_or(p.muted, |r| rel_color(&p, r))),
                        )
                        .child(if passage {
                            format!("{} (to the passage)", a.name())
                        } else {
                            a.name().to_string()
                        }),
                )
                .child(div().mt(px(2.)).child(a.one()))
                .child(
                    div()
                        .mt(px(4.))
                        .text_color(p.muted)
                        .text_size(px(12.))
                        .child(a.long()),
                )
                .child(
                    div()
                        .mt(px(8.))
                        .pt(px(8.))
                        .border_t_1()
                        .border_color(p.line)
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .text_size(px(12.))
                        .children(lineage_vm::FACTS.iter().zip(facts).map(|(q, (ans, yes))| {
                            div()
                                .flex()
                                .gap(px(8.))
                                .child(div().w(px(150.)).flex_none().text_color(p.muted).child(*q))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .when(yes, |d| d.font_weight(FontWeight::SEMIBOLD))
                                        .when(!yes, |d| d.text_color(p.muted))
                                        .child(ans),
                                )
                        })),
                )
                .child(match why {
                    Some(w) => div()
                        .mt(px(8.))
                        .text_color(p.amber)
                        .text_size(px(12.))
                        .child(w),
                    None => div()
                        .mt(px(8.))
                        .text_color(p.muted)
                        .text_size(px(12.))
                        .child(format!(
                            "⏎ or {} again does it · esc back",
                            a.key().to_uppercase()
                        )),
                })
                .into_any_element();
        }
        let m = &s.model;
        let (title, who, text) = match m.node(s.sel) {
            Some(n) => (
                n.title.clone(),
                format!("{} · {}", n.who, vm::host(&n.origin)),
                n.held.as_ref().map(|r| vm::title(&r.content_md)),
            ),
            None => (
                m.centre.title.clone(),
                format!("{} · {}", m.centre.who, vm::host(&m.centre.origin)),
                None,
            ),
        };
        let list = (s.sel == Sel::Centre).then(|| {
            div()
                .mt(px(8.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .text_size(px(12.))
                .children(m.ups.iter().map(|n| {
                    self.lineage_list_row(
                        &p,
                        n.relation,
                        format!(
                            "{} {}'s {}",
                            lineage_vm::relation_words(n.relation, n.partial, true),
                            n.who,
                            n.title
                        ),
                    )
                }))
                .children(m.downs.iter().map(|n| {
                    self.lineage_list_row(
                        &p,
                        n.relation,
                        format!(
                            "{} {}",
                            n.who,
                            lineage_vm::relation_words(n.relation, n.partial, false)
                        ),
                    )
                }))
        });
        let has_back = !s.hist.is_empty();
        side.child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(14.))
                .child(title),
        )
        .child(div().text_color(p.muted).text_size(px(11.5)).child(who))
        .when_some(text.filter(|t| !t.is_empty()), |d, t| {
            d.child(div().mt(px(6.)).line_clamp(4).child(t))
        })
        .children(list)
        .child(keys(
            [
                Some(("← ↑ ↓ →", "move")),
                Some((
                    "⏎",
                    if s.sel == Sel::Centre {
                        "actions (or Space)"
                    } else {
                        "make it the centre"
                    },
                )),
                has_back.then_some(("⌫", "back")),
                Some(("o", "open in the reader")),
                Some(("esc", "close")),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ))
        .into_any_element()
    }

    fn lineage_list_row(&self, p: &Palette, rel: Relation, text: String) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .flex_none()
                    .w(px(8.))
                    .h(px(8.))
                    .rounded(px(2.))
                    .bg(rel_color(p, rel)),
            )
            .child(div().min_w_0().truncate().child(text))
    }

    // ------------------------------------------------------------ the glyph

    /// The glyph for `r`: kinds in from the left (what it draws on), kinds
    /// out to the right (what draws on it). Fixed slots: fork, reply, quote.
    /// A click opens the lineage. Nothing when it has neither.
    pub(super) fn render_lineage_glyph(
        &self,
        r: &blyg_core::ReadingItem,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let up = Kinds::of(&r.references());
        let down = self
            .reading
            .down
            .get(&blyg_core::post_key(&r.origin, &r.remote_id))
            .copied()
            .unwrap_or_default();
        if up.is_empty() && down.is_empty() {
            return None;
        }
        let p = self.palette.on_page();
        let colors = [
            (Relation::Forks, rel_color(&p, Relation::Forks), 3.),
            (Relation::Stubs, rel_color(&p, Relation::Stubs), 8.),
            (Relation::Quotes, rel_color(&p, Relation::Quotes), 13.),
        ];
        let ink = p.ink;
        let bg = p.bg;
        let tip = {
            let mut lines = Vec::new();
            let u = up.words(true);
            if !u.is_empty() {
                lines.push(format!("This post {}", u.join(", ")));
            }
            let d = down.words(false);
            if !d.is_empty() {
                lines.push(format!("Others have {} it", d.join(", ")));
            }
            lines.push("⌘J or a click: see who".into());
            lines.join("\n")
        };
        let (origin, id) = (r.origin.clone(), r.remote_id.clone());
        let painter = canvas(
            |_, _, _| (),
            move |b, (), window, _| {
                let at = |x: f32, y: f32| point(b.origin.x + px(x), b.origin.y + px(y));
                let (cx_, cy_) = (23., 8.);
                for (rel, color, y) in colors {
                    for (kinds, left) in [(up, true), (down, false)] {
                        let mark = kinds.get(rel);
                        if mark == Mark::None {
                            continue;
                        }
                        let mut pb = PathBuilder::stroke(px(1.5));
                        if mark == Mark::Partial {
                            pb = pb.dash_array(&[px(1.5), px(2.5)]);
                        }
                        let (x0, x1) = if left {
                            (3., cx_ - 5.)
                        } else {
                            (43., cx_ + 5.)
                        };
                        pb.move_to(at(x0, y));
                        pb.cubic_bezier_to(
                            at(x1, cy_),
                            at((x0 + x1) / 2., y),
                            at((x0 + x1) / 2., cy_),
                        );
                        if let Ok(path) = pb.build() {
                            window.paint_path(path, color);
                        }
                        window.paint_quad(
                            fill(
                                Bounds {
                                    origin: at(x0 - 2., y - 2.),
                                    size: size(px(4.), px(4.)),
                                },
                                color,
                            )
                            .corner_radii(px(2.)),
                        );
                    }
                }
                let pts = hex_points(cx_, cy_, 4.8);
                let poly = |pb: &mut PathBuilder| {
                    pb.move_to(at(pts[0].0, pts[0].1));
                    for q in &pts[1..] {
                        pb.line_to(at(q.0, q.1));
                    }
                    pb.close();
                };
                let mut f = PathBuilder::fill();
                poly(&mut f);
                if let Ok(path) = f.build() {
                    window.paint_path(path, bg);
                }
                let mut l = PathBuilder::stroke(px(1.1));
                poly(&mut l);
                if let Ok(path) = l.build() {
                    window.paint_path(path, ink);
                }
            },
        )
        .size_full();
        Some(
            div()
                .id("lineage-glyph")
                .debug_selector(|| "lineage-glyph".into())
                .w(px(46.))
                .h(px(16.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|s| s.bg(p.sel))
                .child(painter)
                .tooltip(move |_, cx| cx.new(|_| super::Tip(tip.clone())).into())
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_lineage(origin.clone(), id.clone(), window, cx)
                }))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod hex_tests {
    use super::hex_lines;

    #[test]
    fn the_hexagon_title_fits_two_lines() {
        assert_eq!(
            hex_lines("On turning beds over", 13),
            ["On turning", "beds over"]
        );
        assert_eq!(
            hex_lines("Three things on the bench this week", 13),
            ["Three things", "on the bench…"]
        );
        assert_eq!(hex_lines("Supercalifragilistic", 13), ["Supercalifrag…"]);
        assert!(hex_lines("", 13).is_empty());
    }
}
