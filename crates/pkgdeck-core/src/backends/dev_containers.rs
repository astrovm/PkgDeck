//! Development containers made with Toolbx (`toolbox`) or Distrobox: one row
//! per container, the way image-based systems (Fedora Silverblue, Bazzite,
//! Aurora…) install developer tools.
//!
//! Updating a container upgrades every package inside it with the
//! container's own package manager. Whether anything is pending is never
//! guessed, so rows report updates as unknown. Containers are never created
//! here. Removing one runs the tool's own `rm --force`, which deletes the
//! container and everything installed in it; the home folder it shares with
//! the host is kept.
//!
//! Only the invoking user's rootless containers are listed. A container
//! Distrobox made is a Distrobox row even when its image carries the Toolbx
//! label (as `fedora-toolbox` images do), so it never appears twice.
use super::{bytes, invalid, NativeTransport, Transport};
use crate::{engine::*, package::*, process::*};
use serde::Deserialize;
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Upgrade,
    Capability::Remove,
];
/// Labels Toolbx puts on its containers (current and pre-0.0.90).
const TOOLBOX_LABELS: [&str; 2] = [
    "com.github.containers.toolbox",
    "com.github.debarshiray.toolbox",
];
/// Package managers looked for inside a Toolbx container, in this order.
const MANAGERS: [&str; 6] = ["dnf", "apt-get", "pacman", "zypper", "apk", "xbps-install"];
/// Finds the container's package manager. Fixed text: nothing is interpolated.
const FIND_MANAGER: &str = "for m in dnf apt-get pacman zypper apk xbps-install; do \
    if command -v \"$m\" >/dev/null 2>&1; then echo \"$m\"; exit 0; fi; done; exit 3";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Toolbox,
    Distrobox,
}

impl Kind {
    fn id(self) -> &'static str {
        match self {
            Self::Toolbox => "toolbox",
            Self::Distrobox => "distrobox",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Toolbox => "Toolbx",
            Self::Distrobox => "Distrobox",
        }
    }
    fn homepage(self) -> &'static str {
        match self {
            Self::Toolbox => "https://containertoolbx.org/",
            Self::Distrobox => "https://distrobox.it/",
        }
    }
}

pub struct DevContainers<T = NativeTransport> {
    transport: T,
    kind: Kind,
}

/// One container as its tool reports it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Found {
    /// Short (12 character) container id.
    id: String,
    name: String,
    image: String,
    running: bool,
    /// The container engine's own status text, like "Up 2 minutes".
    status: String,
    /// Creation time, seconds since the Unix epoch, when known.
    created: Option<i64>,
    /// The image's declared version label, when it has one.
    image_version: Option<String>,
}

/// One entry of `podman ps --all --format json`.
#[derive(Deserialize, Debug, Default)]
#[serde(default, rename_all = "PascalCase")]
struct PodmanContainer {
    id: String,
    names: Vec<String>,
    image: String,
    state: String,
    status: String,
    created: Option<serde_json::Value>,
    labels: Option<BTreeMap<String, String>>,
    mounts: Option<Vec<String>>,
}

impl PodmanContainer {
    fn label(&self, key: &str) -> Option<&str> {
        self.labels.as_ref()?.get(key).map(String::as_str)
    }
    /// Distrobox's own test: its label, or one of the mounts it adds.
    fn made_by_distrobox(&self) -> bool {
        self.label("manager") == Some("distrobox")
            || self
                .mounts
                .iter()
                .flatten()
                .any(|mount| mount.contains("distrobox"))
    }
    fn toolbox(&self) -> bool {
        TOOLBOX_LABELS
            .iter()
            .any(|key| self.label(key) == Some("true"))
            && !self.made_by_distrobox()
    }
    fn found(&self) -> Option<Found> {
        let name = self.names.first()?;
        if !valid_name(name) || !valid_id(&self.id) {
            return None;
        }
        Some(Found {
            id: self.id.chars().take(12).collect(),
            name: name.clone(),
            image: self.image.clone(),
            running: self.state == "running",
            status: if self.status.is_empty() {
                self.state.clone()
            } else {
                self.status.clone()
            },
            created: self.created.as_ref().and_then(serde_json::Value::as_i64),
            image_version: self
                .label("org.opencontainers.image.version")
                .or_else(|| self.label("version"))
                .filter(|v| !v.is_empty() && v.len() <= 64 && !v.contains(char::is_whitespace))
                .map(Into::into),
        })
    }
}

/// Container names Podman and Docker accept: `[a-zA-Z0-9][a-zA-Z0-9_.-]*`.
/// The first character also keeps a name from ever reading as an option.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

fn valid_id(id: &str) -> bool {
    id.len() >= 12 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Removes terminal color sequences (`ESC [ … letter`).
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `distrobox list --no-color`:
/// `ID           | NAME                 | STATUS             | IMAGE`
fn parse_distrobox_list(text: &str) -> Vec<Found> {
    plain(text)
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('|').map(str::trim).collect();
            let [id, name, status, image] = fields.as_slice() else {
                return None;
            };
            if !valid_id(id) || !valid_name(name) || image.is_empty() {
                return None;
            }
            Some(Found {
                id: (*id).into(),
                name: (*name).into(),
                image: (*image).into(),
                // Distrobox's own rule for the "running" color.
                running: status.starts_with("Up") || status.contains("running"),
                status: (*status).into(),
                created: None,
                image_version: None,
            })
        })
        .collect()
}

fn parse_podman(output: &[u8]) -> Result<Vec<PodmanContainer>, serde_json::Error> {
    // Podman 3 printed `null` for no containers.
    Ok(serde_json::from_slice::<Option<Vec<PodmanContainer>>>(output)?.unwrap_or_default())
}

/// `registry.fedoraproject.org/fedora-toolbox:43` → `43`;
/// `…/ubuntu@sha256:abcdef…` → `sha256:abcdef012345`; no tag → `latest`.
fn image_tag(image: &str) -> String {
    if let Some((_, digest)) = image.split_once('@') {
        return digest.chars().take(19).collect();
    }
    let last = image.rsplit('/').next().unwrap_or(image);
    match last.split_once(':') {
        Some((_, tag)) if !tag.is_empty() => tag.into(),
        _ => "latest".into(),
    }
}

