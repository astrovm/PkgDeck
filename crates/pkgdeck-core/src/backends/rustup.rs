//! Rust toolchains managed by rustup, apart from the Cargo-installed tools.
//!
//! Each installed toolchain is a row: channels (stable, beta, nightly) update
//! within the channel, and pinned versions never do. Toolchain updates pass
//! `--no-self-update`; rustup's own update is a separate row.
use super::{bytes, invalid, NativeTransport, Transport};
use crate::{engine::*, package::*, process::*};
use std::{collections::BTreeMap, ffi::OsString};

const ID: &str = "rustup";
const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Upgrade,
];
/// The row for rustup itself.
const SELF: &str = "rustup";

pub struct Rustup<T = NativeTransport> {
    transport: T,
}

struct Toolchain {
    name: String,
    default: bool,
}

/// What `rustup check` says about one toolchain (or rustup itself).
#[derive(Debug, PartialEq)]
struct Check {
    installed: String,
    candidate: Option<String>,
}

/// Toolchain names rustup accepts: channels, versions, dates and targets.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// `1.98.1 (48a229cea 2026-09-01)` → `1.98.1`; `(unknown version)` → `unknown`.
fn version(text: &str) -> String {
    let text = text.trim();
    if text.starts_with('(') {
        return "unknown".into();
    }
    text.split_whitespace().next().unwrap_or(text).to_owned()
}

/// `stable-aarch64-apple-darwin - Update available : 1.89.0 (…) -> 1.98.1 (…)`
/// `beta-aarch64-apple-darwin - Up to date : 1.99.0-beta.2 (…)`
fn parse_check(text: &str) -> BTreeMap<String, Check> {
    text.lines()
        .filter_map(|line| {
            let (name, status) = line.split_once(" - ")?;
            // rustup 1.28 prints "Update available : …"; 1.29 "up to date: …".
            let (state, versions) = status.split_once(':')?;
            let check = if state.trim().eq_ignore_ascii_case("update available") {
                let (from, to) = versions.split_once(" -> ")?;
                Check {
                    installed: version(from),
                    candidate: Some(version(to)),
                }
            } else {
                Check {
                    installed: version(versions),
                    candidate: None,
                }
            };
            Some((name.trim().to_owned(), check))
        })
        .collect()
}

impl<T: Transport> Rustup<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    fn run(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<String, EngineError> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let output = match self.transport.dev_tool(ID, &args, cancel, write) {
            // `rustup check` exits 100 when updates exist.
            Err(ExecutionError::Failed(result)) if result.code == Some(100) => result.stdout,
            result => bytes(ID, result?)?,
        };
        String::from_utf8(output).map_err(|error| invalid(ID, error))
    }

    fn toolchains(&self, cancel: &Cancellation) -> Result<Vec<Toolchain>, EngineError> {
        Ok(self
            .run(&["toolchain", "list"], cancel, false)?
            .lines()
            .filter_map(|line| {
                let name = line.split_whitespace().next()?;
                (valid_name(name) && name != "no").then(|| Toolchain {
                    name: name.to_owned(),
                    // "(active)" depends on the directory rustup runs in, so
                    // only the default is reported.
                    default: line.contains("default"),
                })
            })
            .collect())
    }

    fn package(name: &str, summary: String, check: Option<&Check>) -> Package {
        Package {
            id: PackageId {
                backend: ID.into(),
                name: name.into(),
                architecture: "unknown".into(),
                scope: Scope::User {
                    uid: rustix::process::getuid().as_raw(),
                },
                remote: None,
                reference: None,
            },
            display_name: name.into(),
            summary,
            installed_version: Some(
                check.map_or_else(|| "installed".into(), |c| c.installed.clone()),
            ),
            update: match check {
                Some(Check {
                    candidate: Some(_), ..
                }) => UpdateAvailability::Available,
                Some(_) => UpdateAvailability::Current,
                None => UpdateAvailability::Unknown,
            },
            candidate_version: check.and_then(|c| c.candidate.clone()),
            icon: None,
            component_ids: vec![],
            homepages: vec!["https://rust-lang.github.io/rustup/".into()],
        }
    }

    fn rows(&self, check: bool, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let checks = if check {
            parse_check(&self.run(&["check"], cancel, false)?)
        } else {
            BTreeMap::new()
        };
        let mut rows: Vec<Package> = self
            .toolchains(cancel)?
            .iter()
            .map(|toolchain| {
                let mut notes = vec!["Rust toolchain"];
                if toolchain.default {
                    notes.push("default");
                }
                // A pinned version is never updated, so check skips it.
                let pinned = toolchain.name.starts_with(|c: char| c.is_ascii_digit());
                let found = checks.get(&toolchain.name);
                let pinned_check;
                let check = match (found, pinned && check) {
                    (Some(found), _) => Some(found),
                    (None, true) => {
                        pinned_check = Check {
                            installed: toolchain
                                .name
                                .split('-')
                                .next()
                                .unwrap_or_default()
                                .to_owned(),
                            candidate: None,
                        };
                        Some(&pinned_check)
                    }
                    (None, false) => None,
                };
                Self::package(&toolchain.name, notes.join(" · "), check)
            })
            .collect();
        if let Some(own) = checks.get(SELF) {
            rows.push(Self::package(
                SELF,
                "The rustup toolchain manager".into(),
                Some(own),
            ));
        }
        Ok(rows)
    }
}

