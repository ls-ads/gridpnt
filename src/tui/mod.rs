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

use std::io;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

pub use app::App;

use crate::backend::Backend;
use crate::worker::{Response, Worker};

enum Message {
    Input(Event),
    Worker(Response),
}

/// Run the interface until the user quits.
pub fn run(backend: Box<dyn Backend>) -> io::Result<()> {
    let mut app = App::new(backend.label());
    let (sender, inbox) = mpsc::channel::<Message>();

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
            }
        }
    }
}
