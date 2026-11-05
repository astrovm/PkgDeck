//! Waiting for new releases: a defense against a bad version that gets
//! published and caught within days. Off by default.
//!
//! Two layers work together. Package managers that can limit release age
//! themselves (npm, pnpm, Bun, uv, pip, pipx and mise) get that limit, which also
//! covers dependencies and picks the newest version that's old enough. Every
//! other source gets [`ReleaseWatch`]: PkgDeck remembers when it first saw
//! each new version and marks the update as waiting until enough time has
//! passed. A wait restarts when a newer version replaces the one waiting.
//!
//! Automatic updates, Update all and `pkd upgrade` leave waiting updates out.
//! Updating one package by hand still works. System sources never wait: their
//! distributions review what they ship, a wait would delay security fixes,
//! and unattended system updates can't leave out one package.
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::package::{Package, PackageId, UpdateAvailability};

const HOUR: u64 = 3600;
/// How long a new release waits, in hours, unless someone picks otherwise.
pub const DEFAULT_HOURS: u32 = 24;
/// The shortest and longest wait, in hours: a year at most.
pub const HOUR_RANGE: std::ops::RangeInclusive<u32> = 1..=8760;

/// Seconds since 1970, or 0 if the clock is before that.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// What the person chose, shared by the window and `pkd`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ReleaseWait {
    pub enabled: bool,
    /// How long a new release waits. Kept while it's off, for turning it
    /// back on.
    pub hours: u32,
    /// Sources that don't wait, such as a tool that ships every day.
    pub skipped: Vec<String>,
}
impl Default for ReleaseWait {
    fn default() -> Self {
        Self {
            enabled: false,
            hours: DEFAULT_HOURS,
            skipped: vec![],
        }
    }
}
impl ReleaseWait {
    /// Kept with the window's settings in `~/.config/pkgdeck`.
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|path| Path::new(path).is_absolute())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(base.join("pkgdeck/release-wait.json"))
    }
    /// Missing or unreadable settings mean off, the default.
    pub fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .map(|mut wait| {
                wait.hours = wait.hours.clamp(*HOUR_RANGE.start(), *HOUR_RANGE.end());
                wait
            })
            .unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        replace(path, &serde_json::to_vec_pretty(self)?)
    }
    /// Whether new releases from `backend` wait.
    pub fn applies_to(&self, backend: &str) -> bool {
        self.enabled && can_wait(backend) && !self.skipped.iter().any(|skip| skip == backend)
    }
}

/// Whether a source's updates can wait at all. See the module docs for why
/// system sources can't. Inventories never update.
pub fn can_wait(backend: &str) -> bool {
    !matches!(
        backend,
        "apt"
            | "dnf"
            | "pacman"
            | "zypper"
            | "apk"
            | "xbps"
            | "macports"
            | "fwupd"
            | "system-image"
            | "macos-updates"
            | "mas"
            | "toolbox"
            | "distrobox"
    ) && !crate::backends::inventory_only(backend)
}

/// One update PkgDeck has seen, and when it first did.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Seen {
    id: PackageId,
    version: String,
    at: u64,
}

