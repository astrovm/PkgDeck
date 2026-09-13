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
                id.name == selector.name
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
    fn query(&mut self, query: Option<&str>, cancel: &Cancellation) -> PackageReport {
        let mut report = PackageReport::default();
        let ids: Vec<_> = self.backends.keys().cloned().collect();
        for id in ids {
            let capability = if query.is_some() {
                Capability::Search
            } else {
                Capability::Installed
            };
            let result = self.ready(&id, capability, cancel).and_then(|backend| {
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
                        backend: id.clone(),
                        reason: "foreign/duplicate identity or missing installed state".into(),
                    });
                }
                Ok(packages)
            });
            match result {
                Ok(packages) => report.packages.extend(packages),
                Err(error) => report.failures.push(BackendFailure { backend: id, error }),
            }
        }
        report.packages.sort_by(|a, b| a.id.cmp(&b.id));
        report
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

    pub fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        events: &mut dyn FnMut(Event),
    ) -> Result<OperationOutcome, EngineError> {
        events(Event::Started(operation.clone()));
        let result = self
            .ready(operation.backend(), operation.capability(), cancel)
            .and_then(|backend| {
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
