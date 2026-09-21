use clap::{CommandFactory, Parser};
mod cli;
use cli::{Args, Commands};
mod presentation;

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
        println!("APT, Homebrew, Flatpak, Cargo, npm, pnpm, Bun, pip, pipx, uv, Composer, RubyGems, and local AppImage imports are available; pip requires an explicitly selected virtual environment (VIRTUAL_ENV).");
        return Ok(());
    }
    if args.command.is_some() {
        std::process::exit(cli::run(&args).into());
    }
    Args::command().print_help()?;
    println!();
    Ok(())
}