/// Marks updates that are too new, remembering when each first showed up.
#[derive(Debug)]
pub struct ReleaseWatch {
    wait: ReleaseWait,
    path: Option<PathBuf>,
    /// Sources answer at the same time and share the file.
    writing: Mutex<()>,
}
impl ReleaseWatch {
    /// `path` keeps what was seen. Without one, every wait starts over at
    /// each check, which holds updates back rather than letting them through.
    pub fn new(wait: ReleaseWait, path: Option<PathBuf>) -> Self {
        Self {
            wait,
            path,
            writing: Mutex::new(()),
        }
    }
    /// The saved choice, with what was seen kept next to the activity
    /// history. Nothing to do while it's off.
    pub fn from_saved() -> Option<Self> {
        let wait = ReleaseWait::load(&ReleaseWait::default_path()?);
        wait.enabled.then(|| Self::new(wait, default_seen_path()))
    }
    pub fn wait(&self) -> &ReleaseWait {
        &self.wait
    }
    /// Mark what `backend` listed. `complete` says it listed everything it
    /// has, so anything it no longer lists can be forgotten.
    pub fn mark(&self, backend: &str, packages: &mut [Package], complete: bool, now: u64) {
        let _writing = self
            .writing
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut seen = self.read();
        let before = seen.clone();
        let wait = u64::from(self.wait.hours) * HOUR;
        let is_update = |package: &Package| package.update == UpdateAvailability::Available;
        for package in packages.iter_mut().filter(|package| is_update(package)) {
            let version = package.candidate_version.clone().unwrap_or_default();
            let first = match seen
                .iter_mut()
                .find(|entry| entry.id == package.id && entry.version == version)
            {
                Some(entry) => {
                    // A clock that was wrong can't make the wait longer.
                    entry.at = entry.at.min(now);
                    entry.at
                }
                None => {
                    seen.push(Seen {
                        id: package.id.clone(),
                        version,
                        at: now,
                    });
                    now
                }
            };
            let until = first.saturating_add(wait);
            package.waiting_until = (until > now).then_some(until);
        }
        if complete {
            seen.retain(|entry| {
                entry.id.backend != backend
                    || packages
                        .iter()
                        .find(|package| package.id == entry.id)
                        .is_some_and(|package| {
                            // Listed with no update, its check may just have
                            // failed. Only installing this version ends its wait.
                            if is_update(package) {
                                package.candidate_version.as_deref().unwrap_or_default()
                                    == entry.version
                            } else {
                                package.installed_version.as_deref() != Some(&entry.version)
                            }
                        })
            });
        }
        if seen != before {
            self.write(&seen);
        }
    }
    fn read(&self) -> Vec<Seen> {
        self.path
            .as_deref()
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
    fn write(&self, seen: &[Seen]) {
        let (Some(path), Ok(bytes)) = (&self.path, serde_json::to_vec(seen)) else {
            return;
        };
        // Not being able to save only restarts the waits.
        let _ = replace(path, &bytes);
    }
}

fn default_seen_path() -> Option<PathBuf> {
    Some(
        crate::activity::default_path()?
            .parent()?
            .join("release-wait-seen.json"),
    )
}

/// Replaces a file whole, so a crash or another process never sees half of
/// it. The window and `pkd` may write at the same time.
fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let partial = path.with_extension(format!("partial.{}", std::process::id()));
    fs::write(&partial, bytes)?;
    fs::rename(&partial, path).inspect_err(|_| {
        let _ = fs::remove_file(&partial);
    })
}

/// A wait in words, in the largest unit it's a whole number of.
pub fn hours_text(hours: u32) -> String {
    let (count, unit) = match hours {
        hours if hours.is_multiple_of(168) => (hours / 168, "week"),
        hours if hours.is_multiple_of(24) => (hours / 24, "day"),
        hours => (hours, "hour"),
    };
    format!("{count} {unit}{}", if count == 1 { "" } else { "s" })
}

