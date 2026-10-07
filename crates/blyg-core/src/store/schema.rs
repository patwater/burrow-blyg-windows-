//! Forward-only migrations, tracked with `PRAGMA user_version`.
//! Never edit a shipped migration; append a new one.

use rusqlite::Connection;

const MIGRATIONS: &[&str] = &[
    // v1
    r#"
    CREATE TABLE items (
        rid             INTEGER PRIMARY KEY,
        local_id        TEXT NOT NULL UNIQUE,
        server_id       TEXT UNIQUE,
        kind            TEXT NOT NULL,
        -- kind of the server-side draft, when known (for set_kind recreation)
        server_kind     TEXT,
        status          TEXT NOT NULL,
        version         INTEGER NOT NULL DEFAULT 0,
        dirty           INTEGER NOT NULL DEFAULT 1,
        content_md      TEXT NOT NULL DEFAULT '',
        created         TEXT NOT NULL,
        updated         TEXT NOT NULL,
        permalink       TEXT,
        stub_of         TEXT,
        forked_from     TEXT,
        show_responses  INTEGER NOT NULL DEFAULT 0,
        conflict        INTEGER NOT NULL DEFAULT 0,
        -- server `updated` / content_md the local copy was last synced against
        base_updated    TEXT,
        base_content    TEXT,
        -- the server's content while `conflict` = 1
        theirs_content  TEXT
    );
    CREATE INDEX items_updated ON items(updated DESC, rid DESC);

    CREATE VIRTUAL TABLE items_fts USING fts5(
        content_md, content='items', content_rowid='rid', tokenize='trigram'
    );
    CREATE TRIGGER items_ai AFTER INSERT ON items BEGIN
        INSERT INTO items_fts(rowid, content_md) VALUES (new.rid, new.content_md);
    END;
    CREATE TRIGGER items_ad AFTER DELETE ON items BEGIN
        INSERT INTO items_fts(items_fts, rowid, content_md) VALUES ('delete', old.rid, old.content_md);
    END;
    CREATE TRIGGER items_au AFTER UPDATE OF content_md ON items BEGIN
        INSERT INTO items_fts(items_fts, rowid, content_md) VALUES ('delete', old.rid, old.content_md);
        INSERT INTO items_fts(rowid, content_md) VALUES (new.rid, new.content_md);
    END;

    CREATE TABLE outbox (
        seq         INTEGER PRIMARY KEY AUTOINCREMENT,
        local_id    TEXT NOT NULL,
        op          TEXT NOT NULL,          -- create | save | recreate
        payload     TEXT NOT NULL DEFAULT '{}',
        not_before  INTEGER NOT NULL,       -- unix ms; debounce
        in_flight   INTEGER NOT NULL DEFAULT 0,
        attempts    INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX outbox_local ON outbox(local_id);

    CREATE TABLE versions (
        local_id      TEXT NOT NULL,
        version       INTEGER NOT NULL,
        published_at  TEXT NOT NULL,
        note          TEXT,
        pinned        INTEGER NOT NULL DEFAULT 0,
        endcap        INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (local_id, version)
    );

    -- One row per post: keyed by (subscription_id, remote_id), upserted on
    -- every pull, never appended. Read state is local-only.
    CREATE TABLE reading (
        subscription_id   TEXT NOT NULL,
        remote_id         TEXT NOT NULL,
        origin            TEXT NOT NULL,
        observed_at       TEXT NOT NULL,
        -- absolute page URL (resolved against origin), for de-duplication
        page_url          TEXT,
        -- 'blyg' | 'rss' | NULL (subscription unknown)
        sub_kind          TEXT,
        state             TEXT NOT NULL,
        version           INTEGER NOT NULL,
        json              TEXT NOT NULL,
        read_version      INTEGER,
        read_snapshot_md  TEXT,
        PRIMARY KEY (subscription_id, remote_id)
    );
    CREATE INDEX reading_observed ON reading(observed_at DESC);
    CREATE INDEX reading_page ON reading(subscription_id, page_url);

    CREATE TABLE subscriptions (
        pos   INTEGER PRIMARY KEY,
        id    TEXT NOT NULL UNIQUE,
        json  TEXT NOT NULL
    );

    CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
    "#,
    // v2: versions & pins (spec §8.4, §13.4). Stop retaining the unpinned text
    // of other people's past versions, purge the content of withdrawn posts
    // that no pin backs, and cache public changelogs and pins.
    r#"
    UPDATE reading SET read_snapshot_md = NULL;
    ALTER TABLE reading DROP COLUMN read_snapshot_md;
    UPDATE reading
       SET json = json_set(json, '$.content_md', '', '$.content_html', '')
     WHERE state = 'tombstone'
       AND json_extract(json, '$.pinned_version_retained') IS NULL;

    -- Public changelog of someone else's item, keyed by origin (the same post
    -- can arrive through several subscriptions). Short TTL.
    CREATE TABLE remote_changelog (
        origin      TEXT NOT NULL,
        remote_id   TEXT NOT NULL,
        json        TEXT NOT NULL,
        fetched_at  INTEGER NOT NULL,   -- unix ms
        PRIMARY KEY (origin, remote_id)
    );

    -- Pinned versions (items/{id}/v{n}.json). Pins are immutable, so these
    -- are kept forever, including past withdrawal (§13.4 allows it).
    CREATE TABLE remote_pins (
        origin      TEXT NOT NULL,
        remote_id   TEXT NOT NULL,
        version     INTEGER NOT NULL,
        json        TEXT NOT NULL,
        fetched_at  INTEGER NOT NULL,
        PRIMARY KEY (origin, remote_id, version)
    );
    "#,
    // v3: client-recorded TK provenance (docs/SPEC.md § Client-recorded
    // provenance). `tk_prov` is a JSON array, one entry per TK scope of
    // `tk_prov_md` (the text it's keyed to), NULL = not tracked here.
    // `tk_prov_dirty` = the server's copy needs the combined push.
    r#"
    ALTER TABLE items ADD COLUMN tk_prov TEXT;
    ALTER TABLE items ADD COLUMN tk_prov_md TEXT;
    ALTER TABLE items ADD COLUMN tk_prov_dirty INTEGER NOT NULL DEFAULT 0;
    "#,
    // v4 --- profiles --- (docs/SPEC.md § Profiles): someone's public
    // profile as last fetched, for offline use. Keyed by the URL that was
    // opened and by the resolved origin (both rows hold the same JSON).
    r#"
    CREATE TABLE profiles (
        key         TEXT PRIMARY KEY,
        json        TEXT NOT NULL,
        fetched_at  INTEGER NOT NULL    -- unix ms
    );
    "#,
    // v5: the reading list sorts by the post's own date (issue #6), not by
    // when the server imported it. `sort_at` = `ReadingItem::sort_at()`
    // (updated, else created, else observed_at; normalized UTC), written on
    // every upsert. Existing rows are filled in by `Store::init`
    // (`backfill_sort_at`), which parses dates the way the app does.
    r#"
    ALTER TABLE reading ADD COLUMN sort_at TEXT NOT NULL DEFAULT '';
    CREATE INDEX reading_sort ON reading(sort_at DESC, subscription_id, remote_id);
    "#,
    // v6 --- responses --- (issue #7): what each reading row quotes, stubs
    // or forks (`ReadingItem::references`), so "who responded to this post,
    // in your network" is an index lookup. Derived from `reading.json`,
    // rewritten on every upsert, filled for existing rows by `Store::init`.
    r#"
    CREATE TABLE reading_refs (
        subscription_id  TEXT NOT NULL,
        remote_id        TEXT NOT NULL,
        target_origin    TEXT NOT NULL,   -- normalized (post_key)
        target_id        TEXT NOT NULL,   -- lowercased
        relation         TEXT NOT NULL,   -- quotes | stubs | forks
        version          INTEGER,
        PRIMARY KEY (subscription_id, remote_id, target_origin, target_id, relation)
    );
    CREATE INDEX reading_refs_target ON reading_refs(target_id, target_origin);
    "#,
    // v7 --- reader folders --- the Reader's folders of subscriptions. Local
    // only (never synced; the server's hoppers are something else). One
    // folder per subscription; deleting a folder unfiles its subscriptions.
    // Membership is keyed by subscription id without a foreign key, because
    // `subscriptions` is rewritten on every pull.
    r#"
    CREATE TABLE folders (
        id        TEXT PRIMARY KEY,
        name      TEXT NOT NULL,
        position  INTEGER NOT NULL
    );
    CREATE TABLE folder_members (
        subscription_id  TEXT PRIMARY KEY,
        folder_id        TEXT NOT NULL REFERENCES folders(id) ON DELETE CASCADE
    );
    CREATE INDEX folder_members_folder ON folder_members(folder_id);
    "#,
    // v8: the item's own responses choice (studio 0.8): 'default' | 'show' |
    // 'hide', NULL when the server hasn't reported one. `show_responses`
    // stays the effective state.
    r#"
    ALTER TABLE items ADD COLUMN responses_mode TEXT;
    "#,
    // v9: one reference per (post, target), the strongest relation winning
    // (fork > stub > quote), and whether the quote is of a passage. The
    // backfill after any migration rebuilds the rows under the new rule.
    r#"
    ALTER TABLE reading_refs ADD COLUMN partial INTEGER NOT NULL DEFAULT 0;
    "#,
    // v10: the item's generated-text highlight (studio 0.27), like v8's
    // responses: `highlight_mode` is its own choice ('default' | 'show' |
    // 'hide', NULL = unreported), `highlight` the effective state.
    r#"
    ALTER TABLE items ADD COLUMN highlight_mode TEXT;
    ALTER TABLE items ADD COLUMN highlight INTEGER NOT NULL DEFAULT 0;
    "#,
    // v11: full-text search over imported posts for the `[[` / `![[`
    // picker (studio 0.29), a trigram index like `items_fts`. Keyed by the
    // reading row's rowid and kept current by triggers, so the reading
    // code never has to think about it. The text is the post's Markdown
    // (an RSS row without any: its HTML).
    r#"
    CREATE VIRTUAL TABLE reading_fts USING fts5(body, tokenize='trigram');
    INSERT INTO reading_fts(rowid, body)
        SELECT rowid, COALESCE(NULLIF(json_extract(json, '$.content_md'), ''),
                               json_extract(json, '$.content_html'), '')
          FROM reading;
    CREATE TRIGGER reading_fts_ai AFTER INSERT ON reading BEGIN
        DELETE FROM reading_fts WHERE rowid = new.rowid;
        INSERT INTO reading_fts(rowid, body) VALUES (new.rowid,
            COALESCE(NULLIF(json_extract(new.json, '$.content_md'), ''),
                     json_extract(new.json, '$.content_html'), ''));
    END;
    CREATE TRIGGER reading_fts_ad AFTER DELETE ON reading BEGIN
        DELETE FROM reading_fts WHERE rowid = old.rowid;
    END;
    CREATE TRIGGER reading_fts_au AFTER UPDATE OF json ON reading BEGIN
        DELETE FROM reading_fts WHERE rowid = old.rowid;
        INSERT INTO reading_fts(rowid, body) VALUES (new.rowid,
            COALESCE(NULLIF(json_extract(new.json, '$.content_md'), ''),
                     json_extract(new.json, '$.content_html'), ''));
    END;
    "#,
    // v12 --- read/unread --- (store/read_sync.rs)
    // `read_at`: when the row was marked read here (unix ms), until the
    //   server confirms it (then NULL). Sent as `read_at` to a server that
    //   can clear read state, so an old read can't undo a newer "unread".
    //   Reads from before this migration have no known time: 0.
    // `read_floor`: the row was marked unread here; server read versions at
    //   or below it are stale and don't re-mark it read.
    // Subscriptions gain `title_follows_source` (studio 0.30): unknown
    //   until the next pull, so false (the name stays as it is).
    r#"
    ALTER TABLE reading ADD COLUMN read_at INTEGER;
    ALTER TABLE reading ADD COLUMN read_floor INTEGER;
    UPDATE reading SET read_at = 0 WHERE read_version IS NOT NULL;
    UPDATE subscriptions SET json = json_insert(json, '$.title_follows_source', json('false'));
    "#,
];

/// Returns whether any migration ran.
pub fn migrate(conn: &mut Connection) -> rusqlite::Result<bool> {
    migrate_to(conn, MIGRATIONS.len())
}

/// Apply migrations up to (and including) version `target`. Returns whether
/// anything ran. Tests use this to build an old-schema database.
pub(super) fn migrate_to(conn: &mut Connection, target: usize) -> rusqlite::Result<bool> {
    let current: usize =
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as usize;
    let mut ran = false;
    for (i, sql) in MIGRATIONS.iter().enumerate().take(target).skip(current) {
        ran = true;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
        tx.commit()?;
    }
    Ok(ran)
}

pub fn latest() -> usize {
    MIGRATIONS.len()
}
