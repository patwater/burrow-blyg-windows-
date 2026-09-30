//! Performance budget: a 20,000-character thread renders in under 10 ms in
//! a release build (the studio re-renders ~100 ms after typing pauses).
//!
//!     cargo test -p blyg-render --release --test bench -- --nocapture
//!
//! Debug builds report the timing but only enforce a loose sanity bound.

mod common;

use blyg_render::{Kind, RenderOpts, render_preview};
use common::FakeStore;
use std::time::{Duration, Instant};

/// A realistic mixed document of at least `target` characters.
fn document(target: usize) -> String {
    let section = "## A section about small tools\n\n\
Small tools, used daily, beat big tools used rarely. See https://blyg.example.com/notes/tools \
or www.example.org/guide for more, and write to someone@example.com if you disagree. \
Some *emphasis*, some **strong text**, a `code span`, and a [titled link](https://example.org/a \"A\").\n\n\
- first item with example.com in it\n- second item with a [link](/local)\n  - nested item\n\n\
> A plain quote that runs\n> over two lines.\n\n\
The weather was [TK]describe the weather[=]*grey and still*[/TK] that morning.\n\n\
[TK]write a bridging paragraph[=]Which brings us to the point: the tool should disappear.[/TK]\n\n\
![a picture](https://images.example.org/p.jpg)\n\n\
![[01j9zq3k4m5n6p7q8r9s0t1v2w]]\n\n\
```rust\nfn main() { println!(\"hello\"); }\n```\n\n\
| a | b |\n|---|---|\n| 1 | 2 |\n\n\
https://www.youtube.com/watch?v=Zx9_abc-123\n\n\
![[03zzzzzzzzzzzzzzzzzzzzzzzz]]\n\n\
A closing paragraph with a trailing link https://example.org/end.\n\n";
    let mut doc = String::from("# Notes\n\n");
    while doc.chars().count() < target {
        doc.push_str(section);
    }
    doc
}

#[test]
fn renders_20k_chars_under_10ms() {
    let store = FakeStore::load();
    let opts = RenderOpts {
        self_id: Some(store.self_id()),
        ..RenderOpts::default()
    };
    let doc = document(20_000);
    assert!(doc.chars().count() >= 20_000);

    // Warm up (regex compilation, lazy statics).
    let first = render_preview(&doc, Kind::Thread, &store, &opts);
    assert!(
        first.stats.quotes > 10 && first.stats.unresolved.len() > 10 && first.stats.videos > 10
    );

    let runs = 20;
    let mut times: Vec<Duration> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            let out = render_preview(&doc, Kind::Thread, &store, &opts);
            let e = t.elapsed();
            assert_eq!(out.html.len(), first.html.len());
            e
        })
        .collect();
    times.sort();
    let median = times[runs / 2];
    let worst = times[runs - 1];
    eprintln!(
        "render_preview: {} chars, median {:.2} ms, worst {:.2} ms ({} build)",
        doc.chars().count(),
        median.as_secs_f64() * 1e3,
        worst.as_secs_f64() * 1e3,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    if cfg!(debug_assertions) {
        assert!(
            median < Duration::from_millis(200),
            "debug render too slow: {median:?}"
        );
    } else {
        assert!(
            median < Duration::from_millis(10),
            "release render over budget: {median:?}"
        );
    }
}

/// Inputs aimed at the linkify regexes' backtracking. None may be pathological.
#[test]
fn adversarial_inputs_stay_fast() {
    let cases: Vec<(&str, String)> = vec![
        ("dotted words", "foo.bar.baz.qux ".repeat(1300)),
        ("one long label", format!("{}.com", "a".repeat(20_000))),
        ("hyphen run", format!("x{}y.org", "-".repeat(20_000))),
        ("many schemes", "http:// ".repeat(2500)),
        ("many ats", "a@b@c@d@ ".repeat(2200)),
        (
            "path punctuation",
            format!("https://example.org/{}", "a.,;!?".repeat(3300)),
        ),
        (
            "unclosed parens",
            format!("https://example.org/{}", "(".repeat(20_000)),
        ),
        ("www chain", "www.".repeat(5000)),
        ("emphasis soup", "*_".repeat(10_000)),
        ("brackets", "[".repeat(20_000)),
        ("tk tokens", "[TK]a[=]b[/TK] ".repeat(1400)),
    ];
    for (name, doc) in cases {
        let t = Instant::now();
        let out = render_preview(
            &doc,
            Kind::Thread,
            &blyg_render::NoResolver,
            &RenderOpts::default(),
        );
        let e = t.elapsed();
        eprintln!(
            "{name}: {} bytes in {:.2} ms",
            doc.len(),
            e.as_secs_f64() * 1e3
        );
        assert!(!out.html.is_empty());
        let budget = if cfg!(debug_assertions) { 3000 } else { 250 };
        assert!(e < Duration::from_millis(budget), "{name} took {e:?}");
    }
}
