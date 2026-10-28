//! Bounded, local operation history shared by GUI and CLI.
//! Native progress and error output is deliberately never persisted, only
//! PkgDeck's own sentence about why a change didn't finish.
use crate::{
    engine::{EngineError, Refusal},
    package::{Operation, OperationOutcome},
};
use rustix::fs::{flock, FlockOperation};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const LIMIT: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Queued,
    Authorizing,
    Running,
    Finished,
    Failed,
    Cancelled,
    Interrupted,
}
impl State {
    fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Finished | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Finished,
    Failed,
    Cancelled,
    /// It waits for a build to finish, then runs.
    Waiting,
    /// It was left alone, as its reason says.
    Skipped,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    pub frontend: String,
    pub operations: Vec<Operation>,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub state: State,
    pub outcomes: Vec<Outcome>,
    /// Why each change didn't finish, beside its outcome, when one says.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<Option<String>>,
    /// What each change was called when it ran, when PkgDeck knew: a
    /// cleanup task's title can't be found again once it's done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<Option<String>>,
    pub owner_pid: u32,
}

pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|p| Path::new(p).is_absolute())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(base.join("pkgdeck/activity.json"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone)]
pub struct History {
    path: PathBuf,
}
impl History {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn default_store() -> Option<Self> {
        default_path().map(Self::new)
    }

    fn edit<T>(&self, mutate: impl FnOnce(&mut Vec<Entry>) -> T) -> io::Result<T> {
        let parent = self.path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "missing activity directory")
        })?;
        fs::create_dir_all(parent)?;
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(self.path.with_extension("lock"))?;
        flock(&lock, FlockOperation::LockExclusive)?;
        let mut entries: Vec<Entry> = fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let value = mutate(&mut entries);
        if entries.len() > LIMIT {
            entries.drain(..entries.len() - LIMIT);
        }
        let temporary = self
            .path
            .with_extension(format!("{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        serde_json::to_writer(&mut file, &entries)?;
        file.sync_all()?;
        fs::rename(temporary, &self.path)?;
        let _ = File::open(parent)?.sync_all();
        Ok(value)
    }
    pub fn entries(&self) -> io::Result<Vec<Entry>> {
        self.edit(|entries| entries.clone())
    }
    pub fn begin(
        &self,
        frontend: &str,
        operations: Vec<Operation>,
        names: Vec<Option<String>>,
        state: State,
    ) -> io::Result<u64> {
        self.edit(|entries| {
            let id = entries.last().map_or(1, |entry| entry.id + 1);
            entries.push(Entry {
                id,
                frontend: frontend.into(),
                operations,
                started_at: now(),
                finished_at: None,
                state,
                outcomes: vec![],
                reasons: vec![],
                names: said(names),
                owner_pid: std::process::id(),
            });
            id
        })
    }
    /// Adds a run that already ended, like automatic cleaning, which only
    /// knows what it did once it is done.
    pub fn record(
        &self,
        frontend: &str,
        operations: Vec<Operation>,
        names: Vec<Option<String>>,
        started_at: u64,
        ended: Vec<(Outcome, Option<String>)>,
    ) -> io::Result<u64> {
        let (outcomes, reasons) = ended.into_iter().unzip::<_, _, Vec<_>, Vec<_>>();
        self.edit(|entries| {
            let id = entries.last().map_or(1, |entry| entry.id + 1);
            entries.push(Entry {
                id,
                frontend: frontend.into(),
                operations,
                started_at,
                finished_at: Some(now()),
                state: ended_as(&outcomes),
                reasons: said(reasons),
                outcomes,
                names: said(names),
                owner_pid: std::process::id(),
            });
            id
        })
    }
    pub fn state(&self, id: u64, state: State) -> io::Result<()> {
        self.edit(|entries| {
            if let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) {
                if state.terminal() {
                    entry.finished_at = Some(now());
                }
                entry.state = state;
            }
        })
    }
    /// How each change ended, in order, and why when it didn't finish.
    pub fn finish(&self, id: u64, ended: Vec<(Outcome, Option<String>)>) -> io::Result<()> {
        let (outcomes, reasons) = ended.into_iter().unzip::<_, _, Vec<_>, Vec<_>>();
        self.edit(|entries| {
            if let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) {
                entry.state = ended_as(&outcomes);
                entry.outcomes = outcomes;
                entry.reasons = said(reasons);
                entry.finished_at = Some(now());
            }
        })
    }
    /// How the change at `index` of run `id` ended after all, like a
    /// cleanup task that waited for a build.
    pub fn settle(
        &self,
        id: u64,
        index: usize,
        outcome: Outcome,
        reason: Option<String>,
    ) -> io::Result<()> {
        self.edit(|entries| {
            let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) else {
                return;
            };
            if index >= entry.outcomes.len() {
                return;
            }
            entry.outcomes[index] = outcome;
            entry.reasons.resize(entry.outcomes.len(), None);
            entry.reasons[index] = reason;
            entry.reasons = said(std::mem::take(&mut entry.reasons));
            entry.state = ended_as(&entry.outcomes);
            entry.finished_at = Some(now());
        })
    }
    pub fn recover_dead(&self) -> io::Result<()> {
        self.edit(|entries| {
            for entry in entries {
                // A signal-0 probe works on both Linux and macOS, which has no
                // /proc. Permission errors still mean the process may be alive.
                let alive = i32::try_from(entry.owner_pid)
                    .ok()
                    .and_then(rustix::process::Pid::from_raw)
                    .is_some_and(|pid| {
                        rustix::process::test_kill_process(pid) != Err(rustix::io::Errno::SRCH)
                    });
                if alive {
                    continue;
                }
                if !entry.state.terminal() {
                    entry.state = State::Interrupted;
                    entry.finished_at = Some(now());
                }
                // Nothing runs what it left waiting anymore.
                if entry.outcomes.contains(&Outcome::Waiting) {
                    entry.reasons.resize(entry.outcomes.len(), None);
                    for (outcome, reason) in entry.outcomes.iter_mut().zip(&mut entry.reasons) {
                        if *outcome == Outcome::Waiting {
                            *outcome = Outcome::Skipped;
                            *reason = Some(CLOSED_WHILE_WAITING.into());
                        }
                    }
                }
            }
        })
    }
}

