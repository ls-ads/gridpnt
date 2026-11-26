//! Databricks backend: Unity Catalog for browsing, a SQL warehouse for
//! queries, both over the REST API.
//!
//! Safety notes specific to Databricks:
//!
//! - There is no read-only session to ask for, so the guard and the
//!   permissions of the token are the barriers. Use a token that can only
//!   read what you intend to read.
//! - Browsing the catalog uses the Unity Catalog API and never wakes a
//!   warehouse. Only running a query can start one, and gridpnt says so
//!   first.
//! - Cancelling and timing out both cancel the statement on the server, so
//!   nothing is left running on the warehouse.

pub mod auth;
pub mod client;
mod nested;
mod values;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use self::auth::Credentials;
use self::client::{Client, path_segment};
use self::values::ColumnKind;
use super::{Backend, Cancel, CatalogNode, Limits, NodeKind, Result, ResultSet};

/// Upper bound on pages read for one listing, so a misbehaving server
/// cannot keep gridpnt reading forever.
const MAX_PAGES: usize = 500;

/// The most result data asked for in one query. Above 25 MiB Databricks
/// refuses to send a result inline at all; with this limit it sends the
/// first part instead, marked as cut short, and gridpnt says so as it does
/// for the row cap. The margin is there because Databricks measures the
/// size in its own internal form, not as the JSON it sends.
const RESULT_BYTE_LIMIT: u64 = 16 * 1024 * 1024;

pub struct Databricks {
    client: Arc<Client>,
    label: String,
    warehouse: String,
    pins: Vec<Pin>,
    running: Arc<Running>,
    /// Set once the user has been told a stopped warehouse will be started.
    start_announced: bool,
}

/// A part of the catalog the user chose to see: a whole catalog, or one
/// schema in it.
struct Pin {
    catalog: String,
    schema: Option<String>,
}

/// The statement in flight, shared with the cancel handle.
#[derive(Default)]
struct Running {
    statement_id: Mutex<Option<String>>,
    cancel_requested: AtomicBool,
}

// --- Shapes of the JSON replies. Only the fields gridpnt uses. -------------

