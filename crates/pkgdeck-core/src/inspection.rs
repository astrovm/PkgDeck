//! Read-only command resolution and installed-copy audit. Neither service
//! executes the inspected command or derives ownership from similar names.
use crate::{
    host::{Host, Runtime},
    package::{same_app_group_keys_all, Package, PackageId, Scope},
    process::{Cancellation, ExecutionError, Limits},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    Executable,
    NotExecutable,
    BrokenLink,
    LinkLoop,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerState {
    Known,
    Ambiguous,
    Unmatched,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Owner {
    pub path: PathBuf,
    pub manager: String,
    pub native_name: String,
    pub state: OwnerState,
    pub packages: Vec<PackageId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExecutableCandidate {
    pub path: PathBuf,
    pub target: Option<PathBuf>,
    pub state: CandidateState,
    pub owners: Vec<Owner>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExecutableReport {
    pub command: String,
    pub environment: String,
    pub path: String,
    pub resolved: Option<PathBuf>,
    pub candidates: Vec<ExecutableCandidate>,
    pub ownership_note: String,
}

/// Authoritative ownership provider. Fixtures can supply manager records
/// without requiring a package manager on the test host.
pub trait OwnershipSource {
    fn owners(&self, path: &Path, cancel: &Cancellation) -> Vec<(String, String)>;
}

fn valid_command(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.contains('/')
        && name != "."
        && name != ".."
        && !name.chars().any(char::is_control)
}
fn host_path(host: &Host, inspected: &Path, mapped: &Path) -> PathBuf {
    if host.runtime == Runtime::Flatpak {
        mapped.strip_prefix("/run/host").map_or_else(
            |_| inspected.to_owned(),
            |relative| Path::new("/").join(relative),
        )
    } else {
        mapped.to_owned()
    }
}

pub fn inspect_with(
    host: &Host,
    command: &str,
    installed: &[Package],
    source: &impl OwnershipSource,
    cancel: &Cancellation,
) -> Result<ExecutableReport, ExecutionError> {
    if !valid_command(command) {
        return Err(ExecutionError::Invalid(
            "expected a command name without a path".into(),
        ));
    }
    let path = host.var("PATH").unwrap_or_default();
    let mut report = ExecutableReport {
        command: command.into(),
        environment: format!("{:?} host", host.runtime),
        path: path.to_string_lossy().into_owned(),
        resolved: None,
        candidates: Vec::new(),
        ownership_note:
            "Ownership comes only from native package databases; unreported paths remain unknown."
                .into(),
    };
    for directory in std::env::split_paths(&path).take(256) {
        if cancel.requested() {
            return Err(ExecutionError::Cancelled);
        }
        if !directory.is_absolute() {
            continue;
        }
        let candidate = directory.join(command);
        let mapped = host.filesystem_path(&candidate);
        let Ok(link) = fs::symlink_metadata(&mapped) else {
            continue;
        };
        let (target, state) = match fs::canonicalize(&mapped) {
            Ok(canonical) => {
                let state = match fs::metadata(&canonical) {
                    Ok(info)
                        if info.is_file()
                            && info.permissions().mode() & 0o111 != 0
                            && rustix::fs::access(&canonical, rustix::fs::Access::EXEC_OK)
                                .is_ok() =>
                    {
                        CandidateState::Executable
                    }
                    Ok(_) => CandidateState::NotExecutable,
                    Err(_) => CandidateState::Unavailable,
                };
                (
                    link.file_type()
                        .is_symlink()
                        .then(|| host_path(host, &candidate, &canonical)),
                    state,
                )
            }
            Err(error) if error.raw_os_error() == Some(40) => (None, CandidateState::LinkLoop),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (None, CandidateState::BrokenLink)
            }
            Err(_) => (None, CandidateState::Unavailable),
        };
        if report.resolved.is_none() && state == CandidateState::Executable {
            report.resolved = Some(candidate.clone());
        }
        let mut records: Vec<_> = source
            .owners(&candidate, cancel)
            .into_iter()
            .map(|(manager, name)| (candidate.clone(), manager, name))
            .collect();
        if let Some(target) = &target {
            if target != &candidate {
                records.extend(
                    source
                        .owners(target, cancel)
                        .into_iter()
                        .map(|(manager, name)| (target.clone(), manager, name)),
                );
            }
        }
        records.sort();
        records.dedup();
        let mut owners = if records.is_empty() {
            vec![Owner {
                path: candidate.clone(),
                manager: String::new(),
                native_name: String::new(),
                state: OwnerState::Unknown,
                packages: vec![],
            }]
        } else {
            records
                .into_iter()
                .map(|(path, manager, native_name)| {
                    let packages = installed
                        .iter()
                        .filter(|package| {
                            package.installed_version.is_some()
                                && package.id.backend == manager
                                && (package.id.name == native_name
                                    || format!("{}:{}", package.id.name, package.id.architecture)
                                        == native_name)
                        })
                        .map(|package| package.id.clone())
                        .collect::<Vec<_>>();
                    let state = match packages.len() {
                        0 => OwnerState::Unmatched,
                        1 => OwnerState::Known,
                        _ => OwnerState::Ambiguous,
                    };
                    Owner {
                        path,
                        manager,
                        native_name,
                        state,
                        packages,
                    }
                })
                .collect()
        };
        if owners
            .iter()
            .any(|owner| matches!(owner.state, OwnerState::Known | OwnerState::Ambiguous))
        {
            owners.retain(|owner| owner.state != OwnerState::Unmatched);
        }
        report.candidates.push(ExecutableCandidate {
            path: candidate,
            target,
            state,
            owners,
        });
    }
    Ok(report)
}

pub struct NativeOwnership<'a> {
    pub host: &'a Host,
    pub rpm_backend: String,
}
impl NativeOwnership<'_> {
    fn query(&self, executable: &str, args: &[OsString], cancel: &Cancellation) -> Option<String> {
        let path = Path::new("/usr/bin").join(executable);
        let output = self
            .host
            .read(
                &path,
                args,
                Limits {
                    timeout: Duration::from_secs(5),
                    output_bytes: 64 * 1024,
                },
                cancel,
            )
            .ok()?;
        (output.code == Some(0) && !output.truncated)
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }
}
fn parse_owner_records(manager: &str, text: &str) -> Vec<(String, String)> {
    let mut records = Vec::new();
    for line in text.lines().take(32) {
        match manager {
            "apt" => {
                if let Some((names, _)) = line.split_once(": ") {
                    for name in names.split(", ").filter(|name| !name.is_empty()) {
                        records.push(("apt".into(), name.into()));
                    }
                }
            }
            "dnf" | "zypper" | "rpm" => {
                if let Some((name, arch)) = line.split_once('|') {
                    if !name.is_empty() && !arch.is_empty() {
                        records.push((manager.into(), format!("{name}:{arch}")));
                    }
                }
            }
            "pacman" if !line.is_empty() => records.push(("pacman".into(), line.into())),
            _ => {}
        }
    }
    records
}
impl OwnershipSource for NativeOwnership<'_> {
    fn owners(&self, path: &Path, cancel: &Cancellation) -> Vec<(String, String)> {
        let target = path.as_os_str().to_os_string();
        let mut records = Vec::new();
        if let Some(text) = self.query(
            "dpkg-query",
            &["-S".into(), "--".into(), target.clone()],
            cancel,
        ) {
            records.extend(parse_owner_records("apt", &text));
        }
        if let Some(text) = self.query(
            "rpm",
            &[
                "-qf".into(),
                "--qf".into(),
                "%{NAME}|%{ARCH}\\n".into(),
                "--".into(),
                target.clone(),
            ],
            cancel,
        ) {
            records.extend(parse_owner_records(&self.rpm_backend, &text));
        }
        if let Some(text) = self.query("pacman", &["-Qqo".into(), "--".into(), target], cancel) {
            records.extend(parse_owner_records("pacman", &text));
        }
        records
    }
}

