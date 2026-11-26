//! Runs every backend call on its own thread, so the interface never waits
//! on a database. The interface sends a `Request` and later receives a
//! `Response`; it can cancel the running query at any time.

use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::backend::{Backend, Cancel, CatalogNode, Result, ResultSet};
use crate::cache::CatalogCache;
use crate::exec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Load the catalog entries under `path`. With `refresh`, ask the
    /// database even if the answer is remembered from before.
    Children { path: Vec<String>, refresh: bool },
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
            let mut cache = backend.cache_name().map(|name| CatalogCache::for_connection(&name));
            for request in inbox {
                reply(handle(backend.as_mut(), cache.as_mut(), request));
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

fn handle(backend: &mut dyn Backend, cache: Option<&mut CatalogCache>, request: Request) -> Response {
    match request {
        Request::Children { path, refresh } => {
            let result = children(backend, cache, &path, refresh);
            Response::Children { path, result }
        }
        Request::Preview { path } => timed(|| exec::preview(backend, &path)),
        Request::Run { sql } => timed(|| exec::run(backend, &sql)),
    }
}

/// List catalog entries, from the cache when allowed and possible.
pub fn children(
    backend: &mut dyn Backend,
    cache: Option<&mut CatalogCache>,
    path: &[String],
    refresh: bool,
) -> Result<Vec<CatalogNode>> {
    let Some(cache) = cache else {
        return backend.children(path);
    };
    if !refresh && let Some(remembered) = cache.get(path) {
        return Ok(remembered);
    }
    let fresh = backend.children(path)?;
    cache.put(path, &fresh);
    Ok(fresh)
}

fn timed(query: impl FnOnce() -> Result<ResultSet>) -> Response {
    let started = Instant::now();
    let result = query();
    Response::Ran {
        result,
        elapsed: started.elapsed(),
    }
}
