//! Development-only probe. Run in a disposable VM; never installed with PkgDeck.
use pkgdeck_core::{
    host::{AptAction, Authorization, Host},
    process::Cancellation,
};
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        eprintln!("usage: apt-probe sudo|polkit install|remove PACKAGE (disposable VM only)");
        std::process::exit(2);
    }
    let authorization = match args[0].as_str() {
        "sudo" => Authorization::SudoNonInteractive,
        "polkit" => Authorization::Polkit,
        _ => {
            eprintln!("invalid authorization");
            std::process::exit(2);
        }
    };
    let action = match args[1].as_str() {
        "install" => AptAction::Install(args[2].clone()),
        "remove" => AptAction::Remove(args[2].clone()),
        _ => {
            eprintln!("invalid operation");
            std::process::exit(2);
        }
    };
    match Host::current().apt(action, authorization, &Cancellation::default()) {
        Ok(result) => print!("{}", String::from_utf8_lossy(&result.stdout)),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
