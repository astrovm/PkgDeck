//! Read-through cache for slow reads of local package databases, shared by
//! the app and `pkd`.
//!
//! An entry is stored under a fingerprint of every file and folder its
//! answer depends on (inode, size, modification and change times). Any
//! change to those, including an install made outside PkgDeck, gives a new
//! fingerprint, so an old entry can never be returned. The fingerprint is
//! taken before and after the read, and nothing is stored when it moved
//! during the read. PkgDeck's own changes also drop the source's entries.
//! Sources without a reliable fingerprint are simply not cached.
use crate::process::{Completion, ExecutionError};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// Largest entry read back; bigger answers are recomputed.
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
/// Search entries kept per source; the least recently written go first.
const MAX_SEARCHES: usize = 32;
/// Folder entries stat-ed for one fingerprint. Bigger trees are not cached.
const MAX_FINGERPRINT_ENTRIES: usize = 8192;

/// One input of a fingerprint: a file, or a folder read to `depth` levels.
#[derive(Clone, Debug)]
pub struct Watch {
    pub path: PathBuf,
    pub depth: usize,
}
impl Watch {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            depth: 0,
        }
    }
    pub fn tree(path: impl Into<PathBuf>, depth: usize) -> Self {
        Self {
            path: path.into(),
            depth,
        }
    }
}

/// A digest of the metadata of `watches` plus `extra` (such as locale or
/// helper version). `None` when a folder is too large to fingerprint.
pub fn fingerprint(watches: &[Watch], extra: &[&str]) -> Option<String> {
    let mut hash = Sha256::new();
    let mut budget = MAX_FINGERPRINT_ENTRIES;
    for watch in watches {
        visit(&watch.path, watch.depth, &mut hash, &mut budget)?;
    }
    for value in extra {
        hash.update(b"extra\0");
        hash.update(value.as_bytes());
        hash.update(b"\0");
    }
    Some(hex::encode(hash.finalize()))
}

fn visit(path: &Path, depth: usize, hash: &mut Sha256, budget: &mut usize) -> Option<()> {
    *budget = budget.checked_sub(1)?;
    hash.update(path.as_os_str().as_encoded_bytes());
    match fs::symlink_metadata(path) {
        Err(_) => hash.update(b"\0missing\0"),
        Ok(meta) => {
            let kind = if meta.is_dir() {
                "d"
            } else if meta.file_type().is_symlink() {
                "l"
            } else {
                "f"
            };
            hash.update(
                format!(
                    "\0{kind}:{}:{}:{}:{}.{}:{}.{}:{}\0",
                    meta.dev(),
                    meta.ino(),
                    meta.len(),
                    meta.mtime(),
                    meta.mtime_nsec(),
                    meta.ctime(),
                    meta.ctime_nsec(),
                    meta.mode()
                )
                .as_bytes(),
            );
            if kind == "l" {
                if let Ok(target) = fs::read_link(path) {
                    hash.update(target.as_os_str().as_encoded_bytes());
                }
            }
            if meta.is_dir() && depth > 0 {
                let mut names: Vec<_> = fs::read_dir(path)
                    .ok()?
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name())
                    .collect();
                if names.len() > *budget {
                    return None;
                }
                names.sort();
                for name in names {
                    visit(&path.join(name), depth - 1, hash, budget)?;
                }
            }
        }
    }
    Some(())
}

