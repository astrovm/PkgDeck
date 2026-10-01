//! Error paths and less common native answers of the built-in backends,
//! driven through a scripted transport.
use pkgdeck_core::{backends::*, engine::*, host::AptAction, package::*, process::*};
use std::{
    ffi::OsString,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

type Reply = Result<Completion, ExecutionError>;
type Answer = dyn Fn(&str) -> Reply + Send + Sync;

/// Answers every native call from one closure keyed by a readable command
/// line: `tool arg…`, prefixed with `w:` for writes and `sys:` for system
/// Flatpak calls. Every command line is recorded.
#[derive(Clone)]
struct Script {
    answer: Arc<Answer>,
    calls: Arc<Mutex<Vec<String>>>,
    env: Vec<(&'static str, &'static str)>,
}
impl Script {
    fn new(answer: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Self {
        Self {
            answer: Arc::new(answer),
            calls: Arc::default(),
            env: vec![],
        }
    }
    fn with_env(mut self, name: &'static str, value: &'static str) -> Self {
        self.env.push((name, value));
        self
    }
    fn run(&self, line: String) -> Reply {
        self.calls.lock().unwrap().push(line.clone());
        (self.answer)(&line)
    }
    fn line(prefix: &str, tool: &str, args: &[OsString], write: bool) -> String {
        let mut line = String::new();
        if write {
            line.push_str("w:");
        }
        line.push_str(prefix);
        line.push_str(tool);
        for arg in args {
            line.push(' ');
            line.push_str(&arg.to_string_lossy());
        }
        line
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}
impl Transport for Script {
    fn apt_query(&self, mode: &str, query: &str, arch: &str, _: &Cancellation) -> Reply {
        self.run(format!("apt-query {mode} {query} {arch}"))
    }
    fn apt_write(&self, action: AptAction, _: &Cancellation) -> Reply {
        self.run(format!("w:apt {action:?}"))
    }
    fn brew(&self, args: &[OsString], _: &Cancellation, write: bool) -> Reply {
        self.run(Self::line("", "brew", args, write))
    }
    fn flatpak(&self, args: &[OsString], _: &Cancellation, write: bool, system: bool) -> Reply {
        self.run(Self::line(
            if system { "sys:" } else { "" },
            "flatpak",
            args,
            write,
        ))
    }
    fn flatpak_unused(&self, _: &Cancellation) -> Reply {
        self.run("flatpak-unused".into())
    }
    fn env(&self, name: &str) -> Option<OsString> {
        self.env
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.into())
    }
    fn dev_tool(
        &self,
        executable: &str,
        args: &[OsString],
        _: &Cancellation,
        write: bool,
    ) -> Reply {
        self.run(Self::line("", executable, args, write))
    }
    fn venv_pip(
        &self,
        _: &std::path::Path,
        args: &[OsString],
        _: &Cancellation,
        write: bool,
    ) -> Reply {
        self.run(Self::line("", "pip", args, write))
    }
    fn system_manager(
        &self,
        executable: &str,
        args: &[OsString],
        _: &Cancellation,
        write: bool,
    ) -> Reply {
        self.run(Self::line("", executable, args, write))
    }
}

/// Only the transport's defaults.
struct Bare;
impl Transport for Bare {}

fn exit(code: i32, stdout: &str) -> Reply {
    Ok(Completion {
        code: Some(code),
        signal: None,
        stdout: stdout.as_bytes().to_vec(),
        stderr: vec![],
        truncated: false,
        cancellation_deferred: false,
    })
}
fn ok(stdout: &str) -> Reply {
    exit(0, stdout)
}
fn failed(code: i32) -> Reply {
    Err(ExecutionError::Failed(Completion {
        code: Some(code),
        signal: None,
        stdout: vec![],
        stderr: b"native failure".to_vec(),
        truncated: false,
        cancellation_deferred: false,
    }))
}
fn cleanup_id(backend: &str, key: &str) -> CleanupId {
    CleanupId {
        backend: backend.into(),
        key: key.into(),
    }
}
fn system_id(backend: &str, name: &str, architecture: &str) -> PackageId {
    PackageId {
        backend: backend.into(),
        name: name.into(),
        architecture: architecture.into(),
        scope: Scope::System,
        remote: None,
        reference: None,
    }
}
fn run(backend: &mut dyn Backend, operation: &Operation) -> Result<OperationOutcome, EngineError> {
    backend.execute(operation, &Cancellation::default(), &mut |_| {})
}
fn native_failure(result: Result<impl std::fmt::Debug, EngineError>, code: i32) {
    match result {
        Err(EngineError::Execution(ExecutionError::Failed(completion))) => {
            assert_eq!(completion.code, Some(code))
        }
        other => panic!("expected a failed native command, got {other:?}"),
    }
}
fn invalid_response(result: Result<impl std::fmt::Debug, EngineError>, expected: &str) {
    match result {
        Err(EngineError::InvalidResponse { reason, .. }) => {
            assert!(reason.contains(expected), "{reason}")
        }
        other => panic!("expected {expected:?}, got {other:?}"),
    }
}

#[test]
fn transport_defaults_refuse_previews_and_grouped_writes() {
    let cancel = Cancellation::default();
    let disabled = |result: Reply, expected: &str| match result {
        Err(ExecutionError::Disabled(reason)) => assert_eq!(reason, expected),
        other => panic!("expected {expected:?}, got {other:?}"),
    };
    disabled(
        Bare.apt_query_sandboxed("search", "x", "", &cancel),
        "APT not found",
    );
    disabled(
        Bare.apt_write_group(&[AptAction::Install("x".into())], &cancel),
        "grouped APT transaction unavailable",
    );
    disabled(
        Bare.flatpak_unused(&cancel),
        "Flatpak cannot show an exact preview of unused runtimes",
    );
    assert!(Bare.supports_flatpak_cleanup());
    assert!(!Bare.docker_is_podman());
    // Detection reads a disabled transport as an absent manager.
    assert_eq!(
        Apt::new(Bare).detect(&cancel).unwrap(),
        Availability::Unavailable("APT not found".into())
    );
    // A Flatpak inventory preview never falls back to a CLI approximation.
    assert!(matches!(
        Flatpak::new(Bare).cleanup(&cancel),
        Err(EngineError::Execution(ExecutionError::Disabled(_)))
    ));
}

#[test]
fn apt_groups_need_a_transport_that_supports_one_transaction() {
    let script = Script::new(|_| ok(""));
    let mut apt = Apt::new(Bare);
    let operations = [
        Operation::Install(system_id("apt", "one", "amd64")),
        Operation::Install(system_id("apt", "two", "amd64")),
    ];
    let mut messages = vec![];
    let result = apt
        .execute_group(&operations, &Cancellation::default(), &mut |item| {
            messages.push(item)
        })
        .unwrap();
    assert!(matches!(
        result,
        Err(EngineError::Execution(ExecutionError::Disabled(reason)))
            if reason == "grouped APT transaction unavailable"
    ));
    assert!(matches!(&messages[..], [Progress::Message(text)] if text.contains("2 APT packages")));
    // Refresh and upgrade-all never join a package transaction.
    let mut apt = Apt::new(script.clone());
    let mixed = [
        Operation::Install(system_id("apt", "one", "amd64")),
        Operation::UpgradeAll {
            backend: "apt".into(),
        },
    ];
    invalid_response(
        apt.execute_group(&mixed, &Cancellation::default(), &mut |_| {})
            .unwrap(),
        "mixed APT batch",
    );
    assert!(script.calls().is_empty());
}

#[test]
fn apt_cleanup_reports_simulation_failures_and_empty_plans() {
    let cancel = Cancellation::default();
    let answer = |autoremove: Reply, config: Reply, find: Reply| {
        let (autoremove, config, find) = (
            Arc::new(Mutex::new(Some(autoremove))),
            Arc::new(Mutex::new(Some(config))),
            Arc::new(Mutex::new(Some(find))),
        );
        Script::new(move |line| {
            let slot = if line.starts_with("apt-get") {
                &autoremove
            } else if line.starts_with("apt-config") {
                &config
            } else {
                assert!(line.starts_with("find /var/cache/apt/archives/ -maxdepth 1"));
                &find
            };
            slot.lock().unwrap().take().expect("one call per command")
        })
    };
    let config = "ROOT='/'\nCACHE='var/cache/apt'\nARCHIVES='archives/'\n";
    // The autoremove simulation cannot run, or APT rejects it.
    let mut apt = Apt::new(answer(
        Err(ExecutionError::Disabled("APT not found".into())),
        ok(config),
        ok(""),
    ));
    assert!(matches!(
        apt.cleanup_plan(&cleanup_id("apt", "autoremove"), &cancel),
        Err(EngineError::Execution(ExecutionError::Disabled(_)))
    ));
    let mut apt = Apt::new(answer(exit(100, ""), ok(config), ok("")));
    native_failure(
        apt.cleanup_plan(&cleanup_id("apt", "autoremove"), &cancel),
        100,
    );
    // Nothing removable and no cached downloads: both tasks are absent.
    let script = answer(ok("Reading package lists...\n"), ok(config), ok(""));
    let mut apt = Apt::new(script.clone());
    assert_eq!(apt.cleanup(&cancel).unwrap(), vec![]);
    assert_eq!(
        script.calls()[0],
        "apt-get --simulate -o Debug::NoLocking=1 --purge autoremove"
    );
    assert!(matches!(
        Apt::new(answer(ok(""), ok(config), ok("")))
            .cleanup_plan(&cleanup_id("apt", "autoremove"), &cancel),
        Err(EngineError::NotFound)
    ));
    // The cache location cannot be read, or listing downloads fails.
    let mut apt = Apt::new(answer(
        ok(""),
        Err(ExecutionError::Disabled("apt-config not found".into())),
        ok(""),
    ));
    assert!(matches!(
        apt.cleanup_plan(&cleanup_id("apt", "autoclean"), &cancel),
        Err(EngineError::Execution(ExecutionError::Disabled(_)))
    ));
    let mut apt = Apt::new(answer(ok(""), exit(1, ""), ok("")));
    native_failure(
        apt.cleanup_plan(&cleanup_id("apt", "autoclean"), &cancel),
        1,
    );
    let mut apt = Apt::new(answer(ok(""), ok(config), exit(1, "")));
    native_failure(
        apt.cleanup_plan(&cleanup_id("apt", "autoclean"), &cancel),
        1,
    );
    // Both failures are reported, not an empty cleanup list.
    let report = Apt::new(answer(exit(100, ""), exit(1, ""), ok(""))).cleanup_report(&cancel);
    assert!(report.items.is_empty());
    assert_eq!(report.failures.len(), 2);
}

#[test]
fn flatpak_unused_cleanup_reports_a_failed_uninstall() {
    let script = Script::new(|line| match line {
        "flatpak-unused" => ok(
            r#"[{"scope":"system","reference":"runtime/org.example.Platform/x86_64/1","bytes":5}]"#,
        ),
        _ => failed(1),
    });
    let mut flatpak = Flatpak::new(script.clone());
    native_failure(
        run(
            &mut flatpak,
            &Operation::Clean(cleanup_id("flatpak", "unused-system")),
        ),
        1,
    );
    assert_eq!(
        script.calls(),
        [
            "flatpak-unused",
            "w:sys:flatpak --system uninstall --unused --noninteractive --assumeyes"
        ]
    );
}

#[test]
fn development_caches_validate_inventories_and_tasks() {
    let cancel = Cancellation::default();
    // pip's cache report must name both counts.
    let mut pip = DevTool::pip(
        Script::new(|line| match line {
            "pip --version" => ok("pip 25.0\n"),
            "pip cache info" => ok("Package index page cache location: /tmp/x\n"),
            other => panic!("unexpected {other}"),
        })
        .with_env("VIRTUAL_ENV", "/venv"),
    );
    assert_eq!(pip.detect(&cancel).unwrap(), Availability::Available);
    invalid_response(pip.cleanup(&cancel), "unrecognized cache inventory");
    // Cargo has no cache task to clean.
    let script = Script::new(|_| ok(""));
    let mut cargo = DevTool::cargo(script.clone());
    assert!(matches!(
        run(&mut cargo, &Operation::Clean(cleanup_id("cargo", "cache"))),
        Err(EngineError::Unsupported {
            capability: Capability::Clean,
            ..
        })
    ));
    assert!(script.calls().is_empty());
}

#[test]
fn firmware_upgrade_all_without_updates_writes_nothing() {
    let device = "a".repeat(40);
    let devices =
        format!(r#"{{"Devices":[{{"DeviceId":"{device}","Name":"Board","Version":"1"}}]}}"#);
    let script = Script::new(move |line| match line {
        "fwupdmgr --json get-devices" => ok(&devices),
        "fwupdmgr --json get-updates" => ok(r#"{"Devices":[]}"#),
        other => panic!("unexpected {other}"),
    });
    let mut firmware = Firmware::new(script.clone());
    let outcome = run(
        &mut firmware,
        &Operation::UpgradeAll {
            backend: "fwupd".into(),
        },
    )
    .unwrap();
    assert!(!outcome.cancellation_deferred);
    assert!(script.calls().iter().all(|line| !line.starts_with("w:")));
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pkgdeck-engine-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn flatpak_reports_failed_listings_searches_and_writes() {
    let cancel = Cancellation::default();
    let app = "org.example.App\tx86_64\tstable\t1.0\tApp\tflathub\t\tExample";
    // A failed `list` fails the inventory.
    let mut flatpak = Flatpak::new(Script::new(|_| exit(1, "")));
    native_failure(flatpak.installed(&cancel), 1);
    // So does a failed update check for the listed apps.
    let mut flatpak = Flatpak::new(Script::new(move |line| {
        if line.contains(" list ") {
            ok(app)
        } else {
            exit(1, "")
        }
    }));
    native_failure(flatpak.installed(&cancel), 1);
    // A failed user search fails the search.
    let mut flatpak = Flatpak::new(Script::new(|_| exit(1, "")));
    native_failure(flatpak.search("example", &cancel), 1);
    // Upgrading everything stops at the first failed installation.
    let script = Script::new(|_| failed(1));
    let mut flatpak = Flatpak::new(script.clone());
    native_failure(
        run(
            &mut flatpak,
            &Operation::UpgradeAll {
                backend: "flatpak".into(),
            },
        ),
        1,
    );
    assert_eq!(
        script.calls(),
        ["w:flatpak --user update --noninteractive --assumeyes"]
    );
}

#[test]
fn flatpak_bundles_check_scope_before_fetching_and_report_failed_installs() {
    let base = temp_dir("bundle");
    let bundle = base.join("synthetic.flatpak");
    std::fs::write(&bundle, b"synthetic bundle with a failing install").unwrap();
    let cancel = Cancellation::default();
    let package = pkgdeck_core::artifact::inspect(bundle.to_str().unwrap(), &cancel).unwrap();
    let script = Script::new(|_| failed(1));
    let mut flatpak = Flatpak::new(script.clone());
    let mut foreign = package.id.clone();
    foreign.scope = Scope::User { uid: u32::MAX };
    invalid_response(
        run(&mut flatpak, &Operation::Install(foreign)),
        "invalid Flatpak scope",
    );
    assert!(script.calls().is_empty());
    native_failure(run(&mut flatpak, &Operation::Install(package.id)), 1);
    let calls = script.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("w:flatpak --user install --noninteractive --assumeyes --bundle "));
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn apt_simulations_reject_unreadable_summaries_and_report_failures() {
    let cancel = Cancellation::default();
    let install = Operation::Install(system_id("apt", "tool", "amd64"));
    let summary = |text: &'static str| Script::new(move |_| ok(text));
    invalid_response(
        Apt::new(summary(
            "Inst tool (1.0 Debian:stable [amd64])\nmany upgraded, 1 newly installed, 0 to remove and 0 not upgraded.\n",
        ))
        .operation_plan(&install, &cancel),
        "invalid simulation counts",
    );
    // Lines without a package name are not changes.
    let plan = Apt::new(summary(
        "NOTE:\nInst tool (1.0 Debian:stable [amd64])\n0 upgraded, 1 newly installed, 0 to remove and 0 not upgraded.\n",
    ))
    .operation_plan(&install, &cancel)
    .unwrap()
    .unwrap();
    assert_eq!(plan.changes.len(), 1);
    assert_eq!(plan.changes[0].name, "tool");
    assert_eq!(plan.changes[0].candidate_version.as_deref(), Some("1.0"));
    let unavailable = || Script::new(|_| Err(ExecutionError::Disabled("APT not found".into())));
    assert!(matches!(
        Apt::new(unavailable()).operation_plan(&install, &cancel),
        Err(EngineError::Execution(ExecutionError::Disabled(_)))
    ));
    assert!(matches!(
        Apt::new(unavailable()).apt_upgrade_plan(&cancel),
        Err(EngineError::Execution(ExecutionError::Disabled(_)))
    ));
}

#[test]
fn apt_rejects_foreign_tasks_and_identities_before_writing() {
    let cancel = Cancellation::default();
    let script = Script::new(|line| match line {
        line if line.starts_with("apt-get") => exit(100, ""),
        _ => ok(""),
    });
    let mut apt = Apt::new(script.clone());
    // The first failing task fails a plain cleanup listing.
    native_failure(apt.cleanup(&cancel), 100);
    invalid_response(
        apt.cleanup_plan(&cleanup_id("homebrew", "autoremove"), &cancel),
        "foreign cleanup task",
    );
    let mut foreign = system_id("apt", "tool", "amd64");
    foreign.scope = Scope::User { uid: 1 };
    invalid_response(
        run(&mut apt, &Operation::Remove(foreign.clone())),
        "foreign identity or scope",
    );
    assert!(!script.calls().iter().any(|line| line.starts_with("w:")));
    // Removals join one transaction too.
    let grouped = Arc::new(Mutex::new(vec![]));
    struct Group(Arc<Mutex<Vec<Vec<AptAction>>>>);
    impl Transport for Group {
        fn apt_write_group(&self, actions: &[AptAction], _: &Cancellation) -> Reply {
            self.0.lock().unwrap().push(actions.to_vec());
            ok("")
        }
    }
    let outcomes = Apt::new(Group(grouped.clone()))
        .execute_group(
            &[
                Operation::Remove(system_id("apt", "one", "amd64")),
                Operation::Remove(system_id("apt", "two", "all")),
            ],
            &cancel,
            &mut |_| {},
        )
        .unwrap()
        .unwrap();
    assert_eq!(outcomes.len(), 2);
    assert_eq!(
        format!("{:?}", grouped.lock().unwrap()),
        r#"[[Remove("one:amd64"), Remove("two:all")]]"#
    );
}

const PREFIX: &str = "/home/linuxbrew/.linuxbrew";
fn brew_id(backend: &str, name: &str) -> PackageId {
    PackageId {
        backend: backend.into(),
        name: name.into(),
        architecture: std::env::consts::ARCH.into(),
        scope: Scope::Environment {
            path: PREFIX.into(),
        },
        remote: None,
        reference: None,
    }
}
fn cask_json(token: &str, homepage: &str) -> String {
    serde_json::json!({"casks": [{
        "full_token": token, "name": [], "desc": null, "homepage": homepage,
        "version": "2.0", "installed": null, "outdated": false,
    }]})
    .to_string()
}
fn formula_installed(version: &str, outdated: bool) -> String {
    format!(
        r#"{{"formulae":[{{"full_name":"wget","desc":"fetch","homepage":"https://example.invalid","versions":{{"stable":"{version}"}},"revision":0,"installed":[{{"version":"1.0"}}],"linked_keg":"1.0","outdated":{outdated},"dependencies":[]}}]}}"#
    )
}
fn cask_installed(version: &str, outdated: bool) -> String {
    format!(
        r#"{{"casks":[{{"full_token":"pkgdeck","name":["PkgDeck"],"desc":"packages","homepage":"https://example.invalid","version":"{version}","installed":"1.0","outdated":{outdated}}}]}}"#
    )
}
fn brew_updates(script: &Script) -> usize {
    script
        .calls()
        .iter()
        .filter(|line| line.as_str() == "w:brew update")
        .count()
}
/// Homebrew answering detection, then `rest` for everything else.
fn brew(rest: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Script {
    Script::new(move |line| match line {
        "brew --prefix" => ok(&format!("{PREFIX}\n")),
        "brew --version" => ok("Homebrew 7.0.6\n"),
        other => rest(other),
    })
}

#[test]
fn index_refresh_does_not_probe_npm_before_a_named_upgrade_lookup() {
    let cancel = Cancellation::default();
    let script = Script::new(|line| match line {
        "npm root --global" => ok("/opt/npm/lib/node_modules\n"),
        "npm ls --global --depth=0 --json" => {
            ok(r#"{"dependencies":{"eslint":{"version":"9.0.0"}}}"#)
        }
        "npm outdated --global --json" => ok("{}"),
        other => panic!("unexpected {other}"),
    });
    let mut engine = Engine::default();
    engine.register(DevTool::npm(script.clone())).unwrap();

    assert!(engine.refresh_update_indexes(&cancel).is_empty());
    assert!(script.calls().is_empty());

    let report = engine.lookup_for_mutation("eslint", &cancel);
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.packages.len(), 1);
    assert_eq!(report.packages[0].id.name, "eslint");
    assert_eq!(
        report.packages[0].installed_version.as_deref(),
        Some("9.0.0")
    );
    assert_eq!(
        script.calls(),
        [
            "npm root --global",
            "npm ls --global --depth=0 --json",
            "npm outdated --global --json"
        ]
    );
}

#[test]
fn index_refresh_skips_homebrew_cached_as_unavailable() {
    let cancel = Cancellation::default();
    let script = Script::new(|line| panic!("unavailable backend must not run {line}"));
    let mut engine = Engine::default();
    engine.register(HomebrewCask::new(script.clone())).unwrap();
    engine.note_detected(
        "homebrew-cask".into(),
        Ok(Availability::Unavailable("casks require Homebrew 6".into())),
    );

    let failures = engine.refresh_update_indexes(&cancel);
    assert_eq!(failures.len(), 1);
    assert_eq!(
        failures[0],
        BackendFailure {
            backend: "homebrew-cask".into(),
            error: EngineError::Unavailable {
                backend: "homebrew-cask".into(),
                reason: "casks require Homebrew 6".into(),
            },
        }
    );
    // Named upgrades still report the same cached error on lookup.
    assert_eq!(
        engine.lookup_for_mutation("pkgdeck", &cancel).failures,
        failures
    );
    assert!(script.calls().is_empty());
}

#[test]
fn index_refresh_detects_homebrew_before_writing() {
    let cancel = Cancellation::default();
    let script = Script::new(|line| match line {
        "brew --prefix" => Err(ExecutionError::Disabled("Homebrew is not installed".into())),
        other => panic!("unavailable backend must not run {other}"),
    });
    let mut engine = Engine::default();
    engine.register(Homebrew::new(script.clone())).unwrap();

    let failures = engine.refresh_update_indexes(&cancel);
    assert_eq!(failures.len(), 1);
    assert!(matches!(failures[0].error, EngineError::Unavailable { .. }));
    assert_eq!(script.calls(), ["brew --prefix"]);
}

#[test]
fn index_refresh_shares_one_fetch_for_available_homebrew_sources() {
    let cancel = Cancellation::default();
    let script = Script::new(|line| match line {
        "w:brew update" => ok(""),
        other => panic!("cached detection must not run {other}"),
    });
    let mut engine = Engine::default();
    engine.register(Homebrew::new(script.clone())).unwrap();
    engine.register(HomebrewCask::new(script.clone())).unwrap();
    for id in ["homebrew", "homebrew-cask"] {
        engine.note_detected(id.into(), Ok(Availability::Available));
    }

    assert!(engine.refresh_update_indexes(&cancel).is_empty());
    assert_eq!(script.calls(), ["w:brew update"]);
}

#[test]
fn update_checks_fetch_homebrew_once_and_listings_do_not() {
    let cancel = Cancellation::default();
    let fetched = Arc::new(AtomicBool::new(false));
    let script = brew({
        let fetched = fetched.clone();
        move |line| {
            if line == "w:brew update" {
                fetched.store(true, Ordering::SeqCst);
                return ok("");
            }
            let fresh = fetched.load(Ordering::SeqCst);
            let (version, outdated) = if fresh { ("2.0", true) } else { ("1.0", false) };
            match line {
                "brew info --json=v2 --formula --installed" => {
                    ok(&formula_installed(version, outdated))
                }
                "brew info --json=v2 --cask --installed" => ok(&cask_installed(version, outdated)),
                other => panic!("unexpected {other}"),
            }
        }
    });
    let mut engine = Engine::default();
    engine.register(Homebrew::new(script.clone())).unwrap();
    engine.register(HomebrewCask::new(script.clone())).unwrap();
    let listed = engine.installed(&cancel);
    assert!(listed.failures.is_empty(), "{:?}", listed.failures);
    assert_eq!(brew_updates(&script), 0);
    assert!(listed.packages.iter().all(|package| {
        package.update == UpdateAvailability::Current
            && package.installed_version.as_deref() == Some("1.0")
            && package.candidate_version.as_deref() == Some("1.0")
    }));
    assert_eq!(listed.packages.len(), 2);

    let checked = engine.installed_for_updates(&cancel);
    assert!(checked.failures.is_empty(), "{:?}", checked.failures);
    assert_eq!(brew_updates(&script), 1);
    let mut sources = checked.successful_sources.clone();
    sources.sort();
    assert_eq!(sources, ["homebrew", "homebrew-cask"]);
    for name in ["wget", "pkgdeck"] {
        let package = checked
            .packages
            .iter()
            .find(|package| package.id.name == name)
            .unwrap();
        assert_eq!(package.update, UpdateAvailability::Available, "{name}");
        assert_eq!(package.installed_version.as_deref(), Some("1.0"));
        assert_eq!(package.candidate_version.as_deref(), Some("2.0"));
    }

    // The next check fetches again.
    let again = engine.installed_for_updates(&cancel);
    assert!(again.failures.is_empty(), "{:?}", again.failures);
    assert_eq!(brew_updates(&script), 2);
    assert!(again
        .packages
        .iter()
        .all(|package| package.update == UpdateAvailability::Available));
}

#[test]
fn a_failed_homebrew_fetch_is_shared_and_keeps_the_known_packages() {
    let cancel = Cancellation::default();
    let script = brew(|line| {
        if line == "w:brew update" {
            return failed(1);
        }
        match line {
            "brew info --json=v2 --formula --installed" => ok(&formula_installed("1.0", false)),
            "brew info --json=v2 --cask --installed" => ok(&cask_installed("1.0", false)),
            other => panic!("unexpected {other}"),
        }
    });
    let mut engine = Engine::default();
    engine.register(Homebrew::new(script.clone())).unwrap();
    engine.register(HomebrewCask::new(script.clone())).unwrap();
    let report = engine.installed_for_updates(&cancel);
    assert_eq!(brew_updates(&script), 1);
    assert!(
        report.successful_sources.is_empty(),
        "{:?}",
        report.successful_sources
    );
    assert_eq!(report.packages.len(), 2);
    assert!(report
        .packages
        .iter()
        .all(|package| package.update == UpdateAvailability::Current));
    assert_eq!(report.failures.len(), 2);
    let mut backends: Vec<_> = report
        .failures
        .iter()
        .map(|failure| failure.backend.as_str())
        .collect();
    backends.sort();
    assert_eq!(backends, ["homebrew", "homebrew-cask"]);
    for failure in &report.failures {
        match &failure.error {
            EngineError::Execution(ExecutionError::Failed(completion)) => {
                assert_eq!(completion.code, Some(1));
            }
            other => panic!("expected the shared brew failure, got {other:?}"),
        }
    }
}

#[test]
fn homebrew_formula_details_report_a_failed_lookup() {
    let cancel = Cancellation::default();
    let mut homebrew = Homebrew::new(brew(|_| failed(1)));
    assert_eq!(homebrew.detect(&cancel).unwrap(), Availability::Available);
    native_failure(homebrew.details(&brew_id("homebrew", "wget"), &cancel), 1);
}

#[test]
fn homebrew_casks_validate_tokens_and_report_failed_lookups() {
    let cancel = Cancellation::default();
    let detected = |script: Script| {
        let mut cask = HomebrewCask::new(script);
        assert_eq!(cask.detect(&cancel).unwrap(), Availability::Available);
        cask
    };
    // A cask without a homepage has no grouping key; the display name falls
    // back to the token.
    let mut cask = detected(brew(|line| match line {
        "brew casks" => ok("plain\n"),
        "brew info --json=v2 --cask -- plain" => ok(&cask_json("plain", "")),
        other => panic!("unexpected {other}"),
    }));
    let found = cask.search("plain", &cancel).unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].homepages.is_empty());
    assert_eq!(found[0].display_name, "plain");
    // `brew casks` and `brew info` may only name valid tokens.
    let mut cask = detected(brew(|_| ok("Bad Token\n")));
    invalid_response(cask.search("bad", &cancel), "invalid cask token");
    let mut cask = detected(brew(|line| match line {
        "brew casks" => ok("good\n"),
        _ => ok(&cask_json("-bad", "")),
    }));
    invalid_response(cask.search("good", &cancel), "invalid cask token");
    // Lookups keep brew's failures other than "no such cask".
    let mut cask = detected(brew(|_| failed(2)));
    native_failure(cask.lookup("good", &cancel), 2);
    native_failure(cask.installed(&cancel), 2);
    native_failure(cask.details(&brew_id("homebrew-cask", "good"), &cancel), 2);
    let mut cask = detected(brew(|_| ok("not json")));
    assert!(matches!(
        cask.installed(&cancel),
        Err(EngineError::InvalidResponse { .. })
    ));
    // Only installs can adopt an app, so only they have a plan.
    let script = brew(|_| ok(""));
    let mut cask = detected(script.clone());
    let upgrade = Operation::Upgrade(brew_id("homebrew-cask", "good"));
    assert_eq!(cask.operation_plan(&upgrade, &cancel).unwrap(), None);
    assert!(script.calls().iter().all(|line| !line.contains("info")));
}

#[test]
fn homebrew_cask_detection_rejects_a_prefix_that_is_not_text() {
    let mut cask = HomebrewCask::new(Script::new(|_| {
        Ok(Completion {
            code: Some(0),
            signal: None,
            stdout: vec![0xff, b'\n'],
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
        })
    }));
    assert!(matches!(
        cask.detect(&Cancellation::default()),
        Err(EngineError::InvalidResponse { .. })
    ));
}

fn manager(kind: &str, script: Script) -> SystemManager<Script> {
    match kind {
        "dnf" => SystemManager::dnf(script),
        "pacman" => SystemManager::pacman(script),
        "zypper" => SystemManager::zypper(script),
        "snap" => SystemManager::snap(script),
        "apk" => SystemManager::apk(script),
        "xbps" => SystemManager::xbps(script),
        _ => SystemManager::macports(script),
    }
}

#[test]
fn apk_xbps_and_macports_writes_use_fixed_noninteractive_commands() {
    let all = |backend: &str| {
        let id = system_id(backend, "tool", std::env::consts::ARCH);
        [
            Operation::Refresh {
                backend: backend.into(),
            },
            Operation::Install(id.clone()),
            Operation::Remove(id.clone()),
            Operation::Upgrade(id),
            Operation::UpgradeAll {
                backend: backend.into(),
            },
        ]
    };
    for (backend, expected) in [
        (
            "apk",
            [
                "w:apk update",
                "w:apk add -- tool",
                "w:apk del -- tool",
                "w:apk upgrade -- tool",
                "w:apk upgrade",
            ],
        ),
        (
            "xbps",
            [
                "w:xbps-install -S",
                "w:xbps-install -y tool",
                "w:xbps-remove -y tool",
                "w:xbps-install -yu tool",
                "w:xbps-install -yu",
            ],
        ),
        (
            "macports",
            [
                "w:port -N selfupdate",
                "w:port -N install tool",
                "w:port -N uninstall tool",
                "w:port -N upgrade tool",
                "w:port -N upgrade outdated",
            ],
        ),
    ] {
        let script = Script::new(|_| ok(""));
        let mut backend_impl = manager(backend, script.clone());
        for operation in all(backend) {
            run(&mut backend_impl, &operation).unwrap();
        }
        assert_eq!(script.calls(), expected);
        // None of them offers a cleanup task.
        assert!(matches!(
            run(
                &mut backend_impl,
                &Operation::Clean(cleanup_id(backend, "cache"))
            ),
            Err(EngineError::Unsupported {
                capability: Capability::Clean,
                ..
            })
        ));
        assert_eq!(script.calls().len(), 5);
    }
}

#[test]
fn apk_refuses_local_archives_before_fetching_them() {
    let script = Script::new(|_| ok(""));
    let mut apk = SystemManager::apk(script.clone());
    let mut id = system_id("apk", "tool", "x86_64");
    id.reference = Some(format!(
        "artifact:deb:{}::https://example.invalid/tool.deb",
        "0".repeat(64)
    ));
    invalid_response(
        run(&mut apk, &Operation::Install(id)),
        "unsupported local archive",
    );
    assert!(script.calls().is_empty());
}

#[test]
fn system_managers_reject_malformed_listings() {
    let cancel = Cancellation::default();
    for (kind, installed, output, expected) in [
        ("dnf", true, "tool|x86_64\n", "invalid repoquery metadata"),
        (
            "dnf",
            true,
            "tool|x86_64||Summary\n",
            "invalid package metadata",
        ),
        ("pacman", true, "tool\n", "invalid package metadata"),
        (
            "zypper",
            true,
            r#"<solvable name="tool" arch="x86_64"/>"#,
            "invalid XML metadata",
        ),
        (
            "snap",
            true,
            "Name Version\ntool\n",
            "invalid list metadata",
        ),
    ] {
        let mut backend = manager(kind, Script::new(move |_| ok(output)));
        let result = if installed {
            backend.installed(&cancel)
        } else {
            backend.search("tool", &cancel)
        };
        invalid_response(result, expected);
    }
    // Blank lines between Pacman entries are skipped.
    let mut pacman = manager(
        "pacman",
        Script::new(|line| match line {
            "pacman -Q" => ok("\ntool 1.0-1\n\n"),
            _ => failed(1),
        }),
    );
    let installed = pacman.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].installed_version.as_deref(), Some("1.0-1"));
}

#[test]
fn system_manager_queries_validate_names_and_exit_codes() {
    let cancel = Cancellation::default();
    let script = Script::new(|_| exit(1, ""));
    let mut dnf = manager("dnf", script.clone());
    invalid_response(dnf.search("-rf", &cancel), "invalid package query");
    assert!(script.calls().is_empty());
    native_failure(dnf.search("tool", &cancel), 1);
}

#[test]
fn update_listings_propagate_cancellation_and_tolerate_failure() {
    let cancel = Cancellation::default();
    let listing = |updates: fn() -> Reply| {
        manager(
            "apk",
            Script::new(move |line| match line {
                "apk list --installed" => ok("tool-1.0-r0 x86_64 {tool} (MIT) [installed]\n"),
                _ => updates(),
            }),
        )
    };
    assert!(matches!(
        listing(|| Err(ExecutionError::Cancelled)).installed(&cancel),
        Err(EngineError::Cancelled)
    ));
    let installed = listing(|| failed(1)).installed(&cancel).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].update, UpdateAvailability::Current);
}

