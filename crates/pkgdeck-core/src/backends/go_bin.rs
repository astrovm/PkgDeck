//! Programs installed globally with `go install`.
//!
//! Only the directory `go install` writes to is managed: GOBIN, else the
//! first GOPATH entry's `bin`. Each Go executable there is a row named after
//! its file; the build info Go embeds (`go version -m`) says which package
//! and module version it was built from. Project modules are never touched.
use super::{bytes, invalid, NativeTransport, Transport};
use crate::{engine::*, package::*, process::*};
use serde::Deserialize;
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

const ID: &str = "go";
const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Upgrade,
];
/// What `go version -m` prints for a main module built from local source.
const DEVEL: &str = "(devel)";

pub struct GoBinaries<T = NativeTransport> {
    transport: T,
    /// Modules whose update check failed during the last query.
    errors: Vec<EngineError>,
}

/// One Go executable and the build info embedded in it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Binary {
    /// File name inside the bin directory.
    name: String,
    /// Toolchain it was built with, such as `go1.22.2`.
    toolchain: String,
    /// Main package path.
    path: Option<String>,
    /// Main module path and version (`(devel)` for local builds).
    module: Option<String>,
    version: Option<String>,
    /// A `=>` replacement: the module didn't come from its own path.
    replaced: bool,
}

impl Binary {
    /// The module version `go install <path>@latest` can move, if any.
    fn release(&self) -> Option<&str> {
        if self.replaced {
            return None;
        }
        self.version.as_deref().filter(|version| *version != DEVEL)
    }
}

/// `go list -m -json` output, one object per query.
#[derive(Deserialize, Debug)]
struct ModuleInfo {
    #[serde(rename = "Path", default)]
    path: String,
    #[serde(rename = "Version", default)]
    version: Option<String>,
    #[serde(rename = "Error", default)]
    error: Option<ModuleError>,
}
#[derive(Deserialize, Debug)]
struct ModuleError {
    #[serde(rename = "Err", default)]
    err: String,
}

/// A file name in the bin directory.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with(['-', '.'])
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}

/// A Go package path such as `golang.org/x/tools/cmd/stringer`: a
/// lowercase domain-like first element, then conservative path elements.
fn valid_path(path: &str) -> bool {
    let elements: Vec<&str> = path.split('/').collect();
    let first = elements[0];
    path.len() <= 256
        && elements.len() >= 2
        && elements.len() <= 16
        && first.contains('.')
        && first
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-".contains(&b))
        && elements.iter().all(|element| {
            !element.is_empty()
                && !element.starts_with(['-', '.'])
                && !element.ends_with('.')
                && element
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._~+-".contains(&b))
        })
}

/// The file `go install` writes for a package: its last element, or the
/// one before a major version suffix (`example.com/tool/v2` → `tool`).
fn binary_name(path: &str) -> Option<&str> {
    let mut elements = path.rsplit('/');
    let last = elements.next()?;
    let major = last.strip_prefix('v').is_some_and(|n| {
        !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && !n.starts_with('0')
    });
    let name = if major {
        elements.next().filter(|_| path.contains('/'))?
    } else {
        last
    };
    valid_name(name).then_some(name)
}

/// Parse `go version -m` for files directly inside `dir`. Anything else,
/// including files in subdirectories, is skipped.
fn parse_build_info(text: &str, dir: &Path) -> Vec<Binary> {
    let prefix = format!("{}/", dir.display());
    let mut binaries: Vec<Binary> = vec![];
    // Whether detail lines belong to a file that is kept.
    let mut current = false;
    for line in text.lines() {
        if let Some(detail) = line.strip_prefix('\t') {
            let Some(binary) = binaries.last_mut().filter(|_| current) else {
                continue;
            };
            let fields: Vec<&str> = detail.split('\t').collect();
            match fields.as_slice() {
                ["path", path, ..] => binary.path = Some((*path).into()),
                ["mod", module, version, ..] => {
                    binary.module = Some((*module).into());
                    binary.version = Some((*version).into());
                }
                ["=>", ..] => binary.replaced = true,
                _ => {}
            }
            continue;
        }
        current = false;
        let Some((name, toolchain)) = line
            .strip_prefix(&prefix)
            .and_then(|rest| rest.split_once(": "))
        else {
            continue;
        };
        if valid_name(name) && !binaries.iter().any(|b| b.name == name) {
            binaries.push(Binary {
                name: name.into(),
                toolchain: toolchain.trim().into(),
                ..Binary::default()
            });
            current = true;
        }
    }
    binaries
}

/// `v1.2.3-pre+incompatible` → semver without the `v` and build metadata.
fn semver(version: &str) -> Option<semver::Version> {
    let version = version.strip_prefix('v')?;
    let version = version.split_once('+').map_or(version, |(v, _)| v);
    semver::Version::parse(version).ok()
}

