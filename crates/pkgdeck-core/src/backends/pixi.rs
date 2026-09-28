//! `pixi global`: one row per global environment, the way pixi installs tools.
//!
//! An environment is named after the tool it was installed for and exposes
//! its commands. Updates stay within the version spec the global manifest
//! records for the environment, as `pixi global update` does.
use super::{bytes, conda::compare_versions, invalid, NativeTransport, Transport};
use crate::{engine::*, package::*, process::*};
use serde::Deserialize;
use serde_json::Value;
use std::{cmp::Ordering, collections::BTreeMap, ffi::OsString, path::PathBuf};

const ID: &str = "pixi";
const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Upgrade,
];

pub struct Pixi<T = NativeTransport> {
    transport: T,
}

#[derive(Deserialize, Clone, Debug)]
struct GlobalEnvironment {
    name: String,
    #[serde(default)]
    dependencies: Vec<Dependency>,
    #[serde(default)]
    exposed: Vec<Exposed>,
}
#[derive(Deserialize, Clone, Debug)]
struct Dependency {
    name: String,
    version: String,
}
#[derive(Deserialize, Clone, Debug)]
struct Exposed {
    exposed_name: String,
}

/// What the global manifest records for one environment.
#[derive(Default, Debug, PartialEq)]
struct ManifestEnvironment {
    channels: Vec<String>,
    specs: BTreeMap<String, String>,
}

/// The conda subdirectory for this machine.
fn subdir() -> &'static str {
    subdir_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn subdir_for(os: &str, arch: &str) -> &'static str {
    match (os, arch) {
        ("macos", "aarch64") => "osx-arm64",
        ("macos", _) => "osx-64",
        ("linux", "aarch64") => "linux-aarch64",
        _ => "linux-64",
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b))
        && !name.starts_with(['-', '.'])
}

/// Top-level comma-separated items, ignoring commas in strings, arrays and
/// inline tables.
fn items(text: &str) -> Vec<&str> {
    let (mut depth, mut quoted, mut start, mut out) = (0i32, false, 0, vec![]);
    for (at, c) in text.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '{' | '[' if !quoted => depth += 1,
            '}' | ']' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                out.push(text[start..at].trim());
                start = at + 1;
            }
            _ => {}
        }
    }
    out.push(text[start..].trim());
    out.retain(|item| !item.is_empty());
    out
}

fn unquote(text: &str) -> Option<&str> {
    text.trim().strip_prefix('"')?.strip_suffix('"')
}

/// A dependency value: `"spec"`, or an inline table with `version = "spec"`.
fn dependency_spec(value: &str) -> Option<String> {
    if let Some(spec) = unquote(value) {
        return Some(spec.into());
    }
    let table = value.trim().strip_prefix('{')?.strip_suffix('}')?;
    items(table).into_iter().find_map(|item| {
        let (key, value) = item.split_once('=')?;
        (key.trim() == "version").then(|| unquote(value).map(Into::into))?
    })
}

/// Reads the environments' channels and dependency specs from the global
/// manifest pixi writes. Anything it doesn't recognize is skipped, which
/// leaves that environment's update status unknown.
fn read_manifest(text: &str) -> BTreeMap<String, ManifestEnvironment> {
    let mut environments: BTreeMap<String, ManifestEnvironment> = BTreeMap::new();
    // (environment, inside its [envs.<name>.dependencies] table)
    let mut section: Option<(String, bool)> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = header.strip_prefix("envs.").and_then(|rest| {
                let (name, rest) = match rest.strip_prefix('"') {
                    Some(quoted) => {
                        let (name, rest) = quoted.split_once('"')?;
                        (name, rest)
                    }
                    None => rest.split_once('.').unwrap_or((rest, "")),
                };
                let rest = rest.trim_start_matches('.');
                match rest {
                    "" => Some((name.to_owned(), false)),
                    "dependencies" => Some((name.to_owned(), true)),
                    _ => None,
                }
            });
            continue;
        }
        let Some((name, in_dependencies)) = &section else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"');
        let environment = environments.entry(name.clone()).or_default();
        if *in_dependencies {
            if let Some(spec) = dependency_spec(value) {
                environment.specs.insert(key.into(), spec);
            }
        } else if key == "dependencies" {
            if let Some(table) = value
                .trim()
                .strip_prefix('{')
                .and_then(|v| v.strip_suffix('}'))
            {
                for item in items(table) {
                    if let Some((package, spec)) =
                        item.split_once('=').and_then(|(package, rest)| {
                            Some((package.trim().trim_matches('"'), dependency_spec(rest)?))
                        })
                    {
                        environment.specs.insert(package.into(), spec);
                    }
                }
            }
        } else if key == "channels" {
            if let Some(list) = value
                .trim()
                .strip_prefix('[')
                .and_then(|v| v.strip_suffix(']'))
            {
                environment.channels = items(list)
                    .into_iter()
                    .filter_map(unquote)
                    .map(Into::into)
                    .collect();
            }
        }
    }
    environments
}

