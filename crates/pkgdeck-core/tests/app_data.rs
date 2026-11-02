//! App data folders through the real backends: leftovers on the Clean page,
//! an installed app's folder on its page, and both going to the Trash.
use pkgdeck_core::{
    backends::*,
    engine::*,
    host::{AptAction, Authorization},
    package::*,
    process::*,
};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

fn output(value: &str) -> Completion {
    Completion {
        code: Some(0),
        signal: None,
        stdout: value.as_bytes().to_vec(),
        stderr: vec![],
        truncated: false,
        cancellation_deferred: false,
    }
}

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("pkgdeck-data-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

/// A host whose home is a scratch folder and whose package managers answer
/// with fixed lists.
#[derive(Clone)]
struct Home {
    home: PathBuf,
    data_home: Option<PathBuf>,
    /// What `flatpak list --app` prints for every installation, or a failure.
    apps: Option<&'static str>,
    /// What `flatpak --user list` prints.
    user: &'static str,
    /// What `snap list --all` prints.
    snaps: Option<&'static str>,
    calls: Arc<Mutex<Vec<Call>>>,
}
/// A command one manager ran: the tool, its arguments, and whether it writes.
type Call = (String, Vec<String>, bool);
impl Home {
    fn new(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
            data_home: None,
            apps: Some(""),
            user: "",
            snaps: Some(""),
            calls: Arc::default(),
        }
    }
    fn answer(
        &self,
        tool: &str,
        args: &[OsString],
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        let args: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into())
            .collect();
        self.calls
            .lock()
            .unwrap()
            .push((tool.into(), args.clone(), write));
        let failure = || {
            Err(ExecutionError::Failed(Completion {
                code: Some(1),
                stderr: b"error: broken installation\n".to_vec(),
                ..output("")
            }))
        };
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        match (tool, words.as_slice()) {
            ("flatpak", ["list", "--app", ..]) => {
                self.apps.map_or_else(failure, |apps| Ok(output(apps)))
            }
            ("flatpak", ["--user", "list", ..]) => Ok(output(self.user)),
            ("snap", ["list", "--all"]) => {
                self.snaps.map_or_else(failure, |snaps| Ok(output(snaps)))
            }
            ("snap", ["list"]) => Ok(output(
                "Name     Version  Rev   Tracking       Publisher  Notes\n\
                firefox  140.0    6000  latest/stable  mozilla**  -\n",
            )),
            ("snap", ["find", _]) => Ok(output(
                "Name      Version  Publisher  Notes  Summary\n\
                firefox   140.0    mozilla**  -      Mozilla Firefox web browser\n\
                chromium  141.0    canonical  -      Chromium web browser\n",
            )),
            _ => Ok(output("")),
        }
    }
}
impl Transport for Home {
    fn apt_query(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        unreachable!()
    }
    fn apt_write(&self, _: AptAction, _: &Cancellation) -> Result<Completion, ExecutionError> {
        unreachable!()
    }
    fn brew(
        &self,
        _: &[OsString],
        _: &Cancellation,
        _: bool,
    ) -> Result<Completion, ExecutionError> {
        unreachable!()
    }
    fn flatpak(
        &self,
        args: &[OsString],
        _: &Cancellation,
        write: bool,
        _: bool,
    ) -> Result<Completion, ExecutionError> {
        self.answer("flatpak", args, write)
    }
    fn system_manager(
        &self,
        tool: &str,
        args: &[OsString],
        _: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.answer(tool, args, write)
    }
    fn supports_flatpak_cleanup(&self) -> bool {
        false
    }
    fn authorization(&self) -> Authorization {
        Authorization::Polkit
    }
    fn env(&self, name: &str) -> Option<OsString> {
        match name {
            "HOME" => Some(self.home.clone().into()),
            "XDG_DATA_HOME" => self.data_home.clone().map(Into::into),
            _ => None,
        }
    }
}

fn keys(items: &[CleanupItem]) -> Vec<&str> {
    items.iter().map(|item| item.id.key.as_str()).collect()
}

fn clean(
    backend: &mut dyn Backend,
    name: &str,
    key: &str,
) -> Result<OperationOutcome, EngineError> {
    let mut messages = vec![];
    let result = backend.execute(
        &Operation::Clean(CleanupId {
            backend: name.into(),
            key: key.into(),
        }),
        &Cancellation::default(),
        &mut |progress| {
            if let Progress::Message(message) = progress {
                messages.push(message);
            }
        },
    );
    if result.is_ok() {
        assert_eq!(messages.len(), 1, "{messages:?}");
    }
    result
}

