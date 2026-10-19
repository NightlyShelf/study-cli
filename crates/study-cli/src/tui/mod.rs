//! `study` with no subcommand: the TUI (SPEC §4). The loop is plain:
//! draw, poll a key, drain background messages, repeat.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use crate::app;

mod calendar;
mod draw;
mod keys;
mod reader;
mod state;
mod tasks;
mod todo;
mod tree;

use state::App;

pub async fn run() -> Result<()> {
    let config = app::load_config()?;
    let index = app::load_index()?;
    // Not being logged in is fine for browsing; the first network action says so.
    let client = app::moodle_client(&config).ok();
    let graph = app::graph_client(&config).ok();
    let mut app = App::new(config, index, client, graph, std::env::current_dir()?);
    if app.index.is_stale(app::STALE_AFTER_SECS, app::now()) {
        tasks::spawn_sync(&mut app);
    }

    // `ratatui::init` installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    while !app.quit {
        let size = terminal.size()?;
        app.width = size.width;
        app.height = size.height;
        terminal.draw(|frame| draw::draw(frame, app))?;
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            keys::handle(app, key);
        }
        while let Ok(msg) = app.rx.try_recv() {
            tasks::apply(app, msg);
        }
    }
    Ok(())
}
