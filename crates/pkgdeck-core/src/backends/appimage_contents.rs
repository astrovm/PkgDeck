//! What an AppImage says about itself: the desktop entry and icon inside its
//! embedded filesystem (SquashFS, or DwarFS for newer runtimes). Read
//! straight from the file, so the AppImage is never run.

use std::{
    io::Read,
    path::{Path, PathBuf},
};

/// A desktop entry or icon larger than this is not read.
const MAX_ENTRY_BYTES: usize = 64 * 1024;
const MAX_ICON_BYTES: usize = 4 * 1024 * 1024;
/// Symlinks followed inside the image (`.DirIcon` usually is one).
const MAX_LINKS: usize = 8;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Contents {
    pub name: Option<String>,
    pub comment: Option<String>,
    pub version: Option<String>,
    pub categories: Option<String>,
    pub startup_wm_class: Option<String>,
    /// The `Exec=` arguments after the program, e.g. `--no-sandbox %U`.
    pub arguments: Option<String>,
    pub icon: Option<Icon>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Icon {
    pub bytes: Vec<u8>,
    /// `png` or `svg`.
    pub extension: &'static str,
}

/// Read the contents of the Type 2 AppImage at `path`.
pub(super) fn read(path: &Path) -> Result<Contents, String> {
    let mut image = open(path)?;
    let root = image.root();
    let entry = root
        .iter()
        .filter(|name| name.ends_with(".desktop"))
        .min()
        .and_then(|name| image.read(name, MAX_ENTRY_BYTES))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let field = |key: &str| desktop_field(&entry, key);
    let icon_name = field("Icon").filter(|name| !name.contains('/'));
    let candidates = icon_name
        .iter()
        .flat_map(|name| {
            ["png", "svg"].into_iter().flat_map(move |extension| {
                [
                    format!("{name}.{extension}"),
                    format!("usr/share/icons/hicolor/256x256/apps/{name}.{extension}"),
                    format!("usr/share/icons/hicolor/scalable/apps/{name}.{extension}"),
                ]
            })
        })
        .chain([".DirIcon".to_string()]);
    let mut icon = None;
    for candidate in candidates {
        if let Some(found) = image.read(&candidate, MAX_ICON_BYTES).and_then(Icon::new) {
            icon = Some(found);
            break;
        }
    }
    Ok(Contents {
        name: field("Name"),
        comment: field("Comment"),
        version: field("X-AppImage-Version"),
        categories: field("Categories"),
        startup_wm_class: field("StartupWMClass"),
        arguments: field("Exec").and_then(|exec| exec_arguments(&exec)),
        icon,
    })
}

impl Icon {
    fn new(bytes: Vec<u8>) -> Option<Self> {
        let extension = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "png"
        } else {
            let start = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).into_owned();
            let start = start.trim_start_matches('\u{feff}').trim_start();
            if start.starts_with("<svg") || (start.starts_with("<?xml") && start.contains("<svg")) {
                "svg"
            } else {
                return None;
            }
        };
        Some(Self { bytes, extension })
    }
}

/// A key of the `[Desktop Entry]` group, unlocalized, with the desktop entry
/// escapes undone. Empty values count as missing.
pub(super) fn desktop_field(entry: &str, key: &str) -> Option<String> {
    let mut in_group = false;
    for line in entry.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim_end() == key {
            let value = unescape(value.trim_start());
            return (!value.is_empty()).then_some(value);
        }
    }
    None
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out.replace(['\n', '\r'], " ")
}

/// Everything after the program in an `Exec=` value, kept as written.
pub(super) fn exec_arguments(exec: &str) -> Option<String> {
    let exec = exec.trim_start();
    let rest = if let Some(quoted) = exec.strip_prefix('"') {
        let mut escaped = false;
        let end = quoted.char_indices().find_map(|(index, c)| {
            if escaped {
                escaped = false;
                None
            } else if c == '\\' {
                escaped = true;
                None
            } else {
                (c == '"').then_some(index)
            }
        })?;
        &quoted[end + 1..]
    } else {
        exec.split_once(char::is_whitespace)
            .map_or("", |(_, rest)| rest)
    };
    let rest = rest.trim();
    (!rest.is_empty()).then(|| rest.to_string())
}

