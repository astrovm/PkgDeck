//! .NET global tools: `dotnet tool install --global`.
//!
//! Only the user's global tools are managed; local tool manifests
//! (`.config/dotnet-tools.json`) belong to projects and are never read or
//! written. A row is one tool package, named by its package id as dotnet
//! prints it (lowercase). Update checks ask the configured NuGet feeds for
//! the newest stable release, the one `dotnet tool update --global` moves to.
use super::{bytes, invalid, NativeTransport, Transport};
use crate::{engine::*, package::*, process::*};
use serde::Deserialize;
use std::{cmp::Ordering, ffi::OsString};

const ID: &str = "dotnet";
const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Upgrade,
];

pub struct DotnetTools<T = NativeTransport> {
    transport: T,
    /// Update checks that failed during the last inventory.
    errors: Vec<EngineError>,
}

/// One installed global tool.
#[derive(Deserialize, Clone, Debug, PartialEq)]
struct Tool {
    #[serde(rename = "packageId")]
    id: String,
    version: String,
    #[serde(default)]
    commands: Vec<String>,
}

#[derive(Deserialize)]
struct ToolList {
    #[serde(default)]
    data: Vec<Tool>,
}

/// `dotnet package search --format json`.
#[derive(Deserialize)]
struct PackageSearch {
    #[serde(default)]
    problems: Vec<Problem>,
    #[serde(default, rename = "searchResult")]
    results: Vec<SourceResult>,
}
#[derive(Deserialize)]
struct SourceResult {
    #[serde(default, rename = "sourceName")]
    source: String,
    #[serde(default)]
    problems: Vec<Problem>,
    #[serde(default)]
    packages: Vec<Found>,
}
#[derive(Deserialize)]
struct Problem {
    #[serde(default)]
    text: String,
}
#[derive(Deserialize)]
struct Found {
    id: String,
    version: String,
}

/// A catalog match from `dotnet tool search`.
#[derive(Debug, PartialEq)]
struct Offer {
    id: String,
    version: String,
    authors: String,
}

/// NuGet package ids: letters, digits, `.`, `-` and `_`, up to 100
/// characters, never starting like a flag or a hidden file.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && !id.starts_with(['-', '.'])
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// NuGet version order: up to four numeric parts, then a release sorts
/// after its prereleases, whose dot-separated labels compare like SemVer.
/// Build metadata is ignored.
fn compare_versions(left: &str, right: &str) -> Ordering {
    fn split(version: &str) -> (Vec<u64>, Option<&str>) {
        let version = version.split('+').next().unwrap_or(version);
        let (release, pre) = match version.split_once('-') {
            Some((release, pre)) => (release, Some(pre)),
            None => (version, None),
        };
        let mut numbers: Vec<u64> = release
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect();
        while numbers.len() > 1 && numbers.last() == Some(&0) {
            numbers.pop();
        }
        (numbers, pre)
    }
    let (left_numbers, left_pre) = split(left);
    let (right_numbers, right_pre) = split(right);
    left_numbers
        .cmp(&right_numbers)
        .then_with(|| match (left_pre, right_pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(left), Some(right)) => {
                let mut left = left.split('.');
                let mut right = right.split('.');
                loop {
                    match (left.next(), right.next()) {
                        (None, None) => break Ordering::Equal,
                        (None, Some(_)) => break Ordering::Less,
                        (Some(_), None) => break Ordering::Greater,
                        (Some(a), Some(b)) => {
                            let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                                (Ok(a), Ok(b)) => a.cmp(&b),
                                (Ok(_), Err(_)) => Ordering::Less,
                                (Err(_), Ok(_)) => Ordering::Greater,
                                (Err(_), Err(_)) => a.to_lowercase().cmp(&b.to_lowercase()),
                            };
                            if order != Ordering::Equal {
                                break order;
                            }
                        }
                    }
                }
            }
        })
}

/// The JSON document in dotnet's output. A first run prints the welcome
/// banner on stdout before it unless DOTNET_NOLOGO is set.
fn json(text: &str) -> &str {
    if text.starts_with('{') {
        return text;
    }
    text.find("\n{").map_or(text, |at| &text[at + 1..])
}

