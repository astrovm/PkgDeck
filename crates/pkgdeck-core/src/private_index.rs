//! Copies of system package indexes that PkgDeck refreshes as the user, so
//! an update check never asks for a password. Updates still come from the
//! system's own indexes, which the upgrade refreshes as root first.
//!
//! A copy is read only while it's newer than the system's index. APT and
//! pacman date each index file by the server's copy, so that compares what
//! each one holds; zypper and apk date it by the download.

use crate::process::{Cancellation, ExecutionError};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

/// Where the copy lives and what it's compared with: the system folder and
/// how deep its index files sit.
fn layout(source: &str) -> Option<(&'static str, &'static str, usize)> {
    Some(match source {
        // Copy folder, system folder, depth.
        "apt" => ("lists", "/var/lib/apt/lists", 1),
        "pacman" => ("sync", "/var/lib/pacman/sync", 1),
        "zypper" => ("raw", "/var/cache/zypp/raw", 3),
        "apk" => (".", "/var/cache/apk", 1),
        _ => return None,
    })
}

/// `$XDG_CACHE_HOME/pkgdeck/indexes/<source>`, else under `~/.cache`.
/// `None` as root, which refreshes the system's own indexes.
pub fn dir(source: &str) -> Option<PathBuf> {
    dir_with(
        source,
        |name| std::env::var_os(name),
        rustix::process::geteuid().is_root(),
    )
}
fn dir_with(source: &str, var: impl Fn(&str) -> Option<OsString>, root: bool) -> Option<PathBuf> {
    layout(source)?;
    if root {
        return None;
    }
    let base = var("XDG_CACHE_HOME")
        .filter(|p| Path::new(p).is_absolute())
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(base.join("pkgdeck/indexes").join(source))
}

/// A copy ready for a refresh. Only one check fills it at a time: APT's
/// runs without APT's own lock, and two at once could mix their files.
pub struct Copy {
    pub dir: PathBuf,
    _lock: Option<fs::File>,
}
impl Copy {
    /// A copy nobody else fills, for fixtures.
    pub fn unlocked(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            _lock: None,
        }
    }
}

/// The copy's folder, ready for a refresh: APT wants its `partial`
/// folders and its own settings, and pacman reads the installed packages
/// from the system's own `local` database. Waits while another check
/// fills it.
pub fn prepare(source: &str, cancel: &Cancellation) -> Result<Option<Copy>, ExecutionError> {
    dir(source)
        .map(|dir| prepare_in(source, &dir, cancel))
        .transpose()
}
fn prepare_in(source: &str, dir: &Path, cancel: &Cancellation) -> Result<Copy, ExecutionError> {
    let io = |error: std::io::Error| ExecutionError::Io(format!("{}: {error}", dir.display()));
    fs::create_dir_all(dir).map_err(io)?;
    let lock = lock(&dir.join("lock"), cancel)?;
    match source {
        "apt" => {
            fs::create_dir_all(dir.join("lists/partial")).map_err(io)?;
            fs::create_dir_all(dir.join("cache/archives/partial")).map_err(io)?;
            fs::write(dir.join("apt.conf"), APT_CONF).map_err(io)?;
        }
        "pacman" => {
            let local = dir.join("local");
            if fs::symlink_metadata(&local).is_err() {
                std::os::unix::fs::symlink("/var/lib/pacman/local", local).map_err(io)?;
            }
        }
        _ => {}
    }
    Ok(Copy {
        dir: dir.to_owned(),
        _lock: Some(lock),
    })
}

/// Hold `path`'s lock, waiting for whoever holds it now.
fn lock(path: &Path, cancel: &Cancellation) -> Result<fs::File, ExecutionError> {
    let io = |error: std::io::Error| ExecutionError::Io(format!("{}: {error}", path.display()));
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(io)?;
    loop {
        match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Err(rustix::io::Errno::WOULDBLOCK) if !cancel.requested() => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(rustix::io::Errno::WOULDBLOCK) => return Err(ExecutionError::Cancelled),
            locked => return locked.map(|()| file).map_err(|error| io(error.into())),
        }
    }
}

