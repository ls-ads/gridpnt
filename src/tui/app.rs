//! Everything the interface knows, and how keys change it.
//!
//! `App` never touches the terminal or a database. Keys and worker
//! responses go in; requests for the worker come out through
//! `take_requests`. That keeps it easy to test.
//!
//! Keys are fixed and not configurable. (NAV-004)

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Block;

use super::catalog::Catalog;
use super::grid::Grid;
use super::{text, theme};
use crate::worker::{Request, Response};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Catalog,
    Grid,
}

/// A key that is waiting for the key after it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// `g` was pressed; `gg` jumps to the top.
    G,
    /// `Ctrl-w` was pressed; `h`, `l`, or `w` chooses a pane.
    Window,
}

pub struct App {
    /// Connection name shown in the status line.
    label: String,
    focus: Focus,
    catalog: Catalog,
    grid: Grid,
    /// The `:` line being typed, if open.
    prompt: Option<String>,
    /// Latest note for the status line, and whether it is an error.
    message: Option<(String, bool)>,
    /// True while the worker is running a query.
    running: bool,
    /// Whether the running query came from the prompt (so focus should move
    /// to the result when it arrives).
    from_prompt: bool,
    /// Digits typed before a motion, as in `5j`.
    count: String,
    pending: Option<Pending>,
    requests: Vec<Request>,
    cancel_requested: bool,
    pub quit: bool,
}

impl App {
    pub fn new(label: String) -> Self {
        App {
            label,
            focus: Focus::Catalog,
            catalog: Catalog::default(),
            grid: Grid::default(),
            prompt: None,
            message: None,
            running: false,
            from_prompt: false,
            count: String::new(),
            pending: None,
            // Ask for the top level of the catalog straight away.
            requests: vec![Request::Children { path: Vec::new() }],
            cancel_requested: false,
            quit: false,
        }
    }

    /// Requests queued since the last call, for the worker.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// Whether the user asked to cancel since the last call.
    pub fn take_cancel(&mut self) -> bool {
        std::mem::take(&mut self.cancel_requested)
    }

