//! Synchronous orchestration for use on a frontend worker. No shell or UI dependencies.
use crate::{
    host::{Authorization, Host},
    package::*,
    process::{Cancellation, ExecutionError},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct BackendFailure {
    pub backend: String,
    pub error: EngineError,
}

#[derive(serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub enum EngineError {
    UnknownBackend(String),
    DuplicateBackend(String),
    Unavailable {
        backend: String,
        reason: String,
    },
    Unsupported {
        backend: String,
        capability: Capability,
    },
    InvalidResponse {
        backend: String,
        reason: String,
    },
    NotFound,
    Ambiguous(Vec<PackageId>),
    Incomplete(Vec<BackendFailure>),
    Cancelled,
    Execution(ExecutionError),
}
impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownBackend(id) => write!(f, "unknown backend: {id}"),
            Self::DuplicateBackend(id) => write!(f, "duplicate backend: {id}"),
            Self::Unavailable { backend, reason } => {
                write!(f, "{backend} is unavailable: {reason}")
            }
            Self::Unsupported {
                backend,
                capability,
            } => write!(f, "{backend} does not support {capability:?}"),
            Self::InvalidResponse { backend, reason } => {
                write!(f, "invalid response from {backend}: {reason}")
            }
            Self::NotFound => f.write_str("no package matches the selection"),
            Self::Ambiguous(ids) => write!(
                f,
                "{} packages match; select a backend, architecture, or scope",
                ids.len()
            ),
            Self::Incomplete(errors) => write!(
                f,
                "selection is incomplete: {} backend queries failed",
                errors.len()
            ),
            Self::Cancelled => f.write_str("operation cancelled"),
            Self::Execution(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Execution(error) => Some(error),
            _ => None,
        }
    }
}
impl From<ExecutionError> for EngineError {
    fn from(error: ExecutionError) -> Self {
        match error {
            ExecutionError::Cancelled => Self::Cancelled,
            error => Self::Execution(error),
        }
    }
}

/// Implementations use documented APIs/structured output and the host boundary for commands.
/// Cancellation must be propagated to reads; native writes retain their deferred-cancel semantics.
pub trait Backend: Send {
    fn id(&self) -> &str;
    fn capabilities(&self) -> &[Capability];
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError>;
    fn search(
        &mut self,
        _query: &str,
        _cancel: &Cancellation,
    ) -> Result<Vec<Package>, EngineError> {
        Err(self.unsupported(Capability::Search))
    }
    fn installed(&mut self, _cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Err(self.unsupported(Capability::Installed))
    }
    fn details(
        &mut self,
        _id: &PackageId,
        _cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        Err(self.unsupported(Capability::Details))
    }
    fn cleanup(&mut self, _cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        Err(self.unsupported(Capability::Clean))
    }
    fn cleanup_report(&mut self, cancel: &Cancellation) -> CleanupReport {
        match self.cleanup(cancel) {
            Ok(items) => CleanupReport {
                items,
                failures: vec![],
            },
            Err(error) => CleanupReport {
                items: vec![],
                failures: vec![BackendFailure {
                    backend: self.id().into(),
                    error,
                }],
            },
        }
    }
    fn cleanup_plan(
        &mut self,
        id: &CleanupId,
        cancel: &Cancellation,
    ) -> Result<CleanupItem, EngineError> {
        self.cleanup(cancel)?
            .into_iter()
            .find(|item| item.id == *id)
            .ok_or(EngineError::NotFound)
    }
    fn apt_upgrade_plan(&mut self, _cancel: &Cancellation) -> Result<AptUpgradePlan, EngineError> {
        Err(EngineError::Unsupported {
            backend: self.id().into(),
            capability: Capability::Upgrade,
        })
    }
    /// Optional native plan. `None` means the manager cannot provide a
    /// reliable preview for this operation.
    fn operation_plan(
        &mut self,
        _operation: &Operation,
        _cancel: &Cancellation,
    ) -> Result<Option<TransactionPlan>, EngineError> {
        Ok(None)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        _cancel: &Cancellation,
        _progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        Err(self.unsupported(operation.capability()))
    }
    /// A manager may combine exact same-verb selections in one native write.
    fn execute_group(
        &mut self,
        _operations: &[Operation],
        _cancel: &Cancellation,
        _progress: &mut dyn FnMut(Progress),
    ) -> Option<Result<Vec<OperationOutcome>, EngineError>> {
        None
    }
    fn unsupported(&self, capability: Capability) -> EngineError {
        EngineError::Unsupported {
            backend: self.id().into(),
            capability,
        }
    }
}

