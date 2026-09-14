use std::io::{self, IsTerminal};

use clap::Parser;
mod cli;
use cli::{Args, Commands};
mod presentation;
mod tui;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if matches!(args.command, Some(Commands::Doctor)) && !args.json {
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
        println!("APT and Homebrew commands are available; pip requires an explicitly selected virtual environment.");
        return Ok(());
    }
    if args.command.is_some() {
        std::process::exit(cli::run(&args).into());
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!(
            "pkd requires a terminal when run without arguments; use --help for available options."
        );
        std::process::exit(2);
    }
    let mut terminal = ratatui::init();
    let result = tui::run(&mut terminal, &args);
    ratatui::restore();
    result?;
    Ok(())
}
