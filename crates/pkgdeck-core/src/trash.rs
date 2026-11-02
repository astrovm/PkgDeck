//! The freedesktop.org Trash, where file managers list what was moved there
//! and can put it back (https://specifications.freedesktop.org/trash-spec/).
use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

/// A Trash folder: what was moved is in `files`, and where it came from in
/// `info`.
pub struct Trash {
    root: PathBuf,
}

impl Trash {
    /// The home Trash: `$XDG_DATA_HOME/Trash`, or `~/.local/share/Trash`.
    /// Relative values are ignored, as the base directory spec says.
    pub fn home(home: Option<&Path>, data_home: Option<&Path>) -> Option<Self> {
        let data = data_home
            .filter(|path| path.is_absolute())
            .map(Path::to_owned)
            .or_else(|| {
                home.filter(|path| path.is_absolute())
                    .map(|home| home.join(".local/share"))
            })?;
        Some(Self::at(data.join("Trash")))
    }

    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// Moves `path` into the Trash under a name nothing in it has yet, and
    /// returns where it went. The Trash must be on the same disk: a move to
    /// another one would copy everything and isn't attempted.
    pub fn put(&self, path: &Path) -> io::Result<PathBuf> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "only an absolute path can go to the Trash",
            ));
        }
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "nothing to name in the Trash")
        })?;
        fs::symlink_metadata(path)?;
        let (files, info) = (self.root.join("files"), self.root.join("info"));
        for folder in [&files, &info] {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(folder)?;
        }
        let entry = format!(
            "[Trash Info]\nPath={}\nDeletionDate={}\n",
            escaped(path),
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S")
        );
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let mut chosen = name.to_owned();
            if attempt > 1 {
                chosen.push(format!(".{attempt}"));
            }
            let target = files.join(&chosen);
            let mut record = chosen;
            record.push(".trashinfo");
            let record = info.join(record);
            // Creating the info file claims the name, as the spec asks.
            let created = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&record);
            match created {
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
                Ok(_) if fs::symlink_metadata(&target).is_ok() => {
                    // A file left without its info: keep it, try another name.
                    fs::remove_file(&record)?;
                    continue;
                }
                Ok(mut file) => {
                    let moved = io::Write::write_all(&mut file, entry.as_bytes())
                        .and_then(|()| fs::rename(path, &target));
                    if let Err(error) = moved {
                        let _ = fs::remove_file(&record);
                        return Err(error);
                    }
                    return Ok(target);
                }
            }
        }
    }
}

/// The path as the spec stores it: URL-escaped, slashes kept.
fn escaped(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut text = String::new();
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
            text.push(byte as char);
        } else {
            text.push_str(&format!("%{byte:02X}"));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("pkgdeck-trash-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn the_home_trash_follows_the_data_folder() {
        let home = Path::new("/home/fixture");
        assert_eq!(
            Trash::home(Some(home), None).unwrap().root,
            Path::new("/home/fixture/.local/share/Trash")
        );
        assert_eq!(
            Trash::home(Some(home), Some(Path::new("/data")))
                .unwrap()
                .root,
            Path::new("/data/Trash")
        );
        // A relative data folder doesn't count; without a home there's none.
        assert_eq!(
            Trash::home(Some(home), Some(Path::new("data")))
                .unwrap()
                .root,
            Path::new("/home/fixture/.local/share/Trash")
        );
        assert!(Trash::home(None, None).is_none());
        assert!(Trash::home(Some(Path::new("home")), None).is_none());
    }

    #[test]
    fn a_folder_moves_with_a_record_of_where_it_was() {
        let root = scratch("move");
        let folder = root.join("app data/org.example App");
        fs::create_dir_all(folder.join("config")).unwrap();
        fs::write(folder.join("config/settings"), "kept").unwrap();
        let trash = Trash::at(root.join("Trash"));
        let moved = trash.put(&folder).unwrap();
        assert_eq!(moved, root.join("Trash/files/org.example App"));
        assert!(!folder.exists());
        assert_eq!(
            fs::read_to_string(moved.join("config/settings")).unwrap(),
            "kept"
        );
        let info = fs::read_to_string(root.join("Trash/info/org.example App.trashinfo")).unwrap();
        let mut lines = info.lines();
        assert_eq!(lines.next(), Some("[Trash Info]"));
        let path = format!(
            "Path={}",
            root.join("app%20data/org.example%20App").display()
        );
        assert_eq!(lines.next(), Some(path.as_str()));
        let date = lines.next().unwrap().strip_prefix("DeletionDate=").unwrap();
        assert!(
            chrono::NaiveDateTime::parse_from_str(date, "%Y-%m-%dT%H:%M:%S").is_ok(),
            "{date}"
        );
        assert_eq!(lines.next(), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_name_already_in_the_trash_gets_a_number() {
        let root = scratch("names");
        let trash = Trash::at(root.join("Trash"));
        for kept in ["first", "second", "third"] {
            fs::create_dir_all(root.join("app")).unwrap();
            fs::write(root.join("app/which"), kept).unwrap();
            trash.put(&root.join("app")).unwrap();
        }
        // A file left in files/ without its info isn't overwritten either.
        fs::write(root.join("Trash/files/app.4"), "orphan").unwrap();
        fs::create_dir_all(root.join("app")).unwrap();
        assert_eq!(
            trash.put(&root.join("app")).unwrap(),
            root.join("Trash/files/app.5")
        );
        let names: Vec<_> = (1..=3)
            .map(|n| {
                let name = if n == 1 {
                    "app".into()
                } else {
                    format!("app.{n}")
                };
                fs::read_to_string(root.join("Trash/files").join(name).join("which")).unwrap()
            })
            .collect();
        assert_eq!(names, ["first", "second", "third"]);
        assert_eq!(
            fs::read_to_string(root.join("Trash/files/app.4")).unwrap(),
            "orphan"
        );
        assert!(!root.join("Trash/info/app.4.trashinfo").exists());
        assert!(root.join("Trash/info/app.5.trashinfo").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn nothing_is_recorded_for_what_couldnt_move() {
        let root = scratch("missing");
        let trash = Trash::at(root.join("Trash"));
        let error = trash.put(&root.join("gone")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(trash.put(Path::new("relative")).is_err());
        assert!(trash.put(Path::new("/")).is_err());
        // The move itself fails, even as root: a folder can't go inside
        // itself. The claimed name is given back.
        fs::create_dir_all(root.join("app")).unwrap();
        let inside = Trash::at(root.join("app/Trash"));
        assert!(inside.put(&root.join("app")).is_err());
        assert!(!root.join("app/Trash/info/app.trashinfo").exists());
        assert!(root.join("app").is_dir());
        // A name too long to record can't go either, and nothing moves.
        let long = root.join("a".repeat(250));
        fs::create_dir_all(&long).unwrap();
        assert!(trash.put(&long).is_err());
        assert!(long.is_dir());
        fs::remove_dir_all(&root).unwrap();
    }
}
