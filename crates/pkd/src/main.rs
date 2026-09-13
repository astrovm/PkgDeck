use std::io::{self, IsTerminal};

use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::widgets::{Block, Paragraph, Wrap};

#[derive(Parser)]
#[command(version = pkgdeck_core::VERSION, about = "PkgDeck terminal interface (foundation build)")]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Report host execution availability and detected package-manager executables.
    Doctor,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if let Some(Commands::Doctor) = args.command {
        let host = pkgdeck_core::host::Host::current();
        println!("Runtime: {:?}", host.runtime);
        if let Some(reason) = host.runtime.disabled_reason() {
            println!("{reason}");
        } else {
            let result = host.read(
                std::path::Path::new("/usr/bin/uname"),
                &["--machine".into()],
                pkgdeck_core::process::Limits::default(),
                &pkgdeck_core::process::Cancellation::default(),
            )?;
            if result.code != Some(0) {
                return Err(pkgdeck_core::process::ExecutionError::Failed(result).into());
            }
            println!(
                "Host architecture: {}",
                String::from_utf8_lossy(&result.stdout).trim()
            );
            for (name, executable) in pkgdeck_core::host::BACKENDS {
                let path = host.resolve(executable)?;
                println!(
                    "{name}: {}",
                    path.map_or_else(|| "not found".into(), |p| p.display().to_string())
                );
            }
        }
        println!("Package operations are not exposed by the frontends yet. pip requires an explicitly selected virtual environment.");
        return Ok(());
    }
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
