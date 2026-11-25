//! The guard's adversarial corpus. (SAFE-010)
//!
//! Every way we know of to sneak a change past a read-only check belongs
//! here. When a new trick is found, add it to the right list first, watch
//! the test fail, then fix the guard.

use gridpnt::guard::{Kind, classify};

fn check(cases: &[&str], expected: Kind) {
    for sql in cases {
        let verdict = classify(sql);
        assert_eq!(verdict.kind, expected, "\n  sql: {sql}\n  reason: {}", verdict.reason);
    }
}

/// Run `cases` and require that none is allowed in read-only mode,
/// whatever kind the guard assigns.
fn check_refused(cases: &[&str]) {
    for sql in cases {
        let verdict = classify(sql);
        assert!(!verdict.allowed_read_only(), "\n  allowed but must be refused: {sql}");
    }
}

#[test]
fn plain_reads_are_allowed() {
    check(
        &[
            "SELECT 1",
            "select * from t",
            "SELECT * FROM t;",
            "  \n\t SELECT * FROM t ; \n ",
            "SELECT * FROM t; ;",
            "VALUES (1), (2)",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "(SELECT 1)",
            "SELECT a FROM t WHERE b = 'it''s'",
            "SELECT \"weird \"\"name\"\"\" FROM t",
            "SELECT `back``tick` FROM t",
            "SELECT [my column] FROM t",
            "SELECT arr[1] FROM t",
            "SELECT $1, $name",
            "EXPLAIN SELECT * FROM t",
            "EXPLAIN QUERY PLAN SELECT * FROM t",
            "EXPLAIN ANALYZE SELECT * FROM t",
            "SHOW TABLES",
            "SHOW CREATE TABLE t",
            "DESCRIBE t",
            "-- leading comment\nSELECT 1",
            "/* leading */ SELECT 1 /* trailing */",
            "SELECT /*+ BROADCAST(t) */ * FROM t",
            "SELECT 1 -- trailing comment",
        ],
        Kind::Read,
    );
}

#[test]
fn change_words_inside_strings_names_and_comments_do_not_count() {
    check(
        &[
            "SELECT 'DROP TABLE t'",
            "SELECT 'a; DELETE FROM t'",
            "SELECT \"delete\" FROM t",
            "SELECT `update` FROM t",
            "SELECT [insert] FROM t",
            "SELECT 1 -- ; DROP TABLE t",
            "SELECT 1 /* ; DROP TABLE t */",
            "SELECT replace(name, 'a', 'b') FROM t",
            "SELECT updated_at, deleted, created_by FROM t",
        ],
        Kind::Read,
    );
}

#[test]
fn writes_are_recognised() {
    check(
        &[
            "INSERT INTO t VALUES (1)",
            "insert into t values (1)",
            "UPDATE t SET a = 1",
            "DELETE FROM t",
            "REPLACE INTO t VALUES (1)",
            "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN DELETE",
            "/* hi */ DELETE FROM t",
            "-- hi\nDELETE FROM t",
            "EXPLAIN DELETE FROM t",
            "EXPLAIN ANALYZE DELETE FROM t",
        ],
        Kind::Write,
    );
}

#[test]
fn writes_hidden_inside_reads_are_recognised() {
    check(
        &[
            "WITH x AS (SELECT 1) DELETE FROM t",
            "WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x",
            "WITH gone AS (DELETE FROM t RETURNING *) SELECT * FROM gone",
            "SELECT * INTO copy FROM t",
            "SELECT * FROM t INTO OUTFILE '/tmp/x'",
            "SELECT * FROM t FOR UPDATE",
            "SELECT 1 REPLACE INTO t VALUES (1)",
            // SQL Server accepts a second statement with no semicolon.
            "SELECT 1 DROP TABLE t",
            "SELECT 1 EXEC sp_who",
            "(((DELETE FROM t)))",
            "SELECT 1 UNION SELECT 2 /* x */ DELETE FROM t",
        ],
        Kind::Write,
    );
}

