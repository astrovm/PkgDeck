//! Flatpak apps and runtimes, user and system installations.
use super::*;

pub struct Flatpak<T = NativeTransport> {
    pub(super) transport: T,
    /// Remotes the last installed query could not ask about updates. The
    /// rest were asked, so their rows are complete.
    skipped: Vec<EngineError>,
    /// Set during an update check, which asks the remotes again. Other
    /// listings, such as Installed, use the summaries Flatpak already has.
    update_check: Option<u64>,
}
fn flatpak_offer_reference(reference: &str) -> Option<(&str, &str, &str)> {
    let mut parts = reference.split('/');
    let (name, arch, branch) = (parts.next()?, parts.next()?, parts.next()?);
    (flatpak_id(name) && flatpak_id(arch) && flatpak_id(branch) && parts.next().is_none())
        .then_some((name, arch, branch))
}
impl<T: Transport> Flatpak<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            skipped: vec![],
            update_check: None,
        }
    }
}

pub(super) fn flatpak_id(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
pub(super) fn flatpak_reference(reference: &str) -> Option<(&str, &str, &str, &str)> {
    let mut parts = reference.split('/');
    let (kind, name, arch, branch) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    (matches!(kind, "app" | "runtime")
        && flatpak_id(name)
        && flatpak_id(arch)
        && flatpak_id(branch)
        && parts.next().is_none())
    .then_some((kind, name, arch, branch))
}
/// The answer of a Flatpak query run on its own thread.
fn joined<R>(worker: std::thread::ScopedJoinHandle<'_, R>) -> R {
    worker.join().expect("Flatpak worker panicked")
}
// User and system installations are separate, and each `flatpak` read is
// its own process, so their queries run at once.
impl<T: Transport + Sync> Flatpak<T> {
    pub(super) fn call(
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
    /// Remotes Flatpak made for one bundle or local build: disabled, or
    /// never listed. An unreadable list leaves none out.
    fn local_remotes(
        &self,
        prefix: &str,
        cancel: &Cancellation,
        system: bool,
    ) -> std::collections::BTreeSet<String> {
        let args = [
            prefix,
            "remotes",
            "--show-disabled",
            "--columns=name,options",
        ];
        let Ok(output) = self.call(&args, cancel, false, system) else {
            return Default::default();
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .filter(|(_, options)| {
                options
                    .split(',')
                    .any(|option| matches!(option.trim(), "disabled" | "no-enumerate"))
            })
            .map(|(name, _)| name.trim().to_owned())
            .filter(|name| flatpak_id(name))
            .collect()
    }
    fn list(
        &self,
        cancel: &Cancellation,
        system: bool,
        updates: bool,
        skipped: &mut Vec<EngineError>,
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
                    "--columns=application,arch,branch,version,description,origin,options,name",
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
            if !(6..=8).contains(&fields.len())
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
                display_name: fields
                    .get(7)
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(&fields[0])
                    .trim()
                    .into(),
                summary: if runtime {
                    format!("Runtime {} ({})", fields[2], fields[4])
                } else {
                    fields[4].into()
                },
                installed_version: Some(fields[3].into()),
                candidate_version: Some(fields[3].into()),
                update: UpdateAvailability::Current,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
                adopt_with: None,
            });
        }
        if packages.is_empty() || !updates {
            return Ok(packages);
        }
        // `remote-ls` reads versions from the AppStream data Flatpak keeps,
        // which it refreshes about once a day. An update check fetches it
        // first, or a new build shows the version of an older one. A failure
        // only leaves the labels as they were.
        if self.update_check.is_some() && !cancel.requested() {
            let _ = self.call(
                &[prefix, "update", "--appstream", "--noninteractive"],
                cancel,
                false,
                system,
            );
        }
        // Flatpak compares commits, not version labels: rebuilds and runtimes
        // can have an update even when the version is unchanged or empty.
        let remote_updates = |remote: Option<&str>| -> Result<String, EngineError> {
            let ask = |cached: bool| -> Result<String, EngineError> {
                let mut args = vec![
                    prefix,
                    "remote-ls",
                    "--updates",
                    "--columns=ref,version,origin",
                ];
                if cached {
                    args.push("--cached");
                }
                args.extend(remote);
                let output = bytes("flatpak", self.call(&args, cancel, false, system)?)?;
                String::from_utf8(output).map_err(|e| invalid("flatpak", e))
            };
            // Outside an update check, use the summaries Flatpak already has.
            // A remote it never fetched has none, so that one is asked.
            match self.update_check {
                None => ask(true).or_else(|_| ask(false)),
                Some(_) => ask(false),
            }
        };
        let mut remotes: std::collections::BTreeSet<_> = origins
            .iter()
            .filter(|origin| !origin.is_empty())
            .map(String::as_str)
            .collect();
        let mut texts = Vec::new();
        let first = remote_updates(None);
        // A bundle or a local build installs from a remote of its own that
        // Flatpak disables or never lists. Nothing can be asked there, and
        // flatpak refuses to list any remote while one is.
        let local = if first.is_err() && !cancel.requested() {
            self.local_remotes(prefix, cancel, system)
        } else {
            Default::default()
        };
        let installed_from = remotes.len();
        remotes.retain(|remote| !local.contains(*remote));
        match first {
            Ok(text) => texts.push(text),
            // Nothing left to ask: what's installed came from bundles or
            // local builds only.
            Err(_) if remotes.is_empty() && installed_from > 0 && !cancel.requested() => {}
            // One unreachable remote makes flatpak refuse to list any. Ask
            // each remote on its own so the others still show their updates.
            Err(first)
                if !cancel.requested() && (remotes.len() > 1 || remotes.len() < installed_from) =>
            {
                let remote_updates = &remote_updates;
                let answers: Vec<_> = std::thread::scope(|scope| {
                    let workers: Vec<_> = remotes
                        .into_iter()
                        .map(|remote| scope.spawn(move || remote_updates(Some(remote))))
                        .collect();
                    workers.into_iter().map(joined).collect()
                });
                let mut failures = Vec::new();
                for answer in answers {
                    match answer {
                        Ok(text) => texts.push(text),
                        Err(error) => failures.push(error),
                    }
                }
                if texts.is_empty() || cancel.requested() {
                    return Err(first);
                }
                skipped.extend(failures);
            }
            Err(error) => return Err(error),
        }
        for updates in texts {
            for line in updates.lines().filter(|line| !line.trim().is_empty()) {
                let fields: Vec<_> = line.split('\t').collect();
                if fields.len() != 3
                    || flatpak_reference(fields[0]).is_none()
                    || !flatpak_id(fields[2])
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
        let output = String::from_utf8(output).map_err(|e| invalid("flatpak", e))?;
        // Without matches flatpak prints a translated notice such as
        // "No matches found" instead of rows. Every real row has tabs.
        if !output.contains('\t') {
            return Ok(vec![]);
        }
        output
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
                    adopt_with: None,
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
impl<T: Transport + Sync> Backend for Flatpak<T> {
    fn id(&self) -> &str {
        "flatpak"
    }
    /// Flatpak ids are reverse-DNS names with at least one dot, and refs
    /// contain slashes, so a plain name such as `cowsay` never matches.
    fn may_have(&self, name: &str) -> bool {
        (name.contains('.') || name.contains('/'))
            && name
                .split('/')
                .all(|part| part.is_empty() || flatpak_id(part))
    }
    fn capabilities(&self) -> &[Capability] {
        if self.transport.supports_flatpak_cleanup() {
            CLEAN_CAPABILITIES
        } else {
            CAPABILITIES
        }
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
    fn query_errors(&self) -> Vec<EngineError> {
        self.skipped.clone()
    }
    fn arm_update_check(&mut self, token: Option<u64>) {
        self.update_check = token;
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        if query.trim().is_empty() || query.starts_with('-') {
            return Err(invalid("flatpak", "expected a search term"));
        }
        self.skipped.clear();
        if flatpak_reference(query).is_some() {
            return self.installed(cancel).map(|packages| {
                packages
                    .into_iter()
                    .filter(|package| package.id.reference.as_deref() == Some(query))
                    .collect()
            });
        }
        // Local inventory is enough to choose Install versus Remove. Search
        // must not fetch remote update metadata just to establish this state.
        // Both catalogs and both inventories are read at once.
        let this = &*self;
        let (user, system, user_installed, system_installed) = std::thread::scope(|scope| {
            let system = scope.spawn(|| this.search_scope(query, cancel, true));
            let user_installed = scope.spawn(|| this.list(cancel, false, false, &mut vec![]));
            let system_installed = scope.spawn(|| this.list(cancel, true, false, &mut vec![]));
            let user = this.search_scope(query, cancel, false);
            (
                user,
                joined(system),
                joined(user_installed),
                joined(system_installed),
            )
        });
        // The user catalog is the primary source; a failing system query
        // (for example, a machine with no system remotes) must not fail
        // the whole search when user results are available.
        let mut result = user?;
        if let Ok(system) = system {
            result.extend(system);
        }
        // Offers in separate installations remain independently selectable.
        let mut seen = std::collections::BTreeSet::new();
        result.retain(|package| seen.insert(package.id.clone()));
        let mut installed = user_installed?;
        installed.extend(system_installed?);
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
        let this = &*self;
        let (user, system) = std::thread::scope(|scope| {
            let system = scope.spawn(|| {
                let mut skipped = Vec::new();
                this.list(cancel, true, true, &mut skipped)
                    .map(|packages| (packages, skipped))
            });
            let mut skipped = Vec::new();
            let user = this
                .list(cancel, false, true, &mut skipped)
                .map(|packages| (packages, skipped));
            (user, joined(system))
        });
        let (mut result, mut skipped) = user?;
        let (system, system_skipped) = system?;
        result.extend(system);
        skipped.extend(system_skipped);
        self.skipped = skipped;
        for package in &mut result {
            // Exported icons double as the installed check per scope; `list`
            // only reports user and system installations.
            let root = if package.id.scope == Scope::System {
                Some(PathBuf::from("/var/lib/flatpak/exports"))
            } else {
                home.as_ref()
                    .map(|home| home.join(".local/share/flatpak/exports"))
            };
            if let Some(root) = root {
                package.icon = flatpak_icon(&[root], &package.id.name);
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
        if !self.transport.supports_flatpak_cleanup() {
            return Err(self.unsupported(Capability::Clean));
        }
        self.cleanup_unused(cancel)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        if let Operation::Install(id) = operation {
            if id
                .reference
                .as_deref()
                .is_some_and(|value| value.starts_with("artifact:flatpak:"))
            {
                // The scope is checked before the bundle is fetched.
                let (system, scope) = match id.scope {
                    Scope::System => (true, "--system"),
                    Scope::User { uid } if uid == rustix::process::getuid().as_raw() => {
                        (false, "--user")
                    }
                    _ => return Err(invalid("flatpak", "invalid Flatpak scope")),
                };
                let missing = || invalid("flatpak", "missing bundle");
                let staged = crate::artifact::stage(id, cancel)?.ok_or_else(missing)?;
                progress(Progress::Message(format!(
                    "Installing Flatpak bundle {}…",
                    id.name
                )));
                // A bundle replaces an installed copy, newer or not, so its
                // page can offer Update, Reinstall or Downgrade.
                let result = self.call(
                    &[
                        scope,
                        "install",
                        "--noninteractive",
                        "--assumeyes",
                        "--reinstall",
                        "--bundle",
                        &staged.path().to_string_lossy(),
                    ],
                    cancel,
                    true,
                    system,
                )?;
                return Ok(OperationOutcome {
                    cancellation_deferred: result.cancellation_deferred,
                });
            }
            if let Some(bytes) = crate::flatpak_ref::verified_source(id, cancel)? {
                // Verification accepts only system and current-user scopes.
                let (system, scope) = if id.scope == Scope::System {
                    (true, "--system")
                } else {
                    (false, "--user")
                };
                progress(Progress::Message(format!(
                    "Installing Flatpak reference for {}…",
                    id.name
                )));
                let temporary = std::env::temp_dir().join(format!(
                    "pkgdeck-flatpakref-{}-{}.flatpakref",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                ));
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)
                    .map_err(|e| invalid("flatpak", e))?;
                use std::io::Write;
                let written = file.write_all(&bytes);
                drop(file);
                let source = temporary.to_string_lossy().into_owned();
                let result = written
                    .map_err(|error| invalid("flatpak", error))
                    .and_then(|()| {
                        self.call(
                            &[
                                scope,
                                "install",
                                "--noninteractive",
                                "--assumeyes",
                                "--reinstall",
                                "--from",
                                &source,
                            ],
                            cancel,
                            true,
                            system,
                        )
                    });
                let _ = std::fs::remove_file(&temporary);
                let result = result?;
                return Ok(OperationOutcome {
                    cancellation_deferred: result.cancellation_deferred,
                });
            }
        }
        if let Operation::Clean(id) = operation {
            if !self.transport.supports_flatpak_cleanup() {
                return Err(self.unsupported(Capability::Clean));
            }
            return self.clean_unused(id, cancel);
        }
        let (id, verb) = match operation {
            Operation::Refresh { backend } if backend == "flatpak" => {
                // Appstream metadata only: no app is downloaded or changed. The
                // system scope runs as the user because this is not privileged.
                for scope in ["--user", "--system"] {
                    self.call(
                        &[scope, "update", "--appstream", "--noninteractive"],
                        cancel,
                        true,
                        false,
                    )?;
                }
                return Ok(OperationOutcome::default());
            }
            Operation::Install(id) => (id, "install"),
            Operation::Remove(id) => (id, "uninstall"),
            Operation::Upgrade(id) => (id, "update"),
            Operation::UpgradeAll { backend } if backend == "flatpak" => {
                for (system, scope) in [(false, "--user"), (true, "--system")] {
                    self.call(
                        &[scope, "update", "--noninteractive", "--assumeyes"],
                        cancel,
                        true,
                        system,
                    )?;
                }
                progress(Progress::Message("Updated Flatpak packages.".into()));
                return Ok(OperationOutcome::default());
            }
            _ => return Err(invalid("flatpak", "foreign operation")),
        };
        let (system, scope) = self.target(id)?;
        progress(Progress::Message(format!(
            "Running Flatpak {verb} for {}…",
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
