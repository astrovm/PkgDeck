//! Portable, read-only inventory exchange. Import never performs package writes.
use crate::{
    engine::{BackendFailure, Engine, Source},
    package::{Availability, Capability, Package, PackageId, Scope},
    process::Cancellation,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fmt,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_BYTES: u64 = 1024 * 1024;
pub const MAX_PACKAGES: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub packages: Vec<Entry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortableScope {
    System,
    User,
    /// An environment path is intentionally omitted: it belongs to one host.
    Environment,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub backend: String,
    pub name: String,
    pub architecture: String,
    pub scope: PortableScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Informational only. Import does not promise or require this version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_version: Option<String>,
}

impl Entry {
    fn from_package(package: &Package) -> Self {
        Self {
            backend: package.id.backend.clone(),
            name: package.id.name.clone(),
            architecture: package.id.architecture.clone(),
            scope: match package.id.scope {
                Scope::System => PortableScope::System,
                Scope::User { .. } => PortableScope::User,
                Scope::Environment { .. } => PortableScope::Environment,
            },
            remote: package.id.remote.clone(),
            reference: package.id.reference.clone(),
            observed_version: package
                .installed_version
                .as_deref()
                .filter(|version| safe_field(version))
                .map(str::to_owned),
        }
    }

    fn matches(&self, id: &PackageId) -> bool {
        self.backend == id.backend
            && self.name == id.name
            && self.architecture == id.architecture
            && self.remote == id.remote
            && self.reference == id.reference
            && matches!(
                (&self.scope, &id.scope),
                (PortableScope::System, Scope::System) | (PortableScope::User, Scope::User { .. })
            )
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum ManifestError {
    Io(String),
    Invalid(String),
    UnsupportedVersion(u32),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) | Self::Invalid(message) => f.write_str(message),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported inventory version {version}")
            }
        }
    }
}

impl std::error::Error for ManifestError {}

fn safe_field(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.chars().any(char::is_control)
        // Package identities are native names/refs, never credential-bearing URLs.
        && !value.contains("://")
}