/// `2026-09-28` in UTC, from seconds since the Unix epoch.
fn utc_date(seconds: i64) -> String {
    // Howard Hinnant's days-to-civil algorithm.
    let days = seconds.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// What Distrobox has put on the host for one container: the commands
/// `distrobox-export --bin` wrote to `~/.local/bin`, and the applications
/// `distrobox-export --app` added to `~/.local/share/applications`.
#[derive(Debug, Default, PartialEq)]
struct Exports {
    binaries: Vec<String>,
    applications: Vec<String>,
}

/// An exported command is a script with `# distrobox_binary` and
/// `# name: <container>` lines.
fn exported_binary(text: &str, container: &str) -> bool {
    let mut lines = text.lines().map(str::trim);
    lines.clone().any(|line| line == "# distrobox_binary")
        && lines.any(|line| line.strip_prefix("# name:").map(str::trim) == Some(container))
}

/// An exported application's `Exec=` runs `distrobox-enter -n <container>`;
/// its name has " (on <container>)" appended, which is dropped here.
fn exported_application(text: &str, container: &str) -> Option<String> {
    let mut name = None;
    let mut ours = false;
    let mut in_entry = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some(value) = line.strip_prefix("Name=") {
            name.get_or_insert_with(|| value.trim().to_owned());
        } else if let Some(exec) = line.strip_prefix("Exec=") {
            let words: Vec<&str> = exec.split_whitespace().collect();
            ours = words
                .first()
                .is_some_and(|program| program.ends_with("distrobox-enter"))
                && words
                    .windows(2)
                    .any(|pair| pair[0] == "-n" && pair[1] == container);
        }
    }
    let name = name?;
    ours.then(|| {
        name.strip_suffix(&format!(" (on {container})"))
            .unwrap_or(&name)
            .to_owned()
    })
}

/// Small text files only; exports are a few hundred bytes.
fn small_text(path: &std::path::Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 64 * 1024 {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn sorted_entries(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    entries
}

impl<T> DevContainers<T> {
    pub fn toolbox(transport: T) -> Self {
        Self {
            transport,
            kind: Kind::Toolbox,
        }
    }
    pub fn distrobox(transport: T) -> Self {
        Self {
            transport,
            kind: Kind::Distrobox,
        }
    }
}

impl<T: Transport> DevContainers<T> {
    fn id_str(&self) -> &'static str {
        self.kind.id()
    }

    fn tool(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        self.transport.dev_tool(self.id_str(), &args, cancel, write)
    }

    fn podman(&self, cancel: &Cancellation) -> Result<Vec<PodmanContainer>, EngineError> {
        let args: Vec<OsString> = ["ps", "--all", "--format", "json"]
            .iter()
            .map(OsString::from)
            .collect();
        let output = bytes(
            self.id_str(),
            self.transport.container("podman", &args, cancel, false)?,
        )?;
        parse_podman(&output).map_err(|error| invalid(self.id_str(), error))
    }

    fn containers(&self, cancel: &Cancellation) -> Result<Vec<Found>, EngineError> {
        let mut found = match self.kind {
            Kind::Toolbox => self
                .podman(cancel)?
                .iter()
                .filter(|c| c.toolbox())
                .filter_map(PodmanContainer::found)
                .collect(),
            Kind::Distrobox => {
                let output = bytes(
                    self.id_str(),
                    self.tool(&["list", "--no-color"], cancel, false)?,
                )?;
                let text = String::from_utf8(output).map_err(|e| invalid(self.id_str(), e))?;
                parse_distrobox_list(&text)
            }
        };
        found.sort_by(|a, b| a.name.cmp(&b.name));
        found.dedup_by(|a, b| a.name == b.name);
        Ok(found)
    }

    fn home(&self) -> Option<PathBuf> {
        self.transport
            .env("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    }

    fn data_home(&self) -> Option<PathBuf> {
        self.transport
            .env("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| self.home().map(|home| home.join(".local/share")))
    }

    /// The icon of the launcher `distrobox create` adds for the container.
    fn launcher_icon(&self, name: &str) -> Option<PathBuf> {
        let launcher = self
            .data_home()?
            .join("applications")
            .join(format!("{name}.desktop"));
        let text = small_text(&launcher)?;
        let enters = text.lines().any(|line| {
            line.strip_prefix("Exec=").is_some_and(|exec| {
                let words: Vec<&str> = exec.split_whitespace().collect();
                words.first().is_some_and(|p| p.ends_with("distrobox"))
                    && words.get(1) == Some(&"enter")
                    && words.last() == Some(&name)
            })
        });
        let icon = text
            .lines()
            .find_map(|line| line.strip_prefix("Icon="))
            .map(|icon| PathBuf::from(icon.trim()))?;
        (enters && icon.is_absolute() && icon.is_file()).then_some(icon)
    }

    fn exports(&self, name: &str) -> Exports {
        let Some(home) = self.home() else {
            return Exports::default();
        };
        let binaries = sorted_entries(&home.join(".local/bin"))
            .into_iter()
            .filter(|path| small_text(path).is_some_and(|text| exported_binary(&text, name)))
            .filter_map(|path| Some(path.file_name()?.to_str()?.to_owned()))
            .collect();
        let prefix = format!("{name}-");
        let applications = sorted_entries(&home.join(".local/share/applications"))
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|f| f.to_str())
                    .is_some_and(|f| f.starts_with(&prefix) && f.ends_with(".desktop"))
            })
            .filter_map(|path| exported_application(&small_text(&path)?, name))
            .collect();
        Exports {
            binaries,
            applications,
        }
    }

    fn package(&self, found: &Found) -> Package {
        let short_image = found.image.rsplit('/').next().unwrap_or(&found.image);
        let state = if found.running { "running" } else { "stopped" };
        Package {
            id: PackageId {
                backend: self.id_str().into(),
                name: found.name.clone(),
                architecture: "unknown".into(),
                scope: Scope::User {
                    uid: rustix::process::getuid().as_raw(),
                },
                remote: None,
                reference: None,
            },
            display_name: found.name.clone(),
            summary: format!("{short_image} · {state}"),
            installed_version: Some(
                found
                    .image_version
                    .clone()
                    .unwrap_or_else(|| image_tag(&found.image)),
            ),
            candidate_version: None,
            // Knowing would mean starting the container and refreshing its
            // repositories, so it is never guessed.
            update: UpdateAvailability::Unknown,
            icon: match self.kind {
                Kind::Distrobox => self.launcher_icon(&found.name),
                Kind::Toolbox => None,
            },
            component_ids: vec![],
            homepages: vec![self.kind.homepage().into()],
            adopt_with: None,
        }
    }

    fn rows(&self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .containers(cancel)?
            .iter()
            .map(|found| self.package(found))
            .collect())
    }

    fn find(&self, id: &PackageId, cancel: &Cancellation) -> Result<Found, EngineError> {
        if id.backend != self.id_str() || !valid_name(&id.name) {
            return Err(invalid(self.id_str(), "foreign or invalid container"));
        }
        self.containers(cancel)?
            .into_iter()
            .find(|found| found.name == id.name)
            .ok_or(EngineError::NotFound)
    }

    /// Distrobox lists no creation time; Podman does, when it runs the
    /// container. Its entry is used only when the id matches exactly.
    fn podman_details(&self, found: &Found, cancel: &Cancellation) -> Option<Found> {
        self.podman(cancel)
            .ok()?
            .iter()
            .find(|c| c.id.starts_with(&found.id) && c.names.first() == Some(&found.name))
            .and_then(PodmanContainer::found)
    }

    /// Runs `toolbox run --container <name> <args…>`.
    fn toolbox_run(
        &self,
        name: &str,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        let mut all = vec!["run", "--container", name];
        all.extend_from_slice(args);
        self.tool(&all, cancel, write)
    }

    /// The upgrade commands for a Toolbx container's package manager, each
    /// run through `sudo -S` (Toolbx users have an empty password, so sudo
    /// asks nothing; with a real password it fails instead of waiting).
    /// `manager` is one of `MANAGERS`.
    fn toolbox_steps(manager: &str) -> Vec<Vec<&'static str>> {
        const APT: [&str; 2] = ["env", "DEBIAN_FRONTEND=noninteractive"];
        let steps: Vec<Vec<&str>> = match manager {
            "dnf" => vec![vec!["dnf", "-y", "upgrade", "--refresh"]],
            "apt-get" => vec![
                [&APT[..], &["apt-get", "update"]].concat(),
                [
                    &APT[..],
                    &[
                        "apt-get",
                        "-y",
                        "-o",
                        "Dpkg::Options::=--force-confdef",
                        "-o",
                        "Dpkg::Options::=--force-confold",
                        "upgrade",
                    ],
                ]
                .concat(),
            ],
            "pacman" => vec![vec!["pacman", "-Syu", "--noconfirm"]],
            "zypper" => vec![vec!["zypper", "--non-interactive", "update"]],
            "apk" => vec![vec!["apk", "upgrade", "--update-cache"]],
            _ => vec![vec!["xbps-install", "-Suy"]],
        };
        steps
            .into_iter()
            .map(|step| [&["sudo", "-S"][..], &step].concat())
            .collect()
    }

    fn sudo_refused(error: &ExecutionError) -> bool {
        let ExecutionError::Failed(result) = error else {
            return false;
        };
        let stderr = String::from_utf8_lossy(&result.stderr);
        stderr.contains("sudo:")
            && (stderr.contains("password is required")
                || stderr.contains("no password was provided")
                || stderr.contains("incorrect password"))
    }

    fn upgrade(
        &self,
        found: &Found,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<bool, EngineError> {
        let name = found.name.as_str();
        let mut deferred = false;
        let mut last = String::new();
        match self.kind {
            Kind::Distrobox => {
                progress(Progress::Message(format!(
                    "Upgrading the packages in {name} with distrobox upgrade"
                )));
                let completion = self.tool(&["upgrade", name], cancel, true)?;
                deferred |= completion.cancellation_deferred;
                last = String::from_utf8_lossy(&bytes(self.id_str(), completion)?).into();
            }
            Kind::Toolbox => {
                progress(Progress::Message(format!(
                    "Looking for the package manager in {name}"
                )));
                // This starts the container if it was stopped, as entering
                // it would.
                let unknown = || {
                    invalid(
                        self.id_str(),
                        format!("{name} has no package manager PkgDeck knows how to run"),
                    )
                };
                let output =
                    match self.toolbox_run(name, &["sh", "-c", FIND_MANAGER], cancel, false) {
                        Err(ExecutionError::Failed(result)) if result.code == Some(3) => {
                            return Err(unknown())
                        }
                        result => bytes(self.id_str(), result?)?,
                    };
                let manager = String::from_utf8_lossy(&output)
                    .lines()
                    .map(str::trim)
                    .rfind(|line| MANAGERS.contains(line))
                    .map(str::to_owned)
                    .ok_or_else(unknown)?;
                let steps = Self::toolbox_steps(&manager);
                if cancel.requested() {
                    return Err(EngineError::Cancelled);
                }
                for step in steps {
                    progress(Progress::Message(format!(
                        "Running {} in {name}",
                        step[2..].join(" ")
                    )));
                    let completion = match self.toolbox_run(name, &step, cancel, true) {
                        Err(error) if Self::sudo_refused(&error) => {
                            return Err(invalid(
                                self.id_str(),
                                format!(
                                    "sudo in {name} asks for a password; update it from a terminal with `toolbox enter {name}`"
                                ),
                            ))
                        }
                        result => result?,
                    };
                    deferred |= completion.cancellation_deferred;
                    last = String::from_utf8_lossy(&bytes(self.id_str(), completion)?).into();
                    // A later step never starts after cancellation.
                    if cancel.requested() {
                        break;
                    }
                }
            }
        }
        if let Some(line) = last.lines().map(str::trim).rfind(|l| !l.is_empty()) {
            progress(Progress::Message(plain(line)));
        }
        Ok(deferred)
    }

    /// `toolbox rm --force <name>` or `distrobox rm --force <name>`: removes
    /// the container even while it runs, without asking. Distrobox also
    /// deletes its exports and launcher; neither touches the home folder.
    /// No `--` goes before the name: `distrobox rm` drops every name after
    /// it. Names never start with a dash (see `valid_name`).
    fn remove(
        &self,
        found: &Found,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let name = found.name.as_str();
        progress(Progress::Message(format!(
            "Removing the container {name} and everything installed in it with {} rm",
            self.id_str()
        )));
        let completion = self.tool(&["rm", "--force", name], cancel, true)?;
        let deferred = completion.cancellation_deferred;
        bytes(self.id_str(), completion)?;
        // Verify with a fresh read; writes may finish after cancellation.
        if self
            .containers(&Cancellation::default())?
            .iter()
            .any(|c| c.name == found.name)
        {
            return Err(invalid(
                self.id_str(),
                format!(
                    "{} rm finished, but the container {name} is still there",
                    self.id_str()
                ),
            ));
        }
        Ok(OperationOutcome {
            cancellation_deferred: deferred,
        })
    }
}

