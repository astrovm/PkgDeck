//! Game folders Steam left behind: a folder in a library's
//! `steamapps/common` that no installed game names as its own. Uninstalling
//! a game can leave its settings and shader caches there, and so does a
//! library moved by hand. Saves can live there too, so each one waits to
//! be checked once.
use super::*;
use crate::{disk, host::Host};
use dev_caches::shown;
use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime},
};

const CAPABILITIES: &[Capability] = &[Capability::Clean];
/// Folders Steam itself keeps there.
const STEAM_OWN: &[&str] = &["Steam Controller Configs"];
/// A folder this new may be a game Steam is installing right now.
const SETTLED: Duration = Duration::from_secs(86_400);

pub struct Steam {
    host: Host,
}

/// The quoted values of `key` in a Valve key-value file.
fn values(text: &str, key: &str) -> Vec<String> {
    let quoted = format!("\"{key}\"");
    text.lines()
        .filter_map(|line| line.trim().strip_prefix(&quoted))
        .filter_map(|rest| {
            let rest = rest.trim().strip_prefix('"')?;
            rest.strip_suffix('"')
                .map(|value| value.replace("\\\\", "\\"))
        })
        .collect()
}

impl Steam {
    pub fn new(host: Host) -> Self {
        Self { host }
    }

    /// Where Steam keeps itself: the usual install, Flatpak's and Snap's.
    fn roots(&self) -> Vec<PathBuf> {
        let Some(home) = self
            .host
            .var("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
        else {
            return vec![];
        };
        let candidates = if cfg!(target_os = "macos") {
            vec![home.join("Library/Application Support/Steam")]
        } else {
            vec![
                home.join(".local/share/Steam"),
                home.join(".steam/steam"),
                home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
                home.join("snap/steam/common/.local/share/Steam"),
            ]
        };
        let mut seen = vec![];
        let mut roots = vec![];
        for root in candidates {
            if let Ok(real) = fs::canonicalize(&root) {
                if !seen.contains(&real) {
                    seen.push(real);
                    roots.push(root);
                }
            }
        }
        roots
    }

    /// Every library: each install's own, and those its list names.
    fn libraries(&self) -> Vec<PathBuf> {
        let mut seen = vec![];
        let mut libraries = vec![];
        for root in self.roots() {
            let listed = fs::read_to_string(root.join("steamapps/libraryfolders.vdf"))
                .map(|text| values(&text, "path"))
                .unwrap_or_default();
            let found = std::iter::once(root)
                .chain(listed.into_iter().map(PathBuf::from))
                .filter(|library| library.is_absolute());
            for library in found {
                if let Ok(real) = fs::canonicalize(&library) {
                    if !seen.contains(&real) {
                        seen.push(real);
                        libraries.push(library);
                    }
                }
            }
        }
        libraries
    }

    /// The folders installed games in `library` name as theirs. `None`
    /// when the library can't be read, so nothing in it counts as left.
    fn installed(library: &Path) -> Option<Vec<String>> {
        let mut names = vec![];
        for entry in fs::read_dir(library.join("steamapps")).ok()?.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("appmanifest_") && name.ends_with(".acf") {
                names.extend(values(&fs::read_to_string(entry.path()).ok()?, "installdir"));
            }
        }
        Some(names)
    }

    /// The task for `folder`, when it's a game folder no game in its
    /// library names.
    fn item(&self, folder: &Path, cancel: &Cancellation) -> Option<CleanupItem> {
        let name = folder.file_name()?.to_str()?;
        let common = folder.parent()?;
        let library = common.parent()?.parent()?;
        if common.file_name()? != "common"
            || common.parent()?.file_name()? != "steamapps"
            || STEAM_OWN.contains(&name)
            || !self.libraries().iter().any(|known| known == library)
        {
            return None;
        }
        let metadata = fs::symlink_metadata(folder).ok()?;
        let settled = metadata
            .modified()
            .is_ok_and(|changed| changed + SETTLED < SystemTime::now());
        if !metadata.is_dir() || !settled {
            return None;
        }
        let installed = Self::installed(library)?;
        if installed
            .iter()
            .any(|installed| installed.eq_ignore_ascii_case(name))
        {
            return None;
        }
        Some(CleanupItem {
            id: CleanupId {
                backend: "steam".into(),
                key: format!("leftover:{}", folder.display()),
            },
            kind: CleanupKind::Data,
            title: format!("{name} leftovers"),
            summary: "Steam doesn't list this game as installed".into(),
            preview: format!(
                "Deletes this folder, with any settings or saves a game kept in it.\n{}",
                shown(&self.host, folder)
            ),
            bytes: Some(disk::usage(folder, cancel)),
        })
    }
}