/// A pseudo-version names a commit rather than a release:
/// `v0.0.0-20240101120000-abcdefabcdef` and its `-pre.0.` / `-0.` forms.
fn is_pseudo(version: &str) -> bool {
    let Some(parsed) = semver(version) else {
        return false;
    };
    let pre = parsed.pre.as_str();
    let mut parts = pre.rsplitn(3, ['-', '.']);
    let (Some(hash), Some(time)) = (parts.next(), parts.next()) else {
        return false;
    };
    time.len() == 14
        && time.bytes().all(|b| b.is_ascii_digit())
        && hash.len() == 12
        && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether `latest` is an update for `installed`. A commit (pseudo-version)
/// is never offered as one: only a newer release is.
fn newer(installed: &str, latest: &str) -> Option<bool> {
    let (from, to) = (semver(installed)?, semver(latest)?);
    if is_pseudo(latest) {
        return Some(false);
    }
    Some(to.cmp(&from) == Ordering::Greater)
}

fn user_scope() -> Scope {
    Scope::User {
        uid: rustix::process::getuid().as_raw(),
    }
}

fn pkg_go_dev(path: &str) -> String {
    format!("https://pkg.go.dev/{path}")
}

impl<T: Transport> GoBinaries<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            errors: vec![],
        }
    }

    fn go(&self, args: &[&str], cancel: &Cancellation) -> Result<String, EngineError> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let output = bytes(ID, self.transport.dev_tool(ID, &args, cancel, false)?)?;
        String::from_utf8(output).map_err(|error| invalid(ID, error))
    }

    /// Where `go install` puts programs: GOBIN, else GOPATH's first entry.
    fn bin_dir(&self, cancel: &Cancellation) -> Result<PathBuf, EngineError> {
        let text = self.go(&["env", "-json", "GOBIN", "GOPATH"], cancel)?;
        let env: BTreeMap<String, String> =
            serde_json::from_str(&text).map_err(|error| invalid(ID, error))?;
        let absolute = |path: PathBuf| path.is_absolute().then_some(path);
        let dir = env
            .get("GOBIN")
            .filter(|dir| !dir.is_empty())
            .and_then(|dir| absolute(dir.into()))
            .or_else(|| {
                std::env::split_paths(env.get("GOPATH")?)
                    .find(|path| !path.as_os_str().is_empty())
                    .and_then(absolute)
                    .map(|path| path.join("bin"))
            })
            .ok_or_else(|| invalid(ID, "go env names neither GOBIN nor an absolute GOPATH"))?;
        // Normalized, so `go version` prints the same prefix back.
        Ok(dir.components().collect())
    }

    /// Every Go program in the bin directory, in one `go version` call.
    fn binaries(&self, dir: &Path, cancel: &Cancellation) -> Result<Vec<Binary>, EngineError> {
        if !dir.is_dir() {
            return Ok(vec![]);
        }
        let dir_arg = dir.to_string_lossy();
        let text = self.go(&["version", "-m", "--", &dir_arg], cancel)?;
        let mut found = parse_build_info(&text, dir);
        // Only regular files: never a symlink pointing somewhere else.
        found.retain(|binary| {
            std::fs::symlink_metadata(dir.join(&binary.name)).is_ok_and(|meta| meta.is_file())
        });
        found.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(found)
    }

    /// The build info of one file, or `None` when it isn't a Go program.
    fn probe(
        &self,
        dir: &Path,
        name: &str,
        cancel: &Cancellation,
    ) -> Result<Option<Binary>, EngineError> {
        let file = dir.join(name);
        if !std::fs::symlink_metadata(&file).is_ok_and(|meta| meta.is_file()) {
            return Ok(None);
        }
        let args: Vec<OsString> = vec!["version".into(), "-m".into(), "--".into(), file.into()];
        let output = match self.transport.dev_tool(ID, &args, cancel, false) {
            // `go version` fails for files without Go build info.
            Err(ExecutionError::Failed(_)) => return Ok(None),
            result => bytes(ID, result?)?,
        };
        let text = String::from_utf8_lossy(&output);
        Ok(parse_build_info(&text, dir)
            .into_iter()
            .find(|binary| binary.name == name))
    }

    /// `module@latest` for each module, in one `go list` call. Each answer
    /// is the latest version or the error Go reported for that module.
    fn latest(
        &self,
        modules: &[&str],
        cancel: &Cancellation,
    ) -> Result<BTreeMap<String, Result<String, String>>, EngineError> {
        if modules.is_empty() {
            return Ok(BTreeMap::new());
        }
        let queries: Vec<String> = modules.iter().map(|m| format!("{m}@latest")).collect();
        let mut args = vec!["list", "-m", "-e", "-json", "--"];
        args.extend(queries.iter().map(String::as_str));
        let text = self.go(&args, cancel)?;
        let mut answers = BTreeMap::new();
        for info in serde_json::Deserializer::from_str(&text).into_iter::<ModuleInfo>() {
            let info = info.map_err(|error| invalid(ID, error))?;
            let answer = match (info.error, info.version) {
                (Some(error), _) => Err(error.err),
                (None, Some(version)) if semver(&version).is_some() => Ok(version),
                (None, _) => Err("no version reported".into()),
            };
            answers.insert(info.path, answer);
        }
        Ok(answers)
    }

    fn package(binary: &Binary, candidate: Option<Option<String>>) -> Package {
        let summary = match (&binary.module, binary.version.as_deref()) {
            (Some(module), Some(DEVEL)) => {
                format!("Go program built from local source · {module}")
            }
            (Some(module), _) if binary.replaced => {
                format!("Go program · {module} (replaced)")
            }
            (Some(module), _) => format!("Go program · {module}"),
            (None, _) => "Go program without module information".into(),
        };
        Package {
            id: PackageId {
                backend: ID.into(),
                name: binary.name.clone(),
                architecture: "unknown".into(),
                scope: user_scope(),
                remote: None,
                reference: binary.path.clone(),
            },
            display_name: binary.name.clone(),
            summary,
            installed_version: Some(binary.version.clone().unwrap_or_else(|| "unknown".into())),
            update: match &candidate {
                Some(Some(_)) => UpdateAvailability::Available,
                Some(None) => UpdateAvailability::Current,
                None => UpdateAvailability::Unknown,
            },
            candidate_version: candidate.flatten(),
            icon: None,
            component_ids: vec![],
            homepages: binary.path.iter().map(|path| pkg_go_dev(path)).collect(),
            adopt_with: None,
        }
    }

    /// Installed rows; `check` asks the module proxy for newer releases.
    fn rows(&mut self, check: bool, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.errors.clear();
        let dir = self.bin_dir(cancel)?;
        let binaries = self.binaries(&dir, cancel)?;
        let answers = if check {
            let mut modules: Vec<&str> = binaries
                .iter()
                .filter(|binary| binary.release().is_some())
                .filter_map(|binary| binary.module.as_deref())
                .collect();
            modules.sort_unstable();
            modules.dedup();
            match self.latest(&modules, cancel) {
                Ok(answers) => answers,
                Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
                // Offline or an old Go: the programs are still listed.
                Err(error) => {
                    self.errors.push(error);
                    BTreeMap::new()
                }
            }
        } else {
            BTreeMap::new()
        };
        let mut reported = std::collections::BTreeSet::new();
        let rows = binaries
            .iter()
            .map(|binary| {
                let answer = binary
                    .release()
                    .zip(binary.module.as_deref())
                    .and_then(|(version, module)| Some((version, module, answers.get(module)?)));
                let candidate = match answer {
                    Some((version, _, Ok(latest))) => {
                        newer(version, latest).map(|newer| newer.then(|| latest.clone()))
                    }
                    Some((_, module, Err(error))) => {
                        if reported.insert(module) {
                            self.errors.push(invalid(
                                ID,
                                format!("could not check {module} for updates: {error}"),
                            ));
                        }
                        None
                    }
                    None => None,
                };
                Self::package(binary, candidate)
            })
            .collect();
        Ok(rows)
    }

    /// An exact install offer for a package path, verified against the
    /// module proxy: the longest prefix that is a module with a version.
    fn offer(&mut self, path: &str, cancel: &Cancellation) -> Result<Option<Package>, EngineError> {
        let Some(name) = binary_name(path) else {
            return Ok(None);
        };
        let prefixes: Vec<&str> = path
            .match_indices('/')
            .map(|(at, _)| &path[..at])
            .skip(1)
            .chain([path])
            .collect();
        let answers = match self.latest(&prefixes, cancel) {
            Ok(answers) => answers,
            Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
            Err(error) => {
                self.errors.push(error);
                return Ok(None);
            }
        };
        let Some((module, version)) = prefixes.iter().rev().find_map(|prefix| {
            let version = answers.get(*prefix)?.as_ref().ok()?;
            Some((*prefix, version.clone()))
        }) else {
            return Ok(None);
        };
        Ok(Some(Package {
            id: PackageId {
                backend: ID.into(),
                name: name.into(),
                architecture: "unknown".into(),
                scope: user_scope(),
                remote: None,
                reference: Some(path.into()),
            },
            display_name: name.into(),
            summary: format!("Install {path} with go install · {module}"),
            installed_version: None,
            candidate_version: Some(version),
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![pkg_go_dev(path)],
            adopt_with: None,
        }))
    }

    /// Run `go install <path>@latest`.
    fn install(
        &self,
        path: &str,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<bool, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let target = format!("{path}@latest");
        progress(Progress::Message(format!("Running go install {target}")));
        let args: Vec<OsString> = vec!["install".into(), "--".into(), target.into()];
        let completion = self.transport.dev_tool(ID, &args, cancel, true)?;
        let deferred = completion.cancellation_deferred;
        bytes(ID, completion)?;
        Ok(deferred)
    }
}