#[derive(serde::Serialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct CleanupReport {
    pub items: Vec<CleanupItem>,
    pub failures: Vec<BackendFailure>,
}

#[derive(serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct Source {
    pub backend: String,
    pub capabilities: Vec<Capability>,
    pub availability: Result<Availability, EngineError>,
}

#[derive(serde::Serialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct PackageReport {
    pub packages: Vec<Package>,
    pub failures: Vec<BackendFailure>,
    /// Backends whose query completed successfully, including empty results.
    #[serde(default)]
    pub successful_sources: Vec<String>,
}
impl PackageReport {
    /// Never selects across an unqueried/failed source or collapses same-name packages.
    pub fn select(&self, selector: &Selector) -> Result<PackageId, EngineError> {
        let failures: Vec<_> = self
            .failures
            .iter()
            .filter(|failure| {
                selector
                    .backend
                    .as_ref()
                    .is_none_or(|backend| backend == &failure.backend)
            })
            .cloned()
            .collect();
        if !failures.is_empty() {
            return Err(EngineError::Incomplete(failures));
        }
        let matches: BTreeSet<_> = self
            .packages
            .iter()
            .map(|p| &p.id)
            .filter(|id| {
                (id.name == selector.name
                    || id.reference.as_deref() == Some(selector.name.as_str()))
                    && selector.backend.as_ref().is_none_or(|b| b == &id.backend)
                    && selector
                        .architecture
                        .as_ref()
                        .is_none_or(|a| a == &id.architecture)
                    && selector.scope.as_ref().is_none_or(|s| s == &id.scope)
            })
            .cloned()
            .collect();
        match matches.len() {
            0 => Err(EngineError::NotFound),
            1 => Ok(matches.into_iter().next().expect("one match")),
            _ => Err(EngineError::Ambiguous(matches.into_iter().collect())),
        }
    }
}

#[derive(serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub enum Event {
    /// Dispatch started, including availability/capability checks; not proof of a write.
    Started(Operation),
    Progress {
        operation: Operation,
        progress: Progress,
    },
    Finished {
        operation: Operation,
        result: Result<OperationOutcome, EngineError>,
    },
}

#[derive(Default)]
pub struct Engine {
    backends: BTreeMap<String, Box<dyn Backend>>,
    /// Successful detections remembered by [`native_engine`](crate::backends::native_engine)
    /// (and friends) so the immediately following query skips its own round.
    /// Failed detections are never cached: the query retries them. Engines
    /// are short-lived per query, so entries cannot go stale.
    detected: BTreeMap<String, Availability>,
    cleanup_plans: BTreeMap<CleanupId, CleanupItem>,
    apt_upgrade_plan: Option<AptUpgradePlan>,
    operation_plan: Option<TransactionPlan>,
    batch_authorization: Option<(Host, Authorization)>,
}
impl Engine {
    /// Native engines use one trusted runner for the protected part of a
    /// confirmed batch when it is installed by the host package.
    pub fn enable_batch_authorization(&mut self, host: Host, authorization: Authorization) {
        self.batch_authorization = Some((host, authorization));
    }
    /// Simulate the host APT solver before asking the user to approve a full update.
    pub fn plan_apt_upgrade(
        &mut self,
        cancel: &Cancellation,
    ) -> Result<AptUpgradePlan, EngineError> {
        self.apt_upgrade_plan = None;
        let plan = self
            .ready("apt", Capability::Upgrade, cancel)?
            .apt_upgrade_plan(cancel)?;
        self.apt_upgrade_plan = Some(plan.clone());
        Ok(plan)
    }

    /// Transfer an approved plan to a fresh worker engine. It is always
    /// re-simulated there, before authorization or any APT write.
    pub fn remember_apt_upgrade_plan(&mut self, plan: AptUpgradePlan) {
        self.apt_upgrade_plan = Some(plan);
    }
    pub fn plan_operation(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
    ) -> Result<Option<TransactionPlan>, EngineError> {
        self.operation_plan = None;
        let plan = self
            .ready(operation.backend(), operation.capability(), cancel)?
            .operation_plan(operation, cancel)?;
        if plan
            .as_ref()
            .is_some_and(|plan| &plan.operation != operation)
        {
            return Err(EngineError::InvalidResponse {
                backend: operation.backend().into(),
                reason: "native preview targeted a different operation".into(),
            });
        }
        self.operation_plan = plan.clone();
        Ok(plan)
    }
    pub fn remember_operation_plan(&mut self, plan: TransactionPlan) {
        self.operation_plan = Some(plan);
    }
    pub fn register(&mut self, backend: impl Backend + 'static) -> Result<(), EngineError> {
        let id = backend.id().to_owned();
        if self.backends.contains_key(&id) {
            return Err(EngineError::DuplicateBackend(id));
        }
        self.backends.insert(id, Box::new(backend));
        Ok(())
    }

