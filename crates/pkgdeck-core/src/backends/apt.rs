//! APT: Debian and Ubuntu packages, read with apt-cache and changed through the host runner.
use super::*;

pub struct Apt<T = NativeTransport> {
    pub transport: T,
    pub(super) desktop_entries: Option<std::collections::BTreeMap<String, PathBuf>>,
    pub(super) components: Option<std::collections::BTreeMap<String, Vec<String>>>,
    /// What the last exact lookup read, in full. The details that follow it
    /// (`pkd info`) come from here instead of a second helper run. Read
    /// once; changes clear it.
    looked_up: Vec<PackageDetails>,
    /// Set during an update check, which refreshes the lists first.
    update_check: Option<u64>,
}
impl<T> Apt<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            desktop_entries: None,
            components: None,
            looked_up: vec![],
            update_check: None,
        }
    }
}

pub(super) fn parse_apt_upgrade_plan(output: &[u8]) -> Result<AptUpgradePlan, EngineError> {
    let preview = std::str::from_utf8(output)
        .map_err(|e| invalid("apt", e))?
        .trim();
    let summary = preview
        .lines()
        .find(|line| {
            line.contains(" upgraded, ")
                && line.contains(" newly installed, ")
                && line.contains(" to remove")
        })
        .ok_or_else(|| invalid("apt", "simulation did not include a transaction summary"))?;
    let counts: Vec<_> = summary
        .split(',')
        .take(3)
        .map(|part| {
            part.split_whitespace()
                .next()
                .and_then(|n| n.parse::<usize>().ok())
        })
        .collect();
    let [Some(upgrades), Some(installs), Some(removals)] = counts.as_slice() else {
        return Err(invalid("apt", "invalid simulation counts"));
    };
    let mut plan = AptUpgradePlan {
        preview: preview.into(),
        upgrades: Vec::new(),
        installs: Vec::new(),
        removals: Vec::new(),
    };
    for line in preview.lines() {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("Inst") => {
                let name = fields
                    .next()
                    .ok_or_else(|| invalid("apt", "incomplete install action"))?;
                let existing = fields.next().is_some_and(|field| field.starts_with('['));
                if existing {
                    plan.upgrades.push(name.into());
                } else {
                    plan.installs.push(name.into());
                }
            }
            Some("Remv" | "Purg") => {
                let name = fields
                    .next()
                    .ok_or_else(|| invalid("apt", "incomplete removal action"))?;
                plan.removals.push(name.into());
            }
            _ => {}
        }
    }
    if plan.upgrades.len() != *upgrades
        || plan.installs.len() != *installs
        || plan.removals.len() != *removals
    {
        return Err(invalid(
            "apt",
            "simulation actions did not match its summary",
        ));
    }
    Ok(plan)
}
pub(super) fn parse_apt_transaction_plan(
    operation: &Operation,
    output: &[u8],
) -> Result<TransactionPlan, EngineError> {
    let parsed = parse_apt_upgrade_plan(output)?;
    let mut changes = Vec::new();
    for line in parsed.preview.lines() {
        let mut fields = line.split_whitespace();
        let kind = fields.next();
        let Some(name) = fields.next() else { continue };
        let action = match kind {
            Some("Inst") if parsed.upgrades.iter().any(|item| item == name) => {
                PlannedAction::Upgrade
            }
            Some("Inst") => PlannedAction::Install,
            Some("Remv" | "Purg") => PlannedAction::Remove,
            _ => continue,
        };
        let tokens: Vec<_> = fields.collect();
        let installed_version = tokens
            .iter()
            .find(|field| field.starts_with('['))
            .map(|field| field.trim_matches(['[', ']']).to_owned());
        let candidate_version = if matches!(action, PlannedAction::Remove) {
            None
        } else {
            tokens
                .iter()
                .find(|field| field.starts_with('('))
                .map(|field| {
                    field
                        .trim_start_matches('(')
                        .trim_end_matches(')')
                        .to_owned()
                })
        };
        changes.push(PlannedChange {
            action,
            name: name.into(),
            installed_version,
            candidate_version,
        });
    }
    Ok(TransactionPlan {
        operation: operation.clone(),
        native_preview: parsed.preview,
        changes,
        download_bytes: None,
        disk_bytes: None,
        restart_required: None,
        adopts: None,
    })
}

