//! Homebrew formulae and casks.
use super::*;

pub struct Homebrew<T = NativeTransport> {
    pub transport: T,
    prefix: Option<PathBuf>,
    /// Set for an update check so formulae and casks share one `brew update`.
    update_check: Option<u64>,
    /// Search details already read, while this engine is reused.
    found: BrewFound,
}
pub struct HomebrewCask<T = NativeTransport> {
    pub transport: T,
    pub(super) prefix: Option<PathBuf>,
    /// Checks and backs up an app already in the cask's place before
    /// Homebrew adopts it; `None` never adopts (fixture transports).
    pub(super) adoption: Option<Box<dyn adopt::AdoptIo>>,
    /// Set for an update check so formulae and casks share one `brew update`.
    update_check: Option<u64>,
    /// Search details already read, while this engine is reused.
    found: BrewFound,
}

/// One `brew update` at a time across checks. Callers that share a token
/// already share one fetch; this only keeps a second check, or an explicit
/// refresh, from taking Homebrew's git lock at the same moment. The next
/// check still fetches. It does not reuse a finished result.
fn brew_update_turn() -> std::sync::MutexGuard<'static, ()> {
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
/// How long `brew update` waits for another process's update to finish, and
/// how often it looks again.
pub(super) const BREW_UPDATE_PATIENCE: Duration =
    Duration::from_secs(if cfg!(test) { 1 } else { 180 });
const BREW_UPDATE_POLL: Duration = Duration::from_millis(if cfg!(test) { 10 } else { 2000 });
/// Runs `brew update` in its turn. When another process, such as a terminal
/// or a second PkgDeck, is already updating Homebrew, brew refuses at once;
/// this waits for that update to finish and runs again, instead of reporting
/// the source as broken.
pub(super) fn brew_update(
    cancel: &Cancellation,
    mut run: impl FnMut() -> Result<Completion, EngineError>,
) -> Result<Completion, EngineError> {
    let _turn = brew_update_turn();
    let deadline = std::time::Instant::now() + BREW_UPDATE_PATIENCE;
    loop {
        match run() {
            Err(EngineError::Execution(ExecutionError::Failed(result)))
                if brew_update_locked(&result) && std::time::Instant::now() < deadline =>
            {
                let wake = std::time::Instant::now() + BREW_UPDATE_POLL;
                while std::time::Instant::now() < wake {
                    if cancel.requested() {
                        return Err(EngineError::Cancelled);
                    }
                    std::thread::sleep(Duration::from_millis(10).min(BREW_UPDATE_POLL));
                }
            }
            other => return other,
        }
    }
}
pub(super) fn brew_update_locked(result: &Completion) -> bool {
    String::from_utf8_lossy(&result.stderr)
        .contains("Another `brew update` process is already running")
}
impl<T: Transport> Homebrew<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            prefix: None,
            update_check: None,
            found: BrewFound::default(),
        }
    }
}
impl<T: Transport> HomebrewCask<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            prefix: None,
            adoption: None,
            update_check: None,
            found: BrewFound::default(),
        }
    }
    /// Casks can take over apps someone installed themselves (macOS only).
    pub fn with_adoption(self) -> Self {
        #[cfg(target_os = "macos")]
        return Self {
            adoption: Some(Box::new(adopt::NativeAdopt(Host::current()))),
            ..self
        };
        #[cfg(not(target_os = "macos"))]
        self
    }
}

