//! Repository identities and native operations; no shell commands or trust bypasses.
use crate::{
    backends::Transport,
    engine::EngineError,
    package::Scope,
    process::{Cancellation, Completion, ExecutionError},
};
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Repository {
    pub backend: String,
    pub name: String,
    pub title: String,
    pub url: String,
    pub scope: Scope,
    pub enabled: bool,
    pub priority: Option<i32>,
}
#[derive(Default, Serialize)]
pub struct Report {
    pub repositories: Vec<Repository>,
    pub errors: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Change {
    Add { url: String },
    Remove,
    SetEnabled { enabled: bool },
    SetPriority { priority: i32 },
    OpenEditor,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    pub backend: String,
    pub name: String,
    pub scope: Scope,
    #[serde(flatten)]
    pub change: Change,
}
fn invalid(message: impl ToString) -> EngineError {
    EngineError::InvalidResponse {
        backend: "repositories".into(),
        reason: message.to_string(),
    }
}
fn output(result: Result<Completion, ExecutionError>) -> Result<Vec<u8>, EngineError> {
    let result = result?;
    if result.code != Some(0) {
        return Err(ExecutionError::Failed(result).into());
    }
    if result.truncated {
        return Err(invalid("repository metadata exceeded output limit"));
    }
    Ok(result.stdout)
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
fn system(scope: &Scope) -> Result<bool, EngineError> {
    match scope {
        Scope::System => Ok(true),
        Scope::User { uid } if *uid == rustix::process::getuid().as_raw() => Ok(false),
        _ => Err(invalid("foreign installation scope")),
    }
}
pub fn parse_action(text: &str) -> Result<Action, EngineError> {
    let mut value: serde_json::Value = serde_json::from_str(text).map_err(invalid)?;
    if value["scope"] == "user" {
        value["scope"] = serde_json::to_value(Scope::User {
            uid: rustix::process::getuid().as_raw(),
        })
        .map_err(invalid)?;
    }
    serde_json::from_value(value).map_err(invalid)
}
impl Action {
    pub fn label(&self) -> String {
        let scope = match self.scope {
            Scope::System => "System",
            _ => "User",
        };
        let change = match &self.change {
            Change::Add { url } => format!("Add repository from {url}"),
            Change::Remove => "Remove repository".into(),
            Change::SetEnabled { enabled } => if *enabled {
                "Enable repository"
            } else {
                "Disable repository"
            }
            .into(),
            Change::SetPriority { priority } => format!("Set repository priority to {priority}"),
            Change::OpenEditor => "Open Software Sources".into(),
        };
        format!("{change}\n{} · {} · {scope}", self.backend, self.name)
    }
    pub fn validate(&self) -> Result<(), EngineError> {
        system(&self.scope)?;
        if !valid_name(&self.name) {
            return Err(invalid("invalid repository name"));
        }
        match (&*self.backend, &self.change) {
            ("flatpak", Change::Add { url })
                if url.starts_with("https://")
                    && url.ends_with(".flatpakrepo")
                    && !url.chars().any(char::is_whitespace) =>
            {
                Ok(())
            }
            ("flatpak", Change::Remove | Change::SetEnabled { .. }) => Ok(()),
            ("flatpak", Change::SetPriority { priority }) if (0..=9999).contains(priority) => {
                Ok(())
            }
            ("fwupd", Change::SetEnabled { .. }) if self.scope == Scope::System => Ok(()),
            ("apt", Change::OpenEditor) if self.scope == Scope::System => Ok(()),
            _ => Err(invalid("unsupported repository change or invalid value")),
        }
    }
}
pub fn apply(
    transport: &impl Transport,
    action: &Action,
    cancel: &Cancellation,
) -> Result<(), EngineError> {
    action.validate()?;
    if cancel.requested() {
        return Err(EngineError::Cancelled);
    }
    let system = system(&action.scope)?;
    let mut args: Vec<OsString> = vec![];
    match (&*action.backend, &action.change) {
        ("apt", Change::OpenEditor) => {
            transport.repository_editor()?;
            return Ok(());
        }
        ("fwupd", Change::SetEnabled { enabled }) => {
            args.extend(
                [
                    "--assume-yes",
                    "modify-remote",
                    &action.name,
                    "Enabled",
                    if *enabled { "true" } else { "false" },
                ]
                .map(Into::into),
            );
            output(transport.system_manager("fwupdmgr", &args, cancel, true))?;
        }
        ("flatpak", change) => {
            args.extend(
                [
                    if system { "--system" } else { "--user" },
                    "--noninteractive",
                ]
                .map(Into::into),
            );
            match change {
                Change::Add { url } => {
                    args.extend(["remote-add", "--from", &action.name, url].map(Into::into))
                }
                Change::Remove => args.extend(["remote-delete", &action.name].map(Into::into)),
                Change::SetEnabled { enabled } => args.extend(
                    [
                        "remote-modify",
                        if *enabled { "--enable" } else { "--disable" },
                        &action.name,
                    ]
                    .map(Into::into),
                ),
                Change::SetPriority { priority } => args.extend([
                    "remote-modify".into(),
                    format!("--prio={priority}").into(),
                    action.name.clone().into(),
                ]),
                _ => return Err(invalid("unsupported Flatpak operation")),
            }
            output(transport.flatpak(&args, cancel, true, system))?;
        }
        _ => return Err(invalid("unsupported repository operation")),
    }
    Ok(())
}
fn flatpak(
    transport: &impl Transport,
    scope: Scope,
    cancel: &Cancellation,
) -> Result<Vec<Repository>, EngineError> {
    let prefix = if system(&scope)? {
        "--system"
    } else {
        "--user"
    };
    let bytes = output(
        transport.flatpak(
            &[
                prefix,
                "remotes",
                "--show-disabled",
                "--columns=name,title,url,priority,options",
            ]
            .map(Into::into),
            cancel,
            false,
            system(&scope)?,
        ),
    )?;
    let text = String::from_utf8(bytes).map_err(invalid)?;
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            if !(4..=5).contains(&fields.len()) || !valid_name(fields[0]) {
                return Err(invalid("invalid Flatpak repository metadata"));
            }
            Ok(Repository {
                backend: "flatpak".into(),
                name: fields[0].into(),
                title: fields[1].into(),
                url: fields[2].into(),
                scope: scope.clone(),
                priority: Some(fields[3].parse().map_err(invalid)?),
                enabled: !fields
                    .get(4)
                    .unwrap_or(&"")
                    .split(',')
                    .any(|s| s.trim() == "disabled"),
            })
        })
        .collect()
}
fn firmware(
    transport: &impl Transport,
    cancel: &Cancellation,
) -> Result<Vec<Repository>, EngineError> {
    let bytes = output(transport.system_manager(
        "fwupdmgr",
        &["--json".into(), "get-remotes".into()],
        cancel,
        false,
    ))?;
    let value = serde_json::Deserializer::from_slice(&bytes)
        .into_iter::<serde_json::Value>()
        .next()
        .ok_or_else(|| invalid("empty firmware response"))?
        .map_err(invalid)?;
    value["Remotes"]
        .as_array()
        .ok_or_else(|| invalid("missing firmware remotes"))?
        .iter()
        .map(|remote| {
            let name = remote["Id"]
                .as_str()
                .filter(|name| valid_name(name))
                .ok_or_else(|| invalid("invalid firmware remote ID"))?;
            Ok(Repository {
                backend: "fwupd".into(),
                name: name.into(),
                title: remote["Title"].as_str().unwrap_or(name).into(),
                url: remote["MetadataUri"].as_str().unwrap_or("").into(),
                scope: Scope::System,
                enabled: remote["Enabled"]
                    .as_bool()
                    .ok_or_else(|| invalid("missing firmware remote enabled state"))?,
                priority: None,
            })
        })
        .collect()
}
pub fn list(transport: &impl Transport, root: &Path, cancel: &Cancellation) -> Report {
    list_selected(transport, root, cancel, &[], None)
}
pub fn list_selected(
    transport: &impl Transport,
    root: &Path,
    cancel: &Cancellation,
    backends: &[String],
    scope: Option<&Scope>,
) -> Report {
    let mut report = Report::default();
    let allowed = |backend: &str, candidate: &Scope| {
        (backends.is_empty() || backends.iter().any(|b| b == backend))
            && scope.is_none_or(|s| s == candidate)
    };
    for (backend, label, installation) in [
        (
            "flatpak",
            "Flatpak · User",
            Scope::User {
                uid: rustix::process::getuid().as_raw(),
            },
        ),
        ("flatpak", "Flatpak · System", Scope::System),
        ("fwupd", "Firmware", Scope::System),
    ] {
        if !allowed(backend, &installation) {
            continue;
        }
        let result = if backend == "flatpak" {
            flatpak(transport, installation, cancel)
        } else {
            firmware(transport, cancel)
        };
        match result {
            Ok(rows) => report.repositories.extend(rows),
            Err(EngineError::Execution(ExecutionError::Disabled(_))) => (),
            Err(error) => report.errors.push(format!("{label}: {error}")),
        }
    }
    for (backend, directory) in [("dnf", "etc/yum.repos.d"), ("zypper", "etc/zypp/repos.d")] {
        if !allowed(backend, &Scope::System) {
            continue;
        }
        let directory = root.join(directory);
        let directory = if root == Path::new("/") {
            crate::host::Host::current().filesystem_path(&directory)
        } else {
            directory
        };
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                report.errors.push(format!("{backend}: {error}"));
                continue;
            }
        };
        for file in entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "repo"))
        {
            let text = match std::fs::read_to_string(&file) {
                Ok(text) => text,
                Err(error) => {
                    report.errors.push(format!("{}: {error}", file.display()));
                    continue;
                }
            };
            let mut current: Option<Repository> = None;
            for line in text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with(['#', ';']))
            {
                if let Some(name) = line
                    .strip_prefix('[')
                    .and_then(|line| line.strip_suffix(']'))
                {
                    if let Some(row) = current.take() {
                        report.repositories.push(row);
                    }
                    if name.is_empty()
                        || name.starts_with('-')
                        || !name
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-:".contains(&b))
                    {
                        continue;
                    }
                    current = Some(Repository {
                        backend: backend.into(),
                        name: name.into(),
                        title: name.into(),
                        url: String::new(),
                        scope: Scope::System,
                        enabled: true,
                        priority: None,
                    });
                } else if let (Some(row), Some((key, value))) =
                    (current.as_mut(), line.split_once('='))
                {
                    match key.trim() {
                        "name" => row.title = value.trim().into(),
                        "baseurl" | "mirrorlist" | "metalink" if row.url.is_empty() => {
                            row.url = value.trim().into()
                        }
                        "enabled" => row.enabled = value.trim() != "0",
                        _ => {}
                    }
                }
            }
            if let Some(row) = current {
                report.repositories.push(row);
            }
        }
    }
    if !allowed("apt", &Scope::System) {
        return report;
    }
    let apt = root.join("etc/apt");
    let apt = if root == Path::new("/") {
        crate::host::Host::current().filesystem_path(&apt)
    } else {
        apt
    };
    let mut files = vec![apt.join("sources.list")];
    match std::fs::read_dir(apt.join("sources.list.d")) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(entry) => files.push(entry.path()),
                    Err(error) => report.errors.push(format!("APT: {error}")),
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => report.errors.push(format!("APT: {error}")),
    }
    files.sort();
    for file in files {
        if !file
            .extension()
            .is_some_and(|ext| ext == "sources" || ext == "list")
        {
            continue;
        }
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                report
                    .errors
                    .push(format!("APT · {}: {error}", file.display()));
                continue;
            }
        };
        if file.extension().is_some_and(|e| e == "sources") {
            for (index, stanza) in text.split("\n\n").enumerate() {
                let field = |name: &str| {
                    stanza
                        .lines()
                        .find_map(|line| line.strip_prefix(name))
                        .unwrap_or("")
                        .trim()
                };
                if field("URIs:").is_empty() {
                    continue;
                }
                report.repositories.push(Repository {
                    backend: "apt".into(),
                    name: format!("{}:{index}", file.display()),
                    title: format!("{} · {}", field("URIs:"), field("Suites:")),
                    url: field("URIs:").into(),
                    scope: Scope::System,
                    enabled: field("Enabled:") != "no",
                    priority: None,
                });
            }
        } else if file.extension().is_some_and(|e| e == "list") {
            for (index, line) in text.lines().enumerate() {
                let line = line.trim();
                let enabled = !line.starts_with('#');
                let value = line.trim_start_matches('#').trim();
                if !value.starts_with("deb ") && !value.starts_with("deb-src ") {
                    continue;
                }
                report.repositories.push(Repository {
                    backend: "apt".into(),
                    name: format!("{}:{index}", file.display()),
                    title: value.into(),
                    url: value.into(),
                    scope: Scope::System,
                    enabled,
                    priority: None,
                });
            }
        }
    }
    report
}

