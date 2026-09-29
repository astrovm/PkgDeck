//! Conda-family named environments: conda, mamba, or micromamba.
//!
//! Rows are the packages someone asked for in each named environment (the
//! environment's request history), not every dependency. Project prefixes
//! outside an `envs` folder are left alone. Updates are previewed with the
//! manager's own dry-run solve and confirmed afterwards. Removing runs the
//! manager's own `remove` in that environment and checks the package is
//! gone; PkgDeck never installs conda packages.
use super::{bytes, invalid, NativeTransport, Transport};
use crate::{engine::*, package::*, process::*};
use serde_json::Value;
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

const ID: &str = "conda";
const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Installed,
    Capability::Details,
    Capability::Upgrade,
    Capability::Remove,
];
/// Tried in order. mamba 2 and micromamba share libmamba's output.
const MANAGERS: [&str; 3] = ["conda", "mamba", "micromamba"];

pub struct Conda<T = NativeTransport> {
    transport: T,
    manager: Option<&'static str>,
}

/// One named environment and the packages requested in it.
struct Environment {
    name: String,
    prefix: PathBuf,
    requested: Vec<String>,
    installed: BTreeMap<String, String>,
}

/// Conda versions: dot, dash, or underscore separated segments of numbers
/// and letters. Letters sort before numbers (`1.0rc1 < 1.0`), except `post`,
/// and missing segments count as zero (`1.1 == 1.1.0`).
pub(super) fn compare_versions(left: &str, right: &str) -> Ordering {
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    enum Part {
        Dev,
        Text(String),
        Number(u64),
        Post,
    }
    fn parts(version: &str) -> Vec<Part> {
        let version = version.split_once('!').map_or(version, |(_, rest)| rest);
        let version = version.split('+').next().unwrap_or(version);
        let mut parts = vec![];
        for segment in version.to_lowercase().split(['.', '-', '_']) {
            let mut rest = segment;
            while !rest.is_empty() {
                let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                let len = if digits > 0 {
                    digits
                } else {
                    rest.bytes().take_while(|b| !b.is_ascii_digit()).count()
                };
                let (token, tail) = rest.split_at(len);
                parts.push(match token {
                    _ if digits > 0 => Part::Number(token.parse().unwrap_or(u64::MAX)),
                    "dev" => Part::Dev,
                    "post" => Part::Post,
                    text => Part::Text(text.into()),
                });
                rest = tail;
            }
        }
        parts
    }
    // An epoch (`1!2.0`) outranks everything after it.
    let epoch = |version: &str| {
        version
            .split_once('!')
            .and_then(|(epoch, _)| epoch.parse::<u64>().ok())
            .unwrap_or(0)
    };
    let ordering = epoch(left).cmp(&epoch(right));
    if ordering != Ordering::Equal {
        return ordering;
    }
    let (left, right) = (parts(left), parts(right));
    for index in 0..left.len().max(right.len()) {
        let zero = Part::Number(0);
        let ordering = left
            .get(index)
            .unwrap_or(&zero)
            .cmp(right.get(index).unwrap_or(&zero));
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

/// The package a request history entry names: `conda-forge::numpy>=2`,
/// `python=3.12`, and `jq` name `numpy`, `python`, and `jq`.
fn spec_name(spec: &str) -> Option<&str> {
    let spec = spec.rsplit_once("::").map_or(spec, |(_, rest)| rest);
    let name = spec.split(|c: char| "=<>!~ [".contains(c)).next()?.trim();
    (!name.is_empty()).then_some(name)
}

fn json(output: &[u8]) -> Result<Value, EngineError> {
    serde_json::from_slice(output).map_err(|error| invalid(ID, error))
}

/// `conda list --json` prints an array; libmamba wraps it in `packages`.
fn installed_versions(value: &Value) -> BTreeMap<String, String> {
    value
        .as_array()
        .or_else(|| value["packages"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|package| {
            Some((
                package["name"].as_str()?.to_owned(),
                package["version"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// Versions a dry-run solve would link, by package name.
fn planned_versions(value: &Value) -> BTreeMap<String, String> {
    value["actions"]["LINK"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|package| {
            Some((
                package["name"].as_str()?.to_owned(),
                package["version"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

impl<T: Transport> Conda<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            manager: None,
        }
    }

    fn run(
        &self,
        args: &[&OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Vec<u8>, EngineError> {
        let manager = self
            .manager
            .ok_or_else(|| invalid(ID, "no conda-family manager detected"))?;
        let args: Vec<OsString> = args.iter().map(|arg| (*arg).clone()).collect();
        bytes(ID, self.transport.dev_tool(manager, &args, cancel, write)?)
    }

    fn read(
        &self,
        args: &[&str],
        prefix: Option<&Path>,
        cancel: &Cancellation,
    ) -> Result<Value, EngineError> {
        let mut owned: Vec<OsString> = args.iter().map(OsString::from).collect();
        if let Some(prefix) = prefix {
            owned.push("-p".into());
            owned.push(prefix.into());
        }
        owned.push("--json".into());
        json(&self.run(&owned.iter().collect::<Vec<_>>(), cancel, false)?)
    }

    /// Named environments: the base and every environment inside an `envs`
    /// folder. Other prefixes belong to projects.
    fn environments(&self, cancel: &Cancellation) -> Result<Vec<Environment>, EngineError> {
        let info = self.read(&["info"], None, cancel)?;
        let base = info["root_prefix"]
            .as_str()
            .or_else(|| info["base environment"].as_str())
            .map(PathBuf::from);
        let listed = self.read(&["env", "list"], None, cancel)?;
        let mut environments = vec![];
        for prefix in listed["envs"].as_array().into_iter().flatten() {
            let Some(prefix) = prefix.as_str().map(PathBuf::from) else {
                continue;
            };
            let name = if Some(&prefix) == base.as_ref() {
                "base".to_owned()
            } else if prefix
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|dir| dir == "envs")
            {
                match prefix.file_name().and_then(|name| name.to_str()) {
                    Some(name) => name.to_owned(),
                    None => continue,
                }
            } else {
                continue;
            };
            let history = self.read(&["env", "export", "--from-history"], Some(&prefix), cancel)?;
            let requested = history["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(spec_name)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            // Nothing requested, nothing to list: micromamba's base is often
            // only a root folder, which `list` rejects.
            if requested.is_empty() {
                continue;
            }
            let installed = installed_versions(&self.read(&["list"], Some(&prefix), cancel)?);
            environments.push(Environment {
                name,
                prefix,
                requested,
                installed,
            });
        }
        Ok(environments)
    }

    /// What `update` would link for these packages, without changing anything.
    fn plan(
        &self,
        prefix: &Path,
        names: &[&str],
        cancel: &Cancellation,
    ) -> Result<BTreeMap<String, String>, EngineError> {
        let mut args = vec!["update"];
        args.extend(names);
        args.extend(["--dry-run", "--yes"]);
        Ok(planned_versions(&self.read(&args, Some(prefix), cancel)?))
    }

    fn package(
        environment: &Environment,
        name: &str,
        candidate: Option<Option<&str>>,
    ) -> Option<Package> {
        let version = environment.installed.get(name)?;
        Some(Package {
            id: PackageId {
                backend: ID.into(),
                name: name.into(),
                architecture: "unknown".into(),
                scope: Scope::Environment {
                    path: environment.prefix.clone(),
                },
                remote: None,
                reference: None,
            },
            display_name: name.into(),
            summary: format!(
                "Conda environment {} · {}",
                environment.name,
                environment.prefix.display()
            ),
            installed_version: Some(version.clone()),
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
        })
    }

    fn rows(&self, check: bool, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let mut rows = vec![];
        for environment in self.environments(cancel)? {
            let names: Vec<&str> = environment
                .requested
                .iter()
                .map(String::as_str)
                .filter(|name| environment.installed.contains_key(*name))
                .collect();
            let plan = if check && !names.is_empty() {
                Some(self.plan(&environment.prefix, &names, cancel)?)
            } else {
                None
            };
            for name in names {
                let candidate = plan.as_ref().map(|plan| {
                    plan.get(name).map(String::as_str).filter(|new| {
                        compare_versions(new, &environment.installed[name]) == Ordering::Greater
                    })
                });
                rows.extend(Self::package(&environment, name, candidate));
            }
        }
        Ok(rows)
    }

    fn environment_of(
        &self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<Environment, EngineError> {
        let Scope::Environment { path } = &id.scope else {
            return Err(EngineError::NotFound);
        };
        self.environments(cancel)?
            .into_iter()
            .find(|environment| {
                id.backend == ID
                    && environment.prefix == *path
                    && environment.requested.contains(&id.name)
                    && environment.installed.contains_key(&id.name)
            })
            .ok_or(EngineError::NotFound)
    }
}

impl<T: Transport> Backend for Conda<T> {
    fn id(&self) -> &str {
        ID
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        for manager in MANAGERS {
            match self
                .transport
                .dev_tool(manager, &["--version".into()], cancel, false)
            {
                Ok(_) => {
                    self.manager = Some(manager);
                    return Ok(Availability::Available);
                }
                Err(ExecutionError::Disabled(_)) => continue,
                Err(ExecutionError::Cancelled) => return Err(EngineError::Cancelled),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(Availability::Unavailable(
            "conda, mamba, or micromamba not found".into(),
        ))
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        // Installing needs a chosen environment, which is the manager's job.
        let query = query.to_lowercase();
        Ok(self
            .rows(false, cancel)?
            .into_iter()
            .filter(|package| package.id.name.to_lowercase().contains(&query))
            .collect())
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.rows(true, cancel)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let environment = self.environment_of(id, cancel)?;
        let package = Self::package(&environment, &id.name, None).ok_or(EngineError::NotFound)?;
        Ok(PackageDetails {
            description: format!(
                "Requested in the {} environment\n\nLocation: {}\n\nManaged with {}. PkgDeck updates and removes requested packages; install them with the manager.",
                environment.name,
                environment.prefix.display(),
                self.manager.unwrap_or("conda")
            ),
            package,
            homepage: None,
            dependencies: vec![],
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let (id, remove) = match operation {
            Operation::Upgrade(id) => (id, false),
            Operation::Remove(id) => (id, true),
            _ => return Err(self.unsupported(operation.capability())),
        };
        let environment = self.environment_of(id, cancel)?;
        let before = environment.installed[&id.name].clone();
        if remove {
            if cancel.requested() {
                return Err(EngineError::Cancelled);
            }
            progress(Progress::Message(format!(
                "Removing {} {before} from the {} environment",
                id.name, environment.name
            )));
        } else {
            let plan = self.plan(&environment.prefix, &[&id.name], cancel)?;
            let Some(target) = plan
                .get(&id.name)
                .filter(|new| compare_versions(new, &before) == Ordering::Greater)
            else {
                return Ok(OperationOutcome::default());
            };
            if cancel.requested() {
                return Err(EngineError::Cancelled);
            }
            progress(Progress::Message(format!(
                "Updating {} {before} → {target} in the {} environment",
                id.name, environment.name
            )));
        }
        let args: Vec<OsString> = vec![
            if remove { "remove" } else { "update" }.into(),
            id.name.clone().into(),
            "-p".into(),
            environment.prefix.clone().into(),
            "--yes".into(),
            "--json".into(),
        ];
        let completion = self.transport.dev_tool(
            self.manager
                .ok_or_else(|| invalid(ID, "no conda-family manager detected"))?,
            &args,
            cancel,
            true,
        )?;
        let deferred = completion.cancellation_deferred;
        bytes(ID, completion)?;
        // Native writes may complete after cancellation; verification still runs.
        let after = installed_versions(&self.read(
            &["list"],
            Some(&environment.prefix),
            &Cancellation::default(),
        )?);
        if remove {
            if after.contains_key(&id.name) {
                return Err(invalid(
                    ID,
                    format!(
                        "{} is still installed in the {} environment",
                        id.name, environment.name
                    ),
                ));
            }
        } else if after
            .get(&id.name)
            .is_none_or(|now| compare_versions(now, &before) != Ordering::Greater)
        {
            return Err(invalid(
                ID,
                format!(
                    "{} was not updated in the {} environment",
                    id.name, environment.name
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
    use crate::backends::dev_tools::{DevTool, DevTools};
    use std::sync::{Arc, Mutex};

    fn backend(fake: Fake) -> Conda<DevTools<Fake>> {
        Conda::new(DevTools(fake))
    }
    fn ignore(_: Progress) {}

    #[test]
    fn conda_versions_order_like_conda() {
        use Ordering::*;
        for (left, right, expected) in [
            ("1.1", "1.1.0", Equal),
            ("1.10", "1.9", Greater),
            ("1.0rc1", "1.0", Less),
            ("1.0.post1", "1.0", Greater),
            ("1.0.dev1", "1.0a1", Less),
            ("2024.1", "2023.12.1", Greater),
            ("1!1.0", "2.0", Greater),
            ("1.8.2", "1.8.2", Equal),
        ] {
            assert_eq!(compare_versions(left, right), expected, "{left} vs {right}");
        }
        assert_eq!(spec_name("conda-forge::numpy>=2"), Some("numpy"));
        assert_eq!(spec_name("ripgrep=14.1.0"), Some("ripgrep"));
        assert_eq!(spec_name("jq"), Some("jq"));
        assert_eq!(spec_name(">=1"), None);
    }

    /// Replies like micromamba (wrapped lists) or conda (bare arrays).
    #[derive(Clone, Default)]
    struct Fake {
        manager: &'static str,
        ripgrep: Arc<Mutex<String>>,
        calls: Arc<Mutex<Vec<String>>>,
        updates: bool,
        /// How the manager's `--version` fails, if it does.
        broken: Option<fn() -> ExecutionError>,
        /// How `update` fails, if it does.
        write_error: Option<fn() -> ExecutionError>,
        /// The tools environment can't be listed once `update` ran.
        unreadable_after_update: bool,
        /// `remove` takes ripgrep out of the tools environment.
        removes: bool,
    }
    fn done(value: Value) -> Completion {
        Completion {
            code: Some(0),
            signal: None,
            stdout: value.to_string().into_bytes(),
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
        }
    }
    impl DevTool for Fake {
        fn dev_tool(
            &self,
            executable: &str,
            args: &[OsString],
            _: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            if executable != self.manager {
                return Err(ExecutionError::Disabled(format!("{executable} not found")));
            }
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            if let (Some(error), "--version") = (self.broken, args[0].as_str()) {
                return Err(error());
            }
            let line = args.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            let micromamba = self.manager != "conda";
            let prefix = args
                .iter()
                .position(|a| a == "-p")
                .map(|at| args[at + 1].clone())
                .unwrap_or_default();
            let ripgrep = self.ripgrep.lock().unwrap().clone();
            Ok(done(match args[0].as_str() {
                "--version" => Value::String("26.7.2".into()),
                "info" if micromamba => serde_json::json!({"base environment": "/c"}),
                "info" => serde_json::json!({"root_prefix": "/c"}),
                // Also a malformed entry, a path without a name, and an
                // environment where nothing was requested: none is a row.
                "env" if args[1] == "list" => serde_json::json!({"envs": [
                    "/c", "/c/envs/tools", "/work/project/.conda", 7, "/c/envs/..", "/c/envs/empty"
                ]}),
                "env" => {
                    assert_ne!(
                        prefix, "/work/project/.conda",
                        "project prefixes stay untouched"
                    );
                    if prefix == "/c" {
                        serde_json::json!({"dependencies": ["python=3.12"]})
                    } else if prefix == "/c/envs/empty" {
                        serde_json::json!({"name": "empty"})
                    } else {
                        serde_json::json!({"dependencies": ["conda-forge::ripgrep=14.1.0", "jq", "gone"]})
                    }
                }
                "list" if ripgrep == "unreadable" => return Err(ExecutionError::TimedOut),
                "list" => {
                    let packages = if prefix == "/c" {
                        serde_json::json!([{"name": "python", "version": "3.12.1"}, {"name": "zlib", "version": "1"}])
                    } else {
                        let mut packages = vec![
                            serde_json::json!({"name": "jq", "version": "1.8.2"}),
                            serde_json::json!({"name": "libgcc", "version": "15"}),
                        ];
                        if ripgrep != "removed" {
                            packages
                                .push(serde_json::json!({"name": "ripgrep", "version": ripgrep}));
                        }
                        Value::from(packages)
                    };
                    if micromamba {
                        serde_json::json!({"packages": packages})
                    } else {
                        packages
                    }
                }
                "update" if args.contains(&"--dry-run".to_owned()) => {
                    assert!(!write);
                    if prefix == "/c/envs/tools" && ripgrep == "14.1.0" {
                        serde_json::json!({"actions": {"LINK": [{"name": "ripgrep", "version": "15.2.0"}, {"name": "libgcc", "version": "16"}]}})
                    } else {
                        serde_json::json!({"success": true, "message": "All requested packages already installed."})
                    }
                }
                _ => {
                    assert!(write);
                    if let Some(error) = self.write_error {
                        return Err(error());
                    }
                    if args[0] == "remove" {
                        if self.removes {
                            *self.ripgrep.lock().unwrap() = "removed".into();
                        }
                        return Ok(done(serde_json::json!({"success": true})));
                    }
                    assert_eq!(args[0], "update");
                    if self.unreadable_after_update {
                        *self.ripgrep.lock().unwrap() = "unreadable".into();
                    } else if self.updates {
                        *self.ripgrep.lock().unwrap() = "15.2.0".into();
                    }
                    serde_json::json!({"success": true})
                }
            }))
        }
    }
    fn fake(manager: &'static str) -> Fake {
        Fake {
            manager,
            ripgrep: Arc::new(Mutex::new("14.1.0".into())),
            ..Fake::default()
        }
    }
    fn ripgrep() -> PackageId {
        PackageId {
            backend: ID.into(),
            name: "ripgrep".into(),
            architecture: "unknown".into(),
            scope: Scope::Environment {
                path: "/c/envs/tools".into(),
            },
            remote: None,
            reference: None,
        }
    }

    #[test]
    fn named_environments_list_requested_packages_with_planned_updates() {
        for manager in ["conda", "micromamba"] {
            let fake = fake(manager);
            let mut conda = backend(fake.clone());
            assert_eq!(
                conda.detect(&Cancellation::default()).unwrap(),
                Availability::Available
            );
            let rows = conda.installed(&Cancellation::default()).unwrap();
            let names: Vec<(&str, &str)> = rows
                .iter()
                .map(|row| {
                    (
                        row.id.name.as_str(),
                        row.summary.split(" · ").next().unwrap(),
                    )
                })
                .collect();
            // Dependencies and packages no longer installed are not rows.
            assert_eq!(
                names,
                vec![
                    ("python", "Conda environment base"),
                    ("ripgrep", "Conda environment tools"),
                    ("jq", "Conda environment tools")
                ],
                "{manager}"
            );
            assert_eq!(rows[0].update, UpdateAvailability::Current);
            assert_eq!(rows[1].update, UpdateAvailability::Available);
            assert_eq!(rows[1].candidate_version.as_deref(), Some("15.2.0"));
            assert_eq!(rows[2].update, UpdateAvailability::Current);
            assert!(fake
                .calls
                .lock()
                .unwrap()
                .contains(&"update ripgrep jq --dry-run --yes -p /c/envs/tools --json".into()));
            // Search reads the same rows without solving.
            fake.calls.lock().unwrap().clear();
            assert_eq!(
                conda.search("rip", &Cancellation::default()).unwrap().len(),
                1
            );
            assert!(!fake
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call.starts_with("update")));
        }
    }

    #[test]
    fn updates_are_planned_run_and_verified() {
        let mut fake = fake("conda");
        fake.updates = true;
        let mut conda = backend(fake.clone());
        conda.detect(&Cancellation::default()).unwrap();
        conda
            .execute(
                &Operation::Upgrade(ripgrep()),
                &Cancellation::default(),
                &mut ignore,
            )
            .unwrap();
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"update ripgrep -p /c/envs/tools --yes --json".into()));
        // Current packages run nothing.
        fake.calls.lock().unwrap().clear();
        conda
            .execute(
                &Operation::Upgrade(ripgrep()),
                &Cancellation::default(),
                &mut ignore,
            )
            .unwrap();
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.ends_with("--yes --json")));
        // A dependency is not a requested package.
        let mut libgcc = ripgrep();
        libgcc.name = "libgcc".into();
        assert!(matches!(
            conda.execute(
                &Operation::Upgrade(libgcc),
                &Cancellation::default(),
                &mut ignore
            ),
            Err(EngineError::NotFound)
        ));
        assert!(matches!(
            conda.execute(
                &Operation::Install(ripgrep()),
                &Cancellation::default(),
                &mut ignore
            ),
            Err(EngineError::Unsupported { .. })
        ));
    }

    #[test]
    fn an_update_that_changes_nothing_is_an_error() {
        let fake = fake("mamba");
        let mut conda = backend(fake);
        conda.detect(&Cancellation::default()).unwrap();
        let error = conda
            .execute(
                &Operation::Upgrade(ripgrep()),
                &Cancellation::default(),
                &mut ignore,
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("ripgrep was not updated in the tools environment"),
            "{error}"
        );
    }

    #[test]
    fn no_manager_is_unavailable() {
        let mut conda = backend(fake("none"));
        assert!(matches!(
            conda.detect(&Cancellation::default()).unwrap(),
            Availability::Unavailable(_)
        ));
        // A manager that is there but fails is an error, not a missing one.
        let detect = |broken: fn() -> ExecutionError| {
            let mut fake = fake("mamba");
            fake.broken = Some(broken);
            backend(fake).detect(&Cancellation::default())
        };
        assert!(matches!(
            detect(|| ExecutionError::Cancelled),
            Err(EngineError::Cancelled)
        ));
        assert!(matches!(
            detect(|| ExecutionError::TimedOut),
            Err(EngineError::Execution(ExecutionError::TimedOut))
        ));
    }

    #[test]
    fn details_name_the_environment_and_its_manager() {
        let mut conda = backend(fake("micromamba"));
        conda.detect(&Cancellation::default()).unwrap();
        let details = conda.details(&ripgrep(), &Cancellation::default()).unwrap();
        assert_eq!(
            details.description,
            "Requested in the tools environment\n\nLocation: /c/envs/tools\n\nManaged with micromamba. PkgDeck updates and removes requested packages; install them with the manager."
        );
        assert_eq!(details.package.installed_version.as_deref(), Some("14.1.0"));
        let mut libgcc = ripgrep();
        libgcc.name = "libgcc".into();
        assert!(matches!(
            conda.details(&libgcc, &Cancellation::default()),
            Err(EngineError::NotFound)
        ));
        // Conda rows always live in an environment.
        let mut user = ripgrep();
        user.scope = Scope::User { uid: 0 };
        assert!(matches!(
            conda.details(&user, &Cancellation::default()),
            Err(EngineError::NotFound)
        ));
        assert_eq!(conda.id(), "conda");
        assert!(conda.capabilities().contains(&Capability::Upgrade));
        assert!(!conda.capabilities().contains(&Capability::Install));
    }

    #[test]
    fn failed_updates_and_unverifiable_results_are_errors() {
        let mut fake = fake("conda");
        fake.write_error = Some(|| ExecutionError::TimedOut);
        let mut conda = backend(fake.clone());
        conda.detect(&Cancellation::default()).unwrap();
        let mut messages = vec![];
        assert!(matches!(
            conda.execute(
                &Operation::Upgrade(ripgrep()),
                &Cancellation::default(),
                &mut |progress| messages.push(progress)
            ),
            Err(EngineError::Execution(ExecutionError::TimedOut))
        ));
        assert_eq!(
            messages,
            vec![Progress::Message(
                "Updating ripgrep 14.1.0 → 15.2.0 in the tools environment".into()
            )]
        );
        // The update ran, but the environment can't be read back.
        fake.write_error = None;
        fake.unreadable_after_update = true;
        let mut conda = backend(fake.clone());
        conda.detect(&Cancellation::default()).unwrap();
        assert!(matches!(
            conda.execute(
                &Operation::Upgrade(ripgrep()),
                &Cancellation::default(),
                &mut ignore
            ),
            Err(EngineError::Execution(ExecutionError::TimedOut))
        ));
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"update ripgrep -p /c/envs/tools --yes --json".into()));
    }

    #[test]
    fn a_cancelled_update_stops_after_planning() {
        let mut fake = fake("conda");
        fake.updates = true;
        let mut conda = backend(fake.clone());
        conda.detect(&Cancellation::default()).unwrap();
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            conda.execute(&Operation::Upgrade(ripgrep()), &cancel, &mut ignore),
            Err(EngineError::Cancelled)
        ));
        assert_eq!(fake.ripgrep.lock().unwrap().as_str(), "14.1.0");
    }

    #[test]
    fn removals_run_in_the_environment_and_are_verified() {
        let mut fake = fake("micromamba");
        fake.removes = true;
        let mut conda = backend(fake.clone());
        conda.detect(&Cancellation::default()).unwrap();
        assert!(conda.capabilities().contains(&Capability::Remove));
        let mut messages = vec![];
        conda
            .execute(
                &Operation::Remove(ripgrep()),
                &Cancellation::default(),
                &mut |progress| messages.push(progress),
            )
            .unwrap();
        assert_eq!(
            messages,
            vec![Progress::Message(
                "Removing ripgrep 14.1.0 from the tools environment".into()
            )]
        );
        let calls = fake.calls.lock().unwrap().clone();
        assert!(calls.contains(&"remove ripgrep -p /c/envs/tools --yes --json".into()));
        // Removing never solves an update first.
        assert!(!calls.iter().any(|call| call.contains("--dry-run")));
        assert!(!conda
            .installed(&Cancellation::default())
            .unwrap()
            .iter()
            .any(|row| row.id.name == "ripgrep"));
        // Now gone, there is nothing left to remove.
        assert!(matches!(
            conda.execute(
                &Operation::Remove(ripgrep()),
                &Cancellation::default(),
                &mut ignore
            ),
            Err(EngineError::NotFound)
        ));
    }

    #[test]
    fn failed_unverified_foreign_or_cancelled_removals_are_errors() {
        // The manager finished, but the package is still there.
        let fake = fake("conda");
        let mut conda = backend(fake.clone());
        conda.detect(&Cancellation::default()).unwrap();
        let error = conda
            .execute(
                &Operation::Remove(ripgrep()),
                &Cancellation::default(),
                &mut ignore,
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("ripgrep is still installed in the tools environment"),
            "{error}"
        );
        // The manager's own failure is reported as is.
        let mut failing = fake.clone();
        failing.write_error = Some(|| ExecutionError::TimedOut);
        let mut conda = backend(failing);
        conda.detect(&Cancellation::default()).unwrap();
        assert!(matches!(
            conda.execute(
                &Operation::Remove(ripgrep()),
                &Cancellation::default(),
                &mut ignore
            ),
            Err(EngineError::Execution(ExecutionError::TimedOut))
        ));
        // Dependencies, other environments, other sources and cancelled
        // removals never run `remove`.
        fake.calls.lock().unwrap().clear();
        let mut conda = backend(fake.clone());
        conda.detect(&Cancellation::default()).unwrap();
        let mut libgcc = ripgrep();
        libgcc.name = "libgcc".into();
        let mut project = ripgrep();
        project.scope = Scope::Environment {
            path: "/work/project/.conda".into(),
        };
        let mut foreign = ripgrep();
        foreign.backend = "pixi".into();
        for id in [libgcc, project, foreign] {
            assert!(matches!(
                conda.execute(
                    &Operation::Remove(id),
                    &Cancellation::default(),
                    &mut ignore
                ),
                Err(EngineError::NotFound)
            ));
        }
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            conda.execute(&Operation::Remove(ripgrep()), &cancel, &mut ignore),
            Err(EngineError::Cancelled)
        ));
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("remove")));
    }
}
