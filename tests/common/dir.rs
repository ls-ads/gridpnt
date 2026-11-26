//! A temporary directory for one test, removed afterwards.
//!
//! This has its own file rather than living in `common/mod.rs`: a test file
//! that includes a helper it never uses gets an unused-code warning, which
//! CI treats as an error.

use std::path::PathBuf;

/// A fresh path for a test that creates files, cleaned up even if it panics.
pub struct Dir(pub PathBuf);

impl Dir {
    pub fn new(name: &str) -> Self {
        let suite = env!("CARGO_CRATE_NAME");
        let dir = std::env::temp_dir().join(format!("gridpnt-test-{suite}-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Dir(dir)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
