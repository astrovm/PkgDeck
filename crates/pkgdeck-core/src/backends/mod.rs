//! Native package-manager, Linux Homebrew formula, and local AppImage adapters.
mod appimage;
use crate::{
    engine::*,
    host::{AptAction, Authorization, Host},
    package::*,
    process::*,
};
pub use appimage::AppImage;
use serde::Deserialize;
use std::{ffi::OsString, path::PathBuf, time::Duration};

const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Refresh,
    Capability::Upgrade,
];

/// A narrow transport seam lets adapter tests supply synthetic native responses.
pub trait Transport: Send {
    fn apt_query(
        &self,
        mode: &str,
        query: &str,
        arch: &str,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError>;
    fn apt_write(
        &self,
        action: AptAction,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError>;
    fn brew(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError>;
    fn flatpak(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
        system: bool,
    ) -> Result<Completion, ExecutionError>;
    fn system_manager(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        let _ = (args, cancel, write);
        Err(ExecutionError::Disabled(format!("{executable} not found")))
    }
}
pub struct NativeTransport {
    pub host: Host,
    pub authorization: Authorization,
}
impl Transport for NativeTransport {
    fn apt_query(
        &self,
        mode: &str,
        query: &str,
        arch: &str,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        if self.host.resolve("apt-get")?.is_none() {
            return Err(ExecutionError::Disabled("APT not found".into()));
        }
        let executable = std::env::current_exe()
            .map_err(|e| ExecutionError::Io(e.to_string()))?
            .with_file_name("pkgdeck-apt-query");
        let result = self.host.read(
            &executable,
            &[mode.into(), query.into(), arch.into()],
            Limits {
                timeout: Duration::from_secs(120),
                output_bytes: 32 * 1024 * 1024,
            },
            cancel,
        )?;
        if result.code == Some(0) {
            Ok(result)
        } else {
            Err(ExecutionError::Failed(result))
        }
    }
    fn apt_write(
        &self,
        action: AptAction,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        self.host.apt(action, self.authorization, cancel)
    }
    fn brew(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.host.brew(args, cancel, write)
    }
    fn flatpak(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
        system: bool,
    ) -> Result<Completion, ExecutionError> {
        self.host
            .flatpak(args, cancel, write, system, self.authorization)
    }
    fn system_manager(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.host
            .system_manager(executable, args, cancel, write, self.authorization)
    }
}

pub struct Apt<T = NativeTransport>(pub T);
pub struct Homebrew<T = NativeTransport> {
    pub transport: T,
    prefix: Option<PathBuf>,
}
pub struct Flatpak<T = NativeTransport> {
    transport: T,
}
impl<T: Transport> Flatpak<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}
impl<T: Transport> Homebrew<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            prefix: None,
        }
    }
}