#[derive(Deserialize, Default)]
#[serde(default)]
struct Listing {
    catalogs: Vec<Named>,
    schemas: Vec<Named>,
    tables: Vec<TableSummary>,
    next_page_token: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Named {
    name: String,
    comment: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TableSummary {
    full_name: String,
    table_type: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Table {
    columns: Vec<Column>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Column {
    name: String,
    type_text: String,
    nullable: Option<bool>,
    position: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Warehouse {
    name: String,
    state: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Statement {
    statement_id: String,
    status: Status,
    manifest: Manifest,
    result: Chunk,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Status {
    state: String,
    error: Option<StatementError>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StatementError {
    message: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Manifest {
    schema: Schema,
    truncated: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Schema {
    columns: Vec<ResultColumn>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ResultColumn {
    name: String,
    /// The short type name, such as `INT` or `STRUCT`.
    type_name: String,
    /// The whole declaration, such as `STRUCT<id: INT, name: STRING>`.
    type_text: String,
}

/// One piece of a result. Every value arrives as text, or null.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Chunk {
    data_array: Vec<Vec<Option<String>>>,
    next_chunk_internal_link: Option<String>,
}

// ---------------------------------------------------------------------------

impl Databricks {
    pub fn new(label: String, credentials: Credentials, warehouse: String, pins: &[String]) -> Self {
        let pins = pins
            .iter()
            .map(|pin| match pin.split_once('.') {
                Some((catalog, schema)) => Pin {
                    catalog: catalog.to_string(),
                    schema: Some(schema.to_string()),
                },
                None => Pin {
                    catalog: pin.clone(),
                    schema: None,
                },
            })
            .collect();
        Databricks {
            client: Arc::new(Client::new(credentials)),
            label,
            warehouse,
            pins,
            running: Arc::new(Running::default()),
            start_announced: false,
        }
    }

    /// Read every page of a Unity Catalog listing.
    fn list(&self, path: &str, query: &[(&str, &str)]) -> Result<Vec<Listing>> {
        let mut pages = Vec::new();
        let mut page_token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query = query.to_vec();
            if let Some(token) = &page_token {
                query.push(("page_token", token));
            }
            let page: Listing = self.client.get(path, &query)?;
            page_token = page.next_page_token.clone().filter(|token| !token.is_empty());
            pages.push(page);
            if page_token.is_none() {
                return Ok(pages);
            }
        }
        Err("the catalog listing did not end; giving up".into())
    }

    fn catalogs(&self) -> Result<Vec<CatalogNode>> {
        // With pins, the top level is known without asking the server.
        if !self.pins.is_empty() {
            let mut names: Vec<&str> = self.pins.iter().map(|pin| pin.catalog.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            return Ok(names
                .into_iter()
                .map(|name| container(name, NodeKind::Catalog, None))
                .collect());
        }
        let pages = self.list("/api/2.1/unity-catalog/catalogs", &[])?;
        Ok(sorted(
            pages
                .into_iter()
                .flat_map(|page| page.catalogs)
                .map(|c| container(&c.name, NodeKind::Catalog, c.comment)),
        ))
    }

    fn schemas(&self, catalog: &str) -> Result<Vec<CatalogNode>> {
        // If only particular schemas of this catalog are pinned, list
        // exactly those, again without asking the server.
        let pinned: Vec<&Pin> = self.pins.iter().filter(|pin| pin.catalog == catalog).collect();
        if !pinned.is_empty() && pinned.iter().all(|pin| pin.schema.is_some()) {
            let names = pinned.iter().filter_map(|pin| pin.schema.as_deref());
            return Ok(sorted(names.map(|name| container(name, NodeKind::Schema, None))));
        }
        let pages = self.list("/api/2.1/unity-catalog/schemas", &[("catalog_name", catalog)])?;
        Ok(sorted(
            pages
                .into_iter()
                .flat_map(|page| page.schemas)
                .map(|s| container(&s.name, NodeKind::Schema, s.comment)),
        ))
    }

    fn tables(&self, catalog: &str, schema: &str) -> Result<Vec<CatalogNode>> {
        // Table summaries come thousands to a page, where the full table
        // listing comes fifty to a page. The schema is matched with a LIKE
        // pattern, so `_` and `%` in its name are escaped and the result is
        // filtered again below to be certain.
        let pattern = schema.replace('\\', "\\\\").replace('_', "\\_").replace('%', "\\%");
        let query = [("catalog_name", catalog), ("schema_name_pattern", pattern.as_str())];
        let pages = self.list("/api/2.1/unity-catalog/table-summaries", &query)?;

        let prefix = format!("{catalog}.{schema}.");
        let tables = pages.into_iter().flat_map(|page| page.tables).filter_map(|table| {
            let name = table.full_name.strip_prefix(&prefix)?;
            let is_view = matches!(table.table_type.as_str(), "VIEW" | "MATERIALIZED_VIEW" | "METRIC_VIEW");
            Some(CatalogNode {
                name: name.to_string(),
                kind: if is_view { NodeKind::View } else { NodeKind::Table },
                detail: String::new(),
                has_children: true,
            })
        });
        Ok(sorted(tables))
    }

    fn columns(&self, catalog: &str, schema: &str, table: &str) -> Result<Vec<CatalogNode>> {
        let full_name = format!("{catalog}.{schema}.{table}");
        let path = format!("/api/2.1/unity-catalog/tables/{}", path_segment(&full_name));
        let mut table: Table = self.client.get(&path, &[])?;
        table.columns.sort_by_key(|column| column.position);
        Ok(table
            .columns
            .into_iter()
            .map(|column| CatalogNode {
                name: column.name,
                kind: NodeKind::Column,
                detail: match column.nullable {
                    Some(false) => format!("{} not null", column.type_text),
                    _ => column.type_text,
                },
                has_children: false,
            })
            .collect())
    }

    /// Running a query on a stopped warehouse starts it, which costs money.
    /// The first time that would happen, refuse and say so; the user runs
    /// the query again to go ahead.
    fn check_warehouse(&mut self) -> Result<()> {
        if self.start_announced {
            return Ok(());
        }
        let path = format!("/api/2.0/sql/warehouses/{}", path_segment(&self.warehouse));
        let warehouse: Warehouse = self.client.get(&path, &[])?;
        if warehouse.state == "RUNNING" {
            return Ok(());
        }
        self.start_announced = true;
        Err(format!(
            "warehouse \"{}\" is {}. Running a query will start it, which costs money. Run again to go ahead.",
            warehouse.name,
            warehouse.state.to_lowercase()
        ))
    }

    fn cancel_on_server(&self, statement_id: &str) {
        let path = format!("/api/2.0/sql/statements/{}/cancel", path_segment(statement_id));
        // Best effort: if this fails there is nothing more to be done here.
        let _: Result<serde_json::Value> = self.client.post(&path, &json!({}));
    }

    /// Wait for a statement to finish, checking for cancel and timeout
    /// between polls.
    fn wait(&self, mut statement: Statement, limits: &Limits) -> Result<Statement> {
        let deadline = Instant::now() + limits.timeout;
        let mut pause = Duration::from_millis(100);
        let path = format!("/api/2.0/sql/statements/{}", path_segment(&statement.statement_id));

        loop {
            match statement.status.state.as_str() {
                "SUCCEEDED" => return Ok(statement),
                "PENDING" | "RUNNING" => {}
                "CANCELED" => return Err("cancelled".into()),
                other => {
                    let detail = statement.status.error.map(|e| e.message).unwrap_or_default();
                    return Err(if detail.is_empty() {
                        format!("statement ended as {other}")
                    } else {
                        detail
                    });
                }
            }
            if self.running.cancel_requested.load(Ordering::SeqCst) {
                self.cancel_on_server(&statement.statement_id);
                return Err("cancelled".into());
            }
            if Instant::now() >= deadline {
                self.cancel_on_server(&statement.statement_id);
                return Err(format!(
                    "timed out after {} s and was cancelled on the warehouse",
                    limits.timeout.as_secs()
                ));
            }
            thread::sleep(pause);
            pause = (pause * 2).min(Duration::from_secs(1));
            statement = self.client.get(&path, &[])?;
        }
    }
}

impl Backend for Databricks {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn cache_name(&self) -> Option<String> {
        Some(self.label.clone())
    }

    fn children(&mut self, path: &[String]) -> Result<Vec<CatalogNode>> {
        match path {
            [] => self.catalogs(),
            [catalog] => self.schemas(catalog),
            [catalog, schema] => self.tables(catalog, schema),
            [catalog, schema, table] => self.columns(catalog, schema, table),
            _ => Ok(Vec::new()),
        }
    }

    fn preview_sql(&self, path: &[String]) -> Option<String> {
        match path {
            [catalog, schema, table] => Some(format!(
                "SELECT * FROM {}.{}.{}",
                quote_name(catalog),
                quote_name(schema),
                quote_name(table)
            )),
            _ => None,
        }
    }

    fn query(&mut self, sql: &str, limits: &Limits) -> Result<ResultSet> {
        self.check_warehouse()?;

        *self.running.statement_id.lock().unwrap() = None;
        self.running.cancel_requested.store(false, Ordering::SeqCst);

        // `wait_timeout: 0s` makes the call return at once with an id, so
        // the statement can be cancelled from the very start. One extra row
        // is requested to learn whether the cap cut the result short.
        let submitted: Statement = self.client.post(
            "/api/2.0/sql/statements",
            &json!({
                "statement": sql,
                "warehouse_id": self.warehouse,
                "wait_timeout": "0s",
                "disposition": "INLINE",
                "format": "JSON_ARRAY",
                "row_limit": limits.row_cap + 1,
                "byte_limit": RESULT_BYTE_LIMIT,
            }),
        )?;
        *self.running.statement_id.lock().unwrap() = Some(submitted.statement_id.clone());

        let finished = self.wait(submitted, limits);
        *self.running.statement_id.lock().unwrap() = None;
        let finished = finished?;

        let kinds: Vec<ColumnKind> = finished
            .manifest
            .schema
            .columns
            .iter()
            .map(|c| ColumnKind::of(&c.type_name, &c.type_text))
            .collect();
        let mut result = ResultSet {
            columns: finished
                .manifest
                .schema
                .columns
                .iter()
                .map(|c| c.name.clone())
                .collect(),
            rows: Vec::new(),
            truncated: finished.manifest.truncated,
        };

        // A result arrives in one or more chunks, each linking to the next.
        let mut chunk = finished.result;
        loop {
            for row in chunk.data_array {
                if result.rows.len() == limits.row_cap {
                    result.truncated = true;
                    return Ok(result);
                }
                let cells = row.into_iter().zip(&kinds).map(|(text, kind)| kind.value(text));
                result.rows.push(cells.collect());
            }
            match chunk.next_chunk_internal_link {
                // The link is a path on the same workspace. Anything else
                // is not followed, so the token cannot be sent elsewhere.
                Some(link) if link.starts_with("/api/") => chunk = self.client.get(&link, &[])?,
                Some(link) if !link.is_empty() => return Err("unexpected link in the result; stopping".into()),
                _ => return Ok(result),
            }
        }
    }

    fn canceller(&self) -> Cancel {
        let running = Arc::clone(&self.running);
        let client = Arc::clone(&self.client);
        Box::new(move || {
            // The flag stops the polling loop; the request stops the
            // warehouse. Whichever thread notices first also tells the
            // server, and telling it twice is harmless.
            running.cancel_requested.store(true, Ordering::SeqCst);
            let statement_id = running.statement_id.lock().unwrap().clone();
            if let Some(id) = statement_id {
                let path = format!("/api/2.0/sql/statements/{}/cancel", path_segment(&id));
                let _: Result<serde_json::Value> = client.post(&path, &json!({}));
            }
        })
    }
}

fn container(name: &str, kind: NodeKind, comment: Option<String>) -> CatalogNode {
    CatalogNode {
        name: name.to_string(),
        kind,
        detail: comment.unwrap_or_default(),
        has_children: true,
    }
}

fn sorted(nodes: impl Iterator<Item = CatalogNode>) -> Vec<CatalogNode> {
    let mut nodes: Vec<CatalogNode> = nodes.collect();
    nodes.sort_by(|a, b| a.name.cmp(&b.name));
    nodes
}

/// Quote a name so Databricks reads it as an identifier whatever it contains.
fn quote_name(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}
