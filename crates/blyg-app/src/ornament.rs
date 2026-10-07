//! Ornaments: the theme slots drawn natively (gradients, quads, paths and
//! monochrome SVG). Each function takes the resolved [`Theme`] and returns
//! the element for one slot, or `None` when the slot is empty, so a plain
//! theme draws exactly what it did before themes existed.
//!
//! Rules every ornament keeps: low contrast, deterministic (no flicker from
//! frame to frame), cheap (one path per colour, capped counts), and never
//! behind body text. The editor's ornaments are clipped to its margins.

use blyg_core::config::theme::{Border, Kind, Ornament, Slot};
use gpui_kit::*;

use crate::theme::{Palette, Theme, hsla};

// ------------------------------------------------------------------ helpers

/// A stable pseudo-random number in `0..1` for `i` (no RNG, no flicker).
fn noise(i: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x & 0xff_ffff) as f32 / 0x100_0000 as f32
}

/// The `i`th colour of an ornament, or `default`, at the ornament's opacity
/// (or `default_opacity`).
fn col(o: &Ornament, i: usize, default: Hsla, default_opacity: f32) -> Hsla {
    let c = o.color(i).map(hsla).unwrap_or(default);
    c.opacity(o.opacity.unwrap_or(default_opacity))
}

/// A colour without the ornament's opacity.
fn solid(o: &Ornament, i: usize, default: Hsla) -> Hsla {
    o.color(i).map(hsla).unwrap_or(default)
}

fn at(b: &Bounds<Pixels>, x: f32, y: f32) -> Point<Pixels> {
    point(b.origin.x + px(x), b.origin.y + px(y))
}

fn wh(b: &Bounds<Pixels>) -> (f32, f32) {
    (f32::from(b.size.width), f32::from(b.size.height))
}

fn rect(b: &Bounds<Pixels>, x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds {
        origin: at(b, x, y),
        size: size(px(w), px(h)),
    }
}

fn stroke(window: &mut Window, width: f32, color: Hsla, build: impl FnOnce(&mut PathBuilder)) {
    let mut pb = PathBuilder::stroke(px(width));
    build(&mut pb);
    if let Ok(path) = pb.build() {
        window.paint_path(path, color);
    }
}

fn fill_path(window: &mut Window, color: Hsla, build: impl FnOnce(&mut PathBuilder)) {
    let mut pb = PathBuilder::fill();
    build(&mut pb);
    if let Ok(path) = pb.build() {
        window.paint_path(path, color);
    }
}

fn dot(window: &mut Window, b: &Bounds<Pixels>, x: f32, y: f32, r: f32, color: Hsla) {
    window.paint_quad(fill(rect(b, x - r, y - r, r * 2., r * 2.), color).corner_radii(px(r)));
}

/// A canvas that fills its parent and paints `f` into its bounds.
fn painter(f: impl 'static + FnOnce(Bounds<Pixels>, &mut Window, &mut App)) -> impl IntoElement {
    canvas(|_, _, _| (), move |b, (), window, cx| f(b, window, cx)).size_full()
}

/// An absolutely positioned layer over its (relative) parent.
fn layer() -> Div {
    div().absolute().top_0().left_0().size_full()
}

fn paint_svg_art(o: &Ornament, b: Bounds<Pixels>, color: Hsla, window: &mut Window, cx: &App) {
    let Some(svg) = &o.svg else { return };
    let key: SharedString = format!("theme-svg:{}", svg.path.display()).into();
    let _ = window.paint_svg(
        b,
        key,
        Some(&svg.bytes),
        TransformationMatrix::unit(),
        color,
        cx,
    );
}

// ------------------------------------------------------------------ fields

fn gradient_bg(o: &Ornament, fallback: Hsla) -> Div {
    let c0 = solid(o, 0, fallback);
    let c1 = solid(o, 1, c0);
    match o.color(2).map(hsla) {
        // Three colours: across (a chart's paper, lighter in the middle).
        Some(c2) => div()
            .flex()
            .child(div().h_full().flex_1().bg(linear_gradient(
                90.,
                linear_color_stop(c0, 0.),
                linear_color_stop(c1, 1.),
            )))
            .child(div().h_full().flex_1().bg(linear_gradient(
                90.,
                linear_color_stop(c1, 0.),
                linear_color_stop(c2, 1.),
            ))),
        None => div().bg(linear_gradient(
            180.,
            linear_color_stop(c0, 0.),
            linear_color_stop(c1, 1.),
        )),
    }
}

fn paint_strata(o: &Ornament, b: Bounds<Pixels>, window: &mut Window) {
    let (w, h) = wh(&b);
    let grass = solid(o, 0, gpui_kit::green());
    let layers: Vec<Hsla> = o.colors.iter().skip(1).map(|c| hsla(*c)).collect();
    let top = 6.0;
    window.paint_quad(fill(rect(&b, 0., 0., w, top), grass));
    // Deeper strata get thicker, as in a real section.
    let stops: Vec<f32> = match layers.len() {
        4 => vec![0.34, 0.62, 0.82, 1.0],
        n => (1..=n).map(|i| i as f32 / n as f32).collect(),
    };
    let mut y = top;
    for (c, s) in layers.iter().zip(stops) {
        let y1 = top + (h - top) * s;
        window.paint_quad(fill(rect(&b, 0., y, w, y1 - y + 1.), *c));
        y = y1;
    }
    // A wavering seam under the grass, faint bedding lines and pebbles.
    let seam = black().opacity(0.18);
    stroke(window, 1.5, seam, |pb| {
        pb.move_to(at(&b, 0., top + 1.));
        let mut x = 0.;
        let mut i = 0;
        while x < w {
            let nx = x + 18.;
            pb.curve_to(
                at(&b, nx, top + 1. + noise(i) * 2.),
                at(&b, x + 9., top + 3.),
            );
            x = nx;
            i += 1;
        }
    });
    let mut yy = top + 58.;
    while yy < h {
        window.paint_quad(fill(rect(&b, 0., yy, w, 2.), black().opacity(0.06)));
        yy += 60.;
    }
    let n = ((w * h) / 2600.).min(300.) as u32;
    for i in 0..n {
        let (x, y) = (
            noise(i * 3) * w,
            top + 8. + noise(i * 3 + 1) * (h - top - 8.),
        );
        let r = 1.0 + noise(i * 3 + 2) * 1.6;
        dot(window, &b, x, y, r, black().opacity(0.10));
    }
}

fn paint_moss(o: &Ornament, b: Bounds<Pixels>, window: &mut Window) {
    let (w, h) = wh(&b);
    let n = ((w * h) / 15000.).clamp(4., 30.) as u32;
    let colors: Vec<Hsla> = (0..o.colors.len().max(1))
        .map(|i| col(o, i, gpui_kit::green(), 0.3))
        .collect();
    for i in 0..n {
        let c = colors[i as usize % colors.len()];
        let (x, y) = (noise(i * 5) * w, noise(i * 5 + 1) * h);
        let r = 16. + noise(i * 5 + 2) * 18.;
        // Soft rings build a cushion with a blurred edge.
        for (k, a) in [(1.0, 0.18), (0.86, 0.22), (0.72, 0.26), (0.56, 0.3)] {
            dot(window, &b, x, y, r * k, c.opacity(a));
        }
    }
}

