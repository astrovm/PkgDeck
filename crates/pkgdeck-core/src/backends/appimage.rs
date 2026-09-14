use crate::{engine::*, package::*, process::Cancellation};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
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
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(Self::invalid(e.to_string())),
        };
        entries
            .filter_map(Result::ok)
            .map(|e| self.package(e.path()))
            .collect()
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
        Ok(self
            .installed_packages()?
            .into_iter()
            .filter(|p| p.id.name.contains(query))
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
        _: &Cancellation,
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
}
