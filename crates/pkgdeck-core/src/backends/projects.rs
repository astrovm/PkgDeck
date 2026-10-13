//! Old projects: what builds and installs made in projects nobody changed
//! for a while (Rust `target`, Gradle `build`, `node_modules` and the
//! like), and Git worktrees that are done with. Projects are found under the
//! folders in [`ProjectSettings`], the home folder unless changed.
use super::*;
use crate::{disk, host::Host};
use dev_caches::{not_while_building, shown};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::MetadataExt,
    path::Path,
    time::{Duration, SystemTime},
};

const CAPABILITIES: &[Capability] = &[Capability::Clean];
/// How deep under a folder projects are looked for.
const MAX_DEPTH: usize = 12;
/// Files that make a folder a project even without Git.
const MARKERS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "pom.xml",
    "CMakeLists.txt",
    "Package.swift",
    "pubspec.yaml",
    "build.zig",
    "go.mod",
    "pyproject.toml",
];
const GRADLE: &[&str] = &[
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
];

/// Where to look for projects, and how long one stays untouched before its
/// build files count as old.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeProtection {
    #[default]
    AnyLocalFiles,
    UncommittedChanges,
    EditedTrackedFiles,
}

impl WorktreeProtection {
    pub const ALL: [Self; 3] = [
        Self::AnyLocalFiles,
        Self::UncommittedChanges,
        Self::EditedTrackedFiles,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::AnyLocalFiles => "Always keep",
            Self::UncommittedChanges => "Keep if files are new or edited",
            Self::EditedTrackedFiles => "Keep only if files are edited",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::AnyLocalFiles => "Safest. Keeps everything",
            Self::UncommittedChanges => "Loses ignored files, like .env",
            Self::EditedTrackedFiles => "Loses new and ignored files, like .env",
        }
    }
    fn status_args(self) -> &'static [&'static str] {
        match self {
            Self::AnyLocalFiles => &[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--ignored",
            ],
            Self::UncommittedChanges => &["status", "--porcelain", "--untracked-files=all"],
            Self::EditedTrackedFiles => &["status", "--porcelain", "--untracked-files=no"],
        }
    }
}

/// Where to look for old projects and which worktrees to keep.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ProjectSettings {
    pub folders: Vec<PathBuf>,
    pub days: u32,
    pub worktree_protection: WorktreeProtection,
}

impl ProjectSettings {
    /// The home folder, and a week.
    pub fn defaults(host: &Host) -> Self {
        Self {
            folders: home(host).into_iter().collect(),
            days: 7,
            worktree_protection: WorktreeProtection::default(),
        }
    }
    /// `~/.config/pkgdeck/clean.json`, or under `XDG_CONFIG_HOME`.
    pub fn path(host: &Host) -> Option<PathBuf> {
        let config = host
            .var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| home(host).map(|home| home.join(".config")))?;
        Some(config.join("pkgdeck/clean.json"))
    }
    /// The saved settings, or the defaults when there are none or they
    /// can't be read. Relative folders are dropped.
    pub fn load(host: &Host) -> Self {
        let saved = Self::path(host)
            .and_then(|path| fs::read(path).ok())
            .and_then(|text| serde_json::from_slice::<Self>(&text).ok());
        match saved {
            Some(mut settings) => {
                settings.folders.retain(|folder| folder.is_absolute());
                settings
            }
            None => Self::defaults(host),
        }
    }
    pub fn save(&self, host: &Host) -> std::io::Result<()> {
        let path = Self::path(host)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no home folder"))?;
        fs::create_dir_all(path.parent().unwrap_or(Path::new("/")))?;
        let text = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let temporary = path.with_extension("json.new");
        fs::write(&temporary, text)?;
        fs::rename(temporary, path)
    }
}
impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            folders: vec![],
            days: 7,
            worktree_protection: WorktreeProtection::default(),
        }
    }
}

fn home(host: &Host) -> Option<PathBuf> {
    host.var("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

pub struct Projects {
    host: Host,
    settings: Option<ProjectSettings>,
}

/// A project found on disk: its folder, the build folders in it, and when
/// one of its own files last changed.
#[derive(Debug)]
struct Project {
    root: PathBuf,
    /// Its own repository, with a `.git` folder (a worktree has a file).
    repository: bool,
    folders: Vec<PathBuf>,
    newest: SystemTime,
}

/// A linked worktree that can go: merged and unchanged, or already gone.
#[derive(Debug, PartialEq)]
struct Worktree {
    path: PathBuf,
    /// The folder is gone; `git worktree prune` forgets it.
    missing: bool,
    branch: Option<String>,
}

/// Whether files named `names` make their folder a project.
fn marked(names: &BTreeSet<String>) -> bool {
    MARKERS.iter().any(|marker| names.contains(*marker)) || names.iter().any(|name| dotnet(name))
}
fn dotnet(name: &str) -> bool {
    [".csproj", ".fsproj", ".vbproj"]
        .iter()
        .any(|end| name.ends_with(end))
}

/// Whether the folder `name`, next to `siblings`, is something a build or an
/// install made and makes again.
fn build_folder(name: &str, siblings: &BTreeSet<String>) -> bool {
    let has = |file: &str| siblings.contains(file);
    let gradle = GRADLE.iter().any(|file| has(file));
    match name {
        "target" => has("Cargo.toml") || has("pom.xml"),
        "node_modules" | ".next" | ".nuxt" | ".svelte-kit" | ".turbo" | ".parcel-cache"
        | ".angular" => has("package.json"),
        "build" => gradle || has("pubspec.yaml"),
        ".gradle" | ".cxx" => gradle,
        ".dart_tool" => has("pubspec.yaml"),
        ".build" => has("Package.swift"),
        "zig-cache" | ".zig-cache" | "zig-out" => has("build.zig"),
        "obj" | "bin" => siblings.iter().any(|file| dotnet(file)),
        _ => false,
    }
}

/// Finds projects under one folder.
struct Walk<'a> {
    cancel: &'a Cancellation,
    /// The home folder, whose `Library` is never looked into.
    home: Option<PathBuf>,
    device: u64,
    projects: Vec<Project>,
}

impl Walk<'_> {
    fn visit(&mut self, dir: &Path, depth: usize, owner: Option<usize>) {
        if self.cancel.requested() {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let entries: Vec<_> = entries
            .flatten()
            .filter_map(|entry| {
                Some((
                    entry.file_name().into_string().ok()?,
                    entry.file_type().ok()?,
                ))
            })
            .collect();
        let names: BTreeSet<String> = entries.iter().map(|(name, _)| name.clone()).collect();
        // Folders that tools mark as theirs: Cargo's, CMake's.
        if let Some(owner) = owner.filter(|_| depth > 0) {
            if names.contains("CACHEDIR.TAG") || names.contains("CMakeCache.txt") {
                self.projects[owner].folders.push(dir.to_owned());
                return;
            }
        }
        let owner = if names.contains(".git") || (owner.is_none() && marked(&names)) {
            self.projects.push(Project {
                root: dir.to_owned(),
                repository: dir.join(".git").is_dir(),
                folders: vec![],
                newest: SystemTime::UNIX_EPOCH,
            });
            Some(self.projects.len() - 1)
        } else {
            owner
        };
        for (name, kind) in entries {
            let path = dir.join(&name);
            if kind.is_file() {
                if let Some(owner) = owner {
                    if let Ok(changed) = fs::symlink_metadata(&path).and_then(|m| m.modified()) {
                        let project = &mut self.projects[owner];
                        project.newest = project.newest.max(changed);
                    }
                }
                continue;
            }
            if !kind.is_dir() {
                continue;
            }
            if let Some(owner) = owner.filter(|_| build_folder(&name, &names)) {
                self.projects[owner].folders.push(path);
                continue;
            }
            let library = depth == 0 && name == "Library" && self.home.as_deref() == Some(dir);
            if name.starts_with('.') || library || depth >= MAX_DEPTH {
                continue;
            }
            // Other disks, such as network shares, stay out of it.
            if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.dev() == self.device) {
                self.visit(&path, depth + 1, owner);
            }
        }
    }
}