/// A cache folder owned by the current user.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}
impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    /// `$XDG_CACHE_HOME/pkgdeck`, else `~/.cache/pkgdeck`. `None` when
    /// `PKGDECK_NO_CACHE` is set, or when running as root, where a cache in
    /// someone else's home could lock them out of it.
    pub fn user() -> Option<Self> {
        if std::env::var_os("PKGDECK_NO_CACHE").is_some() || rustix::process::geteuid().is_root() {
            return None;
        }
        let base = std::env::var_os("XDG_CACHE_HOME")
            .filter(|p| Path::new(p).is_absolute())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
        Some(Self::new(base.join("pkgdeck")))
    }

    fn file(&self, source: &str, kind: &str, args: &[&str]) -> PathBuf {
        let mut hash = Sha256::new();
        for arg in args {
            hash.update(arg.as_bytes());
            hash.update(b"\0");
        }
        let digest = hex::encode(hash.finalize());
        self.dir
            .join(format!("{source}-{kind}-{}.cache", &digest[..24]))
    }

    /// The stored answer for exactly this `key`, if any.
    pub fn get(&self, source: &str, kind: &str, args: &[&str], key: &str) -> Option<Vec<u8>> {
        let file = fs::File::open(self.file(source, kind, args)).ok()?;
        let mut bytes = Vec::new();
        file.take(MAX_ENTRY_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > MAX_ENTRY_BYTES {
            return None;
        }
        let split = bytes.iter().position(|b| *b == b'\n')?;
        (header(key).as_bytes() == &bytes[..split]).then(|| bytes[split + 1..].to_vec())
    }

    /// Best effort: a cache that can't be written is just not used.
    pub fn put(&self, source: &str, kind: &str, args: &[&str], key: &str, value: &[u8]) {
        let _ = self.write(source, kind, args, key, value);
        if kind == "search" {
            self.prune(&format!("{source}-{kind}-"), MAX_SEARCHES);
        }
    }

    fn write(
        &self,
        source: &str,
        kind: &str,
        args: &[&str],
        key: &str,
        value: &[u8],
    ) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700))?;
        let target = self.file(source, kind, args);
        let temporary = target.with_extension(format!("{}.tmp", std::process::id()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let written = file
            .write_all(header(key).as_bytes())
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.write_all(value))
            .and_then(|()| fs::rename(&temporary, &target));
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        written
    }

    fn entries(&self, prefix: &str) -> Vec<PathBuf> {
        fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".cache"))
            })
            .collect()
    }

    fn prune(&self, prefix: &str, keep: usize) {
        let mut entries: Vec<_> = self
            .entries(prefix)
            .into_iter()
            .map(|path| {
                let modified = fs::metadata(&path).and_then(|meta| meta.modified()).ok();
                (modified, path)
            })
            .collect();
        if entries.len() <= keep {
            return;
        }
        entries.sort();
        for (_, path) in &entries[..entries.len() - keep] {
            let _ = fs::remove_file(path);
        }
    }

    /// Drop every entry of one source, for example after PkgDeck changed it.
    pub fn invalidate(&self, source: &str) {
        for path in self.entries(&format!("{source}-")) {
            let _ = fs::remove_file(path);
        }
    }
}

fn header(key: &str) -> String {
    format!("pkgdeck-cache 1 {} {key}", crate::VERSION)
}

/// Return the stored answer for `watches`, or run `read` and store what it
/// returns when the watched files did not change while it ran. Without a
/// store or a fingerprint, `read` just runs.
pub fn read_through<E>(
    store: Option<&Store>,
    source: &str,
    kind: &str,
    args: &[&str],
    watches: &[Watch],
    extra: &[&str],
    read: impl FnOnce() -> Result<Vec<u8>, E>,
) -> Result<Vec<u8>, E> {
    let Some(store) = store else {
        return read();
    };
    let Some(before) = fingerprint(watches, extra) else {
        return read();
    };
    if let Some(value) = store.get(source, kind, args, &before) {
        return Ok(value);
    }
    let value = read()?;
    if fingerprint(watches, extra).as_deref() == Some(before.as_str()) {
        store.put(source, kind, args, &before, &value);
    }
    Ok(value)
}

/// [`read_through`] for a native command: only a clean, complete exit is
/// stored, and a hit comes back as that exit with the stored stdout. Failed
/// or truncated runs pass through unchanged for the adapter to report.
pub fn completion(
    store: Option<&Store>,
    source: &str,
    kind: &str,
    args: &[&str],
    watches: &[Watch],
    extra: &[&str],
    run: impl FnOnce() -> Result<Completion, ExecutionError>,
) -> Result<Completion, ExecutionError> {
    let mut fresh = None;
    let stdout = read_through(store, source, kind, args, watches, extra, || {
        let result = run().map_err(Err)?;
        if result.code != Some(0) || result.truncated {
            return Err(Ok(result));
        }
        let stdout = result.stdout.clone();
        fresh = Some(result);
        Ok(stdout)
    });
    match stdout {
        Ok(stdout) => Ok(fresh.unwrap_or(Completion {
            code: Some(0),
            signal: None,
            stdout,
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
        })),
        Err(Ok(result)) => Ok(result),
        Err(Err(error)) => Err(error),
    }
}