fn flatpak_id(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
impl<T: Transport> Flatpak<T> {
    fn call(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
        system: bool,
    ) -> Result<Completion, EngineError> {
        Ok(self.transport.flatpak(
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            cancel,
            write,
            system,
        )?)
    }
    fn list(&self, cancel: &Cancellation, system: bool) -> Result<Vec<Package>, EngineError> {
        let scope = if system {
            Scope::System
        } else {
            Scope::User {
                uid: rustix::process::getuid().as_raw(),
            }
        };
        let prefix = if system { "--system" } else { "--user" };
        let output = bytes(
            "flatpak",
            self.call(
                &[
                    prefix,
                    "list",
                    "--app",
                    "--columns=application,arch,branch,version,description",
                ],
                cancel,
                false,
                system,
            )?,
        )?;
        let text = String::from_utf8(output).map_err(|e| invalid("flatpak", e))?;
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let fields: Vec<_> = line.split('\t').collect();
                if fields.len() != 5
                    || !flatpak_id(fields[0])
                    || !flatpak_id(fields[1])
                    || !flatpak_id(fields[2])
                {
                    return Err(invalid("flatpak", "invalid list metadata"));
                }
                Ok(Package {
                    id: PackageId {
                        backend: "flatpak".into(),
                        name: fields[0].into(),
                        architecture: fields[1].into(),
                        scope: scope.clone(),
                        remote: None,
                    },
                    display_name: fields[0].into(),
                    summary: fields[4].into(),
                    installed_version: Some(fields[3].into()),
                    candidate_version: Some(fields[3].into()),
                    update: UpdateAvailability::Unknown,
                })
            })
            .collect()
    }
    fn search_scope(
        &self,
        query: &str,
        cancel: &Cancellation,
        system: bool,
    ) -> Result<Vec<Package>, EngineError> {
        let scope = if system {
            Scope::System
        } else {
            Scope::User {
                uid: rustix::process::getuid().as_raw(),
            }
        };
        let prefix = if system { "--system" } else { "--user" };
        let output = bytes(
            "flatpak",
            self.call(
                &[
                    prefix,
                    "search",
                    "--columns=name,description,application,version,branch,remotes",
                    query,
                ],
                cancel,
                false,
                system,
            )?,
        )?;
        String::from_utf8(output)
            .map_err(|e| invalid("flatpak", e))?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let fields: Vec<_> = line.split('\t').collect();
                let remote = fields
                    .get(5)
                    .and_then(|value| value.split(',').next())
                    .filter(|value| flatpak_id(value));
                if fields.len() != 6 || !flatpak_id(fields[2]) || remote.is_none() {
                    return Err(invalid("flatpak", "invalid remote search metadata"));
                }
                Ok(Package {
                    id: PackageId {
                        backend: "flatpak".into(),
                        name: fields[2].into(),
                        architecture: std::env::consts::ARCH.into(),
                        scope: scope.clone(),
                        remote: remote.map(str::to_owned),
                    },
                    display_name: fields[0].into(),
                    summary: fields[1].into(),
                    installed_version: None,
                    candidate_version: Some(fields[3].into()),
                    update: UpdateAvailability::Unknown,
                })
            })
            .collect()
    }
    fn target(&self, id: &PackageId) -> Result<(bool, &'static str), EngineError> {
        if id.backend != "flatpak" || !flatpak_id(&id.name) || !flatpak_id(&id.architecture) {
            return Err(invalid("flatpak", "foreign or invalid Flatpak identity"));
        }
        match id.scope {
            Scope::System => Ok((true, "--system")),
            Scope::User { .. } => Ok((false, "--user")),
            _ => Err(invalid(
                "flatpak",
                "Flatpak packages must be user or system scoped",
            )),
        }
    }
}
impl<T: Transport> Backend for Flatpak<T> {
    fn id(&self) -> &str {
        "flatpak"
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        match self.call(
            &["--user", "remotes", "--columns=name"],
            cancel,
            false,
            false,
        ) {
            Ok(_) => Ok(Availability::Available),
            Err(EngineError::Execution(ExecutionError::Disabled(reason))) => {
                Ok(Availability::Unavailable(reason))
            }
            Err(e) => Err(e),
        }
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        if query.trim().is_empty() || query.starts_with('-') {
            return Err(invalid("flatpak", "expected a search term"));
        }
        // The user catalog is the primary source; a failing system query
        // (for example, a machine with no system remotes) must not fail
        // the whole search when user results are available.
        let mut result = self.search_scope(query, cancel, false)?;
        if let Ok(system) = self.search_scope(query, cancel, true) {
            result.extend(system);
        }
        // Remote search results do not carry an installed scope: the user and
        // system queries return the same catalog entries. Deduplicate them so
        // a single remote application resolves unambiguously, preferring the
        // unprivileged user scope used for installs by default.
        let mut seen = std::collections::BTreeSet::new();
        result.retain(|package| {
            seen.insert((
                package.id.name.clone(),
                package.id.architecture.clone(),
                package.id.remote.clone(),
            ))
        });
        Ok(result)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let mut result = self.list(cancel, false)?;
        result.extend(self.list(cancel, true)?);
        Ok(result)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        if id.remote.is_some() {
            return self
                .search(id.name.as_str(), cancel)?
                .into_iter()
                .find(|package| package.id == *id)
                .map(|package| PackageDetails {
                    description: package.summary.clone(),
                    homepage: None,
                    dependencies: vec![],
                    package,
                })
                .ok_or(EngineError::NotFound);
        }
        let package = self
            .installed(cancel)?
            .into_iter()
            .find(|p| p.id == *id)
            .ok_or(EngineError::NotFound)?;
        Ok(PackageDetails {
            description: package.summary.clone(),
            homepage: None,
            dependencies: vec![],
            package,
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let (id, verb) = match operation {
            Operation::Refresh { backend } if backend == "flatpak" => {
                for (system, scope) in [(false, "--user"), (true, "--system")] {
                    self.call(
                        &[
                            scope,
                            "update",
                            "--app",
                            "--no-deploy",
                            "--noninteractive",
                            "--assumeyes",
                        ],
                        cancel,
                        true,
                        system,
                    )?;
                }
                return Ok(OperationOutcome::default());
            }
            Operation::Install(id) => (id, "install"),
            Operation::Remove(id) => (id, "uninstall"),
            Operation::Upgrade(id) => (id, "update"),
            Operation::UpgradeAll { backend } if backend == "flatpak" => {
                for (system, scope) in [(false, "--user"), (true, "--system")] {
                    self.call(&[scope, "update", "--noninteractive"], cancel, true, system)?;
                }
                progress(Progress::Message("Updated Flatpak packages.".into()));
                return Ok(OperationOutcome::default());
            }
            _ => return Err(invalid("flatpak", "foreign operation")),
        };
        let (system, scope) = self.target(id)?;
        progress(Progress::Message(format!(
            "Running Flatpak {verb} for {}.",
            id.name
        )));
        let args = if verb == "install" {
            vec![
                scope,
                verb,
                "--app",
                "--noninteractive",
                "--assumeyes",
                id.remote.as_deref().unwrap_or("flathub"),
                &id.name,
            ]
        } else {
            vec![
                scope,
                verb,
                "--app",
                "--noninteractive",
                "--assumeyes",
                &id.name,
            ]
        };
        let result = self.call(&args, cancel, true, system)?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}
fn invalid(backend: &str, reason: impl ToString) -> EngineError {
    EngineError::InvalidResponse {
        backend: backend.into(),
        reason: reason.to_string(),
    }
}
fn bytes(backend: &str, result: Completion) -> Result<Vec<u8>, EngineError> {
    if result.code != Some(0) {
        return Err(ExecutionError::Failed(result).into());
    }
    if result.truncated {
        return Err(invalid(backend, "metadata exceeded output limit"));
    }
    Ok(result.stdout)
}
fn availability(result: Result<Completion, ExecutionError>) -> Result<Availability, EngineError> {
    match result {
        Ok(_) => Ok(Availability::Available),
        Err(ExecutionError::Disabled(reason)) => Ok(Availability::Unavailable(reason)),
        Err(error) => Err(error.into()),
    }
}
impl<T: Transport> Apt<T> {
    fn query(
        &self,
        mode: &str,
        query: &str,
        arch: &str,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        serde_json::from_slice(&bytes("apt", self.0.apt_query(mode, query, arch, cancel)?)?)
            .map_err(|e| invalid("apt", e))
    }
    fn target(&self, id: &PackageId) -> Result<String, EngineError> {
        if id.backend != "apt" || id.scope != Scope::System {
            return Err(invalid("apt", "foreign identity or scope"));
        }
        let target = format!("{}:{}", id.name, id.architecture);
        AptAction::Install(target.clone()).arguments()?;
        Ok(target)
    }
}
impl<T: Transport> Backend for Apt<T> {
    fn id(&self) -> &str {
        "apt"
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        availability(self.0.apt_query("detect", "", "", cancel))
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .query("search", query, "", cancel)?
            .into_iter()
            .map(|d| d.package)
            .collect())
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .query("installed", "", "", cancel)?
            .into_iter()
            .map(|d| d.package)
            .collect())
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        self.target(id)?;
        self.query("details", &id.name, &id.architecture, cancel)?
            .into_iter()
            .find(|d| d.package.id == *id)
            .ok_or(EngineError::NotFound)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let action = match operation {
            Operation::Refresh { backend } if backend == "apt" => AptAction::Refresh,
            Operation::Install(id) => AptAction::Install(self.target(id)?),
            Operation::Remove(id) => AptAction::Remove(self.target(id)?),
            Operation::Upgrade(id) => AptAction::Upgrade(self.target(id)?),
            Operation::UpgradeAll { backend } if backend == "apt" => AptAction::UpgradeAll,
            _ => return Err(invalid("apt", "foreign operation")),
        };
        progress(Progress::Message(
            "Running apt-get; cancellation waits for the native transaction to finish.".into(),
        ));
        let result = self.0.apt_write(action, cancel)?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}

#[derive(Deserialize)]
struct FormulaReport {
    formulae: Vec<Formula>,
}
#[derive(Deserialize)]
struct Formula {
    full_name: String,
    desc: Option<String>,
    homepage: String,
    versions: Versions,
    revision: u64,
    installed: Vec<Installed>,
    linked_keg: Option<String>,
    outdated: bool,
    dependencies: Vec<String>,
}
#[derive(Deserialize)]
struct Versions {
    stable: Option<String>,
}
#[derive(Deserialize)]
struct Installed {
    version: String,
}
fn formula_name(name: &str) -> bool {
    let segments: Vec<_> = name.split('/').collect();
    (segments.len() == 1 || segments.len() == 3)
        && segments.iter().all(|s| {
            !s.is_empty()
                && s.starts_with(|c: char| c.is_ascii_alphanumeric())
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"@+_.-".contains(&b))
                && *s != "."
                && *s != ".."
        })
}
impl<T: Transport> Homebrew<T> {
    fn call(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, EngineError> {
        Ok(self.transport.brew(
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            cancel,
            write,
        )?)
    }
    fn parse(&self, result: Completion) -> Result<Vec<PackageDetails>, EngineError> {
        let report: FormulaReport = serde_json::from_slice(&bytes("homebrew", result)?)
            .map_err(|e| invalid("homebrew", e))?;
        let prefix = self
            .prefix
            .as_ref()
            .ok_or_else(|| invalid("homebrew", "prefix not detected"))?;
        report
            .formulae
            .into_iter()
            .map(|f| {
                if !formula_name(&f.full_name) {
                    return Err(invalid("homebrew", "invalid formula name"));
                }
                let candidate = f.versions.stable.map(|v| {
                    if f.revision == 0 {
                        v
                    } else {
                        format!("{v}_{}", f.revision)
                    }
                });
                let installed = f.linked_keg.or_else(|| {
                    f.installed
                        .iter()
                        .find(|i| Some(&i.version) == candidate.as_ref())
                        .or_else(|| f.installed.last())
                        .map(|i| i.version.clone())
                });
                Ok(PackageDetails {
                    package: Package {
                        id: PackageId {
                            backend: "homebrew".into(),
                            name: f.full_name.clone(),
                            architecture: std::env::consts::ARCH.into(),
                            scope: Scope::Environment {
                                path: prefix.clone(),
                            },
                            remote: None,
                        },
                        display_name: f.full_name,
                        summary: f.desc.clone().unwrap_or_default(),
                        update: if f.outdated {
                            UpdateAvailability::Available
                        } else if installed.is_some() {
                            UpdateAvailability::Current
                        } else {
                            UpdateAvailability::Unknown
                        },
                        installed_version: installed,
                        candidate_version: candidate,
                    },
                    description: f.desc.unwrap_or_default(),
                    homepage: (!f.homepage.is_empty()).then_some(f.homepage),
                    dependencies: f.dependencies,
                })
            })
            .collect()
    }
    fn target<'a>(&self, id: &'a PackageId) -> Result<&'a str, EngineError> {
        if id.backend != "homebrew"
            || id.architecture != std::env::consts::ARCH
            || !formula_name(&id.name)
            || self
                .prefix
                .as_ref()
                .is_none_or(|p| id.scope != (Scope::Environment { path: p.clone() }))
        {
            return Err(invalid(
                "homebrew",
                "foreign identity or invalid formula name",
            ));
        }
        Ok(&id.name)
    }
}
impl<T: Transport> Backend for Homebrew<T> {
    fn id(&self) -> &str {
        "homebrew"
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        let result = self.transport.brew(&["--prefix".into()], cancel, false);
        if let Ok(result) = &result {
            let value = String::from_utf8(bytes("homebrew", result.clone())?)
                .map_err(|e| invalid("homebrew", e))?;
            let path = PathBuf::from(value.trim());
            if !path.is_absolute() {
                return Err(invalid("homebrew", "prefix must be absolute"));
            }
            self.prefix = Some(path);
        }
        availability(result)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let data = bytes("homebrew", self.call(&["formulae"], cancel, false)?)?;
        let names = String::from_utf8(data).map_err(|e| invalid("homebrew", e))?;
        let matches: Vec<_> = names
            .lines()
            .filter(|n| n.to_lowercase().contains(&query.to_lowercase()))
            .collect();
        let mut packages = Vec::new();
        for chunk in matches.chunks(100) {
            if chunk.iter().any(|n| !formula_name(n)) {
                return Err(invalid("homebrew", "invalid formula name"));
            }
            let mut args = vec!["info", "--json=v2", "--formula", "--"];
            args.extend(chunk);
            packages.extend(
                self.parse(self.call(&args, cancel, false)?)?
                    .into_iter()
                    .map(|d| d.package),
            );
        }
        Ok(packages)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .parse(self.call(
                &["info", "--json=v2", "--formula", "--installed"],
                cancel,
                false,
            )?)?
            .into_iter()
            .map(|d| d.package)
            .collect())
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let name = self.target(id)?;
        self.parse(self.call(
            &["info", "--json=v2", "--formula", "--", name],
            cancel,
            false,
        )?)?
        .into_iter()
        .find(|d| d.package.id == *id)
        .ok_or(EngineError::NotFound)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let args = match operation {
            Operation::Refresh { backend } if backend == "homebrew" => vec!["update"],
            Operation::Install(id) => vec!["install", "--formula", "--", self.target(id)?],
            Operation::Remove(id) => {
                vec!["uninstall", "--formula", "--force", "--", self.target(id)?]
            }
            Operation::Upgrade(id) => vec!["upgrade", "--formula", "--", self.target(id)?],
            Operation::UpgradeAll { backend } if backend == "homebrew" => {
                vec!["upgrade", "--formula"]
            }
            _ => return Err(invalid("homebrew", "foreign operation")),
        };
        progress(Progress::Message(
            "Running brew as the invoking user; cancellation waits for completion.".into(),
        ));
        let result = self.call(&args, cancel, true)?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}

