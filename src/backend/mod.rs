//! What every database must provide. One module per database implements
//! `Backend`; nothing outside that module knows which database it is.

pub mod databricks;
pub mod sqlite;

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// One cell of a result.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    /// An exact decimal number, kept as the text the database sent.
    Decimal(String),
    Text(String),
    /// Binary data is not loaded; only its size in bytes is kept.
    Blob(usize),
}

/// The rows a query produced, held in memory up to a row cap.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// True when the query had more rows than the cap allowed.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Catalog,
    Schema,
    Table,
    View,
    Column,
}

/// One entry in the catalog tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogNode {
    pub name: String,
    pub kind: NodeKind,
    /// Short extra text shown beside the name, such as a column's type.
    pub detail: String,
    pub has_children: bool,
}

/// The limits every query runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Most rows kept. More than this and the result is marked truncated.
    pub row_cap: usize,
    /// Longest a query may run before it is cancelled.
    pub timeout: Duration,
}

/// Stops whatever the backend is currently running. Safe to call from any
/// thread, and harmless when nothing is running.
pub type Cancel = Box<dyn Fn() + Send + Sync>;

/// Errors are plain text meant to be shown to the user.
pub type Result<T> = std::result::Result<T, String>;

pub trait Backend: Send {
    /// A short name for this connection, shown in the status line.
    fn label(&self) -> String;

    /// A name under which this connection's catalog may be remembered on
    /// disk, or `None` if listing it is already instant.
    fn cache_name(&self) -> Option<String> {
        None
    }

    /// The catalog entries directly under `path`. An empty path is the root.
    /// Called only when the user opens that part of the tree.
    fn children(&mut self, path: &[String]) -> Result<Vec<CatalogNode>>;

    /// A query that shows the contents of the table or view at `path`, with
    /// names quoted the way this database requires. `None` if `path` is not
    /// something with rows.
    fn preview_sql(&self, path: &[String]) -> Option<String>;

    /// Run one statement within `limits`.
    ///
    /// Callers must go through `exec::run`, which applies the guard first.
    /// A backend must also enforce read-only itself wherever the database
    /// allows it, so the guard is never the only barrier.
    fn query(&mut self, sql: &str, limits: &Limits) -> Result<ResultSet>;

    /// A handle that cancels a running `query` from another thread.
    fn canceller(&self) -> Cancel;
}
