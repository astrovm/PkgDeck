fn main() -> std::process::ExitCode {
    let failed = pkgdeck_core::batch::serve()
        .inspect_err(|error| eprintln!("pkgdeck-host-runner: {error}"))
        .is_err();
    std::process::ExitCode::from(u8::from(failed))
}