#[derive(Clone, Copy)]
enum ManagerKind {
    Dnf,
    Pacman,
    Zypper,
    Snap,
}

impl ManagerKind {
    fn id(self) -> &'static str {
        match self {
            Self::Dnf => "dnf",
            Self::Pacman => "pacman",
            Self::Zypper => "zypper",
            Self::Snap => "snap",
        }
    }
    fn executable(self) -> &'static str {
        self.id()
    }
    fn read_args(self, installed: bool, query: &str) -> Vec<OsString> {
        match (self, installed) {
            (Self::Dnf, true) => vec![
                "--quiet",
                "repoquery",
                "--latest-limit",
                "1",
                "--installed",
                "--queryformat",
                "%{name}|%{arch}|%{version}-%{release}|%{summary}\n",
            ],
            (Self::Dnf, false) => vec![
                "--quiet",
                "repoquery",
                "--latest-limit",
                "1",
                "--queryformat",
                "%{name}|%{arch}|%{version}-%{release}|%{summary}\n",
                query,
            ],
            (Self::Pacman, true) => vec!["-Q"],
            (Self::Pacman, false) => vec!["-Ss", query],
            (Self::Zypper, true) => vec![
                "--xmlout",
                "search",
                "--installed-only",
                "--details",
                "--type",
                "package",
            ],
            (Self::Zypper, false) => vec![
                "--xmlout",
                "search",
                "--details",
                "--type",
                "package",
                query,
            ],
            (Self::Snap, true) => vec!["list"],
            (Self::Snap, false) => vec!["find", query],
        }
        .into_iter()
        .map(OsString::from)
        .collect()
    }
    fn write_args(self, operation: &Operation) -> Vec<&'static str> {
        match (self, operation) {
            (Self::Dnf, Operation::Refresh { .. }) => vec!["-y", "makecache"],
            (Self::Dnf, Operation::Install(_)) => vec!["-y", "install", "--"],
            (Self::Dnf, Operation::Remove(_)) => vec!["-y", "remove", "--"],
            (Self::Dnf, Operation::Upgrade(_)) => vec!["-y", "upgrade", "--"],
            (Self::Dnf, Operation::UpgradeAll { .. }) => vec!["-y", "upgrade"],
            (Self::Pacman, Operation::Refresh { .. }) => vec!["-Sy", "--noconfirm"],
            (Self::Pacman, Operation::Install(_)) => {
                vec!["-S", "--noconfirm", "--needed", "--"]
            }
            (Self::Pacman, Operation::Remove(_)) => vec!["-Rns", "--noconfirm", "--"],
            (Self::Pacman, Operation::Upgrade(_)) => {
                vec!["-S", "--noconfirm", "--needed", "--"]
            }
            (Self::Pacman, Operation::UpgradeAll { .. }) => vec!["-Su", "--noconfirm"],
            (Self::Zypper, Operation::Refresh { .. }) => vec!["--non-interactive", "refresh"],
            (Self::Zypper, Operation::Install(_)) => vec![
                "--non-interactive",
                "install",
                "--auto-agree-with-licenses",
                "--",
            ],
            (Self::Zypper, Operation::Remove(_)) => vec!["--non-interactive", "remove", "--"],
            (Self::Zypper, Operation::Upgrade(_)) => vec!["--non-interactive", "update", "--"],
            (Self::Zypper, Operation::UpgradeAll { .. }) => vec!["--non-interactive", "update"],
            (Self::Snap, Operation::Refresh { .. }) => vec!["refresh"],
            (Self::Snap, Operation::Install(_)) => vec!["install"],
            (Self::Snap, Operation::Remove(_)) => vec!["remove"],
            (Self::Snap, Operation::Upgrade(_)) => vec!["refresh"],
            (Self::Snap, Operation::UpgradeAll { .. }) => vec!["refresh"],
        }
    }
}

