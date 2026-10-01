fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let failed = pkgdeck_core::unattended::run_runner(&args)
        .inspect_err(|error| eprintln!("pkgdeck-host-runner: {error}"))
        .is_err();
    std::process::ExitCode::from(u8::from(failed))
}
