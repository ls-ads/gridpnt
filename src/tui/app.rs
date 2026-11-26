//! Everything the interface knows, and how keys change it.
//!
//! `App` never touches the terminal or a database. Keys and worker
//! responses go in; requests for the worker and text for the clipboard
//! come out through the `take_*` methods. That keeps it easy to test.
//!
//! Keys are fixed and not configurable. (NAV-004)

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use super::catalog::Catalog;
use super::grid::Grid;
use super::{text, theme};
use crate::worker::{Request, Response};

/// Shown by `?`. Kept in step with `on_key` by hand.
const HELP: &str = "\
Everywhere
  j k            down / up              gg G     first / last row
  Ctrl-d Ctrl-u  half page down / up    5j 12G   counts work
  /              search                 n N      next / previous match
  Tab            other pane             Ctrl-w h/l   catalog / result
  :              run a query            Ctrl-c   cancel the running query
  ?              this help              q        quit

Catalog
  l  h           open / close           Enter    preview table
  y              copy name              R        re-read from the database

Result
  h l  w b       left / right           0 $      first / last column
  Enter          show the whole cell    y  Y     copy cell / row

Whole cell view
  j k            scroll                 y        copy
  q Esc Enter    close";

/// The terminal's clipboard takes a limited amount of text.
const MAX_YANK_BYTES: usize = 70_000;
/// The most text the whole-cell view will lay out.
const MAX_INSPECT_CHARS: usize = 200_000;

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