/// Seigaiha scales, row by row: each scale covers the one behind it.
fn paint_seigaiha(
    o: &Ornament,
    b: Bounds<Pixels>,
    ground: (Hsla, Hsla),
    tile: f32,
    window: &mut Window,
) {
    let (w, h) = wh(&b);
    let line = col(o, 0, white(), 0.14);
    let r = tile / 2.;
    let step = r / 2.;
    let mut row = 0;
    let mut y = step;
    while y < h + r {
        let t = (y / h.max(1.)).clamp(0., 1.);
        let g = lerp(ground.0, ground.1, t);
        let off = if row % 2 == 0 { 0. } else { r };
        let centers: Vec<f32> = {
            let mut v = Vec::new();
            let mut x = -r + off;
            while x < w + r {
                v.push(x);
                x += tile;
            }
            v
        };
        fill_path(window, g, |pb| {
            for &cx in &centers {
                pb.move_to(at(&b, cx - r, y));
                pb.arc_to(point(px(r), px(r)), px(0.), false, true, at(&b, cx + r, y));
                pb.close();
            }
        });
        stroke(window, 1., line, |pb| {
            for &cx in &centers {
                for k in [1.0, 0.75, 0.5] {
                    let rr = r * k - 0.5;
                    pb.move_to(at(&b, cx - rr, y));
                    pb.arc_to(
                        point(px(rr), px(rr)),
                        px(0.),
                        false,
                        true,
                        at(&b, cx + rr, y),
                    );
                }
            }
        });
        y += step;
        row += 1;
        if row > 400 {
            break;
        }
    }
}

fn lerp(a: Hsla, b: Hsla, t: f32) -> Hsla {
    crate::theme::mix(a, b, t)
}

fn paint_contours(o: &Ornament, b: Bounds<Pixels>, window: &mut Window) {
    let (w, h) = wh(&b);
    let line = col(o, 0, gpui_kit::blue(), 0.55);
    let mut k = 0u32;
    let mut y = 40.;
    while y < h + 20. && k < 60 {
        let a = 10. + noise(k) * 10.;
        stroke(window, 0.8, line, |pb| {
            pb.move_to(at(&b, -10., y));
            let seg = 110.;
            let mut x = -10.;
            let mut i = 0;
            while x < w + 10. {
                let dir = if i % 2 == 0 { -1. } else { 1. };
                pb.cubic_bezier_to(
                    at(&b, x + seg, y + noise(k * 7 + i) * 6. - 3.),
                    at(&b, x + seg * 0.35, y + dir * a),
                    at(&b, x + seg * 0.65, y - dir * a),
                );
                x += seg;
                i += 1;
            }
        });
        y += 56. + noise(k + 99) * 10.;
        k += 1;
    }
}

fn paint_woodgrain(o: &Ornament, b: Bounds<Pixels>, window: &mut Window) {
    let (w, h) = wh(&b);
    let c = col(o, 0, gpui_kit::black(), 0.07);
    let mut x = 0.;
    let mut i = 0;
    while x < w {
        let wd = if noise(i) > 0.7 { 2.5 } else { 1.5 };
        window.paint_quad(fill(rect(&b, x, 0., wd, h), c));
        x += 5. + noise(i + 500) * 5.;
        i += 1;
    }
    let mut x = 11.;
    while x < w {
        window.paint_quad(fill(rect(&b, x, 0., 1., h), c.opacity(0.7)));
        x += 23.;
    }
}

fn paint_lattice(o: &Ornament, b: Bounds<Pixels>, window: &mut Window) {
    let (w, h) = wh(&b);
    let c = col(o, 0, gpui_kit::black(), 0.35);
    let asanoha = o.variant.as_deref() != Some("kumiko");
    stroke(window, 1., c, |pb| {
        if asanoha {
            // Three families of lines: the triangular lattice asanoha is cut from.
            let s = 10.;
            let dx = h / 3f32.sqrt();
            let mut y = s;
            while y < h {
                pb.move_to(at(&b, 0., y));
                pb.line_to(at(&b, w, y));
                y += s;
            }
            let step = 2. * s / 3f32.sqrt();
            let mut x = -dx;
            while x < w + dx {
                pb.move_to(at(&b, x, 0.));
                pb.line_to(at(&b, x + dx, h));
                pb.move_to(at(&b, x + dx, 0.));
                pb.line_to(at(&b, x, h));
                x += step;
            }
        } else {
            // Square cells with both diagonals (kaku-asanoha).
            let s = 12.;
            let mut x = 0.;
            while x <= w {
                pb.move_to(at(&b, x, 0.));
                pb.line_to(at(&b, x, h));
                x += s;
            }
            let mut y = 0.;
            while y <= h {
                pb.move_to(at(&b, 0., y));
                pb.line_to(at(&b, w, y));
                y += s;
            }
            let mut d = -h;
            while d < w + h {
                pb.move_to(at(&b, d, 0.));
                pb.line_to(at(&b, d + h, h));
                pb.move_to(at(&b, d + h, 0.));
                pb.line_to(at(&b, d, h));
                d += s;
            }
        }
    });
}

/// Root hairlines hanging from the top edge, repeating every 220 points.
fn paint_roots(o: &Ornament, b: Bounds<Pixels>, window: &mut Window) {
    let (w, _) = wh(&b);
    let c = col(o, 0, gpui_kit::black(), 0.55);
    // (start, [end, control a, control b]) from the mock's SVG, per 220-point tile.
    type Pt = (f32, f32);
    #[rustfmt::skip]
    const ROOTS: &[(Pt, [Pt; 3])] = &[
        ((30., 0.), [(26., 44.), (32., 18.), (22., 26.)]),
        ((26., 30.), [(44., 46.), (34., 36.), (40., 34.)]),
        ((90., 0.), [(90., 52.), (86., 16.), (96., 30.)]),
        ((92., 22.), [(70., 40.), (80., 30.), (76., 38.)]),
        ((160., 0.), [(160., 48.), (164., 20.), (154., 28.)]),
        ((158., 28.), [(180., 50.), (170., 34.), (176., 44.)]),
    ];
    stroke(window, 1., c, |pb| {
        let mut ox = 0.;
        while ox < w {
            for ((sx, sy), [to, a, bb]) in ROOTS {
                pb.move_to(at(&b, ox + sx, *sy));
                pb.cubic_bezier_to(
                    at(&b, ox + to.0, to.1),
                    at(&b, ox + a.0, a.1),
                    at(&b, ox + bb.0, bb.1),
                );
            }
            ox += 220.;
        }
    });
}

