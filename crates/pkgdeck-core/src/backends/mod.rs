//! Native APT, Linux Homebrew formula, and local AppImage adapters.
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
        if !flatpak_id(query) {
            return Err(invalid("flatpak", "search expects an application id"));
        }
        Ok(self
            .installed(cancel)?
            .into_iter()
            .filter(|p| p.id.name.contains(query))
            .collect())
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
                "flathub",
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

/// Missing optional managers are omitted from automatic queries, but explicit selections
/// and source discovery retain their unavailability. Detection failures are never hidden.
pub fn native_engine(
    source: Option<&str>,
    discover: bool,
    authorization: Authorization,
    cancel: &Cancellation,
) -> Result<Engine, EngineError> {
    if source.is_some_and(|s| s != "apt" && s != "homebrew" && s != "appimage" && s != "flatpak") {
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
