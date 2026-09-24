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

const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Upgrade,
];

/// Imports only Type 2 AppImages into PkgDeck-owned storage. Metadata inspection
/// reads the ELF header; imported files are never executed by this backend.
pub struct AppImage {
    root: PathBuf,
    applications: PathBuf,
    uid: u32,
    #[cfg(test)]
    updater: Option<PathBuf>,
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
        Self {
            root: data.join("pkgdeck/appimages"),
            applications: data.join("applications"),
            uid: rustix::process::getuid().as_raw(),
            #[cfg(test)]
            updater: None,
        }
    }
    #[cfg(test)]
    pub fn new(root: PathBuf, applications: PathBuf, uid: u32) -> Self {
        Self {
            root,
            applications,
            uid,
            updater: None,
        }
    }
    #[cfg(test)]
    fn with_updater(mut self, updater: PathBuf) -> Self {
        self.updater = Some(updater);
        self
    }
    fn scope(&self) -> Scope {
        Scope::User { uid: self.uid }
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
    fn digest(path: &Path) -> Result<String, EngineError> {
        let mut file = fs::File::open(path).map_err(|e| Self::invalid(e.to_string()))?;
        let mut hash = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut total = 0_u64;
        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|e| Self::invalid(e.to_string()))?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > MAX_IMPORT_BYTES {
                return Err(Self::invalid("AppImage exceeds 2 GiB"));
            }
            hash.update(&buffer[..count]);
        }
        Ok(hex::encode(hash.finalize()))
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
    fn desktop_entry(destination: &Path, display_name: &str) -> String {
        let escaped = destination
            .display()
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%");
        let name = display_name.replace(['\n', '\r'], " ");
        format!("[Desktop Entry]\nType=Application\nName={name}\nExec=\"{escaped}\" %U\nTryExec={escaped}\nTerminal=false\nCategories=Utility;\n")
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
            && Self::digest(destination).ok().as_deref() == Some(digest)
            && fs::read_to_string(entry).ok().is_some_and(|desktop| {
                Self::desktop_entry(destination, "Imported AppImage")
                    .lines()
                    .find(|line| line.starts_with("Exec="))
                    .is_some_and(|line| desktop.lines().any(|existing| existing == line))
            });
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
        Ok(Package {
            id: PackageId {
                backend: "appimage".into(),
                name: name.into(),
                architecture,
                scope: self.scope(),
                remote: None,
                reference: None,
            },
            display_name: "Imported AppImage".into(),
            summary: format!("PkgDeck-managed local Type 2 AppImage ({digest})"),
            installed_version: Some(digest.into()),
            candidate_version: Some(digest.into()),
            update: if Self::has_update_metadata(&path)? && self.updater().is_ok() {
                UpdateAvailability::Available
            } else {
                UpdateAvailability::Current
            },
            icon: None,
            component_ids: vec![],
            homepages: vec![],
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
    /// Return the executable from an `Exec=` value when it is an absolute path.
    /// Desktop entry field codes are deliberately ignored after the executable.
    fn exec_path(value: &str) -> Option<PathBuf> {
        let value = value.trim_start();
        let executable = if let Some(value) = value.strip_prefix('"') {
            let mut escaped = false;
            let mut end = None;
            for (index, character) in value.char_indices() {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    end = Some(index);
                    break;
                }
            }
            let end = end?;
            value[..end].replace("\\\\", "\\").replace("\\\"", "\"")
        } else {
            value.split_whitespace().next()?.into()
        };
        let path = PathBuf::from(executable);
        path.is_absolute().then_some(path)
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
    fn external_entries(&self) -> Result<Vec<(Package, PathBuf)>, EngineError> {
        let entries = match fs::read_dir(&self.applications) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(Self::invalid(e.to_string())),
        };
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
                    || !metadata.is_file()
                    || metadata.uid() != self.uid
                {
                    return None;
                }
                let architecture = Self::type2(&canonical).ok()?;
                let display_name = Self::desktop_value(&contents, "Name=")
                    .unwrap_or_else(|| "External AppImage".into());
                let version = Self::desktop_value(&contents, "X-AppImage-Version=")
                    .unwrap_or_else(|| "local".into());
                // External entries keep their own desktop file, so its Icon=
                // resolves like any local entry. Managed imports stay
                // icon-less until .DirIcon extraction exists.
                let icon = super::desktop_icon(None, &desktop);
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
                        summary: "Externally managed local Type 2 AppImage".into(),
                        installed_version: Some(version.clone()),
                        candidate_version: Some(version),
                        update: if Self::has_update_metadata(&canonical).ok()?
                            && self.updater().is_ok()
                        {
                            UpdateAvailability::Available
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
                    },
                    desktop,
                ))
            })
            .collect::<Vec<_>>())
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
            local.reference = Some(Self::digest(staged.path())?);
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
        let digest = Self::digest(&source)?;
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        if id.reference.as_deref() != Some(digest.as_str()) {
            return Err(Self::invalid("AppImage changed or was not previewed"));
        }
        let name = format!("pkgdeck-{digest}.AppImage");
        fs::create_dir_all(&self.root).map_err(|e| Self::invalid(e.to_string()))?;
        let destination = self.root.join(&name);
        fs::create_dir_all(&self.applications).map_err(|e| Self::invalid(e.to_string()))?;
        let entry = self
            .applications
            .join(format!("pkgdeck-{}.desktop", &name[8..72]));
        if Self::existing_import(&destination, &entry, &digest)? {
            return Ok(());
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
            if Self::digest(&temporary)? != digest {
                return Err(Self::invalid("AppImage changed during import"));
            }
            if cancel.requested() {
                return Err(EngineError::Cancelled);
            }
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))
                .map_err(|e| Self::invalid(e.to_string()))?;
            fs::hard_link(&temporary, &destination).map_err(|e| Self::invalid(e.to_string()))?;
            let display_name = source
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Imported AppImage");
            let desktop = Self::desktop_entry(&destination, display_name);
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
                    Err(Self::invalid(format!("desktop entry failed: {error}")))
                }
            }
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
    fn update(&self, id: &PackageId, cancel: &Cancellation) -> Result<(), EngineError> {
        let target = if Self::managed_name(&id.name) && id.scope == self.scope() {
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
            Err(EngineError::Cancelled)
        } else {
            Ok(Availability::Available)
        }
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let source = PathBuf::from(query);
        if source.is_absolute() && source.symlink_metadata().is_ok() {
            let architecture = Self::type2(&source)?;
            let has_updates = Self::has_update_metadata(&source)?;
            let digest = Self::digest(&source)?;
            if cancel.requested() {
                return Err(EngineError::Cancelled);
            }
            let destination = self.root.join(format!("pkgdeck-{digest}.AppImage"));
            let desktop = self.applications.join(format!("pkgdeck-{digest}.desktop"));
            let already_imported = Self::existing_import(&destination, &desktop, &digest)?;
            let display_name = source
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("Imported AppImage");
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
                display_name: source
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("AppImage")
                    .into(),
                summary: format!("Original: {} (kept)\nManaged file: {} (executable, mode 0755)\nDesktop entry: {}\nDesktop name: {}\nDesktop command: {} %U\nUpdate metadata: {}\n{}", source.display(), destination.display(), desktop.display(), display_name, destination.display(), if has_updates { "available" } else { "unavailable" }, if already_imported { "Already imported; no files will be overwritten." } else { "A managed copy and desktop entry will be created." }),
                installed_version: None,
                candidate_version: None,
                update: UpdateAvailability::Unknown,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
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
    fn installed(&mut self, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.installed_packages()
    }
    fn details(&mut self, id: &PackageId, _: &Cancellation) -> Result<PackageDetails, EngineError> {
        let package = self
            .installed_packages()?
            .into_iter()
            .find(|p| p.id == *id)
            .ok_or(EngineError::NotFound)?;
        Ok(PackageDetails { description: "A PkgDeck-managed local Type 2 AppImage. PkgDeck never executes imported files to inspect metadata.".into(), homepage: None, dependencies: vec![], package })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        match operation {
            Operation::Install(id) => {
                progress(Progress::Message(
                    "Importing AppImage without executing it.".into(),
                ));
                self.import(id, cancel)?;
            }
            Operation::Remove(id)
                if id.backend == "appimage"
                    && id.scope == self.scope()
                    && Self::managed_name(&id.name) =>
            {
                progress(Progress::Message(
                    "Removing PkgDeck-managed AppImage and desktop entry.".into(),
                ));
                fs::remove_file(self.root.join(&id.name))
                    .map_err(|e| Self::invalid(e.to_string()))?;
                let _ = fs::remove_file(
                    self.applications
                        .join(format!("pkgdeck-{}.desktop", &id.name[8..72])),
                );
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
                fs::remove_file(&id.name).map_err(|e| Self::invalid(e.to_string()))?;
                fs::remove_file(desktop).map_err(|e| Self::invalid(e.to_string()))?;
            }
            Operation::Upgrade(id) if id.backend == "appimage" => {
                progress(Progress::Message(
                    "Updating AppImage with its embedded update information.".into(),
                ));
                self.update(id, cancel)?;
            }
            Operation::UpgradeAll { backend } if backend == "appimage" => {
                for package in self.installed_packages()? {
                    self.update(&package.id, cancel)?;
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
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
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
        struct CancelAfterRead(Cancellation, bool);
        impl Read for CancelAfterRead {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.1 {
                    return Ok(0);
                }
                self.1 = true;
                buffer[..4].copy_from_slice(b"data");
                self.0.cancel();
                Ok(4)
            }
        }
        let cancel = Cancellation::default();
        let mut source = CancelAfterRead(cancel.clone(), false);
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
        let mut bytes = fs::read(&source).unwrap();
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
        fs::write(&source, &bytes).unwrap();
        assert!(AppImage::has_update_metadata(&source).unwrap());
        let mut backend = AppImage::new(base.join("owned"), base.join("apps"), 1000);
        assert!(backend
            .search(source.to_str().unwrap(), &Cancellation::default())
            .unwrap()[0]
            .summary
            .contains("Update metadata: available"));
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
        backend
            .execute(
                &Operation::UpgradeAll {
                    backend: "appimage".into(),
                },
                &cancel,
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(fs::read_to_string(&calls).unwrap().lines().count(), 10);

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
        assert!(duplicate[0].summary.contains("Already imported"));
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
            Ok(Availability::Available)
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
                &mut |_| {}
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
}
