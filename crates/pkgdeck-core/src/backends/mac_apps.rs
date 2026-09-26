//! Read-only macOS bundle inventory. A cask suggestion never authorizes a write.
use super::*;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path},
};

const ID: &str = "macos-apps";
const MAX_ENTRIES: usize = 4096;

trait AppIo: Send {
    fn plist(&self, path: &Path, cancel: &Cancellation) -> Result<Value, EngineError>;
    fn ownership(
        &self,
        cancel: &Cancellation,
    ) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError>;
}

struct NativeApps(Host);

impl AppIo for NativeApps {
    fn plist(&self, path: &Path, cancel: &Cancellation) -> Result<Value, EngineError> {
        let metadata = fs::metadata(path).map_err(ExecutionError::from)?;
        if !metadata.is_file() || metadata.len() > 1024 * 1024 {
            return Err(invalid(ID, "app metadata is not a bounded regular file"));
        }
        // plutil handles both binary and XML plists; it never launches the app.
        let result = self.0.read(
            Path::new("/usr/bin/plutil"),
            &[
                "-convert".into(),
                "json".into(),
                "-o".into(),
                "-".into(),
                "--".into(),
                path.into(),
            ],
            Limits {
                timeout: Duration::from_secs(5),
                output_bytes: 1024 * 1024,
            },
            cancel,
        )?;
        serde_json::from_slice(&bytes(ID, result)?).map_err(|e| invalid(ID, e))
    }

    fn ownership(
        &self,
        cancel: &Cancellation,
    ) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError> {
        let root = bytes(ID, self.0.brew(&["--caskroom".into()], cancel, false)?)?;
        let root = PathBuf::from(
            std::str::from_utf8(&root)
                .map_err(|e| invalid(ID, e))?
                .trim(),
        );
        if !root.is_absolute() {
            return Err(invalid(ID, "Homebrew returned a relative Caskroom"));
        }
        let installed = bytes(
            ID,
            self.0.brew(
                &[
                    "info".into(),
                    "--json=v2".into(),
                    "--cask".into(),
                    "--installed".into(),
                ],
                cancel,
                false,
            )?,
        )?;
        cask_owners(&root, &installed)
    }
}

fn relative_artifact(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

/// Homebrew leaves an app-artifact symlink in its installed version directory.
/// Match its canonical destination, never just the app name or bundle ID. Only
/// records from `brew info --installed` participate; stale staging directories
/// and a second copy of the same app do not establish ownership.
fn cask_owners(root: &Path, data: &[u8]) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError> {
    #[derive(Deserialize)]
    struct InstalledCask {
        token: String,
        full_token: String,
        installed: Option<String>,
        artifacts: Vec<Value>,
    }
    #[derive(Deserialize)]
    struct Report {
        casks: Vec<InstalledCask>,
    }
    let report: Report = serde_json::from_slice(data).map_err(|e| invalid(ID, e))?;
    let mut owners: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    for cask in report.casks {
        let Some(version) = cask.installed else {
            continue;
        };
        if !cask_token(&cask.token)
            || cask.token.contains('/')
            || !cask_token(&cask.full_token)
            || !relative_artifact(Path::new(&version))
            || Path::new(&version).components().count() != 1
        {
            return Err(invalid(ID, "invalid installed cask identity"));
        }
        for artifact in cask.artifacts {
            let Some(source) = artifact
                .get("app")
                .and_then(Value::as_array)
                .and_then(|app| app.first())
                .and_then(Value::as_str)
            else {
                continue;
            };
            if !relative_artifact(Path::new(source)) {
                return Err(invalid(ID, "invalid Homebrew app artifact"));
            }
            let link = root.join(&cask.token).join(&version).join(source);
            if fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink()) {
                if let Ok(target) = fs::canonicalize(&link) {
                    owners
                        .entry(target)
                        .or_default()
                        .push(cask.full_token.clone());
                }
            }
        }
    }
    for names in owners.values_mut() {
        names.sort();
        names.dedup();
    }
    Ok(owners)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= 512 && !s.chars().any(char::is_control))
}

