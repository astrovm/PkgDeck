//! Updates for upstream-owned CLI installations. Discovery never installs tools.
use super::*;
use semver::Version;
use serde_json::Value;
use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandaloneTool {
    Codex,
    Claude,
    Grok,
    OpenCode,
}
impl StandaloneTool {
    pub const ALL: [Self; 4] = [Self::Codex, Self::Claude, Self::Grok, Self::OpenCode];
    pub fn id(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Grok => "grok",
            Self::OpenCode => "opencode",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Grok => "Grok",
            Self::OpenCode => "OpenCode",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Installation {
    launcher: PathBuf,
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
}
struct NativeStandalone {
    host: Host,
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
            }),
        }
    }
}
fn invalid_data(tool: StandaloneTool, reason: impl std::fmt::Display) -> EngineError {
    invalid(tool.id(), reason)
}
fn version(tool: StandaloneTool, text: &str) -> Result<Version, EngineError> {
    Version::parse(
        text.trim()
            .trim_start_matches("rust-v")
            .trim_start_matches('v'),
    )
    .map_err(|error| invalid_data(tool, format!("invalid version: {error}")))
}
fn installed_version(tool: StandaloneTool, text: &str) -> Result<Version, EngineError> {
    text.split_whitespace()
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
        }
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
        let Operation::Upgrade(id) = operation else {
            return Err(self.unsupported(operation.capability()));
        };
        let installation = self.installation(cancel)?;
        if self.package(&installation, None).id != *id {
            return Err(EngineError::NotFound);
        }
        let candidate = self.io.latest(self.tool, &installation, cancel)?;
        if !candidate.cmp_precedence(&installation.version).is_gt() {
            return Ok(OperationOutcome::default());
        }
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        progress(Progress::Message(format!(
            "Updating {} {} → {} using its standalone updater",
            self.tool.name(),
            installation.version,
            candidate
        )));
        let completion = self
            .io
            .update(self.tool, &installation, &candidate, cancel)?;
        let deferred = completion.cancellation_deferred;
        let output = bytes(self.id(), completion)?;
        if !output.is_empty() {
            progress(Progress::Message(String::from_utf8_lossy(&output).into()));
        }
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
        let curl = self.host.resolve("curl")?.ok_or_else(|| {
            ExecutionError::Disabled("curl is required to check standalone updates".into())
        })?;
        let args = [
            "-q",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "5",
            "--max-time",
            "15",
            "--max-filesize",
            "1048576",
            "--user-agent",
            "PkgDeck",
            url,
        ]
        .map(Into::into);
        let result = self.host.read(
            &curl,
            &args,
            Limits {
                timeout: Duration::from_secs(20),
                output_bytes: 1024 * 1024,
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
    ) -> Result<Version, EngineError> {
        let output = self
            .host
            .read(launcher, &["--version".into()], Limits::default(), cancel)?;
        installed_version(tool, &String::from_utf8_lossy(&bytes(tool.id(), output)?))
    }
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
        // Enforce the same runtime boundary even though discovery starts with files.
        if let Some(reason) = self.host.runtime.disabled_reason() {
            return Err(ExecutionError::Disabled(reason.into()).into());
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
        };
        let Some(binary) = native_binary(&launcher, &root)? else {
            return Ok(None);
        };
        if tool == StandaloneTool::Codex {
            let Some(manifest) = binary
                .parent()
                .and_then(Path::parent)
                .map(|p| p.join("codex-package.json"))
            else {
                return Ok(None);
            };
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
                PathBuf::from("/etc/claude-code/managed-settings.json"),
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
        Ok(Some(Installation {
            version: self.version_at(tool, &launcher, cancel)?,
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
        let url = match tool {
            StandaloneTool::Codex => "https://releases.openai.com/codex/channels/latest".into(),
            StandaloneTool::Claude => format!(
                "https://downloads.claude.ai/claude-code-releases/{}",
                installation.channel
            ),
            StandaloneTool::OpenCode => {
                "https://api.github.com/repos/anomalyco/opencode/releases/latest".into()
            }
            StandaloneTool::Grok => unreachable!(),
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
        if tool == StandaloneTool::Codex {
            // Download to private storage, then run the official pinned-version
            // installer. No network bytes are piped into a shell.
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
            return Ok(self.host.standalone_write(
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
            )?);
        }
        let args: Vec<OsString> = match tool {
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
            StandaloneTool::Codex => unreachable!(),
        };
        Ok(self
            .host
            .standalone_write(&installation.launcher, &args, &[], cancel)?)
    }
}

/// A standalone install is an owned native binary in the upstream's private
/// storage. npm scripts and Homebrew/package-manager symlinks are not adopted.
fn native_binary(launcher: &Path, root: &Path) -> Result<Option<PathBuf>, EngineError> {
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
    if !binary.starts_with(&root)
        || root.components().any(|p| {
            matches!(
                p.as_os_str().to_str(),
                Some("node_modules" | "Cellar" | "Caskroom")
            )
        })
    {
        return Ok(None);
    }
    let metadata = fs::metadata(&binary).map_err(ExecutionError::from)?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Ok(None);
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
            self.script("bin/curl", "#!/bin/sh\nfor arg do url=$arg; done\nprintf '%s' \"$url\" > \"$HOME/request\"\ncase \"$url\" in */install.sh) cat \"$HOME/installer\";; *) cat \"$HOME/release\";; esac\n");
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
    }
    impl Fixture {
        fn new() -> Self {
            Self {
                installed: Arc::new(Mutex::new(Some(Installation {
                    launcher: "/synthetic/bin/tool".into(),
                    version: Version::new(1, 0, 0),
                    channel: "latest".into(),
                }))),
                latest: Ok(Version::new(2, 0, 0)),
                writes: Arc::default(),
                change: true,
                deferred: false,
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
            Ok(self.installed.lock().unwrap().clone())
        }
        fn latest(
            &self,
            _: StandaloneTool,
            _: &Installation,
            _: &Cancellation,
        ) -> Result<Version, EngineError> {
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
                .execute(&Operation::Upgrade(foreign), &cancel, &mut |_| {})
                .is_err());
            assert!(backend
                .execute(&Operation::Remove(package.id.clone()), &cancel, &mut |_| {})
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
                .execute(&Operation::Upgrade(package.id), &cancel, &mut |_| {})
                .unwrap();
            assert_eq!(*fixture.writes.lock().unwrap(), 1);
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
            temp.network(if tool == StandaloneTool::Claude {
                "2.0.0"
            } else {
                r#"{"tag_name":"rust-v2.0.0"}"#
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
            if rustix::process::geteuid().is_root() {
                assert!(result.is_err());
                continue;
            }
            assert_eq!(result.unwrap().code, Some(0));
            assert_eq!(
                native.locate(tool, &cancel).unwrap().unwrap().version,
                Version::new(2, 0, 0)
            );
            if tool != StandaloneTool::Codex {
                let args = fs::read_to_string(launcher.with_extension("args")).unwrap();
                assert!(args.starts_with(if tool == StandaloneTool::OpenCode {
                    "upgrade\n"
                } else {
                    "update\n"
                }));
                if tool == StandaloneTool::OpenCode {
                    assert_eq!(args, "upgrade\n2.0.0\n--method\ncurl\n");
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
    fn native_rejects_sandbox_missing_home_and_invalid_paths() {
        let native = NativeStandalone {
            host: Host::new(Runtime::Flatpak, BTreeMap::new()),
        };
        assert!(native
            .locate(StandaloneTool::Codex, &Cancellation::default())
            .is_err());
        let native = NativeStandalone {
            host: Host::new(Runtime::Native, BTreeMap::new()),
        };
        assert!(native.home().is_err());
        let native = NativeStandalone {
            host: Host::new(
                Runtime::Native,
                BTreeMap::from([("CODEX_HOME".into(), "relative".into())]),
            ),
        };
        assert!(native
            .setting_path("CODEX_HOME", "/fallback".into())
            .is_err());
    }
}
