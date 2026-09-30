//! Scratch notes (docs/SPEC.md § Scratch notes): local-only items that never
//! reach the network until they're promoted, against the in-process mock.

mod common;

use std::time::Duration;

use blyg_core::*;
use common::*;

const T: Duration = Duration::from_secs(5);

/// `POST /api/items` exactly (not `…/publish`).
fn creates(env: &Env) -> usize {
    env.mock
        .state()
        .log
        .iter()
        .filter(|l| l.as_str() == "POST /api/items")
        .count()
}

#[test]
fn a_scratch_note_never_hits_the_network() {
    let env = Env::new();
    let b = env.manual();

    let id = b
        .create_scratch(Kind::Fragment, "a half-thought about tide tables")
        .unwrap();
    b.save(&id, "a half-thought about tide tables, and moons")
        .unwrap();
    b.set_kind(&id, Kind::Thread).unwrap();
    b.set_kind(&id, Kind::Fragment).unwrap();

    let it = b.item(&id).unwrap();
    assert_eq!(it.status, Status::Scratch);
    assert_eq!(it.content_md, "a half-thought about tide tables, and moons");
    assert!(it.server_id.is_none());
    assert!(!it.pending_sync, "nothing is queued for a scratch note");
    assert_eq!(b.sync_status(), SyncStatus::Synced);

    // Searchable (trigram FTS and the short-query scan), listed.
    assert_eq!(b.search("tide tables").len(), 1);
    assert_eq!(b.search("mo").len(), 1);
    assert_eq!(b.items().len(), 1);

    assert!(
        env.mock.state().log.is_empty(),
        "requests: {:?}",
        env.mock.state().log
    );

    // A full sync (flush + pull) sends nothing about it and keeps it.
    b.sync_now().unwrap();
    let st = env.mock.state();
    assert_eq!(st.count("POST"), 0, "{:?}", st.log);
    assert_eq!(st.count("PUT"), 0, "{:?}", st.log);
    assert!(st.items.is_empty());
    drop(st);
    let it = b.item(&id).expect("the pull didn't drop it");
    assert_eq!(it.status, Status::Scratch);

    // Deleting it is local too.
    let before = env.mock.state().log.len();
    b.delete_draft(&id).unwrap();
    assert!(b.item(&id).is_none());
    assert_eq!(env.mock.state().log.len(), before);
}

#[test]
fn the_sync_worker_ignores_scratch_notes() {
    let env = Env::new();
    let b = env.open(fast());
    let id = b
        .create_scratch(Kind::Fragment, "only on this Mac")
        .unwrap();
    b.save(&id, "only on this Mac, edited").unwrap();
    // Let the worker run a few debounces and pulls.
    std::thread::sleep(Duration::from_millis(800));
    let st = env.mock.state();
    assert_eq!(st.count("POST"), 0, "{:?}", st.log);
    assert_eq!(st.count("PUT"), 0, "{:?}", st.log);
    assert!(st.items.is_empty());
    drop(st);
    assert_eq!(b.item(&id).unwrap().status, Status::Scratch);
}

#[test]
fn promotion_keeps_the_id() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_scratch(Kind::Fragment, "harbour notes").unwrap();

    let p = b.promote(&id, Promote::Draft).unwrap();
    assert_eq!(p.kind, Kind::Fragment);
    assert!(p.published.is_none());
    let it = b.item(&id).unwrap();
    assert_eq!(it.status, Status::Draft);
    assert!(it.pending_sync, "the create is queued");

    b.sync_now().unwrap();
    let it = b.item(&id).unwrap();
    let sid = it.server_id.clone().expect("created on the server");
    assert_eq!(env.mock.state().items[&sid.0].content_md, "harbour notes");
    assert_eq!(b.items().len(), 1, "no duplicate");
    assert_eq!(creates(&env), 1);

    // Promoting again is a no-op; publishing through promote publishes.
    b.promote(&id, Promote::Draft).unwrap();
    let p = b
        .promote(
            &id,
            Promote::Publish {
                note: Some("first".into()),
            },
        )
        .unwrap();
    assert_eq!(p.published.unwrap().version, 1);
    let it = b.item(&id).unwrap();
    assert_eq!(it.status, Status::Public);
    assert_eq!(it.server_id, Some(sid));
    assert_eq!(b.items().len(), 1);
    assert_eq!(creates(&env), 1);
}

