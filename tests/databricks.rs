//! The Databricks backend against a pretend workspace running on this
//! machine. The replies follow the shapes in Databricks' REST API
//! reference; this proves gridpnt's side of the conversation, not that a
//! real workspace behaves the same.

#[path = "common/dir.rs"]
mod dir;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dir::Dir;
use gridpnt::backend::databricks::Databricks;
use gridpnt::backend::databricks::auth::{Credentials, TokenSource};
use gridpnt::backend::{Backend, Limits, NodeKind, Value};
use gridpnt::cache::CatalogCache;
use gridpnt::{exec, worker};
use serde_json::{Value as Json, json};

/// One request the pretend workspace received.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    /// Path and query string.
    target: String,
    body: String,
    authorization: String,
}

struct Workspace {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Workspace {
    /// Start a server that answers each request with `answer(request,
    /// how many requests with this method and path came before it)`.
    fn start(answer: impl Fn(&Seen, usize) -> (u16, Json) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let log = Arc::clone(&seen);

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());

                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut parts = request_line.split_whitespace();
                let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));

                let (mut length, mut authorization) = (0, String::new());
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    let header = header.trim();
                    if header.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':') {
                        match name.to_lowercase().as_str() {
                            "content-length" => length = value.trim().parse().unwrap(),
                            "authorization" => authorization = value.trim().to_string(),
                            _ => {}
                        }
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();

                let request = Seen {
                    method: method.to_string(),
                    target: target.to_string(),
                    body: String::from_utf8(body).unwrap(),
                    authorization,
                };
                let path = |seen: &Seen| seen.target.split('?').next().unwrap().to_string();
                let earlier = log
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|s| s.method == request.method && path(s) == path(&request))
                    .count();
                let (status, reply) = answer(&request, earlier);
                log.lock().unwrap().push(request);

                let reply = reply.to_string();
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                )
                .unwrap();
            }
        });
        Workspace { url, seen }
    }

    fn connect(&self, pins: &[&str]) -> Databricks {
        let credentials = Credentials {
            host: self.url.clone(),
            source: TokenSource::Fixed("secret-token".into()),
        };
        let pins: Vec<String> = pins.iter().map(|p| p.to_string()).collect();
        Databricks::new("work".into(), credentials, "wh1".into(), &pins)
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn count(&self, method: &str, path_start: &str) -> usize {
        self.seen()
            .iter()
            .filter(|s| s.method == method && s.target.starts_with(path_start))
            .count()
    }
}

const WAREHOUSE: &str = "/api/2.0/sql/warehouses/wh1";
const STATEMENTS: &str = "/api/2.0/sql/statements";

fn running_warehouse() -> (u16, Json) {
    (200, json!({"name": "Main", "state": "RUNNING"}))
}

fn names(nodes: &[gridpnt::backend::CatalogNode]) -> Vec<&str> {
    nodes.iter().map(|n| n.name.as_str()).collect()
}

