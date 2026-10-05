use super::appimage_contents::{self, Contents, Icon};
use super::appimage_github;
use super::desktop_exec;
use crate::{
    engine::*,
    host::Host,
    package::*,
    process::{self, Cancellation, ExecutionError, Limits},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

const MAX_IMPORT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Shown when an AppImage names no version.
const UNKNOWN_VERSION: &str = "local";

const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Upgrade,
    Capability::Clean,
];

/// How an AppImage starts from its menu entry. PkgDeck can change it only
/// for AppImages it manages (`editable`), whose entry it wrote.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct LaunchSettings {
    /// The arguments after the program, as written (`--no-sandbox %U`).
    pub arguments: String,
    pub environment: Vec<(String, String)>,
    pub editable: bool,
}

/// Where an AppImage PkgDeck manages gets its updates: a GitHub project's
/// releases when one is set, else its own update address (`builtin`).
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct UpdateSource {
    pub github: Option<String>,
    pub builtin: bool,
    pub editable: bool,
}

/// The file an installed AppImage starts from, as its page shows it.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct AppFile {
    pub path: PathBuf,
    pub bytes: u64,
    /// When the file last changed (installed or updated), in seconds since 1970.
    pub modified: i64,
    pub managed: bool,
    /// Its runtime needs FUSE 2 (`libfuse.so.2`) and this computer lacks
    /// it, so PkgDeck starts it unpacked instead.
    pub without_fuse: bool,
}
/// Where `libfuse.so.2` is found on common Linux systems.
const FUSE2_LIBRARIES: [&str; 6] = [
    "/usr/lib/x86_64-linux-gnu/libfuse.so.2",
    "/usr/lib/aarch64-linux-gnu/libfuse.so.2",
    "/lib/x86_64-linux-gnu/libfuse.so.2",
    "/usr/lib64/libfuse.so.2",
    "/usr/lib/libfuse.so.2",
    "/lib64/libfuse.so.2",
];

/// How long a GitHub answer is used before asking again. GitHub allows 60
/// questions an hour without signing in, shared by everything on the network.
const GITHUB_REUSE_SECONDS: u64 = 15 * 60;

/// Imports only Type 2 AppImages into PkgDeck-owned storage. Metadata inspection
/// reads the ELF header; imported files are never executed by this backend.
pub struct AppImage {
    root: PathBuf,
    applications: PathBuf,
    uid: u32,
    /// Homebrew Caskrooms whose `app_image` casks own AppImages outside PkgDeck.
    caskrooms: Vec<PathBuf>,
    #[cfg(test)]
    updater: Option<PathBuf>,
    #[cfg(test)]
    curl: Option<PathBuf>,
    /// Whether FUSE 2 is installed, decided by a test.
    #[cfg(test)]
    fuse2: Option<bool>,
}

