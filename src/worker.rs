//! Runs every backend call on its own thread, so the interface never waits
//! on a database. The interface sends a `Request` and later receives a
//! `Response`; it can cancel the running query at any time.

use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::backend::{Backend, Cancel, CatalogNode, Result, ResultSet};
use crate::exec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Load the catalog entries under `path`.
    Children { path: Vec<String> },
    /// Show the contents of the table or view at `path`.
    Preview { path: Vec<String> },
    /// Run SQL the user typed.
    Run { sql: String },
}

#[derive(Debug)]
pub enum Response {
    Children {
        path: Vec<String>,
        result: Result<Vec<CatalogNode>>,
    },
    Ran {
        result: Result<ResultSet>,
        elapsed: Duration,
    },
}

pub struct Worker {
    requests: Sender<Request>,
    cancel: Cancel,
}

impl Worker {
    /// Start the worker thread. `reply` is called on that thread with each
    /// response. The thread ends when the `Worker` is dropped.
    pub fn spawn(mut backend: Box<dyn Backend>, reply: impl Fn(Response) + Send + 'static) -> Worker {
        let cancel = backend.canceller();
        let (requests, inbox) = mpsc::channel::<Request>();

        thread::spawn(move || {
            for request in inbox {
                reply(handle(backend.as_mut(), request));
            }
        });

        Worker { requests, cancel }
    }

    pub fn send(&self, request: Request) {
        // Sending only fails if the worker thread has died, in which case
        // there is nothing useful left to do with the request.
        let _ = self.requests.send(request);
    }

    pub fn cancel(&self) {
        (self.cancel)();
    }
}

fn handle(backend: &mut dyn Backend, request: Request) -> Response {
    match request {
        Request::Children { path } => {
            let result = backend.children(&path);
            Response::Children { path, result }
        }
        Request::Preview { path } => match backend.preview_sql(&path) {
            Some(sql) => run(backend, &sql),
            None => Response::Ran {
                result: Err("nothing to preview here".into()),
                elapsed: Duration::ZERO,
            },
        },
        Request::Run { sql } => run(backend, &sql),
    }
}

fn run(backend: &mut dyn Backend, sql: &str) -> Response {
    let started = Instant::now();
    let result = exec::run(backend, sql);
    Response::Ran {
        result,
        elapsed: started.elapsed(),
    }
}