impl<T: Transport> Backend for Rustup<T> {
    fn id(&self) -> &str {
        ID
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        match self
            .transport
            .dev_tool(ID, &["--version".into()], cancel, false)
        {
            Ok(_) => Ok(Availability::Available),
            Err(ExecutionError::Disabled(reason)) => Ok(Availability::Unavailable(reason)),
            Err(ExecutionError::Cancelled) => Err(EngineError::Cancelled),
            Err(error) => Err(error.into()),
        }
    }
    fn may_have(&self, name: &str) -> bool {
        valid_name(name)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let query = query.to_lowercase();
        let mut rows: Vec<Package> = self
            .rows(false, cancel)?
            .into_iter()
            .filter(|row| row.id.name.contains(&query))
            .collect();
        // A channel or version that isn't installed can be.
        let known = ["stable", "beta", "nightly"].contains(&query.as_str())
            || query.starts_with(|c: char| c.is_ascii_digit());
        if known
            && valid_name(&query)
            && !rows.iter().any(|row| {
                row.id.name == query || row.id.name.split('-').next() == Some(query.as_str())
            })
        {
            let mut offer =
                Self::package(&query, format!("Install the {query} Rust toolchain"), None);
            offer.installed_version = None;
            rows.push(offer);
        }
        Ok(rows)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.rows(true, cancel)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let package = self
            .rows(true, cancel)?
            .into_iter()
            .find(|row| row.id == *id)
            .ok_or(EngineError::NotFound)?;
        let description = if id.name == SELF {
            "rustup itself. Updating it runs `rustup self update`.".to_owned()
        } else {
            format!(
                "{}. Updates stay on this toolchain's channel and never update rustup itself; pinned versions don't change.",
                package.summary
            )
        };
        Ok(PackageDetails {
            package,
            description,
            homepage: Some("https://rust-lang.github.io/rustup/".into()),
            dependencies: vec![],
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let (id, args): (&PackageId, Vec<&str>) = match operation {
            Operation::Upgrade(id) if id.name == SELF => (id, vec!["self", "update"]),
            Operation::Upgrade(id) => (id, vec!["update", "--no-self-update", "--", &id.name]),
            Operation::Install(id) => (
                id,
                vec![
                    "toolchain",
                    "install",
                    "--profile",
                    "minimal",
                    "--no-self-update",
                    "--",
                    &id.name,
                ],
            ),
            Operation::Remove(id) => {
                let toolchains = self.toolchains(cancel)?;
                let found = toolchains
                    .iter()
                    .find(|t| t.name == id.name)
                    .ok_or(EngineError::NotFound)?;
                if found.default {
                    return Err(invalid(ID, format!(
                        "{} is the default toolchain; pick another default (rustup default <toolchain>) before removing it",
                        id.name
                    )));
                }
                (id, vec!["toolchain", "uninstall", "--", &id.name])
            }
            other => return Err(self.unsupported(other.capability())),
        };
        if id.backend != ID || !valid_name(&id.name) {
            return Err(invalid(ID, "foreign or invalid toolchain"));
        }
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        progress(Progress::Message(format!(
            "Running rustup {}",
            args.join(" ")
        )));
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let completion = self.transport.dev_tool(ID, &args, cancel, true)?;
        let deferred = completion.cancellation_deferred;
        bytes(ID, completion)?;
        // Verify with a fresh read; writes may finish after cancellation.
        let after = self.toolchains(&Cancellation::default())?;
        let listed = after
            .iter()
            .any(|t| t.name == id.name || t.name.split('-').next() == Some(id.name.as_str()));
        let ok = match operation {
            Operation::Install(_) => listed,
            Operation::Remove(_) => !after.iter().any(|t| t.name == id.name),
            _ => id.name == SELF || listed,
        };
        if !ok {
            return Err(invalid(
                ID,
                format!(
                    "rustup finished, but {} is not in the expected state",
                    id.name
                ),
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
    use crate::host::AptAction;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Fake {
        list: Arc<Mutex<String>>,
        calls: Arc<Mutex<Vec<String>>>,
        /// How `rustup --version` fails, if it does.
        missing: Option<fn() -> ExecutionError>,
    }
    fn done(text: &str, code: i32) -> Completion {
        Completion {
            code: Some(code),
            signal: None,
            stdout: text.as_bytes().to_vec(),
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
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            assert_eq!(executable, "rustup");
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            let line = args.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            let mut list = self.list.lock().unwrap();
            match args[0].as_str() {
                "--version" => self
                    .missing
                    .map_or_else(|| Ok(done("rustup 1.29.1", 0)), |error| Err(error())),
                "toolchain" if args[1] == "list" => Ok(done(&list, 0)),
                // Newer rustup exits 100 when something can be updated.
                "check" => Err(ExecutionError::Failed(done(
                    "stable-aarch64-apple-darwin - Update available : 1.89.0 (29483883e 2025-08-04) -> 1.98.1 (48a229cea 2026-09-01)\n\
                     beta-aarch64-apple-darwin - Up to date : 1.99.0-beta.2 (abc 2026-09-20)\n\
                     nightly-aarch64-apple-darwin - up to date: 1.100.0-nightly (def 2026-09-27)\n\
                     rustup - update available : 1.28.2 -> 1.29.1\n",
                    100,
                ))),
                "toolchain" if args[1] == "install" => {
                    list.push_str("nightly-aarch64-apple-darwin\n");
                    Ok(done("", 0))
                }
                "toolchain" if args[1] == "uninstall" => {
                    *list = list.replace("beta-aarch64-apple-darwin\n", "");
                    Ok(done("", 0))
                }
                "update" | "self" => Ok(done("", 0)),
                other => panic!("unexpected {other}"),
            }
        }
    }
    fn fake() -> Fake {
        Fake {
            list: Arc::new(Mutex::new(
                "stable-aarch64-apple-darwin (default)\nbeta-aarch64-apple-darwin\n1.80.0-aarch64-apple-darwin (active)\n".into(),
            )),
            calls: Arc::default(),
            missing: None,
        }
    }

    #[test]
    fn toolchains_channels_pins_and_rustup_itself() {
        let fake = fake();
        let mut rustup = Rustup::new(fake.clone());
        let rows = rustup.installed(&Cancellation::default()).unwrap();
        let summary: Vec<(&str, &str, Option<&str>)> = rows
            .iter()
            .map(|r| {
                (
                    r.id.name.as_str(),
                    r.installed_version.as_deref().unwrap(),
                    r.candidate_version.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("stable-aarch64-apple-darwin", "1.89.0", Some("1.98.1")),
                ("beta-aarch64-apple-darwin", "1.99.0-beta.2", None),
                ("1.80.0-aarch64-apple-darwin", "1.80.0", None),
                ("rustup", "1.28.2", Some("1.29.1")),
            ]
        );
        assert_eq!(rows[0].summary, "Rust toolchain · default");
        assert_eq!(rows[2].update, UpdateAvailability::Current);
        rustup
            .execute(
                &Operation::Upgrade(rows[0].id.clone()),
                &Cancellation::default(),
                &mut |_| {},
            )
            .unwrap();
        rustup
            .execute(
                &Operation::Upgrade(rows[3].id.clone()),
                &Cancellation::default(),
                &mut |_| {},
            )
            .unwrap();
        let calls = fake.calls.lock().unwrap().clone();
        assert!(calls.contains(&"update --no-self-update -- stable-aarch64-apple-darwin".into()));
        assert!(calls.contains(&"self update".into()));
    }

    #[test]
    fn check_output_from_old_and_new_rustup() {
        let checks = parse_check(
            "stable-x86_64-unknown-linux-gnu - Update available : 1.97.0 (a 2026-08-01) -> 1.98.1 (b 2026-09-01)\n\
             beta-x86_64-unknown-linux-gnu - up to date: 1.99.0-beta.2 (c 2026-09-20)\n\
             nightly-x86_64-unknown-linux-gnu - update available: (unknown version) -> 1.100.0-nightly (d 2026-09-27)\n\
             rustup - update available : 1.28.2 -> 1.29.1\n",
        );
        assert_eq!(
            checks["stable-x86_64-unknown-linux-gnu"]
                .candidate
                .as_deref(),
            Some("1.98.1")
        );
        assert_eq!(
            checks["beta-x86_64-unknown-linux-gnu"],
            Check {
                installed: "1.99.0-beta.2".into(),
                candidate: None
            }
        );
        assert_eq!(
            checks["nightly-x86_64-unknown-linux-gnu"].installed,
            "unknown"
        );
        assert_eq!(checks["rustup"].candidate.as_deref(), Some("1.29.1"));
    }

    #[test]
    fn a_full_toolchain_name_finds_only_the_installed_row() {
        let mut rustup = Rustup::new(fake());
        let found = rustup
            .search("1.80.0-aarch64-apple-darwin", &Cancellation::default())
            .unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].installed_version.is_some());
    }

    #[test]
    fn install_offers_and_removal_rules() {
        let fake = fake();
        let mut rustup = Rustup::new(fake.clone());
        let offers = rustup.search("nightly", &Cancellation::default()).unwrap();
        assert_eq!(offers.len(), 1);
        assert!(unverified_search_offer(&offers[0]));
        assert!(rustup
            .search("stable", &Cancellation::default())
            .unwrap()
            .iter()
            .all(|r| r.installed_version.is_some()));
        rustup
            .execute(
                &Operation::Install(offers[0].id.clone()),
                &Cancellation::default(),
                &mut |_| {},
            )
            .unwrap();
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"toolchain install --profile minimal --no-self-update -- nightly".into()));
        let rows = rustup.installed(&Cancellation::default()).unwrap();
        // The default toolchain is never removed out from under Cargo.
        let error = rustup
            .execute(
                &Operation::Remove(rows[0].id.clone()),
                &Cancellation::default(),
                &mut |_| {},
            )
            .unwrap_err();
        assert!(error.to_string().contains("default toolchain"), "{error}");
        rustup
            .execute(
                &Operation::Remove(rows[1].id.clone()),
                &Cancellation::default(),
                &mut |_| {},
            )
            .unwrap();
        assert!(!fake.list.lock().unwrap().contains("beta"));
        let mut bad = rows[1].id.clone();
        bad.name = "--help".into();
        assert!(rustup
            .execute(
                &Operation::Install(bad),
                &Cancellation::default(),
                &mut |_| {}
            )
            .is_err());
    }

    #[test]
    fn availability_follows_the_rustup_command() {
        let detect = |missing: Option<fn() -> ExecutionError>| {
            Rustup::new(Fake { missing, ..fake() }).detect(&Cancellation::default())
        };
        assert_eq!(detect(None).unwrap(), Availability::Available);
        assert_eq!(
            detect(Some(|| ExecutionError::Disabled("rustup not found".into()))).unwrap(),
            Availability::Unavailable("rustup not found".into())
        );
        assert!(matches!(
            detect(Some(|| ExecutionError::Cancelled)),
            Err(EngineError::Cancelled)
        ));
        assert!(matches!(
            detect(Some(|| ExecutionError::TimedOut)),
            Err(EngineError::Execution(ExecutionError::TimedOut))
        ));
        let rustup = Rustup::new(fake());
        assert!(rustup.may_have("1.80.0-aarch64-apple-darwin") && !rustup.may_have("--help"));
    }

    #[test]
    fn details_explain_toolchain_and_self_updates() {
        let mut rustup = Rustup::new(fake());
        let cancel = Cancellation::default();
        let rows = rustup.installed(&cancel).unwrap();
        let stable = rustup.details(&rows[0].id, &cancel).unwrap();
        assert!(stable
            .description
            .starts_with("Rust toolchain · default. Updates stay on this toolchain's channel"));
        let own = rustup.details(&rows[3].id, &cancel).unwrap();
        assert!(own.description.contains("rustup self update"));
        let mut absent = rows[0].id.clone();
        absent.name = "nightly-aarch64-apple-darwin".into();
        assert!(matches!(
            rustup.details(&absent, &cancel),
            Err(EngineError::NotFound)
        ));
    }

    #[test]
    fn operations_refuse_before_running_and_verify_after() {
        let fake = fake();
        let mut rustup = Rustup::new(fake.clone());
        let cancel = Cancellation::default();
        let rows = rustup.installed(&cancel).unwrap();
        let beta = rows[1].id.clone();
        let mut run = |operation: Operation, cancel: &Cancellation| {
            rustup.execute(&operation, cancel, &mut |_| {})
        };
        assert!(matches!(
            run(Operation::UpgradeAll { backend: ID.into() }, &cancel),
            Err(EngineError::Unsupported { .. })
        ));
        let mut absent = beta.clone();
        absent.name = "nightly".into();
        assert!(matches!(
            run(Operation::Remove(absent), &cancel),
            Err(EngineError::NotFound)
        ));
        let mut foreign = beta.clone();
        foreign.backend = "cargo".into();
        let error = run(Operation::Upgrade(foreign), &cancel).unwrap_err();
        assert!(error.to_string().contains("foreign or invalid toolchain"));
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(matches!(
            run(Operation::Upgrade(beta.clone()), &cancelled),
            Err(EngineError::Cancelled)
        ));
        // Nothing ran for any refusal.
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("update") || call.contains("uninstall")));
        // An update of a toolchain that then isn't listed is an error.
        *fake.list.lock().unwrap() = "stable-aarch64-apple-darwin (default)\n".into();
        let error = run(Operation::Upgrade(beta), &cancel).unwrap_err();
        assert!(
            error.to_string().contains("not in the expected state"),
            "{error}"
        );
    }
}