impl<T: Transport> Backend for GoBinaries<T> {
    fn id(&self) -> &str {
        ID
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        match self
            .transport
            .dev_tool(ID, &["version".into()], cancel, false)
        {
            Ok(_) => Ok(Availability::Available),
            Err(ExecutionError::Disabled(reason)) => Ok(Availability::Unavailable(reason)),
            Err(ExecutionError::Cancelled) => Err(EngineError::Cancelled),
            Err(error) => Err(error.into()),
        }
    }
    fn query_errors(&self) -> Vec<EngineError> {
        self.errors.clone()
    }
    fn may_have(&self, name: &str) -> bool {
        valid_name(name) || valid_path(name)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let query = query.trim();
        let mut rows: Vec<Package> = self
            .rows(false, cancel)?
            .into_iter()
            .filter(|row| {
                search_matches(&row.id.name, query)
                    || row
                        .id
                        .reference
                        .as_deref()
                        .is_some_and(|path| search_matches(path, query))
            })
            .collect();
        // A package path can be installed when it isn't already.
        if valid_path(query)
            && !rows
                .iter()
                .any(|row| row.id.reference.as_deref() == Some(query))
        {
            rows.extend(self.offer(query, cancel)?);
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
        if id.backend != ID || !valid_name(&id.name) {
            return Err(EngineError::NotFound);
        }
        let dir = self.bin_dir(cancel)?;
        let binary = self
            .probe(&dir, &id.name, cancel)?
            .ok_or(EngineError::NotFound)?;
        // Only this program's module is checked, and a failed check leaves
        // its update state unknown.
        let candidate = match (binary.release(), binary.module.as_deref()) {
            (Some(version), Some(module)) => match self.latest(&[module], cancel) {
                Ok(mut answers) => match answers.remove(module) {
                    Some(Ok(latest)) => {
                        newer(version, &latest).map(|newer| newer.then_some(latest))
                    }
                    _ => None,
                },
                Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
                Err(_) => None,
            },
            _ => None,
        };
        let package = Self::package(&binary, candidate);
        if package.id != *id {
            return Err(EngineError::NotFound);
        }
        let mut description = format!(
            "{} in {}, built with {}.",
            id.name,
            dir.display(),
            binary.toolchain
        );
        match (&binary.path, &binary.module) {
            (Some(path), Some(module)) => {
                description.push_str(&format!(
                    " Package {path} from module {module}. Updates run go install {path}@latest."
                ));
            }
            (Some(path), None) => description.push_str(&format!(" Package {path}.")),
            _ => {}
        }
        if binary.release().is_none() {
            description.push_str(
                " It wasn't installed from a published module version, so it isn't updated here.",
            );
        }
        Ok(PackageDetails {
            package,
            description,
            homepage: binary.path.as_deref().map(pkg_go_dev),
            dependencies: vec![],
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let id = match operation {
            Operation::Install(id) | Operation::Upgrade(id) | Operation::Remove(id) => id,
            other => return Err(self.unsupported(other.capability())),
        };
        if id.backend != ID
            || !valid_name(&id.name)
            || id
                .reference
                .as_deref()
                .is_some_and(|path| !valid_path(path))
        {
            return Err(invalid(ID, "foreign or invalid Go program"));
        }
        let dir = self.bin_dir(cancel)?;
        let before = self.probe(&dir, &id.name, cancel)?;
        let fresh = Cancellation::default();
        let deferred = match operation {
            Operation::Install(_) => {
                let path = id
                    .reference
                    .as_deref()
                    .filter(|path| binary_name(path) == Some(id.name.as_str()))
                    .ok_or_else(|| invalid(ID, "install needs the program's package path"))?;
                let file = dir.join(&id.name);
                if std::fs::symlink_metadata(&file).is_ok()
                    && before.as_ref().and_then(|b| b.path.as_deref()) != Some(path)
                {
                    return Err(invalid(
                        ID,
                        format!(
                            "{} already exists and wasn't built from {path}; not replacing it",
                            file.display()
                        ),
                    ));
                }
                let deferred = self.install(path, cancel, progress)?;
                // Verify with a fresh read; writes may finish after cancellation.
                let after = self.probe(&dir, &id.name, &fresh)?;
                if after.and_then(|b| b.path).as_deref() != Some(path) {
                    return Err(invalid(
                        ID,
                        format!(
                            "go install finished, but {} is not in {}",
                            id.name,
                            dir.display()
                        ),
                    ));
                }
                deferred
            }
            Operation::Upgrade(_) => {
                let before = before.ok_or(EngineError::NotFound)?;
                let path = before
                    .path
                    .clone()
                    .filter(|path| id.reference.as_deref().is_none_or(|r| r == path))
                    .ok_or(EngineError::NotFound)?;
                let (Some(version), Some(module)) = (before.release(), before.module.as_deref())
                else {
                    return Err(invalid(
                        ID,
                        format!(
                            "{} wasn't installed from a published module version; reinstall it with go install",
                            id.name
                        ),
                    ));
                };
                if binary_name(&path) != Some(id.name.as_str()) {
                    return Err(invalid(
                        ID,
                        format!(
                            "{} was renamed; go install would write a separate file",
                            id.name
                        ),
                    ));
                }
                let latest = self
                    .latest(&[module], cancel)?
                    .remove(module)
                    .and_then(Result::ok);
                let deferred = self.install(&path, cancel, progress)?;
                let after = self.probe(&dir, &id.name, &fresh)?;
                let after_version = after
                    .as_ref()
                    .filter(|b| b.path.as_deref() == Some(path.as_str()))
                    .and_then(|b| b.version.clone());
                // Moved, or was already at the latest release.
                let ok = after_version.as_deref().is_some_and(|now| {
                    now != version
                        || latest
                            .as_deref()
                            .is_some_and(|latest| newer(now, latest) == Some(false))
                });
                if !ok {
                    return Err(invalid(
                        ID,
                        format!("go install finished, but {} is still at {version}", id.name),
                    ));
                }
                deferred
            }
            _ => {
                // Only a Go program in the bin directory, exactly as listed.
                let before = before.ok_or_else(|| {
                    invalid(
                        ID,
                        format!("{} in {} is not a Go program", id.name, dir.display()),
                    )
                })?;
                if id.reference.is_some() && before.path != id.reference {
                    return Err(invalid(
                        ID,
                        format!("{} changed; refresh and retry", id.name),
                    ));
                }
                if cancel.requested() {
                    return Err(EngineError::Cancelled);
                }
                let file = dir.join(&id.name);
                progress(Progress::Message(format!("Removing {}", file.display())));
                std::fs::remove_file(&file)
                    .map_err(|error| ExecutionError::Io(format!("{}: {error}", file.display())))?;
                false
            }
        };
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

    fn backend(fake: Fake) -> GoBinaries<DevTools<Fake>> {
        GoBinaries::new(DevTools(fake))
    }
    fn ignore(_: Progress) {}

    /// `go version -m ~/go/bin` from Go 1.27.1 on linux/arm64 (some build
    /// lines dropped), plus a program in a subdirectory, which is skipped.
    const REAL: &str = "\
{DIR}/hello: go1.27.1
\tpath\texample.com/hello
\tmod\texample.com/hello\t(devel)\t
\tbuild\t-buildmode=exe
\tbuild\t-compiler=gc
\tbuild\tCGO_ENABLED=1
\tbuild\tGOARCH=arm64
\tbuild\tGOOS=linux
\tbuild\tGOARM64=v8.0
{DIR}/stringer: go1.27.1
\tpath\tgolang.org/x/tools/cmd/stringer
\tmod\tgolang.org/x/tools\tv0.30.0\th1:BgcpHewrV5AUp2G9MebG4XPFI1E2W41zU1SaqVA9vJY=
\tdep\tgolang.org/x/mod\tv0.23.0\th1:Zb7khfcRGKk+kqfxFaP5tZqCnDZMjC5VtUBs87Hr6QM=
\tbuild\t-buildmode=exe
\tbuild\tGOARCH=arm64
{DIR}/sub/nested: go1.27.1
\tpath\texample.com/nested
\tmod\texample.com/nested\tv1.0.0\th1:x=
";

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

    /// (package path, module, version)
    type Built = (String, String, String);

    /// A fake `go` over a real temporary bin directory: files the fake
    /// "built" carry build info, anything else is not a Go program.
    /// A command line (or its start) that fails, and how.
    type Failure = (&'static str, fn() -> ExecutionError);

    #[derive(Clone)]
    struct Fake {
        dir: PathBuf,
        /// file name → (package path, module, version)
        built: Arc<Mutex<BTreeMap<String, Built>>>,
        /// module → latest version
        latest: Arc<Mutex<BTreeMap<String, String>>>,
        calls: Arc<Mutex<Vec<String>>>,
        /// GOBIN is unset, so programs go to GOPATH's first entry.
        gopath_only: bool,
        /// `go install` succeeds without writing anything.
        inert: bool,
        /// Commands starting with this fail, and how.
        fail: Option<Failure>,
    }
    impl Fake {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("pkgdeck-go-{tag}-{}", std::process::id()))
                .join("bin");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self {
                dir,
                built: Arc::default(),
                latest: Arc::default(),
                calls: Arc::default(),
                gopath_only: false,
                inert: false,
                fail: None,
            }
        }
        fn build(&self, name: &str, path: &str, module: &str, version: &str) {
            std::fs::write(self.dir.join(name), b"\x7fELF").unwrap();
            self.built
                .lock()
                .unwrap()
                .insert(name.into(), (path.into(), module.into(), version.into()));
        }
        fn block(&self, name: &str) -> Option<String> {
            let built = self.built.lock().unwrap();
            let (path, module, version) = built.get(name)?;
            self.dir.join(name).is_file().then(|| {
                // An empty path or module stands for a build without it.
                let line = |key: &str, value: String| {
                    if value.is_empty() {
                        String::new()
                    } else {
                        format!("\t{key}\t{value}\n")
                    }
                };
                let path = line("path", path.clone());
                let module = line(
                    "mod",
                    if module.is_empty() {
                        String::new()
                    } else {
                        format!("{module}\t{version}\th1:x=")
                    },
                );
                format!(
                    "{}/{name}: go1.25.1\n{path}{module}\tbuild\t-compiler=gc\n",
                    self.dir.display()
                )
            })
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }
    impl Drop for Fake {
        fn drop(&mut self) {
            if Arc::strong_count(&self.calls) == 1 {
                let _ = std::fs::remove_dir_all(self.dir.parent().unwrap());
            }
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
            assert_eq!(executable, "go");
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            let line = args.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            assert_eq!(write, args[0] == "install", "{args:?}");
            if let Some((_, error)) = self.fail.filter(|(failing, _)| line.starts_with(failing)) {
                return Err(error());
            }
            let dir = self.dir.display().to_string();
            match args
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .as_slice()
            {
                ["version"] => Ok(done("go version go1.25.1 linux/arm64\n", 0)),
                ["env", "-json", "GOBIN", "GOPATH"] if self.gopath_only => Ok(done(
                    &format!(
                        "{{\n\t\"GOBIN\": \"\",\n\t\"GOPATH\": \"{}:/else\"\n}}\n",
                        self.dir.parent().unwrap().display()
                    ),
                    0,
                )),
                ["env", "-json", "GOBIN", "GOPATH"] => Ok(done(
                    &format!(
                        "{{\n\t\"GOBIN\": \"{dir}/\",\n\t\"GOPATH\": \"/nowhere:/else\"\n}}\n"
                    ),
                    0,
                )),
                ["version", "-m", "--", target] if *target == dir => {
                    let names: Vec<String> = self.built.lock().unwrap().keys().cloned().collect();
                    Ok(done(
                        &names
                            .iter()
                            .filter_map(|n| self.block(n))
                            .collect::<String>(),
                        0,
                    ))
                }
                ["version", "-m", "--", file] => {
                    let name = file.strip_prefix(&format!("{dir}/")).unwrap();
                    match self.block(name) {
                        Some(text) => Ok(done(&text, 0)),
                        None => Err(ExecutionError::Failed(done("", 1))),
                    }
                }
                ["list", "-m", "-e", "-json", "--", queries @ ..] => {
                    let latest = self.latest.lock().unwrap();
                    let text: String = queries
                        .iter()
                        .map(|query| {
                            let module = query.strip_suffix("@latest").unwrap();
                            match latest.get(module) {
                                Some(version) => format!(
                                    "{{\n\t\"Path\": \"{module}\",\n\t\"Version\": \"{version}\",\n\t\"Query\": \"latest\"\n}}\n"
                                ),
                                None => format!(
                                    "{{\n\t\"Path\": \"{module}\",\n\t\"Error\": {{\n\t\t\"Err\": \"module {module}: not found\"\n\t}}\n}}\n"
                                ),
                            }
                        })
                        .collect();
                    Ok(done(&text, 0))
                }
                other => {
                    assert!(matches!(other, ["install", "--", _]), "{other:?}");
                    let path = other[2].strip_suffix("@latest").unwrap();
                    if self.inert {
                        return Ok(done("", 0));
                    }
                    let latest = self.latest.lock().unwrap().clone();
                    let (module, version) = latest
                        .iter()
                        .filter(|(module, _)| {
                            path == module.as_str() || path.starts_with(&format!("{module}/"))
                        })
                        .max_by_key(|(module, _)| module.len())
                        .ok_or_else(|| ExecutionError::Failed(done("", 1)))?;
                    self.build(binary_name(path).unwrap(), path, module, version);
                    Ok(done("", 0))
                }
            }
        }
    }

    fn installed_fake(tag: &str) -> Fake {
        let fake = Fake::new(tag);
        fake.build(
            "stringer",
            "golang.org/x/tools/cmd/stringer",
            "golang.org/x/tools",
            "v0.20.0",
        );
        fake.build("hello", "example.com/hello", "example.com/hello", DEVEL);
        fake.build(
            "tip",
            "github.com/a/tip",
            "github.com/a/tip",
            "v0.0.0-20240101120000-abcdefabcdef",
        );
        fake.build("gone", "github.com/a/gone", "github.com/a/gone", "v1.0.0");
        // Not a Go program, and a symlink to one: neither is listed.
        std::fs::write(fake.dir.join("script"), "#!/bin/sh\n").unwrap();
        std::os::unix::fs::symlink(fake.dir.join("stringer"), fake.dir.join("link")).unwrap();
        fake.built.lock().unwrap().insert(
            "link".into(),
            ("x.org/link".into(), "x.org/link".into(), "v1.0.0".into()),
        );
        let mut latest = fake.latest.lock().unwrap();
        latest.insert("golang.org/x/tools".into(), "v0.30.0".into());
        latest.insert(
            "github.com/a/tip".into(),
            "v0.0.0-20250101120000-bbbbbbbbbbbb".into(),
        );
        drop(latest);
        fake
    }

    #[test]
    fn parses_real_version_output() {
        let dir = Path::new("/home/u/go/bin");
        let found = parse_build_info(&REAL.replace("{DIR}", "/home/u/go/bin"), dir);
        assert_eq!(
            found,
            vec![
                Binary {
                    name: "hello".into(),
                    toolchain: "go1.27.1".into(),
                    path: Some("example.com/hello".into()),
                    module: Some("example.com/hello".into()),
                    version: Some(DEVEL.into()),
                    replaced: false,
                },
                Binary {
                    name: "stringer".into(),
                    toolchain: "go1.27.1".into(),
                    path: Some("golang.org/x/tools/cmd/stringer".into()),
                    module: Some("golang.org/x/tools".into()),
                    version: Some("v0.30.0".into()),
                    replaced: false,
                },
            ]
        );
        assert_eq!(found[0].release(), None);
        assert_eq!(found[1].release(), Some("v0.30.0"));
        let replaced = parse_build_info(
            "/b/x: go1.22.0\n\tpath\tx.org/x\n\tmod\tx.org/x\tv1.0.0\n\t=>\t../x\t(devel)\t\n",
            Path::new("/b"),
        );
        assert!(replaced[0].replaced);
        assert_eq!(replaced[0].release(), None);
    }

    #[test]
    fn versions_and_names() {
        assert!(is_pseudo("v0.0.0-20240101120000-abcdefabcdef"));
        assert!(is_pseudo("v1.2.4-0.20240101120000-abcdefabcdef"));
        assert!(is_pseudo("v1.2.4-rc.1.0.20240101120000-abcdefabcdef"));
        assert!(!is_pseudo("v1.2.4-rc.1"));
        assert_eq!(newer("v0.20.0", "v0.30.0"), Some(true));
        assert_eq!(newer("v0.30.0", "v0.30.0"), Some(false));
        assert_eq!(
            newer("v2.0.0+incompatible", "v2.1.0+incompatible"),
            Some(true)
        );
        assert_eq!(newer("v1.0.0-rc.1", "v1.0.0"), Some(true));
        // A commit build moves only to a newer release, never another commit.
        assert_eq!(
            newer("v0.0.0-20240101120000-abcdefabcdef", "v0.1.0"),
            Some(true)
        );
        assert_eq!(
            newer(
                "v0.0.0-20240101120000-abcdefabcdef",
                "v0.0.0-20250101120000-abcdefabcdef"
            ),
            Some(false)
        );
        assert_eq!(
            newer("v1.2.4-0.20240101120000-abcdefabcdef", "v1.2.3"),
            Some(false)
        );
        assert_eq!(newer(DEVEL, "v1.0.0"), None);
        assert_eq!(
            binary_name("golang.org/x/tools/cmd/stringer"),
            Some("stringer")
        );
        assert_eq!(binary_name("github.com/a/tool/v2"), Some("tool"));
        assert_eq!(binary_name("gopkg.in/yaml.v3"), Some("yaml.v3"));
        assert!(valid_path("github.com/BurntSushi/toml/cmd/tomlv"));
        for bad in [
            "stringer",
            "-x.org/a",
            "x.org/-a",
            "x.org/../a",
            "x.org//a",
            "x.org/a@v1",
            "Example.com/a",
            "x.org/a b",
            "noDot/a",
        ] {
            assert!(!valid_path(bad), "{bad}");
        }
        assert!(!valid_name("-rf") && !valid_name("../x") && !valid_name("a/b"));
    }

    #[test]
    fn inventory_with_updates_devel_pseudo_and_failures() {
        let fake = installed_fake("inventory");
        let mut go = backend(fake.clone());
        let rows = go.installed(&Cancellation::default()).unwrap();
        let summary: Vec<_> = rows
            .iter()
            .map(|r| {
                (
                    r.id.name.as_str(),
                    r.installed_version.as_deref().unwrap(),
                    r.update.clone(),
                    r.candidate_version.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("gone", "v1.0.0", UpdateAvailability::Unknown, None),
                ("hello", DEVEL, UpdateAvailability::Unknown, None),
                (
                    "stringer",
                    "v0.20.0",
                    UpdateAvailability::Available,
                    Some("v0.30.0")
                ),
                (
                    "tip",
                    "v0.0.0-20240101120000-abcdefabcdef",
                    UpdateAvailability::Current,
                    None
                ),
            ]
        );
        assert_eq!(
            rows[2].id.reference.as_deref(),
            Some("golang.org/x/tools/cmd/stringer")
        );
        assert_eq!(rows[2].summary, "Go program · golang.org/x/tools");
        // One module failed its check; the rest of the inventory stands.
        let errors = go.query_errors();
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].to_string().contains("github.com/a/gone"),
            "{errors:?}"
        );
        // Local builds are never sent to the proxy; one batched query.
        let lists: Vec<String> = fake
            .calls()
            .into_iter()
            .filter(|c| c.starts_with("list"))
            .collect();
        assert_eq!(
            lists,
            vec!["list -m -e -json -- github.com/a/gone@latest github.com/a/tip@latest golang.org/x/tools@latest"]
        );
        let details = go.details(&rows[2].id, &Cancellation::default()).unwrap();
        assert!(details
            .description
            .contains("go install golang.org/x/tools/cmd/stringer@latest"));
    }

    #[test]
    fn upgrade_is_verified_and_local_builds_refused() {
        let fake = installed_fake("upgrade");
        let mut go = backend(fake.clone());
        let rows = go.installed(&Cancellation::default()).unwrap();
        let stringer = rows.iter().find(|r| r.id.name == "stringer").unwrap();
        go.execute(
            &Operation::Upgrade(stringer.id.clone()),
            &Cancellation::default(),
            &mut ignore,
        )
        .unwrap();
        assert!(fake
            .calls()
            .contains(&"install -- golang.org/x/tools/cmd/stringer@latest".into()));
        assert_eq!(fake.built.lock().unwrap()["stringer"].2, "v0.30.0");
        let hello = rows.iter().find(|r| r.id.name == "hello").unwrap();
        let error = go
            .execute(
                &Operation::Upgrade(hello.id.clone()),
                &Cancellation::default(),
                &mut ignore,
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("published module version"),
            "{error}"
        );
        let fake = installed_fake("current");
        fake.latest
            .lock()
            .unwrap()
            .insert("github.com/a/gone".into(), "v1.0.0".into());
        let mut go = backend(fake.clone());
        let rows = go.installed(&Cancellation::default()).unwrap();
        let gone = rows.iter().find(|r| r.id.name == "gone").unwrap();
        // Current already: a reinstall at the same version is fine.
        go.execute(
            &Operation::Upgrade(gone.id.clone()),
            &Cancellation::default(),
            &mut ignore,
        )
        .unwrap();
    }

    #[test]
    fn search_offers_package_paths_and_install_verifies() {
        let fake = installed_fake("install");
        fake.latest
            .lock()
            .unwrap()
            .insert("github.com/rakyll/hey".into(), "v0.1.4".into());
        let mut go = backend(fake.clone());
        // A plain name only filters what's installed, without the network.
        let found = go.search("STRING", &Cancellation::default()).unwrap();
        assert_eq!(found.len(), 1);
        assert!(!fake.calls().iter().any(|c| c.starts_with("list")));
        // An installed package path is not offered again.
        let found = go
            .search("golang.org/x/tools/cmd/stringer", &Cancellation::default())
            .unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].installed_version.is_some());
        let offers = go
            .search("github.com/rakyll/hey", &Cancellation::default())
            .unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].id.name, "hey");
        assert_eq!(offers[0].candidate_version.as_deref(), Some("v0.1.4"));
        assert!(!unverified_search_offer(&offers[0]));
        assert!(go
            .search("github.com/nobody/nothing/cmd/x", &Cancellation::default())
            .unwrap()
            .is_empty());
        go.execute(
            &Operation::Install(offers[0].id.clone()),
            &Cancellation::default(),
            &mut ignore,
        )
        .unwrap();
        assert!(fake.dir.join("hey").is_file());
        assert!(fake
            .calls()
            .contains(&"install -- github.com/rakyll/hey@latest".into()));
        // A file of the same name that isn't this program is never replaced.
        let mut clash = offers[0].id.clone();
        clash.name = "script".into();
        clash.reference = Some("github.com/a/script".into());
        let error = go
            .execute(
                &Operation::Install(clash),
                &Cancellation::default(),
                &mut ignore,
            )
            .unwrap_err();
        assert!(error.to_string().contains("not replacing"), "{error}");
        // Flag-like and mismatched identities are refused before running go.
        let mut bad = offers[0].id.clone();
        bad.reference = Some("-x.org/evil".into());
        assert!(go
            .execute(
                &Operation::Install(bad),
                &Cancellation::default(),
                &mut ignore
            )
            .is_err());
        let mut bad = offers[0].id.clone();
        bad.name = "other".into();
        assert!(go
            .execute(
                &Operation::Install(bad),
                &Cancellation::default(),
                &mut ignore
            )
            .is_err());
    }

    #[test]
    fn remove_deletes_only_go_programs_in_the_bin_dir() {
        let fake = installed_fake("remove");
        let mut go = backend(fake.clone());
        let rows = go.installed(&Cancellation::default()).unwrap();
        let stringer = rows.iter().find(|r| r.id.name == "stringer").unwrap();
        let mut script = stringer.id.clone();
        script.name = "script".into();
        script.reference = None;
        let error = go
            .execute(
                &Operation::Remove(script),
                &Cancellation::default(),
                &mut ignore,
            )
            .unwrap_err();
        assert!(error.to_string().contains("not a Go program"), "{error}");
        assert!(fake.dir.join("script").exists());
        let mut link = stringer.id.clone();
        link.name = "link".into();
        link.reference = None;
        assert!(go
            .execute(
                &Operation::Remove(link),
                &Cancellation::default(),
                &mut ignore
            )
            .is_err());
        assert!(fake.dir.join("link").exists());
        let mut traversal = stringer.id.clone();
        traversal.name = "../bin/stringer".into();
        assert!(go
            .execute(
                &Operation::Remove(traversal),
                &Cancellation::default(),
                &mut ignore
            )
            .is_err());
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(matches!(
            go.execute(
                &Operation::Remove(stringer.id.clone()),
                &cancelled,
                &mut ignore
            ),
            Err(EngineError::Cancelled)
        ));
        go.execute(
            &Operation::Remove(stringer.id.clone()),
            &Cancellation::default(),
            &mut ignore,
        )
        .unwrap();
        assert!(!fake.dir.join("stringer").exists());
        assert!(fake.dir.join("hello").exists());
    }

    #[test]
    fn details_explain_local_builds_and_refuse_other_rows() {
        let fake = installed_fake("details");
        let mut go = backend(fake.clone());
        let cancel = Cancellation::default();
        assert_eq!(go.detect(&cancel).unwrap(), Availability::Available);
        assert!(go.may_have("stringer") && go.may_have("golang.org/x/tools/cmd/stringer"));
        assert!(!go.may_have("-rf"));
        let rows = go.installed(&cancel).unwrap();
        let hello = rows.iter().find(|r| r.id.name == "hello").unwrap();
        assert_eq!(
            hello.summary,
            "Go program built from local source · example.com/hello"
        );
        let details = go.details(&hello.id, &cancel).unwrap();
        assert!(details
            .description
            .contains("It wasn't installed from a published module version"));
        assert_eq!(
            details.homepage.as_deref(),
            Some("https://pkg.go.dev/example.com/hello")
        );
        let mut foreign = hello.id.clone();
        foreign.backend = "cargo".into();
        assert!(matches!(
            go.details(&foreign, &cancel),
            Err(EngineError::NotFound)
        ));
        let mut script = hello.id.clone();
        script.name = "script".into();
        assert!(matches!(
            go.details(&script, &cancel),
            Err(EngineError::NotFound)
        ));
        assert!(matches!(
            go.execute(
                &Operation::UpgradeAll { backend: ID.into() },
                &cancel,
                &mut ignore
            ),
            Err(EngineError::Unsupported { .. })
        ));
    }

    #[test]
    fn stale_or_renamed_programs_are_left_alone() {
        let fake = installed_fake("stale");
        // Built from a package whose program would be named differently.
        fake.build("renamed", "example.com/tool", "example.com/tool", "v1.0.0");
        let mut go = backend(fake.clone());
        let cancel = Cancellation::default();
        let rows = go.installed(&cancel).unwrap();
        let renamed = rows.iter().find(|r| r.id.name == "renamed").unwrap();
        let error = go
            .execute(
                &Operation::Upgrade(renamed.id.clone()),
                &cancel,
                &mut ignore,
            )
            .unwrap_err();
        assert!(error.to_string().contains("was renamed"), "{error}");
        // A row listed from another package path than what is there now.
        let stringer = rows.iter().find(|r| r.id.name == "stringer").unwrap();
        let mut stale = stringer.id.clone();
        stale.reference = Some("golang.org/x/tools/cmd/other".into());
        let error = go
            .execute(&Operation::Remove(stale), &cancel, &mut ignore)
            .unwrap_err();
        assert!(error.to_string().contains("changed; refresh"), "{error}");
        assert!(fake.dir.join("stringer").exists());
        assert!(!fake.calls().iter().any(|c| c.starts_with("install")));
    }

    #[test]
    fn programs_are_found_in_gopath_without_gobin() {
        let mut fake = installed_fake("gopath");
        fake.gopath_only = true;
        let mut go = backend(fake.clone());
        let rows = go.search("stringer", &Cancellation::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(fake
            .calls()
            .contains(&format!("version -m -- {}", fake.dir.display())));
        // A cancelled install never runs go install.
        let cancel = Cancellation::default();
        cancel.cancel();
        let mut offer = rows[0].id.clone();
        offer.name = "hey".into();
        offer.reference = Some("github.com/rakyll/hey".into());
        assert!(matches!(
            go.execute(&Operation::Install(offer), &cancel, &mut ignore),
            Err(EngineError::Cancelled)
        ));
        assert!(!fake.calls().iter().any(|c| c.starts_with("install")));
    }

    #[test]
    fn foreign_lines_and_missing_module_info() {
        let found = parse_build_info(
            "/elsewhere/x: go1.22.0\n\tpath\tx.org/x\n\n/b/y: go1.22.0\n\tpath\ty.org/y\n",
            Path::new("/b"),
        );
        assert_eq!(
            found,
            vec![Binary {
                name: "y".into(),
                toolchain: "go1.22.0".into(),
                path: Some("y.org/y".into()),
                ..Binary::default()
            }]
        );
        assert!(!is_pseudo(DEVEL));
        let replaced = Binary {
            name: "x".into(),
            module: Some("x.org/x".into()),
            version: Some("v1.0.0".into()),
            replaced: true,
            ..Binary::default()
        };
        let row = GoBinaries::<DevTools<Fake>>::package(&replaced, None);
        assert_eq!(row.summary, "Go program · x.org/x (replaced)");
        let bare = GoBinaries::<DevTools<Fake>>::package(&Binary::default(), None);
        assert_eq!(bare.summary, "Go program without module information");
        assert_eq!(bare.installed_version.as_deref(), Some("unknown"));
        assert!(bare.homepages.is_empty());
    }

    #[test]
    fn a_missing_bin_dir_lists_nothing() {
        let fake = Fake::new("missing");
        std::fs::remove_dir(&fake.dir).unwrap();
        let mut go = backend(fake.clone());
        assert!(go.installed(&Cancellation::default()).unwrap().is_empty());
        assert!(go.query_errors().is_empty());
        assert_eq!(fake.calls(), vec!["env -json GOBIN GOPATH"]);
        assert_eq!(go.id(), "go");
        assert!(go.capabilities().contains(&Capability::Install));
    }

    #[test]
    fn detection_follows_the_go_command() {
        let detect = |error: fn() -> ExecutionError| {
            let mut fake = Fake::new("detect");
            fake.fail = Some(("version", error));
            backend(fake).detect(&Cancellation::default())
        };
        assert_eq!(
            detect(|| ExecutionError::Disabled("go not found".into())).unwrap(),
            Availability::Unavailable("go not found".into())
        );
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
    fn failed_update_checks_keep_the_inventory() {
        let fake = installed_fake("checks");
        // The proxy answers without a usable version.
        fake.latest
            .lock()
            .unwrap()
            .insert("github.com/a/gone".into(), "master".into());
        let mut go = backend(fake.clone());
        let cancel = Cancellation::default();
        let rows = go.installed(&cancel).unwrap();
        let gone = rows.iter().find(|r| r.id.name == "gone").unwrap();
        assert_eq!(gone.update, UpdateAvailability::Unknown);
        let errors: Vec<String> = go.query_errors().iter().map(|e| e.to_string()).collect();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("no version reported"), "{errors:?}");
        let details = go.details(&gone.id, &cancel).unwrap();
        assert_eq!(details.package.update, UpdateAvailability::Unknown);
        let stringer = rows.iter().find(|r| r.id.name == "stringer").unwrap();

        // The proxy can't be reached at all: rows stay, updates are unknown.
        let mut offline = fake.clone();
        offline.fail = Some(("list", || ExecutionError::TimedOut));
        let mut go = backend(offline);
        let rows = go.installed(&cancel).unwrap();
        assert_eq!(rows.len(), 4);
        assert!(rows
            .iter()
            .all(|row| row.update == UpdateAvailability::Unknown));
        assert!(matches!(
            go.query_errors().as_slice(),
            [EngineError::Execution(ExecutionError::TimedOut)]
        ));
        let found = go.search("github.com/rakyll/hey", &cancel).unwrap();
        assert!(found.is_empty());
        assert_eq!(go.query_errors().len(), 1);
        let details = go.details(&stringer.id, &cancel).unwrap();
        assert_eq!(details.package.update, UpdateAvailability::Unknown);

        // Cancelled during a check: nothing is returned.
        let mut cancelled = fake.clone();
        cancelled.fail = Some(("list", || ExecutionError::Cancelled));
        let mut go = backend(cancelled);
        assert!(matches!(go.installed(&cancel), Err(EngineError::Cancelled)));
        assert!(matches!(
            go.search("github.com/rakyll/hey", &cancel),
            Err(EngineError::Cancelled)
        ));
        assert!(matches!(
            go.details(&stringer.id, &cancel),
            Err(EngineError::Cancelled)
        ));
    }

    #[test]
    fn details_of_builds_without_module_info_or_another_path() {
        let fake = installed_fake("plain");
        fake.build("plain", "example.com/plain", "", "");
        let mut go = backend(fake.clone());
        let cancel = Cancellation::default();
        let rows = go.installed(&cancel).unwrap();
        let plain = rows.iter().find(|r| r.id.name == "plain").unwrap();
        assert_eq!(plain.summary, "Go program without module information");
        let details = go.details(&plain.id, &cancel).unwrap();
        assert!(
            details.description.ends_with(
                " Package example.com/plain. It wasn't installed from a published module version, so it isn't updated here."
            ),
            "{}",
            details.description
        );
        // A row listed from another package path than what is there now.
        let mut stale = rows
            .iter()
            .find(|r| r.id.name == "stringer")
            .unwrap()
            .id
            .clone();
        stale.reference = Some("golang.org/x/tools/cmd/other".into());
        assert!(matches!(
            go.details(&stale, &cancel),
            Err(EngineError::NotFound)
        ));
        // Nothing embedded at all: only where it is and how it was built.
        fake.build("bare", "", "", "");
        let bare = go
            .installed(&cancel)
            .unwrap()
            .into_iter()
            .find(|r| r.id.name == "bare")
            .unwrap();
        let details = go.details(&bare.id, &cancel).unwrap();
        assert_eq!(
            details.description,
            format!(
                "bare in {}, built with go1.25.1. It wasn't installed from a published module version, so it isn't updated here.",
                fake.dir.display()
            )
        );
        assert_eq!(details.homepage, None);
        // A path whose program name isn't a valid file name is not offered.
        fake.calls.lock().unwrap().clear();
        assert!(go.search("x.org/a~b", &cancel).unwrap().is_empty());
        assert!(!fake.calls().iter().any(|c| c.starts_with("list")));
    }

    #[test]
    fn installs_and_upgrades_that_change_nothing_are_errors() {
        let mut fake = installed_fake("inert");
        fake.latest
            .lock()
            .unwrap()
            .insert("github.com/rakyll/hey".into(), "v0.1.4".into());
        fake.inert = true;
        let mut go = backend(fake.clone());
        let cancel = Cancellation::default();
        let offers = go.search("github.com/rakyll/hey", &cancel).unwrap();
        let error = go
            .execute(
                &Operation::Install(offers[0].id.clone()),
                &cancel,
                &mut ignore,
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("go install finished, but hey is not in"),
            "{error}"
        );
        let rows = go.installed(&cancel).unwrap();
        let stringer = rows.iter().find(|r| r.id.name == "stringer").unwrap();
        let error = go
            .execute(
                &Operation::Upgrade(stringer.id.clone()),
                &cancel,
                &mut ignore,
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("go install finished, but stringer is still at v0.20.0"),
            "{error}"
        );
        assert!(fake
            .calls()
            .contains(&"install -- golang.org/x/tools/cmd/stringer@latest".into()));
    }
}