#[cfg(test)]
mod repository_file_tests {
    use super::*;
    use crate::{
        backends::NativeTransport,
        host::{Authorization, Host},
    };
    #[test]
    fn lists_synthetic_dnf_sections() {
        let root = std::env::temp_dir().join(format!("pkgdeck-repos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let directory = root.join("etc/yum.repos.d");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("synthetic.repo"),
            "[synthetic]\nname=Synthetic repo\nbaseurl=https://example.invalid/repo\nenabled=0\n",
        )
        .unwrap();
        let transport = NativeTransport {
            host: Host::current(),
            authorization: Authorization::Polkit,
        };
        let report = list_selected(
            &transport,
            &root,
            &Cancellation::default(),
            &["dnf".into()],
            None,
        );
        assert!(report.errors.is_empty());
        assert_eq!(report.repositories.len(), 1);
        assert_eq!(report.repositories[0].title, "Synthetic repo");
        assert!(!report.repositories[0].enabled);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn zypper_repository_listing_skips_invalid_sections_and_honors_scope() {
        let root = std::env::temp_dir().join(format!("pkgdeck-zypp-repos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let directory = root.join("etc/zypp/repos.d");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("synthetic.repo"), "# synthetic\n[vendor:one]\nname=First repo\nbaseurl=https://example.invalid/one\nenabled=1\n[bad name]\nbaseurl=https://example.invalid/bad\n[two]\nmetalink=https://example.invalid/meta\nenabled=0\n").unwrap();
        std::fs::write(
            directory.join("ignored.txt"),
            "[ignored]\nbaseurl=https://example.invalid/ignored\n",
        )
        .unwrap();
        let transport = NativeTransport {
            host: Host::current(),
            authorization: Authorization::Polkit,
        };
        let report = list_selected(
            &transport,
            &root,
            &Cancellation::default(),
            &["zypper".into()],
            Some(&Scope::System),
        );
        assert!(report.errors.is_empty());
        assert_eq!(report.repositories.len(), 2);
        assert_eq!(report.repositories[0].name, "vendor:one");
        assert_eq!(report.repositories[0].title, "First repo");
        assert_eq!(report.repositories[1].url, "https://example.invalid/meta");
        assert!(!report.repositories[1].enabled);
        let user = Scope::User {
            uid: rustix::process::getuid().as_raw(),
        };
        assert!(list_selected(
            &transport,
            &root,
            &Cancellation::default(),
            &["zypper".into()],
            Some(&user)
        )
        .repositories
        .is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn repository_inventory_reports_unreadable_definitions_without_hiding_valid_ones() {
        let root =
            std::env::temp_dir().join(format!("pkgdeck-broken-repos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dnf = root.join("etc/yum.repos.d");
        std::fs::create_dir_all(&dnf).unwrap();
        std::fs::write(dnf.join("broken.repo"), [0xff, 0xfe]).unwrap();
        std::fs::write(
            dnf.join("valid.repo"),
            "[valid]\nname=Valid\nbaseurl=https://example.invalid/repo\nunknown=ignored\n",
        )
        .unwrap();
        let transport = NativeTransport {
            host: Host::current(),
            authorization: Authorization::Polkit,
        };
        let cancel = Cancellation::default();
        let report = list_selected(&transport, &root, &cancel, &["dnf".into()], None);
        assert_eq!(report.repositories.len(), 1);
        assert_eq!(report.repositories[0].name, "valid");
        assert_eq!(report.errors.len(), 1);
        std::fs::remove_dir_all(&dnf).unwrap();
        std::fs::write(&dnf, b"not a directory").unwrap();
        let report = list_selected(&transport, &root, &cancel, &["dnf".into()], None);
        assert!(report.repositories.is_empty());
        assert_eq!(report.errors.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
