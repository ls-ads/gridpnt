//! The single path by which SQL reaches a backend. Every face of gridpnt
//! (the interface today; agents and the command line later) calls `run`,
//! so the safety rules cannot differ between them.

use crate::backend::{Backend, Result, ResultSet};
use crate::guard;

/// The most rows held in memory for one result. Hitting it is always
/// reported to the user, never silent.
pub const ROW_CAP: usize = 10_000;

/// Check `sql` with the guard, then run it. Connections are read-only;
/// there is no way to turn that off yet.
pub fn run(backend: &mut dyn Backend, sql: &str) -> Result<ResultSet> {
    let verdict = guard::classify(sql);
    if !verdict.allowed_read_only() {
        return Err(format!("refused (read-only): {}", verdict.reason));
    }
    backend.query(sql, ROW_CAP)
}
