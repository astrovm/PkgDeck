//! Native package-manager, container image, Homebrew formula/cask, and local AppImage adapters.
mod appimage;
mod cleanup;
mod container;
mod firmware;
mod standalone;
use crate::{
    engine::*,
    host::{AptAction, Authorization, Host},
    package::*,
    process::*,
};
pub use appimage::AppImage;
pub use container::{Container, ContainerKind};
pub use firmware::Firmware;
use serde::Deserialize;
pub use standalone::{Standalone, StandaloneTool};
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
const CLEAN_CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Refresh,
    Capability::Upgrade,
    Capability::Clean,
];

/// Backend ids accepted by `--from` and the source checklist. Adding a
/// backend means extending this list, the GUI `sourceIds`, and the CLI
/// value parser together.
pub const BACKEND_IDS: &[&str] = &[
    "fwupd",
    "apt",
    "dnf",
    "pacman",
    "zypper",
    "snap",
    "homebrew",
    "homebrew-cask",
    "appimage",
    "flatpak",
    "docker",
    "podman",
    "cargo",
    "npm",
    "pnpm",
    "bun",
    "pip",
    "pipx",
    "uv",
    "composer",
    "gem",
    "codex",
    "claude",
    "grok",
    "opencode",
];

/// These sources update existing installations but do not install or remove them.
pub fn update_only(id: &str) -> bool {
    matches!(id, "fwupd" | "codex" | "claude" | "grok" | "opencode")
}

/// A narrow transport seam lets adapter tests supply synthetic native responses.
pub trait Transport: Send {
    fn repository_editor(&self) -> Result<(), ExecutionError> {
        Err(ExecutionError::Disabled(
            "Software Sources editor unavailable".into(),
        ))
    }
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
    /// Read-only libflatpak inventory; never runs an uninstall as a preview.
    fn flatpak_unused(&self, _cancel: &Cancellation) -> Result<Completion, ExecutionError> {
        Err(ExecutionError::Disabled(
            "Flatpak cleanup requires Python GObject and the Flatpak typelib".into(),
        ))
    }
    /// Read one sanitized host environment value, if the fixture provides it.
    fn env(&self, _name: &str) -> Option<OsString> {
        None
    }
    /// Run a user-scoped development tool (`cargo`, `npm`, `pnpm`, `bun`,
    /// `pipx`, `uv`, `composer`, `gem`). Fixtures override this one seam for
    /// every development manager except venv-pinned `pip` (see `venv_pip`).
    fn dev_tool(
        &self,
        executable: &str,
        _args: &[OsString],
        _cancel: &Cancellation,
        _write: bool,
    ) -> Result<Completion, ExecutionError> {
        Err(ExecutionError::Disabled(format!("{executable} not found")))
    }
    /// Run `<venv>/bin/python -m pip` for an explicitly selected virtual
    /// environment. Fixtures override this seam for `pip` tests.
    fn venv_pip(
        &self,
        venv: &std::path::Path,
        _args: &[OsString],
        _cancel: &Cancellation,
        _write: bool,
    ) -> Result<Completion, ExecutionError> {
        let _ = venv;
        Err(ExecutionError::Disabled("pip not found".into()))
    }
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
    fn container(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.dev_tool(executable, args, cancel, write)
    }
}
pub struct NativeTransport {
    pub host: Host,
    pub authorization: Authorization,
}

fn apt_query_executable(
    executable: &std::path::Path,
    built: Option<&str>,
) -> Result<PathBuf, ExecutionError> {
    let adjacent = executable.with_file_name("pkgdeck-apt-query");
    if adjacent.is_file() {
        return Ok(adjacent);
    }
    if let Some(path) = built.map(PathBuf::from).filter(|path| path.is_file()) {
        return Ok(path);
    }
    Err(ExecutionError::Disabled(
        "APT helper is missing. Rebuild with libapt-pkg-dev installed, or reinstall PkgDeck."
            .into(),
    ))
}

