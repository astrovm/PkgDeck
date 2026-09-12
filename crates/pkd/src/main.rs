use std::io::{self, IsTerminal};

use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::widgets::{Block, Paragraph, Wrap};

#[derive(Parser)]
#[command(version = pkgdeck_core::VERSION, about = "PkgDeck terminal interface (foundation build)")]
struct Args {}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    Args::parse();
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!(
            "pkd requires a terminal when run without arguments; use --help for available options."
        );
        std::process::exit(2);
    }
    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result?;
    Ok(())
}

fn run(terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
    loop {
        terminal.draw(|frame| {
            frame.render_widget(
                Paragraph::new(format!(
                    "{}\n\nPress q or Esc to quit.",
                    pkgdeck_core::FOUNDATION_MESSAGE
                ))
                .block(Block::bordered().title("PkgDeck"))
                .wrap(Wrap { trim: true }),
                frame.area(),
            );
        })?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press
                && (matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)))
            {
                return Ok(());
            }
        }
    }
}