/// The line being typed at the bottom of the screen.
struct Prompt {
    kind: PromptKind,
    line: String,
    /// Where the cursor was when a search began, to return to on Esc.
    origin: (usize, usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    /// `:` — SQL to run.
    Query,
    /// `/` — text to find in the focused pane.
    Search,
}

/// Something drawn over the panes until dismissed.
enum Overlay {
    Help,
    /// The whole of one cell, scrolled down by `scroll` lines.
    Cell {
        content: String,
        scroll: u16,
    },
}

pub struct App {
    /// Connection name shown in the status line.
    label: String,
    /// Environment label such as `prod`, always shown. (SAFE-007)
    env: Option<String>,
    focus: Focus,
    catalog: Catalog,
    grid: Grid,
    prompt: Option<Prompt>,
    overlay: Option<Overlay>,
    /// The last search, repeated by `n` and `N`.
    last_search: Option<String>,
    /// Latest note for the status line, and whether it is an error.
    message: Option<(String, bool)>,
    /// True while the worker is running a query.
    running: bool,
    /// Whether focus should move to the result when the query finishes.
    focus_result: bool,
    /// Digits typed before a motion, as in `5j`.
    count: String,
    pending: Option<Pending>,
    requests: Vec<Request>,
    cancel_requested: bool,
    clipboard: Option<String>,
    pub quit: bool,
}

impl App {
    pub fn new(label: String, env: Option<String>) -> Self {
        App {
            label,
            env,
            focus: Focus::Catalog,
            catalog: Catalog::default(),
            grid: Grid::default(),
            prompt: None,
            overlay: None,
            last_search: None,
            message: None,
            running: false,
            focus_result: false,
            count: String::new(),
            pending: None,
            // Ask for the top level of the catalog straight away.
            requests: vec![Request::Children {
                path: Vec::new(),
                refresh: false,
            }],
            cancel_requested: false,
            clipboard: None,
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

    /// Text the user copied since the last call, for the clipboard.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard.take()
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
                        if self.focus_result {
                            self.focus = Focus::Grid;
                        }
                    }
                    // The previous result stays on screen after an error.
                    Err(error) => self.message = Some((error, true)),
                }
            }
        }
    }

    /// SQL sent from another program on this machine, such as an editor.
    /// It is treated exactly like SQL typed at the prompt.
    pub fn on_remote(&mut self, sql: String) {
        self.start_query(Request::Run { sql }, true);
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.prompt.is_some() {
            self.on_prompt_key(key);
            return;
        }
        if self.overlay.is_some() {
            self.on_overlay_key(key);
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
            (KeyCode::Char('?'), false) => self.overlay = Some(Overlay::Help),
            (KeyCode::Char(':'), false) => self.open_prompt(PromptKind::Query),
            (KeyCode::Char('/'), false) => self.open_prompt(PromptKind::Search),
            (KeyCode::Char('n'), false) => self.repeat_search(true),
            (KeyCode::Char('N'), false) => self.repeat_search(false),
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
                    self.requests.push(Request::Children { path, refresh: false });
                }
            }
            KeyCode::Char('h') | KeyCode::Left => self.catalog.close(),
            KeyCode::Char('R') => {
                if let Some(path) = self.catalog.refresh_target() {
                    self.requests.push(Request::Children { path, refresh: true });
                }
            }
            KeyCode::Char('y') => {
                if let Some(name) = self.catalog.current_name() {
                    self.yank(name);
                }
            }
            KeyCode::Enter => {
                if let Some(path) = self.catalog.current_relation() {
                    // Focus stays here so several tables can be previewed
                    // one after another.
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
            KeyCode::Char('y') => {
                if let Some(cell) = self.grid.current_cell() {
                    self.yank(cell);
                }
            }
            KeyCode::Char('Y') => {
                if let Some(row) = self.grid.current_row() {
                    self.yank(row);
                }
            }
            KeyCode::Enter => {
                if let Some(cell) = self.grid.current_cell() {
                    let content = text::clean_lines(&cell, MAX_INSPECT_CHARS);
                    self.overlay = Some(Overlay::Cell { content, scroll: 0 });
                }
            }
            _ => {}
        }
    }

    fn on_overlay_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Overlay::Cell { content, scroll }) = &mut self.overlay else {
            // Any key closes the help.
            self.overlay = None;
            return;
        };
        match (key.code, ctrl) {
            (KeyCode::Char('j') | KeyCode::Down, false) => *scroll = scroll.saturating_add(1),
            (KeyCode::Char('k') | KeyCode::Up, false) => *scroll = scroll.saturating_sub(1),
            (KeyCode::Char('d'), true) => *scroll = scroll.saturating_add(10),
            (KeyCode::Char('u'), true) => *scroll = scroll.saturating_sub(10),
            (KeyCode::Char('g'), false) => *scroll = 0,
            (KeyCode::Char('y'), false) => {
                let copy = content.clone();
                self.yank(copy);
            }
            (KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter, _) => self.overlay = None,
            _ => {}
        }
    }

    fn open_prompt(&mut self, kind: PromptKind) {
        self.prompt = Some(Prompt {
            kind,
            line: String::new(),
            origin: self.position(),
        });
    }

    fn on_prompt_key(&mut self, key: KeyEvent) {
        let Some(prompt) = &mut self.prompt else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let line = &mut prompt.line;
        let mut leaving = false;

        match (key.code, ctrl) {
            (KeyCode::Enter, _) => {
                self.finish_prompt();
                return;
            }
            (KeyCode::Esc, _) | (KeyCode::Char('c'), true) => leaving = true,
            // Backspace on an empty line leaves the prompt, as in vim.
            (KeyCode::Backspace, _) if line.is_empty() => leaving = true,
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
            _ => return,
        }
        let (kind, origin, pattern) = (prompt.kind, prompt.origin, prompt.line.clone());

        // A search moves the cursor as you type, always starting from
        // where the cursor was when `/` was pressed. Leaving puts it back.
        if kind == PromptKind::Search {
            self.set_position(origin);
            if !leaving && !pattern.is_empty() {
                self.find(&pattern, true, true);
            }
        }
        if leaving {
            self.prompt = None;
        }
    }

    fn finish_prompt(&mut self) {
        let Some(prompt) = self.prompt.take() else { return };
        if prompt.line.trim().is_empty() {
            return;
        }
        match prompt.kind {
            PromptKind::Query => self.start_query(Request::Run { sql: prompt.line }, true),
            PromptKind::Search => {
                self.set_position(prompt.origin);
                if !self.find(&prompt.line, true, true) {
                    self.message = Some((format!("not found: {}", prompt.line), true));
                }
                self.last_search = Some(prompt.line);
            }
        }
    }

    fn repeat_search(&mut self, forward: bool) {
        let Some(pattern) = self.last_search.clone() else {
            self.message = Some(("nothing to search for yet; press /".into(), false));
            return;
        };
        if !self.find(&pattern, forward, false) {
            self.message = Some((format!("not found: {pattern}"), true));
        }
    }

    /// Search the focused pane and move its cursor to the match.
    fn find(&mut self, pattern: &str, forward: bool, include_current: bool) -> bool {
        match self.focus {
            Focus::Catalog => match self.catalog.find(pattern, forward, include_current) {
                Some(index) => {
                    self.catalog.move_to(index);
                    true
                }
                None => false,
            },
            Focus::Grid => self.grid.find(pattern, forward, include_current),
        }
    }

    /// The cursor of the focused pane as (row, column).
    fn position(&self) -> (usize, usize) {
        match self.focus {
            Focus::Catalog => (self.catalog.cursor(), 0),
            Focus::Grid => self.grid.position(),
        }
    }

    fn set_position(&mut self, position: (usize, usize)) {
        match self.focus {
            Focus::Catalog => self.catalog.move_to(position.0),
            Focus::Grid => self.grid.set_position(position),
        }
    }

    fn yank(&mut self, content: String) {
        if content.len() > MAX_YANK_BYTES {
            self.message = Some(("too large to copy through the terminal".into(), true));
            return;
        }
        self.message = Some((format!("copied {} characters", content.chars().count()), false));
        self.clipboard = Some(content);
    }

    /// Queue a query unless one is already running. One at a time keeps
    /// the load on the database predictable.
    fn start_query(&mut self, request: Request, focus_result: bool) {
        if self.running {
            self.message = Some(("a query is already running (Ctrl-c cancels it)".into(), true));
            return;
        }
        self.running = true;
        self.focus_result = focus_result;
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

        match &self.overlay {
            Some(Overlay::Help) => draw_overlay(frame, panes, "keys", HELP, 0),
            Some(Overlay::Cell { content, scroll }) => draw_overlay(frame, panes, "cell", content, *scroll),
            None => {}
        }

        match &self.prompt {
            Some(prompt) => draw_prompt(frame, status_line, prompt),
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
        let (mut x, _) = buf.set_stringn(area.x, area.y, " READ-ONLY ", width, badge);

        if let Some(env) = &self.env {
            // Production is red so it cannot be mistaken for anything else.
            let is_prod = matches!(env.to_lowercase().as_str(), "prod" | "production");
            let colour = if is_prod { theme::RED } else { theme::BLUE };
            let style = Style::new().fg(theme::CRUST).bg(colour).add_modifier(Modifier::BOLD);
            let badge = format!(" {} ", text::clean(env, 20).to_uppercase());
            (x, _) = buf.set_stringn(x, area.y, badge, width, style);
        }
        let label = format!(" {} ", text::clean(&self.label, 60));
        let (x, _) = buf.set_stringn(x, area.y, label, width, Style::new().fg(theme::SUBTEXT));

        // The result summary sits at the right edge; a truncation warning
        // goes in front of it in a colour that cannot be missed.
        let mut right_edge = area.right();
        if let Some((summary, truncated)) = self.grid.summary() {
            let warning = truncated.map_or(String::new(), |rows| format!(" TRUNCATED at {rows} rows "));
            let needed = (text::width(&summary) + text::width(&warning) + 1) as u16;
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

        // With nothing else to say, describe the catalog row under the
        // cursor: its comment is too long to sit beside its name.
        let comment = match self.focus {
            Focus::Catalog => self.catalog.current_comment().map(|comment| (comment, theme::OVERLAY)),
            Focus::Grid => None,
        };
        let message = self.message.as_ref().map(|(message, is_error)| {
            let colour = if *is_error { theme::RED } else { theme::SUBTEXT };
            (message.as_str(), colour)
        });
        if let Some((message, colour)) = message.or(comment) {
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

/// Draw a box of text over the middle of `area`.
fn draw_overlay(frame: &mut Frame, area: Rect, title: &str, content: &str, scroll: u16) {
    let margin_x = area.width / 10;
    let margin_y = area.height / 10;
    let boxed = Rect::new(
        area.x + margin_x,
        area.y + margin_y,
        area.width - 2 * margin_x,
        area.height - 2 * margin_y,
    );
    let block = Block::bordered()
        .title(format!(" {title} "))
        .border_style(Style::new().fg(theme::MAUVE));
    let paragraph = Paragraph::new(content)
        .style(Style::new().fg(theme::TEXT))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0))
        .block(block);
    frame.render_widget(Clear, boxed);
    frame.render_widget(paragraph, boxed);
}

fn draw_prompt(frame: &mut Frame, area: Rect, prompt: &Prompt) {
    let sign = match prompt.kind {
        PromptKind::Query => ":",
        PromptKind::Search => "/",
    };
    // When the line is longer than the screen, show its end.
    let room = (area.width as usize).saturating_sub(2);
    let chars: Vec<char> = text::clean(&prompt.line, usize::MAX).chars().collect();
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
    buf.set_string(area.x, area.y, sign, Style::new().fg(theme::MAUVE));
    buf.set_string(area.x + 1, area.y, shown, Style::new().fg(theme::TEXT));
    frame.set_cursor_position((area.x + 1 + used as u16, area.y));
}