fn cask_candidate(bundle_id: &str) -> Option<&'static str> {
    match bundle_id {
        "com.microsoft.VSCode" => Some("visual-studio-code"),
        "org.mozilla.firefox" => Some("firefox"),
        "md.obsidian" => Some("obsidian"),
        _ => None,
    }
}

pub struct MacApps {
    roots: Vec<(PathBuf, Scope)>,
    io: Box<dyn AppIo>,
    snapshot: Option<Vec<PackageDetails>>,
}

impl MacApps {
    pub fn native() -> Self {
        let host = Host::current();
        let mut roots = vec![(PathBuf::from("/Applications"), Scope::System)];
        if let Some(home) = host
            .var("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
        {
            roots.push((
                home.join("Applications"),
                Scope::User {
                    uid: rustix::process::getuid().as_raw(),
                },
            ));
        }
        Self {
            roots,
            io: Box::new(NativeApps(host)),
            snapshot: None,
        }
    }

    fn inventory(&mut self, cancel: &Cancellation) -> Result<&[PackageDetails], EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        if self.snapshot.is_none() {
            let ownership = self.io.ownership(cancel);
            if cancel.requested() || matches!(&ownership, Err(EngineError::Cancelled)) {
                return Err(EngineError::Cancelled);
            }
            let mut apps = Vec::new();
            let mut seen = BTreeSet::new();
            let mut budget = MAX_ENTRIES;
            for (root, scope) in &self.roots {
                self.scan(
                    root,
                    scope,
                    0,
                    &mut budget,
                    &mut seen,
                    &mut apps,
                    &ownership,
                    cancel,
                )?;
            }
            apps.sort_by(|a, b| a.package.id.cmp(&b.package.id));
            self.snapshot = Some(apps);
        }
        Ok(self.snapshot.as_deref().unwrap_or_default())
    }