impl<T: Transport> Apt<T> {
    fn simulated_operation(
        &self,
        operation: &Operation,
        cancel: &Cancellation,
    ) -> Result<Option<TransactionPlan>, EngineError> {
        let (verb, id) = match operation {
            Operation::Install(id) | Operation::Upgrade(id) => ("install", id),
            Operation::Remove(id) => ("remove", id),
            _ => return Ok(None),
        };
        let artifact = if matches!(operation, Operation::Install(_)) {
            crate::artifact::stage(id, cancel)?
        } else {
            None
        };
        let target = if let Some(staged) = artifact.as_ref() {
            staged.path().display().to_string()
        } else if matches!(operation, Operation::Install(_)) {
            crate::local_deb::verified_path(id, cancel)?
                .map_or_else(|| self.target(id), |path| Ok(path.display().to_string()))?
        } else {
            self.target(id)?
        };
        let mut args = self.simulate_args();
        args.extend([OsString::from(verb), OsString::from(target)]);
        let output = bytes(
            "apt",
            self.transport
                .system_manager("apt-get", &args, cancel, false)?,
        )?;
        parse_apt_transaction_plan(operation, &output).map(Some)
    }
    fn simulated_upgrade(&self, cancel: &Cancellation) -> Result<AptUpgradePlan, EngineError> {
        let mut args = self.simulate_args();
        args.push("dist-upgrade".into());
        let output = bytes(
            "apt",
            self.transport
                .system_manager("apt-get", &args, cancel, false)?,
        )?;
        parse_apt_upgrade_plan(&output)
    }
    /// A dry run reads a check's own copy of the lists while it's newer,
    /// like every other APT query. The change itself refreshes the
    /// system's lists first (see `prepare`).
    fn simulate_args(&self) -> Vec<OsString> {
        let mut args = ["--simulate", "-o", "Debug::NoLocking=1"]
            .map(OsString::from)
            .to_vec();
        if let Some(dir) = self.transport.private_index("apt") {
            args.push("-o".into());
            args.push(format!("Dir::State::Lists={}", dir.join("lists").display()).into());
        }
        args
    }
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
    fn has_update_index(&self) -> bool {
        true
    }
    fn arm_update_check(&mut self, token: Option<u64>) {
        self.update_check = token;
    }
    fn refresh_update_index(&mut self, cancel: &Cancellation) -> Result<(), EngineError> {
        if self.update_check.is_none() || self.refresh_without_password(cancel)? {
            return Ok(());
        }
        // Without PackageKit, the check fills its own copy of the lists.
        let Some(copy) = self.transport.prepare_private_index("apt", cancel)? else {
            return Ok(());
        };
        self.looked_up.clear();
        let args = crate::private_index::apt_refresh_args(&copy.dir);
        bytes(
            "apt",
            self.transport.refresh_index("apt-get", &args, cancel)?,
        )
        .map(|_| ())
    }
    fn refresh_without_password(&mut self, cancel: &Cancellation) -> Result<bool, EngineError> {
        let refreshed = self.transport.packagekit_refresh("apt", cancel)?;
        if refreshed {
            self.looked_up.clear();
        }
        Ok(refreshed)
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
    /// Every architecture of one exact name, with the full details search
    /// rows leave out, kept for the details call that usually follows.
    fn lookup(&mut self, name: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.looked_up.clear();
        // A name APT can't have is never sent to it.
        if name.contains(':') || AptAction::Install(name.into()).arguments().is_err() {
            return Ok(vec![]);
        }
        let found = self.query("lookup", name, "", cancel)?;
        let home = self.transport.env("HOME").map(PathBuf::from);
        let packages = found
            .iter()
            .map(|d| {
                let mut package = d.package.clone();
                if let Some(desktop) = self.desktop_entries().get(&package.id.name).cloned() {
                    package.icon = desktop_icon(home.as_deref(), &desktop);
                }
                package
            })
            .collect();
        self.looked_up = found;
        Ok(packages)
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
        let looked_up = std::mem::take(&mut self.looked_up)
            .into_iter()
            .find(|d| d.package.id == *id);
        let mut details = match looked_up {
            Some(details) => details,
            None => self
                .query("details", &id.name, &id.architecture, cancel)?
                .into_iter()
                .find(|d| d.package.id == *id)
                .ok_or(EngineError::NotFound)?,
        };
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
        self.cleanup_apt_report(cancel)
    }
    fn cleanup_plan(
        &mut self,
        id: &CleanupId,
        cancel: &Cancellation,
    ) -> Result<CleanupItem, EngineError> {
        if id.backend != "apt" {
            return Err(invalid("apt", "foreign cleanup task"));
        }
        self.cleanup_apt_task(&id.key, cancel)?
            .ok_or(EngineError::NotFound)
    }
    fn apt_upgrade_plan(&mut self, cancel: &Cancellation) -> Result<AptUpgradePlan, EngineError> {
        self.simulated_upgrade(cancel)
    }
    fn prepare(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<(), EngineError> {
        if !matches!(
            operation,
            Operation::UpgradeAll { .. } | Operation::Upgrade(_)
        ) {
            return Ok(());
        }
        self.looked_up.clear();
        progress(Progress::Message("Refreshing package lists…".into()));
        self.transport.apt_write(AptAction::Refresh, cancel)?;
        Ok(())
    }
    fn operation_plan(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
    ) -> Result<Option<TransactionPlan>, EngineError> {
        self.simulated_operation(operation, cancel)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        // A change can make what a lookup read out of date.
        self.looked_up.clear();
        let staged_artifact = match operation {
            Operation::Install(id) => crate::artifact::stage(id, cancel)?,
            _ => None,
        };
        let staged = match operation {
            Operation::Install(id) => crate::local_deb::stage(id, cancel)?,
            _ => None,
        };
        let action = match operation {
            Operation::Refresh { backend } if backend == "apt" => AptAction::Refresh,
            Operation::Install(id) => {
                if let Some(archive) = staged_artifact.as_ref() {
                    AptAction::InstallLocal(archive.path().to_owned())
                } else if let Some(archive) = staged.as_ref() {
                    AptAction::InstallLocal(archive.path().to_owned())
                } else {
                    AptAction::Install(self.target(id)?)
                }
            }
            Operation::Remove(id) => AptAction::Remove(self.target(id)?),
            Operation::Upgrade(id) => AptAction::Upgrade(self.target(id)?),
            Operation::UpgradeAll { backend } if backend == "apt" => AptAction::UpgradeAll,
            Operation::Clean(id) if id.backend == "apt" && id.key == "autoremove" => {
                AptAction::Autoremove
            }
            Operation::Clean(id) if id.backend == "apt" && id.key == "clean" => AptAction::Clean,
            _ => return Err(invalid("apt", "foreign operation")),
        };
        progress(Progress::Message(
            "Running apt-get. If you cancel, PkgDeck waits for APT to finish.".into(),
        ));
        let result = self.transport.apt_write(action, cancel)?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
    fn execute_group(
        &mut self,
        operations: &[Operation],
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Option<Result<Vec<OperationOutcome>, EngineError>> {
        if operations.len() < 2 {
            return None;
        }
        self.looked_up.clear();
        let grouped = (|| {
            let actions = operations
                .iter()
                .map(|operation| match operation {
                    Operation::Install(id) => Ok(AptAction::Install(self.target(id)?)),
                    Operation::Remove(id) => Ok(AptAction::Remove(self.target(id)?)),
                    Operation::Upgrade(id) => Ok(AptAction::Upgrade(self.target(id)?)),
                    _ => Err(invalid("apt", "mixed APT batch")),
                })
                .collect::<Result<Vec<_>, EngineError>>()?;
            AptAction::group_arguments(&actions)?;
            progress(Progress::Message(format!(
                "Updating {} APT packages in one transaction…",
                actions.len()
            )));
            let result = self.transport.apt_write_group(&actions, cancel)?;
            Ok(vec![
                OperationOutcome {
                    cancellation_deferred: result.cancellation_deferred
                };
                actions.len()
            ])
        })();
        Some(grouped)
    }
}
