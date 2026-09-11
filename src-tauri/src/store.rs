//! SQLite. Migrations are numbered SQL files applied in order, tracked with
//! `PRAGMA user_version`. No ORM, no query builder.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::error::Result;

const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql")];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub trusted: bool,
    pub last_opened_at: String,
}

pub struct Store(Mutex<Connection>);

impl Store {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(dir) = db_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&conn)?;
        Ok(Store(Mutex::new(conn)))
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        migrate(&conn)?;
        Ok(Store(Mutex::new(conn)))
    }

    /// Insert or refresh a project and stamp it as just opened.
    pub fn touch_project(&self, path: &str, name: &str) -> Result<Project> {
        let conn = self.0.lock().expect("store poisoned");
        conn.execute(
            "INSERT INTO projects (path, name, last_opened_at, opened_seq)
             VALUES (?1, ?2, datetime('now'),
                     (SELECT IFNULL(MAX(opened_seq), 0) + 1 FROM projects))
             ON CONFLICT(path) DO UPDATE SET
               name = excluded.name,
               last_opened_at = excluded.last_opened_at,
               opened_seq = (SELECT MAX(opened_seq) + 1 FROM projects)",
            params![path, name],
        )?;
        read_project(&conn, path)
    }

    pub fn set_trusted(&self, path: &str, trusted: bool) -> Result<()> {
        let conn = self.0.lock().expect("store poisoned");
        conn.execute(
            "UPDATE projects
                SET trusted = ?2, trust_scanned_at = datetime('now')
              WHERE path = ?1",
            params![path, trusted],
        )?;
        Ok(())
    }

    pub fn recent_projects(&self, limit: u32) -> Result<Vec<Project>> {
        let conn = self.0.lock().expect("store poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, path, name, trusted, last_opened_at
               FROM projects ORDER BY opened_seq DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], row_to_project)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn forget_project(&self, path: &str) -> Result<()> {
        let conn = self.0.lock().expect("store poisoned");
        conn.execute("DELETE FROM projects WHERE path = ?1", params![path])?;
        Ok(())
    }
}

fn read_project(conn: &Connection, path: &str) -> Result<Project> {
    Ok(conn.query_row(
        "SELECT id, path, name, trusted, last_opened_at FROM projects WHERE path = ?1",
        params![path],
        row_to_project,
    )?)
}

fn row_to_project(row: &rusqlite::Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        path: row.get(1)?,
        name: row.get(2)?,
        trusted: row.get::<_, i64>(3)? != 0,
        last_opened_at: row.get(4)?,
    })
}

fn migrate(conn: &Connection) -> Result<()> {
    let applied: u32 =
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(applied as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent() {
        let store = Store::in_memory().unwrap();
        let conn = store.0.lock().unwrap();
        let before: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(before, MIGRATIONS.len() as u32);
        // Re-running must be a no-op, not a "table already exists" error.
        migrate(&conn).unwrap();
    }

    #[test]
    fn failed_migration_rolls_back_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE blocker (id); CREATE INDEX projects_recent ON blocker(id);").unwrap();
        assert!(migrate(&conn).is_err());
        let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'projects'", [], |r| r.get(0)).unwrap();
        assert_eq!(tables, 0);
        let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, 0);
    }

    #[test]
    fn repeated_opens_order_three_projects() {
        let store = Store::in_memory().unwrap();
        for name in ["a", "b", "c", "b", "a", "c", "a"] {
            store.touch_project(name, name).unwrap();
        }
        let paths: Vec<_> = store.recent_projects(10).unwrap().into_iter().map(|p| p.path).collect();
        assert_eq!(paths, ["a", "c", "b"]);
    }

    #[test]
    fn recents_are_newest_first_and_paths_are_unique() {
        let store = Store::in_memory().unwrap();
        store.touch_project("C:/a", "a").unwrap();
        store.touch_project("C:/b", "b").unwrap();
        // Same path twice must update, not duplicate.
        store.touch_project("C:/a", "a-renamed").unwrap();

        let recents = store.recent_projects(10).unwrap();
        assert_eq!(recents.len(), 2);
        assert_eq!(recents[0].path, "C:/a");
        assert_eq!(recents[0].name, "a-renamed");
    }

    #[test]
    fn trust_is_remembered() {
        let store = Store::in_memory().unwrap();
        let p = store.touch_project("C:/a", "a").unwrap();
        assert!(!p.trusted, "a new project starts untrusted");

        store.set_trusted("C:/a", true).unwrap();
        assert!(store.recent_projects(1).unwrap()[0].trusted);
    }
}