#[derive(Deserialize)]
struct FormulaReport {
    formulae: Vec<Formula>,
}
#[derive(Deserialize)]
struct Formula {
    full_name: String,
    desc: Option<String>,
    homepage: String,
    versions: Versions,
    revision: u64,
    installed: Vec<Installed>,
    linked_keg: Option<String>,
    outdated: bool,
    dependencies: Vec<String>,
}
#[derive(Deserialize)]
struct Versions {
    stable: Option<String>,
}
#[derive(Deserialize)]
struct Installed {
    version: String,
}
fn formula_name(name: &str) -> bool {
    let segments: Vec<_> = name.split('/').collect();
    (segments.len() == 1 || segments.len() == 3)
        && segments.iter().all(|s| {
            !s.is_empty()
                && s.starts_with(|c: char| c.is_ascii_alphanumeric())
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"@+_.-".contains(&b))
                && *s != "."
                && *s != ".."
        })
}
/// Cask tokens share the formula policy: bare tokens and tap-qualified full
/// tokens, including versioned `@` tokens (for example `firefox@esr`).
pub(super) fn cask_token(name: &str) -> bool {
    formula_name(name)
}
/// Names per `brew info` call. Each call starts Homebrew again, which is
/// most of its time, so a few big calls beat many small ones. Output stays
/// well under the read limit (about 3 MB per 1,000 casks).
const BREW_INFO_BATCH: usize = 1000;
/// Packages a search already read from `brew info`, by name. `None` marks a
/// name with no row here, such as a cask this system can't install. Typing
/// one more letter only narrows a search, so most of it is answered from
/// here. Changes and update checks clear it. With a [`BrewDisk`], they are
/// also kept between runs, so the first search after a start is fast too.
#[derive(Default)]
struct BrewFound {
    packages: std::collections::HashMap<String, Option<Package>>,
    /// The fingerprint `packages` were read under, when kept on disk.
    key: Option<String>,
    /// Looked up on the first search.
    disk: Option<Option<BrewDisk>>,
}
impl BrewFound {
    /// `names` in order, reading the ones not seen yet with `read`.
    fn search(
        &mut self,
        names: &[&str],
        source: &str,
        find: impl FnOnce() -> Option<BrewDisk>,
        mut read: impl FnMut(&[&str]) -> Result<Vec<Package>, EngineError>,
    ) -> Result<Vec<Package>, EngineError> {
        let disk = self.disk.get_or_insert_with(find).as_ref();
        let key = disk.and_then(BrewDisk::fingerprint);
        if key != self.key {
            // The first search of this run, or Homebrew changed since.
            self.packages = disk
                .zip(key.as_deref())
                .and_then(|(disk, key)| disk.load(source, key))
                .unwrap_or_default();
            self.key = key;
        }
        let missing: Vec<_> = names
            .iter()
            .copied()
            .filter(|name| !self.packages.contains_key(*name))
            .collect();
        for chunk in missing.chunks(BREW_INFO_BATCH) {
            let mut read = read_around_broken(chunk, &mut read)?
                .into_iter()
                .map(|package| (package.id.name.clone(), package))
                .collect::<std::collections::HashMap<_, _>>();
            for name in chunk {
                self.packages.insert((*name).to_owned(), read.remove(*name));
            }
        }
        let found = names
            .iter()
            .filter_map(|name| self.packages.get(*name).cloned().flatten())
            .collect();
        if let (Some(disk), Some(key), false) = (disk, &self.key, missing.is_empty()) {
            // Homebrew changed while brew read: keep none of it.
            if disk.fingerprint().as_ref() == Some(key) {
                disk.save(source, key, &self.packages);
            } else {
                self.packages.clear();
                self.key = None;
            }
        }
        Ok(found)
    }
    fn clear(&mut self) {
        self.packages.clear();
        self.key = None;
    }
}
/// Extra `brew info` calls one batch may make to find the names it fails on.
pub(super) const BREW_INFO_RETRIES: usize = 64;
/// `read` all of `names`. `brew info` stops at the first name it can't load,
/// such as a macOS-only cask on Linux, and returns nothing. Then each half
/// is read again, down to the names that fail alone, which are left out.
/// When no read works at all, or the retries run out, the first failure
/// stands: Homebrew itself is broken, not one of its packages.
pub(super) fn read_around_broken(
    names: &[&str],
    read: &mut impl FnMut(&[&str]) -> Result<Vec<Package>, EngineError>,
) -> Result<Vec<Package>, EngineError> {
    fn halves(
        names: &[&str],
        read: &mut impl FnMut(&[&str]) -> Result<Vec<Package>, EngineError>,
        budget: &mut usize,
        worked: &mut bool,
    ) -> Result<Vec<Package>, Option<EngineError>> {
        match read(names) {
            Ok(packages) => {
                *worked = true;
                Ok(packages)
            }
            Err(EngineError::Execution(ExecutionError::Failed(_))) if names.len() == 1 => {
                Ok(vec![])
            }
            Err(EngineError::Execution(ExecutionError::Failed(_))) if *budget >= 2 => {
                *budget -= 2;
                let (first, second) = names.split_at(names.len() / 2);
                let mut packages = halves(first, read, budget, worked)?;
                packages.extend(halves(second, read, budget, worked)?);
                Ok(packages)
            }
            // Out of retries: report the batch's own failure.
            Err(EngineError::Execution(ExecutionError::Failed(_))) => Err(None),
            Err(error) => Err(Some(error)),
        }
    }
    let first = match read(names) {
        Err(error @ EngineError::Execution(ExecutionError::Failed(_))) if names.len() > 1 => error,
        result => return result,
    };
    let (mut budget, mut worked) = (BREW_INFO_RETRIES, false);
    let (first_half, second_half) = names.split_at(names.len() / 2);
    let packages = halves(first_half, read, &mut budget, &mut worked).and_then(|mut packages| {
        packages.extend(halves(second_half, read, &mut budget, &mut worked)?);
        Ok(packages)
    });
    match packages {
        Ok(packages) if worked => Ok(packages),
        Ok(_) | Err(None) => Err(first),
        Err(Some(error)) => Err(error),
    }
}
/// Where Homebrew search details are kept between runs, and everything
/// they depend on (see [`Transport::brew_cache`]).
pub(super) struct BrewDisk {
    pub(super) store: crate::cache::Store,
    pub(super) watches: Vec<crate::cache::Watch>,
    /// What else the details depend on: the architecture this program reads
    /// them for, and the OS release brew picks variations for.
    pub(super) system: [String; 2],
}
impl BrewDisk {
    fn find(transport: &impl Transport) -> Option<Self> {
        let (store, watches) = transport.brew_cache()?;
        let release = rustix::system::uname()
            .release()
            .to_string_lossy()
            .into_owned();
        Some(Self {
            store,
            watches,
            system: [std::env::consts::ARCH.into(), release],
        })
    }
    pub(super) fn fingerprint(&self) -> Option<String> {
        let [arch, release] = &self.system;
        crate::cache::fingerprint(&self.watches, &[arch, release])
    }
    pub(super) fn load(
        &self,
        source: &str,
        key: &str,
    ) -> Option<std::collections::HashMap<String, Option<Package>>> {
        serde_json::from_slice(&self.store.get(source, "found", &[], key)?).ok()
    }
    pub(super) fn save(
        &self,
        source: &str,
        key: &str,
        packages: &std::collections::HashMap<String, Option<Package>>,
    ) {
        let value = serde_json::to_vec(packages).unwrap_or_default();
        self.store.put(source, "found", &[], key, &value);
    }
}
#[derive(Deserialize)]
struct CaskReport {
    casks: Vec<Cask>,
}
#[derive(Deserialize)]
pub(super) struct Cask {
    full_token: String,
    name: Vec<String>,
    desc: Option<String>,
    homepage: String,
    /// `null` when the cask has no version for this system, such as
    /// `discord` on Linux, which only has a macOS download.
    version: Option<String>,
    installed: Option<String>,
    outdated: bool,
    #[serde(default)]
    depends_on: serde_json::Value,
    #[serde(default)]
    artifacts: Vec<serde_json::Value>,
}
/// Artifacts only macOS can install. On Linux, `brew info` describes each cask
/// for the running system: a Mac-only cask declares `depends_on: macos` or keeps
/// one of these artifacts, while a Linux cask ships AppImages, binaries or fonts.
const MACOS_ONLY_ARTIFACTS: &[&str] = &[
    "app",
    "pkg",
    "suite",
    "installer",
    "prefpane",
    "qlplugin",
    "mdimporter",
    "dictionary",
    "colorpicker",
    "input_method",
    "internet_plugin",
    "keyboard_layout",
    "screen_saver",
    "service",
    "audio_unit_plugin",
    "vst_plugin",
    "vst3_plugin",
];
pub(super) fn cask_installs_here(cask: &Cask) -> bool {
    cfg!(target_os = "macos")
        || (cask.depends_on.get("macos").is_none()
            && !cask.artifacts.iter().any(|artifact| {
                artifact.as_object().is_some_and(|kinds| {
                    kinds
                        .keys()
                        .any(|kind| MACOS_ONLY_ARTIFACTS.contains(&kind.as_str()))
                })
            }))
}
/// Homebrew 6.0 added AppImage artifacts and Linux variations to casks; earlier
/// releases only had preliminary Linux cask support.
pub(super) fn linux_casks_supported(version: &str) -> bool {
    version
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("Homebrew "))
        .and_then(|version| version.split(['.', '-']).next())
        .and_then(|major| major.parse::<u32>().ok())
        .is_some_and(|major| major >= 6)
}
impl<T: Transport> Homebrew<T> {
    /// What the formulae `brew autoremove --dry-run` lists take in the Cellar.
    fn cellar_usage(&self, preview: &str, cancel: &Cancellation) -> Option<u64> {
        let cellar = self.prefix.as_ref()?.join("Cellar");
        Some(
            preview
                .lines()
                .map(str::trim)
                .filter(|name| formula_name(name) && !name.contains('/'))
                .map(|name| crate::disk::usage(&cellar.join(name), cancel))
                .sum(),
        )
    }
    fn call(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, EngineError> {
        Ok(self.transport.brew(
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            cancel,
            write,
        )?)
    }
    fn parse(&self, result: Completion) -> Result<Vec<PackageDetails>, EngineError> {
        let report: FormulaReport = serde_json::from_slice(&bytes("homebrew", result)?)
            .map_err(|e| invalid("homebrew", e))?;
        let prefix = self
            .prefix
            .as_ref()
            .ok_or_else(|| invalid("homebrew", "prefix not detected"))?;
        report
            .formulae
            .into_iter()
            .map(|f| {
                if !formula_name(&f.full_name) {
                    return Err(invalid("homebrew", "invalid formula name"));
                }
                let candidate = f.versions.stable.map(|v| {
                    if f.revision == 0 {
                        v
                    } else {
                        format!("{v}_{}", f.revision)
                    }
                });
                let installed = f.linked_keg.or_else(|| {
                    f.installed
                        .iter()
                        .find(|i| Some(&i.version) == candidate.as_ref())
                        .or_else(|| f.installed.last())
                        .map(|i| i.version.clone())
                });
                Ok(PackageDetails {
                    package: Package {
                        id: PackageId {
                            backend: "homebrew".into(),
                            name: f.full_name.clone(),
                            architecture: std::env::consts::ARCH.into(),
                            scope: Scope::Environment {
                                path: prefix.clone(),
                            },
                            remote: None,
                            reference: None,
                        },
                        display_name: f.full_name,
                        summary: f.desc.clone().unwrap_or_default(),
                        update: if f.outdated {
                            UpdateAvailability::Available
                        } else if installed.is_some() {
                            UpdateAvailability::Current
                        } else {
                            UpdateAvailability::Unknown
                        },
                        installed_version: installed,
                        candidate_version: candidate,
                        icon: None,
                        component_ids: vec![],
                        homepages: if f.homepage.is_empty() {
                            vec![]
                        } else {
                            vec![f.homepage.clone()]
                        },
                        adopt_with: None,
                    },
                    description: f.desc.unwrap_or_default(),
                    homepage: (!f.homepage.is_empty()).then_some(f.homepage),
                    dependencies: f.dependencies,
                })
            })
            .collect()
    }
    fn target<'a>(&self, id: &'a PackageId) -> Result<&'a str, EngineError> {
        if id.backend != "homebrew"
            || id.architecture != std::env::consts::ARCH
            || !formula_name(&id.name)
            || self
                .prefix
                .as_ref()
                .is_none_or(|p| id.scope != (Scope::Environment { path: p.clone() }))
        {
            return Err(invalid(
                "homebrew",
                "foreign identity or invalid formula name",
            ));
        }
        Ok(&id.name)
    }
}
impl<T: Transport> Backend for Homebrew<T> {
    fn id(&self) -> &str {
        "homebrew"
    }
    /// brew cleanup lists each file with its size, which grows with a
    /// download, and the total it frees. Neither changes what it removes,
    /// and a list that only got shorter removes less.
    fn cleanup_still_reviewed(&self, reviewed: &CleanupItem, current: &CleanupItem) -> bool {
        fn removes(preview: &str) -> Vec<&str> {
            preview
                .lines()
                .filter(|line| !line.starts_with("==> This operation would free approximately"))
                .map(|line| match line.strip_prefix("Would remove: ") {
                    Some(rest) => rest.rsplit_once(" (").map_or(rest, |(path, _)| path),
                    None => line,
                })
                .collect()
        }
        if reviewed == current {
            return true;
        }
        current.id.key == "cleanup"
            && reviewed.summary == current.summary
            && only_fewer(reviewed, current, removes)
    }
    fn has_update_index(&self) -> bool {
        true
    }
    fn arm_update_check(&mut self, token: Option<u64>) {
        self.update_check = token;
    }
    fn refresh_update_index(&mut self, cancel: &Cancellation) -> Result<(), EngineError> {
        let Some(token) = self.update_check else {
            return Ok(());
        };
        self.found.clear();
        // Formulae and casks both enter here. The first `brew update` wins;
        // the other waits and reuses its result. A different check waits its
        // turn, then fetches again.
        // It runs as a read: only fetched data changes, so a cancel or a quit
        // stops it at once and a stalled fetch times out. As a write, quitting
        // waited for it to end.
        crate::engine::once_per_check(token, || {
            brew_update(cancel, || self.call(&["update"], cancel, false))?;
            Ok(())
        })
    }
    fn capabilities(&self) -> &[Capability] {
        CLEAN_CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        let result = self.transport.brew(&["--prefix".into()], cancel, false);
        if let Ok(result) = &result {
            let value = String::from_utf8(bytes("homebrew", result.clone())?)
                .map_err(|e| invalid("homebrew", e))?;
            let path = PathBuf::from(value.trim());
            if !path.is_absolute() {
                return Err(invalid("homebrew", "prefix must be absolute"));
            }
            self.prefix = Some(path);
        }
        availability(result)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let data = bytes("homebrew", self.call(&["formulae"], cancel, false)?)?;
        let names = String::from_utf8(data).map_err(|e| invalid("homebrew", e))?;
        let matches: Vec<_> = names.lines().filter(|n| search_matches(n, query)).collect();
        if matches.iter().any(|n| !formula_name(n)) {
            return Err(invalid("homebrew", "invalid formula name"));
        }
        let mut found = std::mem::take(&mut self.found);
        let packages = found.search(
            &matches,
            "homebrew",
            || BrewDisk::find(&self.transport),
            |chunk| {
                let mut args = vec!["info", "--json=v2", "--formula", "--"];
                args.extend(chunk);
                Ok(self
                    .parse(self.call(&args, cancel, false)?)?
                    .into_iter()
                    .map(|d| d.package)
                    .collect())
            },
        );
        self.found = found;
        packages
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .parse(self.call(
                &["info", "--json=v2", "--formula", "--installed"],
                cancel,
                false,
            )?)?
            .into_iter()
            .map(|d| d.package)
            .collect())
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let name = self.target(id)?;
        self.parse(self.call(
            &["info", "--json=v2", "--formula", "--", name],
            cancel,
            false,
        )?)?
        .into_iter()
        .find(|d| d.package.id == *id)
        .ok_or(EngineError::NotFound)
    }
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        /// "==> This operation would free approximately 18.2GB of disk space."
        fn brew_frees(preview: &str) -> Option<u64> {
            preview.lines().find_map(|line| {
                let size = line.split("would free approximately ").nth(1)?;
                crate::disk::parse_size(size.split_whitespace().next()?)
            })
        }
        fn plan(result: Completion) -> Result<Option<String>, EngineError> {
            let output = String::from_utf8(bytes("homebrew", result)?)
                .map_err(|error| invalid("homebrew", error))?;
            let output = output.trim();
            Ok((!output.is_empty()).then(|| output.to_owned()))
        }
        let mut items = Vec::new();
        if let Some(preview) = plan(self.call(&["autoremove", "--dry-run"], cancel, false)?)? {
            items.push(CleanupItem {
                id: CleanupId {
                    backend: "homebrew".into(),
                    key: "autoremove".into(),
                },
                kind: CleanupKind::OrphanDependencies,
                title: "Unused Homebrew dependencies".into(),
                summary: "Formulae that no installed package needs anymore".into(),
                bytes: self.cellar_usage(&preview, cancel),
                preview,
            });
        }
        if let Some(preview) = plan(self.call(&["cleanup", "--dry-run"], cancel, false)?)? {
            items.push(CleanupItem {
                id: CleanupId {
                    backend: "homebrew".into(),
                    key: "cleanup".into(),
                },
                kind: CleanupKind::PackageCache,
                title: "Old Homebrew downloads and versions".into(),
                summary: "Old files that brew cleanup would remove".into(),
                bytes: brew_frees(&preview),
                preview,
            });
        }
        Ok(items)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        // A change can make what searches read out of date.
        self.found.clear();
        let args = match operation {
            Operation::Refresh { backend } if backend == "homebrew" => vec!["update"],
            Operation::Install(id) => vec!["install", "--formula", "--", self.target(id)?],
            Operation::Remove(id) => {
                vec!["uninstall", "--formula", "--force", "--", self.target(id)?]
            }
            Operation::Upgrade(id) => vec!["upgrade", "--formula", "--", self.target(id)?],
            Operation::UpgradeAll { backend } if backend == "homebrew" => {
                vec!["upgrade", "--formula"]
            }
            Operation::Clean(id) if id.backend == "homebrew" && id.key == "autoremove" => {
                vec!["autoremove"]
            }
            Operation::Clean(id) if id.backend == "homebrew" && id.key == "cleanup" => {
                vec!["cleanup"]
            }
            _ => return Err(invalid("homebrew", "foreign operation")),
        };
        progress(Progress::Message(
            "Running brew. If you cancel, PkgDeck waits for it to finish.".into(),
        ));
        let result = if args == ["update"] {
            brew_update(cancel, || self.call(&args, cancel, true))?
        } else {
            self.call(&args, cancel, true)?
        };
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}

