//! gridpnt: a terminal-native database tool.
//!
//! Layout, from the database outward:
//!
//! - `backend` — one module per database, all behind the `Backend` trait.
//! - `cache`   — the catalog remembered on disk between runs.
//! - `config`  — the user's connections.
//! - `guard`   — classifies SQL so read-only mode can refuse non-reads.
//! - `exec`    — the single path by which SQL reaches a backend.
//! - `remote`  — accepts SQL sent by `gridpnt run` from an editor.
//! - `worker`  — runs backend calls on a thread so the UI never waits.
//! - `tui`     — the terminal interface.
//!
//! The interface never talks to a backend directly, and no SQL reaches a
//! backend without passing the guard.

pub mod backend;
pub mod cache;
pub mod config;
pub mod exec;
pub mod guard;
pub mod remote;
pub mod tui;
pub mod worker;