/// Reading a `.snap` runs `snap info`, so the check runs in a child test
/// process whose PATH holds only a synthetic `snap`.
#[test]
fn snap_archives_stop_when_the_assertion_is_not_acknowledged() {
    use std::os::unix::fs::PermissionsExt;
    let Some(base) = std::env::var_os("PKGDECK_ENGINE_SNAP_DIR").map(std::path::PathBuf::from)
    else {
        let base = temp_dir("snap");
        let snap = base.join("snap");
        std::fs::write(
            &snap,
            "#!/bin/sh\nprintf 'name: synthetic-snap\\nversion: 1.0\\nsummary: Synthetic\\n'\n",
        )
        .unwrap();
        std::fs::set_permissions(&snap, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "snap_archives_stop_when_the_assertion_is_not_acknowledged",
            ])
            .env("PKGDECK_ENGINE_SNAP_DIR", &base)
            .env("PATH", &base)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        std::fs::remove_dir_all(base).unwrap();
        return;
    };
    let archive = base.join("sample.snap");
    std::fs::write(&archive, b"synthetic snap").unwrap();
    std::fs::write(base.join("sample.assert"), b"synthetic assertion").unwrap();
    let cancel = Cancellation::default();
    let package = pkgdeck_core::artifact::inspect(archive.to_str().unwrap(), &cancel).unwrap();
    let install = Operation::Install(package.id);
    for (answer, check) in [
        (
            (|| Err(ExecutionError::Disabled("snap not found".into()))) as fn() -> Reply,
            (|result| {
                assert!(matches!(
                    result,
                    Err(EngineError::Execution(ExecutionError::Disabled(_)))
                ))
            }) as fn(Result<OperationOutcome, EngineError>),
        ),
        (|| exit(1, ""), |result| native_failure(result, 1)),
    ] {
        let script = Script::new(move |_| answer());
        let mut snap = SystemManager::snap(script.clone());
        check(run(&mut snap, &install));
        let calls = script.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].starts_with("w:snap ack "));
    }
}