#[test]
fn promote_publish_creates_then_publishes_and_picks_the_kind() {
    let env = Env::new();
    let b = env.manual();
    let long = "tide ".repeat(250); // 1250 chars: too long for a fragment
    let id = b.create_scratch(Kind::Fragment, &long).unwrap();
    let p = b.promote(&id, Promote::Publish { note: None }).unwrap();
    assert_eq!(p.kind, Kind::Thread, "over 1000 becomes a thread");
    assert_eq!(p.published.unwrap().version, 1);
    let it = b.item(&id).unwrap();
    assert_eq!(it.kind, Kind::Thread);
    assert_eq!(it.status, Status::Public);
    let sid = it.server_id.unwrap();
    assert_eq!(env.mock.state().items[&sid.0].kind, "thread");

    // Plain `publish` on a scratch note goes through promotion too.
    let id2 = b.create_scratch(Kind::Fragment, "short and sweet").unwrap();
    let out = b.publish(&id2, None).unwrap();
    assert_eq!(out.version, 1);
    let it = b.item(&id2).unwrap();
    assert_eq!((it.kind, it.status), (Kind::Fragment, Status::Public));
    assert_eq!(b.items().len(), 2);
}

#[test]
fn offline_promotion_queues() {
    let env = Env::new();
    let b = env.manual();
    let id = b
        .create_scratch(Kind::Fragment, "written on a ferry")
        .unwrap();
    env.mock.set_down(true);

    b.promote(&id, Promote::Draft).unwrap();
    let it = b.item(&id).unwrap();
    assert_eq!(it.status, Status::Draft);
    assert!(it.pending_sync);
    assert!(matches!(b.sync_now(), Err(CoreError::Offline)));
    assert_eq!(b.sync_status(), SyncStatus::Offline { pending: 1 });

    // Publishing while offline fails, but the draft stays queued.
    let id2 = b.create_scratch(Kind::Fragment, "and another").unwrap();
    assert!(matches!(
        b.promote(&id2, Promote::Publish { note: None }),
        Err(CoreError::Offline)
    ));
    assert_eq!(b.item(&id2).unwrap().status, Status::Draft);
    assert!(b.item(&id2).unwrap().pending_sync);

    env.mock.set_down(false);
    b.sync_now().unwrap();
    for id in [&id, &id2] {
        let it = b.item(id).unwrap();
        assert!(it.server_id.is_some() && !it.pending_sync);
    }
    assert_eq!(b.items().len(), 2);
    assert_eq!(creates(&env), 2);
}

#[test]
fn scratch_notes_survive_a_reopen() {
    let env = Env::new();
    let id = {
        let b = env.manual();
        b.create_scratch(Kind::Thread, "kept across launches")
            .unwrap()
    };
    let b = env.manual();
    let it = b.item(&id).unwrap();
    assert_eq!((it.status, it.kind), (Status::Scratch, Kind::Thread));
    assert!(wait_until(T, || b.sync_status() == SyncStatus::Synced));
}

#[test]
fn promotion_kind_rule() {
    assert_eq!(promotion_kind(Kind::Fragment, "short"), Kind::Fragment);
    assert_eq!(promotion_kind(Kind::Thread, "short"), Kind::Thread);
    assert_eq!(
        promotion_kind(Kind::Fragment, &"x".repeat(1000)),
        Kind::Fragment
    );
    assert_eq!(
        promotion_kind(Kind::Fragment, &"x".repeat(1001)),
        Kind::Thread
    );
    // The server's count: TK markup is stripped first.
    let tk = format!("{}[TK]a very long instruction[/TK]", "x".repeat(990));
    assert_eq!(promotion_kind(Kind::Fragment, &tk), Kind::Fragment);
}

// --- scratch media ---

const PNG_A: &[u8] = b"\x89PNG\r\n\x1a\n first image";
const PNG_B: &[u8] = b"\x89PNG\r\n\x1a\n second image";

fn media_posts(env: &Env) -> usize {
    env.mock.state().count("POST /api/media")
}

#[test]
fn an_image_pasted_into_a_scratch_note_stays_on_this_mac() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_scratch(Kind::Fragment, "harbour at dusk").unwrap();
    let url = b.save_scratch_media(PNG_A, "image/png").unwrap();
    assert!(url.starts_with("blyg-local:"), "{url}");
    b.save(&id, &format!("harbour at dusk\n\n![]({url})"))
        .unwrap();

    let file = b.scratch_media_file(&url).expect("kept in the data dir");
    assert!(file.starts_with(env.data_dir().join("scratch-media")));
    assert_eq!(std::fs::read(file).unwrap(), PNG_A);
    b.sync_now().unwrap();
    let st = env.mock.state();
    assert_eq!(st.count("POST"), 0, "{:?}", st.log);
    assert_eq!(st.count("PUT"), 0, "{:?}", st.log);
    assert!(st.media_bodies.is_empty());
}

