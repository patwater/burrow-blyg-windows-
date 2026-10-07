//! --- reader folders --- The Reader's folders of subscriptions (schema v7).
//! Local only: nothing here is ever queued in the outbox or sent to the
//! blyg. Positions are dense (0, 1, 2, …) and rewritten on every reorder.

use std::collections::HashMap;

use rusqlite::{OptionalExtension, Transaction, params};

use super::Store;
use crate::backend::{CoreError, Result};
use crate::model::Folder;

/// A folder name as stored: trimmed, inner whitespace collapsed. Empty and
/// over-long names are refused.
fn clean_name(name: &str) -> Result<String> {
    let n = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if n.is_empty() {
        return Err(CoreError::Rejected {
            status: 400,
            message: "A folder needs a name".into(),
            details: vec![],
        });
    }
    if n.chars().count() > 80 {
        return Err(CoreError::Rejected {
            status: 400,
            message: "Folder names are at most 80 characters".into(),
            details: vec![],
        });
    }
    Ok(n)
}

/// Refuse a name another folder already has (case-insensitive).
fn check_unique(tx: &Transaction, name: &str, except: Option<&str>) -> Result<()> {
    let clash: Option<String> = tx
        .query_row(
            "SELECT id FROM folders WHERE lower(name) = lower(?1) AND id <> ?2",
            params![name, except.unwrap_or("")],
            |r| r.get(0),
        )
        .optional()?;
    match clash {
        Some(_) => Err(CoreError::Rejected {
            status: 409,
            message: format!("There's already a folder called “{name}”"),
            details: vec![],
        }),
        None => Ok(()),
    }
}

fn not_found() -> CoreError {
    CoreError::NotFound
}

/// Rewrite positions as 0..n in the given order.
fn renumber(tx: &Transaction, ids: &[String]) -> Result<()> {
    let mut st = tx.prepare_cached("UPDATE folders SET position = ?1 WHERE id = ?2")?;
    for (i, id) in ids.iter().enumerate() {
        st.execute(params![i as i64, id])?;
    }
    Ok(())
}

fn ordered_ids(tx: &Transaction) -> Result<Vec<String>> {
    let mut st = tx.prepare_cached("SELECT id FROM folders ORDER BY position, rowid")?;
    let ids = st
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

impl Store {
    /// Every folder, in display order.
    pub fn folders(&self) -> Vec<Folder> {
        let c = self.conn();
        let Ok(mut st) =
            c.prepare_cached("SELECT id, name, position FROM folders ORDER BY position, rowid")
        else {
            return vec![];
        };
        st.query_map([], |r| {
            Ok(Folder {
                id: r.get(0)?,
                name: r.get(1)?,
                position: r.get::<_, i64>(2)? as u32,
            })
        })
        .and_then(|it| it.collect())
        .unwrap_or_default()
    }

    /// Subscription id → folder id, for every filed subscription.
    pub fn subscription_folders(&self) -> HashMap<String, String> {
        let c = self.conn();
        let Ok(mut st) = c.prepare_cached("SELECT subscription_id, folder_id FROM folder_members")
        else {
            return HashMap::new();
        };
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .and_then(|it| it.collect())
            .unwrap_or_default()
    }

    /// A new folder at the end of the list.
    pub fn create_folder(&self, name: &str) -> Result<Folder> {
        let name = clean_name(name)?;
        let mut c = self.conn();
        let tx = c.transaction()?;
        check_unique(&tx, &name, None)?;
        let position: i64 =
            tx.query_row("SELECT COUNT(*) FROM folders", [], |r| r.get::<_, i64>(0))?;
        let id = format!("fld-{}", crate::util::new_local_id().0);
        tx.execute(
            "INSERT INTO folders (id, name, position) VALUES (?1, ?2, ?3)",
            params![id, name, position],
        )?;
        tx.commit()?;
        Ok(Folder {
            id,
            name,
            position: position as u32,
        })
    }

    pub fn rename_folder(&self, id: &str, name: &str) -> Result<()> {
        let name = clean_name(name)?;
        let mut c = self.conn();
        let tx = c.transaction()?;
        check_unique(&tx, &name, Some(id))?;
        if tx.execute(
            "UPDATE folders SET name = ?1 WHERE id = ?2",
            params![name, id],
        )? == 0
        {
            return Err(not_found());
        }
        tx.commit()?;
        Ok(())
    }

    /// Delete a folder; its subscriptions go back to unfiled (the
    /// membership rows cascade).
    pub fn delete_folder(&self, id: &str) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        // Explicit as well as cascaded, so it holds even on a connection
        // opened without `foreign_keys`.
        tx.execute(
            "DELETE FROM folder_members WHERE folder_id = ?1",
            params![id],
        )?;
        if tx.execute("DELETE FROM folders WHERE id = ?1", params![id])? == 0 {
            return Err(not_found());
        }
        let ids = ordered_ids(&tx)?;
        renumber(&tx, &ids)?;
        tx.commit()?;
        Ok(())
    }

    /// Move a folder to `index` in the display order (clamped to the end).
    pub fn move_folder(&self, id: &str, index: usize) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let mut ids = ordered_ids(&tx)?;
        let Some(from) = ids.iter().position(|f| f == id) else {
            return Err(not_found());
        };
        let f = ids.remove(from);
        ids.insert(index.min(ids.len()), f);
        renumber(&tx, &ids)?;
        tx.commit()?;
        Ok(())
    }

    /// File a subscription in a folder (`None` = unfiled). A subscription is
    /// in at most one folder, so this moves it.
    pub fn set_subscription_folder(&self, sub_id: &str, folder: Option<&str>) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        match folder {
            None => {
                tx.execute(
                    "DELETE FROM folder_members WHERE subscription_id = ?1",
                    params![sub_id],
                )?;
            }
            Some(fid) => {
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
                    params![fid],
                    |r| r.get(0),
                )?;
                if !exists {
                    return Err(not_found());
                }
                tx.execute(
                    "INSERT INTO folder_members (subscription_id, folder_id) VALUES (?1, ?2) \
                     ON CONFLICT(subscription_id) DO UPDATE SET folder_id = excluded.folder_id",
                    params![sub_id, fid],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}