#[test]
fn flatpak_leftovers_are_folders_no_installation_lists() {
    let home = scratch("flatpak-leftovers");
    for name in ["org.gone.App", "org.kept.App", "org.extra.App"] {
        fs::create_dir_all(home.join(".var/app").join(name)).unwrap();
    }
    let mut fixture = Home::new(&home);
    // org.extra.App is installed somewhere besides the user's and the
    // system's installation: listed only when no installation is named.
    fixture.apps = Some("org.kept.App\tKept\norg.extra.App\tExtra\norg.gnome.Platform\t\n");
    let mut flatpak = Flatpak::new(fixture.clone());
    let cancel = Cancellation::default();
    assert!(flatpak.capabilities().contains(&Capability::Clean));
    let items = flatpak.cleanup(&cancel).unwrap();
    assert_eq!(keys(&items), ["leftover:org.gone.App"]);
    assert!(items[0].restorable && items[0].opt_in());
    // Nothing ran as root, or wrote anything, to find them.
    assert!(fixture.calls.lock().unwrap().iter().all(|call| !call.2));
    // A list that fails offers nothing rather than every folder.
    let mut broken = Flatpak::new(Home {
        apps: None,
        ..fixture.clone()
    });
    assert!(broken.cleanup(&cancel).is_err());
    assert!(broken.cleanup_plan(&items[0].id, &cancel).is_err());
    assert!(clean(&mut broken, "flatpak", "leftover:org.gone.App").is_err());
    assert!(home.join(".var/app/org.gone.App").is_dir());
    // The reviewed task moves the folder to the Trash in the data home.
    let data_home = home.join("data");
    let mut flatpak = Flatpak::new(Home {
        data_home: Some(data_home.clone()),
        ..fixture
    });
    assert_eq!(
        flatpak.cleanup_plan(&items[0].id, &cancel).unwrap(),
        items[0]
    );
    clean(&mut flatpak, "flatpak", "leftover:org.gone.App").unwrap();
    assert!(!home.join(".var/app/org.gone.App").exists());
    assert!(data_home.join("Trash/files/org.gone.App").is_dir());
    assert!(data_home
        .join("Trash/info/org.gone.App.trashinfo")
        .is_file());
    // Gone now, so there's nothing left to do.
    assert!(matches!(
        clean(&mut flatpak, "flatpak", "leftover:org.gone.App"),
        Err(EngineError::NotFound)
    ));
    // An installed app's folder is never a leftover.
    assert!(clean(&mut flatpak, "flatpak", "leftover:org.kept.App").is_err());
    assert!(home.join(".var/app/org.kept.App").is_dir());
    fs::remove_dir_all(&home).unwrap();
}

#[test]
fn an_installed_flatpaks_page_shows_its_data() {
    let home = scratch("flatpak-active");
    fs::create_dir_all(home.join(".var/app/org.kept.App/config")).unwrap();
    fs::write(
        home.join(".var/app/org.kept.App/config/settings"),
        vec![0; 8192],
    )
    .unwrap();
    fs::create_dir_all(home.join(".var/app/org.gnome.Platform")).unwrap();
    let mut fixture = Home::new(&home);
    fixture.apps = Some("org.kept.App\tKept\n");
    fixture.user = "org.kept.App\tx86_64\tstable\t1.0\tKept app\tflathub\t\tKept\n\
        org.gnome.Platform\tx86_64\t48\t\tShared libraries\tflathub\truntime\tGNOME\n";
    let mut flatpak = Flatpak::new(fixture);
    let cancel = Cancellation::default();
    let installed = flatpak.installed(&cancel).unwrap();
    let id = |name: &str| {
        installed
            .iter()
            .find(|p| p.id.name == name)
            .unwrap()
            .id
            .clone()
    };
    let data = flatpak
        .details(&id("org.kept.App"), &cancel)
        .unwrap()
        .data
        .unwrap();
    assert_eq!(data.folder, home.join(".var/app/org.kept.App"));
    assert_eq!(data.shown, "~/.var/app/org.kept.App");
    assert!(data.bytes >= 8192);
    // Runtimes keep no data, whatever a folder is called.
    assert!(flatpak
        .details(&id("org.gnome.Platform"), &cancel)
        .unwrap()
        .data
        .is_none());
    let task = CleanupId {
        backend: "flatpak".into(),
        key: "data:org.kept.App".into(),
    };
    let plan = flatpak.cleanup_plan(&task, &cancel).unwrap();
    assert_eq!(plan.title, "Kept data");
    assert!(plan.restorable);
    clean(&mut flatpak, "flatpak", "data:org.kept.App").unwrap();
    assert!(!home.join(".var/app/org.kept.App").exists());
    assert!(home
        .join(".local/share/Trash/files/org.kept.App/config/settings")
        .is_file());
    assert!(flatpak
        .details(&id("org.kept.App"), &cancel)
        .unwrap()
        .data
        .is_none());
    fs::remove_dir_all(&home).unwrap();
}

