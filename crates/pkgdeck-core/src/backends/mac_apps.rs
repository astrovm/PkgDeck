//! macOS bundle inventory. It lists apps and removes them (to the Trash, or
//! through Homebrew for casks); it never installs or updates. A cask
//! suggestion never authorizes a write.
use super::*;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path},
};

const ID: &str = "macos-apps";
const MAX_ENTRIES: usize = 4096;

/// A folder's entries, each read as the listing goes.
type Entries = Box<dyn Iterator<Item = std::io::Result<PathBuf>>>;

trait AppIo: Send {
    fn read_dir(&self, path: &Path) -> std::io::Result<Entries> {
        Ok(Box::new(
            fs::read_dir(path)?.map(|entry| entry.map(|entry| entry.path())),
        ))
    }
    fn symlink_metadata(&self, path: &Path) -> std::io::Result<fs::Metadata> {
        fs::symlink_metadata(path)
    }
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        fs::canonicalize(path)
    }
    fn plist(&self, path: &Path, cancel: &Cancellation) -> Result<Value, EngineError>;
    fn ownership(
        &self,
        cancel: &Cancellation,
    ) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError>;
}

/// plutil, pkgutil and Homebrew's Caskroom.
#[cfg(target_os = "macos")]
struct NativeApps<R = Host>(R);

/// The commands [`NativeApps`] runs; tests answer them instead.
#[cfg(target_os = "macos")]
trait Commands: Send {
    fn read(
        &self,
        executable: &Path,
        args: &[OsString],
        limits: Limits,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError>;
    fn brew(&self, args: &[OsString], cancel: &Cancellation) -> Result<Completion, ExecutionError>;
}

#[cfg(target_os = "macos")]
impl Commands for Host {
    fn read(
        &self,
        executable: &Path,
        args: &[OsString],
        limits: Limits,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        Host::read(self, executable, args, limits, cancel)
    }
    fn brew(&self, args: &[OsString], cancel: &Cancellation) -> Result<Completion, ExecutionError> {
        Host::brew(self, args, cancel, false)
    }
}

/// Elsewhere there are no app bundles to read and no casks that own them.
#[cfg(not(target_os = "macos"))]
struct NativeApps;

#[cfg(not(target_os = "macos"))]
impl AppIo for NativeApps {
    fn plist(&self, _: &Path, _: &Cancellation) -> Result<Value, EngineError> {
        Err(macos_only())
    }
    fn ownership(&self, _: &Cancellation) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError> {
        Err(macos_only())
    }
}

#[cfg(not(target_os = "macos"))]
fn macos_only() -> EngineError {
    EngineError::Unavailable {
        backend: ID.into(),
        reason: "Application bundle discovery requires macOS".into(),
    }
}

#[cfg(target_os = "macos")]
impl<R: Commands> AppIo for NativeApps<R> {
    fn plist(&self, path: &Path, cancel: &Cancellation) -> Result<Value, EngineError> {
        let metadata = fs::metadata(path).map_err(ExecutionError::from)?;
        if !metadata.is_file() || metadata.len() > 1024 * 1024 {
            return Err(invalid(ID, "app metadata is not a bounded regular file"));
        }
        // plutil handles both binary and XML plists; it never launches the app.
        let result = self.0.read(
            Path::new("/usr/bin/plutil"),
            &[
                "-convert".into(),
                "json".into(),
                "-o".into(),
                "-".into(),
                "--".into(),
                path.into(),
            ],
            Limits {
                timeout: Duration::from_secs(5),
                output_bytes: 1024 * 1024,
            },
            cancel,
        )?;
        serde_json::from_slice(&bytes(ID, result)?).map_err(|e| invalid(ID, e))
    }

    fn ownership(
        &self,
        cancel: &Cancellation,
    ) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError> {
        let root = bytes(ID, self.0.brew(&["--caskroom".into()], cancel)?)?;
        let root = PathBuf::from(
            std::str::from_utf8(&root)
                .map_err(|e| invalid(ID, e))?
                .trim(),
        );
        if !root.is_absolute() {
            return Err(invalid(ID, "Homebrew returned a relative Caskroom"));
        }
        let installed = bytes(
            ID,
            self.0.brew(
                &[
                    "info".into(),
                    "--json=v2".into(),
                    "--cask".into(),
                    "--installed".into(),
                ],
                cancel,
            )?,
        )?;
        let mut owners = cask_owners(&root, &installed)?;
        for (cask, patterns) in receipt_patterns(&installed)? {
            for id in patterns
                .iter()
                .flat_map(|pattern| self.receipts(pattern, cancel))
                .collect::<BTreeSet<_>>()
            {
                for bundle in self.receipt_bundles(&id, cancel)? {
                    if let Ok(target) = fs::canonicalize(&bundle) {
                        owners.entry(target).or_default().push(cask.clone());
                    }
                }
            }
        }
        for names in owners.values_mut() {
            names.sort();
            names.dedup();
        }
        Ok(owners)
    }
}

#[cfg(target_os = "macos")]
impl<R: Commands> NativeApps<R> {
    fn pkgutil(&self, args: &[&str], cancel: &Cancellation) -> Result<Completion, EngineError> {
        Ok(self.0.read(
            Path::new("/usr/sbin/pkgutil"),
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            Limits {
                timeout: Duration::from_secs(10),
                output_bytes: 8 * 1024 * 1024,
            },
            cancel,
        )?)
    }
    /// Installed receipt IDs matching a cask's `uninstall pkgutil:` pattern,
    /// which Homebrew treats as a regular expression. No match is not an error.
    fn receipts(&self, pattern: &str, cancel: &Cancellation) -> Vec<String> {
        match self.pkgutil(&[&format!("--pkgs={pattern}")], cancel) {
            Ok(result) if result.code == Some(0) && !result.truncated => {
                String::from_utf8_lossy(&result.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|id| receipt_id(id))
                    .map(str::to_owned)
                    .collect()
            }
            _ => vec![],
        }
    }
    fn receipt_bundles(
        &self,
        id: &str,
        cancel: &Cancellation,
    ) -> Result<Vec<PathBuf>, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let (Ok(info), Ok(files)) = (
            self.pkgutil(&["--pkg-info", id], cancel),
            self.pkgutil(&["--files", id], cancel),
        ) else {
            return Ok(vec![]);
        };
        if info.code != Some(0) || files.code != Some(0) || info.truncated || files.truncated {
            return Ok(vec![]);
        }
        Ok(receipt_bundles(
            &String::from_utf8_lossy(&info.stdout),
            &String::from_utf8_lossy(&files.stdout),
        ))
    }
}

#[cfg(any(target_os = "macos", test))]
fn receipt_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 255
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// The receipt patterns each installed cask names in `uninstall pkgutil:`.
/// A cask that runs an installer package leaves no app link, but its uninstall
/// step names the receipts the package wrote.
#[cfg(any(target_os = "macos", test))]
fn receipt_patterns(data: &[u8]) -> Result<Vec<(String, Vec<String>)>, EngineError> {
    #[derive(Deserialize)]
    struct InstalledCask {
        full_token: String,
        installed: Option<String>,
        artifacts: Vec<Value>,
    }
    #[derive(Deserialize)]
    struct Report {
        casks: Vec<InstalledCask>,
    }
    let report: Report = serde_json::from_slice(data).map_err(|e| invalid(ID, e))?;
    Ok(report
        .casks
        .into_iter()
        .filter(|cask| cask.installed.is_some() && cask_token(&cask.full_token))
        .filter_map(|cask| {
            let patterns: Vec<String> = cask
                .artifacts
                .iter()
                .filter_map(|artifact| artifact.get("uninstall")?.as_array())
                .flatten()
                .filter_map(|step| step.get("pkgutil"))
                .flat_map(|value| match value {
                    Value::String(pattern) => vec![pattern.clone()],
                    Value::Array(patterns) => patterns
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                    _ => vec![],
                })
                .filter(|pattern| {
                    !pattern.is_empty()
                        && pattern.len() <= 255
                        && !pattern.chars().any(char::is_control)
                })
                .collect();
            (!patterns.is_empty()).then_some((cask.full_token, patterns))
        })
        .collect())
}

/// App bundles a receipt installed: its location when that is a bundle, or
/// each listed `.app` that is not inside another bundle.
#[cfg(any(target_os = "macos", test))]
fn receipt_bundles(info: &str, files: &str) -> Vec<PathBuf> {
    let field = |name: &str| {
        info.lines()
            .find_map(|line| line.strip_prefix(name))
            .map(str::trim)
    };
    let (Some(volume), Some(location)) = (field("volume:"), field("location:")) else {
        return vec![];
    };
    let volume = Path::new(volume);
    // Packages installed at the volume root report their location as `/`.
    let location = if location == "/" { "" } else { location };
    if !volume.is_absolute() || !(location.is_empty() || relative_artifact(Path::new(location))) {
        return vec![];
    }
    let base = volume.join(location);
    let is_bundle = |path: &Path| path.extension().is_some_and(|ext| ext == "app");
    if is_bundle(&base) {
        return vec![base];
    }
    files
        .lines()
        .map(Path::new)
        .filter(|path| relative_artifact(path) && is_bundle(path))
        .filter(|path| path.ancestors().skip(1).all(|parent| !is_bundle(parent)))
        .map(|path| base.join(path))
        .collect()
}

fn relative_artifact(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

/// Homebrew leaves an app-artifact symlink in its installed version directory.
/// Match its canonical destination, never just the app name or bundle ID. Only
/// records from `brew info --installed` participate; stale staging directories
/// and a second copy of the same app do not establish ownership. A renamed
/// cask keeps its Caskroom folder under an old token, and its current
/// definition may no longer list the app it installed, so its installed
/// version folder is also searched for app links under each old token.
#[cfg(any(target_os = "macos", test))]
fn cask_owners(root: &Path, data: &[u8]) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError> {
    #[derive(Deserialize)]
    struct InstalledCask {
        token: String,
        full_token: String,
        installed: Option<String>,
        artifacts: Vec<Value>,
        #[serde(default)]
        old_tokens: Vec<String>,
    }
    #[derive(Deserialize)]
    struct Report {
        casks: Vec<InstalledCask>,
    }
    let report: Report = serde_json::from_slice(data).map_err(|e| invalid(ID, e))?;
    let mut owners: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    for cask in report.casks {
        let Some(version) = cask.installed else {
            continue;
        };
        if !cask_token(&cask.token)
            || cask.token.contains('/')
            || !cask_token(&cask.full_token)
            || !relative_artifact(Path::new(&version))
            || Path::new(&version).components().count() != 1
        {
            return Err(invalid(ID, "invalid installed cask identity"));
        }
        let mut links = BTreeSet::new();
        for artifact in cask.artifacts {
            let Some(source) = artifact
                .get("app")
                .and_then(Value::as_array)
                .and_then(|app| app.first())
                .and_then(Value::as_str)
            else {
                continue;
            };
            if !relative_artifact(Path::new(source)) {
                return Err(invalid(ID, "invalid Homebrew app artifact"));
            }
            links.insert(root.join(&cask.token).join(&version).join(source));
        }
        for old in cask
            .old_tokens
            .iter()
            .filter(|old| cask_token(old) && !old.contains('/'))
        {
            let folder = root.join(old).join(&version);
            let Ok(entries) = fs::read_dir(&folder) else {
                continue;
            };
            links.extend(
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "app")),
            );
        }
        for link in links {
            if fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink()) {
                if let Ok(target) = fs::canonicalize(&link) {
                    owners
                        .entry(target)
                        .or_default()
                        .push(cask.full_token.clone());
                }
            }
        }
    }
    for names in owners.values_mut() {
        names.sort();
        names.dedup();
    }
    Ok(owners)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= 512 && !s.chars().any(char::is_control))
}

fn cask_candidate(bundle_id: &str) -> Option<&'static str> {
    match bundle_id {
        "com.microsoft.VSCode" => Some("visual-studio-code"),
        "org.mozilla.firefox" => Some("firefox"),
        "md.obsidian" => Some("obsidian"),
        _ => None,
    }
}

