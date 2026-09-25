use clap::{CommandFactory, Parser};
mod cli;
mod live;
use cli::{Args, Commands};
mod presentation;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if matches!(args.command, Some(Commands::Doctor)) && !args.json {
        return doctor();
    }
    if args.command.is_some() {
        std::process::exit(cli::run(&args).into());
    }
    Args::command().print_help()?;
    println!();
    Ok(())
}

fn doctor() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::IsTerminal;
    let color = std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").is_ok_and(|term| term != "dumb");
    let paint = |code: &str, text: &str| {
        if color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    };
    let host = pkgdeck_core::host::Host::current();
    let runtime = format!("{:?}", host.runtime).to_lowercase();
    println!("{}  {runtime}", paint("2", "Runtime     "));
    if let Some(reason) = host.runtime.disabled_reason() {
        println!("\n{} {reason}", paint("31", "✗"));
        return Ok(());
    }
    let result = host.read(
        std::path::Path::new("/usr/bin/uname"),
        &["-m".into()],
        pkgdeck_core::process::Limits::default(),
        &pkgdeck_core::process::Cancellation::default(),
    )?;
    if result.code != Some(0) {
        return Err(pkgdeck_core::process::ExecutionError::Failed(result).into());
    }
    println!(
        "{}  {}\n",
        paint("2", "Architecture"),
        String::from_utf8_lossy(&result.stdout).trim()
    );
    let width = pkgdeck_core::host::BACKENDS
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    let mut found = 0;
    for (name, executable) in pkgdeck_core::host::BACKENDS {
        match host.resolve(executable)? {
            Some(path) => {
                found += 1;
                println!(
                    "{} {name:width$}  {}",
                    paint("32", "✓"),
                    paint("2", &path.display().to_string())
                );
            }
            None => println!("{}", paint("2", &format!("- {name:width$}  not found"))),
        }
    }
    println!(
        "\n{}",
        paint(
            "2",
            &format!(
                "{found} package managers found. PkgDeck uses every one it finds. \
                 pip also needs an active virtual environment (VIRTUAL_ENV)."
            )
        )
    );
    Ok(())
}
