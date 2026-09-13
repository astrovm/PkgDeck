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
fn python(script: &str) -> Vec<OsString> {
    vec!["-c".into(), script.into()]
}
fn read(script: &str) -> Completion {
    host()
        .read(
            Path::new("/usr/bin/python3"),
            &python(script),
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

#[test]
fn formats_and_nested_sandbox_markers_fail_closed() {
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
    for runtime in [Runtime::Flatpak, Runtime::Snap] {
        let h = Host::new(runtime, env(&[]));
        assert!(matches!(
            h.resolve("apt-get"),
            Err(ExecutionError::Disabled(_))
        ));
        assert!(matches!(
            h.read(
                Path::new("/bin/true"),
                &[],
                Limits::default(),
                &Cancellation::default()
            ),
            Err(ExecutionError::Disabled(_))
        ));
        assert!(matches!(
            h.apt(
                AptAction::Remove("fixture".into()),
                Authorization::Polkit,
                &Cancellation::default()
            ),
            Err(ExecutionError::Disabled(_))
        ));
    }
    assert!(Runtime::Native.disabled_reason().is_none());
    assert!(Runtime::AppImage.disabled_reason().is_none());
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
    let result=h.read(Path::new("/usr/bin/python3"),&["-c".into(),"import os,sys; print(os.environ['HOME']); print(os.environ['LC_ALL']); print(os.getcwd()); print(sys.argv[1]); assert not any(k in os.environ for k in ['LD_LIBRARY_PATH','LD_PRELOAD','PYTHONPATH','APT_CONFIG','QT_PLUGIN_PATH'])".into(),"$(touch /tmp/DO_NOT_EXECUTE); echo fixture".into()],Limits::default(),&Cancellation::default()).unwrap();
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
fn output_is_drained_capped_and_exit_status_preserved() {
    let result = host()
        .read(
            Path::new("/usr/bin/python3"),
            &python(
                "import os; os.write(1,b'x'*300000); os.write(2,b'y'*300000); raise SystemExit(7)",
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
    let result = read("import os,signal; os.kill(os.getpid(),signal.SIGTERM)");
    assert_eq!(result.signal, Some(15));
    assert_eq!(result.code, None);
    // A descendant retaining the output pipe must not hang the supervisor.
    assert_eq!(
        read("import os,time; pid=os.fork(); time.sleep(0.2) if pid==0 else None").code,
        Some(0)
    );
}

#[test]
fn reads_time_out_and_cancel_without_waiting_for_descendants() {
    let limits = Limits {
        timeout: Duration::from_millis(40),
        ..Limits::default()
    };
    assert_eq!(
        host().read(
            Path::new("/usr/bin/python3"),
            &python("import os,time; os.fork(); time.sleep(30)"),
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
            Path::new("/usr/bin/python3"),
            &python("import time; time.sleep(30)"),
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
    for name in ["fixture:", "fixture:amd64:foreign", "fixture:../amd64"] {
        assert!(AptAction::Install(name.into()).arguments().is_err());
    }
}