impl<T: Transport> Backend for DevContainers<T> {
    fn id(&self) -> &str {
        self.id_str()
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        if !cfg!(target_os = "linux") {
            return Ok(Availability::Unavailable(format!(
                "{} containers run on Linux",
                self.kind.label()
            )));
        }
        let probe = match self.kind {
            Kind::Toolbox => "--version",
            Kind::Distrobox => "version",
        };
        match self.tool(&[probe], cancel, false) {
            Ok(_) => Ok(Availability::Available),
            Err(ExecutionError::Disabled(reason)) => Ok(Availability::Unavailable(reason)),
            Err(error) => Err(error.into()),
        }
    }
    fn may_have(&self, name: &str) -> bool {
        valid_name(name)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        // Only existing containers: creating one is left to the tool.
        let query = query.to_lowercase();
        Ok(self
            .rows(cancel)?
            .into_iter()
            .filter(|row| row.id.name.to_lowercase().contains(&query))
            .collect())
    }
    fn lookup(&mut self, name: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        if !valid_name(name) {
            return Ok(vec![]);
        }
        Ok(self
            .rows(cancel)?
            .into_iter()
            .filter(|row| row.id.name == name)
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
        let mut found = self.find(id, cancel)?;
        if self.kind == Kind::Distrobox {
            if let Some(podman) = self.podman_details(&found, cancel) {
                found.created = podman.created;
                found.image_version = podman.image_version;
            }
        }
        let package = self.package(&found);
        let mut made = format!("Made from {}", found.image);
        if let Some(version) = &found.image_version {
            made.push_str(&format!(" (image version {version})"));
        }
        if let Some(created) = found.created {
            made.push_str(&format!(" on {}", utc_date(created)));
        }
        let mut lines = vec![
            format!("{made}."),
            format!("Container {}, status: {}.", found.id, found.status),
        ];
        let command = match self.kind {
            Kind::Toolbox => {
                lines.push(
                    "Updating runs the container's package manager (dnf, apt-get, pacman, zypper, apk or xbps-install) through `toolbox run` and sudo, starting the container if it is stopped.".into(),
                );
                format!("toolbox enter {}", found.name)
            }
            Kind::Distrobox => {
                let exports = self.exports(&found.name);
                let mut exported = vec![];
                if !exports.applications.is_empty() {
                    exported.push(format!("applications {}", exports.applications.join(", ")));
                }
                if !exports.binaries.is_empty() {
                    exported.push(format!(
                        "commands {} in ~/.local/bin",
                        exports.binaries.join(", ")
                    ));
                }
                if !exported.is_empty() {
                    lines.push(format!("Exported to the host: {}.", exported.join("; ")));
                }
                lines.push(
                    "Updating runs `distrobox upgrade`, which upgrades every package with the container's own package manager, starting the container if it is stopped.".into(),
                );
                format!("distrobox enter {}", found.name)
            }
        };
        lines.push(format!(
            "Install tools with `{command}`. Removing runs `{} rm --force`, which deletes the container and everything installed in it{}; your home folder is kept.",
            self.id_str(),
            match self.kind {
                Kind::Toolbox => "",
                Kind::Distrobox => ", along with what it exported to the host",
            }
        ));
        Ok(PackageDetails {
            package,
            description: lines.join(" "),
            homepage: Some(self.kind.homepage().into()),
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
        let found = self.find(id, cancel)?;
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        if remove {
            return self.remove(&found, cancel, progress);
        }
        let deferred = self.upgrade(&found, cancel, progress)?;
        // Verify with a fresh read; writes may finish after cancellation.
        let after = self.containers(&Cancellation::default())?;
        if !after
            .iter()
            .any(|c| c.name == found.name && c.id == found.id)
        {
            return Err(invalid(
                self.id_str(),
                format!(
                    "{} finished, but the container {} is no longer listed",
                    self.id_str(),
                    found.name
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
    use std::sync::{Arc, Mutex};

    /// `podman ps --all --format json` (Podman 5.8, Fedora 43) with a Toolbx
    /// container, a Distrobox container made from the Toolbx image (which
    /// carries the Toolbx label), a plain Distrobox container and an
    /// unrelated container.
    const PODMAN: &str = r#"[
  {
    "Command": ["toolbox", "--log-level", "debug", "init-container"],
    "CreatedAt": "8 seconds ago",
    "Id": "1b418630ab8e0776d39dab117fe3f214c27e0c774d70c2a698c510a1b0068b55",
    "Image": "registry.fedoraproject.org/fedora-toolbox:43",
    "Labels": {
      "com.github.containers.toolbox": "true",
      "name": "fedora-toolbox",
      "org.opencontainers.image.version": "43",
      "version": "43"
    },
    "Mounts": ["/etc/profile.d/toolbox.sh", "/run/host", "/home/dev", "/usr/bin/toolbox"],
    "Names": ["fedora-toolbox-43"],
    "State": "running",
    "Status": "Up 3 minutes",
    "Created": 1790569975
  },
  {
    "Id": "d03cf38f6ad72c1d9b0d0a6e2b7f1f5e8f3f6c1b1c9e5c4c1f6f1a6b0c2d3e4f",
    "Image": "registry.fedoraproject.org/fedora-toolbox:43",
    "Labels": {
      "com.github.containers.toolbox": "true",
      "distrobox.unshare_groups": "0",
      "manager": "distrobox",
      "org.opencontainers.image.version": "43"
    },
    "Mounts": ["/tmp", "/usr/bin/entrypoint", "/usr/bin/distrobox-host-exec", "/usr/bin/distrobox-export"],
    "Names": ["dbx-fedora"],
    "State": "created",
    "Status": "Created",
    "Created": 1790569990
  },
  {
    "Id": "0c0ffee0c0ffee0c0ffee0c0ffee0c0ffee0c0ffee0c0ffee0c0ffee0c0ffee0c",
    "Image": "registry.fedoraproject.org/fedora-toolbox:42",
    "Labels": {"com.github.containers.toolbox": "true"},
    "Mounts": ["/usr/bin/entrypoint", "/usr/bin/distrobox-export"],
    "Names": ["old-distrobox"],
    "State": "exited",
    "Status": "Exited (0) 2 days ago",
    "Created": 1790000000
  },
  {
    "Id": "2fa79b20b732aa11bb22cc33dd44ee55ff66007788990011223344556677889a",
    "Image": "docker.io/library/alpine:3.22",
    "Labels": {"distrobox.unshare_groups": "0", "manager": "distrobox"},
    "Mounts": ["/usr/bin/entrypoint"],
    "Names": ["alpine-box"],
    "State": "running",
    "Status": "Up About a minute",
    "Created": 1790570000
  },
  {
    "Id": "99aa88bb77cc66dd55ee44ff3300112233445566778899aabbccddeeff001122",
    "Image": "docker.io/library/nginx:latest",
    "Labels": null,
    "Mounts": [],
    "Names": ["web"],
    "State": "running",
    "Status": "Up 1 hour",
    "Created": 1790570001
  },
  {
    "Id": "legacy",
    "Image": "registry.fedoraproject.org/fedora-toolbox:30",
    "Labels": {"com.github.debarshiray.toolbox": "true"},
    "Names": ["--help"],
    "State": "exited"
  }
]"#;

    /// `distrobox list --no-color` (Distrobox 1.8.2.5).
    const DISTROBOX_LIST: &str = "ID           | NAME                 | STATUS             | IMAGE                         \n\
551e7da831c7 | alpine-box           | Up 2 minutes       | docker.io/library/alpine:3.22 \n\
d03cf38f6ad7 | dbx-fedora           | Created            | registry.fedoraproject.org/fedora-toolbox:43\n";

    #[derive(Clone, Default)]
    struct Fake {
        calls: Arc<Mutex<Vec<String>>>,
        /// `distrobox list` output; replaced to simulate a vanished container.
        list: Arc<Mutex<String>>,
        /// The manager `sh -c FIND_MANAGER` reports inside a Toolbx container.
        manager: Option<&'static str>,
        /// Make `sudo -S` fail the way it does when a password is set.
        password: bool,
        /// Every upgrade step inside a Toolbx container fails with this.
        run_failure: Option<ExecutionError>,
        missing: bool,
        /// Every call fails with this.
        failure: Option<ExecutionError>,
        /// Replaces the `podman ps` answer.
        podman: Option<Completion>,
        /// `distrobox list` exits with an error.
        list_fails: bool,
        /// Cancellation arrives while looking for the package manager.
        cancel_after_probe: bool,
        /// Cancellation arrives during the first upgrade step, which finishes.
        cancel_on_write: bool,
        /// `distrobox upgrade` leaves the list without the container.
        vanish: bool,
        /// Set once `rm` took a container out; Podman then lists none.
        removed: Arc<Mutex<bool>>,
        /// `rm` succeeds without removing anything.
        rm_keeps: bool,
        /// `rm` fails with this.
        rm_failure: Option<ExecutionError>,
        home: Option<PathBuf>,
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
        fn env(&self, name: &str) -> Option<OsString> {
            (name == "HOME").then(|| self.home.clone().map(Into::into))?
        }
        fn dev_tool(
            &self,
            executable: &str,
            args: &[OsString],
            cancel: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            let line = format!("{executable} {}", args.join(" "));
            self.calls
                .lock()
                .unwrap()
                .push(format!("{}{line}", if write { "write: " } else { "" }));
            if self.missing {
                return Err(ExecutionError::Disabled(format!("{executable} not found")));
            }
            if let Some(error) = &self.failure {
                return Err(error.clone());
            }
            match (executable, args[0].as_str()) {
                ("podman", "ps") if *self.removed.lock().unwrap() => Ok(done("[]", 0)),
                ("podman", "ps") => Ok(self.podman.clone().unwrap_or_else(|| done(PODMAN, 0))),
                ("toolbox", "--version") => Ok(done("toolbox version 0.3\n", 0)),
                ("distrobox", "version") => Ok(done("distrobox: 1.8.2.5\n", 0)),
                ("distrobox", "list") => Ok(done(
                    &self.list.lock().unwrap(),
                    if self.list_fails { 1 } else { 0 },
                )),
                ("distrobox", "upgrade") => {
                    if self.vanish {
                        self.list.lock().unwrap().clear();
                    }
                    Ok(done(
                        "OK: 26166 distinct packages available\nOK: 537 MiB in 376 packages\n",
                        0,
                    ))
                }
                (_, "rm") => {
                    assert!(write);
                    if let Some(error) = &self.rm_failure {
                        return Err(error.clone());
                    }
                    if !self.rm_keeps {
                        *self.removed.lock().unwrap() = true;
                        let mut list = self.list.lock().unwrap();
                        *list = list
                            .lines()
                            .filter(|line| !line.contains(&format!("| {} ", args[2])))
                            .map(|line| format!("{line}\n"))
                            .collect();
                    }
                    Ok(done("Removing container...\n", 0))
                }
                ("toolbox", "run") if args[3] == "sh" => {
                    if self.cancel_after_probe {
                        cancel.cancel();
                    }
                    match self.manager {
                        Some(manager) => Ok(done(&format!("{manager}\n"), 0)),
                        None => Err(ExecutionError::Failed(done("", 3))),
                    }
                }
                ("toolbox", "run") if self.password => {
                    let mut result = done("", 1);
                    result.stderr =
                        b"sudo: no password was provided\nsudo: a password is required\n".to_vec();
                    Err(ExecutionError::Failed(result))
                }
                _ if self.run_failure.is_some() => Err(self.run_failure.clone().unwrap()),
                _ => {
                    assert_eq!(
                        line.split_whitespace().take(2).collect::<Vec<_>>(),
                        ["toolbox", "run"]
                    );
                    if self.cancel_on_write {
                        cancel.cancel();
                    }
                    Ok(Completion {
                        cancellation_deferred: self.cancel_on_write,
                        ..done("\u{1b}[32mComplete!\u{1b}[0m\n", 0)
                    })
                }
            }
        }
    }
    fn ignore(_: Progress) {}
    fn fake() -> Fake {
        Fake {
            list: Arc::new(Mutex::new(DISTROBOX_LIST.into())),
            manager: Some("dnf"),
            ..Fake::default()
        }
    }
    fn names(rows: &[Package]) -> Vec<&str> {
        rows.iter().map(|r| r.id.name.as_str()).collect()
    }

    #[test]
    fn each_container_is_listed_by_the_tool_that_made_it() {
        let cancel = Cancellation::default();
        let toolbox = DevContainers::toolbox(fake()).installed(&cancel).unwrap();
        // Distrobox containers never appear as Toolbx ones, even with the
        // Toolbx label from their image; unlabelled and invalid ones are skipped.
        assert_eq!(names(&toolbox), vec!["fedora-toolbox-43"]);
        let row = &toolbox[0];
        assert_eq!(row.id.backend, "toolbox");
        assert_eq!(row.installed_version.as_deref(), Some("43"));
        assert_eq!(row.update, UpdateAvailability::Unknown);
        assert_eq!(row.summary, "fedora-toolbox:43 · running");
        let distrobox = DevContainers::distrobox(fake()).installed(&cancel).unwrap();
        assert_eq!(names(&distrobox), vec!["alpine-box", "dbx-fedora"]);
        assert_eq!(distrobox[0].installed_version.as_deref(), Some("3.22"));
        assert_eq!(distrobox[0].summary, "alpine:3.22 · running");
        assert_eq!(distrobox[1].summary, "fedora-toolbox:43 · stopped");
    }

    #[test]
    fn distrobox_list_parsing_tolerates_colors_and_long_status() {
        let colored = "ID           | NAME                 | STATUS             | IMAGE\n\
\u{1b}[32m551e7da831c7 | alpine-box           | Up 2 minutes       | docker.io/library/alpine:3.22 \u{1b}[0m\n\
\u{1b}[33m2fa79b20b732 | very-long-container-name-here | Exited (1) 1 second ago | docker.io/library/ubuntu:26.04\u{1b}[0m\n\
garbage line\n\
nothexnothex | bad | Up | x\n";
        let found = parse_distrobox_list(colored);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id, "551e7da831c7");
        assert!(found[0].running);
        assert_eq!(found[1].name, "very-long-container-name-here");
        assert_eq!(found[1].status, "Exited (1) 1 second ago");
        assert!(!found[1].running);
        assert_eq!(parse_distrobox_list(""), vec![]);
    }

    #[test]
    fn helpers() {
        assert_eq!(
            image_tag("registry.fedoraproject.org/fedora-toolbox:43"),
            "43"
        );
        assert_eq!(image_tag("localhost:5000/tools"), "latest");
        assert_eq!(image_tag("quay.io/toolbx/ubuntu-toolbox:24.04"), "24.04");
        assert_eq!(
            image_tag("docker.io/library/ubuntu@sha256:0123456789abcdef0123"),
            "sha256:0123456789ab"
        );
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_790_569_975), "2026-09-28");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
        assert!(valid_name("fedora-toolbox-43"));
        assert!(!valid_name("-n"));
        assert!(!valid_name("a b"));
        assert!(!valid_name(""));
        assert_eq!(parse_podman(b"null").unwrap().len(), 0);
        assert_eq!(parse_podman(b"[]").unwrap().len(), 0);
    }

    #[test]
    fn exports_are_matched_to_their_container_exactly() {
        // Written by distrobox-export 1.8.2.5.
        let binary = "#!/bin/sh\n# distrobox_binary\n# name: alpine-box\nif [ -z \"${CONTAINER_ID}\" ]; then\n\texec \"/usr/bin/distrobox-enter\"  -n alpine-box  --  '/bin/busybox'  \"$@\"\nfi\n";
        assert!(exported_binary(binary, "alpine-box"));
        assert!(!exported_binary(binary, "alpine"));
        assert!(!exported_binary(
            "#!/bin/sh\n# name: alpine-box\n",
            "alpine-box"
        ));
        let app = "[Desktop Entry]\nType=Application\nName=Hello Tool (on alpine-box)\nExec=/usr/bin/distrobox-enter  -n alpine-box  --   hello-tool  %U\n";
        assert_eq!(
            exported_application(app, "alpine-box").as_deref(),
            Some("Hello Tool")
        );
        assert_eq!(exported_application(app, "alpine"), None);
        // The container's own launcher is not an export.
        let launcher = "[Desktop Entry]\nName=Alpine-box\nExec=/usr/bin/distrobox enter  alpine-box\n\n[Desktop Action Remove]\nExec=/usr/bin/distrobox rm  alpine-box\n";
        assert_eq!(exported_application(launcher, "alpine-box"), None);

        let home = std::env::temp_dir().join(format!("pkgdeck-dbx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let apps = home.join(".local/share/applications");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        std::fs::write(home.join(".local/bin/busybox"), binary).unwrap();
        std::fs::write(home.join(".local/bin/other"), "#!/bin/sh\nexec true\n").unwrap();
        std::fs::write(apps.join("alpine-box-hello-tool.desktop"), app).unwrap();
        std::fs::write(apps.join("alpine-box.desktop"), launcher).unwrap();
        let icon = home.join("alpine.png");
        std::fs::write(&icon, b"png").unwrap();
        std::fs::write(
            apps.join("dbx-fedora.desktop"),
            format!(
                "[Desktop Entry]\nExec=/usr/bin/distrobox enter  dbx-fedora\nIcon={}\n",
                icon.display()
            ),
        )
        .unwrap();
        let backend = DevContainers::distrobox(Fake {
            home: Some(home.clone()),
            ..fake()
        });
        assert_eq!(
            backend.exports("alpine-box"),
            Exports {
                binaries: vec!["busybox".into()],
                applications: vec!["Hello Tool".into()],
            }
        );
        assert_eq!(backend.exports("dbx-fedora"), Exports::default());
        assert_eq!(backend.launcher_icon("dbx-fedora"), Some(icon));
        assert_eq!(backend.launcher_icon("alpine-box"), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn details_show_image_created_and_exports() {
        let cancel = Cancellation::default();
        let mut toolbox = DevContainers::toolbox(fake());
        let row = toolbox.installed(&cancel).unwrap().remove(0);
        let details = toolbox.details(&row.id, &cancel).unwrap();
        assert!(details
            .description
            .starts_with("Made from registry.fedoraproject.org/fedora-toolbox:43 (image version 43) on 2026-09-28. Container 1b418630ab8e, status: Up 3 minutes."), "{}", details.description);
        assert!(details
            .description
            .contains("toolbox enter fedora-toolbox-43"));
        // Distrobox rows borrow the creation time from Podman when the ids match.
        let fake = fake();
        let mut distrobox = DevContainers::distrobox(fake.clone());
        let rows = distrobox.installed(&cancel).unwrap();
        let dbx = distrobox.details(&rows[1].id, &cancel).unwrap();
        assert!(
            dbx.description.contains(" on 2026-09-28."),
            "{}",
            dbx.description
        );
        assert!(dbx.description.contains("(image version 43)"));
        assert!(dbx.description.contains("distrobox upgrade"));
        // alpine-box's Podman id doesn't match the listed one, so nothing is borrowed.
        let alpine = distrobox.details(&rows[0].id, &cancel).unwrap();
        assert!(alpine
            .description
            .starts_with("Made from docker.io/library/alpine:3.22. Container 551e7da831c7"));
        let mut absent = rows[0].id.clone();
        absent.name = "nope".into();
        assert!(matches!(
            distrobox.details(&absent, &cancel),
            Err(EngineError::NotFound)
        ));
    }

    #[test]
    fn distrobox_upgrade_runs_and_verifies() {
        let fake = fake();
        let mut distrobox = DevContainers::distrobox(fake.clone());
        let cancel = Cancellation::default();
        let rows = distrobox.installed(&cancel).unwrap();
        let mut messages = vec![];
        distrobox
            .execute(&Operation::Upgrade(rows[0].id.clone()), &cancel, &mut |p| {
                messages.push(p)
            })
            .unwrap();
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"write: distrobox upgrade alpine-box".into()));
        assert_eq!(
            messages.last(),
            Some(&Progress::Message("OK: 537 MiB in 376 packages".into()))
        );
        // A container gone afterwards is reported, not assumed updated.
        let vanishing = Fake {
            vanish: true,
            ..fake
        };
        let error = DevContainers::distrobox(vanishing)
            .execute(
                &Operation::Upgrade(rows[1].id.clone()),
                &cancel,
                &mut |_| {},
            )
            .unwrap_err();
        assert!(error.to_string().contains("no longer listed"), "{error}");
    }

    #[test]
    fn toolbox_upgrade_uses_the_container_package_manager() {
        let cancel = Cancellation::default();
        for (manager, expected) in [
            ("dnf", vec!["sudo -S dnf -y upgrade --refresh"]),
            (
                "apt-get",
                vec![
                    "sudo -S env DEBIAN_FRONTEND=noninteractive apt-get update",
                    "sudo -S env DEBIAN_FRONTEND=noninteractive apt-get -y -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold upgrade",
                ],
            ),
            ("pacman", vec!["sudo -S pacman -Syu --noconfirm"]),
            ("zypper", vec!["sudo -S zypper --non-interactive update"]),
            ("apk", vec!["sudo -S apk upgrade --update-cache"]),
            ("xbps-install", vec!["sudo -S xbps-install -Suy"]),
        ] {
            let fake = Fake {
                manager: Some(manager),
                ..fake()
            };
            let mut toolbox = DevContainers::toolbox(fake.clone());
            let row = toolbox.installed(&cancel).unwrap().remove(0);
            toolbox
                .execute(&Operation::Upgrade(row.id), &cancel, &mut |_| {})
                .unwrap();
            let writes: Vec<String> = fake
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter_map(|c| {
                    c.strip_prefix("write: toolbox run --container fedora-toolbox-43 ")
                        .map(Into::into)
                })
                .collect();
            assert_eq!(writes, expected, "{manager}");
        }
    }

    #[test]
    fn toolbox_upgrade_refuses_unknown_managers_and_password_prompts() {
        let cancel = Cancellation::default();
        let fake = Fake {
            manager: None,
            ..fake()
        };
        let mut toolbox = DevContainers::toolbox(fake.clone());
        let row = toolbox.installed(&cancel).unwrap().remove(0);
        let error = toolbox
            .execute(&Operation::Upgrade(row.id.clone()), &cancel, &mut |_| {})
            .unwrap_err();
        assert!(error.to_string().contains("no package manager"), "{error}");
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.starts_with("write:")));
        let mut toolbox = DevContainers::toolbox(Fake {
            password: true,
            ..super::tests::fake()
        });
        let error = toolbox
            .execute(&Operation::Upgrade(row.id), &cancel, &mut |_| {})
            .unwrap_err();
        assert!(error.to_string().contains("asks for a password"), "{error}");
    }

    #[test]
    fn nothing_but_upgrades_and_removals_and_nothing_foreign_or_cancelled_runs() {
        let fake = fake();
        let mut distrobox = DevContainers::distrobox(fake.clone());
        assert!(!distrobox.capabilities().contains(&Capability::Install));
        assert!(distrobox.capabilities().contains(&Capability::Remove));
        let cancel = Cancellation::default();
        let row = distrobox.installed(&cancel).unwrap().remove(0);
        for operation in [
            Operation::Install(row.id.clone()),
            Operation::UpgradeAll {
                backend: "distrobox".into(),
            },
            Operation::Refresh {
                backend: "distrobox".into(),
            },
        ] {
            assert!(matches!(
                distrobox.execute(&operation, &cancel, &mut |_| {}),
                Err(EngineError::Unsupported { .. })
            ));
        }
        for operation in [Operation::Upgrade, Operation::Remove] {
            let mut foreign = row.id.clone();
            foreign.backend = "toolbox".into();
            assert!(distrobox
                .execute(&operation(foreign), &cancel, &mut |_| {})
                .unwrap_err()
                .to_string()
                .contains("foreign or invalid container"));
            // `--all` would remove every container.
            let mut option = row.id.clone();
            option.name = "--all".into();
            assert!(distrobox
                .execute(&operation(option), &cancel, &mut |_| {})
                .unwrap_err()
                .to_string()
                .contains("foreign or invalid container"));
            let mut absent = row.id.clone();
            absent.name = "missing-box".into();
            // `distrobox upgrade` offers to create a missing container, so it
            // must never run for one.
            assert!(matches!(
                distrobox.execute(&operation(absent), &cancel, &mut |_| {}),
                Err(EngineError::NotFound)
            ));
            let cancelled = Cancellation::default();
            cancelled.cancel();
            assert!(matches!(
                distrobox.execute(&operation(row.id.clone()), &cancelled, &mut |_| {}),
                Err(EngineError::Cancelled)
            ));
        }
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.starts_with("write:")));
    }

    #[test]
    fn search_and_lookup_find_existing_containers_only() {
        let cancel = Cancellation::default();
        let mut distrobox = DevContainers::distrobox(fake());
        assert_eq!(
            names(&distrobox.search("BOX", &cancel).unwrap()),
            vec!["alpine-box"]
        );
        assert_eq!(
            names(&distrobox.lookup("dbx-fedora", &cancel).unwrap()),
            vec!["dbx-fedora"]
        );
        assert!(distrobox.lookup("dbx", &cancel).unwrap().is_empty());
        assert!(distrobox.lookup("-n", &cancel).unwrap().is_empty());
        assert!(!distrobox.may_have("bad name"));
    }

    #[test]
    fn availability_follows_the_tool() {
        let cancel = Cancellation::default();
        let available = DevContainers::toolbox(fake()).detect(&cancel).unwrap();
        let missing = DevContainers::distrobox(Fake {
            missing: true,
            ..fake()
        })
        .detect(&cancel)
        .unwrap();
        if cfg!(target_os = "linux") {
            assert_eq!(available, Availability::Available);
            assert_eq!(
                missing,
                Availability::Unavailable("distrobox not found".into())
            );
        } else {
            assert!(matches!(available, Availability::Unavailable(_)));
            assert!(matches!(missing, Availability::Unavailable(_)));
        }
    }

    fn sudo_failure(stderr: &str) -> ExecutionError {
        let mut result = done("", 1);
        result.stderr = stderr.as_bytes().to_vec();
        ExecutionError::Failed(result)
    }

    #[test]
    fn toolbox_sudo_refusals_are_told_apart_from_other_failures() {
        let cancel = Cancellation::default();
        let row = DevContainers::toolbox(fake())
            .installed(&cancel)
            .unwrap()
            .remove(0);
        for stderr in [
            "sudo: no password was provided\n",
            "sudo: 1 incorrect password attempt\n",
        ] {
            let mut toolbox = DevContainers::toolbox(Fake {
                run_failure: Some(sudo_failure(stderr)),
                ..fake()
            });
            let error = toolbox
                .execute(&Operation::Upgrade(row.id.clone()), &cancel, &mut ignore)
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("toolbox enter fedora-toolbox-43"),
                "{error}"
            );
        }
        for failure in [
            sudo_failure("sudo: unknown user dev\n"),
            ExecutionError::Io("toolbox crashed".into()),
        ] {
            let mut toolbox = DevContainers::toolbox(Fake {
                run_failure: Some(failure.clone()),
                ..fake()
            });
            assert_eq!(
                toolbox.execute(&Operation::Upgrade(row.id.clone()), &cancel, &mut ignore),
                Err(EngineError::Execution(failure))
            );
        }
    }

    #[test]
    fn toolbox_cancellation_stops_before_the_next_step() {
        let row = DevContainers::toolbox(fake())
            .installed(&Cancellation::default())
            .unwrap()
            .remove(0);
        // Cancelled while looking for the manager: nothing is written.
        let probe = Fake {
            cancel_after_probe: true,
            ..fake()
        };
        let mut toolbox = DevContainers::toolbox(probe.clone());
        assert_eq!(
            toolbox.execute(
                &Operation::Upgrade(row.id.clone()),
                &Cancellation::default(),
                &mut ignore
            ),
            Err(EngineError::Cancelled)
        );
        assert!(!probe
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.starts_with("write:")));
        // Cancelled during apt-get update: the upgrade step never starts, and
        // the finished step is reported as deferred.
        let apt = Fake {
            manager: Some("apt-get"),
            cancel_on_write: true,
            ..fake()
        };
        let mut toolbox = DevContainers::toolbox(apt.clone());
        let mut messages = vec![];
        let outcome = toolbox
            .execute(
                &Operation::Upgrade(row.id),
                &Cancellation::default(),
                &mut |progress| messages.push(progress),
            )
            .unwrap();
        assert!(outcome.cancellation_deferred);
        let writes = apt
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.starts_with("write:"))
            .count();
        assert_eq!(writes, 1);
        assert_eq!(
            messages.last(),
            Some(&Progress::Message("Complete!".into()))
        );
    }

    #[test]
    fn failed_listings_are_errors_not_empty_lists() {
        let cancel = Cancellation::default();
        let mut toolbox = DevContainers::toolbox(Fake {
            podman: Some(done("", 125)),
            ..fake()
        });
        assert!(matches!(
            toolbox.installed(&cancel),
            Err(EngineError::Execution(ExecutionError::Failed(result))) if result.code == Some(125)
        ));
        let mut distrobox = DevContainers::distrobox(Fake {
            list_fails: true,
            ..fake()
        });
        assert!(matches!(
            distrobox.installed(&cancel),
            Err(EngineError::Execution(ExecutionError::Failed(result))) if result.code == Some(1)
        ));
        let failure = ExecutionError::Io("permission denied".into());
        let mut broken = DevContainers::toolbox(Fake {
            failure: Some(failure.clone()),
            ..fake()
        });
        if cfg!(target_os = "linux") {
            assert_eq!(broken.detect(&cancel), Err(EngineError::Execution(failure)));
        } else {
            assert_eq!(
                broken.detect(&cancel),
                Ok(Availability::Unavailable(
                    "Toolbx containers run on Linux".into()
                ))
            );
        }
    }

    #[test]
    fn a_container_without_a_status_shows_its_state() {
        let podman = r#"[{"Id":"1b418630ab8e0776","Image":"registry.fedoraproject.org/fedora-toolbox:43",
            "Labels":{"com.github.containers.toolbox":"true"},"Names":["plain"],"State":"exited","Status":""}]"#;
        let mut toolbox = DevContainers::toolbox(Fake {
            podman: Some(done(podman, 0)),
            ..fake()
        });
        let cancel = Cancellation::default();
        let row = toolbox.installed(&cancel).unwrap().remove(0);
        let details = toolbox.details(&row.id, &cancel).unwrap();
        assert!(
            details
                .description
                .contains("Container 1b418630ab8e, status: exited."),
            "{}",
            details.description
        );
    }

    #[test]
    fn plain_drops_escape_sequences_only() {
        assert_eq!(plain("\u{1b}[1;32mok\u{1b}[0m done"), "ok done");
        // A lone escape swallows the character after it and nothing more.
        assert_eq!(plain("a\u{1b}Xb"), "ab");
        assert_eq!(plain("tail\u{1b}"), "tail");
    }

    #[test]
    fn distrobox_details_name_host_exports() {
        let home = std::env::temp_dir().join(format!("pkgdeck-dbx-details-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let bin = home.join(".local/bin");
        let apps = home.join(".local/share/applications");
        std::fs::create_dir_all(bin.join("not-a-script")).unwrap();
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(
            bin.join("busybox"),
            "#!/bin/sh\n# distrobox_binary\n# name: alpine-box\n",
        )
        .unwrap();
        // Oversized files are never read, even when they look like exports.
        let mut large = "#!/bin/sh\n# distrobox_binary\n# name: alpine-box\n".to_string();
        large.push_str(&"#".repeat(64 * 1024));
        std::fs::write(bin.join("huge"), large).unwrap();
        std::fs::write(
            apps.join("alpine-box-hello.desktop"),
            "[Desktop Entry]\nName=Hello (on alpine-box)\nExec=/usr/bin/distrobox-enter -n alpine-box -- hello\n",
        )
        .unwrap();
        let mut distrobox = DevContainers::distrobox(Fake {
            home: Some(home.clone()),
            ..fake()
        });
        let cancel = Cancellation::default();
        let row = distrobox.installed(&cancel).unwrap().remove(0);
        let details = distrobox.details(&row.id, &cancel).unwrap();
        assert!(
            details.description.contains(
                "Exported to the host: applications Hello; commands busybox in ~/.local/bin."
            ),
            "{}",
            details.description
        );
        std::fs::remove_file(apps.join("alpine-box-hello.desktop")).unwrap();
        let details = distrobox.details(&row.id, &cancel).unwrap();
        assert!(details
            .description
            .contains("Exported to the host: commands busybox in ~/.local/bin."));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn removing_a_container_runs_the_tool_and_is_verified() {
        let cancel = Cancellation::default();
        for (kind, name, expected) in [
            (
                Kind::Toolbox,
                "fedora-toolbox-43",
                "write: toolbox rm --force fedora-toolbox-43",
            ),
            (
                Kind::Distrobox,
                "alpine-box",
                "write: distrobox rm --force alpine-box",
            ),
        ] {
            let fake = fake();
            let mut backend = DevContainers {
                transport: fake.clone(),
                kind,
            };
            let row = backend
                .installed(&cancel)
                .unwrap()
                .into_iter()
                .find(|row| row.id.name == name)
                .unwrap();
            let details = backend.details(&row.id, &cancel).unwrap();
            assert!(
                details.description.contains(&format!(
                    "Removing runs `{} rm --force`, which deletes the container and everything installed in it",
                    kind.id()
                )),
                "{}",
                details.description
            );
            assert_eq!(
                details.description.contains("what it exported to the host"),
                kind == Kind::Distrobox
            );
            let mut messages = vec![];
            backend
                .execute(&Operation::Remove(row.id.clone()), &cancel, &mut |p| {
                    messages.push(p)
                })
                .unwrap();
            assert!(fake.calls.lock().unwrap().contains(&expected.into()));
            assert_eq!(
                messages,
                [Progress::Message(format!(
                    "Removing the container {name} and everything installed in it with {} rm",
                    kind.id()
                ))]
            );
            assert!(!names(&backend.installed(&cancel).unwrap()).contains(&name));
            // Only that container went; the other Distrobox one stays.
            if kind == Kind::Distrobox {
                assert_eq!(names(&backend.installed(&cancel).unwrap()), ["dbx-fedora"]);
            }
        }
    }

    #[test]
    fn failed_or_unverified_container_removals_are_errors() {
        let cancel = Cancellation::default();
        let row = DevContainers::toolbox(fake())
            .installed(&cancel)
            .unwrap()
            .remove(0);
        let failure = ExecutionError::Failed(done("", 1));
        let mut toolbox = DevContainers::toolbox(Fake {
            rm_failure: Some(failure.clone()),
            ..fake()
        });
        assert_eq!(
            toolbox.execute(&Operation::Remove(row.id.clone()), &cancel, &mut ignore),
            Err(EngineError::Execution(failure))
        );
        let mut toolbox = DevContainers::toolbox(Fake {
            rm_keeps: true,
            ..fake()
        });
        let error = toolbox
            .execute(&Operation::Remove(row.id), &cancel, &mut ignore)
            .unwrap_err();
        assert!(
            error.to_string().contains(
                "toolbox rm finished, but the container fedora-toolbox-43 is still there"
            ),
            "{error}"
        );
    }
}