/// `dotnet tool list --global` without `--format json` (SDKs before 8):
///
/// ```text
/// Package Id      Version      Commands
/// -------------------------------------
/// dotnetsay       2.1.7        dotnetsay
/// ```
fn parse_table(text: &str) -> Vec<Tool> {
    text.lines()
        .skip_while(|line| !line.starts_with("---"))
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let id = fields.next()?;
            let version = fields.next()?;
            valid_id(id).then(|| Tool {
                id: id.to_owned(),
                version: version.to_owned(),
                commands: fields
                    .flat_map(|field| field.split(','))
                    .filter(|command| !command.is_empty())
                    .map(str::to_owned)
                    .collect(),
            })
        })
        .collect()
}

/// `dotnet tool search`: a table sized to its contents. Ids and versions
/// never contain spaces; authors can, so they are cut at the header's
/// column positions.
fn parse_search(text: &str) -> Vec<Offer> {
    let mut lines = text.lines();
    let Some(header) = lines.find(|line| line.starts_with("Package ID")) else {
        return vec![];
    };
    let column = |name: &str| header.find(name).map(|at| header[..at].chars().count());
    let (authors, downloads) = (column("Authors"), column("Downloads"));
    lines
        .skip_while(|line| line.starts_with("---"))
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let id = fields.next()?.to_lowercase();
            let version = fields.next()?.to_owned();
            let authors = match (authors, downloads) {
                (Some(start), Some(end)) if end > start => line
                    .chars()
                    .skip(start)
                    .take(end - start)
                    .collect::<String>()
                    .trim()
                    .to_owned(),
                _ => String::new(),
            };
            valid_id(&id).then_some(Offer {
                id,
                version,
                authors,
            })
        })
        .collect()
}

/// The newest stable version in a `dotnet package search --exact-match`
/// reply. `Ok(None)`: no configured feed has the package.
fn newest_stable(text: &str, id: &str) -> Result<Option<String>, String> {
    let reply: PackageSearch =
        serde_json::from_str(json(text)).map_err(|error| error.to_string())?;
    let newest = reply
        .results
        .iter()
        .flat_map(|source| &source.packages)
        .filter(|found| found.id.eq_ignore_ascii_case(id) && !found.version.contains('-'))
        .map(|found| found.version.as_str())
        .max_by(|a, b| compare_versions(a, b))
        .map(str::to_owned);
    if newest.is_none() {
        let problems: Vec<String> = reply
            .problems
            .iter()
            .map(|problem| problem.text.clone())
            .chain(reply.results.iter().flat_map(|source| {
                source
                    .problems
                    .iter()
                    .map(|problem| format!("{}: {}", source.source, problem.text))
            }))
            .collect();
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
    }
    Ok(newest)
}

fn failure_text(error: &ExecutionError) -> String {
    match error {
        ExecutionError::Failed(result) => format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        ),
        _ => String::new(),
    }
}