/// Whether a version satisfies a conda version spec. `None` for specs this
/// reader does not model (alternatives, compatible release, globs other
/// than a trailing `.*`).
fn satisfies(version: &str, spec: &str) -> Option<bool> {
    let spec = spec.trim();
    if spec.is_empty() || spec == "*" {
        return Some(true);
    }
    if spec.contains(['|', '~', '(']) {
        return None;
    }
    for constraint in spec.split(',') {
        let constraint = constraint.trim();
        let (operator, target) = [">=", "<=", "==", "!=", ">", "<", "="]
            .into_iter()
            .find_map(|op| constraint.strip_prefix(op).map(|rest| (op, rest.trim())))
            .unwrap_or(("==", constraint));
        let ok = if let Some(prefix) = target
            .strip_suffix(".*")
            .or_else(|| (operator == "=").then_some(target))
        {
            if prefix.contains('*') {
                return None;
            }
            let matches = version == prefix
                || version
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('.'));
            match operator {
                "==" | "=" => matches,
                "!=" => !matches,
                _ => return None,
            }
        } else {
            if target.contains('*') {
                return None;
            }
            let ordering = compare_versions(version, target);
            match operator {
                ">=" => ordering != Ordering::Less,
                "<=" => ordering != Ordering::Greater,
                ">" => ordering == Ordering::Greater,
                "<" => ordering == Ordering::Less,
                "!=" => ordering != Ordering::Equal,
                _ => ordering == Ordering::Equal,
            }
        };
        if !ok {
            return Some(false);
        }
    }
    Some(true)
}

