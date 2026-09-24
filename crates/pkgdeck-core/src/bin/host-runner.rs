fn main() {
    if let Err(error) = pkgdeck_core::batch::serve() {
        eprintln!("pkgdeck-host-runner: {error}");
        std::process::exit(1);
    }
}
