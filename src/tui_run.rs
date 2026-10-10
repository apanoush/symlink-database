//! Terminal lifecycle and event loop for `lndb tui`.
//!
//! This module is compiled into the binary only (declared from `main.rs`), so
//! the library never performs terminal I/O. `tui::draw` and `tui::handle_key`
//! stay pure; everything crossterm-specific lives here.
//!
//! The terminal is restored through an RAII guard ([`TerminalGuard`]), so raw
//! mode and the alternate screen are left even if the loop panics.

use std::io::{self, IsTerminal, Stdout};
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};

use symlink_database::config::Config;
use symlink_database::database::Database;
use symlink_database::stats;
use symlink_database::tui::{self, Action, App};

/// Open the interactive dashboard. Requires an interactive stdout.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    if !io::stdout().is_terminal() {
        return Err("lndb tui requires an interactive terminal (stdout is not a TTY)".into());
    }

    let config = Config::from_config_file()?;
    let database = Database::new(&config.paths.database)?;
    let initial = stats::collect(database.connection(), stats::DEFAULT_DEPTH)?;
    let mut app = App::new(
        initial,
        config.paths.root.display().to_string(),
        config.paths.database.display().to_string(),
    );

    let mut guard = TerminalGuard::new()?;
    while !app.should_quit {
        guard.terminal.draw(|frame| tui::draw(frame, &app))?;

        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match tui::handle_key(&mut app, key) {
                Action::Refresh => {
                    let snapshot = stats::collect(database.connection(), app.depth)?;
                    app.refresh(snapshot);
                }
                Action::None => {}
            }
        }
    }
    Ok(())
}

/// Owns the terminal in raw mode plus the alternate screen; restores it on drop.
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}
