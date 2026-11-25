//! SQLite backend.
//!
//! Read-only is enforced by SQLite itself, independent of the guard:
//!
//! 1. the file is opened read-only;
//! 2. the connection is set to `query_only`;
//! 3. an authorizer denies every action except reading, so statements such
//!    as `PRAGMA query_only = OFF` or `ATTACH` cannot even be prepared;
//! 4. each prepared statement is checked with SQLite's own "is this
//!    read-only?" test before it runs.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};

use super::{Backend, Cancel, CatalogNode, Limits, NodeKind, Result, ResultSet, Value};

pub struct Sqlite {
    conn: Connection,
    label: String,
    /// When the running query must stop. SQLite checks this as it works.
    deadline: Arc<Mutex<Option<Instant>>>,
}

impl Sqlite {
    /// Open an existing database file. Never creates one.
    pub fn open(path: &Path) -> Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut conn =
            Connection::open_with_flags(path, flags).map_err(|e| format!("cannot open {}: {e}", path.display()))?;

        conn.pragma_update(None, "query_only", true)
            .map_err(|e| format!("cannot set read-only mode: {e}"))?;

        // SQLite opens lazily; reading the schema is what proves the file
        // really is a database.
        conn.query_row("SELECT count(*) FROM sqlite_schema", [], |_| Ok(()))
            .map_err(|e| format!("{} is not a readable SQLite database: {e}", path.display()))?;

        // Installed last, because it would deny the setup above.
        conn.authorizer(Some(allow_reads_only))
            .map_err(|e| format!("cannot install read-only checks: {e}"))?;

        // SQLite calls this every few thousand steps of a query; returning
        // true stops the query. This is how the timeout is enforced.
        let deadline: Arc<Mutex<Option<Instant>>> = Arc::default();
        let watched = Arc::clone(&deadline);
        conn.progress_handler(
            10_000,
            Some(move || watched.lock().unwrap().is_some_and(|at| Instant::now() >= at)),
        )
        .map_err(|e| format!("cannot install the query timeout: {e}"))?;

        let label = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        Ok(Sqlite { conn, label, deadline })
    }

    fn tables_and_views(&self) -> rusqlite::Result<Vec<CatalogNode>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, type FROM sqlite_schema
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
             ORDER BY name",
        )?;
        let nodes = stmt.query_map([], |row| {
            let kind: String = row.get(1)?;
            Ok(CatalogNode {
                name: row.get(0)?,
                kind: if kind == "view" {
                    NodeKind::View
                } else {
                    NodeKind::Table
                },
                detail: String::new(),
                has_children: true,
            })
        })?;
        nodes.collect()
    }

    fn columns(&self, table: &str) -> rusqlite::Result<Vec<CatalogNode>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1)")?;
        let nodes = stmt.query_map([table], |row| {
            let data_type: String = row.get(1)?;
            let not_null: bool = row.get(2)?;
            let primary_key_position: i64 = row.get(3)?;

            let mut detail = data_type.to_lowercase();
            if primary_key_position > 0 {
                detail.push_str(" pk");
            }
            if not_null {
                detail.push_str(" not null");
            }
            Ok(CatalogNode {
                name: row.get(0)?,
                kind: NodeKind::Column,
                detail: detail.trim().to_string(),
                has_children: false,
            })
        })?;
        nodes.collect()
    }
}

impl Backend for Sqlite {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn children(&mut self, path: &[String]) -> Result<Vec<CatalogNode>> {
        let nodes = match path {
            [] => self.tables_and_views(),
            [table] => self.columns(table),
            _ => Ok(Vec::new()),
        };
        nodes.map_err(|e| e.to_string())
    }

    fn preview_sql(&self, path: &[String]) -> Option<String> {
        match path {
            [table] => Some(format!("SELECT * FROM {}", quote_name(table))),
            _ => None,
        }
    }

    fn query(&mut self, sql: &str, limits: &Limits) -> Result<ResultSet> {
        let deadline = Instant::now() + limits.timeout;
        *self.deadline.lock().unwrap() = Some(deadline);
        let outcome = self.query_until_done(sql, limits.row_cap);
        *self.deadline.lock().unwrap() = None;

        // A timeout stops the query the same way a cancel does; the clock
        // tells the two apart.
        match outcome {
            Err(error) if error == "cancelled" && Instant::now() >= deadline => {
                Err(format!("timed out after {} s", limits.timeout.as_secs()))
            }
            other => other,
        }
    }

    fn canceller(&self) -> Cancel {
        let handle = self.conn.get_interrupt_handle();
        Box::new(move || handle.interrupt())
    }
}

impl Sqlite {
    fn query_until_done(&mut self, sql: &str, row_cap: usize) -> Result<ResultSet> {
        let mut stmt = self.conn.prepare(sql).map_err(readable)?;

        if !stmt.readonly() {
            return Err("refused: SQLite reports this statement would write".into());
        }

        let columns: Vec<String> = stmt.column_names().map(str::to_string).collect();
        let mut result = ResultSet {
            columns,
            ..ResultSet::default()
        };

        let mut rows = stmt.query([]).map_err(readable)?;
        while let Some(row) = rows.next().map_err(readable)? {
            if result.rows.len() == row_cap {
                result.truncated = true;
                break;
            }
            let mut cells = Vec::with_capacity(result.columns.len());
            for i in 0..result.columns.len() {
                cells.push(match row.get_ref(i).map_err(readable)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => Value::Integer(n),
                    ValueRef::Real(x) => Value::Real(x),
                    ValueRef::Text(bytes) => Value::Text(String::from_utf8_lossy(bytes).into_owned()),
                    ValueRef::Blob(bytes) => Value::Blob(bytes.len()),
                });
            }
            result.rows.push(cells);
        }
        Ok(result)
    }
}

/// Called by SQLite for each thing a statement wants to do, while the
/// statement is being prepared. Anything not listed here is denied.
fn allow_reads_only(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Select | AuthAction::Read { .. } | AuthAction::Function { .. } | AuthAction::Recursive => {
            Authorization::Allow
        }
        // The catalog reads column lists through `pragma_table_info(...)`.
        AuthAction::Pragma {
            pragma_name: "table_info",
            ..
        } => Authorization::Allow,
        _ => Authorization::Deny,
    }
}

/// Quote a name so SQLite reads it as an identifier whatever it contains.
fn quote_name(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn readable(error: rusqlite::Error) -> String {
    match error {
        rusqlite::Error::SqliteFailure(e, _) if e.code == rusqlite::ErrorCode::OperationInterrupted => {
            "cancelled".into()
        }
        other => other.to_string(),
    }
}
