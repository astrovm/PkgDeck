//! Mac App Store apps through `mas`: inventory, update checks, updates, and
//! removal.
//!
//! App Store apps keep App Store ownership. PkgDeck lists them, reads which
//! have updates, and asks mas to install those updates. mas asks for the Mac
//! password through sudo itself, which needs a terminal, and the App Store
//! account stays Apple's to handle. Removing an app moves it to the Trash
//! like Finder does; App Store apps belong to the system, so macOS asks for
//! an administrator password first. PkgDeck never installs App Store apps.
use super::mac_apps::{refused, visible, Remover};
use super::{bytes, invalid, NativeTransport, Transport};
use crate::{engine::*, package::*, process::*};
use serde::Deserialize;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

const ID: &str = "mas";
const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Installed,
    Capability::Details,
    Capability::Upgrade,
    Capability::Remove,
];
/// How often the App Store listing is read again after a removal, while
/// Spotlight catches up with the move.
const SETTLE_TRIES: usize = 10;

pub struct MacAppStore<T = NativeTransport> {
    transport: T,
    /// The person PkgDeck runs for; their Trash receives removed apps.
    uid: u32,
    settle: Duration,
}

/// One `mas list --json` or `mas outdated --json` record. mas prints one
/// JSON object per line; `newVersion` is only present in outdated records.
#[derive(Deserialize, Clone, Debug)]
struct MasApp {
    #[serde(rename = "adamID")]
    adam_id: u64,
    #[serde(rename = "bundleID", default)]
    bundle_id: Option<String>,
    name: String,
    version: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(rename = "newVersion", default)]
    new_version: Option<String>,
}

fn records(output: &[u8]) -> Result<Vec<MasApp>, EngineError> {
    serde_json::Deserializer::from_slice(output)
        .into_iter::<MasApp>()
        .collect::<Result<_, _>>()
        .map_err(|error| invalid(ID, error))
}

/// mas 7 added the JSON output this adapter reads.
#[cfg(any(target_os = "macos", test))]
fn supported_version(text: &str) -> bool {
    text.trim()
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .is_some_and(|major| major >= 7)
}

/// sudo, run by mas for installs and updates, needs a terminal to ask for
/// the password and fails without one or without a password.
fn needs_password(result: &Completion) -> bool {
    let text = String::from_utf8_lossy(&result.stderr);
    text.contains("sudo:")
        && (text.contains("terminal is required") || text.contains("password is required"))
}

/// Rows are addressed by App Store ID; an update can rename an app.
fn owns(id: &PackageId, app: &MasApp) -> bool {
    id.backend == ID && id.reference.as_deref() == Some(app.adam_id.to_string().as_str())
}