fn dev_id(backend: &str, name: &str, home: &str) -> PackageId {
    PackageId {
        backend: backend.into(),
        name: name.into(),
        architecture: std::env::consts::ARCH.into(),
        scope: Scope::Environment { path: home.into() },
        remote: None,
        reference: None,
    }
}
fn detected(mut tool: DevTool<Script>) -> DevTool<Script> {
    assert_eq!(
        tool.detect(&Cancellation::default()).unwrap(),
        Availability::Available
    );
    tool
}

#[test]
fn explicit_manager_homes_scope_pipx_composer_and_gem_tools() {
    for (make, variable, home, backend, name) in [
        (
            DevTool::pipx as fn(Script) -> DevTool<Script>,
            "PIPX_HOME",
            "/opt/pipx",
            "pipx",
            "tool",
        ),
        (
            DevTool::composer,
            "COMPOSER_HOME",
            "/opt/composer",
            "composer",
            "vendor/tool",
        ),
        (DevTool::gem, "GEM_HOME", "/opt/gems", "gem", "tool"),
    ] {
        let script = Script::new(|line| {
            if line.ends_with("--version") {
                ok("1.0\n")
            } else {
                failed(1)
            }
        })
        .with_env(variable, home);
        let mut tool = detected(make(script.clone()));
        // The configured home is the scope; no home lookup runs.
        assert_eq!(script.calls().len(), 1);
        // Only a package in that home reaches the (failing) native upgrade.
        invalid_response(
            run(
                &mut tool,
                &Operation::Upgrade(dev_id(backend, name, "/elsewhere")),
            ),
            "foreign identity",
        );
        native_failure(
            run(&mut tool, &Operation::Upgrade(dev_id(backend, name, home))),
            1,
        );
        assert!(script.calls()[1].starts_with(&format!("w:{backend} ")));
    }
}