#[cfg(target_os = "linux")]
/// Where the embedded filesystem starts: right after the runtime's ELF
/// section headers.
fn payload_offset(file: &mut std::fs::File) -> Result<u64, String> {
    let mut header = [0_u8; 64];
    file.read_exact(&mut header).map_err(|e| e.to_string())?;
    let table = u64::from_le_bytes(header[40..48].try_into().unwrap());
    let size = u64::from(u16::from_le_bytes(header[58..60].try_into().unwrap()));
    let count = u64::from(u16::from_le_bytes(header[60..62].try_into().unwrap()));
    table
        .checked_add(size * count)
        .ok_or_else(|| "invalid ELF section table".to_string())
}

/// An entry of the embedded filesystem: a file's bytes or a symlink target.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
enum Entry {
    File(Vec<u8>),
    Link(String),
}

trait Image {
    /// Names in the filesystem root.
    fn root(&mut self) -> Vec<String>;
    /// The entry at a root-relative path; files larger than `limit` are
    /// skipped.
    fn entry(&mut self, path: &str, limit: usize) -> Option<Entry>;
    /// A file's bytes, following symlinks that stay inside the image.
    fn read(&mut self, path: &str, limit: usize) -> Option<Vec<u8>> {
        let mut path = normalize(Path::new(path))?;
        for _ in 0..MAX_LINKS {
            match self.entry(&path, limit)? {
                Entry::File(bytes) => return Some(bytes),
                Entry::Link(target) => {
                    let target = Path::new(&target);
                    let joined = if target.is_absolute() {
                        target.to_path_buf()
                    } else {
                        Path::new(&path)
                            .parent()
                            .unwrap_or(Path::new(""))
                            .join(target)
                    };
                    path = normalize(&joined)?;
                }
            }
        }
        None
    }
}

/// A root-relative path with `.` and `..` resolved; `None` when it would
/// leave the image.
fn normalize(path: &Path) -> Option<String> {
    use std::path::Component;
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::Normal(part) => parts.push(part),
            // Root, `.`, and (never on Unix) a Windows prefix.
            _ => {}
        }
    }
    let joined: PathBuf = parts.iter().collect();
    joined.to_str().map(str::to_string)
}

#[cfg(not(target_os = "linux"))]
fn open(_: &Path) -> Result<Box<dyn Image>, String> {
    Err("AppImages need Linux".into())
}

#[cfg(target_os = "linux")]
fn open(path: &Path) -> Result<Box<dyn Image>, String> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let offset = payload_offset(&mut file)?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if offset >= length {
        return Err("AppImage has no embedded filesystem".into());
    }
    let mut magic = [0_u8; 4];
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(&mut magic))
        .map_err(|e| e.to_string())?;
    match &magic {
        b"hsqs" => backhand::FilesystemReader::from_reader_with_offset(
            std::io::BufReader::new(file),
            offset,
        )
        .map(|reader| Box::new(Squash(reader)) as Box<dyn Image>)
        .map_err(|e| format!("cannot read the AppImage's SquashFS: {e}")),
        b"DWAR" => dwarfs::Archive::new(positioned_io::Slice::new(
            file,
            offset,
            Some(length - offset),
        ))
        .map(|(index, archive)| Box::new(Dwarf(index, archive)) as Box<dyn Image>)
        .map_err(|e| format!("cannot read the AppImage's DwarFS: {e}")),
        _ => Err("unknown AppImage filesystem".into()),
    }
}

#[cfg(target_os = "linux")]
struct Squash<'b>(backhand::FilesystemReader<'b>);

#[cfg(target_os = "linux")]
impl Image for Squash<'_> {
    fn root(&mut self) -> Vec<String> {
        self.0
            .files()
            .filter(|node| node.fullpath.parent() == Some(Path::new("/")))
            .filter_map(|node| node.fullpath.file_name()?.to_str().map(str::to_string))
            .collect()
    }
    fn entry(&mut self, path: &str, limit: usize) -> Option<Entry> {
        let wanted = Path::new("/").join(path);
        let node = self.0.files().find(|node| node.fullpath == wanted)?;
        match &node.inner {
            backhand::InnerNode::File(file) => {
                let mut bytes = Vec::new();
                self.0
                    .file(file)
                    .reader()
                    .take(limit as u64 + 1)
                    .read_to_end(&mut bytes)
                    .ok()?;
                (bytes.len() <= limit).then_some(Entry::File(bytes))
            }
            backhand::InnerNode::Symlink(link) => link.link.to_str().map(|t| Entry::Link(t.into())),
            _ => None,
        }
    }
}

