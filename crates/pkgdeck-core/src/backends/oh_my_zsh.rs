//! Oh My Zsh and the plugins and themes cloned into its custom folder.
//!
//! Each git checkout is a row: Oh My Zsh itself (`$ZSH`, by default
//! `~/.oh-my-zsh`) and every `plugins/*` and `themes/*` checkout under
//! `$ZSH_CUSTOM` (by default `$ZSH/custom`). Update checks fetch each
//! checkout; plain listings compare with what was last fetched and never use
//! the network. Updating fast-forwards to the upstream branch, so local
//! commits or edits stop it instead of being merged or lost.
use super::{bytes, invalid, NativeTransport, Transport};
use crate::{engine::*, package::*, process::*};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

const ID: &str = "oh-my-zsh";
const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Upgrade,
];
/// The row for Oh My Zsh itself.
const CORE: &str = "oh-my-zsh";
const HOMEPAGE: &str = "https://ohmyz.sh";

pub struct OhMyZsh<T = NativeTransport> {
    transport: T,
    update_check: Option<u64>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Core,
    Plugin,
    Theme,
}

struct Checkout {
    name: String,
    kind: Kind,
    path: PathBuf,
}

/// What git says about one checkout.
struct State {
    head: String,
    /// The upstream commit, when the branch tracks one.
    upstream: Option<String>,
    behind: u64,
}

/// A folder name under `plugins/` or `themes/` that is safe as a row name.
fn valid_folder(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with(['-', '.'])
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn kind_of(name: &str) -> Option<(Kind, &str)> {
    if name == CORE {
        return Some((Kind::Core, name));
    }
    let (kind, folder) = if let Some(folder) = name.strip_prefix("plugin/") {
        (Kind::Plugin, folder)
    } else {
        (Kind::Theme, name.strip_prefix("theme/")?)
    };
    valid_folder(folder).then_some((kind, folder))
}

impl<T: Transport> OhMyZsh<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            update_check: None,
        }
    }

    fn absolute(&self, name: &str) -> Option<PathBuf> {
        self.transport
            .env(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    }
    /// `$ZSH`, or `~/.oh-my-zsh` where the installer puts it.
    fn root(&self) -> Option<PathBuf> {
        self.absolute("ZSH")
            .or_else(|| self.absolute("HOME").map(|home| home.join(".oh-my-zsh")))
    }
    fn custom(&self, root: &Path) -> PathBuf {
        self.absolute("ZSH_CUSTOM")
            .unwrap_or_else(|| root.join("custom"))
    }

    fn checkouts(&self) -> Result<Vec<Checkout>, EngineError> {
        let root = self.root().ok_or(EngineError::NotFound)?;
        let mut found = vec![Checkout {
            name: CORE.into(),
            kind: Kind::Core,
            path: root.clone(),
        }];
        let custom = self.custom(&root);
        for (folder, kind, prefix) in [
            ("plugins", Kind::Plugin, "plugin/"),
            ("themes", Kind::Theme, "theme/"),
        ] {
            let Ok(entries) = std::fs::read_dir(custom.join(folder)) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().join(".git").exists())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| valid_folder(name))
                .collect();
            names.sort();
            found.extend(names.into_iter().map(|name| Checkout {
                path: custom.join(folder).join(&name),
                name: format!("{prefix}{name}"),
                kind,
            }));
        }
        Ok(found)
    }

    fn git(
        &self,
        path: &Path,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<String, EngineError> {
        let mut all: Vec<OsString> = vec!["-C".into(), path.into()];
        all.extend(args.iter().map(OsString::from));
        let output = bytes(ID, self.transport.dev_tool("git", &all, cancel, write)?)?;
        String::from_utf8(output)
            .map(|text| text.trim().to_owned())
            .map_err(|error| invalid(ID, error))
    }

    fn state(&self, checkout: &Checkout, cancel: &Cancellation) -> Result<State, EngineError> {
        let head = self.git(
            &checkout.path,
            &["rev-parse", "--short=12", "HEAD"],
            cancel,
            false,
        )?;
        // A detached checkout, or a branch without an upstream, has nothing
        // to update to.
        let upstream = match self.git(
            &checkout.path,
            &["rev-parse", "--short=12", "@{upstream}"],
            cancel,
            false,
        ) {
            Ok(upstream) => Some(upstream),
            Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
            Err(_) => None,
        };
        let behind = match upstream {
            Some(_) => self
                .git(
                    &checkout.path,
                    &["rev-list", "--count", "HEAD..@{upstream}"],
                    cancel,
                    false,
                )?
                .parse()
                .map_err(|error| invalid(ID, error))?,
            None => 0,
        };
        Ok(State {
            head,
            upstream,
            behind,
        })
    }

    fn package(checkout: &Checkout, state: &State) -> Package {
        let summary = match checkout.kind {
            Kind::Core => "Zsh configuration framework".to_owned(),
            Kind::Plugin => "Oh My Zsh custom plugin".to_owned(),
            Kind::Theme => "Oh My Zsh custom theme".to_owned(),
        };
        let (update, candidate) = match (&state.upstream, state.behind) {
            (None, _) => (UpdateAvailability::Unknown, None),
            (Some(upstream), behind) if behind > 0 => {
                (UpdateAvailability::Available, Some(upstream.clone()))
            }
            (Some(upstream), _) => (UpdateAvailability::Current, Some(upstream.clone())),
        };
        Package {
            id: PackageId {
                backend: ID.into(),
                name: checkout.name.clone(),
                architecture: "all".into(),
                scope: Scope::User {
                    uid: rustix::process::getuid().as_raw(),
                },
                remote: None,
                reference: None,
            },
            display_name: match checkout.kind {
                Kind::Core => "Oh My Zsh".into(),
                _ => checkout
                    .name
                    .split_once('/')
                    .map_or(checkout.name.clone(), |(_, folder)| folder.to_owned()),
            },
            summary,
            installed_version: Some(state.head.clone()),
            candidate_version: candidate,
            update,
            icon: None,
            component_ids: vec![],
            homepages: if checkout.kind == Kind::Core {
                vec![HOMEPAGE.into()]
            } else {
                vec![]
            },
            adopt_with: None,
        }
    }

    fn rows(&self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.checkouts()?
            .iter()
            .map(|checkout| Ok(Self::package(checkout, &self.state(checkout, cancel)?)))
            .collect()
    }

    fn find(&self, id: &PackageId) -> Result<Checkout, EngineError> {
        if id.backend != ID || kind_of(&id.name).is_none() {
            return Err(invalid(ID, "foreign or invalid Oh My Zsh checkout"));
        }
        self.checkouts()?
            .into_iter()
            .find(|checkout| checkout.name == id.name)
            .ok_or(EngineError::NotFound)
    }
}

