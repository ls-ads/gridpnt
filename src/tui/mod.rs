//! The terminal interface.
//!
//! One loop, one channel. Key presses (from a small input thread) and
//! worker responses arrive on the same channel; the loop sleeps until one
//! does, updates `App`, and redraws. Nothing polls, so an idle gridpnt
//! uses no CPU. (PERF-002, PERF-004)

mod app;
mod catalog;
mod grid;
mod text;
mod theme;

use std::io::{self, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

pub use app::App;

use crate::backend::Backend;
use crate::remote;
use crate::worker::{Response, Worker};

enum Message {
    Input(Event),
    Worker(Response),
    /// SQL sent by `gridpnt run`.
    Remote(String),
}

/// Run the interface until the user quits.
/// `env` is the connection's environment label, if it has one.
pub fn run(backend: Box<dyn Backend>, env: Option<String>) -> io::Result<()> {
    let label = backend.label();
    let mut app = App::new(label.clone(), env);
    let (sender, inbox) = mpsc::channel::<Message>();

    // Accept statements from `gridpnt run`. If listening is not possible
    // gridpnt still works; only the editor hand-off is unavailable.
    let _listener = {
        let sender = sender.clone();
        remote::listen(&label, move |sql| {
            let _ = sender.send(Message::Remote(sql));
        })
        .ok()
    };

    let worker = {
        let sender = sender.clone();
        Worker::spawn(backend, move |response| {
            let _ = sender.send(Message::Worker(response));
        })
    };

    // `init` switches the terminal to full-screen raw mode and makes sure
    // it is put back even if gridpnt panics.
    let mut terminal = ratatui::init();

    thread::spawn(move || {
        while let Ok(input) = event::read() {
            if sender.send(Message::Input(input)).is_err() {
                break;
            }
        }
    });

    let outcome = main_loop(&mut terminal, &mut app, &worker, &inbox);
    ratatui::restore();
    outcome
}

fn main_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    worker: &Worker,
    inbox: &Receiver<Message>,
) -> io::Result<()> {
    loop {
        for request in app.take_requests() {
            worker.send(request);
        }
        if app.take_cancel() {
            worker.cancel();
        }
        if let Some(copied) = app.take_clipboard() {
            copy_to_clipboard(&copied)?;
        }
        if app.quit {
            return Ok(());
        }
        terminal.draw(|frame| app.draw(frame))?;

        // Sleep until something happens, then handle everything that has
        // queued up before drawing once.
        let Ok(first) = inbox.recv() else { return Ok(()) };
        for message in std::iter::once(first).chain(inbox.try_iter()) {
            match message {
                // Windows reports key releases too; only presses count.
                Message::Input(Event::Key(key)) if key.kind == KeyEventKind::Press => app.on_key(key),
                Message::Input(_) => {} // a resize just needs the redraw
                Message::Worker(response) => app.on_response(response),
                Message::Remote(sql) => app.on_remote(sql),
            }
        }
    }
}

/// Ask the terminal to put `content` on the system clipboard, using the
/// OSC 52 escape sequence. It travels through SSH, which a local clipboard
/// tool could not. Inside tmux it needs `set -g set-clipboard on`.
fn copy_to_clipboard(content: &str) -> io::Result<()> {
    let mut out = io::stdout();
    write!(out, "\x1b]52;c;{}\x07", text::base64(content.as_bytes()))?;
    out.flush()
}