#[test]
fn structure_changes_are_recognised() {
    check(
        &[
            "CREATE TABLE t (a int)",
            "DROP TABLE t",
            "ALTER TABLE t ADD COLUMN b int",
            "TRUNCATE TABLE t",
            "GRANT SELECT ON t TO someone",
            "CREATE TABLE copy AS SELECT * FROM t",
        ],
        Kind::Ddl,
    );
}

#[test]
fn session_changes_are_recognised() {
    check(
        &[
            "BEGIN",
            "COMMIT",
            "ROLLBACK",
            "SET search_path = x",
            "USE other",
            "PRAGMA journal_mode = DELETE",
            "PRAGMA query_only = OFF",
            "PRAGMA writable_schema = ON",
            "ATTACH DATABASE 'x.db' AS x",
            "VACUUM",
            "EXPLAIN (ANALYZE, BUFFERS) DELETE FROM t",
        ],
        Kind::Session,
    );
}

#[test]
fn more_than_one_statement_is_refused() {
    check(
        &[
            "SELECT 1; SELECT 2",
            "SELECT 1; DROP TABLE t",
            // The escape from a read-only transaction.
            "COMMIT; DROP TABLE t",
            "SELECT 1; COMMIT; DELETE FROM t; BEGIN",
            "SELECT 1;\n-- comment\nDELETE FROM t",
            "SELECT 'a'; DELETE FROM t; --'",
        ],
        Kind::Multiple,
    );
}

#[test]
fn nothing_to_run_is_empty() {
    check(
        &["", "   ", ";", ";;;", "-- only a comment", "/* only a comment */"],
        Kind::Empty,
    );
}

#[test]
fn unrecognised_statements_are_unknown() {
    check(
        &[
            "CALL do_something()",
            "EXEC sp_who",
            "DO $x$ BEGIN END $x$",
            "FROBNICATE everything",
            "1 + 1",
            "'SELECT 1'",
            "EXPLAIN",
        ],
        Kind::Unknown,
    );
}

/// Text that two databases would read differently. One would see a harmless
/// string or comment; another would see a second statement.
#[test]
fn text_that_databases_read_differently_is_unknown() {
    check(
        &[
            // Backslash-quote: escaped quote in some, end of string in others.
            r"SELECT 'a\'; DROP TABLE t; --'",
            r#"SELECT "a\"; DROP TABLE t; --""#,
            // Nested comment: ends at the first `*/` in some, the second in others.
            "SELECT 1 /* /* */ ; DROP TABLE t; -- */",
            // MySQL runs the inside of this comment.
            "SELECT 1 /*! ; DROP TABLE t */",
            "/*!50000 DROP TABLE t */",
            // `[...]` is a name in some and an array index in others.
            "SELECT [']; DROP TABLE t; --']",
            "SELECT [a; DROP TABLE t; --]",
            // Dollar-quoted string in PostgreSQL only.
            "SELECT $$ ' $$; DROP TABLE t; --'",
            "SELECT $tag$ ' $tag$; DROP TABLE t; --'",
            // `#` starts a comment in MySQL only.
            "SELECT 1 # '\n; DROP TABLE t; -- '",
        ],
        Kind::Unknown,
    );
}

#[test]
fn malformed_text_is_unknown() {
    check(
        &[
            "SELECT 'never closed",
            "SELECT \"never closed",
            "SELECT `never closed",
            "SELECT [never closed",
            "SELECT 1 /* never closed",
            "SELECT 1\0; DROP TABLE t",
            "SELECT 1 \u{1b}[2J",
        ],
        Kind::Unknown,
    );
}

/// Whatever kind these get, they must never run in read-only mode.
#[test]
fn nothing_but_reads_is_allowed() {
    check_refused(&[
        "DELETE FROM t",
        "SELECT 1; DELETE FROM t",
        "WITH x AS (SELECT 1) DELETE FROM t",
        "PRAGMA query_only = OFF",
        "COMMIT",
        "",
        "CALL p()",
        r"SELECT 'a\'; DROP TABLE t; --'",
        "SeLeCt 1 ; dElEtE fRoM t",
        "SELECT 1\u{2028}; DROP TABLE t",
        "ＤＲＯＰ TABLE t",
    ]);
}
