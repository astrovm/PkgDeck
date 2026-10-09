//! Updates that failed, with the version each one couldn't update to.
//! Automatic runs and Update all leave them out until a newer version
//! comes out, so one broken update doesn't fail every run. The person can
//! still update one on its own, and an update that works is forgotten.
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
        let updates = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path.to_owned()),
            updates,
        }
    }
    /// Whether this update to `version` failed before.
    pub fn skips(&self, id: &PackageId, version: &str) -> bool {
        self.updates
            .iter()
            .any(|update| update.id == *id && update.version == version)
    }
    pub fn updates(&self) -> &[FailedUpdate] {
        &self.updates
    }
    /// Remember the updates that failed and forget the ones that worked.
    pub fn remember(&mut self, failed: &[(PackageId, String)], worked: &[PackageId]) {
        let before = self.updates.clone();
        self.updates.retain(|update| {
            !worked.contains(&update.id) && !failed.iter().any(|(id, _)| *id == update.id)
        });
        self.updates
            .extend(failed.iter().map(|(id, version)| FailedUpdate {
                id: id.clone(),
                version: version.clone(),
            }));
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
        store.remember(&[], &[tool.clone()]);
        assert!(!FailedUpdates::at(&path).skips(&tool, "2.1"));
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
