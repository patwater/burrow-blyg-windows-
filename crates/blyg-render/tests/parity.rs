//! Parity with the reference Worker: every fixture in `tests/fixtures/parity`
//! was produced by the Worker's own TypeScript renderer (`gen_parity.mjs`).
//! HTML is compared after collapsing whitespace between tags.

mod common;

use blyg_render::{Attachment, Kind, RenderOpts, media_html, preview_media, render_preview};
use common::{FakeStore, fixtures_dir, normalize};
use serde_json::Value;

/// Fixtures whose output is known to differ, with the reason. Keep this
/// list honest and short; `docs/RENDER.md` explains each entry.
const KNOWN_DIVERGENCES: &[(&str, &str)] = &[];

fn load_fixtures() -> Vec<Value> {
    let dir = fixtures_dir().join("parity");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("parity dir")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter(|p| !p.file_name().unwrap().to_string_lossy().starts_with('_'))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap())
        .collect()
}

#[test]
fn parity_with_worker() {
    let store = FakeStore::load();
    let opts = RenderOpts {
        data_line: false,
        provenance: true,
        mount: store.mount(),
        self_id: Some(store.self_id()),
    };
    let fixtures = load_fixtures();
    assert!(
        fixtures.len() >= 80,
        "expected the full corpus, found {}",
        fixtures.len()
    );

    let mut exact = 0;
    let mut normalized = 0;
    let mut failures = Vec::new();
    let mut unexpected_passes = Vec::new();
    for f in &fixtures {
        let name = f["name"].as_str().unwrap();
        let kind = if f["kind"] == "thread" {
            Kind::Thread
        } else {
            Kind::Fragment
        };
        let md = f["md"].as_str().unwrap();
        let expected = f["html"].as_str().unwrap();
        let got = render_preview(md, kind, &store, &opts);
        let known = KNOWN_DIVERGENCES.iter().any(|(n, _)| *n == name);
        if got.html == expected {
            exact += 1;
        }
        if normalize(&got.html) == normalize(expected) {
            normalized += 1;
            if known {
                unexpected_passes.push(name.to_string());
            }
        } else if !known {
            failures.push(format!(
                "--- {name}\n  md:       {md:?}\n  expected: {}\n  got:      {}",
                normalize(expected),
                normalize(&got.html)
            ));
        }

        // Unresolved reasons and quotes must agree with the Worker too.
        if kind == Kind::Thread {
            let reasons: Vec<String> = f["errors"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["reason"].as_str().unwrap().to_string())
                .collect();
            let got_reasons: Vec<String> = got
                .stats
                .unresolved
                .iter()
                .map(|u| u.reason.to_string())
                .collect();
            assert_eq!(got_reasons, reasons, "{name}: unresolved reasons");
            let ids: Vec<String> = f["transclusions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["id"].as_str().unwrap().to_string())
                .collect();
            let got_ids: Vec<String> = got
                .stats
                .transclusions
                .iter()
                .map(|t| t.id.clone())
                .collect();
            assert_eq!(got_ids, ids, "{name}: transclusions");
        }
        let tk_errors = f["tk"]["errors"].as_array().unwrap().len();
        assert_eq!(got.stats.tk_errors.len(), tk_errors, "{name}: TK errors");
    }
    eprintln!(
        "parity: {normalized}/{} match after whitespace normalisation ({exact} byte-identical); {} known divergences",
        fixtures.len(),
        KNOWN_DIVERGENCES.len()
    );
    assert!(
        unexpected_passes.is_empty(),
        "now passing, remove from KNOWN_DIVERGENCES: {unexpected_passes:?}"
    );
    assert!(
        failures.is_empty(),
        "{} parity failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Attachments (the Worker's patch 8): `mediaHtml`, which public pages put
/// after the content, and `previewMedia`, the studio preview's strip.
#[test]
fn media_parity_with_worker() {
    let text = std::fs::read_to_string(fixtures_dir().join("parity/_media.json")).unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    let mount = doc["mount"].as_str().unwrap();
    let cases = doc["cases"].as_array().unwrap();
    assert!(cases.len() >= 3);
    for c in cases {
        let media: Vec<Attachment> = c["media"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| Attachment {
                key: m["r2_key"].as_str().unwrap().to_string(),
                alt: m["alt"].as_str().map(str::to_string),
            })
            .collect();
        let name = &c["name"];
        assert_eq!(media_html(&media, mount), c["media_html"], "{name}");
        assert_eq!(preview_media(&media, mount), c["preview_media"], "{name}");
    }
}