/// How a change that ran on its own ended, as Activity keeps it. One that
/// PkgDeck left alone, because a build uses what it removes or it changed
/// since it was reviewed, is skipped, not failed: nothing went wrong.
pub fn ended(result: &Result<OperationOutcome, EngineError>) -> (Outcome, Option<String>) {
    match result {
        Ok(_) => (Outcome::Finished, None),
        Err(EngineError::Cancelled) => (Outcome::Cancelled, None),
        Err(
            error @ EngineError::Refused {
                refusal: Refusal::Building | Refusal::CleanupChanged,
                ..
            },
        ) => (Outcome::Skipped, Some(reason(error))),
        Err(error) => (Outcome::Failed, Some(reason(error))),
    }
}

/// Why `error` stopped a change, as Activity keeps it. What a tool answered
/// stays out: it can be long, or say more than it should.
pub fn reason(error: &EngineError) -> String {
    let said = match error {
        // A name like npm keeps its own case.
        EngineError::InvalidResponse { backend, .. } => format!(
            "{} answered in a way PkgDeck didn't understand",
            crate::backends::display_name(backend)
        ),
        error => {
            let said = error.to_string();
            let mut letters = said.chars();
            let first = letters
                .next()
                .map(|first| first.to_uppercase().collect::<String>());
            first.unwrap_or_default() + letters.as_str()
        }
    };
    if said.ends_with('.') {
        said
    } else {
        said + "."
    }
}

/// Why a change that waited for a build never ran.
pub const CLOSED_WHILE_WAITING: &str =
    "PkgDeck closed before the build finished, so it didn't run.";