/// Projects under `folder`, and when each last changed.
fn projects_in(folder: &Path, home: Option<PathBuf>, cancel: &Cancellation) -> Vec<Project> {
    let Ok(metadata) = fs::metadata(folder) else {
        return vec![];
    };
    let mut walk = Walk {
        cancel,
        home,
        device: metadata.dev(),
        projects: vec![],
    };
    walk.visit(folder, 0, None);
    // The same order on every look, so a plan reads the same.
    for project in &mut walk.projects {
        project.folders.sort();
    }
    walk.projects
}

/// How long ago `newest` was, in words that stay the same for a while.
fn age(newest: SystemTime, now: SystemTime) -> String {
    let days = now.duration_since(newest).unwrap_or_default().as_secs() / 86_400;
    let (count, unit) = match days {
        0..=6 => return "Unchanged for a few days".into(),
        7..=59 => (days / 7, "week"),
        60..=729 => (days / 30, "month"),
        _ => (days / 365, "year"),
    };
    let plural = if count == 1 { "" } else { "s" };
    let count = if count == 1 {
        "a".to_owned()
    } else {
        count.to_string()
    };
    format!("Unchanged for {count} {unit}{plural}")
}

impl Projects {
    pub fn new(host: Host) -> Self {
        Self {
            host,
            settings: None,
        }
    }
    /// Look with these settings rather than the saved ones.
    #[cfg(test)]
    pub fn with_settings(mut self, settings: ProjectSettings) -> Self {
        self.settings = Some(settings);
        self
    }
    fn settings(&self) -> ProjectSettings {
        self.settings
            .clone()
            .unwrap_or_else(|| ProjectSettings::load(&self.host))
    }
    fn cutoff(&self) -> SystemTime {
        SystemTime::now() - Duration::from_secs(u64::from(self.settings().days) * 86_400)
    }
    /// Whether `path` is in one of the folders projects are looked for in.
    fn looked_at(&self, path: &Path) -> bool {
        path.is_absolute()
            && !path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
            && self
                .settings()
                .folders
                .iter()
                .any(|folder| path.starts_with(folder))
    }

    /// The task for one project's build folders, when it is old enough.
    fn build_item(
        &self,
        project: &Project,
        cutoff: SystemTime,
        cancel: &Cancellation,
    ) -> Option<CleanupItem> {
        if project.folders.is_empty() || project.newest > cutoff {
            return None;
        }
        let bytes = project
            .folders
            .iter()
            .map(|folder| disk::usage(folder, cancel))
            .sum();
        let name = project
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| shown(&self.host, &project.root));
        Some(CleanupItem {
            id: CleanupId {
                backend: "projects".into(),
                key: format!("build:{}", project.root.display()),
            },
            kind: CleanupKind::BuildFiles,
            title: format!("{name} build files"),
            summary: age(project.newest, SystemTime::now()),
            preview: format!(
                "Deletes these folders. A build or an install makes them again.\n{}",
                project
                    .folders
                    .iter()
                    .map(|folder| shown(&self.host, folder))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            bytes: Some(bytes),
        })
    }

    fn git(&self, repository: &Path, args: &[&str], cancel: &Cancellation) -> Option<String> {
        let git = self.host.resolve("git").ok().flatten()?;
        let mut all: Vec<OsString> = vec!["-C".into(), repository.into()];
        all.extend(args.iter().map(OsString::from));
        let done = self.host.read(&git, &all, Limits::default(), cancel).ok()?;
        (done.code == Some(0)).then(|| String::from_utf8_lossy(&done.stdout).into_owned())
    }