impl AppImage {
    pub fn native() -> Self {
        let host = Host::current();
        let data = host
            .var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                host.var("HOME")
                    .map(|home| PathBuf::from(home).join(".local/share"))
            })
            .unwrap_or_else(|| PathBuf::from("/nonexistent/pkgdeck-host-data-unavailable"));
        let caskrooms = host
            .var("HOMEBREW_PREFIX")
            .map(PathBuf::from)
            .into_iter()
            .chain(
                host.var("HOME")
                    .map(|home| PathBuf::from(home).join(".linuxbrew")),
            )
            .chain([PathBuf::from("/home/linuxbrew/.linuxbrew")])
            .filter(|prefix| prefix.is_absolute())
            .map(|prefix| prefix.join("Caskroom"))
            .collect();
        Self {
            caskrooms,
            ..Self::with_data(&data)
        }
    }
    /// The AppImages of a data folder (`XDG_DATA_HOME`): PkgDeck's own and
    /// those its desktop entries start. Homebrew's are not told apart.
    pub fn with_data(data: &Path) -> Self {
        Self {
            root: data.join("pkgdeck/appimages"),
            applications: data.join("applications"),
            uid: rustix::process::getuid().as_raw(),
            caskrooms: vec![],
            #[cfg(test)]
            updater: None,
            #[cfg(test)]
            curl: None,
            #[cfg(test)]
            fuse2: None,
        }
    }
    #[cfg(test)]
    pub fn new(root: PathBuf, applications: PathBuf, uid: u32) -> Self {
        Self {
            root,
            applications,
            uid,
            caskrooms: vec![],
            updater: None,
            curl: None,
            fuse2: None,
        }
    }
    #[cfg(test)]
    fn with_caskroom(mut self, caskroom: PathBuf) -> Self {
        self.caskrooms.push(caskroom);
        self
    }
    #[cfg(all(test, target_os = "linux"))]
    fn with_updater(mut self, updater: PathBuf) -> Self {
        self.updater = Some(updater);
        self
    }
    fn scope(&self) -> Scope {
        Scope::User { uid: self.uid }
    }
    /// The installed AppImage a file with this `digest` becomes once
    /// PkgDeck installs it.
    pub fn installed_id(&self, digest: &str, architecture: &str) -> PackageId {
        PackageId {
            backend: "appimage".into(),
            name: format!("pkgdeck-{digest}.AppImage"),
            architecture: architecture.into(),
            scope: self.scope(),
            remote: None,
            reference: None,
        }
    }
    pub fn import_scope(&self) -> Scope {
        Scope::Environment {
            path: self.root.clone(),
        }
    }
    fn invalid(reason: impl Into<String>) -> EngineError {
        EngineError::InvalidResponse {
            backend: "appimage".into(),
            reason: reason.into(),
        }
    }
    fn type2(path: &Path) -> Result<String, EngineError> {
        let metadata = fs::symlink_metadata(path).map_err(|e| Self::invalid(e.to_string()))?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_IMPORT_BYTES {
            return Err(Self::invalid(
                "expected a regular AppImage no larger than 2 GiB",
            ));
        }
        let mut file = fs::File::open(path).map_err(|e| Self::invalid(e.to_string()))?;
        let mut header = [0; 64];
        file.read_exact(&mut header)
            .map_err(|e| Self::invalid(e.to_string()))?;
        if &header[..4] != b"\x7fELF" || &header[8..11] != b"AI\x02" {
            return Err(Self::invalid("expected a Type 2 AppImage ELF file"));
        }
        if header[4] != 2
            || header[5] != 1
            || header[6] != 1
            || u32::from_le_bytes(header[20..24].try_into().unwrap()) != 1
            || u16::from_le_bytes(header[52..54].try_into().unwrap()) != 64
        {
            return Err(Self::invalid("malformed or unsupported ELF header"));
        }
        let arch = match u16::from_le_bytes([header[18], header[19]]) {
            62 => "x86_64",
            183 => "aarch64",
            _ => return Err(Self::invalid("unsupported AppImage architecture")),
        };
        Ok(arch.into())
    }
    fn digest(path: &Path, cancel: &Cancellation) -> Result<String, EngineError> {
        Self::digest_bounded(path, MAX_IMPORT_BYTES, cancel)
    }
    /// Hashing a large file takes a while, so cancellation is checked per chunk.
    fn digest_bounded(
        path: &Path,
        limit: u64,
        cancel: &Cancellation,
    ) -> Result<String, EngineError> {
        let mut file = fs::File::open(path).map_err(|e| Self::invalid(e.to_string()))?;
        let mut hash = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut total = 0_u64;
        loop {
            if cancel.requested() {
                return Err(EngineError::Cancelled);
            }
            let count = file
                .read(&mut buffer)
                .map_err(|e| Self::invalid(e.to_string()))?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > limit {
                return Err(Self::invalid("AppImage exceeds 2 GiB"));
            }
            hash.update(&buffer[..count]);
        }
        Ok(hex::encode(hash.finalize()))
    }
    /// Hashes `path` and requires the digest the user reviewed.
    fn verify_digest(
        path: &Path,
        expected: &str,
        cancel: &Cancellation,
    ) -> Result<(), EngineError> {
        if Self::digest(path, cancel)? != expected {
            return Err(Self::invalid("AppImage changed or was not previewed"));
        }
        Ok(())
    }
    fn copy_bounded(
        input: &mut impl Read,
        output: &mut impl Write,
        cancel: &Cancellation,
    ) -> Result<(), EngineError> {
        let mut copied = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            if cancel.requested() {
                return Err(EngineError::Cancelled);
            }
            let count = input
                .read(&mut buffer)
                .map_err(|e| Self::invalid(e.to_string()))?;
            if count == 0 {
                break;
            }
            copied += count as u64;
            if copied > MAX_IMPORT_BYTES {
                return Err(Self::invalid("AppImage exceeds 2 GiB"));
            }
            output
                .write_all(&buffer[..count])
                .map_err(|e| Self::invalid(e.to_string()))?;
        }
        Ok(())
    }
    fn has_update_metadata(path: &Path) -> Result<bool, EngineError> {
        let mut file = fs::File::open(path).map_err(|e| Self::invalid(e.to_string()))?;
        let len = file
            .metadata()
            .map_err(|e| Self::invalid(e.to_string()))?
            .len();
        let mut header = [0_u8; 64];
        file.read_exact(&mut header)
            .map_err(|e| Self::invalid(e.to_string()))?;
        let offset = u64::from_le_bytes(header[40..48].try_into().unwrap());
        let entry_size = u16::from_le_bytes(header[58..60].try_into().unwrap()) as u64;
        let count = u16::from_le_bytes(header[60..62].try_into().unwrap()) as u64;
        let names_index = u16::from_le_bytes(header[62..64].try_into().unwrap()) as u64;
        if offset == 0 && count == 0 {
            return Ok(false);
        }
        if entry_size != 64
            || count == 0
            || count > 4096
            || names_index >= count
            || offset
                .checked_add(entry_size * count)
                .is_none_or(|end| end > len)
        {
            return Err(Self::invalid("invalid ELF section table"));
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| Self::invalid(e.to_string()))?;
        let mut entries = vec![0_u8; (entry_size * count) as usize];
        file.read_exact(&mut entries)
            .map_err(|e| Self::invalid(e.to_string()))?;
        let names = &entries[(names_index * 64) as usize..((names_index + 1) * 64) as usize];
        let names_offset = u64::from_le_bytes(names[24..32].try_into().unwrap());
        let names_size = u64::from_le_bytes(names[32..40].try_into().unwrap());
        if names_size > 1024 * 1024
            || names_offset
                .checked_add(names_size)
                .is_none_or(|end| end > len)
        {
            return Err(Self::invalid("invalid ELF section names"));
        }
        file.seek(SeekFrom::Start(names_offset))
            .map_err(|e| Self::invalid(e.to_string()))?;
        let mut strings = vec![0_u8; names_size as usize];
        file.read_exact(&mut strings)
            .map_err(|e| Self::invalid(e.to_string()))?;
        for entry in entries.as_chunks::<64>().0 {
            let index = u32::from_le_bytes(entry[..4].try_into().unwrap()) as usize;
            if strings
                .get(index..)
                .is_some_and(|suffix| suffix.starts_with(b".upd_info\0"))
            {
                let data_offset = u64::from_le_bytes(entry[24..32].try_into().unwrap());
                let data_size = u64::from_le_bytes(entry[32..40].try_into().unwrap());
                if data_size == 0
                    || data_size > 4096
                    || data_offset
                        .checked_add(data_size)
                        .is_none_or(|end| end > len)
                {
                    return Err(Self::invalid("invalid AppImage update metadata"));
                }
                file.seek(SeekFrom::Start(data_offset))
                    .map_err(|e| Self::invalid(e.to_string()))?;
                let mut data = vec![0_u8; data_size as usize];
                file.read_exact(&mut data)
                    .map_err(|e| Self::invalid(e.to_string()))?;
                return Ok(data.iter().any(|byte| !matches!(byte, 0 | b' ' | b'\n')));
            }
        }
        Ok(false)
    }
    /// The `Exec=` value's program part: the managed file, quoted.
    fn exec_program(destination: &Path) -> String {
        let escaped = destination
            .display()
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%");
        format!("\"{escaped}\"")
    }
    /// A desktop entry for a managed AppImage, from what the AppImage says
    /// about itself: its name, description, icon, categories and the
    /// arguments its own entry starts it with.
    fn desktop_entry(
        destination: &Path,
        fallback_name: &str,
        contents: &Contents,
        icon: Option<&Path>,
    ) -> String {
        let line = |value: &str| value.replace(['\n', '\r'], " ");
        let mut entry = format!(
            "[Desktop Entry]\nType=Application\nName={}\n",
            line(contents.name.as_deref().unwrap_or(fallback_name))
        );
        if let Some(comment) = &contents.comment {
            entry += &format!("Comment={}\n", line(comment));
        }
        if let Some(icon) = icon {
            entry += &format!("Icon={}\n", line(&icon.display().to_string()));
        }
        let arguments = contents.arguments.as_deref().map_or_else(
            || "%U".to_string(),
            |arguments| line(arguments).replace('\\', "\\\\"),
        );
        entry += &format!(
            "Exec={} {arguments}\nTryExec={}\nTerminal=false\nCategories={}\n",
            Self::exec_program(destination),
            line(&destination.display().to_string()),
            line(contents.categories.as_deref().unwrap_or("Utility;"))
        );
        if let Some(class) = &contents.startup_wm_class {
            entry += &format!("StartupWMClass={}\n", line(class));
        }
        if let Some(version) = &contents.version {
            entry += &format!("X-AppImage-Version={}\n", line(version));
        }
        entry
    }
    /// What the AppImage at `path` says about itself, remembered while the
    /// file is unchanged. Unreadable contents count as empty.
    fn contents(path: &Path) -> std::sync::Arc<Contents> {
        use std::{
            collections::HashMap,
            sync::{Arc, Mutex, OnceLock},
        };
        /// Size and modification time: the file is unchanged while they are.
        type Key = (u64, i64, i64);
        type Seen = HashMap<PathBuf, (Key, Arc<Contents>)>;
        static SEEN: OnceLock<Mutex<Seen>> = OnceLock::new();
        let key = fs::metadata(path)
            .ok()
            .map(|m| (m.len(), m.mtime(), m.mtime_nsec()));
        let seen = SEEN.get_or_init(Default::default);
        if let Some((_, contents)) = seen
            .lock()
            .unwrap()
            .get(path)
            .filter(|(known, _)| Some(*known) == key)
        {
            return contents.clone();
        }
        let contents = Arc::new(appimage_contents::read(path).unwrap_or_default());
        if let Some(key) = key {
            seen.lock()
                .unwrap()
                .insert(path.to_path_buf(), (key, contents.clone()));
        }
        contents
    }
    /// Icons PkgDeck read out of AppImages live next to its AppImage folder,
    /// which holds nothing but managed AppImages.
    fn icons(&self) -> PathBuf {
        self.root.with_file_name("appimage-icons")
    }
    /// Save `icon` as `<key>.<png|svg>` among the icons, replacing an older
    /// one with the same key.
    fn save_icon(&self, key: &str, icon: &Icon) -> Option<PathBuf> {
        let dir = self.icons();
        let path = dir.join(format!("{key}.{}", icon.extension));
        if fs::read(&path).is_ok_and(|bytes| bytes == icon.bytes) {
            return Some(path);
        }
        fs::create_dir_all(&dir).ok()?;
        let temporary = dir.join(format!(".{key}.{}", std::process::id()));
        let written =
            fs::write(&temporary, &icon.bytes).and_then(|()| fs::rename(&temporary, &path));
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
            return None;
        }
        Some(path)
    }
    fn remove_icons(&self, key: &str) {
        for extension in ["png", "svg"] {
            let _ = fs::remove_file(self.icons().join(format!("{key}.{extension}")));
        }
    }
    /// `~/…` for paths in the home folder, as people know them.
    fn shown_path(path: &Path) -> String {
        Host::current()
            .var("HOME")
            .and_then(|home| {
                path.strip_prefix(home)
                    .ok()
                    .map(|rest| format!("~/{}", rest.display()))
            })
            .unwrap_or_else(|| path.display().to_string())
    }
    fn existing_import(
        destination: &Path,
        entry: &Path,
        digest: &str,
    ) -> Result<bool, EngineError> {
        if destination.symlink_metadata().is_err() && entry.symlink_metadata().is_err() {
            return Ok(false);
        }
        let identical = destination
            .symlink_metadata()
            .is_ok_and(|meta| meta.file_type().is_file())
            && entry
                .symlink_metadata()
                .is_ok_and(|meta| meta.file_type().is_file())
            && Self::digest(destination, &Cancellation::default())
                .ok()
                .as_deref()
                == Some(digest)
            && Self::exec_line(entry)
                .and_then(|exec| desktop_exec::Command::parse(&exec))
                .is_some_and(|command| Path::new(&command.program) == destination);
        if identical {
            Ok(true)
        } else {
            Err(Self::invalid(
                "AppImage destination conflicts with an existing installation",
            ))
        }
    }
    fn managed_name(name: &str) -> bool {
        name.len() == 81
            && name.starts_with("pkgdeck-")
            && name.ends_with(".AppImage")
            && name[8..72].bytes().all(|b| b.is_ascii_hexdigit())
    }
    fn package(&self, path: PathBuf) -> Result<Package, EngineError> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Self::invalid("managed filename is not UTF-8"))?;
        if !Self::managed_name(name) {
            return Err(Self::invalid("unmanaged AppImage filename"));
        }
        let architecture = Self::type2(&path)?;
        let digest = &name[8..72];
        let contents = Self::contents(&path);
        let version = contents
            .version
            .clone()
            .unwrap_or_else(|| UNKNOWN_VERSION.into());
        let icon = ["png", "svg"]
            .into_iter()
            .map(|extension| self.icons().join(format!("pkgdeck-{digest}.{extension}")))
            .find(|icon| icon.is_file())
            .or_else(|| {
                contents
                    .icon
                    .as_ref()
                    .and_then(|icon| self.save_icon(&format!("pkgdeck-{digest}"), icon))
            });
        Ok(Package {
            id: PackageId {
                backend: "appimage".into(),
                name: name.into(),
                architecture,
                scope: self.scope(),
                remote: None,
                reference: None,
            },
            display_name: contents
                .name
                .clone()
                .unwrap_or_else(|| "Imported AppImage".into()),
            summary: contents.comment.clone().unwrap_or_default(),
            installed_version: Some(version.clone()),
            candidate_version: Some(version),
            update: if Self::has_update_metadata(&path)? || self.github_source(name).is_some() {
                UpdateAvailability::Unknown
            } else {
                UpdateAvailability::Current
            },
            icon,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        })
    }
    fn installed_packages(&self) -> Result<Vec<Package>, EngineError> {
        let mut packages = match fs::read_dir(&self.root) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|e| self.package(e.path()))
                .collect::<Result<Vec<_>, _>>()?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(e) => return Err(Self::invalid(e.to_string())),
        };
        packages.extend(self.external_packages()?);
        // One external AppImage is commonly referenced by several desktop
        // entries (application launcher, Gear Lever entry, stale copies).
        // Collapse them so the engine never sees duplicate identities.
        let mut seen = std::collections::BTreeSet::new();
        packages.retain(|package| seen.insert(package.id.clone()));
        Ok(packages)
    }
    fn desktop_value(contents: &str, key: &str) -> Option<String> {
        contents
            .lines()
            .find_map(|line| line.strip_prefix(key).map(str::to_owned))
            .filter(|value| !value.is_empty())
    }
    /// Return the executable from an `Exec=` value when it is an absolute path,
    /// past a leading `env VAR=value …`. Field codes after it are ignored.
    fn exec_path(value: &str) -> Option<PathBuf> {
        desktop_exec::Command::parse(value)
            .map(|command| PathBuf::from(command.program))
            .filter(|path| path.is_absolute())
    }
    fn desktop_appimage_path(contents: &str) -> Option<PathBuf> {
        Self::desktop_value(contents, "TryExec=")
            .and_then(|value| {
                let path = PathBuf::from(&value);
                path.is_absolute()
                    .then_some(path)
                    .or_else(|| Self::exec_path(&value))
            })
            .or_else(|| {
                Self::desktop_value(contents, "Exec=").and_then(|value| Self::exec_path(&value))
            })
    }
    /// AppImages a Homebrew cask installed. Homebrew moves the file into its
    /// AppImage folder and leaves a symlink to it in the cask's installed
    /// version folder, as it does for macOS apps; those belong to Homebrew
    /// Casks, so this source must not offer to update or remove them.
    fn homebrew_appimages(&self) -> std::collections::BTreeSet<PathBuf> {
        let children = |dir: &Path| {
            fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .collect::<Vec<_>>()
        };
        self.caskrooms
            .iter()
            .flat_map(|caskroom| children(caskroom))
            .flat_map(|cask| children(&cask))
            .filter(|version| version.file_name().is_some_and(|name| name != ".metadata"))
            .flat_map(|version| children(&version))
            .filter(|link| fs::symlink_metadata(link).is_ok_and(|m| m.file_type().is_symlink()))
            .filter_map(|link| fs::canonicalize(link).ok())
            .collect()
    }
    fn external_entries(&self) -> Result<Vec<(Package, PathBuf)>, EngineError> {
        let entries = match fs::read_dir(&self.applications) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(Self::invalid(e.to_string())),
        };
        let homebrew = self.homebrew_appimages();
        Ok(entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "desktop"))
            .filter_map(|entry| {
                let desktop = entry.path();
                let contents = fs::read_to_string(&desktop).ok()?;
                let path = Self::desktop_appimage_path(&contents)?;
                let canonical = fs::canonicalize(path).ok()?;
                let metadata = fs::metadata(&canonical).ok()?;
                if !canonical.is_absolute()
                    || canonical.starts_with(&self.root)
                    || homebrew.contains(&canonical)
                    || !metadata.is_file()
                    || metadata.uid() != self.uid
                {
                    return None;
                }
                let architecture = Self::type2(&canonical).ok()?;
                let display_name = Self::desktop_value(&contents, "Name=")
                    .unwrap_or_else(|| "External AppImage".into());
                // The AppImage's own version wins: an entry written at
                // install time goes stale once the app updates itself.
                let inside = Self::contents(&canonical);
                let version = inside
                    .version
                    .clone()
                    .or_else(|| Self::desktop_value(&contents, "X-AppImage-Version="))
                    .unwrap_or_else(|| UNKNOWN_VERSION.into());
                // External entries keep their own desktop file, so its Icon=
                // resolves like any local entry; otherwise the AppImage's own.
                let icon = super::desktop_icon(None, &desktop).or_else(|| {
                    inside
                        .icon
                        .as_ref()
                        .and_then(|icon| self.save_icon(&Self::external_icon_key(&canonical), icon))
                });
                Some((
                    Package {
                        id: PackageId {
                            backend: "appimage".into(),
                            name: canonical.display().to_string(),
                            architecture,
                            scope: self.scope(),
                            remote: None,
                            reference: None,
                        },
                        display_name,
                        // What the app says it is; else where the file is,
                        // outside PkgDeck's folder. Its page shows the file.
                        summary: inside
                            .comment
                            .clone()
                            .filter(|comment| !comment.trim().is_empty())
                            .unwrap_or_else(|| Self::shown_path(&canonical)),
                        installed_version: Some(version.clone()),
                        candidate_version: Some(version),
                        update: if Self::has_update_metadata(&canonical).ok()? {
                            UpdateAvailability::Unknown
                        } else {
                            UpdateAvailability::Current
                        },
                        icon,
                        // The entry's desktop-id stem joins the shared
                        // namespace (e.g. an Audacity AppImage groups with
                        // an Audacity install from another manager).
                        component_ids: desktop
                            .file_stem()
                            .and_then(|stem| super::component_stem(&stem.to_string_lossy()))
                            .into_iter()
                            .collect(),
                        homepages: vec![],
                        // PkgDeck can take it over: Manage moves it in.
                        adopt_with: Some("appimage".into()),
                    },
                    desktop,
                ))
            })
            .collect::<Vec<_>>())
    }
    fn external_icon_key(path: &Path) -> String {
        let hash = Sha256::digest(path.as_os_str().as_encoded_bytes());
        format!("external-{}", &hex::encode(hash)[..16])
    }
    /// Every desktop entry that starts the AppImage at `canonical`. Gear
    /// Lever and others often leave several.
    fn entries_for(&self, canonical: &Path) -> Vec<PathBuf> {
        fs::read_dir(&self.applications)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "desktop"))
            .filter(|path| {
                fs::read_to_string(path)
                    .ok()
                    .and_then(|contents| Self::desktop_appimage_path(&contents))
                    .and_then(|target| fs::canonicalize(target).ok())
                    .is_some_and(|target| target == canonical)
            })
            .collect()
    }
    /// The external AppImage at `path`, when PkgDeck lists it.
    fn external_at(&self, path: &Path) -> Option<PathBuf> {
        let canonical = fs::canonicalize(path).ok()?;
        self.external_entries()
            .ok()?
            .into_iter()
            .any(|(package, _)| Path::new(&package.id.name) == canonical)
            .then_some(canonical)
    }
    fn external_packages(&self) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .external_entries()?
            .into_iter()
            .map(|(package, _)| package)
            .collect())
    }
    fn import(&self, id: &PackageId, cancel: &Cancellation) -> Result<(), EngineError> {
        if id
            .reference
            .as_deref()
            .is_some_and(|value| value.starts_with("artifact:appimage:"))
        {
            let staged = crate::artifact::stage(id, cancel)?
                .ok_or_else(|| Self::invalid("missing AppImage"))?;
            let mut local = id.clone();
            local.name = staged.path().to_string_lossy().into_owned();
            local.reference = Some(Self::digest(staged.path(), cancel)?);
            return self.import(&local, cancel);
        }
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        if id.backend != "appimage"
            || id.scope
                != (Scope::Environment {
                    path: self.root.clone(),
                })
        {
            return Err(Self::invalid("expected an AppImage import path"));
        }
        let source = PathBuf::from(&id.name);
        if !source.is_absolute() || source.starts_with(&self.root) {
            return Err(Self::invalid("expected an external absolute AppImage path"));
        }
        let architecture = Self::type2(&source)?;
        if architecture != id.architecture {
            return Err(Self::invalid("AppImage architecture changed during import"));
        }
        let digest = id.reference.as_deref().unwrap_or_default();
        Self::verify_digest(&source, digest, cancel)?;
        let taken_over = self.external_at(&source);
        // How its old entry started it (arguments, environment variables set
        // in Gear Lever, say) carries over to PkgDeck's entry.
        let carried = taken_over.as_ref().and_then(|original| {
            self.entries_for(original).iter().find_map(|entry| {
                Self::exec_line(entry).and_then(|exec| desktop_exec::Command::parse(&exec))
            })
        });
        let name = format!("pkgdeck-{digest}.AppImage");
        fs::create_dir_all(&self.root).map_err(|e| Self::invalid(e.to_string()))?;
        let destination = self.root.join(&name);
        fs::create_dir_all(&self.applications).map_err(|e| Self::invalid(e.to_string()))?;
        let entry = self
            .applications
            .join(format!("pkgdeck-{}.desktop", &name[8..72]));
        if Self::existing_import(&destination, &entry, digest)? {
            // PkgDeck already has this very file: the other copy still goes.
            return taken_over.map_or(Ok(()), |original| self.retire(&original));
        }
        let temporary = self.root.join(format!(
            ".pkgdeck-import-{}-{}-{digest}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let result = (|| {
            let mut input = fs::File::open(&source).map_err(|e| Self::invalid(e.to_string()))?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|e| Self::invalid(e.to_string()))?;
            Self::copy_bounded(&mut input, &mut output, cancel)?;
            output.flush().map_err(|e| Self::invalid(e.to_string()))?;
            output
                .sync_all()
                .map_err(|e| Self::invalid(e.to_string()))?;
            drop(output);
            Self::verify_digest(&temporary, digest, cancel)?;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))
                .map_err(|e| Self::invalid(e.to_string()))?;
            fs::hard_link(&temporary, &destination).map_err(|e| Self::invalid(e.to_string()))?;
            let fallback_name = source
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Imported AppImage");
            let contents = Self::contents(&destination);
            let icon = contents
                .icon
                .as_ref()
                .and_then(|icon| self.save_icon(&format!("pkgdeck-{digest}"), icon));
            let mut desktop =
                Self::desktop_entry(&destination, fallback_name, &contents, icon.as_deref());
            if let Some(carried) = &carried {
                let exec = carried.render(&Self::exec_program(&destination));
                desktop = desktop
                    .lines()
                    .map(|line| {
                        if line.starts_with("Exec=") {
                            format!("Exec={exec}\n")
                        } else {
                            format!("{line}\n")
                        }
                    })
                    .collect();
            }
            let file = match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&entry)
            {
                Ok(file) => file,
                Err(error) => {
                    let _ = fs::remove_file(&destination);
                    return Err(Self::invalid(format!("desktop entry failed: {error}")));
                }
            };
            match (|| {
                let mut file = file;
                file.write_all(desktop.as_bytes())?;
                file.sync_all()
            })() {
                Ok(()) => Ok(()),
                Err(error) => {
                    let _ = fs::remove_file(&entry);
                    let _ = fs::remove_file(&destination);
                    self.remove_icons(&format!("pkgdeck-{digest}"));
                    Err(Self::invalid(format!("desktop entry failed: {error}")))
                }
            }
        })();
        let _ = fs::remove_file(&temporary);
        result?;
        taken_over.map_or(Ok(()), |original| self.retire(&original))
    }
    /// An AppImage that was installed elsewhere moved in: its old file and
    /// desktop entries go, so it isn't listed twice.
    fn retire(&self, original: &Path) -> Result<(), EngineError> {
        for entry in self.entries_for(original) {
            let _ = fs::remove_file(entry);
        }
        fs::remove_file(original).map_err(|e| {
            Self::invalid(format!(
                "PkgDeck now manages this AppImage, but couldn't remove the old copy at {}: {e}",
                original.display()
            ))
        })?;
        self.remove_icons(&Self::external_icon_key(original));
        Ok(())
    }
    /// The raw `Exec=` value of a desktop entry's main group.
    fn exec_line(desktop: &Path) -> Option<String> {
        let contents = fs::read_to_string(desktop).ok()?;
        let mut in_group = false;
        contents.lines().find_map(|line| {
            if line.starts_with('[') {
                in_group = line.trim() == "[Desktop Entry]";
                return None;
            }
            in_group
                .then(|| line.strip_prefix("Exec="))
                .flatten()
                .map(str::to_owned)
        })
    }
    /// The menu entry and file of an installed AppImage, and whether PkgDeck
    /// manages it.
    fn entry_of(&self, id: &PackageId) -> Result<(PathBuf, PathBuf, bool), EngineError> {
        if Self::managed_name(&id.name) && id.scope == self.scope() {
            let entry = self
                .applications
                .join(format!("pkgdeck-{}.desktop", &id.name[8..72]));
            return Ok((entry, self.root.join(&id.name), true));
        }
        self.external_entries()?
            .into_iter()
            .find(|(package, _)| package.id == *id)
            .map(|(_, entry)| (entry, PathBuf::from(&id.name), false))
            .ok_or(EngineError::NotFound)
    }
    /// What the AppImage's menu entry starts, checked to be the AppImage.
    fn launch_command(&self, id: &PackageId) -> Result<(desktop_exec::Command, bool), EngineError> {
        let (entry, file, managed) = self.entry_of(id)?;
        let command = Self::exec_line(&entry)
            .and_then(|exec| desktop_exec::Command::parse(&exec))
            .ok_or_else(|| Self::invalid("its menu entry has no command PkgDeck can read"))?;
        let same = fs::canonicalize(&command.program)
            .ok()
            .zip(fs::canonicalize(&file).ok())
            .is_some_and(|(program, file)| program == file);
        if !same {
            return Err(Self::invalid(
                "its menu entry starts something other than this AppImage",
            ));
        }
        Ok((command, managed))
    }
    pub fn launch_settings(&self, id: &PackageId) -> Result<LaunchSettings, EngineError> {
        let (command, editable) = self.launch_command(id)?;
        Ok(LaunchSettings {
            arguments: command
                .arguments
                .iter()
                .map(|argument| desktop_exec::quote(argument))
                .collect::<Vec<_>>()
                .join(" "),
            environment: command.environment,
            editable,
        })
    }
    /// Change how an AppImage PkgDeck manages starts, from PkgDeck and from
    /// the app menu alike: its entry's `Exec=` line is rewritten.
    pub fn set_launch_settings(
        &self,
        id: &PackageId,
        arguments: &str,
        environment: &[(String, String)],
    ) -> Result<(), EngineError> {
        let (mut command, managed) = self.launch_command(id)?;
        if !managed {
            return Err(Self::invalid(
                "PkgDeck can change only AppImages it manages. Manage it first.",
            ));
        }
        if let Some((name, _)) = environment
            .iter()
            .find(|(name, value)| !desktop_exec::valid_name(name) || value.contains(['\n', '\r']))
        {
            return Err(Self::invalid(format!(
                "{name:?} can't be an environment variable"
            )));
        }
        command.arguments = desktop_exec::split(&arguments.replace(['\n', '\r'], " "))
            .ok_or_else(|| Self::invalid("the arguments have a quote that's never closed"))?;
        command.environment = environment.to_vec();
        let (entry, file, _) = self.entry_of(id)?;
        let exec = command.render(&Self::exec_program(&file));
        let contents = fs::read_to_string(&entry).map_err(|e| Self::invalid(e.to_string()))?;
        let mut in_group = false;
        let mut written = false;
        let updated: String = contents
            .lines()
            .map(|line| {
                if line.starts_with('[') {
                    in_group = line.trim() == "[Desktop Entry]";
                }
                if in_group && !written && line.starts_with("Exec=") {
                    written = true;
                    format!("Exec={exec}\n")
                } else {
                    format!("{line}\n")
                }
            })
            .collect();
        let temporary = entry.with_extension(format!("desktop.{}", std::process::id()));
        fs::write(&temporary, updated)
            .and_then(|()| fs::rename(&temporary, &entry))
            .map_err(|e| {
                let _ = fs::remove_file(&temporary);
                Self::invalid(format!("couldn't save its menu entry: {e}"))
            })
    }
    /// The file `id` starts from: where it is, its size, when it last
    /// changed, and whether it can start the usual way here.
    pub fn file(&self, id: &PackageId) -> Result<AppFile, EngineError> {
        let (_, path, managed) = self.entry_of(id)?;
        let metadata = fs::metadata(&path).map_err(|e| Self::invalid(e.to_string()))?;
        Ok(AppFile {
            without_fuse: Self::needs_fuse2(&path) && !self.has_fuse2(),
            bytes: metadata.len(),
            modified: metadata.mtime(),
            path,
            managed,
        })
    }
    /// Older AppImage runtimes load `libfuse.so.2` to mount themselves;
    /// newer ones carry what they need. The runtime is the part before the
    /// app's files, well within the first megabyte.
    fn needs_fuse2(path: &Path) -> bool {
        let mut start = Vec::new();
        fs::File::open(path)
            .and_then(|file| file.take(1 << 20).read_to_end(&mut start))
            .is_ok_and(|_| start.windows(12).any(|window| window == b"libfuse.so.2"))
    }
    fn has_fuse2(&self) -> bool {
        #[cfg(test)]
        if let Some(present) = self.fuse2 {
            return present;
        }
        FUSE2_LIBRARIES
            .iter()
            .any(|library| Path::new(library).exists())
    }
    /// Start the AppImage as its menu entry does, without files. It runs on
    /// its own, outside PkgDeck's sandbox when PkgDeck is a Flatpak. Without
    /// FUSE 2 an older AppImage can't mount itself, so it starts unpacked.
    pub fn launch(&self, id: &PackageId) -> Result<(), EngineError> {
        let (mut command, _) = self.launch_command(id)?;
        if self.file(id)?.without_fuse {
            command
                .environment
                .push(("APPIMAGE_EXTRACT_AND_RUN".into(), "1".into()));
        }
        let bridge = (Host::current().runtime == crate::host::Runtime::Flatpak)
            .then(|| Path::new("flatpak-spawn"));
        Self::spawn(&command, bridge).map_err(|e| Self::invalid(format!("couldn't start it: {e}")))
    }
    /// Start `command` detached; through `bridge` (`flatpak-spawn`) when it
    /// has to leave a Flatpak sandbox.
    fn spawn(command: &desktop_exec::Command, bridge: Option<&Path>) -> std::io::Result<()> {
        use std::os::unix::process::CommandExt;
        let mut process = if let Some(bridge) = bridge {
            let mut process = std::process::Command::new(bridge);
            process.arg("--host");
            for (name, value) in &command.environment {
                process.arg(format!("--env={name}={value}"));
            }
            process.arg(&command.program);
            process
        } else {
            let mut process = std::process::Command::new(&command.program);
            process.envs(
                command
                    .environment
                    .iter()
                    .map(|(name, value)| (name, value)),
            );
            process
        };
        process
            .args(command.launch_arguments())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .map(drop)
    }
    /// Saved GitHub projects, by managed file name.
    fn sources_file(&self) -> PathBuf {
        self.root.with_file_name("appimage-sources.json")
    }
    fn sources(&self) -> std::collections::BTreeMap<String, String> {
        fs::read(self.sources_file())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
    fn github_source(&self, name: &str) -> Option<String> {
        self.sources().remove(name)
    }
    fn save_github_source(&self, name: &str, repository: Option<&str>) -> Result<(), EngineError> {
        let mut sources = self.sources();
        match repository {
            Some(repository) => sources.insert(name.to_owned(), repository.to_owned()),
            None => sources.remove(name),
        };
        let file = self.sources_file();
        let temporary = file.with_extension(format!("json.{}", std::process::id()));
        fs::create_dir_all(file.parent().unwrap_or(Path::new("/")))
            .and_then(|()| {
                fs::write(
                    &temporary,
                    serde_json::to_vec_pretty(&sources).unwrap_or_default(),
                )
            })
            .and_then(|()| fs::rename(&temporary, &file))
            .map_err(|e| {
                let _ = fs::remove_file(&temporary);
                Self::invalid(format!("couldn't save where it updates from: {e}"))
            })
    }
    pub fn update_source(&self, id: &PackageId) -> Result<UpdateSource, EngineError> {
        let (_, file, managed) = self.entry_of(id)?;
        Ok(UpdateSource {
            github: managed.then(|| self.github_source(&id.name)).flatten(),
            builtin: Self::has_update_metadata(&file).unwrap_or(false),
            editable: managed,
        })
    }
    /// Update an AppImage PkgDeck manages from a GitHub project's releases
    /// (`owner/name` or a link), or stop: an empty `github`.
    pub fn set_update_source(&self, id: &PackageId, github: &str) -> Result<(), EngineError> {
        let (_, _, managed) = self.entry_of(id)?;
        if !managed {
            return Err(Self::invalid(
                "PkgDeck can change only AppImages it manages. Manage it first.",
            ));
        }
        if github.trim().is_empty() {
            return self.save_github_source(&id.name, None);
        }
        let repository = appimage_github::repository(github)
            .ok_or_else(|| Self::invalid("that isn't a GitHub project (owner/name)"))?;
        self.save_github_source(&id.name, Some(&repository))
    }
    fn curl(&self) -> Result<PathBuf, EngineError> {
        #[cfg(test)]
        if let Some(curl) = &self.curl {
            return Ok(curl.clone());
        }
        Host::current()
            .resolve("curl")?
            .ok_or_else(|| Self::invalid("curl is unavailable"))
    }
    /// A project's releases, newest first. Answers are kept and reused for a
    /// while, and asked again with their ETag, which GitHub doesn't count.
    fn github_releases(
        &self,
        repository: &str,
        cancel: &Cancellation,
    ) -> Result<serde_json::Value, EngineError> {
        let cache = self.root.with_file_name("appimage-github");
        let file = cache.join(format!("{}.json", repository.replace('/', "__")));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let kept: Option<serde_json::Value> = fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        if let Some(kept) = kept.as_ref().filter(|kept| {
            kept["fetched"]
                .as_u64()
                .is_some_and(|fetched| now.saturating_sub(fetched) < GITHUB_REUSE_SECONDS)
        }) {
            return Ok(kept["releases"].clone());
        }
        fs::create_dir_all(&cache).map_err(|e| Self::invalid(e.to_string()))?;
        let stamp = format!("{}-{}", std::process::id(), now);
        let headers = cache.join(format!(".headers-{stamp}"));
        let body = cache.join(format!(".body-{stamp}"));
        let mut command = std::process::Command::new(self.curl()?);
        command.args([
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-filesize",
            "16777216",
            "--connect-timeout",
            "5",
            "--max-time",
            "30",
            "--header",
            "Accept: application/vnd.github+json",
            "--write-out",
            "%{http_code}",
        ]);
        if let Some(etag) = kept.as_ref().and_then(|kept| kept["etag"].as_str()) {
            command
                .arg("--header")
                .arg(format!("If-None-Match: {etag}"));
        }
        command
            .arg("--dump-header")
            .arg(&headers)
            .arg("--output")
            .arg(&body)
            .arg(format!(
                "https://api.github.com/repos/{repository}/releases?per_page=20"
            ));
        let result = process::run(
            command,
            Limits {
                timeout: std::time::Duration::from_secs(35),
                output_bytes: 1024,
            },
            cancel,
            false,
        );
        let header_text = fs::read_to_string(&headers).unwrap_or_default();
        let body_bytes = fs::read(&body).unwrap_or_default();
        let _ = fs::remove_file(&headers);
        let _ = fs::remove_file(&body);
        let result = result?;
        let status = String::from_utf8_lossy(&result.stdout).trim().to_owned();
        let releases = match (status.as_str(), kept) {
            ("304", Some(kept)) => kept["releases"].clone(),
            ("200", _) => serde_json::from_slice(&body_bytes)
                .map_err(|_| Self::invalid("GitHub's answer couldn't be read"))?,
            ("403" | "429", _) => {
                return Err(Self::invalid(
                    "GitHub is limiting how often PkgDeck can ask. It tries again later.",
                ))
            }
            ("404", _) => return Err(Self::invalid(format!("GitHub has no project {repository}"))),
            _ => return Err(Self::invalid(format!("GitHub didn't answer ({status})"))),
        };
        let etag = header_text.lines().rev().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("etag")
                .then(|| value.trim().to_owned())
        });
        let record = serde_json::json!({"fetched": now, "etag": etag, "releases": releases});
        let _ = fs::write(&file, serde_json::to_vec(&record).unwrap_or_default());
        Ok(releases)
    }
    /// Whether the project has a newer AppImage than the file at `path`: by
    /// version when both have one (in order when both read as versions),
    /// else by GitHub's checksum.
    fn github_check(
        &self,
        path: &Path,
        repository: &str,
        architecture: &str,
        cancel: &Cancellation,
    ) -> Result<(UpdateAvailability, Option<String>), EngineError> {
        let releases = self.github_releases(repository, cancel)?;
        let Some(asset) = appimage_github::newest(&releases, architecture) else {
            return Ok((UpdateAvailability::Unknown, None));
        };
        let theirs = appimage_github::plain_version(&asset.tag);
        let offered = Some(theirs.to_owned());
        if let Some(ours) = Self::contents(path).version.clone() {
            let ours = appimage_github::plain_version(&ours);
            let newer = match (
                appimage_github::ordered(theirs),
                appimage_github::ordered(ours),
            ) {
                (Some(theirs), Some(ours)) => theirs > ours,
                _ => theirs != ours,
            };
            let update = if newer {
                UpdateAvailability::Available
            } else {
                UpdateAvailability::Current
            };
            return Ok((update, offered));
        }
        let Some(sha256) = asset.sha256 else {
            return Ok((UpdateAvailability::Unknown, None));
        };
        let update = if Self::digest(path, cancel)? == sha256 {
            UpdateAvailability::Current
        } else {
            UpdateAvailability::Available
        };
        Ok((update, offered))
    }
    /// Replace the managed file at `target` with the project's newest
    /// AppImage, checked against GitHub's checksum when it has one, and
    /// only if it is a Type 2 AppImage for the same computer.
    fn github_update(
        &self,
        target: &Path,
        repository: &str,
        architecture: &str,
        cancel: &Cancellation,
    ) -> Result<(), EngineError> {
        let releases = self.github_releases(repository, cancel)?;
        let asset = appimage_github::newest(&releases, architecture).ok_or_else(|| {
            Self::invalid(format!("{repository} has no AppImage for this computer"))
        })?;
        if asset.size > MAX_IMPORT_BYTES {
            return Err(Self::invalid("its newest AppImage is larger than 2 GiB"));
        }
        let temporary = self.root.join(format!(
            ".pkgdeck-update-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let result = (|| {
            let mut command = std::process::Command::new(self.curl()?);
            command
                .args([
                    "--silent",
                    "--show-error",
                    "--fail",
                    "--location",
                    "--proto",
                    "=https",
                    "--proto-redir",
                    "=https",
                    "--max-redirs",
                    "5",
                    "--max-filesize",
                ])
                .arg(MAX_IMPORT_BYTES.to_string())
                .args(["--connect-timeout", "5", "--max-time", "1800", "--output"])
                .arg(&temporary)
                .arg(&asset.url);
            let result = process::run(
                command,
                Limits {
                    timeout: std::time::Duration::from_secs(1805),
                    output_bytes: 16 * 1024,
                },
                cancel,
                true,
            )?;
            if result.code != Some(0) {
                return Err(Self::invalid(format!("downloading {} failed", asset.name)));
            }
            if let Some(sha256) = &asset.sha256 {
                Self::verify_digest(&temporary, sha256, cancel)?;
            }
            if Self::type2(&temporary)? != architecture {
                return Err(Self::invalid(
                    "the new AppImage is for another kind of computer",
                ));
            }
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))
                .and_then(|()| fs::rename(&temporary, target))
                .map_err(|e| Self::invalid(e.to_string()))
        })();
        let _ = fs::remove_file(&temporary);
        result
    }
    fn updater(&self) -> Result<PathBuf, EngineError> {
        #[cfg(test)]
        if let Some(updater) = &self.updater {
            return Ok(updater.clone());
        }
        let executable = std::env::current_exe().map_err(|e| Self::invalid(e.to_string()))?;
        let helper = executable
            .parent()
            .and_then(Path::parent)
            .map(|root| root.join("lib/pkgdeck/appimageupdatetool.AppImage"))
            .ok_or_else(|| Self::invalid("cannot locate bundled AppImage updater"))?;
        helper
            .is_file()
            .then_some(helper)
            .ok_or_else(|| Self::invalid("bundled AppImage updater is unavailable"))
    }
    /// Metadata identifies the update source, not whether it has a newer file.
    /// Only the bundled updater's read-only probe can establish availability.
    fn check_update(
        &self,
        path: &Path,
        cancel: &Cancellation,
    ) -> Result<UpdateAvailability, EngineError> {
        let Ok(updater) = self.updater() else {
            return Ok(UpdateAvailability::Unknown);
        };
        let mut command = std::process::Command::new(updater);
        command.args(["--appimage-extract-and-run", "--check-for-update", "--"]);
        command.arg(path);
        let result = process::run(
            command,
            Limits {
                timeout: std::time::Duration::from_secs(60),
                ..Limits::default()
            },
            cancel,
            false,
        )?;
        match result.code {
            Some(0) => Ok(UpdateAvailability::Current),
            Some(1) => Ok(UpdateAvailability::Available),
            _ => Err(ExecutionError::Failed(result).into()),
        }
    }
    fn update(&self, id: &PackageId, cancel: &Cancellation) -> Result<(), EngineError> {
        let managed = Self::managed_name(&id.name) && id.scope == self.scope();
        if let Some(repository) = managed.then(|| self.github_source(&id.name)).flatten() {
            return self.github_update(
                &self.root.join(&id.name),
                &repository,
                &id.architecture,
                cancel,
            );
        }
        let target = if managed {
            self.root.join(&id.name)
        } else {
            self.external_entries()?
                .into_iter()
                .find(|(package, _)| package.id == *id)
                .map(|(_, _)| PathBuf::from(&id.name))
                .ok_or(EngineError::NotFound)?
        };
        let mut command = std::process::Command::new(self.updater()?);
        command.args([
            "--appimage-extract-and-run",
            "--overwrite",
            "--remove-old",
            "--",
        ]);
        command.arg(target);
        let result = process::run(
            command,
            Limits {
                timeout: std::time::Duration::from_secs(600),
                output_bytes: 32 * 1024 * 1024,
            },
            cancel,
            true,
        )?;
        if result.code == Some(0) {
            Ok(())
        } else {
            Err(ExecutionError::Failed(result).into())
        }
    }
}