    /// Remember a detection outcome for the query that follows registration.
    /// Only definitive outcomes are kept; failures fall through to a fresh
    /// detection at query time, preserving today's retry behavior.
    pub fn note_detected(&mut self, id: String, status: Result<Availability, EngineError>) {
        if let Ok(availability) = status {
            self.detected.insert(id, availability);
        }
    }

    pub fn discover(&mut self, cancel: &Cancellation) -> Vec<Source> {
        self.backends
            .iter_mut()
            .map(|(id, backend)| Source {
                backend: id.clone(),
                capabilities: backend.capabilities().to_vec(),
                availability: if cancel.requested() {
                    Err(EngineError::Cancelled)
                } else {
                    backend.detect(cancel)
                },
            })
            .collect()
    }

    fn ready(
        &mut self,
        id: &str,
        capability: Capability,
        cancel: &Cancellation,
    ) -> Result<&mut Box<dyn Backend>, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let backend = self
            .backends
            .get_mut(id)
            .ok_or_else(|| EngineError::UnknownBackend(id.into()))?;
        if !backend.capabilities().contains(&capability) {
            return Err(backend.unsupported(capability));
        }
        if let Availability::Unavailable(reason) = backend.detect(cancel)? {
            return Err(EngineError::Unavailable {
                backend: id.into(),
                reason,
            });
        }
        // Detection may have taken time; do not start an operation after cancellation.
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        Ok(backend)
    }

    pub fn search(&mut self, query: &str, cancel: &Cancellation) -> PackageReport {
        self.query(Some(query), cancel)
    }
    /// Query one exact backend for an imported manifest entry. This avoids
    /// sending each imported name to every remote manager.
    pub fn search_backend(
        &mut self,
        id: &str,
        query: &str,
        cancel: &Cancellation,
    ) -> PackageReport {
        match self
            .ready(id, Capability::Search, cancel)
            .and_then(|backend| backend.search(query, cancel))
        {
            Ok(packages) => PackageReport {
                packages,
                failures: vec![],
                successful_sources: vec![id.into()],
            },
            Err(error) => PackageReport {
                packages: vec![],
                failures: vec![BackendFailure {
                    backend: id.into(),
                    error,
                }],
                successful_sources: vec![],
            },
        }
    }
    pub fn installed(&mut self, cancel: &Cancellation) -> PackageReport {
        self.query(None, cancel)
    }
    /// Discover cleanup plans independently per backend. Sources without a
    /// cleanup capability are omitted; they have nothing to show on this view.
    pub fn cleanup(&mut self, cancel: &Cancellation) -> CleanupReport {
        self.cleanup_plans.clear();
        let mut report = CleanupReport::default();
        let ids: Vec<_> = self.backends.keys().cloned().collect();
        for id in ids {
            if !self
                .backends
                .get(&id)
                .is_some_and(|backend| backend.capabilities().contains(&Capability::Clean))
            {
                continue;
            }
            let mut seen = BTreeSet::new();
            let result = self
                .ready(&id, Capability::Clean, cancel)
                .map(|backend| backend.cleanup_report(cancel));
            match result {
                Ok(partial)
                    if partial.failures.iter().all(|failure| failure.backend == id)
                        && partial.items.iter().all(|item| {
                            item.id.backend == id
                                && !item.id.key.is_empty()
                                && !item.title.trim().is_empty()
                                && seen.insert(item.id.clone())
                        }) =>
                {
                    report.items.extend(partial.items);
                    report.failures.extend(partial.failures);
                }
                Ok(_) => report.failures.push(BackendFailure {
                    backend: id.clone(),
                    error: EngineError::InvalidResponse {
                        backend: id,
                        reason: "foreign, duplicate, or incomplete cleanup item".into(),
                    },
                }),
                Err(error) => report.failures.push(BackendFailure { backend: id, error }),
            }
        }
        report.items.sort_by(|a, b| a.id.cmp(&b.id));
        for item in &report.items {
            self.remember_cleanup_plan(item.clone());
        }
        report
    }

    /// Preserve the preview a frontend showed for confirmation. Execution
    /// checks it against a fresh native preview before allowing a cleanup write.
    pub fn remember_cleanup_plan(&mut self, item: CleanupItem) {
        self.cleanup_plans.insert(item.id.clone(), item);
    }
    fn query(&mut self, query: Option<&str>, cancel: &Cancellation) -> PackageReport {
        let mut report = PackageReport::default();
        let ids: Vec<_> = self.backends.keys().cloned().collect();
        for id in ids {
            let capability = if query.is_some() {
                Capability::Search
            } else {
                Capability::Installed
            };
            let noted = self.detected.get(&id).cloned();
            let result = match self.backends.get_mut(&id) {
                None => Err(EngineError::UnknownBackend(id.clone())),
                Some(backend) => {
                    Self::query_backend(&mut **backend, &id, noted, capability, query, cancel)
                }
            };
            match result {
                Ok(packages) => {
                    report.packages.extend(packages);
                    report.successful_sources.push(id);
                }
                Err(error) => report.failures.push(BackendFailure { backend: id, error }),
            }
        }
        report.packages.sort_by(|a, b| a.id.cmp(&b.id));
        report
    }

    /// Query every backend concurrently, emitting the cumulative sorted
    /// report as each backend answers. The final emission equals [`search`]
    /// / [`installed`](Self::installed); frontends render partials for
    /// perceived speed while the terminal state stays deterministic.
    /// Backends return to the engine afterwards for reuse. Cancellation
    /// surfaces per backend like the synchronous query; there is no
    /// cross-backend rollback.
    pub fn search_stream(
        &mut self,
        query: &str,
        cancel: &Cancellation,
        emit: &mut dyn FnMut(PackageReport),
    ) -> PackageReport {
        self.stream(Some(query), cancel, emit)
    }
    /// Installed-set counterpart to [`search_stream`](Self::search_stream):
    /// same cumulative sorted partials, same terminal report as
    /// [`installed`](Self::installed).
    pub fn installed_stream(
        &mut self,
        cancel: &Cancellation,
        emit: &mut dyn FnMut(PackageReport),
    ) -> PackageReport {
        self.stream(None, cancel, emit)
    }
    fn stream(
        &mut self,
        query: Option<&str>,
        cancel: &Cancellation,
        emit: &mut dyn FnMut(PackageReport),
    ) -> PackageReport {
        // Workers own their backends so queries run concurrently; the engine
        // reassembles itself afterwards for reuse.
        let backends = std::mem::take(&mut self.backends);
        let noted = &self.detected;
        let (tx, rx) = std::sync::mpsc::channel();
        let (accumulated, stash) = std::thread::scope(|s| {
            for (id, mut backend) in backends {
                let tx = tx.clone();
                let capability = if query.is_some() {
                    Capability::Search
                } else {
                    Capability::Installed
                };
                let noted = noted.get(&id).cloned();
                s.spawn(move || {
                    let result =
                        Self::query_backend(&mut *backend, &id, noted, capability, query, cancel);
                    let _ = tx.send((id, backend, result));
                });
            }
            drop(tx);
            let mut accumulated = PackageReport::default();
            let mut stash = Vec::new();
            for (id, backend, result) in rx {
                match result {
                    Ok(packages) => {
                        accumulated.packages.extend(packages);
                        accumulated.successful_sources.push(id.clone());
                    }
                    Err(error) => accumulated.failures.push(BackendFailure {
                        backend: id.clone(),
                        error,
                    }),
                }
                accumulated.packages.sort_by(|a, b| a.id.cmp(&b.id));
                accumulated.successful_sources.sort();
                stash.push((id, backend));
                emit(accumulated.clone());
            }
            (accumulated, stash)
        });
        for (id, backend) in stash {
            self.backends.insert(id, backend);
        }
        accumulated
    }

    /// One backend query with the same checks as [`Engine::ready`], consulting a
    /// remembered detection outcome first. Shared by the synchronous query and
    /// the streaming fan-out so both agree on success, failure, and validation.
    fn query_backend(
        backend: &mut dyn Backend,
        id: &str,
        noted: Option<Availability>,
        capability: Capability,
        query: Option<&str>,
        cancel: &Cancellation,
    ) -> Result<Vec<Package>, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        if !backend.capabilities().contains(&capability) {
            return Err(backend.unsupported(capability));
        }
        match noted {
            Some(Availability::Available) => {}
            Some(Availability::Unavailable(reason)) => {
                return Err(EngineError::Unavailable {
                    backend: id.into(),
                    reason,
                });
            }
            None => {
                if let Availability::Unavailable(reason) = backend.detect(cancel)? {
                    return Err(EngineError::Unavailable {
                        backend: id.into(),
                        reason,
                    });
                }
                // Detection may have taken time; do not start a query after
                // cancellation.
                if cancel.requested() {
                    return Err(EngineError::Cancelled);
                }
            }
        }
        let packages = match query {
            Some(query) => backend.search(query, cancel)?,
            None => backend.installed(cancel)?,
        };
        let mut seen = BTreeSet::new();
        if packages.iter().any(|p| {
            p.id.backend != id
                || !seen.insert(&p.id)
                || (query.is_none() && p.installed_version.is_none())
        }) {
            return Err(EngineError::InvalidResponse {
                backend: id.into(),
                reason: "foreign/duplicate identity or missing installed state".into(),
            });
        }
        Ok(packages)
    }

    pub fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let result = self
            .ready(&id.backend, Capability::Details, cancel)?
            .details(id, cancel)?;
        if result.package.id != *id {
            return Err(EngineError::InvalidResponse {
                backend: id.backend.clone(),
                reason: "details returned a different identity".into(),
            });
        }
        Ok(result)
    }

    /// Details against previously detected state, without re-running
    /// detection. Prefer [`details`](Self::details) for cold engines: this
    /// skips availability re-validation, so a backend removed after
    /// discovery surfaces its command failure instead of `Unavailable`.
    /// Frontends reuse warm engines for selections and rebuild on
    /// navigation, where discovery runs again.
    pub fn details_reuse(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let backend = self
            .backends
            .get_mut(&id.backend)
            .ok_or_else(|| EngineError::UnknownBackend(id.backend.clone()))?;
        if !backend.capabilities().contains(&Capability::Details) {
            return Err(backend.unsupported(Capability::Details));
        }
        let result = backend.details(id, cancel)?;
        if result.package.id != *id {
            return Err(EngineError::InvalidResponse {
                backend: id.backend.clone(),
                reason: "details returned a different identity".into(),
            });
        }
        Ok(result)
    }

    pub fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        events: &mut dyn FnMut(Event),
    ) -> Result<OperationOutcome, EngineError> {
        self.execute_started(operation, cancel, events, false)
    }

    fn execute_started(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        events: &mut dyn FnMut(Event),
        already_started: bool,
    ) -> Result<OperationOutcome, EngineError> {
        if !already_started {
            events(Event::Started(operation.clone()));
        }
        let expected = match operation {
            Operation::Clean(id) => self.cleanup_plans.remove(id),
            _ => None,
        };
        let expected_apt = if matches!(operation, Operation::UpgradeAll { backend } if backend == "apt")
        {
            self.apt_upgrade_plan.take()
        } else {
            None
        };
        let expected_plan = self
            .operation_plan
            .take()
            .filter(|plan| &plan.operation == operation);
        let result = self
            .ready(operation.backend(), operation.capability(), cancel)
            .and_then(|backend| {
                if matches!(operation, Operation::UpgradeAll { backend } if backend == "apt") {
                    let current = backend.apt_upgrade_plan(cancel)?;
                    if expected_apt.as_ref() != Some(&current) {
                        return Err(EngineError::InvalidResponse {
                            backend: "apt".into(),
                            reason:
                                "APT upgrade plan changed or was not reviewed; preview it again."
                                    .into(),
                        });
                    }
                }
                if let Operation::Clean(id) = operation {
                    // Backends may need an explicitly authorized preview for
                    // this exact task (APT autoclean, for example). A general
                    // discovery would omit it and reject a valid reviewed plan.
                    let current = backend.cleanup_plan(id, cancel)?;
                    if expected.as_ref() != Some(&current) {
                        return Err(EngineError::InvalidResponse {
                            backend: id.backend.clone(),
                            reason: "Cleanup plan changed or expired; reload and review it again."
                                .into(),
                        });
                    }
                }
                if let Some(expected) = &expected_plan {
                    let current = backend.operation_plan(operation, cancel)?;
                    if current.as_ref() != Some(expected) {
                        return Err(EngineError::InvalidResponse {
                            backend: operation.backend().into(),
                            reason: "Transaction plan changed; review the action again.".into(),
                        });
                    }
                }
                backend.execute(operation, cancel, &mut |progress| {
                    events(Event::Progress {
                        operation: operation.clone(),
                        progress,
                    })
                })
            });
        events(Event::Finished {
            operation: operation.clone(),
            result: result.clone(),
        });
        result
    }

    /// Check every native preview and typed target before authorization. A
    /// failure prevents every write in this confirmation scope.
    fn preflight(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
    ) -> Result<(), EngineError> {
        let expected_apt = self.apt_upgrade_plan.clone();
        let expected_cleanup = match operation {
            Operation::Clean(id) => self.cleanup_plans.get(id).cloned(),
            _ => None,
        };
        let expected_plan = self
            .operation_plan
            .as_ref()
            .filter(|plan| &plan.operation == operation)
            .cloned();
        let backend = self.ready(operation.backend(), operation.capability(), cancel)?;
        if matches!(operation, Operation::UpgradeAll { backend } if backend == "apt") {
            let current = backend.apt_upgrade_plan(cancel)?;
            if expected_apt.as_ref() != Some(&current) {
                return Err(EngineError::InvalidResponse {
                    backend: "apt".into(),
                    reason: "APT upgrade plan changed or was not reviewed; preview it again."
                        .into(),
                });
            }
        }
        if let Operation::Clean(id) = operation {
            let current = backend.cleanup_plan(id, cancel)?;
            if expected_cleanup.as_ref() != Some(&current) {
                return Err(EngineError::InvalidResponse {
                    backend: id.backend.clone(),
                    reason: "Cleanup plan changed or expired; reload and review it again.".into(),
                });
            }
        }
        if let Some(expected) = expected_plan {
            let current = backend.operation_plan(operation, cancel)?;
            if current.as_ref() != Some(&expected) {
                return Err(EngineError::InvalidResponse {
                    backend: operation.backend().into(),
                    reason: "Transaction plan changed; review the action again.".into(),
                });
            }
        }
        crate::batch::protected_commands(operation)?;
        Ok(())
    }

    /// Ordered, best-effort batch. Successful writes are not rolled back after another failure.
    /// Every input receives its own result and terminal event, including cancelled items.
    pub fn execute_batch(
        &mut self,
        operations: &[Operation],
        cancel: &Cancellation,
        events: &mut dyn FnMut(Event),
    ) -> Vec<Result<OperationOutcome, EngineError>> {
        if operations.is_empty() {
            return vec![];
        }
        if self.batch_authorization.is_none() {
            return operations
                .iter()
                .map(|operation| self.execute(operation, cancel, events))
                .collect();
        }
        let checked: Vec<_> = operations
            .iter()
            .map(|operation| self.preflight(operation, cancel))
            .collect();
        if checked.iter().any(Result::is_err) {
            return operations.iter().zip(checked).map(|(operation, result)| {
                let result = Err(result.err().unwrap_or_else(|| EngineError::InvalidResponse {
                    backend: operation.backend().into(),
                    reason: "Another operation in the batch failed validation; review the batch again.".into(),
                }));
                events(Event::Started(operation.clone()));
                events(Event::Finished { operation: operation.clone(), result: result.clone() });
                result
            }).collect();
        }
        let protected = crate::batch::batch_commands(operations)
            .expect("individual protected commands were validated above")
            .iter()
            .any(|commands| !commands.is_empty());
        if !protected {
            return operations
                .iter()
                .map(|operation| self.execute(operation, cancel, events))
                .collect();
        }
        let (host, authorization) = self.batch_authorization.clone().expect("configured above");
        events(Event::Started(operations[0].clone()));
        events(Event::Progress {
            operation: operations[0].clone(),
            progress: Progress::Message("Authorizing system changes for this batch.".into()),
        });
        let guard = match crate::batch::begin(&host, authorization, operations, cancel) {
            Ok(guard) => guard,
            Err(error) => {
                return operations
                    .iter()
                    .enumerate()
                    .map(|(index, operation)| {
                        if index != 0 {
                            events(Event::Started(operation.clone()));
                        }
                        let result = Err(EngineError::from(error.clone()));
                        events(Event::Finished {
                            operation: operation.clone(),
                            result: result.clone(),
                        });
                        result
                    })
                    .collect();
            }
        };
        if guard.is_none() {
            events(Event::Progress {
                operation: operations[0].clone(),
                progress: Progress::Message(
                    "Batch runner missing. System changes may request separate authorization."
                        .into(),
                ),
            });
        }
        let mut results = Vec::with_capacity(operations.len());
        let mut index = 0;
        while index < operations.len() {
            let operation = &operations[index];
            let mut end = index + 1;
            if operation.backend() == "apt"
                && matches!(
                    operation,
                    Operation::Install(_) | Operation::Remove(_) | Operation::Upgrade(_)
                )
            {
                while end < operations.len()
                    && operations[end].backend() == "apt"
                    && std::mem::discriminant(&operations[end]) == std::mem::discriminant(operation)
                {
                    end += 1;
                }
            }
            crate::batch::set_operation(index);
            if end - index > 1 {
                for (offset, member) in operations[index..end].iter().enumerate() {
                    if index + offset != 0 {
                        events(Event::Started(member.clone()));
                    }
                }
                let rechecked = operations[index..end]
                    .iter()
                    .map(|member| self.preflight(member, cancel))
                    .collect::<Vec<_>>();
                let grouped = if let Some(error) = rechecked.into_iter().find_map(Result::err) {
                    Err(error)
                } else {
                    self.ready("apt", operation.capability(), cancel)
                        .and_then(|backend| {
                            backend
                                .execute_group(&operations[index..end], cancel, &mut |progress| {
                                    events(Event::Progress {
                                        operation: operation.clone(),
                                        progress,
                                    });
                                })
                                .unwrap_or_else(|| {
                                    Err(EngineError::InvalidResponse {
                                        backend: "apt".into(),
                                        reason: "grouped APT operation unavailable".into(),
                                    })
                                })
                        })
                };
                for member in &operations[index..end] {
                    let result = match &grouped {
                        Ok(outcomes) => Ok(outcomes[0].clone()),
                        Err(error) => Err(error.clone()),
                    };
                    events(Event::Finished {
                        operation: member.clone(),
                        result: result.clone(),
                    });
                    results.push(result);
                }
            } else {
                results.push(self.execute_started(operation, cancel, events, index == 0));
            }
            index = end;
        }
        drop(guard);
        results
    }
}

