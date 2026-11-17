use std::path::Path;

use abmx_core::{Error, Result, ABMX_SCHEMA_VERSION};
use rusqlite::{params, Connection, OptionalExtension};

/// What the run remembers between invocations.
///
/// SQLite rather than a JSON blob because "have I already delivered this
/// message" is a lookup, not a linear scan.
///
/// Note what is *not* stored: which transactions the sink accepted. Sinks that
/// answer `idempotent() == true` are their own source of truth. `messages` only
/// records what `abmx` itself delivered, and only for messages whose rows all
/// succeeded.
pub struct State {
    conn: Connection,
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
                 CREATE TABLE IF NOT EXISTS messages (
                     key         TEXT PRIMARY KEY,
                     sink        TEXT NOT NULL,
                     tx_id       TEXT NOT NULL,
                     disposition TEXT NOT NULL,
                     seen_at     TEXT NOT NULL
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

    /// Forget what has been delivered. Leaves `meta` alone.
    pub fn reset_progress(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM messages", [])
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
            .mark_seen("gmail:1", "actual", "mail_1", "imported")
            .unwrap();

        assert_eq!(state.seen_count().unwrap(), 1);

        state.reset_progress().unwrap();
        assert_eq!(state.seen_count().unwrap(), 0);
        assert!(!state.is_seen("gmail:1").unwrap());

        // The schema guard survives a reset; it is not progress.
        assert_eq!(
            state.meta_get("abmx_schema_version").unwrap(),
            Some(ABMX_SCHEMA_VERSION.to_string())
        );
    }
}
