//! Folders apps keep their settings and files in: `~/.var/app/<id>` for
//! Flatpak apps and `~/snap/<name>` for snaps. An installed app's folder
//! shows on its page, where it can go to the Trash. A folder whose app is
//! gone is leftover data on the Clean page, unchecked. Both go to the Trash,
//! so they can be restored.
use super::*;
use crate::{disk, package::AppData, trash::Trash};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

const LEFTOVER: &str = "leftover:";
const DATA: &str = "data:";

/// Whether a cleanup key names one of these folders. Such tasks run as the
/// person, never through the password-protected runner.
pub fn is_data_key(key: &str) -> bool {
    key.starts_with(LEFTOVER) || key.starts_with(DATA)
}

/// The task that moves an installed app's own folder to the Trash, for the
/// sources that keep one per app.
pub fn app_data_task(id: &PackageId) -> Option<CleanupId> {
    matches!(id.backend.as_str(), "flatpak" | "snap").then(|| CleanupId {
        backend: id.backend.clone(),
        key: format!("{DATA}{}", id.name),
    })
}

/// The person's home on the host, when it's known and absolute.
pub(super) fn home<T: Transport + ?Sized>(transport: &T) -> Option<PathBuf> {
    transport
        .env("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

/// The person's Trash on the host.
pub(super) fn trash<T: Transport + ?Sized>(transport: &T) -> Option<Trash> {
    let data_home = transport.env("XDG_DATA_HOME").map(PathBuf::from);
    Trash::home(home(transport).as_deref(), data_home.as_deref())
}

pub(super) struct DataFolders {
    backend: &'static str,
    manager: &'static str,
    root: PathBuf,
    /// `root` as people know it: `~/.var/app`.
    shown: String,
}

impl DataFolders {
    pub(super) fn flatpak(home: &Path) -> Self {
        Self::new("flatpak", "Flatpak", home, ".var/app")
    }

    pub(super) fn snap(home: &Path) -> Self {
        Self::new("snap", "Snap", home, "snap")
    }

    fn new(backend: &'static str, manager: &'static str, home: &Path, root: &str) -> Self {
        Self {
            backend,
            manager,
            root: home.join(root),
            shown: format!("~/{root}"),
        }
    }

    /// The folder `name` keeps, if it has one: a real folder, not a link.
    fn folder(&self, name: &str) -> Option<PathBuf> {
        let plain = !name.is_empty() && !name.starts_with('.') && !name.contains('/');
        let folder = self.root.join(name);
        (plain && fs::symlink_metadata(&folder).is_ok_and(|metadata| metadata.is_dir()))
            .then_some(folder)
    }

    /// The folder `name` keeps, as people know it: `~/.var/app/org.example.App`.
    fn shown(&self, name: &str) -> String {
        format!("{}/{name}", self.shown)
    }

    /// The folder an installed app keeps, and how much it takes.
    pub(super) fn data(&self, name: &str, cancel: &Cancellation) -> Option<AppData> {
        let folder = self.folder(name)?;
        Some(AppData {
            shown: self.shown(name),
            bytes: disk::usage(&folder, cancel),
            folder,
        })
    }

    /// Folders no installed app owns. `installed` must name every installed
    /// app, from every installation: one it leaves out would look abandoned.
    pub(super) fn leftovers(
        &self,
        installed: &BTreeMap<String, String>,
        cancel: &Cancellation,
    ) -> Result<Vec<CleanupItem>, EngineError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(error) => return Err(failed(format!("Couldn't read {}", self.shown), &error)),
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !installed.contains_key(name))
            .collect();
        names.sort();
        let mut items = vec![];
        for name in names {
            if cancel.requested() {
                return Err(EngineError::Cancelled);
            }
            if let Some(item) = self.item(&format!("{LEFTOVER}{name}"), installed, cancel) {
                items.push(item);
            }
        }
        Ok(items)
    }

    /// The task `key` names, as it stands now: a leftover only while its
    /// app isn't installed, an app's data only while it is.
    pub(super) fn plan(
        &self,
        key: &str,
        installed: &BTreeMap<String, String>,
        cancel: &Cancellation,
    ) -> Result<CleanupItem, EngineError> {
        self.item(key, installed, cancel)
            .ok_or(EngineError::NotFound)
    }

    fn item(
        &self,
        key: &str,
        installed: &BTreeMap<String, String>,
        cancel: &Cancellation,
    ) -> Option<CleanupItem> {
        let (name, title, summary) = if let Some(name) = key.strip_prefix(LEFTOVER) {
            if installed.contains_key(name) {
                return None;
            }
            let summary = format!("{} doesn't list this app as installed", self.manager);
            (name, format!("{name} data"), summary)
        } else {
            let name = key.strip_prefix(DATA)?;
            let app = installed.get(name)?;
            (
                name,
                format!("{app} data"),
                format!("{app} starts over without it"),
            )
        };
        let folder = self.folder(name)?;
        Some(CleanupItem {
            id: CleanupId {
                backend: self.backend.into(),
                key: key.into(),
            },
            kind: CleanupKind::Data,
            title,
            summary,
            preview: format!(
                "Moves this folder to the Trash, with the settings and files the app kept in it.\n{}",
                self.shown(name)
            ),
            bytes: Some(disk::usage(&folder, cancel)),
            restorable: true,
        })
    }

    /// Moves the folder `key` names to the Trash, once it's still the task
    /// that was reviewed.
    pub(super) fn trash(
        &self,
        key: &str,
        installed: &BTreeMap<String, String>,
        trash: Option<Trash>,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let plan = self.plan(key, installed, cancel)?;
        let name = key
            .strip_prefix(LEFTOVER)
            .or_else(|| key.strip_prefix(DATA))
            .unwrap_or_default();
        let folder = self.folder(name).ok_or(EngineError::NotFound)?;
        let trash = trash.ok_or_else(|| {
            EngineError::Execution(ExecutionError::Io(
                "Couldn't find the Trash: the home folder isn't known".into(),
            ))
        })?;
        progress(Progress::Message(format!(
            "Moving {} to the Trash…",
            plan.title
        )));
        trash.put(&folder).map_err(|error| {
            failed(
                format!("Couldn't move {} to the Trash", self.shown(name)),
                &error,
            )
        })?;
        Ok(OperationOutcome::default())
    }
}

