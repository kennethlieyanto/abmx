use std::path::Path;

use abmx_core::{Error, Result, Transaction, ABMX_SCHEMA_VERSION};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// What the run remembers between invocations.
///
/// SQLite rather than a JSON blob for three reasons: the cursor must survive a
/// crash mid-write, the review queue needs to be queryable, and "have I already
/// delivered this message" is a lookup, not a linear scan.
///
/// Note what is *not* stored: which transactions the sink accepted. Sinks that
/// answer `idempotent() == true` are their own source of truth. `messages` only
/// records what `abmx` itself delivered, and only for messages whose rows all
/// succeeded.
pub struct State {
    conn: Connection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewEntry {
    pub tx_id: String,
    /// Source message identity, so approving a row can mark it delivered.
    pub message: String,
    pub reason: String,
    pub transaction: Transaction,
    pub created_at: String,
}

impl State {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| Error::io(parent, error))?;
        }

        let conn = Connection::open(path)
            .map_err(|error| Error::State(format!("opening {}: {error}", path.display())))?;
        let state = Self { conn };
        state.migrate()?;
        Ok(state)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|error| Error::State(format!("opening in-memory db: {error}")))?;
        let state = Self { conn };
        state.migrate()?;
        Ok(state)
    }

    fn migrate(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = NORMAL;
                 CREATE TABLE IF NOT EXISTS meta (
                     key   TEXT PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS cursors (
                     name       TEXT PRIMARY KEY,
                     value      TEXT NOT NULL,
                     updated_at TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS messages (
                     key         TEXT PRIMARY KEY,
                     sink        TEXT NOT NULL,
                     tx_id       TEXT NOT NULL,
                     disposition TEXT NOT NULL,
                     seen_at     TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS review (
                     tx_id       TEXT PRIMARY KEY,
                     message     TEXT NOT NULL,
                     reason      TEXT NOT NULL,
                     payload     TEXT NOT NULL,
                     created_at  TEXT NOT NULL
                 );",
            )
            .map_err(state_error)
    }

    /// Refuse to run against a state file written by a build that expects a
    /// different `abmx-parser` output contract.
    pub fn check_schema(&self) -> Result<()> {
        const KEY: &str = "abmx_schema_version";
        let expected = ABMX_SCHEMA_VERSION.to_string();

        match self.meta_get(KEY)? {
            None => self.meta_set(KEY, &expected),
            Some(found) if found == expected => Ok(()),
            Some(found) => Err(Error::State(format!(
                "state was written against parser schema {found}, this build expects {expected}; \
                 the transaction contract changed, so old rows cannot be reinterpreted"
            ))),
        }
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(state_error)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(state_error)?;
        Ok(())
    }

    pub fn cursor(&self, name: &str) -> Result<Option<serde_json::Value>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM cursors WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()
            .map_err(state_error)?;

        match raw {
            Some(raw) => serde_json::from_str(&raw).map(Some).map_err(Into::into),
            None => Ok(None),
        }
    }

    pub fn set_cursor(&self, name: &str, value: &serde_json::Value) -> Result<()> {
        let encoded = serde_json::to_string(value)?;
        self.conn
            .execute(
                "INSERT INTO cursors (name, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![name, encoded, now()],
            )
            .map_err(state_error)?;
        Ok(())
    }

    pub fn clear_cursor(&self, name: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM cursors WHERE name = ?1", params![name])
            .map_err(state_error)?;
        Ok(())
    }

    pub fn is_seen(&self, key: &str) -> Result<bool> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM messages WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(state_error)?;
        Ok(found.is_some())
    }

    pub fn mark_seen(&self, key: &str, sink: &str, tx_id: &str, disposition: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO messages (key, sink, tx_id, disposition, seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(key) DO UPDATE SET
                     sink = excluded.sink,
                     tx_id = excluded.tx_id,
                     disposition = excluded.disposition,
                     seen_at = excluded.seen_at",
                params![key, sink, tx_id, disposition, now()],
            )
            .map_err(state_error)?;
        Ok(())
    }

    pub fn upsert_review(&self, entry: &ReviewEntry) -> Result<()> {
        let transaction = serde_json::to_string(&entry.transaction)?;
        self.conn
            .execute(
                "INSERT INTO review (tx_id, message, reason, payload, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(tx_id) DO UPDATE SET
                     message = excluded.message,
                     reason = excluded.reason,
                     payload = excluded.payload",
                params![
                    entry.tx_id,
                    entry.message,
                    entry.reason,
                    transaction,
                    entry.created_at
                ],
            )
            .map_err(state_error)?;
        Ok(())
    }

    pub fn review(&self) -> Result<Vec<ReviewEntry>> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT tx_id, message, reason, payload, created_at
                 FROM review ORDER BY created_at DESC, tx_id",
            )
            .map_err(state_error)?;

        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(state_error)?;

        let mut entries = Vec::new();
        for row in rows {
            let (tx_id, message, reason, transaction, created_at) = row.map_err(state_error)?;
            entries.push(ReviewEntry {
                tx_id,
                message,
                reason,
                transaction: serde_json::from_str(&transaction)?,
                created_at,
            });
        }

        Ok(entries)
    }

    pub fn review_entry(&self, tx_id: &str) -> Result<Option<ReviewEntry>> {
        let row: Option<(String, String, String, String, String)> = self
            .conn
            .query_row(
                "SELECT tx_id, message, reason, payload, created_at FROM review WHERE tx_id = ?1",
                params![tx_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()
            .map_err(state_error)?;

        let Some((tx_id, message, reason, transaction, created_at)) = row else {
            return Ok(None);
        };

        Ok(Some(ReviewEntry {
            tx_id,
            message,
            reason,
            transaction: serde_json::from_str(&transaction)?,
            created_at,
        }))
    }

    pub fn clear_review(&self, tx_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM review WHERE tx_id = ?1", params![tx_id])
            .map_err(state_error)?;
        Ok(())
    }

    /// `(cursors, delivered messages, review rows)`.
    pub fn counts(&self) -> Result<(usize, usize, usize)> {
        let cursors: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM cursors", [], |row| row.get(0))
            .map_err(state_error)?;
        let messages: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .map_err(state_error)?;
        let reviews: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM review", [], |row| row.get(0))
            .map_err(state_error)?;
        Ok((cursors as usize, messages as usize, reviews as usize))
    }

    /// Forget the cursor and what has been delivered. Leaves `meta` alone.
    pub fn reset_progress(&self) -> Result<()> {
        self.conn
            .execute_batch("DELETE FROM cursors; DELETE FROM messages;")
            .map_err(state_error)
    }

    pub fn clear_reviews(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM review", [])
            .map_err(state_error)?;
        Ok(())
    }

    pub fn seen_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .map_err(state_error)?;
        Ok(count as usize)
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn state_error(error: rusqlite::Error) -> Error {
    Error::State(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_clears_progress_but_keeps_the_schema_marker() {
        let state = State::open_in_memory().unwrap();
        state.check_schema().unwrap();
        state
            .set_cursor("gmail", &serde_json::json!({ "history_id": "1" }))
            .unwrap();
        state
            .mark_seen("gmail:1", "actual", "mail_1", "imported")
            .unwrap();

        assert_eq!(state.counts().unwrap(), (1, 1, 0));

        state.reset_progress().unwrap();
        assert_eq!(state.counts().unwrap(), (0, 0, 0));
        assert!(state.cursor("gmail").unwrap().is_none());
        assert!(!state.is_seen("gmail:1").unwrap());

        // The schema guard survives a reset; it is not progress.
        assert_eq!(
            state.meta_get("abmx_schema_version").unwrap(),
            Some(ABMX_SCHEMA_VERSION.to_string())
        );
    }

    #[test]
    fn clearing_reviews_leaves_progress_alone() {
        let state = State::open_in_memory().unwrap();
        state
            .mark_seen("gmail:1", "actual", "mail_1", "imported")
            .unwrap();

        state.clear_reviews().unwrap();
        assert_eq!(state.counts().unwrap(), (0, 1, 0));
    }
}
