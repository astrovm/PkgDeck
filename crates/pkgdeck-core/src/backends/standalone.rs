//! Updates for upstream-owned CLI installations. Discovery never installs tools.
use super::*;
use semver::Version;
use serde_json::Value;
use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    sync::Mutex,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandaloneTool {
    Codex,
    Claude,
    Grok,
    OpenCode,
    Cursor,
    Copilot,
    Kiro,
    Antigravity,
    Amp,
    Droid,
    Solana,
    Anchor,
    Foundry,
}
impl StandaloneTool {
    pub const ALL: [Self; 13] = [
        Self::Codex,
        Self::Claude,
        Self::Grok,
        Self::OpenCode,
        Self::Cursor,
        Self::Copilot,
        Self::Kiro,
        Self::Antigravity,
        Self::Amp,
        Self::Droid,
        Self::Solana,
        Self::Anchor,
        Self::Foundry,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Grok => "grok",
            Self::OpenCode => "opencode",
            Self::Cursor => "cursor",
            Self::Copilot => "copilot",
            Self::Kiro => "kiro",
            Self::Antigravity => "antigravity",
            Self::Amp => "amp",
            Self::Droid => "droid",
            Self::Solana => "solana",
            Self::Anchor => "anchor",
            Self::Foundry => "foundry",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Grok => "Grok",
            Self::OpenCode => "OpenCode",
            Self::Cursor => "Cursor CLI",
            Self::Copilot => "GitHub Copilot CLI",
            Self::Kiro => "Kiro CLI",
            Self::Antigravity => "Antigravity CLI",
            Self::Amp => "Amp",
            Self::Droid => "Factory Droid",
            Self::Solana => "Solana CLI (Agave)",
            Self::Anchor => "Anchor (AVM)",
            Self::Foundry => "Foundry",
        }
    }
    /// Text a tool's `--version` output must contain, where it names itself.
    /// The rest install a bare binary whose output is only a version.
    fn version_marker(self) -> Option<&'static str> {
        match self {
            Self::Copilot => Some("GitHub Copilot CLI"),
            Self::Kiro => Some("kiro-cli"),
            Self::Solana => Some("agave-install"),
            Self::Foundry => Some("forge"),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Installation {
    launcher: PathBuf,
    /// What installs a new release: the launcher itself, or the toolchain's
    /// own installer beside it (`avm`, `foundryup`).
    updater: PathBuf,
    version: Version,
    channel: String,
}
trait StandaloneIo: Send {
    fn locate(
        &self,
        tool: StandaloneTool,
        cancel: &Cancellation,
    ) -> Result<Option<Installation>, EngineError>;
    fn latest(
        &self,
        tool: StandaloneTool,
        installation: &Installation,
        cancel: &Cancellation,
    ) -> Result<Version, EngineError>;
    fn update(
        &self,
        tool: StandaloneTool,
        installation: &Installation,
        version: &Version,
        cancel: &Cancellation,
    ) -> Result<Completion, EngineError>;
    fn remove(
        &self,
        tool: StandaloneTool,
        installation: &Installation,
        cancel: &Cancellation,
    ) -> Result<Completion, EngineError>;
}
struct NativeStandalone {
    host: Host,
    /// Removal moves files here instead of deleting them, so a mistake can
    /// be undone: macOS's `trash`. Linux deletes them.
    trash: Option<PathBuf>,
    /// Remembered `--version` answers; `None` asks every time.
    versions: Option<Versions>,
}
/// Each binary's `--version` answer, kept until the binary changes. Some
/// tools take seconds to answer it.
struct Versions {
    store: Option<crate::cache::Store>,
    /// Clean answers by fingerprint of the launcher and its binary.
    seen: Mutex<BTreeMap<String, Completion>>,
}
impl Versions {
    fn new(store: Option<crate::cache::Store>) -> Self {
        Self {
            store,
            seen: Mutex::default(),
        }
    }
    fn seen(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Completion>> {
        self.seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    fn read(
        &self,
        tool: StandaloneTool,
        launcher: &Path,
        run: impl FnOnce() -> Result<Completion, ExecutionError>,
    ) -> Result<Completion, ExecutionError> {
        use crate::cache::{fingerprint, Watch};
        let binary = fs::canonicalize(launcher).unwrap_or_else(|_| launcher.into());
        let watches = [Watch::file(launcher), Watch::file(binary)];
        let before = fingerprint(&watches, &[]).unwrap_or_default();
        if let Some(known) = self.seen().get(&before) {
            return Ok(known.clone());
        }
        let result = crate::cache::completion(
            self.store.as_ref(),
            tool.id(),
            "version",
            &[&launcher.to_string_lossy()],
            &watches,
            &[],
            run,
        )?;
        let clean = result.code == Some(0) && !result.truncated;
        if clean && fingerprint(&watches, &[]).unwrap_or_default() == before {
            self.seen().insert(before, result.clone());
        }
        Ok(result)
    }
    /// After an update or removal, which may change more than the binary.
    fn forget(&self, tool: StandaloneTool) {
        self.seen().clear();
        if let Some(store) = &self.store {
            store.invalidate(tool.id());
        }
    }
}
pub struct Standalone {
    tool: StandaloneTool,
    io: Box<dyn StandaloneIo>,
}
impl Standalone {
    pub fn native(tool: StandaloneTool) -> Self {
        Self {
            tool,
            io: Box::new(NativeStandalone {
                host: Host::current(),
                trash: cfg!(target_os = "macos").then(|| "/usr/bin/trash".into()),
                versions: Some(Versions::new(crate::cache::Store::user())),
            }),
        }
    }
}
fn invalid_data(tool: StandaloneTool, reason: impl std::fmt::Display) -> EngineError {
    invalid(tool.id(), reason)
}
/// GitHub answers 403 or 429 when an address checks too often. Say that
/// instead of showing curl's words.
fn github_rate_limit(tool: StandaloneTool, url: &str, error: EngineError) -> EngineError {
    let EngineError::Execution(ExecutionError::Failed(result)) = &error else {
        return error;
    };
    let stderr = String::from_utf8_lossy(&result.stderr);
    if url.starts_with("https://github.com/")
        && (stderr.contains("error: 403") || stderr.contains("error: 429"))
    {
        return invalid_data(
            tool,
            "GitHub is limiting update checks from this network. Try again in about an hour",
        );
    }
    error
}
enum ReleaseLink<'a> {
    Tag(&'a str),
    /// The repository moved; its latest link is at the new name.
    Moved,
}
/// Where GitHub's latest-release link points: a release tag, or the same
/// link under a moved repository's new name.
fn release_link(location: &str) -> Option<ReleaseLink<'_>> {
    let rest = location.strip_prefix("https://github.com/")?;
    let mut parts = rest.splitn(5, '/');
    let (owner, repo, releases) = (parts.next()?, parts.next()?, parts.next()?);
    let plain = |part: &str| {
        part.bytes().any(|b| b != b'.')
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    if !plain(owner) || !plain(repo) || releases != "releases" {
        return None;
    }
    match (parts.next()?, parts.next()) {
        ("latest", None) => Some(ReleaseLink::Moved),
        ("tag", Some(tag)) if plain(tag) => Some(ReleaseLink::Tag(tag)),
        _ => None,
    }
}
fn version(tool: StandaloneTool, text: &str) -> Result<Version, EngineError> {
    // Foundry marks its stable builds `1.3.5-stable`; the suffix is not a
    // pre-release.
    let text = text
        .trim()
        .trim_start_matches("rust-v")
        .trim_start_matches('v')
        .trim_end_matches("-stable");
    Version::parse(text)
        .or_else(|error| date_version(text).ok_or(error))
        .map_err(|error| invalid_data(tool, format!("invalid version: {error}")))
}
/// Date versions such as Cursor's `2026.09.26-dd393fe`, which semver rejects
/// for the leading zero. The build suffix is metadata: dates set precedence.
fn date_version(text: &str) -> Option<Version> {
    let (date, build) = match text.split_once('-') {
        Some((date, build)) => (date, Some(build)),
        None => (text, None),
    };
    let mut parts = date.split('.').map(|part| {
        (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u64>().ok())
            .flatten()
    });
    let (major, minor, patch) = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() {
        return None;
    }
    let mut version = Version::new(major, minor, patch);
    if let Some(build) = build {
        version.build = semver::BuildMetadata::new(build).ok()?;
    }
    Some(version)
}
fn installed_version(tool: StandaloneTool, text: &str) -> Result<Version, EngineError> {
    text.split_whitespace()
        .map(|part| part.trim_end_matches(['.', ',']))
        .find_map(|part| version(tool, part).ok())
        .ok_or_else(|| invalid_data(tool, "unrecognized installed version"))
}
impl Standalone {
    fn installation(&self, cancel: &Cancellation) -> Result<Installation, EngineError> {
        self.io
            .locate(self.tool, cancel)?
            .ok_or(EngineError::NotFound)
    }
    fn package(&self, installation: &Installation, candidate: Option<Version>) -> Package {
        Package {
            id: PackageId {
                backend: self.tool.id().into(),
                name: self.tool.id().into(),
                architecture: std::env::consts::ARCH.into(),
                scope: Scope::User {
                    uid: rustix::process::getuid().as_raw(),
                },
                remote: None,
                reference: Some(installation.launcher.to_string_lossy().into()),
            },
            display_name: self.tool.name().into(),
            summary: "Standalone CLI tool".into(),
            installed_version: Some(installation.version.to_string()),
            update: match &candidate {
                Some(candidate) if candidate.cmp_precedence(&installation.version).is_gt() => {
                    UpdateAvailability::Available
                }
                Some(_) => UpdateAvailability::Current,
                None => UpdateAvailability::Unknown,
            },
            candidate_version: candidate.map(|v| v.to_string()),
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        }
    }
    /// Shows what a tool's updater or removal printed; returns whether it
    /// finished after a cancellation.
    fn finish(
        &self,
        completion: Completion,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<bool, EngineError> {
        let deferred = completion.cancellation_deferred;
        let output = bytes(self.id(), completion)?;
        if !output.is_empty() {
            progress(Progress::Message(String::from_utf8_lossy(&output).into()));
        }
        Ok(deferred)
    }
    fn remove(
        &self,
        installation: &Installation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        progress(Progress::Message(format!(
            "Removing {} {}",
            self.tool.name(),
            installation.version
        )));
        let completion = self.io.remove(self.tool, installation, cancel)?;
        let deferred = self.finish(completion, progress)?;
        // Native writes may complete after cancellation. Verification must still run.
        if self
            .io
            .locate(self.tool, &Cancellation::default())?
            .is_some()
        {
            return Err(invalid_data(
                self.tool,
                "removal finished but the tool is still installed",
            ));
        }
        progress(Progress::Message(format!(
            "{} removed. Its settings were kept.",
            self.tool.name()
        )));
        Ok(OperationOutcome {
            cancellation_deferred: deferred,
        })
    }
}
impl Backend for Standalone {
    fn id(&self) -> &str {
        self.tool.id()
    }
    fn capabilities(&self) -> &[Capability] {
        &[
            Capability::Search,
            Capability::Installed,
            Capability::Details,
            Capability::Upgrade,
            Capability::Remove,
        ]
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        Ok(if self.io.locate(self.tool, cancel)?.is_some() {
            Availability::Available
        } else {
            Availability::Unavailable("No supported standalone installation found".into())
        })
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        // Search only known installed tools; never manufacture install offers.
        if !self
            .tool
            .name()
            .to_lowercase()
            .contains(&query.to_lowercase())
            && !self.tool.id().contains(&query.to_lowercase())
        {
            return Ok(vec![]);
        }
        Ok(self
            .io
            .locate(self.tool, cancel)?
            .map(|i| self.package(&i, None))
            .into_iter()
            .collect())
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let Some(installation) = self.io.locate(self.tool, cancel)? else {
            return Ok(vec![]);
        };
        let candidate = self.io.latest(self.tool, &installation, cancel)?;
        Ok(vec![self.package(&installation, Some(candidate))])
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let installation = self.installation(cancel)?;
        let package = self.package(&installation, None);
        if package.id != *id {
            return Err(EngineError::NotFound);
        }
        Ok(PackageDetails {
            description: format!(
                "{} standalone installation\n{}\nChannel: {}",
                self.tool.name(),
                installation.launcher.display(),
                installation.channel
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
        let (Operation::Upgrade(id) | Operation::Remove(id)) = operation else {
            return Err(self.unsupported(operation.capability()));
        };
        let installation = self.installation(cancel)?;
        if self.package(&installation, None).id != *id {
            return Err(EngineError::NotFound);
        }
        if let Operation::Remove(_) = operation {
            return self.remove(&installation, cancel, progress);
        }
        let candidate = self.io.latest(self.tool, &installation, cancel)?;
        if !candidate.cmp_precedence(&installation.version).is_gt() {
            return Ok(OperationOutcome::default());
        }
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        progress(Progress::Message(format!(
            "Updating {} {} → {} with its own updater",
            self.tool.name(),
            installation.version,
            candidate
        )));
        let completion = self
            .io
            .update(self.tool, &installation, &candidate, cancel)?;
        let deferred = self.finish(completion, progress)?;
        // Native writes may complete after cancellation. Verification must still run.
        let updated = self.installation(&Cancellation::default())?;
        if !updated
            .version
            .cmp_precedence(&installation.version)
            .is_gt()
        {
            return Err(invalid_data(self.tool, "updater completed without installing a newer version; check the tool's update policy"));
        }
        progress(Progress::Message(format!(
            "{} updated to {}. Restart existing sessions to use it.",
            self.tool.name(),
            updated.version
        )));
        Ok(OperationOutcome {
            cancellation_deferred: deferred,
        })
    }
}

fn read_text(path: &Path) -> Result<Option<String>, EngineError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ExecutionError::from(e).into()),
    };
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(ExecutionError::from)?;
    if bytes.len() > 1024 * 1024 {
        return Err(ExecutionError::Invalid("standalone metadata too large".into()).into());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|e| ExecutionError::Invalid(e.to_string()).into())
}
impl NativeStandalone {
    fn home(&self) -> Result<PathBuf, EngineError> {
        self.host
            .var("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or_else(|| ExecutionError::Invalid("absolute HOME required".into()).into())
    }
    fn setting_path(&self, key: &str, default: PathBuf) -> Result<PathBuf, EngineError> {
        let path = self.host.var(key).map(PathBuf::from).unwrap_or(default);
        if !path.is_absolute() {
            return Err(ExecutionError::Invalid(format!("{key} must be absolute")).into());
        }
        Ok(path)
    }
    fn fetch(&self, url: &str, cancel: &Cancellation) -> Result<String, EngineError> {
        self.curl(
            &[
                "--location",
                "--proto-redir",
                "=https",
                "--max-filesize",
                "1048576",
                url,
            ],
            1024 * 1024,
            cancel,
        )
    }
    /// Where `url` redirects, without following it.
    fn redirect(&self, url: &str, cancel: &Cancellation) -> Result<String, EngineError> {
        self.curl(
            &[
                "--output",
                "/dev/null",
                "--write-out",
                "%{redirect_url}",
                url,
            ],
            64 * 1024,
            cancel,
        )
    }
    /// curl over HTTPS only, with short timeouts, then `args`.
    fn curl(
        &self,
        args: &[&str],
        output_bytes: usize,
        cancel: &Cancellation,
    ) -> Result<String, EngineError> {
        let curl = self.host.resolve("curl")?.ok_or_else(|| {
            ExecutionError::Disabled("curl is required to check standalone updates".into())
        })?;
        let args: Vec<OsString> = [
            "-q",
            "--fail",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--connect-timeout",
            "5",
            "--max-time",
            "15",
            "--user-agent",
            "PkgDeck",
        ]
        .iter()
        .chain(args)
        .map(Into::into)
        .collect();
        let result = self.host.read(
            &curl,
            &args,
            Limits {
                timeout: Duration::from_secs(20),
                output_bytes,
            },
            cancel,
        )?;
        let bytes = bytes("standalone", result)?;
        String::from_utf8(bytes).map_err(|e| invalid("standalone", e))
    }
    fn version_at(
        &self,
        tool: StandaloneTool,
        launcher: &Path,
        cancel: &Cancellation,
    ) -> Result<Option<Version>, EngineError> {
        let run = || {
            self.host
                .read(launcher, &["--version".into()], Limits::default(), cancel)
        };
        let output = match &self.versions {
            Some(versions) => versions.read(tool, launcher, run)?,
            None => run()?,
        };
        let text = String::from_utf8_lossy(&bytes(tool.id(), output)?).into_owned();
        // A binary with the right name that does not identify as the tool is
        // someone else's.
        if tool
            .version_marker()
            .is_some_and(|marker| !text.contains(marker))
        {
            return Ok(None);
        }
        installed_version(tool, &text).map(Some)
    }
}

impl NativeStandalone {
    /// Agave's installer keeps releases in its data folder and records the
    /// release channel it follows in its config.
    fn locate_solana(
        &self,
        home: &Path,
        cancel: &Cancellation,
    ) -> Result<Option<Installation>, EngineError> {
        let tool = StandaloneTool::Solana;
        let install = home.join(".local/share/solana/install");
        let launcher = install.join("active_release/bin/agave-install");
        if native_binary(tool, &launcher, &install.join("releases"))?.is_none() {
            return Ok(None);
        }
        let Some(config) = read_text(&home.join(".config/solana/install/config.yml"))? else {
            return Ok(None);
        };
        let Some(channel) = solana_channel(&config) else {
            return Ok(None);
        };
        let Some(version) = self.version_at(tool, &launcher, cancel)? else {
            return Ok(None);
        };
        Ok(Some(Installation {
            updater: launcher.clone(),
            launcher,
            version,
            channel,
        }))
    }
    /// AVM keeps each Anchor release as `bin/anchor-<version>` and names the
    /// active one in `.version`.
    fn locate_anchor(&self, home: &Path) -> Result<Option<Installation>, EngineError> {
        let tool = StandaloneTool::Anchor;
        let avm = self.setting_path("AVM_HOME", home.join(".avm"))?;
        let bin = avm.join("bin");
        let launcher = bin.join("avm");
        if native_binary(tool, &launcher, &bin)?.is_none() {
            return Ok(None);
        }
        let Some(active) = read_text(&avm.join(".version"))? else {
            return Ok(None);
        };
        let active = active.trim();
        let version = version(tool, active)?;
        if native_binary(tool, &bin.join(format!("anchor-{active}")), &bin)?.is_none() {
            return Ok(None);
        }
        Ok(Some(Installation {
            updater: launcher.clone(),
            launcher,
            version,
            channel: "latest".into(),
        }))
    }
    /// foundryup installs each release under `versions` and links forge,
    /// cast, anvil and chisel into `bin`, beside itself.
    fn locate_foundry(
        &self,
        home: &Path,
        cancel: &Cancellation,
    ) -> Result<Option<Installation>, EngineError> {
        let tool = StandaloneTool::Foundry;
        let root = self.setting_path("FOUNDRY_DIR", home.join(".foundry"))?;
        let launcher = root.join("bin/forge");
        let updater = root.join("bin/foundryup");
        if native_binary(tool, &launcher, &root)?.is_none()
            || native_binary(tool, &updater, &root.join("bin"))?.is_none()
        {
            return Ok(None);
        }
        let Some(version) = self.version_at(tool, &launcher, cancel)? else {
            return Ok(None);
        };
        Ok(Some(Installation {
            channel: if version.pre.as_str() == "nightly" {
                "nightly"
            } else {
                "stable"
            }
            .into(),
            launcher,
            updater,
            version,
        }))
    }
    /// The newest release on an Agave channel: the channel names a commit,
    /// and the workspace version at that commit is its release.
    fn solana_latest(
        &self,
        installation: &Installation,
        cancel: &Cancellation,
    ) -> Result<Version, EngineError> {
        let tool = StandaloneTool::Solana;
        let target = match (std::env::consts::ARCH, std::env::consts::OS) {
            (arch, "macos") => format!("{arch}-apple-darwin"),
            (arch, _) => format!("{arch}-unknown-linux-gnu"),
        };
        let manifest = self.fetch(
            &format!(
                "https://release.anza.xyz/{}/solana-release-{target}.yml",
                installation.channel
            ),
            cancel,
        )?;
        let commit = manifest
            .lines()
            .find_map(|line| line.strip_prefix("commit:"))
            .map(str::trim)
            .filter(|commit| commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| invalid_data(tool, "channel names no release commit"))?;
        let cargo = self.fetch(
            &format!("https://raw.githubusercontent.com/anza-xyz/agave/{commit}/Cargo.toml"),
            cancel,
        )?;
        let release = cargo
            .split("[workspace.package]")
            .nth(1)
            .and_then(|section| {
                section
                    .lines()
                    .find_map(|line| line.trim().strip_prefix("version = \""))
            })
            .and_then(|rest| rest.split('"').next())
            .ok_or_else(|| invalid_data(tool, "release names no version"))?;
        version(tool, release)
    }
}

/// The channel an Agave config follows (`explicit_release: !Channel
/// stable`), or `pinned` for a fixed release; anything else isn't Agave's.
fn solana_channel(config: &str) -> Option<String> {
    let release = config
        .lines()
        .find_map(|line| line.strip_prefix("explicit_release:"))?
        .trim();
    if release.starts_with("!Semver ") {
        return Some("pinned".into());
    }
    release
        .strip_prefix("!Channel ")
        .filter(|channel| matches!(*channel, "stable" | "beta" | "edge"))
        .map(Into::into)
}

impl StandaloneIo for NativeStandalone {
    fn locate(
        &self,
        tool: StandaloneTool,
        cancel: &Cancellation,
    ) -> Result<Option<Installation>, EngineError> {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let home = self.home()?;
        let (launcher, root) = match tool {
            StandaloneTool::Codex => (
                self.setting_path("CODEX_INSTALL_DIR", home.join(".local/bin"))?
                    .join("codex"),
                self.setting_path("CODEX_HOME", home.join(".codex"))?
                    .join("packages/standalone/releases"),
            ),
            StandaloneTool::Claude => (
                home.join(".local/bin/claude"),
                self.setting_path("XDG_DATA_HOME", home.join(".local/share"))?
                    .join("claude/versions"),
            ),
            StandaloneTool::Grok => (
                self.setting_path("GROK_BIN_DIR", home.join(".grok/bin"))?
                    .join("grok"),
                home.join(".grok/downloads"),
            ),
            StandaloneTool::OpenCode => (
                home.join(".opencode/bin/opencode"),
                home.join(".opencode/bin"),
            ),
            // Grok's installer can also create `~/.local/bin/agent`; only a
            // launcher resolving into Cursor's own versions folder is Cursor's.
            StandaloneTool::Cursor => {
                let root = home.join(".local/share/cursor-agent/versions");
                let agent = home.join(".local/bin/agent");
                let launcher = if native_binary(tool, &agent, &root)?.is_some() {
                    agent
                } else {
                    home.join(".local/bin/cursor-agent")
                };
                (launcher, root)
            }
            // These installers place one binary in ~/.local/bin; Homebrew, npm
            // and system packages link from elsewhere and are not adopted.
            StandaloneTool::Copilot => (home.join(".local/bin/copilot"), home.join(".local/bin")),
            StandaloneTool::Kiro => (home.join(".local/bin/kiro-cli"), home.join(".local/bin")),
            StandaloneTool::Antigravity => (home.join(".local/bin/agy"), home.join(".local/bin")),
            StandaloneTool::Amp => {
                let root = self
                    .setting_path("AMP_HOME", home.join(".amp"))?
                    .join("bin");
                (root.join("amp"), root)
            }
            StandaloneTool::Droid => (home.join(".local/bin/droid"), home.join(".local/bin")),
            StandaloneTool::Solana => return self.locate_solana(&home, cancel),
            StandaloneTool::Anchor => return self.locate_anchor(&home),
            StandaloneTool::Foundry => return self.locate_foundry(&home, cancel),
        };
        let Some(binary) = native_binary(tool, &launcher, &root)? else {
            return Ok(None);
        };
        if tool == StandaloneTool::Codex {
            // bin/codex sits two levels below its release folder.
            let manifest = binary
                .ancestors()
                .nth(2)
                .map(|release| release.join("codex-package.json"))
                .unwrap_or_default();
            let Some(text) = read_text(&manifest)? else {
                return Ok(None);
            };
            let data: Value = serde_json::from_str(&text).map_err(|e| invalid_data(tool, e))?;
            if data["layoutVersion"] != 1
                || data["entrypoint"] != "bin/codex"
                || data["variant"] != "codex"
            {
                return Ok(None);
            }
        }
        let mut channel = "latest".to_owned();
        if tool == StandaloneTool::Claude {
            let config = self.setting_path("CLAUDE_CONFIG_DIR", home.join(".claude"))?;
            for path in [
                config.join("settings.json"),
                self.host.filesystem_path(std::path::Path::new(
                    "/etc/claude-code/managed-settings.json",
                )),
            ] {
                if let Some(text) = read_text(&path)? {
                    let settings: Value =
                        serde_json::from_str(&text).map_err(|e| invalid_data(tool, e))?;
                    if let Some(value) = settings.get("autoUpdatesChannel") {
                        channel = value
                            .as_str()
                            .filter(|v| matches!(*v, "stable" | "latest"))
                            .ok_or_else(|| invalid_data(tool, "unsupported update channel"))?
                            .into();
                    }
                }
            }
        } else if tool == StandaloneTool::Grok {
            channel = "configured channel".into();
        }
        let Some(version) = self.version_at(tool, &launcher, cancel)? else {
            return Ok(None);
        };
        Ok(Some(Installation {
            version,
            updater: launcher.clone(),
            launcher,
            channel,
        }))
    }
    fn latest(
        &self,
        tool: StandaloneTool,
        installation: &Installation,
        cancel: &Cancellation,
    ) -> Result<Version, EngineError> {
        match (tool, installation.channel.as_str()) {
            // A pinned Agave release and Foundry's nightly builds have
            // no newer release to offer.
            (StandaloneTool::Solana, "pinned") | (StandaloneTool::Foundry, "nightly") => {
                return Ok(installation.version.clone());
            }
            (StandaloneTool::Solana, _) => return self.solana_latest(installation, cancel),
            _ => {}
        }
        if tool == StandaloneTool::Grok {
            let output = self.host.read(
                &installation.launcher,
                &["update".into(), "--check".into(), "--json".into()],
                Limits {
                    timeout: Duration::from_secs(30),
                    ..Limits::default()
                },
                cancel,
            )?;
            let data: Value = serde_json::from_slice(&bytes(tool.id(), output)?)
                .map_err(|e| invalid_data(tool, e))?;
            if !data["error"].is_null() || data["installer"] != "internal" {
                return Err(invalid_data(
                    tool,
                    "native update check failed or installation is managed externally",
                ));
            }
            match data["updateAvailable"].as_bool() {
                Some(false) => return Ok(installation.version.clone()),
                Some(true) => {}
                None => return Err(invalid_data(tool, "missing update availability")),
            }
            return version(
                tool,
                data["latestVersion"]
                    .as_str()
                    .ok_or_else(|| invalid_data(tool, "missing latestVersion"))?,
            );
        }
        if tool == StandaloneTool::Droid {
            // The installer script pins the current release as VER="x.y.z".
            let script = self.fetch("https://app.factory.ai/cli", cancel)?;
            let release = script
                .lines()
                .find_map(|line| line.trim().strip_prefix("VER=\""))
                .and_then(|rest| rest.split('"').next())
                .ok_or_else(|| invalid_data(tool, "installer names no release"))?;
            return version(tool, release);
        }
        if tool == StandaloneTool::Amp {
            return version(
                tool,
                &self.fetch("https://static.ampcode.com/cli/cli-version.txt", cancel)?,
            );
        }
        if tool == StandaloneTool::Cursor {
            // The installer script names the current release in its download URL.
            let script = self.fetch("https://cursor.com/install", cancel)?;
            let release = script
                .split("downloads.cursor.com/lab/")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .ok_or_else(|| invalid_data(tool, "installer names no release"))?;
            return version(tool, release);
        }
        if matches!(tool, StandaloneTool::Kiro | StandaloneTool::Antigravity) {
            let url = if tool == StandaloneTool::Kiro {
                "https://prod.download.cli.kiro.dev/stable/latest/manifest.json".to_owned()
            } else {
                let os = std::env::consts::OS.replace("macos", "darwin");
                let arch = std::env::consts::ARCH
                    .replace("aarch64", "arm64")
                    .replace("x86_64", "amd64");
                format!("https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests/{os}_{arch}.json")
            };
            let data: Value = serde_json::from_str(&self.fetch(&url, cancel)?)
                .map_err(|e| invalid_data(tool, e))?;
            return version(
                tool,
                data["version"]
                    .as_str()
                    .ok_or_else(|| invalid_data(tool, "missing version"))?,
            );
        }
        let repository = match tool {
            StandaloneTool::OpenCode => Some("anomalyco/opencode"),
            StandaloneTool::Anchor => Some("solana-foundation/anchor"),
            StandaloneTool::Foundry => Some("foundry-rs/foundry"),
            StandaloneTool::Copilot => Some("github/copilot-cli"),
            _ => None,
        };
        if let Some(repository) = repository {
            // The release page's latest link redirects to the newest stable
            // release, never a draft or pre-release. GitHub's API would allow
            // an address only 60 checks an hour.
            // A moved repository redirects to its new name first.
            let mut url = format!("https://github.com/{repository}/releases/latest");
            for _ in 0..3 {
                let location = self
                    .redirect(&url, cancel)
                    .map_err(|error| github_rate_limit(tool, &url, error))?;
                match release_link(location.trim()) {
                    Some(ReleaseLink::Tag(tag)) => return version(tool, tag),
                    Some(ReleaseLink::Moved) => url = location.trim().to_owned(),
                    None => break,
                }
            }
            return Err(invalid_data(tool, "no stable release"));
        }
        let url = match tool {
            StandaloneTool::Codex => "https://releases.openai.com/codex/channels/latest".into(),
            // Claude; every other tool returned above.
            _ => format!(
                "https://downloads.claude.ai/claude-code-releases/{}",
                installation.channel
            ),
        };
        let text = self.fetch(&url, cancel)?;
        if tool == StandaloneTool::Claude {
            return version(tool, &text);
        }
        let data: Value = serde_json::from_str(&text).map_err(|e| invalid_data(tool, e))?;
        if data["draft"].as_bool() == Some(true) || data["prerelease"].as_bool() == Some(true) {
            return Err(invalid_data(tool, "release is not stable"));
        }
        version(
            tool,
            data["tag_name"]
                .as_str()
                .ok_or_else(|| invalid_data(tool, "missing release tag"))?,
        )
    }
    fn update(
        &self,
        tool: StandaloneTool,
        installation: &Installation,
        candidate: &Version,
        cancel: &Cancellation,
    ) -> Result<Completion, EngineError> {
        let result = self.run_update(tool, installation, candidate, cancel);
        self.forget(tool);
        result
    }
    fn remove(
        &self,
        tool: StandaloneTool,
        installation: &Installation,
        cancel: &Cancellation,
    ) -> Result<Completion, EngineError> {
        let result = self.run_remove(tool, installation, cancel);
        self.forget(tool);
        result
    }
}

impl NativeStandalone {
    fn forget(&self, tool: StandaloneTool) {
        if let Some(versions) = &self.versions {
            versions.forget(tool);
        }
    }
    fn run_update(
        &self,
        tool: StandaloneTool,
        installation: &Installation,
        candidate: &Version,
        cancel: &Cancellation,
    ) -> Result<Completion, EngineError> {
        // Recheck ownership/layout immediately before invoking any updater.
        let current = self.locate(tool, cancel)?.ok_or(EngineError::NotFound)?;
        if current.launcher != installation.launcher {
            return Err(EngineError::NotFound);
        }
        if !current.version.cmp_precedence(candidate).is_lt() {
            return Ok(Completion {
                code: Some(0),
                signal: None,
                stdout: vec![],
                stderr: vec![],
                truncated: false,
                cancellation_deferred: false,
            });
        }
        let args: Vec<OsString> = match tool {
            StandaloneTool::Codex => return self.install_codex(installation, candidate, cancel),
            StandaloneTool::Claude => vec!["update".into()],
            StandaloneTool::Grok => vec![
                "update".into(),
                "--version".into(),
                candidate.to_string().into(),
            ],
            StandaloneTool::OpenCode => vec![
                "upgrade".into(),
                candidate.to_string().into(),
                "--method".into(),
                "curl".into(),
            ],
            // These update to their latest release; the version check that
            // follows confirms it.
            StandaloneTool::Cursor
            | StandaloneTool::Copilot
            | StandaloneTool::Antigravity
            | StandaloneTool::Amp
            | StandaloneTool::Droid => vec!["update".into()],
            StandaloneTool::Kiro => vec!["update".into(), "--non-interactive".into()],
            // `agave-install update` follows the configured channel.
            StandaloneTool::Solana => vec!["update".into()],
            // `avm install` activates the release it installs. `avm update`
            // is not used: older AVMs pick pre-releases without binaries.
            StandaloneTool::Anchor => vec!["install".into(), candidate.to_string().into()],
            StandaloneTool::Foundry => vec!["--install".into(), "stable".into()],
        };
        Ok(self
            .host
            .standalone_write(&installation.updater, &args, &[], cancel)?)
    }
    fn run_remove(
        &self,
        tool: StandaloneTool,
        installation: &Installation,
        cancel: &Cancellation,
    ) -> Result<Completion, EngineError> {
        crate::host::refuse_root(true)?;
        // Recheck ownership/layout immediately before removing anything.
        let current = self.locate(tool, cancel)?.ok_or(EngineError::NotFound)?;
        if current.launcher != installation.launcher {
            return Err(EngineError::NotFound);
        }
        let home = self.home()?;
        let removal = self.removal(tool, &home)?;
        let targets = removal.targets(tool, &home)?;
        let completion = match &self.trash {
            Some(trash) => {
                let args: Vec<OsString> = targets.iter().map(Into::into).collect();
                self.host.standalone_write(trash, &args, &[], cancel)?
            }
            None => {
                for target in &targets {
                    delete(target)?;
                }
                Completion {
                    code: Some(0),
                    signal: None,
                    stdout: vec![],
                    stderr: vec![],
                    truncated: false,
                    cancellation_deferred: false,
                }
            }
        };
        // Only folders left empty go; anything else in them stays.
        for folder in &removal.emptied {
            let _ = fs::remove_dir(folder);
        }
        Ok(completion)
    }
}

/// What removing a tool takes away: what its official installer put there,
/// never the tool's settings, credentials or keys.
struct Removal {
    /// The tool's own folder. A launcher goes only while it resolves here.
    root: PathBuf,
    launchers: Vec<PathBuf>,
    /// Folders and files the installer created.
    paths: Vec<PathBuf>,
    /// Folders that go too once nothing else is left in them.
    emptied: Vec<PathBuf>,
}
impl NativeStandalone {
    /// Paths come from the same settings `locate` reads.
    fn removal(&self, tool: StandaloneTool, home: &Path) -> Result<Removal, EngineError> {
        let bin = home.join(".local/bin");
        let removal = |root: PathBuf, launchers: &[&str], paths: Vec<PathBuf>| Removal {
            launchers: launchers.iter().map(|name| bin.join(name)).collect(),
            root,
            paths,
            emptied: vec![],
        };
        Ok(match tool {
            StandaloneTool::Codex => {
                let dir = self.setting_path("CODEX_INSTALL_DIR", bin.clone())?;
                let codex = self.setting_path("CODEX_HOME", home.join(".codex"))?;
                let root = codex.join("packages/standalone");
                Removal {
                    launchers: vec![dir.join("codex"), dir.join("codex-code-mode-host")],
                    paths: vec![root.clone()],
                    root,
                    emptied: vec![codex.join("packages")],
                }
            }
            StandaloneTool::Claude => {
                let root = self
                    .setting_path("XDG_DATA_HOME", home.join(".local/share"))?
                    .join("claude");
                removal(root.clone(), &["claude"], vec![root])
            }
            // Grok links both `grok` and `agent`, beside itself and in
            // ~/.local/bin.
            StandaloneTool::Grok => {
                let dir = self.setting_path("GROK_BIN_DIR", home.join(".grok/bin"))?;
                let root = home.join(".grok/downloads");
                let mut removal = removal(root.clone(), &["grok", "agent"], vec![root]);
                removal
                    .launchers
                    .extend([dir.join("grok"), dir.join("agent")]);
                removal.emptied.push(home.join(".grok/bin"));
                removal
            }
            // ~/.opencode also holds OpenCode's plugins, which stay.
            StandaloneTool::OpenCode => {
                let root = home.join(".opencode/bin");
                removal(root.clone(), &[], vec![root])
            }
            StandaloneTool::Cursor => {
                let root = home.join(".local/share/cursor-agent");
                removal(root.clone(), &["agent", "cursor-agent"], vec![root])
            }
            StandaloneTool::Copilot => removal(bin.clone(), &["copilot"], vec![]),
            StandaloneTool::Kiro => removal(bin.clone(), &["kiro-cli", "kiro-cli-chat"], vec![]),
            StandaloneTool::Antigravity => removal(bin.clone(), &["agy"], vec![]),
            StandaloneTool::Droid => removal(bin.clone(), &["droid"], vec![]),
            // The installer links amp into ~/.local/bin and keeps its
            // download checks beside the binary.
            StandaloneTool::Amp => {
                let amp = self.setting_path("AMP_HOME", home.join(".amp"))?;
                let root = amp.join("bin");
                let mut paths = vec![root.clone()];
                paths.extend(
                    [
                        "amp-install-version.txt",
                        "amp-install-checksum.txt",
                        "amp-install-signature.minisign",
                        "signing-key.pub",
                    ]
                    .map(|name| amp.join(name)),
                );
                let mut removal = removal(root, &["amp"], paths);
                removal.emptied.push(amp);
                removal
            }
            // Keypairs and the installer's channel live in ~/.config/solana
            // and stay.
            StandaloneTool::Solana => {
                let solana = home.join(".local/share/solana");
                let mut removal =
                    removal(solana.join("install"), &[], vec![solana.join("install")]);
                removal.emptied.push(solana);
                removal
            }
            // AVM installs itself with Cargo, which records it beside `bin`.
            StandaloneTool::Anchor => {
                let avm = self.setting_path("AVM_HOME", home.join(".avm"))?;
                let paths = ["bin", ".version", ".crates.toml", ".crates2.json"]
                    .map(|name| avm.join(name))
                    .into();
                let mut removal = removal(avm.join("bin"), &[], paths);
                removal.emptied.push(avm);
                removal
            }
            // Cast wallets (keystores) and caches stay in the Foundry folder.
            StandaloneTool::Foundry => {
                let foundry = self.setting_path("FOUNDRY_DIR", home.join(".foundry"))?;
                let paths = ["bin", "versions", "share/man"]
                    .map(|name| foundry.join(name))
                    .into();
                let mut removal = removal(foundry.join("bin"), &[], paths);
                removal.emptied.extend([foundry.join("share"), foundry]);
                removal
            }
        })
    }
}
impl Removal {
    /// What to remove, checked like `native_binary` checks a launcher: a
    /// folder or file must be the user's own, not a link somewhere else, not
    /// in a package manager's prefix and not the home folder or above it.
    fn targets(&self, tool: StandaloneTool, home: &Path) -> Result<Vec<PathBuf>, EngineError> {
        let mut launchers: Vec<PathBuf> = vec![];
        let root = fs::canonicalize(&self.root).ok();
        for launcher in &self.launchers {
            // Launchers that now point elsewhere belong to something else.
            let ours = fs::canonicalize(launcher)
                .is_ok_and(|target| root.as_ref().is_some_and(|root| target.starts_with(root)));
            // GROK_BIN_DIR may name ~/.local/bin, listing a link twice.
            if ours && !launchers.contains(launcher) {
                launchers.push(launcher.clone());
            }
        }
        // Launchers go last, so a removal failing partway still leaves the
        // tool findable, and removable again.
        let mut targets = vec![];
        let home = fs::canonicalize(home).map_err(ExecutionError::from)?;
        for path in &self.paths {
            let metadata = match fs::symlink_metadata(path) {
                Ok(metadata) => metadata,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(ExecutionError::from(e).into()),
            };
            let foreign = metadata.file_type().is_symlink()
                || metadata.uid() != rustix::process::getuid().as_raw()
                || {
                    let path = fs::canonicalize(path).map_err(ExecutionError::from)?;
                    home.starts_with(&path) || managed(&path)
                };
            if foreign {
                return Err(invalid_data(
                    tool,
                    format!(
                        "PkgDeck won't remove {}: it links somewhere else, isn't yours, or holds your home folder. Nothing was removed.",
                        path.display()
                    ),
                ));
            }
            targets.push(path.clone());
        }
        targets.extend(launchers);
        Ok(targets)
    }
}

/// Deletes a file, a link (not what it points to) or a whole folder.
fn delete(path: &Path) -> Result<(), EngineError> {
    let folder = fs::symlink_metadata(path)
        .map_err(ExecutionError::from)?
        .is_dir();
    if folder {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .map_err(|e| ExecutionError::from(e).into())
}

/// Whether a path lies in npm's or Homebrew's own storage.
fn managed(path: &Path) -> bool {
    path.components().any(|p| {
        matches!(
            p.as_os_str().to_str(),
            Some("node_modules" | "Cellar" | "Caskroom")
        )
    })
}

impl NativeStandalone {
    /// Download to private storage, then run the official pinned-version
    /// installer. No network bytes are piped into a shell.
    fn install_codex(
        &self,
        installation: &Installation,
        candidate: &Version,
        cancel: &Cancellation,
    ) -> Result<Completion, EngineError> {
        let script = self.fetch("https://chatgpt.com/codex/install.sh", cancel)?;
        let temporary = InstallerFile::create(script.as_bytes())?;
        let sh = self
            .host
            .resolve("sh")?
            .ok_or_else(|| ExecutionError::Disabled("sh not found".into()))?;
        let parent = installation
            .launcher
            .parent()
            .ok_or(EngineError::NotFound)?;
        Ok(self.host.standalone_write(
            &sh,
            &[
                temporary.0.join("install.sh").into(),
                "--release".into(),
                candidate.to_string().into(),
            ],
            &[
                ("CODEX_NON_INTERACTIVE", "1".into()),
                ("CODEX_INSTALL_DIR", parent.as_os_str().into()),
            ],
            cancel,
        )?)
    }
}

/// A standalone install is an owned native binary in the upstream's private
/// storage. npm scripts and Homebrew/package-manager symlinks are not adopted.
fn native_binary(
    tool: StandaloneTool,
    launcher: &Path,
    root: &Path,
) -> Result<Option<PathBuf>, EngineError> {
    let binary = match fs::canonicalize(launcher) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ExecutionError::from(e).into()),
    };
    let root = match fs::canonicalize(root) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ExecutionError::from(e).into()),
    };
    // Do not follow a private storage root redirected into a manager prefix.
    if !binary.starts_with(&root) || managed(&root) {
        return Ok(None);
    }
    let metadata = fs::metadata(&binary).map_err(ExecutionError::from)?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Ok(None);
    }
    // Cursor ships a launcher script beside its bundled Node app.
    if tool == StandaloneTool::Cursor {
        let bundled = binary
            .file_name()
            .is_some_and(|name| name == "cursor-agent")
            && binary
                .parent()
                .is_some_and(|dir| dir.join("index.js").is_file());
        return Ok(bundled.then_some(binary));
    }
    let mut header = [0; 4];
    if fs::File::open(&binary)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_err()
    {
        return Ok(None);
    }
    if !matches!(
        header,
        [0x7f, b'E', b'L', b'F']
            | [0xcf, 0xfa, 0xed, 0xfe]
            | [0xfe, 0xed, 0xfa, 0xcf]
            | [0xca, 0xfe, 0xba, 0xbe]
    ) {
        return Ok(None);
    }
    Ok(Some(binary))
}

struct InstallerFile(PathBuf);
impl InstallerFile {
    fn create(bytes: &[u8]) -> Result<Self, EngineError> {
        use std::io::Write;
        use std::os::unix::fs::DirBuilderExt;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "pkgdeck-installer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(ExecutionError::from)?;
        let owned = Self(path);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(owned.0.join("install.sh"))
            .map_err(ExecutionError::from)?;
        file.write_all(bytes).map_err(ExecutionError::from)?;
        Ok(owned)
    }
}
impl Drop for InstallerFile {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Runtime;
    use std::{
        collections::BTreeMap,
        os::unix::fs::symlink,
        sync::{Arc, Mutex, OnceLock},
    };
    fn ok(text: &str) -> Completion {
        Completion {
            code: Some(0),
            signal: None,
            stdout: text.as_bytes().to_vec(),
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
        }
    }
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "pkgdeck-standalone-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, name: &str, text: impl AsRef<[u8]>) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            path
        }
        fn script(&self, name: &str, text: &str) -> PathBuf {
            let path = self.write(name, text);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
        fn native(&self) -> NativeStandalone {
            // The curl shim is invariant, so install it once before any spawn:
            // rewriting an executable right before executing it trips an
            // ETXTBSY race under parallel test churn (see tests/host.rs).
            self.script("bin/curl", "#!/bin/sh\nfor arg do url=$arg; done\nprintf '%s' \"$url\" > \"$HOME/request\"\ncase \"$url\" in */install.sh) cat \"$HOME/installer\";; *.yml) cat \"$HOME/channel\";; *) cat \"$HOME/release\";; esac\n");
            NativeStandalone {
                host: Host::new(
                    Runtime::Native,
                    BTreeMap::from([
                        ("HOME".into(), self.0.as_os_str().into()),
                        (
                            "PATH".into(),
                            format!("{}:/usr/bin:/bin", self.0.join("bin").display()).into(),
                        ),
                    ]),
                ),
                trash: None,
                // The fixture reads its version from a file beside it, which
                // real tools don't; see `versions_are_asked_once_per_binary`.
                versions: None,
            }
        }
        /// Like `native`, moving removals to the Trash the way macOS does:
        /// a shim records what it was given, then deletes it, or fails while
        /// `trash-fails` exists. Written once, like the curl shim.
        fn trashing(&self) -> NativeStandalone {
            let trash = self.script("trash/trash", "#!/bin/sh\n[ -e \"$HOME/trash-fails\" ] && { echo 'could not move to the Trash' >&2; exit 5; }\nfor path do printf '%s\\n' \"$path\" >> \"$HOME/trashed\"; rm -rf \"$path\"; done\n");
            NativeStandalone {
                trash: Some(trash),
                ..self.native()
            }
        }
        fn install(&self, tool: StandaloneTool) -> PathBuf {
            static BINARY: OnceLock<PathBuf> = OnceLock::new();
            let fixture = BINARY.get_or_init(|| {
                let dir = Temp::new();
                let binary = dir.0.join("fixture");
                assert!(std::process::Command::new("cc")
                    .args(["-o"])
                    .arg(&binary)
                    .arg(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/tests/fixtures/standalone.c"
                    ))
                    .status()
                    .unwrap()
                    .success());
                std::mem::forget(dir);
                binary
            });
            let (launcher, target) = match tool {
                StandaloneTool::Codex => (
                    ".local/bin/codex",
                    ".codex/packages/standalone/releases/1.0.0-test/bin/codex",
                ),
                StandaloneTool::Claude => {
                    (".local/bin/claude", ".local/share/claude/versions/1.0.0")
                }
                StandaloneTool::Grok => (".grok/bin/grok", ".grok/downloads/grok-linux-test"),
                StandaloneTool::OpenCode => (".opencode/bin/opencode", ".opencode/bin/opencode"),
                StandaloneTool::Cursor => (
                    ".local/bin/agent",
                    ".local/share/cursor-agent/versions/1.0.0/cursor-agent",
                ),
                StandaloneTool::Copilot => (".local/bin/copilot", ".local/bin/copilot"),
                StandaloneTool::Kiro => (".local/bin/kiro-cli", ".local/bin/kiro-cli"),
                StandaloneTool::Antigravity => (".local/bin/agy", ".local/bin/agy"),
                StandaloneTool::Amp => (".amp/bin/amp", ".amp/bin/amp"),
                StandaloneTool::Droid => (".local/bin/droid", ".local/bin/droid"),
                StandaloneTool::Solana => (
                    ".local/share/solana/install/active_release/bin/agave-install",
                    ".local/share/solana/install/releases/stable-test/solana-release/bin/agave-install",
                ),
                StandaloneTool::Anchor => (".avm/bin/avm", ".avm/bin/avm"),
                StandaloneTool::Foundry => (
                    ".foundry/bin/forge",
                    ".foundry/versions/foundry-rs/foundry/v1.0.0/forge",
                ),
            };
            let target_path = self.0.join(target);
            fs::create_dir_all(target_path.parent().unwrap()).unwrap();
            fs::hard_link(fixture, &target_path).unwrap();
            let launcher = self.0.join(launcher);
            fs::create_dir_all(launcher.parent().unwrap()).unwrap();
            if launcher != target_path {
                symlink(&target_path, &launcher).unwrap();
            }
            if tool == StandaloneTool::Codex {
                self.write(".codex/packages/standalone/releases/1.0.0-test/codex-package.json", r#"{"layoutVersion":1,"version":"1.0.0","entrypoint":"bin/codex","variant":"codex"}"#);
            }
            if tool == StandaloneTool::Cursor {
                self.write(".local/share/cursor-agent/versions/1.0.0/index.js", "");
            }
            match tool {
                StandaloneTool::Solana => {
                    self.write(
                        ".config/solana/install/config.yml",
                        "---\ncurrent_update_manifest: null\nexplicit_release: !Channel stable\n",
                    );
                }
                // AVM's active Anchor, and foundryup beside Foundry's links.
                StandaloneTool::Anchor => {
                    self.write(".avm/.version", "1.0.0");
                    fs::hard_link(fixture, self.0.join(".avm/bin/anchor-1.0.0")).unwrap();
                }
                StandaloneTool::Foundry => {
                    fs::hard_link(fixture, self.0.join(".foundry/bin/foundryup")).unwrap();
                }
                _ => {}
            }
            // Tools that name themselves in --version keep doing so after updates.
            if let Some(prefix) = match tool {
                StandaloneTool::Copilot => Some("GitHub Copilot CLI "),
                StandaloneTool::Kiro => Some("kiro-cli "),
                StandaloneTool::Solana => Some("agave-install "),
                StandaloneTool::Foundry => Some("forge Version: "),
                _ => None,
            } {
                self.write(
                    &format!(
                        "{}.prefix",
                        launcher.strip_prefix(&self.0).unwrap().display()
                    ),
                    prefix,
                );
                self.write(
                    &format!(
                        "{}.version",
                        launcher.strip_prefix(&self.0).unwrap().display()
                    ),
                    format!("{prefix}1.0.0."),
                );
            }
            launcher
        }
        fn network(&self, release: &str) {
            // Only release metadata varies per check; the curl shim is
            // installed once by native() and never rewritten (see above).
            self.write("release", release);
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[derive(Clone)]
    struct Fixture {
        installed: Arc<Mutex<Option<Installation>>>,
        latest: Result<Version, EngineError>,
        writes: Arc<Mutex<usize>>,
        change: bool,
        deferred: bool,
        /// Cancel while checking for the newer version.
        cancel_on_check: bool,
        /// Cancel once the installation is found, as pressing Cancel right
        /// after confirming would.
        cancel_on_locate: bool,
    }
    impl Fixture {
        fn new() -> Self {
            Self {
                installed: Arc::new(Mutex::new(Some(Installation {
                    launcher: "/synthetic/bin/tool".into(),
                    updater: "/synthetic/bin/tool".into(),
                    version: Version::new(1, 0, 0),
                    channel: "latest".into(),
                }))),
                latest: Ok(Version::new(2, 0, 0)),
                writes: Arc::default(),
                change: true,
                deferred: false,
                cancel_on_check: false,
                cancel_on_locate: false,
            }
        }
    }
    impl StandaloneIo for Fixture {
        fn locate(
            &self,
            _: StandaloneTool,
            cancel: &Cancellation,
        ) -> Result<Option<Installation>, EngineError> {
            if cancel.requested() {
                return Err(EngineError::Cancelled);
            }
            if self.cancel_on_locate {
                cancel.cancel();
            }
            Ok(self.installed.lock().unwrap().clone())
        }
        fn latest(
            &self,
            _: StandaloneTool,
            _: &Installation,
            cancel: &Cancellation,
        ) -> Result<Version, EngineError> {
            if self.cancel_on_check {
                cancel.cancel();
            }
            self.latest.clone()
        }
        fn update(
            &self,
            _: StandaloneTool,
            _: &Installation,
            candidate: &Version,
            _: &Cancellation,
        ) -> Result<Completion, EngineError> {
            *self.writes.lock().unwrap() += 1;
            if self.change {
                self.installed.lock().unwrap().as_mut().unwrap().version = candidate.clone();
            }
            let mut result = ok("Updated");
            result.cancellation_deferred = self.deferred;
            Ok(result)
        }
        fn remove(
            &self,
            _: StandaloneTool,
            _: &Installation,
            _: &Cancellation,
        ) -> Result<Completion, EngineError> {
            *self.writes.lock().unwrap() += 1;
            if self.change {
                *self.installed.lock().unwrap() = None;
            }
            let mut result = ok("");
            result.cancellation_deferred = self.deferred;
            Ok(result)
        }
    }
    #[test]
    fn standalone_lifecycle_uses_exact_identity_and_confirms_version() {
        for tool in StandaloneTool::ALL {
            let fixture = Fixture::new();
            let mut backend = Standalone {
                tool,
                io: Box::new(fixture.clone()),
            };
            let cancel = Cancellation::default();
            assert_eq!(backend.id(), tool.id());
            assert_eq!(
                backend.capabilities(),
                [
                    Capability::Search,
                    Capability::Installed,
                    Capability::Details,
                    Capability::Upgrade,
                    Capability::Remove
                ]
            );
            assert_eq!(backend.detect(&cancel).unwrap(), Availability::Available);
            assert!(backend.search("nonexistent", &cancel).unwrap().is_empty());
            assert_eq!(
                backend.search(tool.id(), &cancel).unwrap()[0].update,
                UpdateAvailability::Unknown
            );
            let package = backend.installed(&cancel).unwrap().remove(0);
            assert_eq!(package.update, UpdateAvailability::Available);
            assert_eq!(package.installed_version.as_deref(), Some("1.0.0"));
            assert!(backend
                .details(&package.id, &cancel)
                .unwrap()
                .description
                .contains("standalone"));
            let mut foreign = package.id.clone();
            foreign.reference = Some("/other/tool".into());
            assert!(backend.details(&foreign, &cancel).is_err());
            assert!(backend
                .execute(&Operation::Upgrade(foreign.clone()), &cancel, &mut |_| {})
                .is_err());
            assert!(backend
                .execute(&Operation::Remove(foreign), &cancel, &mut |_| {})
                .is_err());
            assert_eq!(*fixture.writes.lock().unwrap(), 0);
            let mut progress = vec![];
            backend
                .execute(&Operation::Upgrade(package.id.clone()), &cancel, &mut |p| {
                    progress.push(p)
                })
                .unwrap();
            assert_eq!(*fixture.writes.lock().unwrap(), 1);
            assert!(!progress.is_empty());
            assert_eq!(
                backend.installed(&cancel).unwrap()[0].update,
                UpdateAvailability::Current
            );
            backend
                .execute(
                    &Operation::Upgrade(package.id.clone()),
                    &cancel,
                    &mut drop::<Progress>,
                )
                .unwrap();
            assert_eq!(*fixture.writes.lock().unwrap(), 1);
            // Removal confirms the tool is gone afterwards.
            let mut progress = vec![];
            backend
                .execute(&Operation::Remove(package.id), &cancel, &mut |p| {
                    progress.push(p)
                })
                .unwrap();
            assert_eq!(*fixture.writes.lock().unwrap(), 2);
            assert!(matches!(
                progress.last(),
                Some(Progress::Message(text)) if text.ends_with("removed. Its settings were kept.")
            ));
            assert!(backend.installed(&cancel).unwrap().is_empty());
        }
    }
    #[test]
    fn standalone_errors_cancellation_missing_and_newer_installations() {
        let cancel = Cancellation::default();
        let mut fixture = Fixture::new();
        fixture.change = false;
        let mut backend = Standalone {
            tool: StandaloneTool::Codex,
            io: Box::new(fixture.clone()),
        };
        let id = backend.installed(&cancel).unwrap()[0].id.clone();
        assert!(backend
            .execute(&Operation::Upgrade(id.clone()), &cancel, &mut |_| {})
            .unwrap_err()
            .to_string()
            .contains("without installing"));
        fixture.change = true;
        fixture.deferred = true;
        backend.io = Box::new(fixture.clone());
        assert!(
            backend
                .execute(&Operation::Upgrade(id.clone()), &cancel, &mut |_| {})
                .unwrap()
                .cancellation_deferred
        );
        // A cancellation during the version check stops before the updater.
        fixture.cancel_on_check = true;
        fixture.latest = Ok(Version::new(3, 0, 0));
        backend.io = Box::new(fixture.clone());
        let writes = *fixture.writes.lock().unwrap();
        assert!(matches!(
            backend.execute(
                &Operation::Upgrade(id.clone()),
                &Cancellation::default(),
                &mut drop::<Progress>
            ),
            Err(EngineError::Cancelled)
        ));
        assert_eq!(*fixture.writes.lock().unwrap(), writes);
        fixture.cancel_on_check = false;
        fixture.latest = Ok(Version::new(1, 0, 0));
        backend.io = Box::new(fixture.clone());
        assert_eq!(
            backend.installed(&cancel).unwrap()[0].update,
            UpdateAvailability::Current
        );
        fixture.latest = Err(ExecutionError::TimedOut.into());
        backend.io = Box::new(fixture.clone());
        assert!(backend.installed(&cancel).is_err());
        assert!(backend
            .execute(&Operation::Upgrade(id), &cancel, &mut |_| {})
            .is_err());
        *fixture.installed.lock().unwrap() = None;
        assert!(matches!(
            backend.detect(&cancel).unwrap(),
            Availability::Unavailable(_)
        ));
        assert!(backend.installed(&cancel).unwrap().is_empty());
        assert!(backend.search("codex", &cancel).unwrap().is_empty());
        cancel.cancel();
        assert!(backend.detect(&cancel).is_err());
        assert!(installed_version(StandaloneTool::Codex, "broken").is_err());
        assert_eq!(
            installed_version(StandaloneTool::Codex, "codex-cli 1.2.3-beta.1").unwrap(),
            Version::parse("1.2.3-beta.1").unwrap()
        );
    }
    #[test]
    fn native_discovery_and_channels_use_only_upstream_owned_binaries() {
        let temp = Temp::new();
        let native = temp.native();
        let cancel = Cancellation::default();
        for tool in StandaloneTool::ALL {
            assert!(native.locate(tool, &cancel).unwrap().is_none());
            let launcher = temp.install(tool);
            let installation = native.locate(tool, &cancel).unwrap().unwrap();
            assert_eq!(installation.launcher, launcher);
            assert_eq!(installation.version, Version::new(1, 0, 0));
        }
        // Settings without a channel keep the default one.
        temp.write(".claude/settings.json", r#"{"theme":"dark"}"#);
        assert_eq!(
            native
                .locate(StandaloneTool::Claude, &cancel)
                .unwrap()
                .unwrap()
                .channel,
            "latest"
        );
        temp.write(
            ".claude/settings.json",
            r#"{"autoUpdatesChannel":"stable"}"#,
        );
        assert_eq!(
            native
                .locate(StandaloneTool::Claude, &cancel)
                .unwrap()
                .unwrap()
                .channel,
            "stable"
        );
        temp.write(".claude/settings.json", r#"{"autoUpdatesChannel":"other"}"#);
        assert!(native.locate(StandaloneTool::Claude, &cancel).is_err());
        let launcher = temp.0.join(".opencode/bin/opencode");
        fs::remove_file(&launcher).unwrap();
        let npm = temp.script("node_modules/opencode/bin/opencode", "#!/bin/sh\nexit 0\n");
        symlink(npm, &launcher).unwrap();
        assert!(native
            .locate(StandaloneTool::OpenCode, &cancel)
            .unwrap()
            .is_none());
        fs::remove_file(&launcher).unwrap();
        temp.script(".opencode/bin/opencode", "#!/bin/sh\nexit 0\n");
        assert!(native
            .locate(StandaloneTool::OpenCode, &cancel)
            .unwrap()
            .is_none());
        cancel.cancel();
        assert!(native.locate(StandaloneTool::Codex, &cancel).is_err());
    }
    #[test]
    fn new_ai_clis_are_adopted_only_from_their_own_installers() {
        let temp = Temp::new();
        let native = temp.native();
        let cancel = Cancellation::default();
        // Grok's installer can own ~/.local/bin/agent; Cursor then answers to
        // its other launcher, and a Grok link is never taken for Cursor.
        let grok = temp.install(StandaloneTool::Grok);
        let agent = temp.0.join(".local/bin/agent");
        fs::create_dir_all(agent.parent().unwrap()).unwrap();
        symlink(&grok, &agent).unwrap();
        assert!(native
            .locate(StandaloneTool::Cursor, &cancel)
            .unwrap()
            .is_none());
        fs::remove_file(&agent).unwrap();
        temp.install(StandaloneTool::Cursor);
        fs::remove_file(&agent).unwrap();
        symlink(&grok, &agent).unwrap();
        let fallback = temp.0.join(".local/bin/cursor-agent");
        symlink(
            temp.0
                .join(".local/share/cursor-agent/versions/1.0.0/cursor-agent"),
            &fallback,
        )
        .unwrap();
        assert_eq!(
            native
                .locate(StandaloneTool::Cursor, &cancel)
                .unwrap()
                .unwrap()
                .launcher,
            fallback
        );
        // A Cursor script without its bundle is not Cursor's.
        fs::remove_file(
            temp.0
                .join(".local/share/cursor-agent/versions/1.0.0/index.js"),
        )
        .unwrap();
        assert!(native
            .locate(StandaloneTool::Cursor, &cancel)
            .unwrap()
            .is_none());
        // A binary named copilot that does not identify as Copilot is not adopted.
        temp.install(StandaloneTool::Copilot);
        temp.write(".local/bin/copilot.version", "some other tool 3.0.0");
        assert!(native
            .locate(StandaloneTool::Copilot, &cancel)
            .unwrap()
            .is_none());
        temp.write(
            ".local/bin/copilot.version",
            "GitHub Copilot CLI 1.0.88. Run 'copilot update' to check for updates.",
        );
        assert_eq!(
            native
                .locate(StandaloneTool::Copilot, &cancel)
                .unwrap()
                .unwrap()
                .version,
            Version::new(1, 0, 88)
        );
        // Homebrew and npm copies link from elsewhere and are not adopted.
        let npm = temp.script(
            "lib/node_modules/@github/copilot/copilot",
            "#!/bin/sh
",
        );
        fs::remove_file(temp.0.join(".local/bin/copilot")).unwrap();
        symlink(npm, temp.0.join(".local/bin/copilot")).unwrap();
        assert!(native
            .locate(StandaloneTool::Copilot, &cancel)
            .unwrap()
            .is_none());
    }
    #[test]
    fn blockchain_toolchains_follow_their_own_installers() {
        let cancel = Cancellation::default();
        // Agave: the channel comes from its config; a pinned release has no
        // newer one, and only Agave's own channels are followed.
        assert_eq!(
            solana_channel("explicit_release: !Channel beta\n").as_deref(),
            Some("beta")
        );
        assert_eq!(
            solana_channel("explicit_release: !Semver 2.2.21\n").as_deref(),
            Some("pinned")
        );
        assert_eq!(solana_channel("explicit_release: !Channel nightly\n"), None);
        assert_eq!(solana_channel("json_rpc_url: http://localhost\n"), None);
        let temp = Temp::new();
        let native = temp.native();
        temp.install(StandaloneTool::Solana);
        let config = temp.0.join(".config/solana/install/config.yml");
        fs::write(&config, "explicit_release: !Semver 1.0.0\n").unwrap();
        let pinned = native
            .locate(StandaloneTool::Solana, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(pinned.channel, "pinned");
        temp.network("never fetched");
        assert_eq!(
            native
                .latest(StandaloneTool::Solana, &pinned, &cancel)
                .unwrap(),
            Version::new(1, 0, 0)
        );
        assert!(!temp.0.join("request").exists());
        // A stable channel whose manifest names no commit is an error.
        let stable = Installation {
            channel: "stable".into(),
            ..pinned
        };
        for manifest in ["channel: stable\n", "commit: 44b42d4\n", "commit: zz\n"] {
            temp.write("channel", manifest);
            assert!(native
                .latest(StandaloneTool::Solana, &stable, &cancel)
                .unwrap_err()
                .to_string()
                .contains("no release commit"));
        }
        // Either download failing fails the check (the curl shim fails for
        // a missing response file).
        fs::remove_file(temp.0.join("channel")).unwrap();
        assert!(native
            .latest(StandaloneTool::Solana, &stable, &cancel)
            .is_err());
        temp.write(
            "channel",
            "commit: 44b42d45ec7e555b26ca15ad924a7432d18aaa9f\n",
        );
        fs::remove_file(temp.0.join("release")).unwrap();
        assert!(native
            .latest(StandaloneTool::Solana, &stable, &cancel)
            .is_err());
        assert!(fs::read_to_string(temp.0.join("request"))
            .unwrap()
            .ends_with("/44b42d45ec7e555b26ca15ad924a7432d18aaa9f/Cargo.toml"));
        fs::write(&config, "explicit_release: !Channel nightly\n").unwrap();
        assert!(native
            .locate(StandaloneTool::Solana, &cancel)
            .unwrap()
            .is_none());
        fs::remove_file(&config).unwrap();
        assert!(native
            .locate(StandaloneTool::Solana, &cancel)
            .unwrap()
            .is_none());
        // Another program answering as agave-install isn't Agave's.
        temp.write(
            ".config/solana/install/config.yml",
            "explicit_release: !Channel stable\n",
        );
        temp.write(
            ".local/share/solana/install/active_release/bin/agave-install.version",
            "something else 1.0.0",
        );
        assert!(native
            .locate(StandaloneTool::Solana, &cancel)
            .unwrap()
            .is_none());
        // AVM: the active Anchor must be one AVM installed.
        temp.install(StandaloneTool::Anchor);
        assert_eq!(
            native
                .locate(StandaloneTool::Anchor, &cancel)
                .unwrap()
                .unwrap()
                .version,
            Version::new(1, 0, 0)
        );
        temp.write(".avm/.version", "1.1.0\n");
        assert!(native
            .locate(StandaloneTool::Anchor, &cancel)
            .unwrap()
            .is_none());
        temp.write(".avm/.version", "../../bin/other");
        assert!(native.locate(StandaloneTool::Anchor, &cancel).is_err());
        fs::remove_file(temp.0.join(".avm/.version")).unwrap();
        assert!(native
            .locate(StandaloneTool::Anchor, &cancel)
            .unwrap()
            .is_none());
        // Foundry: forge counts only beside its own foundryup, and nightly
        // builds are not compared with stable releases.
        temp.install(StandaloneTool::Foundry);
        temp.write(".foundry/bin/forge.version", "forge Version: 1.4.0-nightly");
        let nightly = native
            .locate(StandaloneTool::Foundry, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(nightly.channel, "nightly");
        assert_eq!(
            native
                .latest(StandaloneTool::Foundry, &nightly, &cancel)
                .unwrap(),
            nightly.version
        );
        temp.write(".foundry/bin/forge.version", "cast Version: 1.4.0");
        assert!(native
            .locate(StandaloneTool::Foundry, &cancel)
            .unwrap()
            .is_none());
        temp.write(".foundry/bin/forge.version", "forge Version: 1.4.0");
        fs::remove_file(temp.0.join(".foundry/bin/foundryup")).unwrap();
        assert!(native
            .locate(StandaloneTool::Foundry, &cancel)
            .unwrap()
            .is_none());
    }
    #[test]
    fn date_versions_compare_by_date() {
        let cursor = version(StandaloneTool::Cursor, "2026.09.26-dd393fe").unwrap();
        assert_eq!((cursor.major, cursor.minor, cursor.patch), (2026, 9, 26));
        assert_eq!(cursor.build.as_str(), "dd393fe");
        assert!(version(StandaloneTool::Cursor, "2026.10.01-aaaaaaa")
            .unwrap()
            .cmp_precedence(&cursor)
            .is_gt());
        assert!(version(StandaloneTool::Cursor, "2026.09").is_err());
        assert!(version(StandaloneTool::Cursor, "2026.09.26.1").is_err());
        assert!(version(StandaloneTool::Cursor, "2026.09.26-bad!build").is_err());
        assert_eq!(
            version(StandaloneTool::Cursor, "2026.09.26").unwrap(),
            Version::new(2026, 9, 26)
        );
        assert_eq!(
            installed_version(StandaloneTool::Copilot, "GitHub Copilot CLI 1.0.88.").unwrap(),
            Version::new(1, 0, 88)
        );
    }
    #[test]
    fn release_links_name_a_tag_or_a_moved_repository() {
        assert!(matches!(
            release_link("https://github.com/foundry-rs/foundry/releases/tag/v1.8.4"),
            Some(ReleaseLink::Tag("v1.8.4"))
        ));
        assert!(matches!(
            release_link("https://github.com/otter-sec/anchor/releases/latest"),
            Some(ReleaseLink::Moved)
        ));
        for location in [
            "",
            "https://github.com/owner/repo/releases",
            "https://github.com/owner/repo/releases/tag/",
            "https://github.com/owner/repo/releases/tag/v1/extra",
            "https://github.com/owner/repo/tree/main",
            "https://github.com/../repo/releases/latest",
            "https://example.com/owner/repo/releases/tag/v1",
        ] {
            assert!(release_link(location).is_none(), "{location}");
        }
    }
    #[test]
    fn github_rate_limits_are_said_plainly() {
        let curl_failed = |stderr: &str| {
            EngineError::Execution(ExecutionError::Failed(Completion {
                code: Some(22),
                signal: None,
                stdout: vec![],
                stderr: stderr.as_bytes().to_vec(),
                truncated: false,
                cancellation_deferred: false,
            }))
        };
        let api = "https://github.com/foundry-rs/foundry/releases/latest";
        for status in [403, 429] {
            let error = github_rate_limit(
                StandaloneTool::Foundry,
                api,
                curl_failed(&format!(
                    "curl: (56) The requested URL returned error: {status}"
                )),
            );
            assert!(
                error.to_string().contains("limiting update checks"),
                "{error}"
            );
        }
        // Other failures, and other hosts, keep curl's own words.
        assert!(matches!(
            github_rate_limit(StandaloneTool::Foundry, api, EngineError::NotFound),
            EngineError::NotFound
        ));
        let not_found = curl_failed("curl: (22) The requested URL returned error: 404");
        assert!(matches!(
            github_rate_limit(StandaloneTool::Foundry, api, not_found),
            EngineError::Execution(ExecutionError::Failed(_))
        ));
        let elsewhere = curl_failed("curl: (22) The requested URL returned error: 403");
        assert!(matches!(
            github_rate_limit(
                StandaloneTool::Claude,
                "https://downloads.claude.ai/claude-code-releases/stable",
                elsewhere
            ),
            EngineError::Execution(ExecutionError::Failed(_))
        ));
    }

    #[test]
    fn network_checks_never_rewrite_the_curl_shim() {
        let temp = Temp::new();
        let _native = temp.native();
        let shim = temp.0.join("bin/curl");
        let before = fs::metadata(&shim).unwrap();
        let content = fs::read(&shim).unwrap();
        temp.network(r#"{"tag_name":"v2.0.0"}"#);
        temp.network("invalid");
        let after = fs::metadata(&shim).unwrap();
        assert_eq!(
            (before.ino(), before.mtime(), before.mtime_nsec()),
            (after.ino(), after.mtime(), after.mtime_nsec())
        );
        assert_eq!(content, fs::read(&shim).unwrap());
        assert_eq!(fs::read(temp.0.join("release")).unwrap(), b"invalid");
    }
    #[test]
    fn native_latest_checks_are_read_only_and_fail_on_bad_metadata() {
        let temp = Temp::new();
        let native = temp.native();
        let cancel = Cancellation::default();
        for tool in StandaloneTool::ALL {
            temp.install(tool);
            let installation = native.locate(tool, &cancel).unwrap().unwrap();
            // An Agave channel names a commit; its Cargo.toml has the release.
            temp.write(
                "channel",
                "channel: \ncommit: 44b42d45ec7e555b26ca15ad924a7432d18aaa9f\n",
            );
            temp.network(match tool {
                StandaloneTool::Solana => "[workspace]\nversion = \"9.0.0\"\n[workspace.package]\nversion = \"2.0.0\"\n",
                StandaloneTool::Claude | StandaloneTool::Amp => "2.0.0",
                StandaloneTool::Droid => "#!/bin/sh\nbinary_name=\"droid\"\nVER=\"2.0.0\"\n",
                StandaloneTool::Cursor => "DOWNLOAD_URL=\"https://downloads.cursor.com/lab/2.0.0/${OS}/${ARCH}/agent-cli-package.tar.gz\"",
                StandaloneTool::Kiro | StandaloneTool::Antigravity => r#"{"version":"2.0.0"}"#,
                StandaloneTool::OpenCode
                | StandaloneTool::Anchor
                | StandaloneTool::Foundry
                | StandaloneTool::Copilot => "https://github.com/owner/repo/releases/tag/v2.0.0",
                _ => r#"{"tag_name":"rust-v2.0.0"}"#,
            });
            assert_eq!(
                native.latest(tool, &installation, &cancel).unwrap(),
                Version::new(2, 0, 0)
            );
            assert!(!installation.launcher.with_extension("args").exists());
            if tool != StandaloneTool::Grok {
                temp.network("invalid");
                assert!(native.latest(tool, &installation, &cancel).is_err());
                temp.network(r#"{"tag_name":"v2.0.0","prerelease":true}"#);
                assert!(native.latest(tool, &installation, &cancel).is_err());
                temp.network("{}");
                assert!(native.latest(tool, &installation, &cancel).is_err());
                // A repository that keeps redirecting to a new name has no
                // release to offer.
                temp.network("https://github.com/owner/moved/releases/latest");
                assert!(native.latest(tool, &installation, &cancel).is_err());
            }
        }
        let grok = native
            .locate(StandaloneTool::Grok, &cancel)
            .unwrap()
            .unwrap();
        for text in [
            r#"{"installer":"npm"}"#,
            r#"{"installer":"internal","error":"offline"}"#,
            r#"{"installer":"internal"}"#,
            r#"{"installer":"internal","updateAvailable":true}"#,
        ] {
            temp.write(".grok/bin/grok.check.json", text);
            assert!(native.latest(StandaloneTool::Grok, &grok, &cancel).is_err());
        }
        temp.write(
            ".grok/bin/grok.check.json",
            r#"{"installer":"internal","updateAvailable":false}"#,
        );
        assert_eq!(
            native.latest(StandaloneTool::Grok, &grok, &cancel).unwrap(),
            Version::new(1, 0, 0)
        );
    }
    #[test]
    fn native_updaters_target_the_selected_installation_and_private_installer() {
        let temp = Temp::new();
        let native = temp.native();
        let cancel = Cancellation::default();
        for tool in StandaloneTool::ALL {
            let launcher = temp.install(tool);
            let installation = native.locate(tool, &cancel).unwrap().unwrap();
            temp.network(r#"{"tag_name":"v2.0.0"}"#);
            temp.write("installer", "#!/bin/sh\n[ \"$CODEX_NON_INTERACTIVE\" = 1 ] || exit 8\n[ \"$1\" = --release ] || exit 9\nprintf '%s' \"$2\" > \"$CODEX_INSTALL_DIR/codex.version\"\n");
            let result = native.update(tool, &installation, &Version::new(2, 0, 0), &cancel);
            // Updaters never run as root.
            assert_eq!(result.is_err(), rustix::process::geteuid().is_root());
            let Ok(result) = result else { continue };
            assert_eq!(result.code, Some(0));
            // AVM and foundryup switch the release the launcher runs; the
            // fixture only records their arguments, so switch as they would.
            match tool {
                StandaloneTool::Anchor => {
                    temp.write(".avm/.version", "2.0.0");
                    fs::hard_link(&launcher, temp.0.join(".avm/bin/anchor-2.0.0")).unwrap();
                }
                StandaloneTool::Foundry => {
                    temp.write(".foundry/bin/forge.version", "forge Version: 2.0.0-stable");
                }
                _ => {}
            }
            assert_eq!(
                native.locate(tool, &cancel).unwrap().unwrap().version,
                Version::new(2, 0, 0)
            );
            if tool != StandaloneTool::Codex {
                let args = fs::read_to_string(installation.updater.with_extension("args")).unwrap();
                match tool {
                    StandaloneTool::OpenCode => {
                        assert_eq!(args, "upgrade\n2.0.0\n--method\ncurl\n")
                    }
                    StandaloneTool::Anchor => assert_eq!(args, "install\n2.0.0\n"),
                    StandaloneTool::Foundry => assert_eq!(args, "--install\nstable\n"),
                    _ => assert!(args.starts_with("update\n")),
                }
            }
            assert_eq!(
                native
                    .update(tool, &installation, &Version::new(2, 0, 0), &cancel)
                    .unwrap()
                    .code,
                Some(0)
            );
        }
        let file = InstallerFile::create(b"synthetic").unwrap();
        let root = file.0.clone();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        drop(file);
        assert!(!root.exists());
    }
    #[test]
    fn versions_are_asked_once_per_binary() {
        let temp = Temp::new();
        let cancel = Cancellation::default();
        let store = || Some(crate::cache::Store::new(temp.0.join("cache")));
        let remembering = |store| NativeStandalone {
            versions: Some(Versions::new(store)),
            ..temp.native()
        };
        // Updaters replace the binary with a new file, as these do.
        let release = |path: &str, body: &str| {
            let fresh = temp.script(
                "fresh",
                &format!("#!/bin/sh\necho run >> \"$HOME/runs\"\n{body}\n"),
            );
            fs::rename(fresh, temp.0.join(path)).unwrap();
        };
        let runs = || {
            fs::read_to_string(temp.0.join("runs"))
                .unwrap_or_default()
                .lines()
                .count()
        };
        let tool = StandaloneTool::OpenCode;
        let launcher = temp.0.join(".opencode/bin/opencode");
        fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        release(".opencode/bin/opencode", "echo 1.2.3");
        let version = |native: &NativeStandalone| native.version_at(tool, &launcher, &cancel);

        let native = remembering(store());
        assert_eq!(version(&native).unwrap(), Some(Version::new(1, 2, 3)));
        assert_eq!(version(&native).unwrap(), Some(Version::new(1, 2, 3)));
        assert_eq!(runs(), 1);
        // A later run, such as the next `pkd` command, reads it back.
        assert_eq!(
            version(&remembering(store())).unwrap(),
            Some(Version::new(1, 2, 3))
        );
        assert_eq!(runs(), 1);
        // A new binary is asked again.
        release(".opencode/bin/opencode", "echo 1.2.4");
        assert_eq!(version(&native).unwrap(), Some(Version::new(1, 2, 4)));
        assert_eq!(
            version(&remembering(store())).unwrap(),
            Some(Version::new(1, 2, 4))
        );
        assert_eq!(runs(), 2);
        // A failed answer is never kept.
        release(".opencode/bin/opencode", "echo 1.2.5; exit 3");
        assert!(version(&native).is_err());
        assert!(version(&native).is_err());
        assert!(version(&remembering(store())).is_err());
        assert_eq!(runs(), 5);
        // Without a cache folder, one backend still asks once.
        release(".opencode/bin/opencode", "echo 1.2.6");
        let uncached = remembering(None);
        assert_eq!(version(&uncached).unwrap(), Some(Version::new(1, 2, 6)));
        assert_eq!(version(&uncached).unwrap(), Some(Version::new(1, 2, 6)));
        assert_eq!(runs(), 6);
        assert_eq!(
            version(&remembering(None)).unwrap(),
            Some(Version::new(1, 2, 6))
        );
        assert_eq!(runs(), 7);
        // Nor is a binary that can't run.
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(version(&native).is_err());
        assert_eq!(runs(), 7);

        // A launcher link follows the binary it points to.
        let claude = temp.0.join(".local/bin/claude");
        fs::create_dir_all(claude.parent().unwrap()).unwrap();
        fs::create_dir_all(temp.0.join(".local/share/claude/versions")).unwrap();
        release(".local/share/claude/versions/1.0.0", "echo 1.0.0");
        symlink(temp.0.join(".local/share/claude/versions/1.0.0"), &claude).unwrap();
        let claude_version =
            |native: &NativeStandalone| native.version_at(StandaloneTool::Claude, &claude, &cancel);
        assert_eq!(
            claude_version(&native).unwrap(),
            Some(Version::new(1, 0, 0))
        );
        assert_eq!(
            claude_version(&native).unwrap(),
            Some(Version::new(1, 0, 0))
        );
        assert_eq!(runs(), 8);
        release(".local/share/claude/versions/1.0.0", "echo 1.0.1");
        assert_eq!(
            claude_version(&native).unwrap(),
            Some(Version::new(1, 0, 1))
        );
        assert_eq!(runs(), 9);
    }
    #[test]
    fn updates_and_removals_forget_remembered_versions() {
        let temp = Temp::new();
        let cancel = Cancellation::default();
        let native = NativeStandalone {
            versions: Some(Versions::new(Some(crate::cache::Store::new(
                temp.0.join("cache"),
            )))),
            ..temp.native()
        };
        let tool = StandaloneTool::OpenCode;
        temp.install(tool);
        let installation = native.locate(tool, &cancel).unwrap().unwrap();
        assert_eq!(installation.version, Version::new(1, 0, 0));
        // The fixture's updater changes only what it answers, not itself.
        let result = native.update(tool, &installation, &Version::new(2, 0, 0), &cancel);
        // Updaters and removal never run as root; the checks below need them to.
        assert_eq!(result.is_err(), rustix::process::geteuid().is_root());
        let Ok(result) = result else { return };
        assert_eq!(result.code, Some(0));
        let updated = native.locate(tool, &cancel).unwrap().unwrap();
        assert_eq!(updated.version, Version::new(2, 0, 0));
        assert_eq!(
            native.remove(tool, &updated, &cancel).unwrap().code,
            Some(0)
        );
        assert!(native.locate(tool, &cancel).unwrap().is_none());
    }
    #[test]
    fn native_rejects_sandbox_missing_home_and_invalid_paths() {
        let native = NativeStandalone {
            host: Host::new(Runtime::Flatpak, BTreeMap::new()),
            trash: None,
            versions: None,
        };
        assert!(native
            .locate(StandaloneTool::Codex, &Cancellation::default())
            .is_err());
        let native = NativeStandalone {
            host: Host::new(Runtime::Native, BTreeMap::new()),
            trash: None,
            versions: None,
        };
        assert!(native.home().is_err());
        let native = NativeStandalone {
            host: Host::new(
                Runtime::Native,
                BTreeMap::from([("CODEX_HOME".into(), "relative".into())]),
            ),
            trash: None,
            versions: None,
        };
        assert!(native
            .setting_path("CODEX_HOME", "/fallback".into())
            .is_err());
    }
    #[test]
    fn native_discovery_refuses_unreadable_settings_and_foreign_layouts() {
        let cancel = Cancellation::default();
        // Each check refuses before anything runs, so a bare Mach-O header
        // stands in for a tool.
        let binary = |temp: &Temp, path: &str| {
            let path = temp.write(path, [0xcf, 0xfa, 0xed, 0xfe]);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        // Claude's settings must be a readable, bounded UTF-8 file.
        let temp = Temp::new();
        let native = temp.native();
        let claude = binary(&temp, ".local/share/claude/versions/1.0.0");
        fs::create_dir_all(temp.0.join(".local/bin")).unwrap();
        symlink(&claude, temp.0.join(".local/bin/claude")).unwrap();
        let settings = temp.write(".claude/settings.json", vec![b' '; 1024 * 1024 + 1]);
        assert!(native
            .locate(StandaloneTool::Claude, &cancel)
            .unwrap_err()
            .to_string()
            .contains("too large"));
        fs::write(&settings, b"\xff").unwrap();
        assert!(native.locate(StandaloneTool::Claude, &cancel).is_err());
        fs::remove_dir_all(temp.0.join(".claude")).unwrap();
        temp.write(".claude", "not a folder");
        assert!(native.locate(StandaloneTool::Claude, &cancel).is_err());
        // Codex needs its release manifest, and only its own.
        let codex = binary(&temp, ".codex/packages/standalone/releases/1.0.0/bin/codex");
        symlink(&codex, temp.0.join(".local/bin/codex")).unwrap();
        let manifest = temp
            .0
            .join(".codex/packages/standalone/releases/1.0.0/codex-package.json");
        assert!(native
            .locate(StandaloneTool::Codex, &cancel)
            .unwrap()
            .is_none());
        for text in [
            r#"{"layoutVersion":1,"entrypoint":"bin/codex","variant":"other"}"#,
            r#"{"layoutVersion":2,"entrypoint":"bin/codex","variant":"codex"}"#,
        ] {
            fs::write(&manifest, text).unwrap();
            assert!(native
                .locate(StandaloneTool::Codex, &cancel)
                .unwrap()
                .is_none());
        }
        fs::write(&manifest, "not json").unwrap();
        assert!(native.locate(StandaloneTool::Codex, &cancel).is_err());
        // A private folder that really lives in an npm tree is not adopted.
        binary(&temp, "lib/node_modules/amp/bin/amp");
        symlink(temp.0.join("lib/node_modules/amp"), temp.0.join(".amp")).unwrap();
        assert!(native
            .locate(StandaloneTool::Amp, &cancel)
            .unwrap()
            .is_none());
        // Only an executable native binary counts.
        temp.write(".local/bin/droid", [0xcf, 0xfa, 0xed, 0xfe]);
        assert!(native
            .locate(StandaloneTool::Droid, &cancel)
            .unwrap()
            .is_none());
        fs::remove_file(temp.0.join(".local/bin/droid")).unwrap();
        temp.script(".local/bin/droid", "ab");
        assert!(native
            .locate(StandaloneTool::Droid, &cancel)
            .unwrap()
            .is_none());
        // A path through a file is an error, not a missing tool.
        let temp = Temp::new();
        let native = temp.native();
        temp.write(".opencode", "not a folder");
        assert!(native.locate(StandaloneTool::OpenCode, &cancel).is_err());
        temp.script(".local/bin/claude", "#!/bin/sh\n");
        temp.write(".local/share", "not a folder");
        assert!(native.locate(StandaloneTool::Claude, &cancel).is_err());
    }

    #[test]
    fn native_checks_and_updates_fail_without_running_anything_unexpected() {
        let temp = Temp::new();
        let native = temp.native();
        let cancel = Cancellation::default();
        let cancelled = Cancellation::default();
        cancelled.cancel();
        temp.install(StandaloneTool::Amp);
        let amp = native
            .locate(StandaloneTool::Amp, &cancel)
            .unwrap()
            .unwrap();
        assert!(matches!(
            native.latest(StandaloneTool::Amp, &amp, &cancelled),
            Err(EngineError::Cancelled)
        ));
        temp.install(StandaloneTool::Grok);
        let grok = native
            .locate(StandaloneTool::Grok, &cancel)
            .unwrap()
            .unwrap();
        assert!(matches!(
            native.latest(StandaloneTool::Grok, &grok, &cancelled),
            Err(EngineError::Cancelled)
        ));
        // Another copy than the one checked is never updated.
        let moved = Installation {
            launcher: temp.0.join("elsewhere/amp"),
            ..amp.clone()
        };
        assert!(matches!(
            native.update(StandaloneTool::Amp, &moved, &Version::new(2, 0, 0), &cancel),
            Err(EngineError::NotFound)
        ));
        assert!(!temp.0.join(".amp/bin/amp.args").exists());
        // Without curl there is no update check.
        let offline = NativeStandalone {
            host: Host::new(
                Runtime::Native,
                BTreeMap::from([
                    ("HOME".into(), temp.0.as_os_str().into()),
                    ("PATH".into(), temp.0.join("empty").into_os_string()),
                ]),
            ),
            trash: None,
            versions: None,
        };
        assert!(matches!(
            offline.latest(StandaloneTool::Amp, &amp, &cancel),
            Err(EngineError::Execution(ExecutionError::Disabled(_)))
        ));
        // An installer shell that can't start fails the Codex update.
        let temp = Temp::new();
        let native = temp.native();
        temp.install(StandaloneTool::Codex);
        let codex = native
            .locate(StandaloneTool::Codex, &cancel)
            .unwrap()
            .unwrap();
        temp.write("installer", "#!/bin/sh\nexit 0\n");
        temp.script("bin/sh", "#!/nonexistent/interpreter\n");
        assert!(native
            .update(
                StandaloneTool::Codex,
                &codex,
                &Version::new(2, 0, 0),
                &cancel
            )
            .is_err());
        assert_eq!(
            native
                .locate(StandaloneTool::Codex, &cancel)
                .unwrap()
                .unwrap()
                .version,
            Version::new(1, 0, 0)
        );
    }
    #[test]
    fn standalone_removal_confirms_the_tool_is_gone() {
        let cancel = Cancellation::default();
        let mut fixture = Fixture::new();
        fixture.change = false;
        let mut backend = Standalone {
            tool: StandaloneTool::Claude,
            io: Box::new(fixture.clone()),
        };
        let id = backend.installed(&cancel).unwrap()[0].id.clone();
        // Only upgrades and removals are offered.
        assert!(backend
            .execute(&Operation::Install(id.clone()), &cancel, &mut |_| {})
            .is_err());
        assert!(backend
            .execute(&Operation::Remove(id.clone()), &cancel, &mut |_| {})
            .unwrap_err()
            .to_string()
            .contains("still installed"));
        // A cancellation after the checks stops before anything is removed.
        let writes = *fixture.writes.lock().unwrap();
        fixture.cancel_on_locate = true;
        backend.io = Box::new(fixture.clone());
        assert!(matches!(
            backend.execute(
                &Operation::Remove(id.clone()),
                &Cancellation::default(),
                &mut drop::<Progress>
            ),
            Err(EngineError::Cancelled)
        ));
        assert_eq!(*fixture.writes.lock().unwrap(), writes);
        // A removal finishing after a cancellation says so.
        fixture.cancel_on_locate = false;
        fixture.change = true;
        fixture.deferred = true;
        backend.io = Box::new(fixture.clone());
        assert!(
            backend
                .execute(&Operation::Remove(id.clone()), &cancel, &mut |_| {})
                .unwrap()
                .cancellation_deferred
        );
        assert!(matches!(
            backend.execute(&Operation::Remove(id), &cancel, &mut |_| {}),
            Err(EngineError::NotFound)
        ));
    }
    fn exists(path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok()
    }
    #[test]
    fn native_removal_takes_only_what_each_installer_put_there() {
        let cancel = Cancellation::default();
        for trash in [false, true] {
            for tool in StandaloneTool::ALL {
                let temp = Temp::new();
                let native = if trash {
                    temp.trashing()
                } else {
                    temp.native()
                };
                let launcher = temp.install(tool);
                let link = |target: &str, link: &str| {
                    let link = temp.0.join(link);
                    fs::create_dir_all(link.parent().unwrap()).unwrap();
                    symlink(temp.0.join(target), link).unwrap();
                };
                // What else each installer leaves, and what's the user's.
                let (removed, kept): (&[&str], &[&str]) = match tool {
                    StandaloneTool::Codex => {
                        link(
                            ".codex/packages/standalone/releases/1.0.0-test/bin/codex",
                            ".local/bin/codex-code-mode-host",
                        );
                        (
                            &[
                                ".local/bin/codex",
                                ".local/bin/codex-code-mode-host",
                                ".codex/packages",
                            ],
                            &[".codex/config.toml", ".codex/auth.json"],
                        )
                    }
                    StandaloneTool::Claude => (
                        &[".local/bin/claude", ".local/share/claude"],
                        &[".claude/settings.json", ".claude.json"],
                    ),
                    StandaloneTool::Grok => {
                        link(".grok/downloads/grok-linux-test", ".grok/bin/agent");
                        link(".grok/bin/grok", ".local/bin/grok");
                        link(".grok/bin/agent", ".local/bin/agent");
                        (
                            &[
                                ".grok/bin",
                                ".grok/downloads",
                                ".local/bin/grok",
                                ".local/bin/agent",
                            ],
                            &[".grok/auth.json"],
                        )
                    }
                    StandaloneTool::OpenCode => (&[".opencode/bin"], &[".opencode/package.json"]),
                    StandaloneTool::Cursor => {
                        link(
                            ".local/share/cursor-agent/versions/1.0.0/cursor-agent",
                            ".local/bin/cursor-agent",
                        );
                        (
                            &[
                                ".local/bin/agent",
                                ".local/bin/cursor-agent",
                                ".local/share/cursor-agent",
                            ],
                            &[".cursor/cli-config.json"],
                        )
                    }
                    StandaloneTool::Copilot => (&[".local/bin/copilot"], &[".copilot/config.json"]),
                    StandaloneTool::Kiro => {
                        temp.script(".local/bin/kiro-cli-chat", "#!/bin/sh\n");
                        (
                            &[".local/bin/kiro-cli", ".local/bin/kiro-cli-chat"],
                            &[".kiro/settings/cli.json"],
                        )
                    }
                    StandaloneTool::Antigravity => {
                        (&[".local/bin/agy"], &[".gemini/settings.json"])
                    }
                    StandaloneTool::Amp => {
                        link(".amp/bin/amp", ".local/bin/amp");
                        temp.write(".amp/amp-install-version.txt", "1.0.0");
                        temp.write(".amp/signing-key.pub", "key");
                        (&[".amp", ".local/bin/amp"], &[".config/amp/settings.json"])
                    }
                    StandaloneTool::Droid => (&[".local/bin/droid"], &[".factory/settings.json"]),
                    StandaloneTool::Solana => (
                        &[".local/share/solana"],
                        &[
                            ".config/solana/id.json",
                            ".config/solana/cli/config.yml",
                            ".config/solana/install/config.yml",
                        ],
                    ),
                    StandaloneTool::Anchor => {
                        link(".avm/bin/avm", ".avm/bin/anchor");
                        temp.write(".avm/.crates.toml", "[v1]\n");
                        (&[".avm"], &[".config/solana/id.json"])
                    }
                    StandaloneTool::Foundry => {
                        temp.write(".foundry/share/man/man1/forge.1", "manual");
                        (
                            &[".foundry/bin", ".foundry/versions", ".foundry/share"],
                            &[".foundry/keystores/dev", ".foundry/cache/rpc.json"],
                        )
                    }
                };
                let kept: Vec<_> = kept.iter().chain(&[".local/bin/unrelated"]).collect();
                for path in &kept {
                    if !temp.0.join(path).exists() {
                        // Settings are JSON, which Claude Code discovery reads.
                        temp.write(path, "{}");
                    }
                }
                let installation = native.locate(tool, &cancel).unwrap().unwrap();
                assert_eq!(installation.launcher, launcher);
                let result = native.remove(tool, &installation, &cancel);
                // Removal never runs as root.
                assert_eq!(result.is_err(), rustix::process::geteuid().is_root());
                let Ok(result) = result else { continue };
                assert_eq!(result.code, Some(0));
                assert!(native.locate(tool, &cancel).unwrap().is_none());
                for path in removed {
                    assert!(!exists(&temp.0.join(path)), "{tool:?} left {path}");
                }
                for path in &kept {
                    assert!(temp.0.join(path).is_file(), "{tool:?} removed {path}");
                }
                // The Trash gets each item once, all inside this home.
                let trashed = fs::read_to_string(temp.0.join("trashed")).unwrap_or_default();
                let mut lines: Vec<_> = trashed.lines().collect();
                assert_eq!(lines.is_empty(), !trash);
                assert!(lines
                    .iter()
                    .all(|line| Path::new(line).starts_with(&temp.0)));
                let count = lines.len();
                lines.sort();
                lines.dedup();
                assert_eq!(lines.len(), count);
            }
        }
    }
    #[test]
    fn native_removal_refuses_what_isnt_the_tools_own() {
        let cancel = Cancellation::default();
        // Cursor leaves the `agent` link Grok's installer made.
        let temp = Temp::new();
        let native = temp.native();
        let grok = temp.install(StandaloneTool::Grok);
        temp.install(StandaloneTool::Cursor);
        let agent = temp.0.join(".local/bin/agent");
        fs::remove_file(&agent).unwrap();
        symlink(&grok, &agent).unwrap();
        symlink(
            temp.0
                .join(".local/share/cursor-agent/versions/1.0.0/cursor-agent"),
            temp.0.join(".local/bin/cursor-agent"),
        )
        .unwrap();
        let cursor = native
            .locate(StandaloneTool::Cursor, &cancel)
            .unwrap()
            .unwrap();
        let result = native.remove(StandaloneTool::Cursor, &cursor, &cancel);
        // Removal never runs as root; the checks below need it to.
        assert_eq!(result.is_err(), rustix::process::geteuid().is_root());
        let Ok(_) = result else { return };
        assert!(native
            .locate(StandaloneTool::Cursor, &cancel)
            .unwrap()
            .is_none());
        assert!(exists(&agent));
        let grok = native
            .locate(StandaloneTool::Grok, &cancel)
            .unwrap()
            .unwrap();
        // Another copy than the one checked is never removed, and a
        // cancelled removal removes nothing.
        let moved = Installation {
            launcher: temp.0.join("elsewhere/grok"),
            ..grok.clone()
        };
        assert!(matches!(
            native.remove(StandaloneTool::Grok, &moved, &cancel),
            Err(EngineError::NotFound)
        ));
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(matches!(
            native.remove(StandaloneTool::Grok, &grok, &cancelled),
            Err(EngineError::Cancelled)
        ));
        assert!(native
            .locate(StandaloneTool::Grok, &cancel)
            .unwrap()
            .is_some());
        // A tool folder that links somewhere else is left alone.
        let temp = Temp::new();
        let native = temp.native();
        let elsewhere = temp.0.join("elsewhere/bin");
        fs::create_dir_all(&elsewhere).unwrap();
        temp.install(StandaloneTool::OpenCode);
        fs::rename(
            temp.0.join(".opencode/bin/opencode"),
            elsewhere.join("opencode"),
        )
        .unwrap();
        fs::remove_dir(temp.0.join(".opencode/bin")).unwrap();
        symlink(&elsewhere, temp.0.join(".opencode/bin")).unwrap();
        let opencode = native
            .locate(StandaloneTool::OpenCode, &cancel)
            .unwrap()
            .unwrap();
        assert!(native
            .remove(StandaloneTool::OpenCode, &opencode, &cancel)
            .unwrap_err()
            .to_string()
            .contains("won't remove"));
        assert!(elsewhere.join("opencode").is_file());
        // Nor is a folder holding the home folder, whatever FOUNDRY_DIR says.
        let temp = Temp::new();
        temp.install(StandaloneTool::Foundry);
        let foundry = temp.0.join(".foundry");
        let native = NativeStandalone {
            host: Host::new(
                Runtime::Native,
                BTreeMap::from([
                    ("HOME".into(), foundry.join("versions").into_os_string()),
                    ("FOUNDRY_DIR".into(), foundry.as_os_str().into()),
                    ("PATH".into(), "/usr/bin:/bin".into()),
                ]),
            ),
            trash: None,
            versions: None,
        };
        let installation = native
            .locate(StandaloneTool::Foundry, &cancel)
            .unwrap()
            .unwrap();
        assert!(native
            .remove(StandaloneTool::Foundry, &installation, &cancel)
            .unwrap_err()
            .to_string()
            .contains("won't remove"));
        assert!(foundry.join("bin/forge").exists());
        // A path through a file is an error, not a missing folder.
        let temp = Temp::new();
        let native = temp.native();
        temp.install(StandaloneTool::Foundry);
        temp.write(".foundry/share", "not a folder");
        let installation = native
            .locate(StandaloneTool::Foundry, &cancel)
            .unwrap()
            .unwrap();
        assert!(native
            .remove(StandaloneTool::Foundry, &installation, &cancel)
            .is_err());
        assert!(temp.0.join(".foundry/bin/forge").exists());
    }
    #[test]
    fn native_removal_reports_failures_and_removes_each_link_once() {
        // Removal never runs as root, so neither does this test.
        let user = crate::host::refuse_root(true);
        let Ok(()) = user else { return };
        let cancel = Cancellation::default();
        // A Trash that fails leaves everything in place.
        let temp = Temp::new();
        let native = temp.trashing();
        temp.install(StandaloneTool::Solana);
        temp.write("trash-fails", "");
        let solana = native
            .locate(StandaloneTool::Solana, &cancel)
            .unwrap()
            .unwrap();
        let result = native
            .remove(StandaloneTool::Solana, &solana, &cancel)
            .unwrap();
        assert_eq!(result.code, Some(5));
        assert!(native
            .locate(StandaloneTool::Solana, &cancel)
            .unwrap()
            .is_some());
        // A deletion that fails is reported, and the launcher still works.
        let temp = Temp::new();
        let native = temp.native();
        temp.install(StandaloneTool::Claude);
        let versions = temp.0.join(".local/share/claude/versions");
        fs::set_permissions(&versions, fs::Permissions::from_mode(0o555)).unwrap();
        let claude = native
            .locate(StandaloneTool::Claude, &cancel)
            .unwrap()
            .unwrap();
        let result = native.remove(StandaloneTool::Claude, &claude, &cancel);
        fs::set_permissions(&versions, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err());
        assert!(native
            .locate(StandaloneTool::Claude, &cancel)
            .unwrap()
            .is_some());
        // With GROK_BIN_DIR at ~/.local/bin each link is removed once.
        let temp = Temp::new();
        let bin = temp.0.join(".local/bin");
        let trash = temp.trashing().trash;
        let native = NativeStandalone {
            host: Host::new(
                Runtime::Native,
                BTreeMap::from([
                    ("HOME".into(), temp.0.as_os_str().into()),
                    ("GROK_BIN_DIR".into(), bin.as_os_str().into()),
                    ("PATH".into(), "/usr/bin:/bin".into()),
                ]),
            ),
            trash,
            versions: None,
        };
        temp.install(StandaloneTool::Grok);
        let binary = temp.0.join(".grok/downloads/grok-linux-test");
        fs::create_dir_all(&bin).unwrap();
        symlink(&binary, bin.join("grok")).unwrap();
        symlink(&binary, bin.join("agent")).unwrap();
        let grok = native
            .locate(StandaloneTool::Grok, &cancel)
            .unwrap()
            .unwrap();
        native.remove(StandaloneTool::Grok, &grok, &cancel).unwrap();
        assert_eq!(
            fs::read_to_string(temp.0.join("trashed")).unwrap(),
            format!(
                "{}\n{}\n{}\n",
                temp.0.join(".grok/downloads").display(),
                bin.join("grok").display(),
                bin.join("agent").display(),
            )
        );
    }
}
