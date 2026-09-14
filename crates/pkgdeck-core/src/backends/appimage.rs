use crate::{
    engine::*,
    package::*,
    process::{self, Cancellation, ExecutionError, Limits},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

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
}

impl AppImage {
    pub fn native() -> Self {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Self {
            root: data.join("pkgdeck/appimages"),
            applications: data.join("applications"),
            uid: rustix::process::getuid().as_raw(),
        }
    }
    #[cfg(test)]
    pub fn new(root: PathBuf, applications: PathBuf, uid: u32) -> Self {
        Self {
            root,
            applications,
            uid,
        }
    }
    fn scope(&self) -> Scope {
        Scope::User { uid: self.uid }
    }
    fn invalid(reason: impl Into<String>) -> EngineError {
        EngineError::InvalidResponse {
            backend: "appimage".into(),
            reason: reason.into(),
        }
    }
    fn type2(path: &Path) -> Result<String, EngineError> {
        let mut file = fs::File::open(path).map_err(|e| Self::invalid(e.to_string()))?;
        let mut header = [0; 20];
        file.read_exact(&mut header)
            .map_err(|e| Self::invalid(e.to_string()))?;
        if &header[..4] != b"\x7fELF" || &header[8..11] != b"AI\x02" {
            return Err(Self::invalid("expected a Type 2 AppImage ELF file"));
        }
        let arch = match u16::from_le_bytes([header[18], header[19]]) {
            62 => "x86_64",
            183 => "aarch64",
            _ => "unknown",
        };
        // Type 2 magic is at offset 8; this seek makes short/truncated files fail above.
        file.seek(SeekFrom::Start(0))
            .map_err(|e| Self::invalid(e.to_string()))?;
        Ok(arch.into())
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
            },
            display_name: "Imported AppImage".into(),
            summary: format!("PkgDeck-managed local Type 2 AppImage ({digest})"),
            installed_version: Some(digest.into()),
            candidate_version: Some(digest.into()),
            update: UpdateAvailability::Current,
        })
    }
    fn installed_packages(&self) -> Result<Vec<Package>, EngineError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return self.external_packages(),
            Err(e) => return Err(Self::invalid(e.to_string())),
        };
        let mut packages: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|e| self.package(e.path()))
            .collect::<Result<_, _>>()?;
        packages.extend(self.external_packages()?);
        Ok(packages)
    }
    fn desktop_value(contents: &str, key: &str) -> Option<String> {
        contents
            .lines()
            .find_map(|line| line.strip_prefix(key).map(str::to_owned))
            .filter(|value| !value.is_empty())
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
                let source = Self::desktop_value(&contents, "TryExec=")?;
                let path = PathBuf::from(source);
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
                Some((
                    Package {
                        id: PackageId {
                            backend: "appimage".into(),
                            name: canonical.display().to_string(),
                            architecture,
                            scope: self.scope(),
                            remote: None,
                        },
                        display_name,
                        summary: "Externally managed local Type 2 AppImage".into(),
                        installed_version: Some(version.clone()),
                        candidate_version: Some(version),
                        update: UpdateAvailability::Current,
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
    fn import(&self, id: &PackageId) -> Result<(), EngineError> {
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
        let bytes = fs::read(&source).map_err(|e| Self::invalid(e.to_string()))?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let name = format!("pkgdeck-{digest}.AppImage");
        fs::create_dir_all(&self.root).map_err(|e| Self::invalid(e.to_string()))?;
        let destination = self.root.join(&name);
        if !destination.exists() {
            fs::copy(&source, &destination).map_err(|e| Self::invalid(e.to_string()))?;
        }
        fs::create_dir_all(&self.applications).map_err(|e| Self::invalid(e.to_string()))?;
        let entry = self
            .applications
            .join(format!("pkgdeck-{}.desktop", &name[8..72]));
        fs::write(entry, format!("[Desktop Entry]\nType=Application\nName=Imported AppImage\nExec=\"{}\" %U\nTerminal=false\n", destination.display())).map_err(|e| Self::invalid(e.to_string()))
    }
    fn updater() -> Result<PathBuf, EngineError> {
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
        let mut command = std::process::Command::new(Self::updater()?);
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
    fn search(&mut self, query: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let source = PathBuf::from(query);
        if source.is_absolute() && source.is_file() {
            let architecture = Self::type2(&source)?;
            return Ok(vec![Package {
                id: PackageId {
                    backend: "appimage".into(),
                    name: query.into(),
                    architecture,
                    scope: Scope::Environment {
                        path: self.root.clone(),
                    },
                    remote: None,
                },
                display_name: source
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("AppImage")
                    .into(),
                summary: "Import local Type 2 AppImage into PkgDeck-managed storage".into(),
                installed_version: None,
                candidate_version: None,
                update: UpdateAvailability::Unknown,
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
                self.import(id)?;
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
            _ => return Err(Self::invalid("foreign or unsupported AppImage operation")),
        }
        Ok(OperationOutcome::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type2(path: &Path) {
        let mut header = [0_u8; 20];
        header[..4].copy_from_slice(b"\x7fELF");
        header[8..11].copy_from_slice(b"AI\x02");
        header[18..20].copy_from_slice(&62_u16.to_le_bytes());
        fs::write(path, header).unwrap();
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
        let import = backend
            .search(source.to_str().unwrap(), &cancel)
            .unwrap()
            .remove(0);
        assert!(backend.details(&import.id, &cancel).is_err());
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
        let desktop = applications.join("audacity.desktop");
        fs::write(
            &desktop,
            format!(
                "[Desktop Entry]\nType=Application\nName=Audacity Portable\nTryExec={}\nX-AppImage-Version=4.0\n",
                external.display()
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
        assert_eq!(
            backend.search(source.to_str().unwrap(), &cancel).unwrap()[0]
                .id
                .architecture,
            "unknown"
        );
        assert!(backend
            .search("relative.AppImage", &cancel)
            .unwrap()
            .is_empty());
        fs::remove_dir_all(base).unwrap();
    }
}
