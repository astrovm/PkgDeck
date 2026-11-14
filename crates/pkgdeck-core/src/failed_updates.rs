//! Updates that failed, with the version each one couldn't update to and
//! how many times. Update all leaves them out until a newer version comes
//! out, so one broken update doesn't fail every run. Automatic runs leave
//! one out once it failed twice: a run that lost the network or stopped
//! for a shutdown doesn't hold its updates back. The person can still
//! update one on its own, and an update that works is forgotten.
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::package::PackageId;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FailedUpdate {
    pub id: PackageId,
    pub version: String,
    /// How many times it failed at this version. 0 in a store from
    /// PkgDeck 0.15.2 or older, which also kept updates its system helper
    /// refused or never started: those are forgotten.
    #[serde(default)]
    pub failures: u32,
}

#[derive(Debug, Default)]
pub struct FailedUpdates {
    path: Option<PathBuf>,
    updates: Vec<FailedUpdate>,
}

impl FailedUpdates {
    /// Kept next to the activity history.
    pub fn default_store() -> Self {
        crate::activity::default_path()
            .and_then(|path| Some(path.parent()?.join("failed-updates.json")))
            .map_or_else(Self::default, |path| Self::at(&path))
    }
    pub fn at(path: &Path) -> Self {
        let mut updates: Vec<FailedUpdate> = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        updates.retain(|update| update.failures > 0);
        Self {
            path: Some(path.to_owned()),
            updates,
        }
    }
    /// Whether this update to `version` failed before.
    pub fn skips(&self, id: &PackageId, version: &str) -> bool {
        self.failures(id, version) > 0
    }
    /// Whether automatic runs leave this update to `version` out: it failed
    /// more than once, so it isn't a lost network or a shutdown.
    pub fn keeps_failing(&self, id: &PackageId, version: &str) -> bool {
        self.failures(id, version) > 1
    }
    fn failures(&self, id: &PackageId, version: &str) -> u32 {
        self.updates
            .iter()
            .find(|update| update.id == *id && update.version == version)
            .map_or(0, |update| update.failures)
    }
    pub fn updates(&self) -> &[FailedUpdate] {
        &self.updates
    }
    /// Remember the updates that failed and forget the ones that worked.
    pub fn remember(&mut self, failed: &[(PackageId, String)], worked: &[PackageId]) {
        let before = self.updates.clone();
        let failures: Vec<FailedUpdate> = failed
            .iter()
            .map(|(id, version)| FailedUpdate {
                id: id.clone(),
                version: version.clone(),
                failures: self.failures(id, version) + 1,
            })
            .collect();
        self.updates.retain(|update| {
            !worked.contains(&update.id) && !failed.iter().any(|(id, _)| *id == update.id)
        });
        self.updates.extend(failures);
        if self.updates != before {
            self.save();
        }
    }
    /// Forget updates no longer waiting at the version that failed:
    /// `waiting` is every update listed now from sources that answered.
    pub fn keep_waiting(&mut self, answered: &[&str], waiting: &[(PackageId, String)]) {
        let before = self.updates.len();
        self.updates.retain(|update| {
            !answered.contains(&update.id.backend.as_str())
                || waiting
                    .iter()
                    .any(|(id, version)| *id == update.id && *version == update.version)
        });
        if self.updates.len() != before {
            self.save();
        }
    }
    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let written = serde_json::to_vec(&self.updates)
            .ok()
            .is_some_and(|bytes| fs::write(&temporary, bytes).is_ok());
        if written {
            let _ = fs::rename(&temporary, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::Scope;

    fn id(backend: &str, name: &str) -> PackageId {
        PackageId {
            backend: backend.into(),
            name: name.into(),
            architecture: "aarch64".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        }
    }
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pkgdeck-failed-updates-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn skips_a_failed_update_until_it_works_or_a_newer_version_comes() {
        let dir = scratch("skip");
        let path = dir.join("failed-updates.json");
        let tool = id("example", "tool");
        let other = id("example", "other");
        let mut store = FailedUpdates::at(&path);
        assert!(!store.skips(&tool, "2.0"));
        store.remember(&[(tool.clone(), "2.0".into())], &[]);
        assert!(store.skips(&tool, "2.0"));
        assert!(!store.skips(&tool, "2.1"), "a newer version is tried again");
        assert!(!store.skips(&other, "2.0"), "only that package is skipped");
        // It survives a restart.
        let mut store = FailedUpdates::at(&path);
        assert!(store.skips(&tool, "2.0"));
        // Failing again at a newer version replaces the old entry.
        store.remember(&[(tool.clone(), "2.1".into())], &[]);
        assert_eq!(store.updates().len(), 1);
        assert!(store.skips(&tool, "2.1"));
        // Updated on its own: forgotten.
        store.remember(&[], std::slice::from_ref(&tool));
        assert!(!FailedUpdates::at(&path).skips(&tool, "2.1"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn automatic_runs_skip_an_update_once_it_failed_twice() {
        let dir = scratch("twice");
        let path = dir.join("failed-updates.json");
        let tool = id("example", "tool");
        let mut store = FailedUpdates::at(&path);
        store.remember(&[(tool.clone(), "2.0".into())], &[]);
        assert!(store.skips(&tool, "2.0") && !store.keeps_failing(&tool, "2.0"));
        store.remember(&[(tool.clone(), "2.0".into())], &[]);
        assert!(FailedUpdates::at(&path).keeps_failing(&tool, "2.0"));
        // A newer version starts over.
        store.remember(&[(tool.clone(), "2.1".into())], &[]);
        assert!(store.skips(&tool, "2.1") && !store.keeps_failing(&tool, "2.1"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failures_kept_before_they_were_counted_are_forgotten() {
        let dir = scratch("uncounted");
        let path = dir.join("failed-updates.json");
        fs::create_dir_all(&dir).unwrap();
        let tool = id("example", "tool");
        let old = serde_json::json!([{"id": tool, "version": "2.0"}]);
        fs::write(&path, old.to_string()).unwrap();
        let store = FailedUpdates::at(&path);
        assert!(store.updates().is_empty());
        assert!(!store.skips(&tool, "2.0"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn forgets_updates_that_are_no_longer_waiting() {
        let dir = scratch("waiting");
        let path = dir.join("failed-updates.json");
        let tool = id("example", "tool");
        let cask = id("homebrew-cask", "app");
        let mut store = FailedUpdates::at(&path);
        store.remember(
            &[(tool.clone(), "2.0".into()), (cask.clone(), "5".into())],
            &[],
        );
        // A source that didn't answer keeps its entries.
        store.keep_waiting(&["example"], &[(tool.clone(), "2.0".into())]);
        assert!(store.skips(&tool, "2.0") && store.skips(&cask, "5"));
        // Updated somewhere else, or a newer version listed: forgotten.
        store.keep_waiting(&["example", "homebrew-cask"], &[(cask.clone(), "6".into())]);
        assert!(store.updates().is_empty());
        assert!(FailedUpdates::at(&path).updates().is_empty());
        // A broken file reads as empty.
        fs::write(&path, "not json").unwrap();
        assert!(FailedUpdates::at(&path).updates().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}