#[test]
fn promotion_uploads_local_images_and_rewrites_them() {
    let env = Env::new();
    let b = env.manual();
    let a = b.save_scratch_media(PNG_A, "image/png").unwrap();
    let c = b.save_scratch_media(PNG_B, "image/png").unwrap();
    let text = format!("two views\n\n![]({a})\n\n![b]({c})\n\nagain ![]({a})");
    let id = b.create_scratch(Kind::Thread, &text).unwrap();

    b.promote(&id, Promote::Draft).unwrap();
    assert_eq!(media_posts(&env), 2, "one upload per distinct image");
    let it = b.item(&id).unwrap();
    assert_eq!(it.status, Status::Draft);
    assert!(!it.content_md.contains("blyg-local:"), "{}", it.content_md);
    let origin = env.mock.url.clone();
    assert_eq!(
        it.content_md.matches(&format!("{origin}/media/")).count(),
        3,
        "{}",
        it.content_md
    );
    // Uploaded unattached (no item_id), like a paste into a draft.
    for body in &env.mock.state().media_bodies {
        assert!(!String::from_utf8_lossy(body).contains("name=\"item_id\""));
    }

    b.sync_now().unwrap();
    let sid = b.item(&id).unwrap().server_id.unwrap();
    let server = env.mock.state().items[&sid.0].content_md.clone();
    assert_eq!(server, b.item(&id).unwrap().content_md);
    assert!(!server.contains("blyg-local:"));
    // Uploads came before the create.
    let log = env.mock.state().log.clone();
    let create = log.iter().position(|l| l == "POST /api/items").unwrap();
    let last_media = log.iter().rposition(|l| l == "POST /api/media").unwrap();
    assert!(last_media < create, "{log:?}");
}

#[test]
fn a_failed_upload_leaves_the_note_scratch() {
    let env = Env::new();
    let b = env.manual();
    let ok = b.save_scratch_media(PNG_A, "image/png").unwrap();
    // The mock refuses anything but PNG (415).
    let bad = b.save_scratch_media(b"GIF89a", "image/gif").unwrap();
    let text = format!("gulls\n\n![]({ok})\n\n![]({bad})");
    let id = b.create_scratch(Kind::Fragment, &text).unwrap();

    let err = b.promote(&id, Promote::Draft).err().expect("refused");
    assert!(
        err.to_string().contains("image couldn't be uploaded"),
        "{err}"
    );
    let it = b.item(&id).unwrap();
    assert_eq!(it.status, Status::Scratch, "no half-promoted state");
    assert_eq!(it.content_md, text, "references untouched");
    assert!(!it.pending_sync);
    let st = env.mock.state();
    assert_eq!(st.count("POST /api/items"), 0, "{:?}", st.log);
    assert_eq!(st.count("DELETE /api/media/"), 1, "the orphan is removed");
    drop(st);

    // The same through publish.
    assert!(b.publish(&id, None).is_err());
    assert_eq!(b.item(&id).unwrap().status, Status::Scratch);
}

#[test]
fn offline_promotion_queues_the_uploads() {
    let env = Env::new();
    let b = env.manual();
    let a = b.save_scratch_media(PNG_A, "image/png").unwrap();
    let id = b
        .create_scratch(Kind::Fragment, &format!("ferry deck ![]({a})"))
        .unwrap();
    env.mock.set_down(true);

    b.promote(&id, Promote::Draft).unwrap();
    let it = b.item(&id).unwrap();
    assert_eq!(it.status, Status::Draft);
    assert!(it.pending_sync);
    assert!(it.content_md.contains("blyg-local:"), "rewritten later");

    env.mock.set_down(false);
    b.sync_now().unwrap();
    assert_eq!(media_posts(&env), 1);
    let it = b.item(&id).unwrap();
    assert!(!it.content_md.contains("blyg-local:"), "{}", it.content_md);
    let sid = it.server_id.clone().expect("created");
    let server = env.mock.state().items[&sid.0].content_md.clone();
    assert_eq!(server, it.content_md);
    assert!(server.contains(&format!("{}/media/", env.mock.url)));
}