const SNAPS: &str = "Name     Version  Rev   Tracking       Publisher  Notes\n\
    firefox  140.0    6000  latest/stable  mozilla**  -\n\
    firefox  139.0    5900  latest/stable  mozilla**  disabled\n\
    core22   2025     1900  latest/stable  canonical  base\n";

#[test]
fn snap_leftovers_sit_beside_old_revisions_and_run_as_the_person() {
    let home = scratch("snap-leftovers");
    for name in ["firefox", "chromium", "core22"] {
        fs::create_dir_all(home.join("snap").join(name).join("common")).unwrap();
    }
    let mut fixture = Home::new(&home);
    fixture.snaps = Some(SNAPS);
    let mut snap = SystemManager::snap(fixture.clone());
    let cancel = Cancellation::default();
    let items = snap.cleanup(&cancel).unwrap();
    assert_eq!(
        keys(&items),
        ["disabled-revisions:firefox=5900", "leftover:chromium"]
    );
    assert!(!items[0].restorable && items[1].restorable);
    assert_eq!(items[1].summary, "Snap doesn't list this app as installed");
    // Old revisions need the password, app data never does.
    for item in &items {
        let commands =
            pkgdeck_core::batch::protected_commands(&Operation::Clean(item.id.clone())).unwrap();
        assert_eq!(commands.is_empty(), item.restorable, "{}", item.id.key);
    }
    for key in ["data:firefox", "leftover:chromium", "leftover:../../etc"] {
        let task = Operation::Clean(CleanupId {
            backend: "snap".into(),
            key: key.into(),
        });
        assert!(pkgdeck_core::batch::protected_commands(&task)
            .unwrap()
            .is_empty());
    }
    // An installed snap's page shows its folder, one that isn't shows none.
    let found = snap.search("firefox", &cancel).unwrap();
    let firefox = found.iter().find(|p| p.id.name == "firefox").unwrap();
    let chromium = found.iter().find(|p| p.id.name == "chromium").unwrap();
    let data = snap.details(&firefox.id, &cancel).unwrap().data.unwrap();
    assert_eq!(data.shown, "~/snap/firefox");
    assert!(snap.details(&chromium.id, &cancel).unwrap().data.is_none());
    assert_eq!(snap.cleanup_plan(&items[1].id, &cancel).unwrap(), items[1]);
    clean(&mut snap, "snap", "leftover:chromium").unwrap();
    assert!(!home.join("snap/chromium").exists());
    assert!(home
        .join(".local/share/Trash/files/chromium/common")
        .is_dir());
    // An installed snap's folder only goes as its data, from its page.
    assert!(clean(&mut snap, "snap", "leftover:firefox").is_err());
    let plan = snap
        .cleanup_plan(
            &CleanupId {
                backend: "snap".into(),
                key: "data:firefox".into(),
            },
            &cancel,
        )
        .unwrap();
    assert_eq!(plan.title, "firefox data");
    clean(&mut snap, "snap", "data:firefox").unwrap();
    assert!(!home.join("snap/firefox").exists());
    // Nothing wrote through Snap itself.
    assert!(fixture.calls.lock().unwrap().iter().all(|call| !call.2));
    // A listing that fails offers nothing.
    let mut broken = SystemManager::snap(Home {
        snaps: None,
        ..fixture
    });
    assert!(broken.cleanup(&cancel).is_err());
    assert!(clean(&mut broken, "snap", "data:core22").is_err());
    assert!(home.join("snap/core22").is_dir());
    fs::remove_dir_all(&home).unwrap();
}
