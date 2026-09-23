//! Bounded, local operation history shared by GUI and CLI.
//! Native progress and error strings are deliberately never persisted.
use crate::package::Operation;
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

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Finished,
    Failed,
    Cancelled,
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
    pub fn finish(&self, id: u64, outcomes: Vec<Outcome>) -> io::Result<()> {
        self.edit(|entries| {
            if let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) {
                entry.state = if outcomes.iter().all(|outcome| *outcome == Outcome::Finished) {
                    State::Finished
                } else if outcomes.contains(&Outcome::Failed) {
                    State::Failed
                } else {
                    State::Cancelled
                };
                entry.outcomes = outcomes;
                entry.finished_at = Some(now());
            }
        })
    }
    pub fn recover_dead(&self) -> io::Result<()> {
        self.edit(|entries| {
            for entry in entries {
                if !entry.state.terminal()
                    && !Path::new(&format!("/proc/{}", entry.owner_pid)).exists()
                {
                    entry.state = State::Interrupted;
                    entry.finished_at = Some(now());
                }
            }
        })
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
                    State::Queued,
                )
                .unwrap();
            store.finish(id, vec![Outcome::Finished]).unwrap();
        }
        let mut entries = store.entries().unwrap();
        assert_eq!(entries.len(), LIMIT);
        assert_eq!(entries[0].id, 6);
        let id = store.begin("test", vec![], State::Running).unwrap();
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
            .all(|entry| entry.get("diagnostics").is_none() && entry.get("message").is_none()));
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
            .begin("cli", operations.clone(), State::Running)
            .unwrap();
        store
            .finish(
                activity_id,
                vec![Outcome::Finished, Outcome::Failed, Outcome::Cancelled],
            )
            .unwrap();
        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].operations, operations);
        assert_eq!(entries[0].state, State::Failed);
        assert_eq!(
            entries[0].outcomes,
            [Outcome::Finished, Outcome::Failed, Outcome::Cancelled]
        );
        assert!(entries[0].finished_at.is_some());
        fs::remove_dir_all(path).unwrap();
    }
}