    #[allow(clippy::too_many_arguments)]
    fn scan(
        &self,
        root: &Path,
        scope: &Scope,
        depth: usize,
        budget: &mut usize,
        seen: &mut BTreeSet<PathBuf>,
        apps: &mut Vec<PackageDetails>,
        ownership: &Result<BTreeMap<PathBuf, Vec<String>>, EngineError>,
        cancel: &Cancellation,
    ) -> Result<(), EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(ExecutionError::from(error).into()),
        };
        // Sorting makes the chosen path deterministic when aliases exist.
        let mut paths = Vec::new();
        for entry in entries {
            if cancel.requested() {
                return Err(EngineError::Cancelled);
            }
            *budget = budget
                .checked_sub(1)
                .ok_or_else(|| invalid(ID, "application inventory exceeds 4096 entries"))?;
            paths.push(entry.map_err(ExecutionError::from)?.path());
        }
        paths.sort();
        for path in paths {
            if cancel.requested() {
                return Err(EngineError::Cancelled);
            }
            let metadata = fs::symlink_metadata(&path).map_err(ExecutionError::from)?;
            if path.extension().is_some_and(|e| e == "app") && path.is_dir() {
                let canonical = fs::canonicalize(&path).map_err(ExecutionError::from)?;
                if seen.insert(canonical.clone()) {
                    apps.push(self.describe(&path, &canonical, scope, ownership, cancel)?);
                }
                // Never recurse into app bundles and list their helper apps.
            } else if metadata.is_dir()
                && depth < 4
                && !path
                    .file_name()
                    .is_some_and(|n| n.as_encoded_bytes().starts_with(b"."))
            {
                self.scan(
                    &path,
                    scope,
                    depth + 1,
                    budget,
                    seen,
                    apps,
                    ownership,
                    cancel,
                )?;
            }
        }
        Ok(())
    }

    fn describe(
        &self,
        path: &Path,
        canonical: &Path,
        scope: &Scope,
        ownership: &Result<BTreeMap<PathBuf, Vec<String>>, EngineError>,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let metadata = self.io.plist(&path.join("Contents/Info.plist"), cancel);
        if cancel.requested() || matches!(&metadata, Err(EngineError::Cancelled)) {
            return Err(EngineError::Cancelled);
        }
        let value = metadata.as_ref().ok().unwrap_or(&Value::Null);
        let bundle_id = text(value, "CFBundleIdentifier");
        let display = text(value, "CFBundleDisplayName")
            .or_else(|| text(value, "CFBundleName"))
            .map(str::to_owned)
            .unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into()
            });
        let version =
            text(value, "CFBundleShortVersionString").or_else(|| text(value, "CFBundleVersion"));
        let owner = match ownership {
            Ok(records) => match records.get(canonical) {
                Some(names) if names.len() == 1 => format!("Managed by Homebrew ({})", names[0]),
                Some(names) => format!("Homebrew ownership is ambiguous ({})", names.join(", ")),
                None => "No Homebrew ownership record found".into(),
            },
            Err(_) => "Homebrew ownership could not be checked".into(),
        };
        let receipt = path.join("Contents/_MASReceipt/receipt").is_file();
        let has_owner = ownership
            .as_ref()
            .is_ok_and(|records| records.contains_key(canonical));
        let candidate = bundle_id
            .and_then(cask_candidate)
            .filter(|_| !receipt && !has_owner);
        let suggestion =
            candidate.map(|token| format!("Available through Homebrew: {token} (candidate)"));
        let mut description = vec![format!("Location: {}", path.display()), owner.clone()];
        if let Some(id) = bundle_id {
            description.push(format!("Bundle identifier: {id}"));
        }
        if let Some(build) = text(value, "CFBundleVersion") {
            description.push(format!("Bundle build: {build}"));
        }
        if let Err(error) = &metadata {
            description.push(format!("Bundle metadata unavailable: {error}"));
        }
        if let Err(error) = ownership {
            description.push(format!("Homebrew check: {error}"));
        }
        if receipt {
            description.push("App Store receipt present. Keep App Store management; no cask suggestion is offered.".into());
        } else if let Some(suggestion) = &suggestion {
            description.push(suggestion.clone());
            description.push("Matching evidence: the bundle identifier matches PkgDeck's curated cask catalog. Publisher signature, edition/channel, architecture, and artifact equality have not been verified. This is a discovery suggestion, not an adoption plan.".into());
        }
        description.push("Read-only inventory. PkgDeck cannot install, update, remove, or adopt this app from this source.".into());
        let summary = format!(
            "{owner}{} · {}",
            suggestion
                .as_ref()
                .map(|s| format!(" · {s}"))
                .unwrap_or_default(),
            path.display()
        );
        Ok(PackageDetails {
            package: Package {
                id: PackageId {
                    backend: ID.into(),
                    name: path.to_string_lossy().into(),
                    architecture: "unknown".into(),
                    scope: scope.clone(),
                    remote: None,
                    reference: Some(path.to_string_lossy().into()),
                },
                display_name: display,
                summary,
                // The engine requires an installed marker. Unknown is explicit,
                // never inferred from a cask's version or the host architecture.
                installed_version: Some(version.unwrap_or("unknown").into()),
                candidate_version: None,
                update: UpdateAvailability::Unknown,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
            },
            description: description.join("\n\n"),
            homepage: candidate.map(|token| format!("https://formulae.brew.sh/cask/{token}")),
            dependencies: vec![],
        })
    }
}

