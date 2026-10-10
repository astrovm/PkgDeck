//! Synchronous orchestration for use on a frontend worker. No shell or UI dependencies.
use crate::{
    host::{Authorization, Host},
    package::*,
    process::{Cancellation, ExecutionError},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{
        atomic::{AtomicU64, Ordering},
        Condvar, Mutex,
    },
};

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

/// One in-flight update check. Formulae and casks both ask for `brew update`;
/// the first runs it and the other waits, then both see the same result.
struct IndexRefresh {
    started: bool,
    finished: bool,
    error: Option<EngineError>,
}
static UPDATE_CHECKS: Mutex<Option<HashMap<u64, IndexRefresh>>> = Mutex::new(None);
static UPDATE_CHECK_TURN: Condvar = Condvar::new();
static NEXT_UPDATE_CHECK: AtomicU64 = AtomicU64::new(1);

fn update_checks() -> std::sync::MutexGuard<'static, Option<HashMap<u64, IndexRefresh>>> {
    UPDATE_CHECKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
/// How long a read waits before its second try.
const READ_AGAIN_AFTER: std::time::Duration =
    std::time::Duration::from_millis(if cfg!(test) { 10 } else { 1500 });
/// A read the manager refused for a moment, because it was busy or exited
/// with an error, gets a second try before its source is shown as broken.
/// Timeouts aren't tried again, since that would double a long wait.
pub(crate) fn read_again_once<T>(
    cancel: &Cancellation,
    mut read: impl FnMut() -> Result<T, EngineError>,
) -> Result<T, EngineError> {
    match read() {
        Err(EngineError::Execution(ExecutionError::Failed(_) | ExecutionError::LockBusy)) => {
            let wake = std::time::Instant::now() + READ_AGAIN_AFTER;
            while std::time::Instant::now() < wake {
                if cancel.requested() {
                    return Err(EngineError::Cancelled);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            read()
        }
        other => other,
    }
}
pub(crate) fn begin_update_check_token() -> u64 {
    let token = NEXT_UPDATE_CHECK.fetch_add(1, Ordering::Relaxed);
    update_checks().get_or_insert_with(HashMap::new).insert(
        token,
        IndexRefresh {
            started: false,
            finished: false,
            error: None,
        },
    );
    token
}
pub(crate) fn end_update_check_token(token: u64) {
    if let Some(checks) = update_checks().as_mut() {
        checks.remove(&token);
    }
}
/// Run `refresh` once for `token`. A second caller with the same token waits
/// and returns the first result, so two backends cannot fetch twice. If the
/// runner stops early, the waiter still wakes.
pub(crate) fn once_per_check(
    token: u64,
    refresh: impl FnOnce() -> Result<(), EngineError>,
) -> Result<(), EngineError> {
    loop {
        let mut checks = update_checks();
        let Some(gate) = checks.as_mut().and_then(|map| map.get_mut(&token)) else {
            return Ok(());
        };
        if gate.finished {
            return match gate.error.clone() {
                Some(error) => Err(error),
                None => Ok(()),
            };
        }
        if gate.started {
            drop(
                UPDATE_CHECK_TURN
                    .wait(checks)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
            continue;
        }
        gate.started = true;
        drop(checks);
        // Drop publishes the outcome, including when `refresh` panics, so the
        // waiter cannot sit on the condvar forever.
        let mut finish = CheckFinish {
            token,
            outcome: None,
        };
        let result = refresh();
        finish.outcome = Some(result.clone());
        return result;
    }
}
struct CheckFinish {
    token: u64,
    outcome: Option<Result<(), EngineError>>,
}
impl Drop for CheckFinish {
    fn drop(&mut self) {
        let error = match self.outcome.take() {
            Some(Ok(())) => None,
            Some(Err(error)) => Some(error),
            None => Some(EngineError::Execution(ExecutionError::Invalid(
                "update check stopped before it finished".into(),
            ))),
        };
        if let Some(gate) = update_checks()
            .as_mut()
            .and_then(|checks| checks.get_mut(&self.token))
        {
            gate.finished = true;
            gate.error = error;
        }
        UPDATE_CHECK_TURN.notify_all();
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
    /// Join an update check. Backends that share one refresh (Homebrew
    /// formulae and casks share `brew update`) keep `token` until the check
    /// ends. `None` is every other query, which must not fetch.
    fn arm_update_check(&mut self, _token: Option<u64>) {}
    /// Whether update checks need an explicit metadata refresh. This is a
    /// cheap declaration: non-participants are not detected just to refresh.
    fn has_update_index(&self) -> bool {
        false
    }
    /// Refresh metadata before an update check lists installed packages.
    /// Listing Homebrew sets `HOMEBREW_NO_AUTO_UPDATE`, and its `outdated`
    /// flag is computed from the local tap, so a new cask stays invisible
    /// until something runs `brew update`. Armed Homebrew backends do that
    /// once per check. Everything else leaves lists as they are.
    fn refresh_update_index(&mut self, _cancel: &Cancellation) -> Result<(), EngineError> {
        Ok(())
    }
    /// Failures from skipped portions of the most recent successful search or
    /// installed query. These accompany usable rows, but prevent the source
    /// from being treated as complete. Cached queries must retain their errors.
    fn query_errors(&self) -> Vec<EngineError> {
        vec![]
    }
    /// Cheap, local check used by exact-name lookups: `false` only when no
    /// package or reference of this backend can ever be called `name` (for
    /// example a Flatpak app id always contains a dot). Never runs commands.
    fn may_have(&self, _name: &str) -> bool {
        true
    }
    /// Exact-name lookup can avoid enumerating an entire inventory. The
    /// default retains each manager's existing search behavior.
    fn lookup(&mut self, name: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.search(name, cancel)
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
        self.check_failures(selector)?;
        Self::decide(self.matches(selector).map(|p| &p.id).cloned().collect())
    }

    /// Selection for a name typed without `--from`. Registry sources (npm,
    /// Cargo, pipx, …) answer every valid name with an unverified install
    /// offer, so they must not make a catalog match ambiguous: packages a
    /// source confirmed (installed, or listed with a version) win, and an
    /// offer is chosen only when it is the single possibility. With
    /// `offers` false, offers never count, which suits reads such as
    /// details. Several offers and nothing confirmed is `NotFound`; see
    /// [`offer_sources`](Self::offer_sources) for what could still be tried.
    /// A pinned backend behaves exactly like [`select`](Self::select).
    pub fn select_confirmed(
        &self,
        selector: &Selector,
        offers: bool,
    ) -> Result<PackageId, EngineError> {
        if selector.backend.is_some() {
            return self.select(selector);
        }
        self.check_failures(selector)?;
        let (unverified, confirmed): (Vec<&Package>, Vec<&Package>) = self
            .matches(selector)
            .partition(|package| unverified_search_offer(package));
        if !confirmed.is_empty() {
            return Self::decide(confirmed.iter().map(|p| p.id.clone()).collect());
        }
        match unverified.as_slice() {
            [one] if offers => Ok(one.id.clone()),
            _ => Err(EngineError::NotFound),
        }
    }

    /// Sorted, distinct backends that only offered to try installing this
    /// exact name, without confirming that it exists.
    pub fn offer_sources(&self, selector: &Selector) -> Vec<String> {
        self.matches(selector)
            .filter(|package| unverified_search_offer(package))
            .map(|package| package.id.backend.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn matches<'a>(&'a self, selector: &'a Selector) -> impl Iterator<Item = &'a Package> + 'a {
        self.packages.iter().filter(move |p| {
            let id = &p.id;
            (id.name == selector.name || id.reference.as_deref() == Some(selector.name.as_str()))
                && selector.backend.as_ref().is_none_or(|b| b == &id.backend)
                && selector
                    .architecture
                    .as_ref()
                    .is_none_or(|a| a == &id.architecture)
                && selector.scope.as_ref().is_none_or(|s| s == &id.scope)
        })
    }

    fn decide(matches: BTreeSet<PackageId>) -> Result<PackageId, EngineError> {
        match matches.len() {
            0 => Err(EngineError::NotFound),
            1 => Ok(matches.into_iter().next().expect("one match")),
            _ => Err(EngineError::Ambiguous(matches.into_iter().collect())),
        }
    }

    fn check_failures(&self, selector: &Selector) -> Result<(), EngineError> {
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
        if failures.is_empty() {
            Ok(())
        } else {
            Err(EngineError::Incomplete(failures))
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
    /// Successful detections, noted by [`native_engine`](crate::backends::native_engine)
    /// (and friends) or learned by a query, so later queries on the same
    /// engine skip their own round. Failed detections are never cached: the
    /// next query retries them. Frontends reuse an engine only briefly.
    detected: BTreeMap<String, Availability>,
    /// Sources registered without a probe that drop out silently once
    /// detection finds them missing, as if never registered.
    optional: BTreeSet<String>,
    cleanup_plans: BTreeMap<CleanupId, CleanupItem>,
    apt_upgrade_plan: Option<AptUpgradePlan>,
    operation_plan: Option<TransactionPlan>,
    batch_authorization: Option<(Host, Authorization)>,
    batch_mode: crate::batch::BatchMode,
    unattended: bool,
}
/// Whether an exact-name lookup should ask `backend`. A source that can't
/// search, such as macOS Updates, holds no name to look up.
fn can_look_up(backend: &dyn Backend, name: &str) -> bool {
    backend.capabilities().contains(&Capability::Search) && backend.may_have(name)
}
impl Engine {
    /// Native engines use one trusted runner for the protected part of a
    /// confirmed batch when it is installed by the host package.
    pub fn enable_batch_authorization(&mut self, host: Host, authorization: Authorization) {
        self.batch_authorization = Some((host, authorization));
    }
    /// Run batches through an upgrade-only runner, for unattended updates
    /// under a saved approval: no password prompt, and nothing but
    /// refreshes and full upgrades of system managers.
    pub fn set_batch_mode(&mut self, mode: crate::batch::BatchMode) {
        self.batch_mode = mode;
    }
    /// Nobody is watching: refuse sources that would ask for a password or
    /// need someone there (see [`crate::unattended::Unattended::Never`]).
    pub fn set_unattended(&mut self, unattended: bool) {
        self.unattended = unattended;
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
        self.register_boxed(Box::new(backend))
    }
    pub fn register_boxed(&mut self, backend: Box<dyn Backend>) -> Result<(), EngineError> {
        let id = backend.id().to_owned();
        if self.backends.contains_key(&id) {
            return Err(EngineError::DuplicateBackend(id));
        }
        self.backends.insert(id, backend);
        Ok(())
    }
    /// Register a source that may be missing. Each query detects it in its
    /// own worker, so a slow probe never holds back the other sources, and a
    /// missing one is skipped instead of reported.
    pub fn register_optional(&mut self, backend: Box<dyn Backend>) -> Result<(), EngineError> {
        let id = backend.id().to_owned();
        self.register_boxed(backend)?;
        self.optional.insert(id);
        Ok(())
    }
    /// Whether a worker's outcome for `id`, already learned, stays out of
    /// reports.
    fn quiet(&self, id: &str, error: Option<&EngineError>) -> bool {
        Self::silent(self.optional.contains(id), self.detected.get(id), error)
    }
    /// Whether an optional source's outcome stays out of reports: detection
    /// found it missing, or the query was cancelled before detection could
    /// tell, so a missing source never shows up as cancelled.
    fn silent(optional: bool, noted: Option<&Availability>, error: Option<&EngineError>) -> bool {
        optional
            && match noted {
                Some(Availability::Unavailable(_)) => true,
                Some(Availability::Available) => false,
                None => matches!(error, Some(EngineError::Cancelled)),
            }
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
        // Detection spawns native tools; probe every backend concurrently and
        // keep the registration order in the result.
        let sources: Vec<Source> = std::thread::scope(|s| {
            let workers: Vec<_> = self
                .backends
                .iter_mut()
                .map(|(id, backend)| {
                    s.spawn(move || Source {
                        backend: id.clone(),
                        capabilities: backend.capabilities().to_vec(),
                        availability: if cancel.requested() {
                            Err(EngineError::Cancelled)
                        } else {
                            backend.detect(cancel)
                        },
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().expect("detection worker panicked"))
                .collect()
        });
        // Optional sources keep what this learned; missing ones stay hidden.
        for source in &sources {
            if let (true, Ok(availability)) = (
                self.optional.contains(&source.backend),
                &source.availability,
            ) {
                self.detected
                    .insert(source.backend.clone(), availability.clone());
            }
        }
        sources
            .into_iter()
            .filter(|source| !self.quiet(&source.backend, None))
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
        if self.optional.contains(id) {
            // Detect again before acting, as for any source. A missing one
            // was never there.
            let mut noted = None;
            let available = Self::available_backend(&mut **backend, id, &mut noted, cancel);
            if let Some(availability) = noted {
                self.detected.insert(id.into(), availability);
            }
            match available {
                Err(EngineError::Unavailable { .. }) => {
                    return Err(EngineError::UnknownBackend(id.into()))
                }
                result => result?,
            }
            if !backend.capabilities().contains(&capability) {
                return Err(backend.unsupported(capability));
            }
            return Ok(backend);
        }
        Self::ready_backend(&mut **backend, id, capability, cancel)?;
        Ok(backend)
    }

    /// Capability, availability, and cancellation checks for one backend.
    fn ready_backend<'a>(
        backend: &'a mut dyn Backend,
        id: &str,
        capability: Capability,
        cancel: &Cancellation,
    ) -> Result<&'a mut dyn Backend, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
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
    /// Installed packages after refreshing metadata that an update check
    /// would otherwise miss. Homebrew fetches its taps once; formulae and
    /// casks share that fetch. Other managers list as [`installed`] does.
    pub fn installed_for_updates(&mut self, cancel: &Cancellation) -> PackageReport {
        self.checking_updates(|engine| engine.installed(cancel))
    }
    /// [`installed_stream`](Self::installed_stream) for an update check.
    /// Homebrew's rows wait on `brew update`; other sources still arrive as
    /// they answer.
    pub fn installed_for_updates_stream(
        &mut self,
        cancel: &Cancellation,
        emit: &mut dyn FnMut(PackageReport),
    ) -> PackageReport {
        self.checking_updates(|engine| engine.installed_stream(cancel, emit))
    }
    /// Fetch update metadata without listing, so later listings, even in
    /// another process, see new versions. `pkd refresh` and `pkd update`
    /// call this once after their refresh operations; `skip` names the
    /// sources those already fetched (Homebrew's refresh is the same
    /// `brew update`). Failures are returned and not cached as a list.
    pub fn refresh_update_indexes(
        &mut self,
        skip: &[String],
        cancel: &Cancellation,
    ) -> Vec<BackendFailure> {
        self.checking_updates(|engine| {
            let mut failures = Vec::new();
            for (id, backend) in engine
                .backends
                .iter_mut()
                .filter(|(id, backend)| backend.has_update_index() && !skip.contains(id))
            {
                if cancel.requested() {
                    if Self::silent(
                        engine.optional.contains(id),
                        engine.detected.get(id),
                        Some(&EngineError::Cancelled),
                    ) {
                        continue;
                    }
                    failures.push(BackendFailure {
                        backend: id.clone(),
                        error: EngineError::Cancelled,
                    });
                    break;
                }
                let mut noted = engine.detected.get(id).cloned();
                let available = Self::available_backend(&mut **backend, id, &mut noted, cancel);
                if let Some(availability) = noted {
                    engine.detected.insert(id.clone(), availability);
                }
                if Self::silent(
                    engine.optional.contains(id),
                    engine.detected.get(id),
                    available.as_ref().err(),
                ) {
                    continue;
                }
                let result = available.and_then(|()| {
                    read_again_once(cancel, || backend.refresh_update_index(cancel))
                });
                if let Err(error) = result {
                    failures.push(BackendFailure {
                        backend: id.clone(),
                        error,
                    });
                }
            }
            failures
        })
    }
    /// Arm every backend, run `body`, then disarm. The token stays live for
    /// the whole call, including worker threads `body` spawns.
    fn checking_updates<T>(&mut self, body: impl FnOnce(&mut Self) -> T) -> T {
        let token = begin_update_check_token();
        for backend in self.backends.values_mut() {
            backend.arm_update_check(Some(token));
        }
        let result = body(self);
        for backend in self.backends.values_mut() {
            backend.arm_update_check(None);
        }
        end_update_check_token(token);
        result
    }
    /// Mutation planning excludes inventory-only sources, including their
    /// errors: they cannot contribute an operation or an ambiguous target.
    pub fn installed_for_mutation(&mut self, cancel: &Cancellation) -> PackageReport {
        self.query_where(None, false, cancel, &|backend| {
            !crate::backends::inventory_only(backend.id())
        })
    }
    /// Installed packages a removal of `name` may pick: everything
    /// [`installed_for_mutation`](Self::installed_for_mutation) reads, plus
    /// an exact lookup in the inventory-only sources that could hold `name`
    /// (a macOS app's path). Other names never read those inventories.
    pub fn installed_for_removal(&mut self, name: &str, cancel: &Cancellation) -> PackageReport {
        let mut report = self.installed_for_mutation(cancel);
        let inventory = self.query_where(Some(name), true, cancel, &|backend| {
            crate::backends::inventory_only(backend.id()) && can_look_up(backend, name)
        });
        report.packages.extend(inventory.packages);
        report.packages.sort_by(|a, b| a.id.cmp(&b.id));
        report.failures.extend(inventory.failures);
        report
            .successful_sources
            .extend(inventory.successful_sources);
        report
    }
    pub fn lookup_for_mutation(&mut self, name: &str, cancel: &Cancellation) -> PackageReport {
        self.query_where(Some(name), true, cancel, &|backend| {
            !crate::backends::inventory_only(backend.id()) && can_look_up(backend, name)
        })
    }
    /// Every registered source, sorted: the ones a stream will answer for,
    /// so a frontend can show which are still pending.
    pub fn source_ids(&self) -> Vec<String> {
        self.backends.keys().cloned().collect()
    }
    /// Whether a registered backend declares `capability`. Frontends check
    /// this before asking for confirmation; [`execute`](Self::execute)
    /// refuses unsupported changes regardless.
    pub fn supports(&self, backend: &str, capability: Capability) -> bool {
        self.backends
            .get(backend)
            .is_some_and(|backend| backend.capabilities().contains(&capability))
    }
    /// Discover cleanup plans independently per backend. Sources without a
    /// cleanup capability are omitted; they have nothing to show on this view.
    pub fn cleanup(&mut self, cancel: &Cancellation) -> CleanupReport {
        self.cleanup_plans.clear();
        let mut report = CleanupReport::default();
        // Each backend previews its cleanup independently, so run them
        // concurrently and merge the results in registration order.
        let (noted, optional) = (&self.detected, &self.optional);
        let results: Vec<_> = std::thread::scope(|s| {
            let workers: Vec<_> = self
                .backends
                .iter_mut()
                .filter(|(_, backend)| backend.capabilities().contains(&Capability::Clean))
                .map(|(id, backend)| {
                    let mut noted = noted.get(id).cloned();
                    let optional = optional.contains(id);
                    s.spawn(move || {
                        // An optional source detects once per engine; the
                        // others detect before each preview, as always.
                        let result = if !optional {
                            Self::ready_backend(&mut **backend, id, Capability::Clean, cancel)
                                .map(|backend| backend.cleanup_report(cancel))
                        } else if cancel.requested() {
                            Err(EngineError::Cancelled)
                        } else {
                            Self::available_backend(&mut **backend, id, &mut noted, cancel)
                                .map(|()| backend.cleanup_report(cancel))
                        };
                        (id.clone(), noted, result)
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().expect("cleanup worker panicked"))
                .collect()
        });
        for (id, noted, result) in results {
            self.learn(&id, noted);
            if self.quiet(&id, result.as_ref().err()) {
                continue;
            }
            let mut seen = BTreeSet::new();
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
        self.query_where(query, false, cancel, &|_: &dyn Backend| true)
    }
    /// Search for one exact package name. Backends whose package names can
    /// never be `name` (see [`Backend::may_have`]), or that can't search at
    /// all, are not asked, so they appear neither as failures nor as
    /// successful sources. Selection on the result behaves as on a full
    /// [`search`](Self::search).
    pub fn lookup(&mut self, name: &str, cancel: &Cancellation) -> PackageReport {
        self.query_where(Some(name), true, cancel, &|backend: &dyn Backend| {
            can_look_up(backend, name)
        })
    }
    fn query_where(
        &mut self,
        query: Option<&str>,
        exact: bool,
        cancel: &Cancellation,
        ask: &(dyn Fn(&dyn Backend) -> bool + Sync),
    ) -> PackageReport {
        let capability = if query.is_some() {
            Capability::Search
        } else {
            Capability::Installed
        };
        let noted = &self.detected;
        // Query backends concurrently; results merge in registration order so
        // the report is identical to a sequential traversal.
        let results: Vec<_> = std::thread::scope(|s| {
            let workers: Vec<_> = self
                .backends
                .iter_mut()
                .filter(|(_, backend)| ask(&***backend))
                .map(|(id, backend)| {
                    let mut noted = noted.get(id).cloned();
                    s.spawn(move || {
                        let result = Self::query_backend(
                            &mut **backend,
                            id,
                            &mut noted,
                            capability,
                            query,
                            exact,
                            cancel,
                        );
                        (id.clone(), noted, result)
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().expect("query worker panicked"))
                .collect()
        });
        let mut report = PackageReport::default();
        for (id, noted, result) in results {
            self.learn(&id, noted);
            if self.quiet(&id, result.as_ref().err()) {
                continue;
            }
            match result {
                Ok((packages, errors)) => {
                    report.packages.extend(packages);
                    if errors.is_empty() {
                        report.successful_sources.push(id.clone());
                    }
                    report
                        .failures
                        .extend(errors.into_iter().map(|error| BackendFailure {
                            backend: id.clone(),
                            error,
                        }));
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
        let (noted, optional) = (&self.detected, &self.optional);
        let (tx, rx) = std::sync::mpsc::channel();
        let (accumulated, stash, learned) = std::thread::scope(|s| {
            for (id, mut backend) in backends {
                let tx = tx.clone();
                let capability = if query.is_some() {
                    Capability::Search
                } else {
                    Capability::Installed
                };
                let mut noted = noted.get(&id).cloned();
                s.spawn(move || {
                    let result = Self::query_backend(
                        &mut *backend,
                        &id,
                        &mut noted,
                        capability,
                        query,
                        false,
                        cancel,
                    );
                    let _ = tx.send((id, backend, noted, result));
                });
            }
            drop(tx);
            let mut accumulated = PackageReport::default();
            let mut stash = Vec::new();
            let mut learned = Vec::new();
            for (id, backend, noted, result) in rx {
                // A missing optional source answers nothing at all, even
                // when cancelled before detection could tell.
                let missing = Self::silent(
                    optional.contains(&id),
                    noted.as_ref(),
                    result.as_ref().err(),
                );
                learned.push((id.clone(), noted));
                if missing {
                    stash.push((id, backend));
                    continue;
                }
                match result {
                    Ok((packages, errors)) => {
                        accumulated.packages.extend(packages);
                        if errors.is_empty() {
                            accumulated.successful_sources.push(id.clone());
                        }
                        accumulated.failures.extend(errors.into_iter().map(|error| {
                            BackendFailure {
                                backend: id.clone(),
                                error,
                            }
                        }));
                    }
                    Err(error) => accumulated.failures.push(BackendFailure {
                        backend: id.clone(),
                        error,
                    }),
                }
                accumulated.packages.sort_by(|a, b| a.id.cmp(&b.id));
                accumulated.successful_sources.sort();
                accumulated
                    .failures
                    .sort_by(|a, b| a.backend.cmp(&b.backend));
                stash.push((id, backend));
                emit(accumulated.clone());
            }
            (accumulated, stash, learned)
        });
        for (id, backend) in stash {
            self.backends.insert(id, backend);
        }
        for (id, noted) in learned {
            self.learn(&id, noted);
        }
        accumulated
    }

    /// One backend query with the same checks as [`Engine::ready`], consulting a
    /// remembered detection outcome first. Shared by the synchronous query and
    /// the streaming fan-out so both agree on success, failure, and validation.
    fn query_backend(
        backend: &mut dyn Backend,
        id: &str,
        noted: &mut Option<Availability>,
        capability: Capability,
        query: Option<&str>,
        exact: bool,
        cancel: &Cancellation,
    ) -> Result<(Vec<Package>, Vec<EngineError>), EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        if !backend.capabilities().contains(&capability) {
            return Err(backend.unsupported(capability));
        }
        Self::available_backend(backend, id, noted, cancel)?;
        // An update check refreshes before listing. A fetch failure still
        // lists what is already known and reports the failure beside those rows.
        let mut index_error = None;
        if query.is_none() {
            match read_again_once(cancel, || backend.refresh_update_index(cancel)) {
                Ok(()) => {}
                Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
                Err(error) => index_error = Some(error),
            }
        }
        let packages = read_again_once(cancel, || match query {
            Some(name) if exact => backend.lookup(name, cancel),
            Some(query) => backend.search(query, cancel),
            None => backend.installed(cancel),
        })?;
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
        let mut errors = backend.query_errors();
        if let Some(error) = index_error {
            errors.insert(0, error);
        }
        Ok((packages, errors))
    }

    /// Queries and index refreshes honor the same cached detection outcomes.
    /// A fresh detection's outcome is left in `noted` for the engine to
    /// remember. Callers check cancellation first.
    fn available_backend(
        backend: &mut dyn Backend,
        id: &str,
        noted: &mut Option<Availability>,
        cancel: &Cancellation,
    ) -> Result<(), EngineError> {
        let fresh = noted.is_none();
        let availability = match noted {
            Some(availability) => availability.clone(),
            None => noted.insert(backend.detect(cancel)?).clone(),
        };
        if let Availability::Unavailable(reason) = availability {
            return Err(EngineError::Unavailable {
                backend: id.into(),
                reason,
            });
        }
        // Detection may have taken time; do not start a query after
        // cancellation.
        if fresh && cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        Ok(())
    }
    /// Keep what a worker's detection learned for later queries.
    fn learn(&mut self, id: &str, noted: Option<Availability>) {
        if let Some(availability) = noted {
            self.detected.insert(id.into(), availability);
        }
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
        // An optional source counts as registered once a query found it.
        let found = !self.optional.contains(&id.backend)
            || matches!(
                self.detected.get(&id.backend),
                Some(Availability::Available)
            );
        let backend = self
            .backends
            .get_mut(&id.backend)
            .filter(|_| found)
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
                            reason:
                                "This cleanup task changed or expired. Reload and review it again."
                                    .into(),
                        });
                    }
                }
                if let Some(expected) = &expected_plan {
                    let current = backend.operation_plan(operation, cancel)?;
                    if current.as_ref() != Some(expected) {
                        return Err(EngineError::InvalidResponse {
                            backend: operation.backend().into(),
                            reason: "The planned changes are different now. Review them again."
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
        // Even a failed write may have changed something.
        crate::cache::invalidate(operation.backend());
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
                    reason: "This cleanup task changed or expired. Reload and review it again."
                        .into(),
                });
            }
        }
        if let Some(expected) = expected_plan {
            let current = backend.operation_plan(operation, cancel)?;
            if current.as_ref() != Some(&expected) {
                return Err(EngineError::InvalidResponse {
                    backend: operation.backend().into(),
                    reason: "The planned changes are different now. Review them again.".into(),
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
            .map(|operation| {
                // A source that would ask for a password, or that needs
                // someone there (firmware), never runs unattended.
                if self.unattended
                    && crate::unattended::unattended(operation.backend())
                        == crate::unattended::Unattended::Never
                {
                    return Err(EngineError::InvalidResponse {
                        backend: operation.backend().into(),
                        reason: "This source is never updated automatically.".into(),
                    });
                }
                self.preflight(operation, cancel)
            })
            .collect();
        if checked.iter().any(Result::is_err) {
            return operations.iter().zip(checked).map(|(operation, result)| {
                let result = Err(result.err().unwrap_or_else(|| EngineError::InvalidResponse {
                    backend: operation.backend().into(),
                    reason: "Another change in this batch could not be checked. Review the batch again.".into(),
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
        let guard =
            match crate::batch::begin(&host, authorization, self.batch_mode, operations, cancel) {
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
                    "Batch helper not found. You may be asked for permission more than once."
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
                crate::cache::invalidate("apt");
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
        // With the reviewed plan still current, both writes run.
        let mut engine = Engine::default();
        engine.register(UserFixture(writes.clone())).unwrap();
        engine
            .register(AptFixture {
                reads: 0,
                drift: false,
                writes: writes.clone(),
            })
            .unwrap();
        engine.enable_batch_authorization(
            Host::new(Runtime::Native, Default::default()),
            Authorization::Polkit,
        );
        engine.plan_apt_upgrade(&cancel).unwrap();
        let results = engine.execute_batch(&operations, &cancel, &mut |_| {});
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(writes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn unattended_batches_refuse_sources_that_need_someone_there() {
        struct Fixture(&'static str, Arc<AtomicUsize>);
        impl Backend for Fixture {
            fn id(&self) -> &str {
                self.0
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
                self.1.fetch_add(1, Ordering::SeqCst);
                Ok(OperationOutcome::default())
            }
        }
        let writes = Arc::new(AtomicUsize::new(0));
        let mut engine = Engine::default();
        engine.register(Fixture("fwupd", writes.clone())).unwrap();
        engine.register(Fixture("user", writes.clone())).unwrap();
        engine.enable_batch_authorization(
            Host::new(Runtime::Native, Default::default()),
            Authorization::Polkit,
        );
        engine.set_batch_mode(crate::batch::BatchMode::UpgradeOnly { removals: false });
        engine.set_unattended(true);
        let operations = ["fwupd", "user"].map(|backend| Operation::Refresh {
            backend: backend.into(),
        });
        let results = engine.execute_batch(&operations, &Cancellation::default(), &mut |_| {});
        assert!(matches!(
            &results[0],
            Err(EngineError::InvalidResponse { backend, reason })
                if backend == "fwupd" && reason.contains("never updated automatically")
        ));
        assert!(matches!(
            &results[1],
            Err(EngineError::InvalidResponse { reason, .. }) if reason.contains("could not be checked")
        ));
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        // With someone there, the same source runs.
        engine.set_unattended(false);
        let results = engine.execute_batch(&operations[1..], &Cancellation::default(), &mut |_| {});
        assert!(results.iter().all(Result::is_ok), "{results:?}");
        assert_eq!(writes.load(Ordering::SeqCst), 1);
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
                adopts: None,
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

#[cfg(test)]
mod update_check_tests {
    use super::*;
    use std::{sync::mpsc, thread, time::Duration};

    fn failed_read() -> EngineError {
        EngineError::Execution(ExecutionError::Failed(crate::process::Completion {
            code: Some(1),
            signal: None,
            stdout: vec![],
            stderr: b"busy for a moment".to_vec(),
            truncated: false,
            cancellation_deferred: false,
        }))
    }

    #[test]
    fn a_read_that_fails_once_is_tried_again() {
        for first in [
            failed_read(),
            EngineError::Execution(ExecutionError::LockBusy),
        ] {
            let mut calls = 0;
            let result = read_again_once(&Cancellation::default(), || {
                calls += 1;
                if calls == 1 {
                    Err(first.clone())
                } else {
                    Ok("rows")
                }
            });
            assert_eq!(result.unwrap(), "rows");
            assert_eq!(calls, 2);
        }
    }

    #[test]
    fn a_read_that_fails_twice_reports_the_second_failure() {
        let mut calls = 0;
        let result: Result<(), _> = read_again_once(&Cancellation::default(), || {
            calls += 1;
            Err(if calls == 1 {
                failed_read()
            } else {
                EngineError::Execution(ExecutionError::LockBusy)
            })
        });
        assert!(matches!(
            result,
            Err(EngineError::Execution(ExecutionError::LockBusy))
        ));
        assert_eq!(calls, 2);
    }

    #[test]
    fn timeouts_and_other_errors_are_not_tried_again() {
        for error in [
            EngineError::Execution(ExecutionError::TimedOut),
            EngineError::Execution(ExecutionError::Io("no such file".into())),
            EngineError::NotFound,
            EngineError::Cancelled,
        ] {
            let mut calls = 0;
            let result: Result<(), _> = read_again_once(&Cancellation::default(), || {
                calls += 1;
                Err(error.clone())
            });
            assert!(result.is_err());
            assert_eq!(calls, 1, "{error}");
        }
        let mut calls = 0;
        assert_eq!(
            read_again_once(&Cancellation::default(), || {
                calls += 1;
                Ok::<_, EngineError>(1)
            })
            .unwrap(),
            1
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn cancelling_skips_the_second_try() {
        let cancel = Cancellation::default();
        let mut calls = 0;
        let result: Result<(), _> = read_again_once(&cancel, || {
            calls += 1;
            cancel.cancel();
            Err(failed_read())
        });
        assert!(matches!(result, Err(EngineError::Cancelled)));
        assert_eq!(calls, 1);
    }

    #[test]
    fn a_check_without_a_live_token_does_not_refresh() {
        assert_eq!(
            once_per_check(0, || panic!("no live check, nothing to refresh")),
            Ok(())
        );
    }

    #[test]
    fn a_second_caller_waits_for_the_first_result() {
        let token = begin_update_check_token();
        let (started_tx, started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<()>();
        let first = thread::spawn(move || {
            once_per_check(token, || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Err(EngineError::Cancelled)
            })
        });
        started.recv().unwrap();
        let second =
            thread::spawn(move || once_per_check(token, || panic!("the first caller refreshes")));
        // Let the second caller reach the wait before the first finishes.
        thread::sleep(Duration::from_millis(100));
        release.send(()).unwrap();
        assert_eq!(first.join().unwrap(), Err(EngineError::Cancelled));
        assert_eq!(second.join().unwrap(), Err(EngineError::Cancelled));
        end_update_check_token(token);
    }

    #[test]
    fn a_refresh_that_panics_still_finishes_the_check() {
        let token = begin_update_check_token();
        let panicked =
            std::panic::catch_unwind(|| once_per_check(token, || panic!("refresh stopped")));
        assert!(panicked.is_err());
        assert_eq!(
            once_per_check(token, || panic!("the check already ran")),
            Err(EngineError::Execution(ExecutionError::Invalid(
                "update check stopped before it finished".into()
            )))
        );
        end_update_check_token(token);
    }
}

#[cfg(test)]
mod read_again_tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn busy() -> EngineError {
        EngineError::Execution(ExecutionError::LockBusy)
    }
    fn package(name: &str) -> Package {
        Package {
            id: PackageId {
                backend: "flaky".into(),
                name: name.into(),
                architecture: "amd64".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: name.into(),
            summary: String::new(),
            installed_version: Some("1".into()),
            candidate_version: Some("1".into()),
            update: UpdateAvailability::Current,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        }
    }
    /// Busy on every first read, and always busy refreshing its index.
    #[derive(Default)]
    struct Flaky {
        reads: Arc<AtomicUsize>,
        refreshes: Arc<AtomicUsize>,
    }
    impl Flaky {
        fn read(&self, name: &str) -> Result<Vec<Package>, EngineError> {
            if self.reads.fetch_add(1, Ordering::SeqCst).is_multiple_of(2) {
                Err(busy())
            } else {
                Ok(vec![package(name)])
            }
        }
    }
    impl Backend for Flaky {
        fn id(&self) -> &str {
            "flaky"
        }
        fn capabilities(&self) -> &[Capability] {
            &[Capability::Search, Capability::Installed]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn has_update_index(&self) -> bool {
            true
        }
        fn refresh_update_index(&mut self, _: &Cancellation) -> Result<(), EngineError> {
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            Err(busy())
        }
        fn search(&mut self, _: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
            self.read("searched")
        }
        fn lookup(&mut self, _: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
            self.read("looked-up")
        }
        fn installed(&mut self, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
            self.read("installed")
        }
    }
    fn names(report: &PackageReport) -> Vec<&str> {
        report.packages.iter().map(|p| p.id.name.as_str()).collect()
    }

    #[test]
    fn searches_lookups_and_listings_survive_one_busy_read() {
        let flaky = Flaky::default();
        let reads = flaky.reads.clone();
        let mut engine = Engine::default();
        engine.register(flaky).unwrap();
        let cancel = Cancellation::default();
        let search = engine.search("x", &cancel);
        assert_eq!(names(&search), ["searched"]);
        assert!(search.failures.is_empty());
        let lookup = engine.lookup("x", &cancel);
        assert_eq!(names(&lookup), ["looked-up"]);
        assert!(lookup.failures.is_empty());
        // The listing still arrives; only the busy index is reported.
        let installed = engine.installed(&cancel);
        assert_eq!(names(&installed), ["installed"]);
        assert_eq!(installed.failures.len(), 1);
        assert_eq!(reads.load(Ordering::SeqCst), 6);
    }

    #[test]
    fn an_index_that_stays_busy_is_reported_after_a_second_try() {
        let flaky = Flaky::default();
        let refreshes = flaky.refreshes.clone();
        let mut engine = Engine::default();
        engine.register(flaky).unwrap();
        let failures = engine.refresh_update_indexes(&[], &Cancellation::default());
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].backend, "flaky");
        assert!(matches!(
            failures[0].error,
            EngineError::Execution(ExecutionError::LockBusy)
        ));
        assert_eq!(refreshes.load(Ordering::SeqCst), 2);
    }
}