fn path(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

#[test]
fn browsing_follows_pages_and_sends_the_token() {
    let workspace = Workspace::start(|request, _| {
        let target = request.target.as_str();
        if target == "/api/2.1/unity-catalog/catalogs" {
            (200, json!({"catalogs": [{"name": "main"}], "next_page_token": "p2"}))
        } else if target == "/api/2.1/unity-catalog/catalogs?page_token=p2" {
            // A page may be empty and still not be the last.
            (200, json!({"catalogs": [], "next_page_token": "p3"}))
        } else if target == "/api/2.1/unity-catalog/catalogs?page_token=p3" {
            (
                200,
                json!({"catalogs": [{"name": "analytics", "comment": "team data"}]}),
            )
        } else if target == "/api/2.1/unity-catalog/schemas?catalog_name=main" {
            (200, json!({"schemas": [{"name": "sales"}, {"name": "finance"}]}))
        } else if target
            .starts_with("/api/2.1/unity-catalog/table-summaries?catalog_name=main&schema_name_pattern=my%5C_schema")
        {
            (
                200,
                json!({"tables": [
                    {"full_name": "main.my_schema.orders", "table_type": "MANAGED"},
                    {"full_name": "main.my_schema.daily", "table_type": "VIEW"},
                    // `_` in a LIKE pattern matches any character, so a
                    // careless server could return a neighbouring schema.
                    {"full_name": "main.myXschema.other", "table_type": "MANAGED"},
                ]}),
            )
        } else if target == "/api/2.1/unity-catalog/tables/main.sales.odd%20name" {
            (
                200,
                json!({"columns": [
                    {"name": "total", "type_text": "decimal(10,2)", "position": 1},
                    {"name": "id", "type_text": "bigint", "position": 0, "nullable": false},
                ]}),
            )
        } else {
            (404, json!({"message": format!("unexpected {target}")}))
        }
    });
    let mut databricks = workspace.connect(&[]);

    let catalogs = databricks.children(&[]).unwrap();
    assert_eq!(names(&catalogs), ["analytics", "main"]);
    assert_eq!(catalogs[0].kind, NodeKind::Catalog);
    assert_eq!(catalogs[0].detail, "team data");

    assert_eq!(
        names(&databricks.children(&path(&["main"])).unwrap()),
        ["finance", "sales"]
    );

    let tables = databricks.children(&path(&["main", "my_schema"])).unwrap();
    assert_eq!(names(&tables), ["daily", "orders"]);
    assert_eq!(tables[0].kind, NodeKind::View);
    assert_eq!(tables[1].kind, NodeKind::Table);

    let columns = databricks.children(&path(&["main", "sales", "odd name"])).unwrap();
    let described: Vec<(&str, &str)> = columns.iter().map(|c| (c.name.as_str(), c.detail.as_str())).collect();
    assert_eq!(described, [("id", "bigint not null"), ("total", "decimal(10,2)")]);

    assert!(
        workspace
            .seen()
            .iter()
            .all(|s| s.authorization == "Bearer secret-token")
    );
    // Browsing never touches the warehouse.
    assert_eq!(workspace.count("GET", "/api/2.0/sql"), 0);
}

#[test]
fn pinned_scopes_are_listed_without_asking_the_server() {
    let workspace = Workspace::start(|_, _| (500, json!({})));
    let mut databricks = workspace.connect(&["main.sales", "main.finance", "analytics"]);

    assert_eq!(names(&databricks.children(&[]).unwrap()), ["analytics", "main"]);
    assert_eq!(
        names(&databricks.children(&path(&["main"])).unwrap()),
        ["finance", "sales"]
    );
    assert!(workspace.seen().is_empty());
}

/// A workspace that lists 10,000 tables in `main.sales`, over eleven pages,
/// and answers nothing else.
fn large_catalog_workspace() -> Workspace {
    // Interleave the names across pages so simply appending each page gives the wrong display order.
    let mut batches: Vec<Vec<usize>> = (1..=10).map(|first| (first..=10_000).step_by(10).collect()).collect();
    // A page may be empty and still not be the last.
    batches.insert(5, Vec::new());
    let mut cursors = vec![String::new()];
    cursors.extend((1..batches.len()).map(|page| format!("batch-{page}")));

    Workspace::start(move |request, _| {
        let query = request
            .target
            .strip_prefix("/api/2.1/unity-catalog/table-summaries?")
            .unwrap_or("");
        let fields: Vec<_> = query.split('&').collect();
        if request.method != "GET"
            || !fields.contains(&"catalog_name=main")
            || !fields.contains(&"schema_name_pattern=sales")
        {
            return (404, json!({"message": "expected a listing scoped to main.sales"}));
        }
        let cursor = fields
            .iter()
            .find_map(|field| field.strip_prefix("page_token="))
            .unwrap_or("");
        let Some(page) = cursors.iter().position(|key| key == cursor) else {
            return (400, json!({"message": "unknown page cursor"}));
        };
        let tables: Vec<_> = batches[page]
            .iter()
            .map(|id| json!({"full_name": format!("main.sales.table_{id:05}"), "table_type": "MANAGED"}))
            .collect();
        let mut reply = json!({"tables": tables});
        if let Some(next) = cursors.get(page + 1) {
            reply["next_page_token"] = json!(next);
        }
        (200, reply)
    })
}

#[test]
fn ten_thousand_pinned_tables_reopen_without_requests() {
    let workspace = large_catalog_workspace();
    let dir = Dir::new("large-catalog");
    let file = dir.0.join("catalog.json");

    for (run, expected_requests) in [("fresh", 11), ("reopened", 0)] {
        let before = workspace.seen().len();
        let mut backend = workspace.connect(&["main.sales"]);
        let mut cache = CatalogCache::at(file.clone());
        let mut browse =
            |parts: &[&str]| worker::children(&mut backend, Some(&mut cache), &path(parts), false).unwrap();

        assert_eq!(names(&browse(&[])), ["main"]);
        assert_eq!(names(&browse(&["main"])), ["sales"]);
        assert_eq!(workspace.seen().len(), before, "{run}: listing pinned parents");

        let tables = browse(&["main", "sales"]);
        assert_eq!(tables.len(), 10_000, "{run}: table count");
        for (index, table) in tables.iter().enumerate() {
            assert_eq!(table.name, format!("table_{:05}", index + 1), "{run}: index {index}");
        }
        assert_eq!(
            workspace.seen().len() - before,
            expected_requests,
            "{run}: HTTP requests"
        );
    }
}

#[test]
fn a_query_is_submitted_polled_and_read_chunk_by_chunk() {
    let workspace = Workspace::start(
        |request, earlier| match (request.method.as_str(), request.target.as_str()) {
            ("GET", WAREHOUSE) => running_warehouse(),
            ("POST", STATEMENTS) => (200, json!({"statement_id": "s1", "status": {"state": "PENDING"}})),
            ("GET", "/api/2.0/sql/statements/s1") if earlier == 0 => {
                (200, json!({"statement_id": "s1", "status": {"state": "RUNNING"}}))
            }
            ("GET", "/api/2.0/sql/statements/s1") => (
                200,
                json!({
                    "statement_id": "s1",
                    "status": {"state": "SUCCEEDED"},
                    "manifest": {"truncated": false, "schema": {"columns": [
                        {"name": "id", "type_name": "LONG"},
                        {"name": "price", "type_name": "DECIMAL"},
                        {"name": "ratio", "type_name": "DOUBLE"},
                        {"name": "note", "type_name": "STRING"},
                    ]}},
                    "result": {
                        "data_array": [["1", "9.50", "0.5", "first"]],
                        "next_chunk_internal_link": "/api/2.0/sql/statements/s1/result/chunks/1",
                    },
                }),
            ),
            ("GET", "/api/2.0/sql/statements/s1/result/chunks/1") => {
                (200, json!({"data_array": [["2", null, null, null]]}))
            }
            _ => (404, json!({"message": "unexpected"})),
        },
    );
    let mut databricks = workspace.connect(&[]);

    let result = exec::run(&mut databricks, "SELECT * FROM main.sales.orders").unwrap();
    assert_eq!(result.columns, ["id", "price", "ratio", "note"]);
    assert_eq!(
        result.rows,
        [
            vec![
                Value::Integer(1),
                Value::Decimal("9.50".into()),
                Value::Real(0.5),
                Value::Text("first".into())
            ],
            vec![Value::Integer(2), Value::Null, Value::Null, Value::Null],
        ]
    );
    assert!(!result.truncated);

    let submitted = workspace.seen().into_iter().find(|s| s.method == "POST").unwrap();
    let body: Json = serde_json::from_str(&submitted.body).unwrap();
    assert_eq!(body["statement"], "SELECT * FROM main.sales.orders");
    assert_eq!(body["warehouse_id"], "wh1");
    assert_eq!(body["wait_timeout"], "0s");
    assert_eq!(body["row_limit"], exec::ROW_CAP + 1);
}

#[test]
fn rows_beyond_the_cap_are_dropped_and_reported() {
    let workspace = Workspace::start(|request, _| match request.method.as_str() {
        "GET" => running_warehouse(),
        _ => (
            200,
            json!({
                "statement_id": "s1",
                "status": {"state": "SUCCEEDED"},
                "manifest": {"schema": {"columns": [{"name": "n", "type_name": "INT"}]}},
                "result": {"data_array": [["1"], ["2"], ["3"]]},
            }),
        ),
    });
    let mut databricks = workspace.connect(&[]);
    let limits = Limits {
        row_cap: 2,
        timeout: Duration::from_secs(5),
    };
    let result = exec::run_within(&mut databricks, "SELECT n FROM t", &limits).unwrap();
    assert_eq!(result.rows.len(), 2);
    assert!(result.truncated);
}

#[test]
fn a_stopped_warehouse_is_announced_before_it_is_started() {
    let workspace = Workspace::start(|request, _| match request.method.as_str() {
        "GET" => (200, json!({"name": "Main", "state": "STOPPED"})),
        _ => (
            200,
            json!({"statement_id": "s1", "status": {"state": "SUCCEEDED"},
                   "manifest": {"schema": {"columns": [{"name": "one", "type_name": "INT"}]}},
                   "result": {"data_array": [["1"]]}}),
        ),
    });
    let mut databricks = workspace.connect(&[]);

    let warning = exec::run(&mut databricks, "SELECT 1").unwrap_err();
    assert!(warning.contains("\"Main\" is stopped"), "{warning}");
    assert!(warning.contains("costs money"), "{warning}");
    assert_eq!(
        workspace.count("POST", STATEMENTS),
        0,
        "nothing may be submitted before the warning"
    );

    // Running again is the go-ahead.
    assert!(exec::run(&mut databricks, "SELECT 1").is_ok());
    assert_eq!(workspace.count("POST", STATEMENTS), 1);
}

#[test]
fn a_failed_statement_reports_the_database_message() {
    let workspace = Workspace::start(|request, _| match request.method.as_str() {
        "GET" => running_warehouse(),
        _ => (
            200,
            json!({"statement_id": "s1", "status": {"state": "FAILED",
                   "error": {"error_code": "BAD_REQUEST", "message": "TABLE_OR_VIEW_NOT_FOUND: nope"}}}),
        ),
    });
    let mut databricks = workspace.connect(&[]);
    assert_eq!(
        exec::run(&mut databricks, "SELECT * FROM nope").unwrap_err(),
        "TABLE_OR_VIEW_NOT_FOUND: nope"
    );
}

/// A workspace where the statement never finishes.
fn never_finishing() -> Workspace {
    Workspace::start(|request, _| match (request.method.as_str(), request.target.as_str()) {
        ("GET", WAREHOUSE) => running_warehouse(),
        ("POST", "/api/2.0/sql/statements/s1/cancel") => (200, json!({})),
        _ => (200, json!({"statement_id": "s1", "status": {"state": "RUNNING"}})),
    })
}

#[test]
fn a_query_that_runs_too_long_is_cancelled_on_the_server() {
    let workspace = never_finishing();
    let mut databricks = workspace.connect(&[]);
    let limits = Limits {
        row_cap: 10,
        timeout: Duration::from_millis(300),
    };
    let error = exec::run_within(&mut databricks, "SELECT 1", &limits).unwrap_err();
    assert!(error.starts_with("timed out"), "{error}");
    assert_eq!(workspace.count("POST", "/api/2.0/sql/statements/s1/cancel"), 1);
}

#[test]
fn cancelling_stops_the_wait_and_tells_the_server() {
    let workspace = never_finishing();
    let mut databricks = workspace.connect(&[]);
    let cancel = databricks.canceller();

    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        cancel();
    });
    let error = exec::run(&mut databricks, "SELECT 1").unwrap_err();
    canceller.join().unwrap();

    assert_eq!(error, "cancelled");
    assert!(workspace.count("POST", "/api/2.0/sql/statements/s1/cancel") >= 1);
}

