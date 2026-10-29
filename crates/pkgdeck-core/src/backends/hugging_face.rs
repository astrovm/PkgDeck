//! Models, datasets and Spaces Hugging Face downloaded and nobody used for
//! a while. Each one is a folder in the hub cache; newer versions of the
//! Hugging Face library keep big files once for every repository that has
//! them, in the hub's own `blobs` folder, and list who uses each one in a
//! `.refs` file next to it. A shared file goes only once nothing else lists
//! it, and one with no list stays.
use super::*;
use crate::{disk, host::Host};
use dev_caches::shown;
use projects::how_long;
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::Path,
    time::{Duration, SystemTime},
};

/// The prefixes the library names its folders with.
const KINDS: &[&str] = &["models--", "datasets--", "spaces--"];
/// Linux updates when a file was last read at most once a day, so a model
/// counts as unused only after two.
const SHORTEST: Duration = Duration::from_secs(2 * 86_400);

/// The hub cache: `HF_HUB_CACHE`, `HF_HOME/hub`, or `huggingface/hub` in
/// the cache folder, which is `~/.cache` on macOS too.
pub(super) fn hub(host: &Host) -> Option<PathBuf> {
    let var = |name: &str| {
        host.var(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    var("HF_HUB_CACHE")
        .or_else(|| var("HF_HOME").map(|home| home.join("hub")))
        .or_else(|| {
            let cache =
                var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|home| home.join(".cache")))?;
            Some(cache.join("huggingface/hub"))
        })
}

/// Whether the disk `path` is on records when files are read. Mounted with
/// `noatime`, it doesn't, and there is no telling what is in use.
fn records_reads(path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        rustix::fs::statvfs(path)
            .is_ok_and(|disk| !disk.f_flag.contains(rustix::fs::StatVfsMountFlags::NOATIME))
    }
    #[cfg(not(target_os = "linux"))]
    {
        path.exists()
    }
}

/// A repository folder's name as people know the repository:
/// `models--Systran--faster-whisper-small` is `Systran/faster-whisper-small`.
fn repository(folder: &str) -> Option<String> {
    let rest = KINDS.iter().find_map(|kind| folder.strip_prefix(kind))?;
    (!rest.is_empty() && !rest.contains('/')).then(|| rest.replace("--", "/"))
}

/// A file in the hub's shared store that `link` leads to.
fn shared(hub: &Path, link: &Path) -> Option<PathBuf> {
    let target = fs::canonicalize(link).ok()?;
    let store = fs::canonicalize(hub.join("blobs")).ok()?;
    (target.parent()?.parent()? == store).then_some(target)
}

/// The repository folders that list `store` and still lead to it. `None`
/// when the list can't be read: then the file must stay.
fn users(hub: &Path, store: &Path) -> Option<Vec<String>> {
    let mut list = store.as_os_str().to_owned();
    list.push(".refs");
    let text = fs::read_to_string(PathBuf::from(list)).ok()?;
    Some(
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.contains(".."))
            .filter(|line| {
                let link = hub.join(line);
                fs::symlink_metadata(&link).is_ok_and(|metadata| metadata.is_symlink())
                    && fs::canonicalize(&link).is_ok_and(|target| target == store)
            })
            .filter_map(|line| line.split('/').next().map(str::to_owned))
            .collect(),
    )
}

/// What one repository has: its own files, the shared ones only it uses,
/// and when any of them was last read or written.
struct Repository {
    bytes: u64,
    shared: Vec<PathBuf>,
    used: SystemTime,
}