impl Transport for NativeTransport {
    fn repository_editor(&self) -> Result<(), ExecutionError> {
        self.host.open_source_editor()
    }
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
        if self.host.runtime == crate::host::Runtime::Flatpak {
            let result = self.host.read(
                std::path::Path::new("/usr/bin/python3"),
                &[
                    "-c".into(),
                    include_str!("../apt-query.py").into(),
                    mode.into(),
                    query.into(),
                    arch.into(),
                ],
                Limits {
                    timeout: Duration::from_secs(120),
                    output_bytes: 32 * 1024 * 1024,
                },
                cancel,
            )?;
            return if result.code == Some(0) {
                Ok(result)
            } else {
                Err(ExecutionError::Failed(result))
            };
        }
        let executable = apt_query_executable(
            &std::env::current_exe().map_err(|e| ExecutionError::Io(e.to_string()))?,
            option_env!("PKGDECK_BUILT_APT_QUERY"),
        )?;
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
    fn flatpak_unused(&self, cancel: &Cancellation) -> Result<Completion, ExecutionError> {
        self.host.flatpak_unused(cancel)
    }
    fn env(&self, name: &str) -> Option<OsString> {
        self.host.var(name)
    }
    fn dev_tool(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        let label = match executable {
            "cargo" => "Cargo",
            "npm" => "npm",
            "pnpm" => "pnpm",
            "bun" => "Bun",
            "pipx" => "pipx",
            "uv" => "uv",
            "composer" => "Composer",
            "gem" => "RubyGems",
            other => other,
        };
        self.host.dev_tool(executable, label, args, cancel, write)
    }
    fn venv_pip(
        &self,
        venv: &std::path::Path,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.host.venv_pip(venv, args, cancel, write)
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
    fn container(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        let label = if executable == "docker" {
            "Docker"
        } else {
            "Podman"
        };
        self.host
            .container_engine(executable, label, args, cancel, write)
    }
}

pub struct Apt<T = NativeTransport> {
    pub transport: T,
    desktop_entries: Option<std::collections::BTreeMap<String, PathBuf>>,
    components: Option<std::collections::BTreeMap<String, Vec<String>>>,
}
impl<T> Apt<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            desktop_entries: None,
            components: None,
        }
    }
}
pub struct Homebrew<T = NativeTransport> {
    pub transport: T,
    prefix: Option<PathBuf>,
}
pub struct HomebrewCask<T = NativeTransport> {
    pub transport: T,
    prefix: Option<PathBuf>,
}
pub struct Flatpak<T = NativeTransport> {
    transport: T,
}
fn flatpak_offer_reference(reference: &str) -> Option<(&str, &str, &str)> {
    let mut parts = reference.split('/');
    let (name, arch, branch) = (parts.next()?, parts.next()?, parts.next()?);
    (flatpak_id(name) && flatpak_id(arch) && flatpak_id(branch) && parts.next().is_none())
        .then_some((name, arch, branch))
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
impl<T: Transport> HomebrewCask<T> {
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
fn flatpak_reference(reference: &str) -> Option<(&str, &str, &str, &str)> {
    let mut parts = reference.split('/');
    let (kind, name, arch, branch) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    (matches!(kind, "app" | "runtime")
        && flatpak_id(name)
        && flatpak_id(arch)
        && flatpak_id(branch)
        && parts.next().is_none())
    .then_some((kind, name, arch, branch))
}
impl<T: Transport> Flatpak<T> {
    fn call(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
        system: bool,
    ) -> Result<Completion, EngineError> {
        let result = self.transport.flatpak(
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            cancel,
            write,
            system,
        )?;
        if write {
            bytes("flatpak", result.clone())?;
        }
        Ok(result)
    }
    fn list(
        &self,
        cancel: &Cancellation,
        system: bool,
        updates: bool,
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
                    "list",
                    "--columns=application,arch,branch,version,description,origin,options",
                ],
                cancel,
                false,
                system,
            )?,
        )?;
        let text = String::from_utf8(output).map_err(|e| invalid("flatpak", e))?;
        let mut packages = Vec::new();
        let mut origins = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let fields: Vec<_> = line.split('\t').collect();
            if !(6..=7).contains(&fields.len())
                || !flatpak_id(fields[0])
                || !flatpak_id(fields[1])
                || !flatpak_id(fields[2])
                || (!fields[5].is_empty() && !flatpak_id(fields[5]))
            {
                return Err(invalid("flatpak", "invalid list metadata"));
            }
            let runtime = fields
                .get(6)
                .is_some_and(|options| options.split(',').any(|option| option.trim() == "runtime"));
            let kind = if runtime { "runtime" } else { "app" };
            let reference = format!("{kind}/{}/{}/{}", fields[0], fields[1], fields[2]);
            origins.push(fields[5].to_owned());
            packages.push(Package {
                id: PackageId {
                    backend: "flatpak".into(),
                    name: fields[0].into(),
                    architecture: fields[1].into(),
                    scope: scope.clone(),
                    remote: None,
                    reference: Some(reference),
                },
                display_name: fields[0].into(),
                summary: if runtime {
                    format!("Runtime · {} · {}", fields[2], fields[4])
                } else {
                    fields[4].into()
                },
                installed_version: Some(fields[3].into()),
                candidate_version: Some(fields[3].into()),
                update: UpdateAvailability::Current,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
            });
        }
        if packages.is_empty() || !updates {
            return Ok(packages);
        }
        // Flatpak compares commits, not version labels: rebuilds and runtimes
        // can have an update even when the version is unchanged or empty.
        let output = bytes(
            "flatpak",
            self.call(
                &[
                    prefix,
                    "remote-ls",
                    "--updates",
                    "--columns=ref,version,origin",
                ],
                cancel,
                false,
                system,
            )?,
        )?;
        let updates = String::from_utf8(output).map_err(|e| invalid("flatpak", e))?;
        for line in updates.lines().filter(|line| !line.trim().is_empty()) {
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 3 || flatpak_reference(fields[0]).is_none() || !flatpak_id(fields[2])
            {
                return Err(invalid("flatpak", "invalid update metadata"));
            }
            for (package, origin) in packages.iter_mut().zip(&origins) {
                if package.id.reference.as_deref() == Some(fields[0]) && origin == fields[2] {
                    package.candidate_version = Some(fields[1].into());
                    package.update = UpdateAvailability::Available;
                }
            }
        }
        Ok(packages)
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
                if fields.len() != 6
                    || !flatpak_id(fields[2])
                    || !flatpak_id(fields[4])
                    || remote.is_none()
                {
                    return Err(invalid("flatpak", "invalid remote search metadata"));
                }
                Ok(Package {
                    id: PackageId {
                        backend: "flatpak".into(),
                        name: fields[2].into(),
                        architecture: std::env::consts::ARCH.into(),
                        scope: scope.clone(),
                        remote: remote.map(str::to_owned),
                        reference: Some(format!(
                            "{}/{}/{}",
                            fields[2],
                            std::env::consts::ARCH,
                            fields[4]
                        )),
                    },
                    display_name: fields[0].into(),
                    summary: fields[1].into(),
                    installed_version: None,
                    candidate_version: Some(fields[3].into()),
                    update: UpdateAvailability::Unknown,
                    icon: None,
                    component_ids: vec![],
                    homepages: vec![],
                })
            })
            .collect()
    }
    fn target(&self, id: &PackageId) -> Result<(bool, &'static str), EngineError> {
        if id.backend != "flatpak" || !flatpak_id(&id.name) || !flatpak_id(&id.architecture) {
            return Err(invalid("flatpak", "foreign or invalid Flatpak identity"));
        }
        if let Some(reference) = &id.reference {
            let (name, arch, _) = flatpak_reference(reference)
                .map(|(_, name, arch, branch)| (name, arch, branch))
                .or_else(|| flatpak_offer_reference(reference))
                .ok_or_else(|| invalid("flatpak", "invalid native ref"))?;
            if name != id.name || arch != id.architecture {
                return Err(invalid(
                    "flatpak",
                    "native ref does not match package identity",
                ));
            }
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
        CLEAN_CAPABILITIES
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
        if flatpak_reference(query).is_some() {
            return self.installed(cancel).map(|packages| {
                packages
                    .into_iter()
                    .filter(|package| package.id.reference.as_deref() == Some(query))
                    .collect()
            });
        }
        // The user catalog is the primary source; a failing system query
        // (for example, a machine with no system remotes) must not fail
        // the whole search when user results are available.
        let mut result = self.search_scope(query, cancel, false)?;
        if let Ok(system) = self.search_scope(query, cancel, true) {
            result.extend(system);
        }
        // Offers in separate installations remain independently selectable.
        let mut seen = std::collections::BTreeSet::new();
        result.retain(|package| seen.insert(package.id.clone()));
        // Local inventory is enough to choose Install versus Remove. Search
        // must not fetch remote update metadata just to establish this state.
        let mut installed = self.list(cancel, false, false)?;
        installed.extend(self.list(cancel, true, false)?);
        let mut offers = Vec::new();
        for offer in result {
            let matches: Vec<_> =
                installed
                    .iter()
                    .filter(|package| {
                        package.id.scope == offer.id.scope
                            && package.id.reference.as_deref().and_then(|reference| {
                                reference.split_once('/').map(|(_, rest)| rest)
                            }) == offer.id.reference.as_deref()
                    })
                    .collect();
            if matches.is_empty() {
                offers.push(offer);
            } else {
                for package in matches {
                    let mut row = offer.clone();
                    row.id = package.id.clone();
                    row.installed_version = package.installed_version.clone();
                    offers.push(row);
                }
            }
        }
        offers.dedup_by(|a, b| a.id == b.id);
        Ok(offers)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let home = self.transport.env("HOME").map(PathBuf::from);
        let mut result = self.list(cancel, false, true)?;
        result.extend(self.list(cancel, true, true)?);
        for package in &mut result {
            // Exported icons double as the installed check per scope.
            let roots: Vec<PathBuf> = match &package.id.scope {
                Scope::User { .. } => home
                    .as_ref()
                    .map(|home| home.join(".local/share/flatpak/exports"))
                    .into_iter()
                    .collect(),
                Scope::System => vec![PathBuf::from("/var/lib/flatpak/exports")],
                _ => vec![],
            };
            if !roots.is_empty() {
                package.icon = flatpak_icon(&roots, &package.id.name);
            }
            // The Flatpak application id is itself the AppStream component id.
            if package
                .id
                .reference
                .as_deref()
                .is_some_and(|reference| reference.starts_with("app/"))
            {
                package.component_ids = vec![package.id.name.clone()];
            }
        }
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
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        self.cleanup_unused(cancel)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        if let Operation::Clean(id) = operation {
            return self.clean_unused(id, cancel);
        }
        let (id, verb) = match operation {
            Operation::Refresh { backend } if backend == "flatpak" => {
                for (system, scope) in [(false, "--user"), (true, "--system")] {
                    self.call(
                        &[
                            scope,
                            "update",
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
        let reference = id.reference.as_deref().unwrap_or(&id.name);
        let mut args = vec![scope, verb, "--noninteractive", "--assumeyes"];
        if reference.starts_with("runtime/") {
            args.push("--runtime");
        } else if reference.starts_with("app/") || id.reference.is_none() {
            args.push("--app");
        }
        // Search metadata does not identify app versus runtime. Preserve its
        // name/architecture/branch and let Flatpak resolve the kind.
        if verb == "install" {
            args.push(id.remote.as_deref().unwrap_or("flathub"));
        }
        args.push(reference);
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

/// Local application icons, resolved without network access for installed
/// packages only. Remote catalog entries never carry icons: fetching them
/// would add a download cache with eviction semantics on top of every
/// backend. Names that cannot resolve keep the generic source icon.
fn icon_file(dir: &std::path::Path, stem: &str) -> Option<PathBuf> {
    for extension in ["png", "svg", "xpm"] {
        let candidate = dir.join(format!("{stem}.{extension}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Installed snaps expose their icon beside the desktop entry. `sysroot`
/// is `/` on a real system and a fixture directory in tests.
fn snap_icon(sysroot: &std::path::Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return None;
    }
    icon_file(
        &sysroot.join("snap").join(name).join("current/meta/gui"),
        "icon",
    )
}

/// Installed Flatpak applications export icons per installation scope.
/// `export_roots` holds the `exports` directories, user scope first.
fn flatpak_icon(export_roots: &[PathBuf], app_id: &str) -> Option<PathBuf> {
    if !flatpak_id(app_id) {
        return None;
    }
    for root in export_roots {
        for size in ["128x128", "64x64", "48x48", "32x32"] {
            let dir = root.join("share/icons/hicolor").join(size).join("apps");
            if let Some(icon) = icon_file(&dir, app_id) {
                return Some(icon);
            }
        }
    }
    None
}

/// Map installed Debian packages to their first desktop entry by scanning
/// the dpkg file lists once per query. File names are `<package>.list` or
/// `<package>:<arch>.list`; only GUI applications resolve.
fn apt_desktop_map(info_dir: &std::path::Path) -> std::collections::BTreeMap<String, PathBuf> {
    let mut map = std::collections::BTreeMap::new();
    let entries = std::fs::read_dir(info_dir).into_iter().flatten().flatten();
    for entry in entries {
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = file.strip_suffix(".list") else {
            continue;
        };
        let package = stem.split_once(':').map_or(stem, |(name, _)| name);
        if package.is_empty() || map.contains_key(package) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        if let Some(desktop) = content.lines().find(|line| {
            line.strip_prefix("/usr/share/applications/")
                .is_some_and(|path| path.ends_with(".desktop"))
        }) {
            map.insert(package.to_owned(), PathBuf::from(desktop));
        }
    }
    map
}

/// Resolve a desktop entry's `Icon` value: absolute paths are used directly
/// while bare names search the icon theme. `home` selects the user theme
/// directory and is `None` when the sanitized environment hides it.
fn desktop_icon(home: Option<&std::path::Path>, desktop: &std::path::Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(host_metadata_path(desktop)).ok()?;
    let name = content
        .lines()
        .find_map(|line| line.strip_prefix("Icon="))?
        .trim();
    themed_icon(home, name)
}

fn host_metadata_path(path: &std::path::Path) -> PathBuf {
    static HOST: std::sync::OnceLock<Host> = std::sync::OnceLock::new();
    HOST.get_or_init(Host::current).filesystem_path(path)
}

/// Resolve an exact desktop/AppStream icon name without network requests.
pub fn themed_icon(home: Option<&std::path::Path>, name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    let path = PathBuf::from(name);
    if path.is_absolute() {
        let path = host_metadata_path(&path);
        return path.is_file().then_some(path);
    }
    if name.contains('/') {
        return None;
    }
    let mut dirs = vec![
        host_metadata_path(std::path::Path::new("/usr/local/share/icons")),
        host_metadata_path(std::path::Path::new("/usr/share/icons")),
    ];
    if let Some(home) = home {
        dirs.insert(0, home.join(".local/share/icons"));
    }
    for dir in &dirs {
        for size in ["128x128", "64x64", "48x48", "32x32", "scalable"] {
            if let Some(icon) = icon_file(&dir.join("hicolor").join(size).join("apps"), name) {
                return Some(icon);
            }
        }
    }
    icon_file(&host_metadata_path(std::path::Path::new("/usr/share/pixmaps")), name)
}

/// Strip one trailing `.desktop` suffix so DEP-11 component ids
/// (`org.mozilla.firefox.desktop`), Flatpak app ids (`org.mozilla.firefox`),
/// and snap desktop basenames (`firefox.desktop`) share one namespace.
/// Empty stems are `None` so they never join unrelated rows.
fn component_stem(id: &str) -> Option<String> {
    let stem = id.strip_suffix(".desktop").unwrap_or(id).trim();
    (!stem.is_empty()).then(|| stem.to_owned())
}

/// Map installed Debian packages to their AppStream component-id stems by
/// scanning DEP-11 YAML (`<id>.yml.gz`) as a gzip line stream. Documents are
/// `---`-separated with top-level `ID:`/`Package:` scalars; a package can
/// own several components. Missing or unreadable data maps nothing.
fn dep11_component_ids(
    yaml_dir: &std::path::Path,
) -> std::collections::BTreeMap<String, Vec<String>> {
    use std::io::{BufRead, BufReader};
    let mut map: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(yaml_dir) else {
        return map;
    };
    for entry in entries.flatten() {
        if entry.path().extension().is_none_or(|ext| ext != "gz") {
            continue;
        }
        let Ok(file) = std::fs::File::open(entry.path()) else {
            continue;
        };
        let decoder = BufReader::new(flate2::read::GzDecoder::new(file));
        let mut id: Option<String> = None;
        let mut package: Option<String> = None;
        let mut flush = |id: &mut Option<String>, package: &mut Option<String>| {
            if let (Some(id), Some(package)) = (id.take(), package.take()) {
                if let Some(stem) = component_stem(&id) {
                    map.entry(package).or_default().push(stem);
                }
            }
        };
        for line in decoder.lines() {
            let Ok(line) = line else {
                break;
            };
            if line == "---" {
                flush(&mut id, &mut package);
            } else if let Some(value) = line.strip_prefix("ID: ") {
                id = Some(value.trim().to_owned());
            } else if let Some(value) = line.strip_prefix("Package: ") {
                package = Some(value.trim().to_owned());
            }
        }
        flush(&mut id, &mut package);
    }
    for ids in map.values_mut() {
        ids.sort();
        ids.dedup();
    }
    map
}

/// Desktop-id stems shipped by an installed snap (`meta/gui/*.desktop`).
/// CLI-only snaps expose no desktop entry and group with nothing.
fn snap_desktop_ids(sysroot: &std::path::Path, name: &str) -> Vec<String> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return vec![];
    }
    let gui = sysroot.join("snap").join(name).join("current/meta/gui");
    let Ok(entries) = std::fs::read_dir(&gui) else {
        return vec![];
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "desktop"))
        .filter_map(|entry| component_stem(&entry.file_name().to_string_lossy()))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}
impl<T: Transport> Apt<T> {
    fn query(
        &self,
        mode: &str,
        query: &str,
        arch: &str,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        serde_json::from_slice(&bytes(
            "apt",
            self.transport.apt_query(mode, query, arch, cancel)?,
        )?)
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
    /// Installed packages mapped to their desktop entries, scanned once per
    /// backend lifetime. Backend instances are rebuilt for every load, so the
    /// cache never outlives the query that populated it; repeated selections
    /// reuse it instead of re-reading every dpkg file list.
    fn desktop_entries(&mut self) -> &std::collections::BTreeMap<String, PathBuf> {
        self.desktop_entries
            .get_or_insert_with(|| apt_desktop_map(std::path::Path::new("/var/lib/dpkg/info")))
    }
    /// Installed packages mapped to their AppStream component-id stems,
    /// scanned once per backend lifetime like the desktop entries above.
    fn component_map(&mut self) -> &std::collections::BTreeMap<String, Vec<String>> {
        self.components.get_or_insert_with(|| {
            dep11_component_ids(std::path::Path::new("/var/lib/app-info/yaml"))
        })
    }
}
impl<T: Transport> Backend for Apt<T> {
    fn id(&self) -> &str {
        "apt"
    }
    fn capabilities(&self) -> &[Capability] {
        CLEAN_CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        availability(self.transport.apt_query("detect", "", "", cancel))
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .query("search", query, "", cancel)?
            .into_iter()
            .map(|mut d| {
                let home = self.transport.env("HOME").map(PathBuf::from);
                if let Some(desktop) = self.desktop_entries().get(&d.package.id.name).cloned() {
                    d.package.icon = desktop_icon(home.as_deref(), &desktop);
                }
                d.package
            })
            .collect())
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let home = self.transport.env("HOME").map(PathBuf::from);
        let packages = self.query("installed", "", "", cancel)?;
        Ok(packages
            .into_iter()
            .map(|mut d| {
                if let Some(desktop) = self.desktop_entries().get(&d.package.id.name).cloned() {
                    d.package.icon = desktop_icon(home.as_deref(), &desktop);
                }
                d.package.component_ids = self
                    .component_map()
                    .get(&d.package.id.name)
                    .cloned()
                    .unwrap_or_default();
                // The native helper already reports the control-file
                // homepage for every package: reuse it as a grouping key
                // so CLI tools (no AppStream entry) group across managers.
                d.package.homepages = d.homepage.clone().into_iter().collect();
                d.package
            })
            .collect())
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        self.target(id)?;
        let mut details = self
            .query("details", &id.name, &id.architecture, cancel)?
            .into_iter()
            .find(|d| d.package.id == *id)
            .ok_or(EngineError::NotFound)?;
        if details.package.installed_version.is_some() {
            let home = self.transport.env("HOME").map(PathBuf::from);
            if let Some(desktop) = self.desktop_entries().get(&id.name).cloned() {
                details.package.icon = desktop_icon(home.as_deref(), &desktop);
            }
            details.package.component_ids = self
                .component_map()
                .get(&id.name)
                .cloned()
                .unwrap_or_default();
            details.package.homepages = details.homepage.clone().into_iter().collect();
        }
        Ok(details)
    }
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        let report = self.cleanup_report(cancel);
        if let Some(failure) = report.failures.into_iter().next() {
            Err(failure.error)
        } else {
            Ok(report.items)
        }
    }
    fn cleanup_report(&mut self, cancel: &Cancellation) -> CleanupReport {
        self.cleanup_apt_report(cancel, false)
    }
    fn cleanup_authenticated(&mut self, cancel: &Cancellation) -> CleanupReport {
        self.cleanup_apt_report(cancel, true)
    }
    fn cleanup_plan(
        &mut self,
        id: &CleanupId,
        cancel: &Cancellation,
    ) -> Result<CleanupItem, EngineError> {
        if id.backend != "apt" {
            return Err(invalid("apt", "foreign cleanup task"));
        }
        self.cleanup_apt_task(&id.key, cancel, id.key == "autoclean")?
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
            Operation::Clean(id) if id.backend == "apt" && id.key == "autoremove" => {
                AptAction::Autoremove
            }
            Operation::Clean(id) if id.backend == "apt" && id.key == "autoclean" => {
                AptAction::Autoclean
            }
            _ => return Err(invalid("apt", "foreign operation")),
        };
        progress(Progress::Message(
            "Running apt-get; cancellation waits for the native transaction to finish.".into(),
        ));
        let result = self.transport.apt_write(action, cancel)?;
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
/// Cask tokens share the formula policy: bare tokens and tap-qualified full
/// tokens, including versioned `@` tokens (for example `firefox@esr`).
fn cask_token(name: &str) -> bool {
    formula_name(name)
}
#[derive(Deserialize)]
struct CaskReport {
    casks: Vec<Cask>,
}
#[derive(Deserialize)]
struct Cask {
    full_token: String,
    name: Vec<String>,
    desc: Option<String>,
    homepage: String,
    version: String,
    installed: Option<String>,
    outdated: bool,
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
                            reference: None,
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
                        icon: None,
                        component_ids: vec![],
                        homepages: if f.homepage.is_empty() {
                            vec![]
                        } else {
                            vec![f.homepage.clone()]
                        },
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
        CLEAN_CAPABILITIES
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
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        fn plan(result: Completion) -> Result<Option<String>, EngineError> {
            let output = String::from_utf8(bytes("homebrew", result)?)
                .map_err(|error| invalid("homebrew", error))?;
            let output = output.trim();
            Ok((!output.is_empty()).then(|| output.to_owned()))
        }
        let mut items = Vec::new();
        if let Some(preview) = plan(self.call(&["autoremove", "--dry-run"], cancel, false)?)? {
            items.push(CleanupItem {
                id: CleanupId {
                    backend: "homebrew".into(),
                    key: "autoremove".into(),
                },
                kind: CleanupKind::OrphanDependencies,
                title: "Unused Homebrew dependencies".into(),
                summary: "Formulae no installed package still requires".into(),
                preview,
            });
        }
        if let Some(preview) = plan(self.call(&["cleanup", "--dry-run"], cancel, false)?)? {
            items.push(CleanupItem {
                id: CleanupId {
                    backend: "homebrew".into(),
                    key: "cleanup".into(),
                },
                kind: CleanupKind::PackageCache,
                title: "Old Homebrew downloads and versions".into(),
                summary: "Stale files selected by brew cleanup".into(),
                preview,
            });
        }
        Ok(items)
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
            Operation::Clean(id) if id.backend == "homebrew" && id.key == "autoremove" => {
                vec!["autoremove"]
            }
            Operation::Clean(id) if id.backend == "homebrew" && id.key == "cleanup" => {
                vec!["cleanup"]
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

impl<T: Transport> HomebrewCask<T> {
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
        let report: CaskReport = serde_json::from_slice(&bytes("homebrew-cask", result)?)
            .map_err(|e| invalid("homebrew-cask", e))?;
        let prefix = self
            .prefix
            .as_ref()
            .ok_or_else(|| invalid("homebrew-cask", "prefix not detected"))?;
        report
            .casks
            .into_iter()
            .map(|c| {
                if !cask_token(&c.full_token) {
                    return Err(invalid("homebrew-cask", "invalid cask token"));
                }
                let installed = c.installed.filter(|v| !v.is_empty());
                let display = c
                    .name
                    .iter()
                    .find(|n| !n.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| c.full_token.clone());
                Ok(PackageDetails {
                    package: Package {
                        id: PackageId {
                            backend: "homebrew-cask".into(),
                            name: c.full_token.clone(),
                            architecture: std::env::consts::ARCH.into(),
                            scope: Scope::Environment {
                                path: prefix.clone(),
                            },
                            remote: None,
                            reference: None,
                        },
                        display_name: display,
                        summary: c.desc.clone().unwrap_or_default(),
                        update: if c.outdated {
                            UpdateAvailability::Available
                        } else if installed.is_some() {
                            UpdateAvailability::Current
                        } else {
                            UpdateAvailability::Unknown
                        },
                        installed_version: installed,
                        candidate_version: Some(c.version),
                        icon: None,
                        component_ids: vec![],
                        homepages: if c.homepage.is_empty() {
                            vec![]
                        } else {
                            vec![c.homepage.clone()]
                        },
                    },
                    description: c.desc.unwrap_or_default(),
                    homepage: (!c.homepage.is_empty()).then_some(c.homepage),
                    dependencies: vec![],
                })
            })
            .collect()
    }
    fn target<'a>(&self, id: &'a PackageId) -> Result<&'a str, EngineError> {
        if id.backend != "homebrew-cask"
            || id.architecture != std::env::consts::ARCH
            || !cask_token(&id.name)
            || self
                .prefix
                .as_ref()
                .is_none_or(|p| id.scope != (Scope::Environment { path: p.clone() }))
        {
            return Err(invalid(
                "homebrew-cask",
                "foreign identity or invalid cask token",
            ));
        }
        Ok(&id.name)
    }
}
impl<T: Transport> Backend for HomebrewCask<T> {
    fn id(&self) -> &str {
        "homebrew-cask"
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        let result = self.transport.brew(&["--prefix".into()], cancel, false);
        if let Ok(result) = &result {
            let value = String::from_utf8(bytes("homebrew-cask", result.clone())?)
                .map_err(|e| invalid("homebrew-cask", e))?;
            let path = PathBuf::from(value.trim());
            if !path.is_absolute() {
                return Err(invalid("homebrew-cask", "prefix must be absolute"));
            }
            self.prefix = Some(path);
        }
        availability(result)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let data = bytes("homebrew-cask", self.call(&["casks"], cancel, false)?)?;
        let names = String::from_utf8(data).map_err(|e| invalid("homebrew-cask", e))?;
        let matches: Vec<_> = names
            .lines()
            .filter(|n| !n.trim().is_empty() && n.to_lowercase().contains(&query.to_lowercase()))
            .collect();
        let mut packages = Vec::new();
        for chunk in matches.chunks(100) {
            if chunk.iter().any(|n| !cask_token(n)) {
                return Err(invalid("homebrew-cask", "invalid cask token"));
            }
            let mut args = vec!["info", "--json=v2", "--cask", "--"];
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
                &["info", "--json=v2", "--cask", "--installed"],
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
        self.parse(self.call(&["info", "--json=v2", "--cask", "--", name], cancel, false)?)?
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
            // brew update refreshes formulae and casks together; both Homebrew
            // backends run it so each Refresh honestly refreshes its metadata.
            Operation::Refresh { backend } if backend == "homebrew-cask" => vec!["update"],
            Operation::Install(id) => vec!["install", "--cask", "--", self.target(id)?],
            Operation::Remove(id) => {
                vec!["uninstall", "--cask", "--force", "--", self.target(id)?]
            }
            Operation::Upgrade(id) => vec!["upgrade", "--cask", "--", self.target(id)?],
            Operation::UpgradeAll { backend } if backend == "homebrew-cask" => {
                vec!["upgrade", "--cask"]
            }
            _ => return Err(invalid("homebrew-cask", "foreign operation")),
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
            (_, Operation::Clean(_)) => vec![],
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
                reference: None,
            },
            display_name: name.into(),
            summary: summary.into(),
            installed_version: None,
            candidate_version: Some(version.into()),
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
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
                    // `snap find` prints Name Version Publisher Notes
                    // Summary; the store summary is what matched the query,
                    // so keep it for ranking instead of a placeholder.
                    // `snap list` has no summary column.
                    let summary: String = if installed {
                        "Snap package".into()
                    } else {
                        fields
                            .get(4..)
                            .map(|tail| tail.join(" "))
                            .unwrap_or_default()
                    };
                    let mut package =
                        self.package(fields[0], std::env::consts::ARCH, fields[1], &summary)?;
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
    /// Local snap icons resolve straight from the filesystem, which doubles
    /// as the installed check: remote rows only match a file when an
    /// installed snap shares the name, which is the same application.
    fn snap_icon(&self, package: &mut Package) {
        if matches!(self.kind, ManagerKind::Snap) {
            package.icon = snap_icon(std::path::Path::new("/"), &package.id.name);
        }
    }
    /// Desktop-id stems shipped by an installed snap. CLI-only snaps expose
    /// no desktop entry and group with nothing.
    fn snap_components(&self, package: &mut Package) {
        if matches!(self.kind, ManagerKind::Snap) {
            package.component_ids = snap_desktop_ids(std::path::Path::new("/"), &package.id.name);
        }
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
        let mut packages = self.query(false, query, cancel)?;
        let installed = self.query(true, "", cancel)?;
        let versions: std::collections::BTreeMap<_, _> = installed
            .iter()
            .map(|package| {
                (
                    (package.id.name.as_str(), package.id.architecture.as_str()),
                    &package.installed_version,
                )
            })
            .collect();
        for package in &mut packages {
            if let Some(version) =
                versions.get(&(package.id.name.as_str(), package.id.architecture.as_str()))
            {
                package.installed_version = (*version).clone();
            }
        }
        Ok(packages)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let mut packages = self.query(true, "", cancel)?;
        for package in &mut packages {
            self.snap_icon(package);
            self.snap_components(package);
        }
        Ok(packages)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let name = self.target(id)?;
        let mut package = self
            .query(false, &name, cancel)?
            .into_iter()
            .find(|package| package.id == *id)
            .ok_or(EngineError::NotFound)?;
        self.snap_icon(&mut package);
        self.snap_components(&mut package);
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
            Operation::Clean(_) => return Err(self.unsupported(Capability::Clean)),
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
        bytes(self.kind.id(), result.clone())?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}

/// Capabilities shared by user-scoped development managers. Search covers
/// installed tools by name; registry search and metadata refresh have no
/// safe offline analog, so the engine reports refresh explicitly unsupported
/// instead of guessing.
const DEV_CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Upgrade,
];

/// User-installed command-line tools (Waves 4-5). These managers run only as
/// the invoking user and never cross a privilege boundary. `pip` is pinned to
/// an explicitly selected virtual environment; the rest use one user-scoped
/// home each.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DevKind {
    Cargo,
    Npm,
    Pnpm,
    Bun,
    Pip,
    Pipx,
    Uv,
    Composer,
    Gem,
}

impl DevKind {
    fn id(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Bun => "bun",
            Self::Pip => "pip",
            Self::Pipx => "pipx",
            Self::Uv => "uv",
            Self::Composer => "composer",
            Self::Gem => "gem",
        }
    }
    fn executable(self) -> &'static str {
        match self {
            Self::Composer => "composer",
            Self::Gem => "gem",
            _ => self.id(),
        }
    }
    /// Per-manager package-name policy. Anything resembling an option, path,
    /// URL, version specifier, or whitespace is rejected before any manager runs.
    fn valid_name(self, name: &str) -> bool {
        match self {
            Self::Cargo | Self::Npm | Self::Pnpm | Self::Bun => dev_name(name),
            Self::Pip | Self::Pipx | Self::Uv => python_name(name),
            Self::Composer => composer_name(name),
            Self::Gem => gem_name(name),
        }
    }
}

/// Shared package-name policy for development registries: an optional
/// `@scope/` prefix plus dot-separated segments. Anything else (options,
/// paths, URLs, whitespace) is rejected before reaching the manager.
fn dev_name(name: &str) -> bool {
    fn segment(segment: &str) -> bool {
        !segment.is_empty()
            && segment.len() <= 214
            && segment.starts_with(|c: char| c.is_ascii_alphanumeric())
            && segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+_.-".contains(&b))
            && segment != "."
            && segment != ".."
    }
    if name.is_empty() || name.len() > 214 {
        return false;
    }
    match name.strip_prefix('@') {
        Some(scoped) => match scoped.split_once('/') {
            Some((scope, package)) => segment(scope) && segment(package),
            None => false,
        },
        None => segment(name) && !name.contains('/'),
    }
}

#[derive(Deserialize)]
struct NpmLs {
    dependencies: Option<std::collections::BTreeMap<String, NpmInstalled>>,
}

/// Explain an `npm ls` document without an installed tree: prefer npm's own
/// `error.summary`, then `error.detail` or code, then the first `problems`
/// entries. Falls back to a generic reason when npm said nothing usable.
fn npm_ls_problem(value: &serde_json::Value) -> String {
    const LIMIT: usize = 240;
    fn truncate(text: &str) -> String {
        if text.chars().count() > LIMIT {
            text.chars().take(LIMIT).collect::<String>() + "…"
        } else {
            text.to_owned()
        }
    }
    let detail = value
        .get("error")
        .and_then(|error| {
            error
                .get("summary")
                .or_else(|| error.get("detail"))
                .and_then(|detail| detail.as_str())
                .map(str::trim)
                .filter(|detail| !detail.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    error
                        .get("code")
                        .and_then(|code| code.as_str())
                        .map(|code| format!("npm error {code}"))
                })
        })
        .or_else(|| {
            value
                .get("problems")
                .and_then(|problems| problems.as_array())
                .map(|problems| {
                    problems
                        .iter()
                        .filter_map(|problem| problem.as_str())
                        .take(3)
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .filter(|joined| !joined.is_empty())
        });
    match detail {
        Some(detail) => format!("ls failed: {}", truncate(&detail)),
        None => "npm reported an error".to_owned(),
    }
}

#[derive(Deserialize)]
struct NpmInstalled {
    version: String,
}

#[derive(Deserialize)]
struct NpmOutdated {
    wanted: String,
}

#[derive(Deserialize)]
struct PnpmRoot {
    #[serde(default)]
    dependencies: std::collections::BTreeMap<String, PnpmEntry>,
}

#[derive(Deserialize)]
struct PnpmEntry {
    version: String,
}

#[derive(Deserialize)]
struct PnpmOutdated {
    latest: String,
}

#[derive(Deserialize)]
struct BunManifest {
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    homepage: Option<String>,
}

/// PEP 508 bare project names for `pip`/`pipx`/`uv`: no options, paths,
/// URLs, extras, or version specifiers.
fn python_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 214
        && !name.starts_with('-')
        && !name.ends_with(['-', '_', '.'])
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && !name.contains('/')
}

/// Composer `vendor/package` names, both parts lowercase alphanumerics with
/// `.`, `-`, `_` separators.
fn composer_name(name: &str) -> bool {
    fn part(part: &str) -> bool {
        !part.is_empty()
            && part.len() <= 214
            && part.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && part.ends_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
    }
    match name.split_once('/') {
        Some((vendor, package)) => {
            !name.starts_with('-')
                && !name.contains("//")
                && part(vendor)
                && part(package)
                && name.len() <= 214
        }
        None => false,
    }
}

/// RubyGems bare gem names.
fn gem_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 214
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && !name.starts_with('-')
        && !name.ends_with(['-', '_', '.'])
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

#[derive(Deserialize)]
struct PipEntry {
    name: String,
    version: String,
}

#[derive(Deserialize)]
struct PipOutdated {
    name: String,
    latest_version: String,
}

#[derive(Deserialize)]
struct PipxList {
    #[serde(default)]
    venvs: std::collections::BTreeMap<String, PipxVenv>,
}

#[derive(Deserialize)]
struct PipxVenv {
    metadata: PipxMetadata,
}

#[derive(Deserialize)]
struct PipxMetadata {
    main_package: PipxMain,
}

#[derive(Deserialize)]
struct PipxMain {
    package: String,
    package_version: String,
}

#[derive(Deserialize)]
struct PipxOutdatedList {
    data: PipxOutdatedData,
}

#[derive(Deserialize)]
struct PipxOutdatedData {
    #[serde(default)]
    packages: Vec<PipxOutdated>,
}

#[derive(Deserialize)]
struct PipxOutdated {
    package: String,
    latest_version: String,
}

#[derive(Deserialize)]
struct ComposerShow {
    #[serde(default)]
    installed: Vec<ComposerPackage>,
}

#[derive(Deserialize)]
struct ComposerPackage {
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    latest: Option<String>,
}

pub struct DevTool<T = NativeTransport> {
    kind: DevKind,
    transport: T,
    home: Option<PathBuf>,
}
pub type Cargo<T = NativeTransport> = DevTool<T>;
pub type Npm<T = NativeTransport> = DevTool<T>;
pub type Pnpm<T = NativeTransport> = DevTool<T>;
pub type Bun<T = NativeTransport> = DevTool<T>;
pub type Pip<T = NativeTransport> = DevTool<T>;
pub type Pipx<T = NativeTransport> = DevTool<T>;
pub type Uv<T = NativeTransport> = DevTool<T>;
pub type Composer<T = NativeTransport> = DevTool<T>;
pub type Gem<T = NativeTransport> = DevTool<T>;

impl<T> DevTool<T> {
    fn new(kind: DevKind, transport: T) -> Self {
        Self {
            kind,
            transport,
            home: None,
        }
    }
    pub fn cargo(transport: T) -> Self {
        Self::new(DevKind::Cargo, transport)
    }
    pub fn npm(transport: T) -> Self {
        Self::new(DevKind::Npm, transport)
    }
    pub fn pnpm(transport: T) -> Self {
        Self::new(DevKind::Pnpm, transport)
    }
    pub fn bun(transport: T) -> Self {
        Self::new(DevKind::Bun, transport)
    }
    pub fn pip(transport: T) -> Self {
        Self::new(DevKind::Pip, transport)
    }
    pub fn pipx(transport: T) -> Self {
        Self::new(DevKind::Pipx, transport)
    }
    pub fn uv(transport: T) -> Self {
        Self::new(DevKind::Uv, transport)
    }
    pub fn composer(transport: T) -> Self {
        Self::new(DevKind::Composer, transport)
    }
    pub fn gem(transport: T) -> Self {
        Self::new(DevKind::Gem, transport)
    }
}

impl<T: Transport> DevTool<T> {
    fn call(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.transport.dev_tool(
            self.kind.executable(),
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            cancel,
            write,
        )
    }
    /// Resolve the manager home used as the package scope. npm, pnpm, uv,
    /// Composer, and gem report it; Cargo, Bun, and pipx derive it from the
    /// sanitized environment; `pip` requires an explicitly selected `VIRTUAL_ENV`.
    fn home(&self, cancel: &Cancellation) -> Result<PathBuf, ExecutionError> {
        match self.kind {
            DevKind::Cargo | DevKind::Bun => {
                let home = self.transport.env("HOME").ok_or_else(|| {
                    ExecutionError::Disabled(format!("{} home not found", self.kind.id()))
                })?;
                let mut path = PathBuf::from(home);
                path.push(if self.kind == DevKind::Cargo {
                    ".cargo"
                } else {
                    ".bun/install/global"
                });
                Ok(path)
            }
            DevKind::Pip => {
                let venv = self.transport.env("VIRTUAL_ENV").ok_or_else(|| {
                    ExecutionError::Disabled(
                        "pip requires an explicitly selected virtual environment".to_string(),
                    )
                })?;
                let path = PathBuf::from(venv);
                if !path.is_absolute() {
                    return Err(ExecutionError::Invalid(
                        "virtual environment path must be absolute".into(),
                    ));
                }
                Ok(path)
            }
            DevKind::Pipx => {
                if let Some(dir) = self.transport.env("PIPX_HOME") {
                    let path = PathBuf::from(dir);
                    if !path.is_absolute() {
                        return Err(ExecutionError::Invalid("pipx home must be absolute".into()));
                    }
                    return Ok(path);
                }
                let home = self
                    .transport
                    .env("HOME")
                    .ok_or_else(|| ExecutionError::Disabled("pipx home not found".to_string()))?;
                Ok(PathBuf::from(home).join(".local/share/pipx"))
            }
            DevKind::Uv => {
                if let Some(dir) = self.transport.env("UV_TOOL_DIR") {
                    let path = PathBuf::from(dir);
                    if !path.is_absolute() {
                        return Err(ExecutionError::Invalid(
                            "uv tool directory must be absolute".into(),
                        ));
                    }
                    return Ok(path);
                }
                let output = self.call(&["tool", "dir"], cancel, false)?;
                let path = PathBuf::from(
                    String::from_utf8(output.stdout)
                        .map_err(|e| ExecutionError::Invalid(e.to_string()))?
                        .trim(),
                );
                if !path.is_absolute() {
                    return Err(ExecutionError::Invalid(
                        "uv tool directory must be absolute".into(),
                    ));
                }
                Ok(path)
            }
            DevKind::Composer => {
                if let Some(dir) = self.transport.env("COMPOSER_HOME") {
                    let path = PathBuf::from(dir);
                    if !path.is_absolute() {
                        return Err(ExecutionError::Invalid(
                            "Composer home must be absolute".into(),
                        ));
                    }
                    return Ok(path);
                }
                let output = self.call(&["config", "--global", "home"], cancel, false)?;
                let text = String::from_utf8(output.stdout)
                    .map_err(|e| ExecutionError::Invalid(e.to_string()))?;
                let line = text.lines().map(str::trim).rfind(|line| !line.is_empty());
                match line.map(PathBuf::from) {
                    Some(path) if path.is_absolute() => Ok(path),
                    _ => Err(ExecutionError::Invalid(
                        "Composer home must be absolute".into(),
                    )),
                }
            }
            DevKind::Gem => {
                if let Some(dir) = self.transport.env("GEM_HOME") {
                    let path = PathBuf::from(dir);
                    if !path.is_absolute() {
                        return Err(ExecutionError::Invalid(
                            "RubyGems home must be absolute".into(),
                        ));
                    }
                    return Ok(path);
                }
                let output = self.call(&["env"], cancel, false)?;
                let text = String::from_utf8(output.stdout)
                    .map_err(|e| ExecutionError::Invalid(e.to_string()))?;
                for line in text.lines() {
                    if let Some(dir) = line.trim().strip_prefix("- USER INSTALLATION DIRECTORY:") {
                        let path = PathBuf::from(dir.trim());
                        if path.is_absolute() {
                            return Ok(path);
                        }
                        break;
                    }
                }
                Err(ExecutionError::Invalid(
                    "RubyGems user directory must be absolute".into(),
                ))
            }
            DevKind::Npm | DevKind::Pnpm => {
                let output = self.call(&["root", "--global"], cancel, false)?;
                let path = PathBuf::from(
                    String::from_utf8(output.stdout)
                        .map_err(|e| ExecutionError::Invalid(e.to_string()))?
                        .trim(),
                );
                if !path.is_absolute() {
                    return Err(ExecutionError::Invalid(format!(
                        "{} global root must be absolute",
                        self.kind.id()
                    )));
                }
                Ok(path)
            }
        }
    }
    /// Parse manager JSON leniently: clean exits must parse, while
    /// data-bearing failure exits (npm outdated packages, troubled trees)
    /// are accepted only when stdout holds the expected document.
    fn lenient_json(
        &self,
        args: &[&str],
        cancel: &Cancellation,
    ) -> Result<(serde_json::Value, bool), EngineError> {
        match self.call(args, cancel, false) {
            Ok(result) => {
                let value = serde_json::from_slice(&bytes(self.kind.id(), result)?)
                    .map_err(|e| invalid(self.kind.id(), e))?;
                Ok((value, false))
            }
            Err(ExecutionError::Failed(result)) => {
                let value = serde_json::from_slice(&result.stdout)
                    .map_err(|_| EngineError::from(ExecutionError::Failed(result)))?;
                Ok((value, true))
            }
            Err(error) => Err(error.into()),
        }
    }
    fn pip_call(
        &self,
        venv: &std::path::Path,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.transport.venv_pip(
            venv,
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            cancel,
            write,
        )
    }
    fn target<'a>(&self, id: &'a PackageId) -> Result<&'a str, EngineError> {
        if id.backend != self.kind.id()
            || id.architecture != std::env::consts::ARCH
            || !self.kind.valid_name(&id.name)
            || self
                .home
                .as_ref()
                .is_none_or(|home| id.scope != (Scope::Environment { path: home.clone() }))
        {
            return Err(invalid(
                self.kind.id(),
                "foreign identity or invalid package name",
            ));
        }
        Ok(&id.name)
    }
    fn detail(
        &self,
        home: &std::path::Path,
        name: String,
        summary: String,
        homepage: Option<String>,
        installed: String,
        candidate: Option<String>,
    ) -> Result<PackageDetails, EngineError> {
        if !self.kind.valid_name(&name) {
            return Err(invalid(
                self.kind.id(),
                "foreign identity or invalid package name",
            ));
        }
        Ok(PackageDetails {
            package: Package {
                id: PackageId {
                    backend: self.kind.id().into(),
                    name: name.clone(),
                    architecture: std::env::consts::ARCH.into(),
                    scope: Scope::Environment {
                        path: home.to_path_buf(),
                    },
                    remote: None,
                    reference: None,
                },
                display_name: name,
                summary: summary.clone(),
                update: match &candidate {
                    Some(candidate) if candidate != &installed => UpdateAvailability::Available,
                    _ => UpdateAvailability::Current,
                },
                installed_version: Some(installed),
                candidate_version: candidate,
                icon: None,
                component_ids: vec![],
                // Manifest homepages (bun, composer, …) double as grouping
                // keys so dev tools group with native builds of the same app.
                homepages: homepage.clone().into_iter().collect(),
            },
            description: summary,
            homepage,
            dependencies: vec![],
        })
    }
    /// Split one `cargo install --list` header (`name vversion [(source)]:`)
    /// without allocating. Registry installs omit the source; anything else
    /// is not a package entry.
    fn cargo_header(line: &str) -> Option<(&str, &str)> {
        let header = line.strip_suffix(':')?;
        let (name, rest) = header.split_once(' ')?;
        let (version, sourced) = match rest.split_once(' ') {
            Some((version, source)) => (version, source.starts_with('(') && source.ends_with(')')),
            None => (rest, true),
        };
        let version = version.strip_prefix('v')?;
        if !sourced
            || !dev_name(name)
            || version.is_empty()
            || version.chars().any(char::is_whitespace)
        {
            return None;
        }
        Some((name, version))
    }
    fn cargo_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let output = bytes(id, self.call(&["install", "--list"], cancel, false)?)?;
        let text = String::from_utf8(output).map_err(|e| invalid(id, e))?;
        let mut details = Vec::new();
        for line in text.lines() {
            if line.is_empty() || line.starts_with([' ', '\t']) {
                continue;
            }
            let Some((name, version)) = Self::cargo_header(line) else {
                return Err(invalid(id, "invalid cargo metadata"));
            };
            details.push(self.detail(
                home,
                name.into(),
                "Cargo-installed command-line tool".into(),
                None,
                version.into(),
                None,
            )?);
        }
        Ok(details)
    }
    fn npm_table(
        &self,
        cancel: &Cancellation,
    ) -> Result<std::collections::BTreeMap<String, (String, Option<String>)>, EngineError> {
        let id = self.kind.id();
        let (value, lenient) =
            self.lenient_json(&["ls", "--global", "--depth=0", "--json"], cancel)?;
        // Keep npm's own diagnostic before the document moves: a failing
        // tree without installed packages reports its cause here instead of
        // a bare failure downstream.
        let problem = npm_ls_problem(&value);
        let report: NpmLs = serde_json::from_value(value).map_err(|e| invalid(id, e))?;
        let installed = match (report.dependencies, lenient) {
            (Some(dependencies), _) => dependencies,
            (None, false) => std::collections::BTreeMap::new(),
            (None, true) => return Err(invalid(id, problem)),
        };
        let outdated: std::collections::BTreeMap<String, NpmOutdated> =
            match self.lenient_json(&["outdated", "--global", "--json"], cancel) {
                Ok((value, _)) => serde_json::from_value(value).map_err(|e| invalid(id, e))?,
                // Registry metadata may be unreachable; installed state stays.
                Err(_) => std::collections::BTreeMap::new(),
            };
        Ok(installed
            .into_iter()
            .map(|(name, entry)| {
                let wanted = outdated.get(&name).map(|outdated| outdated.wanted.clone());
                (name, (entry.version, wanted))
            })
            .collect())
    }
    fn npm_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        self.npm_table(cancel)?
            .into_iter()
            .map(|(name, (installed, wanted))| {
                let candidate = match wanted {
                    Some(wanted) if wanted != installed => wanted,
                    _ => installed.clone(),
                };
                self.detail(
                    home,
                    name,
                    "npm-installed command-line tool".into(),
                    None,
                    installed,
                    Some(candidate),
                )
            })
            .collect()
    }
    fn pnpm_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let (value, _) = self.lenient_json(&["ls", "--global", "--json", "--depth=0"], cancel)?;
        let roots: Vec<PnpmRoot> = serde_json::from_value(value).map_err(|e| invalid(id, e))?;
        let outdated: std::collections::BTreeMap<String, PnpmOutdated> =
            match self.lenient_json(&["outdated", "--global", "--json"], cancel) {
                Ok((value, _)) => serde_json::from_value(value).map_err(|e| invalid(id, e))?,
                // Registry metadata may be unreachable; installed state stays.
                Err(_) => std::collections::BTreeMap::new(),
            };
        let mut details = Vec::new();
        for root in roots {
            for (name, entry) in root.dependencies {
                let candidate = outdated.get(&name).map(|outdated| outdated.latest.clone());
                let candidate = match candidate {
                    Some(latest) if latest != entry.version => Some(latest),
                    _ => Some(entry.version.clone()),
                };
                details.push(self.detail(
                    home,
                    name,
                    "pnpm-installed command-line tool".into(),
                    None,
                    entry.version,
                    candidate,
                )?);
            }
        }
        Ok(details)
    }
    fn bun_package(&self, home: &std::path::Path, dir: &std::path::Path) -> Option<PackageDetails> {
        let manifest = std::fs::read_to_string(host_metadata_path(&dir.join("package.json"))).ok()?;
        let manifest: BunManifest = serde_json::from_str(&manifest).ok()?;
        if manifest.version.is_empty() {
            return None;
        }
        let homepage = manifest.homepage.filter(|homepage| !homepage.is_empty());
        self.detail(
            home,
            manifest.name,
            if manifest.description.is_empty() {
                "Bun-installed command-line tool".into()
            } else {
                manifest.description
            },
            homepage,
            manifest.version,
            None,
        )
        .ok()
    }
    fn bun_inventory(&self, home: &std::path::Path) -> Result<Vec<PackageDetails>, EngineError> {
        let mut details = Vec::new();
        let entries = match std::fs::read_dir(host_metadata_path(&home.join("node_modules"))) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(invalid(self.kind.id(), e.to_string())),
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path
                .file_name()
                .is_some_and(|name| name.to_str().is_some_and(|name| name.starts_with('@')))
            {
                for scoped in std::fs::read_dir(&path)
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                {
                    details.extend(self.bun_package(home, &scoped.path()));
                }
            } else {
                details.extend(self.bun_package(home, &path));
            }
        }
        Ok(details)
    }
    fn pip_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let output = bytes(
            id,
            self.pip_call(home, &["list", "--format=json"], cancel, false)?,
        )?;
        let entries: Vec<PipEntry> = serde_json::from_slice(&output).map_err(|e| invalid(id, e))?;
        let outdated: std::collections::BTreeMap<String, String> = match self.pip_call(
            home,
            &["list", "--outdated", "--format=json"],
            cancel,
            false,
        ) {
            Ok(result) => serde_json::from_slice(&bytes(id, result)?)
                .map(|entries: Vec<PipOutdated>| {
                    entries
                        .into_iter()
                        .map(|entry| (entry.name, entry.latest_version))
                        .collect()
                })
                .map_err(|e| invalid(id, e))?,
            // Registry metadata may be unreachable; installed state stays.
            Err(_) => std::collections::BTreeMap::new(),
        };
        entries
            .into_iter()
            // The installer itself is never managed: upgrading pip from
            // inside pip breaks its own output pipe and risks the venv.
            .filter(|entry| {
                !matches!(
                    entry.name.to_ascii_lowercase().as_str(),
                    "pip" | "setuptools" | "wheel" | "distribute"
                )
            })
            .map(|entry| {
                let candidate = outdated.get(&entry.name).cloned().or_else(|| {
                    // Installed state without newer metadata stays current.
                    Some(entry.version.clone())
                });
                self.detail(
                    home,
                    entry.name,
                    format!("Python package installed in {}", home.display()),
                    None,
                    entry.version,
                    candidate,
                )
            })
            .collect()
    }
    fn pipx_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let output = bytes(id, self.call(&["list", "--json"], cancel, false)?)?;
        let list: PipxList = serde_json::from_slice(&output).map_err(|e| invalid(id, e))?;
        let outdated: std::collections::BTreeMap<String, String> =
            match self.call(&["list", "--outdated", "--json"], cancel, false) {
                Ok(result) => serde_json::from_slice(&bytes(id, result)?)
                    .map(|list: PipxOutdatedList| {
                        list.data
                            .packages
                            .into_iter()
                            .map(|entry| (entry.package, entry.latest_version))
                            .collect()
                    })
                    .unwrap_or_default(),
                Err(_) => std::collections::BTreeMap::new(),
            };
        list.venvs
            .into_values()
            .map(|venv| {
                let main = venv.metadata.main_package;
                let candidate = outdated
                    .get(&main.package)
                    .cloned()
                    .unwrap_or_else(|| main.package_version.clone());
                self.detail(
                    home,
                    main.package,
                    "Isolated Python application installed with pipx".into(),
                    None,
                    main.package_version,
                    Some(candidate),
                )
            })
            .collect()
    }
    /// Parse one `uv tool list [--outdated]` line: `name vversion` with an
    /// optional `[latest: version]` suffix. Executable continuation lines
    /// (`- name`) carry no version and are skipped by the caller via `None`;
    /// anything else without a version is not a tool entry.
    fn uv_entry(line: &str) -> Option<(String, String, Option<String>)> {
        let line = line.trim();
        if line.is_empty() || line == "-" || line.starts_with("- ") {
            return None;
        }
        let (name, rest) = line.split_once(' ')?;
        let rest = rest.trim();
        let (version, latest) = match rest.split_once("[latest:") {
            Some((version, latest)) => {
                let latest = latest.strip_suffix(']')?.trim();
                (version.trim(), Some(latest.to_string()))
            }
            None => (rest, None),
        };
        let version = version.strip_prefix('v')?;
        if version.is_empty() || version.chars().any(char::is_whitespace) {
            return None;
        }
        Some((name.to_string(), version.to_string(), latest))
    }
    fn uv_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let output = bytes(id, self.call(&["tool", "list"], cancel, false)?)?;
        let text = String::from_utf8(output).map_err(|e| invalid(id, e))?;
        if text.trim().is_empty() || text.trim() == "No tools installed" {
            return Ok(vec![]);
        }
        let outdated: std::collections::BTreeMap<String, String> =
            match self.call(&["tool", "list", "--outdated"], cancel, false) {
                Ok(result) => String::from_utf8(bytes(id, result)?)
                    .map(|text| {
                        text.lines()
                            .filter_map(Self::uv_entry)
                            .filter_map(|(name, _, latest)| latest.map(|latest| (name, latest)))
                            .collect()
                    })
                    .unwrap_or_default(),
                Err(_) => std::collections::BTreeMap::new(),
            };
        let mut details = Vec::new();
        for line in text.lines() {
            let Some((name, version, _)) = Self::uv_entry(line) else {
                continue;
            };
            // Skip executable continuation lines already filtered; only tool
            // headers carry versions.
            if name.is_empty() {
                continue;
            }
            let candidate = outdated
                .get(&name)
                .cloned()
                .unwrap_or_else(|| version.clone());
            details.push(self.detail(
                home,
                name,
                "Isolated Python tool installed with uv".into(),
                None,
                version,
                Some(candidate),
            )?);
        }
        Ok(details)
    }
    fn composer_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let output = bytes(
            id,
            self.call(&["global", "show", "--format=json"], cancel, false)?,
        )?;
        // An empty global home has no composer.json; Composer reports that on
        // stderr but still exits 0 with no JSON. Treat unparseable empty
        // output as no packages only when stdout holds no document.
        if output.iter().all(|b| b.is_ascii_whitespace()) {
            return Ok(vec![]);
        }
        let show: ComposerShow = serde_json::from_slice(&output).map_err(|e| invalid(id, e))?;
        let outdated: std::collections::BTreeMap<String, String> = match self.call(
            &["global", "show", "--outdated", "--format=json"],
            cancel,
            false,
        ) {
            Ok(result) => serde_json::from_slice(&bytes(id, result)?)
                .map(|show: ComposerShow| {
                    show.installed
                        .into_iter()
                        .filter_map(|package| package.latest.map(|latest| (package.name, latest)))
                        .collect()
                })
                .unwrap_or_default(),
            Err(_) => std::collections::BTreeMap::new(),
        };
        show.installed
            .into_iter()
            .map(|package| {
                let candidate = outdated
                    .get(&package.name)
                    .cloned()
                    .or(package.latest.clone())
                    .unwrap_or_else(|| package.version.clone());
                self.detail(
                    home,
                    package.name,
                    package.description.clone().unwrap_or_default(),
                    package.homepage.clone(),
                    package.version,
                    Some(candidate),
                )
            })
            .collect()
    }
    /// Split a `*.gemspec` file name into `(name, version)`. Gem names may
    /// contain dashes, so split at the last dash preceding a version.
    fn gem_filename(file: &str) -> Option<(String, String)> {
        let base = file.strip_suffix(".gemspec")?;
        let (name, version) = base.rsplit_once('-')?;
        if name.is_empty()
            || version.is_empty()
            || !version.starts_with(|c: char| c.is_ascii_digit())
        {
            return None;
        }
        Some((name.to_string(), version.to_string()))
    }
    /// Compare dot-separated gem versions numerically where possible so the
    /// inventory reports one identity per gem (the highest installed).
    fn gem_version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
        let mut a_parts = a.split('.');
        let mut b_parts = b.split('.');
        loop {
            match (a_parts.next(), b_parts.next()) {
                (None, None) => return std::cmp::Ordering::Equal,
                (None, _) => return std::cmp::Ordering::Less,
                (_, None) => return std::cmp::Ordering::Greater,
                (Some(x), Some(y)) => {
                    let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                        (Ok(x), Ok(y)) => x.cmp(&y),
                        _ => x.cmp(y),
                    };
                    if ord != std::cmp::Ordering::Equal {
                        return ord;
                    }
                }
            }
        }
    }
    /// Parse one `gem outdated` line: `name (installed < latest[, ...])`.
    fn gem_outdated(line: &str) -> Option<(String, String)> {
        let (name, rest) = line.split_once(' ')?;
        if name.is_empty() || name.starts_with('-') {
            return None;
        }
        let inner = rest.trim().strip_prefix('(')?.strip_suffix(')')?;
        let (before, latest) = inner.split_once('<')?;
        // Validate the installed side, then report the latest version.
        before.split(',').rfind(|part| !part.trim().is_empty())?;
        let latest = latest.split(',').next()?;
        Some((name.to_string(), latest.trim().to_string()))
    }
    fn gem_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let mut names: Vec<(String, String)> = Vec::new();
        match std::fs::read_dir(host_metadata_path(&home.join("specifications"))) {
            Ok(entries) => {
                for entry in entries.filter_map(Result::ok) {
                    if cancel.requested() {
                        return Err(EngineError::Cancelled);
                    }
                    let file = entry.file_name().to_string_lossy().into_owned();
                    if let Some((name, version)) = Self::gem_filename(&file) {
                        names.push((name, version));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(invalid(id, e.to_string())),
        }
        let outdated: std::collections::BTreeMap<String, String> =
            match self.call(&["outdated"], cancel, false) {
                Ok(result) => String::from_utf8(bytes(id, result)?)
                    .map(|text| text.lines().filter_map(Self::gem_outdated).collect())
                    .unwrap_or_default(),
                Err(_) => std::collections::BTreeMap::new(),
            };
        // Native updates retain older versions beside the new one; report one
        // identity per gem at the highest installed version so engine
        // selection stays unambiguous. Removal still drops every version.
        // Sorted collection keeps the surviving identity deterministic.
        names.sort();
        let mut highest: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for (name, version) in names {
            highest
                .entry(name)
                .and_modify(|keep| {
                    if Self::gem_version_cmp(&version, keep) == std::cmp::Ordering::Greater {
                        *keep = version.clone();
                    }
                })
                .or_insert(version);
        }
        highest
            .into_iter()
            .map(|(name, version)| {
                let candidate = outdated
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| version.clone());
                self.detail(
                    home,
                    name,
                    "RubyGem installed for the invoking user".into(),
                    None,
                    version,
                    Some(candidate),
                )
            })
            .collect()
    }
    fn inventory(&self, cancel: &Cancellation) -> Result<Vec<PackageDetails>, EngineError> {
        let home = self
            .home
            .clone()
            .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
        match self.kind {
            DevKind::Cargo => self.cargo_inventory(&home, cancel),
            DevKind::Npm => self.npm_inventory(&home, cancel),
            DevKind::Pnpm => self.pnpm_inventory(&home, cancel),
            DevKind::Bun => self.bun_inventory(&home),
            DevKind::Pip => self.pip_inventory(&home, cancel),
            DevKind::Pipx => self.pipx_inventory(&home, cancel),
            DevKind::Uv => self.uv_inventory(&home, cancel),
            DevKind::Composer => self.composer_inventory(&home, cancel),
            DevKind::Gem => self.gem_inventory(&home, cancel),
        }
    }
    fn upgrade(
        &self,
        target: &PackageId,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        // pnpm and Bun keep pinned ranges, so plain `update` would not move
        // explicitly versioned tools. Reinstalling at `@latest` is also what
        // the reported candidate reflects. Composer re-requires the package
        // for the same reason; pip reinstalls with `--upgrade`.
        let name = self.target(target)?;
        progress(Progress::Message(format!(
            "Upgrading {name} as the invoking user."
        )));
        let home = self
            .home
            .clone()
            .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
        let result = match self.kind {
            DevKind::Cargo => self.call(&["install", "--force", name], cancel, true)?,
            DevKind::Npm => self.call(&["update", "--global", name], cancel, true)?,
            DevKind::Pnpm | DevKind::Bun => {
                let latest = format!("{name}@latest");
                self.call(&["add", "--global", &latest], cancel, true)?
            }
            DevKind::Pip => self.pip_call(&home, &["install", "--upgrade", name], cancel, true)?,
            DevKind::Pipx => self.call(&["upgrade", name], cancel, true)?,
            // uv keeps exact version pins, so plain `tool upgrade` would not
            // move explicitly versioned tools. Reinstalling at `@latest` is
            // also what the reported candidate reflects.
            DevKind::Uv => {
                let latest = format!("{name}@latest");
                self.call(&["tool", "install", "--force", &latest], cancel, true)?
            }
            DevKind::Composer => self.call(
                &[
                    "global",
                    "require",
                    "--no-interaction",
                    "--no-progress",
                    name,
                ],
                cancel,
                true,
            )?,
            DevKind::Gem => self.call(
                &["update", "--user-install", "--no-document", name],
                cancel,
                true,
            )?,
        };
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
    fn upgrade_all(
        &self,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        // Cargo and pip have no registry upgrade-all command; refresh every
        // installed tool in place as the single per-backend transaction.
        if self.kind == DevKind::Npm {
            progress(Progress::Message(format!(
                "Updating {} packages as the invoking user.",
                self.kind.id()
            )));
            let result = self.call(&["update", "--global"], cancel, true)?;
            return Ok(OperationOutcome {
                cancellation_deferred: result.cancellation_deferred,
            });
        }
        if self.kind == DevKind::Pipx {
            progress(Progress::Message(
                "Upgrading pipx packages as the invoking user.".into(),
            ));
            let result = self.call(&["upgrade-all"], cancel, true)?;
            return Ok(OperationOutcome {
                cancellation_deferred: result.cancellation_deferred,
            });
        }
        if self.kind == DevKind::Uv {
            // `tool upgrade --all` keeps exact pins like per-package upgrade,
            // so reinstall every tool at `@latest` as the single transaction.
            for package in self.inventory(cancel)? {
                let name = &package.package.id.name;
                progress(Progress::Message(format!("Upgrading {name} to latest.")));
                let latest = format!("{name}@latest");
                self.call(&["tool", "install", "--force", &latest], cancel, true)?;
            }
            return Ok(OperationOutcome::default());
        }
        if self.kind == DevKind::Composer {
            // `global update` respects pinned constraints, so re-require each
            // installed package to reach the reported latest candidates.
            for package in self.inventory(cancel)? {
                let name = &package.package.id.name;
                progress(Progress::Message(format!("Upgrading {name}.")));
                self.call(
                    &[
                        "global",
                        "require",
                        "--no-interaction",
                        "--no-progress",
                        name,
                    ],
                    cancel,
                    true,
                )?;
            }
            return Ok(OperationOutcome::default());
        }
        if self.kind == DevKind::Gem {
            progress(Progress::Message(
                "Updating RubyGems as the invoking user.".into(),
            ));
            let result = self.call(&["update", "--user-install", "--no-document"], cancel, true)?;
            return Ok(OperationOutcome {
                cancellation_deferred: result.cancellation_deferred,
            });
        }
        for package in self.inventory(cancel)? {
            let name = &package.package.id.name;
            if self.kind == DevKind::Cargo {
                progress(Progress::Message(format!("Reinstalling {name}.")));
                self.call(&["install", "--force", name], cancel, true)?;
            } else if self.kind == DevKind::Pip {
                let home = self
                    .home
                    .clone()
                    .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
                progress(Progress::Message(format!("Upgrading {name}.")));
                self.pip_call(&home, &["install", "--upgrade", name], cancel, true)?;
            } else {
                progress(Progress::Message(format!("Upgrading {name} to latest.")));
                let latest = format!("{name}@latest");
                self.call(&["add", "--global", &latest], cancel, true)?;
            }
        }
        Ok(OperationOutcome::default())
    }
}

impl<T: Transport> Backend for DevTool<T> {
    fn id(&self) -> &str {
        self.kind.id()
    }
    fn capabilities(&self) -> &[Capability] {
        if matches!(self.kind, DevKind::Npm | DevKind::Pip | DevKind::Uv) {
            cleanup::DEV_CLEAN_CAPABILITIES
        } else {
            DEV_CAPABILITIES
        }
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        let home = match self.home(cancel) {
            Ok(home) => home,
            Err(ExecutionError::Disabled(reason)) => {
                return Ok(Availability::Unavailable(reason));
            }
            Err(error) => return Err(error.into()),
        };
        self.home = Some(home.clone());
        if self.kind == DevKind::Pip {
            availability(self.transport.venv_pip(
                &home,
                &[OsString::from("--version")],
                cancel,
                false,
            ))
        } else {
            availability(self.call(&["--version"], cancel, false))
        }
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .inventory(cancel)?
            .into_iter()
            .map(|d| d.package)
            .collect())
    }
    /// Search filters installed tools by name. A valid query with no
    /// installed match resolves to a synthetic installable candidate so
    /// selection, confirmation, and installation keep working uniformly;
    /// details remain available only for installed tools.
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let home = self
            .home
            .clone()
            .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
        let lowered = query.to_ascii_lowercase();
        let mut results: Vec<Package> = self
            .inventory(cancel)?
            .into_iter()
            .map(|d| d.package)
            .filter(|package| {
                package.id.name.to_ascii_lowercase().contains(&lowered)
                    || package.display_name.to_ascii_lowercase().contains(&lowered)
            })
            .collect();
        if results.is_empty() && self.kind.valid_name(query) {
            results.push(Package {
                id: PackageId {
                    backend: self.kind.id().into(),
                    name: query.into(),
                    architecture: std::env::consts::ARCH.into(),
                    scope: Scope::Environment { path: home },
                    remote: None,
                    reference: None,
                },
                display_name: query.into(),
                summary: format!("Install {query} with {}", self.kind.id()),
                installed_version: None,
                candidate_version: None,
                update: UpdateAvailability::Unknown,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
            });
        }
        Ok(results)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        self.target(id)?;
        self.inventory(cancel)?
            .into_iter()
            .find(|d| d.package.id == *id)
            .ok_or(EngineError::NotFound)
    }
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        self.cleanup_cache(cancel)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let id = self.kind.id();
        if operation.backend() != id {
            return Err(invalid(id, "foreign operation"));
        }
        let args: Vec<&str> = match operation {
            Operation::Refresh { .. } => return Err(self.unsupported(Capability::Refresh)),
            Operation::Install(target) => match self.kind {
                DevKind::Cargo => vec!["install", self.target(target)?],
                DevKind::Npm => vec!["install", "--global", self.target(target)?],
                DevKind::Pnpm | DevKind::Bun => vec!["add", "--global", self.target(target)?],
                DevKind::Pipx => vec!["install", self.target(target)?],
                DevKind::Uv => vec!["tool", "install", self.target(target)?],
                DevKind::Composer => vec![
                    "global",
                    "require",
                    "--no-interaction",
                    "--no-progress",
                    self.target(target)?,
                ],
                DevKind::Gem => vec![
                    "install",
                    "--user-install",
                    "--no-document",
                    self.target(target)?,
                ],
                DevKind::Pip => {
                    let name = self.target(target)?;
                    let home = self
                        .home
                        .clone()
                        .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
                    progress(Progress::Message(format!(
                        "Running {id} as the invoking user; cancellation waits for completion."
                    )));
                    let result = self.pip_call(&home, &["install", name], cancel, true)?;
                    return Ok(OperationOutcome {
                        cancellation_deferred: result.cancellation_deferred,
                    });
                }
            },
            Operation::Remove(target) => match self.kind {
                DevKind::Cargo => vec!["uninstall", self.target(target)?],
                DevKind::Npm => vec!["uninstall", "--global", self.target(target)?],
                DevKind::Pnpm | DevKind::Bun => vec!["remove", "--global", self.target(target)?],
                DevKind::Pipx => vec!["uninstall", self.target(target)?],
                DevKind::Uv => vec!["tool", "uninstall", self.target(target)?],
                DevKind::Composer => vec!["global", "remove", self.target(target)?],
                DevKind::Gem => vec![
                    "uninstall",
                    "--user-install",
                    "-x",
                    "-a",
                    self.target(target)?,
                ],
                DevKind::Pip => {
                    let name = self.target(target)?;
                    let home = self
                        .home
                        .clone()
                        .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
                    progress(Progress::Message(format!(
                        "Running {id} as the invoking user; cancellation waits for completion."
                    )));
                    let result = self.pip_call(&home, &["uninstall", "-y", name], cancel, true)?;
                    return Ok(OperationOutcome {
                        cancellation_deferred: result.cancellation_deferred,
                    });
                }
            },
            Operation::Upgrade(target) => {
                return self.upgrade(target, cancel, progress);
            }
            Operation::UpgradeAll { .. } => {
                return self.upgrade_all(cancel, progress);
            }
            Operation::Clean(target) => return self.clean_cache(target, cancel),
        };
        progress(Progress::Message(format!(
            "Running {id} as the invoking user; cancellation waits for completion."
        )));
        let result = self.call(&args, cancel, true)?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}