impl Backend for Steam {
    fn id(&self) -> &str {
        "steam"
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, _cancel: &Cancellation) -> Result<Availability, EngineError> {
        Ok(if self.roots().is_empty() {
            Availability::Unavailable("Steam isn't installed.".into())
        } else {
            Availability::Available
        })
    }
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        let mut folders: Vec<_> = self
            .libraries()
            .iter()
            .flat_map(|library| fs::read_dir(library.join("steamapps/common")).into_iter())
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        folders.sort();
        let items = folders
            .iter()
            .filter_map(|folder| self.item(folder, cancel))
            .collect();
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        Ok(items)
    }
    fn cleanup_plan(
        &mut self,
        id: &CleanupId,
        cancel: &Cancellation,
    ) -> Result<CleanupItem, EngineError> {
        id.key
            .strip_prefix("leftover:")
            .map(Path::new)
            .filter(|folder| id.backend == "steam" && folder.is_absolute())
            .filter(|folder| {
                !folder
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
            })
            .and_then(|folder| self.item(folder, cancel))
            .ok_or(EngineError::NotFound)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let Operation::Clean(id) = operation else {
            return Err(self.unsupported(operation.capability()));
        };
        let plan = self.cleanup_plan(id, cancel)?;
        let folder = Path::new(id.key.strip_prefix("leftover:").unwrap_or_default());
        progress(Progress::Message(format!("Deleting {}…", plan.title)));
        disk::remove(folder).map_err(|error| {
            EngineError::Execution(ExecutionError::Io(format!(
                "Couldn't remove {}: {error}",
                shown(&self.host, folder)
            )))
        })?;
        Ok(OperationOutcome {
            cancellation_deferred: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Runtime;
    use std::collections::BTreeMap;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("pkgdeck-steam-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }
    fn steam(home: &Path) -> Steam {
        Steam::new(Host::new(
            Runtime::Native,
            BTreeMap::from([("HOME".into(), home.as_os_str().to_owned())]),
        ))
    }
    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn game(library: &Path, id: u32, folder: &str) {
        write(
            &library.join(format!("steamapps/appmanifest_{id}.acf")),
            &format!("\"AppState\"\n{{\n\t\"appid\"\t\t\"{id}\"\n\t\"installdir\"\t\t\"{folder}\"\n}}\n"),
        );
        write(&library.join("steamapps/common").join(folder).join("game.bin"), "x");
    }
    fn settle(path: &Path) {
        let when = SystemTime::now() - Duration::from_secs(3 * 86_400);
        fs::File::open(path).unwrap().set_modified(when).unwrap();
    }

    #[test]
    fn game_folders_no_installed_game_names_are_offered_and_deleted() {
        let home = scratch("leftovers");
        let root = home.join(if cfg!(target_os = "macos") {
            "Library/Application Support/Steam"
        } else {
            ".local/share/Steam"
        });
        let other = home.join("games/SteamLibrary");
        write(
            &root.join("steamapps/libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"2\"\n\t{{\n\t\t\"path\"\t\t\"/nonexistent/pkgdeck/unplugged\"\n\t}}\n}}\n",
                root.display(),
                other.display()
            ),
        );
        game(&root, 228980, "Steamworks Shared");
        game(&other, 12100, "Grand Theft Auto 3");
        let left = root.join("steamapps/common/RESIDENT EVIL 2  BIOHAZARD RE2");
        write(&left.join("re2_config.ini"), "x");
        let elsewhere = other.join("steamapps/common/Old Game");
        write(&elsewhere.join("shader.cache"), "x");
        // Steam's own folder, a file, a game that names its folder in
        // another case, and one that showed up just now.
        write(&root.join("steamapps/common/Steam Controller Configs/x.vdf"), "x");
        write(&root.join("steamapps/common/Steam.dll"), "x");
        game(&other, 12110, "grand theft auto vice city");
        fs::rename(
            other.join("steamapps/common/grand theft auto vice city"),
            other.join("steamapps/common/Grand Theft Auto Vice City"),
        )
        .unwrap();
        let installing = other.join("steamapps/common/Installing");
        fs::create_dir_all(&installing).unwrap();
        for folder in fs::read_dir(root.join("steamapps/common"))
            .unwrap()
            .chain(fs::read_dir(other.join("steamapps/common")).unwrap())
        {
            let path = folder.unwrap().path();
            if path != installing {
                settle(&path);
            }
        }
        let mut backend = steam(&home);
        let cancel = Cancellation::default();
        assert_eq!(backend.detect(&cancel).unwrap(), Availability::Available);
        let items = backend.cleanup(&cancel).unwrap();
        let keys: Vec<_> = items.iter().map(|item| item.id.key.clone()).collect();
        let mut expected = vec![
            format!("leftover:{}", left.display()),
            format!("leftover:{}", elsewhere.display()),
        ];
        expected.sort();
        assert_eq!(keys, expected);
        let item = items
            .iter()
            .find(|item| item.title == "RESIDENT EVIL 2  BIOHAZARD RE2 leftovers")
            .unwrap();
        assert_eq!(item.kind, CleanupKind::Data);
        assert!(item.opt_in() && !item.kind.in_clean_all());
        assert!(item.preview.ends_with("/steamapps/common/RESIDENT EVIL 2  BIOHAZARD RE2"));
        assert_eq!(backend.cleanup_plan(&item.id, &cancel).unwrap(), *item);
        // Only a game folder in a known library can be named.
        for key in [
            format!("leftover:{}", root.join("steamapps/common/Steam Controller Configs").display()),
            format!("leftover:{}", root.join("steamapps/common/Steamworks Shared").display()),
            format!("leftover:{}", home.join("elsewhere/steamapps/common/x").display()),
            format!("leftover:{}/../common/x", root.join("steamapps/common").display()),
            "leftover:relative/steamapps/common/x".into(),
            "other".into(),
        ] {
            let id = CleanupId {
                backend: "steam".into(),
                key,
            };
            assert!(matches!(backend.cleanup_plan(&id, &cancel), Err(EngineError::NotFound)));
        }
        let mut said = vec![];
        backend
            .execute(&Operation::Clean(item.id.clone()), &cancel, &mut |progress| said.push(progress))
            .unwrap();
        assert_eq!(
            said,
            [Progress::Message("Deleting RESIDENT EVIL 2  BIOHAZARD RE2 leftovers…".into())]
        );
        assert!(!left.exists() && elsewhere.exists());
        // A game installed into the folder since keeps it.
        game(&other, 99, "Old Game");
        assert!(backend.cleanup(&cancel).unwrap().is_empty());
        assert!(matches!(
            backend.execute(
                &Operation::Refresh {
                    backend: "steam".into()
                },
                &cancel,
                &mut |_| {}
            ),
            Err(EngineError::Unsupported { .. })
        ));
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(matches!(backend.cleanup(&cancelled), Err(EngineError::Cancelled)));
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn without_steam_there_is_nothing_to_look_at() {
        let home = scratch("none");
        let cancel = Cancellation::default();
        assert!(matches!(
            steam(&home).detect(&cancel),
            Ok(Availability::Unavailable(_))
        ));
        let mut homeless = Steam::new(Host::new(Runtime::Native, BTreeMap::new()));
        assert!(homeless.roots().is_empty());
        assert!(homeless.cleanup(&cancel).unwrap().is_empty());
        assert_eq!(values("\"path\"\t\t\"C:\\\\Games\"\n\"label\" \"\"", "path"), ["C:\\Games"]);
        fs::remove_dir_all(&home).unwrap();
    }
}