/// Specks: stone floor, laterite pitting or salt grain.
fn paint_speckle(o: &Ornament, b: Bounds<Pixels>, window: &mut Window) {
    let (w, h) = wh(&b);
    match o.kind {
        Kind::StoneSpeckle => {
            let c = col(o, 0, gpui_kit::white(), 0.08);
            grid_dots(window, &b, (w, h), (7., 9.), (0., 0.), 0.9, c);
            grid_dots(window, &b, (w, h), (11., 6.), (3., 4.), 0.8, c.opacity(0.6));
        }
        Kind::LateriteSpeckle => {
            let c = col(o, 0, gpui_kit::red(), 0.5).opacity(0.4);
            grid_dots(window, &b, (w, h), (9., 11.), (0., 0.), 1.1, c);
            grid_dots(
                window,
                &b,
                (w, h),
                (13., 7.),
                (4., 6.),
                1.0,
                c.opacity(0.66),
            );
        }
        Kind::SaltGrain => {
            let c = col(o, 0, gpui_kit::black(), 0.35);
            let n = ((w * h) / 90.).min(4000.) as u32;
            for i in 0..n {
                let (x, y) = (noise(i * 4) * w, noise(i * 4 + 1) * h);
                let s = 0.8 + noise(i * 4 + 2) * 0.9;
                let a = 0.25 + noise(i * 4 + 3) * 0.75;
                window.paint_quad(fill(rect(&b, x, y, s, s), c.opacity(a)));
            }
        }
        Kind::Woodgrain => paint_woodgrain(o, b, window),
        _ => {}
    }
}

/// Dots on a jittered grid (capped at a few thousand per surface).
fn grid_dots(
    window: &mut Window,
    b: &Bounds<Pixels>,
    (w, h): (f32, f32),
    (sx, sy): (f32, f32),
    (ox, oy): (f32, f32),
    r: f32,
    c: Hsla,
) {
    let mut i = 0u32;
    let mut y = oy;
    while y < h {
        let mut x = ox;
        while x < w {
            let jx = (noise(i * 2) - 0.5) * sx * 0.6;
            let jy = (noise(i * 2 + 1) - 0.5) * sy * 0.6;
            dot(window, b, x + jx, y + jy, r, c);
            x += sx;
            i += 1;
            if i > 4000 {
                return;
            }
        }
        y += sy;
    }
}

/// A compass rose centred at `(cx, cy)` (relative to `b`).
fn paint_rose(o: &Ornament, b: Bounds<Pixels>, (cx, cy): (f32, f32), r: f32, window: &mut Window) {
    let red = col(o, 0, gpui_kit::red(), 0.75);
    let ink = col(o, 1, gpui_kit::black(), 0.6);
    stroke(window, 0.8, ink, |pb| {
        for rr in [r, r * 0.8] {
            pb.move_to(at(&b, cx + rr, cy));
            pb.arc_to(
                point(px(rr), px(rr)),
                px(0.),
                false,
                true,
                at(&b, cx - rr, cy),
            );
            pb.arc_to(
                point(px(rr), px(rr)),
                px(0.),
                false,
                true,
                at(&b, cx + rr, cy),
            );
        }
    });
    let k = r * 1.16;
    let s = r * 0.16;
    // The four cardinal points, then the four minor ones.
    fill_path(window, red, |pb| {
        for (dx, dy) in [(0., -1.), (0., 1.), (-1., 0.), (1., 0.)] {
            let (px_, py) = (-dy, dx);
            pb.move_to(at(&b, cx + dx * k, cy + dy * k));
            pb.line_to(at(&b, cx + px_ * s, cy + py * s));
            pb.line_to(at(&b, cx, cy));
            pb.line_to(at(&b, cx - px_ * s, cy - py * s));
            pb.close();
        }
    });
    let m = r * 0.62;
    fill_path(window, ink, |pb| {
        let d = std::f32::consts::FRAC_1_SQRT_2;
        for (dx, dy) in [(d, d), (d, -d), (-d, d), (-d, -d)] {
            let (px_, py) = (-dy, dx);
            pb.move_to(at(&b, cx + dx * m, cy + dy * m));
            pb.line_to(at(&b, cx + px_ * s * 0.7, cy + py * s * 0.7));
            pb.line_to(at(&b, cx - px_ * s * 0.7, cy - py * s * 0.7));
            pb.close();
        }
    });
}

/// Rhumb lines fanning from a rose near the pane's lower right.
fn paint_rhumb(o: &Ornament, b: Bounds<Pixels>, (cx, cy): (f32, f32), r: f32, window: &mut Window) {
    let (w, h) = wh(&b);
    let red = col(o, 0, gpui_kit::red(), 0.5).opacity(0.6);
    let ink = col(o, 1, gpui_kit::black(), 0.5).opacity(0.45);
    let len = (w + h) * 1.2;
    for (i, c) in [(0, red), (1, ink)] {
        stroke(window, 0.6, c, |pb| {
            for k in 0..32 {
                // The cardinal lines would read as rules; a chart's rhumbs fan.
                if k % 2 != i || k % 8 == 0 {
                    continue;
                }
                let a = k as f32 * std::f32::consts::TAU / 32.;
                pb.move_to(at(&b, cx, cy));
                pb.line_to(at(&b, cx + a.cos() * len, cy + a.sin() * len));
            }
        });
    }
    paint_rose(o, b, (cx, cy), r, window);
}

fn paint_shoji(o: &Ornament, b: Bounds<Pixels>, window: &mut Window) {
    let (w, h) = wh(&b);
    let c = col(o, 0, gpui_kit::black(), 1.0);
    let mut x = 0.;
    while x < w {
        window.paint_quad(fill(rect(&b, x, 0., 1., h), c));
        x += 60.;
    }
    let mut y = 0.;
    while y < h {
        window.paint_quad(fill(rect(&b, 0., y, w, 1.), c));
        y += 80.;
    }
}

// ------------------------------------------------------------------ slots

/// `titlebar.band`: a layer behind the title bar's contents.
pub fn titlebar_band(t: &Theme) -> Option<AnyElement> {
    let o = t.slot(Slot::TitlebarBand).clone();
    let p = t.palette;
    let el = match o.kind {
        Kind::None => return None,
        Kind::Gradient | Kind::IndigoDye => gradient_bg(&o, p.bar).size_full().into_any_element(),
        Kind::Lattice => painter(move |b, w, _| paint_lattice(&o, b, w)).into_any_element(),
        Kind::Horizon => {
            let c = solid(&o, 0, p.muted);
            div()
                .size_full()
                .flex()
                .items_center()
                .child(div().flex_1().h(px(1.)).bg(linear_gradient(
                    90.,
                    linear_color_stop(c.opacity(0.), 0.),
                    linear_color_stop(c, 1.),
                )))
                .child(div().flex_1().h(px(1.)).bg(linear_gradient(
                    90.,
                    linear_color_stop(c, 0.),
                    linear_color_stop(c.opacity(0.), 1.),
                )))
                .into_any_element()
        }
        Kind::Svg => {
            let c = col(&o, 0, p.bar_ink, 0.5);
            painter(move |b, w, cx| paint_svg_art(&o, b, c, w, cx)).into_any_element()
        }
        _ => return None,
    };
    Some(layer().child(el).into_any_element())
}

/// Whether the title needs a plate of the bar colour behind it (a busy band).
pub fn title_plate(t: &Theme) -> bool {
    matches!(
        t.slot(Slot::TitlebarBand).kind,
        Kind::Lattice | Kind::Horizon | Kind::Svg
    )
}