/// Missing optional managers are omitted from automatic queries, but explicit selections
/// and source discovery retain their unavailability. Detection failures are never hidden.
///
/// `sources` selects backends by id: empty means every available backend,
/// while a non-empty set registers exactly those members (unavailable ones
/// included, so discovery and explicit queries report their status).
pub fn native_engine(
    sources: &[String],
    discover: bool,
    authorization: Authorization,
    cancel: &Cancellation,
) -> Result<Engine, EngineError> {
    if let Some(unknown) = sources.iter().find(|s| !BACKEND_IDS.contains(&s.as_str())) {
        return Err(EngineError::UnknownBackend(unknown.into()));
    }
    let allowed = |id: &str| sources.is_empty() || sources.iter().any(|s| s == id);
    let explicit = !sources.is_empty();
    let mut engine = Engine::default();
    let host = Host::current();
    if !discover && sources.is_empty() {
        if let Some(reason) = host.runtime.disabled_reason() {
            return Err(ExecutionError::Disabled(reason.into()).into());
        }
    }
    let mut apt = Apt::new(NativeTransport {
        host: host.clone(),
        authorization,
    });
    let mut brew = Homebrew::new(NativeTransport {
        host: host.clone(),
        authorization,
    });
    let mut cask = HomebrewCask::new(NativeTransport {
        host,
        authorization,
    });
    if allowed("apt") {
        let status = apt.detect(cancel);
        if discover || explicit || !matches!(status, Ok(Availability::Unavailable(_))) {
            engine.note_detected("apt".into(), status);
            engine.register(apt)?;
        }
    }
    if allowed("homebrew") {
        let status = brew.detect(cancel);
        if discover || explicit || !matches!(status, Ok(Availability::Unavailable(_))) {
            engine.note_detected("homebrew".into(), status);
            engine.register(brew)?;
        }
    }
    if cfg!(target_os = "macos") && allowed("homebrew-cask") {
        let status = cask.detect(cancel);
        if discover || explicit || !matches!(status, Ok(Availability::Unavailable(_))) {
            engine.note_detected("homebrew-cask".into(), status);
            engine.register(cask)?;
        }
    }
    for backend in ["dnf", "pacman", "zypper", "snap"] {
        if allowed(backend) {
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
            if discover || explicit {
                engine.register(manager)?;
                continue;
            }
            let status = manager.detect(cancel);
            if !matches!(status, Ok(Availability::Unavailable(_))) {
                engine.note_detected(backend.into(), status);
                engine.register(manager)?;
            }
        }
    }
    if allowed("fwupd") {
        let mut firmware = Firmware::new(NativeTransport {
            host: Host::current(),
            authorization,
        });
        let status = firmware.detect(cancel);
        if discover || explicit || !matches!(status, Ok(Availability::Unavailable(_))) {
            engine.note_detected("fwupd".into(), status);
            engine.register(firmware)?;
        }
    }
    if allowed("appimage") {
        engine.register(AppImage::native())?;
    }
    for tool in StandaloneTool::ALL {
        if allowed(tool.id()) {
            let mut backend = Standalone::native(tool);
            let status = backend.detect(cancel);
            if discover || explicit || !matches!(status, Ok(Availability::Unavailable(_))) {
                engine.note_detected(tool.id().into(), status);
                engine.register(backend)?;
            }
        }
    }
    if allowed("flatpak") {
        let mut flatpak = Flatpak::new(NativeTransport {
            host: Host::current(),
            authorization,
        });
        // Like every other optional manager, an absent Flatpak stays out of
        // automatic queries instead of failing each one; explicit selections
        // and discovery still report its status.
        let status = flatpak.detect(cancel);
        if discover || explicit || !matches!(status, Ok(Availability::Unavailable(_))) {
            engine.note_detected("flatpak".into(), status);
            engine.register(flatpak)?;
        }
    }
    for (id, make) in [
        (
            "docker",
            Container::docker as fn(NativeTransport) -> Container<NativeTransport>,
        ),
        (
            "podman",
            Container::podman as fn(NativeTransport) -> Container<NativeTransport>,
        ),
    ] {
        if allowed(id) {
            let mut backend = make(NativeTransport {
                host: Host::current(),
                authorization,
            })
            .with_remote_offers(explicit && sources.len() == 1);
            let status = backend.detect(cancel);
            if discover || explicit || !matches!(status, Ok(Availability::Unavailable(_))) {
                engine.note_detected(id.into(), status);
                engine.register(backend)?;
            }
        }
    }
    for (id, make) in [
        (
            "cargo",
            DevTool::cargo as fn(NativeTransport) -> DevTool<NativeTransport>,
        ),
        (
            "npm",
            DevTool::npm as fn(NativeTransport) -> DevTool<NativeTransport>,
        ),
        (
            "pnpm",
            DevTool::pnpm as fn(NativeTransport) -> DevTool<NativeTransport>,
        ),
        (
            "bun",
            DevTool::bun as fn(NativeTransport) -> DevTool<NativeTransport>,
        ),
        (
            "pip",
            DevTool::pip as fn(NativeTransport) -> DevTool<NativeTransport>,
        ),
        (
            "pipx",
            DevTool::pipx as fn(NativeTransport) -> DevTool<NativeTransport>,
        ),
        (
            "uv",
            DevTool::uv as fn(NativeTransport) -> DevTool<NativeTransport>,
        ),
        (
            "composer",
            DevTool::composer as fn(NativeTransport) -> DevTool<NativeTransport>,
        ),
        (
            "gem",
            DevTool::gem as fn(NativeTransport) -> DevTool<NativeTransport>,
        ),
    ] {
        if allowed(id) {
            let mut tool = make(NativeTransport {
                host: Host::current(),
                authorization,
            });
            if discover
                || explicit
                || !matches!(tool.detect(cancel), Ok(Availability::Unavailable(_)))
            {
                engine.register(tool)?;
            }
        }
    }
    Ok(engine)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apt_helper_supports_cargo_builds_and_relocated_bundles() {
        let root = std::env::temp_dir().join(format!("pkgdeck-apt-helper-{}", std::process::id()));
        std::fs::create_dir_all(root.join("bundle")).unwrap();
        let executable = root.join("bundle/pkgdeck");
        let built = root.join("cargo-helper");
        let adjacent = root.join("bundle/pkgdeck-apt-query");
        assert!(matches!(
            apt_query_executable(&executable, None),
            Err(ExecutionError::Disabled(message)) if message.contains("APT helper is missing")
        ));
        assert!(apt_query_executable(&executable, built.to_str()).is_err());
        std::fs::write(&built, "synthetic helper").unwrap();
        assert_eq!(
            apt_query_executable(&executable, built.to_str()).unwrap(),
            built
        );
        std::fs::write(&adjacent, "packaged helper").unwrap();
        assert_eq!(
            apt_query_executable(&executable, built.to_str()).unwrap(),
            adjacent
        );
        std::fs::remove_file(&built).unwrap();
        assert_eq!(
            apt_query_executable(&executable, built.to_str()).unwrap(),
            adjacent
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wave_five_name_policies() {
        for name in [
            "cowsay", "requests", "Pillow", "foo-bar", "foo_bar", "foo.bar", "a", "a1",
        ] {
            assert!(python_name(name), "{name}");
            assert!(gem_name(name), "{name}");
        }
        for name in [
            "",
            "-evil",
            "evil-",
            "evil_",
            "evil.",
            "../evil",
            "evil/thing",
            "https://example.invalid/tool.tar.gz",
            "name@latest",
            "name==1.0",
            "name;evil",
            "white space",
            "évil",
        ] {
            assert!(!python_name(name), "{name}");
            assert!(!gem_name(name), "{name}");
        }
        assert!(python_name(&"a".repeat(214)));
        assert!(!python_name(&"a".repeat(215)));
        for name in ["psr/log", "monolog/monolog", "vendor123/pkg-name_1.2"] {
            assert!(composer_name(name), "{name}");
        }
        for name in [
            "",
            "monolog",
            "/package",
            "vendor/",
            "Vendor/Package",
            "-vendor/package",
            "vendor//package",
            "vendor/pack age",
            "vendor/package/extra",
        ] {
            assert!(!composer_name(name), "{name}");
        }
        assert!(!dev_name(""));
        assert!(!dev_name("@scope"));
        assert!(!dev_name("@scope/tool/extra"));
        assert!(dev_name("@scope/tool"));
        assert!(!dev_name("."));
        assert!(!dev_name(".."));
    }

    #[test]
    fn wave_five_parsers() {
        assert_eq!(
            DevTool::<NativeTransport>::uv_entry("cowsay v6.0"),
            Some(("cowsay".into(), "6.0".into(), None))
        );
        assert_eq!(
            DevTool::<NativeTransport>::uv_entry("cowsay v6.0 [latest: 6.1]"),
            Some(("cowsay".into(), "6.0".into(), Some("6.1".into())))
        );
        for line in [
            "", "   ", "-", "- cowsay", "nodash", "name ", "name v", "name 6.0",
        ] {
            assert_eq!(DevTool::<NativeTransport>::uv_entry(line), None, "{line}");
        }
        assert_eq!(
            DevTool::<NativeTransport>::gem_filename("cowsay-0.3.0.gemspec"),
            Some(("cowsay".into(), "0.3.0".into()))
        );
        assert_eq!(
            DevTool::<NativeTransport>::gem_filename("foo-bar-1.0.gemspec"),
            Some(("foo-bar".into(), "1.0".into()))
        );
        for file in [
            "nodash.gemspec",
            "README",
            "name-.gemspec",
            "name-x.gemspec",
            "-1.0.gemspec",
        ] {
            assert_eq!(
                DevTool::<NativeTransport>::gem_filename(file),
                None,
                "{file}"
            );
        }
        assert_eq!(
            DevTool::<NativeTransport>::gem_outdated("cowsay (0.3.0 < 0.4.0)"),
            Some(("cowsay".into(), "0.4.0".into()))
        );
        assert_eq!(
            DevTool::<NativeTransport>::gem_outdated("rake (13.0.0, 13.3.0 < 13.4.2)"),
            Some(("rake".into(), "13.4.2".into()))
        );
        for line in [
            "",
            "cowsay",
            "cowsay (0.3.0)",
            "--evil (1.0 < 2.0)",
            "-x (1 < 2)",
        ] {
            assert_eq!(
                DevTool::<NativeTransport>::gem_outdated(line),
                None,
                "{line}"
            );
        }
        use std::cmp::Ordering::*;
        assert_eq!(
            DevTool::<NativeTransport>::gem_version_cmp("1.0", "1.0"),
            Equal
        );
        assert_eq!(
            DevTool::<NativeTransport>::gem_version_cmp("0.2.0", "0.3.0"),
            Less
        );
        assert_eq!(
            DevTool::<NativeTransport>::gem_version_cmp("13.4.2", "13.0.0"),
            Greater
        );
        assert_eq!(
            DevTool::<NativeTransport>::gem_version_cmp("1.0", "1.0.0"),
            Less
        );
        assert_eq!(
            DevTool::<NativeTransport>::gem_version_cmp("1.0.0", "1.0"),
            Greater
        );
        assert_eq!(
            DevTool::<NativeTransport>::gem_version_cmp("1.0.a", "1.0.b"),
            Less
        );
    }

    #[test]
    fn component_stems_share_one_namespace() {
        assert_eq!(
            component_stem("firefox.desktop"),
            Some("firefox".to_string())
        );
        assert_eq!(
            component_stem("org.mozilla.firefox.desktop"),
            Some("org.mozilla.firefox".to_string())
        );
        assert_eq!(
            component_stem("org.mozilla.firefox"),
            Some("org.mozilla.firefox".to_string())
        );
        assert_eq!(component_stem("plain"), Some("plain".to_string()));
        assert_eq!(component_stem(""), None);
        assert_eq!(component_stem(".desktop"), None);
        assert_eq!(component_stem("   "), None);
    }

    #[test]
    fn dep11_maps_packages_to_component_stems() {
        use std::io::Write;
        let base = std::env::temp_dir().join(format!("pkgdeck-dep11-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(
                b"---\nFile: DEP-11\nVersion: '1.0'\n---\nType: desktop-application\nID: org.mozilla.firefox.desktop\nPackage: firefox\n---\nType: desktop-application\nID: firefox.desktop\nPackage: firefox\n---\nType: desktop-application\nID: org.chromium.Chromium.desktop\nPackage: chromium\nDescription:\n  C: >-\n    ID: not-a-component\n---\nType: desktop-application\nID: .desktop\nPackage: emptyid\n",
            )
            .unwrap();
        std::fs::write(
            base.join("test_dep11_Components-amd64.yml.gz"),
            encoder.finish().unwrap(),
        )
        .unwrap();
        // Non-gzipped and corrupt files never contribute.
        std::fs::write(base.join("notes.txt"), "ID: fake.desktop\nPackage: fake\n").unwrap();
        std::fs::write(base.join("broken.yml.gz"), b"not gzip data").unwrap();
        let map = dep11_component_ids(&base);
        assert_eq!(
            map.get("firefox"),
            Some(&vec![
                "firefox".to_string(),
                "org.mozilla.firefox".to_string()
            ])
        );
        assert_eq!(
            map.get("chromium"),
            Some(&vec!["org.chromium.Chromium".to_string()])
        );
        assert!(!map.contains_key("fake"));
        assert!(!map.contains_key("emptyid"));
        assert!(dep11_component_ids(&base.join("missing")).is_empty());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn snap_desktop_ids_list_gui_entries() {
        let base = std::env::temp_dir().join(format!("pkgdeck-snapids-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let gui = base.join("snap/spot/current/meta/gui");
        std::fs::create_dir_all(&gui).unwrap();
        std::fs::write(gui.join("spot.desktop"), "[Desktop Entry]\n").unwrap();
        std::fs::write(gui.join("spot-settings.desktop"), "[Desktop Entry]\n").unwrap();
        std::fs::write(gui.join("icon.png"), "png").unwrap();
        assert_eq!(
            snap_desktop_ids(&base, "spot"),
            vec!["spot".to_string(), "spot-settings".to_string()]
        );
        assert!(snap_desktop_ids(&base, "missing").is_empty());
        assert!(snap_desktop_ids(&base, "../evil").is_empty());
        assert!(snap_desktop_ids(&base, "").is_empty());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn local_icons_resolve_without_network_access() {
        let base = std::env::temp_dir().join(format!("pkgdeck-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // Snap layout: <sysroot>/snap/<name>/current/meta/gui/icon.<ext>.
        let gui = base.join("snap/hello/current/meta/gui");
        std::fs::create_dir_all(&gui).unwrap();
        std::fs::write(gui.join("icon.svg"), "<svg/>").unwrap();
        assert_eq!(snap_icon(&base, "hello"), Some(gui.join("icon.svg")));
        assert_eq!(snap_icon(&base, "missing"), None);
        assert_eq!(snap_icon(&base, "../evil"), None);
        assert_eq!(snap_icon(&base, ""), None);
        // Flatpak layout: <exports>/share/icons/hicolor/<size>/apps/<id>.<ext>.
        let exports = base.join("exports");
        let apps = exports.join("share/icons/hicolor/64x64/apps");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(apps.join("org.example.App.png"), "png").unwrap();
        assert_eq!(
            flatpak_icon(std::slice::from_ref(&exports), "org.example.App"),
            Some(apps.join("org.example.App.png"))
        );
        assert_eq!(
            flatpak_icon(std::slice::from_ref(&exports), "missing.app"),
            None
        );
        assert_eq!(flatpak_icon(&[exports], "--evil"), None);
        assert_eq!(flatpak_icon(&[], "org.example.App"), None);
        // Debian layout: <info>/<pkg>[_<arch>].list naming desktop entries.
        let info = base.join("info");
        std::fs::create_dir_all(&info).unwrap();
        std::fs::write(
            info.join("brave-browser.list"),
            "/usr/bin/brave\n/usr/share/applications/brave-browser.desktop\n",
        )
        .unwrap();
        std::fs::write(info.join("plain.list"), "/usr/bin/plain\n").unwrap();
        // Non-list files never name a package, and duplicate entries for
        // one package resolve exactly once.
        std::fs::write(info.join("README"), "inventory\n").unwrap();
        std::fs::write(
            info.join("brave-browser:amd64.list"),
            "/usr/bin/brave\n/usr/share/applications/other.desktop\n",
        )
        .unwrap();
        let map = apt_desktop_map(&info);
        // Either list may win the readdir order, but duplicates resolve once.
        let brave = map.get("brave-browser").unwrap();
        assert!(
            brave.ends_with("brave-browser.desktop") || brave.ends_with("other.desktop"),
            "{brave:?}"
        );
        assert_eq!(
            map.values()
                .filter(|p| {
                    p.ends_with("brave-browser.desktop") || p.ends_with("other.desktop")
                })
                .count(),
            1
        );
        assert!(!map.contains_key("plain"));
        assert!(!map.contains_key("README"));
        assert_eq!(
            map.values()
                .filter(|p| p.ends_with("brave-browser.desktop"))
                .count()
                + map
                    .values()
                    .filter(|p| p.ends_with("other.desktop"))
                    .count(),
            1
        );
        // A bare Icon= name resolves through the theme; absolute paths
        // resolve directly when the file exists.
        let real = base.join("real-icon.png");
        std::fs::write(&real, "png").unwrap();
        let desktop = base.join("myapp.desktop");
        std::fs::write(
            &desktop,
            format!("[Desktop Entry]\nName=Mine\nIcon={}\n", real.display()),
        )
        .unwrap();
        assert_eq!(desktop_icon(None, &desktop), Some(real));
        std::fs::write(&desktop, "[Desktop Entry]\nName=Mine\n").unwrap();
        assert_eq!(desktop_icon(None, &desktop), None);
        std::fs::write(&desktop, "[Desktop Entry]\nName=Mine\nIcon=\n").unwrap();
        assert_eq!(desktop_icon(None, &desktop), None);
        std::fs::write(
            &desktop,
            "[Desktop Entry]\nName=Mine\nIcon=/nowhere/icon.png\n",
        )
        .unwrap();
        assert_eq!(desktop_icon(None, &desktop), None);
        // Relative names with a slash never touch the filesystem.
        std::fs::write(&desktop, "[Desktop Entry]\nName=Mine\nIcon=subdir/icon\n").unwrap();
        assert_eq!(desktop_icon(None, &desktop), None);
        // Unique names miss every theme directory including pixmaps.
        std::fs::write(
            &desktop,
            "[Desktop Entry]\nName=Mine\nIcon=pkgdeck-definitely-missing-icon\n",
        )
        .unwrap();
        assert_eq!(desktop_icon(None, &desktop), None);
        // The user theme directory wins over the system ones.
        let home = base.join("home");
        let theme = home.join(".local/share/icons/hicolor/48x48/apps");
        std::fs::create_dir_all(&theme).unwrap();
        std::fs::write(theme.join("pkgdeck-fixture-icon.png"), "png").unwrap();
        std::fs::write(
            &desktop,
            "[Desktop Entry]\nName=Mine\nIcon=pkgdeck-fixture-icon\n",
        )
        .unwrap();
        assert_eq!(
            desktop_icon(Some(&home), &desktop),
            Some(theme.join("pkgdeck-fixture-icon.png"))
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
}