/// The hooks that run around `apt-get update` maintain the system's own
/// lists and stamps. Run as the user for a copy, they'd fail or do nothing.
const APT_CONF: &str = "#clear APT::Update::Pre-Invoke;
#clear APT::Update::Post-Invoke;
#clear APT::Update::Post-Invoke-Success;
";

/// The copy's folder, while it's newer than the system's index.
pub fn fresher(source: &str) -> Option<PathBuf> {
    dir(source).and_then(|dir| fresher_at(source, dir))
}
fn fresher_at(source: &str, dir: PathBuf) -> Option<PathBuf> {
    let (_, system, _) = layout(source)?;
    fresher_than(source, &dir, Path::new(system)).then_some(dir)
}
fn fresher_than(source: &str, dir: &Path, system: &Path) -> bool {
    let Some((copy, _, depth)) = layout(source) else {
        return false;
    };
    let Some(copy) = newest(&dir.join(copy), depth) else {
        return false;
    };
    newest(system, depth).is_none_or(|system| copy > system)
}

/// The newest index file under `dir`, `depth` folders down at most.
/// Downloads in progress and locks don't count.
fn newest(dir: &Path, depth: usize) -> Option<SystemTime> {
    let skipped = |name: &std::ffi::OsStr| {
        ["partial", "lock", "db.lck"]
            .iter()
            .any(|skip| name == *skip)
    };
    let mut found = None;
    for path in fs::read_dir(dir).ok()?.flatten().map(|entry| entry.path()) {
        let time = if path.file_name().is_some_and(skipped) {
            None
        } else if path.is_dir() {
            Some(depth - 1)
                .filter(|depth| *depth > 0)
                .and_then(|depth| newest(&path, depth))
        } else {
            fs::metadata(&path).and_then(|meta| meta.modified()).ok()
        };
        found = found.max(time);
    }
    found
}