#[cfg(test)]
mod apt_upgrade_tests {
    use super::*;
    use crate::host::Runtime;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct AptFixture {
        reads: usize,
        drift: bool,
        writes: Arc<AtomicUsize>,
    }
    impl Backend for AptFixture {
        fn id(&self) -> &str {
            "apt"
        }
        fn capabilities(&self) -> &[Capability] {
            &[Capability::Upgrade]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn apt_upgrade_plan(&mut self, _: &Cancellation) -> Result<AptUpgradePlan, EngineError> {
            self.reads += 1;
            Ok(AptUpgradePlan {
                preview: if self.drift && self.reads > 1 {
                    "Inst changed"
                } else {
                    "Inst original"
                }
                .into(),
                upgrades: vec!["synthetic".into()],
                installs: vec![],
                removals: vec![],
            })
        }
        fn execute(
            &mut self,
            _: &Operation,
            _: &Cancellation,
            _: &mut dyn FnMut(Progress),
        ) -> Result<OperationOutcome, EngineError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(OperationOutcome::default())
        }
    }
    #[test]
    fn apt_full_upgrade_requires_a_fresh_matching_preview() {
        let cancel = Cancellation::default();
        let op = Operation::UpgradeAll {
            backend: "apt".into(),
        };
        for (preview, drift, succeeds) in [
            (false, false, false),
            (true, true, false),
            (true, false, true),
        ] {
            let writes = Arc::new(AtomicUsize::new(0));
            let mut engine = Engine::default();
            engine
                .register(AptFixture {
                    reads: 0,
                    drift,
                    writes: writes.clone(),
                })
                .unwrap();
            if preview {
                engine.plan_apt_upgrade(&cancel).unwrap();
            }
            assert_eq!(engine.execute(&op, &cancel, &mut |_| {}).is_ok(), succeeds);
            assert_eq!(writes.load(Ordering::SeqCst), usize::from(succeeds));
        }
    }