/// The cutoff some managers take: this long ago, as an RFC 3339 time.
pub(crate) fn cutoff(hours: u32, now: u64) -> String {
    let seconds = now.saturating_sub(u64::from(hours) * HOUR);
    chrono::DateTime::from_timestamp(i64::try_from(seconds).unwrap_or(0), 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{PackageId, Scope};

    const NOW: u64 = 1_000_000_000;
    const DAY: u64 = 86_400;

    /// A folder of its own, gone when the test ends.
    struct Scratch(PathBuf);
    impl Scratch {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn scratch() -> Scratch {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pkgdeck-release-wait-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn update(name: &str, version: &str) -> Package {
        Package {
            id: PackageId {
                backend: "npm".into(),
                name: name.into(),
                architecture: "x86_64".into(),
                scope: Scope::User { uid: 1000 },
                remote: None,
                reference: None,
            },
            display_name: name.into(),
            summary: String::new(),
            installed_version: Some("1".into()),
            candidate_version: Some(version.into()),
            update: UpdateAvailability::Available,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
            waiting_until: None,
        }
    }
    fn watch(dir: &Path, days: u32) -> ReleaseWatch {
        ReleaseWatch::new(
            ReleaseWait {
                enabled: true,
                hours: days * 24,
                skipped: vec![],
            },
            Some(dir.join("seen.json")),
        )
    }

    #[test]
    fn a_new_version_waits_until_its_time_is_up() {
        let dir = scratch();
        let watch = watch(dir.path(), 2);
        let mut rows = [update("tool", "2")];

        watch.mark("npm", &mut rows, true, NOW);
        assert_eq!(rows[0].waiting_until, Some(NOW + 2 * DAY));

        // The next check doesn't restart the clock.
        rows[0].waiting_until = None;
        watch.mark("npm", &mut rows, true, NOW + DAY);
        assert_eq!(rows[0].waiting_until, Some(NOW + 2 * DAY));

        // One second short still waits; on time it's ready.
        watch.mark("npm", &mut rows, true, NOW + 2 * DAY - 1);
        assert_eq!(rows[0].waiting_until, Some(NOW + 2 * DAY));
        watch.mark("npm", &mut rows, true, NOW + 2 * DAY);
        assert_eq!(rows[0].waiting_until, None);
    }

    #[test]
    fn a_newer_version_starts_its_own_wait() {
        let dir = scratch();
        let watch = watch(dir.path(), 1);
        watch.mark("npm", &mut [update("tool", "2")], true, NOW);

        // Without this, a release days older than the bad one would open
        // the door for the bad one.
        let mut rows = [update("tool", "3")];
        watch.mark("npm", &mut rows, true, NOW + DAY);
        assert_eq!(rows[0].waiting_until, Some(NOW + 2 * DAY));
    }

    #[test]
    fn only_updates_wait() {
        let dir = scratch();
        let watch = watch(dir.path(), 1);
        let mut current = update("tool", "2");
        current.update = UpdateAvailability::Current;
        let mut unknown = update("other", "2");
        unknown.update = UpdateAvailability::Unknown;
        let mut rows = [current, unknown];
        watch.mark("npm", &mut rows, true, NOW);
        assert!(rows.iter().all(|row| row.waiting_until.is_none()));
        assert!(watch.read().is_empty());
    }

    #[test]
    fn an_update_with_no_known_version_waits_too() {
        let dir = scratch();
        let watch = watch(dir.path(), 1);
        let mut package = update("tool", "");
        package.candidate_version = None;
        let mut rows = [package];
        watch.mark("npm", &mut rows, true, NOW);
        assert_eq!(rows[0].waiting_until, Some(NOW + DAY));
    }

    #[test]
    fn what_a_source_stops_listing_is_forgotten_only_when_it_listed_everything() {
        let dir = scratch();
        let watch = watch(dir.path(), 1);
        let mut other = update("elsewhere", "2");
        other.id.backend = "pnpm".into();
        watch.mark("pnpm", &mut [other], true, NOW);
        watch.mark("npm", &mut [update("tool", "2")], true, NOW);

        // A source that failed partway says nothing about what's gone.
        watch.mark("npm", &mut [], false, NOW + 10);
        assert_eq!(watch.read().len(), 2);

        // Updated by hand: gone from npm, and other sources keep theirs.
        watch.mark("npm", &mut [], true, NOW + 10);
        let seen = watch.read();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].id.backend, "pnpm");

        // So if it shows up again, it waits from then.
        let mut rows = [update("tool", "2")];
        watch.mark("npm", &mut rows, true, NOW + 20);
        assert_eq!(rows[0].waiting_until, Some(NOW + 20 + DAY));
    }

    #[test]
    fn a_check_that_failed_quietly_doesnt_restart_the_wait() {
        let dir = scratch();
        let watch = watch(dir.path(), 1);
        watch.mark("npm", &mut [update("tool", "2")], true, NOW);

        // A manager that couldn't check lists the package with no update.
        let mut unchecked = update("tool", "2");
        unchecked.candidate_version = None;
        unchecked.update = UpdateAvailability::Current;
        watch.mark("npm", &mut [unchecked.clone()], true, NOW + 10);
        let mut rows = [update("tool", "2")];
        watch.mark("npm", &mut rows, true, NOW + 20);
        assert_eq!(rows[0].waiting_until, Some(NOW + DAY));

        // Installed by name: this version is done with.
        unchecked.installed_version = Some("2".into());
        watch.mark("npm", &mut [unchecked], true, NOW + 30);
        assert!(watch.read().is_empty());
    }

    #[test]
    fn a_newer_release_takes_the_place_of_the_one_waiting() {
        let dir = scratch();
        let watch = watch(dir.path(), 1);
        watch.mark("npm", &mut [update("tool", "2")], true, NOW);
        let mut rows = [update("tool", "3")];
        watch.mark("npm", &mut rows, true, NOW + 10);
        assert_eq!(rows[0].waiting_until, Some(NOW + 10 + DAY));
        let seen = watch.read();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].version, "3");
    }

    #[test]
    fn a_memory_that_cant_be_saved_leaves_nothing_behind() {
        let dir = scratch();
        let wait = ReleaseWait {
            enabled: true,
            hours: 24,
            skipped: vec![],
        };
        // Its folder is a file.
        let file = dir.path().join("file");
        fs::write(&file, "").unwrap();
        ReleaseWatch::new(wait.clone(), Some(file.join("seen.json"))).mark(
            "npm",
            &mut [update("tool", "2")],
            true,
            NOW,
        );
        assert_eq!(fs::read(&file).unwrap(), b"");
        // Its name is taken by a folder.
        let taken = dir.path().join("seen.json");
        fs::create_dir(&taken).unwrap();
        ReleaseWatch::new(wait, Some(taken.clone())).mark(
            "npm",
            &mut [update("tool", "2")],
            true,
            NOW,
        );
        assert!(taken.is_dir());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn a_clock_that_went_backwards_cant_stretch_the_wait() {
        let dir = scratch();
        let watch = watch(dir.path(), 1);
        watch.mark("npm", &mut [update("tool", "2")], true, NOW + 100 * DAY);
        let mut rows = [update("tool", "2")];
        watch.mark("npm", &mut rows, true, NOW);
        assert_eq!(rows[0].waiting_until, Some(NOW + DAY));
    }

    #[test]
    fn a_damaged_or_unsaveable_memory_holds_updates_back() {
        let dir = scratch();
        let watch = watch(dir.path(), 1);
        std::fs::write(dir.path().join("seen.json"), "not json").unwrap();
        let mut rows = [update("tool", "2")];
        watch.mark("npm", &mut rows, true, NOW);
        assert_eq!(rows[0].waiting_until, Some(NOW + DAY));

        let nowhere = ReleaseWatch::new(
            ReleaseWait {
                enabled: true,
                hours: 24,
                skipped: vec![],
            },
            None,
        );
        let mut rows = [update("tool", "2")];
        nowhere.mark("npm", &mut rows, true, NOW);
        assert_eq!(rows[0].waiting_until, Some(NOW + DAY));
        nowhere.mark("npm", &mut rows, true, NOW + 2 * DAY);
        assert_eq!(rows[0].waiting_until, Some(NOW + 3 * DAY));
    }

    #[test]
    fn sources_answering_together_keep_each_others_entries() {
        let dir = scratch();
        let watch = watch(dir.path(), 1);
        std::thread::scope(|scope| {
            for index in 0..16 {
                let watch = &watch;
                scope.spawn(move || {
                    let backend = format!("source-{index}");
                    let mut row = update("tool", "2");
                    row.id.backend = backend.clone();
                    watch.mark(&backend, &mut [row], true, NOW);
                });
            }
        });
        assert_eq!(watch.read().len(), 16);
    }

    #[test]
    fn settings_default_to_off_and_survive_a_round_trip() {
        let dir = scratch();
        let path = dir.path().join("deep/release-wait.json");
        assert_eq!(ReleaseWait::load(&path), ReleaseWait::default());
        assert!(!ReleaseWait::default().enabled);

        let wait = ReleaseWait {
            enabled: true,
            hours: 72,
            skipped: vec!["claude".into()],
        };
        wait.save(&path).unwrap();
        assert_eq!(ReleaseWait::load(&path), wait);
        let leftovers = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1, "no partial file stays behind");

        std::fs::write(&path, "{ broken").unwrap();
        assert_eq!(ReleaseWait::load(&path), ReleaseWait::default());
        std::fs::write(&path, r#"{"hours": 4000000000}"#).unwrap();
        assert_eq!(ReleaseWait::load(&path).hours, *HOUR_RANGE.end());
        std::fs::write(&path, r#"{"hours": 0}"#).unwrap();
        assert_eq!(ReleaseWait::load(&path).hours, *HOUR_RANGE.start());
    }

    #[test]
    fn system_sources_and_skipped_sources_never_wait() {
        let wait = ReleaseWait {
            enabled: true,
            hours: 24,
            skipped: vec!["claude".into()],
        };
        for backend in [
            "apt",
            "dnf",
            "pacman",
            "zypper",
            "apk",
            "xbps",
            "macports",
            "fwupd",
            "system-image",
            "macos-updates",
            "mas",
            "toolbox",
            "distrobox",
            "claude",
        ] {
            assert!(!wait.applies_to(backend), "{backend}");
        }
        for backend in ["npm", "homebrew", "homebrew-cask", "flatpak", "snap", "aur"] {
            assert!(wait.applies_to(backend), "{backend}");
        }
        assert!(!ReleaseWait::default().applies_to("npm"));
    }

    /// A source that may limit release age itself, and remembers its limit.
    struct Limiting {
        fake: crate::fake::Fake,
        limits: bool,
        hours: std::sync::Arc<std::sync::atomic::AtomicU32>,
    }
    impl crate::engine::Backend for Limiting {
        fn id(&self) -> &str {
            self.fake.id()
        }
        fn capabilities(&self) -> &[crate::package::Capability] {
            self.fake.capabilities()
        }
        fn detect(
            &mut self,
            cancel: &crate::process::Cancellation,
        ) -> Result<crate::package::Availability, crate::engine::EngineError> {
            self.fake.detect(cancel)
        }
        fn installed(
            &mut self,
            cancel: &crate::process::Cancellation,
        ) -> Result<Vec<Package>, crate::engine::EngineError> {
            self.fake.installed(cancel)
        }
        fn set_release_age(&mut self, hours: u32) {
            self.hours
                .store(hours, std::sync::atomic::Ordering::Relaxed);
        }
        fn limits_release_age(&mut self, _: &crate::process::Cancellation) -> bool {
            self.limits
        }
    }
    fn source(backend: &str, name: &str) -> crate::fake::Fake {
        use crate::package::Capability;
        let mut row = update(name, "2");
        row.id.backend = backend.into();
        crate::fake::Fake::new(
            backend,
            &[
                Capability::Installed,
                Capability::Search,
                Capability::Upgrade,
            ],
        )
        .listing(vec![row])
    }
    fn waiting_names(report: &crate::engine::PackageReport) -> Vec<&str> {
        report
            .packages
            .iter()
            .filter(|package| package.waiting())
            .map(|package| package.id.name.as_str())
            .collect()
    }

    #[test]
    fn an_engine_marks_installed_lists_and_not_names_someone_asked_for() {
        let dir = scratch();
        let cancel = crate::process::Cancellation::default();
        let mut engine = crate::engine::Engine::default();
        engine.register(source("npm", "tool")).unwrap();
        engine.register(source("apt", "curl")).unwrap();
        engine.register(source("cargo", "skipped")).unwrap();
        // Nothing waits before it's turned on.
        assert_eq!(waiting_names(&engine.installed(&cancel)), [] as [&str; 0]);

        engine.set_release_watch(ReleaseWatch::new(
            ReleaseWait {
                enabled: true,
                hours: 24,
                skipped: vec!["cargo".into()],
            },
            Some(dir.path().join("seen.json")),
        ));
        // Only npm: APT is a system source and Cargo was skipped.
        assert_eq!(waiting_names(&engine.installed(&cancel)), ["tool"]);
        assert_eq!(
            waiting_names(&engine.installed_for_mutation(&cancel)),
            ["tool"]
        );
        assert_eq!(
            waiting_names(&engine.installed_stream(&cancel, &mut |_| {})),
            ["tool"]
        );
        assert_eq!(
            waiting_names(&engine.lookup("tool", &cancel)),
            [] as [&str; 0]
        );
        assert_eq!(
            waiting_names(&engine.search("tool", &cancel)),
            [] as [&str; 0]
        );
    }

    #[test]
    fn a_manager_that_limits_release_age_itself_isnt_held_back_twice() {
        use std::sync::{atomic::AtomicU32, atomic::Ordering, Arc};
        let dir = scratch();
        let cancel = crate::process::Cancellation::default();
        let (npm, uv, skipped) = (
            Arc::new(AtomicU32::new(99)),
            Arc::new(AtomicU32::new(99)),
            Arc::new(AtomicU32::new(99)),
        );
        let mut engine = crate::engine::Engine::default();
        for (id, name, limits, hours) in [
            ("npm", "limited", true, &npm),
            ("uv", "older", false, &uv),
            ("pnpm", "skipped", false, &skipped),
        ] {
            engine
                .register(Limiting {
                    fake: source(id, name),
                    limits,
                    hours: hours.clone(),
                })
                .unwrap();
        }
        engine.set_release_watch(ReleaseWatch::new(
            ReleaseWait {
                enabled: true,
                hours: 72,
                skipped: vec!["pnpm".into()],
            },
            Some(dir.path().join("seen.json")),
        ));
        assert_eq!(npm.load(Ordering::Relaxed), 72);
        assert_eq!(uv.load(Ordering::Relaxed), 72);
        assert_eq!(skipped.load(Ordering::Relaxed), 0);
        // A manager too old to limit it falls back to PkgDeck's own wait.
        assert_eq!(waiting_names(&engine.installed(&cancel)), ["older"]);
    }

    #[test]
    fn a_listed_wait_ends_on_its_own_without_another_check() {
        let mut row = update("tool", "2");
        row.waiting_until = Some(u64::MAX);
        assert!(row.waiting());
        assert!(!row.update_ready());
        row.waiting_until = Some(1);
        assert!(!row.waiting());
        assert!(row.update_ready());
        row.waiting_until = None;
        assert!(row.update_ready());
        row.installed_version = None;
        assert!(!row.update_ready());
        row.installed_version = Some("1".into());
        row.update = UpdateAvailability::Current;
        assert!(!row.update_ready());
    }

    #[test]
    fn the_cutoff_is_the_format_managers_accept() {
        assert_eq!(cutoff(0, 0), "1970-01-01T00:00:00Z");
        assert_eq!(cutoff(24, 2 * DAY + 61), "1970-01-02T00:01:01Z");
        assert_eq!(cutoff(1, 2 * HOUR), "1970-01-01T01:00:00Z");
        // Never before 1970, however long the wait.
        assert_eq!(cutoff(*HOUR_RANGE.end(), 10), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn a_wait_reads_in_the_largest_whole_unit() {
        for (hours, text) in [
            (1, "1 hour"),
            (12, "12 hours"),
            (24, "1 day"),
            (36, "36 hours"),
            (72, "3 days"),
            (168, "1 week"),
            (336, "2 weeks"),
        ] {
            assert_eq!(hours_text(hours), text);
        }
    }
}
