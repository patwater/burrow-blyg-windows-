//! The `[[` / `![[` picker's search over the store (see `crate::pick`):
//! own published items through `items_fts`, imported posts from blyg
//! subscriptions through `reading_fts` (v11). The indexes narrow the
//! candidates; `pick::matches` then decides, exactly as the in-memory
//! search does (words under three characters, which trigrams can't index,
//! are only checked there).

use rusqlite::params_from_iter;

use super::{ITEM_COLS, Store, row_to_item};
use crate::backend::Result;
use crate::model::ReadingItem;
use crate::pick::{self, PickQuery, Pickable};

/// `"word"` as an FTS5 string (quotes doubled).
fn phrase(w: &str) -> String {
    format!("\"{}\"", w.replace('"', "\"\""))
}

/// One `AND`ed condition per indexable word: in the text (the index) or in
/// the id. Pushes the binds.
fn word_filter(
    words: &[String],
    fts: &str,
    rowid: &str,
    id: &str,
    binds: &mut Vec<String>,
) -> String {
    let mut sql = String::new();
    for w in words.iter().filter(|w| w.chars().count() >= 3) {
        binds.push(phrase(w));
        let n = binds.len();
        binds.push(w.clone());
        let m = binds.len();
        sql.push_str(&format!(
            " AND ({rowid} IN (SELECT rowid FROM {fts} WHERE {fts} MATCH ?{n}) \
             OR instr(lower({id}), ?{m}) > 0)"
        ));
    }
    sql
}

impl Store {
    pub fn pick_search(&self, q: &PickQuery) -> Result<Vec<Pickable>> {
        let words = pick::words(&q.text);
        let mut rows = Vec::new();
        let c = self.conn();
        if q.wants_mine() {
            let mut binds = Vec::new();
            let filter = word_filter(&words, "items_fts", "rid", "server_id", &mut binds);
            let mut st = c.prepare(&format!(
                "SELECT {ITEM_COLS} FROM items \
                 WHERE status = 'public' AND server_id IS NOT NULL{filter}"
            ))?;
            let items = st
                .query_map(params_from_iter(binds.iter()), row_to_item)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.extend(items.iter().filter_map(|it| pick::mine(it, q, &words)));
        }
        if q.wants_imported() {
            let mut binds = Vec::new();
            let sub = match &q.sub {
                Some(s) => {
                    binds.push(s.clone());
                    " AND subscription_id = ?1"
                }
                None => "",
            };
            let filter = word_filter(&words, "reading_fts", "rowid", "remote_id", &mut binds);
            let mut st = c.prepare(&format!(
                "SELECT json FROM reading WHERE sub_kind = 'blyg' \
                 AND (state = 'current' \
                      OR json_extract(json, '$.pinned_version_retained') IS NOT NULL){sub}{filter}"
            ))?;
            let found = st
                .query_map(params_from_iter(binds.iter()), |r| r.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .filter_map(|json| serde_json::from_str::<ReadingItem>(&json).ok())
                .filter_map(|r| pick::imported(&r, true, q, &words))
                .collect::<Vec<_>>();
            rows.extend(found);
        }
        Ok(pick::finish(rows, q))
    }
}