/// How many "Name N.app" spellings the Trash is searched for a free name.
const TRASH_NAMES: usize = 100;

/// A plain-language refusal, shown as it is.
pub(super) fn refused(reason: impl Into<String>) -> EngineError {
    ExecutionError::Invalid(reason.into()).into()
}

/// The path people see. mas reports App Store apps through the Data
/// volume's firmlink (`/System/Volumes/Data/Applications/…`).
pub(super) fn visible(path: &Path) -> PathBuf {
    path.strip_prefix("/System/Volumes/Data")
        .map_or_else(|_| path.to_owned(), |rest| Path::new("/").join(rest))
}

/// Apps that come with macOS live on the sealed system volume (Safari in its
/// Cryptex, reached through an alias in /Applications); System Integrity
/// Protection's restricted flag marks any other protected item.
pub(super) fn part_of_macos(path: &Path) -> bool {
    let canonical = visible(&fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()));
    canonical.starts_with("/System") || restricted(&canonical)
}

#[cfg(target_os = "macos")]
fn restricted(path: &Path) -> bool {
    use std::os::macos::fs::MetadataExt;
    // SF_RESTRICTED in <sys/stat.h>.
    fs::symlink_metadata(path).is_ok_and(|m| m.st_flags() & 0x0008_0000 != 0)
}

#[cfg(not(target_os = "macos"))]
fn restricted(_: &Path) -> bool {
    false
}

/// Takes an app bundle off the Mac by moving it to the Trash, where it can
/// be put back. The Mac App Store source and this inventory share it.
pub(super) struct Remover<'a> {
    pub(super) transport: &'a dyn Transport,
    /// The person PkgDeck runs for; their Trash receives the app.
    pub(super) uid: u32,
    pub(super) backend: &'static str,
}

impl Remover<'_> {
    /// Refuses what must stay: parts of macOS, aliases, and open apps. Also
    /// refuses to start as root, whose Trash is not the person's.
    pub(super) fn check(
        &self,
        name: &str,
        path: &Path,
        cancel: &Cancellation,
    ) -> Result<(), EngineError> {
        if self.uid == 0 {
            return Err(refused(crate::host::ROOT_REFUSAL));
        }
        if part_of_macos(path) {
            return Err(refused(format!(
                "{name} is part of macOS, so PkgDeck won't remove it."
            )));
        }
        let metadata = fs::symlink_metadata(path).map_err(|_| EngineError::NotFound)?;
        if metadata.file_type().is_symlink() {
            return Err(refused(format!(
                "{} is an alias, not the app itself. Remove the app it points to.",
                path.display()
            )));
        }
        let output = self
            .transport
            .macos_tool(
                Path::new("/bin/ps"),
                &["-axww".into(), "-o".into(), "comm=".into()],
                cancel,
                false,
            )
            .map_err(EngineError::from)
            .and_then(|result| bytes(self.backend, result))?;
        let open = String::from_utf8_lossy(&output)
            .lines()
            .any(|line| visible(Path::new(line.trim())).starts_with(path));
        if open {
            return Err(refused(format!(
                "{name} is open. Quit it, then remove it again."
            )));
        }
        Ok(())
    }

    /// Moves the bundle to the Trash and checks that it left. An app the
    /// person owns in a folder they can change goes through `/usr/bin/trash`
    /// with no prompt; anything else (App Store apps belong to root) is moved
    /// by the system's `mv` after the administrator password prompt.
    pub(super) fn trash(
        &self,
        name: &str,
        path: &Path,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        use std::os::unix::fs::MetadataExt;
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let metadata = fs::symlink_metadata(path).map_err(|_| EngineError::NotFound)?;
        let owned = metadata.uid() == self.uid
            && path.parent().is_some_and(|parent| {
                rustix::fs::access(parent, rustix::fs::Access::WRITE_OK).is_ok()
            });
        let result = if owned {
            progress(Progress::Message(format!("Moving {name} to the Trash")));
            self.transport.macos_tool(
                Path::new("/usr/bin/trash"),
                &["-s".into(), path.into()],
                cancel,
                true,
            )
        } else {
            let destination = self.destination(path)?;
            progress(Progress::Message(format!(
                "Moving {name} to the Trash. It belongs to the system, so macOS asks for an administrator password."
            )));
            self.transport.system_manager(
                "mv",
                &["-n".into(), "--".into(), path.into(), destination.into()],
                cancel,
                true,
            )
        }?;
        if result.code != Some(0) {
            if String::from_utf8_lossy(&result.stderr).contains("sudo:") {
                return Err(ExecutionError::AuthorizationDenied.into());
            }
            return Err(ExecutionError::Failed(result).into());
        }
        if fs::symlink_metadata(path).is_ok() {
            return Err(refused(format!(
                "{name} is still at {} after moving it to the Trash.",
                path.display()
            )));
        }
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }

    /// A free name in the person's Trash, the way Finder numbers copies.
    fn destination(&self, path: &Path) -> Result<PathBuf, EngineError> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        let home = self
            .transport
            .env("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
            .ok_or_else(|| {
                refused("PkgDeck can't find your home folder, so it has no Trash to use.")
            })?;
        let trash = home.join(".Trash");
        if fs::symlink_metadata(&trash).is_err() {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&trash)
                .map_err(ExecutionError::from)?;
        }
        // The Trash must be the home owner's own folder, never an alias
        // that sends the app somewhere else.
        let home_owner = fs::metadata(&home).map_err(ExecutionError::from)?.uid();
        if !fs::symlink_metadata(&trash).is_ok_and(|m| m.is_dir() && m.uid() == home_owner) {
            return Err(refused(format!(
                "{} isn't a folder you own, so PkgDeck won't move apps into it.",
                trash.display()
            )));
        }
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        (1..=TRASH_NAMES)
            .map(|n| match n {
                1 => trash.join(format!("{stem}.app")),
                n => trash.join(format!("{stem} {n}.app")),
            })
            .find(|candidate| fs::symlink_metadata(candidate).is_err())
            .ok_or_else(|| {
                refused(format!(
                    "The Trash already holds {TRASH_NAMES} apps named {stem}. Empty it, then try again."
                ))
            })
    }
}

/// One listed bundle and the identity its removal is checked against.
#[derive(Clone)]
struct Listed {
    details: PackageDetails,
    bundle_id: Option<String>,
    canonical: PathBuf,
}

pub struct MacApps {
    roots: Vec<(PathBuf, Scope)>,
    io: Box<dyn AppIo>,
    snapshot: Option<Vec<Listed>>,
    scan_errors: Vec<EngineError>,
    exact_query: bool,
    selected: Option<Listed>,
    /// Runs Homebrew, the Trash and the password prompt for removals.
    transport: Box<dyn Transport>,
    uid: u32,
}

impl MacApps {
    pub fn native(transport: NativeTransport) -> Self {
        let host = transport.host.clone();
        let mut roots = vec![(PathBuf::from("/Applications"), Scope::System)];
        if let Some(home) = host
            .var("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
        {
            roots.push((
                home.join("Applications"),
                Scope::User {
                    uid: rustix::process::getuid().as_raw(),
                },
            ));
        }
        Self {
            roots,
            #[cfg(target_os = "macos")]
            io: Box::new(NativeApps(host)),
            #[cfg(not(target_os = "macos"))]
            io: Box::new(NativeApps),
            snapshot: None,
            scan_errors: vec![],
            exact_query: false,
            selected: None,
            transport: Box::new(transport),
            uid: rustix::process::getuid().as_raw(),
        }
    }

    fn inventory(&mut self, cancel: &Cancellation) -> Result<&[Listed], EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        if self.snapshot.is_none() {
            let ownership = self.io.ownership(cancel);
            if cancel.requested() || matches!(&ownership, Err(EngineError::Cancelled)) {
                return Err(EngineError::Cancelled);
            }
            let mut apps = Vec::new();
            let mut seen = BTreeSet::new();
            let mut budget = MAX_ENTRIES;
            let mut errors = Vec::new();
            for (root, scope) in &self.roots {
                self.scan(
                    root,
                    scope,
                    0,
                    &mut budget,
                    &mut seen,
                    &mut apps,
                    &mut errors,
                    &ownership,
                    cancel,
                )?;
            }
            apps.sort_by(|a, b| a.details.package.id.cmp(&b.details.package.id));
            self.snapshot = Some(apps);
            self.scan_errors = errors;
        }
        Ok(self.snapshot.as_deref().unwrap_or_default())
    }

    /// Validate an exact details target using the same traversal boundaries as
    /// discovery, without enumerating siblings or reading their metadata.
    fn details_path(&self, id: &PackageId) -> Result<PathBuf, EngineError> {
        let path = Path::new(&id.name);
        if id.backend != ID
            || id.architecture != "unknown"
            || id.remote.is_some()
            || id.reference.as_deref() != Some(id.name.as_str())
            || !path.is_absolute()
            || path.extension().is_none_or(|ext| ext != "app")
            || !path.is_dir()
        {
            return Err(EngineError::NotFound);
        }
        for (root, scope) in &self.roots {
            if &id.scope != scope {
                continue;
            }
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            if !relative_artifact(relative) {
                continue;
            }
            let parts: Vec<_> = relative.components().collect();
            if !(1..=5).contains(&parts.len()) {
                continue;
            }
            let mut parent = root.clone();
            let valid = parts[..parts.len() - 1].iter().all(|part| {
                let name = part.as_os_str();
                parent.push(name);
                !name.as_encoded_bytes().starts_with(b".")
                    && parent.extension().is_none_or(|ext| ext != "app")
                    && fs::symlink_metadata(&parent).is_ok_and(|m| m.is_dir())
            });
            if valid {
                return fs::canonicalize(path).map_err(|e| ExecutionError::from(e).into());
            }
        }
        Err(EngineError::NotFound)
    }

