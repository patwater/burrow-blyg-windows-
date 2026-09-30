//! blyg-core against an in-process mock of the owner API.

mod common;

use std::time::Duration;

use blyg_core::*;
use common::*;

const T: Duration = Duration::from_secs(5);

fn server_content(env: &Env, sid: &ServerId) -> String {
    env.mock.state().items[&sid.0].content_md.clone()
}

// ------------------------------------------------------------------ outbox

#[test]
fn create_offline_then_sync_maps_ids() {
    let env = Env::new();
    let b = env.manual();
    env.mock.set_down(true);

    let id = b
        .create_draft(Kind::Fragment, "written on a plane")
        .unwrap();
    b.save(&id, "written on a plane, edited").unwrap();
    assert!(matches!(b.sync_now(), Err(CoreError::Offline)));
    assert_eq!(b.sync_status(), SyncStatus::Offline { pending: 1 });
    let it = b.item(&id).unwrap();
    assert!(it.server_id.is_none());
    assert!(it.pending_sync);

    env.mock.set_down(false);
    b.sync_now().unwrap();
    let it = b.item(&id).unwrap();
    let sid = it.server_id.clone().expect("server id assigned");
    assert!(!it.pending_sync);
    assert_eq!(server_content(&env, &sid), "written on a plane, edited");
    assert_eq!(b.sync_status(), SyncStatus::Synced);
    assert_eq!(b.items().len(), 1, "the pull didn't duplicate the item");

    // later saves go to the mapped id, never a second POST
    b.save(&id, "third").unwrap();
    b.sync_now().unwrap();
    assert_eq!(server_content(&env, &sid), "third");
    let st = env.mock.state();
    assert_eq!(st.count("POST /api/items"), 1);
    assert_eq!(st.count(&format!("PUT /api/items/{}", sid.0)), 1);
}

#[test]
fn later_ops_wait_for_the_create() {
    let env = Env::new();
    let b = env.manual();
    let a = b.create_draft(Kind::Fragment, "a").unwrap();
    b.sync_now().unwrap();
    let c = b.create_draft(Kind::Thread, "c").unwrap();
    b.save(&a, "a2").unwrap();
    b.save(&c, "c2").unwrap();
    b.sync_now().unwrap();
    let ca = b.item(&c).unwrap();
    assert_eq!(server_content(&env, ca.server_id.as_ref().unwrap()), "c2");
    assert_eq!(
        env.mock.state().items[&ca.server_id.unwrap().0].kind,
        "thread"
    );
    assert_eq!(
        server_content(&env, b.item(&a).unwrap().server_id.as_ref().unwrap()),
        "a2"
    );
}

#[test]
fn save_coalescing_with_the_worker() {
    let env = Env::new();
    let b = env.open(SyncOptions {
        debounce: Duration::from_millis(250),
        ..fast()
    });
    let id = b.create_draft(Kind::Fragment, "").unwrap();
    let mut text = String::new();
    for ch in "fifty keystrokes worth of typing, more or less!!".chars() {
        text.push(ch);
        b.save(&id, &text).unwrap();
    }
    assert!(wait_until(T, || b
        .item(&id)
        .is_some_and(|i| !i.pending_sync && i.server_id.is_some())));
    let sid = b.item(&id).unwrap().server_id.unwrap();
    assert_eq!(server_content(&env, &sid), text);
    {
        let st = env.mock.state();
        assert_eq!(st.count("POST /api/items"), 1, "{:?}", st.log);
        assert_eq!(
            st.count("PUT /api/items/"),
            0,
            "saves before the create landed were absorbed"
        );
    }
    for i in 0..50 {
        b.save(&id, &format!("{text} {i}")).unwrap();
    }
    assert!(wait_until(T, || !b.item(&id).unwrap().pending_sync));
    assert_eq!(server_content(&env, &sid), format!("{text} 49"));
    assert_eq!(
        env.mock.state().count("PUT /api/items/"),
        1,
        "50 saves = one PUT"
    );
    assert!(wait_until(T, || b.sync_status() == SyncStatus::Synced));
}

