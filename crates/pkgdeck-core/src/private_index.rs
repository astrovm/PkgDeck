//! Copies of system package indexes that PkgDeck refreshes as the user, so
//! an update check never asks for a password. Updates still come from the
//! system's own indexes, which the upgrade refreshes as root first.
//!
//! A copy is read only while it's newer than the system's index. APT and
//! pacman date each index file by the server's copy, so that compares what
//! each one holds; zypper and apk date it by the download.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
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

/// The copy's folder, ready for a refresh: APT wants its `partial`
/// folders, and pacman reads the installed packages from the system's own
/// `local` database.
pub fn prepare(source: &str) -> std::io::Result<Option<PathBuf>> {
    let Some(dir) = dir(source) else {
        return Ok(None);
    };
    prepare_in(source, &dir)?;
    Ok(Some(dir))
}
fn prepare_in(source: &str, dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    match source {
        "apt" => {
            fs::create_dir_all(dir.join("lists/partial"))?;
            fs::create_dir_all(dir.join("cache/archives/partial"))?;
        }
        "pacman" => {
            let local = dir.join("local");
            if fs::symlink_metadata(&local).is_err() {
                std::os::unix::fs::symlink("/var/lib/pacman/local", local)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The copy's folder, while it's newer than the system's index.
pub fn fresher(source: &str) -> Option<PathBuf> {
    let dir = dir(source)?;
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
    let mut found = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        if name == "partial" || name == "lock" || name == "db.lck" {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let time = if kind.is_dir() {
            depth
                .checked_sub(1)
                .filter(|depth| *depth > 0)
                .and_then(|depth| newest(&entry.path(), depth))
        } else if kind.is_file() {
            entry.metadata().and_then(|meta| meta.modified()).ok()
        } else {
            None
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
    let mut args: Vec<OsString> = vec!["-q".into()];
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
    use std::time::{Duration, UNIX_EPOCH};

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
        let var = |name: &str| match name {
            "HOME" => Some(OsString::from("/home/fixture")),
            "XDG_CACHE_HOME" => Some(OsString::from("relative")),
            _ => None,
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
        assert_eq!(dir_with("apt", var, true), None);
        assert_eq!(dir_with("dnf", var, false), None);
        assert_eq!(dir_with("apt", |_| None, false), None);
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
        prepare_in("apt", &root.join("apt")).unwrap();
        assert!(root.join("apt/lists/partial").is_dir());
        assert!(root.join("apt/cache/archives/partial").is_dir());
        prepare_in("pacman", &root.join("pacman")).unwrap();
        assert_eq!(
            fs::read_link(root.join("pacman/local")).unwrap(),
            Path::new("/var/lib/pacman/local")
        );
        // Again is fine.
        prepare_in("pacman", &root.join("pacman")).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn apt_fills_the_copy_without_appstream_data() {
        let args = apt_refresh_args(Path::new("/c/apt"));
        let text: Vec<_> = args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(text.last(), Some(&"update"));
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
