//! Synchronous orchestration for use on a frontend worker. No shell or UI dependencies.
use crate::{
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
    fn execute(
        &mut self,
        operation: &Operation,
        _cancel: &Cancellation,
        _progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        Err(self.unsupported(operation.capability()))
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
}
impl Engine {
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
                Ok(packages) => report.packages.extend(packages),
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
                    Ok(packages) => accumulated.packages.extend(packages),
                    Err(error) => accumulated.failures.push(BackendFailure {
                        backend: id.clone(),
                        error,
                    }),
                }
                accumulated.packages.sort_by(|a, b| a.id.cmp(&b.id));
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
        events(Event::Started(operation.clone()));
        let expected = match operation {
            Operation::Clean(id) => self.cleanup_plans.remove(id),
            _ => None,
        };
        let result = self
            .ready(operation.backend(), operation.capability(), cancel)
            .and_then(|backend| {
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

    /// Ordered, best-effort batch. Successful writes are not rolled back after another failure.
    /// Every input receives its own result and terminal event, including cancelled items.
    pub fn execute_batch(
        &mut self,
        operations: &[Operation],
        cancel: &Cancellation,
        events: &mut dyn FnMut(Event),
    ) -> Vec<Result<OperationOutcome, EngineError>> {
        operations
            .iter()
            .map(|operation| self.execute(operation, cancel, events))
            .collect()
    }
}