impl<T: Transport> Pixi<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    fn run(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Vec<u8>, EngineError> {
        bytes(ID, self.transport.dev_tool(ID, args, cancel, write)?)
    }

    fn home(&self) -> Option<PathBuf> {
        let absolute = |name: &str| {
            self.transport
                .env(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        absolute("PIXI_HOME").or_else(|| absolute("HOME").map(|home| home.join(".pixi")))
    }

    fn list(&self, cancel: &Cancellation) -> Result<Vec<GlobalEnvironment>, EngineError> {
        let output = self.run(
            &["global".into(), "list".into(), "--json".into()],
            cancel,
            false,
        )?;
        serde_json::from_slice(&output).map_err(|error| invalid(ID, error))
    }

    fn manifest(&self) -> BTreeMap<String, ManifestEnvironment> {
        self.home()
            .and_then(|home| std::fs::read_to_string(home.join("manifests/pixi-global.toml")).ok())
            .map(|text| read_manifest(&text))
            .unwrap_or_default()
    }

    /// Every published version of a package for this machine, newest last.
    /// (`--json` lists them all; pixi rejects `--limit` with it.)
    fn versions(
        &self,
        name: &str,
        channels: &[String],
        cancel: &Cancellation,
    ) -> Result<Vec<String>, EngineError> {
        let mut args: Vec<OsString> = vec![
            "search".into(),
            "--json".into(),
            "--platform".into(),
            subdir().into(),
        ];
        for channel in channels {
            args.push("--channel".into());
            args.push(channel.into());
        }
        args.push(name.into());
        let output = match self.transport.dev_tool(ID, &args, cancel, false) {
            Err(ExecutionError::Failed(result))
                if String::from_utf8_lossy(&result.stderr).contains("No packages found") =>
            {
                return Ok(vec![]);
            }
            result => bytes(ID, result?)?,
        };
        // Progress lines such as "Using channels: …" precede the JSON.
        let text = String::from_utf8_lossy(&output);
        let start = text.find('{').unwrap_or(0);
        let value: Value =
            serde_json::from_str(&text[start..]).map_err(|error| invalid(ID, error))?;
        let mut versions: Vec<String> = value
            .as_object()
            .into_iter()
            .flat_map(|platforms| platforms.values())
            .filter_map(Value::as_array)
            .flatten()
            .filter(|record| record["name"].as_str() == Some(name))
            .filter_map(|record| record["version"].as_str().map(str::to_owned))
            .collect();
        versions.sort_by(|a, b| compare_versions(a, b));
        versions.dedup();
        Ok(versions)
    }

    /// The package an environment was installed for: the dependency named
    /// like the environment, or its only dependency.
    fn main_dependency(environment: &GlobalEnvironment) -> Option<&Dependency> {
        environment
            .dependencies
            .iter()
            .find(|dependency| dependency.name == environment.name)
            .or(match environment.dependencies.as_slice() {
                [only] => Some(only),
                _ => None,
            })
    }

    /// `None`: unknown. `Some(None)`: nothing newer within the spec.
    fn candidate(
        &self,
        environment: &GlobalEnvironment,
        manifest: &BTreeMap<String, ManifestEnvironment>,
        cancel: &Cancellation,
    ) -> Result<Option<Option<String>>, EngineError> {
        let Some(dependency) = Self::main_dependency(environment) else {
            return Ok(None);
        };
        let Some(recorded) = manifest.get(&environment.name) else {
            return Ok(None);
        };
        let Some(spec) = recorded.specs.get(&dependency.name) else {
            return Ok(None);
        };
        if satisfies(&dependency.version, spec).is_none() {
            return Ok(None);
        }
        let newest = self
            .versions(&dependency.name, &recorded.channels, cancel)?
            .into_iter()
            .rfind(|version| satisfies(version, spec) == Some(true));
        Ok(Some(newest.filter(|version| {
            compare_versions(version, &dependency.version) == Ordering::Greater
        })))
    }

    fn package(environment: &GlobalEnvironment, candidate: Option<Option<String>>) -> Package {
        let commands: Vec<&str> = environment
            .exposed
            .iter()
            .map(|exposed| exposed.exposed_name.as_str())
            .collect();
        Package {
            id: PackageId {
                backend: ID.into(),
                name: environment.name.clone(),
                architecture: subdir().into(),
                scope: Scope::User {
                    uid: rustix::process::getuid().as_raw(),
                },
                remote: None,
                reference: None,
            },
            display_name: environment.name.clone(),
            summary: if commands.is_empty() {
                "pixi global environment".into()
            } else {
                format!("pixi global environment · {}", commands.join(", "))
            },
            installed_version: Some(
                Self::main_dependency(environment)
                    .map(|dependency| dependency.version.clone())
                    .unwrap_or_else(|| "unknown".into()),
            ),
            update: match &candidate {
                Some(Some(_)) => UpdateAvailability::Available,
                Some(None) => UpdateAvailability::Current,
                None => UpdateAvailability::Unknown,
            },
            candidate_version: candidate.flatten(),
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        }
    }

    fn offer(name: &str, version: String) -> Package {
        Package {
            id: PackageId {
                backend: ID.into(),
                name: name.into(),
                architecture: subdir().into(),
                scope: Scope::User {
                    uid: rustix::process::getuid().as_raw(),
                },
                remote: None,
                reference: None,
            },
            display_name: name.into(),
            summary: format!("Install {name} as a pixi global tool"),
            installed_version: None,
            candidate_version: Some(version),
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        }
    }

    fn environment(
        &self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<Option<GlobalEnvironment>, EngineError> {
        if id.backend != ID {
            return Err(EngineError::NotFound);
        }
        Ok(self
            .list(cancel)?
            .into_iter()
            .find(|environment| environment.name == id.name))
    }
}

impl<T: Transport> Backend for Pixi<T> {
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
        let installed = self.list(cancel)?;
        let lower = query.to_lowercase();
        let mut rows: Vec<Package> = installed
            .iter()
            .filter(|environment| environment.name.contains(&lower))
            .map(|environment| Self::package(environment, None))
            .collect();
        // An exact conda-forge package can be installed as a new environment.
        if valid_name(&lower)
            && !installed
                .iter()
                .any(|environment| environment.name == lower)
        {
            if let Some(latest) = self.versions(&lower, &[], cancel)?.pop() {
                rows.push(Self::offer(&lower, latest));
            }
        }
        Ok(rows)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let manifest = self.manifest();
        self.list(cancel)?
            .iter()
            .map(|environment| {
                Ok(Self::package(
                    environment,
                    self.candidate(environment, &manifest, cancel)?,
                ))
            })
            .collect()
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let environment = self.environment(id, cancel)?.ok_or(EngineError::NotFound)?;
        let dependencies = environment
            .dependencies
            .iter()
            .map(|dependency| format!("{} {}", dependency.name, dependency.version))
            .collect::<Vec<_>>()
            .join(", ");
        let manifest = self.manifest();
        let spec = Self::main_dependency(&environment).and_then(|dependency| {
            manifest
                .get(&environment.name)?
                .specs
                .get(&dependency.name)
                .cloned()
        });
        Ok(PackageDetails {
            description: format!(
                "pixi global environment\n\nPackages: {dependencies}{}",
                spec.map(|spec| format!(
                    "\n\nUpdates stay within {spec}, as recorded in the global manifest."
                ))
                .unwrap_or_default()
            ),
            package: Self::package(&environment, None),
            homepage: Some(format!(
                "https://prefix.dev/channels/conda-forge/packages/{}",
                environment.name
            )),
            dependencies: environment
                .dependencies
                .iter()
                .map(|dependency| dependency.name.clone())
                .collect(),
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let (id, args, verb): (&PackageId, Vec<OsString>, &str) = match operation {
            Operation::Install(id) => {
                if !valid_name(&id.name) {
                    return Err(invalid(ID, format!("{} is not a package name", id.name)));
                }
                if self.environment(id, cancel)?.is_some() {
                    return Ok(OperationOutcome::default());
                }
                (
                    id,
                    vec!["global".into(), "install".into(), id.name.clone().into()],
                    "Installing",
                )
            }
            Operation::Remove(id) => {
                if self.environment(id, cancel)?.is_none() {
                    return Err(EngineError::NotFound);
                }
                (
                    id,
                    vec!["global".into(), "uninstall".into(), id.name.clone().into()],
                    "Removing",
                )
            }
            Operation::Upgrade(id) => {
                let environment = self.environment(id, cancel)?.ok_or(EngineError::NotFound)?;
                if self.candidate(&environment, &self.manifest(), cancel)? == Some(None) {
                    return Ok(OperationOutcome::default());
                }
                (
                    id,
                    vec!["global".into(), "update".into(), id.name.clone().into()],
                    "Updating",
                )
            }
            other => return Err(self.unsupported(other.capability())),
        };
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let before = self.environment(id, cancel)?;
        progress(Progress::Message(format!(
            "{verb} {} with pixi global",
            id.name
        )));
        let completion = self.transport.dev_tool(ID, &args, cancel, true)?;
        let deferred = completion.cancellation_deferred;
        let output = bytes(ID, completion)?;
        if !output.is_empty() {
            progress(Progress::Message(String::from_utf8_lossy(&output).into()));
        }
        // Native writes may complete after cancellation; verification still runs.
        let after = self.environment(id, &Cancellation::default())?;
        let version = |environment: &Option<GlobalEnvironment>| {
            environment
                .as_ref()
                .and_then(Self::main_dependency)
                .map(|dependency| dependency.version.clone())
        };
        let verified = match operation {
            Operation::Install(_) => after.is_some(),
            Operation::Remove(_) => after.is_none(),
            // An update may only refresh dependencies; the tool must still be there.
            _ => {
                after.is_some()
                    && version(&after).is_some_and(|now| {
                        version(&before)
                            .is_none_or(|was| compare_versions(&now, &was) != Ordering::Less)
                    })
            }
        };
        if !verified {
            return Err(invalid(
                ID,
                format!(
                    "pixi finished, but {} is not in the expected state",
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
    use crate::backends::dev_tools::{DevTool, DevTools};
    use std::sync::{Arc, Mutex};

    fn backend(fake: Fake) -> Pixi<DevTools<Fake>> {
        Pixi::new(DevTools(fake))
    }
    fn ignore(_: Progress) {}

    #[test]
    fn manifest_specs_are_read_from_both_table_styles() {
        let manifest = read_manifest(
            r#"version = 1

[envs.ripgrep]
channels = ["conda-forge", "bioconda"]
dependencies = { ripgrep = ">=14,<15", "other" = { version = "==1.0", channel = "x" } }
exposed = { rg = "rg" }

[envs."python-tools"]
channels = ["conda-forge"]

[envs."python-tools".dependencies]
python = "3.12.*"
ruff = { version = "*" }
"#,
        );
        assert_eq!(
            manifest["ripgrep"].channels,
            vec!["conda-forge", "bioconda"]
        );
        assert_eq!(manifest["ripgrep"].specs["ripgrep"], ">=14,<15");
        assert_eq!(manifest["ripgrep"].specs["other"], "==1.0");
        assert_eq!(manifest["python-tools"].specs["python"], "3.12.*");
        assert_eq!(manifest["python-tools"].specs["ruff"], "*");
    }

    #[test]
    fn version_specs() {
        for (version, spec, expected) in [
            ("15.2.0", "*", Some(true)),
            ("15.2.0", ">=14,<15", Some(false)),
            ("14.9", ">=14,<15", Some(true)),
            ("3.12.4", "3.12.*", Some(true)),
            ("3.13.0", "3.12.*", Some(false)),
            ("3.12.4", "=3.12", Some(true)),
            ("1.0", "==1.0", Some(true)),
            ("1.0.1", "1.0", Some(false)),
            ("2.0", ">=1|<0.5", None),
            ("2.0", "~=1.0", None),
        ] {
            assert_eq!(satisfies(version, spec), expected, "{version} {spec}");
        }
    }

    #[derive(Clone)]
    struct Fake {
        home: PathBuf,
        ripgrep: Arc<Mutex<Option<String>>>,
        calls: Arc<Mutex<Vec<String>>>,
        /// How every pixi command fails, if it does.
        broken: Option<fn() -> ExecutionError>,
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
    impl DevTool for Fake {
        fn env(&self, name: &str) -> Option<OsString> {
            (name == "PIXI_HOME").then(|| self.home.clone().into())
        }
        fn dev_tool(
            &self,
            executable: &str,
            args: &[OsString],
            _: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            assert_eq!(executable, "pixi");
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            self.calls.lock().unwrap().push(args.join(" "));
            if let Some(error) = self.broken {
                return Err(error());
            }
            let mut ripgrep = self.ripgrep.lock().unwrap();
            let environment = |version: &str| {
                format!(
                    r#"{{"name":"ripgrep","dependencies":[{{"name":"ripgrep","version":"{version}"}}],"exposed":[{{"exposed_name":"rg","executable":"rg"}}]}}"#
                )
            };
            match (args[0].as_str(), args.get(1).map(String::as_str)) {
                ("--version", _) => Ok(done("pixi 0.81.0".into())),
                ("global", Some("list")) => Ok(done(format!(
                    "[{}]",
                    ripgrep.as_deref().map(environment).unwrap_or_default()
                ))),
                ("search", _) => {
                    let name = args.last().unwrap();
                    if name != "ripgrep" {
                        let mut failed = done(String::new());
                        failed.code = Some(1);
                        failed.stderr = b"Error: No packages found matching 'x'".to_vec();
                        return Err(ExecutionError::Failed(failed));
                    }
                    assert!(args.contains(&subdir().to_owned()));
                    Ok(done(format!(
                        "Using channels: conda-forge\n{{\"{}\": [{{\"name\":\"ripgrep\",\"version\":\"14.1.1\"}},{{\"name\":\"ripgrep\",\"version\":\"15.2.0\"}},{{\"name\":\"ripgrep\",\"version\":\"14.1.0\"}}]}}",
                        subdir()
                    )))
                }
                _ => {
                    assert!(write && args[0] == "global", "{args:?}");
                    let output = match args[1].as_str() {
                        "install" => {
                            *ripgrep = Some("15.2.0".into());
                            "Installed ripgrep"
                        }
                        "uninstall" => {
                            *ripgrep = None;
                            ""
                        }
                        verb => {
                            assert_eq!(verb, "update");
                            *ripgrep = Some("14.1.1".into());
                            ""
                        }
                    };
                    Ok(done(output.into()))
                }
            }
        }
    }
    fn fake(test: &str, installed: Option<&str>, spec: &str) -> (Fake, PathBuf) {
        let home = std::env::temp_dir().join(format!("pkgdeck-pixi-{}-{test}", std::process::id()));
        std::fs::create_dir_all(home.join("manifests")).unwrap();
        std::fs::write(
            home.join("manifests/pixi-global.toml"),
            format!("version = 1\n\n[envs.ripgrep]\nchannels = [\"conda-forge\"]\ndependencies = {{ ripgrep = \"{spec}\" }}\n"),
        )
        .unwrap();
        (
            Fake {
                home: home.clone(),
                ripgrep: Arc::new(Mutex::new(installed.map(Into::into))),
                calls: Arc::default(),
                broken: None,
            },
            home,
        )
    }
    fn id() -> PackageId {
        Pixi::<DevTools<Fake>>::offer("ripgrep", "0".into()).id
    }

    #[test]
    fn updates_stay_within_the_manifest_spec() {
        let (fake, home) = fake("range", Some("14.1.0"), ">=14,<15");
        let mut pixi = backend(fake.clone());
        let rows = pixi.installed(&Cancellation::default()).unwrap();
        assert_eq!(rows[0].summary, "pixi global environment · rg");
        assert_eq!(rows[0].update, UpdateAvailability::Available);
        assert_eq!(rows[0].candidate_version.as_deref(), Some("14.1.1"));
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.contains("--channel conda-forge")));
        pixi.execute(
            &Operation::Upgrade(id()),
            &Cancellation::default(),
            &mut ignore,
        )
        .unwrap();
        assert_eq!(fake.ripgrep.lock().unwrap().as_deref(), Some("14.1.1"));
        // Now current within its range: nothing runs.
        fake.calls.lock().unwrap().clear();
        pixi.execute(
            &Operation::Upgrade(id()),
            &Cancellation::default(),
            &mut ignore,
        )
        .unwrap();
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("global update")));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn unmodeled_specs_leave_updates_unknown() {
        let (fake, home) = fake("unmodeled", Some("14.1.0"), ">=14|<2");
        let mut pixi = backend(fake);
        assert_eq!(
            pixi.installed(&Cancellation::default()).unwrap()[0].update,
            UpdateAvailability::Unknown
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn install_and_remove_are_verified() {
        let (fake, home) = fake("lifecycle", None, "*");
        let mut pixi = backend(fake.clone());
        let offers = pixi.search("ripgrep", &Cancellation::default()).unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].candidate_version.as_deref(), Some("15.2.0"));
        assert!(!unverified_search_offer(&offers[0]));
        // Unknown names offer nothing instead of failing the search.
        assert!(pixi
            .search("nothing-here", &Cancellation::default())
            .unwrap()
            .is_empty());
        let mut messages = vec![];
        pixi.execute(
            &Operation::Install(id()),
            &Cancellation::default(),
            &mut |progress| messages.push(progress),
        )
        .unwrap();
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"global install ripgrep".into()));
        // pixi's own output follows the step.
        assert_eq!(
            messages,
            vec![
                Progress::Message("Installing ripgrep with pixi global".into()),
                Progress::Message("Installed ripgrep".into()),
            ]
        );
        // Installed now: searching its name lists it without an offer.
        fake.calls.lock().unwrap().clear();
        let found = pixi.search("ripgrep", &Cancellation::default()).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].installed_version.is_some());
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("search")));
        pixi.execute(
            &Operation::Remove(id()),
            &Cancellation::default(),
            &mut ignore,
        )
        .unwrap();
        assert!(fake.ripgrep.lock().unwrap().is_none());
        assert!(matches!(
            pixi.execute(
                &Operation::Remove(id()),
                &Cancellation::default(),
                &mut ignore
            ),
            Err(EngineError::NotFound)
        ));
        let mut bad = id();
        bad.name = "--help".into();
        assert!(pixi
            .execute(
                &Operation::Install(bad),
                &Cancellation::default(),
                &mut ignore
            )
            .is_err());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn details_name_the_spec_updates_stay_within() {
        let (fake, home) = fake("details", Some("14.1.0"), ">=14,<15");
        let mut pixi = backend(fake);
        assert_eq!(
            pixi.detect(&Cancellation::default()).unwrap(),
            Availability::Available
        );
        let details = pixi.details(&id(), &Cancellation::default()).unwrap();
        assert_eq!(
            details.description,
            "pixi global environment\n\nPackages: ripgrep 14.1.0\n\nUpdates stay within >=14,<15, as recorded in the global manifest."
        );
        assert_eq!(details.dependencies, vec!["ripgrep"]);
        assert_eq!(
            details.homepage.as_deref(),
            Some("https://prefix.dev/channels/conda-forge/packages/ripgrep")
        );
        let mut foreign = id();
        foreign.backend = "conda".into();
        assert!(matches!(
            pixi.details(&foreign, &Cancellation::default()),
            Err(EngineError::NotFound)
        ));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn operations_refuse_or_skip_before_pixi_writes() {
        let (fake, home) = fake("refusals", Some("15.2.0"), "*");
        let mut pixi = backend(fake.clone());
        let run = |pixi: &mut Pixi<DevTools<Fake>>, operation: Operation, cancel: &Cancellation| {
            pixi.execute(&operation, cancel, &mut ignore)
        };
        let cancel = Cancellation::default();
        // Already installed: nothing to do.
        run(&mut pixi, Operation::Install(id()), &cancel).unwrap();
        assert!(matches!(
            run(
                &mut pixi,
                Operation::UpgradeAll { backend: ID.into() },
                &cancel
            ),
            Err(EngineError::Unsupported { .. })
        ));
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(matches!(
            run(&mut pixi, Operation::Remove(id()), &cancelled),
            Err(EngineError::Cancelled)
        ));
        assert!(
            !fake
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call.starts_with("global install")
                    || call.starts_with("global uninstall"))
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn an_update_that_moves_backwards_is_reported() {
        // An unmodeled spec leaves the update to pixi, which here installs
        // an older build than the one that was there.
        let (fake, home) = fake("backwards", Some("15.2.0"), ">=14|<2");
        let mut pixi = backend(fake);
        let error = pixi
            .execute(
                &Operation::Upgrade(id()),
                &Cancellation::default(),
                &mut ignore,
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("not in the expected state"),
            "{error}"
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn environments_without_a_recorded_spec_have_unknown_updates() {
        let (fake, home) = fake("unplaced", None, "*");
        let pixi = backend(fake);
        let cancel = Cancellation::default();
        let manifest = pixi.manifest();
        let environment = |name: &str, dependencies: &[&str]| GlobalEnvironment {
            name: name.into(),
            dependencies: dependencies
                .iter()
                .map(|name| Dependency {
                    name: (*name).into(),
                    version: "1.0".into(),
                })
                .collect(),
            exposed: vec![],
        };
        // Two packages, neither named like the environment: no main package.
        let tools = environment("tools", &["jq", "yq"]);
        assert_eq!(pixi.candidate(&tools, &manifest, &cancel).unwrap(), None);
        let row = Pixi::<DevTools<Fake>>::package(&tools, None);
        assert_eq!(row.installed_version.as_deref(), Some("unknown"));
        assert_eq!(row.summary, "pixi global environment");
        // Not in the manifest, or no spec recorded for its package.
        for unrecorded in [
            environment("jq", &["jq"]),
            environment("ripgrep", &["other"]),
        ] {
            assert_eq!(
                pixi.candidate(&unrecorded, &manifest, &cancel).unwrap(),
                None
            );
        }
        assert!(pixi.may_have("ripgrep") && !pixi.may_have("-rf"));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn subdirs_follow_the_platform() {
        assert_eq!(subdir_for("macos", "aarch64"), "osx-arm64");
        assert_eq!(subdir_for("macos", "x86_64"), "osx-64");
        assert_eq!(subdir_for("linux", "aarch64"), "linux-aarch64");
        assert_eq!(subdir_for("linux", "x86_64"), "linux-64");
    }

    #[test]
    fn unrecognized_manifest_lines_are_skipped() {
        let manifest = read_manifest(
            r#"[envs.jq]
channels = "conda-forge"
dependencies = "jq"
stray line

[envs.jq.exposed]
jq = "jq"

[envs.yq]
dependencies = { yq = "*" }
"#,
        );
        // Only the inline table counts; the rest is not recognized.
        assert_eq!(manifest["jq"], ManifestEnvironment::default());
        assert_eq!(manifest["yq"].specs["yq"], "*");
        assert_eq!(manifest.len(), 2);
    }

    #[test]
    fn unmodeled_globs_and_operators_are_unknown() {
        for (version, spec, expected) in [
            ("1.2.3", "1.*.*", None),
            ("1.2.3", ">=1.*", None),
            ("1.2.3", ">1*", None),
            ("1.2.3", "!=1.2.*", Some(false)),
            ("1.3.0", "!=1.2.*", Some(true)),
            ("1.2.3", "<=1.2.3", Some(true)),
            ("1.2.3", ">1.2.3", Some(false)),
            ("1.2.3", "!=1.2.3", Some(false)),
            ("1.2.3", "", Some(true)),
        ] {
            assert_eq!(satisfies(version, spec), expected, "{version} {spec}");
        }
    }

    #[test]
    fn the_newest_release_within_the_spec_is_current() {
        let (fake, home) = fake("current", Some("15.2.0"), "*");
        let mut pixi = backend(fake);
        let rows = pixi.installed(&Cancellation::default()).unwrap();
        assert_eq!(rows[0].update, UpdateAvailability::Current);
        assert_eq!(rows[0].candidate_version, None);
        // Invalid names are never offered, and nothing is searched for them.
        assert!(pixi
            .search("No Such", &Cancellation::default())
            .unwrap()
            .is_empty());
        assert_eq!(pixi.id(), "pixi");
        assert!(pixi.capabilities().contains(&Capability::Install));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_failing_pixi_is_unavailable_or_an_error() {
        let (fake, home) = fake("broken", Some("15.2.0"), "*");
        let run = |broken: fn() -> ExecutionError| {
            let mut pixi = backend(Fake {
                broken: Some(broken),
                ..fake.clone()
            });
            (
                pixi.detect(&Cancellation::default()),
                pixi.installed(&Cancellation::default()),
            )
        };
        let (detected, _) = run(|| ExecutionError::Disabled("pixi not found".into()));
        assert_eq!(
            detected.unwrap(),
            Availability::Unavailable("pixi not found".into())
        );
        let (detected, _) = run(|| ExecutionError::Cancelled);
        assert!(matches!(detected, Err(EngineError::Cancelled)));
        let (detected, listed) = run(|| ExecutionError::TimedOut);
        assert!(matches!(
            detected,
            Err(EngineError::Execution(ExecutionError::TimedOut))
        ));
        assert!(matches!(
            listed,
            Err(EngineError::Execution(ExecutionError::TimedOut))
        ));
        std::fs::remove_dir_all(home).unwrap();
    }
}