#[test]
fn development_inventories_report_failed_listings() {
    let cancel = Cancellation::default();
    let mut pip = detected(DevTool::pip(
        Script::new(|line| match line {
            "pip --version" => ok("pip 25.0\n"),
            _ => failed(1),
        })
        .with_env("VIRTUAL_ENV", "/venv"),
    ));
    native_failure(pip.installed(&cancel), 1);
    let mut composer = detected(DevTool::composer(
        Script::new(|line| match line {
            "composer --version" => ok("Composer 2\n"),
            _ => failed(1),
        })
        .with_env("COMPOSER_HOME", "/opt/composer"),
    ));
    native_failure(composer.installed(&cancel), 1);
    // Composer upgrades each installed package; a failed one stops the batch.
    let script = Script::new(|line| match line {
        "composer --version" => ok("Composer 2\n"),
        "composer global show --format=json" => {
            ok(r#"{"installed":[{"name":"vendor/tool","version":"1.0.0"}]}"#)
        }
        "composer global show --outdated --format=json" => ok(r#"{"installed":[]}"#),
        _ => failed(1),
    })
    .with_env("COMPOSER_HOME", "/opt/composer");
    let mut composer = detected(DevTool::composer(script.clone()));
    native_failure(
        run(
            &mut composer,
            &Operation::UpgradeAll {
                backend: "composer".into(),
            },
        ),
        1,
    );
    assert_eq!(
        script.calls().last().unwrap(),
        "w:composer global require --no-interaction --no-progress vendor/tool"
    );
}

#[test]
fn mise_lists_tools_without_update_data_and_upgrades_only_installed_ones() {
    let cancel = Cancellation::default();
    let script = Script::new(|line| match line {
        "mise --version" => ok("2026.9.0\n"),
        "mise ls --global --json" => ok(
            r#"{"node":[{"version":"22.0.0","installed":true,"active":true}],"go":[{"version":"1.25","installed":false}]}"#,
        ),
        "mise outdated --json" => failed(1),
        "mise registry --json" => ok(
            r#"[{"short":"nodejs-extra","description":"Other","aliases":[]},{"short":"node","aliases":["nodejs"]},{"short":"bun","aliases":["nodejs"]}]"#,
        ),
        other => panic!("unexpected {other}"),
    })
    .with_env("MISE_DATA_DIR", "/opt/mise");
    let mut mise = detected(DevTool::mise(script.clone()));
    // An unavailable update check leaves the installed version current.
    let installed = mise.installed(&cancel).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].summary, "Global mise tool");
    assert_eq!(installed[0].update, UpdateAvailability::Current);
    // Exact names use the registry too; alias matches rank before others.
    let found = mise.lookup("nodejs", &cancel).unwrap();
    let names: Vec<_> = found
        .iter()
        .map(|package| package.id.name.as_str())
        .collect();
    assert_eq!(names, ["node", "bun", "nodejs-extra", "nodejs"]);
    assert_eq!(found[0].installed_version.as_deref(), Some("22.0.0"));
    // With no installed global tool, upgrading everything does nothing.
    let script = Script::new(|line| match line {
        "mise --version" => ok("2026.9.0\n"),
        "mise ls --global --json" => ok("{}"),
        other => panic!("unexpected {other}"),
    })
    .with_env("MISE_DATA_DIR", "/opt/mise");
    let mut mise = detected(DevTool::mise(script.clone()));
    run(
        &mut mise,
        &Operation::UpgradeAll {
            backend: "mise".into(),
        },
    )
    .unwrap();
    assert!(script.calls().iter().all(|line| !line.starts_with("w:")));
}