impl<T: Transport> Backend for OhMyZsh<T> {
    fn id(&self) -> &str {
        ID
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        let Some(root) = self.root() else {
            return Ok(Availability::Unavailable("HOME is not set".into()));
        };
        if !root.join("oh-my-zsh.sh").is_file() || !root.join(".git").exists() {
            return Ok(Availability::Unavailable(format!(
                "Oh My Zsh not found in {}",
                root.display()
            )));
        }
        match self
            .transport
            .dev_tool("git", &["--version".into()], cancel, false)
        {
            Ok(_) => Ok(Availability::Available),
            Err(ExecutionError::Disabled(reason)) => Ok(Availability::Unavailable(reason)),
            Err(ExecutionError::Cancelled) => Err(EngineError::Cancelled),
            Err(error) => Err(error.into()),
        }
    }
    fn may_have(&self, name: &str) -> bool {
        kind_of(name).is_some()
    }
    fn has_update_index(&self) -> bool {
        true
    }
    fn arm_update_check(&mut self, token: Option<u64>) {
        self.update_check = token;
    }
    /// Fetch every checkout. One that fails (offline, or a remote that is
    /// gone) does not stop the others; the first failure is reported.
    fn refresh_update_index(&mut self, cancel: &Cancellation) -> Result<(), EngineError> {
        if self.update_check.is_none() {
            return Ok(());
        }
        let mut first = None;
        for checkout in self.checkouts()? {
            match self.git(&checkout.path, &["fetch", "--quiet"], cancel, false) {
                Ok(_) => {}
                Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
                Err(error) => {
                    first.get_or_insert(error);
                }
            }
        }
        first.map_or(Ok(()), Err)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let query = query.to_lowercase();
        Ok(self
            .rows(cancel)?
            .into_iter()
            .filter(|row| {
                row.id.name.contains(&query) || row.display_name.to_lowercase().contains(&query)
            })
            .collect())
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.rows(cancel)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let checkout = self.find(id)?;
        let state = self.state(&checkout, cancel)?;
        let package = Self::package(&checkout, &state);
        let remote = self
            .git(
                &checkout.path,
                &["config", "--get", "remote.origin.url"],
                cancel,
                false,
            )
            .ok()
            .filter(|url| url.starts_with("https://"));
        let mut description = format!("A git checkout in {}.", checkout.path.display());
        description.push_str(if state.upstream.is_some() {
            " Updating fast-forwards it to its upstream branch; local commits or changes stop the update."
        } else {
            " It does not track an upstream branch, so it is never updated."
        });
        Ok(PackageDetails {
            package,
            description,
            homepage: remote.or_else(|| (checkout.kind == Kind::Core).then(|| HOMEPAGE.into())),
            dependencies: vec![],
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let Operation::Upgrade(id) = operation else {
            return Err(self.unsupported(operation.capability()));
        };
        let checkout = self.find(id)?;
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        progress(Progress::Message(format!("Updating {}", checkout.name)));
        let args: Vec<OsString> = ["-C".as_ref(), checkout.path.as_os_str()]
            .into_iter()
            .map(OsString::from)
            .chain(["pull", "--ff-only", "--quiet"].map(OsString::from))
            .collect();
        let completion = self.transport.dev_tool("git", &args, cancel, true)?;
        let deferred = completion.cancellation_deferred;
        bytes(ID, completion)?;
        // Verify with a fresh read; writes may finish after cancellation.
        let after = self.state(&checkout, &Cancellation::default())?;
        if after.behind > 0 {
            return Err(invalid(
                ID,
                format!("git finished, but {} is still behind", checkout.name),
            ));
        }
        Ok(OperationOutcome {
            cancellation_deferred: deferred,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::dev_tools::{DevTool, DevTools};
    use std::{
        collections::BTreeMap,
        process::Command,
        sync::{Arc, Mutex},
    };

    /// A command line part that makes the call fail, and how.
    type Failure = (&'static str, fn() -> ExecutionError);

    /// Runs the real `git`, with `HOME`, `ZSH` and `ZSH_CUSTOM` from `env`.
    /// A command whose arguments contain `fail` fails as `failure` does.
    #[derive(Clone, Default)]
    struct Git {
        env: BTreeMap<&'static str, OsString>,
        fail: Option<Failure>,
        calls: Arc<Mutex<Vec<String>>>,
    }
    impl DevTool for Git {
        fn dev_tool(
            &self,
            executable: &str,
            args: &[OsString],
            _: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            let line = std::iter::once(executable.to_owned())
                .chain(args.iter().map(|arg| arg.to_string_lossy().into_owned()))
                .collect::<Vec<_>>()
                .join(" ");
            self.calls
                .lock()
                .unwrap()
                .push(format!("{}{line}", if write { "w:" } else { "" }));
            if let Some((needle, error)) = self.fail {
                if line.contains(needle) {
                    return Err(error());
                }
            }
            assert_eq!(executable, "git");
            let output = Command::new("git")
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("HOME", &self.env["HOME"])
                .output()
                .unwrap();
            let completion = Completion {
                code: output.status.code(),
                signal: None,
                stdout: output.stdout,
                stderr: output.stderr,
                truncated: false,
                cancellation_deferred: false,
            };
            if completion.code == Some(0) {
                Ok(completion)
            } else {
                Err(ExecutionError::Failed(completion))
            }
        }
        fn env(&self, name: &str) -> Option<OsString> {
            self.env.get(name).cloned()
        }
    }

    /// A home folder removed when the test ends.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "pkgdeck-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=PkgDeck",
                "-c",
                "user.email=pkgdeck@example.invalid",
                "-c",
                "init.defaultBranch=main",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", dir)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
    fn commit(repo: &Path, file: &str) {
        std::fs::write(repo.join(file), file).unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "--quiet", "-m", file]);
    }
    /// An upstream repository and a clone of it at `into`.
    fn published(home: &Path, name: &str, into: &Path, file: &str) -> PathBuf {
        let upstream = home.join("upstream").join(name);
        std::fs::create_dir_all(&upstream).unwrap();
        git(&upstream, &["init", "--quiet"]);
        commit(&upstream, file);
        std::fs::create_dir_all(into.parent().unwrap()).unwrap();
        git(
            home,
            &[
                "clone",
                "--quiet",
                upstream.to_str().unwrap(),
                into.to_str().unwrap(),
            ],
        );
        upstream
    }

    struct Fixture {
        home: TempDir,
        core: PathBuf,
        plugin: PathBuf,
        backend: OhMyZsh<DevTools<Git>>,
        calls: Arc<Mutex<Vec<String>>>,
    }
    fn fixture() -> Fixture {
        let home = TempDir::new("oh-my-zsh");
        let root = home.path().join(".oh-my-zsh");
        let core = published(home.path(), "ohmyzsh", &root, "oh-my-zsh.sh");
        let plugin = published(
            home.path(),
            "zsh-autosuggestions",
            &root.join("custom/plugins/zsh-autosuggestions"),
            "plugin.zsh",
        );
        // A theme that is not a git checkout is not a row.
        std::fs::create_dir_all(root.join("custom/themes/handmade")).unwrap();
        let fake = Git {
            env: BTreeMap::from([("HOME", home.path().into())]),
            ..Git::default()
        };
        let calls = fake.calls.clone();
        Fixture {
            home,
            core,
            plugin,
            backend: OhMyZsh::new(DevTools(fake)),
            calls,
        }
    }
    fn ignore(_: Progress) {}

    #[test]
    fn checkouts_are_rows_and_updates_show_only_after_a_check_fetches() {
        let mut f = fixture();
        let cancel = Cancellation::default();
        assert_eq!(f.backend.detect(&cancel).unwrap(), Availability::Available);
        let rows = f.backend.installed(&cancel).unwrap();
        let names: Vec<_> = rows.iter().map(|row| row.id.name.as_str()).collect();
        assert_eq!(names, ["oh-my-zsh", "plugin/zsh-autosuggestions"]);
        assert_eq!(rows[0].display_name, "Oh My Zsh");
        assert_eq!(rows[1].display_name, "zsh-autosuggestions");
        assert!(rows
            .iter()
            .all(|row| row.update == UpdateAvailability::Current));

        commit(&f.core, "news");
        // A plain listing never fetches.
        f.backend.refresh_update_index(&cancel).unwrap();
        assert!(f
            .backend
            .installed(&cancel)
            .unwrap()
            .iter()
            .all(|row| row.update == UpdateAvailability::Current));
        assert!(!f.calls.lock().unwrap().iter().any(|c| c.contains("fetch")));

        f.backend.arm_update_check(Some(1));
        f.backend.refresh_update_index(&cancel).unwrap();
        f.backend.arm_update_check(None);
        let rows = f.backend.installed(&cancel).unwrap();
        assert_eq!(rows[0].update, UpdateAvailability::Available);
        assert_eq!(
            rows[0].candidate_version.as_deref(),
            Some(&*git(&f.core, &["rev-parse", "--short=12", "HEAD"]))
        );
        assert_eq!(rows[1].update, UpdateAvailability::Current);

        let id = rows[0].id.clone();
        f.backend
            .execute(&Operation::Upgrade(id.clone()), &cancel, &mut ignore)
            .unwrap();
        let rows = f.backend.installed(&cancel).unwrap();
        assert_eq!(rows[0].update, UpdateAvailability::Current);
        assert!(f.home.path().join(".oh-my-zsh/news").exists());
        assert!(f
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.starts_with("w:git -C ") && c.ends_with("pull --ff-only --quiet")));

        let details = f.backend.details(&id, &cancel).unwrap();
        assert!(details.description.contains("fast-forwards"));
        assert_eq!(details.homepage.as_deref(), Some(HOMEPAGE));
        assert_eq!(f.backend.search("autosugg", &cancel).unwrap().len(), 1);
    }

    #[test]
    fn local_commits_stop_an_update_instead_of_merging() {
        let mut f = fixture();
        let cancel = Cancellation::default();
        commit(&f.plugin, "upstream-change");
        let local = f
            .home
            .path()
            .join(".oh-my-zsh/custom/plugins/zsh-autosuggestions");
        commit(&local, "local-change");
        f.backend.arm_update_check(Some(1));
        f.backend.refresh_update_index(&cancel).unwrap();
        let rows = f.backend.installed(&cancel).unwrap();
        assert_eq!(rows[1].update, UpdateAvailability::Available);
        let error = f
            .backend
            .execute(
                &Operation::Upgrade(rows[1].id.clone()),
                &cancel,
                &mut ignore,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            EngineError::Execution(ExecutionError::Failed(_))
        ));
        assert!(local.join("local-change").exists());
        assert!(!local.join("upstream-change").exists());
    }

    #[test]
    fn checkouts_without_upstream_or_remote_report_what_they_can() {
        let mut f = fixture();
        assert!(f.plugin.exists());
        let cancel = Cancellation::default();
        git(
            &f.home.path().join(".oh-my-zsh"),
            &["checkout", "--quiet", "--detach"],
        );
        std::fs::remove_dir_all(f.home.path().join("upstream/zsh-autosuggestions")).unwrap();
        f.backend.arm_update_check(Some(1));
        // The missing remote fails; the core is still fetched and listed.
        assert!(matches!(
            f.backend.refresh_update_index(&cancel),
            Err(EngineError::Execution(ExecutionError::Failed(_)))
        ));
        let rows = f.backend.installed(&cancel).unwrap();
        assert_eq!(rows[0].update, UpdateAvailability::Unknown);
        assert_eq!(rows[0].candidate_version, None);
        let details = f.backend.details(&rows[0].id, &cancel).unwrap();
        assert!(details.description.contains("never updated"));
    }

    #[test]
    fn detection_and_identity_checks() {
        let cancel = Cancellation::default();
        let f = fixture();
        let mut missing = OhMyZsh::new(DevTools(Git {
            env: BTreeMap::from([
                ("HOME", f.home.path().into()),
                ("ZSH", f.home.path().join("elsewhere").into()),
            ]),
            ..Git::default()
        }));
        assert!(matches!(
            missing.detect(&cancel).unwrap(),
            Availability::Unavailable(reason) if reason.contains("elsewhere")
        ));
        let mut homeless = OhMyZsh::new(DevTools(Git::default()));
        assert!(matches!(
            homeless.detect(&cancel).unwrap(),
            Availability::Unavailable(_)
        ));
        assert_eq!(homeless.installed(&cancel), Err(EngineError::NotFound));
        for (fail, expected) in [
            (
                (|| ExecutionError::Disabled("git not found".into())) as fn() -> ExecutionError,
                Ok(Availability::Unavailable("git not found".into())),
            ),
            (|| ExecutionError::Cancelled, Err(EngineError::Cancelled)),
            (
                || ExecutionError::Invalid("broken".into()),
                Err(EngineError::Execution(ExecutionError::Invalid(
                    "broken".into(),
                ))),
            ),
        ] {
            let mut backend = OhMyZsh::new(DevTools(Git {
                env: BTreeMap::from([("HOME", f.home.path().into())]),
                fail: Some(("--version", fail)),
                ..Git::default()
            }));
            assert_eq!(backend.detect(&cancel), expected);
        }

        let mut backend = OhMyZsh::new(DevTools(Git {
            env: BTreeMap::from([
                ("HOME", f.home.path().into()),
                ("ZSH_CUSTOM", f.home.path().join("nowhere").into()),
            ]),
            ..Git::default()
        }));
        assert_eq!(backend.installed(&cancel).unwrap().len(), 1);
        assert!(backend.may_have("oh-my-zsh"));
        assert!(backend.may_have("theme/agnoster"));
        assert!(!backend.may_have("plugin/../x"));
        assert!(!backend.may_have("wget"));
        let foreign = PackageId {
            backend: "homebrew".into(),
            name: CORE.into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        };
        assert!(matches!(
            backend.details(&foreign, &cancel),
            Err(EngineError::InvalidResponse { .. })
        ));
        let gone = PackageId {
            backend: ID.into(),
            name: "plugin/gone".into(),
            ..foreign.clone()
        };
        assert_eq!(
            backend.execute(&Operation::Upgrade(gone), &cancel, &mut ignore),
            Err(EngineError::NotFound)
        );
        assert!(matches!(
            backend.execute(&Operation::Install(foreign), &cancel, &mut ignore),
            Err(EngineError::Unsupported { .. })
        ));
    }

    #[test]
    fn cancellation_stops_checks_and_updates() {
        let mut f = fixture();
        let cancel = Cancellation::default();
        let rows = f.backend.installed(&cancel).unwrap();
        cancel.cancel();
        assert_eq!(
            f.backend.execute(
                &Operation::Upgrade(rows[0].id.clone()),
                &cancel,
                &mut ignore
            ),
            Err(EngineError::Cancelled)
        );
        let fake = Git {
            env: BTreeMap::from([("HOME", f.home.path().into())]),
            fail: Some(("fetch", || ExecutionError::Cancelled)),
            ..Git::default()
        };
        let mut backend = OhMyZsh::new(DevTools(fake.clone()));
        backend.arm_update_check(Some(1));
        assert_eq!(
            backend.refresh_update_index(&Cancellation::default()),
            Err(EngineError::Cancelled)
        );
        let mut backend = OhMyZsh::new(DevTools(Git {
            fail: Some(("@{upstream}", || ExecutionError::Cancelled)),
            ..fake
        }));
        assert_eq!(
            backend.installed(&Cancellation::default()),
            Err(EngineError::Cancelled)
        );
    }
}