    /// The branch finished work lands on: the remote's default, or a local
    /// `main` or `master`.
    fn default_branch(&self, repository: &Path, cancel: &Cancellation) -> Option<String> {
        if let Some(remote) = self.git(
            repository,
            &[
                "symbolic-ref",
                "--quiet",
                "--short",
                "refs/remotes/origin/HEAD",
            ],
            cancel,
        ) {
            return Some(remote.trim().to_owned());
        }
        ["main", "master"].into_iter().find_map(|branch| {
            self.git(
                repository,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ],
                cancel,
            )
            .map(|_| branch.to_owned())
        })
    }

    /// Linked worktrees of `repository` that can go: their folder is gone,
    /// or they have nothing uncommitted, nobody changed them since
    /// `cutoff`, and their branch is merged or its remote branch was
    /// deleted. Locked ones always stay.
    fn worktrees(
        &self,
        repository: &Path,
        cutoff: SystemTime,
        cancel: &Cancellation,
    ) -> Vec<Worktree> {
        let Some(list) = self.git(repository, &["worktree", "list", "--porcelain"], cancel) else {
            return vec![];
        };
        let default = self.default_branch(repository, cancel);
        let mut found = vec![];
        // The first block is the main worktree.
        for block in list.split("\n\n").skip(1) {
            let mut path = None;
            let mut head = None;
            let mut branch = None;
            let (mut locked, mut prunable) = (false, false);
            for line in block.lines() {
                let (key, value) = line.split_once(' ').unwrap_or((line, ""));
                match key {
                    "worktree" => path = Some(PathBuf::from(value)),
                    "HEAD" => head = Some(value.to_owned()),
                    "branch" => branch = value.strip_prefix("refs/heads/").map(str::to_owned),
                    "locked" => locked = true,
                    "prunable" => prunable = true,
                    _ => {}
                }
            }
            let Some(path) = path.filter(|path| path.is_absolute()) else {
                continue;
            };
            if locked {
                continue;
            }
            if prunable || !path.exists() {
                found.push(Worktree {
                    path,
                    missing: true,
                    branch,
                });
                continue;
            }
            // Only a commit ID reaches Git's arguments.
            let head = head.filter(|head| head.bytes().all(|b| b.is_ascii_hexdigit()));
            let merged = head
                .as_deref()
                .zip(default.as_deref())
                .is_some_and(|(head, default)| {
                    self.git(
                        repository,
                        &["merge-base", "--is-ancestor", head, default],
                        cancel,
                    )
                    .is_some()
                });
            let gone = branch.as_deref().is_some_and(|branch| {
                self.git(
                    repository,
                    &[
                        "for-each-ref",
                        "--format=%(upstream:track)",
                        &format!("refs/heads/{branch}"),
                    ],
                    cancel,
                )
                .is_some_and(|track| track.trim() == "[gone]")
            });
            if !(merged || gone) {
                continue;
            }
            let newest = projects_in(&path, None, cancel)
                .iter()
                .map(|project| project.newest)
                .max()
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let clean = self
                .git(
                    &path,
                    self.settings().worktree_protection.status_args(),
                    cancel,
                )
                .is_some_and(|status| status.trim().is_empty());
            if newest <= cutoff && clean {
                found.push(Worktree {
                    path,
                    missing: false,
                    branch,
                });
            }
        }
        found.sort_by(|a, b| a.path.cmp(&b.path));
        found
    }

    fn worktree_item(
        &self,
        repository: &Path,
        worktrees: &[Worktree],
        cancel: &Cancellation,
    ) -> Option<CleanupItem> {
        if worktrees.is_empty() {
            return None;
        }
        let name = repository
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| shown(&self.host, repository));
        let lines: Vec<_> = worktrees
            .iter()
            .map(|worktree| {
                let path = shown(&self.host, &worktree.path);
                match (&worktree.branch, worktree.missing) {
                    (_, true) => format!("{path} (folder already gone)"),
                    (Some(branch), false) => format!("{path} ({branch})"),
                    (None, false) => path,
                }
            })
            .collect();
        let count = worktrees.len();
        Some(CleanupItem {
            id: CleanupId {
                backend: "projects".into(),
                key: format!("worktrees:{}", repository.display()),
            },
            kind: CleanupKind::Worktree,
            title: format!("{name} worktrees"),
            summary: format!(
                "{count} finished {}",
                if count == 1 { "worktree" } else { "worktrees" }
            ),
            preview: format!(
                "Removes these worktrees. Their branches stay, so you can check them out again.\n{}",
                lines.join("\n")
            ),
            bytes: Some(
                worktrees
                    .iter()
                    .filter(|worktree| !worktree.missing)
                    .map(|worktree| disk::usage(&worktree.path, cancel))
                    .sum(),
            ),
        })
    }

    /// Every task for the projects under `folders`, measured side by side.
    fn items(&self, folders: &[PathBuf], cancel: &Cancellation) -> Vec<CleanupItem> {
        let home = home(&self.host);
        let cutoff = self.cutoff();
        let mut seen = BTreeSet::new();
        let projects: Vec<Project> = folders
            .iter()
            .flat_map(|folder| projects_in(folder, home.clone(), cancel))
            // A folder inside another one is looked at once.
            .filter(|project| seen.insert(project.root.clone()))
            .collect();
        std::thread::scope(|scope| {
            let workers: Vec<_> = projects
                .iter()
                .map(|project| {
                    scope.spawn(move || {
                        let mut items = vec![];
                        if project.repository {
                            let worktrees = self.worktrees(&project.root, cutoff, cancel);
                            items.extend(self.worktree_item(&project.root, &worktrees, cancel));
                        }
                        items.extend(self.build_item(project, cutoff, cancel));
                        items
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().expect("project measuring panicked"))
                .collect()
        })
    }
}

impl Backend for Projects {
    fn id(&self) -> &str {
        "projects"
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, _cancel: &Cancellation) -> Result<Availability, EngineError> {
        Ok(if self.settings().folders.is_empty() {
            Availability::Unavailable("No folders to look for projects in.".into())
        } else {
            Availability::Available
        })
    }
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        // Read saved settings again so a choice applies to the next scan.
        let items = self.items(&self.settings().folders, cancel);
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        Ok(items)
    }
    /// Looks at the one project again, not every folder.
    fn cleanup_plan(
        &mut self,
        id: &CleanupId,
        cancel: &Cancellation,
    ) -> Result<CleanupItem, EngineError> {
        let root = id
            .key
            .strip_prefix("build:")
            .or_else(|| id.key.strip_prefix("worktrees:"))
            .map(PathBuf::from)
            .filter(|root| id.backend == "projects" && self.looked_at(root))
            .ok_or(EngineError::NotFound)?;
        let cutoff = self.cutoff();
        let item = if id.key.starts_with("build:") {
            projects_in(&root, None, cancel)
                .into_iter()
                .find(|project| project.root == root)
                .and_then(|project| self.build_item(&project, cutoff, cancel))
        } else {
            let worktrees = self.worktrees(&root, cutoff, cancel);
            self.worktree_item(&root, &worktrees, cancel)
        };
        item.filter(|item| item.id == *id)
            .ok_or(EngineError::NotFound)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let Operation::Clean(id) = operation else {
            return Err(self.unsupported(operation.capability()));
        };
        // What is removed is what a fresh look finds, never the key itself.
        let plan = self.cleanup_plan(id, cancel)?;
        not_while_building(&self.host)?;
        let failed = |what: &Path, error: String| {
            EngineError::Execution(ExecutionError::Io(format!(
                "Couldn't remove {}: {error}",
                shown(&self.host, what)
            )))
        };
        if let Some(root) = id.key.strip_prefix("build:").map(Path::new) {
            let project = projects_in(root, None, cancel)
                .into_iter()
                .find(|project| project.root == root)
                .ok_or(EngineError::NotFound)?;
            progress(Progress::Message(format!("Cleaning {}", plan.title)));
            // Real folders only: a link stays, wherever it leads.
            let folders = project.folders.iter().filter(|folder| {
                fs::symlink_metadata(folder).is_ok_and(|metadata| metadata.is_dir())
            });
            for folder in folders {
                if cancel.requested() {
                    return Err(EngineError::Cancelled);
                }
                disk::remove(folder).map_err(|error| failed(folder, error.to_string()))?;
            }
        } else {
            // `cleanup_plan` found the task, so it is one or the other.
            let repository = Path::new(id.key.strip_prefix("worktrees:").unwrap_or_default());
            let worktrees = self.worktrees(repository, self.cutoff(), cancel);
            progress(Progress::Message(format!("Cleaning {}", plan.title)));
            if worktrees.iter().any(|worktree| worktree.missing) {
                self.git(repository, &["worktree", "prune"], cancel)
                    .ok_or_else(|| failed(repository, "git worktree prune failed".into()))?;
            }
            for worktree in worktrees.iter().filter(|worktree| !worktree.missing) {
                if cancel.requested() {
                    return Err(EngineError::Cancelled);
                }
                // Without --force, Git keeps a worktree that has changes.
                let path = worktree.path.to_string_lossy();
                if self
                    .git(repository, &["worktree", "remove", &path], cancel)
                    .is_none()
                {
                    // Only this explicit choice permits removing untracked files.
                    // Recheck tracked edits after Git refuses ordinary removal.
                    let protection = self.settings().worktree_protection;
                    let clean = self
                        .git(&worktree.path, protection.status_args(), cancel)
                        .is_some_and(|status| status.trim().is_empty());
                    if protection != WorktreeProtection::EditedTrackedFiles || !clean {
                        return Err(failed(&worktree.path, "Git kept it".into()));
                    }
                    self.git(
                        repository,
                        &["worktree", "remove", "--force", &path],
                        cancel,
                    )
                    .ok_or_else(|| failed(&worktree.path, "Git kept it".into()))?;
                }
            }
        }
        Ok(OperationOutcome {
            cancellation_deferred: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Runtime;
    use std::{collections::BTreeMap, process::Command};

    fn scratch(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("pkgdeck-projects-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        // Git prints worktrees by their real path.
        fs::canonicalize(root).unwrap()
    }
    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    // Git may remove a file between listing it and dating it. Other errors
    // must still fail the fixture instead of leaving a file freshly dated.
    fn if_present<T>(path: &Path, result: std::io::Result<T>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("{}: {error}", path.display()),
        }
    }
    /// Every file under `dir` last changed `days` ago.
    fn age_files(dir: &Path, days: u64) {
        age_files_after_metadata(dir, days, |_| {});
    }
    // Tests can remove a path at the exact point Git's cleanup would race.
    fn age_files_after_metadata(dir: &Path, days: u64, mut after_metadata: impl FnMut(&Path)) {
        let when = SystemTime::now() - Duration::from_secs(days * 86_400);
        let mut pending = vec![dir.to_owned()];
        while let Some(path) = pending.pop() {
            let Some(metadata) = if_present(&path, fs::symlink_metadata(&path)) else {
                continue;
            };
            after_metadata(&path);
            if metadata.is_dir() {
                if let Some(entries) = if_present(&path, fs::read_dir(&path)) {
                    pending
                        .extend(entries.filter_map(|entry| {
                            if_present(&path, entry).map(|entry| entry.path())
                        }));
                }
            } else if metadata.is_file() {
                // The owner may date a file it can't write, like Git's objects.
                if_present(
                    &path,
                    fs::File::open(&path).and_then(|file| file.set_modified(when)),
                );
            }
        }
    }

    #[test]
    fn aging_files_skips_disappeared_paths_but_reports_other_errors() {
        let root = scratch("aging-missing");
        let missing = root.join("gone");
        age_files(&missing, 30);
        // The same rule applies if a directory or file vanishes after its
        // metadata was read, before listing or opening it.
        assert!(if_present(&missing, fs::read_dir(&missing)).is_none());
        assert!(if_present(&missing, fs::File::open(&missing)).is_none());

        let file = root.join("file");
        write(&file, "keep");
        // A real error that also occurs for root: a file is not a directory.
        let error = std::panic::catch_unwind(|| if_present(&file, fs::read_dir(&file)))
            .expect_err("filesystem errors must fail the fixture");
        let message = error.downcast_ref::<String>().unwrap();
        assert!(message.contains(file.to_str().unwrap()), "{message}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "keep");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn aging_files_tolerates_removal_between_metadata_and_listing_or_opening() {
        let root = scratch("aging-race");
        let dir = root.join("gone-dir");
        fs::create_dir(&dir).unwrap();
        age_files_after_metadata(&dir, 30, |path| fs::remove_dir(path).unwrap());
        assert!(!dir.exists());

        let file = root.join("gone-file");
        write(&file, "remove");
        let survivor = root.join("keep-file");
        write(&survivor, "keep");
        age_files_after_metadata(&root, 30, |path| {
            if path == file {
                fs::remove_file(path).unwrap();
            }
        });
        assert!(!file.exists());
        assert_eq!(fs::read_to_string(&survivor).unwrap(), "keep");
        assert!(
            SystemTime::now()
                .duration_since(fs::metadata(&survivor).unwrap().modified().unwrap())
                .unwrap()
                >= Duration::from_secs(30 * 86_400)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn aging_files_dates_read_only_files_without_following_symlinks() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("aging-read-only");
        let dir = root.join("project");
        let file = dir.join("objects/object");
        write(&file, "object");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o444)).unwrap();
        let outside = root.join("outside");
        write(&outside, "keep");
        let original_time = fs::metadata(&outside).unwrap().modified().unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();
        age_files(&dir, 30);

        let metadata = fs::metadata(&file).unwrap();
        assert!(
            SystemTime::now()
                .duration_since(metadata.modified().unwrap())
                .unwrap()
                >= Duration::from_secs(30 * 86_400)
        );
        assert_eq!(metadata.permissions().mode() & 0o777, 0o444);
        assert_eq!(fs::read_to_string(&file).unwrap(), "object");
        assert!(fs::symlink_metadata(dir.join("link"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::metadata(&outside).unwrap().modified().unwrap(),
            original_time
        );
        fs::remove_dir_all(root).unwrap();
    }
    /// Tools for the machine in `root`: the real Git, and a `ps` that lists
    /// `processes`.
    fn tools(root: &Path, processes: &str) -> PathBuf {
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let git = Host::current().resolve("git").unwrap().expect("git");
        let _ = fs::remove_file(bin.join("git"));
        std::os::unix::fs::symlink(git, bin.join("git")).unwrap();
        let ps = bin.join("ps");
        fs::write(&ps, format!("#!/bin/sh\nprintf '{processes}'\n")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&ps, fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }
    fn host(home: &Path, bin: &Path, extra: &[(&str, &Path)]) -> Host {
        let mut env = BTreeMap::from([
            ("HOME".into(), home.as_os_str().to_owned()),
            ("PATH".into(), bin.as_os_str().to_owned()),
        ]);
        for (name, value) in extra {
            env.insert(name.into(), value.as_os_str().to_owned());
        }
        Host::new(Runtime::Native, env)
    }
    fn projects(home: &Path, bin: &Path, folders: Vec<PathBuf>, days: u32) -> Projects {
        Projects::new(host(home, bin, &[])).with_settings(ProjectSettings {
            folders,
            days,
            ..ProjectSettings::default()
        })
    }
    fn keys(items: &[CleanupItem]) -> Vec<String> {
        items.iter().map(|item| item.id.key.clone()).collect()
    }

    #[test]
    fn build_files_of_old_projects_are_offered_and_deleted() {
        let home = scratch("build");
        let code = home.join("code");
        let fill = |path: &Path| write(path, &"x".repeat(64 * 1024));
        // An old Rust project.
        write(&code.join("rusty/Cargo.toml"), "[package]");
        write(&code.join("rusty/src/main.rs"), "fn main() {}");
        fill(&code.join("rusty/target/debug/app"));
        // An old web app, with a nested Git checkout inside node_modules
        // that is not a project of its own.
        write(&code.join("web/package.json"), "{}");
        fill(&code.join("web/node_modules/left-pad/index.js"));
        write(&code.join("web/node_modules/dep/.git/HEAD"), "ref");
        fill(&code.join("web/.next/cache"));
        // Gradle, Flutter, .NET, CMake and Zig.
        write(&code.join("android/settings.gradle"), "");
        write(&code.join("android/app/build.gradle"), "");
        fill(&code.join("android/app/build/out.apk"));
        fill(&code.join("android/.gradle/state"));
        write(&code.join("flutter/pubspec.yaml"), "");
        fill(&code.join("flutter/.dart_tool/x"));
        write(&code.join("dotnet/App.csproj"), "");
        fill(&code.join("dotnet/bin/App.dll"));
        fill(&code.join("dotnet/obj/App.o"));
        write(&code.join("native/CMakeLists.txt"), "");
        fill(&code.join("native/out/CMakeCache.txt"));
        write(&code.join("zig/build.zig"), "");
        fill(&code.join("zig/zig-out/app"));
        // Folders that only look like build output.
        write(&code.join("plain/notes.txt"), "");
        fill(&code.join("plain/target/keep"));
        write(&code.join("docs/package.json"), "{}");
        fill(&code.join("docs/build/index.html"));
        // Out of reach: the home's Library, hidden folders, a link out.
        write(&home.join("Library/app/Cargo.toml"), "");
        fill(&home.join("Library/app/target/x"));
        write(&home.join(".hidden/app/Cargo.toml"), "");
        fill(&home.join(".hidden/app/target/x"));
        let elsewhere = scratch("build-elsewhere");
        write(&elsewhere.join("app/Cargo.toml"), "");
        fill(&elsewhere.join("app/target/x"));
        std::os::unix::fs::symlink(&elsewhere, code.join("link")).unwrap();
        age_files(&home, 30);
        age_files(&elsewhere, 30);
        // Changed today: still in use.
        write(&code.join("fresh/package.json"), "{}");
        fill(&code.join("fresh/node_modules/x"));
        let bin = tools(&home, "");
        let mut backend = projects(&home, &bin, vec![home.clone()], 7);
        let cancel = Cancellation::default();
        assert_eq!(backend.detect(&cancel).unwrap(), Availability::Available);
        let items = backend.cleanup(&cancel).unwrap();
        let build = |name: &str| format!("build:{}", code.join(name).display());
        let mut found = keys(&items);
        found.sort();
        let mut expected: Vec<_> = [
            "android", "dotnet", "flutter", "native", "rusty", "web", "zig",
        ]
        .map(build)
        .to_vec();
        expected.sort();
        assert_eq!(found, expected);
        let item = |name: &str| {
            items
                .iter()
                .find(|item| item.id.key == build(name))
                .unwrap()
        };
        assert_eq!(
            item("web").preview,
            "Deletes these folders. A build or an install makes them again.\n~/code/web/.next\n~/code/web/node_modules"
        );
        assert!(item("android").preview.contains("~/code/android/app/build"));
        assert!(item("android").preview.contains("~/code/android/.gradle"));
        assert!(item("native").preview.contains("~/code/native/out"));
        assert_eq!(item("rusty").title, "rusty build files");
        assert_eq!(item("rusty").kind, CleanupKind::BuildFiles);
        assert_eq!(item("rusty").summary, "Unchanged for 4 weeks");
        assert!(item("web").bytes.unwrap() >= 128 * 1024);

        // The plan is the same when looked at again, and deleting it keeps
        // the project itself.
        let id = item("rusty").id.clone();
        assert_eq!(&backend.cleanup_plan(&id, &cancel).unwrap(), item("rusty"));
        backend
            .execute(&Operation::Clean(id.clone()), &cancel, &mut |_| {})
            .unwrap();
        assert!(!code.join("rusty/target").exists());
        assert!(code.join("rusty/src/main.rs").exists());
        assert!(matches!(
            backend.cleanup_plan(&id, &cancel),
            Err(EngineError::NotFound)
        ));
        // A project changed since the review is left alone.
        let web = item("web").id.clone();
        write(&code.join("web/index.js"), "");
        assert!(matches!(
            backend.execute(&Operation::Clean(web), &cancel, &mut |_| {}),
            Err(EngineError::NotFound)
        ));
        assert!(code.join("web/node_modules").exists());
        fs::remove_dir_all(&home).unwrap();
        fs::remove_dir_all(&elsewhere).unwrap();
    }

    #[test]
    fn keys_outside_the_folders_builds_and_cancelling_change_nothing() {
        let home = scratch("refuse");
        let project = home.join("code/app");
        write(&project.join("Cargo.toml"), "");
        write(&project.join("target/x"), "built");
        age_files(&home, 30);
        let cancel = Cancellation::default();
        // Something is being built.
        let bin = tools(&home, "/usr/bin/cargo build\\n");
        let mut backend = projects(&home, &bin, vec![home.join("code")], 7);
        let id = backend.cleanup(&cancel).unwrap()[0].id.clone();
        assert!(matches!(
            backend.execute(&Operation::Clean(id.clone()), &cancel, &mut |_| {}),
            Err(EngineError::Execution(ExecutionError::Disabled(_)))
        ));
        assert!(project.join("target/x").exists());
        // Only folders the settings name, written plainly.
        let bin = tools(&home, "");
        // A folder in the settings that is gone is skipped.
        let mut backend = projects(
            &home,
            &bin,
            vec![home.join("missing"), home.join("code")],
            7,
        );
        assert_eq!(backend.cleanup(&cancel).unwrap().len(), 1);
        for key in [
            format!("build:{}", home.display()),
            format!("build:{}/code/../code/app", home.display()),
            "build:relative/app".into(),
            format!("other:{}", project.display()),
        ] {
            let other = CleanupId {
                backend: "projects".into(),
                key,
            };
            assert!(matches!(
                backend.cleanup_plan(&other, &cancel),
                Err(EngineError::NotFound)
            ));
        }
        let wrong_backend = CleanupId {
            backend: "dev-caches".into(),
            ..id.clone()
        };
        assert!(backend.cleanup_plan(&wrong_backend, &cancel).is_err());
        let mut ignore = |_: Progress| {};
        let refresh = Operation::Refresh {
            backend: "projects".into(),
        };
        assert!(backend.execute(&refresh, &cancel, &mut ignore).is_err());
        // A longer wait leaves the project for now.
        let mut patient = projects(&home, &bin, vec![home.join("code")], 60);
        assert!(patient.cleanup(&cancel).unwrap().is_empty());
        // No folders: nothing to look at.
        let mut nowhere = projects(&home, &bin, vec![], 7);
        assert!(matches!(
            nowhere.detect(&cancel).unwrap(),
            Availability::Unavailable(_)
        ));
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(matches!(
            backend.cleanup(&cancelled),
            Err(EngineError::Cancelled)
        ));
        assert!(matches!(
            backend.execute(&Operation::Clean(id), &cancelled, &mut |_| {}),
            Err(EngineError::NotFound | EngineError::Cancelled)
        ));
        assert!(project.join("target/x").exists());
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn cleaning_stops_when_cancelled_and_says_what_it_could_not_remove() {
        let home = scratch("stop");
        let code = home.join("code");
        write(&code.join("web/package.json"), "{}");
        write(&code.join("web/node_modules/x"), "dep");
        write(&code.join("web/.next/cache"), "built");
        // Not a repository after all, and a folder nobody may read.
        fs::create_dir_all(code.join("broken/.git")).unwrap();
        write(&code.join("broken/Cargo.toml"), "");
        fs::create_dir_all(code.join("private/app")).unwrap();
        age_files(&home, 30);
        let mode = |path: &Path, mode: u32| {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        };
        let bin = tools(&home, "");
        let mut backend = projects(&home, &bin, vec![code.clone()], 7);
        let cancel = Cancellation::default();
        let mut ignore = |_: Progress| {};
        mode(&code.join("private"), 0o000);
        let items = backend.cleanup(&cancel).unwrap();
        mode(&code.join("private"), 0o755);
        assert_eq!(
            keys(&items),
            [format!("build:{}", code.join("web").display())]
        );
        let web = Operation::Clean(items[0].id.clone());
        // Cancelled once the clean begins: nothing goes.
        let stopping = Cancellation::default();
        assert!(matches!(
            backend.execute(&web, &stopping, &mut |_| stopping.cancel()),
            Err(EngineError::Cancelled)
        ));
        assert!(code.join("web/node_modules/x").exists());
        // A folder that can't go says which one. Root may change anything,
        // so this only shows for everyone else.
        mode(&code.join("web"), 0o555);
        let result = backend.execute(&web, &cancel, &mut ignore);
        mode(&code.join("web"), 0o755);
        let root_user = rustix::process::geteuid().is_root();
        assert!(
            root_user
                || result
                    .is_err_and(|error| error.to_string().contains("Couldn't remove ~/code/web/"))
        );

        // A detached worktree shows its folder alone.
        let repo = home.join("repo");
        write(&repo.join("README"), "hi");
        git(&repo, &["init", "--quiet", "-b", "main"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "first"]);
        let detached = home.join(".trees/detached");
        let path = detached.display().to_string();
        git(&repo, &["worktree", "add", "--quiet", "--detach", &path]);
        age_files(&home, 30);
        let mut backend = projects(&home, &bin, vec![repo.clone()], 7);
        let items = backend.cleanup(&cancel).unwrap();
        assert_eq!(items.len(), 1, "{items:?}");
        assert!(
            items[0].preview.ends_with("\n~/.trees/detached"),
            "{items:?}"
        );
        let clean = Operation::Clean(items[0].id.clone());
        let stopping = Cancellation::default();
        assert!(matches!(
            backend.execute(&clean, &stopping, &mut |_| stopping.cancel()),
            Err(EngineError::Cancelled)
        ));
        assert!(detached.exists());
        // Git refusing to remove or prune one says so.
        let refusing = home.join("refusing");
        fs::create_dir_all(&refusing).unwrap();
        let real = Host::current().resolve("git").unwrap().expect("git");
        fs::write(
            refusing.join("git"),
            format!(
                "#!/bin/sh\ncase \"$3 $4\" in 'worktree remove'|'worktree prune') exit 1 ;; esac\nexec '{}' \"$@\"\n",
                real.display()
            ),
        )
        .unwrap();
        mode(&refusing.join("git"), 0o755);
        fs::copy(bin.join("ps"), refusing.join("ps")).unwrap();
        let mut refused = projects(&home, &refusing, vec![repo.clone()], 7);
        let error = refused.execute(&clean, &cancel, &mut ignore).unwrap_err();
        assert!(
            error.to_string().contains("~/.trees/detached: Git kept it"),
            "{error}"
        );
        // A folder deleted by hand needs a prune first.
        fs::remove_dir_all(&detached).unwrap();
        let error = refused.execute(&clean, &cancel, &mut ignore).unwrap_err();
        assert!(
            error.to_string().contains("git worktree prune failed"),
            "{error}"
        );
        fs::remove_dir_all(&home).unwrap();
    }

    fn git(dir: &Path, args: &[&str]) {
        let done = Command::new("git")
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(done.status.success(), "{args:?}: {done:?}");
    }

    #[test]
    fn only_finished_untouched_worktrees_are_offered_and_removed() {
        let home = scratch("worktrees");
        let repo = home.join("repo");
        write(&repo.join("README"), "hi");
        git(&repo, &["init", "--quiet", "-b", "main"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "first"]);
        let trees = home.join(".trees");
        let tree = |name: &str| trees.join(name);
        for name in ["merged", "ahead", "dirty", "locked", "gone", "fresh"] {
            let path = tree(name).display().to_string();
            git(&repo, &["worktree", "add", "--quiet", "-b", name, &path]);
        }
        // Work that isn't on main yet.
        write(&tree("ahead").join("new"), "work");
        git(&tree("ahead"), &["add", "."]);
        git(&tree("ahead"), &["commit", "--quiet", "-m", "wip"]);
        write(&tree("dirty").join("draft"), "unsaved");
        git(
            &repo,
            &["worktree", "lock", &tree("locked").display().to_string()],
        );
        age_files(&home, 30);
        // Deleted by hand, and one still in use.
        fs::remove_dir_all(tree("gone")).unwrap();
        write(&tree("fresh").join("README"), "edited today");
        git(&tree("fresh"), &["commit", "--quiet", "-am", "today"]);
        git(&repo, &["merge", "--quiet", "--ff-only", "fresh"]);
        let bin = tools(&home, "");
        let mut backend = projects(&home, &bin, vec![home.clone()], 7);
        let cancel = Cancellation::default();
        let items = backend.cleanup(&cancel).unwrap();
        let key = format!("worktrees:{}", repo.display());
        let item = items
            .iter()
            .find(|item| item.id.key == key)
            .expect("worktrees");
        assert_eq!(item.kind, CleanupKind::Worktree);
        assert_eq!(item.title, "repo worktrees");
        assert_eq!(item.summary, "2 finished worktrees");
        assert_eq!(
            item.preview,
            "Removes these worktrees. Their branches stay, so you can check them out again.\n~/.trees/gone (folder already gone)\n~/.trees/merged (merged)"
        );
        assert!(item.bytes.unwrap() > 0);
        assert_eq!(&backend.cleanup_plan(&item.id, &cancel).unwrap(), item);
        backend
            .execute(&Operation::Clean(item.id.clone()), &cancel, &mut |_| {})
            .unwrap();
        assert!(!tree("merged").exists());
        for name in ["ahead", "dirty", "locked", "fresh"] {
            assert!(tree(name).exists(), "{name}");
        }
        let list = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let list = String::from_utf8(list.stdout).unwrap();
        assert!(
            !list.contains("/gone") && !list.contains("/merged"),
            "{list}"
        );
        // The branch stays.
        git(
            &repo,
            &["rev-parse", "--verify", "--quiet", "refs/heads/merged"],
        );
        // Nothing left to offer for the repository.
        assert!(matches!(
            backend.cleanup_plan(&item.id, &cancel),
            Err(EngineError::NotFound)
        ));
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_worktree_whose_remote_branch_was_deleted_counts_as_finished() {
        let home = scratch("gone-upstream");
        let origin = home.join("origin.git");
        let repo = home.join("repo");
        fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "--quiet", "--bare", "-b", "main"]);
        write(&repo.join("README"), "hi");
        git(&repo, &["init", "--quiet", "-b", "main"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "first"]);
        git(
            &repo,
            &["remote", "add", "origin", &origin.display().to_string()],
        );
        git(&repo, &["push", "--quiet", "-u", "origin", "main"]);
        git(&repo, &["remote", "set-head", "origin", "main"]);
        let squashed = home.join(".trees/squashed");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "squashed",
                &squashed.display().to_string(),
            ],
        );
        write(&squashed.join("feature"), "done");
        git(&squashed, &["add", "."]);
        git(&squashed, &["commit", "--quiet", "-m", "feature"]);
        git(&squashed, &["push", "--quiet", "-u", "origin", "squashed"]);
        // Merged with a squash on the server, which deletes the branch.
        git(&origin, &["branch", "-D", "squashed"]);
        git(&repo, &["fetch", "--quiet", "--prune"]);
        age_files(&home, 30);
        let bin = tools(&home, "");
        let mut backend = projects(&home, &bin, vec![repo.clone()], 7);
        let items = backend.cleanup(&Cancellation::default()).unwrap();
        assert_eq!(items.len(), 1, "{items:?}");
        assert!(items[0].preview.ends_with("~/.trees/squashed (squashed)"));
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn worktree_protection_checks_real_git_files_and_removal_for_every_choice() {
        // Exhaust the local-file states for each policy, using real repositories.
        for (policy_index, protection) in WorktreeProtection::ALL.into_iter().enumerate() {
            for state in 0..6 {
                let home = scratch(&format!("protection-{policy_index}-{state}"));
                let repo = home.join("repo");
                write(&repo.join("tracked"), "original");
                write(&repo.join(".gitignore"), ".env\nignored/\n");
                git(&repo, &["init", "--quiet", "-b", "main"]);
                git(&repo, &["add", "."]);
                git(&repo, &["commit", "--quiet", "-m", "fixture"]);
                let tree = home.join(".trees/finished");
                git(
                    &repo,
                    &[
                        "worktree",
                        "add",
                        "--quiet",
                        "-b",
                        "finished",
                        tree.to_str().unwrap(),
                    ],
                );
                match state {
                    0 => {}
                    1 => write(&tree.join(".env"), "fake config"),
                    2 => write(&tree.join("draft"), "untracked"),
                    3 => write(&tree.join("tracked"), "edited"),
                    4 => {
                        write(&tree.join("tracked"), "staged");
                        git(&tree, &["add", "tracked"]);
                    }
                    _ => {
                        fs::remove_file(tree.join("tracked")).unwrap();
                    }
                }
                age_files(&home, 30);
                let bin = tools(&home, "");
                let mut backend = projects(&home, &bin, vec![repo.clone()], 7);
                backend.settings.as_mut().unwrap().worktree_protection = protection;
                let cancel = Cancellation::default();
                let items = backend.cleanup(&cancel).unwrap();
                let allowed = state == 0
                    || (state == 1 && protection != WorktreeProtection::AnyLocalFiles)
                    || (state == 2 && protection == WorktreeProtection::EditedTrackedFiles);
                assert_eq!(!items.is_empty(), allowed, "{protection:?}, state {state}");
                if let Some(item) = items.first() {
                    backend
                        .execute(&Operation::Clean(item.id.clone()), &cancel, &mut |_| {})
                        .unwrap();
                    assert!(!tree.exists(), "{protection:?}, state {state}");
                    git(&repo, &["rev-parse", "--verify", "refs/heads/finished"]);
                } else {
                    assert!(tree.exists());
                }
                fs::remove_dir_all(home).unwrap();
            }
        }
    }

    #[test]
    fn changed_settings_and_new_edits_are_checked_before_worktree_removal() {
        let home = scratch("settings-recheck");
        let repo = home.join("repo");
        write(&repo.join("tracked"), "original");
        write(&repo.join(".gitignore"), ".env\n");
        git(&repo, &["init", "--quiet", "-b", "main"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "fixture"]);
        let tree = home.join(".trees/finished");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "finished",
                tree.to_str().unwrap(),
            ],
        );
        write(&tree.join(".env"), "fake config");
        age_files(&home, 30);
        let bin = tools(&home, "");
        let host = host(&home, &bin, &[]);
        let mut settings = ProjectSettings {
            folders: vec![repo.clone()],
            ..ProjectSettings::default()
        };
        settings.save(&host).unwrap();
        let mut backend = Projects::new(host.clone());
        let cancel = Cancellation::default();
        assert!(backend.cleanup(&cancel).unwrap().is_empty());
        settings.worktree_protection = WorktreeProtection::UncommittedChanges;
        settings.save(&host).unwrap();
        let item = backend.cleanup(&cancel).unwrap().remove(0);
        write(&tree.join("tracked"), "new edit");
        assert!(backend
            .execute(&Operation::Clean(item.id), &cancel, &mut |_| {})
            .is_err());
        assert_eq!(
            fs::read_to_string(tree.join("tracked")).unwrap(),
            "new edit"
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn settings_default_to_the_home_and_a_week_and_survive_bad_files() {
        let home = scratch("settings");
        let bin = home.join("bin");
        let host = host(&home, &bin, &[]);
        assert_eq!(
            ProjectSettings::load(&host),
            ProjectSettings {
                folders: vec![home.clone()],
                days: 7,
                ..ProjectSettings::default()
            }
        );
        let path = ProjectSettings::path(&host).unwrap();
        assert_eq!(path, home.join(".config/pkgdeck/clean.json"));
        let saved = ProjectSettings {
            folders: vec![home.join("code"), PathBuf::from("/srv/projects")],
            days: 30,
            worktree_protection: WorktreeProtection::UncommittedChanges,
        };
        saved.save(&host).unwrap();
        assert_eq!(ProjectSettings::load(&host), saved);
        // Hand edits: missing fields, relative folders, broken JSON.
        fs::write(&path, r#"{"folders": ["relative", "/abs"]}"#).unwrap();
        assert_eq!(
            ProjectSettings::load(&host),
            ProjectSettings {
                folders: vec![PathBuf::from("/abs")],
                days: 7,
                ..ProjectSettings::default()
            }
        );
        fs::write(&path, "{").unwrap();
        assert_eq!(ProjectSettings::load(&host).days, 7);
        // XDG_CONFIG_HOME wins when it's a real path.
        let config = home.join("xdg");
        let xdg = super::tests::host(&home, &bin, &[("XDG_CONFIG_HOME", &config)]);
        assert_eq!(
            ProjectSettings::path(&xdg).unwrap(),
            config.join("pkgdeck/clean.json")
        );
        let relative = super::tests::host(&home, &bin, &[("XDG_CONFIG_HOME", Path::new("xdg"))]);
        assert_eq!(ProjectSettings::path(&relative).unwrap(), path);
        // Without a home there is nowhere to keep them.
        let homeless = Host::new(
            Runtime::Native,
            BTreeMap::from([("PATH".into(), bin.into_os_string())]),
        );
        assert_eq!(ProjectSettings::path(&homeless), None);
        assert!(ProjectSettings::defaults(&homeless).folders.is_empty());
        assert!(saved.save(&homeless).is_err());
        // A file where the folder should be.
        fs::remove_dir_all(home.join(".config")).unwrap();
        fs::write(home.join(".config"), "").unwrap();
        assert!(saved.save(&host).is_err());
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn protection_choices_have_the_promised_labels_and_file_rules() {
        assert_eq!(
            WorktreeProtection::ALL.map(WorktreeProtection::name),
            [
                "Always keep",
                "Keep if files are new or edited",
                "Keep only if files are edited"
            ]
        );
        assert_eq!(
            WorktreeProtection::ALL.map(WorktreeProtection::description),
            [
                "Safest. Keeps everything",
                "Loses ignored files, like .env",
                "Loses new and ignored files, like .env"
            ]
        );
    }

    #[test]
    fn ages_read_in_rough_steps() {
        let now = SystemTime::now();
        let ago = |days: u64| age(now - Duration::from_secs(days * 86_400 + 60), now);
        assert_eq!(ago(0), "Unchanged for a few days");
        assert_eq!(ago(6), "Unchanged for a few days");
        assert_eq!(ago(7), "Unchanged for a week");
        assert_eq!(ago(21), "Unchanged for 3 weeks");
        assert_eq!(ago(59), "Unchanged for 8 weeks");
        assert_eq!(ago(60), "Unchanged for 2 months");
        assert_eq!(ago(729), "Unchanged for 24 months");
        assert_eq!(ago(730), "Unchanged for 2 years");
        assert_eq!(ago(400 * 3), "Unchanged for 3 years");
        assert_eq!(
            age(now + Duration::from_secs(60), now),
            "Unchanged for a few days"
        );
    }
}
