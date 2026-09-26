use pkgdeck_core::{
    host::{classify_apt, AptAction, Authorization, Host, Runtime},
    process::{Cancellation, Completion, ExecutionError, Limits},
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

fn env(values: &[(&str, &str)]) -> BTreeMap<OsString, OsString> {
    values
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect()
}
fn host() -> Host {
    Host::new(Runtime::Native, env(&[("PATH", "/usr/bin:/bin")]))
}
fn shell(script: &str) -> Vec<OsString> {
    vec!["-c".into(), script.into()]
}
fn read(script: &str) -> Completion {
    host()
        .read(
            Path::new("/bin/sh"),
            &shell(script),
            Limits::default(),
            &Cancellation::default(),
        )
        .unwrap()
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-host-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn executable(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
/// Fixture executables are symlinks to system binaries, never freshly
/// written scripts: parallel write→exec of new files trips an ETXTBSY race
/// (observed at a few percent under thread churn, never single-threaded,
/// on both tmpfs and disk), while execs of pre-existing files never fail.
fn link_executable(dir: &Path, name: &str, target: &str) -> PathBuf {
    let path = dir.join(name);
    symlink(target, &path).unwrap();
    path
}

#[test]
fn formats_detect_native_and_bridged_runtimes() {
    assert_eq!(Runtime::detect(&env(&[]), false), Runtime::Native);
    assert_eq!(
        Runtime::detect(&env(&[("APPIMAGE", "/tmp/example")]), false),
        Runtime::AppImage
    );
    assert_eq!(
        Runtime::detect(&env(&[("APPDIR", "/tmp/example")]), false),
        Runtime::AppImage
    );
    assert_eq!(Runtime::detect(&env(&[]), true), Runtime::Flatpak);
    assert_eq!(
        Runtime::detect(&env(&[("FLATPAK_ID", "fixture")]), false),
        Runtime::Flatpak
    );
    assert_eq!(
        Runtime::detect(&env(&[("SNAP", "fixture"), ("APPIMAGE", "fixture")]), true),
        Runtime::Snap
    );
    assert!(Runtime::Flatpak.disabled_reason().is_none());
    let snap = Host::new(Runtime::Snap, env(&[("PATH", "/usr/bin:/bin")]));
    assert!(snap.resolve("sh").unwrap().is_some());
    assert!(Runtime::Native.disabled_reason().is_none());
    assert!(Runtime::AppImage.disabled_reason().is_none());
    assert!(Runtime::Snap.disabled_reason().is_none());
    let _ = Host::current();
}

#[test]
fn detection_skips_relative_empty_nonexecutable_and_bundled_paths() {
    let user = Fixture::new();
    let bundle = Fixture::new();
    user.executable("fixture");
    bundle.executable("bundled");
    symlink(bundle.0.join("bundled"), user.0.join("alias")).unwrap();
    fs::write(user.0.join("text"), "not executable").unwrap();
    fs::create_dir(user.0.join("directory")).unwrap();
    let path = format!(":.:relative:{}:{}", bundle.0.display(), user.0.display());
    let h = Host::new(
        Runtime::AppImage,
        env(&[("PATH", &path), ("APPDIR", bundle.0.to_str().unwrap())]),
    );
    assert_eq!(h.resolve("fixture").unwrap(), Some(user.0.join("fixture")));
    for name in ["missing", "bundled", "alias", "text", "directory"] {
        assert!(h.resolve(name).unwrap().is_none());
    }
    for name in ["", "/bin/true", "../fixture"] {
        assert!(matches!(h.resolve(name), Err(ExecutionError::Invalid(_))));
    }
    assert!(Host::new(Runtime::Native, env(&[]))
        .resolve("sh")
        .unwrap()
        .is_some());

    let snap = Host::new(
        Runtime::Snap,
        env(&[
            ("PATH", &format!("{}:/usr/bin", bundle.0.display())),
            ("SNAP", bundle.0.to_str().unwrap()),
        ]),
    );
    assert!(snap.resolve("bundled").unwrap().is_none());
    assert!(snap.resolve("sh").unwrap().is_some());
}

#[test]
fn host_environment_and_arguments_are_isolated_from_packaging() {
    let h = Host::new(
        Runtime::AppImage,
        env(&[
            ("HOME", "/tmp/synthetic-home"),
            ("PATH", "/usr/bin"),
            ("LD_LIBRARY_PATH", "/app/lib"),
            ("LD_PRELOAD", "/app/injected.so"),
            ("PYTHONPATH", "/app/python"),
            ("APT_CONFIG", "/app/apt.conf"),
            ("QT_PLUGIN_PATH", "/app/plugins"),
            ("LANG", "invalid"),
        ]),
    );
    let result=h.read(Path::new("/bin/sh"),&["-c".into(),r#"printf '%s\n' "$HOME" "$LC_ALL" "$PWD" "$1"; test -z "${LD_LIBRARY_PATH+x}${LD_PRELOAD+x}${PYTHONPATH+x}${APT_CONFIG+x}${QT_PLUGIN_PATH+x}""#.into(),"fixture".into(),"$(touch /tmp/DO_NOT_EXECUTE); echo fixture".into()],Limits::default(),&Cancellation::default()).unwrap();
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "/tmp/synthetic-home\nC\n/\n$(touch /tmp/DO_NOT_EXECUTE); echo fixture\n"
    );
    assert_eq!(result.code, Some(0));
    assert!(result.stderr.is_empty());
    assert!(matches!(
        h.read(
            Path::new("relative"),
            &[],
            Limits::default(),
            &Cancellation::default()
        ),
        Err(ExecutionError::Invalid(_))
    ));
    assert!(matches!(
        h.read(
            Path::new("/pkgdeck-missing-executable"),
            &[],
            Limits::default(),
            &Cancellation::default()
        ),
        Err(ExecutionError::Io(_))
    ));
}

#[test]
fn flatpak_uses_the_sanitized_user_path_and_pins_system_writes() {
    let fixture = Fixture::new();
    symlink("/bin/true", fixture.0.join("flatpak")).unwrap();
    let host = Host::new(
        Runtime::Native,
        env(&[
            ("PATH", fixture.0.to_str().unwrap()),
            ("HOME", "/tmp/fixture-home"),
        ]),
    );
    let cancel = Cancellation::default();
    let result = host
        .flatpak(
            &["--user".into(), "remotes".into()],
            &cancel,
            false,
            false,
            Authorization::SudoNonInteractive,
        )
        .unwrap();
    assert_eq!(result.code, Some(0));
    let write = host
        .flatpak(
            &["--user".into(), "update".into()],
            &cancel,
            true,
            false,
            Authorization::SudoNonInteractive,
        )
        .unwrap();
    assert_eq!(write.code, Some(0));
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert_eq!(
        host.flatpak(
            &["--user".into(), "list".into()],
            &cancelled,
            false,
            false,
            Authorization::SudoNonInteractive,
        ),
        Err(ExecutionError::Cancelled)
    );
    let system_cancelled = Cancellation::default();
    system_cancelled.cancel();
    let system = host.flatpak(
        &["--system".into(), "update".into()],
        &system_cancelled,
        true,
        true,
        Authorization::SudoNonInteractive,
    );
    assert!(
        matches!(system, Err(ExecutionError::Disabled(ref reason)) if reason == "system Flatpak not found")
            || system == Err(ExecutionError::Cancelled)
    );
}

#[test]
fn output_is_drained_capped_and_exit_status_preserved() {
    let result = host()
        .read(
            Path::new("/bin/sh"),
            &shell(
                r"head -c 300000 /dev/zero | tr '\000' x; head -c 300000 /dev/zero | tr '\000' y >&2; exit 7",
            ),
            Limits {
                output_bytes: 1024,
                ..Limits::default()
            },
            &Cancellation::default(),
        )
        .unwrap();
    assert_eq!(result.code, Some(7));
    assert!(result.truncated);
    assert_eq!(result.stdout, vec![b'x'; 1024]);
    assert_eq!(result.stderr, vec![b'y'; 1024]);
    let result = read("kill -TERM $$");
    assert_eq!(result.signal, Some(15));
    assert_eq!(result.code, None);
    // A descendant retaining the output pipe must not hang the supervisor.
    assert_eq!(read("sleep 0.2 &").code, Some(0));
}

#[test]
fn reads_time_out_and_cancel_without_waiting_for_descendants() {
    let limits = Limits {
        timeout: Duration::from_millis(40),
        ..Limits::default()
    };
    assert_eq!(
        host().read(
            Path::new("/bin/sh"),
            &shell("sleep 30 & wait"),
            limits,
            &Cancellation::default()
        ),
        Err(ExecutionError::TimedOut)
    );
    let cancel = Cancellation::default();
    cancel.cancel();
    assert_eq!(
        host().read(Path::new("/bin/true"), &[], limits, &cancel),
        Err(ExecutionError::Cancelled)
    );
    let other = cancel.clone();
    assert!(other.requested());
    let cancel = Cancellation::default();
    let other = cancel.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        other.cancel();
    });
    assert_eq!(
        host().read(
            Path::new("/bin/sh"),
            &shell("exec /bin/sleep 30"),
            Limits::default(),
            &cancel
        ),
        Err(ExecutionError::Cancelled)
    );
    thread.join().unwrap();
}

#[test]
fn apt_requests_cannot_inject_options_paths_or_other_backends() {
    for package in [
        "",
        "a",
        "-y",
        "../fixture.deb",
        "fixture;id",
        "Fixture",
        "fixture=1",
        "fixture:--amd64:",
    ] {
        assert!(matches!(
            AptAction::Install(package.into()).arguments(),
            Err(ExecutionError::Invalid(_))
        ));
    }
    let install = AptAction::Install("pkgdeck-fixture+one.1".into())
        .arguments()
        .unwrap();
    assert!(install.contains(&"--no-remove".into()));
    assert!(install.contains(&"DPkg::Lock::Timeout=0".into()));
    let local_path = std::path::PathBuf::from("/tmp/Synthetic package.deb");
    let local = AptAction::InstallLocal(local_path.clone())
        .arguments()
        .unwrap();
    assert_eq!(local.last(), Some(&local_path.into_os_string()));
    assert!(local.contains(&"--no-remove".into()));
    assert!(AptAction::InstallLocal("relative.deb".into())
        .arguments()
        .is_err());
    let remove = AptAction::Remove("pkgdeck-fixture".into())
        .arguments()
        .unwrap();
    assert!(!remove.contains(&"--no-remove".into()));
    let cancel = Cancellation::default();
    cancel.cancel();
    for authorization in [Authorization::Polkit, Authorization::SudoNonInteractive] {
        let error = host()
            .apt(AptAction::Install("fixture".into()), authorization, &cancel)
            .unwrap_err();
        assert!(matches!(
            error,
            ExecutionError::Cancelled | ExecutionError::Invalid(_)
        ));
    }
}

fn completion(code: Option<i32>, message: &str) -> Completion {
    Completion {
        code,
        signal: None,
        stdout: vec![],
        stderr: message.as_bytes().to_vec(),
        truncated: false,
        cancellation_deferred: false,
    }
}
#[test]
fn apt_failures_preserve_authorization_lock_and_interruption_meaning() {
    for (code, message, expected) in [
        (Some(126), "", ExecutionError::AuthorizationCancelled),
        (Some(127), "", ExecutionError::AuthorizationDenied),
        (
            Some(1),
            "sudo: a password is required",
            ExecutionError::AuthorizationDenied,
        ),
        (
            Some(100),
            "E: Could not get lock /var/lib/dpkg/lock-frontend",
            ExecutionError::LockBusy,
        ),
        (
            Some(100),
            "E: Unable to acquire the dpkg frontend lock",
            ExecutionError::LockBusy,
        ),
        (
            Some(100),
            "E: dpkg was interrupted",
            ExecutionError::Interrupted,
        ),
        (None, "", ExecutionError::Interrupted),
    ] {
        assert_eq!(classify_apt(completion(code, message)), Err(expected));
    }
    assert!(classify_apt(completion(Some(0), "")).is_ok());
    assert_eq!(
        classify_apt(completion(Some(100), "fixture failed")),
        Err(ExecutionError::Failed(completion(
            Some(100),
            "fixture failed"
        )))
    );
}

#[test]
fn diagnostics_are_actionable() {
    for error in [
        ExecutionError::Disabled("disabled".into()),
        ExecutionError::Invalid("invalid".into()),
        ExecutionError::Io("io".into()),
        ExecutionError::Cancelled,
        ExecutionError::TimedOut,
        ExecutionError::AuthorizationCancelled,
        ExecutionError::AuthorizationDenied,
        ExecutionError::LockBusy,
        ExecutionError::Interrupted,
        ExecutionError::Failed(completion(Some(100), "fixture failed")),
    ] {
        assert!(!error.to_string().is_empty());
    }
    assert!(ExecutionError::LockBusy
        .to_string()
        .contains("never remove lock"));
}

#[test]
fn apt_refresh_upgrade_and_multiarch_keep_native_safety_options() {
    let refresh = AptAction::Refresh.arguments().unwrap();
    assert!(refresh.contains(&"APT::Update::Error-Mode=any".into()));
    assert_eq!(refresh.last().unwrap(), "update");
    let upgrade = AptAction::Upgrade("synthetic-fixture:amd64".into())
        .arguments()
        .unwrap();
    assert!(upgrade.contains(&"--only-upgrade".into()));
    assert!(upgrade.contains(&"--no-remove".into()));
    assert_eq!(upgrade.last().unwrap(), "synthetic-fixture:amd64");
    let upgrade_all = AptAction::UpgradeAll.arguments().unwrap();
    assert_eq!(upgrade_all.last().unwrap(), "dist-upgrade");
    assert!(!upgrade_all.contains(&"--only-upgrade".into()));
    let autoremove = AptAction::Autoremove.arguments().unwrap();
    assert_eq!(autoremove.last().unwrap(), "autoremove");
    assert!(autoremove.contains(&"--assume-yes".into()));
    assert!(autoremove.contains(&"--purge".into()));
    let autoclean = AptAction::Autoclean.arguments().unwrap();
    assert_eq!(autoclean.last().unwrap(), "autoclean");
    assert!(autoclean.contains(&"DPkg::Lock::Timeout=0".into()));
    for name in ["fixture:", "fixture:amd64:foreign", "fixture:../amd64"] {
        assert!(AptAction::Install(name.into()).arguments().is_err());
    }
}

#[test]
fn system_manager_reads_keep_snap_timeout_and_exit_status() {
    let fixture = Fixture::new();
    link_executable(&fixture.0, "snap", "/bin/true");
    link_executable(&fixture.0, "synthetic-ok", "/bin/true");
    link_executable(&fixture.0, "synthetic-tool", "/bin/false");
    let host = Host::new(
        Runtime::Native,
        env(&[("PATH", fixture.0.to_str().unwrap())]),
    );
    let cancel = Cancellation::default();
    for executable in ["snap", "synthetic-ok"] {
        let ok = host
            .system_manager(
                executable,
                &[],
                &cancel,
                false,
                Authorization::SudoNonInteractive,
            )
            .unwrap();
        assert_eq!(ok.code, Some(0));
    }
    assert!(matches!(
        host.system_manager(
            "synthetic-tool",
            &[],
            &cancel,
            false,
            Authorization::SudoNonInteractive,
        ),
        Err(ExecutionError::Failed(_))
    ));
}

#[test]
fn dev_tool_runs_unprivileged_and_reports_status() {
    let fixture = Fixture::new();
    link_executable(&fixture.0, "synthetic-ok", "/bin/true");
    link_executable(&fixture.0, "synthetic-fail", "/bin/false");
    let host = Host::new(
        Runtime::Native,
        env(&[
            ("PATH", fixture.0.to_str().unwrap()),
            ("HOME", "/home/test"),
        ]),
    );
    assert_eq!(
        host.var("HOME").as_deref(),
        Some(std::ffi::OsStr::new("/home/test"))
    );
    assert!(host.var("CARGO_HOME").is_none());
    let cancel = Cancellation::default();
    let ok = host
        .dev_tool("synthetic-ok", "Synthetic", &[], &cancel, false)
        .unwrap();
    assert_eq!(ok.code, Some(0));
    assert!(matches!(
        host.dev_tool("synthetic-fail", "Synthetic", &[], &cancel, false),
        Err(ExecutionError::Failed(_))
    ));
    assert!(matches!(
        host.dev_tool("synthetic-missing", "Synthetic", &[], &cancel, false),
        Err(ExecutionError::Disabled(_))
    ));
}

#[test]
fn container_engine_uses_bounded_user_commands_and_preserves_failures() {
    let fixture = Fixture::new();
    link_executable(&fixture.0, "synthetic-ok", "/bin/true");
    link_executable(&fixture.0, "synthetic-fail", "/bin/false");
    let host = Host::new(
        Runtime::Native,
        env(&[("PATH", fixture.0.to_str().unwrap())]),
    );
    let cancel = Cancellation::default();
    assert_eq!(
        host.container_engine("synthetic-ok", "Synthetic", &[], &cancel, false)
            .unwrap()
            .code,
        Some(0)
    );
    assert_eq!(
        host.container_engine("synthetic-ok", "Synthetic", &[], &cancel, true)
            .unwrap()
            .code,
        Some(0)
    );
    assert!(matches!(
        host.container_engine("synthetic-fail", "Synthetic", &[], &cancel, false),
        Err(ExecutionError::Failed(_))
    ));
    assert!(matches!(
        host.container_engine("missing", "Synthetic", &[], &cancel, false),
        Err(ExecutionError::Disabled(reason)) if reason == "Synthetic not found"
    ));
}

#[test]
fn venv_pip_runs_only_inside_explicit_absolute_environments() {
    let fixture = Fixture::new();
    let venv = fixture.0.join("venv");
    fs::create_dir_all(venv.join("bin")).unwrap();
    symlink("/bin/true", venv.join("bin/python")).unwrap();
    fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
    let host = Host::new(
        Runtime::Native,
        env(&[
            ("PATH", fixture.0.to_str().unwrap()),
            ("HOME", "/home/test"),
            ("VIRTUAL_ENV", venv.to_str().unwrap()),
            ("PIPX_HOME", "/home/test/.local/share/pipx"),
            ("UV_TOOL_DIR", "/home/test/.local/share/uv/tools"),
            ("PNPM_HOME", "/home/test/.local/share/pnpm"),
            ("CARGO_HOME", "/home/test/.cargo"),
            ("BUN_INSTALL", "/home/test/.bun"),
            ("COMPOSER_HOME", "/home/test/.config/composer"),
            ("GEM_HOME", "/home/test/gem"),
            ("LD_LIBRARY_PATH", "/app/lib"),
            ("PYTHONPATH", "/app/python"),
        ]),
    );
    // Explicit manager homes survive sanitization; injection variables do not.
    for name in [
        "VIRTUAL_ENV",
        "PIPX_HOME",
        "UV_TOOL_DIR",
        "PNPM_HOME",
        "CARGO_HOME",
        "BUN_INSTALL",
        "COMPOSER_HOME",
        "GEM_HOME",
    ] {
        assert!(host.var(name).is_some());
    }
    assert!(host.var("LD_LIBRARY_PATH").is_none());
    assert!(host.var("PYTHONPATH").is_none());
    let cancel = Cancellation::default();
    let ok = host
        .venv_pip(&venv, &["--version".into()], &cancel, false)
        .unwrap();
    assert_eq!(ok.code, Some(0));
    // Relative roots, missing interpreters, and directories without a
    // virtual-environment marker fail closed without running pip.
    assert!(matches!(
        host.venv_pip(Path::new("relative/venv"), &[], &cancel, false),
        Err(ExecutionError::Invalid(_))
    ));
    assert!(matches!(
        host.venv_pip(&fixture.0.join("missing"), &[], &cancel, false),
        Err(ExecutionError::Disabled(_))
    ));
    assert!(matches!(
        host.venv_pip(&fixture.0, &[], &cancel, false),
        Err(ExecutionError::Disabled(_))
    ));
}

#[test]
fn venv_pip_rejects_malformed_and_packaged_environments() {
    let fixture = Fixture::new();
    let interpreter = fixture.0.join("bin/python");
    fs::create_dir_all(&interpreter).unwrap();
    let cancel = Cancellation::default();
    let rejected = |h: &Host| {
        assert!(matches!(
            h.venv_pip(&fixture.0, &[], &cancel, false),
            Err(ExecutionError::Disabled(_))
        ));
    };
    rejected(&host());
    fs::remove_dir(&interpreter).unwrap();
    symlink("/bin/true", &interpreter).unwrap();
    fs::create_dir(fixture.0.join("pyvenv.cfg")).unwrap();
    rejected(&host());
    fs::remove_dir(fixture.0.join("pyvenv.cfg")).unwrap();
    fs::write(fixture.0.join("pyvenv.cfg"), "home = /synthetic\n").unwrap();
    let packaged = Host::new(
        Runtime::AppImage,
        env(&[("APPDIR", fixture.0.to_str().unwrap())]),
    );
    rejected(&packaged);
    fs::remove_file(&interpreter).unwrap();
    fs::write(&interpreter, "not an executable").unwrap();
    fs::set_permissions(&interpreter, fs::Permissions::from_mode(0o644)).unwrap();
    rejected(&host());
}

#[test]
fn flatpak_remote_queries_allow_slow_and_large_catalogs() {
    let fixture = Fixture::new();
    link_executable(&fixture.0, "flatpak", "/bin/sh");
    let h = Host::new(
        Runtime::Native,
        env(&[("PATH", fixture.0.to_str().unwrap())]),
    );
    let output = h
        .flatpak(
            &shell("/bin/sleep 11; /usr/bin/head -c 262144 /dev/zero"),
            &Cancellation::default(),
            false,
            false,
            Authorization::SudoNonInteractive,
        )
        .unwrap();
    assert_eq!(output.code, Some(0));
    assert_eq!(output.stdout.len(), 262144);
    assert!(!output.truncated);
}

#[test]
fn flatpak_failures_missing_tools_and_cancellation_are_diagnostic() {
    let fixture = Fixture::new();
    link_executable(&fixture.0, "flatpak", "/bin/false");
    let host = Host::new(
        Runtime::Native,
        env(&[("PATH", fixture.0.to_str().unwrap())]),
    );
    let cancel = Cancellation::default();
    assert!(matches!(
        host.flatpak(
            &["--user".into(), "list".into()],
            &cancel,
            false,
            false,
            Authorization::SudoNonInteractive,
        ),
        Err(ExecutionError::Failed(result)) if result.code == Some(1)
    ));

    let missing = Host::new(
        Runtime::Native,
        env(&[("PATH", fixture.0.join("missing").to_str().unwrap())]),
    );
    assert!(matches!(
        missing.flatpak(
            &["--user".into(), "list".into()],
            &cancel,
            false,
            false,
            Authorization::SudoNonInteractive,
        ),
        Err(ExecutionError::Disabled(reason)) if reason == "Flatpak not found"
    ));

    fs::remove_file(fixture.0.join("flatpak")).unwrap();
    link_executable(&fixture.0, "flatpak", "/bin/sh");
    let cancelling = Cancellation::default();
    let requested = cancelling.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        requested.cancel();
    });
    assert_eq!(
        host.flatpak(
            &shell("exec /bin/sleep 30"),
            &cancelling,
            false,
            false,
            Authorization::SudoNonInteractive,
        ),
        Err(ExecutionError::Cancelled)
    );
    thread.join().unwrap();
}

#[test]
fn apt_group_preserves_exact_targets_and_rejects_mixed_verbs() {
    let args = AptAction::group_arguments(&[
        AptAction::Install("fixture-one:amd64".into()),
        AptAction::Install("fixture-two:arm64".into()),
    ])
    .unwrap();
    assert_eq!(
        args[args.len() - 2..],
        [
            OsString::from("fixture-one:amd64"),
            OsString::from("fixture-two:arm64")
        ]
    );
    assert!(matches!(
        AptAction::group_arguments(&[]),
        Err(ExecutionError::Invalid(_))
    ));
    assert!(matches!(
        AptAction::group_arguments(&[
            AptAction::Install("fixture-one:amd64".into()),
            AptAction::Remove("fixture-two:amd64".into()),
        ]),
        Err(ExecutionError::Invalid(_))
    ));
    assert!(matches!(
        AptAction::group_arguments(&[AptAction::Refresh]),
        Err(ExecutionError::Invalid(_))
    ));
    assert!(matches!(
        AptAction::group_arguments(&[
            AptAction::Install("--purge".into()),
            AptAction::Install("fixture-two:amd64".into()),
        ]),
        Err(ExecutionError::Invalid(_))
    ));
}