/// Drop a source's cached answers in the user's cache after a change.
pub fn invalidate(source: &str) {
    if let Some(store) = Store::user() {
        store.invalidate(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pkgdeck-cache-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn answers_are_reused_until_a_watched_file_changes() {
        let root = scratch("reuse");
        let status = root.join("status");
        fs::write(&status, "one").unwrap();
        let store = Store::new(root.join("store"));
        let watches = [Watch::file(&status), Watch::tree(root.join("lists"), 1)];
        let runs = Cell::new(0);
        let read = |answer: &'static str| {
            runs.set(runs.get() + 1);
            Ok::<_, ()>(answer.as_bytes().to_vec())
        };
        let get = |answer| {
            read_through(
                Some(&store),
                "apt",
                "installed",
                &[],
                &watches,
                &["C"],
                || read(answer),
            )
            .unwrap()
        };
        assert_eq!(get("first"), b"first");
        assert_eq!(get("second"), b"first");
        assert_eq!(runs.get(), 1);
        // An outside install rewrites the database: never answer from before.
        fs::write(&status, "two, longer").unwrap();
        assert_eq!(get("third"), b"third");
        // A new folder entry changes the fingerprint too.
        fs::create_dir_all(root.join("lists")).unwrap();
        fs::write(root.join("lists/Packages"), "x").unwrap();
        assert_eq!(get("fourth"), b"fourth");
        // Other arguments and extra inputs are separate entries.
        let other = read_through(
            Some(&store),
            "apt",
            "installed",
            &[],
            &watches,
            &["de"],
            || read("fifth"),
        )
        .unwrap();
        assert_eq!(other, b"fifth");
        // PkgDeck's own changes drop the source.
        store.invalidate("apt");
        assert_eq!(get("sixth"), b"sixth");
        assert_eq!(runs.get(), 5);
        // Entries are private to the user.
        let mode = fs::metadata(root.join("store"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failures_changes_during_a_read_and_missing_stores_are_not_cached() {
        let root = scratch("failures");
        let status = root.join("status");
        fs::write(&status, "one").unwrap();
        let store = Store::new(root.join("store"));
        let watches = [Watch::file(&status)];
        let failed: Result<Vec<u8>, &str> =
            read_through(Some(&store), "apt", "installed", &[], &watches, &[], || {
                Err("broken")
            });
        assert_eq!(failed, Err("broken"));
        assert!(store.entries("apt-").is_empty());
        // The database changed while it was read: the answer isn't stored.
        let moving = read_through(Some(&store), "apt", "installed", &[], &watches, &[], || {
            fs::write(&status, "changed meanwhile").unwrap();
            Ok::<_, ()>(b"old".to_vec())
        });
        assert_eq!(moving, Ok(b"old".to_vec()));
        assert!(store.entries("apt-").is_empty());
        assert_eq!(
            read_through(
                None,
                "apt",
                "installed",
                &[],
                &watches,
                &[],
                || Ok::<_, ()>(b"plain".to_vec())
            ),
            Ok(b"plain".to_vec())
        );
        // A store that can't be created is ignored.
        let blocked = Store::new(status.join("not-a-folder"));
        assert_eq!(
            read_through(
                Some(&blocked),
                "apt",
                "installed",
                &[],
                &watches,
                &[],
                || Ok::<_, ()>(b"still works".to_vec())
            ),
            Ok(b"still works".to_vec())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_clean_complete_command_exits_are_stored() {
        let root = scratch("completion");
        let status = root.join("status");
        fs::write(&status, "one").unwrap();
        let store = Store::new(root.join("store"));
        let watches = [Watch::file(&status)];
        let exit = |code: i32, truncated: bool, stdout: &str| Completion {
            code: Some(code),
            signal: None,
            stdout: stdout.as_bytes().to_vec(),
            stderr: b"diagnostic".to_vec(),
            truncated,
            cancellation_deferred: false,
        };
        let run = |result: Result<Completion, ExecutionError>| {
            completion(
                Some(&store),
                "flatpak",
                "search",
                &["vlc"],
                &watches,
                &[],
                || result,
            )
        };
        let failed = run(Ok(exit(1, false, "partial"))).unwrap();
        assert_eq!(failed.code, Some(1));
        let truncated = run(Ok(exit(0, true, "cut"))).unwrap();
        assert!(truncated.truncated);
        assert_eq!(
            run(Err(ExecutionError::TimedOut)),
            Err(ExecutionError::TimedOut)
        );
        assert!(store.entries("flatpak-").is_empty());
        let fresh = run(Ok(exit(0, false, "rows"))).unwrap();
        assert_eq!(fresh.stderr, b"diagnostic");
        let hit = run(Err(ExecutionError::TimedOut)).unwrap();
        assert_eq!((hit.code, hit.stdout.as_slice()), (Some(0), &b"rows"[..]));
        assert!(hit.stderr.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_or_foreign_entries_are_misses_and_searches_are_bounded() {
        let root = scratch("bounded");
        let store = Store::new(root.join("store"));
        store.put("apt", "installed", &[], "key", b"value");
        assert_eq!(store.get("apt", "installed", &[], "key").unwrap(), b"value");
        assert!(store.get("apt", "installed", &[], "other").is_none());
        fs::write(store.file("apt", "installed", &[]), b"no header").unwrap();
        assert!(store.get("apt", "installed", &[], "key").is_none());
        for index in 0..(MAX_SEARCHES + 5) {
            let query = index.to_string();
            store.put("apt", "search", &[&query], "key", b"hit");
        }
        assert_eq!(store.entries("apt-search-").len(), MAX_SEARCHES);
        assert_eq!(store.entries("apt-installed-").len(), 1);
        store.invalidate("flatpak");
        assert_eq!(store.entries("apt-").len(), MAX_SEARCHES + 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fingerprints_cover_symlinks_and_refuse_huge_trees() {
        let root = scratch("fingerprint");
        fs::create_dir_all(root.join("appstream/remote/x86_64/one")).unwrap();
        fs::create_dir_all(root.join("appstream/remote/x86_64/two")).unwrap();
        let active = root.join("appstream/remote/x86_64/active");
        std::os::unix::fs::symlink("one", &active).unwrap();
        let watches = [Watch::tree(root.join("appstream"), 3)];
        let first = fingerprint(&watches, &[]).unwrap();
        fs::remove_file(&active).unwrap();
        std::os::unix::fs::symlink("two", &active).unwrap();
        assert_ne!(fingerprint(&watches, &[]).unwrap(), first);
        assert_ne!(
            fingerprint(&watches, &["a"]).unwrap(),
            fingerprint(&watches, &["b"]).unwrap()
        );
        let big = root.join("big");
        fs::create_dir_all(&big).unwrap();
        for index in 0..=MAX_FINGERPRINT_ENTRIES {
            fs::write(big.join(index.to_string()), "").unwrap();
        }
        assert!(fingerprint(&[Watch::tree(&big, 1)], &[]).is_none());
        // A missing path is still a stable input.
        let missing = [Watch::file(root.join("absent"))];
        assert_eq!(fingerprint(&missing, &[]), fingerprint(&missing, &[]));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_user_store_follows_xdg_and_can_be_turned_off() {
        // Only checks the resolution rules; the environment is restored.
        let store = Store::user();
        if rustix::process::geteuid().is_root() {
            assert!(store.is_none());
        } else if std::env::var_os("PKGDECK_NO_CACHE").is_none() {
            assert!(store.is_some_and(|store| store.dir.ends_with("pkgdeck")));
        }
        invalidate("pkgdeck-test-source-that-never-exists");
    }
}
