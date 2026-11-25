//! The single path by which SQL reaches a backend. Every face of gridpnt
//! (the interface today; agents and the command line later) calls `run`,
//! so the safety rules cannot differ between them.

use std::time::Duration;

use crate::backend::{Backend, Limits, Result, ResultSet};
use crate::guard;

/// The most rows held in memory for one result. Hitting it is always
/// reported to the user, never silent.
pub const ROW_CAP: usize = 10_000;

/// The longest a query may run before gridpnt cancels it. Long enough for
/// a cold warehouse to start; short enough that a forgotten query does not
/// run all night.
pub const TIMEOUT: Duration = Duration::from_secs(600);

pub const LIMITS: Limits = Limits {
    row_cap: ROW_CAP,
    timeout: TIMEOUT,
};

/// Check `sql` with the guard, then run it. Connections are read-only;
/// there is no way to turn that off yet.
pub fn run(backend: &mut dyn Backend, sql: &str) -> Result<ResultSet> {
    run_within(backend, sql, &LIMITS)
}

/// `run` with explicit limits, for tests that need small ones.
pub fn run_within(backend: &mut dyn Backend, sql: &str, limits: &Limits) -> Result<ResultSet> {
    let verdict = guard::classify(sql);
    if !verdict.allowed_read_only() {
        return Err(format!("refused (read-only): {}", verdict.reason));
    }
    backend.query(sql, limits)
}