/// `sidebar.ground` + `sidebar.top` + `texture`: the layer behind the posts
/// list's rows.
pub fn sidebar_ground(t: &Theme) -> Option<AnyElement> {
    let ground = t.slot(Slot::SidebarGround).clone();
    let top = t.slot(Slot::SidebarTop).clone();
    let texture = t.slot(Slot::Texture).clone();
    if ground.is_none() && top.is_none() && texture.is_none() {
        return None;
    }
    let p = t.palette;
    let base: Option<AnyElement> = match ground.kind {
        Kind::None => None,
        Kind::Gradient | Kind::IndigoDye => {
            Some(gradient_bg(&ground, p.side).size_full().into_any_element())
        }
        Kind::Strata => {
            let o = ground.clone();
            Some(painter(move |b, w, _| paint_strata(&o, b, w)).into_any_element())
        }
        Kind::Moss => {
            let o = ground.clone();
            Some(painter(move |b, w, _| paint_moss(&o, b, w)).into_any_element())
        }
        Kind::Woodgrain => {
            let o = ground.clone();
            Some(painter(move |b, w, _| paint_woodgrain(&o, b, w)).into_any_element())
        }
        Kind::Lattice => {
            let o = ground.clone();
            Some(painter(move |b, w, _| paint_lattice(&o, b, w)).into_any_element())
        }
        Kind::Seigaiha => {
            let o = ground.clone();
            let g = (solid(&o, 1, p.side), solid(&o, 2, solid(&o, 1, p.side)));
            Some(painter(move |b, w, _| paint_seigaiha(&o, b, g, 40., w)).into_any_element())
        }
        Kind::Contours => {
            let o = ground.clone();
            let (g0, g1) = (solid(&o, 1, p.side), solid(&o, 2, solid(&o, 1, p.side)));
            let label = solid(&o, 0, p.side_muted).opacity(0.45);
            Some(
                div()
                    .size_full()
                    .relative()
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(g0, 0.),
                        linear_color_stop(g1, 1.),
                    ))
                    .child(painter(move |b, w, _| paint_contours(&o, b, w)))
                    .children(soundings_labels(
                        &[
                            (0.72, 64.),
                            (0.22, 124.),
                            (0.8, 184.),
                            (0.14, 240.),
                            (0.6, 300.),
                            (0.3, 420.),
                        ],
                        label,
                        &["12", "18", "24", "31", "37", "44"],
                    ))
                    .into_any_element(),
            )
        }
        Kind::Svg => {
            let o = ground.clone();
            let c = col(&o, 0, p.side_muted, 0.3);
            Some(painter(move |b, w, cx| paint_svg_art(&o, b, c, w, cx)).into_any_element())
        }
        _ => None,
    };
    let top_el = match top.kind {
        Kind::Roots => Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h(px(60.))
                .child(painter(move |b, w, _| paint_roots(&top, b, w)))
                .into_any_element(),
        ),
        Kind::Svg => {
            let c = col(&top, 0, p.side_muted, 0.5);
            Some(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .h(px(60.))
                    .child(painter(move |b, w, cx| paint_svg_art(&top, b, c, w, cx)))
                    .into_any_element(),
            )
        }
        _ => None,
    };
    let tex = texture_layer(&texture, p, None);
    Some(
        layer()
            .overflow_hidden()
            .children(base.map(|b| layer().child(b)))
            .children(tex)
            .children(top_el)
            .into_any_element(),
    )
}