    #[allow(clippy::too_many_arguments)]
    fn scan(
        &self,
        root: &Path,
        scope: &Scope,
        depth: usize,
        budget: &mut usize,
        seen: &mut BTreeSet<PathBuf>,
        apps: &mut Vec<Listed>,
        errors: &mut Vec<EngineError>,
        ownership: &Result<BTreeMap<PathBuf, Vec<String>>, EngineError>,
        cancel: &Cancellation,
    ) -> Result<(), EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let entries = match self.io.read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if depth > 0 => {
                errors.push(invalid(
                    ID,
                    format!("skipped folder {}: {error}", root.display()),
                ));
                return Ok(());
            }
            Err(error) => return Err(ExecutionError::from(error).into()),
        };
        // Sorting makes the chosen path deterministic when aliases exist.
        let mut paths = Vec::new();
        for entry in entries {
            if cancel.requested() {
                return Err(EngineError::Cancelled);
            }
            *budget = budget
                .checked_sub(1)
                .ok_or_else(|| invalid(ID, "application inventory exceeds 4096 entries"))?;
            match entry {
                Ok(path) => paths.push(path),
                Err(error) => errors.push(invalid(
                    ID,
                    format!("skipped entry in {}: {error}", root.display()),
                )),
            }
        }
        paths.sort();
        for path in paths {
            if cancel.requested() {
                return Err(EngineError::Cancelled);
            }
            let metadata = match self.io.symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    errors.push(invalid(
                        ID,
                        format!("skipped entry {}: {error}", path.display()),
                    ));
                    continue;
                }
            };
            if path.extension().is_some_and(|e| e == "app") && path.is_dir() {
                // Identities are strings; a lossy name would point elsewhere
                // and could collide with another bundle's identity.
                if path.to_str().is_none() {
                    errors.push(invalid(
                        ID,
                        format!("skipped entry {}: path is not valid UTF-8", path.display()),
                    ));
                    continue;
                }
                let canonical = match self.io.canonicalize(&path) {
                    Ok(canonical) => canonical,
                    Err(error) => {
                        errors.push(invalid(
                            ID,
                            format!("skipped entry {}: {error}", path.display()),
                        ));
                        continue;
                    }
                };
                if seen.insert(canonical.clone()) {
                    apps.push(self.describe(&path, &canonical, scope, ownership, cancel)?);
                }
                // Never recurse into app bundles and list their helper apps.
            } else if metadata.is_dir()
                && depth < 4
                && !path
                    .file_name()
                    .is_some_and(|n| n.as_encoded_bytes().starts_with(b"."))
            {
                self.scan(
                    &path,
                    scope,
                    depth + 1,
                    budget,
                    seen,
                    apps,
                    errors,
                    ownership,
                    cancel,
                )?;
            }
        }
        Ok(())
    }

    fn describe(
        &self,
        path: &Path,
        canonical: &Path,
        scope: &Scope,
        ownership: &Result<BTreeMap<PathBuf, Vec<String>>, EngineError>,
        cancel: &Cancellation,
    ) -> Result<Listed, EngineError> {
        let metadata = self.io.plist(&path.join("Contents/Info.plist"), cancel);
        if cancel.requested() || matches!(&metadata, Err(EngineError::Cancelled)) {
            return Err(EngineError::Cancelled);
        }
        let value = metadata.as_ref().ok().unwrap_or(&Value::Null);
        let bundle_id = text(value, "CFBundleIdentifier");
        let display = text(value, "CFBundleDisplayName")
            .or_else(|| text(value, "CFBundleName"))
            .map(str::to_owned)
            .unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into()
            });
        let version =
            text(value, "CFBundleShortVersionString").or_else(|| text(value, "CFBundleVersion"));
        let owner = match ownership {
            Ok(records) => match records.get(canonical) {
                Some(names) if names.len() == 1 => format!("Managed by Homebrew ({})", names[0]),
                Some(names) => format!("Homebrew ownership is ambiguous ({})", names.join(", ")),
                // Casks that install a .pkg, renamed casks and broken
                // Caskroom records leave no app link, so this is unknown,
                // not proof that Homebrew did not install the app.
                None => "Homebrew ownership unknown: no cask app link points here".into(),
            },
            Err(_) => "Homebrew ownership could not be checked".into(),
        };
        let receipt = path.join("Contents/_MASReceipt/receipt").is_file();
        let has_owner = ownership
            .as_ref()
            .is_ok_and(|records| records.contains_key(canonical));
        let candidate = bundle_id
            .and_then(cask_candidate)
            .filter(|_| !receipt && !has_owner);
        let suggestion =
            candidate.map(|token| format!("Available through Homebrew: {token} (candidate)"));
        // Only casks PkgDeck can check an existing copy against may take it
        // over, and only once Homebrew's records were read: a failed check
        // could hide an owner. Installing the cask re-checks this copy first.
        let adopt_with =
            candidate.filter(|token| ownership.is_ok() && super::adopt::rule(token).is_some());
        let mut description = vec![format!("Location: {}", path.display()), owner.clone()];
        if let Some(id) = bundle_id {
            description.push(format!("Bundle identifier: {id}"));
        }
        if let Some(build) = text(value, "CFBundleVersion") {
            description.push(format!("Bundle build: {build}"));
        }
        if let Err(error) = &metadata {
            description.push(format!("Bundle metadata unavailable: {error}"));
        }
        if let Err(error) = ownership {
            description.push(format!("Homebrew check: {error}"));
        }
        if receipt {
            description.push("App Store receipt present. Keep App Store management (the Mac App Store source updates it through mas); no cask suggestion is offered.".into());
        } else if let Some(suggestion) = &suggestion {
            description.push(suggestion.clone());
            description.push("Matching evidence: the bundle identifier matches PkgDeck's curated cask catalog. Publisher signature, edition/channel, architecture, and artifact equality have not been verified here.".into());
            if let Some(token) = adopt_with {
                description.push(format!("To let Homebrew manage this copy, install the {token} cask. PkgDeck first checks the publisher signature, version, architecture, and every file the cask adds, and keeps a copy of the app until Homebrew finishes."));
            }
        }
        if part_of_macos(canonical) {
            description.push("Part of macOS, so PkgDeck won't remove it.".into());
        }
        description.push("PkgDeck can't install or update apps from this source. Removing an app moves it to the Trash; Homebrew uninstalls the apps it manages.".into());
        let name = path
            .to_str()
            .ok_or_else(|| invalid(ID, format!("{} is not valid UTF-8", path.display())))?;
        let summary = format!(
            "{owner}{} · {}",
            suggestion
                .as_ref()
                .map(|s| format!(" · {s}"))
                .unwrap_or_default(),
            path.display()
        );
        let details = PackageDetails {
            package: Package {
                id: PackageId {
                    backend: ID.into(),
                    name: name.into(),
                    architecture: "unknown".into(),
                    scope: scope.clone(),
                    remote: None,
                    reference: Some(name.into()),
                },
                display_name: display,
                summary,
                // The engine requires an installed marker. Unknown is explicit,
                // never inferred from a cask's version or the host architecture.
                installed_version: Some(version.unwrap_or("unknown").into()),
                candidate_version: None,
                update: UpdateAvailability::Unknown,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
                adopt_with: adopt_with.map(str::to_owned),
            },
            description: description.join("\n\n"),
            homepage: candidate.map(|token| format!("https://formulae.brew.sh/cask/{token}")),
            dependencies: vec![],
        };
        Ok(Listed {
            details,
            bundle_id: bundle_id.map(str::to_owned),
            canonical: canonical.to_owned(),
        })
    }

    /// Removes one listed app: Homebrew uninstalls a cask it manages, any
    /// other app goes to the Trash. The bundle must still be the one listed.
    fn remove(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let canonical = self.details_path(id)?;
        let listed = self
            .selected
            .iter()
            .chain(self.snapshot.iter().flatten())
            .find(|listed| listed.details.package.id == *id)
            .cloned();
        // Whatever happens next, the listing may no longer be true.
        self.snapshot = None;
        self.selected = None;
        let path = Path::new(&id.name);
        let ownership = self.io.ownership(cancel);
        if cancel.requested() || matches!(&ownership, Err(EngineError::Cancelled)) {
            return Err(EngineError::Cancelled);
        }
        let now = self.describe(path, &canonical, &id.scope, &ownership, cancel)?;
        if listed.is_some_and(|listed| {
            listed.bundle_id != now.bundle_id || listed.canonical != now.canonical
        }) {
            return Err(refused(format!(
                "{} changed since PkgDeck listed it. Reload, then try again.",
                path.display()
            )));
        }
        let name = now.details.package.display_name;
        let remover = Remover {
            transport: &*self.transport,
            uid: self.uid,
            backend: ID,
        };
        remover.check(&name, path, cancel)?;
        let owners = match &ownership {
            Ok(records) => records.get(&canonical).cloned().unwrap_or_default(),
            // Without Homebrew nothing can manage the app.
            Err(EngineError::Execution(ExecutionError::Disabled(_))) => vec![],
            Err(error) => {
                return Err(refused(format!(
                    "PkgDeck couldn't check whether Homebrew manages {name} ({error}), so it won't remove it."
                )))
            }
        };
        let token = match owners.as_slice() {
            [] => return remover.trash(&name, path, cancel, progress),
            [token] => token,
            _ => {
                return Err(refused(format!(
                "Homebrew's records name more than one cask for {name} ({}). Remove it with brew.",
                owners.join(", ")
            )))
            }
        };
        progress(Progress::Message(format!(
            "Uninstalling {name} with Homebrew ({token})"
        )));
        let result = self.transport.brew(
            &[
                "uninstall".into(),
                "--cask".into(),
                "--force".into(),
                "--".into(),
                token.into(),
            ],
            cancel,
            true,
        )?;
        if fs::symlink_metadata(path).is_ok() {
            return Err(refused(format!(
                "Homebrew finished, but {name} is still at {}.",
                path.display()
            )));
        }
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}

impl Backend for MacApps {
    fn id(&self) -> &str {
        ID
    }
    fn capabilities(&self) -> &[Capability] {
        &[
            Capability::Search,
            Capability::Installed,
            Capability::Details,
            Capability::Remove,
        ]
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        Ok(if cfg!(target_os = "macos") {
            Availability::Available
        } else {
            Availability::Unavailable("Application bundle discovery requires macOS".into())
        })
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.exact_query = false;
        Ok(self
            .inventory(cancel)?
            .iter()
            .map(|d| d.details.package.clone())
            .collect())
    }
    fn query_errors(&self) -> Vec<EngineError> {
        if self.exact_query {
            vec![]
        } else {
            self.scan_errors.clone()
        }
    }
    fn may_have(&self, name: &str) -> bool {
        let path = Path::new(name);
        path.is_absolute() && path.extension().is_some_and(|ext| ext == "app")
    }
    fn lookup(&mut self, name: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.exact_query = true;
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let Some((_, scope)) = self
            .roots
            .iter()
            .find(|(root, _)| Path::new(name).starts_with(root))
        else {
            return Ok(vec![]);
        };
        let id = PackageId {
            backend: ID.into(),
            name: name.into(),
            architecture: "unknown".into(),
            scope: scope.clone(),
            remote: None,
            reference: Some(name.into()),
        };
        match self.listed(&id, cancel) {
            Ok(listed) => {
                let package = listed.details.package.clone();
                self.selected = Some(listed);
                Ok(vec![package])
            }
            Err(EngineError::NotFound) => Ok(vec![]),
            Err(error) => Err(error),
        }
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let query = query.to_lowercase();
        Ok(self
            .installed(cancel)?
            .into_iter()
            .filter(|p| {
                p.id.name.to_lowercase().contains(&query)
                    || p.display_name.to_lowercase().contains(&query)
            })
            .collect())
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        Ok(self.listed(id, cancel)?.details)
    }
    /// Only removal: this source never installs or updates an app.
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let Operation::Remove(id) = operation else {
            return Err(self.unsupported(operation.capability()));
        };
        self.remove(id, cancel, progress)
    }
}

impl MacApps {
    fn listed(&mut self, id: &PackageId, cancel: &Cancellation) -> Result<Listed, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        if let Some(selected) = self
            .selected
            .as_ref()
            .filter(|d| d.details.package.id == *id)
        {
            return Ok(selected.clone());
        }
        if let Some(snapshot) = &self.snapshot {
            return snapshot
                .iter()
                .find(|d| d.details.package.id == *id)
                .cloned()
                .ok_or(EngineError::NotFound);
        }
        let canonical = self.details_path(id)?;
        let ownership = self.io.ownership(cancel);
        if cancel.requested() || matches!(&ownership, Err(EngineError::Cancelled)) {
            return Err(EngineError::Cancelled);
        }
        self.describe(
            Path::new(&id.name),
            &canonical,
            &id.scope,
            &ownership,
            cancel,
        )
    }
}

/// A transport that moves fixture bundles the way the Trash, the password
/// prompt and Homebrew would, and records what ran.
#[cfg(test)]
pub(super) mod removal_fakes {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// What the administrator `mv` does.
    #[derive(Clone, Copy, PartialEq)]
    pub(crate) enum Privileged {
        Move,
        /// The person pressed Cancel in the password dialog.
        Cancel,
        /// `sudo -n` had no password to use.
        Denied,
        Fail,
        /// Reports success without moving anything.
        Ignore,
    }

    #[derive(Clone)]
    pub(crate) struct Mover {
        pub(crate) calls: Arc<Mutex<Vec<String>>>,
        pub(crate) home: Option<PathBuf>,
        /// What `ps` lists, one executable per line.
        pub(crate) ps: String,
        pub(crate) privileged: Privileged,
        /// The bundle `brew uninstall` takes away, if any.
        pub(crate) brew_removes: Option<PathBuf>,
        /// Cancel while PkgDeck checks for open apps.
        pub(crate) cancel_on_ps: bool,
        pub(crate) ps_fails: bool,
        /// stderr of a failing `brew uninstall`.
        pub(crate) brew_error: Option<&'static str>,
    }