    #[test]
    fn changed_later_plan_blocks_every_batch_write_before_authorization() {
        struct UserFixture(Arc<AtomicUsize>);
        impl Backend for UserFixture {
            fn id(&self) -> &str {
                "user"
            }
            fn capabilities(&self) -> &[Capability] {
                &[Capability::Refresh]
            }
            fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
                Ok(Availability::Available)
            }
            fn execute(
                &mut self,
                _: &Operation,
                _: &Cancellation,
                _: &mut dyn FnMut(Progress),
            ) -> Result<OperationOutcome, EngineError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(OperationOutcome::default())
            }
        }
        let writes = Arc::new(AtomicUsize::new(0));
        let mut engine = Engine::default();
        engine.register(UserFixture(writes.clone())).unwrap();
        engine
            .register(AptFixture {
                reads: 0,
                drift: true,
                writes: writes.clone(),
            })
            .unwrap();
        engine.enable_batch_authorization(
            Host::new(Runtime::Native, Default::default()),
            Authorization::Polkit,
        );
        let cancel = Cancellation::default();
        engine.plan_apt_upgrade(&cancel).unwrap();
        let operations = [
            Operation::Refresh {
                backend: "user".into(),
            },
            Operation::UpgradeAll {
                backend: "apt".into(),
            },
        ];
        let mut events = Vec::new();
        let results = engine.execute_batch(&operations, &cancel, &mut |event| events.push(event));
        assert!(results.iter().all(Result::is_err));
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::Started(_)))
                .count(),
            2
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::Finished { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn exact_apt_selections_share_one_native_transaction_and_keep_two_outcomes() {
        struct GroupedFixture(Arc<AtomicUsize>);
        impl Backend for GroupedFixture {
            fn id(&self) -> &str {
                "apt"
            }
            fn capabilities(&self) -> &[Capability] {
                &[Capability::Install]
            }
            fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
                Ok(Availability::Available)
            }
            fn execute_group(
                &mut self,
                operations: &[Operation],
                _: &Cancellation,
                _: &mut dyn FnMut(Progress),
            ) -> Option<Result<Vec<OperationOutcome>, EngineError>> {
                assert_eq!(operations.len(), 2);
                self.0.fetch_add(1, Ordering::SeqCst);
                Some(Ok(vec![OperationOutcome::default(); 2]))
            }
            fn execute(
                &mut self,
                _: &Operation,
                _: &Cancellation,
                _: &mut dyn FnMut(Progress),
            ) -> Result<OperationOutcome, EngineError> {
                panic!("separate transaction")
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let mut engine = Engine::default();
        engine.register(GroupedFixture(calls.clone())).unwrap();
        engine.enable_batch_authorization(
            Host::new(Runtime::Native, Default::default()),
            Authorization::Polkit,
        );
        let selected = ["synthetic-one", "synthetic-two"].map(|name| {
            Operation::Install(PackageId {
                backend: "apt".into(),
                name: name.into(),
                architecture: "amd64".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            })
        });
        let mut events = Vec::new();
        let outcomes = engine.execute_batch(&selected, &Cancellation::default(), &mut |event| {
            events.push(event)
        });
        assert!(outcomes.iter().all(Result::is_ok));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::Finished { .. }))
                .count(),
            2
        );
    }
}

