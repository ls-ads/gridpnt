//! gridpnt: a terminal-native database tool.
//!
//! Layout, from the database outward:
//!
//! - `backend` — one module per database, all behind the `Backend` trait.
//! - `guard`   — classifies SQL so read-only mode can refuse non-reads.
//! - `exec`    — the single path by which SQL reaches a backend.
//! - `worker`  — runs backend calls on a thread so the UI never waits.
//! - `tui`     — the terminal interface.
//!
//! The interface never talks to a backend directly, and no SQL reaches a
//! backend without passing the guard.

pub mod backend;
pub mod exec;
pub mod guard;
pub mod tui;
pub mod worker;