impl<T: Transport> DotnetTools<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            errors: vec![],
        }
    }

    fn run(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<String, EngineError> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let output = bytes(ID, self.transport.dev_tool(ID, &args, cancel, write)?)?;
        String::from_utf8(output).map_err(|error| invalid(ID, error))
    }

    fn list(&self, cancel: &Cancellation) -> Result<Vec<Tool>, EngineError> {
        let args: Vec<OsString> = ["tool", "list", "--global", "--format", "json"]
            .iter()
            .map(OsString::from)
            .collect();
        match self.transport.dev_tool(ID, &args, cancel, false) {
            Ok(result) => {
                let text = String::from_utf8(bytes(ID, result)?).map_err(|e| invalid(ID, e))?;
                let list: ToolList =
                    serde_json::from_str(json(&text)).map_err(|error| invalid(ID, error))?;
                Ok(list.data)
            }
            // SDKs before 8 have no `--format`; read their table instead.
            Err(error) if failure_text(&error).contains("--format") => Ok(parse_table(&self.run(
                &["tool", "list", "--global"],
                cancel,
                false,
            )?)),
            Err(error) => Err(error.into()),
        }
    }

    fn find(&self, name: &str, cancel: &Cancellation) -> Result<Option<Tool>, EngineError> {
        Ok(self
            .list(cancel)?
            .into_iter()
            .find(|tool| tool.id.eq_ignore_ascii_case(name)))
    }

    /// The newest stable release on the configured feeds.
    fn latest(&self, id: &str, cancel: &Cancellation) -> Result<Option<String>, EngineError> {
        let text = self.run(
            &[
                "package",
                "search",
                "--exact-match",
                "--format",
                "json",
                "--",
                id,
            ],
            cancel,
            false,
        )?;
        newest_stable(&text, id).map_err(|reason| invalid(ID, format!("{id}: {reason}")))
    }

    /// `None`: unknown. `Some(None)`: nothing newer.
    fn candidate(tool: &Tool, latest: Option<String>) -> Option<Option<String>> {
        let latest = latest?;
        Some((compare_versions(&latest, &tool.version) == Ordering::Greater).then_some(latest))
    }

    fn id(name: &str) -> PackageId {
        PackageId {
            backend: ID.into(),
            name: name.to_lowercase(),
            architecture: "unknown".into(),
            scope: Scope::User {
                uid: rustix::process::getuid().as_raw(),
            },
            remote: None,
            reference: None,
        }
    }

    fn package(tool: &Tool, candidate: Option<Option<String>>) -> Package {
        let id = Self::id(&tool.id);
        Package {
            display_name: id.name.clone(),
            summary: if tool.commands.is_empty() {
                ".NET global tool".into()
            } else {
                format!(".NET global tool · {}", tool.commands.join(", "))
            },
            installed_version: Some(tool.version.clone()),
            update: match &candidate {
                Some(Some(_)) => UpdateAvailability::Available,
                Some(None) => UpdateAvailability::Current,
                None => UpdateAvailability::Unknown,
            },
            candidate_version: candidate.flatten(),
            icon: None,
            component_ids: vec![],
            homepages: vec![format!("https://www.nuget.org/packages/{}", id.name)],
            id,
            adopt_with: None,
        }
    }

    fn offer(offer: &Offer) -> Package {
        let id = Self::id(&offer.id);
        Package {
            display_name: id.name.clone(),
            summary: if offer.authors.is_empty() {
                "Install as a .NET global tool".into()
            } else {
                format!(".NET global tool by {}", offer.authors)
            },
            installed_version: None,
            candidate_version: Some(offer.version.clone()),
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![format!("https://www.nuget.org/packages/{}", id.name)],
            id,
            adopt_with: None,
        }
    }

    /// Installed rows with update checks. A failed check leaves that row's
    /// update unknown and is reported beside the rows; the first failure
    /// stops further checks, since it is almost always shared (offline, a
    /// feed down, or an SDK without `dotnet package search`).
    fn rows(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.errors.clear();
        let tools = self.list(cancel)?;
        let mut rows = Vec::with_capacity(tools.len());
        let mut checking = true;
        for tool in &tools {
            if cancel.requested() {
                return Err(EngineError::Cancelled);
            }
            let latest = if checking {
                match self.latest(&tool.id, cancel) {
                    Ok(latest) => latest,
                    Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
                    Err(error) => {
                        checking = false;
                        self.errors.push(error);
                        None
                    }
                }
            } else {
                None
            };
            rows.push(Self::package(tool, Self::candidate(tool, latest)));
        }
        Ok(rows)
    }

    fn search_catalog(
        &self,
        query: &str,
        cancel: &Cancellation,
    ) -> Result<Vec<Offer>, EngineError> {
        Ok(parse_search(&self.run(
            &["tool", "search", "--", query],
            cancel,
            false,
        )?))
    }

    fn check_id(id: &PackageId) -> Result<(), EngineError> {
        if id.backend != ID || !valid_id(&id.name) {
            return Err(invalid(
                ID,
                format!("{} is not a .NET tool package id", id.name),
            ));
        }
        Ok(())
    }

    /// Run one write and verify it with a fresh listing.
    fn write(
        &self,
        verb: &str,
        id: &PackageId,
        before: Option<&Tool>,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<bool, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let (label, command) = match verb {
            "install" => ("Installing", "install"),
            "update" => ("Updating", "update"),
            _ => ("Removing", "uninstall"),
        };
        progress(Progress::Message(format!(
            "{label} {} with dotnet tool",
            id.name
        )));
        let args: Vec<OsString> = ["tool", command, "--global", "--", &id.name]
            .iter()
            .map(OsString::from)
            .collect();
        let completion = self.transport.dev_tool(ID, &args, cancel, true)?;
        let deferred = completion.cancellation_deferred;
        let output = bytes(ID, completion)?;
        // The last line says what happened; the rest is PATH advice.
        if let Some(line) = String::from_utf8_lossy(&output)
            .lines()
            .rfind(|line| !line.trim().is_empty())
        {
            progress(Progress::Message(line.trim().into()));
        }
        // Writes may finish after cancellation; verification still runs.
        let after = self.find(&id.name, &Cancellation::default())?;
        let verified = match command {
            "install" => after.is_some(),
            "uninstall" => after.is_none(),
            _ => after.as_ref().is_some_and(|now| {
                before.is_none_or(|was| {
                    compare_versions(&now.version, &was.version) != Ordering::Less
                })
            }),
        };
        if !verified {
            return Err(invalid(
                ID,
                format!(
                    "dotnet finished, but {} is not in the expected state",
                    id.name
                ),
            ));
        }
        Ok(deferred)
    }
}