    pub(crate) fn completion(code: i32, stderr: &str) -> Completion {
        Completion {
            code: Some(code),
            signal: None,
            stdout: vec![],
            stderr: stderr.as_bytes().to_vec(),
            truncated: false,
            cancellation_deferred: false,
        }
    }

    impl Mover {
        pub(crate) fn new(home: PathBuf) -> Self {
            Self {
                calls: Arc::default(),
                home: Some(home),
                ps: "/usr/libexec/launchd\n".into(),
                privileged: Privileged::Move,
                brew_removes: None,
                cancel_on_ps: false,
                ps_fails: false,
                brew_error: None,
            }
        }
        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        fn record(&self, executable: &str, args: &[OsString]) -> Vec<String> {
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            self.calls
                .lock()
                .unwrap()
                .push(format!("{executable} {}", args.join(" ")));
            args
        }
    }

    impl Transport for Mover {
        fn env(&self, name: &str) -> Option<OsString> {
            assert_eq!(name, "HOME");
            self.home.clone().map(Into::into)
        }
        fn macos_tool(
            &self,
            executable: &Path,
            args: &[OsString],
            cancel: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            let args = self.record(&executable.to_string_lossy(), args);
            if executable == Path::new("/bin/ps") {
                assert!(!write);
                if self.cancel_on_ps {
                    cancel.cancel();
                }
                if self.ps_fails {
                    return Ok(completion(1, "ps: sysctl failed\n"));
                }
                let mut result = completion(0, "");
                result.stdout = self.ps.clone().into_bytes();
                return Ok(result);
            }
            assert_eq!(executable, Path::new("/usr/bin/trash"));
            assert!(write);
            assert_eq!(args[0], "-s");
            let source = Path::new(&args[1]);
            let trash = self.home.as_ref().unwrap().join(".Trash");
            fs::create_dir_all(&trash).unwrap();
            fs::rename(source, trash.join(source.file_name().unwrap())).unwrap();
            Ok(completion(0, ""))
        }
        fn system_manager(
            &self,
            executable: &str,
            args: &[OsString],
            _: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            let args = self.record(executable, args);
            assert_eq!(executable, "mv");
            assert!(write);
            assert_eq!(args[..2], ["-n", "--"]);
            match self.privileged {
                Privileged::Move => {
                    fs::rename(&args[2], &args[3]).unwrap();
                    Ok(completion(0, ""))
                }
                Privileged::Cancel => Err(ExecutionError::AuthorizationCancelled),
                Privileged::Denied => Ok(completion(1, "sudo: a password is required\n")),
                Privileged::Fail => Ok(completion(1, "mv: rename failed\n")),
                Privileged::Ignore => Ok(completion(0, "")),
            }
        }
        fn brew(
            &self,
            args: &[OsString],
            _: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            let args = self.record("brew", args);
            assert!(write);
            assert_eq!(args[..4], ["uninstall", "--cask", "--force", "--"]);
            if let Some(stderr) = self.brew_error {
                return Err(ExecutionError::Failed(completion(1, stderr)));
            }
            if let Some(app) = &self.brew_removes {
                fs::remove_dir_all(app).unwrap();
            }
            Ok(completion(0, ""))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::removal_fakes::*;
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "pkgdeck-mac-apps-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn bundle(&self, name: &str, id: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(path.join("Contents")).unwrap();
            fs::write(
                path.join("Contents/Info.plist"),
                json!({
                    "CFBundleIdentifier": id, "CFBundleName": "Fixture App",
                    "CFBundleShortVersionString": "1.2.3", "CFBundleVersion": "456"
                })
                .to_string(),
            )
            .unwrap();
            path
        }
        fn backend(
            &self,
            ownership: Result<BTreeMap<PathBuf, Vec<String>>, EngineError>,
        ) -> MacApps {
            MacApps {
                roots: vec![(self.0.clone(), Scope::System)],
                io: Box::new(FakeIo(ownership)),
                snapshot: None,
                scan_errors: vec![],
                exact_query: false,
                selected: None,
                transport: Box::new(self.mover()),
                uid: rustix::process::getuid().as_raw(),
            }
        }
        fn mover(&self) -> Mover {
            Mover::new(self.0.join("home"))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct FakeIo(Result<BTreeMap<PathBuf, Vec<String>>, EngineError>);
    impl AppIo for FakeIo {
        fn plist(&self, path: &Path, _: &Cancellation) -> Result<Value, EngineError> {
            let data = fs::read(path).map_err(ExecutionError::from)?;
            serde_json::from_slice(&data).map_err(|e| invalid(ID, e))
        }
        fn ownership(
            &self,
            _: &Cancellation,
        ) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError> {
            self.0.clone()
        }
    }
    fn record(token: &str, source: &str, version: Value) -> Value {
        json!({"token": token, "full_token": token, "installed": version, "artifacts": [{"app": [source]}]})
    }
    fn owners(
        root: &Path,
        casks: Vec<Value>,
    ) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError> {
        cask_owners(root, &serde_json::to_vec(&json!({"casks": casks})).unwrap())
    }

    #[test]
    fn exact_package_names_do_not_probe_the_application_inventory() {
        let f = Fixture::new();
        let backend = f.backend(Err(EngineError::Cancelled));
        for name in [
            "curl",
            "obsidian",
            "App.app",
            "/Applications",
            "/Applications/App.app/Contents",
        ] {
            assert!(!backend.may_have(name), "{name}");
        }
        assert!(backend.may_have("/Applications/Utilities/App.app"));
        let mut engine = Engine::default();
        engine.register(backend).unwrap();
        let report = engine.lookup("curl", &Cancellation::default());
        assert!(report.failures.is_empty());
        assert!(report.packages.is_empty());
        assert!(report.successful_sources.is_empty());
    }

    #[test]
    fn disappearing_entries_preserve_siblings_and_report_partial_inventory() {
        struct VanishingApp {
            path: PathBuf,
            during_canonicalize: bool,
        }
        impl AppIo for VanishingApp {
            fn symlink_metadata(&self, path: &Path) -> std::io::Result<fs::Metadata> {
                if path == self.path && !self.during_canonicalize {
                    fs::remove_dir_all(path)?;
                }
                fs::symlink_metadata(path)
            }
            fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
                if path == self.path && self.during_canonicalize {
                    fs::remove_dir_all(path)?;
                }
                fs::canonicalize(path)
            }
            fn plist(&self, path: &Path, cancel: &Cancellation) -> Result<Value, EngineError> {
                FakeIo(Ok(BTreeMap::new())).plist(path, cancel)
            }
            fn ownership(
                &self,
                _: &Cancellation,
            ) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError> {
                Ok(BTreeMap::new())
            }
        }
        for during_canonicalize in [false, true] {
            let f = Fixture::new();
            let first = f.bundle("A.app", "md.obsidian");
            let gone = f.bundle("M.app", "md.obsidian");
            let last = f.bundle("Z.app", "md.obsidian");
            let mut backend = f.backend(Ok(BTreeMap::new()));
            backend.io = Box::new(VanishingApp {
                path: gone.clone(),
                during_canonicalize,
            });
            let packages = backend.installed(&Cancellation::default()).unwrap();
            assert_eq!(
                packages
                    .iter()
                    .map(|p| PathBuf::from(&p.id.name))
                    .collect::<Vec<_>>(),
                vec![first, last]
            );
            let errors = backend.query_errors();
            assert_eq!(errors.len(), 1);
            assert!(errors[0].to_string().contains(gone.to_str().unwrap()));
        }
    }

    // APFS refuses non-UTF-8 names, so only Linux can create this fixture.
    #[cfg(target_os = "linux")]
    #[test]
    fn non_utf8_bundle_paths_are_skipped_instead_of_published_lossily() {
        use std::os::unix::ffi::OsStrExt;
        let f = Fixture::new();
        let kept = f.bundle("A.app", "md.obsidian");
        let raw = f.0.join(std::ffi::OsStr::from_bytes(b"Bad\xff.app"));
        fs::create_dir_all(raw.join("Contents")).unwrap();
        let mut backend = f.backend(Ok(BTreeMap::new()));
        let packages = backend.installed(&Cancellation::default()).unwrap();
        assert_eq!(
            packages
                .iter()
                .map(|p| PathBuf::from(&p.id.name))
                .collect::<Vec<_>>(),
            vec![kept]
        );
        let errors = backend.query_errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("not valid UTF-8"));
    }

    #[test]
    fn unreadable_nested_folders_preserve_siblings_and_report_the_skipped_path() {
        struct UnreadableFolder(PathBuf);
        impl AppIo for UnreadableFolder {
            fn read_dir(&self, path: &Path) -> std::io::Result<Entries> {
                if path == self.0 {
                    return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
                }
                FakeIo(Ok(BTreeMap::new())).read_dir(path)
            }
            fn plist(&self, path: &Path, cancel: &Cancellation) -> Result<Value, EngineError> {
                FakeIo(Ok(BTreeMap::new())).plist(path, cancel)
            }
            fn ownership(
                &self,
                _: &Cancellation,
            ) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError> {
                Ok(BTreeMap::new())
            }
        }
        let f = Fixture::new();
        f.bundle("A.app", "md.obsidian");
        f.bundle("Restricted/Hidden.app", "md.obsidian");
        f.bundle("Z.app", "md.obsidian");
        let skipped = f.0.join("Restricted");
        let mut backend = f.backend(Ok(BTreeMap::new()));
        backend.io = Box::new(UnreadableFolder(skipped.clone()));
        let cancel = Cancellation::default();
        let packages = backend.installed(&cancel).unwrap();
        assert_eq!(packages.len(), 2);
        assert!(packages.iter().all(|p| !p.id.name.contains("Restricted")));
        let errors = backend.query_errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains(skipped.to_str().unwrap()));
        assert!(errors[0].to_string().contains("skipped folder"));
        // Filtering a cached snapshot must not erase its incompleteness.
        assert!(backend.search("no matches", &cancel).unwrap().is_empty());
        assert_eq!(backend.query_errors(), errors);
        let path = packages[0].id.name.as_str();
        assert_eq!(
            backend.lookup(path, &cancel).unwrap(),
            vec![packages[0].clone()]
        );
        assert!(backend.query_errors().is_empty());
        backend.installed(&cancel).unwrap();
        assert_eq!(
            backend.query_errors(),
            errors,
            "an exact lookup must not erase cached scan errors"
        );
        let mut fresh = f.backend(Ok(BTreeMap::new()));
        fresh.io = Box::new(UnreadableFolder(skipped));
        assert_eq!(
            fresh.lookup(path, &cancel).unwrap(),
            vec![packages[0].clone()]
        );
        assert!(
            fresh.snapshot.is_none(),
            "exact lookup must not scan siblings"
        );
        assert!(fresh.query_errors().is_empty());
        assert_eq!(
            fresh.details(&packages[0].id, &cancel).unwrap().package,
            packages[0]
        );

