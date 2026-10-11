//! Disk usage and removal for cleanup tasks that delete files themselves,
//! such as caches and build folders that no manager command clears.
use crate::process::Cancellation;
use std::{
    collections::BTreeSet,
    fs, io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

/// How much space `path` takes on disk. Symbolic links are never followed,
/// a hard-linked file counts once, and a cancelled walk stops early with
/// what it counted so far.
pub fn usage(path: &Path, cancel: &Cancellation) -> u64 {
    let mut seen = BTreeSet::new();
    let mut total = 0;
    let mut pending = vec![path.to_owned()];
    while let Some(path) = pending.pop() {
        if cancel.requested() {
            break;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.nlink() > 1
            && !metadata.is_dir()
            && !seen.insert((metadata.dev(), metadata.ino()))
        {
            continue;
        }
        total += metadata.blocks() * 512;
        if metadata.is_dir() {
            pending.extend(
                fs::read_dir(&path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|entry| entry.path()),
            );
        }
    }
    total
}

/// Remove `path` and everything in it, without following symbolic links.
/// Read-only folders, such as Go's module cache, are made writable first.
pub fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            writable(path);
            fs::remove_dir_all(path)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotADirectory => fs::remove_file(path),
        other => other,
    }
}

/// Remove everything in the folder `dir` but keep the folder itself, with
/// its owner, permissions and any link to it.
pub fn empty(dir: &Path) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        entries => entries?,
    };
    for entry in entries {
        remove(&entry?.path())?;
    }
    Ok(())
}

/// Let the owner change every folder under `path`, so its files can go.
fn writable(path: &Path) {
    let mut pending = vec![path.to_owned()];
    while let Some(path) = pending.pop() {
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let mode = metadata.permissions().mode();
        if mode & 0o700 != 0o700 {
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(mode | 0o700));
        }
        pending.extend(
            fs::read_dir(&path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path()),
        );
    }
}

/// Bytes in base 1000, as people read sizes: "820 bytes", "4.2 MB",
/// "31 MB".
pub fn size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64;
    let mut unit = "bytes";
    for next in ["KB", "MB", "GB", "TB"] {
        if value < 1000.0 {
            break;
        }
        value /= 1000.0;
        unit = next;
    }
    if value < 10.0 {
        format!("{value:.1} {unit}")
    } else {
        format!("{value:.0} {unit}")
    }
}

/// A size as tools print it: `1.2GB`, `512.3 MB`, `4kB`, `0B`, `1.5GiB`.
/// Decimal units count in thousands and binary ones (`KiB`, `MiB`) in
/// 1024s, except that a bare `K`, `M` or `G` is binary, as `du` prints them.
pub fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: f64 = number.parse().ok()?;
    let scale: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" | "bytes" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "k" | "kib" => 1024.0,
        "m" | "mib" => 1024.0 * 1024.0,
        "g" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((number * scale).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn scratch(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("pkgdeck-disk-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn usage_counts_files_once_and_never_follows_links() {
        let root = scratch("usage");
        let outside = scratch("usage-outside");
        fs::write(outside.join("big"), vec![1; 256 * 1024]).unwrap();
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/file"), vec![1; 64 * 1024]).unwrap();
        let alone = usage(&root, &Cancellation::default());
        assert!(alone >= 64 * 1024, "{alone}");
        // A hard link to the same file and a link out of the folder add
        // nothing worth counting.
        fs::hard_link(root.join("dir/file"), root.join("again")).unwrap();
        symlink(&outside, root.join("link")).unwrap();
        let linked = usage(&root, &Cancellation::default());
        assert!(linked < alone + 64 * 1024, "{alone} {linked}");
        // The linked file still counts, once.
        assert!(linked >= alone, "{alone} {linked}");
        assert_eq!(usage(&root.join("missing"), &Cancellation::default()), 0);
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert_eq!(usage(&root, &cancelled), 0);
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn emptying_keeps_the_folder_reaches_read_only_ones_and_spares_link_targets() {
        let root = scratch("empty");
        let outside = scratch("empty-outside");
        fs::write(outside.join("keep"), "mine").unwrap();
        // Go's module cache: read-only folders of read-only files.
        let module = root.join("mod/example.com@v1");
        fs::create_dir_all(&module).unwrap();
        fs::write(module.join("go.mod"), "module example.com").unwrap();
        fs::set_permissions(module.join("go.mod"), fs::Permissions::from_mode(0o444)).unwrap();
        fs::set_permissions(&module, fs::Permissions::from_mode(0o555)).unwrap();
        fs::write(root.join("file"), "cached").unwrap();
        symlink(&outside, root.join("link")).unwrap();
        empty(&root).unwrap();
        assert!(root.is_dir());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "mine");
        // Gone already, or a single file.
        empty(&root.join("missing")).unwrap();
        remove(&root.join("missing")).unwrap();
        fs::write(root.join("single"), "x").unwrap();
        remove(&root.join("single")).unwrap();
        assert!(!root.join("single").exists());
        // A file where a folder should be can't be emptied.
        assert!(empty(&outside.join("keep")).is_err());
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn sizes_read_as_tools_print_them() {
        for (text, bytes) in [
            ("0B", 0),
            ("512", 512),
            ("4kB", 4_000),
            ("1.2GB", 1_200_000_000),
            ("512.3 MB", 512_300_000),
            ("3 TB", 3_000_000_000_000),
            ("1.5GiB", 1_610_612_736),
            ("2KiB", 2_048),
            ("1MiB", 1_048_576),
            ("1TiB", 1_099_511_627_776),
            ("12K", 12_288),
            ("3M", 3_145_728),
            ("1G", 1_073_741_824),
            ("2T", 2_199_023_255_552),
            (" 10 bytes ", 10),
        ] {
            assert_eq!(parse_size(text), Some(bytes), "{text}");
        }
        for text in ["", "GB", "1.2 parsecs", "-1B", "1.2.3MB"] {
            assert_eq!(parse_size(text), None, "{text}");
        }
    }
}