impl<T: Transport> Backend for DotnetTools<T> {
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
            // A runtime without an SDK has no `dotnet tool`.
            Err(ExecutionError::Failed(_)) => Ok(Availability::Unavailable(
                ".NET SDK not found; global tools need the SDK, not only the runtime".into(),
            )),
            Err(ExecutionError::Cancelled) => Err(EngineError::Cancelled),
            Err(error) => Err(error.into()),
        }
    }
    fn query_errors(&self) -> Vec<EngineError> {
        self.errors.clone()
    }
    fn may_have(&self, name: &str) -> bool {
        valid_id(name)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let query = query.trim().to_lowercase();
        let installed = self.list(cancel)?;
        let mut rows: Vec<Package> = installed
            .iter()
            .filter(|tool| tool.id.to_lowercase().contains(&query))
            .map(|tool| Self::package(tool, None))
            .collect();
        if query.is_empty() {
            return Ok(rows);
        }
        rows.extend(
            self.search_catalog(&query, cancel)?
                .iter()
                .filter(|offer| {
                    !installed
                        .iter()
                        .any(|tool| tool.id.eq_ignore_ascii_case(&offer.id))
                })
                .map(Self::offer),
        );
        Ok(rows)
    }
    fn lookup(&mut self, name: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let name = name.trim().to_lowercase();
        if !valid_id(&name) {
            return Ok(vec![]);
        }
        // An installed tool is answered without asking the feeds.
        if let Some(tool) = self.find(&name, cancel)? {
            return Ok(vec![Self::package(&tool, None)]);
        }
        Ok(self
            .search_catalog(&name, cancel)?
            .iter()
            .filter(|offer| offer.id == name)
            .map(Self::offer)
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
        if id.backend != ID {
            return Err(EngineError::NotFound);
        }
        let tool = self.find(&id.name, cancel)?.ok_or(EngineError::NotFound)?;
        let (candidate, note) = match self.latest(&tool.id, cancel) {
            Ok(latest) => (Self::candidate(&tool, latest), String::new()),
            Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
            Err(error) => (None, format!("\n\nUpdate check failed: {error}")),
        };
        let commands = if tool.commands.is_empty() {
            "none listed".into()
        } else {
            tool.commands.join(", ")
        };
        Ok(PackageDetails {
            description: format!(
                ".NET global tool, installed with `dotnet tool install --global`.\n\nCommands: {commands}\n\nUpdates move to the newest stable release on the configured NuGet feeds.{note}"
            ),
            homepage: Some(format!("https://www.nuget.org/packages/{}", tool.id)),
            package: Self::package(&tool, candidate),
            dependencies: vec![],
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let deferred = match operation {
            Operation::Install(id) => {
                Self::check_id(id)?;
                if self.find(&id.name, cancel)?.is_some() {
                    return Ok(OperationOutcome::default());
                }
                self.write("install", id, None, cancel, progress)?
            }
            Operation::Remove(id) => {
                Self::check_id(id)?;
                let tool = self.find(&id.name, cancel)?.ok_or(EngineError::NotFound)?;
                self.write("uninstall", id, Some(&tool), cancel, progress)?
            }
            Operation::Upgrade(id) => {
                Self::check_id(id)?;
                let tool = self.find(&id.name, cancel)?.ok_or(EngineError::NotFound)?;
                // Nothing newer: `dotnet tool update` would only reinstall.
                if let Ok(latest) = self.latest(&tool.id, cancel) {
                    if Self::candidate(&tool, latest) == Some(None) {
                        return Ok(OperationOutcome::default());
                    }
                }
                self.write("update", id, Some(&tool), cancel, progress)?
            }
            // No SDK-independent single command: update each tool behind.
            Operation::UpgradeAll { backend } if backend == ID => {
                let mut deferred = false;
                for tool in self.list(cancel)? {
                    if cancel.requested() {
                        return Err(EngineError::Cancelled);
                    }
                    let latest = self.latest(&tool.id, cancel)?;
                    if let Some(Some(_)) = Self::candidate(&tool, latest) {
                        let id = Self::id(&tool.id);
                        deferred |= self.write("update", &id, Some(&tool), cancel, progress)?;
                    }
                }
                deferred
            }
            other => return Err(self.unsupported(other.capability())),
        };
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

    /// Captured from SDK 10.0.401.
    const SEARCH: &str = "Package ID                          Latest Version      Authors                            Downloads      Verified
------------------------------------------------------------------------------------------------------------------
microsoft.dotnet-interactive        1.0.712001          Microsoft                          35277839       x
teamcity.csi                        1.0.7               NikolayP, JetBrains                65737
jirabridge                          0.1.0               Damian Mączyński                   218
";
    const DOTNETSAY_SEARCH: &str =
        "Package ID          Latest Version      Authors             Downloads      Verified
-----------------------------------------------------------------------------------
dotnetsay           3.0.3               Richard Lander      1384752
dx                  0.5.0               Lambda3             4918
";
    const BANNER: &str = "
Welcome to .NET 10.0!
---------------------
SDK Version: 10.0.401

----------------
Installed an ASP.NET Core HTTPS development certificate.
--------------------------------------------------------------------------------------
";

    #[test]
    fn search_table_columns() {
        let offers = parse_search(SEARCH);
        assert_eq!(offers.len(), 3);
        assert_eq!(
            offers[1],
            Offer {
                id: "teamcity.csi".into(),
                version: "1.0.7".into(),
                authors: "NikolayP, JetBrains".into()
            }
        );
        assert_eq!(offers[2].authors, "Damian Mączyński");
        assert!(parse_search("Could not find any results.\n").is_empty());
    }

    #[test]
    fn old_sdk_table_and_banner() {
        let tools = parse_table(
            "Package Id      Version      Commands\n-------------------------------------\ndotnet-ef       10.0.12      dotnet-ef\npwsh-tool       7.4.0        pwsh, pwsh-preview\n",
        );
        assert_eq!(tools[1].commands, vec!["pwsh", "pwsh-preview"]);
        let text = format!("{BANNER}{{\"version\":1,\"data\":[]}}");
        assert_eq!(json(&text), "{\"version\":1,\"data\":[]}");
    }

    #[test]
    fn nuget_version_order() {
        for (a, b, order) in [
            ("3.0.3", "2.1.7", Ordering::Greater),
            ("1.0", "1.0.0.0", Ordering::Equal),
            ("1.0.20474.1", "1.0.20474", Ordering::Greater),
            ("2.0.0-preview.2", "2.0.0", Ordering::Less),
            ("2.0.0-preview.10", "2.0.0-preview.2", Ordering::Greater),
            ("2.0.0-alpha", "2.0.0-alpha.1", Ordering::Less),
            ("1.2.3+build", "1.2.3", Ordering::Equal),
        ] {
            assert_eq!(compare_versions(a, b), order, "{a} {b}");
        }
    }

    #[test]
    fn newest_stable_ignores_prereleases_and_reports_feed_problems() {
        let reply = r#"{"version":2,"problems":[],"searchResult":[{"sourceName":"nuget.org","packages":[{"id":"dotnetsay","version":"2.1.7"},{"id":"dotnetsay","version":"3.0.3"},{"id":"dotnetsay","version":"4.0.0-preview.1"},{"id":"dotnetsay","version":"3.0.2"}]}]}"#;
        assert_eq!(newest_stable(reply, "dotnetsay"), Ok(Some("3.0.3".into())));
        let empty = r#"{"version":2,"problems":[],"searchResult":[{"sourceName":"nuget.org","packages":[]}]}"#;
        assert_eq!(newest_stable(empty, "gone"), Ok(None));
        let down = r#"{"version":2,"problems":[],"searchResult":[{"sourceName":"https://bad.invalid/v3/index.json","problems":[{"text":"Unable to load the service index for source https://bad.invalid/v3/index.json.","problemType":"Error"}],"packages":[]}]}"#;
        assert!(newest_stable(down, "dotnetsay")
            .unwrap_err()
            .contains("Unable to load the service index"));
    }

    #[test]
    fn ids() {
        assert!(valid_id("dotnetsay"));
        assert!(valid_id("Microsoft.dotnet-interactive"));
        assert!(valid_id("my_tool.v2"));
        for bad in [
            "",
            "--help",
            "-g",
            ".hidden",
            "a b",
            "a/b",
            "a@1",
            &"x".repeat(101),
        ] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[derive(Clone, Default)]
    struct Fake {
        /// Installed tools: id → version.
        tools: Arc<Mutex<Vec<(String, String)>>>,
        calls: Arc<Mutex<Vec<String>>>,
        old_sdk: bool,
        offline: bool,
        /// Only the .NET runtime: `dotnet --version` fails without an SDK.
        runtime_only: bool,
    }
    fn done(stdout: &str, code: i32) -> Completion {
        Completion {
            code: Some(code),
            signal: None,
            stdout: stdout.as_bytes().to_vec(),
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
        }
    }
    fn failed(stdout: &str) -> Result<Completion, ExecutionError> {
        Err(ExecutionError::Failed(done(stdout, 1)))
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
            assert_eq!(executable, "dotnet");
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            self.calls.lock().unwrap().push(args.join(" "));
            let mut tools = self.tools.lock().unwrap();
            let words: Vec<&str> = args.iter().map(String::as_str).collect();
            match words.as_slice() {
                ["--version"] if self.runtime_only => failed(""),
                ["--version"] => Ok(done("10.0.401\n", 0)),
                ["tool", "list", "--global", "--format", "json"] if self.old_sdk => failed(
                    "Unrecognized command or argument '--format'.\nUnrecognized command or argument 'json'.\n",
                ),
                ["tool", "list", "--global", "--format", "json"] => {
                    let data: Vec<String> = tools
                        .iter()
                        .map(|(id, version)| {
                            format!(
                                r#"{{"packageId":"{id}","version":"{version}","commands":["{id}"]}}"#
                            )
                        })
                        .collect();
                    Ok(done(
                        &format!(r#"{BANNER}{{"version":1,"data":[{}]}}"#, data.join(",")),
                        0,
                    ))
                }
                ["tool", "list", "--global"] => {
                    let mut table = "Package Id      Version      Commands\n-------------------------------------\n".to_owned();
                    for (id, version) in tools.iter() {
                        table.push_str(&format!("{id:<16}{version:<13}{id}\n"));
                    }
                    Ok(done(&table, 0))
                }
                ["package", "search", "--exact-match", "--format", "json", "--", id] => {
                    if self.offline {
                        return Ok(done(
                            r#"{"version":2,"problems":[],"searchResult":[{"sourceName":"nuget.org","problems":[{"text":"Unable to load the service index for source https://api.nuget.org/v3/index.json.","problemType":"Error"}],"packages":[]}]}"#,
                            0,
                        ));
                    }
                    let versions: &[&str] = match *id {
                        "dotnetsay" => &["2.1.7", "3.0.2", "3.0.3", "4.0.0-beta.1"],
                        "dotnet-ef" => &["9.0.0", "10.0.12"],
                        _ => &[],
                    };
                    let packages: Vec<String> = versions
                        .iter()
                        .map(|v| format!(r#"{{"id":"{id}","version":"{v}"}}"#))
                        .collect();
                    Ok(done(
                        &format!(
                            r#"{{"version":2,"problems":[],"searchResult":[{{"sourceName":"nuget.org","packages":[{}]}}]}}"#,
                            packages.join(",")
                        ),
                        0,
                    ))
                }
                ["tool", "search", "--", query] => Ok(done(
                    if "dotnetsay".contains(query) {
                        DOTNETSAY_SEARCH
                    } else {
                        "Could not find any results.\n"
                    },
                    0,
                )),
                ["tool", verb, "--global", "--", id] => {
                    assert!(write);
                    match *verb {
                        "install" => tools.push((id.to_string(), "3.0.3".into())),
                        "update" => {
                            for tool in tools.iter_mut().filter(|(name, _)| name == id) {
                                tool.1 = "3.0.3".into();
                            }
                        }
                        "uninstall" => tools.retain(|(name, _)| name != id),
                        other => panic!("unexpected {other}"),
                    }
                    Ok(done(
                        &format!("Skipping NuGet package signature verification.\nTool '{id}' was successfully {verb}ed.\n"),
                        0,
                    ))
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    fn fake(tools: &[(&str, &str)]) -> Fake {
        Fake {
            tools: Arc::new(Mutex::new(
                tools
                    .iter()
                    .map(|(id, v)| (id.to_string(), v.to_string()))
                    .collect(),
            )),
            ..Fake::default()
        }
    }
    fn run(backend: &mut DotnetTools<Fake>, operation: Operation) -> Result<(), EngineError> {
        backend
            .execute(&operation, &Cancellation::default(), &mut |_| {})
            .map(|_| ())
    }

    #[test]
    fn inventory_with_update_checks() {
        let fake = fake(&[("dotnet-ef", "10.0.12"), ("dotnetsay", "2.1.7")]);
        let mut dotnet = DotnetTools::new(fake.clone());
        assert_eq!(
            dotnet.detect(&Cancellation::default()).unwrap(),
            Availability::Available
        );
        let rows = dotnet.installed(&Cancellation::default()).unwrap();
        assert!(dotnet.query_errors().is_empty());
        assert_eq!(rows[0].update, UpdateAvailability::Current);
        assert_eq!(rows[1].id.name, "dotnetsay");
        assert_eq!(rows[1].summary, ".NET global tool · dotnetsay");
        assert_eq!(rows[1].installed_version.as_deref(), Some("2.1.7"));
        // Prereleases are not what `dotnet tool update` moves to.
        assert_eq!(rows[1].candidate_version.as_deref(), Some("3.0.3"));
        assert_eq!(rows[1].update, UpdateAvailability::Available);
        run(&mut dotnet, Operation::Upgrade(rows[1].id.clone())).unwrap();
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"tool update --global -- dotnetsay".into()));
        // Current now: nothing runs.
        fake.calls.lock().unwrap().clear();
        run(&mut dotnet, Operation::Upgrade(rows[1].id.clone())).unwrap();
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("tool update")));
    }

    #[test]
    fn failed_checks_keep_the_rows() {
        let mut fake = fake(&[("dotnet-ef", "9.0.0"), ("dotnetsay", "2.1.7")]);
        fake.offline = true;
        let mut dotnet = DotnetTools::new(fake.clone());
        let rows = dotnet.installed(&Cancellation::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|row| row.update == UpdateAvailability::Unknown));
        // One shared failure is reported once, and checks stop there.
        let errors = dotnet.query_errors();
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].to_string().contains("service index"),
            "{}",
            errors[0]
        );
        let checks = fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.starts_with("package search"))
            .count();
        assert_eq!(checks, 1);
    }

    #[test]
    fn old_sdks_list_through_the_table() {
        let mut fake = fake(&[("dotnetsay", "2.1.7")]);
        fake.old_sdk = true;
        let mut dotnet = DotnetTools::new(fake);
        let rows = dotnet.installed(&Cancellation::default()).unwrap();
        assert_eq!(rows[0].installed_version.as_deref(), Some("2.1.7"));
        assert_eq!(rows[0].summary, ".NET global tool · dotnetsay");
    }

    #[test]
    fn search_install_remove_are_verified() {
        let fake = fake(&[("dotnet-ef", "10.0.12")]);
        let mut dotnet = DotnetTools::new(fake.clone());
        let found = dotnet
            .search("DotNetSay", &Cancellation::default())
            .unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id.name, "dotnetsay");
        assert_eq!(found[0].candidate_version.as_deref(), Some("3.0.3"));
        assert_eq!(found[0].summary, ".NET global tool by Richard Lander");
        assert!(!unverified_search_offer(&found[0]));
        // Exact lookups keep only the exact id.
        let exact = dotnet
            .lookup("dotnetsay", &Cancellation::default())
            .unwrap();
        assert_eq!(exact.len(), 1);
        run(&mut dotnet, Operation::Install(exact[0].id.clone())).unwrap();
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"tool install --global -- dotnetsay".into()));
        // Installed tools are found without asking the feeds, and are no
        // longer offered.
        fake.calls.lock().unwrap().clear();
        let exact = dotnet
            .lookup("dotnetsay", &Cancellation::default())
            .unwrap();
        assert!(exact[0].installed_version.is_some());
        assert_eq!(fake.calls.lock().unwrap().len(), 1);
        let found = dotnet
            .search("dotnetsay", &Cancellation::default())
            .unwrap();
        assert_eq!(
            found
                .iter()
                .filter(|row| row.id.name == "dotnetsay")
                .count(),
            1
        );
        // Installing again is a no-op.
        fake.calls.lock().unwrap().clear();
        run(&mut dotnet, Operation::Install(exact[0].id.clone())).unwrap();
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("tool install")));
        let details = dotnet
            .details(&exact[0].id, &Cancellation::default())
            .unwrap();
        assert!(details.description.contains("Commands: dotnetsay"));
        assert_eq!(details.package.update, UpdateAvailability::Current);
        run(&mut dotnet, Operation::Remove(exact[0].id.clone())).unwrap();
        assert!(!fake
            .tools
            .lock()
            .unwrap()
            .iter()
            .any(|(id, _)| id == "dotnetsay"));
        assert!(matches!(
            run(&mut dotnet, Operation::Remove(exact[0].id.clone())),
            Err(EngineError::NotFound)
        ));
        let mut bad = exact[0].id.clone();
        bad.name = "--help".into();
        assert!(run(&mut dotnet, Operation::Install(bad)).is_err());
        assert!(dotnet
            .lookup("--help", &Cancellation::default())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn upgrade_all_updates_each_tool_behind() {
        let fake = fake(&[("dotnet-ef", "10.0.12"), ("dotnetsay", "2.1.7")]);
        let mut dotnet = DotnetTools::new(fake.clone());
        run(
            &mut dotnet,
            Operation::UpgradeAll {
                backend: "dotnet".into(),
            },
        )
        .unwrap();
        let calls = fake.calls.lock().unwrap().clone();
        assert!(calls.contains(&"tool update --global -- dotnetsay".into()));
        assert!(!calls.contains(&"tool update --global -- dotnet-ef".into()));
    }

    #[test]
    fn an_empty_search_lists_installed_tools_without_the_catalog() {
        let fake = fake(&[("dotnet-ef", "10.0.12")]);
        let mut dotnet = DotnetTools::new(fake.clone());
        assert!(dotnet.may_have("dotnet-ef") && !dotnet.may_have("--global"));
        let rows = dotnet.search("  ", &Cancellation::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("tool search")));
        // Update checks stop at cancellation.
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(
            dotnet.installed(&cancel),
            Err(EngineError::Cancelled)
        ));
    }

    #[test]
    fn a_runtime_without_the_sdk_is_unavailable() {
        let mut fake = fake(&[]);
        fake.runtime_only = true;
        assert_eq!(
            DotnetTools::new(fake)
                .detect(&Cancellation::default())
                .unwrap(),
            Availability::Unavailable(
                ".NET SDK not found; global tools need the SDK, not only the runtime".into()
            )
        );
    }

    #[test]
    fn prerelease_labels_compare_part_by_part() {
        for (a, b, order) in [
            ("2.0.0", "2.0.0-rc.1", Ordering::Greater),
            ("2.0.0-rc.1", "2.0.0-rc", Ordering::Greater),
            ("2.0.0-rc.1", "2.0.0-rc.1", Ordering::Equal),
            ("2.0.0-1", "2.0.0-alpha", Ordering::Less),
            ("2.0.0-alpha", "2.0.0-1", Ordering::Greater),
            ("2.0.0-Beta", "2.0.0-alpha", Ordering::Greater),
        ] {
            assert_eq!(compare_versions(a, b), order, "{a} {b}");
        }
    }

    #[test]
    fn offline_details_and_updates_stay_honest() {
        let mut fake = fake(&[("dotnetsay", "4.0.0")]);
        fake.offline = true;
        let mut dotnet = DotnetTools::new(fake.clone());
        let rows = dotnet.installed(&Cancellation::default()).unwrap();
        let details = dotnet
            .details(&rows[0].id, &Cancellation::default())
            .unwrap();
        assert!(
            details.description.contains("Update check failed"),
            "{}",
            details.description
        );
        assert_eq!(details.package.update, UpdateAvailability::Unknown);
        let mut foreign = rows[0].id.clone();
        foreign.backend = "nuget".into();
        assert!(matches!(
            dotnet.details(&foreign, &Cancellation::default()),
            Err(EngineError::NotFound)
        ));
        // Without a check the update runs, and an older result is an error.
        let error = run(&mut dotnet, Operation::Upgrade(rows[0].id.clone())).unwrap_err();
        assert!(
            error.to_string().contains("not in the expected state"),
            "{error}"
        );
        // Cancelled before anything is written.
        let cancel = Cancellation::default();
        cancel.cancel();
        fake.calls.lock().unwrap().clear();
        assert!(matches!(
            dotnet.execute(&Operation::Remove(rows[0].id.clone()), &cancel, &mut |_| {}),
            Err(EngineError::Cancelled)
        ));
        assert!(matches!(
            dotnet.execute(
                &Operation::UpgradeAll { backend: ID.into() },
                &cancel,
                &mut |_| {}
            ),
            Err(EngineError::Cancelled)
        ));
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("tool uninstall") || call.starts_with("tool update")));
        assert!(matches!(
            run(&mut dotnet, Operation::Refresh { backend: ID.into() }),
            Err(EngineError::Unsupported { .. })
        ));
    }
}
