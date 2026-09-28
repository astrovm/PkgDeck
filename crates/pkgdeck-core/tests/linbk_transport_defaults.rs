use pkgdeck_core::{backends::Transport, host::AptAction, process::*};

/// A fixture for an adapter that drives no system package manager.
struct Bare;
impl Transport for Bare {}

fn disabled(result: Result<Completion, ExecutionError>) -> String {
    match result {
        Err(ExecutionError::Disabled(reason)) => reason,
        other => panic!("expected a disabled manager, got {other:?}"),
    }
}

#[test]
fn fixtures_without_a_manager_report_it_missing_instead_of_running_it() {
    let cancel = Cancellation::default();
    assert_eq!(
        disabled(Bare.apt_query("installed", "", "", &cancel)),
        "APT not found"
    );
    assert_eq!(
        disabled(Bare.apt_write(AptAction::Refresh, &cancel)),
        "APT not found"
    );
    assert_eq!(
        disabled(Bare.brew(&["list".into()], &cancel, false)),
        "Homebrew not found"
    );
    assert_eq!(
        disabled(Bare.flatpak(&["list".into()], &cancel, false, true)),
        "Flatpak not found"
    );
}