pub fn inspect_native(
    host: &Host,
    command: &str,
    installed: &[Package],
    cancel: &Cancellation,
) -> Result<ExecutableReport, ExecutionError> {
    let rpm_sources: BTreeSet<_> = installed
        .iter()
        .filter(|package| {
            package.installed_version.is_some()
                && matches!(package.id.backend.as_str(), "dnf" | "zypper")
        })
        .map(|package| package.id.backend.as_str())
        .collect();
    let rpm_backend = if rpm_sources.len() == 1 {
        rpm_sources.into_iter().next().unwrap()
    } else {
        "rpm"
    };
    inspect_with(
        host,
        command,
        installed,
        &NativeOwnership {
            host,
            rpm_backend: rpm_backend.into(),
        },
        cancel,
    )
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InstalledCopy {
    pub package: PackageId,
    pub display_name: String,
    pub installed_version: String,
    pub location: Option<PathBuf>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CopyGroup {
    pub key: String,
    pub copies: Vec<InstalledCopy>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LeftoverData {
    pub manager: String,
    pub native_name: String,
    pub path: PathBuf,
    pub size_bytes: Option<u64>,
    pub kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuditReport {
    pub groups: Vec<CopyGroup>,
    pub installed_copies: Vec<InstalledCopy>,
    pub leftovers: Vec<LeftoverData>,
    pub data_note: String,
}

fn known_location(id: &PackageId) -> Option<PathBuf> {
    match (&*id.backend, &id.scope) {
        ("appimage", Scope::Environment { path })
            if id.name.starts_with("pkgdeck-")
                && id.name.ends_with(".AppImage")
                && !id.name.contains('/') =>
        {
            Some(path.join(&id.name))
        }
        ("appimage", _) if Path::new(&id.name).is_absolute() => Some(PathBuf::from(&id.name)),
        _ => None,
    }
}

pub fn audit(installed: &[Package], leftovers: Vec<LeftoverData>) -> AuditReport {
    let keys = same_app_group_keys_all(installed);
    let mut groups: BTreeMap<String, Vec<InstalledCopy>> = BTreeMap::new();
    let mut copies = Vec::new();
    for (package, key) in installed.iter().zip(keys) {
        let Some(version) = &package.installed_version else {
            continue;
        };
        let copy = InstalledCopy {
            package: package.id.clone(),
            display_name: package.display_name.clone(),
            installed_version: version.clone(),
            location: known_location(&package.id),
        };
        if let Some(key) = key {
            groups.entry(key).or_default().push(copy.clone());
        }
        copies.push(copy);
    }
    copies.sort_by(|a, b| a.package.cmp(&b.package));
    let groups = groups
        .into_iter()
        .map(|(key, mut copies)| {
            copies.sort_by(|a, b| a.package.cmp(&b.package));
            CopyGroup { key, copies }
        })
        .collect();
    AuditReport { groups, installed_copies: copies, leftovers,
        data_note: "Only manager-reported residual files are listed. Other application data is unknown and has not been scanned.".into() }
}

/// Parse only dpkg's explicit residual-config state and Conffiles ownership.
/// The caller decides whether paths still exist; no directory is searched.
pub fn dpkg_residuals(status: &str, root: &Path) -> Vec<LeftoverData> {
    let mut rows = Vec::new();
    let mut claims: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    for paragraph in status.split("\n\n") {
        let mut name = None;
        let mut residual = false;
        let mut conffiles = false;
        let mut paths = Vec::new();
        for line in paragraph.lines() {
            if let Some(value) = line.strip_prefix("Package: ") {
                name = Some(value.trim());
                conffiles = false;
            } else if let Some(value) = line.strip_prefix("Status: ") {
                residual = value.trim() == "deinstall ok config-files";
                conffiles = false;
            } else if line == "Conffiles:" {
                conffiles = true;
            } else if line.starts_with(' ') && conffiles {
                if let Some(path) = line.split_whitespace().next() {
                    paths.push(path);
                }
            } else if !line.starts_with(' ') {
                conffiles = false;
            }
        }
        let Some(name) = name.filter(|name| !name.is_empty()) else {
            continue;
        };
        for path in paths {
            let candidate = Path::new(path);
            if !candidate.is_absolute()
                || candidate
                    .components()
                    .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
            {
                continue;
            }
            let mapped = root.join(candidate.strip_prefix("/").unwrap());
            let Ok(info) = fs::symlink_metadata(&mapped) else {
                continue;
            };
            claims
                .entry(candidate.to_owned())
                .or_default()
                .insert(name.into());
            if residual {
                rows.push(LeftoverData {
                    manager: "apt".into(),
                    native_name: name.into(),
                    path: candidate.to_owned(),
                    size_bytes: info.is_file().then_some(info.len()),
                    kind: "residual configuration".into(),
                });
            }
        }
    }
    // A path claimed by more than one residual package has no single
    // authoritative owner. Leave it unknown rather than assigning it twice.
    rows.retain(|row| {
        claims
            .get(&row.path)
            .is_some_and(|owners| owners.len() == 1)
    });
    rows.dedup_by(|a, b| a.path == b.path && a.native_name == b.native_name);
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    rows
}

pub fn native_leftovers(host: &Host) -> Vec<LeftoverData> {
    let status = host.filesystem_path(Path::new("/var/lib/dpkg/status"));
    let Ok(bytes) = fs::read(status) else {
        return vec![];
    };
    if bytes.len() > 64 * 1024 * 1024 {
        return vec![];
    }
    let root = if host.runtime == Runtime::Flatpak {
        Path::new("/run/host")
    } else {
        Path::new("/")
    };
    dpkg_residuals(&String::from_utf8_lossy(&bytes), root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::UpdateAvailability;
    use std::{
        os::unix::fs::symlink,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let serial = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "pkgdeck-inspection-{}-{serial}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn package(backend: &str, name: &str, component: &str) -> Package {
        Package {
            id: PackageId {
                backend: backend.into(),
                name: name.into(),
                architecture: "amd64".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: name.into(),
            summary: String::new(),
            installed_version: Some("1".into()),
            candidate_version: None,
            update: UpdateAvailability::Current,
            icon: None,
            component_ids: vec![component.into()],
            homepages: vec![],
        }
    }
    struct Fixture(BTreeMap<PathBuf, Vec<(String, String)>>);
    impl OwnershipSource for Fixture {
        fn owners(&self, path: &Path, _: &Cancellation) -> Vec<(String, String)> {
            self.0.get(path).cloned().unwrap_or_default()
        }
    }
    fn host(path: &str) -> Host {
        Host::new(
            Runtime::Native,
            BTreeMap::from([(OsString::from("PATH"), OsString::from(path))]),
        )
    }
    #[test]
    fn path_candidates_preserve_order_links_ambiguity_and_unknowns_without_running_targets() {
        let temp = Temp::new();
        for dir in ["first", "second", "third", "fourth"] {
            fs::create_dir(temp.0.join(dir)).unwrap();
        }
        let first = temp.0.join("first/tool");
        let second = temp.0.join("second/tool");
        let third = temp.0.join("third/tool");
        let fourth = temp.0.join("fourth/tool");
        fs::write(&second, b"not run").unwrap();
        fs::set_permissions(&second, fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&second, &first).unwrap();
        symlink("missing", &third).unwrap();
        symlink("tool", &fourth).unwrap();
        let path = ["first", "second", "third", "fourth"]
            .into_iter()
            .map(|name| temp.0.join(name).display().to_string())
            .collect::<Vec<_>>()
            .join(":");
        let inventory = vec![
            package("apt", "tool-pkg", "tool"),
            package("apt", "tool-pkg", "tool"),
        ];
        let owners = Fixture(BTreeMap::from([(
            first.clone(),
            vec![("apt".into(), "tool-pkg".into())],
        )]));
        let result = inspect_with(
            &host(&path),
            "tool",
            &inventory,
            &owners,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(result.resolved, Some(first.clone()));
        assert_eq!(result.candidates.len(), 4);
        assert_eq!(result.candidates[0].target, Some(second));
        assert_eq!(result.candidates[0].owners[0].state, OwnerState::Ambiguous);
        assert_eq!(result.candidates[1].owners[0].state, OwnerState::Unknown);
        assert_eq!(result.candidates[2].state, CandidateState::BrokenLink);
        assert_eq!(result.candidates[3].state, CandidateState::LinkLoop);
        assert_eq!(
            inspect_with(
                &host(&path),
                "../tool",
                &inventory,
                &owners,
                &Cancellation::default()
            )
            .unwrap_err()
            .to_string(),
            "expected a command name without a path"
        );
        assert!(inspect_with(
            &host(&temp.0.join("second").display().to_string()),
            "tool",
            &inventory,
            &owners,
            &Cancellation::default()
        )
        .unwrap()
        .resolved
        .is_some());
    }
    #[test]
    fn symlink_and_target_ownership_retain_the_claimed_path() {
        let temp = Temp::new();
        let bin = temp.0.join("bin");
        fs::create_dir(&bin).unwrap();
        let link = bin.join("fixture");
        let target = temp.0.join("fixture-target");
        fs::write(&target, b"never run").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&target, &link).unwrap();
        let records = Fixture(BTreeMap::from([
            (link.clone(), vec![("apt".into(), "shim:amd64".into())]),
            (target.clone(), vec![("apt".into(), "real:amd64".into())]),
        ]));
        let report = inspect_with(
            &host(&bin.display().to_string()),
            "fixture",
            &[
                package("apt", "shim", "shim"),
                package("apt", "real", "real"),
            ],
            &records,
            &Cancellation::default(),
        )
        .unwrap();
        let owners = &report.candidates[0].owners;
        assert_eq!(owners.len(), 2);
        assert!(owners.iter().any(|owner| owner.path == link
            && owner.native_name == "shim:amd64"
            && owner.state == OwnerState::Known));
        assert!(owners.iter().any(|owner| owner.path == target
            && owner.native_name == "real:amd64"
            && owner.state == OwnerState::Known));
    }
    #[test]
    fn native_owner_parsers_keep_exact_architecture_and_ignore_invalid_lines() {
        assert_eq!(
            parse_owner_records(
                "apt",
                "fixture:amd64, helper:all: /usr/bin/fixture\ninvalid\n"
            ),
            vec![
                ("apt".into(), "fixture:amd64".into()),
                ("apt".into(), "helper:all".into())
            ]
        );
        assert_eq!(
            parse_owner_records("dnf", "fixture|x86_64\nmissing-arch|\ninvalid\n"),
            vec![("dnf".into(), "fixture:x86_64".into())]
        );
        assert_eq!(
            parse_owner_records("zypper", "fixture|noarch\n"),
            vec![("zypper".into(), "fixture:noarch".into())]
        );
        assert_eq!(
            parse_owner_records("pacman", "fixture\n\nhelper\n"),
            vec![
                ("pacman".into(), "fixture".into()),
                ("pacman".into(), "helper".into())
            ]
        );
        assert!(parse_owner_records("unknown", "fixture").is_empty());
    }
    #[test]
    fn native_inspection_uses_only_read_only_manager_queries_for_synthetic_paths() {
        let temp = Temp::new();
        let bin = temp.0.join("bin");
        fs::create_dir(&bin).unwrap();
        let path = bin.join("fixture");
        fs::write(&path, b"never executed").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let result = inspect_native(
            &host(&bin.display().to_string()),
            "fixture",
            &[],
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(result.resolved, Some(path));
        assert_eq!(result.candidates.len(), 1);
        assert!(result.candidates[0]
            .owners
            .iter()
            .all(|owner| owner.state != OwnerState::Known));
    }
    #[test]
    fn audit_locations_require_an_explicit_appimage_identity() {
        let mut managed = package("appimage", "pkgdeck-aaaa.AppImage", "appimage");
        managed.id.scope = Scope::Environment {
            path: "/synthetic/apps".into(),
        };
        let mut external = package("appimage", "/synthetic/external.AppImage", "external");
        external.id.scope = Scope::User { uid: 1000 };
        let mut environment = package("pip", "fixture", "pip");
        environment.id.scope = Scope::Environment {
            path: "/synthetic/venv".into(),
        };
        let report = audit(&[managed, external, environment], vec![]);
        assert_eq!(
            report
                .installed_copies
                .iter()
                .find(|copy| copy.package.name == "pkgdeck-aaaa.AppImage")
                .unwrap()
                .location
                .as_deref(),
            Some(Path::new("/synthetic/apps/pkgdeck-aaaa.AppImage"))
        );
        assert_eq!(
            report
                .installed_copies
                .iter()
                .find(|copy| copy.package.name == "/synthetic/external.AppImage")
                .unwrap()
                .location
                .as_deref(),
            Some(Path::new("/synthetic/external.AppImage"))
        );
        assert_eq!(
            report
                .installed_copies
                .iter()
                .find(|copy| copy.package.name == "fixture")
                .unwrap()
                .location,
            None
        );
    }
    #[test]
    fn audit_keeps_exact_copies_and_only_dpkg_owned_residual_paths() {
        let temp = Temp::new();
        let config = temp.0.join("etc/synthetic.conf");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, b"abc").unwrap();
        fs::write(temp.0.join("etc/unique.conf"), b"four").unwrap();
        symlink(&config, temp.0.join("etc/synthetic-link")).unwrap();
        let status = "Package: removed-pkg\nStatus: deinstall ok config-files\nConffiles:\n /etc/synthetic.conf deadbeef\n /etc/synthetic-link deadbeef\n /etc/unique.conf deadbeef\n /etc/missing deadbeef\n\nPackage: installed-pkg\nStatus: install ok installed\nConffiles:\n /etc/synthetic.conf deadbeef\n";
        let leftovers = dpkg_residuals(status, &temp.0);
        assert_eq!(leftovers.len(), 2);
        assert!(leftovers
            .iter()
            .all(|row| row.path != Path::new("/etc/synthetic.conf")));
        assert_eq!(
            leftovers
                .iter()
                .find(|row| row.path == Path::new("/etc/unique.conf"))
                .unwrap()
                .size_bytes,
            Some(4)
        );
        assert_eq!(
            leftovers
                .iter()
                .find(|row| row.path == Path::new("/etc/synthetic-link"))
                .unwrap()
                .size_bytes,
            None
        );
        let inventory = vec![
            package("apt", "player", "player"),
            package("flatpak", "org.example.Player", "player"),
        ];
        let report = audit(&inventory, leftovers);
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].copies.len(), 2);
        assert_ne!(
            report.groups[0].copies[0].package,
            report.groups[0].copies[1].package
        );
        assert_eq!(report.installed_copies.len(), 2);
    }
}