#[test]
fn offline_then_online_flushes_with_backoff() {
    let env = Env::new();
    env.mock.set_down(true);
    let b = env.open(fast());
    let id = b.create_draft(Kind::Fragment, "queued").unwrap();
    b.save(&id, "queued twice").unwrap();
    let other = b.create_draft(Kind::Thread, "another").unwrap();
    assert!(wait_until(T, || matches!(
        b.sync_status(),
        SyncStatus::Offline { pending: 2 }
    )));
    assert!(
        env.events()
            .iter()
            .any(|e| matches!(e, CoreEvent::SyncStatus(SyncStatus::Offline { .. })))
    );

    env.mock.set_down(false);
    assert!(
        wait_until(T, || b.sync_status() == SyncStatus::Synced),
        "status {:?}",
        b.sync_status()
    );
    for i in [&id, &other] {
        let it = b.item(i).unwrap();
        assert!(!it.pending_sync);
        assert!(it.server_id.is_some());
    }
    assert_eq!(
        server_content(&env, &b.item(&id).unwrap().server_id.unwrap()),
        "queued twice"
    );
    assert_eq!(env.mock.state().items.len(), 2);
    let ev = env.events();
    let last_status = ev.iter().rev().find_map(|e| match e {
        CoreEvent::SyncStatus(s) => Some(*s),
        _ => None,
    });
    assert_eq!(last_status, Some(SyncStatus::Synced));
}

#[test]
fn state_survives_restart() {
    let env = Env::new();
    env.mock.set_down(true);
    {
        let b = env.manual();
        b.create_draft(Kind::Fragment, "before quitting").unwrap();
    }
    env.mock.set_down(false);
    let b = env.manual();
    assert_eq!(b.items().len(), 1);
    b.sync_now().unwrap();
    assert_eq!(
        env.mock.state().items.values().next().unwrap().content_md,
        "before quitting"
    );
}

#[test]
fn pull_brings_in_server_items_and_removals() {
    let env = Env::new();
    let b = env.manual();
    let sid = env
        .mock
        .state()
        .add_item("thread", "# From the studio\nbody");
    b.sync_now().unwrap();
    let items = b.items();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title(), "From the studio");
    assert_eq!(items[0].kind, Kind::Thread);
    assert!(env.events().contains(&CoreEvent::ItemsChanged));
    // edited in the studio, no local edits → just updates
    env.mock.state().edit(&sid, "changed there");
    b.pull_now().unwrap();
    assert_eq!(b.items()[0].content_md, "changed there");
    assert!(!b.items()[0].conflict);
    // deleted in the studio → gone here
    env.mock.state().items.remove(&sid);
    b.pull_now().unwrap();
    assert!(b.items().is_empty());
}

#[test]
fn save_to_an_item_deleted_on_the_server_recreates_it() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, "hello").unwrap();
    b.sync_now().unwrap();
    let old = b.item(&id).unwrap().server_id.unwrap();
    env.mock.state().items.remove(&old.0);
    b.save(&id, "still mine").unwrap();
    b.sync_now().unwrap();
    let new = b.item(&id).unwrap().server_id.unwrap();
    assert_ne!(new, old);
    assert_eq!(server_content(&env, &new), "still mine");
}

// ------------------------------------------------------------------ publish

#[test]
fn publish_flushes_first() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, "draft").unwrap();
    b.save(&id, "final words").unwrap();
    let out = b.publish(&id, Some("first")).unwrap();
    assert_eq!(out.version, 1);
    let it = b.item(&id).unwrap();
    let sid = it.server_id.clone().unwrap();
    assert_eq!(out.permalink, format!("http://mock.test/f/{}", sid.0));
    assert_eq!(it.status, Status::Public);
    assert_eq!(it.version, 1);
    assert!(!it.dirty);
    assert!(!it.pending_sync);
    assert_eq!(it.permalink.as_deref(), Some(out.permalink.as_str()));
    let st = env.mock.state();
    assert_eq!(st.items[&sid.0].content_md, "final words");
    assert_eq!(st.items[&sid.0].versions[0]["note"], "first");
    let post = st.log.iter().position(|l| l == "POST /api/items").unwrap();
    let publish = st.log.iter().position(|l| l.ends_with("/publish")).unwrap();
    assert!(post < publish);
}

#[test]
fn publish_pushes_pending_edits_of_a_synced_item() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, "v1").unwrap();
    b.publish(&id, None).unwrap();
    b.save(&id, "v2 text").unwrap();
    assert!(b.item(&id).unwrap().dirty);
    let out = b.publish(&id, None).unwrap();
    assert_eq!(out.version, 2);
    let it = b.item(&id).unwrap();
    assert_eq!(
        server_content(&env, it.server_id.as_ref().unwrap()),
        "v2 text"
    );
    assert!(!it.dirty);
}

