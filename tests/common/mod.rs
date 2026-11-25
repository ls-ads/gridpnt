//! Shared test helper: a small SQLite database in a temporary file.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A database file that is deleted when the value is dropped.
pub struct TempDb {
    pub path: PathBuf,
}

impl TempDb {
    /// Tables: `people` (3 rows), `numbers` (`rows` rows), view `adults`.
    pub fn new(rows: usize) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("gridpnt-test-{}-{unique}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER, score REAL, photo BLOB);
             INSERT INTO people VALUES (1, 'Ada', 36, 9.5, x'0102'), (2, 'Grace', 85, NULL, NULL), (3, 'Li\nnus', 17, 7.0, NULL);
             CREATE VIEW adults AS SELECT * FROM people WHERE age >= 18;
             CREATE TABLE numbers (n INTEGER);",
        )
        .unwrap();
        conn.execute(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < ?1)
             INSERT INTO numbers SELECT n FROM seq WHERE ?1 > 0",
            [rows as i64],
        )
        .unwrap();
        TempDb { path }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