impl Manifest {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedVersion(self.schema_version));
        }
        if self.packages.len() > MAX_PACKAGES {
            return Err(ManifestError::Invalid(
                "inventory has too many packages".into(),
            ));
        }
        let mut seen = BTreeSet::new();
        for entry in &self.packages {
            if !safe_field(&entry.backend)
                || !safe_field(&entry.name)
                || !safe_field(&entry.architecture)
                || entry.remote.as_deref().is_some_and(|s| !safe_field(s))
                || entry.reference.as_deref().is_some_and(|s| !safe_field(s))
                || entry
                    .observed_version
                    .as_deref()
                    .is_some_and(|s| !safe_field(s))
            {
                return Err(ManifestError::Invalid(
                    "inventory contains an invalid identity or version".into(),
                ));
            }
            let key = (
                &entry.backend,
                &entry.name,
                &entry.architecture,
                &entry.scope,
                &entry.remote,
                &entry.reference,
            );
            if !seen.insert(key) {
                return Err(ManifestError::Invalid(
                    "inventory contains a duplicate package".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Export only exact selected installed identities. Pass an empty selection to
/// export every installed package in the supplied complete inventory.
pub fn export(packages: &[Package], selected: &[PackageId]) -> Result<Manifest, ManifestError> {
    let mut entries = Vec::new();
    let mut found = BTreeSet::new();
    for package in packages {
        if package.installed_version.is_none() {
            continue;
        }
        if selected.is_empty() || selected.contains(&package.id) {
            found.insert(&package.id);
            entries.push(Entry::from_package(package));
        }
    }
    if selected.iter().any(|id| !found.contains(id)) {
        return Err(ManifestError::Invalid(
            "selected package is no longer installed".into(),
        ));
    }
    entries.sort();
    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        packages: entries,
    };
    manifest.validate()?;
    Ok(manifest)
}

pub fn read(path: &Path) -> Result<Manifest, ManifestError> {
    let file = File::open(path).map_err(|e| ManifestError::Io(e.to_string()))?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ManifestError::Io(e.to_string()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ManifestError::Invalid("inventory file is too large".into()));
    }
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|e| ManifestError::Invalid(format!("invalid inventory: {e}")))?;
    manifest.validate()?;
    Ok(manifest)
}

/// Never overwrite an existing path (including a symlink).
pub fn write_new(path: &Path, manifest: &Manifest) -> Result<(), ManifestError> {
    manifest.validate()?;
    let bytes =
        serde_json::to_vec_pretty(manifest).map_err(|e| ManifestError::Invalid(e.to_string()))?;
    if bytes.len() as u64 + 1 > MAX_BYTES {
        return Err(ManifestError::Invalid("inventory file is too large".into()));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| ManifestError::Io(e.to_string()))?;
    file.write_all(&bytes)
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|e| ManifestError::Io(e.to_string()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewStatus {
    AlreadyInstalled,
    Installable,
    Unavailable,
    Unsupported,
    Ambiguous,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PreviewEntry {
    pub package: Entry,
    pub status: PreviewStatus,
    pub reason: String,
    pub proposed_changes: Vec<ProposedChange>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposedChangeKind {
    RepositoryAddition,
    SourceMapping,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProposedChange {
    pub kind: ProposedChangeKind,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Preview {
    pub schema_version: u32,
    pub packages: Vec<PreviewEntry>,
}

/// The caller supplies read-only installed and search reports. No operation is
/// generated or executed by this module.
pub fn preview(
    manifest: &Manifest,
    installed: &[Package],
    offers: &[Package],
    sources: &[Source],
    failures: &[BackendFailure],
) -> Result<Preview, ManifestError> {
    manifest.validate()?;
    let packages = manifest
        .packages
        .iter()
        .map(|entry| {
            let (status, reason) = if entry.scope == PortableScope::Environment {
                (
                    PreviewStatus::Unsupported,
                    "Select an environment on this machine before installing".into(),
                )
            } else if installed.iter().any(|package| entry.matches(&package.id)) {
                (PreviewStatus::AlreadyInstalled, String::new())
            } else if failures
                .iter()
                .any(|failure| failure.backend == entry.backend)
            {
                (
                    PreviewStatus::Unavailable,
                    "The source could not be checked. Try the preview again.".into(),
                )
            } else if let Some(source) = sources.iter().find(|s| s.backend == entry.backend) {
                match &source.availability {
                    Ok(Availability::Available)
                        if !source.capabilities.contains(&Capability::Install) =>
                    {
                        (
                            PreviewStatus::Unsupported,
                            "This source cannot install packages".into(),
                        )
                    }
                    Ok(Availability::Available) => {
                        if offers.iter().any(|package| entry.matches(&package.id)) {
                            (PreviewStatus::Installable, String::new())
                        } else if offers.iter().any(|package| {
                            package.id.backend == entry.backend && package.id.name == entry.name
                        }) {
                            (
                                PreviewStatus::Ambiguous,
                                "Scope, architecture, repository, or ref is different here. Choose one explicitly."
                                    .into(),
                            )
                        } else {
                            (
                                PreviewStatus::Unavailable,
                                "Package or recorded repository is unavailable".into(),
                            )
                        }
                    }
                    _ => (PreviewStatus::Unavailable, "Source is unavailable".into()),
                }
            } else {
                (PreviewStatus::Unavailable, "Source is not installed".into())
            };
            let mut proposed_changes = Vec::new();
            if status != PreviewStatus::AlreadyInstalled && status != PreviewStatus::Installable {
                if let Some(remote) = &entry.remote {
                    if !failures
                        .iter()
                        .any(|failure| failure.backend == entry.backend)
                        && !offers.iter().any(|offer| {
                            offer.id.backend == entry.backend
                                && offer.id.name == entry.name
                                && offer.id.remote == entry.remote
                        })
                    {
                        proposed_changes.push(ProposedChange {
                            kind: ProposedChangeKind::RepositoryAddition,
                            detail: format!(
                                "Check or add repository {remote} for {}",
                                entry.backend
                            ),
                        });
                    }
                }
                let alternatives: BTreeSet<_> = installed
                    .iter()
                    .chain(offers.iter())
                    .filter(|package| {
                        package.id.name == entry.name && package.id.backend != entry.backend
                    })
                    .map(|package| package.id.backend.as_str())
                    .collect();
                for backend in alternatives {
                    proposed_changes.push(ProposedChange {
                        kind: ProposedChangeKind::SourceMapping,
                        detail: format!(
                            "Review {backend} as an alternative source for {}",
                            entry.name
                        ),
                    });
                }
            }
            PreviewEntry {
                package: entry.clone(),
                status,
                reason,
                proposed_changes,
            }
        })
        .collect();
    Ok(Preview {
        schema_version: SCHEMA_VERSION,
        packages,
    })
}

/// Gather only the native read results needed for a manifest preview. Search
/// failures remain visible per entry; they never become installable guesses.
pub fn inspect(
    engine: &mut Engine,
    manifest: &Manifest,
    cancel: &Cancellation,
) -> Result<Preview, ManifestError> {
    manifest.validate()?;
    let installed = engine.installed(cancel);
    let sources = engine.discover(cancel);
    let mut offers = Vec::new();
    let mut failures = installed.failures;
    let mut searched = BTreeSet::new();
    for entry in &manifest.packages {
        if cancel.requested() {
            return Err(ManifestError::Invalid("inventory preview cancelled".into()));
        }
        if entry.scope == PortableScope::Environment
            || installed
                .packages
                .iter()
                .any(|package| entry.matches(&package.id))
            || !searched.insert((&entry.backend, &entry.name))
        {
            continue;
        }
        let result = engine.search_backend(&entry.backend, &entry.name, cancel);
        offers.extend(result.packages);
        failures.extend(result.failures);
    }
    preview(manifest, &installed.packages, &offers, &sources, &failures)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Backend, EngineError};
    use crate::package::UpdateAvailability;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct InventoryFixture {
        installed: Vec<Package>,
        offers: Vec<Package>,
        search_calls: Arc<AtomicUsize>,
        fail_search: bool,
    }
    impl Backend for InventoryFixture {
        fn id(&self) -> &str {
            "flatpak"
        }
        fn capabilities(&self) -> &[Capability] {
            &[
                Capability::Installed,
                Capability::Search,
                Capability::Install,
            ]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn installed(&mut self, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
            Ok(self.installed.clone())
        }
        fn search(&mut self, name: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
            self.search_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_search {
                return Err(EngineError::InvalidResponse {
                    backend: "flatpak".into(),
                    reason: "synthetic catalog failure".into(),
                });
            }
            Ok(self
                .offers
                .iter()
                .filter(|offer| offer.id.name == name)
                .cloned()
                .collect())
        }
    }

    fn package(name: &str, scope: Scope, remote: Option<&str>) -> Package {
        Package {
            id: PackageId {
                backend: "flatpak".into(),
                name: name.into(),
                architecture: "x86_64".into(),
                scope,
                remote: remote.map(str::to_owned),
                reference: Some(format!("app/{name}/x86_64/stable")),
            },
            display_name: name.into(),
            summary: String::new(),
            installed_version: Some("1.0".into()),
            candidate_version: None,
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
        }
    }

    #[test]
    fn export_removes_machine_specific_scope_and_rejects_missing_selection() {
        let selected = package(
            "org.example.App",
            Scope::User { uid: 4242 },
            Some("flathub"),
        );
        let manifest = export(
            std::slice::from_ref(&selected),
            std::slice::from_ref(&selected.id),
        )
        .unwrap();
        let json = serde_json::to_string(&manifest).unwrap();
        assert!(json.contains("\"scope\":\"user\""));
        assert!(!json.contains("4242"));
        assert!(export(&[], &[selected.id]).is_err());
    }

    #[test]
    fn preview_requires_exact_source_and_never_substitutes_a_variant() {
        let source = Source {
            backend: "flatpak".into(),
            capabilities: vec![Capability::Install],
            availability: Ok(Availability::Available),
        };
        let installed_package = package("org.example.App", Scope::User { uid: 1 }, Some("flathub"));
        let manifest = export(std::slice::from_ref(&installed_package), &[]).unwrap();
        let other_remote = package("org.example.App", Scope::User { uid: 2 }, Some("other"));
        let mismatched = preview(
            &manifest,
            &[],
            &[other_remote],
            std::slice::from_ref(&source),
            &[],
        )
        .unwrap();
        assert_eq!(mismatched.packages[0].status, PreviewStatus::Ambiguous);
        assert_eq!(
            mismatched.packages[0].proposed_changes[0].kind,
            ProposedChangeKind::RepositoryAddition
        );
        let matching = preview(&manifest, &[], &[installed_package], &[source], &[]).unwrap();
        assert_eq!(matching.packages[0].status, PreviewStatus::Installable);
        assert!(matching.packages[0].proposed_changes.is_empty());
        let mut partial_offer = package("org.example.App", Scope::User { uid: 3 }, Some("flathub"));
        partial_offer.id.reference = None;
        let partial = preview(&manifest, &[], &[partial_offer], &[], &[]).unwrap();
        assert_eq!(partial.packages[0].status, PreviewStatus::Unavailable);
        assert!(partial.packages[0].proposed_changes.is_empty());
    }

    #[test]
    fn invalid_manifest_cannot_be_read_or_overwrite_existing_file() {
        let mut manifest = Manifest {
            schema_version: SCHEMA_VERSION,
            packages: vec![Entry {
                backend: "apt".into(),
                name: "https://user:secret@example.invalid/pkg".into(),
                architecture: "amd64".into(),
                scope: PortableScope::System,
                remote: None,
                reference: None,
                observed_version: None,
            }],
        };
        assert!(manifest.validate().is_err());
        manifest.packages.clear();
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-manifest-test-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        write_new(&path, &manifest).unwrap();
        assert_eq!(read(&path).unwrap(), manifest);
        assert!(write_new(&path, &manifest).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn preview_proposes_source_mapping_without_acting_on_it() {
        let requested = package("org.example.App", Scope::System, Some("flathub"));
        let manifest = export(std::slice::from_ref(&requested), &[]).unwrap();
        let mut alternative = requested.clone();
        alternative.id.backend = "apt".into();
        alternative.id.remote = None;
        alternative.id.reference = None;
        let preview = preview(&manifest, &[alternative], &[], &[], &[]).unwrap();
        assert_eq!(preview.packages[0].status, PreviewStatus::Unavailable);
        assert!(preview.packages[0]
            .proposed_changes
            .iter()
            .any(|change| change.kind == ProposedChangeKind::SourceMapping
                && change.detail.contains("apt")));
    }

    #[test]
    fn validation_rejects_unsupported_version_duplicates_and_large_lists() {
        let entry = Entry::from_package(&package("org.example.App", Scope::System, None));
        let mut manifest = Manifest {
            schema_version: SCHEMA_VERSION + 1,
            packages: vec![entry.clone()],
        };
        assert_eq!(
            manifest.validate(),
            Err(ManifestError::UnsupportedVersion(SCHEMA_VERSION + 1))
        );
        manifest.schema_version = SCHEMA_VERSION;
        manifest.packages.push(entry.clone());
        assert!(manifest.validate().is_err());
        manifest.packages = (0..=MAX_PACKAGES)
            .map(|index| Entry {
                name: format!("package-{index}"),
                ..entry.clone()
            })
            .collect();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn environment_export_omits_path_and_requires_local_choice() {
        let package = package(
            "virtual-tool",
            Scope::Environment {
                path: "/home/private/.venv".into(),
            },
            None,
        );
        let manifest = export(std::slice::from_ref(&package), &[]).unwrap();
        let text = serde_json::to_string(&manifest).unwrap();
        assert!(!text.contains("/home/private"));
        let preview = preview(&manifest, &[], &[], &[], &[]).unwrap();
        assert_eq!(preview.packages[0].status, PreviewStatus::Unsupported);
    }

    #[test]
    fn cross_machine_versions_are_informational_and_malformed_input_is_rejected() {
        let mut linux = package("org.example.App", Scope::System, None);
        let manifest = export(std::slice::from_ref(&linux), &[]).unwrap();
        linux.installed_version = Some("2.0".into());
        let result = preview(&manifest, &[linux], &[], &[], &[]).unwrap();
        assert_eq!(result.packages[0].status, PreviewStatus::AlreadyInstalled);

        let mut mac = package(
            "example-formula",
            Scope::Environment {
                path: "/opt/homebrew".into(),
            },
            None,
        );
        mac.id.backend = "homebrew".into();
        let exported = export(&[mac], &[]).unwrap();
        assert_eq!(exported.packages[0].backend, "homebrew");
        assert!(!serde_json::to_string(&exported)
            .unwrap()
            .contains("/opt/homebrew"));
        assert!(serde_json::from_str::<Manifest>("{broken").is_err());
        let mut secret_version = package("another-app", Scope::System, None);
        secret_version.installed_version =
            Some("git+https://user:secret@example.invalid/repo".into());
        assert!(export(&[secret_version], &[]).unwrap().packages[0]
            .observed_version
            .is_none());
    }

    #[test]
    fn inspection_queries_only_missing_exact_sources_and_reports_failures() {
        let installed_package = package("org.example.App", Scope::System, Some("flathub"));
        let manifest = export(std::slice::from_ref(&installed_package), &[]).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut engine = Engine::default();
        engine
            .register(InventoryFixture {
                installed: vec![installed_package.clone()],
                offers: vec![],
                search_calls: calls.clone(),
                fail_search: false,
            })
            .unwrap();
        let result = inspect(&mut engine, &manifest, &Cancellation::default()).unwrap();
        assert_eq!(result.packages[0].status, PreviewStatus::AlreadyInstalled);
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let mut offer = installed_package.clone();
        offer.installed_version = None;
        let mut engine = Engine::default();
        engine
            .register(InventoryFixture {
                installed: vec![],
                offers: vec![offer],
                search_calls: calls.clone(),
                fail_search: false,
            })
            .unwrap();
        let result = inspect(&mut engine, &manifest, &Cancellation::default()).unwrap();
        assert_eq!(result.packages[0].status, PreviewStatus::Installable);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let mut engine = Engine::default();
        engine
            .register(InventoryFixture {
                installed: vec![],
                offers: vec![],
                search_calls: calls.clone(),
                fail_search: true,
            })
            .unwrap();
        let result = inspect(&mut engine, &manifest, &Cancellation::default()).unwrap();
        assert_eq!(result.packages[0].status, PreviewStatus::Unavailable);
        assert!(result.packages[0].reason.contains("could not be checked"));
        assert!(result.packages[0].proposed_changes.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn preview_distinguishes_missing_capability_from_unavailable_repository() {
        let installed_package = package("org.example.App", Scope::System, Some("flathub"));
        let manifest = export(std::slice::from_ref(&installed_package), &[]).unwrap();
        let source = Source {
            backend: "flatpak".into(),
            capabilities: vec![Capability::Search],
            availability: Ok(Availability::Available),
        };
        let result = preview(&manifest, &[], &[], std::slice::from_ref(&source), &[]).unwrap();
        assert_eq!(result.packages[0].status, PreviewStatus::Unsupported);
        assert!(result.packages[0].reason.contains("cannot install"));

        let mut install_source = source.clone();
        install_source.capabilities.push(Capability::Install);
        let result = preview(&manifest, &[], &[], &[install_source.clone()], &[]).unwrap();
        assert_eq!(result.packages[0].status, PreviewStatus::Unavailable);
        assert!(result.packages[0].reason.contains("repository"));
        assert_eq!(
            result.packages[0].proposed_changes[0].kind,
            ProposedChangeKind::RepositoryAddition
        );

        install_source.availability = Ok(Availability::Unavailable(
            "synthetic missing manager".into(),
        ));
        let result = preview(&manifest, &[], &[], &[install_source], &[]).unwrap();
        assert_eq!(result.packages[0].status, PreviewStatus::Unavailable);
        assert!(result.packages[0].reason.contains("Source is unavailable"));
    }

    #[test]
    fn oversized_files_and_cancelled_preview_stop_before_install_plan() {
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-oversized-inventory-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(
            matches!(read(&path), Err(ManifestError::Invalid(message)) if message.contains("too large"))
        );
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(read(&path), Err(ManifestError::Io(_))));

        let package = package("org.example.App", Scope::System, None);
        let manifest = export(std::slice::from_ref(&package), &[]).unwrap();
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(
            matches!(inspect(&mut Engine::default(), &manifest, &cancel), Err(ManifestError::Invalid(message)) if message.contains("cancelled"))
        );
        assert!(ManifestError::UnsupportedVersion(42)
            .to_string()
            .contains("42"));
    }
}
