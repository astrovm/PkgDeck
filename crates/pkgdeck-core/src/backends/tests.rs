use super::*;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

#[test]
fn pacman_pins_are_the_exact_versions_each_package_depends_on() {
    let text = "Name            : systemd\n\
Version         : 262-1\n\
Architecture    : x86_64\n\
Depends On      : systemd-libs=262  acl  linux-api-headers>=4.10  bash<6  zlib=1:1.3.2-3\n\
Optional Deps   : curl: systemd-journal-upload [installed]\n                  \
polkit=1: not a dependency\n\
\n\
Name            : tool\n\
Architecture    : any\n\
Depends On      : None\n                  \
tool-libs=2\n\
\n\
Version         : 1-1\n";
    assert_eq!(
        pacman_pins(text),
        [
            PacmanPins {
                name: "systemd".into(),
                architecture: "x86_64".into(),
                pins: vec![
                    ("systemd-libs".into(), "262".into()),
                    ("zlib".into(), "1:1.3.2-3".into()),
                ]
            },
            // A value that goes on below still counts.
            PacmanPins {
                name: "tool".into(),
                architecture: "any".into(),
                pins: vec![("tool-libs".into(), "2".into())],
            },
        ]
    );
    assert!(pacman_pins("").is_empty());
}

#[test]
fn a_pin_without_a_release_matches_every_release_of_its_version() {
    assert!(same_pacman_version("262-1", "262"));
    assert!(same_pacman_version("262-2", "262"));
    assert!(same_pacman_version("262-1", "262-1"));
    assert!(same_pacman_version("1:1.3.2-4", "1:1.3.2"));
    assert!(!same_pacman_version("262.1-1", "262"));
    assert!(!same_pacman_version("262-2", "262-1"));
    assert!(!same_pacman_version("2620-1", "262"));
    assert!(!same_pacman_version("1:1.3.2-4", "1:1.3.2-3"));
}

#[test]
fn names_read_as_a_list() {
    let names = |list: &[&str]| and_list(&list.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>());
    assert_eq!(names(&[]), "");
    assert_eq!(names(&["a"]), "a");
    assert_eq!(names(&["a", "b"]), "a and b");
    assert_eq!(names(&["a", "b", "c"]), "a, b, and c");
}