#[test]
fn over_limit_publish_is_rejected() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, &"x".repeat(1001)).unwrap();
    match b.publish(&id, None) {
        Err(CoreError::Rejected {
            status: 400,
            message,
            ..
        }) => assert!(message.contains("exceeds 1000")),
        Err(e) => panic!("wrong error {e:?}"),
        Ok(_) => panic!("should be rejected"),
    }
    let it = b.item(&id).unwrap();
    assert_eq!(it.status, Status::Draft);
    assert_eq!(it.version, 0);
    assert!(it.server_id.is_some(), "the draft itself did sync");
    assert_eq!(
        b.sync_status(),
        SyncStatus::Synced,
        "a rejection isn't a sync failure"
    );
}

#[test]
fn transclusion_errors_carry_details() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Thread, "![[BAD]]").unwrap();
    match b.publish(&id, None) {
        Err(CoreError::Rejected { details, .. }) => {
            assert_eq!(details, vec!["![[BAD]]: unknown item"])
        }
        other => panic!("{:?}", other.err()),
    }
}

#[test]
fn publish_offline_is_offline() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, "x").unwrap();
    env.mock.set_down(true);
    assert!(matches!(b.publish(&id, None), Err(CoreError::Offline)));
    assert!(b.item(&id).unwrap().pending_sync);
}

// ---------------------------------------------------------------- conflicts

/// A synced item, edited here (not yet pushed) and on the server.
fn diverge(env: &Env, b: &LiveBackend) -> (LocalId, ServerId) {
    let id = b.create_draft(Kind::Fragment, "base").unwrap();
    b.sync_now().unwrap();
    let sid = b.item(&id).unwrap().server_id.unwrap();
    env.mock.state().edit(&sid.0, "theirs");
    b.save(&id, "mine").unwrap();
    (id, sid)
}

#[test]
fn conflict_is_detected_on_pull() {
    let env = Env::new();
    let b = env.manual();
    let (id, sid) = diverge(&env, &b);
    b.pull_now().unwrap();
    let it = b.item(&id).unwrap();
    assert!(it.conflict);
    assert_eq!(it.content_md, "mine");
    assert!(env.events().contains(&CoreEvent::Conflict {
        local_id: id.clone(),
        mine: "mine".into(),
        theirs: "theirs".into()
    }));
    // conflicted items don't push
    b.sync_now().unwrap();
    assert_eq!(server_content(&env, &sid), "theirs");
    assert!(b.item(&id).unwrap().conflict);
    assert!(matches!(
        b.publish(&id, None),
        Err(CoreError::Rejected { status: 409, .. })
    ));
}

#[test]
fn conflict_is_detected_before_a_push_overwrites() {
    // sync_now pushes before it pulls; the pre-push check must still catch it.
    let env = Env::new();
    let b = env.manual();
    let (id, sid) = diverge(&env, &b);
    b.sync_now().unwrap();
    assert!(b.item(&id).unwrap().conflict);
    assert_eq!(
        server_content(&env, &sid),
        "theirs",
        "nothing was clobbered"
    );
    assert!(
        env.events()
            .iter()
            .any(|e| matches!(e, CoreEvent::Conflict { .. }))
    );
}

#[test]
fn no_conflict_when_the_server_only_saw_our_own_pushes() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, "one").unwrap();
    b.sync_now().unwrap();
    for n in ["two", "three", "four"] {
        b.save(&id, n).unwrap();
        b.sync_now().unwrap();
        b.save(&id, &format!("{n}+")).unwrap();
        b.pull_now().unwrap();
        assert!(!b.item(&id).unwrap().conflict, "false conflict at {n}");
    }
    // server changed to exactly what we have: converged, not a conflict
    let sid = b.item(&id).unwrap().server_id.unwrap();
    env.mock.state().edit(&sid.0, "four+");
    b.pull_now().unwrap();
    assert!(!b.item(&id).unwrap().conflict);
}

#[test]
fn resolve_keep_mine() {
    let env = Env::new();
    let b = env.manual();
    let (id, sid) = diverge(&env, &b);
    b.pull_now().unwrap();
    b.resolve_conflict(&id, Resolution::KeepMine).unwrap();
    assert!(!b.item(&id).unwrap().conflict);
    b.sync_now().unwrap();
    assert_eq!(server_content(&env, &sid), "mine");
    let it = b.item(&id).unwrap();
    assert!(!it.conflict && !it.pending_sync);
    assert_eq!(it.content_md, "mine");
}

