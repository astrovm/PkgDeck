//! Mac App Store apps through `mas`: inventory, update checks, and updates.
//!
//! App Store apps keep App Store ownership. PkgDeck lists them, reads which
//! have updates, and asks mas to install those updates; it never installs,
//! removes, or moves them. mas asks for the Mac password through sudo itself,
//! which needs a terminal, and the App Store account stays Apple's to handle.
use super::{bytes, invalid, NativeTransport, Transport};
use crate::{engine::*, package::*, process::*};
use serde::Deserialize;
use std::ffi::OsString;

const ID: &str = "mas";
const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Installed,
    Capability::Details,
    Capability::Upgrade,
];

pub struct MacAppStore<T = NativeTransport> {
    transport: T,
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
        Self { transport }
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
        let path = app.path.as_deref().unwrap_or("unknown location");
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

    fn inventory(&self, check: bool, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let installed = self.list(cancel)?;
        let outdated = if check {
            Some(self.outdated(cancel)?)
        } else {
            None
        };
        Ok(installed
            .iter()
            .map(|app| {
                let candidate = outdated.as_ref().map(|outdated| {
                    outdated
                        .iter()
                        .find(|update| update.adam_id == app.adam_id)
                        .and_then(|update| update.new_version.as_deref())
                });
                Self::package(app, candidate)
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
        self.inventory(true, cancel)
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
             PkgDeck does not install or remove App Store apps."
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
        let Operation::Upgrade(id) = operation else {
            return Err(self.unsupported(operation.capability()));
        };
        Ok(self
            .update(std::slice::from_ref(id), cancel, progress)?
            .remove(0))
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
    use super::*;
    use crate::host::AptAction;
    use std::sync::{Arc, Mutex};

    /// Answers `mas` with canned output and records every command it ran.
    #[derive(Clone, Default)]
    struct Fake {
        list: Arc<Mutex<Vec<String>>>,
        outdated: Arc<Mutex<Vec<String>>>,
        calls: Arc<Mutex<Vec<String>>>,
        update_error: Option<&'static str>,
        update_installs: bool,
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
        fn apt_query(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &Cancellation,
        ) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
        fn apt_write(&self, _: AptAction, _: &Cancellation) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
        fn brew(
            &self,
            _: &[OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
        fn flatpak(
            &self,
            _: &[OsString],
            _: &Cancellation,
            _: bool,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
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
                "version" => Ok(done("7.0.0\n".into())),
                "list" => Ok(done(self.list.lock().unwrap().join("\n"))),
                "outdated" => Ok(done(self.outdated.lock().unwrap().join("\n"))),
                "update" => {
                    assert!(write);
                    if let Some(stderr) = self.update_error {
                        let mut result = done(String::new());
                        result.code = Some(1);
                        result.stderr = stderr.as_bytes().to_vec();
                        return Err(ExecutionError::Failed(result));
                    }
                    if self.update_installs {
                        *self.list.lock().unwrap() =
                            vec![app(1, "Alpha", "2.0"), app(2, "Beta", "1.0")];
                        self.outdated.lock().unwrap().clear();
                    }
                    Ok(done(String::new()))
                }
                other => panic!("unexpected mas {other}"),
            }
        }
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
        let outcomes = store
            .execute_group(&operations, &Cancellation::default(), &mut |_| {})
            .unwrap()
            .unwrap();
        assert_eq!(outcomes.len(), 2);
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
                &mut |_| {},
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
                &mut |_| {}
            ),
            Err(EngineError::NotFound)
        ));
        assert!(matches!(
            store.execute(
                &Operation::Remove(id("1")),
                &Cancellation::default(),
                &mut |_| {}
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
                &mut |_| {},
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
                &mut |_| {}
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
        let details = store.details(&id("2"), &Cancellation::default()).unwrap();
        assert_eq!(details.package.display_name, "Beta");
        assert_eq!(
            details.homepage.as_deref(),
            Some("https://apps.apple.com/app/id2")
        );
        assert!(details
            .description
            .starts_with("Location: /Applications/Beta.app\n\nApp Store ID: 2\n\nBundle identifier: com.example.Beta"));
        assert!(details.description.contains("does not install or remove"));
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
            .execute_group(&mixed, &Cancellation::default(), &mut |_| {})
            .is_none());
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            store.execute(&Operation::Upgrade(id("1")), &cancel, &mut |_| {}),
            Err(EngineError::Cancelled)
        ));
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("update")));
    }
}