#[test]
fn writes_never_reach_the_workspace() {
    let workspace = Workspace::start(|_, _| (500, json!({})));
    let mut databricks = workspace.connect(&[]);

    for sql in [
        "DELETE FROM main.sales.orders",
        "SELECT 1; DROP TABLE t",
        "OPTIMIZE main.sales.orders",
        "COPY INTO t FROM 's3://x'",
    ] {
        let error = exec::run(&mut databricks, sql).unwrap_err();
        assert!(error.starts_with("refused (read-only)"), "{sql}: {error}");
    }
    assert!(workspace.seen().is_empty());
}

#[test]
fn an_error_status_shows_the_reason() {
    let workspace = Workspace::start(|_, _| {
        (
            403,
            json!({"error_code": "PERMISSION_DENIED", "message": "User lacks USE CATALOG"}),
        )
    });
    let mut databricks = workspace.connect(&[]);
    let error = databricks.children(&[]).unwrap_err();
    assert_eq!(error, "Databricks said no (HTTP 403): User lacks USE CATALOG");
}

#[test]
fn a_link_that_leaves_the_workspace_is_not_followed() {
    let workspace = Workspace::start(|request, _| match request.method.as_str() {
        "GET" => running_warehouse(),
        _ => (
            200,
            json!({"statement_id": "s1", "status": {"state": "SUCCEEDED"},
                   "manifest": {"schema": {"columns": [{"name": "n", "type_name": "INT"}]}},
                   "result": {"data_array": [["1"]], "next_chunk_internal_link": "https://elsewhere.example/steal"}}),
        ),
    });
    let mut databricks = workspace.connect(&[]);
    assert!(
        exec::run(&mut databricks, "SELECT 1")
            .unwrap_err()
            .contains("unexpected link")
    );
}

#[test]
fn previews_quote_every_part_of_the_name() {
    let workspace = Workspace::start(|_, _| (500, json!({})));
    let databricks = workspace.connect(&[]);
    let sql = databricks
        .preview_sql(&path(&["main", "my schema", "odd`name"]))
        .unwrap();
    assert_eq!(sql, "SELECT * FROM `main`.`my schema`.`odd``name`");
}
