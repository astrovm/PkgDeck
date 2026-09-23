//! Policy for opt-in, read-only update checks. The caller supplies a clock and
//! network state so scheduling can be tested without waiting or networking.
use crate::{
    engine::PackageReport,
    package::{PackageId, UpdateAvailability},
};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

pub const CHECK_INTERVAL_SECONDS: u64 = 30 * 60;

#[derive(Default)]
pub struct Schedule {
    next_due: u64,
    last_complete_set: Option<Vec<(PackageId, Option<String>)>>,
}
pub struct CheckResult {
    pub count: usize,
    pub failures: usize,
    pub changed: bool,
}
impl Schedule {
    pub fn ready(
        &mut self,
        now: u64,
        enabled: bool,
        offline: bool,
        metered: bool,
        busy: bool,
        force: bool,
    ) -> bool {
        if !enabled && !force || offline || metered || busy || !force && now < self.next_due {
            return false;
        }
        self.next_due = now.saturating_add(CHECK_INTERVAL_SECONDS);
        true
    }
    pub fn complete(&mut self, report: &PackageReport) -> CheckResult {
        let mut updates: Vec<_> = report
            .packages
            .iter()
            .filter(|package| package.update == UpdateAvailability::Available)
            .map(|package| (package.id.clone(), package.candidate_version.clone()))
            .collect();
        updates.sort();
        updates.dedup();
        let changed = report.failures.is_empty()
            && self
                .last_complete_set
                .as_ref()
                .is_some_and(|previous| previous != &updates);
        if report.failures.is_empty() {
            self.last_complete_set = Some(updates.clone());
        }
        CheckResult {
            count: updates.len(),
            failures: report.failures.len(),
            changed,
        }
    }
}

pub fn autostart_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| Path::new(p).is_absolute())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("autostart/io.github.astrovm.PkgDeck.desktop"))
}
pub fn set_autostart(path: &Path, enabled: bool) -> io::Result<()> {
    if !enabled {
        if path.exists() {
            fs::remove_file(path)?;
        }
        return Ok(());
    }
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid autostart path"))?,
    )?;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    file.write_all(b"[Desktop Entry]\nType=Application\nName=PkgDeck\nExec=pkgdeck --background\nIcon=io.github.astrovm.PkgDeck\nX-GNOME-Autostart-enabled=true\n")?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schedule_skips_offline_metered_busy_and_resume_bursts() {
        let mut schedule = Schedule::default();
        assert!(!schedule.ready(10, false, false, false, false, false));
        assert!(!schedule.ready(10, true, true, false, false, false));
        assert!(!schedule.ready(10, true, false, true, false, false));
        assert!(!schedule.ready(10, true, false, false, true, false));
        assert!(schedule.ready(10, true, false, false, false, false));
        assert!(!schedule.ready(20, true, false, false, false, false));
        assert!(schedule.ready(
            10 + CHECK_INTERVAL_SECONDS,
            true,
            false,
            false,
            false,
            false
        ));
        assert!(!schedule.ready(
            10 + CHECK_INTERVAL_SECONDS + 1,
            true,
            true,
            false,
            false,
            true
        ));
    }
    #[test]
    fn changed_complete_set_notifies_once_but_partial_does_not_replace_baseline() {
        let mut schedule = Schedule::default();
        let mut report = PackageReport::default();
        assert!(!schedule.complete(&report).changed);
        assert!(!schedule.complete(&report).changed);
        let package = serde_json::from_value(serde_json::json!({
            "id": {"backend":"apt", "name":"synthetic", "architecture":"amd64", "scope":"system"},
            "display_name":"Synthetic", "summary":"Test", "installed_version":"1", "candidate_version":"2", "update":"available"
        })).unwrap();
        report.packages.push(package);
        assert!(schedule.complete(&report).changed);
        assert!(!schedule.complete(&report).changed);
        // A failed source cannot prove that the update set shrank.
        report.packages.clear();
        report.failures.push(crate::engine::BackendFailure {
            backend: "fixture".into(),
            error: crate::engine::EngineError::Cancelled,
        });
        assert!(!schedule.complete(&report).changed);
        report.failures.clear();
        assert!(schedule.complete(&report).changed);
        assert!(!schedule.complete(&report).changed);
    }
    #[test]
    fn autostart_is_reversible() {
        let path = std::env::temp_dir()
            .join(format!("pkgdeck-autostart-{}", std::process::id()))
            .join("app.desktop");
        set_autostart(&path, true).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("--background"));
        set_autostart(&path, false).unwrap();
        assert!(!path.exists());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