impl<T: Transport> MacAppStore<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            uid: rustix::process::getuid().as_raw(),
            settle: Duration::from_secs(1),
        }
    }

    fn run(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Vec<u8>, EngineError> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        match self.transport.dev_tool(ID, &args, cancel, write) {
            Err(ExecutionError::Failed(result)) if needs_password(&result) => {
                Err(ExecutionError::AuthorizationDenied.into())
            }
            result => bytes(ID, result?),
        }
    }

    fn list(&self, cancel: &Cancellation) -> Result<Vec<MasApp>, EngineError> {
        records(&self.run(&["list", "--json"], cancel, false)?)
    }

    /// The default check compares installed versions with the App Store's
    /// published ones. It reads Apple's catalog over the network but never
    /// starts a download (that is `--accurate`, which can open dialogs).
    fn outdated(&self, cancel: &Cancellation) -> Result<Vec<MasApp>, EngineError> {
        records(&self.run(&["outdated", "--json", "--inaccurate"], cancel, false)?)
    }

    fn package(app: &MasApp, candidate: Option<Option<&str>>) -> Package {
        // macOS reports apps at their real volume path; people know
        // /Applications.
        let path = app.path.as_deref().map(|path| {
            path.strip_prefix("/System/Volumes/Data")
                .filter(|rest| rest.starts_with('/'))
                .unwrap_or(path)
        });
        let path = path.unwrap_or("unknown location");
        Package {
            id: PackageId {
                backend: ID.into(),
                // People know apps by name; the App Store ID also selects one.
                name: app.name.clone(),
                architecture: "unknown".into(),
                scope: Scope::System,
                remote: None,
                reference: Some(app.adam_id.to_string()),
            },
            display_name: app.name.clone(),
            summary: format!("App Store app · {path}"),
            installed_version: Some(app.version.clone()),
            // `None`: the check did not run; `Some(None)`: nothing newer.
            update: match candidate {
                Some(Some(_)) => UpdateAvailability::Available,
                Some(None) => UpdateAvailability::Current,
                None => UpdateAvailability::Unknown,
            },
            candidate_version: candidate.flatten().map(Into::into),
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        }
    }

    fn inventory(&self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let installed = self.list(cancel)?;
        let outdated = self.outdated(cancel)?;
        Ok(installed
            .iter()
            .map(|app| {
                let candidate = outdated
                    .iter()
                    .find(|update| update.adam_id == app.adam_id)
                    .and_then(|update| update.new_version.as_deref());
                Self::package(app, Some(candidate))
            })
            .collect())
    }

    fn find(&self, id: &PackageId, cancel: &Cancellation) -> Result<MasApp, EngineError> {
        self.list(cancel)?
            .into_iter()
            .find(|app| owns(id, app))
            .ok_or(EngineError::NotFound)
    }

    fn update(
        &self,
        ids: &[PackageId],
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Vec<OperationOutcome>, EngineError> {
        let installed = self.list(cancel)?;
        let mut apps = Vec::new();
        for id in ids {
            let app = installed
                .iter()
                .find(|app| owns(id, app))
                .ok_or(EngineError::NotFound)?;
            apps.push(app.clone());
        }
        let pending: Vec<MasApp> = {
            let outdated = self.outdated(cancel)?;
            apps.iter()
                .filter(|app| outdated.iter().any(|update| update.adam_id == app.adam_id))
                .cloned()
                .collect()
        };
        if pending.is_empty() {
            return Ok(vec![OperationOutcome::default(); ids.len()]);
        }
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let names: Vec<&str> = pending.iter().map(|app| app.name.as_str()).collect();
        progress(Progress::Message(format!(
            "Updating {} from the App Store",
            names.join(", ")
        )));
        let adam_ids: Vec<String> = pending.iter().map(|app| app.adam_id.to_string()).collect();
        let mut args = vec!["update"];
        args.extend(adam_ids.iter().map(String::as_str));
        let args: Vec<OsString> = args.into_iter().map(OsString::from).collect();
        let completion = match self.transport.dev_tool(ID, &args, cancel, true) {
            Err(ExecutionError::Failed(result)) if needs_password(&result) => {
                // Without a terminal mas can't ask for the password. The App
                // Store app can: open its Updates page so the update is one
                // click away.
                let _ = self.transport.dev_tool(
                    "open",
                    &["macappstore://showUpdatesPage".into()],
                    &Cancellation::default(),
                    false,
                );
                return Err(ExecutionError::AuthorizationDenied.into());
            }
            result => result?,
        };
        let deferred = completion.cancellation_deferred;
        let output = bytes(ID, completion)?;
        if !output.is_empty() {
            progress(Progress::Message(String::from_utf8_lossy(&output).into()));
        }
        // mas reports some per-app failures without failing the command.
        // Native writes may complete after cancellation; verification still runs.
        let after = self.list(&Cancellation::default())?;
        let stale: Vec<&str> = pending
            .iter()
            .filter(|app| {
                after
                    .iter()
                    .find(|now| now.adam_id == app.adam_id)
                    .is_none_or(|now| now.version == app.version)
            })
            .map(|app| app.name.as_str())
            .collect();
        if !stale.is_empty() {
            return Err(invalid(
                ID,
                format!(
                    "the App Store did not install an update for {}; check that you are signed in to the App Store",
                    stale.join(", ")
                ),
            ));
        }
        Ok(vec![
            OperationOutcome {
                cancellation_deferred: deferred,
            };
            ids.len()
        ])
    }

    /// Moves the selected app to the Trash. The App Store's own listing names
    /// the bundle, and its receipt must still be there; afterwards the listing
    /// must no longer show the app where it was.
    fn remove(
        &self,
        id: &PackageId,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let app = self.find(id, cancel)?;
        let path = app
            .path
            .as_deref()
            .map(|path| visible(std::path::Path::new(path)))
            .filter(|path| path.is_absolute() && path.extension().is_some_and(|ext| ext == "app"))
            .ok_or_else(|| {
                refused(format!(
                    "mas doesn't say where {} is installed, so PkgDeck can't remove it.",
                    app.name
                ))
            })?;
        if !path.join("Contents/_MASReceipt/receipt").is_file() {
            return Err(refused(format!(
                "{} has no App Store receipt, so it may not be {} from the App Store. Reload, then try again.",
                path.display(),
                app.name
            )));
        }
        let remover = Remover {
            transport: &self.transport,
            uid: self.uid,
            backend: ID,
        };
        remover.check(&app.name, &path, cancel)?;
        let outcome = remover.trash(&app.name, &path, cancel, progress)?;
        // Spotlight, which mas reads, notices the move a moment later.
        let listed_here = |now: &MasApp| {
            now.adam_id == app.adam_id
                && now
                    .path
                    .as_deref()
                    .map(|p| visible(std::path::Path::new(p)))
                    == Some(PathBuf::from(&path))
        };
        for attempt in 0..SETTLE_TRIES {
            if attempt > 0 {
                std::thread::sleep(self.settle);
            }
            if !self.list(&Cancellation::default())?.iter().any(listed_here) {
                return Ok(outcome);
            }
        }
        Err(refused(format!(
            "{} is in the Trash, but the App Store still lists it at {}. Reload in a moment to check.",
            app.name,
            path.display()
        )))
    }
}