/// What `apt-get update` needs to fill the copy at `dir` as the user.
/// AppStream data and command lists are left out: PkgDeck reads those
/// from the system, and they're most of the download. Lists stay
/// compressed and no cache files are saved, which keeps it near 80 MB.
pub fn apt_refresh_args(dir: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "-q".into(),
        "-c".into(),
        dir.join("apt.conf").into_os_string(),
    ];
    let options = [
        format!("Dir::State::Lists={}", dir.join("lists").display()),
        format!("Dir::Cache={}", dir.join("cache").display()),
        "Dir::Cache::pkgcache=".into(),
        "Dir::Cache::srcpkgcache=".into(),
        "Acquire::GzipIndexes=true".into(),
        "Debug::NoLocking=1".into(),
        "Acquire::Languages=en".into(),
    ]
    .into_iter()
    .chain(
        [
            "DEP-11",
            "DEP-11-icons-small",
            "DEP-11-icons",
            "DEP-11-icons-large",
            "DEP-11-icons-hidpi",
            "DEP-11-icons-large-hidpi",
            "CNF",
        ]
        .map(|target| format!("Acquire::IndexTargets::deb::{target}::DefaultEnabled=false")),
    );
    for option in options {
        args.push("-o".into());
        args.push(option.into());
    }
    args.push("update".into());
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pkgdeck-private-index-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn file(path: &Path, seconds: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "index").unwrap();
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
    }

    #[test]
    fn copies_live_in_the_users_cache_and_never_for_root() {
        let var = |name: &str| {
            [("HOME", "/home/fixture"), ("XDG_CACHE_HOME", "relative")]
                .into_iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        };
        assert_eq!(
            dir_with("apt", var, false),
            Some(PathBuf::from("/home/fixture/.cache/pkgdeck/indexes/apt"))
        );
        let xdg = |name: &str| (name == "XDG_CACHE_HOME").then(|| OsString::from("/x"));
        assert_eq!(
            dir_with("pacman", xdg, false),
            Some(PathBuf::from("/x/pkgdeck/indexes/pacman"))
        );
        for source in ["zypper", "apk"] {
            assert_eq!(
                dir_with(source, xdg, false),
                Some(PathBuf::from(format!("/x/pkgdeck/indexes/{source}")))
            );
        }
        assert_eq!(dir_with("apt", var, true), None);
        assert_eq!(dir_with("dnf", var, false), None);
        assert_eq!(dir_with("apt", |_| None, false), None);
    }

    #[test]
    fn files_deeper_than_the_index_are_not_looked_at() {
        let root = scratch("depth");
        let (copy, system) = (root.join("copy"), root.join("system"));
        file(&system.join("a_InRelease"), 100);
        // APT's lists sit right in the folder, so this one isn't an index.
        file(&copy.join("lists/nested/deeper/a_InRelease"), 200);
        assert!(!fresher_than("apt", &copy, &system));
        file(&copy.join("lists/a_InRelease"), 150);
        assert!(fresher_than("apt", &copy, &system));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_copy_counts_only_while_it_is_newer_than_the_system() {
        let root = scratch("fresher");
        let (copy, system) = (root.join("copy"), root.join("system"));
        // Nothing copied yet.
        assert!(!fresher_than("apt", &copy, &system));
        file(&copy.join("lists/a_InRelease"), 100);
        // The system has no index this folder can read.
        assert!(fresher_than("apt", &copy, &system));
        file(&system.join("a_InRelease"), 100);
        assert!(!fresher_than("apt", &copy, &system), "same index");
        file(&copy.join("lists/b_Packages"), 200);
        assert!(fresher_than("apt", &copy, &system));
        // Downloads in progress and locks don't count.
        file(&system.join("partial/c_Packages"), 300);
        file(&system.join("lock"), 300);
        assert!(fresher_than("apt", &copy, &system));
        file(&system.join("b_Packages"), 300);
        assert!(!fresher_than("apt", &copy, &system));
        // zypper's files sit three folders down.
        file(&copy.join("raw/repo-oss/repodata/repomd.xml"), 400);
        file(&system.join("repo-oss/repodata/repomd.xml"), 350);
        assert!(fresher_than("zypper", &copy, &system));
        assert!(!fresher_than("dnf", &copy, &system));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preparing_a_copy_makes_what_its_manager_expects() {
        let root = scratch("prepare");
        let cancel = Cancellation::default();
        let apt = prepare_in("apt", &root.join("apt"), &cancel).unwrap();
        assert_eq!(apt.dir, root.join("apt"));
        assert!(root.join("apt/lists/partial").is_dir());
        assert!(root.join("apt/cache/archives/partial").is_dir());
        assert_eq!(
            fs::read_to_string(root.join("apt/apt.conf")).unwrap(),
            APT_CONF
        );
        drop(apt);
        drop(prepare_in("pacman", &root.join("pacman"), &cancel).unwrap());
        assert_eq!(
            fs::read_link(root.join("pacman/local")).unwrap(),
            Path::new("/var/lib/pacman/local")
        );
        // Again is fine.
        drop(prepare_in("pacman", &root.join("pacman"), &cancel).unwrap());
        // The lock file never counts as an index, even in apk's own folder.
        drop(prepare_in("apk", &root.join("apk"), &cancel).unwrap());
        assert!(root.join("apk/lock").is_file());
        assert!(!fresher_than("apk", &root.join("apk"), &root.join("none")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copies_are_found_only_for_managers_that_keep_one() {
        let cancel = Cancellation::default();
        assert!(prepare("dnf", &cancel).unwrap().is_none());
        assert_eq!(fresher("dnf"), None);
        let root = scratch("fresher-at");
        // Nothing copied yet, so the system's lists win.
        assert_eq!(fresher_at("apt", root.join("apt")), None);
        assert_eq!(fresher_at("dnf", root.join("dnf")), None);
        file(&root.join("pacman/sync/core.db"), u64::from(u32::MAX));
        assert_eq!(
            fresher_at("pacman", root.join("pacman")),
            Some(root.join("pacman"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_copy_that_cannot_be_written_is_an_error_naming_it() {
        let root = scratch("readonly");
        // A file where the folder should be.
        fs::write(root.join("apt"), "").unwrap();
        let error = prepare_in("apt", &root.join("apt"), &Cancellation::default())
            .err()
            .unwrap();
        assert!(
            matches!(&error, ExecutionError::Io(reason) if reason.contains("apt")),
            "{error:?}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn one_check_at_a_time_fills_a_copy() {
        let root = scratch("lock");
        let cancel = Cancellation::default();
        let first = prepare_in("apt", &root.join("apt"), &cancel).unwrap();
        let (sender, waited) = std::sync::mpsc::channel();
        let dir = root.join("apt");
        let second = std::thread::spawn(move || {
            let copy = prepare_in("apt", &dir, &Cancellation::default()).unwrap();
            sender.send(()).unwrap();
            copy
        });
        // The second waits while the first holds it.
        assert!(waited.recv_timeout(Duration::from_millis(400)).is_err());
        drop(first);
        waited.recv_timeout(Duration::from_secs(5)).unwrap();
        let second = second.join().unwrap();
        // A cancelled check stops waiting.
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert_eq!(
            prepare_in("apt", &root.join("apt"), &cancelled).err(),
            Some(ExecutionError::Cancelled)
        );
        drop(second);
        // A fixture's copy holds nothing.
        assert!(Copy::unlocked("/c")._lock.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    /// Generated cases: a copy counts exactly when its newest index file is
    /// newer than the system's newest, wherever in the tree each one sits.
    #[test]
    fn freshness_follows_the_newest_index_on_each_side() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move |below: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % below
        };
        let root = scratch("generated");
        for case in 0..200 {
            let (copy, system) = (
                root.join(format!("{case}/copy")),
                root.join(format!("{case}/system")),
            );
            let mut newest = [None, None];
            for (side, (base, depth)) in [(copy.join("raw"), 3), (system.clone(), 3)]
                .into_iter()
                .enumerate()
            {
                for index in 0..next(4) {
                    let level = next(depth as u64) as usize;
                    let mut path = base.clone();
                    for folder in 0..level {
                        path.push(format!("d{folder}"));
                    }
                    path.push(format!("f{index}"));
                    let time = 1 + next(5);
                    file(&path, time);
                    newest[side] = newest[side].max(Some(time));
                }
            }
            let [copy_newest, system_newest] = newest;
            let expected =
                copy_newest.is_some_and(|copy| system_newest.is_none_or(|system| copy > system));
            assert_eq!(
                fresher_than("zypper", &copy, &system),
                expected,
                "case {case}: {newest:?}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn apt_fills_the_copy_without_appstream_data() {
        let args = apt_refresh_args(Path::new("/c/apt"));
        let text: Vec<_> = args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(text.last(), Some(&"update"));
        // Its settings turn off the system's update hooks.
        assert_eq!(text[1..3], ["-c", "/c/apt/apt.conf"]);
        assert!(text.contains(&"Dir::State::Lists=/c/apt/lists"));
        assert!(text.contains(&"Dir::Cache=/c/apt/cache"));
        assert!(text.contains(&"Debug::NoLocking=1"));
        assert!(text.contains(&"Acquire::IndexTargets::deb::DEP-11::DefaultEnabled=false"));
        assert!(text.contains(&"Acquire::IndexTargets::deb::CNF::DefaultEnabled=false"));
        assert!(text.contains(&"Dir::Cache::pkgcache="));
        assert!(text.contains(&"Acquire::GzipIndexes=true"));
        // Every option follows its -o.
        for (index, arg) in text.iter().enumerate() {
            if arg.contains('=') {
                assert_eq!(text[index - 1], "-o");
            }
        }
    }
}