        // An unreadable root still fails, rather than becoming empty success.
        let mut backend = f.backend(Ok(BTreeMap::new()));
        backend.io = Box::new(UnreadableFolder(f.0.clone()));
        assert!(backend.installed(&cancel).is_err());
        assert!(backend.snapshot.is_none());
    }

    #[test]
    fn cancellation_anywhere_in_a_scan_stops_it_and_bad_entries_are_reported() {
        #[derive(Clone, Copy, PartialEq)]
        enum Step {
            Listing,
            Entry,
            Folder,
            Metadata,
        }
        struct Interrupting {
            cancel: Cancellation,
            at: Option<Step>,
            bad_entry: bool,
        }
        impl AppIo for Interrupting {
            fn read_dir(&self, path: &Path) -> std::io::Result<Entries> {
                if self.at == Some(Step::Listing) {
                    self.cancel.cancel();
                }
                let mut entries: Vec<_> = FakeIo(Ok(BTreeMap::new())).read_dir(path)?.collect();
                if self.bad_entry {
                    entries.push(Err(std::io::Error::other("disk read failed")));
                }
                Ok(Box::new(entries.into_iter()))
            }
            fn symlink_metadata(&self, path: &Path) -> std::io::Result<fs::Metadata> {
                if self.at == Some(Step::Entry)
                    || (self.at == Some(Step::Folder) && path.ends_with("Utilities"))
                {
                    self.cancel.cancel();
                }
                fs::symlink_metadata(path)
            }
            fn plist(&self, path: &Path, cancel: &Cancellation) -> Result<Value, EngineError> {
                if self.at == Some(Step::Metadata) {
                    return Err(EngineError::Cancelled);
                }
                FakeIo(Ok(BTreeMap::new())).plist(path, cancel)
            }
            fn ownership(
                &self,
                _: &Cancellation,
            ) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError> {
                Ok(BTreeMap::new())
            }
        }
        let f = Fixture::new();
        // A plain file sorts first, so the next entry notices a cancellation.
        fs::write(f.0.join("0-readme.txt"), "").unwrap();
        f.bundle("Utilities/Tool.app", "md.obsidian");
        f.bundle("Z.app", "md.obsidian");
        for step in [Step::Listing, Step::Entry, Step::Folder, Step::Metadata] {
            let cancel = Cancellation::default();
            let mut backend = f.backend(Ok(BTreeMap::new()));
            backend.io = Box::new(Interrupting {
                cancel: cancel.clone(),
                at: Some(step),
                bad_entry: false,
            });
            assert!(matches!(
                backend.installed(&cancel),
                Err(EngineError::Cancelled)
            ));
            assert!(backend.snapshot.is_none());
        }
        let cancel = Cancellation::default();
        let mut backend = f.backend(Ok(BTreeMap::new()));
        backend.io = Box::new(Interrupting {
            cancel: cancel.clone(),
            at: None,
            bad_entry: true,
        });
        assert_eq!(backend.installed(&cancel).unwrap().len(), 2);
        let errors = backend.query_errors();
        // Once for the root and once for the nested folder.
        assert_eq!(errors.len(), 2);
        assert!(errors.iter().all(|error| {
            let text = error.to_string();
            text.contains("skipped entry in") && text.contains("disk read failed")
        }));
    }

    #[test]
    fn exact_lookups_stay_inside_the_roots_and_report_shared_ownership() {
        let f = Fixture::new();
        let app = f.bundle("Shared.app", "com.example.shared");
        let name = app.to_str().unwrap();
        let cancel = Cancellation::default();
        let mut backend = f.backend(Ok(BTreeMap::from([(
            fs::canonicalize(&app).unwrap(),
            vec!["first".into(), "second".into()],
        )])));
        let found = backend.lookup(name, &cancel).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0]
            .summary
            .starts_with("Homebrew ownership is ambiguous (first, second)"));
        assert_eq!(found[0].adopt_with, None);
        assert!(backend
            .lookup("/elsewhere/App.app", &cancel)
            .unwrap()
            .is_empty());
        let missing = f.0.join("Missing.app");
        assert!(backend
            .lookup(missing.to_str().unwrap(), &cancel)
            .unwrap()
            .is_empty());
        // A cancelled Homebrew check is never downgraded to "not found".
        let mut cancelled = f.backend(Err(EngineError::Cancelled));
        assert!(matches!(
            cancelled.lookup(name, &cancel),
            Err(EngineError::Cancelled)
        ));
        cancel.cancel();
        assert!(matches!(
            backend.lookup(name, &cancel),
            Err(EngineError::Cancelled)
        ));
    }

    #[test]
    fn fresh_details_read_only_the_selected_bundle_and_validate_discovery_boundaries() {
        use std::sync::Arc;
        struct CountedIo {
            reads: Arc<AtomicU64>,
            owners: Arc<AtomicU64>,
        }
        impl AppIo for CountedIo {
            fn plist(&self, path: &Path, cancel: &Cancellation) -> Result<Value, EngineError> {
                self.reads.fetch_add(1, Ordering::Relaxed);
                FakeIo(Ok(BTreeMap::new())).plist(path, cancel)
            }
            fn ownership(
                &self,
                _: &Cancellation,
            ) -> Result<BTreeMap<PathBuf, Vec<String>>, EngineError> {
                self.owners.fetch_add(1, Ordering::Relaxed);
                Ok(BTreeMap::new())
            }
        }
        let f = Fixture::new();
        let target = f.bundle("Applications/Utilities/Editor.app", "com.microsoft.VSCode");
        // A full rescan would read these unrelated bundles, too.
        for n in 0..20 {
            f.bundle(&format!("Applications/Other{n}.app"), "md.obsidian");
        }
        let cancel = Cancellation::default();
        let mut inventory = f.backend(Ok(BTreeMap::new()));
        inventory.roots = vec![(f.0.join("Applications"), Scope::System)];
        let package = inventory
            .installed(&cancel)
            .unwrap()
            .into_iter()
            .find(|p| p.id.name == target.to_string_lossy())
            .unwrap();
        let reads = Arc::new(AtomicU64::new(0));
        let owners = Arc::new(AtomicU64::new(0));
        let mut fresh = f.backend(Ok(BTreeMap::new()));
        fresh.roots = inventory.roots.clone();
        fresh.io = Box::new(CountedIo {
            reads: reads.clone(),
            owners: owners.clone(),
        });
        assert_eq!(
            fresh.details(&package.id, &cancel).unwrap(),
            inventory.details(&package.id, &cancel).unwrap()
        );
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        assert_eq!(owners.load(Ordering::Relaxed), 1);
        assert!(fresh.snapshot.is_none());

        let mut variants = Vec::new();
        let mut id = package.id.clone();
        id.backend = "homebrew-cask".into();
        variants.push(id);
        let mut id = package.id.clone();
        id.scope = Scope::User { uid: 999 };
        variants.push(id);
        let mut id = package.id.clone();
        id.architecture = "arm64".into();
        variants.push(id);
        let mut id = package.id.clone();
        id.reference = None;
        variants.push(id);
        let mut id = package.id.clone();
        id.remote = Some("remote".into());
        variants.push(id);
        let hidden = f.bundle("Applications/.hidden/App.app", "md.obsidian");
        let nested = f.bundle("Applications/Parent.app/Contents/Helper.app", "md.obsidian");
        let deep = f.bundle("Applications/a/b/c/d/e/App.app", "md.obsidian");
        let outside = f.bundle("Outside/App.app", "md.obsidian");
        symlink(f.0.join("Outside"), f.0.join("Applications/Linked")).unwrap();
        for path in [
            hidden,
            nested,
            deep,
            outside,
            f.0.join("Applications/Linked/App.app"),
            f.0.join("Applications/../Outside/App.app"),
            f.0.join("Applications/Missing.app"),
        ] {
            let mut id = package.id.clone();
            id.name = path.to_string_lossy().into();
            id.reference = Some(id.name.clone());
            variants.push(id);
        }
        for id in variants {
            assert!(
                matches!(fresh.details(&id, &cancel), Err(EngineError::NotFound)),
                "{id:?}"
            );
        }
        assert_eq!(
            reads.load(Ordering::Relaxed),
            1,
            "invalid identities must not read metadata"
        );
        assert_eq!(
            owners.load(Ordering::Relaxed),
            1,
            "invalid identities must not query Homebrew"
        );
        cancel.cancel();
        assert!(matches!(
            fresh.details(&package.id, &cancel),
            Err(EngineError::Cancelled)
        ));
        assert!(matches!(
            inventory.details(&package.id, &cancel),
            Err(EngineError::Cancelled)
        ));
    }

    #[test]
    fn inventory_preserves_copies_scopes_and_candidate_evidence_without_writes() {
        let f = Fixture::new();
        let system = f.bundle("system/Visual Studio Code.app", "com.microsoft.VSCode");
        let user = f.bundle("user/Renamed Editor.app", "com.microsoft.VSCode");
        f.bundle(
            "system/Visual Studio Code.app/Contents/Helper.app",
            "com.microsoft.helper",
        );
        symlink(&system, f.0.join("system/Alias.app")).unwrap();
        let mut backend = f.backend(Ok(BTreeMap::from([(
            fs::canonicalize(&system).unwrap(),
            vec!["visual-studio-code".into()],
        )])));
        backend.roots = vec![
            (f.0.join("system"), Scope::System),
            (f.0.join("user"), Scope::User { uid: 123 }),
        ];
        let cancel = Cancellation::default();
        let packages = backend.installed(&cancel).unwrap();
        assert_eq!(
            packages.len(),
            2,
            "aliases and nested helper apps are not extra installations"
        );
        let external = packages
            .iter()
            .find(|p| p.id.reference.as_deref() == user.to_str())
            .unwrap();
        assert_eq!(external.id.scope, Scope::User { uid: 123 });
        assert_eq!(external.installed_version.as_deref(), Some("1.2.3"));
        assert_eq!(external.update, UpdateAvailability::Unknown);
        assert!(external.candidate_version.is_none());
        assert!(external
            .summary
            .contains("Homebrew ownership unknown: no cask app link points here"));
        assert!(external
            .summary
            .contains("Available through Homebrew: visual-studio-code (candidate)"));
        let details = backend.details(&external.id, &cancel).unwrap();
        assert!(details.description.contains("com.microsoft.VSCode"));
        assert!(details.description.contains("have not been verified"));
        assert!(details.description.contains("Bundle build: 456"));
        let managed = packages
            .iter()
            .find(|p| {
                p.summary
                    .contains("Managed by Homebrew (visual-studio-code)")
            })
            .unwrap();
        assert!(!managed.summary.contains("Available through Homebrew"));
        // Homebrew already owns that copy: nothing to hand over.
        assert_eq!(managed.adopt_with, None);
        assert_eq!(external.adopt_with.as_deref(), Some("visual-studio-code"));
        assert_eq!(
            serde_json::to_value(external).unwrap()["adopt_with"],
            "visual-studio-code"
        );
        assert!(serde_json::to_value(managed)
            .unwrap()
            .get("adopt_with")
            .is_none());
        assert_eq!(backend.search("renamed editor", &cancel).unwrap().len(), 1);
        let mut wrong = external.id.clone();
        wrong.scope = Scope::System;
        assert!(matches!(
            backend.details(&wrong, &cancel),
            Err(EngineError::NotFound)
        ));
        for operation in [
            Operation::Install(external.id.clone()),
            Operation::Upgrade(external.id.clone()),
        ] {
            assert!(matches!(
                backend.execute(&operation, &cancel, &mut |_| panic!(
                    "this source never installs or updates"
                )),
                Err(EngineError::Unsupported { .. })
            ));
        }
        assert!(user.exists() && system.exists());
    }

    #[test]
    fn catalog_uses_identifiers_never_names_and_excludes_app_store_receipts() {
        let f = Fixture::new();
        f.bundle("Obsidian.app", "unrelated.publisher");
        f.bundle("Firefox Beta.app", "org.mozilla.firefoxbeta");
        f.bundle("Known Firefox.app", "org.mozilla.firefox");
        f.bundle("Renamed.app", "md.obsidian");
        let store = f.bundle("Store.app", "md.obsidian");
        fs::create_dir_all(store.join("Contents/_MASReceipt")).unwrap();
        fs::write(store.join("Contents/_MASReceipt/receipt"), "receipt").unwrap();
        let mut backend = f.backend(Ok(BTreeMap::new()));
        let cancel = Cancellation::default();
        let packages = backend.installed(&cancel).unwrap();
        for package in packages {
            let details = backend.details(&package.id, &cancel).unwrap();
            if package.id.name.ends_with("Known Firefox.app") {
                assert!(details.homepage.unwrap().ends_with("/firefox"));
                assert_eq!(package.adopt_with.as_deref(), Some("firefox"));
            } else if package.id.name.ends_with("Renamed.app") {
                assert!(details.homepage.unwrap().ends_with("/obsidian"));
                assert_eq!(package.adopt_with.as_deref(), Some("obsidian"));
            } else {
                // Unknown publishers, other editions and App Store copies
                // are never offered to Homebrew.
                assert_eq!(package.adopt_with, None);
                assert!(details.homepage.is_none());
                assert!(!details.description.contains("Available through Homebrew"));
            }
            if package.id.name.ends_with("Store.app") {
                assert!(details.description.contains("App Store receipt present"));
            }
        }
    }

    #[test]
    fn unreadable_metadata_and_failed_brew_checks_keep_apps_visible_as_unknown() {
        let f = Fixture::new();
        let broken = f.bundle("Broken.app", "md.obsidian");
        fs::write(broken.join("Contents/Info.plist"), "bad plist").unwrap();
        f.bundle("Healthy.app", "md.obsidian");
        let mut backend = f.backend(Err(invalid(ID, "brew unavailable")));
        let cancel = Cancellation::default();
        let apps = backend.installed(&cancel).unwrap();
        assert_eq!(apps.len(), 2);
        assert!(apps
            .iter()
            .all(|p| p.summary.contains("could not be checked") && p.adopt_with.is_none()));
        let broken = apps.iter().find(|p| p.display_name == "Broken").unwrap();
        assert_eq!(broken.installed_version.as_deref(), Some("unknown"));
        let details = backend.details(&broken.id, &cancel).unwrap();
        assert!(details.description.contains("metadata unavailable"));
        assert!(details.description.contains("brew unavailable"));
        assert!(details.homepage.is_none());
    }

    #[test]
    fn ownership_requires_installed_record_and_exact_artifact_symlink_target() {
        let f = Fixture::new();
        let app = f.bundle("Applications/Editor.app", "com.microsoft.VSCode");
        let other = f.bundle("Other/Editor.app", "com.microsoft.VSCode");
        let root = f.0.join("Caskroom");
        let stage = root.join("editor/1.0");
        fs::create_dir_all(&stage).unwrap();
        symlink(&app, stage.join("Editor.app")).unwrap();
        let records = owners(&root, vec![record("editor", "Editor.app", json!("1.0"))]).unwrap();
        assert_eq!(records[&fs::canonicalize(&app).unwrap()], vec!["editor"]);
        assert!(!records.contains_key(&fs::canonicalize(other).unwrap()));
        assert!(owners(&root, vec![]).unwrap().is_empty());
        assert!(
            owners(&root, vec![record("editor", "Editor.app", Value::Null)])
                .unwrap()
                .is_empty()
        );
        assert!(
            owners(&root, vec![record("editor", "Editor.app", json!("2.0"))])
                .unwrap()
                .is_empty()
        );
        fs::remove_file(stage.join("Editor.app")).unwrap();
        fs::create_dir(stage.join("Editor.app")).unwrap();
        assert!(
            owners(&root, vec![record("editor", "Editor.app", json!("1.0"))])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn renamed_casks_own_app_links_left_under_their_old_token() {
        let f = Fixture::new();
        let app = f.bundle("Applications/Viewer.app", "com.example.viewer");
        let copied = f.bundle("Other/Copied.app", "com.example.copied");
        let root = f.0.join("Caskroom");
        let old = root.join("old-viewer/7.1");
        fs::create_dir_all(&old).unwrap();
        symlink(&app, old.join("Viewer.app")).unwrap();
        // Only Homebrew's links count, not a bundle copied into the folder.
        fs::create_dir(old.join("Copied.app")).unwrap();
        let renamed = |old_tokens: Value| {
            json!({"token": "new-viewer", "full_token": "new-viewer", "installed": "7.1",
                "old_tokens": old_tokens, "artifacts": [{"pkg": ["Viewer.pkg"]}]})
        };
        let records = owners(&root, vec![renamed(json!(["old-viewer"]))]).unwrap();
        assert_eq!(
            records[&fs::canonicalize(&app).unwrap()],
            vec!["new-viewer"]
        );
        assert!(!records.contains_key(&fs::canonicalize(copied).unwrap()));
        assert!(owners(&root, vec![renamed(json!([]))]).unwrap().is_empty());
        // An old token with nothing left under it owns nothing.
        assert!(owners(&root, vec![renamed(json!(["never-installed"]))])
            .unwrap()
            .is_empty());
        assert!(owners(&root, vec![renamed(json!(["../old-viewer"]))])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn installer_package_casks_name_their_receipts() {
        let casks = json!({"casks": [
            {"full_token": "drive", "installed": "130.0", "artifacts": [
                {"pkg": ["Drive.pkg"]},
                {"uninstall": [{"pkgutil": ["com.example.drive", "com.example.drive\\..*"], "delete": null}]}]},
            {"full_token": "vpn", "installed": "1.0", "artifacts": [
                {"uninstall": [{"pkgutil": "com.example.vpn"}, {"pkgutil": 5}]}]},
            {"full_token": "gone", "installed": null, "artifacts": [
                {"uninstall": [{"pkgutil": "com.example.gone"}]}]},
            {"full_token": "plain", "installed": "2.0", "artifacts": [{"app": ["Plain.app"]}]}
        ]});
        assert_eq!(
            receipt_patterns(&serde_json::to_vec(&casks).unwrap()).unwrap(),
            vec![
                (
                    "drive".to_owned(),
                    vec![
                        "com.example.drive".to_owned(),
                        "com.example.drive\\..*".to_owned()
                    ]
                ),
                ("vpn".to_owned(), vec!["com.example.vpn".to_owned()]),
            ]
        );
        assert!(receipt_id("com.google.drivefs.arm64"));
        assert!(!receipt_id("com.example; rm"));
        assert!(!receipt_id(""));
    }

    #[test]
    fn receipts_locate_top_level_bundles_only() {
        // A package that installs into the bundle itself.
        assert_eq!(
            receipt_bundles(
                "package-id: com.example.vpn\nvolume: /\nlocation: Applications/VPN.app\n",
                "Contents\nContents/Frameworks/Sparkle.framework/Updater.app\n"
            ),
            vec![PathBuf::from("/Applications/VPN.app")]
        );
        // A package that installs several apps and their contents.
        assert_eq!(
            receipt_bundles(
                "volume: /\nlocation: Applications\n",
                "Docs.app\nDocs.app/Contents\nDocs.app/Contents/Helper.app\nSheets.app\nREADME\n"
            ),
            vec![
                PathBuf::from("/Applications/Docs.app"),
                PathBuf::from("/Applications/Sheets.app")
            ]
        );
        // A package installed at the volume root lists paths from there.
        assert_eq!(
            receipt_bundles(
                "volume: /\nlocation: /\n",
                "Applications\nApplications/Tool.app\nApplications/Tool.app/Contents\n"
            ),
            vec![PathBuf::from("/Applications/Tool.app")]
        );
        assert!(receipt_bundles("volume: /\nlocation: ../Applications\n", "Docs.app\n").is_empty());
        assert!(
            receipt_bundles("volume: relative\nlocation: Applications\n", "Docs.app\n").is_empty()
        );
        assert!(receipt_bundles("location: Applications\n", "Docs.app\n").is_empty());
        assert!(
            receipt_bundles("volume: /\nlocation: Applications\n", "../Escape.app\n").is_empty()
        );
    }

    #[test]
    fn ownership_rejects_malformed_and_escaping_records() {
        let f = Fixture::new();
        assert!(cask_owners(&f.0, br#"{}"#).is_err());
        for entry in [
            record("../editor", "Editor.app", json!("1")),
            record("editor", "../Editor.app", json!("1")),
            record("editor", "/Applications/Editor.app", json!("1")),
            record("editor", "Editor.app", json!("../../outside")),
        ] {
            assert!(owners(&f.0, vec![entry]).is_err());
        }
    }

    #[test]
    fn cancellation_is_not_downgraded_to_unknown_or_served_from_cache() {
        let f = Fixture::new();
        f.bundle("App.app", "md.obsidian");
        let mut backend = f.backend(Err(EngineError::Cancelled));
        assert!(matches!(
            backend.installed(&Cancellation::default()),
            Err(EngineError::Cancelled)
        ));
        let mut backend = f.backend(Ok(BTreeMap::new()));
        let cancel = Cancellation::default();
        backend.installed(&cancel).unwrap();
        cancel.cancel();
        assert!(matches!(
            backend.installed(&cancel),
            Err(EngineError::Cancelled)
        ));
        assert!(matches!(
            backend.detect(&cancel),
            Err(EngineError::Cancelled)
        ));
        assert_eq!(
            backend.capabilities(),
            [
                Capability::Search,
                Capability::Installed,
                Capability::Details,
                Capability::Remove
            ]
        );
    }

    #[test]
    fn nested_folders_are_scanned_but_directory_symlinks_are_not_followed() {
        let f = Fixture::new();
        f.bundle("Utilities/App.app", "md.obsidian");
        symlink(&f.0, f.0.join("loop")).unwrap();
        let mut backend = f.backend(Ok(BTreeMap::new()));
        backend.roots.push((f.0.join("missing"), Scope::System));
        assert_eq!(
            backend.installed(&Cancellation::default()).unwrap().len(),
            1
        );
    }

    #[test]
    fn inventory_rejects_excessive_directory_entries_instead_of_truncating() {
        let f = Fixture::new();
        for n in 0..=MAX_ENTRIES {
            fs::write(f.0.join(n.to_string()), "").unwrap();
        }
        let mut backend = f.backend(Ok(BTreeMap::new()));
        assert!(matches!(
            backend.installed(&Cancellation::default()),
            Err(EngineError::InvalidResponse { .. })
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_plutil_reads_xml_and_binary_bundles_without_launching_them() {
        let f = Fixture::new();
        let app = f.bundle("Native.app", "md.obsidian");
        let path = app.join("Contents/Info.plist");
        fs::write(&path, r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>md.obsidian</string></dict></plist>"#).unwrap();
        let native = NativeApps(Host::current());
        let cancel = Cancellation::default();
        assert_eq!(
            native.plist(&path, &cancel).unwrap()["CFBundleIdentifier"],
            "md.obsidian"
        );
        let mut cold = f.backend(Ok(BTreeMap::new()));
        cold.io = Box::new(NativeApps(Scripted::new(&f.0.join("Caskroom"), json!([]))));
        let id = PackageId {
            backend: ID.into(),
            name: app.to_string_lossy().into(),
            architecture: "unknown".into(),
            scope: Scope::System,
            remote: None,
            reference: Some(app.to_string_lossy().into()),
        };
        let details = cold.details(&id, &cancel).unwrap();
        assert!(cold.snapshot.is_none());
        assert!(details
            .description
            .contains("Bundle identifier: md.obsidian"));
        assert_eq!(cold.installed(&cancel).unwrap(), vec![details.package]);
        let result = native
            .0
            .read(
                Path::new("/usr/bin/plutil"),
                &[
                    "-convert".into(),
                    "binary1".into(),
                    "--".into(),
                    path.clone().into(),
                ],
                Limits::default(),
                &cancel,
            )
            .unwrap();
        assert_eq!(result.code, Some(0));
        assert_eq!(
            native.plist(&path, &cancel).unwrap()["CFBundleIdentifier"],
            "md.obsidian"
        );
    }

    /// Answers Homebrew and pkgutil from fixtures; plutil runs for real on
    /// the fixture's own files.
    #[cfg(target_os = "macos")]
    struct Scripted {
        host: Host,
        caskroom: Vec<u8>,
        casks: Value,
        /// `pkgutil` arguments joined by spaces → its output.
        pkgutil: BTreeMap<String, String>,
        calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }
    #[cfg(target_os = "macos")]
    impl Scripted {
        fn new(caskroom: &Path, casks: Value) -> Self {
            Self {
                host: Host::current(),
                caskroom: format!("{}\n", caskroom.display()).into_bytes(),
                casks,
                pkgutil: BTreeMap::new(),
                calls: Default::default(),
            }
        }
    }
    #[cfg(target_os = "macos")]
    fn completion(code: i32, stdout: impl Into<Vec<u8>>) -> Completion {
        Completion {
            code: Some(code),
            signal: None,
            stdout: stdout.into(),
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
        }
    }
    #[cfg(target_os = "macos")]
    impl Commands for Scripted {
        fn read(
            &self,
            executable: &Path,
            args: &[OsString],
            limits: Limits,
            cancel: &Cancellation,
        ) -> Result<Completion, ExecutionError> {
            if executable == Path::new("/usr/bin/plutil") {
                return self.host.read(executable, args, limits, cancel);
            }
            assert_eq!(executable, Path::new("/usr/sbin/pkgutil"));
            let line = args
                .iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            self.calls.lock().unwrap().push(format!("pkgutil {line}"));
            match line.as_str() {
                "--pkg-info com.example.broken" => Err(ExecutionError::TimedOut),
                "--files com.example.truncated" => {
                    let mut result = completion(0, "VPN.app\n");
                    result.truncated = true;
                    Ok(result)
                }
                _ => Ok(self
                    .pkgutil
                    .get(&line)
                    .map_or_else(|| completion(1, ""), |out| completion(0, out.as_str()))),
            }
        }
        fn brew(&self, args: &[OsString], _: &Cancellation) -> Result<Completion, ExecutionError> {
            let args: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
            self.calls
                .lock()
                .unwrap()
                .push(format!("brew {}", args.join(" ")));
            if args[0] == "--caskroom" {
                return Ok(completion(0, self.caskroom.clone()));
            }
            assert_eq!(args, ["info", "--json=v2", "--cask", "--installed"]);
            // `null` stands for a failed listing, a string for a cut-off one.
            if self.casks.is_null() {
                return Err(ExecutionError::TimedOut);
            }
            let mut result = completion(0, json!({"casks": self.casks}).to_string());
            result.truncated = self.casks.is_string();
            Ok(result)
        }
    }

    /// Homebrew's records come from `brew --caskroom` and the installed
    /// listing; a cask that ran an installer package is matched through
    /// the pkgutil receipts its uninstall step names.
    #[cfg(target_os = "macos")]
    #[test]
    fn native_ownership_reads_caskroom_links_and_installer_receipts() {
        let f = Fixture::new();
        let linked = f.bundle("Applications/Editor.app", "com.microsoft.VSCode");
        let vpn = f.bundle("Applications/VPN.app", "com.example.vpn");
        let docs = f.bundle("Applications/Docs.app", "com.example.docs");
        let root = f.0.join("Caskroom");
        fs::create_dir_all(root.join("editor/1.0")).unwrap();
        symlink(&linked, root.join("editor/1.0/Editor.app")).unwrap();
        let volume = f.0.to_string_lossy().into_owned();
        let mut commands = Scripted::new(
            &root,
            json!([
                {"token": "editor", "full_token": "editor", "installed": "1.0",
                 "artifacts": [{"app": ["Editor.app"]},
                               {"uninstall": [{"pkgutil": "com.example.editor"}]}]},
                {"token": "vpn", "full_token": "vpn", "installed": "2.0", "artifacts": [
                    {"uninstall": [{"pkgutil": ["com.example.vpn", "com.example.vpn.*"]}]}]},
                {"token": "suite", "full_token": "suite", "installed": "3.0", "artifacts": [
                    {"uninstall": [{"pkgutil": ["com.example.suite", "com.example.missing"]}]}]},
            ]),
        );
        commands.pkgutil = BTreeMap::from([
            (
                "--pkgs=com.example.editor".into(),
                "com.example.editor\n".into(),
            ),
            (
                "--pkg-info com.example.editor".into(),
                format!("volume: {volume}\nlocation: Applications/Editor.app\n"),
            ),
            ("--files com.example.editor".into(), "Contents\n".into()),
            // Both patterns find the same receipt; bad IDs are dropped.
            (
                "--pkgs=com.example.vpn".into(),
                "com.example.vpn\nnot valid; id\n".into(),
            ),
            (
                "--pkgs=com.example.vpn.*".into(),
                "com.example.vpn\ncom.example.broken\ncom.example.truncated\n".into(),
            ),
            (
                "--pkg-info com.example.vpn".into(),
                format!("volume: {volume}\nlocation: Applications/VPN.app\n"),
            ),
            ("--files com.example.vpn".into(), "Contents\n".into()),
            (
                "--pkg-info com.example.truncated".into(),
                format!("volume: {volume}\nlocation: Applications\n"),
            ),
            (
                "--pkgs=com.example.suite".into(),
                "com.example.suite\ncom.example.failed\n".into(),
            ),
            (
                "--pkg-info com.example.suite".into(),
                format!("volume: {volume}\nlocation: Applications\n"),
            ),
            // A listed bundle that is no longer there is skipped.
            (
                "--files com.example.suite".into(),
                "Docs.app\nDocs.app/Contents\nGone.app\n".into(),
            ),
            (
                "--pkg-info com.example.failed".into(),
                format!("volume: {volume}\nlocation: Applications\n"),
            ),
        ]);
        let calls = commands.calls.clone();
        let native = NativeApps(commands);
        let cancel = Cancellation::default();
        let owners = native.ownership(&cancel).unwrap();
        let canonical = |path: &Path| fs::canonicalize(path).unwrap();
        assert_eq!(
            owners,
            BTreeMap::from([
                (canonical(&linked), vec!["editor".to_owned()]),
                (canonical(&vpn), vec!["vpn".to_owned()]),
                (canonical(&docs), vec!["suite".to_owned()]),
            ])
        );
        let calls = calls.lock().unwrap().clone();
        assert_eq!(
            calls[..2],
            ["brew --caskroom", "brew info --json=v2 --cask --installed"]
        );
        // Each receipt is read once, however many patterns found it.
        assert_eq!(
            calls
                .iter()
                .filter(|call| *call == "pkgutil --pkg-info com.example.vpn")
                .count(),
            1
        );
        assert!(!calls.iter().any(|call| call.contains("not valid")));
        // A failed pattern lookup is no match, not an error.
        assert!(calls.contains(&"pkgutil --pkgs=com.example.missing".to_owned()));
        cancel.cancel();
        assert!(matches!(
            native.ownership(&cancel),
            Err(EngineError::Cancelled)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_ownership_rejects_a_bad_caskroom_and_brew_failures() {
        let cancel = Cancellation::default();
        let relative = Scripted::new(Path::new("Caskroom"), json!([]));
        assert!(NativeApps(relative)
            .ownership(&cancel)
            .unwrap_err()
            .to_string()
            .contains("relative Caskroom"));
        let mut garbled = Scripted::new(Path::new("/opt"), json!([]));
        garbled.caskroom = b"/opt/\xff\n".to_vec();
        assert!(NativeApps(garbled).ownership(&cancel).is_err());
        assert!(matches!(
            NativeApps(Scripted::new(Path::new("/opt"), Value::Null)).ownership(&cancel),
            Err(EngineError::Execution(ExecutionError::TimedOut))
        ));
        assert!(
            NativeApps(Scripted::new(Path::new("/opt"), json!("cut off")))
                .ownership(&cancel)
                .is_err()
        );
        // Without Homebrew on PATH nothing runs, and the check fails.
        let empty = Fixture::new();
        let host = Host::new(
            crate::host::Runtime::Native,
            BTreeMap::from([("PATH".into(), empty.0.clone().into_os_string())]),
        );
        assert!(matches!(
            NativeApps(host).ownership(&cancel),
            Err(EngineError::Execution(ExecutionError::Disabled(_)))
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_plists_must_be_small_regular_files() {
        let f = Fixture::new();
        let native = NativeApps(Host::current());
        let cancel = Cancellation::default();
        assert!(native.plist(&f.0.join("missing.plist"), &cancel).is_err());
        assert!(native
            .plist(&f.0, &cancel)
            .unwrap_err()
            .to_string()
            .contains("bounded regular file"));
        let big = f.0.join("big.plist");
        fs::write(&big, vec![b' '; 1024 * 1024 + 1]).unwrap();
        assert!(native
            .plist(&big, &cancel)
            .unwrap_err()
            .to_string()
            .contains("bounded regular file"));
        let bad = f.0.join("bad.plist");
        fs::write(&bad, "not a plist").unwrap();
        assert!(native.plist(&bad, &cancel).is_err());
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(matches!(
            native.plist(&bad, &cancelled),
            Err(EngineError::Cancelled)
        ));
    }

    /// Off macOS nothing runs plutil or asks Homebrew for casks: a bundle
    /// found anyway is listed without metadata or an owner.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn other_systems_read_no_bundles() {
        let f = Fixture::new();
        f.bundle("Native.app", "md.obsidian");
        let mut backend = f.backend(Ok(BTreeMap::new()));
        backend.io = Box::new(NativeApps);
        let cancel = Cancellation::default();
        assert!(matches!(
            backend.io.ownership(&cancel),
            Err(EngineError::Unavailable { .. })
        ));
        let rows = backend.installed(&cancel).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].installed_version.as_deref(), Some("unknown"));
        assert!(rows[0]
            .summary
            .starts_with("Homebrew ownership could not be checked"));
    }

    fn app_id(path: &Path) -> PackageId {
        let name: String = path.to_string_lossy().into();
        PackageId {
            backend: ID.into(),
            name: name.clone(),
            architecture: "unknown".into(),
            scope: Scope::System,
            remote: None,
            reference: Some(name),
        }
    }
    fn removal(path: &Path) -> Operation {
        Operation::Remove(app_id(path))
    }
    fn remove(
        backend: &mut MacApps,
        path: &Path,
    ) -> (Result<OperationOutcome, EngineError>, Vec<String>) {
        let mut messages = vec![];
        let result = backend.execute(&removal(path), &Cancellation::default(), &mut |p| {
            if let Progress::Message(text) = p {
                messages.push(text);
            }
        });
        (result, messages)
    }
    fn uid() -> u32 {
        rustix::process::getuid().as_raw()
    }

    #[test]
    fn removal_moves_an_app_you_own_to_the_trash_and_forgets_the_listing() {
        let f = Fixture::new();
        let app = f.bundle("Editor.app", "com.example.editor");
        let mut backend = f.backend(Ok(BTreeMap::new()));
        let mover = f.mover();
        backend.transport = Box::new(mover.clone());
        backend.installed(&Cancellation::default()).unwrap();
        let (result, messages) = remove(&mut backend, &app);
        result.unwrap();
        assert!(!app.exists());
        assert!(f
            .0
            .join("home/.Trash/Editor.app/Contents/Info.plist")
            .is_file());
        assert_eq!(messages, ["Moving Fixture App to the Trash"]);
        assert_eq!(
            mover.calls(),
            [
                "/bin/ps -axww -o comm=".to_owned(),
                format!("/usr/bin/trash -s {}", app.display())
            ]
        );
        // The listing is read again next time, and the app is gone from it.
        assert!(backend.snapshot.is_none());
        assert!(matches!(
            remove(&mut backend, &app).0,
            Err(EngineError::NotFound)
        ));
        // This source still never installs or updates.
        assert!(backend.capabilities().contains(&Capability::Remove));
    }

    #[test]
    fn apps_owned_by_the_system_move_after_the_password_prompt() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let first = f.bundle("Store.app", "com.example.store");
        let second = f.bundle("Utilities/Store.app", "com.example.store");
        let mut backend = f.backend(Ok(BTreeMap::new()));
        // Someone else owns these bundles, as root owns App Store apps.
        backend.uid = uid() + 1;
        let mover = f.mover();
        backend.transport = Box::new(mover.clone());
        let trash = f.0.join("home/.Trash");
        fs::create_dir_all(f.0.join("home")).unwrap();
        let (result, messages) = remove(&mut backend, &first);
        result.unwrap();
        assert!(messages[0].contains("macOS asks for an administrator password"));
        // A missing Trash is created for its owner only.
        assert_eq!(
            fs::metadata(&trash).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(trash.join("Store.app").is_dir());
        // A same-named app already in the Trash is kept; this one is numbered.
        remove(&mut backend, &second).0.unwrap();
        assert!(trash.join("Store 2.app").is_dir());
        assert_eq!(
            mover.calls()[1],
            format!(
                "mv -n -- {} {}",
                first.display(),
                trash.join("Store.app").display()
            )
        );
    }

    #[test]
    fn a_refused_or_failed_password_prompt_leaves_the_app_in_place() {
        let f = Fixture::new();
        let app = f.bundle("Store.app", "com.example.store");
        fs::create_dir_all(f.0.join("home/.Trash")).unwrap();
        for (privileged, expected) in [
            (Privileged::Cancel, "authorization cancelled"),
            (Privileged::Denied, "authorization denied"),
            (Privileged::Fail, "the package manager"),
            (Privileged::Ignore, "is still at"),
        ] {
            let mut backend = f.backend(Ok(BTreeMap::new()));
            backend.uid = uid() + 1;
            let mut mover = f.mover();
            mover.privileged = privileged;
            backend.transport = Box::new(mover);
            let error = remove(&mut backend, &app).0.unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
            assert!(app.is_dir());
        }
        // Cancel in the password dialog is its own outcome, not a failure.
        let mut backend = f.backend(Ok(BTreeMap::new()));
        backend.uid = uid() + 1;
        let mut mover = f.mover();
        mover.privileged = Privileged::Cancel;
        backend.transport = Box::new(mover);
        assert!(matches!(
            remove(&mut backend, &app).0,
            Err(EngineError::Execution(
                ExecutionError::AuthorizationCancelled
            ))
        ));
    }

    #[test]
    fn the_trash_must_be_a_folder_the_person_owns() {
        let f = Fixture::new();
        let app = f.bundle("Store.app", "com.example.store");
        let attempt = |mover: Mover| {
            let mut backend = f.backend(Ok(BTreeMap::new()));
            backend.uid = uid() + 1;
            backend.transport = Box::new(mover.clone());
            let error = remove(&mut backend, &app).0.unwrap_err().to_string();
            // Nothing was moved.
            assert!(!mover.calls().iter().any(|call| call.starts_with("mv")));
            error
        };
        let mut homeless = f.mover();
        homeless.home = None;
        assert!(attempt(homeless).contains("can't find your home folder"));
        let mut relative = f.mover();
        relative.home = Some("home".into());
        assert!(attempt(relative).contains("can't find your home folder"));
        // An alias would send the app somewhere else.
        fs::create_dir_all(f.0.join("home")).unwrap();
        fs::create_dir_all(f.0.join("elsewhere")).unwrap();
        symlink(f.0.join("elsewhere"), f.0.join("home/.Trash")).unwrap();
        assert!(attempt(f.mover()).contains("isn't a folder you own"));
        fs::remove_file(f.0.join("home/.Trash")).unwrap();
        // Every name is taken.
        fs::create_dir(f.0.join("home/.Trash")).unwrap();
        fs::create_dir(f.0.join("home/.Trash/Store.app")).unwrap();
        for n in 2..=TRASH_NAMES {
            fs::create_dir(f.0.join(format!("home/.Trash/Store {n}.app"))).unwrap();
        }
        assert!(attempt(f.mover()).contains("already holds 100 apps named Store"));
        assert!(app.is_dir());
    }

    #[test]
    fn homebrew_uninstalls_the_casks_it_manages() {
        let f = Fixture::new();
        let owned = f.bundle("Owned.app", "com.example.owned");
        let kept = f.bundle("Kept.app", "com.example.kept");
        let shared = f.bundle("Shared.app", "com.example.shared");
        let canonical = |path: &Path| fs::canonicalize(path).unwrap();
        let records = BTreeMap::from([
            (canonical(&owned), vec!["tap/tools/owned".to_owned()]),
            (canonical(&kept), vec!["kept".to_owned()]),
            (
                canonical(&shared),
                vec!["first".to_owned(), "second".to_owned()],
            ),
        ]);
        let backend = || f.backend(Ok(records.clone()));
        let mut apps = backend();
        let mut mover = f.mover();
        mover.brew_removes = Some(owned.clone());
        apps.transport = Box::new(mover.clone());
        let (result, messages) = remove(&mut apps, &owned);
        result.unwrap();
        assert!(!owned.exists());
        assert_eq!(
            messages,
            ["Uninstalling Fixture App with Homebrew (tap/tools/owned)"]
        );
        assert_eq!(
            mover.calls().last().unwrap(),
            "brew uninstall --cask --force -- tap/tools/owned"
        );
        // Homebrew that leaves the app behind is reported.
        let mut apps = backend();
        let error = remove(&mut apps, &kept).0.unwrap_err();
        assert!(error
            .to_string()
            .contains("Homebrew finished, but Fixture App is still at"));
        // Two casks claim it: PkgDeck can't pick one.
        let mut apps = backend();
        let error = remove(&mut apps, &shared).0.unwrap_err();
        assert!(error
            .to_string()
            .contains("more than one cask for Fixture App (first, second)"));
        // A failing Homebrew is reported as it is.
        let mut apps = backend();
        let mut mover = f.mover();
        mover.brew_error = Some("Error: It seems the App source is not there.\n");
        apps.transport = Box::new(mover);
        assert!(matches!(
            remove(&mut apps, &kept).0,
            Err(EngineError::Execution(ExecutionError::Failed(_)))
        ));
        assert!(kept.is_dir() && shared.is_dir());
    }

    #[test]
    fn homebrew_that_cannot_be_checked_blocks_removal_unless_it_is_missing() {
        let f = Fixture::new();
        let app = f.bundle("App.app", "com.example.app");
        let mut broken = f.backend(Err(invalid(ID, "brew broke")));
        let error = remove(&mut broken, &app).0.unwrap_err();
        assert!(error
            .to_string()
            .contains("couldn't check whether Homebrew manages Fixture App"));
        let mut cancelled = f.backend(Err(EngineError::Cancelled));
        assert!(matches!(
            remove(&mut cancelled, &app).0,
            Err(EngineError::Cancelled)
        ));
        assert!(app.is_dir());
        // No Homebrew at all: nothing else can manage the app.
        let mut plain = f.backend(Err(
            ExecutionError::Disabled("Homebrew not found".into()).into()
        ));
        remove(&mut plain, &app).0.unwrap();
        assert!(!app.exists());
    }

    #[test]
    fn removal_refuses_a_bundle_that_changed_since_it_was_listed() {
        let f = Fixture::new();
        let app = f.bundle("App.app", "com.example.app");
        let rewrite = |id: &str| {
            fs::write(
                app.join("Contents/Info.plist"),
                json!({"CFBundleIdentifier": id}).to_string(),
            )
            .unwrap()
        };
        let cancel = Cancellation::default();
        // Listed by a scan, then replaced by another app at the same path.
        let mut scanned = f.backend(Ok(BTreeMap::new()));
        let mover = f.mover();
        scanned.transport = Box::new(mover.clone());
        scanned.installed(&cancel).unwrap();
        rewrite("com.example.other");
        let error = remove(&mut scanned, &app).0.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("changed since PkgDeck listed it"),
            "{error}"
        );
        // Picked by its exact path, then changed.
        let mut picked = f.backend(Ok(BTreeMap::new()));
        picked.transport = Box::new(mover.clone());
        picked.lookup(app.to_str().unwrap(), &cancel).unwrap();
        rewrite("com.example.third");
        assert!(remove(&mut picked, &app).0.is_err());
        assert!(app.is_dir());
        assert!(mover.calls().is_empty(), "nothing ran");
    }

    #[test]
    fn removal_refuses_open_apps_aliases_root_and_cancellation() {
        let f = Fixture::new();
        let app = f.bundle("App.app", "com.example.app");
        // Open: its executable is running.
        let mut open = f.backend(Ok(BTreeMap::new()));
        let mut mover = f.mover();
        mover.ps = format!(
            "/usr/libexec/launchd\n{}/Contents/MacOS/fixture\n",
            app.display()
        );
        open.transport = Box::new(mover);
        let error = remove(&mut open, &app).0.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Fixture App is open. Quit it, then remove it again."
        );
        let mut other = f.backend(Ok(BTreeMap::new()));
        let alias = f.0.join("Alias.app");
        symlink(&app, &alias).unwrap();
        let error = remove(&mut other, &alias).0.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("is an alias, not the app itself"),
            "{error}"
        );
        // PkgDeck running as root has no person's Trash to use.
        let mut root = f.backend(Ok(BTreeMap::new()));
        root.uid = 0;
        let error = remove(&mut root, &app).0.unwrap_err();
        assert_eq!(error.to_string(), crate::host::ROOT_REFUSAL);
        // Cancelling before or while checking moves nothing.
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            other.execute(&removal(&app), &cancel, &mut drop::<Progress>),
            Err(EngineError::Cancelled)
        ));
        let mut cancelling = f.backend(Ok(BTreeMap::new()));
        let mut mover = f.mover();
        mover.cancel_on_ps = true;
        cancelling.transport = Box::new(mover);
        assert!(matches!(
            remove(&mut cancelling, &app).0,
            Err(EngineError::Cancelled)
        ));
        // A failed check for open apps stops the removal, too.
        let mut unchecked = f.backend(Ok(BTreeMap::new()));
        let mut mover = f.mover();
        mover.ps_fails = true;
        unchecked.transport = Box::new(mover.clone());
        assert!(matches!(
            remove(&mut unchecked, &app).0,
            Err(EngineError::Execution(ExecutionError::Failed(_)))
        ));
        assert_eq!(mover.calls().len(), 1, "only ps ran");
        assert!(app.is_dir());
        // Another app whose name starts the same is not this one.
        let mut mover = f.mover();
        mover.ps = format!(
            "{} 2.app/Contents/MacOS/fixture\n",
            app.with_extension("").display()
        );
        other.transport = Box::new(mover);
        remove(&mut other, &app).0.unwrap();
        assert!(!app.exists());
    }

    #[test]
    fn apps_that_come_with_macos_are_never_removed() {
        let f = Fixture::new();
        assert!(part_of_macos(Path::new(
            "/System/Applications/PkgDeck Missing.app"
        )));
        let data = Path::new("/System/Volumes/Data/Applications/PkgDeck Missing.app");
        assert_eq!(
            visible(data),
            PathBuf::from("/Applications/PkgDeck Missing.app")
        );
        assert!(!part_of_macos(data));
        assert!(!part_of_macos(&f.0));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn protected_macos_apps_are_listed_but_never_removed() {
        let f = Fixture::new();
        // System Integrity Protection's restricted flag.
        assert!(part_of_macos(Path::new("/bin")));
        // Like Safari: an alias in the folder, the app on the system volume.
        let finder = Path::new("/System/Library/CoreServices/Finder.app");
        let alias = f.0.join("Finder.app");
        symlink(finder, &alias).unwrap();
        let mut backend = f.backend(Ok(BTreeMap::new()));
        let listed = backend
            .details(&app_id(&alias), &Cancellation::default())
            .unwrap();
        assert!(listed
            .description
            .contains("Part of macOS, so PkgDeck won't remove it."));
        let error = remove(&mut backend, &alias).0.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Finder is part of macOS, so PkgDeck won't remove it."
        );
        assert!(finder.is_dir());
    }

    #[test]
    fn only_the_native_transport_runs_macos_tools() {
        struct Nothing;
        impl Transport for Nothing {}
        let cancel = Cancellation::default();
        assert!(matches!(
            Nothing.macos_tool(Path::new("/bin/ps"), &[], &cancel, false),
            Err(ExecutionError::Disabled(reason)) if reason == "/bin/ps is unavailable"
        ));
        let native = NativeTransport {
            host: Host::current(),
            authorization: crate::host::Authorization::SudoNonInteractive,
        };
        let pid = std::process::id().to_string();
        let ps = native
            .macos_tool(
                Path::new("/bin/ps"),
                &["-p".into(), pid.clone().into(), "-o".into(), "pid=".into()],
                &cancel,
                false,
            )
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&ps.stdout).trim(), pid);
    }
}