#[cfg(test)]
mod operation_plan_tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct PlannedFixture {
        reads: usize,
        drift: bool,
        writes: Arc<AtomicUsize>,
    }
    impl Backend for PlannedFixture {
        fn id(&self) -> &str {
            "fixture"
        }
        fn capabilities(&self) -> &[Capability] {
            &[Capability::Install]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn operation_plan(
            &mut self,
            operation: &Operation,
            _: &Cancellation,
        ) -> Result<Option<TransactionPlan>, EngineError> {
            self.reads += 1;
            Ok(Some(TransactionPlan {
                operation: operation.clone(),
                native_preview: if self.drift && self.reads > 1 {
                    "extra removal"
                } else {
                    "install only"
                }
                .into(),
                changes: if self.drift && self.reads > 1 {
                    vec![PlannedChange {
                        action: PlannedAction::Remove,
                        name: "extra-library".into(),
                        installed_version: Some("1".into()),
                        candidate_version: None,
                    }]
                } else {
                    vec![]
                },
                download_bytes: None,
                disk_bytes: None,
                restart_required: None,
            }))
        }
        fn execute(
            &mut self,
            _: &Operation,
            _: &Cancellation,
            _: &mut dyn FnMut(Progress),
        ) -> Result<OperationOutcome, EngineError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(OperationOutcome::default())
        }
    }
    #[test]
    fn changed_single_operation_plan_blocks_write() {
        let cancel = Cancellation::default();
        let operation = Operation::Install(PackageId {
            backend: "fixture".into(),
            name: "anonymous".into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        });
        for drift in [false, true] {
            let writes = Arc::new(AtomicUsize::new(0));
            let mut engine = Engine::default();
            engine
                .register(PlannedFixture {
                    reads: 0,
                    drift,
                    writes: writes.clone(),
                })
                .unwrap();
            assert!(engine
                .plan_operation(&operation, &cancel)
                .unwrap()
                .is_some());
            assert_eq!(
                engine.execute(&operation, &cancel, &mut |_| {}).is_ok(),
                !drift
            );
            assert_eq!(writes.load(Ordering::SeqCst), usize::from(!drift));
        }
    }
}