impl Backend for AppImage {
    fn id(&self) -> &str {
        "appimage"
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        #[cfg(not(target_os = "linux"))]
        return Ok(Availability::Unavailable("AppImage requires Linux".into()));
        #[cfg(target_os = "linux")]
        Ok(Availability::Available)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let source = PathBuf::from(query);
        if source.is_absolute() && source.symlink_metadata().is_ok() {
            let architecture = Self::type2(&source)?;
            let digest = Self::digest(&source, cancel)?;
            let destination = self.root.join(format!("pkgdeck-{digest}.AppImage"));
            let desktop = self.applications.join(format!("pkgdeck-{digest}.desktop"));
            let already_imported = Self::existing_import(&destination, &desktop, &digest)?;
            let contents = Self::contents(&source);
            let icon = contents
                .icon
                .as_ref()
                .and_then(|icon| self.save_icon(&format!("pkgdeck-{digest}"), icon));
            let version = contents.version.clone();
            return Ok(vec![Package {
                id: PackageId {
                    backend: "appimage".into(),
                    name: query.into(),
                    architecture,
                    scope: Scope::Environment {
                        path: self.root.clone(),
                    },
                    remote: None,
                    reference: Some(digest),
                },
                display_name: contents.name.clone().unwrap_or_else(|| {
                    source
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("AppImage")
                        .into()
                }),
                summary: contents.comment.clone().unwrap_or_default(),
                installed_version: already_imported
                    .then(|| version.clone().unwrap_or_else(|| UNKNOWN_VERSION.into())),
                candidate_version: version,
                update: UpdateAvailability::Unknown,
                icon,
                component_ids: vec![],
                homepages: vec![],
                // Already installed some other way: PkgDeck would move it in.
                adopt_with: self.external_at(&source).map(|_| "appimage".into()),
            }]);
        }
        let query = query.to_ascii_lowercase();
        Ok(self
            .installed_packages()?
            .into_iter()
            .filter(|p| {
                p.id.name.to_ascii_lowercase().contains(&query)
                    || p.display_name.to_ascii_lowercase().contains(&query)
            })
            .collect())
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let mut packages = self.installed_packages()?;
        for package in &mut packages {
            if package.update == UpdateAvailability::Unknown {
                let managed = Self::managed_name(&package.id.name);
                let path = if managed {
                    self.root.join(&package.id.name)
                } else {
                    PathBuf::from(&package.id.name)
                };
                package.update = match managed
                    .then(|| self.github_source(&package.id.name))
                    .flatten()
                {
                    // GitHub not answering leaves it unknown, never the list.
                    Some(repository) => {
                        let (update, offered) = self
                            .github_check(&path, &repository, &package.id.architecture, cancel)
                            .unwrap_or((UpdateAvailability::Unknown, None));
                        if update == UpdateAvailability::Available {
                            // The version on offer, when GitHub names one.
                            package.candidate_version =
                                offered.or(package.candidate_version.take());
                        }
                        update
                    }
                    None => self.check_update(&path, cancel)?,
                };
            }
        }
        Ok(packages)
    }
    fn details(&mut self, id: &PackageId, _: &Cancellation) -> Result<PackageDetails, EngineError> {
        let package = self
            .installed_packages()?
            .into_iter()
            .find(|p| p.id == *id)
            .ok_or(EngineError::NotFound)?;
        let path = if Self::managed_name(&package.id.name) {
            self.root.join(&package.id.name)
        } else {
            PathBuf::from(&package.id.name)
        };
        Ok(PackageDetails {
            description: Self::contents(&path).comment.clone().unwrap_or_default(),
            homepage: None,
            dependencies: vec![],
            package,
        })
    }
    /// Installing an AppImage that's already installed some other way moves
    /// it in; say so, so the move is reviewed before it happens.
    fn operation_plan(
        &mut self,
        operation: &Operation,
        _: &Cancellation,
    ) -> Result<Option<TransactionPlan>, EngineError> {
        let Operation::Install(id) = operation else {
            return Ok(None);
        };
        let source = Path::new(&id.name);
        if id.scope != self.import_scope() || !source.is_absolute() {
            return Ok(None);
        }
        let Some(original) = self.external_at(source) else {
            return Ok(None);
        };
        let contents = Self::contents(&original);
        let name = contents.name.clone().unwrap_or_else(|| {
            source
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| id.name.clone())
        });
        Ok(Some(TransactionPlan {
            operation: operation.clone(),
            native_preview: format!(
                "Moves {} into PkgDeck's folder and replaces its menu entries.",
                Self::shown_path(&original)
            ),
            changes: vec![PlannedChange {
                action: PlannedAction::Install,
                name,
                installed_version: None,
                candidate_version: contents.version.clone(),
            }],
            download_bytes: None,
            disk_bytes: None,
            restart_required: None,
            adopts: Some(original),
        }))
    }
    /// Copies of an app PkgDeck already manages, installed some other way
    /// (left in Downloads with a menu entry, or by Gear Lever): each can go.
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let packages = self.installed_packages()?;
        let plain = |name: &str| name.to_lowercase().replace([' ', '-', '_', '.'], "");
        let managed: std::collections::BTreeSet<String> = packages
            .iter()
            .filter(|package| Self::managed_name(&package.id.name))
            .map(|package| plain(&package.display_name))
            .collect();
        Ok(packages
            .iter()
            .filter(|package| {
                !Self::managed_name(&package.id.name)
                    && !package.display_name.is_empty()
                    && managed.contains(&plain(&package.display_name))
            })
            .map(|package| {
                let path = Self::shown_path(Path::new(&package.id.name));
                CleanupItem {
                    id: CleanupId {
                        backend: "appimage".into(),
                        key: package.id.name.clone(),
                    },
                    kind: CleanupKind::DuplicateCopy,
                    title: format!("Extra copy of {}", package.display_name),
                    summary: format!(
                        "PkgDeck already manages {}. This copy and its menu entries can go.",
                        package.display_name
                    ),
                    preview: format!("Removes {path} and its menu entries."),
                }
            })
            .collect())
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        // Only a copy that's still an extra one goes, checked again now.
        if let Operation::Clean(id) = operation {
            let extra = self.cleanup(cancel)?.into_iter().any(|item| item.id == *id);
            let package = self
                .installed_packages()?
                .into_iter()
                .find(|package| extra && package.id.name == id.key)
                .ok_or(EngineError::NotFound)?;
            return self.execute(&Operation::Remove(package.id), cancel, progress);
        }
        match operation {
            Operation::Install(id) => {
                progress(Progress::Message(
                    "Importing AppImage without running it.".into(),
                ));
                self.import(id, cancel)?;
            }
            Operation::Remove(id)
                if id.backend == "appimage"
                    && id.scope == self.scope()
                    && Self::managed_name(&id.name) =>
            {
                progress(Progress::Message(
                    "Removing the AppImage and its desktop entry.".into(),
                ));
                fs::remove_file(self.root.join(&id.name))
                    .map_err(|e| Self::invalid(e.to_string()))?;
                let _ = fs::remove_file(
                    self.applications
                        .join(format!("pkgdeck-{}.desktop", &id.name[8..72])),
                );
                self.remove_icons(&id.name[..72]);
                let _ = self.save_github_source(&id.name, None);
            }
            Operation::Remove(id)
                if id.backend == "appimage" && id.scope == self.scope() && id.remote.is_none() =>
            {
                let (package, desktop) = self
                    .external_entries()?
                    .into_iter()
                    .find(|(package, _)| package.id == *id)
                    .ok_or(EngineError::NotFound)?;
                progress(Progress::Message(format!(
                    "Removing external AppImage {} and its desktop entry.",
                    package.display_name
                )));
                // Other entries for the same file (Gear Lever leaves several),
                // found while the file still exists.
                let others = self.entries_for(Path::new(&id.name));
                fs::remove_file(&id.name).map_err(|e| Self::invalid(e.to_string()))?;
                fs::remove_file(&desktop).map_err(|e| Self::invalid(e.to_string()))?;
                for other in others.into_iter().filter(|other| *other != desktop) {
                    let _ = fs::remove_file(other);
                }
                self.remove_icons(&Self::external_icon_key(Path::new(&id.name)));
            }
            Operation::Upgrade(id) if id.backend == "appimage" => {
                progress(Progress::Message(
                    "Updating AppImage with its built-in updater.".into(),
                ));
                self.update(id, cancel)?;
            }
            Operation::UpgradeAll { backend } if backend == "appimage" => {
                for package in self.installed(cancel)? {
                    if package.update == UpdateAvailability::Available {
                        self.update(&package.id, cancel)?;
                    }
                }
            }
            _ => return Err(Self::invalid("foreign or unsupported AppImage operation")),
        }
        Ok(OperationOutcome::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type2(path: &Path) {
        let mut header = [0_u8; 64];
        header[..4].copy_from_slice(b"\x7fELF");
        header[4] = 2;
        header[5] = 1;
        header[6] = 1;
        header[8..11].copy_from_slice(b"AI\x02");
        header[18..20].copy_from_slice(&62_u16.to_le_bytes());
        header[20..24].copy_from_slice(&1_u32.to_le_bytes());
        header[52..54].copy_from_slice(&64_u16.to_le_bytes());
        fs::write(path, header).unwrap();
    }

    fn ignore(_: Progress) {}

    #[cfg(target_os = "linux")]
    fn appimage_with(
        path: &Path,
        compressor: backhand::compression::Compressor,
        files: &[(&str, &[u8])],
    ) {
        appimage_contents::fixture::squash(
            path,
            compressor,
            files,
            &[(".DirIcon", "demo.png")],
            &[],
        );
    }

    #[cfg(target_os = "linux")]
    const DEMO_ENTRY: &[u8] = b"[Desktop Entry]\nType=Application\nName=Demo App\nComment=Does demo things\nIcon=demo\nExec=AppRun --no-sandbox %U\nCategories=Game;\nStartupWMClass=demo-app\nX-AppImage-Version=2.0\n\n[Desktop Action New]\nName=Wrong\n";
    #[cfg(target_os = "linux")]
    const DEMO_PNG: &[u8] = b"\x89PNG\r\n\x1a\ndemo icon";

    #[test]
    #[cfg(target_os = "linux")]
    fn reads_name_version_icon_and_arguments_from_inside_the_appimage() {
        use backhand::compression::Compressor;
        let base = test_base("contents");
        for (index, compressor) in [Compressor::Gzip, Compressor::Xz, Compressor::Zstd]
            .into_iter()
            .enumerate()
        {
            let path = base.join(format!("demo-{index}.AppImage"));
            appimage_with(
                &path,
                compressor,
                &[("demo.desktop", DEMO_ENTRY), ("demo.png", DEMO_PNG)],
            );
            let contents = appimage_contents::read(&path).unwrap();
            assert_eq!(contents.name.as_deref(), Some("Demo App"));
            assert_eq!(contents.comment.as_deref(), Some("Does demo things"));
            assert_eq!(contents.version.as_deref(), Some("2.0"));
            assert_eq!(contents.categories.as_deref(), Some("Game;"));
            assert_eq!(contents.startup_wm_class.as_deref(), Some("demo-app"));
            assert_eq!(contents.arguments.as_deref(), Some("--no-sandbox %U"));
            assert_eq!(
                contents
                    .icon
                    .as_ref()
                    .map(|icon| (icon.bytes.as_slice(), icon.extension)),
                Some((DEMO_PNG, "png"))
            );
        }
        // Without a named icon, .DirIcon is used; without an entry, nothing.
        let bare = base.join("bare.AppImage");
        appimage_with(&bare, Compressor::Gzip, &[("demo.png", DEMO_PNG)]);
        let contents = appimage_contents::read(&bare).unwrap();
        assert_eq!(contents.name, None);
        assert_eq!(
            contents.icon.map(|icon| icon.bytes),
            Some(DEMO_PNG.to_vec())
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn an_extra_copy_of_a_managed_appimage_is_cleaned_up_or_moved_in() {
        let base = test_base("extra");
        let uid = rustix::process::getuid().as_raw();
        let root = base.join("data/pkgdeck/appimages");
        let applications = base.join("data/applications");
        fs::create_dir_all(&applications).unwrap();
        let folder = base.join("AppImages");
        fs::create_dir_all(&folder).unwrap();
        let original = folder.join("demo.appimage");
        appimage_with(
            &original,
            backhand::compression::Compressor::Gzip,
            &[("demo.desktop", DEMO_ENTRY), ("demo.png", DEMO_PNG)],
        );
        let mut backend = AppImage::new(root.clone(), applications.clone(), uid);
        let cancel = Cancellation::default();
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert_eq!(backend.cleanup(&cancelled), Err(EngineError::Cancelled));
        // PkgDeck manages one copy; the file it came from has no menu entry.
        let preview = backend
            .search(original.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        let digest = preview.id.reference.clone().unwrap();
        let architecture = preview.id.architecture.clone();
        backend
            .execute(&Operation::Install(preview.id), &cancel, &mut ignore)
            .unwrap();
        assert!(backend.cleanup(&cancel).unwrap().is_empty());
        // Another copy with its own menu entry is an extra one.
        let copy = folder.join("demo-copy.appimage");
        let entry = applications.join("demo-copy.desktop");
        let place_copy = || {
            fs::copy(&original, &copy).unwrap();
            fs::write(
                &entry,
                format!(
                    "[Desktop Entry]\nName=Demo App\nExec=\"{}\" %U\n",
                    copy.display()
                ),
            )
            .unwrap();
        };
        place_copy();
        let items = backend.cleanup(&cancel).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, CleanupKind::DuplicateCopy);
        assert_eq!(items[0].title, "Extra copy of Demo App");
        // Only a copy that's still an extra one goes.
        let elsewhere = CleanupId {
            backend: "appimage".into(),
            key: base.join("elsewhere.appimage").display().to_string(),
        };
        assert_eq!(
            backend.execute(&Operation::Clean(elsewhere), &cancel, &mut ignore),
            Err(EngineError::NotFound)
        );
        backend
            .execute(&Operation::Clean(items[0].id.clone()), &cancel, &mut ignore)
            .unwrap();
        assert!(!copy.exists());
        assert!(!entry.exists());
        assert!(backend.cleanup(&cancel).unwrap().is_empty());
        // Managing a copy identical to the managed one retires that copy too.
        place_copy();
        let again = backend
            .search(copy.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        backend
            .execute(&Operation::Install(again.id), &cancel, &mut ignore)
            .unwrap();
        assert!(!copy.exists());
        assert!(!entry.exists());
        // Its page shows the file it starts from.
        let managed = backend.installed_id(&digest, &architecture);
        let file = backend.file(&managed).unwrap();
        assert!(file.managed);
        assert_eq!(file.path, root.join(&managed.name));
        assert_eq!(file.bytes, fs::metadata(&original).unwrap().len());
        assert!(file.modified > 0);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn appimages_that_need_fuse2_start_unpacked_without_it() {
        let base = test_base("fuse");
        let uid = rustix::process::getuid().as_raw();
        let root = base.join("data/pkgdeck/appimages");
        let applications = base.join("data/applications");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&applications).unwrap();
        let mut backend = AppImage::new(root.clone(), applications.clone(), uid);
        let id = backend.installed_id(&"ef".repeat(32), "x86_64");
        let file = root.join(&id.name);
        let record = base.join("record");
        // An older runtime names libfuse.so.2, which it loads to mount itself.
        fs::write(
            &file,
            format!(
                "#!/bin/sh\n# libfuse.so.2\nprintf '%s\\n' \"$APPIMAGE_EXTRACT_AND_RUN\" > '{}'\n",
                record.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            applications.join(format!("pkgdeck-{}.desktop", &id.name[8..72])),
            format!(
                "[Desktop Entry]\nName=Old\nExec=\"{}\" %U\n",
                file.display()
            ),
        )
        .unwrap();
        // A script written a moment ago may still be busy; try again.
        let launch = |backend: &AppImage| {
            let _ = fs::remove_file(&record);
            (0..200)
                .map(|_| backend.launch(&id))
                .inspect(|_| std::thread::sleep(std::time::Duration::from_millis(1)))
                .find(Result::is_ok)
                .unwrap()
                .unwrap();
            (0..200)
                .find_map(|_| {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    fs::read_to_string(&record)
                        .ok()
                        .filter(|text| text.ends_with('\n'))
                })
                .unwrap()
        };
        backend.fuse2 = Some(true);
        assert!(!backend.file(&id).unwrap().without_fuse);
        assert_eq!(launch(&backend), "\n");
        backend.fuse2 = Some(false);
        assert!(backend.file(&id).unwrap().without_fuse);
        assert_eq!(launch(&backend), "1\n");
        // This computer's own answer.
        backend.fuse2 = None;
        let here = FUSE2_LIBRARIES
            .iter()
            .any(|library| Path::new(library).exists());
        assert_eq!(backend.file(&id).unwrap().without_fuse, !here);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn manage_moves_an_external_appimage_in_with_a_full_desktop_entry() {
        let base = test_base("manage");
        let uid = rustix::process::getuid().as_raw();
        let root = base.join("data/pkgdeck/appimages");
        let applications = base.join("data/applications");
        fs::create_dir_all(&applications).unwrap();
        let folder = base.join("AppImages");
        fs::create_dir_all(&folder).unwrap();
        let original = folder.join("demo.appimage");
        appimage_with(
            &original,
            backhand::compression::Compressor::Gzip,
            &[("demo.desktop", DEMO_ENTRY), ("demo.png", DEMO_PNG)],
        );
        // Gear Lever style: two entries for one file, with a stale version,
        // and a variable someone set there, which carries over.
        for name in ["demo.desktop", "gearlever_demo.desktop"] {
            fs::write(
                applications.join(name),
                format!(
                    "[Desktop Entry]\nName=Demo App\nExec=env DESKTOPINTEGRATION=1 \"{}\" --no-sandbox %U\nX-AppImage-Version=1.0\n",
                    original.display()
                ),
            )
            .unwrap();
        }
        let mut backend = AppImage::new(root.clone(), applications.clone(), uid);
        let cancel = Cancellation::default();
        let external = backend.installed(&cancel).unwrap();
        assert_eq!(external.len(), 1);
        assert_eq!(external[0].installed_version.as_deref(), Some("2.0"));
        // What the app says it is; its page shows where the file is.
        assert_eq!(external[0].summary, "Does demo things");
        assert_eq!(external[0].adopt_with.as_deref(), Some("appimage"));

        let preview = backend
            .search(original.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        assert_eq!(preview.display_name, "Demo App");
        assert_eq!(preview.summary, "Does demo things");
        assert_eq!(preview.candidate_version.as_deref(), Some("2.0"));
        assert_eq!(preview.installed_version, None);
        let digest = preview.id.reference.clone().unwrap();
        let icon = base.join(format!("data/pkgdeck/appimage-icons/pkgdeck-{digest}.png"));
        assert_eq!(preview.icon.as_deref(), Some(icon.as_path()));

        assert_eq!(preview.adopt_with.as_deref(), Some("appimage"));
        let install = Operation::Install(preview.id);
        let plan = backend.operation_plan(&install, &cancel).unwrap().unwrap();
        assert_eq!(
            plan.adopts.as_deref(),
            Some(original.canonicalize().unwrap().as_path())
        );
        assert_eq!(plan.changes[0].name, "Demo App");
        assert_eq!(plan.changes[0].candidate_version.as_deref(), Some("2.0"));
        backend.execute(&install, &cancel, &mut ignore).unwrap();
        assert!(!original.exists());
        assert!(!applications.join("demo.desktop").exists());
        assert!(!applications.join("gearlever_demo.desktop").exists());
        let managed = root.join(format!("pkgdeck-{digest}.AppImage"));
        let entry =
            fs::read_to_string(applications.join(format!("pkgdeck-{digest}.desktop"))).unwrap();
        assert_eq!(
            entry,
            format!(
                "[Desktop Entry]\nType=Application\nName=Demo App\nComment=Does demo things\nIcon={}\nExec=env DESKTOPINTEGRATION=1 \"{}\" --no-sandbox %U\nTryExec={}\nTerminal=false\nCategories=Game;\nStartupWMClass=demo-app\nX-AppImage-Version=2.0\n",
                icon.display(),
                managed.display(),
                managed.display()
            )
        );
        assert_eq!(fs::read(&icon).unwrap(), DEMO_PNG);

        let installed = backend.installed(&cancel).unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].display_name, "Demo App");
        assert_eq!(installed[0].summary, "Does demo things");
        assert_eq!(installed[0].installed_version.as_deref(), Some("2.0"));
        assert_eq!(installed[0].icon.as_deref(), Some(icon.as_path()));
        assert_eq!(installed[0].adopt_with, None);
        assert_eq!(
            backend
                .details(&installed[0].id, &cancel)
                .unwrap()
                .description,
            "Does demo things"
        );

        backend
            .execute(
                &Operation::Remove(installed[0].id.clone()),
                &cancel,
                &mut ignore,
            )
            .unwrap();
        assert!(!managed.exists());
        assert!(!icon.exists());
        assert!(backend.installed(&cancel).unwrap().is_empty());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn manage_plans_only_installs_of_appimages_installed_elsewhere() {
        let base = test_base("plans");
        let uid = rustix::process::getuid().as_raw();
        let applications = base.join("applications");
        fs::create_dir_all(&applications).unwrap();
        let mut backend = AppImage::new(base.join("pkgdeck/appimages"), applications.clone(), uid);
        let cancel = Cancellation::default();
        // No desktop entry inside: the plan names the file.
        let original = base.join("bare.appimage");
        appimage_with(
            &original,
            backhand::compression::Compressor::Gzip,
            &[("demo.png", DEMO_PNG)],
        );
        let mut id = backend
            .search(original.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0)
            .id;
        // Not installed elsewhere yet: an ordinary install, nothing to plan.
        assert_eq!(
            backend
                .operation_plan(&Operation::Install(id.clone()), &cancel)
                .unwrap(),
            None
        );
        fs::write(
            applications.join("bare.desktop"),
            format!(
                "[Desktop Entry]\nName=Bare\nExec={} %U\n",
                original.display()
            ),
        )
        .unwrap();
        let plan = backend
            .operation_plan(&Operation::Install(id.clone()), &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(plan.changes[0].name, "bare.appimage");
        assert_eq!(plan.changes[0].candidate_version, None);
        assert_eq!(
            plan.native_preview,
            format!(
                "Moves {} into PkgDeck's folder and replaces its menu entries.",
                original.display()
            )
        );
        // Other operations and identities are never planned.
        assert_eq!(
            backend
                .operation_plan(&Operation::Remove(id.clone()), &cancel)
                .unwrap(),
            None
        );
        id.scope = Scope::System;
        assert_eq!(
            backend
                .operation_plan(&Operation::Install(id.clone()), &cancel)
                .unwrap(),
            None
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn manage_reports_an_old_copy_it_cannot_remove() {
        let base = test_base("stuck-original");
        let uid = rustix::process::getuid().as_raw();
        let applications = base.join("applications");
        fs::create_dir_all(&applications).unwrap();
        let folder = base.join("readonly");
        fs::create_dir_all(&folder).unwrap();
        let original = folder.join("demo.appimage");
        appimage_with(
            &original,
            backhand::compression::Compressor::Gzip,
            &[("demo.desktop", DEMO_ENTRY), ("demo.png", DEMO_PNG)],
        );
        fs::write(
            applications.join("demo.desktop"),
            format!(
                "[Desktop Entry]\nName=Demo\nExec={} %U\n",
                original.display()
            ),
        )
        .unwrap();
        let mut backend = AppImage::new(base.join("pkgdeck/appimages"), applications.clone(), uid);
        let cancel = Cancellation::default();
        let id = backend
            .search(original.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0)
            .id;
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o555)).unwrap();
        // Needs a non-root user: root ignores directory permissions.
        let result = backend.execute(&Operation::Install(id), &cancel, &mut ignore);
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("PkgDeck now manages this AppImage, but couldn't remove the old copy"),
            "{error}"
        );
        assert!(original.exists());
        // The managed copy works and the old entry is gone, so it's listed once.
        let installed = backend.installed(&cancel).unwrap();
        assert_eq!(installed.len(), 1);
        assert!(AppImage::managed_name(&installed[0].id.name));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn icons_that_cannot_be_saved_are_skipped() {
        let base = test_base("icons");
        let backend = AppImage::new(
            base.join("pkgdeck/appimages"),
            base.join("applications"),
            1000,
        );
        let icon = Icon {
            bytes: DEMO_PNG.to_vec(),
            extension: "png",
        };
        let saved = backend.save_icon("demo", &icon).unwrap();
        assert_eq!(fs::read(&saved).unwrap(), DEMO_PNG);
        // The same icon again is left as is.
        assert_eq!(backend.save_icon("demo", &icon), Some(saved));
        fs::set_permissions(backend.icons(), fs::Permissions::from_mode(0o555)).unwrap();
        // Needs a non-root user: root ignores directory permissions.
        let unwritable = backend.save_icon("other", &icon);
        fs::set_permissions(backend.icons(), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(unwritable, None);
        assert_eq!(fs::read_dir(backend.icons()).unwrap().count(), 1);
        fs::remove_dir_all(base).unwrap();
    }

    /// A managed AppImage, imported from a fixture with `--no-sandbox %U`.
    #[cfg(target_os = "linux")]
    fn managed_demo(base: &Path) -> (AppImage, PackageId, PathBuf) {
        let uid = rustix::process::getuid().as_raw();
        let applications = base.join("applications");
        fs::create_dir_all(&applications).unwrap();
        let source = base.join("demo.AppImage");
        appimage_with(
            &source,
            backhand::compression::Compressor::Gzip,
            &[("demo.desktop", DEMO_ENTRY), ("demo.png", DEMO_PNG)],
        );
        let mut backend = AppImage::new(base.join("pkgdeck/appimages"), applications, uid);
        let cancel = Cancellation::default();
        let id = backend
            .search(source.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0)
            .id;
        backend
            .execute(&Operation::Install(id), &cancel, &mut ignore)
            .unwrap();
        let installed = backend.installed(&cancel).unwrap().remove(0);
        let entry = base.join(format!(
            "applications/pkgdeck-{}.desktop",
            &installed.id.name[8..72]
        ));
        (backend, installed.id, entry)
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn launch_settings_live_in_the_menu_entry() {
        let base = test_base("launch-settings");
        let (mut backend, id, entry) = managed_demo(&base);
        let file = base.join("pkgdeck/appimages").join(&id.name);
        assert_eq!(
            backend.launch_settings(&id).unwrap(),
            LaunchSettings {
                arguments: "--no-sandbox %U".into(),
                environment: vec![],
                editable: true
            }
        );
        backend
            .set_launch_settings(
                &id,
                "--no-sandbox --name \"My App\" %U",
                &[
                    ("DESKTOPINTEGRATION".into(), "1".into()),
                    ("GREETING".into(), "hi there".into()),
                ],
            )
            .unwrap();
        let written = fs::read_to_string(&entry).unwrap();
        assert!(
            written.contains(&format!("\nExec=env DESKTOPINTEGRATION=1 \"GREETING=hi there\" \"{}\" --no-sandbox --name \"My App\" %U\n", file.display())),
            "{written}"
        );
        // Everything else in the entry stays as it was.
        assert!(written.starts_with("[Desktop Entry]\nType=Application\nName=Demo App\n"));
        assert!(written.contains("\nStartupWMClass=demo-app\n"));
        let settings = backend.launch_settings(&id).unwrap();
        assert_eq!(settings.arguments, "--no-sandbox --name \"My App\" %U");
        assert_eq!(
            settings.environment[1],
            ("GREETING".into(), "hi there".into())
        );
        // Opening the same file again still finds it installed.
        let cancel = Cancellation::default();
        let again = backend
            .search(base.join("demo.AppImage").to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        assert!(again.installed_version.is_some());
        // Bad input changes nothing.
        for (arguments, environment) in [
            ("\"unclosed", vec![]),
            ("", vec![("1BAD".to_owned(), "x".to_owned())]),
            ("", vec![("OK".to_owned(), "line\nbreak".to_owned())]),
        ] {
            assert!(backend
                .set_launch_settings(&id, arguments, &environment)
                .is_err());
        }
        assert_eq!(fs::read_to_string(&entry).unwrap(), written);
        // An entry PkgDeck can't read, or that starts something else, is reported.
        fs::write(&entry, "[Desktop Entry]\nName=Demo\n").unwrap();
        assert!(backend
            .launch_settings(&id)
            .unwrap_err()
            .to_string()
            .contains("no command"));
        fs::write(&entry, "[Desktop Entry]\nExec=/bin/true\n").unwrap();
        assert!(backend
            .launch(&id)
            .unwrap_err()
            .to_string()
            .contains("something other"));
        // The fixture is no real program: starting it fails, and says so.
        fs::write(&entry, &written).unwrap();
        assert!(backend
            .launch(&id)
            .unwrap_err()
            .to_string()
            .contains("couldn't start it"));
        // An entry that can't be saved leaves no partial file behind.
        let applications = base.join("applications");
        fs::set_permissions(&applications, fs::Permissions::from_mode(0o555)).unwrap();
        let unwritable = backend.set_launch_settings(&id, "%U", &[]);
        fs::set_permissions(&applications, fs::Permissions::from_mode(0o755)).unwrap();
        // Needs a non-root user: root ignores directory permissions.
        assert!(unwritable
            .unwrap_err()
            .to_string()
            .contains("couldn't save"));
        assert_eq!(fs::read_dir(&applications).unwrap().count(), 1);
        assert_eq!(
            backend
                .launch_settings(&PackageId {
                    name: "missing".into(),
                    ..id
                })
                .unwrap_err(),
            EngineError::NotFound
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn external_appimages_launch_but_only_managed_ones_change() {
        let base = test_base("launch-external");
        let uid = rustix::process::getuid().as_raw();
        let applications = base.join("applications");
        fs::create_dir_all(&applications).unwrap();
        let original = base.join("demo.appimage");
        appimage_with(
            &original,
            backhand::compression::Compressor::Gzip,
            &[("demo.png", DEMO_PNG)],
        );
        fs::write(
            applications.join("demo.desktop"),
            "[Desktop Entry]\nName=Demo\n[Desktop Action New]\nExec=/bin/false\n",
        )
        .unwrap();
        let backend = AppImage::new(base.join("pkgdeck/appimages"), applications.clone(), uid);
        // Without a command in its main group it isn't listed at all.
        assert!(backend.external_packages().unwrap().is_empty());
        fs::write(
            applications.join("demo.desktop"),
            format!(
                "[Desktop Entry]\nName=Demo\nExec=env A=1 \"{}\" %f\n",
                original.display()
            ),
        )
        .unwrap();
        let id = backend.external_packages().unwrap().remove(0).id;
        let settings = backend.launch_settings(&id).unwrap();
        assert_eq!(
            settings,
            LaunchSettings {
                arguments: "%f".into(),
                environment: vec![("A".into(), "1".into())],
                editable: false
            }
        );
        assert!(backend
            .set_launch_settings(&id, "", &[])
            .unwrap_err()
            .to_string()
            .contains("Manage it first"));
        fs::remove_dir_all(base).unwrap();
    }

    /// A stand-in for curl: GitHub's API from `releases.json`, `status` and
    /// `etag` in `dir` (304 when asked again with that ETag), and downloads
    /// from `download`. Every call is logged to `calls`.
    #[cfg(target_os = "linux")]
    fn fake_curl(dir: &Path) -> PathBuf {
        let curl = dir.join("curl");
        fs::write(
            &curl,
            format!(
                r#"#!/bin/sh
dir='{}'
out=''; headers=''; known=''; url=''
while [ $# -gt 0 ]; do
  case "$1" in
    --output) shift; out="$1" ;;
    --dump-header) shift; headers="$1" ;;
    --header) shift; case "$1" in If-None-Match:*) known="${{1#If-None-Match: }}" ;; esac ;;
    --max-filesize|--connect-timeout|--max-time|--proto|--proto-redir|--max-redirs|--write-out) shift ;;
    --*) ;;
    *) url="$1" ;;
  esac
  shift
done
echo "$url $known" >> "$dir/calls"
case "$url" in
  https://api.github.com/*)
    status=$(cat "$dir/status")
    if [ -n "$known" ] && [ "$known" = "$(cat "$dir/etag")" ]; then status=304; : > "$out"; else cat "$dir/releases.json" > "$out"; fi
    printf 'HTTP/2 %s\r\nETag: %s\r\n\r\n' "$status" "$(cat "$dir/etag")" > "$headers"
    printf '%s' "$status" ;;
  *) [ -f "$dir/download" ] || exit 22; cat "$dir/download" > "$out" ;;
esac
"#,
                dir.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(dir.join("status"), "200").unwrap();
        fs::write(dir.join("etag"), "\"v1\"").unwrap();
        curl
    }

    #[cfg(target_os = "linux")]
    fn release(tag: &str, name: &str, sha256: Option<&str>) -> String {
        serde_json::json!([{"tag_name": tag, "draft": false, "prerelease": false, "assets": [{
            "name": name,
            "browser_download_url": format!("https://example.invalid/{name}"),
            "size": 1000,
            "digest": sha256.map(|sha| format!("sha256:{sha}")),
        }]}])
        .to_string()
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn github_releases_update_appimages_without_their_own_address() {
        let base = test_base("github");
        let (mut backend, id, _) = managed_demo(&base);
        let server = base.join("server");
        fs::create_dir_all(&server).unwrap();
        backend.curl = Some(fake_curl(&server));
        let cancel = Cancellation::default();
        let calls = || {
            fs::read_to_string(server.join("calls"))
                .unwrap_or_default()
                .lines()
                .count()
        };
        // Nothing set: it has no way to update.
        assert_eq!(
            backend.update_source(&id).unwrap(),
            UpdateSource {
                github: None,
                builtin: false,
                editable: true
            }
        );
        assert_eq!(
            backend.installed(&cancel).unwrap()[0].update,
            UpdateAvailability::Current
        );
        assert!(backend
            .set_update_source(&id, "not a project")
            .unwrap_err()
            .to_string()
            .contains("owner/name"));
        backend
            .set_update_source(&id, "https://github.com/example/demo/releases")
            .unwrap();
        assert_eq!(
            backend.update_source(&id).unwrap().github.as_deref(),
            Some("example/demo")
        );
        // A newer version on GitHub is offered; the same one isn't.
        fs::write(
            server.join("releases.json"),
            release("v2.1.0", "Demo-2.1.0.AppImage", None),
        )
        .unwrap();
        let offered = backend.installed(&cancel).unwrap().remove(0);
        assert_eq!(offered.update, UpdateAvailability::Available);
        assert_eq!(offered.installed_version.as_deref(), Some("2.0"));
        assert_eq!(offered.candidate_version.as_deref(), Some("2.1.0"));
        assert_eq!(calls(), 1);
        // The answer is reused for a while, then asked again with its ETag.
        assert_eq!(
            backend.installed(&cancel).unwrap()[0].update,
            UpdateAvailability::Available
        );
        assert_eq!(calls(), 1);
        let kept = base.join("pkgdeck/appimage-github/example__demo.json");
        let mut record: serde_json::Value =
            serde_json::from_slice(&fs::read(&kept).unwrap()).unwrap();
        assert_eq!(record["etag"], "\"v1\"");
        record["fetched"] = 0.into();
        fs::write(&kept, record.to_string()).unwrap();
        fs::write(
            server.join("releases.json"),
            "not JSON, never read on a 304",
        )
        .unwrap();
        assert_eq!(
            backend.installed(&cancel).unwrap()[0].update,
            UpdateAvailability::Available
        );
        assert!(fs::read_to_string(server.join("calls"))
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .ends_with("\"v1\""));
        // Older or equal versions aren't updates; unreadable ones that differ are.
        for (tag, expected) in [
            ("v2.0", UpdateAvailability::Current),
            ("2.0.0", UpdateAvailability::Current),
            ("v1.9.0", UpdateAvailability::Current),
            ("v2.0.1", UpdateAvailability::Available),
            ("build-7", UpdateAvailability::Available),
        ] {
            fs::remove_file(&kept).unwrap();
            fs::write(server.join("etag"), format!("\"{tag}\"")).unwrap();
            fs::write(
                server.join("releases.json"),
                release(tag, "Demo.AppImage", None),
            )
            .unwrap();
            assert_eq!(
                backend.installed(&cancel).unwrap()[0].update,
                expected,
                "{tag}"
            );
        }
        // GitHub saying no leaves the update unknown and the list intact.
        for status in ["403", "404", "500"] {
            let _ = fs::remove_file(&kept);
            fs::write(server.join("status"), status).unwrap();
            assert_eq!(
                backend.installed(&cancel).unwrap()[0].update,
                UpdateAvailability::Unknown,
                "{status}"
            );
        }
        let _ = fs::remove_file(&kept);
        fs::write(server.join("status"), "200").unwrap();
        fs::write(server.join("releases.json"), "[{]").unwrap();
        assert!(backend
            .github_releases("example/demo", &cancel)
            .unwrap_err()
            .to_string()
            .contains("couldn't be read"));
        // No AppImage for this computer: unknown, and nothing to update to.
        fs::write(
            server.join("releases.json"),
            release("v3.0.0", "Demo-3.0.0-arm64.AppImage", None),
        )
        .unwrap();
        assert_eq!(
            backend.installed(&cancel).unwrap()[0].update,
            UpdateAvailability::Unknown
        );
        assert!(backend
            .update(&id, &cancel)
            .unwrap_err()
            .to_string()
            .contains("no AppImage for this computer"));
        // Updating checks GitHub's checksum and the file, then replaces it.
        let file = base.join("pkgdeck/appimages").join(&id.name);
        let newer = server.join("download");
        let entry = DEMO_ENTRY.to_vec();
        let entry = String::from_utf8(entry)
            .unwrap()
            .replace("X-AppImage-Version=2.0", "X-AppImage-Version=2.1.0");
        appimage_with(
            &newer,
            backhand::compression::Compressor::Gzip,
            &[("demo.desktop", entry.as_bytes()), ("demo.png", DEMO_PNG)],
        );
        let sha = AppImage::digest(&newer, &cancel).unwrap();
        let before = fs::read(&file).unwrap();
        let _ = fs::remove_file(&kept);
        fs::write(
            server.join("releases.json"),
            release("v2.1.0", "Demo-2.1.0.AppImage", Some(&"0".repeat(64))),
        )
        .unwrap();
        assert!(backend.update(&id, &cancel).is_err());
        assert_eq!(fs::read(&file).unwrap(), before);
        let _ = fs::remove_file(&kept);
        fs::write(
            server.join("releases.json"),
            release("v2.1.0", "Demo-2.1.0.AppImage", Some(&sha)),
        )
        .unwrap();
        backend
            .execute(
                &Operation::UpgradeAll {
                    backend: "appimage".into(),
                },
                &cancel,
                &mut ignore,
            )
            .unwrap();
        assert_eq!(fs::read(&file).unwrap(), fs::read(&newer).unwrap());
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o755
        );
        let installed = backend.installed(&cancel).unwrap().remove(0);
        assert_eq!(installed.installed_version.as_deref(), Some("2.1.0"));
        assert_eq!(installed.update, UpdateAvailability::Current);
        // A download that fails, or isn't for this computer, changes nothing.
        let current = fs::read(&file).unwrap();
        fs::remove_file(&newer).unwrap();
        assert!(backend
            .update(&id, &cancel)
            .unwrap_err()
            .to_string()
            .contains("downloading"));
        // Without a checksum from GitHub, the file itself is still checked.
        let _ = fs::remove_file(&kept);
        fs::write(
            server.join("releases.json"),
            release("v2.2.0", "Demo-2.2.0.AppImage", None),
        )
        .unwrap();
        let mut arm = fs::read(&file).unwrap();
        arm[18..20].copy_from_slice(&183_u16.to_le_bytes());
        fs::write(&newer, &arm).unwrap();
        assert!(backend
            .update(&id, &cancel)
            .unwrap_err()
            .to_string()
            .contains("another kind of computer"));
        assert_eq!(fs::read(&file).unwrap(), current);
        assert!(fs::read_dir(base.join("pkgdeck/appimages"))
            .unwrap()
            .all(|e| !e.unwrap().file_name().to_string_lossy().starts_with('.')));
        // Releases kept from a moment ago are used without asking; a file
        // too large, or curl missing for the download, is refused.
        let _ = fs::remove_file(&kept);
        let mut huge: serde_json::Value =
            serde_json::from_str(&release("v9.0.0", "Demo-9.0.0.AppImage", None)).unwrap();
        huge[0]["assets"][0]["size"] = (MAX_IMPORT_BYTES + 1).into();
        fs::write(server.join("releases.json"), huge.to_string()).unwrap();
        assert!(backend
            .update(&id, &cancel)
            .unwrap_err()
            .to_string()
            .contains("larger than 2 GiB"));
        fs::write(&kept, serde_json::json!({"fetched": u64::MAX / 2, "releases": serde_json::from_str::<serde_json::Value>(&release("v9.0.0", "Demo-9.0.0.AppImage", None)).unwrap()}).to_string()).unwrap();
        backend.curl = Some(base.join("missing-curl"));
        assert!(backend.update(&id, &cancel).is_err());
        assert_eq!(fs::read(&file).unwrap(), current);
        backend.curl = Some(fake_curl(&server));
        // Clearing it stops GitHub updates; removing the app forgets it.
        backend.set_update_source(&id, " ").unwrap();
        assert_eq!(backend.update_source(&id).unwrap().github, None);
        backend.set_update_source(&id, "example/demo").unwrap();
        backend
            .execute(&Operation::Remove(id.clone()), &cancel, &mut ignore)
            .unwrap();
        assert!(
            !fs::read_to_string(base.join("pkgdeck/appimage-sources.json"))
                .unwrap()
                .contains("example")
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn curl_comes_from_the_host() {
        let backend = AppImage::new("/nonexistent/a".into(), "/nonexistent/b".into(), 1000);
        // Every build PkgDeck tests on has curl; it's found on the PATH.
        assert!(backend.curl().unwrap().ends_with("curl"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn without_a_version_github_checksums_decide() {
        let base = test_base("github-digest");
        let uid = rustix::process::getuid().as_raw();
        let applications = base.join("applications");
        fs::create_dir_all(&applications).unwrap();
        let source = base.join("bare.AppImage");
        appimage_with(
            &source,
            backhand::compression::Compressor::Gzip,
            &[("demo.png", DEMO_PNG)],
        );
        let mut backend = AppImage::new(base.join("pkgdeck/appimages"), applications.clone(), uid);
        let server = base.join("server");
        fs::create_dir_all(&server).unwrap();
        backend.curl = Some(fake_curl(&server));
        let cancel = Cancellation::default();
        let id = backend
            .search(source.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0)
            .id;
        backend
            .execute(&Operation::Install(id), &cancel, &mut ignore)
            .unwrap();
        let id = backend.installed(&cancel).unwrap().remove(0).id;
        backend.set_update_source(&id, "example/bare").unwrap();
        let kept = base.join("pkgdeck/appimage-github/example__bare.json");
        let sha = AppImage::digest(&source, &cancel).unwrap();
        for (digest, expected) in [
            (Some(sha.as_str()), UpdateAvailability::Current),
            (Some(&*"f".repeat(64)), UpdateAvailability::Available),
            (None, UpdateAvailability::Unknown),
        ] {
            let _ = fs::remove_file(&kept);
            fs::write(
                server.join("releases.json"),
                release("v1", "Bare.AppImage", digest),
            )
            .unwrap();
            assert_eq!(
                backend.installed(&cancel).unwrap()[0].update,
                expected,
                "{digest:?}"
            );
        }
        // AppImages PkgDeck doesn't manage can't be pointed at GitHub.
        let other = base.join("other.appimage");
        appimage_with(
            &other,
            backhand::compression::Compressor::Gzip,
            &[("demo.png", DEMO_PNG)],
        );
        fs::write(
            applications.join("other.desktop"),
            format!(
                "[Desktop Entry]\nName=Other\nExec=\"{}\"\n",
                other.display()
            ),
        )
        .unwrap();
        let external = backend.external_packages().unwrap().remove(0).id;
        assert_eq!(
            backend.update_source(&external).unwrap(),
            UpdateSource {
                github: None,
                builtin: false,
                editable: false
            }
        );
        assert!(backend
            .set_update_source(&external, "example/other")
            .unwrap_err()
            .to_string()
            .contains("Manage it first"));
        // A sources file that can't be written is reported.
        fs::remove_file(base.join("pkgdeck/appimage-sources.json")).unwrap();
        fs::create_dir(base.join("pkgdeck/appimage-sources.json")).unwrap();
        assert!(backend
            .set_update_source(&id, "example/bare")
            .unwrap_err()
            .to_string()
            .contains("couldn't save"));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn launching_runs_the_command_detached_with_its_environment() {
        let base = test_base("spawn");
        let record = base.join("record");
        let program = base.join("program");
        fs::write(
            &program,
            format!(
                "#!/bin/sh\nprintf '%s|%s\\n' \"$GREETING\" \"$*\" > '{}'\n",
                record.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let command = desktop_exec::Command {
            environment: vec![("GREETING".into(), "hi".into())],
            program: program.display().to_string(),
            arguments: vec!["--flag".into(), "%U".into()],
        };
        AppImage::spawn(&command, None).unwrap();
        let wait = || {
            (0..200)
                .find_map(|_| {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    fs::read_to_string(&record)
                        .ok()
                        .filter(|text| text.ends_with('\n'))
                })
                .expect("the program never ran")
        };
        assert_eq!(wait(), "hi|--flag\n");
        // From a Flatpak, flatpak-spawn starts it on the host.
        fs::remove_file(&record).unwrap();
        let bridge = base.join("flatpak-spawn");
        fs::write(
            &bridge,
            format!(
                "#!/bin/sh\nprintf '%s|%s\\n' \"${{GREETING:-}}\" \"$*\" > '{}'\n",
                record.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&bridge, fs::Permissions::from_mode(0o755)).unwrap();
        AppImage::spawn(&command, Some(&bridge)).unwrap();
        assert_eq!(
            wait(),
            format!("|--host --env=GREETING=hi {} --flag\n", program.display())
        );
        assert!(AppImage::spawn(&command, Some(&base.join("missing"))).is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn removing_an_external_appimage_removes_every_entry_for_it() {
        let base = test_base("external-entries");
        let uid = rustix::process::getuid().as_raw();
        let applications = base.join("applications");
        fs::create_dir_all(&applications).unwrap();
        let original = base.join("demo.appimage");
        appimage_with(
            &original,
            backhand::compression::Compressor::Gzip,
            &[("demo.png", DEMO_PNG)],
        );
        for name in ["a.desktop", "b.desktop"] {
            fs::write(
                applications.join(name),
                format!(
                    "[Desktop Entry]\nName=Demo\nExec={} %U\n",
                    original.display()
                ),
            )
            .unwrap();
        }
        let mut backend = AppImage::new(base.join("pkgdeck/appimages"), applications.clone(), uid);
        let cancel = Cancellation::default();
        let external = backend.installed(&cancel).unwrap().remove(0);
        // No usable Icon= in its entry, so the AppImage's own icon is shown.
        let icon = external.icon.clone().unwrap();
        assert_eq!(fs::read(&icon).unwrap(), DEMO_PNG);
        backend
            .execute(&Operation::Remove(external.id), &cancel, &mut ignore)
            .unwrap();
        assert!(!original.exists());
        assert!(fs::read_dir(&applications).unwrap().next().is_none());
        assert!(!icon.exists());
        fs::remove_dir_all(base).unwrap();
    }

    /// Adds a `.upd_info` section carrying `zsyn` to the Type 2 file at `path`.
    fn with_update_info(path: &Path) -> Vec<u8> {
        let mut bytes = fs::read(path).unwrap();
        bytes[40..48].copy_from_slice(&64_u64.to_le_bytes());
        bytes[58..60].copy_from_slice(&64_u16.to_le_bytes());
        bytes[60..62].copy_from_slice(&3_u16.to_le_bytes());
        bytes[62..64].copy_from_slice(&1_u16.to_le_bytes());
        bytes.resize(64 + 3 * 64, 0);
        let names = b"\0.shstrtab\0.upd_info\0";
        let names_offset = bytes.len() as u64;
        bytes[64 + 64 + 24..64 + 64 + 32].copy_from_slice(&names_offset.to_le_bytes());
        bytes[64 + 64 + 32..64 + 64 + 40].copy_from_slice(&(names.len() as u64).to_le_bytes());
        let update_offset = names_offset + names.len() as u64;
        bytes[64 + 128..64 + 132].copy_from_slice(&11_u32.to_le_bytes());
        bytes[64 + 128 + 24..64 + 128 + 32].copy_from_slice(&update_offset.to_le_bytes());
        bytes[64 + 128 + 32..64 + 128 + 40].copy_from_slice(&4_u64.to_le_bytes());
        bytes.extend_from_slice(names);
        bytes.extend_from_slice(b"zsyn");
        fs::write(path, &bytes).unwrap();
        bytes
    }

    #[cfg(target_os = "linux")]
    fn updater(base: &Path, status: u8) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let updater = base.join("updater");
        let calls = base.join("updater-calls");
        fs::write(
            &updater,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\nexit {status}\n",
                calls.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&updater, fs::Permissions::from_mode(0o755)).unwrap();
        (updater, calls)
    }

    #[test]
    fn imports_and_removes_only_managed_type2_files() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-appimage-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let source = base.join("external.AppImage");
        type2(&source);
        let root = base.join("data/pkgdeck/appimages");
        let applications = base.join("data/applications");
        let mut backend = AppImage::new(root.clone(), applications.clone(), 1000);
        let cancel = Cancellation::default();
        let cancelled_preview = Cancellation::default();
        cancelled_preview.cancel();
        assert_eq!(
            backend.search(source.to_str().unwrap(), &cancelled_preview),
            Err(EngineError::Cancelled)
        );
        let import = backend
            .search(source.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        assert!(backend.details(&import.id, &cancel).is_err());
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert_eq!(
            backend.execute(
                &Operation::Install(import.id.clone()),
                &cancelled,
                &mut |_| {}
            ),
            Err(EngineError::Cancelled)
        );
        assert!(!root.exists());
        backend
            .execute(&Operation::Install(import.id), &cancel, &mut |_| {})
            .unwrap();
        let installed = backend.installed(&cancel).unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].id.scope, Scope::User { uid: 1000 });
        assert_eq!(
            fs::read(&source).unwrap(),
            fs::read(root.join(&installed[0].id.name)).unwrap()
        );
        assert!(applications
            .join(format!("pkgdeck-{}.desktop", &installed[0].id.name[8..72]))
            .exists());
        backend
            .execute(
                &Operation::Remove(installed[0].id.clone()),
                &cancel,
                &mut |_| {},
            )
            .unwrap();
        assert!(backend.installed(&cancel).unwrap().is_empty());
        assert!(source.exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn imports_reviewed_https_appimage_without_executing_it() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::var_os("PKGDECK_REMOTE_APPIMAGE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("pkgdeck-remote-appimage-{}", std::process::id()))
            });
        if std::env::var_os("PKGDECK_REMOTE_APPIMAGE_CHILD").is_none() {
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).unwrap();
            let image = base.join("payload");
            type2(&image);
            let curl = base.join("curl");
            fs::write(&curl, format!("#!/bin/sh\nfor arg; do\n if [ \"$previous\" = '--output' ]; then output=\"$arg\"; fi\n previous=\"$arg\"\ndone\n/bin/cp '{}' \"$output\"\n", image.display())).unwrap();
            fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "backends::appimage::tests::imports_reviewed_https_appimage_without_executing_it", "--nocapture"])
                .env("PKGDECK_REMOTE_APPIMAGE_CHILD", "1")
                .env("PKGDECK_REMOTE_APPIMAGE_DIR", &base)
                .env("XDG_DATA_HOME", base.join("data"))
                .env("PATH", &base)
                .output().unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{stderr}");
            fs::remove_dir_all(base).unwrap();
            return;
        }
        let cancel = Cancellation::default();
        let url = "https://example.invalid/Synthetic.AppImage";
        let package = crate::artifact::inspect(url, &cancel).unwrap();
        assert_eq!(package.id.scope, AppImage::native().import_scope());
        let mut backend = AppImage::native();
        backend
            .execute(&Operation::Install(package.id), &cancel, &mut |_| {})
            .unwrap();
        let installed = backend.installed(&cancel).unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].id.backend, "appimage");
        assert!(backend.root.join(&installed[0].id.name).is_file());
    }

    #[test]
    fn rejects_non_appimage_imports() {
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-appimage-invalid-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let source = base.join("not-an-appimage");
        fs::write(&source, "not executable metadata").unwrap();
        let mut backend = AppImage::new(base.join("owned"), base.join("applications"), 1000);
        assert!(backend
            .search(source.to_str().unwrap(), &Cancellation::default())
            .is_err());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn import_rejects_symlinks_and_existing_destination_conflicts() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-appimage-conflict-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let source = base.join("Sample App.AppImage");
        type2(&source);
        let alias = base.join("alias.AppImage");
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        let root = base.join("owned");
        let mut backend = AppImage::new(root.clone(), base.join("applications"), 1000);
        let cancel = Cancellation::default();
        assert!(backend.search(alias.to_str().unwrap(), &cancel).is_err());
        let candidate = backend
            .search(source.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        let digest = candidate.id.reference.clone().unwrap();
        fs::create_dir_all(&root).unwrap();
        let target = root.join(format!("pkgdeck-{digest}.AppImage"));
        fs::write(&target, b"foreign file").unwrap();
        assert!(backend
            .execute(&Operation::Install(candidate.id), &cancel, &mut |_| {})
            .is_err());
        assert_eq!(fs::read(&target).unwrap(), b"foreign file");
        assert!(source.exists());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn interrupted_copy_stops_before_publishing_more_bytes() {
        struct CancelAfterRead(Cancellation);
        impl Read for CancelAfterRead {
            // Copying checks cancellation before every read, so this runs once.
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                buffer[..4].copy_from_slice(b"data");
                self.0.cancel();
                Ok(4)
            }
        }
        let cancel = Cancellation::default();
        let mut source = CancelAfterRead(cancel.clone());
        let mut output = Vec::new();
        assert_eq!(
            AppImage::copy_bounded(&mut source, &mut output, &cancel),
            Err(EngineError::Cancelled)
        );
        assert_eq!(output, b"data");
    }
    #[test]
    fn unavailable_bundled_updater_is_reported() {
        let backend = AppImage::new(
            PathBuf::from("/nonexistent/pkgdeck-owned"),
            PathBuf::from("/nonexistent/pkgdeck-applications"),
            rustix::process::getuid().as_raw(),
        );
        let error = backend.updater().unwrap_err().to_string();
        assert!(error.contains("bundled AppImage updater is unavailable"));
    }
    #[test]
    fn bounded_elf_sections_report_update_metadata() {
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-appimage-update-info-{}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&base);
        let source = base.join("with-update.AppImage");
        type2(&source);
        let mut bytes = with_update_info(&source);
        assert!(AppImage::has_update_metadata(&source).unwrap());
        let mut backend = AppImage::new(base.join("owned"), base.join("apps"), 1000);
        assert!(backend
            .search(source.to_str().unwrap(), &Cancellation::default())
            .is_ok());
        let mut invalid_table = bytes.clone();
        invalid_table[58..60].copy_from_slice(&32_u16.to_le_bytes());
        fs::write(&source, invalid_table).unwrap();
        assert!(AppImage::has_update_metadata(&source).is_err());
        let mut invalid_names = bytes.clone();
        invalid_names[64 + 64 + 32..64 + 64 + 40]
            .copy_from_slice(&(1024_u64 * 1024 + 1).to_le_bytes());
        fs::write(&source, invalid_names).unwrap();
        assert!(AppImage::has_update_metadata(&source).is_err());
        let mut invalid_data = bytes.clone();
        invalid_data[64 + 128 + 32..64 + 128 + 40].copy_from_slice(&5000_u64.to_le_bytes());
        fs::write(&source, invalid_data).unwrap();
        assert!(AppImage::has_update_metadata(&source).is_err());
        bytes[64 + 128..64 + 132].copy_from_slice(&0_u32.to_le_bytes());
        fs::write(&source, bytes).unwrap();
        assert!(!AppImage::has_update_metadata(&source).unwrap());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn desktop_exec_paths_accept_quoted_and_plain_appimages() {
        assert_eq!(
            AppImage::exec_path("\"/home/user/Apps/Audacity.AppImage\" %U"),
            Some(PathBuf::from("/home/user/Apps/Audacity.AppImage"))
        );
        assert_eq!(
            AppImage::exec_path("/home/user/Apps/Audacity.AppImage --verbose"),
            Some(PathBuf::from("/home/user/Apps/Audacity.AppImage"))
        );
        assert_eq!(
            AppImage::exec_path("\"/home/user/Apps/Audacity\\\"Edition.AppImage\""),
            Some(PathBuf::from("/home/user/Apps/Audacity\"Edition.AppImage"))
        );
        assert_eq!(AppImage::exec_path("Audacity.AppImage"), None);
        // Gear Lever writes environment variables through `env`.
        assert_eq!(
            AppImage::exec_path("env DESKTOPINTEGRATION=1 \"/home/user/Apps/Trezor.AppImage\" %U"),
            Some(PathBuf::from("/home/user/Apps/Trezor.AppImage"))
        );
        assert_eq!(
            AppImage::exec_path("\"/home/user/Apps/Audacity.AppImage"),
            None
        );
    }

    #[test]
    fn local_search_details_and_validation_are_explicit() {
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-appimage-coverage-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("owned");
        let applications = base.join("applications");
        let source = base.join("external.AppImage");
        fs::create_dir_all(&base).unwrap();
        type2(&source);
        let mut backend = AppImage::new(root.clone(), applications, 1000);
        let cancel = Cancellation::default();
        let candidate = backend
            .search(source.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        assert_eq!(
            candidate.id.scope,
            Scope::Environment { path: root.clone() }
        );
        assert!(backend.search("not-present", &cancel).unwrap().is_empty());
        assert_eq!(
            backend.details(&candidate.id, &cancel),
            Err(EngineError::NotFound)
        );
        assert!(backend
            .execute(&Operation::Remove(candidate.id), &cancel, &mut |_| {})
            .is_err());
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert_eq!(backend.detect(&cancelled), Err(EngineError::Cancelled));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn discovers_and_removes_user_owned_external_desktop_entries() {
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-appimage-external-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let applications = base.join("applications");
        let external = base.join("Audacity.AppImage");
        fs::create_dir_all(&applications).unwrap();
        type2(&external);
        let icon = base.join("audacity.png");
        fs::write(&icon, "png").unwrap();
        let desktop = applications.join("audacity.desktop");
        fs::write(
            &desktop,
            format!(
                "[Desktop Entry]\nType=Application\nName=Audacity Portable\nExec=\"{}\" %U\nIcon={}\nX-AppImage-Version=4.0\n",
                external.display(),
                icon.display()
            ),
        )
        .unwrap();
        let mut backend = AppImage::new(
            base.join("owned"),
            applications,
            rustix::process::getuid().as_raw(),
        );
        let cancel = Cancellation::default();
        let package = backend.search("audacity", &cancel).unwrap().remove(0);
        assert_eq!(package.display_name, "Audacity Portable");
        assert_eq!(package.installed_version.as_deref(), Some("4.0"));
        assert_eq!(package.icon, Some(icon));
        assert_eq!(
            backend.details(&package.id, &cancel).unwrap().package,
            package
        );
        backend
            .execute(&Operation::Remove(package.id), &cancel, &mut |_| {})
            .unwrap();
        assert!(!external.exists());
        assert!(!desktop.exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn homebrew_cask_appimages_belong_to_homebrew_casks() {
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-appimage-homebrew-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let applications = base.join("applications");
        fs::create_dir_all(&applications).unwrap();
        let obsidian = base.join("Applications/Obsidian.AppImage");
        fs::create_dir_all(obsidian.parent().unwrap()).unwrap();
        type2(&obsidian);
        fs::write(
            applications.join("appimagekit-obsidian.desktop"),
            format!(
                "[Desktop Entry]\nName=Obsidian\nExec={} %U\n",
                obsidian.display()
            ),
        )
        .unwrap();
        let caskroom = base.join("Caskroom");
        let version = caskroom.join("obsidian/1.13.7");
        fs::create_dir_all(&version).unwrap();
        fs::create_dir_all(caskroom.join("obsidian/.metadata/1.13.7")).unwrap();
        let backend = || {
            AppImage::new(
                base.join("owned"),
                applications.clone(),
                rustix::process::getuid().as_raw(),
            )
            .with_caskroom(caskroom.clone())
        };
        let cancel = Cancellation::default();
        // Until Homebrew links it, the AppImage looks like any external one.
        assert_eq!(backend().search("obsidian", &cancel).unwrap().len(), 1);
        std::os::unix::fs::symlink(&obsidian, version.join("Obsidian-1.13.7-arm64.AppImage"))
            .unwrap();
        assert!(backend().search("obsidian", &cancel).unwrap().is_empty());
        assert!(backend().installed(&cancel).unwrap().is_empty());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn upgrades_managed_and_external_appimages_with_the_bundled_helper() {
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-appimage-updater-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let applications = base.join("applications");
        fs::create_dir_all(&applications).unwrap();
        let (updater, calls) = updater(&base, 0);
        let source = base.join("source.AppImage");
        type2(&source);
        with_update_info(&source);
        let root = base.join("owned");
        let mut backend = AppImage::new(
            root.clone(),
            applications.clone(),
            rustix::process::getuid().as_raw(),
        )
        .with_updater(updater.clone());
        let cancel = Cancellation::default();
        let candidate = backend
            .search(source.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        backend
            .execute(&Operation::Install(candidate.id), &cancel, &mut |_| {})
            .unwrap();
        let managed = backend.installed(&cancel).unwrap().remove(0);

        let external = base.join("external.AppImage");
        type2(&external);
        with_update_info(&external);
        fs::write(
            applications.join("external.desktop"),
            format!("[Desktop Entry]\nExec={}\n", external.display()),
        )
        .unwrap();
        let external = backend.search("external", &cancel).unwrap().remove(0);

        backend
            .execute(
                &Operation::Upgrade(managed.id.clone()),
                &cancel,
                &mut |_| {},
            )
            .unwrap();
        assert!(fs::read_to_string(&calls)
            .unwrap()
            .contains(&root.join(&managed.id.name).display().to_string()));
        fs::write(&calls, "").unwrap();
        backend
            .execute(
                &Operation::Upgrade(external.id.clone()),
                &cancel,
                &mut |_| {},
            )
            .unwrap();
        assert!(fs::read_to_string(&calls)
            .unwrap()
            .contains(&external.id.name));
        fs::write(&calls, "").unwrap();
        fs::write(
            &updater,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\n[ \"$2\" != --check-for-update ]\n",
                calls.display()
            ),
        )
        .unwrap();
        backend
            .execute(
                &Operation::UpgradeAll {
                    backend: "appimage".into(),
                },
                &cancel,
                &mut ignore,
            )
            .unwrap();
        assert_eq!(fs::read_to_string(&calls).unwrap().lines().count(), 18);

        fs::write(&updater, "#!/bin/sh\nexit 9\n").unwrap();
        assert!(backend
            .execute(&Operation::Upgrade(managed.id), &cancel, &mut |_| {})
            .is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn managed_files_support_details_and_idempotent_imports() {
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-appimage-managed-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let source = base.join("external.AppImage");
        type2(&source);
        let root = base.join("owned");
        let mut backend = AppImage::new(root.clone(), base.join("applications"), 1000);
        let cancel = Cancellation::default();
        let candidate = backend
            .search(source.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        backend
            .execute(
                &Operation::Install(candidate.id.clone()),
                &cancel,
                &mut |_| {},
            )
            .unwrap();
        backend
            .execute(&Operation::Install(candidate.id), &cancel, &mut |_| {})
            .unwrap();
        let duplicate = backend.search(source.to_str().unwrap(), &cancel).unwrap();
        // Opening a file that's already imported shows it as installed.
        assert_eq!(duplicate[0].installed_version.as_deref(), Some("local"));
        let installed = backend.installed(&cancel).unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(
            backend.details(&installed[0].id, &cancel).unwrap().package,
            installed[0]
        );
        fs::write(root.join("foreign.AppImage"), b"ignored").unwrap();
        assert!(backend.installed(&cancel).is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn import_rejects_changed_architecture_and_managed_sources() {
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-appimage-identity-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let source = base.join("external.AppImage");
        type2(&source);
        let root = base.join("owned");
        let mut backend = AppImage::new(root.clone(), base.join("applications"), 1000);
        let cancel = Cancellation::default();
        let mut candidate = backend
            .search(source.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        let mut unreviewed = candidate.id.clone();
        unreviewed.reference = None;
        assert!(backend
            .execute(&Operation::Install(unreviewed), &cancel, &mut |_| {})
            .is_err());
        candidate.id.architecture = "aarch64".into();
        assert!(backend
            .execute(&Operation::Install(candidate.id), &cancel, &mut |_| {})
            .is_err());
        fs::create_dir_all(&root).unwrap();
        let managed_source = root.join("nested.AppImage");
        type2(&managed_source);
        let foreign = PackageId {
            backend: "appimage".into(),
            name: managed_source.display().to_string(),
            architecture: "x86_64".into(),
            scope: Scope::Environment { path: root },
            remote: None,
            reference: None,
        };
        assert!(backend
            .execute(&Operation::Install(foreign), &cancel, &mut |_| {})
            .is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn type2_metadata_reports_unknown_architecture_and_rejects_relative_paths() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-appimage-arch-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let source = base.join("external.AppImage");
        type2(&source);
        let mut bytes = fs::read(&source).unwrap();
        bytes[18..20].copy_from_slice(&0_u16.to_le_bytes());
        fs::write(&source, bytes).unwrap();
        let mut backend = AppImage::new(base.join("owned"), base.join("applications"), 1000);
        let cancel = Cancellation::default();
        assert!(backend.search(source.to_str().unwrap(), &cancel).is_err());
        assert!(backend
            .search("relative.AppImage", &cancel)
            .unwrap()
            .is_empty());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn reports_appimage_interface_and_invalid_storage_paths() {
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-appimage-interface-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let aarch64 = base.join("aarch64.AppImage");
        type2(&aarch64);
        let mut bytes = fs::read(&aarch64).unwrap();
        bytes[18..20].copy_from_slice(&183_u16.to_le_bytes());
        fs::write(&aarch64, bytes).unwrap();
        assert_eq!(AppImage::type2(&aarch64).unwrap(), "aarch64");
        assert_eq!(
            AppImage::desktop_appimage_path("TryExec=/opt/Audacity.AppImage\nExec=audacity"),
            Some(PathBuf::from("/opt/Audacity.AppImage"))
        );
        assert_eq!(
            AppImage::desktop_appimage_path(
                "TryExec=gearlever\nExec=\"/opt/Audacity Portable.AppImage\" %U"
            ),
            Some(PathBuf::from("/opt/Audacity Portable.AppImage"))
        );

        let root = base.join("owned");
        let applications = base.join("applications");
        let uid = rustix::process::getuid().as_raw();
        let mut backend = AppImage::new(root.clone(), applications.clone(), uid);
        assert_eq!(backend.id(), "appimage");
        assert!(backend.capabilities().contains(&Capability::Upgrade));
        assert_eq!(
            backend.detect(&Cancellation::default()),
            Ok(if cfg!(target_os = "linux") {
                Availability::Available
            } else {
                Availability::Unavailable("AppImage requires Linux".into())
            })
        );
        let foreign = PackageId {
            backend: "other".into(),
            name: aarch64.display().to_string(),
            architecture: "aarch64".into(),
            scope: Scope::Environment { path: root.clone() },
            remote: None,
            reference: None,
        };
        assert!(backend
            .execute(
                &Operation::Install(foreign),
                &Cancellation::default(),
                &mut ignore
            )
            .is_err());

        fs::write(&root, "not a directory").unwrap();
        assert!(backend.installed(&Cancellation::default()).is_err());
        fs::remove_file(&root).unwrap();
        fs::write(&applications, "not a directory").unwrap();
        assert!(backend
            .search("anything", &Cancellation::default())
            .is_err());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn duplicate_desktop_entries_collapse_to_one_identity() {
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-appimage-duplicate-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let applications = base.join("applications");
        let external = base.join("Example.AppImage");
        fs::create_dir_all(&applications).unwrap();
        type2(&external);
        for name in ["example.desktop", "example-copy.desktop"] {
            fs::write(
                applications.join(name),
                format!(
                    "[Desktop Entry]\nType=Application\nName=Example\nExec=\"{}\" %U\n",
                    external.display()
                ),
            )
            .unwrap();
        }
        let mut backend = AppImage::new(
            base.join("owned"),
            applications,
            rustix::process::getuid().as_raw(),
        );
        let cancel = Cancellation::default();
        let installed = backend.installed(&cancel).unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(
            backend.details(&installed[0].id, &cancel).unwrap().package,
            installed[0]
        );
        fs::remove_dir_all(base).unwrap();
    }

    fn test_base(name: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-appimage-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn non_type2_files_are_rejected_before_hashing() {
        let base = test_base("header");
        let file = base.join("file.AppImage");
        let mut backend = AppImage::new(base.join("owned"), base.join("applications"), 1000);
        let cancel = Cancellation::default();
        fs::write(&file, [b'#'; 64]).unwrap();
        assert_eq!(
            backend.search(file.to_str().unwrap(), &cancel),
            Err(AppImage::invalid("expected a Type 2 AppImage ELF file"))
        );
        type2(&file);
        let mut bytes = fs::read(&file).unwrap();
        // A 32-bit ELF class.
        bytes[4] = 1;
        fs::write(&file, bytes).unwrap();
        assert_eq!(
            backend.search(file.to_str().unwrap(), &cancel),
            Err(AppImage::invalid("malformed or unsupported ELF header"))
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn hashing_and_copying_stop_at_the_size_limit() {
        let base = test_base("limits");
        let file = base.join("file");
        fs::write(&file, b"0123456789").unwrap();
        let cancel = Cancellation::default();
        assert_eq!(
            AppImage::digest_bounded(&file, 9, &cancel),
            Err(AppImage::invalid("AppImage exceeds 2 GiB"))
        );
        assert_eq!(
            AppImage::digest_bounded(&file, 10, &cancel),
            AppImage::digest(&file, &cancel)
        );
        fs::remove_dir_all(base).unwrap();

        /// Endless input that never fills the buffer with real bytes.
        struct Endless;
        impl Read for Endless {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                Ok(buffer.len())
            }
        }
        assert_eq!(
            AppImage::copy_bounded(&mut Endless, &mut std::io::sink(), &cancel),
            Err(AppImage::invalid("AppImage exceeds 2 GiB"))
        );
    }

    #[test]
    fn imports_require_the_reviewed_digest() {
        let base = test_base("digest");
        let source = base.join("external.AppImage");
        type2(&source);
        let root = base.join("owned");
        let mut backend = AppImage::new(root.clone(), base.join("applications"), 1000);
        let cancel = Cancellation::default();
        let candidate = backend
            .search(source.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        for reference in [None, Some("0".repeat(64))] {
            let id = PackageId {
                reference,
                ..candidate.id.clone()
            };
            assert_eq!(
                backend.execute(&Operation::Install(id), &cancel, &mut ignore),
                Err(AppImage::invalid("AppImage changed or was not previewed"))
            );
        }
        // Changed after the preview: the reviewed digest no longer matches.
        fs::write(
            &source,
            [fs::read(&source).unwrap(), b"more".to_vec()].concat(),
        )
        .unwrap();
        assert_eq!(
            backend.execute(&Operation::Install(candidate.id), &cancel, &mut ignore),
            Err(AppImage::invalid("AppImage changed or was not previewed"))
        );
        assert!(!root.exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn an_unwritable_applications_folder_leaves_no_managed_copy() {
        // Needs a non-root user: root ignores directory permissions.
        let base = test_base("readonly");
        let source = base.join("external.AppImage");
        type2(&source);
        let root = base.join("owned");
        let applications = base.join("applications");
        fs::create_dir_all(&applications).unwrap();
        fs::set_permissions(&applications, fs::Permissions::from_mode(0o555)).unwrap();
        let mut backend = AppImage::new(root.clone(), applications.clone(), 1000);
        let cancel = Cancellation::default();
        let candidate = backend
            .search(source.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        let error = backend
            .execute(&Operation::Install(candidate.id), &cancel, &mut ignore)
            .unwrap_err()
            .to_string();
        assert!(error.contains("desktop entry failed"), "{error}");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::set_permissions(&applications, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_failed_desktop_entry_write_removes_the_partial_import() {
        use rustix::process::{getrlimit, setrlimit, Resource, Rlimit};
        const CHILD: &str = "PKGDECK_APPIMAGE_FSIZE_CHILD";
        let base = match std::env::var_os(CHILD) {
            Some(base) => PathBuf::from(base),
            None => {
                let base = test_base("fsize");
                // A size limit that fails the write needs SIGXFSZ ignored, which
                // only an exec can set up; the child runs just this test.
                let output = std::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg("trap '' XFSZ; exec \"$0\" \"$@\"")
                    .arg(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "backends::appimage::tests::a_failed_desktop_entry_write_removes_the_partial_import",
                        "--nocapture",
                    ])
                    .env(CHILD, &base)
                    .output()
                    .unwrap();
                let stderr = String::from_utf8_lossy(&output.stderr);
                assert!(output.status.success(), "{stderr}");
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(stdout.contains("1 passed"), "{stdout}");
                fs::remove_dir_all(base).unwrap();
                return;
            }
        };
        let source = base.join("external.AppImage");
        type2(&source);
        let root = base.join("owned");
        let applications = base.join("applications");
        let mut backend = AppImage::new(root.clone(), applications.clone(), 1000);
        let cancel = Cancellation::default();
        let candidate = backend
            .search(source.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        // The 64-byte AppImage fits; its much longer desktop entry does not.
        let limit = getrlimit(Resource::Fsize);
        setrlimit(
            Resource::Fsize,
            Rlimit {
                current: Some(100),
                maximum: limit.maximum,
            },
        )
        .unwrap();
        let result = backend.execute(&Operation::Install(candidate.id), &cancel, &mut ignore);
        setrlimit(Resource::Fsize, limit).unwrap();
        let error = result.unwrap_err().to_string();
        assert!(error.contains("desktop entry failed"), "{error}");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&applications).unwrap().count(), 0);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn appimages_with_update_metadata_offer_updates() {
        let base = test_base("offers");
        let applications = base.join("applications");
        fs::create_dir_all(&applications).unwrap();
        let (updater, calls) = updater(&base, 1);
        let source = base.join("source.AppImage");
        type2(&source);
        with_update_info(&source);
        let external = base.join("external.AppImage");
        type2(&external);
        with_update_info(&external);
        fs::write(
            applications.join("external.desktop"),
            format!("[Desktop Entry]\nExec={}\n", external.display()),
        )
        .unwrap();
        let mut backend = AppImage::new(
            base.join("owned"),
            applications,
            rustix::process::getuid().as_raw(),
        )
        .with_updater(updater.clone());
        let cancel = Cancellation::default();
        let candidate = backend
            .search(source.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        backend
            .execute(&Operation::Install(candidate.id), &cancel, &mut ignore)
            .unwrap();
        let installed = backend.installed(&cancel).unwrap();
        assert_eq!(installed.len(), 2);
        assert!(installed
            .iter()
            .all(|package| package.update == UpdateAvailability::Available));
        assert!(fs::read_to_string(&calls)
            .unwrap()
            .contains("--check-for-update"));
        let original = fs::read(&source).unwrap();
        // Current files stay out of Updates even though metadata is present.
        fs::write(&updater, "#!/bin/sh\nexit 0\n").unwrap();
        assert!(backend
            .installed(&cancel)
            .unwrap()
            .iter()
            .all(|package| package.update == UpdateAvailability::Current));
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(
            backend
                .details(&installed[0].id, &cancel)
                .unwrap()
                .package
                .update
                == UpdateAvailability::Unknown
        );
        let _ = self::updater(&base, 0);
        fs::write(&calls, "").unwrap();
        backend
            .execute(
                &Operation::UpgradeAll {
                    backend: "appimage".into(),
                },
                &cancel,
                &mut ignore,
            )
            .unwrap();
        let probes = fs::read_to_string(&calls).unwrap();
        assert_eq!(probes.matches("--check-for-update").count(), 2);
        assert!(!probes.contains("--overwrite"));
        // Failed network/probe operations must never become Current or Available.
        fs::write(
            &updater,
            "#!/bin/sh\necho 'error: unavailable source' >&2\nexit 2\n",
        )
        .unwrap();
        assert!(matches!(
            backend.installed(&cancel),
            Err(EngineError::Execution(_))
        ));
        fs::remove_file(&updater).unwrap();
        assert!(matches!(
            backend.installed(&cancel),
            Err(EngineError::Execution(_))
        ));
        // Cancellation applies to the read-only check, including while it runs.
        fs::write(&updater, "#!/bin/sh\nexec sleep 30\n").unwrap();
        fs::set_permissions(&updater, fs::Permissions::from_mode(0o755)).unwrap();
        let interrupted = Cancellation::default();
        let signal = interrupted.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            signal.cancel();
        });
        assert!(matches!(
            backend.installed(&interrupted),
            Err(EngineError::Cancelled)
        ));
        thread.join().unwrap();
        assert!(matches!(
            backend.installed(&interrupted),
            Err(EngineError::Cancelled)
        ));
        // A build missing its bundled updater cannot establish availability.
        let mut unavailable = AppImage::new(
            base.join("owned"),
            base.join("applications"),
            rustix::process::getuid().as_raw(),
        );
        assert!(unavailable
            .installed(&cancel)
            .unwrap()
            .iter()
            .all(|package| package.update == UpdateAvailability::Unknown));
        // An updater that cannot start fails the update.
        let mut broken = AppImage::new(
            base.join("owned"),
            base.join("applications"),
            rustix::process::getuid().as_raw(),
        )
        .with_updater(base.join("missing-updater"));
        let managed = installed
            .into_iter()
            .find(|package| AppImage::managed_name(&package.id.name))
            .unwrap();
        assert!(matches!(
            broken.execute(&Operation::Upgrade(managed.id), &cancel, &mut ignore),
            Err(EngineError::Execution(_))
        ));
        fs::remove_dir_all(base).unwrap();
    }
}