#[cfg(target_os = "linux")]
struct Dwarf(
    dwarfs::ArchiveIndex,
    dwarfs::Archive<positioned_io::Slice<std::fs::File>>,
);

#[cfg(target_os = "linux")]
impl Image for Dwarf {
    fn root(&mut self) -> Vec<String> {
        self.0
            .root()
            .entries()
            .map(|entry| entry.name().to_string())
            .collect()
    }
    fn entry(&mut self, path: &str, limit: usize) -> Option<Entry> {
        use dwarfs::AsChunks;
        let inode = self.0.get_path(path.split('/'))?;
        match inode.classify() {
            dwarfs::InodeKind::File(file) => {
                let mut bytes = Vec::new();
                file.as_reader(&mut self.1)
                    .take(limit as u64 + 1)
                    .read_to_end(&mut bytes)
                    .ok()?;
                (bytes.len() <= limit).then_some(Entry::File(bytes))
            }
            dwarfs::InodeKind::Symlink(link) => Some(Entry::Link(link.target().to_string())),
            _ => None,
        }
    }
}

/// Real Type 2 AppImages for tests: the runtime's ELF header with one empty
/// section header, then the embedded filesystem.
#[cfg(all(test, target_os = "linux"))]
pub(super) mod fixture {
    use std::path::Path;

    fn runtime() -> Vec<u8> {
        let mut bytes = vec![0_u8; 128];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4..7].copy_from_slice(&[2, 1, 1]);
        bytes[8..11].copy_from_slice(b"AI\x02");
        bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
        bytes[40..48].copy_from_slice(&64_u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
        bytes[58..60].copy_from_slice(&64_u16.to_le_bytes());
        bytes[60..62].copy_from_slice(&1_u16.to_le_bytes());
        bytes
    }

    /// A SquashFS AppImage with `files`, `links` (path, target) and `dirs`.
    pub fn squash(
        path: &Path,
        compressor: backhand::compression::Compressor,
        files: &[(&str, &[u8])],
        links: &[(&str, &str)],
        dirs: &[&str],
    ) {
        use backhand::{FilesystemCompressor, FilesystemWriter, NodeHeader};
        let header = NodeHeader::new(0o755, 0, 0, 0);
        let mut writer = FilesystemWriter::default();
        writer.set_compressor(FilesystemCompressor::new(compressor, None).unwrap());
        for dir in dirs {
            writer.push_dir(dir, header).unwrap();
        }
        for (name, data) in files {
            writer
                .push_file(std::io::Cursor::new(data.to_vec()), name, header)
                .unwrap();
        }
        for (name, target) in links {
            writer.push_symlink(target, name, header).unwrap();
        }
        let mut image = std::io::Cursor::new(Vec::new());
        writer.write(&mut image).unwrap();
        let mut bytes = runtime();
        bytes.extend_from_slice(image.get_ref());
        std::fs::write(path, bytes).unwrap();
    }