fn look(hub: &Path, folder: &str, cancel: &Cancellation) -> Repository {
    let root = hub.join(folder);
    let mut found = Repository {
        bytes: disk::usage(&root, cancel),
        shared: vec![],
        used: SystemTime::UNIX_EPOCH,
    };
    let note = |used: &mut SystemTime, metadata: &fs::Metadata| {
        for time in [metadata.accessed(), metadata.modified()]
            .into_iter()
            .flatten()
        {
            *used = (*used).max(time);
        }
    };
    // A check for a newer revision writes its ref.
    for entry in fs::read_dir(root.join("refs"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if let Ok(metadata) = fs::metadata(entry.path()) {
            note(&mut found.used, &metadata);
        }
    }
    for entry in fs::read_dir(root.join("blobs"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let link = entry.path();
        let Some(store) = shared(hub, &link) else {
            if let Ok(metadata) = fs::metadata(&link) {
                note(&mut found.used, &metadata);
            }
            continue;
        };
        // A file others use too may have been read through them.
        if users(hub, &store).is_some_and(|users| users.iter().all(|user| user == folder)) {
            if let Ok(metadata) = fs::symlink_metadata(&store) {
                note(&mut found.used, &metadata);
                found.bytes += metadata.blocks() * 512;
            }
            found.shared.push(store);
        }
    }
    found
}

fn id(folder: &str) -> CleanupId {
    CleanupId {
        backend: "dev-caches".into(),
        key: format!("hugging-face:{folder}"),
    }
}

/// The task for the repository in `folder`, when nobody used it since
/// `cutoff`.
pub(super) fn item(
    host: &Host,
    folder: &str,
    cutoff: SystemTime,
    cancel: &Cancellation,
) -> Option<CleanupItem> {
    let hub = hub(host)?;
    let name = repository(folder)?;
    let root = hub.join(folder);
    if !fs::symlink_metadata(&root).is_ok_and(|metadata| metadata.is_dir()) || !records_reads(&root)
    {
        return None;
    }
    let cutoff = cutoff.min(SystemTime::now() - SHORTEST);
    let found = look(&hub, folder, cancel);
    if found.used > cutoff {
        return None;
    }
    let kind = folder.split("--").next().unwrap_or_default();
    let mut listed = vec![shown(host, &root)];
    listed.extend(found.shared.iter().map(|store| shown(host, store)));
    Some(CleanupItem {
        id: id(folder),
        kind: CleanupKind::Unused,
        title: format!("Hugging Face {} {name}", kind.trim_end_matches('s')),
        summary: format!("Not used for {}", how_long(found.used, SystemTime::now())),
        preview: format!(
            "Deletes these. Hugging Face downloads them again when something needs them.\n{}",
            listed.join("\n")
        ),
        bytes: Some(found.bytes),
    })
}

/// Every repository nobody used since `cutoff`.
pub(super) fn items(host: &Host, cutoff: SystemTime, cancel: &Cancellation) -> Vec<CleanupItem> {
    let Some(hub) = hub(host) else {
        return vec![];
    };
    let mut folders: Vec<_> = fs::read_dir(&hub)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|folder| repository(folder).is_some())
        .collect();
    folders.sort();
    folders
        .iter()
        .filter_map(|folder| item(host, folder, cutoff, cancel))
        .collect()
}

/// Delete the repository in `folder`, and the shared files nothing else
/// uses once it's gone.
pub(super) fn remove(host: &Host, folder: &str, cancel: &Cancellation) -> Result<(), EngineError> {
    let hub = hub(host).ok_or(EngineError::NotFound)?;
    let found = look(&hub, folder, cancel);
    let failed = |path: &Path, error: std::io::Error| {
        EngineError::Execution(ExecutionError::Io(format!(
            "Couldn't remove {}: {error}",
            shown(host, path)
        )))
    };
    let root = hub.join(folder);
    disk::remove(&root).map_err(|error| failed(&root, error))?;
    let _ = disk::remove(&hub.join(".locks").join(folder));
    for store in found.shared {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        // Something may have started using it since.
        if users(&hub, &store).is_some_and(|users| users.is_empty()) {
            fs::remove_file(&store).map_err(|error| failed(&store, error))?;
            let mut list = store.into_os_string();
            list.push(".refs");
            let _ = fs::remove_file(PathBuf::from(list));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Runtime;
    use std::collections::BTreeMap;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("pkgdeck-hf-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::canonicalize(root).unwrap()
    }
    fn host(home: &Path, extra: &[(&str, &Path)]) -> Host {
        let mut env = BTreeMap::from([("HOME".into(), home.as_os_str().to_owned())]);
        for (name, value) in extra {
            env.insert(name.into(), value.as_os_str().to_owned());
        }
        Host::new(Runtime::Native, env)
    }
    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![7; bytes]).unwrap();
    }
    /// Read and written `days` ago.
    fn age(path: &Path, days: u64) {
        let when = SystemTime::now() - Duration::from_secs(days * 86_400);
        let times = fs::FileTimes::new().set_accessed(when).set_modified(when);
        fs::File::open(path).unwrap().set_times(times).unwrap();
    }
    /// A repository with a file of its own and one in the shared store,
    /// listed as used by `users`.
    fn cached(hub: &Path, folder: &str, store: &str, users: &[&str], days: u64) {
        let own = hub.join(folder).join("blobs/config");
        write(&own, 1000);
        age(&own, days);
        let shared = hub.join("blobs").join(&store[..2]).join(store);
        if !shared.exists() {
            write(&shared, 64 * 1024);
        }
        age(&shared, days);
        let link = hub.join(folder).join("blobs").join(store);
        std::os::unix::fs::symlink(format!("../../blobs/{}/{store}", &store[..2]), &link).unwrap();
        let list: String = users
            .iter()
            .map(|user| format!("{user}/blobs/{store}\n"))
            .collect();
        fs::write(shared.with_extension("refs"), list).unwrap();
        fs::create_dir_all(hub.join(folder).join("refs")).unwrap();
        fs::write(hub.join(folder).join("refs/main"), "abc").unwrap();
        age(&hub.join(folder).join("refs/main"), days);
    }

    #[test]
    fn models_nobody_used_go_with_the_shared_files_only_they_use() {
        let home = scratch("unused");
        let hub = home.join(".cache/huggingface/hub");
        if !records_reads(&home) {
            // Nothing can be told on a disk that doesn't record reads.
            return;
        }
        let only = "aa".repeat(32);
        let both = "bb".repeat(32);
        cached(&hub, "models--org--old", &only, &["models--org--old"], 30);
        // A shared file both use counts for neither and stays.
        cached(
            &hub,
            "datasets--org--data",
            &both,
            &["datasets--org--data", "models--org--fresh"],
            30,
        );
        cached(
            &hub,
            "models--org--fresh",
            &both,
            &["datasets--org--data", "models--org--fresh"],
            0,
        );
        // Read lately, though written long ago.
        let fresh_read = hub.join("models--org--fresh/blobs/config");
        let long_ago = SystemTime::now() - Duration::from_secs(30 * 86_400);
        fs::File::open(&fresh_read)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(long_ago)
                    .set_accessed(SystemTime::now()),
            )
            .unwrap();
        fs::create_dir_all(hub.join("not-a-repository")).unwrap();
        let host = host(&home, &[]);
        let cancel = Cancellation::default();
        let cutoff = SystemTime::now() - Duration::from_secs(7 * 86_400);
        let found = items(&host, cutoff, &cancel);
        let keys: Vec<_> = found.iter().map(|item| item.id.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "hugging-face:datasets--org--data",
                "hugging-face:models--org--old"
            ]
        );
        let (data, old) = (&found[0], &found[1]);
        assert_eq!(data.title, "Hugging Face dataset org/data");
        assert_eq!(old.title, "Hugging Face model org/old");
        assert_eq!(old.summary, "Not used for 4 weeks");
        assert_eq!(
            old.preview,
            format!("Deletes these. Hugging Face downloads them again when something needs them.\n~/.cache/huggingface/hub/models--org--old\n~/.cache/huggingface/hub/blobs/aa/{only}")
        );
        assert!(data.preview.lines().count() == 2, "{}", data.preview);
        assert!(old.bytes.unwrap() > data.bytes.unwrap() + 32 * 1024);
        // Even a short setting waits two days.
        let soon = SystemTime::now() - Duration::from_secs(3600);
        assert!(item(&host, "models--org--fresh", soon, &cancel).is_none());
        // Removing the model takes its shared file, and the dataset's
        // shared file stays for the model that still uses it.
        remove(&host, "models--org--old", &cancel).unwrap();
        remove(&host, "datasets--org--data", &cancel).unwrap();
        assert!(!hub.join("models--org--old").exists());
        assert!(!hub.join(format!("blobs/aa/{only}")).exists());
        assert!(!hub.join(format!("blobs/aa/{only}.refs")).exists());
        assert!(hub.join(format!("blobs/bb/{both}")).exists());
        assert!(fs::canonicalize(hub.join("models--org--fresh/blobs").join(&both)).is_ok());
        assert!(items(&host, cutoff, &cancel).is_empty());
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_shared_file_without_a_list_stays() {
        let home = scratch("unlisted");
        let hub = home.join("hub");
        let store = "cc".repeat(32);
        cached(&hub, "models--org--x", &store, &["models--org--x"], 30);
        fs::remove_file(hub.join(format!("blobs/cc/{store}.refs"))).unwrap();
        let host = host(&home, &[("HF_HUB_CACHE", &hub)]);
        let cancel = Cancellation::default();
        if let Some(found) = item(&host, "models--org--x", SystemTime::now(), &cancel) {
            assert_eq!(found.preview.lines().count(), 2, "{}", found.preview);
        }
        remove(&host, "models--org--x", &cancel).unwrap();
        assert!(hub.join(format!("blobs/cc/{store}")).exists());
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn the_hub_is_where_the_library_looks() {
        let home = Path::new("/home/me");
        assert_eq!(
            hub(&host(home, &[])),
            Some(PathBuf::from("/home/me/.cache/huggingface/hub"))
        );
        assert_eq!(
            hub(&host(home, &[("XDG_CACHE_HOME", Path::new("/c"))])),
            Some(PathBuf::from("/c/huggingface/hub"))
        );
        assert_eq!(
            hub(&host(home, &[("HF_HOME", Path::new("/hf"))])),
            Some(PathBuf::from("/hf/hub"))
        );
        assert_eq!(
            hub(&host(
                home,
                &[
                    ("HF_HOME", Path::new("/hf")),
                    ("HF_HUB_CACHE", Path::new("/h"))
                ]
            )),
            Some(PathBuf::from("/h"))
        );
        assert_eq!(
            hub(&host(home, &[("HF_HUB_CACHE", Path::new("relative"))])),
            Some(PathBuf::from("/home/me/.cache/huggingface/hub"))
        );
        assert_eq!(hub(&Host::new(Runtime::Native, BTreeMap::new())), None);
        assert_eq!(repository("models--a--b"), Some("a/b".into()));
        assert_eq!(repository("spaces--a"), Some("a".into()));
        assert_eq!(repository("models--"), None);
        assert_eq!(repository("blobs"), None);
        let cancel = Cancellation::default();
        let nowhere = host(Path::new("/nonexistent/pkgdeck"), &[]);
        assert!(items(&nowhere, SystemTime::now(), &cancel).is_empty());
        assert!(item(&nowhere, "blobs", SystemTime::now(), &cancel).is_none());
        let homeless = Host::new(Runtime::Native, BTreeMap::new());
        assert!(items(&homeless, SystemTime::now(), &cancel).is_empty());
        assert!(matches!(
            remove(&homeless, "models--a--b", &cancel),
            Err(EngineError::NotFound)
        ));
    }
}