#[test]
fn ai_tool_offers_do_not_repeat_a_registry_hit() {
    let script = Script::new(|line| match line {
        "npm root --global" => ok("/opt/npm/lib/node_modules\n"),
        "npm ls --global --depth=0 --json" => ok(r#"{"dependencies":{}}"#),
        line if line.starts_with("npm search") => {
            ok(r#"[{"name":"@openai/codex","version":"1.0.0","description":"Registry entry"}]"#)
        }
        _ => failed(1),
    });
    let mut npm = detected(DevTool::npm(script));
    let found = npm.search("codex", &Cancellation::default()).unwrap();
    let codex: Vec<_> = found
        .iter()
        .filter(|package| package.id.name == "@openai/codex")
        .collect();
    assert_eq!(codex.len(), 1);
    assert_eq!(codex[0].summary, "Registry entry");
}

#[test]
fn cargo_rejects_listed_tools_with_invalid_names() {
    let mut cargo = detected(DevTool::cargo(
        Script::new(|line| match line {
            "cargo --version" => ok("cargo 1.90.0\n"),
            _ => ok("-evil v1.0.0:\n    evil\n"),
        })
        .with_env("HOME", "/home/user"),
    ));
    invalid_response(
        cargo.installed(&Cancellation::default()),
        "invalid package name",
    );
}

#[test]
fn flatpak_update_checks_and_bundle_scopes() {
    let cancel = Cancellation::default();
    let app = "org.example.App\tx86_64\tstable\t1.0\tApp\tflathub\t\tExample";
    // The update check cannot run at all.
    let mut flatpak = Flatpak::new(Script::new(move |line| {
        if line.contains(" list ") {
            ok(app)
        } else {
            Err(ExecutionError::Disabled("Flatpak not found".into()))
        }
    }));
    assert!(matches!(
        flatpak.installed(&cancel),
        Err(EngineError::Execution(ExecutionError::Disabled(_)))
    ));
    // Bundles are imported for the current user; the reviewed identity
    // cannot be moved to the system installation.
    let base = temp_dir("system-bundle");
    let bundle = base.join("synthetic.flatpak");
    std::fs::write(&bundle, b"synthetic system-scope bundle").unwrap();
    let mut id = pkgdeck_core::artifact::inspect(bundle.to_str().unwrap(), &cancel)
        .unwrap()
        .id;
    id.scope = Scope::System;
    let script = Script::new(|_| ok(""));
    let mut flatpak = Flatpak::new(script.clone());
    invalid_response(
        run(&mut flatpak, &Operation::Install(id)),
        "identity changed",
    );
    assert!(script.calls().is_empty());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn apt_plans_and_writes_cover_upgrades() {
    let cancel = Cancellation::default();
    let upgrade = Operation::Upgrade(system_id("apt", "tool", "amd64"));
    let plan = Apt::new(Script::new(|_| {
        ok("Inst tool [0.9] (1.0 Debian:stable [amd64])\n1 upgraded, 0 newly installed, 0 to remove and 0 not upgraded.\n")
    }))
    .operation_plan(&upgrade, &cancel)
    .unwrap()
    .unwrap();
    assert_eq!(plan.changes[0].action, PlannedAction::Upgrade);
    assert_eq!(plan.changes[0].installed_version.as_deref(), Some("0.9"));
    // APT refusing a simulation is a failure, not an empty plan.
    native_failure(
        Apt::new(Script::new(|_| exit(100, ""))).operation_plan(&upgrade, &cancel),
        100,
    );
    native_failure(
        Apt::new(Script::new(|_| exit(100, ""))).apt_upgrade_plan(&cancel),
        100,
    );
    // A full upgrade is one native write; upgrades of several packages
    // share one transaction.
    let script = Script::new(|_| ok(""));
    let mut apt = Apt::new(script.clone());
    run(
        &mut apt,
        &Operation::UpgradeAll {
            backend: "apt".into(),
        },
    )
    .unwrap();
    assert_eq!(script.calls(), ["w:apt UpgradeAll"]);
    struct Group(Arc<Mutex<String>>);
    impl Transport for Group {
        fn apt_write_group(&self, actions: &[AptAction], _: &Cancellation) -> Reply {
            *self.0.lock().unwrap() = format!("{actions:?}");
            ok("")
        }
    }
    let grouped = Arc::new(Mutex::new(String::new()));
    Apt::new(Group(grouped.clone()))
        .execute_group(
            &[
                upgrade,
                Operation::Upgrade(system_id("apt", "other", "amd64")),
            ],
            &cancel,
            &mut |_| {},
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        *grouped.lock().unwrap(),
        r#"[Upgrade("tool:amd64"), Upgrade("other:amd64")]"#
    );
}

#[test]
fn homebrew_upgrades_every_formula_and_validates_cask_answers() {
    let cancel = Cancellation::default();
    let script = brew(|_| ok(""));
    let mut homebrew = Homebrew::new(script.clone());
    homebrew.detect(&cancel).unwrap();
    run(
        &mut homebrew,
        &Operation::UpgradeAll {
            backend: "homebrew".into(),
        },
    )
    .unwrap();
    assert_eq!(script.calls().last().unwrap(), "w:brew upgrade --formula");
    // A cask prefix must be absolute.
    let mut cask = HomebrewCask::new(Script::new(|_| ok("relative/prefix\n")));
    invalid_response(cask.detect(&cancel), "prefix must be absolute");
    // Details need a readable report.
    let mut cask = HomebrewCask::new(brew(|_| ok("not json")));
    cask.detect(&cancel).unwrap();
    assert!(matches!(
        cask.details(&brew_id("homebrew-cask", "good"), &cancel),
        Err(EngineError::InvalidResponse { .. })
    ));
}

#[test]
fn development_homes_come_from_the_environment_or_the_manager() {
    let cancel = Cancellation::default();
    // uv's configured tool directory is used as is.
    let script = Script::new(|_| ok("uv 0.9\n")).with_env("UV_TOOL_DIR", "/opt/uv-tools");
    let mut uv = detected(DevTool::uv(script.clone()));
    assert_eq!(script.calls(), ["uv --version"]);
    native_failure_or_ok(&mut uv);
    // RubyGems names its user directory in `gem env`.
    for (env, expected) in [
        (
            "  - USER INSTALLATION DIRECTORY: /home/user/.local/share/gem/ruby/3.3.0\n",
            Ok(()),
        ),
        ("  - USER INSTALLATION DIRECTORY: relative\n", Err(())),
        ("  - INSTALLATION DIRECTORY: /usr/lib/ruby\n", Err(())),
    ] {
        let mut gem = DevTool::gem(Script::new(move |line| match line {
            "gem env" => ok(env),
            _ => ok("3.5\n"),
        }));
        match (gem.detect(&cancel), expected) {
            (Ok(Availability::Available), Ok(())) => {}
            (Err(EngineError::Execution(ExecutionError::Invalid(reason))), Err(())) => {
                assert_eq!(reason, "RubyGems user directory must be absolute")
            }
            (other, _) => panic!("{env:?}: {other:?}"),
        }
    }
    let _ = cancel;
}

/// uv lists nothing installed here, so upgrading everything writes nothing.
fn native_failure_or_ok(uv: &mut DevTool<Script>) {
    assert!(uv.installed(&Cancellation::default()).unwrap().is_empty());
}

#[test]
fn development_listings_report_failing_exit_statuses() {
    let cancel = Cancellation::default();
    let mut pip = detected(DevTool::pip(
        Script::new(|line| match line {
            "pip --version" => ok("pip 25.0\n"),
            _ => exit(2, ""),
        })
        .with_env("VIRTUAL_ENV", "/venv"),
    ));
    native_failure(pip.installed(&cancel), 2);
    let mut composer = detected(DevTool::composer(
        Script::new(|line| match line {
            "composer --version" => ok("Composer 2\n"),
            _ => exit(2, ""),
        })
        .with_env("COMPOSER_HOME", "/opt/composer"),
    ));
    native_failure(composer.installed(&cancel), 2);
    // mise names with an invalid backend prefix are refused.
    let script = Script::new(|_| ok("2026.9.0\n")).with_env("MISE_DATA_DIR", "/opt/mise");
    let mut mise = detected(DevTool::mise(script.clone()));
    for name in ["Upper:tool", ":tool"] {
        invalid_response(
            run(
                &mut mise,
                &Operation::Upgrade(dev_id("mise", name, "/opt/mise")),
            ),
            "foreign identity",
        );
    }
    assert_eq!(script.calls().len(), 1);
}
