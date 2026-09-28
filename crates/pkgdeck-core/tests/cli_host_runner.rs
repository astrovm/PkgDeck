use std::process::{Command, Stdio};

#[test]
fn host_runner_reports_why_it_refuses_and_exits_nonzero() {
    let output = Command::new(env!("CARGO_BIN_EXE_pkgdeck-host-runner"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    // Root gets past this check and fails on the missing plan instead.
    let root = rustix::process::geteuid().is_root();
    assert!(stderr.starts_with("pkgdeck-host-runner: "), "{stderr}");
    assert!(
        root || stderr.ends_with(": host runner requires root\n"),
        "{stderr}"
    );
}