    /// A DwarFS AppImage, made with mkdwarfs 0.12.4 from `demo.desktop`
    /// (Demo App 2.0, `AppRun --no-sandbox %U`), `demo.png`, a `.DirIcon`
    /// link to it and an empty `usr` folder.
    pub fn dwarf(path: &Path) {
        let mut bytes = runtime();
        bytes.extend_from_slice(include_bytes!("fixtures/demo.dwarfs"));
        std::fs::write(path, bytes).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_fields_come_from_the_main_group_only() {
        let entry = "[Desktop Entry]\nName=Real App\nName[es]=App real\nComment= A tool\\sfor things \nIcon=real\nX-AppImage-Version=1.2.3\nExec=AppRun --no-sandbox %U\n\n[Desktop Action New]\nName=New window\nExec=AppRun --new\n";
        assert_eq!(desktop_field(entry, "Name").as_deref(), Some("Real App"));
        assert_eq!(
            desktop_field(entry, "Comment").as_deref(),
            Some("A tool for things")
        );
        assert_eq!(
            desktop_field(entry, "X-AppImage-Version").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(desktop_field(entry, "Categories"), None);
        assert_eq!(
            desktop_field("[Desktop Action A]\nName=Wrong\n", "Name"),
            None
        );
        assert_eq!(desktop_field("[Desktop Entry]\nName=\n", "Name"), None);
        assert_eq!(
            desktop_field(
                "[Desktop Entry]\nComment=a\\nb\\tc\\rd\\\\e\\;f\\\n",
                "Comment"
            )
            .as_deref(),
            Some("a b\tc d\\e;f\\")
        );
    }

    #[test]
    fn exec_arguments_skip_the_program() {
        assert_eq!(
            exec_arguments("AppRun --no-sandbox %U").as_deref(),
            Some("--no-sandbox %U")
        );
        assert_eq!(exec_arguments("eden %f").as_deref(), Some("%f"));
        assert_eq!(exec_arguments("app").as_deref(), None);
        assert_eq!(
            exec_arguments("\"/opt/my \\\"app\\\"\" --flag").as_deref(),
            Some("--flag")
        );
        assert_eq!(exec_arguments("\"unterminated --flag"), None);
    }

    #[test]
    fn paths_stay_inside_the_image() {
        assert_eq!(normalize(Path::new("/a/./b/../c")).as_deref(), Some("a/c"));
        assert_eq!(normalize(Path::new("usr/../../etc/passwd")), None);
    }

    #[test]
    fn icons_are_png_or_svg() {
        assert_eq!(
            Icon::new(b"\x89PNG\r\n\x1a\nrest".to_vec()).map(|i| i.extension),
            Some("png")
        );
        assert_eq!(
            Icon::new(b"<?xml version=\"1.0\"?>\n<svg/>".to_vec()).map(|i| i.extension),
            Some("svg")
        );
        assert_eq!(Icon::new(b"GIF89a".to_vec()), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn dwarfs_appimages_are_read_like_squashfs_ones() {
        let dir = std::env::temp_dir().join(format!("pkgdeck-dwarfs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("demo.AppImage");
        fixture::dwarf(&path);
        let contents = read(&path).unwrap();
        assert_eq!(contents.name.as_deref(), Some("Demo App"));
        assert_eq!(contents.comment.as_deref(), Some("Does demo things"));
        assert_eq!(contents.version.as_deref(), Some("2.0"));
        assert_eq!(contents.arguments.as_deref(), Some("--no-sandbox %U"));
        assert_eq!(
            contents.icon.map(|icon| icon.bytes),
            Some(b"\x89PNG\r\n\x1a\ndemo icon".to_vec())
        );
        let mut image = open(&path).unwrap();
        assert!(image.read("usr", MAX_ICON_BYTES).is_none());
        assert_eq!(image.read(".DirIcon", 4), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn links_are_followed_only_inside_the_image() {
        let dir = std::env::temp_dir().join(format!("pkgdeck-links-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("links.AppImage");
        fixture::squash(
            &path,
            backhand::compression::Compressor::Gzip,
            &[("usr/icon.png", b"bytes")],
            &[
                ("absolute", "/usr/icon.png"),
                ("relative", "usr/icon.png"),
                ("loop-a", "loop-b"),
                ("loop-b", "loop-a"),
                ("outside", "../../etc/passwd"),
            ],
            &["usr"],
        );
        let mut image = open(&path).unwrap();
        assert_eq!(image.read("absolute", 16).as_deref(), Some(&b"bytes"[..]));
        assert_eq!(image.read("relative", 16).as_deref(), Some(&b"bytes"[..]));
        assert_eq!(image.read("loop-a", 16), None);
        assert_eq!(image.read("outside", 16), None);
        assert_eq!(image.read("usr", 16), None);
        assert_eq!(image.read("usr/icon.png", 2), None);
        assert_eq!(image.read("missing", 16), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn non_appimage_files_are_reported() {
        let dir = std::env::temp_dir().join(format!("pkgdeck-contents-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("short");
        std::fs::write(&path, b"tiny").unwrap();
        assert!(read(&path).is_err());
        let mut elf = vec![0_u8; 64];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[40..48].copy_from_slice(&64_u64.to_le_bytes());
        std::fs::write(&path, &elf).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            "AppImage has no embedded filesystem"
        );
        elf.extend_from_slice(b"nope");
        std::fs::write(&path, &elf).unwrap();
        assert_eq!(read(&path).unwrap_err(), "unknown AppImage filesystem");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