pub struct SystemManager<T = NativeTransport> {
    kind: ManagerKind,
    transport: T,
}
pub type Dnf<T = NativeTransport> = SystemManager<T>;
pub type Pacman<T = NativeTransport> = SystemManager<T>;
pub type Zypper<T = NativeTransport> = SystemManager<T>;
pub type Snap<T = NativeTransport> = SystemManager<T>;
impl<T: Transport> SystemManager<T> {
    fn new(kind: ManagerKind, transport: T) -> Self {
        Self { kind, transport }
    }
    pub fn dnf(transport: T) -> Self {
        Self::new(ManagerKind::Dnf, transport)
    }
    pub fn pacman(transport: T) -> Self {
        Self::new(ManagerKind::Pacman, transport)
    }
    pub fn zypper(transport: T) -> Self {
        Self::new(ManagerKind::Zypper, transport)
    }
    pub fn snap(transport: T) -> Self {
        Self::new(ManagerKind::Snap, transport)
    }
    fn call(
        &self,
        args: Vec<OsString>,
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, EngineError> {
        Ok(self
            .transport
            .system_manager(self.kind.executable(), &args, cancel, write)?)
    }
    fn valid_name(&self, name: &str) -> bool {
        !name.is_empty()
            && !name.starts_with('-')
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+@-".contains(&b))
    }
    fn package(
        &self,
        name: &str,
        arch: &str,
        version: &str,
        summary: &str,
    ) -> Result<Package, EngineError> {
        if !self.valid_name(name) || arch.is_empty() || version.is_empty() {
            return Err(invalid(self.kind.id(), "invalid package metadata"));
        }
        Ok(Package {
            id: PackageId {
                backend: self.kind.id().into(),
                name: name.into(),
                architecture: arch.into(),
                scope: Scope::System,
                remote: None,
            },
            display_name: name.into(),
            summary: summary.into(),
            installed_version: None,
            candidate_version: Some(version.into()),
            update: UpdateAvailability::Unknown,
        })
    }
    fn parse(&self, value: Vec<u8>, installed: bool) -> Result<Vec<Package>, EngineError> {
        let text = String::from_utf8(value).map_err(|e| invalid(self.kind.id(), e))?;
        let mut packages = Vec::new();
        match self.kind {
            ManagerKind::Dnf => {
                for line in text.lines().filter(|line| !line.is_empty()) {
                    let fields: Vec<_> = line.splitn(4, '|').collect();
                    if fields.len() != 4 {
                        return Err(invalid("dnf", "invalid repoquery metadata"));
                    }
                    let mut package = self.package(fields[0], fields[1], fields[2], fields[3])?;
                    if installed {
                        package.installed_version = Some(fields[2].into());
                        package.update = UpdateAvailability::Current;
                    }
                    packages.push(package);
                }
            }
            ManagerKind::Pacman => {
                let mut lines = text.lines().peekable();
                while let Some(line) = lines.next() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let header = line.split_whitespace().collect::<Vec<_>>();
                    let (name, version) = if installed {
                        (header.first().copied(), header.get(1).copied())
                    } else {
                        (
                            header
                                .first()
                                .and_then(|n| n.split_once('/').map(|(_, n)| n)),
                            header.get(1).copied(),
                        )
                    };
                    let (Some(name), Some(version)) = (name, version) else {
                        return Err(invalid("pacman", "invalid package metadata"));
                    };
                    let summary = if installed {
                        "Installed package"
                    } else {
                        lines.next().map(str::trim).unwrap_or("")
                    };
                    let mut package =
                        self.package(name, std::env::consts::ARCH, version, summary)?;
                    if installed {
                        package.installed_version = Some(version.into());
                        package.update = UpdateAvailability::Current;
                    }
                    packages.push(package);
                }
            }
            ManagerKind::Zypper => {
                for entry in text.split("<solvable ").skip(1) {
                    let attr = |key: &str| {
                        entry
                            .split_once(&format!("{key}=\""))
                            .and_then(|(_, tail)| tail.split_once('"'))
                            .map(|(value, _)| value)
                    };
                    let (Some(name), Some(version), Some(arch)) =
                        (attr("name"), attr("edition"), attr("arch"))
                    else {
                        return Err(invalid("zypper", "invalid XML metadata"));
                    };
                    let mut package =
                        self.package(name, arch, version, attr("summary").unwrap_or(""))?;
                    if installed {
                        package.installed_version = Some(version.into());
                        package.update = UpdateAvailability::Current;
                    }
                    packages.push(package);
                }
            }
            ManagerKind::Snap => {
                for line in text.lines().skip(1).filter(|line| !line.trim().is_empty()) {
                    let fields = line.split_whitespace().collect::<Vec<_>>();
                    if fields.len() < 2 {
                        return Err(invalid("snap", "invalid list metadata"));
                    }
                    let mut package =
                        self.package(fields[0], std::env::consts::ARCH, fields[1], "Snap package")?;
                    if installed {
                        package.installed_version = Some(fields[1].into());
                        package.update = UpdateAvailability::Current;
                    }
                    packages.push(package);
                }
            }
        }
        Ok(packages)
    }
    fn query(
        &self,
        installed: bool,
        query: &str,
        cancel: &Cancellation,
    ) -> Result<Vec<Package>, EngineError> {
        if !installed && !self.valid_name(query) {
            return Err(invalid(self.kind.id(), "invalid package query"));
        }
        let args = self.kind.read_args(installed, query);
        self.parse(
            bytes(self.kind.id(), self.call(args, cancel, false)?)?,
            installed,
        )
    }
    fn target(&self, id: &PackageId) -> Result<String, EngineError> {
        if id.backend != self.kind.id() || id.scope != Scope::System || !self.valid_name(&id.name) {
            return Err(invalid(
                self.kind.id(),
                "foreign or invalid package identity",
            ));
        }
        Ok(id.name.clone())
    }
}
impl<T: Transport> Backend for SystemManager<T> {
    fn id(&self) -> &str {
        self.kind.id()
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        match self.query(true, "", cancel) {
            Ok(_) => Ok(Availability::Available),
            Err(EngineError::Execution(ExecutionError::Disabled(reason))) => {
                Ok(Availability::Unavailable(reason))
            }
            Err(error) => Err(error),
        }
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.query(false, query, cancel)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.query(true, "", cancel)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let name = self.target(id)?;
        let package = self
            .query(false, &name, cancel)?
            .into_iter()
            .find(|package| package.id == *id)
            .ok_or(EngineError::NotFound)?;
        Ok(PackageDetails {
            description: package.summary.clone(),
            homepage: None,
            dependencies: vec![],
            package,
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        if operation.backend() != self.kind.id() {
            return Err(invalid(self.kind.id(), "foreign operation"));
        }
        let name = match operation {
            Operation::Refresh { .. } => None,
            Operation::Install(id) | Operation::Remove(id) | Operation::Upgrade(id) => {
                Some(self.target(id)?)
            }
            Operation::UpgradeAll { .. } => None,
        };
        let mut args: Vec<OsString> = self
            .kind
            .write_args(operation)
            .into_iter()
            .map(OsString::from)
            .collect();
        if let Some(name) = name {
            args.push(name.into());
        }
        progress(Progress::Message(format!(
            "Running {} with non-interactive authorization; cancellation waits for completion.",
            self.kind.id()
        )));
        let result = self.call(args, cancel, true)?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}

/// Missing optional managers are omitted from automatic queries, but explicit selections
/// and source discovery retain their unavailability. Detection failures are never hidden.
pub fn native_engine(
    source: Option<&str>,
    discover: bool,
    authorization: Authorization,
    cancel: &Cancellation,
) -> Result<Engine, EngineError> {
    if source.is_some_and(|s| {
        ![
            "apt", "dnf", "pacman", "zypper", "snap", "homebrew", "appimage", "flatpak",
        ]
        .contains(&s)
    }) {
        return Err(EngineError::UnknownBackend(source.unwrap().into()));
    }
    let mut engine = Engine::default();
    let host = Host::current();
    if !discover && source.is_none() {
        if let Some(reason) = host.runtime.disabled_reason() {
            return Err(ExecutionError::Disabled(reason.into()).into());
        }
    }
    let mut apt = Apt(NativeTransport {
        host: host.clone(),
        authorization,
    });
    let mut brew = Homebrew::new(NativeTransport {
        host,
        authorization,
    });
    if source.is_none_or(|s| s == "apt")
        && (discover
            || source.is_some()
            || !matches!(apt.detect(cancel), Ok(Availability::Unavailable(_))))
    {
        engine.register(apt)?;
    }
    if source.is_none_or(|s| s == "homebrew")
        && (discover
            || source.is_some()
            || !matches!(brew.detect(cancel), Ok(Availability::Unavailable(_))))
    {
        engine.register(brew)?;
    }
    for backend in ["dnf", "pacman", "zypper", "snap"] {
        if source.is_none_or(|source| source == backend) {
            let transport = NativeTransport {
                host: Host::current(),
                authorization,
            };
            let mut manager = match backend {
                "dnf" => SystemManager::dnf(transport),
                "pacman" => SystemManager::pacman(transport),
                "zypper" => SystemManager::zypper(transport),
                "snap" => SystemManager::snap(transport),
                _ => unreachable!(),
            };
            if discover
                || source.is_some()
                || !matches!(manager.detect(cancel), Ok(Availability::Unavailable(_)))
            {
                engine.register(manager)?;
            }
        }
    }
    if source.is_none_or(|s| s == "appimage") {
        engine.register(AppImage::native())?;
    }
    if source.is_none_or(|s| s == "flatpak") {
        engine.register(Flatpak::new(NativeTransport {
            host: Host::current(),
            authorization,
        }))?;
    }
    Ok(engine)
}