/// What a run whose changes ended as `outcomes` counts as. A change that
/// was skipped or waits did what PkgDeck meant it to, so it fails nothing.
fn ended_as(outcomes: &[Outcome]) -> State {
    if outcomes.contains(&Outcome::Failed) {
        State::Failed
    } else if outcomes.contains(&Outcome::Cancelled) {
        State::Cancelled
    } else {
        State::Finished
    }
}
/// `reasons` or `names` as stored: nothing at all when none is known.
fn said(reasons: Vec<Option<String>>) -> Vec<Option<String>> {
    if reasons.iter().all(Option::is_none) {
        vec![]
    } else {
        reasons
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{PackageId, Scope};
    #[test]
    fn bounded_history_recovers_and_stores_only_fixed_outcomes() {
        let path =
            std::env::temp_dir().join(format!("pkgdeck-activity-{}-{}", std::process::id(), now()));
        let store = History::new(path.join("activity.json"));
        for _ in 0..105 {
            let id = store
                .begin(
                    "test",
                    vec![Operation::Refresh {
                        backend: "apt".into(),
                    }],
                    vec![],
                    State::Queued,
                )
                .unwrap();
            store.finish(id, vec![(Outcome::Finished, None)]).unwrap();
        }
        let mut entries = store.entries().unwrap();
        assert_eq!(entries.len(), LIMIT);
        assert_eq!(entries[0].id, 6);
        let id = store.begin("test", vec![], vec![], State::Running).unwrap();
        store.recover_dead().unwrap();
        assert_eq!(
            store.entries().unwrap().last().unwrap().state,
            State::Running
        );
        store
            .edit(|rows| {
                rows.iter_mut()
                    .find(|entry| entry.id == id)
                    .unwrap()
                    .owner_pid = u32::MAX
            })
            .unwrap();
        store.recover_dead().unwrap();
        entries = store.entries().unwrap();
        assert_eq!(entries.last().unwrap().state, State::Interrupted);
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(path.join("activity.json")).unwrap()).unwrap();
        assert!(stored
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry.get("diagnostics").is_none()
                && entry.get("message").is_none()
                // No names known, none listed.
                && entry.get("names").is_none()));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn mixed_batch_preserves_targets_and_each_result() {
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-mixed-activity-{}-{}",
            std::process::id(),
            now()
        ));
        let store = History::new(path.join("activity.json"));
        let id = PackageId {
            backend: "fixture".into(),
            name: "synthetic".into(),
            architecture: "all".into(),
            scope: Scope::User { uid: 1234 },
            remote: None,
            reference: None,
        };
        let operations = vec![
            Operation::Install(id.clone()),
            Operation::Upgrade(id.clone()),
            Operation::Remove(id),
        ];
        let activity_id = store
            .begin(
                "cli",
                operations.clone(),
                vec![Some("Synthetic".into()), None, None],
                State::Running,
            )
            .unwrap();
        store
            .finish(
                activity_id,
                vec![
                    (Outcome::Finished, None),
                    (Outcome::Failed, None),
                    (Outcome::Cancelled, None),
                ],
            )
            .unwrap();
        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].operations, operations);
        assert_eq!(entries[0].names, [Some("Synthetic".into()), None, None]);
        assert_eq!(entries[0].state, State::Failed);
        assert_eq!(
            entries[0].outcomes,
            [Outcome::Finished, Outcome::Failed, Outcome::Cancelled]
        );
        assert!(entries[0].finished_at.is_some());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn state_changes_stamp_only_terminal_states_and_skip_unknown_entries() {
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-state-activity-{}-{}",
            std::process::id(),
            now()
        ));
        let store = History::new(path.join("activity.json"));
        let id = store.begin("gui", vec![], vec![], State::Queued).unwrap();
        store.state(id, State::Authorizing).unwrap();
        let entry = store.entries().unwrap().remove(0);
        assert_eq!((entry.state, entry.finished_at), (State::Authorizing, None));
        store.state(id, State::Cancelled).unwrap();
        let entry = store.entries().unwrap().remove(0);
        assert_eq!(entry.state, State::Cancelled);
        assert!(entry.finished_at.is_some());
        store.state(id + 1, State::Running).unwrap();
        store
            .finish(id + 1, vec![(Outcome::Finished, None)])
            .unwrap();
        let second = store.begin("cli", vec![], vec![], State::Running).unwrap();
        store
            .finish(
                second,
                vec![(Outcome::Finished, None), (Outcome::Cancelled, None)],
            )
            .unwrap();
        let states: Vec<_> = store
            .entries()
            .unwrap()
            .into_iter()
            .map(|entry| entry.state)
            .collect();
        assert_eq!(states, [State::Cancelled, State::Cancelled]);
        assert_eq!(
            History::new(PathBuf::from("/"))
                .entries()
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn a_change_that_waits_or_is_skipped_keeps_why_and_fails_nothing() {
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-waiting-activity-{}-{}",
            std::process::id(),
            now()
        ));
        let store = History::new(path.join("activity.json"));
        let clean = |key: &str| {
            Operation::Clean(crate::package::CleanupId {
                backend: "dev-caches".into(),
                key: key.into(),
            })
        };
        let waits = Some("Waits for a build.".to_owned());
        let id = store
            .record(
                "auto",
                vec![clean("cargo"), clean("bun"), clean("gradle")],
                vec![Some("Cargo downloads".into()), None, None],
                5,
                vec![
                    (Outcome::Waiting, waits.clone()),
                    (Outcome::Finished, None),
                    (Outcome::Skipped, Some("It changed.".into())),
                ],
            )
            .unwrap();
        let entry = store.entries().unwrap().remove(0);
        assert_eq!((entry.frontend.as_str(), entry.started_at), ("auto", 5));
        assert_eq!(entry.names, [Some("Cargo downloads".into()), None, None]);
        assert_eq!(entry.state, State::Finished);
        assert!(entry.finished_at.is_some());
        assert_eq!(
            entry.reasons,
            [waits.clone(), None, Some("It changed.".into())]
        );
        // The build ended: what waited ran, and needs no reason now.
        store.settle(id, 0, Outcome::Finished, None).unwrap();
        // Steps and runs that aren't there change nothing.
        store.settle(id, 9, Outcome::Failed, None).unwrap();
        store.settle(id + 1, 0, Outcome::Failed, None).unwrap();
        let entry = store.entries().unwrap().remove(0);
        assert_eq!(
            entry.outcomes,
            [Outcome::Finished, Outcome::Finished, Outcome::Skipped]
        );
        assert_eq!(entry.reasons, [None, None, Some("It changed.".into())]);
        store.settle(id, 2, Outcome::Finished, None).unwrap();
        let entry = store.entries().unwrap().remove(0);
        assert!(entry.reasons.is_empty());
        // Saved without reasons, the file doesn't name them.
        let stored = fs::read_to_string(path.join("activity.json")).unwrap();
        assert!(!stored.contains("reasons"), "{stored}");
        // One that failed fails the run.
        store
            .settle(id, 1, Outcome::Failed, Some("It broke.".into()))
            .unwrap();
        assert_eq!(store.entries().unwrap()[0].state, State::Failed);
        // Closing PkgDeck while one waits means it never runs.
        let waiting = store
            .record(
                "auto",
                vec![clean("cargo"), clean("bun")],
                vec![None, None],
                6,
                vec![(Outcome::Waiting, waits), (Outcome::Finished, None)],
            )
            .unwrap();
        store
            .edit(|rows| {
                for row in rows {
                    row.owner_pid = u32::MAX;
                }
            })
            .unwrap();
        store.recover_dead().unwrap();
        let entry = store
            .entries()
            .unwrap()
            .into_iter()
            .find(|entry| entry.id == waiting)
            .unwrap();
        assert_eq!(entry.outcomes, [Outcome::Skipped, Outcome::Finished]);
        assert_eq!(entry.reasons, [Some(CLOSED_WHILE_WAITING.into()), None]);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn how_a_change_ended_says_why_in_a_sentence_without_what_tools_said() {
        use crate::process::ExecutionError;
        assert_eq!(
            ended(&Ok(OperationOutcome::default())),
            (Outcome::Finished, None)
        );
        assert_eq!(
            ended(&Err(EngineError::Cancelled)),
            (Outcome::Cancelled, None)
        );
        for refusal in [Refusal::Building, Refusal::CleanupChanged] {
            let error = EngineError::Refused {
                backend: "dev-caches".into(),
                refusal: refusal.clone(),
            };
            assert_eq!(
                ended(&Err(error)),
                (Outcome::Skipped, Some(refusal.to_string()))
            );
        }
        let error = EngineError::InvalidResponse {
            backend: "npm".into(),
            reason: "secret output".into(),
        };
        assert_eq!(
            ended(&Err(error)),
            (
                Outcome::Failed,
                Some("npm answered in a way PkgDeck didn't understand.".into())
            )
        );
        assert_eq!(
            reason(&EngineError::Execution(ExecutionError::Io(
                "couldn't remove ~/x".into()
            ))),
            "Couldn't remove ~/x."
        );
        assert_eq!(
            reason(&EngineError::Execution(ExecutionError::Io(String::new()))),
            "."
        );
    }
}