impl<T: Transport> Backend for MacAppStore<T> {
    fn id(&self) -> &str {
        ID
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    #[cfg(not(target_os = "macos"))]
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Unavailable(
            "The Mac App Store requires macOS".into(),
        ))
    }
    #[cfg(target_os = "macos")]
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        let version = match self.run(&["version"], cancel, false) {
            Ok(output) => String::from_utf8_lossy(&output).into_owned(),
            Err(EngineError::Execution(ExecutionError::Disabled(reason))) => {
                return Ok(Availability::Unavailable(reason))
            }
            Err(error) => return Err(error),
        };
        Ok(if supported_version(&version) {
            Availability::Available
        } else {
            Availability::Unavailable(format!(
                "mas {} is too old; PkgDeck needs mas 7 or newer",
                version.trim()
            ))
        })
    }
    fn may_have(&self, name: &str) -> bool {
        !name.is_empty()
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        // Only installed apps: the App Store installs apps an account got,
        // which is Apple's flow, so search never offers an install.
        let query = query.to_lowercase();
        Ok(self
            .list(cancel)?
            .iter()
            .filter(|app| {
                app.name.to_lowercase().contains(&query)
                    || app.adam_id.to_string() == query
                    || app
                        .bundle_id
                        .as_deref()
                        .is_some_and(|bundle| bundle.to_lowercase() == query)
            })
            .map(|app| Self::package(app, None))
            .collect())
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.inventory(cancel)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let app = self.find(id, cancel)?;
        let mut description = vec![
            format!(
                "Location: {}",
                app.path.as_deref().unwrap_or("unknown location")
            ),
            format!("App Store ID: {}", app.adam_id),
        ];
        if let Some(bundle) = &app.bundle_id {
            description.push(format!("Bundle identifier: {bundle}"));
        }
        description.push(
            "Updates come from the App Store through mas and need the Mac password. \
             Removing it moves it to the Trash; macOS asks for an administrator password first. \
             PkgDeck does not install App Store apps."
                .into(),
        );
        Ok(PackageDetails {
            package: Self::package(&app, None),
            description: description.join("\n\n"),
            homepage: Some(format!("https://apps.apple.com/app/id{}", app.adam_id)),
            dependencies: vec![],
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        match operation {
            Operation::Upgrade(id) => Ok(self
                .update(std::slice::from_ref(id), cancel, progress)?
                .remove(0)),
            Operation::Remove(id) => self.remove(id, cancel, progress),
            _ => Err(self.unsupported(operation.capability())),
        }
    }
    /// One mas run updates every selected app, so sudo asks once.
    fn execute_group(
        &mut self,
        operations: &[Operation],
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Option<Result<Vec<OperationOutcome>, EngineError>> {
        let ids: Vec<PackageId> = operations
            .iter()
            .map(|operation| match operation {
                Operation::Upgrade(id) => Some(id.clone()),
                _ => None,
            })
            .collect::<Option<_>>()?;
        Some(self.update(&ids, cancel, progress))
    }
}

#[cfg(test)]
mod tests {
    use super::super::mac_apps::removal_fakes::{Mover, Privileged};
    use super::*;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    /// Answers `mas` with canned output and records every command it ran.
    #[derive(Clone)]
    struct Fake {
        list: Arc<Mutex<Vec<String>>>,
        outdated: Arc<Mutex<Vec<String>>>,
        calls: Arc<Mutex<Vec<String>>>,
        update_error: Option<&'static str>,
        update_installs: bool,
        /// What `mas version` prints, or how it fails.
        version: Result<&'static str, fn() -> ExecutionError>,
        /// stderr of a failing `mas list`.
        list_error: Option<&'static str>,
        /// The Trash, the password prompt and `ps`.
        mover: Mover,
        /// Spotlight still lists apps that moved away.
        stale: bool,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                list: Arc::default(),
                outdated: Arc::default(),
                calls: Arc::default(),
                update_error: None,
                update_installs: false,
                version: Ok("7.0.0\n"),
                list_error: None,
                mover: Mover::new(std::env::temp_dir().join("pkgdeck-mas-no-home")),
                stale: false,
            }
        }
    }
    fn failed(stderr: &str) -> ExecutionError {
        let mut result = done(String::new());
        result.code = Some(1);
        result.stderr = stderr.as_bytes().to_vec();
        ExecutionError::Failed(result)
    }
    fn done(stdout: String) -> Completion {
        Completion {
            code: Some(0),
            signal: None,
            stdout: stdout.into_bytes(),
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
        }
    }
    impl Transport for Fake {
        fn dev_tool(
            &self,
            executable: &str,
            args: &[OsString],
            _: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            if executable == "open" {
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("open {}", args.join(" ")));
                return Ok(done(String::new()));
            }
            assert_eq!(executable, "mas");
            self.calls.lock().unwrap().push(args.join(" "));
            match args[0].as_str() {
                "version" => self.version.map(|text| done(text.into())).map_err(|e| e()),
                "list" => match self.list_error {
                    Some(stderr) => Err(failed(stderr)),
                    // Fixture apps that left their folder drop out, as
                    // Spotlight eventually notices.
                    None => Ok(done(
                        self.list
                            .lock()
                            .unwrap()
                            .iter()
                            .filter(|line| {
                                let record: serde_json::Value = serde_json::from_str(line).unwrap();
                                let path =
                                    visible(Path::new(record["path"].as_str().unwrap_or("/")));
                                self.stale
                                    || !path.starts_with(std::env::temp_dir())
                                    || path.exists()
                            })
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )),
                },
                "outdated" => Ok(done(self.outdated.lock().unwrap().join("\n"))),
                _ => {
                    assert_eq!(args[0], "update");
                    assert!(write);
                    if let Some(stderr) = self.update_error {
                        return Err(failed(stderr));
                    }
                    if !self.update_installs {
                        return Ok(done(String::new()));
                    }
                    *self.list.lock().unwrap() =
                        vec![app(1, "Alpha", "2.0"), app(2, "Beta", "1.0")];
                    self.outdated.lock().unwrap().clear();
                    Ok(done("==> Installed Alpha (2.0)\n".into()))
                }
            }
        }
        fn env(&self, name: &str) -> Option<OsString> {
            self.mover.env(name)
        }
        fn macos_tool(
            &self,
            executable: &Path,
            args: &[OsString],
            cancel: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            self.mover.macos_tool(executable, args, cancel, write)
        }
        fn system_manager(
            &self,
            executable: &str,
            args: &[OsString],
            cancel: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            self.mover.system_manager(executable, args, cancel, write)
        }
    }

    /// An App Store app in a temporary Applications folder, with its receipt.
    struct Installed {
        root: PathBuf,
        app: PathBuf,
    }
    impl Installed {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "pkgdeck-mas-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let app = root.join("Applications/Alpha.app");
            std::fs::create_dir_all(app.join("Contents/_MASReceipt")).unwrap();
            std::fs::write(app.join("Contents/_MASReceipt/receipt"), "receipt").unwrap();
            std::fs::create_dir_all(root.join("home")).unwrap();
            Self { root, app }
        }
        /// A store whose listing names this app; root owns App Store apps.
        fn store(&self, path: &str) -> (MacAppStore<Fake>, Fake) {
            let mut fake = fake();
            *fake.list.lock().unwrap() = vec![app(1, "Alpha", "1.0").replace(
                "/Applications/Alpha.app",
                &path.replace("{app}", self.app.to_str().unwrap()),
            )];
            fake.mover = Mover::new(self.root.join("home"));
            let mut store = MacAppStore::new(fake.clone());
            store.uid = rustix::process::getuid().as_raw() + 1;
            store.settle = Duration::ZERO;
            (store, fake)
        }
    }
    impl Drop for Installed {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn remove(store: &mut MacAppStore<Fake>) -> Result<OperationOutcome, EngineError> {
        store.execute(
            &Operation::Remove(id("1")),
            &Cancellation::default(),
            &mut drop::<Progress>,
        )
    }

    #[test]
    fn removal_moves_the_app_to_the_trash_after_the_password_prompt() {
        let installed = Installed::new();
        let (mut store, fake) = installed.store("{app}");
        let mut messages = vec![];
        store
            .execute(
                &Operation::Remove(id("1")),
                &Cancellation::default(),
                &mut |progress| {
                    if let Progress::Message(text) = progress {
                        messages.push(text);
                    }
                },
            )
            .unwrap();
        let trashed = installed.root.join("home/.Trash/Alpha.app");
        assert!(!installed.app.exists());
        assert!(trashed.join("Contents/_MASReceipt/receipt").is_file());
        assert!(messages[0].starts_with("Moving Alpha to the Trash."));
        assert_eq!(
            fake.mover.calls(),
            [
                "/bin/ps -axww -o comm=".to_owned(),
                format!("mv -n -- {} {}", installed.app.display(), trashed.display())
            ]
        );
        // mas never removes anything itself.
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("uninstall")));
        // Removed apps are no longer there to remove.
        assert!(matches!(remove(&mut store), Err(EngineError::NotFound)));
    }

    #[test]
    fn an_app_store_app_you_own_goes_through_the_trash_command() {
        let installed = Installed::new();
        // mas reports apps through the Data volume's firmlink.
        let (mut store, fake) = installed.store("/System/Volumes/Data{app}");
        store.uid = rustix::process::getuid().as_raw();
        remove(&mut store).unwrap();
        assert_eq!(
            fake.mover.calls().last().unwrap(),
            &format!("/usr/bin/trash -s {}", installed.app.display())
        );
        assert!(!installed.app.exists());
        assert!(installed.root.join("home/.Trash/Alpha.app").is_dir());
    }

    #[test]
    fn a_listing_that_keeps_the_app_is_reported_after_retrying() {
        let installed = Installed::new();
        let (mut store, mut fake) = installed.store("{app}");
        fake.stale = true;
        store.transport = fake.clone();
        let error = remove(&mut store).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Alpha is in the Trash, but the App Store still lists it"),
            "{error}"
        );
        // One listing to find the app, then every retry.
        let lists = fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.starts_with("list"))
            .count();
        assert_eq!(lists, 1 + SETTLE_TRIES);
    }

    #[test]
    fn removal_refusals_leave_the_app_in_place() {
        let installed = Installed::new();
        let refusal = |path: &str, change: &dyn Fn(&mut MacAppStore<Fake>)| {
            let (mut store, _) = installed.store(path);
            change(&mut store);
            let error = remove(&mut store).unwrap_err();
            assert!(installed.app.is_dir());
            error
        };
        // The password dialog's Cancel button.
        let error = refusal("{app}", &|store| {
            store.transport.mover.privileged = Privileged::Cancel;
        });
        assert!(matches!(
            error,
            EngineError::Execution(ExecutionError::AuthorizationCancelled)
        ));
        let error = refusal("{app}", &|store| {
            store.transport.mover.privileged = Privileged::Denied;
        });
        assert!(matches!(
            error,
            EngineError::Execution(ExecutionError::AuthorizationDenied)
        ));
        let running = format!("{}/Contents/MacOS/Alpha\n", installed.app.display());
        let error = refusal("{app}", &|store| {
            store.transport.mover.ps = running.clone();
        });
        assert_eq!(
            error.to_string(),
            "Alpha is open. Quit it, then remove it again."
        );
        let error = refusal("{app}", &|store| store.uid = 0);
        assert_eq!(error.to_string(), crate::host::ROOT_REFUSAL);
        let error = refusal("relative/Alpha.app", &|_| {});
        assert!(error
            .to_string()
            .contains("doesn't say where Alpha is installed"));
        let error = refusal("{app}/Contents", &|_| {});
        assert!(error
            .to_string()
            .contains("doesn't say where Alpha is installed"));
        let error = refusal("{app}", &|store| {
            store.transport.list.lock().unwrap()[0] =
                r#"{"adamID":1,"name":"Alpha","version":"1.0"}"#.into();
        });
        assert!(error
            .to_string()
            .contains("doesn't say where Alpha is installed"));
        // Without the receipt this may be some other app at that path.
        let copy = installed.root.join("Applications/Copy.app");
        std::fs::create_dir_all(&copy).unwrap();
        let error = refusal(copy.to_str().unwrap(), &|_| {});
        assert!(
            error.to_string().contains("has no App Store receipt"),
            "{error}"
        );
        assert!(copy.is_dir());
    }
    fn app(id: u64, name: &str, version: &str) -> String {
        format!(
            r#"{{"adamID":{id},"bundleID":"com.example.{name}","name":"{name}","version":"{version}","path":"/Applications/{name}.app","category":"Utilities"}}"#
        )
    }
    fn fake() -> Fake {
        let fake = Fake::default();
        *fake.list.lock().unwrap() = vec![app(1, "Alpha", "1.0"), app(2, "Beta", "1.0")];
        *fake.outdated.lock().unwrap() = vec![app(1, "Alpha", "1.0").replace(
            r#""version":"1.0""#,
            r#""newVersion":"2.0","version":"1.0""#,
        )];
        fake
    }
    fn id(adam_id: &str) -> PackageId {
        PackageId {
            backend: ID.into(),
            name: "renamed".into(),
            architecture: "unknown".into(),
            scope: Scope::System,
            remote: None,
            reference: Some(adam_id.into()),
        }
    }

    #[test]
    fn summary_shows_the_path_people_know() {
        let fake = fake();
        fake.list.lock().unwrap()[0] = app(1, "Alpha", "1.0").replace(
            "/Applications/Alpha.app",
            "/System/Volumes/Data/Applications/Alpha.app",
        );
        let rows = MacAppStore::new(fake)
            .installed(&Cancellation::default())
            .unwrap();
        assert_eq!(rows[0].summary, "App Store app · /Applications/Alpha.app");
    }

    #[test]
    fn inventory_reads_updates_from_the_app_store_check() {
        let fake = fake();
        let mut store = MacAppStore::new(fake.clone());
        let rows = store.installed(&Cancellation::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].display_name, "Alpha");
        assert_eq!(rows[0].id.name, "Alpha");
        assert_eq!(rows[0].id.reference.as_deref(), Some("1"));
        assert_eq!(rows[0].update, UpdateAvailability::Available);
        assert_eq!(rows[0].candidate_version.as_deref(), Some("2.0"));
        assert_eq!(rows[1].update, UpdateAvailability::Current);
        // The check never starts downloads.
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"outdated --json --inaccurate".into()));
        // Search filters installed apps and never runs the update check.
        fake.calls.lock().unwrap().clear();
        let hits = store.search("bet", &Cancellation::default()).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].update, UpdateAvailability::Unknown);
        assert_eq!(*fake.calls.lock().unwrap(), vec!["list --json"]);
        assert_eq!(
            store
                .search("com.example.alpha", &Cancellation::default())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store.search("2", &Cancellation::default()).unwrap()[0]
                .id
                .name,
            "Beta"
        );
        assert!(store
            .search("gamma", &Cancellation::default())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn updates_run_once_for_every_outdated_selection_and_verify() {
        let mut fake = fake();
        fake.update_installs = true;
        let mut store = MacAppStore::new(fake.clone());
        let operations = [Operation::Upgrade(id("1")), Operation::Upgrade(id("2"))];
        let mut messages = vec![];
        let outcomes = store
            .execute_group(&operations, &Cancellation::default(), &mut |progress| {
                if let Progress::Message(text) = progress {
                    messages.push(text);
                }
            })
            .unwrap()
            .unwrap();
        assert_eq!(outcomes.len(), 2);
        // mas's own output is shown as it reports it.
        assert_eq!(
            messages,
            [
                "Updating Alpha from the App Store",
                "==> Installed Alpha (2.0)\n"
            ]
        );
        // Beta is current, so only Alpha is passed to mas.
        let calls = fake.calls.lock().unwrap().clone();
        assert_eq!(
            calls
                .iter()
                .filter(|c| c.starts_with("update"))
                .collect::<Vec<_>>(),
            vec!["update 1"]
        );
        // Nothing to do runs nothing.
        fake.calls.lock().unwrap().clear();
        store
            .execute(
                &Operation::Upgrade(id("1")),
                &Cancellation::default(),
                &mut drop::<Progress>,
            )
            .unwrap();
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.starts_with("update")));
        // Unknown apps and other operations are refused.
        assert!(matches!(
            store.execute(
                &Operation::Upgrade(id("9")),
                &Cancellation::default(),
                &mut drop::<Progress>
            ),
            Err(EngineError::NotFound)
        ));
        assert!(matches!(
            store.execute(
                &Operation::Install(id("1")),
                &Cancellation::default(),
                &mut drop::<Progress>
            ),
            Err(EngineError::Unsupported { .. })
        ));
    }

    #[test]
    fn an_update_that_installs_nothing_is_an_error() {
        let fake = fake();
        let mut store = MacAppStore::new(fake);
        let error = store
            .execute(
                &Operation::Upgrade(id("1")),
                &Cancellation::default(),
                &mut drop::<Progress>,
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("did not install an update for Alpha"),
            "{error}"
        );
    }

    #[test]
    fn a_missing_password_prompt_is_an_authorization_failure() {
        let mut fake = fake();
        fake.update_error = Some("sudo: a terminal is required to read the password; either use the -S option to read from standard input or configure an askpass helper\nsudo: a password is required\n");
        let mut store = MacAppStore::new(fake.clone());
        assert!(matches!(
            store.execute(
                &Operation::Upgrade(id("1")),
                &Cancellation::default(),
                &mut drop::<Progress>
            ),
            Err(EngineError::Execution(ExecutionError::AuthorizationDenied))
        ));
        // The App Store app can ask for the password, so its Updates page opens.
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"open macappstore://showUpdatesPage".into()));
    }

    #[test]
    fn only_mas_seven_is_supported() {
        assert!(supported_version("7.0.0\n"));
        assert!(supported_version("12.1"));
        assert!(!supported_version("6.9.0"));
        assert!(!supported_version("garbage"));
    }

    #[test]
    fn the_app_store_is_only_offered_on_macos() {
        let availability = MacAppStore::new(fake())
            .detect(&Cancellation::default())
            .unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(availability, Availability::Available);
        } else {
            assert_eq!(
                availability,
                Availability::Unavailable("The Mac App Store requires macOS".into())
            );
        }
    }

    #[test]
    fn details_link_the_store_page_and_never_offer_installs() {
        let mut store = MacAppStore::new(fake());
        // Any app name can be an installed App Store app.
        assert!(store.may_have("Final Cut Pro") && !store.may_have(""));
        // Updates and removals; installs stay with the App Store.
        assert_eq!(store.capabilities(), CAPABILITIES);
        let details = store.details(&id("2"), &Cancellation::default()).unwrap();
        assert_eq!(details.package.display_name, "Beta");
        assert_eq!(
            details.homepage.as_deref(),
            Some("https://apps.apple.com/app/id2")
        );
        assert!(details
            .description
            .starts_with("Location: /Applications/Beta.app\n\nApp Store ID: 2\n\nBundle identifier: com.example.Beta"));
        assert!(details.description.contains("moves it to the Trash"));
        assert!(details
            .description
            .contains("does not install App Store apps"));
        let mut foreign = id("2");
        foreign.backend = "homebrew-cask".into();
        assert!(matches!(
            store.details(&foreign, &Cancellation::default()),
            Err(EngineError::NotFound)
        ));
    }

    #[test]
    fn groups_take_only_updates_and_cancellation_stops_before_mas_runs() {
        let fake = fake();
        let mut store = MacAppStore::new(fake.clone());
        let mixed = [Operation::Upgrade(id("1")), Operation::Remove(id("2"))];
        assert!(store
            .execute_group(&mixed, &Cancellation::default(), &mut drop::<Progress>)
            .is_none());
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            store.execute(&Operation::Upgrade(id("1")), &cancel, &mut drop::<Progress>),
            Err(EngineError::Cancelled)
        ));
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("update")));
    }

    #[test]
    fn a_password_prompt_while_listing_is_an_authorization_failure() {
        let mut fake = fake();
        fake.list_error = Some("sudo: a password is required\n");
        let mut store = MacAppStore::new(fake.clone());
        assert!(matches!(
            store.installed(&Cancellation::default()),
            Err(EngineError::Execution(ExecutionError::AuthorizationDenied))
        ));
        // Any other failure is reported as it is.
        fake.list_error = Some("mas: not signed in\n");
        let mut store = MacAppStore::new(fake);
        assert!(matches!(
            store.installed(&Cancellation::default()),
            Err(EngineError::Execution(ExecutionError::Failed(_)))
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detection_needs_mas_seven() {
        let detect = |version: Result<&'static str, fn() -> ExecutionError>| {
            MacAppStore::new(Fake { version, ..fake() }).detect(&Cancellation::default())
        };
        assert_eq!(
            detect(Ok("6.3.0\n")).unwrap(),
            Availability::Unavailable("mas 6.3.0 is too old; PkgDeck needs mas 7 or newer".into())
        );
        assert_eq!(
            detect(Err(|| ExecutionError::Disabled("mas not found".into()))).unwrap(),
            Availability::Unavailable("mas not found".into())
        );
        assert!(matches!(
            detect(Err(|| ExecutionError::TimedOut)),
            Err(EngineError::Execution(ExecutionError::TimedOut))
        ));
    }
}
