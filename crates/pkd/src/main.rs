use clap::{CommandFactory, Parser};
mod cli;
mod live;
use cli::{Args, Commands};
mod presentation;
mod session;

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
    doctor_with(
        &pkgdeck_core::host::Host::current(),
        std::path::Path::new("/usr/bin/uname"),
        cli::color_for(&std::io::stdout()),
        &mut std::io::stdout().lock(),
    )
}

fn doctor_with(
    host: &pkgdeck_core::host::Host,
    uname: &std::path::Path,
    color: bool,
    out: &mut dyn std::io::Write,
) -> Result<(), Box<dyn std::error::Error>> {
    use pkgdeck_core::process::{Cancellation, ExecutionError, Limits};
    let paint = |code: &str, text: &str| {
        if color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    };
    let runtime = format!("{:?}", host.runtime).to_lowercase();
    // Shown even when the machine can't be read, as the reason why.
    writeln!(out, "{}  {runtime}", paint("2", "Runtime     "))?;
    let mut text = String::new();
    let cancel = Cancellation::default();
    let result = host.read(uname, &["-m".into()], Limits::default(), &cancel)?;
    if result.code != Some(0) {
        return Err(ExecutionError::Failed(result).into());
    }
    let architecture = String::from_utf8_lossy(&result.stdout);
    text += &format!(
        "{}  {}\n\n",
        paint("2", "Architecture"),
        architecture.trim()
    );
    let width = pkgdeck_core::host::BACKENDS
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    let mut found = 0;
    for (name, executable) in pkgdeck_core::host::BACKENDS {
        text += &match host.resolve(executable)? {
            Some(path) => {
                found += 1;
                let path = paint("2", &path.display().to_string());
                format!("{} {name:width$}  {path}\n", paint("32", "✓"))
            }
            None => format!("{}\n", paint("2", &format!("- {name:width$}  not found"))),
        };
    }
    let summary = format!(
        "{found} package managers found. PkgDeck uses every one it finds. \
         pip also needs an active virtual environment (VIRTUAL_ENV). \
         `pkd sources` also lists sources without a command here, such as \
         installed apps and standalone tools."
    );
    text += &format!("\n{}\n", paint("2", &summary));
    out.write_all(text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pkgdeck_core::host::{Host, Runtime};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn doctor_lists_found_managers_and_fails_when_the_machine_cannot_be_read() {
        let root = std::env::temp_dir().join(format!("pkd-doctor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let script = |name: &str, body: &str| {
            let path = root.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        script("brew", "exit 0");
        let uname = script("uname", "echo synthetic-arch");
        let host = Host::new(
            Runtime::Native,
            [("PATH".into(), root.as_os_str().to_owned())].into(),
        );
        let mut plain = Vec::new();
        doctor_with(&host, &uname, false, &mut plain).unwrap();
        let plain = String::from_utf8(plain).unwrap();
        assert!(plain.starts_with("Runtime       native\n"), "{plain}");
        assert!(plain.contains("Architecture  synthetic-arch\n"), "{plain}");
        assert!(plain.contains("✓ Homebrew "), "{plain}");
        assert!(
            plain.contains(&root.join("brew").display().to_string()),
            "{plain}"
        );
        assert!(plain.contains("- APT "), "{plain}");
        assert!(plain.contains("\n1 package managers found."), "{plain}");
        assert!(!plain.contains('\x1b'));
        let mut colored = Vec::new();
        doctor_with(&host, &uname, true, &mut colored).unwrap();
        let colored = String::from_utf8(colored).unwrap();
        assert!(colored.contains("\x1b[32m✓\x1b[0m"), "{colored}");
        let broken = script("broken-uname", "exit 3");
        let error = doctor_with(&host, &broken, false, &mut Vec::new()).unwrap_err();
        assert!(error.to_string().contains("exited with code 3"), "{error}");
        std::fs::remove_dir_all(root).unwrap();
    }
}