    pub fn on_response(&mut self, response: Response) {
        match response {
            Response::Children { path, result } => match result {
                Ok(nodes) => self.catalog.insert_children(&path, nodes),
                Err(error) => self.message = Some((error, true)),
            },
            Response::Ran { result, elapsed } => {
                self.running = false;
                match result {
                    Ok(rows) => {
                        self.grid.set_result(rows, elapsed);
                        self.message = None;
                        if self.from_prompt {
                            self.focus = Focus::Grid;
                        }
                    }
                    // The previous result stays on screen after an error.
                    Err(error) => self.message = Some((error, true)),
                }
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.prompt.is_some() {
            self.on_prompt_key(key);
            return;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let pending = self.pending.take();

        if ctrl && key.code == KeyCode::Char('c') {
            if self.running {
                self.cancel_requested = true;
            } else {
                self.message = Some(("press q to quit".into(), false));
            }
            return;
        }

        if pending == Some(Pending::Window) {
            match key.code {
                KeyCode::Char('h') => self.focus = Focus::Catalog,
                KeyCode::Char('l') => self.focus = Focus::Grid,
                KeyCode::Char('w') => self.toggle_focus(),
                _ => {}
            }
            return;
        }

        // Digits build up a count. A leading 0 is not a count; in the grid
        // it means "first column".
        if let KeyCode::Char(digit @ '0'..='9') = key.code
            && !ctrl
            && (digit != '0' || !self.count.is_empty())
        {
            self.count.push(digit);
            return;
        }
        let count: Option<usize> = std::mem::take(&mut self.count).parse().ok();
        let times = count.unwrap_or(1) as isize;

        match (key.code, ctrl) {
            (KeyCode::Char('q'), false) => self.quit = true,
            (KeyCode::Char(':'), false) => self.prompt = Some(String::new()),
            (KeyCode::Tab, _) => self.toggle_focus(),
            (KeyCode::Char('w'), true) => self.pending = Some(Pending::Window),

            (KeyCode::Char('j') | KeyCode::Down, false) => self.move_rows(times),
            (KeyCode::Char('k') | KeyCode::Up, false) => self.move_rows(-times),
            (KeyCode::Char('d'), true) => self.move_rows(times * self.half_page()),
            (KeyCode::Char('u'), true) => self.move_rows(-times * self.half_page()),
            (KeyCode::Char('g'), false) if pending == Some(Pending::G) => self.move_to_row(0),
            (KeyCode::Char('g'), false) => self.pending = Some(Pending::G),
            // `G` goes to the last row; `12G` goes to row 12.
            (KeyCode::Char('G'), false) => self.move_to_row(count.map_or(usize::MAX, |n| n.saturating_sub(1))),

            (code, false) => match self.focus {
                Focus::Catalog => self.on_catalog_key(code),
                Focus::Grid => self.on_grid_key(code, times),
            },
            _ => {}
        }
    }

    fn on_catalog_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('l') | KeyCode::Right => {
                if let Some(path) = self.catalog.open() {
                    self.requests.push(Request::Children { path });
                }
            }
            KeyCode::Char('h') | KeyCode::Left => self.catalog.close(),
            KeyCode::Enter => {
                if let Some(path) = self.catalog.current_relation() {
                    self.start_query(Request::Preview { path }, false);
                }
            }
            _ => {}
        }
    }

    fn on_grid_key(&mut self, code: KeyCode, times: isize) {
        match code {
            KeyCode::Char('l' | 'w') | KeyCode::Right => self.grid.move_cols(times),
            KeyCode::Char('h' | 'b') | KeyCode::Left => self.grid.move_cols(-times),
            KeyCode::Char('0') => self.grid.move_to_col(0),
            KeyCode::Char('$') => self.grid.move_to_col(usize::MAX),
            _ => {}
        }
    }

    fn on_prompt_key(&mut self, key: KeyEvent) {
        let Some(line) = &mut self.prompt else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match (key.code, ctrl) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), true) => self.prompt = None,
            (KeyCode::Enter, _) => {
                let sql = self.prompt.take().unwrap_or_default();
                if !sql.trim().is_empty() {
                    self.start_query(Request::Run { sql }, true);
                }
            }
            // Backspace on an empty line closes the prompt, as in vim.
            (KeyCode::Backspace, _) if line.is_empty() => self.prompt = None,
            (KeyCode::Backspace, _) => {
                line.pop();
            }
            (KeyCode::Char('u'), true) => line.clear(),
            (KeyCode::Char('w'), true) => {
                // Delete back to the start of the previous word.
                let kept = line.trim_end().rfind(char::is_whitespace).map_or(0, |i| i + 1);
                line.truncate(kept);
            }
            (KeyCode::Char(c), false) => line.push(c),
            _ => {}
        }
    }

    /// Queue a query unless one is already running. One at a time keeps
    /// the load on the database predictable.
    fn start_query(&mut self, request: Request, from_prompt: bool) {
        if self.running {
            self.message = Some(("a query is already running (Ctrl-c cancels it)".into(), true));
            return;
        }
        self.running = true;
        self.from_prompt = from_prompt;
        self.message = Some(("running… Ctrl-c cancels".into(), false));
        self.requests.push(request);
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Catalog => Focus::Grid,
            Focus::Grid => Focus::Catalog,
        };
    }

    fn half_page(&self) -> isize {
        (match self.focus {
            Focus::Catalog => self.catalog.half_page(),
            Focus::Grid => self.grid.half_page(),
        }) as isize
    }

    fn move_rows(&mut self, delta: isize) {
        match self.focus {
            Focus::Catalog => self.catalog.move_by(delta),
            Focus::Grid => self.grid.move_rows(delta),
        }
    }

    fn move_to_row(&mut self, index: usize) {
        match self.focus {
            Focus::Catalog if index == usize::MAX => self.catalog.move_to_end(),
            Focus::Catalog => self.catalog.move_to(index),
            Focus::Grid => self.grid.move_to_row(index),
        }
    }

    pub fn draw(&mut self, frame: &mut Frame) {
        let [panes, status_line] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
        let catalog_width = (frame.area().width / 4).clamp(20, 40);
        let [catalog_area, grid_area] =
            Layout::horizontal([Constraint::Length(catalog_width), Constraint::Min(1)]).areas(panes);

        let catalog_inner = draw_frame(frame, catalog_area, "catalog", self.focus == Focus::Catalog);
        self.catalog
            .draw(catalog_inner, frame.buffer_mut(), self.focus == Focus::Catalog);

        let grid_inner = draw_frame(frame, grid_area, "result", self.focus == Focus::Grid);
        self.grid
            .draw(grid_inner, frame.buffer_mut(), self.focus == Focus::Grid);

        match &self.prompt {
            Some(line) => draw_prompt(frame, status_line, line),
            None => self.draw_status(frame, status_line),
        }
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let buf = frame.buffer_mut();
        let width = area.width as usize;

        // The read-only badge is always visible. (SAFE-001)
        let badge = Style::new()
            .fg(theme::CRUST)
            .bg(theme::GREEN)
            .add_modifier(Modifier::BOLD);
        let (x, _) = buf.set_stringn(area.x, area.y, " READ-ONLY ", width, badge);
        let label = format!(" {} ", text::clean(&self.label, 60));
        let (x, _) = buf.set_stringn(x, area.y, label, width, Style::new().fg(theme::SUBTEXT));

        // The result summary sits at the right edge; a truncation warning
        // goes in front of it in a colour that cannot be missed.
        let mut right_edge = area.right();
        if let Some((summary, truncated)) = self.grid.summary() {
            let warning = if truncated { " TRUNCATED at row cap " } else { "" };
            let needed = (text::width(&summary) + text::width(warning) + 1) as u16;
            if needed < area.right().saturating_sub(x) {
                right_edge = area.right() - needed;
                let warn_style = Style::new()
                    .fg(theme::CRUST)
                    .bg(theme::PEACH)
                    .add_modifier(Modifier::BOLD);
                let (after_warning, _) = buf.set_stringn(right_edge, area.y, warning, width, warn_style);
                buf.set_string(after_warning + 1, area.y, summary, Style::new().fg(theme::SUBTEXT));
            }
        }

        if let Some((message, is_error)) = &self.message {
            let colour = if *is_error { theme::RED } else { theme::SUBTEXT };
            let room = right_edge.saturating_sub(x + 1) as usize;
            let message = text::fit(&text::clean(message, room + 1), room);
            buf.set_stringn(x, area.y, message, room, Style::new().fg(colour));
        }
    }
}

/// Draw a pane's border and title, and return the area inside it.
fn draw_frame(frame: &mut Frame, area: Rect, title: &str, focused: bool) -> Rect {
    let colour = if focused { theme::MAUVE } else { theme::SURFACE2 };
    let block = Block::bordered()
        .title(format!(" {title} "))
        .border_style(Style::new().fg(colour));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

fn draw_prompt(frame: &mut Frame, area: Rect, line: &str) {
    // When the line is longer than the screen, show its end.
    let room = (area.width as usize).saturating_sub(2);
    let chars: Vec<char> = text::clean(line, usize::MAX).chars().collect();
    let mut start = chars.len();
    let mut used = 0;
    while start > 0 {
        let w = text::width(&chars[start - 1].to_string());
        if used + w > room {
            break;
        }
        used += w;
        start -= 1;
    }
    let shown: String = chars[start..].iter().collect();

    let buf = frame.buffer_mut();
    buf.set_string(area.x, area.y, ":", Style::new().fg(theme::MAUVE));
    buf.set_string(area.x + 1, area.y, shown, Style::new().fg(theme::TEXT));
    frame.set_cursor_position((area.x + 1 + used as u16, area.y));
}