impl<T: Transport> HomebrewCask<T> {
    fn call(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, EngineError> {
        Ok(self.transport.brew(
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            cancel,
            write,
        )?)
    }
    /// `installable_only` drops casks this system cannot install; searches use
    /// it on Linux, while installed casks always stay listed.
    fn parse(
        &self,
        result: Completion,
        installable_only: bool,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let report: CaskReport = serde_json::from_slice(&bytes("homebrew-cask", result)?)
            .map_err(|e| invalid("homebrew-cask", e))?;
        let prefix = self
            .prefix
            .as_ref()
            .ok_or_else(|| invalid("homebrew-cask", "prefix not detected"))?;
        report
            .casks
            .into_iter()
            .filter(|c| !installable_only || cask_installs_here(c))
            .map(|c| {
                if !cask_token(&c.full_token) {
                    return Err(invalid("homebrew-cask", "invalid cask token"));
                }
                let installed = c.installed.filter(|v| !v.is_empty());
                let display = c
                    .name
                    .iter()
                    .find(|n| !n.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| c.full_token.clone());
                Ok(PackageDetails {
                    package: Package {
                        id: PackageId {
                            backend: "homebrew-cask".into(),
                            name: c.full_token.clone(),
                            architecture: std::env::consts::ARCH.into(),
                            scope: Scope::Environment {
                                path: prefix.clone(),
                            },
                            remote: None,
                            reference: None,
                        },
                        display_name: display,
                        summary: c.desc.clone().unwrap_or_default(),
                        update: if c.outdated {
                            UpdateAvailability::Available
                        } else if installed.is_some() {
                            UpdateAvailability::Current
                        } else {
                            UpdateAvailability::Unknown
                        },
                        installed_version: installed,
                        candidate_version: c.version,
                        icon: None,
                        component_ids: vec![],
                        homepages: if c.homepage.is_empty() {
                            vec![]
                        } else {
                            vec![c.homepage.clone()]
                        },
                        adopt_with: None,
                    },
                    description: c.desc.unwrap_or_default(),
                    homepage: (!c.homepage.is_empty()).then_some(c.homepage),
                    dependencies: vec![],
                })
            })
            .collect()
    }
    fn target<'a>(&self, id: &'a PackageId) -> Result<&'a str, EngineError> {
        if id.backend != "homebrew-cask"
            || id.architecture != std::env::consts::ARCH
            || !cask_token(&id.name)
            || self
                .prefix
                .as_ref()
                .is_none_or(|p| id.scope != (Scope::Environment { path: p.clone() }))
        {
            return Err(invalid(
                "homebrew-cask",
                "foreign identity or invalid cask token",
            ));
        }
        Ok(&id.name)
    }
}
impl<T: Transport> HomebrewCask<T> {
    /// A checked adoption when an app is already in the cask's place.
    fn adoption_plan(
        &self,
        token: &str,
        cancel: &Cancellation,
    ) -> Result<Option<adopt::Plan>, EngineError> {
        let Some(io) = &self.adoption else {
            return Ok(None);
        };
        let info: serde_json::Value = serde_json::from_slice(&bytes(
            "homebrew-cask",
            self.call(&["info", "--json=v2", "--cask", "--", token], cancel, false)?,
        )?)
        .map_err(|e| invalid("homebrew-cask", e))?;
        adopt::plan(io.as_ref(), &info["casks"][0], cancel)
    }
}
impl<T: Transport> Backend for HomebrewCask<T> {
    /// Installing over an app already in place adopts it; the preview says
    /// so, and names the version and publisher that were checked.
    fn operation_plan(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
    ) -> Result<Option<TransactionPlan>, EngineError> {
        let Operation::Install(id) = operation else {
            return Ok(None);
        };
        let token = self.target(id)?;
        Ok(self
            .adoption_plan(token, cancel)?
            .map(|plan| TransactionPlan {
                operation: operation.clone(),
                native_preview: plan.preview(),
                changes: vec![PlannedChange {
                    action: PlannedAction::Install,
                    name: token.into(),
                    installed_version: Some(plan.version.clone()),
                    candidate_version: Some(plan.version.clone()),
                }],
                download_bytes: None,
                disk_bytes: None,
                restart_required: None,
                adopts: Some(plan.app.clone()),
            }))
    }
    fn id(&self) -> &str {
        "homebrew-cask"
    }
    fn has_update_index(&self) -> bool {
        true
    }
    fn arm_update_check(&mut self, token: Option<u64>) {
        self.update_check = token;
    }
    fn refresh_update_index(&mut self, cancel: &Cancellation) -> Result<(), EngineError> {
        let Some(token) = self.update_check else {
            return Ok(());
        };
        self.found.clear();
        crate::engine::once_per_check(token, || {
            brew_update(cancel, || self.call(&["update"], cancel, false))?;
            Ok(())
        })
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        let result = self.transport.brew(&["--prefix".into()], cancel, false);
        if let Ok(result) = &result {
            let value = String::from_utf8(bytes("homebrew-cask", result.clone())?)
                .map_err(|e| invalid("homebrew-cask", e))?;
            let path = PathBuf::from(value.trim());
            if !path.is_absolute() {
                return Err(invalid("homebrew-cask", "prefix must be absolute"));
            }
            self.prefix = Some(path);
            if !cfg!(target_os = "macos") {
                let version = bytes("homebrew-cask", self.call(&["--version"], cancel, false)?)?;
                if !linux_casks_supported(&String::from_utf8_lossy(&version)) {
                    return Ok(Availability::Unavailable(
                        "Homebrew Casks on Linux need Homebrew 6.0 or later".into(),
                    ));
                }
            }
        }
        availability(result)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let data = bytes("homebrew-cask", self.call(&["casks"], cancel, false)?)?;
        let names = String::from_utf8(data).map_err(|e| invalid("homebrew-cask", e))?;
        let matches: Vec<_> = names
            .lines()
            .filter(|n| !n.trim().is_empty() && search_matches(n, query))
            .collect();
        if matches.iter().any(|n| !cask_token(n)) {
            return Err(invalid("homebrew-cask", "invalid cask token"));
        }
        let mut found = std::mem::take(&mut self.found);
        let packages = found.search(
            &matches,
            "homebrew-cask",
            || BrewDisk::find(&self.transport),
            |chunk| {
                let mut args = vec!["info", "--json=v2", "--cask", "--"];
                args.extend(chunk);
                Ok(self
                    .parse(self.call(&args, cancel, false)?, true)?
                    .into_iter()
                    .map(|d| d.package)
                    .collect())
            },
        );
        self.found = found;
        packages
    }
    /// An exact name is looked up directly, which also finds casks in a tap
    /// that `brew casks` hasn't listed yet, and skips reading every cask.
    fn lookup(&mut self, name: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        if !cask_token(name) {
            return Ok(vec![]);
        }
        match self.call(&["info", "--json=v2", "--cask", "--", name], cancel, false) {
            Ok(result) => Ok(self
                .parse(result, true)?
                .into_iter()
                .map(|details| details.package)
                .collect()),
            // brew exits 1 for a name that is no cask.
            Err(EngineError::Execution(ExecutionError::Failed(result)))
                if result.code == Some(1) =>
            {
                Ok(vec![])
            }
            Err(error) => Err(error),
        }
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .parse(
                self.call(
                    &["info", "--json=v2", "--cask", "--installed"],
                    cancel,
                    false,
                )?,
                false,
            )?
            .into_iter()
            .map(|d| d.package)
            .collect())
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let name = self.target(id)?;
        self.parse(
            self.call(&["info", "--json=v2", "--cask", "--", name], cancel, false)?,
            false,
        )?
        .into_iter()
        .find(|d| d.package.id == *id)
        .ok_or(EngineError::NotFound)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        // A change can make what searches read out of date.
        self.found.clear();
        let args = match operation {
            // brew update refreshes formulae and casks together; both Homebrew
            // backends run it so each Refresh honestly refreshes its metadata.
            Operation::Refresh { backend } if backend == "homebrew-cask" => vec!["update"],
            Operation::Install(id) => {
                let token = self.target(id)?;
                if let (Some(io), Some(plan)) = (&self.adoption, self.adoption_plan(token, cancel)?)
                {
                    return adopt::run(
                        io.as_ref(),
                        &plan,
                        cancel,
                        progress,
                        &mut || {
                            self.call(&["install", "--cask", "--adopt", "--", token], cancel, true)
                        },
                        // brew list rejects a tap cask's full name; brew info
                        // takes both and reports the installed version.
                        &mut || {
                            self.call(
                                &["info", "--json=v2", "--cask", "--", token],
                                &Cancellation::default(),
                                false,
                            )
                            .ok()
                            .and_then(|done| {
                                serde_json::from_slice::<serde_json::Value>(&done.stdout).ok()
                            })
                            .is_some_and(|info| {
                                info["casks"][0]["installed"]
                                    .as_str()
                                    .is_some_and(|version| !version.is_empty())
                            })
                        },
                    );
                }
                vec!["install", "--cask", "--", token]
            }
            Operation::Remove(id) => {
                vec!["uninstall", "--cask", "--force", "--", self.target(id)?]
            }
            Operation::Upgrade(id) => vec!["upgrade", "--cask", "--", self.target(id)?],
            Operation::UpgradeAll { backend } if backend == "homebrew-cask" => {
                vec!["upgrade", "--cask"]
            }
            _ => return Err(invalid("homebrew-cask", "foreign operation")),
        };
        progress(Progress::Message(
            "Running brew. If you cancel, PkgDeck waits for it to finish.".into(),
        ));
        let result = if args == ["update"] {
            brew_update(cancel, || self.call(&args, cancel, true))?
        } else {
            self.call(&args, cancel, true)?
        };
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}