#[test]
fn resolve_take_server() {
    let env = Env::new();
    let b = env.manual();
    let (id, sid) = diverge(&env, &b);
    b.pull_now().unwrap();
    b.resolve_conflict(&id, Resolution::TakeServer).unwrap();
    let it = b.item(&id).unwrap();
    assert_eq!(it.content_md, "theirs");
    assert!(!it.conflict && !it.pending_sync);
    b.sync_now().unwrap();
    assert_eq!(server_content(&env, &sid), "theirs");
    assert_eq!(b.items().len(), 1);
}

#[test]
fn resolve_keep_both() {
    let env = Env::new();
    let b = env.manual();
    let (id, sid) = diverge(&env, &b);
    b.pull_now().unwrap();
    b.resolve_conflict(&id, Resolution::KeepBoth).unwrap();
    let it = b.item(&id).unwrap();
    assert_eq!(it.content_md, "theirs");
    assert_eq!(it.server_id.as_ref(), Some(&sid));
    assert!(!it.conflict);
    let mine: Vec<_> = b
        .items()
        .into_iter()
        .filter(|i| i.content_md == "mine")
        .collect();
    assert_eq!(mine.len(), 1);
    assert_ne!(mine[0].local_id, id);
    assert_eq!(mine[0].status, Status::Draft);
    b.sync_now().unwrap();
    let st = env.mock.state();
    assert_eq!(st.items.len(), 2);
    assert_eq!(st.items[&sid.0].content_md, "theirs");
    assert!(st.items.values().any(|i| i.content_md == "mine"));
}

// ----------------------------------------------------------------- set_kind

#[test]
fn set_kind_before_first_sync_is_just_local() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, "long thought").unwrap();
    b.set_kind(&id, Kind::Thread).unwrap();
    assert_eq!(b.item(&id).unwrap().kind, Kind::Thread);
    b.sync_now().unwrap();
    let st = env.mock.state();
    assert_eq!(st.count("POST /api/items"), 1);
    assert_eq!(st.items.values().next().unwrap().kind, "thread");
}

#[test]
fn set_kind_of_a_synced_draft_recreates_it() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, "grows up").unwrap();
    b.sync_now().unwrap();
    let old = b.item(&id).unwrap().server_id.unwrap();
    b.save(&id, "grows up into a thread").unwrap();
    b.set_kind(&id, Kind::Thread).unwrap();
    b.sync_now().unwrap();
    let it = b.item(&id).unwrap();
    let new = it.server_id.clone().unwrap();
    assert_ne!(new, old);
    assert_eq!(it.kind, Kind::Thread);
    assert!(!it.pending_sync);
    assert_eq!(b.items().len(), 1);
    let st = env.mock.state();
    assert_eq!(st.items.len(), 1, "old draft deleted");
    assert_eq!(st.items[&new.0].kind, "thread");
    assert_eq!(st.items[&new.0].content_md, "grows up into a thread");
    assert_eq!(st.count(&format!("DELETE /api/items/{}", old.0)), 1);
}

#[test]
fn set_kind_toggled_back_is_a_no_op() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, "hmm").unwrap();
    b.sync_now().unwrap();
    b.set_kind(&id, Kind::Thread).unwrap();
    b.set_kind(&id, Kind::Fragment).unwrap();
    b.sync_now().unwrap();
    let st = env.mock.state();
    assert_eq!(st.count("POST /api/items"), 1);
    assert_eq!(st.count("DELETE"), 0);
}

#[test]
fn set_kind_after_publish_is_refused() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, "out there").unwrap();
    b.publish(&id, None).unwrap();
    assert!(matches!(
        b.set_kind(&id, Kind::Thread),
        Err(CoreError::Rejected { status: 409, .. })
    ));
}

// ------------------------------------------------------------ other remote

