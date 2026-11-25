//! The SQLite backend against a real database file.

mod common;

use common::TempDb;
use gridpnt::backend::sqlite::Sqlite;
use gridpnt::backend::{Backend, NodeKind, Value};
use gridpnt::exec;

fn row_count(db: &TempDb, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(&db.path).unwrap();
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn opening_a_missing_file_fails_and_does_not_create_it() {
    let path = std::env::temp_dir().join("gridpnt-test-does-not-exist.db");
    assert!(Sqlite::open(&path).is_err());
    assert!(!path.exists());
}

#[test]
fn opening_a_file_that_is_not_a_database_fails() {
    let path = std::env::temp_dir().join(format!("gridpnt-test-not-a-db-{}.txt", std::process::id()));
    std::fs::write(&path, "this is not a database, just some text that is long enough").unwrap();
    let outcome = Sqlite::open(&path);
    std::fs::remove_file(&path).unwrap();
    assert!(outcome.is_err());
}

#[test]
fn catalog_lists_tables_views_and_columns() {
    let db = TempDb::new(0);
    let mut sqlite = Sqlite::open(&db.path).unwrap();

    let top = sqlite.children(&[]).unwrap();
    let names: Vec<(&str, NodeKind)> = top.iter().map(|n| (n.name.as_str(), n.kind)).collect();
    assert_eq!(
        names,
        [
            ("adults", NodeKind::View),
            ("numbers", NodeKind::Table),
            ("people", NodeKind::Table)
        ]
    );

    let columns = sqlite.children(&["people".to_string()]).unwrap();
    let described: Vec<(&str, &str)> = columns.iter().map(|n| (n.name.as_str(), n.detail.as_str())).collect();
    assert_eq!(
        described,
        [
            ("id", "integer pk"),
            ("name", "text not null"),
            ("age", "integer"),
            ("score", "real"),
            ("photo", "blob")
        ]
    );
}

#[test]
fn query_returns_typed_values() {
    let db = TempDb::new(0);
    let mut sqlite = Sqlite::open(&db.path).unwrap();

    let result = exec::run(&mut sqlite, "SELECT id, name, score, photo FROM people ORDER BY id").unwrap();
    assert_eq!(result.columns, ["id", "name", "score", "photo"]);
    assert!(!result.truncated);
    assert_eq!(
        result.rows[0],
        [
            Value::Integer(1),
            Value::Text("Ada".into()),
            Value::Real(9.5),
            Value::Blob(2)
        ]
    );
    assert_eq!(result.rows[1][2], Value::Null);
}

#[test]
fn preview_quotes_awkward_names() {
    let db = TempDb::new(0);
    let sqlite = Sqlite::open(&db.path).unwrap();
    let sql = sqlite.preview_sql(&["we\"ird; DROP TABLE people".to_string()]).unwrap();
    assert_eq!(sql, "SELECT * FROM \"we\"\"ird; DROP TABLE people\"");
}

#[test]
fn results_are_capped_and_say_so() {
    let db = TempDb::new(exec::ROW_CAP + 5);
    let mut sqlite = Sqlite::open(&db.path).unwrap();

    let capped = exec::run(&mut sqlite, "SELECT n FROM numbers").unwrap();
    assert_eq!(capped.rows.len(), exec::ROW_CAP);
    assert!(capped.truncated);

    // Exactly at the cap is not truncation.
    let exact = exec::run(&mut sqlite, &format!("SELECT n FROM numbers LIMIT {}", exec::ROW_CAP)).unwrap();
    assert_eq!(exact.rows.len(), exec::ROW_CAP);
    assert!(!exact.truncated);
}

#[test]
fn the_guard_refuses_writes_before_they_reach_sqlite() {
    let db = TempDb::new(0);
    let mut sqlite = Sqlite::open(&db.path).unwrap();

    for sql in [
        "DELETE FROM people",
        "DROP TABLE people",
        "SELECT 1; DELETE FROM people",
        "PRAGMA query_only = OFF",
        "WITH x AS (SELECT 1) DELETE FROM people",
    ] {
        let error = exec::run(&mut sqlite, sql).unwrap_err();
        assert!(error.starts_with("refused (read-only)"), "{sql}: {error}");
    }
    assert_eq!(row_count(&db, "people"), 3);
}

/// If the guard were ever wrong, SQLite's own read-only checks must still
/// hold. This calls the backend directly, skipping the guard on purpose.
#[test]
fn sqlite_itself_refuses_writes_without_the_guard() {
    let db = TempDb::new(0);
    let mut sqlite = Sqlite::open(&db.path).unwrap();

    for sql in [
        "DELETE FROM people",
        "INSERT INTO people (name) VALUES ('x')",
        "DROP TABLE people",
        "CREATE TABLE extra (a)",
        "PRAGMA query_only = OFF",
        "PRAGMA journal_mode = DELETE",
        "VACUUM",
        "ATTACH DATABASE ':memory:' AS other",
    ] {
        assert!(sqlite.query(sql, 10).is_err(), "sqlite ran: {sql}");
    }

    // Turning query_only off must not have worked either.
    assert!(sqlite.query("DELETE FROM people", 10).is_err());
    assert_eq!(row_count(&db, "people"), 3);
}

#[test]
fn a_running_query_can_be_cancelled() {
    let db = TempDb::new(0);
    let mut sqlite = Sqlite::open(&db.path).unwrap();
    let cancel = sqlite.canceller();

    let canceller = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        cancel();
    });
    // Counts forever unless interrupted.
    let endless = "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n) SELECT count(*) FROM n";
    let outcome = exec::run(&mut sqlite, endless);
    canceller.join().unwrap();

    assert_eq!(outcome.unwrap_err(), "cancelled");
    // The connection is still usable afterwards.
    assert!(exec::run(&mut sqlite, "SELECT 1").is_ok());
}
