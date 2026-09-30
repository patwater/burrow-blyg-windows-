//! Locally tracked TK provenance (schema v3): one entry per scope of
//! `tk_prov_md`, pushed with the text by the combined call whenever the
//! server's copy may be keyed to different scopes (see `crate::tk`).

use rusqlite::{Connection, OptionalExtension, params};

use super::Store;
use crate::backend::Result;
use crate::model::{LocalId, ScopeProvenance};
use crate::tk;

/// What the store tracks for one item.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tracked {
    /// Keyed to `keyed_to`; `None` = not tracked (the server may hold some).
    pub scopes: Option<Vec<Option<ScopeProvenance>>>,
    pub keyed_to: Option<String>,
    /// The server needs the combined push.
    pub dirty: bool,
}

impl Tracked {
    pub fn any(&self) -> bool {
        self.scopes
            .as_ref()
            .is_some_and(|s| s.iter().any(Option::is_some))
    }
}

fn to_json(p: &[Option<ScopeProvenance>]) -> String {
    serde_json::to_string(p).unwrap_or_else(|_| "[]".into())
}

fn get_tx(tx: &Connection, id: &LocalId) -> rusqlite::Result<Tracked> {
    let row: Option<(Option<String>, Option<String>, bool)> = tx
        .query_row(
            "SELECT tk_prov, tk_prov_md, tk_prov_dirty FROM items WHERE local_id = ?1",
            [&id.0],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(match row {
        Some((json, md, dirty)) => Tracked {
            scopes: json.and_then(|j| serde_json::from_str(&j).ok()),
            keyed_to: md,
            dirty,
        },
        None => Tracked::default(),
    })
}

/// Replace the tracked provenance (validated by the caller) and mark it for push.
pub(super) fn set_tx(
    tx: &Connection,
    id: &LocalId,
    content: &str,
    p: &[Option<ScopeProvenance>],
) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE items SET tk_prov = ?2, tk_prov_md = ?3, tk_prov_dirty = 1 WHERE local_id = ?1",
        params![id.0, to_json(p), content],
    )?;
    Ok(())
}

/// The working copy is about to become `content`: carry tracked provenance
/// over to its scopes. Malformed text leaves it keyed to the old text until
/// the grammar is whole again.
pub(super) fn follow_tx(tx: &Connection, id: &LocalId, content: &str) -> rusqlite::Result<()> {
    let t = get_tx(tx, id)?;
    let (Some(old), Some(old_md)) = (t.scopes, t.keyed_to) else {
        return Ok(());
    };
    let Some(new) = tk::remap(&old_md, &old, content) else {
        return Ok(());
    };
    let dirty = t.dirty || new != old || tk::structure_changed(&old_md, content);
    tx.execute(
        "UPDATE items SET tk_prov = ?2, tk_prov_md = ?3, tk_prov_dirty = ?4 WHERE local_id = ?1",
        params![id.0, to_json(&new), content, dirty],
    )?;
    Ok(())
}

/// Stop tracking (the text came from elsewhere, or a restore cleared it).
pub(super) fn forget_tx(tx: &Connection, id: &LocalId) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE items SET tk_prov = NULL, tk_prov_md = NULL, tk_prov_dirty = 0 WHERE local_id = ?1",
        [&id.0],
    )?;
    Ok(())
}

pub(super) fn mark_dirty_tx(tx: &Connection, id: &LocalId) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE items SET tk_prov_dirty = 1 WHERE local_id = ?1 AND tk_prov IS NOT NULL",
        [&id.0],
    )?;
    Ok(())
}

impl Store {
    pub fn provenance(&self, id: &LocalId) -> Tracked {
        get_tx(&self.conn(), id).unwrap_or_default()
    }

    /// The combined push of `sent` + `scopes` succeeded. Tracking is updated
    /// only if the working copy is still `sent` (a newer edit re-pushes).
    pub fn prov_pushed(
        &self,
        id: &LocalId,
        sent: &str,
        scopes: &[Option<ScopeProvenance>],
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE items SET tk_prov = ?2, tk_prov_md = ?3, tk_prov_dirty = 0 \
             WHERE local_id = ?1 AND content_md = ?3",
            params![id.0, to_json(scopes), sent],
        )?;
        Ok(())
    }

    /// Start tracking provenance the server held for `keyed_to` (not dirty:
    /// the server has it already).
    pub fn adopt_prov(
        &self,
        id: &LocalId,
        keyed_to: &str,
        scopes: &[Option<ScopeProvenance>],
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE items SET tk_prov = ?2, tk_prov_md = ?3, tk_prov_dirty = 0 WHERE local_id = ?1",
            params![id.0, to_json(scopes), keyed_to],
        )?;
        Ok(())
    }

    /// The item just got a (new) server id: if it carries provenance, queue
    /// the combined push that records it there.
    pub fn queue_prov_push(&self, id: &LocalId) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        if get_tx(&tx, id)?.any() {
            mark_dirty_tx(&tx, id)?;
            Store::ensure_push(&tx, id, true)?;
        }
        tx.commit()?;
        Ok(())
    }
}
