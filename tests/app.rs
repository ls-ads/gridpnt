//! The interface, driven by keys and drawn to an in-memory screen.
//!
//! `Session` stands in for the real event loop: it passes the app's
//! requests to a real SQLite backend and feeds the answers straight back,
//! so each key press is fully handled before the next.

mod common;

use common::TempDb;
use gridpnt::backend::Backend;
use gridpnt::backend::sqlite::Sqlite;
use gridpnt::exec;
use gridpnt::tui::App;
use gridpnt::worker::{Request, Response};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

struct Session {
    app: App,
    sqlite: Sqlite,
    terminal: Terminal<TestBackend>,
    _db: TempDb,
}

impl Session {
    fn new(rows: usize) -> Self {
        let db = TempDb::new(rows);
        let sqlite = Sqlite::open(&db.path).unwrap();
        let mut session = Session {
            app: App::new(sqlite.label(), None),
            sqlite,
            terminal: Terminal::new(TestBackend::new(100, 20)).unwrap(),
            _db: db,
        };
        session.settle();
        session
    }

    /// Answer every request the app has queued.
    fn settle(&mut self) {
        for request in self.app.take_requests() {
            let ran = |result| Response::Ran {
                result,
                elapsed: Duration::from_millis(7),
            };
            let response = match request {
                Request::Children { path } => {
                    let result = self.sqlite.children(&path);
                    Response::Children { path, result }
                }
                Request::Preview { path } => {
                    let sql = self.sqlite.preview_sql(&path).unwrap();
                    ran(exec::run(&mut self.sqlite, &sql))
                }
                Request::Run { sql } => ran(exec::run(&mut self.sqlite, &sql)),
            };
            self.app.on_response(response);
        }
    }

    fn press(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        self.app.on_key(KeyEvent::new(code, modifiers));
        self.settle();
    }

    /// Type plain characters. `\n` is Enter and `\t` is Tab.
    fn keys(&mut self, keys: &str) {
        for c in keys.chars() {
            let code = match c {
                '\n' => KeyCode::Enter,
                '\t' => KeyCode::Tab,
                c => KeyCode::Char(c),
            };
            self.press(code, KeyModifiers::NONE);
        }
    }

    fn ctrl(&mut self, c: char) {
        self.press(KeyCode::Char(c), KeyModifiers::CONTROL);
    }

    /// Draw and return the screen as lines of text.
    fn screen(&mut self) -> Vec<String> {
        self.terminal.draw(|frame| self.app.draw(frame)).unwrap();
        let buffer = self.terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    fn status(&mut self) -> String {
        self.screen().last().unwrap().clone()
    }

    fn shows(&mut self, needle: &str) -> bool {
        self.screen().iter().any(|line| line.contains(needle))
    }
}

#[test]
fn starts_with_the_catalog_loaded_and_read_only_shown() {
    let mut s = Session::new(0);
    assert!(s.shows("▸ adults"));
    assert!(s.shows("▸ people"));
    assert!(s.status().contains("READ-ONLY"));
}

#[test]
fn catalog_opens_and_closes_with_l_and_h() {
    let mut s = Session::new(0);
    s.keys("G"); // last row: people
    s.keys("l");
    assert!(s.shows("▾ people"));
    assert!(s.shows("name text not null"));

    s.keys("l"); // steps onto the first column
    s.keys("h"); // back to the table
    s.keys("h"); // closes it
    assert!(s.shows("▸ people"));
    assert!(!s.shows("name text not null"));
}

#[test]
fn enter_previews_a_table() {
    let mut s = Session::new(0);
    s.keys("G\n");
    assert!(s.shows("Grace"));
    assert!(s.shows("NULL"));
    assert!(s.shows("<blob 2 bytes>"));
    assert!(s.status().contains("row 1/3 · col 1/5 · 7 ms"));
    // A line break inside a value must not break the grid.
    assert!(s.shows("Li↵nus"));
}

#[test]
fn prompt_runs_a_query_and_moves_to_the_grid() {
    let mut s = Session::new(0);
    s.keys(":select name, age from people order by age\n");
    assert!(s.shows("Linus") || s.shows("Li↵nus"));
    assert!(s.status().contains("row 1/3 · col 1/2"));

    // Focus is now on the grid, so motions move the cell cursor.
    s.keys("jl");
    assert!(s.status().contains("row 2/3 · col 2/2"));
    s.keys("gg0");
    assert!(s.status().contains("row 1/3 · col 1/2"));
    s.keys("G$");
    assert!(s.status().contains("row 3/3 · col 2/2"));
}

#[test]
fn counts_and_paging_move_the_grid_cursor() {
    let mut s = Session::new(500);
    s.keys(":select n from numbers\n");
    s.keys("12j");
    assert!(s.status().contains("row 13/500"));
    s.keys("100G");
    assert!(s.status().contains("row 100/500"));
    s.screen(); // paging uses the height from the last draw
    s.ctrl('d');
    assert!(s.status().contains("row 108/500"));
    s.ctrl('u');
    assert!(s.status().contains("row 100/500"));
    s.keys("G");
    assert!(s.shows("500"));
}

#[test]
fn writes_are_refused_and_reported() {
    let mut s = Session::new(0);
    s.keys(":delete from people\n");
    assert!(s.status().contains("refused (read-only)"));
    s.keys(":select count(*) from people\n");
    assert!(s.shows("3"));
}

#[test]
fn truncation_is_announced() {
    let mut s = Session::new(exec::ROW_CAP + 1);
    s.keys(":select n from numbers\n");
    assert!(s.status().contains("TRUNCATED at row cap"));
}

#[test]
fn an_error_keeps_the_previous_result() {
    let mut s = Session::new(0);
    s.keys(":select name from people\n");
    s.keys(":select nope from nowhere\n");
    assert!(s.status().contains("no such table"));
    assert!(s.shows("Grace"));
}

#[test]
fn prompt_editing_and_escape() {
    let mut s = Session::new(0);
    s.keys(":select 1 junk");
    s.ctrl('w');
    assert!(s.status().starts_with(":select 1 "));
    s.ctrl('u');
    assert_eq!(s.status().trim_end(), ":");
    s.press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(s.status().contains("READ-ONLY"));
}

#[test]
fn panes_switch_with_tab_and_ctrl_w() {
    let mut s = Session::new(0);
    s.keys(":select n from (select 1 as n union all select 2)\n");
    s.ctrl('w');
    s.keys("h"); // catalog
    s.keys("j");
    assert!(s.status().contains("row 1/2")); // grid cursor did not move
    s.keys("\tj"); // back to the grid
    assert!(s.status().contains("row 2/2"));
}

#[test]
fn q_quits() {
    let mut s = Session::new(0);
    assert!(!s.app.quit);
    s.keys("q");
    assert!(s.app.quit);
}