impl Backend for MacApps {
    fn id(&self) -> &str {
        ID
    }
    fn capabilities(&self) -> &[Capability] {
        &[
            Capability::Search,
            Capability::Installed,
            Capability::Details,
        ]
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        Ok(if cfg!(target_os = "macos") {
            Availability::Available
        } else {
            Availability::Unavailable("Application bundle discovery requires macOS".into())
        })
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .inventory(cancel)?
            .iter()
            .map(|d| d.package.clone())
            .collect())
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let query = query.to_lowercase();
        Ok(self
            .installed(cancel)?
            .into_iter()
            .filter(|p| {
                p.id.name.to_lowercase().contains(&query)
                    || p.display_name.to_lowercase().contains(&query)
            })
            .collect())
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        self.inventory(cancel)?
            .iter()
            .find(|d| d.package.id == *id)
            .cloned()
            .ok_or(EngineError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "pkgdeck-mac-apps-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn bundle(&self, name: &str, id: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(path.join("Contents")).unwrap();
            fs::write(
                path.join("Contents/Info.plist"),
                json!({
                    "CFBundleIdentifier": id, "CFBundleName": "Fixture App",
                    "CFBundleShortVersionString": "1.2.3", "CFBundleVersion": "456"
                })
                .to_string(),
            )
            .unwrap();
            path
        }
        fn backend(
            &self,
            ownership: Result<BTreeMap<PathBuf, Vec<String>>, EngineError>,
        ) -> MacApps {
            MacApps {
                roots: vec![(self.0.clone(), Scope::System)],
                io: Box::new(FakeIo(ownership)),
                snapshot: None,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct FakeIo(Result<BTreeMap<PathBuf, Vec<String>>, EngineError>);
    impl AppIo for FakeIo {
        fn plist(&self, path: &Path, _: &Cancellation) -> Result<Value, EngineError> {
            let data = fs::read(path).map_err(ExecutionError::from)?;
            serde_json::from_slice(&data).map_err(|e| invalid(ID, e))
        }
        fn ownership(
            &self,
            _: &Cancellation,
        ) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError> {
            self.0.clone()
        }
    }
    fn record(token: &str, source: &str, version: Value) -> Value {
        json!({"token": token, "full_token": token, "installed": version, "artifacts": [{"app": [source]}]})
    }
    fn owners(
        root: &Path,
        casks: Vec<Value>,
    ) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError> {
        cask_owners(root, &serde_json::to_vec(&json!({"casks": casks})).unwrap())
    }

    #[test]
    fn inventory_preserves_copies_scopes_and_candidate_evidence_without_writes() {
        let f = Fixture::new();
        let system = f.bundle("system/Visual Studio Code.app", "com.microsoft.VSCode");
        let user = f.bundle("user/Renamed Editor.app", "com.microsoft.VSCode");
        f.bundle(
            "system/Visual Studio Code.app/Contents/Helper.app",
            "com.microsoft.helper",
        );
        symlink(&system, f.0.join("system/Alias.app")).unwrap();
        let mut backend = f.backend(Ok(BTreeMap::from([(
            fs::canonicalize(&system).unwrap(),
            vec!["visual-studio-code".into()],
        )])));
        backend.roots = vec![
            (f.0.join("system"), Scope::System),
            (f.0.join("user"), Scope::User { uid: 123 }),
        ];
        let cancel = Cancellation::default();
        let packages = backend.installed(&cancel).unwrap();
        assert_eq!(
            packages.len(),
            2,
            "aliases and nested helper apps are not extra installations"
        );
        let external = packages
            .iter()
            .find(|p| p.id.reference.as_deref() == user.to_str())
            .unwrap();
        assert_eq!(external.id.scope, Scope::User { uid: 123 });
        assert_eq!(external.installed_version.as_deref(), Some("1.2.3"));
        assert_eq!(external.update, UpdateAvailability::Unknown);
        assert!(external.candidate_version.is_none());
        assert!(external
            .summary
            .contains("No Homebrew ownership record found"));
        assert!(external
            .summary
            .contains("Available through Homebrew: visual-studio-code (candidate)"));
        let details = backend.details(&external.id, &cancel).unwrap();
        assert!(details.description.contains("com.microsoft.VSCode"));
        assert!(details.description.contains("have not been verified"));
        assert!(details.description.contains("Bundle build: 456"));
        let managed = packages
            .iter()
            .find(|p| {
                p.summary
                    .contains("Managed by Homebrew (visual-studio-code)")
            })
            .unwrap();
        assert!(!managed.summary.contains("Available through Homebrew"));
        assert_eq!(backend.search("renamed editor", &cancel).unwrap().len(), 1);
        let mut wrong = external.id.clone();
        wrong.scope = Scope::System;
        assert!(matches!(
            backend.details(&wrong, &cancel),
            Err(EngineError::NotFound)
        ));
        for operation in [
            Operation::Install(external.id.clone()),
            Operation::Remove(external.id.clone()),
            Operation::Upgrade(external.id.clone()),
        ] {
            assert!(matches!(
                backend.execute(&operation, &cancel, &mut |_| panic!(
                    "read-only source emitted write progress"
                )),
                Err(EngineError::Unsupported { .. })
            ));
        }
        assert!(user.exists() && system.exists());
    }

    #[test]
    fn catalog_uses_identifiers_never_names_and_excludes_app_store_receipts() {
        let f = Fixture::new();
        f.bundle("Obsidian.app", "unrelated.publisher");
        f.bundle("Firefox Beta.app", "org.mozilla.firefoxbeta");
        f.bundle("Known Firefox.app", "org.mozilla.firefox");
        f.bundle("Renamed.app", "md.obsidian");
        let store = f.bundle("Store.app", "md.obsidian");
        fs::create_dir_all(store.join("Contents/_MASReceipt")).unwrap();
        fs::write(store.join("Contents/_MASReceipt/receipt"), "receipt").unwrap();
        let mut backend = f.backend(Ok(BTreeMap::new()));
        let cancel = Cancellation::default();
        let packages = backend.installed(&cancel).unwrap();
        for package in packages {
            let details = backend.details(&package.id, &cancel).unwrap();
            if package.id.name.ends_with("Known Firefox.app") {
                assert!(details.homepage.unwrap().ends_with("/firefox"));
            } else if package.id.name.ends_with("Renamed.app") {
                assert!(details.homepage.unwrap().ends_with("/obsidian"));
            } else {
                assert!(details.homepage.is_none());
                assert!(!details.description.contains("Available through Homebrew"));
            }
            if package.id.name.ends_with("Store.app") {
                assert!(details.description.contains("App Store receipt present"));
            }
        }
    }

    #[test]
    fn unreadable_metadata_and_failed_brew_checks_keep_apps_visible_as_unknown() {
        let f = Fixture::new();
        let broken = f.bundle("Broken.app", "md.obsidian");
        fs::write(broken.join("Contents/Info.plist"), "bad plist").unwrap();
        f.bundle("Healthy.app", "md.obsidian");
        let mut backend = f.backend(Err(invalid(ID, "brew unavailable")));
        let cancel = Cancellation::default();
        let apps = backend.installed(&cancel).unwrap();
        assert_eq!(apps.len(), 2);
        assert!(apps
            .iter()
            .all(|p| p.summary.contains("could not be checked")));
        let broken = apps.iter().find(|p| p.display_name == "Broken").unwrap();
        assert_eq!(broken.installed_version.as_deref(), Some("unknown"));
        let details = backend.details(&broken.id, &cancel).unwrap();
        assert!(details.description.contains("metadata unavailable"));
        assert!(details.description.contains("brew unavailable"));
        assert!(details.homepage.is_none());
    }

    #[test]
    fn ownership_requires_installed_record_and_exact_artifact_symlink_target() {
        let f = Fixture::new();
        let app = f.bundle("Applications/Editor.app", "com.microsoft.VSCode");
        let other = f.bundle("Other/Editor.app", "com.microsoft.VSCode");
        let root = f.0.join("Caskroom");
        let stage = root.join("editor/1.0");
        fs::create_dir_all(&stage).unwrap();
        symlink(&app, stage.join("Editor.app")).unwrap();
        let records = owners(&root, vec![record("editor", "Editor.app", json!("1.0"))]).unwrap();
        assert_eq!(records[&fs::canonicalize(&app).unwrap()], vec!["editor"]);
        assert!(!records.contains_key(&fs::canonicalize(other).unwrap()));
        assert!(owners(&root, vec![]).unwrap().is_empty());
        assert!(
            owners(&root, vec![record("editor", "Editor.app", Value::Null)])
                .unwrap()
                .is_empty()
        );
        assert!(
            owners(&root, vec![record("editor", "Editor.app", json!("2.0"))])
                .unwrap()
                .is_empty()
        );
        fs::remove_file(stage.join("Editor.app")).unwrap();
        fs::create_dir(stage.join("Editor.app")).unwrap();
        assert!(
            owners(&root, vec![record("editor", "Editor.app", json!("1.0"))])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn ownership_rejects_malformed_and_escaping_records() {
        let f = Fixture::new();
        assert!(cask_owners(&f.0, br#"{}"#).is_err());
        for entry in [
            record("../editor", "Editor.app", json!("1")),
            record("editor", "../Editor.app", json!("1")),
            record("editor", "/Applications/Editor.app", json!("1")),
            record("editor", "Editor.app", json!("../../outside")),
        ] {
            assert!(owners(&f.0, vec![entry]).is_err());
        }
    }

    #[test]
    fn cancellation_is_not_downgraded_to_unknown_or_served_from_cache() {
        let f = Fixture::new();
        f.bundle("App.app", "md.obsidian");
        let mut backend = f.backend(Err(EngineError::Cancelled));
        assert!(matches!(
            backend.installed(&Cancellation::default()),
            Err(EngineError::Cancelled)
        ));
        let mut backend = f.backend(Ok(BTreeMap::new()));
        let cancel = Cancellation::default();
        backend.installed(&cancel).unwrap();
        cancel.cancel();
        assert!(matches!(
            backend.installed(&cancel),
            Err(EngineError::Cancelled)
        ));
        assert!(matches!(
            backend.detect(&cancel),
            Err(EngineError::Cancelled)
        ));
    }

    #[test]
    fn nested_folders_are_scanned_but_directory_symlinks_are_not_followed() {
        let f = Fixture::new();
        f.bundle("Utilities/App.app", "md.obsidian");
        symlink(&f.0, f.0.join("loop")).unwrap();
        let mut backend = f.backend(Ok(BTreeMap::new()));
        backend.roots.push((f.0.join("missing"), Scope::System));
        assert_eq!(
            backend.installed(&Cancellation::default()).unwrap().len(),
            1
        );
    }

    #[test]
    fn inventory_rejects_excessive_directory_entries_instead_of_truncating() {
        let f = Fixture::new();
        for n in 0..=MAX_ENTRIES {
            fs::write(f.0.join(n.to_string()), "").unwrap();
        }
        let mut backend = f.backend(Ok(BTreeMap::new()));
        assert!(matches!(
            backend.installed(&Cancellation::default()),
            Err(EngineError::InvalidResponse { .. })
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_plutil_reads_xml_and_binary_bundles_without_launching_them() {
        let f = Fixture::new();
        let app = f.bundle("Native.app", "md.obsidian");
        let path = app.join("Contents/Info.plist");
        fs::write(&path, r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>md.obsidian</string></dict></plist>"#).unwrap();
        let native = NativeApps(Host::current());
        let cancel = Cancellation::default();
        assert_eq!(
            native.plist(&path, &cancel).unwrap()["CFBundleIdentifier"],
            "md.obsidian"
        );
        let result = native
            .0
            .read(
                Path::new("/usr/bin/plutil"),
                &[
                    "-convert".into(),
                    "binary1".into(),
                    "--".into(),
                    path.clone().into(),
                ],
                Limits::default(),
                &cancel,
            )
            .unwrap();
        assert_eq!(result.code, Some(0));
        assert_eq!(
            native.plist(&path, &cancel).unwrap()["CFBundleIdentifier"],
            "md.obsidian"
        );
    }
}