#[test]
fn withdraw_pin_versions_restore() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, "v1").unwrap();
    assert!(
        b.versions(&id).unwrap().is_empty(),
        "local-only: no versions"
    );
    b.publish(&id, Some("note one")).unwrap();
    b.save(&id, "v2").unwrap();
    b.publish(&id, None).unwrap();
    b.pin(&id, 1).unwrap();
    let vs = b.versions(&id).unwrap();
    assert_eq!(vs.len(), 2);
    assert!(vs[0].pinned);
    assert_eq!(vs[0].note.as_deref(), Some("note one"));

    b.restore(&id, 1).unwrap();
    let it = b.item(&id).unwrap();
    assert_eq!(it.content_md, "restored v1");
    assert!(it.dirty);
    assert!(!it.pending_sync);

    let v = b.withdraw(&id, None).unwrap();
    assert_eq!(v, 3);
    let it = b.item(&id).unwrap();
    assert_eq!(it.status, Status::Withdrawn);
    assert_eq!(it.kind, Kind::Fragment, "authored kind survives withdrawal");
    // offline: cached versions still answer
    env.mock.set_down(true);
    assert_eq!(b.versions(&id).unwrap().len(), 3);
}

#[test]
fn delete_drafts() {
    let env = Env::new();
    let b = env.manual();
    let local = b.create_draft(Kind::Fragment, "never synced").unwrap();
    env.mock.set_down(true);
    b.delete_draft(&local).unwrap(); // local-only: no network needed
    assert!(b.item(&local).is_none());
    env.mock.set_down(false);
    b.sync_now().unwrap();
    assert!(env.mock.state().items.is_empty());

    let synced = b.create_draft(Kind::Fragment, "synced").unwrap();
    b.sync_now().unwrap();
    b.delete_draft(&synced).unwrap();
    assert!(b.items().is_empty());
    assert!(env.mock.state().items.is_empty());

    let public = b.create_draft(Kind::Fragment, "public").unwrap();
    b.publish(&public, None).unwrap();
    assert!(matches!(
        b.delete_draft(&public),
        Err(CoreError::Rejected { status: 409, .. })
    ));
}

#[test]
fn media_upload_is_multipart() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Thread, "pic").unwrap();
    let png = vec![0x89, b'P', b'N', b'G', 1, 2, 3];
    let m = b
        .upload_media(png, "image/png", Some(&id), Some("a cat"))
        .unwrap();
    assert!(m.url.starts_with("media/") && m.url.ends_with(".png"));
    assert_eq!(m.mime, "image/png");
    let st = env.mock.state();
    let body = String::from_utf8_lossy(&st.media_bodies[0]).to_string();
    let sid = b.item(&id).unwrap().server_id.unwrap();
    assert!(
        body.contains(&sid.0),
        "item_id sent (item was synced first)"
    );
    assert!(body.contains("a cat"));
    drop(st);
    assert!(matches!(
        b.upload_media(vec![1], "application/pdf", None, None),
        Err(CoreError::Rejected { status: 415, .. })
    ));
}

#[test]
fn fork_and_show_responses() {
    let env = Env::new();
    let b = env.manual();
    let of = RemoteRef {
        origin: "https://else.example/".into(),
        id: "X".repeat(26),
        version: 2,
    };
    let id = b.fork(&of).unwrap();
    let it = b.item(&id).unwrap();
    assert_eq!(it.content_md, "forked content");
    assert_eq!(it.forked_from.as_ref(), Some(&of));
    b.set_show_responses(&id, true).unwrap();
    assert!(b.item(&id).unwrap().show_responses);
    assert!(env.mock.state().items[&it.server_id.unwrap().0].show_responses);
    b.sync_now().unwrap();
    assert_eq!(b.items().len(), 1, "pull didn't duplicate the fork");
}

#[test]
fn unauthorized_is_an_error_status() {
    let env = Env::new();
    let b = env.open_token(
        SyncOptions {
            start_worker: false,
            ..fast()
        },
        "wrong",
    );
    b.create_draft(Kind::Fragment, "x").unwrap();
    assert!(matches!(b.sync_now(), Err(CoreError::Unauthorized)));
    assert_eq!(b.sync_status(), SyncStatus::Error);
    assert_eq!(b.items().len(), 1, "nothing lost");
}

#[test]
fn not_synced_errors() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, "x").unwrap();
    assert!(matches!(b.withdraw(&id, None), Err(CoreError::NotSynced)));
    assert!(matches!(b.pin(&id, 1), Err(CoreError::NotSynced)));
    assert!(matches!(
        b.save(&LocalId("nope".into()), "x"),
        Err(CoreError::NotFound)
    ));
}

#[test]
fn token_is_redacted_in_debug() {
    let api = blyg_core::api::Api::new("http://127.0.0.1:1", "super-secret-token");
    assert!(!format!("{api:?}").contains("super-secret"));
}
