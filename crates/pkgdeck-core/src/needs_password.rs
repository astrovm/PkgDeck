//! Homebrew casks an automatic update couldn't finish because their
//! installer asked for the administrator password, which automatic runs
//! never show. Automatic runs skip each one until a newer version comes
//! out; the person updates it from Updates.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Default)]
pub struct NeedsPassword {
    path: Option<PathBuf>,
    /// Cask name to the version it couldn't update to.
    casks: BTreeMap<String, String>,
}

impl NeedsPassword {
    /// Kept next to the activity history.
    pub fn default_store() -> Self {
        crate::activity::default_path()
            .and_then(|path| Some(path.parent()?.join("needs-password.json")))
            .map_or_else(Self::default, |path| Self::at(&path))
    }
    pub fn at(path: &Path) -> Self {
        let casks = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path: Some(path.to_owned()),
            casks,
        }
    }
    /// Whether an automatic run should leave this update alone.
    pub fn skips(&self, name: &str, version: &str) -> bool {
        self.casks.get(name).is_some_and(|saved| saved == version)
    }
    /// Remember these updates and forget casks no longer waiting on one:
    /// `waiting` is every cask update listed now, by name and version.
    pub fn remember(&mut self, needed: &[(String, String)], waiting: &[(String, String)]) {
        let before = self.casks.clone();
        self.casks
            .retain(|name, version| waiting.iter().any(|(n, v)| n == name && v == version));
        self.casks.extend(needed.iter().cloned());
        if self.casks != before {
            self.save();
        }
    }
    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let written = serde_json::to_vec(&self.casks)
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

    fn pair(name: &str, version: &str) -> (String, String) {
        (name.into(), version.into())
    }

    #[test]
    fn skips_a_cask_until_a_newer_version_and_survives_restarts() {
        let dir =
            std::env::temp_dir().join(format!("pkgdeck-needs-password-{}", std::process::id()));
        let path = dir.join("needs-password.json");
        let mut store = NeedsPassword::at(&path);
        assert!(!store.skips("example-app", "2.0"));
        store.remember(&[pair("example-app", "2.0")], &[pair("example-app", "2.0")]);
        assert!(store.skips("example-app", "2.0"));
        assert!(
            !store.skips("example-app", "2.1"),
            "a newer version is tried again"
        );
        let reopened = NeedsPassword::at(&path);
        assert!(reopened.skips("example-app", "2.0"));
        // Updated by hand: it is no longer listed, so it is forgotten.
        let mut reopened = reopened;
        reopened.remember(&[], &[pair("other-app", "1.0")]);
        assert!(!NeedsPassword::at(&path).skips("example-app", "2.0"));
        fs::remove_dir_all(dir).unwrap();
    }
}
