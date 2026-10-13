pub mod commands;
pub mod migrations;
pub mod models;

use rusqlite::Connection;
use std::sync::Mutex;

pub struct Database {
    pub conn: Mutex<Connection>,
}

impl Database {
    pub fn new() -> Result<Self, String> {
        let dir = dirs::data_dir()
            .ok_or("Cannot determine data directory")?
            .join("com.ai-quick-input.assistant");

        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

        let db_path = dir.join("history.db");
        let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;

        // Enable WAL mode for better concurrent read performance
        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| e.to_string())?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn run_migrations(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        migrations::run(&conn)
    }

    pub fn purge_expired(&self) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        purge_expired(&conn, chrono::Utc::now()).map_err(|e| e.to_string())
    }
}

pub(super) fn purge_expired(
    conn: &Connection,
    now: chrono::DateTime<chrono::Utc>,
) -> rusqlite::Result<usize> {
    let cutoff = (now - chrono::Duration::days(5)).to_rfc3339();
    // SQLite normalizes UTC offsets and legacy date formats. Unparseable
    // dates remain intact rather than risking deletion of unknown records.
    conn.execute(
        "DELETE FROM history WHERE julianday(created_at) < julianday(?1)",
        [cutoff],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_deletes_only_records_older_than_five_days() {
        let conn = Connection::open_in_memory().unwrap();
        migrations::run(&conn).unwrap();
        let now = "2026-10-03T12:00:00Z".parse().unwrap();
        for (id, date) in [
            ("expired", "2026-09-28T11:59:59Z"),
            ("legacy", "2026-09-27 12:00:00"),
            ("boundary", "2026-09-28T12:00:00Z"),
            ("offset", "2026-09-28T20:00:00+08:00"),
            ("recent", "2026-10-03T11:59:00Z"),
            ("unknown", "unknown date"),
        ] {
            conn.execute(
                "INSERT INTO history (id, created_at) VALUES (?1, ?2)",
                [id, date],
            )
            .unwrap();
        }
        assert_eq!(purge_expired(&conn, now).unwrap(), 2);
        let kept: Vec<String> = conn
            .prepare("SELECT id FROM history ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(kept, ["boundary", "offset", "recent", "unknown"]);
        assert_eq!(purge_expired(&conn, now).unwrap(), 0);
    }
}
