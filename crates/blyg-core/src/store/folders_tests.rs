//! --- reader folders --- the local folders of subscriptions (schema v7).

use super::Store;
use crate::backend::CoreError;
use crate::model::*;

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

fn names(s: &Store) -> Vec<String> {
    s.folders().into_iter().map(|f| f.name).collect()
}

#[test]
fn folders_are_created_renamed_and_listed_in_order() {
    let s = store();
    assert!(s.folders().is_empty());
    let a = s.create_folder("  Friends  ").unwrap();
    let b = s.create_folder("Gardens").unwrap();
    assert_eq!(a.name, "Friends", "trimmed");
    assert_eq!((a.position, b.position), (0, 1));
    assert_eq!(names(&s), ["Friends", "Gardens"]);

    s.rename_folder(&b.id, "Gardens   and  sheds").unwrap();
    assert_eq!(names(&s), ["Friends", "Gardens and sheds"]);
    // Renaming to its own name (another case) is fine.
    s.rename_folder(&a.id, "friends").unwrap();
    assert_eq!(names(&s)[0], "friends");

    // Empty and duplicate names are refused; unknown ids are NotFound.
    assert!(matches!(
        s.create_folder("   "),
        Err(CoreError::Rejected { status: 400, .. })
    ));
    assert!(matches!(
        s.create_folder("GARDENS AND SHEDS"),
        Err(CoreError::Rejected { status: 409, .. })
    ));
    assert!(matches!(
        s.rename_folder(&a.id, "Gardens and sheds"),
        Err(CoreError::Rejected { status: 409, .. })
    ));
    assert!(matches!(
        s.rename_folder("fld-nope", "x"),
        Err(CoreError::NotFound)
    ));
    assert_eq!(s.folders().len(), 2);
}

#[test]
fn folders_reorder_with_dense_positions() {
    let s = store();
    let a = s.create_folder("A").unwrap();
    let b = s.create_folder("B").unwrap();
    let c = s.create_folder("C").unwrap();
    s.move_folder(&c.id, 0).unwrap();
    assert_eq!(names(&s), ["C", "A", "B"]);
    s.move_folder(&c.id, 99).unwrap();
    assert_eq!(names(&s), ["A", "B", "C"], "clamped to the end");
    s.move_folder(&a.id, 1).unwrap();
    assert_eq!(names(&s), ["B", "A", "C"]);
    let pos: Vec<u32> = s.folders().iter().map(|f| f.position).collect();
    assert_eq!(pos, [0, 1, 2]);
    assert!(matches!(
        s.move_folder("fld-nope", 0),
        Err(CoreError::NotFound)
    ));
    let _ = b;
}

#[test]
fn a_subscription_is_in_at_most_one_folder() {
    let s = store();
    let a = s.create_folder("A").unwrap();
    let b = s.create_folder("B").unwrap();
    s.set_subscription_folder("sub-1", Some(&a.id)).unwrap();
    s.set_subscription_folder("sub-2", Some(&a.id)).unwrap();
    s.set_subscription_folder("sub-1", Some(&b.id)).unwrap();
    let filed = s.subscription_folders();
    assert_eq!(filed.get("sub-1"), Some(&b.id), "moved, not copied");
    assert_eq!(filed.get("sub-2"), Some(&a.id));
    s.set_subscription_folder("sub-2", None).unwrap();
    assert!(!s.subscription_folders().contains_key("sub-2"));
    // Filing into a folder that doesn't exist is refused.
    assert!(matches!(
        s.set_subscription_folder("sub-3", Some("fld-nope")),
        Err(CoreError::NotFound)
    ));
}

#[test]
fn deleting_a_folder_unfiles_its_subscriptions() {
    let s = store();
    let a = s.create_folder("A").unwrap();
    let b = s.create_folder("B").unwrap();
    let c = s.create_folder("C").unwrap();
    s.set_subscription_folder("sub-1", Some(&b.id)).unwrap();
    s.set_subscription_folder("sub-2", Some(&b.id)).unwrap();
    s.set_subscription_folder("sub-3", Some(&c.id)).unwrap();
    s.delete_folder(&b.id).unwrap();
    assert_eq!(names(&s), ["A", "C"]);
    let pos: Vec<u32> = s.folders().iter().map(|f| f.position).collect();
    assert_eq!(pos, [0, 1], "renumbered");
    let filed = s.subscription_folders();
    assert!(!filed.contains_key("sub-1") && !filed.contains_key("sub-2"));
    assert_eq!(filed.get("sub-3"), Some(&c.id), "other folders untouched");
    assert!(matches!(s.delete_folder(&b.id), Err(CoreError::NotFound)));
    let _ = a;
}

#[test]
fn folders_survive_a_subscriptions_refresh() {
    // `replace_subscriptions` rewrites the table on every pull; membership
    // is keyed by id, so it outlives that.
    let s = store();
    let f = s.create_folder("Friends").unwrap();
    let sub = Subscription {
        id: "sub-rue".into(),
        kind: SubscriptionKind::Blyg,
        origin: "https://rue.blyg.example.com/".into(),
        feed_url: "https://rue.blyg.example.com/feed.json".into(),
        title: "Rue".into(),
        status: "active".into(),
        in_blogroll: false,
        title_follows_source: true,
    };
    s.replace_subscriptions(std::slice::from_ref(&sub)).unwrap();
    s.set_subscription_folder("sub-rue", Some(&f.id)).unwrap();
    let mut renamed = sub.clone();
    renamed.title = "Rue's notes".into();
    s.replace_subscriptions(&[renamed]).unwrap();
    assert_eq!(s.subscription_folders().get("sub-rue"), Some(&f.id));
}

#[test]
fn migration_to_v7_adds_empty_folders_and_keeps_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v6.db");
    {
        let mut c = rusqlite::Connection::open(&path).unwrap();
        super::schema::migrate_to(&mut c, 6).unwrap();
        c.execute(
            "INSERT INTO subscriptions (pos, id, json) VALUES (0, 'sub-a', ?1)",
            [serde_json::json!({
                "id": "sub-a", "kind": "blyg", "origin": "https://a.example/",
                "feed_url": "https://a.example/feed.json", "title": "A",
                "status": "active", "in_blogroll": false
            })
            .to_string()],
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), super::schema::latest());
    assert!(super::schema::latest() >= 7);
    assert!(s.folders().is_empty());
    assert!(s.subscription_folders().is_empty());
    assert_eq!(s.subscriptions().len(), 1, "existing data kept");
    let f = s.create_folder("New").unwrap();
    s.set_subscription_folder("sub-a", Some(&f.id)).unwrap();
    drop(s);
    // And it's all still there after reopening.
    let s = Store::open(&path).unwrap();
    assert_eq!(names(&s), ["New"]);
    assert_eq!(s.subscription_folders().get("sub-a"), Some(&f.id));
}