fn failed(what: String, error: &io::Error) -> EngineError {
    let reason = if error.kind() == io::ErrorKind::CrossesDevices {
        "it's on another disk than the Trash".to_owned()
    } else {
        error.to_string()
    };
    EngineError::Execution(ExecutionError::Io(format!("{what}: {reason}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("pkgdeck-app-data-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn installed(names: &[(&str, &str)]) -> BTreeMap<String, String> {
        names
            .iter()
            .map(|(name, app)| ((*name).into(), (*app).into()))
            .collect()
    }

    #[test]
    fn folders_of_apps_that_are_gone_are_leftovers_to_restore() {
        let home = scratch("leftovers");
        let folders = DataFolders::flatpak(&home);
        let apps = home.join(".var/app");
        for name in ["org.gone.App", "org.kept.App", "org.also.Gone"] {
            fs::create_dir_all(apps.join(name).join("config")).unwrap();
        }
        fs::write(
            apps.join("org.gone.App/config/settings"),
            vec![0; 64 * 1024],
        )
        .unwrap();
        // Not folders an app keeps: a file, a link, a hidden entry.
        fs::write(apps.join("org.file.App"), "").unwrap();
        std::os::unix::fs::symlink(apps.join("org.kept.App"), apps.join("org.link.App")).unwrap();
        fs::create_dir_all(apps.join(".cache")).unwrap();
        let kept = installed(&[("org.kept.App", "Kept")]);
        let items = folders.leftovers(&kept, &Cancellation::default()).unwrap();
        let keys: Vec<_> = items.iter().map(|item| item.id.key.as_str()).collect();
        assert_eq!(keys, ["leftover:org.also.Gone", "leftover:org.gone.App"]);
        let gone = &items[1];
        assert_eq!(gone.id.backend, "flatpak");
        assert_eq!(gone.kind, CleanupKind::Data);
        assert!(gone.opt_in() && !gone.kind.in_clean_all() && gone.restorable);
        assert_eq!(gone.title, "org.gone.App data");
        assert_eq!(gone.summary, "Flatpak doesn't list this app as installed");
        assert_eq!(
            gone.preview,
            "Moves this folder to the Trash, with the settings and files the app kept in it.\n~/.var/app/org.gone.App"
        );
        assert!(gone.bytes.unwrap() >= 64 * 1024);
        // The plan is the same task again, until the app comes back.
        let cancel = Cancellation::default();
        assert_eq!(folders.plan(&gone.id.key, &kept, &cancel).unwrap(), *gone);
        let back = installed(&[("org.kept.App", "Kept"), ("org.gone.App", "Gone")]);
        assert!(matches!(
            folders.plan(&gone.id.key, &back, &cancel),
            Err(EngineError::NotFound)
        ));
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn an_installed_apps_folder_shows_and_can_go_to_the_trash() {
        let home = scratch("active");
        let folders = DataFolders::snap(&home);
        fs::create_dir_all(home.join("snap/firefox/common")).unwrap();
        let cancel = Cancellation::default();
        let data = folders.data("firefox", &cancel).unwrap();
        assert_eq!(data.folder, home.join("snap/firefox"));
        assert_eq!(data.shown, "~/snap/firefox");
        assert!(folders.data("chromium", &cancel).is_none());
        let apps = installed(&[("firefox", "Firefox")]);
        let item = folders.plan("data:firefox", &apps, &cancel).unwrap();
        assert_eq!(item.id.backend, "snap");
        assert_eq!(item.title, "Firefox data");
        assert_eq!(item.summary, "Firefox starts over without it");
        // Only while it's installed, and never a leftover meanwhile.
        assert!(folders.plan("leftover:firefox", &apps, &cancel).is_err());
        assert!(folders
            .plan("data:firefox", &BTreeMap::new(), &cancel)
            .is_err());
        assert!(folders.leftovers(&apps, &cancel).unwrap().is_empty());
        let mut messages = vec![];
        let trash = Trash::at(home.join(".local/share/Trash"));
        folders
            .trash(
                "data:firefox",
                &apps,
                Some(trash),
                &cancel,
                &mut |progress| {
                    if let Progress::Message(message) = progress {
                        messages.push(message);
                    }
                },
            )
            .unwrap();
        assert_eq!(messages, ["Moving Firefox data to the Trash…"]);
        assert!(!home.join("snap/firefox").exists());
        assert!(home
            .join(".local/share/Trash/files/firefox/common")
            .is_dir());
        assert!(home
            .join(".local/share/Trash/info/firefox.trashinfo")
            .is_file());
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn only_a_plain_folder_name_inside_the_root_is_a_task() {
        let home = scratch("names");
        let folders = DataFolders::flatpak(&home);
        fs::create_dir_all(home.join(".var/app/org.real.App")).unwrap();
        fs::create_dir_all(home.join(".var/elsewhere")).unwrap();
        let cancel = Cancellation::default();
        let none = BTreeMap::new();
        for key in [
            "leftover:",
            "leftover:..",
            "leftover:../elsewhere",
            "leftover:.hidden",
            "leftover:org.real.App/..",
            "data:",
            "unused-user",
            "org.real.App",
        ] {
            assert!(folders.plan(key, &none, &cancel).is_err(), "{key}");
        }
        assert!(folders
            .plan("leftover:org.real.App", &none, &cancel)
            .is_ok());
        assert!(is_data_key("leftover:x") && is_data_key("data:x"));
        assert!(!is_data_key("unused-user") && !is_data_key("disabled-revisions:a=1"));
        // A home with no such folder has nothing to clean.
        let empty = scratch("empty");
        assert!(DataFolders::flatpak(&empty)
            .leftovers(&none, &cancel)
            .unwrap()
            .is_empty());
        // One it can't read says which, rather than offering nothing.
        fs::write(empty.join("snap"), "").unwrap();
        let error = DataFolders::snap(&empty)
            .leftovers(&none, &cancel)
            .unwrap_err();
        assert!(
            error.to_string().contains("Couldn't read ~/snap: "),
            "{error}"
        );
        // Only Flatpak and Snap keep a folder per app.
        for (backend, task) in [
            ("flatpak", Some("data:org.real.App")),
            ("snap", Some("data:org.real.App")),
            ("apt", None),
        ] {
            let id = PackageId {
                backend: backend.into(),
                name: "org.real.App".into(),
                architecture: String::new(),
                scope: crate::package::Scope::System,
                remote: None,
                reference: None,
            };
            let found = app_data_task(&id);
            assert_eq!(found.as_ref().map(|id| id.key.as_str()), task, "{backend}");
            assert!(found.is_none_or(|id| id.backend == backend));
        }
        fs::remove_dir_all(&home).unwrap();
        fs::remove_dir_all(&empty).unwrap();
    }

    #[test]
    fn a_folder_that_cant_move_is_named_in_the_error() {
        let home = scratch("fails");
        let folders = DataFolders::flatpak(&home);
        fs::create_dir_all(home.join(".var/app/org.gone.App")).unwrap();
        let cancel = Cancellation::default();
        let none = BTreeMap::new();
        let error = folders
            .trash("leftover:org.gone.App", &none, None, &cancel, &mut |_| {})
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            EngineError::Execution(ExecutionError::Io(
                "Couldn't find the Trash: the home folder isn't known".into()
            ))
            .to_string()
        );
        // The Trash on another disk: the move would copy everything.
        let error = failed(
            "Couldn't move ~/.var/app/org.gone.App to the Trash".into(),
            &io::Error::from(io::ErrorKind::CrossesDevices),
        );
        assert!(error.to_string().contains(
            "Couldn't move ~/.var/app/org.gone.App to the Trash: it's on another disk than the Trash"
        ));
        // A trash that can't be made names the folder.
        fs::write(home.join("not-a-folder"), "").unwrap();
        let error = folders
            .trash(
                "leftover:org.gone.App",
                &none,
                Some(Trash::at(home.join("not-a-folder/Trash"))),
                &cancel,
                &mut |_| {},
            )
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("Couldn't move ~/.var/app/org.gone.App to the Trash: "));
        assert!(home.join(".var/app/org.gone.App").is_dir());
        // A cancelled look stops.
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(matches!(
            folders.leftovers(&none, &cancelled),
            Err(EngineError::Cancelled)
        ));
        fs::remove_dir_all(&home).unwrap();
    }
}