/// Small italic depth numbers at `(x fraction, y)` positions.
fn soundings_labels(at: &[(f32, f32)], color: Hsla, labels: &[&'static str]) -> Vec<AnyElement> {
    at.iter()
        .zip(labels.iter().cycle())
        .map(|((x, y), l)| {
            div()
                .absolute()
                .left(relative(*x))
                .top(px(*y))
                .italic()
                .font_family("ETBembo")
                .text_size(px(10.))
                .text_color(color)
                .child(*l)
                .into_any_element()
        })
        .collect()
}

/// `texture` over a surface. With `gutter`, only in the left and right
/// margins of that width (the editor: never behind the text).
fn texture_layer(o: &Ornament, p: Palette, gutter: Option<f32>) -> Option<AnyElement> {
    let o = o.clone();
    match o.kind {
        Kind::None => None,
        Kind::Soundings => {
            let c = col(&o, 0, p.muted, 0.6);
            let labels = ["7", "12", "19", "23", "31", "9", "15"];
            // Only in the editor's margins: on the sidebar they'd sit among
            // the titles.
            let g = gutter?;
            Some(
                layer()
                    .children(gutter_labels(g, c, &labels, true))
                    .children(gutter_labels(g, c, &labels, false))
                    .into_any_element(),
            )
        }
        Kind::StoneSpeckle | Kind::LateriteSpeckle | Kind::SaltGrain | Kind::Woodgrain => Some(
            layer()
                .child(painter(move |b, w, _| {
                    for g in gutters(b, gutter) {
                        w.with_content_mask(Some(ContentMask { bounds: g }), |w| {
                            paint_speckle(&o, g, w)
                        });
                    }
                }))
                .into_any_element(),
        ),
        Kind::Svg => {
            let c = col(&o, 0, p.muted, 0.25);
            Some(
                layer()
                    .child(painter(move |b, w, cx| {
                        for g in gutters(b, gutter) {
                            paint_svg_art(&o, g, c, w, cx);
                        }
                    }))
                    .into_any_element(),
            )
        }
        _ => None,
    }
}

/// Depth numbers down one margin of an editor.
fn gutter_labels(g: f32, color: Hsla, labels: &[&'static str], left: bool) -> Vec<AnyElement> {
    if g < 30. {
        return Vec::new();
    }
    let seed = if left { 0 } else { 50 };
    (0..6u32)
        .map(|i| {
            let x = 8. + noise(seed + i) * (g - 26.);
            let y = 60. + i as f32 * 110. + noise(seed + i + 20) * 40.;
            let el = div()
                .absolute()
                .top(px(y))
                .italic()
                .font_family("ETBembo")
                .text_size(px(10.5))
                .text_color(color)
                .child(labels[(i as usize + seed as usize) % labels.len()]);
            if left {
                el.left(px(x)).into_any_element()
            } else {
                el.right(px(x)).into_any_element()
            }
        })
        .collect()
}

/// The rects of the margins (both sides), or the whole bounds.
fn gutters(b: Bounds<Pixels>, gutter: Option<f32>) -> Vec<Bounds<Pixels>> {
    let Some(g) = gutter else { return vec![b] };
    let (w, h) = wh(&b);
    let g = g.min(w / 2.).max(0.);
    if g < 1. {
        return Vec::new();
    }
    vec![rect(&b, 0., 0., g, h), rect(&b, w - g, 0., g, h)]
}

/// `scroll.edge`: a fade at the bottom of the posts list.
pub fn scroll_edge(t: &Theme) -> Option<AnyElement> {
    let o = t.slot(Slot::ScrollEdge);
    if o.kind != Kind::Mist {
        return None;
    }
    let c = col(o, 0, t.palette.side, 0.55);
    Some(
        div()
            .absolute()
            .bottom_0()
            .left_0()
            .w_full()
            .h(px(70.))
            .bg(linear_gradient(
                180.,
                linear_color_stop(c.opacity(0.), 0.),
                linear_color_stop(c, 1.),
            ))
            .into_any_element(),
    )
}

/// Whether rows are drawn as inset cells (then every row gets the same
/// inset, so titles line up).
pub fn row_inset(t: &Theme) -> bool {
    matches!(
        t.slot(Slot::RowSelected).kind,
        Kind::LitCell | Kind::Cushion | Kind::Outline | Kind::Lantern
    )
}

/// `row.selected`: style a list row (selected or not).
pub fn row(t: &Theme, d: Stateful<Div>, selected: bool) -> Stateful<Div> {
    let d = if row_inset(t) {
        d.px(px(8.))
    } else {
        d.px(px(14.))
    };
    if !selected {
        return d;
    }
    selected_surface(t, d, t.palette.sel)
}

/// `row.selected` for a list on the page (the Reader's posts, the stream):
/// the theme's shape on [`Palette::page_sel`], so page text reads on it.
/// No padding: the caller's row keeps its own.
pub fn page_row<E: Styled>(t: &Theme, d: E, selected: bool) -> E {
    if !selected {
        return d;
    }
    selected_surface(t, d, t.palette.page_sel())
}

/// The selected row's shape and ground (`sel`), without its padding: the
/// sidebar's rows, the Reader's sources and the lists on the page share it.
pub fn selected_surface<E: Styled>(t: &Theme, d: E, sel: Hsla) -> E {
    let o = t.slot(Slot::RowSelected);
    let p = Palette { sel, ..t.palette };
    let r = t.radius;
    match o.kind {
        Kind::LitCell => {
            let c0 = solid(o, 0, p.sel);
            let c1 = solid(o, 1, c0);
            d.bg(linear_gradient(
                180.,
                linear_color_stop(c0, 0.),
                linear_color_stop(c1, 1.),
            ))
            .rounded_tl(px(16.))
            .rounded_tr(px(16.))
            .rounded_bl(px(7.))
            .rounded_br(px(7.))
            .shadow(vec![
                BoxShadow {
                    color: c1.opacity(0.55),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(14.),
                    spread_radius: px(0.),
                    inset: false,
                },
                BoxShadow {
                    color: black().opacity(0.15),
                    offset: point(px(0.), px(-3.)),
                    blur_radius: px(0.),
                    spread_radius: px(0.),
                    inset: true,
                },
            ])
        }
        Kind::Cushion => d
            .bg(p.sel)
            .rounded_tl(px(14.))
            .rounded_tr(px(22.))
            .rounded_br(px(14.))
            .rounded_bl(px(18.))
            .shadow(vec![BoxShadow {
                color: solid(o, 0, p.accent),
                offset: point(px(0.), px(2.)),
                blur_radius: px(0.),
                spread_radius: px(0.),
                inset: false,
            }]),
        Kind::Outline => d
            .bg(p.sel)
            .rounded(px(r))
            .border_1()
            .border_color(solid(o, 0, p.line)),
        Kind::Lantern => d
            .bg(p.sel)
            .rounded(px(r))
            .border_1()
            .border_color(solid(o, 0, p.line))
            .shadow(vec![BoxShadow {
                color: solid(o, 1, p.accent).opacity(0.25),
                offset: point(px(0.), px(0.)),
                blur_radius: px(12.),
                spread_radius: px(0.),
                inset: false,
            }]),
        // An inset shadow, not a border: the row's own hairline (a list on
        // the page has one along its foot) keeps its colour.
        Kind::InsetBar => d.bg(p.sel).shadow(vec![BoxShadow {
            color: solid(o, 0, p.accent),
            offset: point(px(3.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(0.),
            inset: true,
        }]),
        _ => d.bg(p.sel),
    }
}

/// Seat a row made by [`row`] in the list: inset cells get a margin (list
/// items fill the list's width, so the margin is a wrapper's padding).
pub fn row_wrap(t: &Theme, cell: Stateful<Div>) -> AnyElement {
    if row_inset(t) {
        div().px(px(6.)).py(px(1.)).child(cell).into_any_element()
    } else {
        cell.into_any_element()
    }
}

/// `editor.frame` `harbour-glow`: the sea around the editor and the glow.
pub fn harbour(t: &Theme) -> Option<(Hsla, Hsla)> {
    let o = t.slot(Slot::EditorFrame);
    (o.kind == Kind::HarbourGlow)
        .then(|| (solid(o, 0, t.palette.side), solid(o, 1, t.palette.accent)))
}

/// `editor.frame` (grid, rhumb lines, rose, your SVG) and `texture`, in the
/// editor's margins of width `gutter` only.
pub fn editor_margins(t: &Theme, gutter: f32) -> Option<AnyElement> {
    let frame = t.slot(Slot::EditorFrame).clone();
    let texture = t.slot(Slot::Texture).clone();
    let p = t.palette;
    let frame_el = match frame.kind {
        Kind::ShojiGrid | Kind::RhumbLines | Kind::CompassRose | Kind::Svg => {
            let c = col(&frame, 0, p.muted, 0.35);
            Some(
                layer()
                    .child(painter(move |b, w, cx| {
                        for (side, g) in gutters(b, Some(gutter)).into_iter().enumerate() {
                            w.with_content_mask(Some(ContentMask { bounds: g }), |w| {
                                match frame.kind {
                                    Kind::ShojiGrid => paint_shoji(&frame, b, w),
                                    Kind::RhumbLines => {
                                        let (bw, bh) = wh(&b);
                                        let r = (gutter / 2. - 6.).clamp(10., 34.);
                                        let c = if side == 0 {
                                            (gutter / 2., 150.)
                                        } else {
                                            (bw - gutter / 2., bh - 90.)
                                        };
                                        paint_rhumb(&frame, b, c, r, w)
                                    }
                                    Kind::CompassRose => {
                                        let (bw, bh) = wh(&b);
                                        paint_rose(&frame, b, (bw - gutter / 2., bh - 70.), 30., w)
                                    }
                                    _ => {
                                        let (bw, bh) = wh(&b);
                                        let s = (gutter - 16.).clamp(0., 160.);
                                        let r = rect(&b, bw - gutter + 8., bh - s - 16., s, s);
                                        paint_svg_art(&frame, r, c, w, cx);
                                    }
                                }
                            });
                        }
                    }))
                    .into_any_element(),
            )
        }
        _ => None,
    };
    let tex = texture_layer(&texture, p, Some(gutter));
    if frame_el.is_none() && tex.is_none() {
        return None;
    }
    Some(layer().children(frame_el).children(tex).into_any_element())
}

/// `divider`: the band under the editor and between stream posts, and its
/// height (`None`: a plain hairline does the job).
pub fn divider(t: &Theme) -> Option<AnyElement> {
    let o = t.slot(Slot::Divider).clone();
    let p = t.palette;
    let (h, el): (f32, AnyElement) = match o.kind {
        Kind::None | Kind::Line => return None,
        Kind::Kintsugi => {
            let c = col(&o, 0, p.amber, 1.0);
            (
                10.,
                painter(move |b, w, _| {
                    let (bw, _) = wh(&b);
                    stroke(w, 1.6, c, |pb| {
                        let mut x = 0.;
                        let mut i = 0u32;
                        pb.move_to(at(&b, 0., 5.));
                        while x < bw {
                            x += 12. + noise(i) * 30.;
                            pb.line_to(at(&b, x, 2. + noise(i + 300) * 6.));
                            i += 1;
                        }
                    });
                    // A few hairline branches off the main seam.
                    stroke(w, 0.8, c.opacity(0.7), |pb| {
                        let mut x = 60.;
                        let mut i = 0u32;
                        while x < bw {
                            pb.move_to(at(&b, x, 5.));
                            pb.line_to(at(&b, x + 6. + noise(i) * 8., 1. + noise(i + 7) * 2.));
                            x += 140. + noise(i + 11) * 120.;
                            i += 1;
                        }
                    });
                })
                .into_any_element(),
            )
        }
        Kind::Roots => {
            let c = col(&o, 0, p.line, 1.0);
            (
                12.,
                painter(move |b, w, _| {
                    let (bw, _) = wh(&b);
                    stroke(w, 1., c, |pb| {
                        pb.move_to(at(&b, 0., 1.));
                        pb.line_to(at(&b, bw, 1.));
                        let mut x = 30.;
                        let mut i = 0u32;
                        while x < bw {
                            let d = 5. + noise(i) * 6.;
                            pb.move_to(at(&b, x, 1.));
                            pb.curve_to(
                                at(&b, x + 4. - noise(i + 3) * 8., 1. + d),
                                at(&b, x + 3., 1. + d * 0.5),
                            );
                            x += 50. + noise(i + 9) * 60.;
                            i += 1;
                        }
                    });
                })
                .into_any_element(),
            )
        }
        Kind::TideLine => {
            let c0 = col(&o, 0, p.line, 1.0);
            let c1 = col(&o, 1, c0.opacity(0.6), 1.0);
            (
                14.,
                painter(move |b, w, _| {
                    let (bw, _) = wh(&b);
                    for (c, y, a, wd, seed) in [(c0, 6., 3., 1.2, 0u32), (c1, 10., 2., 1.0, 40)] {
                        stroke(w, wd, c, |pb| {
                            pb.move_to(at(&b, 0., y));
                            let mut x = 0.;
                            let mut i = seed;
                            while x < bw {
                                let seg = 40. + noise(i) * 30.;
                                pb.curve_to(
                                    at(&b, x + seg, y + (noise(i + 1) - 0.5) * a),
                                    at(&b, x + seg / 2., y + (noise(i + 2) - 0.5) * a * 2.),
                                );
                                x += seg;
                                i += 3;
                            }
                        });
                    }
                })
                .into_any_element(),
            )
        }
        Kind::Seigaiha => {
            let paper = p.editor;
            (
                18.,
                painter(move |b, w, _| {
                    let o = Ornament {
                        opacity: Some(o.opacity.unwrap_or(0.9)),
                        ..o
                    };
                    let (bw, _) = wh(&b);
                    let line = col(&o, 0, p.ink, 0.9);
                    let r = 12.;
                    // The back row first, then the front row over it.
                    for (y, off) in [(11., r), (18., 0.)] {
                        let xs: Vec<f32> = (0..)
                            .map(|k| off + k as f32 * 2. * r)
                            .take_while(|x| *x < bw + r)
                            .collect();
                        fill_path(w, paper, |pb| {
                            for &x in &xs {
                                pb.move_to(at(&b, x - r, y));
                                pb.arc_to(
                                    point(px(r), px(r)),
                                    px(0.),
                                    false,
                                    true,
                                    at(&b, x + r, y),
                                );
                                pb.close();
                            }
                        });
                        stroke(w, 1., line, |pb| {
                            for &x in &xs {
                                for rr in [r - 0.5, r * 0.66, r * 0.33] {
                                    pb.move_to(at(&b, x - rr, y));
                                    pb.arc_to(
                                        point(px(rr), px(rr)),
                                        px(0.),
                                        false,
                                        true,
                                        at(&b, x + rr, y),
                                    );
                                }
                            }
                        });
                    }
                })
                .into_any_element(),
            )
        }
        Kind::Sashiko => {
            let c = col(&o, 0, p.ink, 0.8);
            (
                6.,
                painter(move |b, w, _| {
                    let (bw, _) = wh(&b);
                    let mut pb = PathBuilder::stroke(px(1.5)).dash_array(&[px(6.), px(4.)]);
                    pb.move_to(at(&b, 0., 3.));
                    pb.line_to(at(&b, bw, 3.));
                    if let Ok(path) = pb.build() {
                        w.paint_path(path, c);
                    }
                })
                .into_any_element(),
            )
        }
        Kind::Horizon => {
            let c = solid(&o, 0, p.muted);
            (
                1.,
                div()
                    .size_full()
                    .flex()
                    .child(div().flex_1().bg(linear_gradient(
                        90.,
                        linear_color_stop(c.opacity(0.), 0.),
                        linear_color_stop(c, 1.),
                    )))
                    .child(div().flex_1().bg(linear_gradient(
                        90.,
                        linear_color_stop(c, 0.),
                        linear_color_stop(c.opacity(0.), 1.),
                    )))
                    .into_any_element(),
            )
        }
        Kind::GlyphDivider => {
            let c = col(&o, 0, p.muted, 0.8);
            let g = o.text.clone().unwrap_or_else(|| "≈".into());
            (
                16.,
                div()
                    .size_full()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_family("Menlo")
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .text_color(c)
                    .child(g.repeat(400 / g.chars().count().max(1)))
                    .into_any_element(),
            )
        }
        Kind::Svg => {
            let c = col(&o, 0, p.muted, 0.6);
            (
                16.,
                painter(move |b, w, cx| paint_svg_art(&o, b, c, w, cx)).into_any_element(),
            )
        }
        _ => return None,
    };
    Some(
        div()
            .flex_none()
            .w_full()
            .h(px(h))
            .overflow_hidden()
            .child(el)
            .into_any_element(),
    )
}

/// What the status ornaments can show.
pub struct StatusData {
    /// Posts written per day, oldest first (the tide sparkline).
    pub activity: Vec<u32>,
    /// The selected post's depth in the list (1 = the top).
    pub depth: Option<usize>,
}

/// `status.ornament`.
pub fn status(t: &Theme, data: StatusData, chrome_font: &'static str) -> Option<AnyElement> {
    let o = t.slot(Slot::StatusOrnament).clone();
    let p = t.palette;
    match o.kind {
        Kind::TideSparkline => {
            let line = col(&o, 0, p.accent, 1.0);
            let buoy = solid(&o, 1, p.bg);
            let values = data.activity;
            Some(
                div()
                    .id("tide-sparkline")
                    .w(px(120.))
                    .h(px(14.))
                    .flex_none()
                    .child(painter(move |b, w, _| {
                        let (bw, bh) = wh(&b);
                        let max = values.iter().copied().max().unwrap_or(0).max(1) as f32;
                        let n = values.len().max(2);
                        let pts: Vec<(f32, f32)> = (0..n)
                            .map(|i| {
                                let v = values.get(i).copied().unwrap_or(0) as f32 / max;
                                let x = 2. + (bw - 4.) * i as f32 / (n - 1) as f32;
                                (x, bh - 2. - v * (bh - 4.))
                            })
                            .collect();
                        stroke(w, 1.2, line, |pb| {
                            pb.move_to(at(&b, pts[0].0, pts[0].1));
                            for win in pts.windows(2) {
                                let (a, c) = (win[0], win[1]);
                                let mid = ((a.0 + c.0) / 2., (a.1 + c.1) / 2.);
                                pb.curve_to(at(&b, mid.0, mid.1), at(&b, a.0, a.1));
                            }
                            let last = pts[pts.len() - 1];
                            pb.line_to(at(&b, last.0, last.1));
                        });
                        let last = pts[pts.len() - 1];
                        dot(w, &b, last.0, last.1, 2., buoy);
                    }))
                    .into_any_element(),
            )
        }
        Kind::ZLevel => {
            let c = solid(&o, 0, p.accent);
            let z = data.depth.unwrap_or(0);
            Some(
                div()
                    .id("z-level")
                    .font_family(chrome_font)
                    .text_color(c)
                    .child(if z == 0 {
                        "z 0  ▲".to_string()
                    } else {
                        format!("z −{z}  ▼")
                    })
                    .into_any_element(),
            )
        }
        _ => None,
    }
}

/// `marker.pinned` / `marker.new`: the mark in a list row. `None` from a
/// `none` slot; the caller draws its default when the slot is unset.
pub fn marker(t: &Theme, slot: Slot, color: Hsla, chrome_font: &'static str) -> Option<AnyElement> {
    let o = t.slot(slot);
    match o.kind {
        Kind::Glyph => Some(
            div()
                .font_family(chrome_font)
                .text_color(solid(o, 0, color))
                .child(o.text.clone().unwrap_or_default())
                .into_any_element(),
        ),
        Kind::Dot => Some(
            div()
                .size(px(6.))
                .rounded_full()
                .bg(solid(o, 0, color))
                .into_any_element(),
        ),
        _ => None,
    }
}

/// `empty.art`: a small picture above an empty list's message (on the
/// sidebar).
pub fn empty_art(t: &Theme) -> Option<AnyElement> {
    empty_art_on(t, t.palette.side)
}

/// `empty.art` on `ground` (the page, for the reading side's empty states):
/// waves are drawn on it rather than on a tile of the sidebar.
pub fn empty_art_on(t: &Theme, ground: Hsla) -> Option<AnyElement> {
    let o = t.slot(Slot::EmptyArt).clone();
    let p = t.palette;
    let s = 72.;
    let el: AnyElement = match o.kind {
        Kind::None => return None,
        Kind::Glyph => div()
            .text_size(px(40.))
            .font_family("Menlo")
            .text_color(solid(&o, 0, p.side_accent))
            .child(o.text.clone().unwrap_or_default())
            .into_any_element(),
        Kind::LitCell => {
            let (c0, c1) = (solid(&o, 0, p.sel), solid(&o, 1, p.accent));
            let soil = p.side_muted.opacity(0.5);
            painter(move |b, w, _| {
                // A room dug into the soil: an arch with the lamp on.
                fill_path(w, c1.opacity(0.35), |pb| arch(pb, &b, 6., 10., 60., 54.));
                fill_path(w, c0, |pb| arch(pb, &b, 12., 18., 48., 42.));
                stroke(w, 1., soil, |pb| {
                    pb.move_to(at(&b, 0., 66.));
                    pb.line_to(at(&b, 72., 66.));
                });
                dot(w, &b, 36., 40., 4., c1);
            })
            .into_any_element()
        }
        Kind::CompassRose => {
            painter(move |b, w, _| paint_rose(&o, b, (36., 36.), 28., w)).into_any_element()
        }
        Kind::Moss => painter(move |b, w, _| {
            let c = col(&o, 0, p.green, 0.6);
            for (x, y, r) in [(22., 46., 16.), (44., 40., 20.), (34., 54., 12.)] {
                dot(w, &b, x, y, r, c.opacity(0.5));
                dot(w, &b, x, y, r * 0.7, c.opacity(0.7));
            }
        })
        .into_any_element(),
        Kind::Seigaiha => {
            let paper = ground;
            painter(move |b, w, _| {
                paint_seigaiha(
                    &Ornament {
                        opacity: Some(o.opacity.unwrap_or(0.8)),
                        ..o
                    },
                    b,
                    (paper, paper),
                    24.,
                    w,
                )
            })
            .into_any_element()
        }
        Kind::Lattice => painter(move |b, w, _| {
            paint_lattice(
                &Ornament {
                    opacity: Some(o.opacity.unwrap_or(0.6)),
                    ..o
                },
                b,
                w,
            )
        })
        .into_any_element(),
        Kind::Contours => painter(move |b, w, _| paint_contours(&o, b, w)).into_any_element(),
        Kind::Svg => {
            let c = col(&o, 0, p.side_muted, 0.8);
            painter(move |b, w, cx| paint_svg_art(&o, b, c, w, cx)).into_any_element()
        }
        _ => return None,
    };
    Some(
        div()
            .id("empty-art")
            .size(px(s))
            .overflow_hidden()
            .rounded(px(t.radius))
            .flex()
            .items_center()
            .justify_center()
            .child(el)
            .into_any_element(),
    )
}

/// An arch (rounded top, flat bottom) at `(x, y)` of `w × h`.
fn arch(pb: &mut PathBuilder, b: &Bounds<Pixels>, x: f32, y: f32, w: f32, h: f32) {
    let r = w / 2.;
    pb.move_to(at(b, x, y + h));
    pb.line_to(at(b, x, y + r));
    pb.arc_to(
        point(px(r), px(r)),
        px(0.),
        false,
        true,
        at(b, x + w, y + r),
    );
    pb.line_to(at(b, x + w, y + h));
    pb.close();
}

/// `quote.frame`: frame a stream quote box. `none` keeps the classic box.
pub fn quote_frame(t: &Theme, d: Stateful<Div>) -> AnyElement {
    let o = t.slot(Slot::QuoteFrame);
    let p = t.palette;
    let rule = solid(o, 0, p.quote_rule);
    let d = d.px(px(12.)).py(px(8.));
    match o.kind {
        Kind::None => d
            .rounded(px(6.))
            .bg(p.muted.opacity(0.09))
            .border_l_2()
            .border_color(p.muted.opacity(0.5))
            .into_any_element(),
        Kind::Rule => d
            .rounded(px(t.radius.min(6.)))
            .bg(p.quote_bg)
            .border_l_2()
            .border_color(rule)
            .into_any_element(),
        Kind::Arch => d
            .bg(p.quote_bg)
            .rounded_tl(px(14.))
            .rounded_tr(px(14.))
            .rounded_bl(px(6.))
            .rounded_br(px(6.))
            .border_1()
            .border_color(rule)
            .into_any_element(),
        Kind::Cushion => d
            .bg(p.quote_bg)
            .rounded_tl(px(22.))
            .rounded_tr(px(12.))
            .rounded_br(px(18.))
            .rounded_bl(px(10.))
            .border_2()
            .border_color(rule)
            .shadow(vec![BoxShadow {
                color: rule.opacity(0.35),
                offset: point(px(0.), px(4.)),
                blur_radius: px(10.),
                spread_radius: px(-4.),
                inset: true,
            }])
            .into_any_element(),
        Kind::Outline => d
            .bg(p.quote_bg)
            .rounded(px(t.radius))
            .border_1()
            .border_color(rule)
            .into_any_element(),
        Kind::InsetBar => d
            .bg(p.quote_bg)
            .border_l(px(3.))
            .border_color(rule)
            .into_any_element(),
        Kind::Buoy => d
            .bg(p.quote_bg)
            .rounded(px(1.))
            .border_t_2()
            .border_color(rule)
            .shadow(vec![
                BoxShadow {
                    color: p.line,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(1.),
                    inset: false,
                },
                BoxShadow {
                    color: p.line.opacity(0.6),
                    offset: point(px(2.), px(2.)),
                    blur_radius: px(0.),
                    spread_radius: px(1.),
                    inset: false,
                },
            ])
            .into_any_element(),
        Kind::DoubleRule => div()
            .p(px(3.))
            .border_1()
            .border_color(rule)
            .child(d.bg(p.quote_bg).border_1().border_color(rule))
            .into_any_element(),
        Kind::Sashiko => div()
            .p(px(3.))
            .rounded(px(3.))
            .bg(p.quote_bg)
            .child(d.border_1().border_dashed().border_color(rule))
            .into_any_element(),
        Kind::BoxCorners => {
            let corner = |g: &'static str| {
                div()
                    .absolute()
                    .font_family("Menlo")
                    .text_size(px(12.))
                    .line_height(px(12.))
                    .text_color(rule)
                    .child(g)
            };
            div()
                .relative()
                .child(
                    d.bg(p.quote_bg)
                        .rounded(px(2.))
                        .border_1()
                        .border_color(p.quote_rule),
                )
                .child(corner("╔").top(px(-6.)).left(px(-4.)))
                .child(corner("╝").bottom(px(-6.)).right(px(-4.)))
                .into_any_element()
        }
        _ => d
            .rounded(px(6.))
            .bg(p.quote_bg)
            .border_l_2()
            .border_color(rule)
            .into_any_element(),
    }
}

/// Apply a theme border style (`solid`, `dashed`, `double`, `none`). A
/// `double` border is the inner rule here; its outer rule is a ring of
/// shadows ([`border_rings`]) that the caller puts in its `shadow` list
/// (GPUI keeps one list, and sheets and toasts have a drop shadow too).
pub fn border<E: Styled>(e: E, style: Border, color: Hsla) -> E {
    match style {
        Border::None => e,
        Border::Solid | Border::Double => e.border_1().border_color(color),
        Border::Dashed => e.border_1().border_dashed().border_color(color),
    }
}

/// The shadows a border style adds outside the element: for `double`, a
/// second thin rule 2px out, the gap in `ground` (a chart's double rule).
/// Painted in order, so put the drop shadow first and these after it.
pub fn border_rings(style: Border, color: Hsla, ground: Hsla) -> Vec<BoxShadow> {
    if style != Border::Double {
        return Vec::new();
    }
    let ring = |spread: f32, color: Hsla| BoxShadow {
        color,
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(spread),
        inset: false,
    };
    vec![ring(3., color), ring(2., ground)]
}

/// [`border`] with its rings and a drop shadow: the whole edge of a sheet
/// or a toast on `ground`.
pub fn border_with_shadow<E: Styled>(
    e: E,
    style: Border,
    color: Hsla,
    ground: Hsla,
    drop: Option<BoxShadow>,
) -> E {
    let mut shadows: Vec<BoxShadow> = drop.into_iter().collect();
    shadows.extend(border_rings(style, color, ground));
    border(e, style, color).shadow(shadows)
}

/// A hairline that stands out at least 1.5:1 from `ground`: toward black
/// on a light ground, toward white on a dark one.
pub fn edge_on(ground: Hsla) -> Hsla {
    let toward =
        if crate::theme::contrast(ground, black()) > crate::theme::contrast(ground, white()) {
            black()
        } else {
            white()
        };
    let mut k = 0.15;
    let mut c = crate::theme::mix(ground, toward, k);
    while crate::theme::contrast(c, ground) < 1.5 && k < 1.0 {
        k += 0.05;
        c = crate::theme::mix(ground, toward, k);
    }
    c
}

/// A small swatch of a theme for Settings (five colour chips in a row) on
/// `ground`, the sheet it sits on: its edge reads there, dark or light.
pub fn swatch_on(t: &Theme, ground: Hsla) -> AnyElement {
    div()
        .flex()
        .rounded(px(3.))
        .overflow_hidden()
        .border_1()
        .border_color(edge_on(ground))
        .children(
            t.swatch()
                .into_iter()
                .map(|c| div().w(px(7.)).h(px(12.)).bg(c)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{
        divider, editor_margins, gutters, harbour, noise, row_inset, sidebar_ground, titlebar_band,
    };
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn noise_is_stable_and_in_range() {
        for i in 0..1000 {
            let n = noise(i);
            assert!((0.0..1.0).contains(&n));
            assert_eq!(n, noise(i));
        }
        assert_ne!(noise(1), noise(2));
    }

    #[test]
    fn plain_themes_have_no_ornaments() {
        let t = crate::theme::builtins();
        for id in ["light", "dark"] {
            let th = t.get(id);
            assert!(titlebar_band(&th).is_none());
            assert!(sidebar_ground(&th).is_none());
            assert!(divider(&th).is_none());
            assert!(editor_margins(&th, 100.).is_none());
            assert!(harbour(&th).is_none());
            assert!(!row_inset(&th));
        }
        assert!(harbour(&t.get("konkan")).is_some());
        assert!(row_inset(&t.get("cutaway")));
        assert!(divider(&t.get("kumiko")).is_some());
    }

    #[test]
    fn a_double_border_is_two_thin_rules_with_a_gap() {
        use blyg_core::config::theme::Border;
        let (line, ground) = (gpui_kit::black(), gpui_kit::white());
        let rings = super::border_rings(Border::Double, line, ground);
        assert_eq!(rings.len(), 2);
        // The outer rule, then the gap painted over its inner part.
        assert_eq!((rings[0].spread_radius, rings[0].color), (px(3.), line));
        assert_eq!((rings[1].spread_radius, rings[1].color), (px(2.), ground));
        assert!(super::border_rings(Border::Solid, line, ground).is_empty());
    }

    #[test]
    fn margins_split_into_two_gutters() {
        let b = Bounds {
            origin: point(px(10.), px(0.)),
            size: size(px(800.), px(600.)),
        };
        let g = gutters(b, Some(120.));
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].origin.x, px(10.));
        assert_eq!(g[1].origin.x, px(690.));
        assert_eq!(g[1].size.width, px(120.));
        assert!(gutters(b, Some(0.)).is_empty(), "no margin: nothing drawn");
        assert_eq!(gutters(b, None), vec![b]);
    }
}