#[test]
fn flatpak_apps_start_from_the_installation_they_are_in() {
    let id = |scope: Scope, name: &str| PackageId {
        backend: "flatpak".into(),
        name: name.into(),
        architecture: "x86_64".into(),
        scope,
        remote: None,
        reference: Some("flatpakref:ab:https://example.org/App.flatpakref".into()),
    };
    let arguments = |id: &PackageId| flatpak_launch_command(id).map(|c| (c.program, c.arguments));
    assert_eq!(
        arguments(&id(Scope::System, "org.example.App")).unwrap(),
        (
            "flatpak".into(),
            vec![
                "run".into(),
                "--system".into(),
                "--arch=x86_64".into(),
                "org.example.App".into()
            ]
        )
    );
    assert_eq!(
        arguments(&id(Scope::User { uid: 1000 }, "org.example.App"))
            .unwrap()
            .1[1],
        "--user"
    );
    let mut no_arch = id(Scope::System, "org.example.App");
    no_arch.architecture.clear();
    assert_eq!(
        arguments(&no_arch).unwrap().1,
        ["run", "--system", "org.example.App"]
    );
    // An option is never taken for the app, and other managers never run.
    assert!(arguments(&id(Scope::System, "--command=sh")).is_err());
    assert!(arguments(&id(Scope::System, "")).is_err());
    assert!(arguments(&id(
        Scope::Environment {
            path: "/opt/env".into()
        },
        "org.example.App"
    ))
    .is_err());
    let mut apt = id(Scope::System, "org.example.App");
    apt.backend = "apt".into();
    assert!(arguments(&apt).is_err());
    // Refused before anything starts.
    assert!(launch_flatpak(&apt).is_err());
    // It starts detached, with those arguments, and says when it can't.
    let base = std::env::temp_dir().join(format!("pkgdeck-flatpak-launch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let program = base.join("flatpak");
    let record = base.join("arguments");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}.tmp'\nmv '{0}.tmp' '{0}'\n",
            record.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(
        &program,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    start_flatpak(
        &id(Scope::System, "org.example.App"),
        program.to_str().unwrap(),
        None,
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !record.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
        "run\n--system\n--arch=x86_64\norg.example.App\n"
    );
    let missing = base.join("missing");
    let error = start_flatpak(
        &id(Scope::System, "org.example.App"),
        missing.to_str().unwrap(),
        None,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("couldn't start it"), "{error}");
    std::fs::remove_dir_all(&base).unwrap();
}

fn completion(code: i32, stdout: &str) -> Completion {
    Completion {
        code: Some(code),
        signal: None,
        stdout: stdout.into(),
        stderr: vec![],
        truncated: false,
        cancellation_deferred: false,
    }
}

fn brew_run(code: i32, stderr: &str) -> Result<Completion, ExecutionError> {
    let result = Completion {
        code: Some(code),
        signal: None,
        stdout: vec![],
        stderr: stderr.into(),
        truncated: false,
        cancellation_deferred: false,
    };
    if code == 0 {
        Ok(result)
    } else {
        Err(ExecutionError::Failed(result))
    }
}
fn brew_exit(code: i32, stderr: &str) -> Result<Completion, EngineError> {
    Ok(brew_run(code, stderr)?)
}
const BREW_LOCKED: &str = "Error: Another `brew update` process is already running.\n\
        Please wait for it to finish or terminate it to continue.\n";

/// `brew` whose `update` finds another update running the first time.
#[derive(Clone, Default)]
struct LockedBrew {
    calls: Arc<Mutex<Vec<String>>>,
    writes: Arc<Mutex<Vec<bool>>>,
}
impl Transport for LockedBrew {
    fn brew(
        &self,
        args: &[OsString],
        _: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        let line = args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        let mut calls = self.calls.lock().unwrap();
        calls.push(line.clone());
        assert_eq!(line, "update");
        self.writes.lock().unwrap().push(write);
        let locked = calls.len() == 1;
        brew_run(i32::from(locked), if locked { BREW_LOCKED } else { "" })
    }
}

#[test]
fn homebrew_update_checks_wait_for_another_brew_update() {
    let token = crate::engine::begin_update_check_token();
    let formulae = LockedBrew::default();
    let mut homebrew = Homebrew::new(formulae.clone());
    homebrew.arm_update_check(Some(token));
    homebrew
        .refresh_update_index(&Cancellation::default())
        .unwrap();
    crate::engine::end_update_check_token(token);
    assert_eq!(*formulae.calls.lock().unwrap(), ["update", "update"]);
    assert_eq!(*formulae.writes.lock().unwrap(), [false, false]);

    let token = crate::engine::begin_update_check_token();
    let casks = LockedBrew::default();
    let mut cask = HomebrewCask::new(casks.clone());
    cask.arm_update_check(Some(token));
    cask.refresh_update_index(&Cancellation::default()).unwrap();
    crate::engine::end_update_check_token(token);
    assert_eq!(*casks.calls.lock().unwrap(), ["update", "update"]);
    assert_eq!(*casks.writes.lock().unwrap(), [false, false]);
}

#[test]
fn homebrew_refresh_waits_for_another_brew_update() {
    for backend in ["homebrew", "homebrew-cask"] {
        let brew = LockedBrew::default();
        let operation = Operation::Refresh {
            backend: backend.into(),
        };
        let outcome = if backend == "homebrew" {
            Homebrew::new(brew.clone()).execute(&operation, &Cancellation::default(), &mut |_| {})
        } else {
            HomebrewCask::new(brew.clone()).execute(
                &operation,
                &Cancellation::default(),
                &mut |_| {},
            )
        };
        assert!(outcome.is_ok(), "{backend}: {outcome:?}");
        assert_eq!(
            *brew.calls.lock().unwrap(),
            ["update", "update"],
            "{backend}"
        );
        assert_eq!(*brew.writes.lock().unwrap(), [true, true], "{backend}");
    }
}

/// A system with PackageKit (or not), whose managers print `listing`
/// for any read, and the commands they ran. With `copies`, the user's
/// own copies of indexes live under /c, newer than the system's when
/// `fresher`.
#[derive(Clone)]
struct Kit {
    packagekit: Result<bool, ExecutionError>,
    listing: &'static str,
    copies: bool,
    fresher: bool,
    missing: &'static str,
    ran: Arc<Mutex<Vec<String>>>,
}
impl Kit {
    fn new(packagekit: Result<bool, ExecutionError>, listing: &'static str) -> Self {
        Self {
            packagekit,
            listing,
            copies: false,
            fresher: false,
            missing: "",
            ran: Arc::default(),
        }
    }
    fn copies(mut self, fresher: bool) -> Self {
        self.copies = true;
        self.fresher = fresher;
        self
    }
    fn missing(mut self, executable: &'static str) -> Self {
        self.missing = executable;
        self
    }
    fn ran(&self) -> Vec<String> {
        self.ran.lock().unwrap().clone()
    }
    fn clear(&self) {
        self.ran.lock().unwrap().clear();
    }
}
impl Transport for Kit {
    fn private_index(&self, source: &str) -> Option<PathBuf> {
        self.fresher.then(|| PathBuf::from(format!("/c/{source}")))
    }
    fn prepare_private_index(
        &self,
        source: &str,
        _: &Cancellation,
    ) -> Result<Option<crate::private_index::Copy>, ExecutionError> {
        Ok(self
            .copies
            .then(|| crate::private_index::Copy::unlocked(format!("/c/{source}"))))
    }
    fn refresh_index(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        if executable == self.missing {
            return Err(ExecutionError::Disabled(format!("{executable} not found")));
        }
        // Marked apart from a plain read, which gives up after seconds.
        let result = self.system_manager(executable, args, cancel, false);
        let mut ran = self.ran.lock().unwrap();
        let line = ran.pop().unwrap();
        ran.push(format!("fetch {line}"));
        result
    }
    fn apt_write(&self, action: AptAction, _: &Cancellation) -> Result<Completion, ExecutionError> {
        self.ran
            .lock()
            .unwrap()
            .push(format!("root apt {action:?}"));
        Ok(completion(0, ""))
    }
    fn packagekit_refresh(&self, source: &str, _: &Cancellation) -> Result<bool, ExecutionError> {
        self.ran
            .lock()
            .unwrap()
            .push(format!("packagekit {source}"));
        self.packagekit.clone()
    }
    fn system_manager(
        &self,
        executable: &str,
        args: &[OsString],
        _: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        let line = std::iter::once(executable.to_owned())
            .chain(args.iter().map(|arg| arg.to_string_lossy().into_owned()))
            .collect::<Vec<_>>()
            .join(" ");
        self.ran
            .lock()
            .unwrap()
            .push(if write { format!("root {line}") } else { line });
        Ok(completion(0, self.listing))
    }
}

#[test]
fn a_check_refreshes_through_packagekit_only_when_armed() {
    let cancel = Cancellation::default();
    let kit = Kit::new(Ok(true), "");
    let mut apt = Apt::new(kit.clone());
    assert!(apt.has_update_index());
    // Listing Installed never refreshes.
    apt.refresh_update_index(&cancel).unwrap();
    assert!(kit.ran().is_empty());
    apt.arm_update_check(Some(7));
    apt.refresh_update_index(&cancel).unwrap();
    assert_eq!(kit.ran(), ["packagekit apt"]);
    for (manager, source) in [
        (SystemManager::pacman(kit.clone()), "pacman"),
        (SystemManager::zypper(kit.clone()), "zypper"),
    ] {
        let mut manager = manager;
        kit.ran.lock().unwrap().clear();
        assert!(manager.has_update_index());
        manager.arm_update_check(Some(7));
        manager.refresh_update_index(&cancel).unwrap();
        assert_eq!(kit.ran(), [format!("packagekit {source}")]);
    }
    // DNF refreshes its own cache as the user, never as root.
    kit.ran.lock().unwrap().clear();
    let mut dnf = SystemManager::dnf(kit.clone());
    dnf.arm_update_check(Some(7));
    dnf.refresh_update_index(&cancel).unwrap();
    assert_eq!(kit.ran(), ["fetch dnf --refresh --quiet makecache"]);
    // Without PackageKit's copy or one of its own, apk leaves its index.
    kit.clear();
    let mut apk = SystemManager::apk(kit.clone());
    assert!(apk.has_update_index());
    apk.arm_update_check(Some(7));
    apk.refresh_update_index(&cancel).unwrap();
    assert!(kit.ran().is_empty());
    // The rest have no index of their own to refresh.
    for mut manager in [
        SystemManager::xbps(kit.clone()),
        SystemManager::snap(kit.clone()),
        SystemManager::macports(kit.clone()),
    ] {
        kit.ran.lock().unwrap().clear();
        assert!(!manager.has_update_index());
        manager.arm_update_check(Some(7));
        manager.refresh_update_index(&cancel).unwrap();
        assert!(!manager.refresh_without_password(&cancel).unwrap());
        assert!(kit.ran().is_empty(), "{}", manager.id());
    }
}

#[test]
fn without_packagekit_a_check_fills_its_own_copy_as_the_user() {
    let cancel = Cancellation::default();
    // Root, or no home folder: nothing to fill, so the lists stay.
    let kit = Kit::new(Ok(false), "");
    let mut apt = Apt::new(kit.clone());
    apt.arm_update_check(Some(7));
    apt.refresh_update_index(&cancel).unwrap();
    assert_eq!(kit.ran(), ["packagekit apt"]);

    let kit = Kit::new(Ok(false), "").copies(false);
    let mut apt = Apt::new(kit.clone());
    apt.arm_update_check(Some(7));
    apt.refresh_update_index(&cancel).unwrap();
    let ran = kit.ran();
    assert_eq!(ran[0], "packagekit apt");
    assert!(
            ran[1].starts_with(
                "fetch apt-get -q -c /c/apt/apt.conf -o Dir::State::Lists=/c/apt/lists -o Dir::Cache=/c/apt/cache"
            ) && ran[1].ends_with(" update"),
            "{ran:?}"
        );
    assert_eq!(ran.len(), 2);
    for (mut manager, refresh) in [
        (
            SystemManager::pacman(kit.clone()),
            "fetch fakeroot -- pacman -Sy --dbpath /c/pacman --logfile /dev/null",
        ),
        (
            SystemManager::zypper(kit.clone()),
            "fetch zypper --cache-dir /c/zypper --quiet --non-interactive list-updates",
        ),
    ] {
        kit.clear();
        manager.arm_update_check(Some(7));
        manager.refresh_update_index(&cancel).unwrap();
        assert_eq!(
            kit.ran(),
            [format!("packagekit {}", manager.id()), refresh.into()]
        );
    }
    // apk has no PackageKit backend.
    kit.clear();
    let mut apk = SystemManager::apk(kit.clone());
    apk.arm_update_check(Some(7));
    apk.refresh_update_index(&cancel).unwrap();
    assert_eq!(
        kit.ran(),
        ["fetch apk --cache-dir /c/apk --update-cache list --upgradable"]
    );
    // The rest keep no copy.
    for mut manager in [
        SystemManager::xbps(kit.clone()),
        SystemManager::snap(kit.clone()),
        SystemManager::macports(kit.clone()),
    ] {
        kit.clear();
        manager.arm_update_check(Some(7));
        manager.refresh_update_index(&cancel).unwrap();
        assert!(kit.ran().is_empty(), "{}", manager.id());
    }
    // Not armed: Installed never refreshes anything.
    kit.clear();
    let mut pacman = SystemManager::pacman(kit.clone());
    pacman.refresh_update_index(&cancel).unwrap();
    assert!(kit.ran().is_empty());
    // PackageKit's refresh is enough.
    let kit = Kit::new(Ok(true), "").copies(false);
    let mut zypper = SystemManager::zypper(kit.clone());
    zypper.arm_update_check(Some(7));
    zypper.refresh_update_index(&cancel).unwrap();
    assert_eq!(kit.ran(), ["packagekit zypper"]);
    // Without fakeroot, pacman's updates stay as the system saw them.
    let kit = Kit::new(Ok(false), "").copies(false).missing("fakeroot");
    let mut pacman = SystemManager::pacman(kit.clone());
    pacman.arm_update_check(Some(7));
    assert_eq!(pacman.refresh_update_index(&cancel), Ok(()));
    assert_eq!(kit.ran(), ["packagekit pacman"]);
    // A missing manager is no such excuse: the check says so.
    let kit = Kit::new(Ok(false), "").copies(false).missing("zypper");
    let mut zypper = SystemManager::zypper(kit.clone());
    zypper.arm_update_check(Some(7));
    assert!(zypper.refresh_update_index(&cancel).is_err());
}

#[test]
fn a_failed_refresh_shows_in_the_check() {
    let cancel = Cancellation::default();
    let failed = ExecutionError::Invalid("PackageKit: no network".into());
    let mut pacman = SystemManager::pacman(Kit::new(Err(failed.clone()), "").copies(false));
    pacman.arm_update_check(Some(7));
    assert_eq!(
        pacman.refresh_update_index(&cancel),
        Err(EngineError::Execution(failed))
    );
    // apt-get can't reach a mirror.
    #[derive(Clone)]
    struct Offline;
    impl Transport for Offline {
        fn prepare_private_index(
            &self,
            _: &str,
            _: &Cancellation,
        ) -> Result<Option<crate::private_index::Copy>, ExecutionError> {
            Ok(Some(crate::private_index::Copy::unlocked("/c/apt")))
        }
        fn system_manager(
            &self,
            _: &str,
            _: &[OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            Err(ExecutionError::Failed(completion(100, "")))
        }
    }
    let mut apt = Apt::new(Offline);
    apt.arm_update_check(Some(7));
    assert!(matches!(
        apt.refresh_update_index(&cancel),
        Err(EngineError::Execution(ExecutionError::Failed(_)))
    ));
    let mut zypper = SystemManager::zypper(Offline);
    zypper.arm_update_check(Some(7));
    assert!(zypper.refresh_update_index(&cancel).is_err());
}

#[test]
fn updates_come_from_the_users_copy_only_while_it_is_newer() {
    let cancel = Cancellation::default();
    for (fresher, expected) in [
            (
                true,
                [
                    "pacman -Qu --dbpath /c/pacman",
                    "zypper --cache-dir /c/zypper --no-refresh --quiet --xmlout --non-interactive list-updates",
                    "apk --cache-dir /c/apk list --upgradable",
                    "apt-get --simulate -o Debug::NoLocking=1 -o Dir::State::Lists=/c/apt/lists dist-upgrade",
                ],
            ),
            (
                false,
                [
                    "pacman -Qu",
                    "zypper --quiet --xmlout --non-interactive list-updates",
                    "apk list --upgradable",
                    "apt-get --simulate -o Debug::NoLocking=1 dist-upgrade",
                ],
            ),
        ] {
            let kit = Kit::new(Ok(false), "").copies(fresher);
            for manager in [
                SystemManager::pacman(kit.clone()),
                SystemManager::zypper(kit.clone()),
                SystemManager::apk(kit.clone()),
            ] {
                manager.updates(&cancel).unwrap();
            }
            let _ = Apt::new(kit.clone()).apt_upgrade_plan(&cancel);
            assert_eq!(kit.ran(), expected);
            // The others keep their own: DNF's cache is the user's already.
            kit.clear();
            SystemManager::xbps(kit.clone()).updates(&cancel).unwrap();
            assert_eq!(kit.ran(), ["xbps-install -Mun"]);
        }
}

#[test]
fn a_transport_that_overrides_nothing_refreshes_nothing() {
    struct Bare;
    impl Transport for Bare {}
    let cancel = Cancellation::default();
    assert_eq!(Bare.packagekit_refresh("apt", &cancel), Ok(false));
    assert_eq!(Bare.private_index("apt"), None);
    assert!(Bare
        .prepare_private_index("apt", &cancel)
        .unwrap()
        .is_none());
    assert_eq!(
        Bare.refresh_index("dnf", &[], &cancel),
        Err(ExecutionError::Disabled("dnf not found".into()))
    );
}

#[test]
fn only_system_managers_refresh_as_root() {
    for id in ["apt", "dnf", "pacman", "zypper", "apk", "xbps", "macports"] {
        assert!(refresh_needs_root(id), "{id}");
    }
    for id in ["flatpak", "snap", "brew", "cargo", "npm"] {
        assert!(!refresh_needs_root(id), "{id}");
    }
}

#[test]
fn the_native_transport_refreshes_indexes_as_the_user() {
    let root = std::env::temp_dir().join(format!("pkgdeck-native-index-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let fetch = root.join("fetch");
    std::fs::write(&fetch, "#!/bin/sh\necho fetched\n").unwrap();
    std::fs::set_permissions(&fetch, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let native = NativeTransport {
        host: Host::new(
            crate::host::Runtime::Native,
            [(OsString::from("PATH"), root.clone().into_os_string())].into(),
        ),
        authorization: Authorization::Polkit,
    };
    let cancel = Cancellation::default();
    assert_eq!(
        native.refresh_index("fetch", &[], &cancel).unwrap().stdout,
        b"fetched\n"
    );
    // DNF keeps its own cache, not one of PkgDeck's copies.
    assert_eq!(native.private_index("dnf"), None);
    assert!(native
        .prepare_private_index("dnf", &cancel)
        .unwrap()
        .is_none());
    // Only APT, Pacman and Zypper refresh through PackageKit.
    assert_eq!(native.packagekit_refresh("flatpak", &cancel), Ok(false));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn every_root_command_is_the_one_the_batch_helper_approves() {
    // The runner refuses any command it didn't approve, so a manager's
    // write and its approved command must never drift apart.
    let cancel = Cancellation::default();
    type Make = fn(Kit) -> SystemManager<Kit>;
    let managers: [(&str, Make); 4] = [
        ("dnf", SystemManager::dnf),
        ("pacman", SystemManager::pacman),
        ("zypper", SystemManager::zypper),
        ("snap", SystemManager::snap),
    ];
    let mut approved_lines = BTreeMap::new();
    for (backend, manager) in managers {
        let id = PackageId {
            backend: backend.into(),
            name: "synthetic".into(),
            architecture: "x86_64".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        };
        let mut operations = vec![
            Operation::Install(id.clone()),
            Operation::Remove(id.clone()),
            Operation::Upgrade(id),
            Operation::UpgradeAll {
                backend: backend.into(),
            },
        ];
        // Snap's refresh only lists snaps, as the user.
        if backend != "snap" {
            operations.push(Operation::Refresh {
                backend: backend.into(),
            });
        }
        for operation in operations {
            let kit = Kit::new(Ok(false), "");
            manager(kit.clone())
                .execute(&operation, &cancel, &mut |_| {})
                .unwrap();
            let ran: Vec<String> = kit
                .ran()
                .into_iter()
                .filter(|line| line.starts_with("root "))
                .collect();
            let approved: Vec<String> = crate::batch::protected_commands(&operation)
                .unwrap()
                .iter()
                .map(|command| {
                    let program = std::path::Path::new(&command.program);
                    let program = program.file_name().unwrap().to_string_lossy();
                    format!("root {program} {}", command.args.join(" "))
                })
                .collect();
            assert_eq!(ran, approved, "{operation:?}");
            approved_lines.insert(format!("{operation:?}"), approved.join("; "));
        }
    }
    // Updates refresh the system's index first; a check refreshed only
    // the user's copy. Installs leave it, and pacman upgrades everything.
    let line = |op: &str, backend: &str| {
        approved_lines
            .iter()
            .find(|(key, _)| key.starts_with(op) && key.contains(&format!("\"{backend}\"")))
            .map(|(_, line)| line.clone())
            .unwrap()
    };
    assert_eq!(line("UpgradeAll", "dnf"), "root dnf -y --refresh upgrade");
    assert!(line("Upgrade(", "dnf").starts_with("root dnf -y --refresh upgrade -- "));
    assert_eq!(line("UpgradeAll", "pacman"), "root pacman -Syu --noconfirm");
    assert!(line("Upgrade(", "pacman").starts_with("root pacman -Sy --noconfirm --needed -- "));
    assert!(line("Install", "pacman").starts_with("root pacman -S --noconfirm --needed -- "));
    assert_eq!(line("Refresh", "pacman"), "root pacman -Sy --noconfirm");
}

#[test]
fn a_single_apt_update_previews_from_a_fresher_copy() {
    let cancel = Cancellation::default();
    let id = PackageId {
        backend: "apt".into(),
        name: "curl".into(),
        architecture: "amd64".into(),
        scope: Scope::System,
        remote: None,
        reference: None,
    };
    for (fresher, expected) in [
            (
                true,
                "apt-get --simulate -o Debug::NoLocking=1 -o Dir::State::Lists=/c/apt/lists install curl:amd64",
            ),
            (
                false,
                "apt-get --simulate -o Debug::NoLocking=1 install curl:amd64",
            ),
        ] {
            let kit = Kit::new(Ok(false), "").copies(fresher);
            let _ = Apt::new(kit.clone()).operation_plan(&Operation::Upgrade(id.clone()), &cancel);
            assert_eq!(kit.ran(), [expected]);
        }
}

#[test]
fn an_apt_update_refreshes_the_system_lists_first() {
    let cancel = Cancellation::default();
    let kit = Kit::new(Ok(true), "");
    let mut apt = Apt::new(kit.clone());
    let id = PackageId {
        backend: "apt".into(),
        name: "curl".into(),
        architecture: "amd64".into(),
        scope: Scope::System,
        remote: None,
        reference: None,
    };
    for operation in [
        Operation::UpgradeAll {
            backend: "apt".into(),
        },
        Operation::Upgrade(id.clone()),
    ] {
        kit.clear();
        let mut said = vec![];
        apt.prepare(&operation, &cancel, &mut |progress| said.push(progress))
            .unwrap();
        // As root, not through PackageKit: the update's approval covers it.
        assert_eq!(kit.ran(), ["root apt Refresh"], "{operation:?}");
        assert_eq!(said.len(), 1);
    }
    kit.clear();
    for operation in [
        Operation::Install(id.clone()),
        Operation::Remove(id),
        Operation::Refresh {
            backend: "apt".into(),
        },
    ] {
        apt.prepare(&operation, &cancel, &mut |_| {}).unwrap();
    }
    assert!(kit.ran().is_empty());
}

#[test]
fn dnf_pacman_and_zypper_say_which_packages_have_updates() {
    let cancel = Cancellation::default();
    let dnf = SystemManager::dnf(Kit::new(
        Ok(false),
        "htop|x86_64|3.4.1-1.fc44\nnot a row\n|x86_64|\nless|x86_64|704-4.fc44\n",
    ));
    assert_eq!(
        dnf.updates(&cancel).unwrap(),
        BTreeMap::from([
            ("htop".to_owned(), "3.4.1-1.fc44".to_owned()),
            ("less".to_owned(), "704-4.fc44".to_owned()),
        ])
    );
    let pacman = SystemManager::pacman(Kit::new(
        Ok(false),
        "curl 8.22.0-1 -> 8.23.0-1\nlinux 6.1-1 -> 6.2-1 [ignored]\nzlib 1:1.3.2-3 -> 1:1.3.2-4\n",
    ));
    // Held back in pacman.conf, the kernel isn't one an upgrade installs.
    assert_eq!(
        pacman.updates(&cancel).unwrap(),
        BTreeMap::from([
            ("curl".to_owned(), "8.23.0-1".to_owned()),
            ("zlib".to_owned(), "1:1.3.2-4".to_owned()),
        ])
    );
    let zypper = SystemManager::zypper(Kit::new(
        Ok(false),
        r#"<?xml version='1.0'?>
<stream>
<update-status version="0.6">
<update-list>
<update kind="package" name="openSUSE-build-key" edition="1.0-69.1" arch="x86_64" edition-old="1.0-68.1"><summary>The public gpg keys</summary><description>Keys > 1</description><license/><source url="http://download.opensuse.org/update/tumbleweed/" alias="repo-update"/></update>
<update kind="patch" name="openSUSE-2026-1" edition="1" arch="noarch"><summary>Fixes</summary></update>
<update kind="package" name="" edition="2" arch="x86_64"></update>
</update-list>
</update-status>
</stream>
"#,
    ));
    assert_eq!(
        zypper.updates(&cancel).unwrap(),
        BTreeMap::from([("openSUSE-build-key".to_owned(), "1.0-69.1".to_owned())])
    );
}

#[test]
fn dnf_reads_updates_online_when_it_has_no_cache_yet() {
    #[derive(Clone)]
    struct NoCache(Arc<Mutex<Vec<Vec<OsString>>>>);
    impl Transport for NoCache {
        fn system_manager(
            &self,
            _: &str,
            args: &[OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            self.0.lock().unwrap().push(args.to_vec());
            if args[0] == "-C" {
                return Err(ExecutionError::Failed(completion(1, "")));
            }
            Ok(completion(0, "htop|x86_64|3.4.1-1.fc44\n"))
        }
    }
    let calls = NoCache(Arc::default());
    let dnf = SystemManager::dnf(calls.clone());
    assert_eq!(dnf.updates(&Cancellation::default()).unwrap().len(), 1);
    let calls = calls.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1], calls[0][1..]);
    drop(calls);
    // Other managers have no online fallback: a failed listing is asked once.
    #[derive(Clone)]
    struct Down(Arc<Mutex<usize>>);
    impl Transport for Down {
        fn system_manager(
            &self,
            _: &str,
            _: &[OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            *self.0.lock().unwrap() += 1;
            Err(ExecutionError::Failed(completion(1, "")))
        }
    }
    let down = Down(Arc::default());
    let pacman = SystemManager::pacman(down.clone());
    assert_eq!(
        pacman.updates(&Cancellation::default()),
        Ok(BTreeMap::new())
    );
    assert_eq!(*down.0.lock().unwrap(), 1);
}

#[test]
fn snap_refresh_reports_a_failed_listing() {
    #[derive(Clone)]
    struct SnapdDown;
    impl Transport for SnapdDown {
        fn system_manager(
            &self,
            executable: &str,
            args: &[OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            assert_eq!(executable, "snap");
            assert_eq!(
                args.to_vec(),
                vec![OsString::from("refresh"), OsString::from("--list")]
            );
            Err(ExecutionError::Disabled("snapd is not running".into()))
        }
    }
    // Snap has no metadata-only refresh, so a failed listing must not
    // read as a successful refresh.
    let mut messages = Vec::new();
    let outcome = SystemManager::snap(SnapdDown).execute(
        &Operation::Refresh {
            backend: "snap".into(),
        },
        &Cancellation::default(),
        &mut |progress| messages.push(progress),
    );
    assert!(outcome.is_err(), "{outcome:?}");
    assert!(
        matches!(
            messages.as_slice(),
            [Progress::Message(text)] if text == "Reading the Snap package list…"
        ),
        "{messages:?}"
    );
}

#[test]
fn brew_update_waits_for_another_update_to_finish() {
    let mut calls = 0;
    let result = brew_update(&Cancellation::default(), || {
        calls += 1;
        if calls < 3 {
            brew_exit(1, BREW_LOCKED)
        } else {
            brew_exit(0, "")
        }
    });
    assert_eq!(result.unwrap().code, Some(0));
    assert_eq!(calls, 3);
}

#[test]
fn brew_update_reports_other_failures_at_once() {
    let mut calls = 0;
    let result = brew_update(&Cancellation::default(), || {
        calls += 1;
        brew_exit(
            1,
            "Error: Failure while executing; `git fetch` exited with 128.",
        )
    });
    assert!(matches!(
        result,
        Err(EngineError::Execution(ExecutionError::Failed(r))) if r.code == Some(1)
    ));
    assert_eq!(calls, 1);
}

#[test]
fn brew_update_gives_up_when_the_other_update_never_ends() {
    let started = std::time::Instant::now();
    let mut calls = 0;
    let result = brew_update(&Cancellation::default(), || {
        calls += 1;
        brew_exit(1, BREW_LOCKED)
    });
    assert!(matches!(
        result,
        Err(EngineError::Execution(ExecutionError::Failed(r))) if brew_update_locked(&r)
    ));
    assert!(started.elapsed() >= BREW_UPDATE_PATIENCE);
    assert!(calls > 2);
}

#[test]
fn brew_update_stops_waiting_when_cancelled() {
    let cancel = Cancellation::default();
    let mut calls = 0;
    let result = brew_update(&cancel, || {
        calls += 1;
        cancel.cancel();
        brew_exit(1, BREW_LOCKED)
    });
    assert!(matches!(result, Err(EngineError::Cancelled)));
    assert_eq!(calls, 1);
}

#[test]
fn linux_searches_only_offer_casks_linux_can_install() {
    // Shapes `brew info --json=v2 --cask` reports on Linux (Homebrew 7.0.6).
    let cask = |depends_on: serde_json::Value, artifacts: serde_json::Value| {
        serde_json::from_value::<Cask>(serde_json::json!({
            "full_token": "fixture", "name": ["Fixture"], "desc": null,
            "homepage": "", "version": "1", "installed": null, "outdated": false,
            "depends_on": depends_on, "artifacts": artifacts,
        }))
        .unwrap()
    };
    let obsidian = cask(
        serde_json::json!({}),
        serde_json::json!([{"app_image": ["Obsidian.AppImage"]}]),
    );
    let codex = cask(
        serde_json::json!({}),
        serde_json::json!([{"binary": ["codex"]}, {"zap": []}]),
    );
    let font = cask(
        serde_json::json!({}),
        serde_json::json!([{"font": ["Fira.ttf"]}]),
    );
    let rectangle = cask(
        serde_json::json!({"macos": {">=": ["14"]}}),
        serde_json::json!([{"app": ["Rectangle.app"]}]),
    );
    let undeclared_app = cask(
        serde_json::json!({}),
        serde_json::json!([{"app": ["Tool.app"]}]),
    );
    let pkg = cask(
        serde_json::json!({}),
        serde_json::json!([{"pkg": ["Tool.pkg"]}]),
    );
    for installable in [&obsidian, &codex, &font] {
        assert!(cask_installs_here(installable));
    }
    for mac_only in [&rectangle, &undeclared_app, &pkg] {
        assert_eq!(cask_installs_here(mac_only), cfg!(target_os = "macos"));
    }
    // Older reports without these fields still parse.
    assert!(serde_json::from_value::<Cask>(serde_json::json!({
        "full_token": "old", "name": [], "desc": null, "homepage": "",
        "version": "1", "installed": null, "outdated": false
    }))
    .is_ok());
}

#[test]
fn linux_casks_need_homebrew_6() {
    assert!(linux_casks_supported("Homebrew 7.0.6\n"));
    assert!(linux_casks_supported(
        "Homebrew 6.0.0-12-gabcdef\nHomebrew/core"
    ));
    assert!(!linux_casks_supported("Homebrew 5.1.2\n"));
    assert!(!linux_casks_supported("Homebrew 4.6.20\n"));
    assert!(!linux_casks_supported("brew: command not found\n"));
    assert!(!linux_casks_supported(""));
}

#[test]
fn upgrade_everything_lists_packages_where_there_is_no_single_command() {
    for id in [
        "pixi", "rustup", "nix", "aur", "mas", "conda", "fwupd", "codex",
    ] {
        assert!(per_package_upgrades(id), "{id}");
    }
    for id in ["apt", "homebrew", "npm", "mise"] {
        assert!(!per_package_upgrades(id), "{id}");
    }
}

#[test]
fn every_source_has_a_display_name() {
    for id in BACKEND_IDS {
        assert!(!display_name(id).is_empty());
    }
    assert_eq!(display_name("apt"), "APT");
    assert_eq!(display_name("homebrew-cask"), "Homebrew Casks");
    assert_eq!(display_name("npm"), "npm");
    assert_eq!(display_name("unknown-source"), "unknown-source");
    let renamed = BACKEND_IDS
        .iter()
        .filter(|id| display_name(id) != **id)
        .count();
    assert_eq!(renamed, BACKEND_IDS.len() - 9);
    assert_eq!(display_name("projects"), "Old projects");
}

#[test]
fn exact_lookups_skip_sources_that_cannot_have_the_name() {
    let transport = || NativeTransport {
        host: Host::current(),
        authorization: Authorization::Polkit,
    };
    let flatpak = Flatpak::new(transport());
    assert!(!flatpak.may_have("cowsay"));
    assert!(flatpak.may_have("org.videolan.VLC"));
    assert!(flatpak.may_have("app/org.videolan.VLC/x86_64/stable"));
    assert!(!flatpak.may_have("bad name.x"));
    let npm = DevTool::npm(transport());
    assert!(npm.may_have("@scope/tool"));
    assert!(!npm.may_have("org.example/App/x86_64"));
    let snap = SystemManager::snap(transport());
    assert!(snap.may_have("cowsay"));
    assert!(!snap.may_have("app/org.example.App"));
    assert!(AppImage::native().may_have("anything at all"));
}

#[test]
fn cache_keys_cover_relocations_and_locale() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    };
    assert_eq!(
        flatpak_installation(true, env(&[])),
        Some(PathBuf::from("/var/lib/flatpak"))
    );
    assert_eq!(
        flatpak_installation(false, env(&[("HOME", "/home/me")])),
        Some(PathBuf::from("/home/me/.local/share/flatpak"))
    );
    assert_eq!(
        flatpak_installation(
            false,
            env(&[("HOME", "/home/me"), ("XDG_DATA_HOME", "/data")])
        ),
        Some(PathBuf::from("/data/flatpak"))
    );
    assert_eq!(
        flatpak_installation(
            false,
            env(&[("HOME", "/home/me"), ("XDG_DATA_HOME", "relative")])
        ),
        Some(PathBuf::from("/home/me/.local/share/flatpak"))
    );
    assert_eq!(flatpak_installation(false, env(&[])), None);
    assert_eq!(
        flatpak_installation(true, env(&[("FLATPAK_SYSTEM_DIR", "/srv/flatpak")])),
        None
    );
    assert_eq!(
        flatpak_installation(false, env(&[("FLATPAK_USER_DIR", "/srv/mine")])),
        None
    );
    assert_eq!(
        locale_key(env(&[("LANG", "de_DE.UTF-8")])),
        ["", "", "", "de_DE.UTF-8"]
    );
    assert!(
        flatpak_search_watches(std::path::Path::new("/var/lib/flatpak"))
            .iter()
            .any(|watch| watch.path.ends_with("appstream"))
    );
    let host = Host::new(crate::host::Runtime::Native, Default::default());
    let watches = apt_watches(&host, None);
    for path in ["/var/lib/dpkg/status", "/etc/apt", "/etc/machine-id"] {
        assert!(watches
            .iter()
            .any(|watch| watch.path == std::path::Path::new(path)));
    }
    // A check's own copy of the lists changes the answers too.
    assert!(!watches.iter().any(|watch| watch.path.starts_with("/c")));
    let watches = apt_watches(&host, Some("/c/apt".into()));
    assert!(watches
        .iter()
        .any(|watch| watch.path == std::path::Path::new("/c/apt/lists")));
}

#[test]
fn the_apt_helper_hears_of_the_flatpak_and_of_a_fresher_copy() {
    assert!(apt_helper_env(false, None).is_empty());
    assert_eq!(
        apt_helper_env(true, Some("/c/apt/lists".into())),
        [
            ("PKGDECK_APT_HOST", "/run/host".to_owned()),
            ("PKGDECK_APT_LISTS", "/c/apt/lists".to_owned()),
        ]
    );
    assert_eq!(
        apt_helper_env(false, Some("/c/apt/lists".into())),
        [("PKGDECK_APT_LISTS", "/c/apt/lists".to_owned())]
    );
}

#[test]
fn apt_dist_upgrade_preview_classifies_actions() {
    let plan = parse_apt_upgrade_plan(b"Reading package lists...\n1 upgraded, 1 newly installed, 2 to remove and 0 not upgraded.\nInst old [1.0] (2.0 Ubuntu:stable [amd64])\nInst dependency (1.0 Ubuntu:stable [amd64])\nRemv retired [1.0]\nPurg obsolete [1.0]\nConf old (2.0 Ubuntu:stable [amd64])\n").unwrap();
    assert_eq!(plan.upgrades, ["old"]);
    assert_eq!(plan.installs, ["dependency"]);
    assert_eq!(plan.removals, ["retired", "obsolete"]);
    assert!(plan.summary().contains("Remove (2): retired, obsolete"));
    assert!(parse_apt_upgrade_plan(
        b"1 upgraded, 0 newly installed, 0 to remove and 0 not upgraded.\nInst\n"
    )
    .is_err());
    assert!(parse_apt_upgrade_plan(
        b"0 upgraded, 0 newly installed, 1 to remove and 0 not upgraded.\n"
    )
    .is_err());
    assert!(parse_apt_upgrade_plan(&[0xff]).is_err());
}

#[test]
fn apt_single_action_preview_keeps_exact_target_and_extra_removal() {
    let operation = Operation::Install(PackageId {
        backend: "apt".into(),
        name: "anonymous".into(),
        architecture: "amd64".into(),
        scope: Scope::System,
        remote: None,
        reference: None,
    });
    let plan = parse_apt_transaction_plan(&operation, b"0 upgraded, 2 newly installed, 1 to remove and 0 not upgraded.\nInst anonymous (2.0 Ubuntu:stable [amd64])\nInst dependency (1.0 Ubuntu:stable [amd64])\nRemv obsolete [1.0]\n").unwrap();
    assert_eq!(plan.operation, operation);
    assert_eq!(plan.changes.len(), 3);
    assert_eq!(plan.changes[0].action, PlannedAction::Install);
    assert_eq!(plan.changes[0].candidate_version.as_deref(), Some("2.0"));
    assert_eq!(plan.changes[2].action, PlannedAction::Remove);
    assert_eq!(plan.changes[2].installed_version.as_deref(), Some("1.0"));
    assert!(plan.download_bytes.is_none());
}

#[test]
fn apt_helper_supports_cargo_builds_and_relocated_bundles() {
    let root = std::env::temp_dir().join(format!("pkgdeck-apt-helper-{}", std::process::id()));
    std::fs::create_dir_all(root.join("bundle")).unwrap();
    let executable = root.join("bundle/pkgdeck");
    let built = root.join("cargo-helper");
    let adjacent = root.join("bundle/pkgdeck-apt-query");
    assert!(matches!(
        apt_query_executable(&executable, None),
        Err(ExecutionError::Disabled(message)) if message.contains("APT helper is missing")
    ));
    assert!(apt_query_executable(&executable, built.to_str()).is_err());
    std::fs::write(&built, "synthetic helper").unwrap();
    assert_eq!(
        apt_query_executable(&executable, built.to_str()).unwrap(),
        built
    );
    std::fs::write(&adjacent, "packaged helper").unwrap();
    assert_eq!(
        apt_query_executable(&executable, built.to_str()).unwrap(),
        adjacent
    );
    std::fs::remove_file(&built).unwrap();
    assert_eq!(
        apt_query_executable(&executable, built.to_str()).unwrap(),
        adjacent
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn python_php_ruby_and_node_names_accept_registry_forms_only() {
    for name in [
        "cowsay", "requests", "Pillow", "foo-bar", "foo_bar", "foo.bar", "a", "a1",
    ] {
        assert!(python_name(name), "{name}");
        assert!(gem_name(name), "{name}");
    }
    for name in [
        "",
        "-evil",
        "evil-",
        "evil_",
        "evil.",
        "../evil",
        "evil/thing",
        "https://example.invalid/tool.tar.gz",
        "name@latest",
        "name==1.0",
        "name;evil",
        "white space",
        "évil",
    ] {
        assert!(!python_name(name), "{name}");
        assert!(!gem_name(name), "{name}");
    }
    assert!(python_name(&"a".repeat(214)));
    assert!(!python_name(&"a".repeat(215)));
    for name in ["psr/log", "monolog/monolog", "vendor123/pkg-name_1.2"] {
        assert!(composer_name(name), "{name}");
    }
    for name in [
        "",
        "monolog",
        "/package",
        "vendor/",
        "Vendor/Package",
        "-vendor/package",
        "vendor//package",
        "vendor/pack age",
        "vendor/package/extra",
    ] {
        assert!(!composer_name(name), "{name}");
    }
    assert!(!dev_name(""));
    assert!(!dev_name("@scope"));
    assert!(!dev_name("@scope/tool/extra"));
    assert!(dev_name("@scope/tool"));
    assert!(!dev_name("."));
    assert!(!dev_name(".."));
}

#[test]
fn uv_and_gem_parsers_reject_malformed_lines() {
    assert_eq!(
        DevTool::<NativeTransport>::uv_entry("cowsay v6.0"),
        Some(("cowsay".into(), "6.0".into(), None))
    );
    assert_eq!(
        DevTool::<NativeTransport>::uv_entry("cowsay v6.0 [latest: 6.1]"),
        Some(("cowsay".into(), "6.0".into(), Some("6.1".into())))
    );
    for line in [
        "", "   ", "-", "- cowsay", "nodash", "name ", "name v", "name 6.0",
    ] {
        assert_eq!(DevTool::<NativeTransport>::uv_entry(line), None, "{line}");
    }
    assert_eq!(
        DevTool::<NativeTransport>::gem_filename("cowsay-0.3.0.gemspec"),
        Some(("cowsay".into(), "0.3.0".into()))
    );
    assert_eq!(
        DevTool::<NativeTransport>::gem_filename("foo-bar-1.0.gemspec"),
        Some(("foo-bar".into(), "1.0".into()))
    );
    for file in [
        "nodash.gemspec",
        "README",
        "name-.gemspec",
        "name-x.gemspec",
        "-1.0.gemspec",
    ] {
        assert_eq!(
            DevTool::<NativeTransport>::gem_filename(file),
            None,
            "{file}"
        );
    }
    assert_eq!(
        DevTool::<NativeTransport>::gem_outdated("cowsay (0.3.0 < 0.4.0)"),
        Some(("cowsay".into(), "0.4.0".into()))
    );
    assert_eq!(
        DevTool::<NativeTransport>::gem_outdated("rake (13.0.0, 13.3.0 < 13.4.2)"),
        Some(("rake".into(), "13.4.2".into()))
    );
    for line in [
        "",
        "cowsay",
        "cowsay (0.3.0)",
        "--evil (1.0 < 2.0)",
        "-x (1 < 2)",
    ] {
        assert_eq!(
            DevTool::<NativeTransport>::gem_outdated(line),
            None,
            "{line}"
        );
    }
    use std::cmp::Ordering::*;
    assert_eq!(
        DevTool::<NativeTransport>::gem_version_cmp("1.0", "1.0"),
        Equal
    );
    assert_eq!(
        DevTool::<NativeTransport>::gem_version_cmp("0.2.0", "0.3.0"),
        Less
    );
    assert_eq!(
        DevTool::<NativeTransport>::gem_version_cmp("13.4.2", "13.0.0"),
        Greater
    );
    assert_eq!(
        DevTool::<NativeTransport>::gem_version_cmp("1.0", "1.0.0"),
        Less
    );
    assert_eq!(
        DevTool::<NativeTransport>::gem_version_cmp("1.0.0", "1.0"),
        Greater
    );
    assert_eq!(
        DevTool::<NativeTransport>::gem_version_cmp("1.0.a", "1.0.b"),
        Less
    );
}

#[test]
fn component_stems_share_one_namespace() {
    assert_eq!(
        component_stem("firefox.desktop"),
        Some("firefox".to_string())
    );
    assert_eq!(
        component_stem("org.mozilla.firefox.desktop"),
        Some("org.mozilla.firefox".to_string())
    );
    assert_eq!(
        component_stem("org.mozilla.firefox"),
        Some("org.mozilla.firefox".to_string())
    );
    assert_eq!(component_stem("plain"), Some("plain".to_string()));
    assert_eq!(component_stem(""), None);
    assert_eq!(component_stem(".desktop"), None);
    assert_eq!(component_stem("   "), None);
}

#[test]
fn dep11_maps_packages_to_component_stems() {
    use std::io::Write;
    let base = std::env::temp_dir().join(format!("pkgdeck-dep11-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
            .write_all(
                b"---\nFile: DEP-11\nVersion: '1.0'\n---\nType: desktop-application\nID: org.mozilla.firefox.desktop\nPackage: firefox\n---\nType: desktop-application\nID: firefox.desktop\nPackage: firefox\n---\nType: desktop-application\nID: org.chromium.Chromium.desktop\nPackage: chromium\nDescription:\n  C: >-\n    ID: not-a-component\n---\nType: desktop-application\nID: .desktop\nPackage: emptyid\n",
            )
            .unwrap();
    std::fs::write(
        base.join("test_dep11_Components-amd64.yml.gz"),
        encoder.finish().unwrap(),
    )
    .unwrap();
    // Non-gzipped and corrupt files never contribute.
    std::fs::write(base.join("notes.txt"), "ID: fake.desktop\nPackage: fake\n").unwrap();
    std::fs::write(base.join("broken.yml.gz"), b"not gzip data").unwrap();
    std::os::unix::fs::symlink(base.join("absent"), base.join("dangling.yml.gz")).unwrap();
    let map = dep11_cached(None, &base, dep11_scan);
    assert_eq!(
        map.get("firefox"),
        Some(&vec![
            "firefox".to_string(),
            "org.mozilla.firefox".to_string()
        ])
    );
    assert_eq!(
        map.get("chromium"),
        Some(&vec!["org.chromium.Chromium".to_string()])
    );
    assert!(!map.contains_key("fake"));
    assert!(!map.contains_key("emptyid"));
    assert!(dep11_component_ids(&base.join("missing")).is_empty());
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn dep11_maps_are_cached_until_the_data_changes() {
    use std::io::Write;
    let base = std::env::temp_dir().join(format!(
        "pkgdeck-dep11-cache-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    let lists = base.join("lists");
    let yaml = base.join("yaml");
    std::fs::create_dir_all(&lists).unwrap();
    std::fs::create_dir_all(&yaml).unwrap();
    let gz = |body: &str| {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(body.as_bytes()).unwrap();
        encoder.finish().unwrap()
    };
    // Like AppStream's APT hook: the folder links into APT's lists.
    let main = lists.join("main_dep11_Components-amd64.yml.gz");
    std::fs::write(
        &main,
        gz("---\nID: org.example.Tool.desktop\nPackage: tool\n"),
    )
    .unwrap();
    std::os::unix::fs::symlink(&main, yaml.join("main.yml.gz")).unwrap();
    let store = crate::cache::Store::new(base.join("cache"));
    let scans = std::cell::Cell::new(0);
    let get = || {
        dep11_cached(Some(&store), &yaml, |files| {
            scans.set(scans.get() + 1);
            dep11_scan(files)
        })
    };
    let tool = |stems: &[&str]| {
        std::collections::BTreeMap::from([(
            "tool".to_string(),
            stems
                .iter()
                .map(|stem| stem.to_string())
                .collect::<Vec<_>>(),
        )])
    };
    assert_eq!(get(), tool(&["org.example.Tool"]));
    assert_eq!(get(), tool(&["org.example.Tool"]));
    assert_eq!(scans.get(), 1);
    // `apt update` rewrites the list behind the unchanged link.
    std::fs::write(
            &main,
            gz("---\nID: org.example.Tool.desktop\nPackage: tool\n---\nID: tool.desktop\nPackage: tool\n"),
        )
        .unwrap();
    assert_eq!(get(), tool(&["org.example.Tool", "tool"]));
    assert_eq!(scans.get(), 2);
    // A link whose list appears later counts as a change too.
    let universe = lists.join("universe_dep11_Components-amd64.yml.gz");
    std::os::unix::fs::symlink(&universe, yaml.join("universe.yml.gz")).unwrap();
    assert_eq!(get(), tool(&["org.example.Tool", "tool"]));
    assert_eq!(get(), tool(&["org.example.Tool", "tool"]));
    assert_eq!(scans.get(), 3);
    std::fs::write(
        &universe,
        gz("---\nID: org.example.Other\nPackage: other\n"),
    )
    .unwrap();
    assert_eq!(get()["other"], vec!["org.example.Other".to_string()]);
    assert_eq!(scans.get(), 4);
    // Without a store (root or PKGDECK_NO_CACHE) every call scans.
    dep11_cached(None, &yaml, |files| {
        scans.set(scans.get() + 1);
        dep11_scan(files)
    });
    assert_eq!(scans.get(), 5);
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn snap_desktop_ids_list_gui_entries() {
    let base = std::env::temp_dir().join(format!("pkgdeck-snapids-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let gui = base.join("snap/spot/current/meta/gui");
    std::fs::create_dir_all(&gui).unwrap();
    std::fs::write(gui.join("spot.desktop"), "[Desktop Entry]\n").unwrap();
    std::fs::write(gui.join("spot-settings.desktop"), "[Desktop Entry]\n").unwrap();
    std::fs::write(gui.join("icon.png"), "png").unwrap();
    assert_eq!(
        snap_desktop_ids(&base, "spot"),
        vec!["spot".to_string(), "spot-settings".to_string()]
    );
    assert!(snap_desktop_ids(&base, "missing").is_empty());
    assert!(snap_desktop_ids(&base, "../evil").is_empty());
    assert!(snap_desktop_ids(&base, "").is_empty());
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn local_icons_resolve_without_network_access() {
    let base = std::env::temp_dir().join(format!("pkgdeck-icons-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    // Snap layout: <sysroot>/snap/<name>/current/meta/gui/icon.<ext>.
    let gui = base.join("snap/hello/current/meta/gui");
    std::fs::create_dir_all(&gui).unwrap();
    std::fs::write(gui.join("icon.svg"), "<svg/>").unwrap();
    assert_eq!(snap_icon(&base, "hello"), Some(gui.join("icon.svg")));
    assert_eq!(snap_icon(&base, "missing"), None);
    assert_eq!(snap_icon(&base, "../evil"), None);
    assert_eq!(snap_icon(&base, ""), None);
    // Flatpak layout: <exports>/share/icons/hicolor/<size>/apps/<id>.<ext>.
    let exports = base.join("exports");
    let apps = exports.join("share/icons/hicolor/64x64/apps");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::write(apps.join("org.example.App.png"), "png").unwrap();
    assert_eq!(
        flatpak_icon(std::slice::from_ref(&exports), "org.example.App"),
        Some(apps.join("org.example.App.png"))
    );
    assert_eq!(
        flatpak_icon(std::slice::from_ref(&exports), "missing.app"),
        None
    );
    assert_eq!(flatpak_icon(&[exports], "--evil"), None);
    assert_eq!(flatpak_icon(&[], "org.example.App"), None);
    // Debian layout: <info>/<pkg>[_<arch>].list naming desktop entries.
    let info = base.join("info");
    std::fs::create_dir_all(&info).unwrap();
    std::fs::write(
        info.join("brave-browser.list"),
        "/usr/bin/brave\n/usr/share/applications/brave-browser.desktop\n",
    )
    .unwrap();
    std::fs::write(info.join("plain.list"), "/usr/bin/plain\n").unwrap();
    // An unreadable list is skipped.
    std::os::unix::fs::symlink(info.join("absent"), info.join("ghost.list")).unwrap();
    // Non-list files never name a package, and duplicate entries for
    // one package resolve exactly once.
    std::fs::write(info.join("README"), "inventory\n").unwrap();
    std::fs::write(
        info.join("brave-browser:amd64.list"),
        "/usr/bin/brave\n/usr/share/applications/other.desktop\n",
    )
    .unwrap();
    let map = apt_desktop_map(&info);
    // Either list may win the readdir order, but duplicates resolve once.
    let brave = map.get("brave-browser").unwrap();
    assert!(
        brave.ends_with("brave-browser.desktop") || brave.ends_with("other.desktop"),
        "{brave:?}"
    );
    assert_eq!(
        map.values()
            .filter(|p| { p.ends_with("brave-browser.desktop") || p.ends_with("other.desktop") })
            .count(),
        1
    );
    assert!(!map.contains_key("plain"));
    assert!(!map.contains_key("README"));
    assert!(!map.contains_key("ghost"));
    assert_eq!(
        map.values()
            .filter(|p| p.ends_with("brave-browser.desktop"))
            .count()
            + map
                .values()
                .filter(|p| p.ends_with("other.desktop"))
                .count(),
        1
    );
    // A bare Icon= name resolves through the theme; absolute paths
    // resolve directly when the file exists.
    let real = base.join("real-icon.png");
    std::fs::write(&real, "png").unwrap();
    let desktop = base.join("myapp.desktop");
    std::fs::write(
        &desktop,
        format!("[Desktop Entry]\nName=Mine\nIcon={}\n", real.display()),
    )
    .unwrap();
    assert_eq!(desktop_icon(None, &desktop), Some(real));
    std::fs::write(&desktop, "[Desktop Entry]\nName=Mine\n").unwrap();
    assert_eq!(desktop_icon(None, &desktop), None);
    std::fs::write(&desktop, "[Desktop Entry]\nName=Mine\nIcon=\n").unwrap();
    assert_eq!(desktop_icon(None, &desktop), None);
    std::fs::write(
        &desktop,
        "[Desktop Entry]\nName=Mine\nIcon=/nowhere/icon.png\n",
    )
    .unwrap();
    assert_eq!(desktop_icon(None, &desktop), None);
    // Relative names with a slash never touch the filesystem.
    std::fs::write(&desktop, "[Desktop Entry]\nName=Mine\nIcon=subdir/icon\n").unwrap();
    assert_eq!(desktop_icon(None, &desktop), None);
    // Unique names miss every theme directory including pixmaps.
    std::fs::write(
        &desktop,
        "[Desktop Entry]\nName=Mine\nIcon=pkgdeck-definitely-missing-icon\n",
    )
    .unwrap();
    assert_eq!(desktop_icon(None, &desktop), None);
    // The user theme directory wins over the system ones.
    let home = base.join("home");
    let theme = home.join(".local/share/icons/hicolor/48x48/apps");
    std::fs::create_dir_all(&theme).unwrap();
    std::fs::write(theme.join("pkgdeck-fixture-icon.png"), "png").unwrap();
    std::fs::write(
        &desktop,
        "[Desktop Entry]\nName=Mine\nIcon=pkgdeck-fixture-icon\n",
    )
    .unwrap();
    assert_eq!(
        desktop_icon(Some(&home), &desktop),
        Some(theme.join("pkgdeck-fixture-icon.png"))
    );
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn update_output_names_the_package_each_manager_is_on() {
    for (backend, line, name) in [
        ("homebrew", "==> Upgrading openssl@3", Some("openssl@3")),
        (
            "homebrew-cask",
            "==> Upgrading astrovm/pkgdeck/pkgdeck",
            Some("pkgdeck"),
        ),
        ("homebrew-cask", "==> Upgrading 3 outdated packages:", None),
        ("homebrew", "  3.6.4 -> 3.6.4_1", None),
        (
            "apt",
            "Unpacking curl:amd64 (8.5.0-2) over (8.4.0-1) ...",
            Some("curl"),
        ),
        ("apt", "Setting up curl:amd64 (8.5.0-2) ...", None),
        (
            "dnf",
            "  Upgrading        : htop-3.3.0-1.fc41.x86_64        1/2",
            Some("htop"),
        ),
        (
            "dnf",
            "[1/4] Upgrading htop-0:3.3.0-1.fc41.x86_64 100% |",
            Some("htop"),
        ),
        ("dnf", "  Upgrading        :", None),
        ("dnf", "Upgrading:", None),
        (
            "zypper",
            "(1/2) Installing: vim-9.1.0-1.1.x86_64 [...done]",
            Some("vim"),
        ),
        ("zypper", "(1/2) Installing: broken [...done]", None),
        (
            "pacman",
            "(1/3) upgrading linux-firmware          [####]",
            Some("linux-firmware"),
        ),
        (
            "apk",
            "(2/5) Upgrading musl (1.2.4-r2 -> 1.2.5-r0)",
            Some("musl"),
        ),
        ("xbps", "htop-3.3.0_1: unpacking ...", Some("htop")),
        ("xbps", "nodash: unpacking ...", None),
        ("macports", "--->  Installing wget @1.24.5_0", Some("wget")),
        (
            "flatpak",
            "Updating app/org.gnome.Maps/x86_64/stable",
            Some("org.gnome.Maps"),
        ),
        (
            "flatpak",
            "Updating org.freedesktop.Platform",
            Some("org.freedesktop.Platform"),
        ),
        (
            "snap",
            "firefox 131.0 from Mozilla✓ refreshed",
            Some("firefox"),
        ),
        ("snap", "All snaps up to date.", None),
        (
            "pipx",
            "upgraded package black from 24.1 to 24.2 (location: ...)",
            Some("black"),
        ),
        ("gem", "Updating rake", Some("rake")),
        ("gem", "Updating installed gems", None),
        (
            "mas",
            "==> Downloading Final Cut Pro (11.0)",
            Some("Final Cut Pro"),
        ),
        ("mas", "==> Downloading Xcode", Some("Xcode")),
        ("npm", "changed 3 packages in 2s", None),
    ] {
        assert_eq!(
            output_package(backend, line).as_deref(),
            name,
            "{backend}: {line}"
        );
    }
    assert_eq!(output_package("flatpak", "Updating "), None);
    assert_eq!(output_package("mas", "==> Downloading  "), None);
}

const CODE_APP: &str = "/Applications/Visual Studio Code.app";
const CODE_COMMAND: &str = "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code";

/// `brew` answering for a few casks; any other name is no cask.
#[derive(Clone, Default)]
struct CaskBrew {
    version: &'static str,
    installed: Arc<Mutex<bool>>,
    calls: Arc<Mutex<Vec<String>>>,
}
impl Transport for CaskBrew {
    fn brew(
        &self,
        args: &[OsString],
        _: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        let line = args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        self.calls.lock().unwrap().push(line.clone());
        let installed = (*self.installed.lock().unwrap()).then_some("1.139.1");
        let cask = |token: &str, artifacts: serde_json::Value| {
            serde_json::json!({"casks": [{
                "full_token": token, "name": [token], "desc": null,
                "homepage": "https://example.com", "version": "1.139.1",
                "installed": installed, "outdated": false, "auto_updates": true,
                "artifacts": artifacts,
            }]})
            .to_string()
        };
        let stdout = match line.as_str() {
            "--prefix" => "/home/linuxbrew/.linuxbrew\n".into(),
            "--version" => format!("Homebrew {}\n", self.version),
            "casks" => "codex\ncodex-cli\nvisual-studio-code\n".into(),
            "info --json=v2 --cask -- visual-studio-code" => cask(
                "visual-studio-code",
                serde_json::json!([
                    {"app": ["Visual Studio Code.app"], "target": CODE_APP},
                    {"binary": [CODE_COMMAND], "target": "/home/linuxbrew/.linuxbrew/bin/code"},
                ]),
            ),
            "info --json=v2 --cask -- codex" => {
                cask("codex", serde_json::json!([{"binary": ["codex"]}]))
            }
            // Homebrew 7.0.9 on Linux: no version for a Mac-only cask.
            "info --json=v2 --cask -- discord" => serde_json::json!({"casks": [{
                "full_token": "discord", "name": ["Discord"], "desc": null,
                "homepage": "https://discord.com/", "version": null,
                "installed": null, "outdated": false, "depends_on": {"macos": {}},
                "artifacts": [{"app": ["Discord.app"]}],
            }]})
            .to_string(),
            "info --json=v2 --cask -- codex-cli" => {
                cask("codex-cli", serde_json::json!([{"binary": ["codex"]}]))
            }
            "info --json=v2 --cask -- codex codex-cli" => {
                let mut both: serde_json::Value = serde_json::from_str(&cask(
                    "codex",
                    serde_json::json!([{"binary": ["codex"]}]),
                ))
                .unwrap();
                let other: serde_json::Value = serde_json::from_str(&cask(
                    "codex-cli",
                    serde_json::json!([{"binary": ["codex"]}]),
                ))
                .unwrap();
                both["casks"]
                    .as_array_mut()
                    .unwrap()
                    .push(other["casks"][0].clone());
                both.to_string()
            }
            "upgrade --cask" => {
                assert!(write);
                String::new()
            }
            "install --cask --adopt -- visual-studio-code" => {
                assert!(write);
                *self.installed.lock().unwrap() = true;
                String::new()
            }
            // Homebrew 7.0.11 refuses a cask from a tap no one trusted yet.
            "info --json=v2 --cask -- acme/tools/widget" => {
                return Err(ExecutionError::Failed(Completion {
                    code: Some(1),
                    signal: None,
                    stdout: vec![],
                    stderr: b"Error: Refusing to load cask acme/tools/widget from untrusted tap acme/tools.\nRun `brew trust --cask acme/tools/widget` or `brew trust acme/tools` to trust it.\n".to_vec(),
                    truncated: false,
                    cancellation_deferred: false,
                }))
            }
            _ => {
                return Err(ExecutionError::Failed(Completion {
                    code: Some(1),
                    signal: None,
                    stdout: vec![],
                    stderr: b"Error: No available cask".to_vec(),
                    truncated: false,
                    cancellation_deferred: false,
                }))
            }
        };
        Ok(Completion {
            code: Some(0),
            signal: None,
            stdout: stdout.into_bytes(),
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
        })
    }
}

/// A signed copy of the app already in place, with the cask's command
/// already linked where Homebrew would link it.
/// The second field makes the app vanish once it is backed up, as when
/// Homebrew fails halfway through adopting it.
struct Adoptable(Arc<Mutex<Vec<&'static str>>>, bool);
impl adopt::AdoptIo for Adoptable {
    fn occupied(&self, path: &Path) -> bool {
        path == Path::new(CODE_APP) || path.ends_with("bin/code")
    }
    fn is_folder(&self, path: &Path) -> bool {
        path == Path::new(CODE_APP) && !(self.1 && self.0.lock().unwrap().contains(&"backup"))
    }
    fn is_file(&self, path: &Path) -> bool {
        path == Path::new(CODE_COMMAND)
    }
    fn links_to(&self, link: &Path, source: &Path) -> bool {
        link.starts_with("/home/linuxbrew") && source == Path::new(CODE_COMMAND)
    }
    fn plist(&self, _: &Path, _: &Cancellation) -> Result<serde_json::Value, EngineError> {
        Ok(serde_json::json!({
            "CFBundleIdentifier": "com.microsoft.VSCode",
            "CFBundleExecutable": "Electron",
            "CFBundleShortVersionString": "1.139.1",
        }))
    }
    fn team(&self, _: &Path, _: &Cancellation) -> Result<Option<String>, EngineError> {
        Ok(Some("UBF8T346G9".into()))
    }
    fn architectures(&self, _: &Path, _: &Cancellation) -> Result<Vec<String>, EngineError> {
        Ok(vec!["x86_64".into(), "arm64".into()])
    }
    fn backup(&self, app: &Path, _: &Cancellation) -> Result<PathBuf, EngineError> {
        self.0.lock().unwrap().push("backup");
        Ok(Path::new("/backups/1").join(app.file_name().unwrap()))
    }
    fn restore(&self, _: &Path, _: &Path, _: &Cancellation) -> Result<(), EngineError> {
        self.0.lock().unwrap().push("restore");
        Ok(())
    }
    fn discard(&self, _: &Path) {
        self.0.lock().unwrap().push("discard");
    }
}

#[test]
fn cask_search_reads_spaces_as_the_dashes_in_tokens() {
    let mut casks = HomebrewCask::new(CaskBrew {
        version: "7.0.6",
        ..CaskBrew::default()
    });
    let cancel = Cancellation::default();
    assert_eq!(casks.detect(&cancel).unwrap(), Availability::Available);
    // A binary-only cask, so Linux searches offer it too.
    for query in ["codex cli", "Codex_CLI", "codex-cli"] {
        let found = casks.search(query, &cancel).unwrap();
        assert_eq!(
            found.iter().map(|p| p.id.name.as_str()).collect::<Vec<_>>(),
            ["codex-cli"],
            "{query}"
        );
    }
    assert!(casks.search("codex x", &cancel).unwrap().is_empty());
}

#[test]
fn one_cask_brew_cannot_load_does_not_hide_the_others() {
    let failed = || {
        EngineError::Execution(ExecutionError::Failed(Completion {
            code: Some(1),
            signal: None,
            stdout: vec![],
            stderr: b"Error: key not found: :screen_saverdir".to_vec(),
            truncated: false,
            cancellation_deferred: false,
        }))
    };
    let package = |name: &str| Package {
        id: PackageId {
            backend: "homebrew-cask".into(),
            name: name.into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        },
        display_name: name.into(),
        summary: String::new(),
        installed_version: None,
        candidate_version: Some("1".into()),
        update: UpdateAvailability::Unknown,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
        adopt_with: None,
    };
    let names = ["a", "b", "clocksaver", "d", "e", "grid-clock", "g"];
    // Like brew: any broken name fails the whole call.
    let mut calls = 0;
    let mut read = |chunk: &[&str]| {
        calls += 1;
        if chunk.iter().any(|name| name.contains("clock")) {
            Err(failed())
        } else {
            Ok(chunk.iter().map(|name| package(name)).collect())
        }
    };
    let found: Vec<_> = read_around_broken(&names, &mut read)
        .unwrap()
        .into_iter()
        .map(|package| package.id.name)
        .collect();
    assert_eq!(found, ["a", "b", "d", "e", "g"]);
    assert!(calls < 2 * names.len());
    // A Homebrew that can't read anything still reports its failure,
    // and so does one that stopped for another reason.
    let mut broken = |_: &[&str]| -> Result<Vec<Package>, EngineError> { Err(failed()) };
    assert!(matches!(
        read_around_broken(&names, &mut broken),
        Err(EngineError::Execution(ExecutionError::Failed(_)))
    ));
    let mut cancelled = |chunk: &[&str]| {
        if chunk.len() == names.len() {
            Err(failed())
        } else {
            Err(EngineError::Cancelled)
        }
    };
    assert!(matches!(
        read_around_broken(&names, &mut cancelled),
        Err(EngineError::Cancelled)
    ));
    // One name that fails alone is that name's failure.
    let mut one = |_: &[&str]| -> Result<Vec<Package>, EngineError> { Err(failed()) };
    assert!(read_around_broken(&["clocksaver"], &mut one).is_err());
    // Too many broken names to single out: the batch's failure stands,
    // after a bounded number of tries.
    let many: Vec<String> = (0..300).map(|i| format!("cask-{i}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let mut calls = 0;
    let mut mostly_broken = |chunk: &[&str]| {
        calls += 1;
        if chunk.iter().any(|name| name.ends_with('0')) {
            Err(failed())
        } else {
            Ok(chunk.iter().map(|name| package(name)).collect())
        }
    };
    assert!(matches!(
        read_around_broken(&many, &mut mostly_broken),
        Err(EngineError::Execution(ExecutionError::Failed(_)))
    ));
    assert!(calls <= BREW_INFO_RETRIES + 3, "{calls} calls");
}

#[test]
fn narrower_cask_searches_reuse_what_brew_already_read() {
    let brew = CaskBrew {
        version: "7.0.6",
        ..CaskBrew::default()
    };
    let mut casks = HomebrewCask::new(brew.clone());
    let cancel = Cancellation::default();
    casks.detect(&cancel).unwrap();
    let names = |found: Vec<Package>| found.into_iter().map(|p| p.id.name).collect::<Vec<_>>();
    let reads = || {
        brew.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.starts_with("info "))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(casks.search("codex", &cancel).unwrap()),
        ["codex", "codex-cli"]
    );
    assert_eq!(reads(), ["info --json=v2 --cask -- codex codex-cli"]);
    // One more letter: answered without asking brew again.
    assert_eq!(
        names(casks.search("codex-c", &cancel).unwrap()),
        ["codex-cli"]
    );
    assert_eq!(reads().len(), 1);
    // A change may install or update any of them, so they're read again.
    let _ = casks.execute(
        &Operation::Refresh {
            backend: "homebrew-cask".into(),
        },
        &cancel,
        &mut |_| {},
    );
    assert_eq!(
        names(casks.search("codex-c", &cancel).unwrap()),
        ["codex-cli"]
    );
    assert_eq!(
        reads().last().unwrap(),
        "info --json=v2 --cask -- codex-cli"
    );
}

/// [`CaskBrew`] on a Homebrew laid out in `root`, with a cache there.
#[derive(Clone)]
struct KeptBrew {
    brew: CaskBrew,
    root: PathBuf,
    /// Someone installs a formula while brew reads.
    busy: Arc<Mutex<bool>>,
}
impl Transport for KeptBrew {
    fn brew(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        if *self.busy.lock().unwrap() {
            let keg = self.root.join("prefix/Cellar/busy");
            std::fs::create_dir_all(&keg).unwrap();
            let calls = self.brew.calls.lock().unwrap().len();
            std::fs::write(keg.join(calls.to_string()), "").unwrap();
        }
        self.brew.brew(args, cancel, write)
    }
    fn brew_cache(&self) -> Option<(crate::cache::Store, Vec<crate::cache::Watch>)> {
        let host = Host::new(
            crate::host::Runtime::Native,
            [
                ("HOME".into(), self.root.clone().into_os_string()),
                ("HOMEBREW_CACHE".into(), self.root.join("cache").into()),
            ]
            .into(),
        );
        Some((
            crate::cache::Store::new(self.root.join("store")),
            host.brew_details_watches(
                &self.root.join("prefix/Homebrew/bin/brew"),
                &self.root.join("cache"),
            )
            .unwrap(),
        ))
    }
}

#[test]
fn cask_details_are_kept_between_runs_until_homebrew_changes() {
    let root = std::env::temp_dir().join(format!("pkgdeck-brew-kept-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("cache/api")).unwrap();
    let brew = KeptBrew {
        brew: CaskBrew {
            version: "7.0.6",
            ..CaskBrew::default()
        },
        root,
        busy: Arc::default(),
    };
    let cancel = Cancellation::default();
    let names = |found: Vec<Package>| found.into_iter().map(|p| p.id.name).collect::<Vec<_>>();
    // Each run is a new process, so a new backend.
    let run = |query: &str| {
        let mut casks = HomebrewCask::new(brew.clone());
        casks.detect(&cancel).unwrap();
        names(casks.search(query, &cancel).unwrap())
    };
    let reads = || {
        brew.brew
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.starts_with("info "))
            .count()
    };
    assert_eq!(run("codex"), ["codex", "codex-cli"]);
    assert_eq!(reads(), 1);
    // The next run answers from disk, narrower searches too.
    assert_eq!(run("codex"), ["codex", "codex-cli"]);
    assert_eq!(run("codex-c"), ["codex-cli"]);
    assert_eq!(reads(), 1);
    // New Homebrew data, installs and links made outside PkgDeck, taps,
    // or a Homebrew update: brew reads again, once.
    for change in [
        "cache/api/internal/packages.jws.json",
        "prefix/Caskroom/codex/1.0",
        "prefix/Cellar/git/2.0",
        "prefix/var/homebrew/linked/git",
        "prefix/Homebrew/Library/Taps/someone/homebrew-tap/Casks/tool.rb",
        "prefix/Homebrew/.git/HEAD",
    ] {
        let before = reads();
        let file = brew.root.join(change);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "changed").unwrap();
        assert_eq!(run("codex"), ["codex", "codex-cli"]);
        assert_eq!(run("codex"), ["codex", "codex-cli"]);
        assert_eq!(reads(), before + 1, "{change}");
    }
    // A running app notices an outside install too.
    let mut casks = HomebrewCask::new(brew.clone());
    casks.detect(&cancel).unwrap();
    assert_eq!(names(casks.search("codex", &cancel).unwrap()).len(), 2);
    std::fs::create_dir_all(brew.root.join("prefix/Caskroom/codex-cli/1.0")).unwrap();
    assert_eq!(names(casks.search("codex", &cancel).unwrap()).len(), 2);
    assert_eq!(reads(), 8);
    // Something installed while brew read: nothing is kept.
    *brew.busy.lock().unwrap() = true;
    assert_eq!(run("codex-cli"), ["codex-cli"]);
    *brew.busy.lock().unwrap() = false;
    assert_eq!(run("codex"), ["codex", "codex-cli"]);
    assert_eq!(run("codex"), ["codex", "codex-cli"]);
    assert_eq!(reads(), 10);
    std::fs::remove_dir_all(&brew.root).unwrap();
}

#[test]
fn kept_details_belong_to_one_architecture_and_os_release() {
    let root = std::env::temp_dir().join(format!("pkgdeck-brew-system-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let disk = |system: [&str; 2]| BrewDisk {
        store: crate::cache::Store::new(root.join("store")),
        watches: vec![crate::cache::Watch::tree(root.join("Cellar"), 2)],
        system: system.map(String::from),
    };
    let here = disk(["aarch64", "25.0.0"]);
    let key = here.fingerprint().unwrap();
    here.save("homebrew", &key, &[("git".to_owned(), None)].into());
    assert!(here.load("homebrew", &key).is_some());
    // Another build sharing the cache, or the same Mac after an upgrade.
    for other in [disk(["x86_64", "25.0.0"]), disk(["aarch64", "26.0.0"])] {
        let other_key = other.fingerprint().unwrap();
        assert_ne!(other_key, key);
        assert!(other.load("homebrew", &other_key).is_none());
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_cask_install_adopts_the_copy_already_in_place() {
    let brew = CaskBrew {
        version: "7.0.6",
        ..CaskBrew::default()
    };
    let log = Arc::new(Mutex::new(vec![]));
    let mut casks = HomebrewCask::new(brew.clone());
    casks.adoption = Some(Box::new(Adoptable(log.clone(), false)));
    let cancel = Cancellation::default();
    assert_eq!(casks.detect(&cancel).unwrap(), Availability::Available);
    let id = PackageId {
        backend: "homebrew-cask".into(),
        name: "visual-studio-code".into(),
        architecture: std::env::consts::ARCH.into(),
        scope: Scope::Environment {
            path: "/home/linuxbrew/.linuxbrew".into(),
        },
        remote: None,
        reference: None,
    };
    let mut messages = vec![];
    casks
        .execute(&Operation::Install(id.clone()), &cancel, &mut |progress| {
            if let Progress::Message(message) = progress {
                messages.push(message);
            }
        })
        .unwrap();
    assert!(brew
        .calls
        .lock()
        .unwrap()
        .contains(&"install --cask --adopt -- visual-studio-code".into()));
    assert_eq!(*log.lock().unwrap(), vec!["backup", "discard"]);
    // The preview says the copy in place is adopted, with what was checked.
    let plan = casks
        .operation_plan(&Operation::Install(id.clone()), &cancel)
        .unwrap()
        .unwrap();
    assert!(
        plan.native_preview
            .starts_with("Visual Studio Code is already in /Applications (version"),
        "{}",
        plan.native_preview
    );
    assert!(plan.native_preview.contains("brew install --cask --adopt"));
    assert_eq!(
        plan.adopts.as_deref(),
        Some(std::path::Path::new("/Applications/Visual Studio Code.app"))
    );
    // Without an app in place there is no special preview.
    let mut plain = HomebrewCask::new(brew.clone());
    plain.detect(&cancel).unwrap();
    assert!(plain
        .operation_plan(&Operation::Install(id.clone()), &cancel)
        .unwrap()
        .is_none());
    assert_eq!(
        messages.last().unwrap(),
        "Homebrew now manages Visual Studio Code."
    );
}

#[test]
fn a_failed_cask_adoption_puts_the_app_back() {
    let brew = CaskBrew {
        version: "7.0.6",
        ..CaskBrew::default()
    };
    let log = Arc::new(Mutex::new(vec![]));
    let mut casks = HomebrewCask::new(brew.clone());
    casks.adoption = Some(Box::new(Adoptable(log.clone(), true)));
    let cancel = Cancellation::default();
    casks.detect(&cancel).unwrap();
    let id = |name: &str| PackageId {
        backend: "homebrew-cask".into(),
        name: name.into(),
        architecture: std::env::consts::ARCH.into(),
        scope: Scope::Environment {
            path: "/home/linuxbrew/.linuxbrew".into(),
        },
        remote: None,
        reference: None,
    };
    let error = casks
        .execute(
            &Operation::Install(id("visual-studio-code")),
            &cancel,
            &mut |_| {},
        )
        .unwrap_err();
    assert!(error.to_string().contains("PkgDeck put it back"), "{error}");
    assert_eq!(*log.lock().unwrap(), vec!["backup", "restore", "discard"]);
    // A cask brew doesn't know fails before anything is touched.
    assert!(matches!(
        casks.operation_plan(&Operation::Install(id("unknown-cask")), &cancel),
        Err(EngineError::Execution(ExecutionError::Failed(_)))
    ));
    assert_eq!(log.lock().unwrap().len(), 3);
    // So does a lookup brew answers with a failing exit status.
    struct Refusing;
    impl Transport for Refusing {
        fn brew(
            &self,
            _: &[OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            Ok(Completion {
                code: Some(1),
                signal: None,
                stdout: vec![],
                stderr: vec![],
                truncated: false,
                cancellation_deferred: false,
            })
        }
    }
    let mut refusing = HomebrewCask::new(Refusing);
    refusing.adoption = Some(Box::new(Adoptable(log.clone(), false)));
    refusing.prefix = Some("/opt/homebrew".into());
    let mut code = id("visual-studio-code");
    code.scope = Scope::Environment {
        path: "/opt/homebrew".into(),
    };
    assert!(matches!(
        refusing.operation_plan(&Operation::Install(code), &cancel),
        Err(EngineError::Execution(ExecutionError::Failed(result))) if result.code == Some(1)
    ));
    assert_eq!(log.lock().unwrap().len(), 3);
}

#[test]
fn exact_cask_names_are_looked_up_directly() {
    let brew = CaskBrew {
        version: "7.0.6",
        ..CaskBrew::default()
    };
    let mut casks = HomebrewCask::new(brew.clone());
    let cancel = Cancellation::default();
    casks.detect(&cancel).unwrap();
    let found = casks.lookup("codex", &cancel).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id.name, "codex");
    // A cask with no version here reads as one, not as a broken report.
    let discord = casks.lookup("discord", &cancel).unwrap();
    if cfg!(target_os = "macos") {
        assert_eq!(discord.len(), 1);
        assert_eq!(discord[0].candidate_version, None);
    } else {
        assert!(discord.is_empty());
    }
    // Not a cask, or not a cask name at all: nothing, without an error.
    assert!(casks.lookup("no-such-cask", &cancel).unwrap().is_empty());
    // A cask from an untrusted tap is there, but Homebrew won't read it:
    // say how to trust the tap instead of calling it missing.
    match casks.lookup("acme/tools/widget", &cancel) {
        Err(EngineError::Refused {
            backend,
            refusal: Refusal::Explained(reason),
        }) => {
            assert_eq!(backend, "homebrew-cask");
            assert_eq!(
                reason,
                "Homebrew doesn't trust the tap acme/tools yet. If you trust it, run `brew trust --tap acme/tools`, then try again."
            );
        }
        other => panic!("expected an untrusted tap refusal, got {other:?}"),
    }
    let calls = brew.calls.lock().unwrap().len();
    assert!(casks.lookup("--help", &cancel).unwrap().is_empty());
    assert_eq!(brew.calls.lock().unwrap().len(), calls);
    casks
        .execute(
            &Operation::UpgradeAll {
                backend: "homebrew-cask".into(),
            },
            &cancel,
            &mut |_| {},
        )
        .unwrap();
    assert!(brew
        .calls
        .lock()
        .unwrap()
        .contains(&"upgrade --cask".into()));
}

#[test]
fn linux_casks_wait_for_homebrew_6() {
    let mut casks = HomebrewCask::new(CaskBrew {
        version: "5.1.2",
        ..CaskBrew::default()
    });
    let availability = casks.detect(&Cancellation::default()).unwrap();
    if cfg!(target_os = "macos") {
        assert_eq!(availability, Availability::Available);
    } else {
        assert_eq!(
            availability,
            Availability::Unavailable("Homebrew Casks on Linux need Homebrew 6.0 or later".into())
        );
    }
}

#[test]
fn old_snap_revisions_are_listed_and_removed_by_the_approved_commands() {
    const LISTING: &str =
        "Name                 Version     Rev    Tracking         Publisher     Notes
core22               20260824    2955   latest/stable    canonical**   base,disabled
core22               20260901    3001   latest/stable    canonical**   base
firefox              158.0.1-1   9113   latest/stable    mozilla**     disabled
firefox              159.0-1     9200   latest/stable    mozilla**     -
adventure-mods       1.3.0       x2     -                -             disabled
cut-short            1.0
";
    let cancel = Cancellation::default();
    let kit = Kit::new(Ok(false), LISTING);
    let mut snap = SystemManager::snap(kit.clone());
    assert!(snap.capabilities().contains(&Capability::Clean));
    assert!(!SystemManager::dnf(Kit::new(Ok(false), ""))
        .capabilities()
        .contains(&Capability::Clean));
    let items = snap.cleanup(&cancel).unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(
        item.id.key,
        "disabled-revisions:adventure-mods=x2,core22=2955,firefox=9113"
    );
    assert_eq!(item.kind, CleanupKind::Unused);
    assert_eq!(item.summary, "3 older versions kept after updates");
    assert!(item
        .preview
        .ends_with("\nadventure-mods 1.3.0 (revision x2)\ncore22 20260824 (revision 2955)\nfirefox 158.0.1-1 (revision 9113)"));
    assert!(!item.automatic(), "it needs a password");
    snap.execute(&Operation::Clean(item.id.clone()), &cancel, &mut |_| {})
        .unwrap();
    let ran: Vec<String> = kit
        .ran()
        .into_iter()
        .filter(|line| line.starts_with("root "))
        .collect();
    let approved: Vec<String> =
        crate::batch::protected_commands(&Operation::Clean(item.id.clone()))
            .unwrap()
            .iter()
            .map(|command| format!("root snap {}", command.args.join(" ")))
            .collect();
    assert_eq!(ran, approved);
    assert_eq!(ran[0], "root snap remove adventure-mods --revision=x2");
    // Nothing disabled, or a name Snap wouldn't make, is no task.
    let current = Kit::new(Ok(false), "Name Version Rev Tracking Publisher Notes\nfirefox 159.0-1 9200 latest/stable mozilla** -\n");
    assert!(SystemManager::snap(current)
        .cleanup(&cancel)
        .unwrap()
        .is_empty());
    let odd = Kit::new(
        Ok(false),
        "Name Version Rev Tracking Publisher Notes\nBad;Name 1 2 - - disabled\n",
    );
    assert!(SystemManager::snap(odd)
        .cleanup(&cancel)
        .unwrap()
        .is_empty());
    assert!(SystemManager::dnf(Kit::new(Ok(false), ""))
        .cleanup(&cancel)
        .is_err());
    for key in [
        "disabled-revisions:",
        "disabled-revisions:a=1;rm",
        "disabled-revisions:-a=1",
        "disabled-revisions:a=x",
        "disabled-revisions:a",
        "other",
    ] {
        assert_eq!(snap_revisions(key), None, "{key}");
        assert!(
            crate::batch::protected_commands(&Operation::Clean(CleanupId {
                backend: "snap".into(),
                key: key.into(),
            }))
            .is_err()
        );
        assert!(matches!(
            SystemManager::snap(Kit::new(Ok(false), "")).execute(
                &Operation::Clean(CleanupId {
                    backend: "snap".into(),
                    key: key.into(),
                }),
                &cancel,
                &mut |_| {}
            ),
            Err(EngineError::NotFound)
        ));
    }
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(matches!(
        SystemManager::snap(Kit::new(Ok(false), "")).execute(
            &Operation::Clean(item.id.clone()),
            &cancelled,
            &mut |_| {}
        ),
        Err(EngineError::Cancelled)
    ));
    // Snap that fails to list its revisions says so.
    #[derive(Clone)]
    struct Broken;
    impl Transport for Broken {
        fn system_manager(
            &self,
            _: &str,
            _: &[OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            Ok(completion(1, ""))
        }
    }
    assert!(SystemManager::snap(Broken).cleanup(&cancel).is_err());
}

#[test]
fn system_file_tasks_have_fixed_root_commands() {
    for (key, program, last) in [
        ("journal", "/usr/bin/journalctl", "--vacuum-time=1week"),
        ("coredumps", "/usr/bin/find", "-delete"),
        ("crash-reports", "/usr/bin/find", "-delete"),
    ] {
        let commands = crate::batch::protected_commands(&Operation::Clean(CleanupId {
            backend: "system-files".into(),
            key: key.into(),
        }))
        .unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].program, program);
        assert_eq!(commands[0].args.last().map(String::as_str), Some(last));
    }
    assert!(
        crate::batch::protected_commands(&Operation::Clean(CleanupId {
            backend: "system-files".into(),
            key: "everything".into(),
        }))
        .is_err()
    );
    assert_eq!(display_name("system-files"), "System files");
    assert_eq!(display_name("steam"), "Steam");
    assert_eq!(
        crate::unattended::unattended("system-files"),
        crate::unattended::Unattended::Never
    );
}
