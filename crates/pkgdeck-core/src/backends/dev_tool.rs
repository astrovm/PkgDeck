//! Language package managers: Cargo, npm, pnpm, Bun, pip, pipx, uv, mise, Composer and RubyGems.
use super::*;

/// Capabilities shared by user-scoped development managers. Search covers
/// installed tools by name; registry search and metadata refresh have no
/// safe offline analog, so the engine reports refresh explicitly unsupported
/// instead of guessing.
const DEV_CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Upgrade,
];

/// User-installed command-line tools (Waves 4-5). These managers run only as
/// the invoking user and never cross a privilege boundary. `pip` is pinned to
/// an explicitly selected virtual environment; the rest use one user-scoped
/// home each.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DevKind {
    Cargo,
    Npm,
    Pnpm,
    Bun,
    Pip,
    Pipx,
    Uv,
    Mise,
    Composer,
    Gem,
}

impl DevKind {
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Bun => "bun",
            Self::Pip => "pip",
            Self::Pipx => "pipx",
            Self::Uv => "uv",
            Self::Mise => "mise",
            Self::Composer => "composer",
            Self::Gem => "gem",
        }
    }
    fn executable(self) -> &'static str {
        match self {
            Self::Composer => "composer",
            Self::Gem => "gem",
            _ => self.id(),
        }
    }
    /// Per-manager package-name policy. Anything resembling an option, path,
    /// URL, version specifier, or whitespace is rejected before any manager runs.
    fn valid_name(self, name: &str) -> bool {
        match self {
            Self::Cargo | Self::Npm | Self::Pnpm | Self::Bun => dev_name(name),
            Self::Pip | Self::Pipx | Self::Uv => python_name(name),
            Self::Mise => mise_name(name),
            Self::Composer => composer_name(name),
            Self::Gem => gem_name(name),
        }
    }
}

/// mise tools: a registry short name such as `node`, or `backend:identifier`
/// such as `aqua:cli/cli`, `cargo:ripgrep` or `npm:@scope/package`. `@` would
/// select a version, so it may only open an npm scope right after the prefix.
fn mise_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 214 || name.starts_with('-') {
        return false;
    }
    let (prefix, body) = match name.split_once(':') {
        Some((prefix, body)) => (Some(prefix), body),
        None => (None, name),
    };
    if prefix.is_some_and(|prefix| {
        prefix.is_empty()
            || !prefix
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }) {
        return false;
    }
    let body = match body.strip_prefix('@') {
        Some(scoped) if prefix.is_some() => scoped,
        _ => body,
    };
    (prefix.is_some() || !body.contains('/'))
        && body.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}

/// Shared package-name policy for development registries: an optional
/// `@scope/` prefix plus dot-separated segments. Anything else (options,
/// paths, URLs, whitespace) is rejected before reaching the manager.
pub(super) fn dev_name(name: &str) -> bool {
    fn segment(segment: &str) -> bool {
        !segment.is_empty()
            && segment.len() <= 214
            && segment.starts_with(|c: char| c.is_ascii_alphanumeric())
            && segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+_.-".contains(&b))
            && segment != "."
            && segment != ".."
    }
    if name.is_empty() || name.len() > 214 {
        return false;
    }
    match name.strip_prefix('@') {
        Some(scoped) => match scoped.split_once('/') {
            Some((scope, package)) => segment(scope) && segment(package),
            None => false,
        },
        None => segment(name) && !name.contains('/'),
    }
}

#[derive(Deserialize)]
struct NpmLs {
    dependencies: Option<std::collections::BTreeMap<String, NpmInstalled>>,
}

/// Explain an `npm ls` document without an installed tree: prefer npm's own
/// `error.summary`, then `error.detail` or code, then the first `problems`
/// entries. Falls back to a generic reason when npm said nothing usable.
fn npm_ls_problem(value: &serde_json::Value) -> String {
    const LIMIT: usize = 240;
    fn truncate(text: &str) -> String {
        if text.chars().count() > LIMIT {
            text.chars().take(LIMIT).collect::<String>() + "…"
        } else {
            text.to_owned()
        }
    }
    let detail = value
        .get("error")
        .and_then(|error| {
            error
                .get("summary")
                .or_else(|| error.get("detail"))
                .and_then(|detail| detail.as_str())
                .map(str::trim)
                .filter(|detail| !detail.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    error
                        .get("code")
                        .and_then(|code| code.as_str())
                        .map(|code| format!("npm error {code}"))
                })
        })
        .or_else(|| {
            value
                .get("problems")
                .and_then(|problems| problems.as_array())
                .map(|problems| {
                    problems
                        .iter()
                        .filter_map(|problem| problem.as_str())
                        .take(3)
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .filter(|joined| !joined.is_empty())
        });
    match detail {
        Some(detail) => format!("ls failed: {}", truncate(&detail)),
        None => "npm reported an error".to_owned(),
    }
}

#[derive(Deserialize)]
struct NpmInstalled {
    version: String,
}

#[derive(Deserialize)]
struct NpmOutdated {
    wanted: String,
}

#[derive(Deserialize)]
struct PnpmRoot {
    #[serde(default)]
    dependencies: std::collections::BTreeMap<String, PnpmEntry>,
}

#[derive(Deserialize)]
struct PnpmEntry {
    version: String,
}

#[derive(Deserialize)]
struct PnpmOutdated {
    latest: String,
}

/// Which installed development tools an inventory checks for newer versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Updates<'a> {
    None,
    All,
    /// Only the tool whose details are open.
    Only(&'a str),
}

impl Updates<'_> {
    fn covers(self, name: &str) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Only(only) => only == name,
        }
    }
}

/// Bun's global `package.json`: what `bun add --global` installed.
#[derive(Deserialize)]
struct BunGlobal {
    #[serde(default)]
    dependencies: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct BunManifest {
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    homepage: Option<String>,
}

/// Whether a `package.json` dependency comes from the npm registry: a
/// version, range or tag, not `github:`, `git+`, `file:`, `link:`, a URL,
/// an `npm:` alias or `owner/repo`.
fn registry_spec(spec: &serde_json::Value) -> bool {
    spec.as_str().is_some_and(|spec| !spec.contains([':', '/']))
}

/// PEP 508 bare project names for `pip`/`pipx`/`uv`: no options, paths,
/// URLs, extras, or version specifiers.
pub(super) fn python_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 214
        && !name.starts_with('-')
        && !name.ends_with(['-', '_', '.'])
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && !name.contains('/')
}

/// Composer `vendor/package` names, both parts lowercase alphanumerics with
/// `.`, `-`, `_` separators.
pub(super) fn composer_name(name: &str) -> bool {
    fn part(part: &str) -> bool {
        !part.is_empty()
            && part.len() <= 214
            && part.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && part.ends_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
    }
    match name.split_once('/') {
        Some((vendor, package)) => {
            !name.starts_with('-')
                && !name.contains("//")
                && part(vendor)
                && part(package)
                && name.len() <= 214
        }
        None => false,
    }
}

/// Queries sent to `gem search --remote`: plain gem names, as it takes a
/// regular expression, of at least three letters, as shorter ones list
/// most of RubyGems.
fn gem_searchable(query: &str) -> bool {
    gem_name(query) && query.len() >= 3
}
/// RubyGems bare gem names.
pub(super) fn gem_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 214
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && !name.starts_with('-')
        && !name.ends_with(['-', '_', '.'])
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

#[derive(Deserialize)]
struct PipEntry {
    name: String,
    version: String,
}

#[derive(Deserialize)]
struct PipOutdated {
    name: String,
    latest_version: String,
}

#[derive(Deserialize)]
struct PipxList {
    #[serde(default)]
    venvs: std::collections::BTreeMap<String, PipxVenv>,
}

#[derive(Deserialize)]
struct PipxVenv {
    metadata: PipxMetadata,
}

#[derive(Deserialize)]
struct PipxMetadata {
    main_package: PipxMain,
}

#[derive(Deserialize)]
struct PipxMain {
    package: String,
    package_version: String,
}

#[derive(Deserialize)]
struct PipxOutdatedList {
    data: PipxOutdatedData,
}

#[derive(Deserialize)]
struct PipxOutdatedData {
    #[serde(default)]
    packages: Vec<PipxOutdated>,
}

#[derive(Deserialize)]
struct PipxOutdated {
    package: String,
    latest_version: String,
}

#[derive(Deserialize)]
struct ComposerShow {
    #[serde(default)]
    installed: Vec<ComposerPackage>,
}

#[derive(Deserialize)]
struct ComposerPackage {
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    latest: Option<String>,
}

#[derive(Deserialize)]
struct NpmSearchHit {
    name: String,
    version: Option<String>,
    description: Option<String>,
}
#[derive(Deserialize)]
struct ComposerSearchHit {
    name: String,
    description: Option<String>,
}
/// Registry search results kept per source; engines rank them afterwards.
const REGISTRY_RESULTS: usize = 20;
/// `cargo search` rows: `name = "version"    # description`.
fn cargo_search_hit(line: &str) -> Option<(&str, &str, &str)> {
    let (name, rest) = line.split_once(" = \"")?;
    let (version, rest) = rest.split_once('"')?;
    let summary = rest.trim().strip_prefix('#').unwrap_or_default().trim();
    Some((name.trim(), version, summary))
}
/// `gem search --remote` rows: `name (version[ platforms][, older versions])`,
/// such as `nokogiri (1.19.4 ruby aarch64-linux-gnu, 1.17.2 x86_64-linux)`.
fn gem_search_hit(line: &str) -> Option<(&str, &str)> {
    let (name, rest) = line.trim().split_once(" (")?;
    let versions = rest.strip_suffix(')')?;
    let version = versions.split([',', ' ']).next()?.trim();
    (!version.is_empty()).then_some((name, version))
}

#[derive(Deserialize)]
struct MiseInstall {
    version: String,
    requested_version: Option<String>,
    #[serde(default)]
    installed: bool,
    #[serde(default)]
    active: bool,
}
#[derive(Deserialize)]
struct MiseOutdated {
    latest: Option<String>,
}
#[derive(Deserialize)]
struct MiseRegistryTool {
    short: String,
    description: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
}

pub struct DevTool<T = NativeTransport> {
    pub(super) kind: DevKind,
    transport: T,
    pub(super) home: Option<PathBuf>,
    /// Recent registry answers, oldest first. Typing runs one search per
    /// keystroke, so going back to a query, or narrowing a complete one,
    /// does not wait for the network again. Any change clears them.
    searches: Vec<RegistrySearch>,
}
/// One registry answer, before ranking.
struct RegistrySearch {
    query: String,
    hits: Vec<RegistryHit>,
}
/// A registry match: name, latest version and summary.
type RegistryHit = (String, String, String);
/// Registry answers kept per source.
const REGISTRY_SEARCHES: usize = 16;
pub type Cargo<T = NativeTransport> = DevTool<T>;
pub type Npm<T = NativeTransport> = DevTool<T>;
pub type Pnpm<T = NativeTransport> = DevTool<T>;
pub type Bun<T = NativeTransport> = DevTool<T>;
pub type Pip<T = NativeTransport> = DevTool<T>;
pub type Pipx<T = NativeTransport> = DevTool<T>;
pub type Uv<T = NativeTransport> = DevTool<T>;
pub type Mise<T = NativeTransport> = DevTool<T>;
pub type Composer<T = NativeTransport> = DevTool<T>;
pub type Gem<T = NativeTransport> = DevTool<T>;

impl<T> DevTool<T> {
    fn new(kind: DevKind, transport: T) -> Self {
        Self {
            kind,
            transport,
            home: None,
            searches: vec![],
        }
    }
    pub fn cargo(transport: T) -> Self {
        Self::new(DevKind::Cargo, transport)
    }
    pub fn npm(transport: T) -> Self {
        Self::new(DevKind::Npm, transport)
    }
    pub fn pnpm(transport: T) -> Self {
        Self::new(DevKind::Pnpm, transport)
    }
    pub fn bun(transport: T) -> Self {
        Self::new(DevKind::Bun, transport)
    }
    pub fn pip(transport: T) -> Self {
        Self::new(DevKind::Pip, transport)
    }
    pub fn pipx(transport: T) -> Self {
        Self::new(DevKind::Pipx, transport)
    }
    pub fn uv(transport: T) -> Self {
        Self::new(DevKind::Uv, transport)
    }
    pub fn mise(transport: T) -> Self {
        Self::new(DevKind::Mise, transport)
    }
    pub fn composer(transport: T) -> Self {
        Self::new(DevKind::Composer, transport)
    }
    pub fn gem(transport: T) -> Self {
        Self::new(DevKind::Gem, transport)
    }
}

impl<T: Transport> DevTool<T> {
    pub(super) fn call(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.transport.dev_tool(
            self.kind.executable(),
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            cancel,
            write,
        )
    }
    /// Resolve the manager home used as the package scope. npm, pnpm, uv,
    /// Composer, and gem report it; Cargo, Bun, and pipx derive it from the
    /// sanitized environment; `pip` requires an explicitly selected `VIRTUAL_ENV`.
    fn home(&self, cancel: &Cancellation) -> Result<PathBuf, ExecutionError> {
        match self.kind {
            DevKind::Cargo | DevKind::Bun => {
                let home = self.transport.env("HOME").ok_or_else(|| {
                    ExecutionError::Disabled(format!("{} home not found", self.kind.id()))
                })?;
                let mut path = PathBuf::from(home);
                path.push(if self.kind == DevKind::Cargo {
                    ".cargo"
                } else {
                    ".bun/install/global"
                });
                Ok(path)
            }
            DevKind::Pip => {
                let venv = self.transport.env("VIRTUAL_ENV").ok_or_else(|| {
                    ExecutionError::Disabled(
                        "pip requires an explicitly selected virtual environment".to_string(),
                    )
                })?;
                let path = PathBuf::from(venv);
                if !path.is_absolute() {
                    return Err(ExecutionError::Invalid(
                        "virtual environment path must be absolute".into(),
                    ));
                }
                Ok(path)
            }
            DevKind::Pipx => {
                if let Some(dir) = self.transport.env("PIPX_HOME") {
                    let path = PathBuf::from(dir);
                    if !path.is_absolute() {
                        return Err(ExecutionError::Invalid("pipx home must be absolute".into()));
                    }
                    return Ok(path);
                }
                let home = self
                    .transport
                    .env("HOME")
                    .ok_or_else(|| ExecutionError::Disabled("pipx home not found".to_string()))?;
                Ok(PathBuf::from(home).join(".local/share/pipx"))
            }
            DevKind::Uv => {
                if let Some(dir) = self.transport.env("UV_TOOL_DIR") {
                    let path = PathBuf::from(dir);
                    if !path.is_absolute() {
                        return Err(ExecutionError::Invalid(
                            "uv tool directory must be absolute".into(),
                        ));
                    }
                    return Ok(path);
                }
                let output = self.call(&["tool", "dir"], cancel, false)?;
                let path = PathBuf::from(
                    String::from_utf8(output.stdout)
                        .map_err(|e| ExecutionError::Invalid(e.to_string()))?
                        .trim(),
                );
                if !path.is_absolute() {
                    return Err(ExecutionError::Invalid(
                        "uv tool directory must be absolute".into(),
                    ));
                }
                Ok(path)
            }
            // mise keeps installs in its data directory; only tools in its
            // global configuration are listed (see `mise_inventory`).
            DevKind::Mise => {
                let path = match (
                    self.transport.env("MISE_DATA_DIR"),
                    self.transport.env("XDG_DATA_HOME"),
                    self.transport.env("HOME"),
                ) {
                    (Some(dir), _, _) => PathBuf::from(dir),
                    (None, Some(data), _) => PathBuf::from(data).join("mise"),
                    (None, None, Some(home)) => PathBuf::from(home).join(".local/share/mise"),
                    (None, None, None) => {
                        return Err(ExecutionError::Disabled("mise home not found".into()))
                    }
                };
                if !path.is_absolute() {
                    return Err(ExecutionError::Invalid(
                        "mise data directory must be absolute".into(),
                    ));
                }
                Ok(path)
            }
            DevKind::Composer => {
                if let Some(dir) = self.transport.env("COMPOSER_HOME") {
                    let path = PathBuf::from(dir);
                    if !path.is_absolute() {
                        return Err(ExecutionError::Invalid(
                            "Composer home must be absolute".into(),
                        ));
                    }
                    return Ok(path);
                }
                let output = self.call(&["config", "--global", "home"], cancel, false)?;
                let text = String::from_utf8(output.stdout)
                    .map_err(|e| ExecutionError::Invalid(e.to_string()))?;
                let line = text.lines().map(str::trim).rfind(|line| !line.is_empty());
                match line.map(PathBuf::from) {
                    Some(path) if path.is_absolute() => Ok(path),
                    _ => Err(ExecutionError::Invalid(
                        "Composer home must be absolute".into(),
                    )),
                }
            }
            DevKind::Gem => {
                if let Some(dir) = self.transport.env("GEM_HOME") {
                    let path = PathBuf::from(dir);
                    if !path.is_absolute() {
                        return Err(ExecutionError::Invalid(
                            "RubyGems home must be absolute".into(),
                        ));
                    }
                    return Ok(path);
                }
                let output = self.call(&["env"], cancel, false)?;
                let text = String::from_utf8(output.stdout)
                    .map_err(|e| ExecutionError::Invalid(e.to_string()))?;
                for line in text.lines() {
                    if let Some(dir) = line.trim().strip_prefix("- USER INSTALLATION DIRECTORY:") {
                        let path = PathBuf::from(dir.trim());
                        if path.is_absolute() {
                            return Ok(path);
                        }
                        break;
                    }
                }
                Err(ExecutionError::Invalid(
                    "RubyGems user directory must be absolute".into(),
                ))
            }
            DevKind::Npm | DevKind::Pnpm => {
                let output = match self.call(&["root", "--global"], cancel, false) {
                    // pnpm 11 and later refuse every global command until
                    // `pnpm setup` puts their bin directory on PATH.
                    Err(ExecutionError::Failed(result))
                        if self.kind == DevKind::Pnpm
                            && String::from_utf8_lossy(&result.stderr)
                                .contains("ERR_PNPM_GLOBAL_BIN_DIR_NOT_IN_PATH") =>
                    {
                        return Err(ExecutionError::Disabled(
                            "pnpm's global bin directory isn't in PATH. Run `pnpm setup`, then open a new terminal".into(),
                        ));
                    }
                    result => result?,
                };
                let path = PathBuf::from(
                    String::from_utf8(output.stdout)
                        .map_err(|e| ExecutionError::Invalid(e.to_string()))?
                        .trim(),
                );
                if !path.is_absolute() {
                    return Err(ExecutionError::Invalid(format!(
                        "{} global root must be absolute",
                        self.kind.id()
                    )));
                }
                Ok(path)
            }
        }
    }
    /// Parse manager JSON leniently: clean exits must parse, while
    /// data-bearing failure exits (npm outdated packages, troubled trees)
    /// are accepted only when stdout holds the expected document.
    fn lenient_json(
        &self,
        args: &[&str],
        cancel: &Cancellation,
    ) -> Result<(serde_json::Value, bool), EngineError> {
        match self.call(args, cancel, false) {
            Ok(result) => {
                let value = serde_json::from_slice(&bytes(self.kind.id(), result)?)
                    .map_err(|e| invalid(self.kind.id(), e))?;
                Ok((value, false))
            }
            Err(ExecutionError::Failed(result)) => {
                let value = serde_json::from_slice(&result.stdout)
                    .map_err(|_| EngineError::from(ExecutionError::Failed(result)))?;
                Ok((value, true))
            }
            Err(error) => Err(error.into()),
        }
    }
    pub(super) fn pip_call(
        &self,
        venv: &std::path::Path,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.transport.venv_pip(
            venv,
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            cancel,
            write,
        )
    }
    fn target<'a>(&self, id: &'a PackageId) -> Result<&'a str, EngineError> {
        if id.backend != self.kind.id()
            || id.architecture != std::env::consts::ARCH
            || !self.kind.valid_name(&id.name)
            || self
                .home
                .as_ref()
                .is_none_or(|home| id.scope != (Scope::Environment { path: home.clone() }))
        {
            return Err(invalid(
                self.kind.id(),
                "foreign identity or invalid package name",
            ));
        }
        Ok(&id.name)
    }
    fn detail(
        &self,
        home: &std::path::Path,
        name: String,
        summary: String,
        homepage: Option<String>,
        installed: String,
        candidate: Option<String>,
    ) -> Result<PackageDetails, EngineError> {
        if !self.kind.valid_name(&name) {
            return Err(invalid(
                self.kind.id(),
                "foreign identity or invalid package name",
            ));
        }
        Ok(PackageDetails {
            package: Package {
                id: PackageId {
                    backend: self.kind.id().into(),
                    name: name.clone(),
                    architecture: std::env::consts::ARCH.into(),
                    scope: Scope::Environment {
                        path: home.to_path_buf(),
                    },
                    remote: None,
                    reference: None,
                },
                display_name: name,
                summary: summary.clone(),
                update: match &candidate {
                    Some(candidate) if candidate != &installed => UpdateAvailability::Available,
                    _ => UpdateAvailability::Current,
                },
                installed_version: Some(installed),
                candidate_version: candidate,
                icon: None,
                component_ids: vec![],
                // Manifest homepages (bun, composer, …) double as grouping
                // keys so dev tools group with native builds of the same app.
                homepages: homepage.clone().into_iter().collect(),
                adopt_with: None,
            },
            description: summary,
            homepage,
            dependencies: vec![],
        })
    }
    /// Split one `cargo install --list` header (`name vversion [(source)]:`)
    /// without allocating, and whether the package came from crates.io, which
    /// current Cargo leaves unnamed. Anything else is not a package entry.
    fn cargo_header(line: &str) -> Option<(&str, &str, bool)> {
        let header = line.strip_suffix(':')?;
        let (name, rest) = header.split_once(' ')?;
        let (version, sourced, crates_io) = match rest.split_once(' ') {
            Some((version, source)) => (
                version,
                source.starts_with('(') && source.ends_with(')'),
                // Older Cargo names crates.io too.
                matches!(
                    source,
                    "(registry+https://github.com/rust-lang/crates.io-index)"
                        | "(registry+sparse+https://index.crates.io/)"
                ),
            ),
            None => (rest, true, true),
        };
        let version = version.strip_prefix('v')?;
        if !sourced || version.is_empty() || version.chars().any(char::is_whitespace) {
            return None;
        }
        Some((name, version, crates_io))
    }
    /// What `cargo install --list` says.
    fn cargo_list(&self, cancel: &Cancellation) -> Result<String, EngineError> {
        let id = self.kind.id();
        let output = bytes(id, self.call(&["install", "--list"], cancel, false)?)?;
        String::from_utf8(output).map_err(|e| invalid(id, e))
    }
    fn cargo_inventory(
        &self,
        home: &std::path::Path,
        updates: Updates,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let text = self.cargo_list(cancel)?;
        let mut details = Vec::new();
        for line in text.lines() {
            if line.is_empty() || line.starts_with([' ', '\t']) {
                continue;
            }
            let Some((name, version, crates_io)) = Self::cargo_header(line) else {
                return Err(invalid(id, "invalid cargo metadata"));
            };
            // Git and path installs have no registry version to compare.
            let candidate = if updates.covers(name) && crates_io {
                self.newer_version(home, name, version, cancel)?
            } else {
                None
            };
            details.push(self.detail(
                home,
                name.into(),
                "Cargo-installed command-line tool".into(),
                None,
                version.into(),
                candidate,
            )?);
        }
        Ok(details)
    }
    fn npm_table(
        &self,
        cancel: &Cancellation,
    ) -> Result<std::collections::BTreeMap<String, (String, Option<String>)>, EngineError> {
        let id = self.kind.id();
        let (value, lenient) =
            self.lenient_json(&["ls", "--global", "--depth=0", "--json"], cancel)?;
        // Keep npm's own diagnostic before the document moves: a failing
        // tree without installed packages reports its cause here instead of
        // a bare failure downstream.
        let problem = npm_ls_problem(&value);
        let report: NpmLs = serde_json::from_value(value).map_err(|e| invalid(id, e))?;
        let installed = match (report.dependencies, lenient) {
            (Some(dependencies), _) => dependencies,
            (None, false) => std::collections::BTreeMap::new(),
            (None, true) => return Err(invalid(id, problem)),
        };
        let outdated: std::collections::BTreeMap<String, NpmOutdated> =
            match self.lenient_json(&["outdated", "--global", "--json"], cancel) {
                Ok((value, _)) => serde_json::from_value(value).map_err(|e| invalid(id, e))?,
                // Registry metadata may be unreachable; installed state stays.
                Err(_) => std::collections::BTreeMap::new(),
            };
        Ok(installed
            .into_iter()
            .map(|(name, entry)| {
                let wanted = outdated.get(&name).map(|outdated| outdated.wanted.clone());
                (name, (entry.version, wanted))
            })
            .collect())
    }
    fn npm_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        self.npm_table(cancel)?
            .into_iter()
            .map(|(name, (installed, wanted))| {
                let candidate = match wanted {
                    Some(wanted) if wanted != installed => wanted,
                    _ => installed.clone(),
                };
                self.detail(
                    home,
                    name,
                    "npm-installed command-line tool".into(),
                    None,
                    installed,
                    Some(candidate),
                )
            })
            .collect()
    }
    fn pnpm_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let (value, _) = self.lenient_json(&["ls", "--global", "--json", "--depth=0"], cancel)?;
        let roots: Vec<PnpmRoot> = serde_json::from_value(value).map_err(|e| invalid(id, e))?;
        let outdated: std::collections::BTreeMap<String, PnpmOutdated> =
            match self.lenient_json(&["outdated", "--global", "--json"], cancel) {
                Ok((value, _)) => serde_json::from_value(value).map_err(|e| invalid(id, e))?,
                // Registry metadata may be unreachable; installed state stays.
                Err(_) => std::collections::BTreeMap::new(),
            };
        let mut details = Vec::new();
        for root in roots {
            for (name, entry) in root.dependencies {
                let candidate = outdated.get(&name).map(|outdated| outdated.latest.clone());
                let candidate = match candidate {
                    Some(latest) if latest != entry.version => Some(latest),
                    _ => Some(entry.version.clone()),
                };
                details.push(self.detail(
                    home,
                    name,
                    "pnpm-installed command-line tool".into(),
                    None,
                    entry.version,
                    candidate,
                )?);
            }
        }
        Ok(details)
    }
    fn bun_package(&self, home: &std::path::Path, dir: &std::path::Path) -> Option<PackageDetails> {
        let manifest =
            std::fs::read_to_string(host_metadata_path(&dir.join("package.json"))).ok()?;
        let manifest: BunManifest = serde_json::from_str(&manifest).ok()?;
        if manifest.version.is_empty() {
            return None;
        }
        let homepage = manifest.homepage.filter(|homepage| !homepage.is_empty());
        self.detail(
            home,
            manifest.name,
            if manifest.description.is_empty() {
                "Bun-installed command-line tool".into()
            } else {
                manifest.description
            },
            homepage,
            manifest.version,
            None,
        )
        .ok()
    }
    fn bun_inventory(
        &self,
        home: &std::path::Path,
        updates: Updates,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        // Bun hoists every dependency into one flat `node_modules` and keeps
        // them after a removal, so only the global manifest's dependencies
        // are packages the user installed.
        let manifest = match std::fs::read_to_string(host_metadata_path(&home.join("package.json")))
        {
            Ok(manifest) => manifest,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(invalid(id, e.to_string())),
        };
        let manifest: BunGlobal = serde_json::from_str(&manifest).map_err(|e| invalid(id, e))?;
        let modules = home.join("node_modules");
        let mut details: Vec<PackageDetails> = manifest
            .dependencies
            .keys()
            .filter(|name| self.kind.valid_name(name))
            .filter_map(|name| self.bun_package(home, &modules.join(name)))
            .collect();
        for detail in &mut details {
            let package = &mut detail.package;
            // A tool from git, a folder or a link has no registry version.
            let from_registry = manifest
                .dependencies
                .get(&package.id.name)
                .is_some_and(registry_spec);
            if !updates.covers(&package.id.name) || !from_registry {
                continue;
            }
            let installed = package.installed_version.as_deref().unwrap_or_default();
            if let Some(newer) = self.newer_version(home, &package.id.name, installed, cancel)? {
                package.candidate_version = Some(newer);
                package.update = UpdateAvailability::Available;
            }
        }
        Ok(details)
    }
    /// The registry's latest version of an installed Cargo or Bun tool when
    /// it is newer than `installed`. These managers have no outdated command,
    /// so each tool is looked up; offline or unknown tools get no candidate.
    fn newer_version(
        &self,
        home: &std::path::Path,
        name: &str,
        installed: &str,
        cancel: &Cancellation,
    ) -> Result<Option<String>, EngineError> {
        let latest = match self.kind {
            DevKind::Cargo => self
                .registry_hits(name, cancel)?
                .into_iter()
                .find(|(hit, _, _)| hit == name)
                .map(|(_, version, _)| version),
            // `bun pm view` needs a package.json, so it runs in Bun's
            // global folder.
            _ => {
                let args = [
                    OsStr::new("pm"),
                    OsStr::new("view"),
                    OsStr::new("--cwd"),
                    home.as_os_str(),
                    OsStr::new(name),
                    OsStr::new("version"),
                ]
                .map(OsString::from);
                match self
                    .transport
                    .dev_tool(self.kind.executable(), &args, cancel, false)
                {
                    Err(ExecutionError::Cancelled) => return Err(EngineError::Cancelled),
                    Ok(result) if !result.truncated => String::from_utf8(result.stdout)
                        .ok()
                        .map(|version| version.trim().to_string()),
                    _ => None,
                }
            }
        };
        let newer = |latest: &str| match (
            semver::Version::parse(latest),
            semver::Version::parse(installed),
        ) {
            (Ok(latest), Ok(installed)) => latest > installed,
            _ => false,
        };
        Ok(latest.filter(|latest| newer(latest)))
    }
    fn pip_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let output = bytes(
            id,
            self.pip_call(home, &["list", "--format=json"], cancel, false)?,
        )?;
        let entries: Vec<PipEntry> = serde_json::from_slice(&output).map_err(|e| invalid(id, e))?;
        let outdated: std::collections::BTreeMap<String, String> = match self.pip_call(
            home,
            &["list", "--outdated", "--format=json"],
            cancel,
            false,
        ) {
            Ok(result) => serde_json::from_slice(&bytes(id, result)?)
                .map(|entries: Vec<PipOutdated>| {
                    entries
                        .into_iter()
                        .map(|entry| (entry.name, entry.latest_version))
                        .collect()
                })
                .map_err(|e| invalid(id, e))?,
            // Registry metadata may be unreachable; installed state stays.
            Err(_) => std::collections::BTreeMap::new(),
        };
        entries
            .into_iter()
            // The installer itself is never managed: upgrading pip from
            // inside pip breaks its own output pipe and risks the venv.
            .filter(|entry| {
                !matches!(
                    entry.name.to_ascii_lowercase().as_str(),
                    "pip" | "setuptools" | "wheel" | "distribute"
                )
            })
            .map(|entry| {
                let candidate = outdated.get(&entry.name).cloned().or_else(|| {
                    // Installed state without newer metadata stays current.
                    Some(entry.version.clone())
                });
                self.detail(
                    home,
                    entry.name,
                    format!("Python package installed in {}", home.display()),
                    None,
                    entry.version,
                    candidate,
                )
            })
            .collect()
    }
    fn pipx_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let output = bytes(id, self.call(&["list", "--json"], cancel, false)?)?;
        let list: PipxList = serde_json::from_slice(&output).map_err(|e| invalid(id, e))?;
        let outdated: std::collections::BTreeMap<String, String> =
            match self.call(&["list", "--outdated", "--json"], cancel, false) {
                Ok(result) => serde_json::from_slice(&bytes(id, result)?)
                    .map(|list: PipxOutdatedList| {
                        list.data
                            .packages
                            .into_iter()
                            .map(|entry| (entry.package, entry.latest_version))
                            .collect()
                    })
                    .unwrap_or_default(),
                Err(_) => std::collections::BTreeMap::new(),
            };
        list.venvs
            .into_values()
            .map(|venv| {
                let main = venv.metadata.main_package;
                let candidate = outdated
                    .get(&main.package)
                    .cloned()
                    .unwrap_or_else(|| main.package_version.clone());
                self.detail(
                    home,
                    main.package,
                    "Isolated Python application installed with pipx".into(),
                    None,
                    main.package_version,
                    Some(candidate),
                )
            })
            .collect()
    }
    /// Parse one `uv tool list [--outdated]` line: `name vversion` with an
    /// optional `[latest: version]` suffix. Executable continuation lines
    /// (`- name`) carry no version and are skipped by the caller via `None`;
    /// anything else without a version is not a tool entry.
    pub(super) fn uv_entry(line: &str) -> Option<(String, String, Option<String>)> {
        let line = line.trim();
        if line.is_empty() || line == "-" || line.starts_with("- ") {
            return None;
        }
        let (name, rest) = line.split_once(' ')?;
        let rest = rest.trim();
        let (version, latest) = match rest.split_once("[latest:") {
            Some((version, latest)) => {
                let latest = latest.strip_suffix(']')?.trim();
                (version.trim(), Some(latest.to_string()))
            }
            None => (rest, None),
        };
        let version = version.strip_prefix('v')?;
        if version.is_empty() || version.chars().any(char::is_whitespace) {
            return None;
        }
        Some((name.to_string(), version.to_string(), latest))
    }
    fn uv_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let output = bytes(id, self.call(&["tool", "list"], cancel, false)?)?;
        let text = String::from_utf8(output).map_err(|e| invalid(id, e))?;
        if text.trim().is_empty() || text.trim() == "No tools installed" {
            return Ok(vec![]);
        }
        let outdated: std::collections::BTreeMap<String, String> =
            match self.call(&["tool", "list", "--outdated"], cancel, false) {
                Ok(result) => String::from_utf8(bytes(id, result)?)
                    .map(|text| {
                        text.lines()
                            .filter_map(Self::uv_entry)
                            .filter_map(|(name, _, latest)| latest.map(|latest| (name, latest)))
                            .collect()
                    })
                    .unwrap_or_default(),
                Err(_) => std::collections::BTreeMap::new(),
            };
        let mut details = Vec::new();
        for line in text.lines() {
            // Only tool headers carry versions; executable lines are skipped.
            let Some((name, version, _)) = Self::uv_entry(line) else {
                continue;
            };
            let candidate = outdated
                .get(&name)
                .cloned()
                .unwrap_or_else(|| version.clone());
            details.push(self.detail(
                home,
                name,
                "Isolated Python tool installed with uv".into(),
                None,
                version,
                Some(candidate),
            )?);
        }
        Ok(details)
    }
    fn composer_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let output = bytes(
            id,
            self.call(&["global", "show", "--format=json"], cancel, false)?,
        )?;
        // An empty global home has no composer.json; Composer reports that on
        // stderr but still exits 0 with no JSON. Treat unparseable empty
        // output as no packages only when stdout holds no document.
        if output.iter().all(|b| b.is_ascii_whitespace()) {
            return Ok(vec![]);
        }
        let show: ComposerShow = serde_json::from_slice(&output).map_err(|e| invalid(id, e))?;
        let outdated: std::collections::BTreeMap<String, String> = match self.call(
            &["global", "show", "--outdated", "--format=json"],
            cancel,
            false,
        ) {
            Ok(result) => serde_json::from_slice(&bytes(id, result)?)
                .map(|show: ComposerShow| {
                    show.installed
                        .into_iter()
                        .filter_map(|package| package.latest.map(|latest| (package.name, latest)))
                        .collect()
                })
                .unwrap_or_default(),
            Err(_) => std::collections::BTreeMap::new(),
        };
        show.installed
            .into_iter()
            .map(|package| {
                let candidate = outdated
                    .get(&package.name)
                    .cloned()
                    .or(package.latest.clone())
                    .unwrap_or_else(|| package.version.clone());
                self.detail(
                    home,
                    package.name,
                    package.description.clone().unwrap_or_default(),
                    package.homepage.clone(),
                    package.version,
                    Some(candidate),
                )
            })
            .collect()
    }
    /// Split a `*.gemspec` file name into `(name, version)`. Gem names may
    /// contain dashes, so split at the last dash preceding a version.
    pub(super) fn gem_filename(file: &str) -> Option<(String, String)> {
        let base = file.strip_suffix(".gemspec")?;
        let (name, version) = base.rsplit_once('-')?;
        if name.is_empty()
            || version.is_empty()
            || !version.starts_with(|c: char| c.is_ascii_digit())
        {
            return None;
        }
        Some((name.to_string(), version.to_string()))
    }
    /// Compare dot-separated gem versions numerically where possible so the
    /// inventory reports one identity per gem (the highest installed).
    pub(super) fn gem_version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
        let mut a_parts = a.split('.');
        let mut b_parts = b.split('.');
        loop {
            match (a_parts.next(), b_parts.next()) {
                (None, None) => return std::cmp::Ordering::Equal,
                (None, _) => return std::cmp::Ordering::Less,
                (_, None) => return std::cmp::Ordering::Greater,
                (Some(x), Some(y)) => {
                    let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                        (Ok(x), Ok(y)) => x.cmp(&y),
                        _ => x.cmp(y),
                    };
                    if ord != std::cmp::Ordering::Equal {
                        return ord;
                    }
                }
            }
        }
    }
    /// Parse one `gem outdated` line: `name (installed < latest[, ...])`.
    pub(super) fn gem_outdated(line: &str) -> Option<(String, String)> {
        let (name, rest) = line.split_once(' ')?;
        if name.is_empty() || name.starts_with('-') {
            return None;
        }
        let inner = rest.trim().strip_prefix('(')?.strip_suffix(')')?;
        let (before, latest) = inner.split_once('<')?;
        // Validate the installed side, then report the latest version.
        before.split(',').rfind(|part| !part.trim().is_empty())?;
        let latest = latest.split(',').next()?;
        Some((name.to_string(), latest.trim().to_string()))
    }
    fn gem_inventory(
        &self,
        home: &std::path::Path,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let mut names: Vec<(String, String)> = Vec::new();
        match std::fs::read_dir(host_metadata_path(&home.join("specifications"))) {
            Ok(entries) => {
                for entry in entries.filter_map(Result::ok) {
                    if cancel.requested() {
                        return Err(EngineError::Cancelled);
                    }
                    let file = entry.file_name().to_string_lossy().into_owned();
                    if let Some((name, version)) = Self::gem_filename(&file) {
                        names.push((name, version));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(invalid(id, e.to_string())),
        }
        let outdated: std::collections::BTreeMap<String, String> =
            match self.call(&["outdated"], cancel, false) {
                Ok(result) => String::from_utf8(bytes(id, result)?)
                    .map(|text| text.lines().filter_map(Self::gem_outdated).collect())
                    .unwrap_or_default(),
                Err(_) => std::collections::BTreeMap::new(),
            };
        // Native updates retain older versions beside the new one; report one
        // identity per gem at the highest installed version so engine
        // selection stays unambiguous. Removal still drops every version.
        // Sorted collection keeps the surviving identity deterministic.
        names.sort();
        let mut highest: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for (name, version) in names {
            highest
                .entry(name)
                .and_modify(|keep| {
                    if Self::gem_version_cmp(&version, keep) == std::cmp::Ordering::Greater {
                        *keep = version.clone();
                    }
                })
                .or_insert(version);
        }
        highest
            .into_iter()
            .map(|(name, version)| {
                let candidate = outdated
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| version.clone());
                self.detail(
                    home,
                    name,
                    "RubyGem installed for the invoking user".into(),
                    None,
                    version,
                    Some(candidate),
                )
            })
            .collect()
    }
    /// Tools from mise's global configuration only: project files such as
    /// `mise.toml` or `.tool-versions` pin versions for that project and stay
    /// out of PkgDeck. Candidates stay within the configured version range,
    /// matching `mise upgrade`, which never rewrites the configuration. Host
    /// commands run from `/` with a cleared environment, so the project in
    /// the caller's working directory never joins mise's configuration stack.
    ///
    /// `check_updates` asks mise for newer versions, which reaches every
    /// tool's version source over the network and can take many seconds
    /// offline. Searches skip it and leave the update state unknown.
    fn mise_inventory(
        &self,
        home: &std::path::Path,
        check_updates: bool,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let output = bytes(id, self.call(&["ls", "--global", "--json"], cancel, false)?)?;
        let tools: std::collections::BTreeMap<String, Vec<MiseInstall>> =
            serde_json::from_slice(&output).map_err(|e| invalid(id, e))?;
        let outdated: std::collections::BTreeMap<String, MiseOutdated> = if !check_updates {
            std::collections::BTreeMap::new()
        } else {
            match self.call(&["outdated", "--json"], cancel, false) {
                Ok(result) => serde_json::from_slice(&bytes(id, result)?).unwrap_or_default(),
                // A cancelled check must not report every tool as current.
                Err(ExecutionError::Cancelled) => return Err(EngineError::Cancelled),
                Err(_) => std::collections::BTreeMap::new(),
            }
        };
        tools
            .into_iter()
            // Tools with backend options (`ubi:owner/repo[exe=x]`) are listed
            // by mise but cannot be addressed safely; leave them out.
            .filter(|(name, _)| mise_name(name))
            .filter_map(|(name, installs)| {
                // With several configured versions (`go = ["1.22", "1.25"]`)
                // all are active and the first one is what runs.
                let mut installed: Vec<_> = installs
                    .into_iter()
                    .filter(|install| install.installed)
                    .collect();
                if installed.is_empty() {
                    return None;
                }
                let first_active = installed.iter().position(|install| install.active);
                let install = installed.swap_remove(first_active.unwrap_or(0));
                let candidate = outdated
                    .get(&name)
                    .and_then(|entry| entry.latest.clone())
                    .unwrap_or_else(|| install.version.clone());
                let summary = match &install.requested_version {
                    Some(requested) => format!("Global mise tool, version {requested}"),
                    None => "Global mise tool".into(),
                };
                let mut details =
                    self.detail(home, name, summary, None, install.version, Some(candidate));
                if !check_updates {
                    if let Ok(details) = &mut details {
                        details.package.update = UpdateAvailability::Unknown;
                        details.package.candidate_version = None;
                    }
                }
                Some(details)
            })
            .collect()
    }
    /// Search mise's registry for tools to install globally. A match that is
    /// already installed comes back as its installed row.
    fn mise_offers(
        &self,
        home: &std::path::Path,
        query: &str,
        installed: &[Package],
        cancel: &Cancellation,
    ) -> Result<Vec<Package>, EngineError> {
        let id = self.kind.id();
        let output = bytes(id, self.call(&["registry", "--json"], cancel, false)?)?;
        let registry: Vec<MiseRegistryTool> =
            serde_json::from_slice(&output).map_err(|e| invalid(id, e))?;
        let lowered = query.to_ascii_lowercase();
        let matches = |text: &str| text.to_ascii_lowercase().contains(&lowered);
        let mut found: Vec<MiseRegistryTool> = registry
            .into_iter()
            .filter(|tool| mise_name(&tool.short))
            .filter(|tool| {
                matches(&tool.short)
                    || tool.aliases.iter().any(|alias| matches(alias))
                    || tool.description.as_deref().is_some_and(matches)
            })
            .collect();
        // Exact names and aliases first, so the result cap never drops them.
        found.sort_by_key(|tool| {
            if tool.short.eq_ignore_ascii_case(query) {
                0
            } else if tool
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(query))
            {
                1
            } else {
                2
            }
        });
        Ok(found
            .into_iter()
            .take(50)
            .map(|tool| {
                match installed
                    .iter()
                    .find(|package| package.id.name == tool.short)
                {
                    Some(package) => package.clone(),
                    None => Package {
                        id: PackageId {
                            backend: id.into(),
                            name: tool.short.clone(),
                            architecture: std::env::consts::ARCH.into(),
                            scope: Scope::Environment {
                                path: home.to_path_buf(),
                            },
                            remote: None,
                            reference: None,
                        },
                        display_name: tool.short,
                        summary: tool.description.unwrap_or_default(),
                        installed_version: None,
                        // Registry tools are real matches; `mise use --global` installs the
                        // latest version, so that is the candidate.
                        candidate_version: Some("latest".into()),
                        update: UpdateAvailability::Unknown,
                        icon: None,
                        component_ids: vec![],
                        homepages: vec![],
                        adopt_with: None,
                    },
                }
            })
            .collect())
    }
    /// Packages matching `query` in the source's registry, using the manager's
    /// own search command: npm (also for pnpm and Bun, which share the npm
    /// registry but have no search command), crates.io, RubyGems and
    /// Packagist. PyPI has had no search API since 2020. A registry that
    /// cannot be reached (offline) leaves only installed matches.
    fn registry_hits(
        &self,
        query: &str,
        cancel: &Cancellation,
    ) -> Result<Vec<RegistryHit>, EngineError> {
        let hits = self.registry_search(query, cancel)?.unwrap_or_default();
        Ok(self.rank(query, hits))
    }
    /// [`Self::registry_hits`] answered from recent searches when it can be.
    /// The same query reuses its answer. RubyGems lists every gem whose name
    /// contains the query, so a longer query that contains a searched one
    /// keeps the matching rows of that complete list. Other registries cap
    /// and rank their answers, so only the same query is reused there.
    fn cached_registry_hits(
        &mut self,
        query: &str,
        cancel: &Cancellation,
    ) -> Result<Vec<RegistryHit>, EngineError> {
        let lowered = query.to_ascii_lowercase();
        let narrows = |searched: &str| {
            self.kind == DevKind::Gem
                && gem_searchable(query)
                && lowered.contains(&searched.to_ascii_lowercase())
        };
        let known = self.searches.iter().rev().find_map(|search| {
            if search.query == query {
                Some(search.hits.clone())
            } else if narrows(&search.query) {
                Some(
                    search
                        .hits
                        .iter()
                        .filter(|(name, ..)| name.to_ascii_lowercase().contains(&lowered))
                        .cloned()
                        .collect(),
                )
            } else {
                None
            }
        });
        let hits = match known {
            Some(hits) => hits,
            None => {
                let Some(hits) = self.registry_search(query, cancel)? else {
                    return Ok(vec![]);
                };
                // `gem search` exits cleanly with no rows when it cannot
                // reach a source, so an empty gem answer is asked again.
                if self.kind == DevKind::Gem && hits.is_empty() {
                    return Ok(hits);
                }
                if self.searches.len() == REGISTRY_SEARCHES {
                    self.searches.remove(0);
                }
                self.searches.push(RegistrySearch {
                    query: query.into(),
                    hits: hits.clone(),
                });
                hits
            }
        };
        Ok(self.rank(query, hits))
    }
    /// The registry's answer for `query`, before ranking. `None` when no
    /// registry was asked or it gave no usable answer, so nothing is reused.
    fn registry_search(
        &self,
        query: &str,
        cancel: &Cancellation,
    ) -> Result<Option<Vec<RegistryHit>>, EngineError> {
        let limit = REGISTRY_RESULTS.to_string();
        // Cancellation propagates; any other failure (offline, npm not
        // installed) leaves only the installed matches.
        let text = |result: Result<Completion, ExecutionError>| match result {
            Err(ExecutionError::Cancelled) => Err(EngineError::Cancelled),
            result => Ok(result
                .ok()
                .filter(|result| !result.truncated)
                .map(|result| String::from_utf8_lossy(&result.stdout).into_owned())),
        };
        Ok(match self.kind {
            // The npm registry refuses queries outside 2 to 64 characters
            // (after trimming) with an error, after a Node start-up and a
            // round trip, so those are not sent.
            DevKind::Npm | DevKind::Pnpm | DevKind::Bun
                if (2..=64).contains(&query.trim().chars().count()) =>
            {
                let limit = format!("--searchlimit={limit}");
                // Without these caps an unreachable registry holds npm for
                // about 70 seconds of retries; with them it fails at once.
                let args = [
                    "search",
                    "--json",
                    &limit,
                    "--fetch-retries=0",
                    "--fetch-timeout=15000",
                    "--",
                    query,
                ]
                .map(OsString::from);
                text(self.transport.dev_tool("npm", &args, cancel, false))?
                    .and_then(|json| serde_json::from_str::<Vec<NpmSearchHit>>(&json).ok())
                    .map(|hits| {
                        hits.into_iter()
                            .filter_map(|hit| {
                                Some((hit.name, hit.version?, hit.description.unwrap_or_default()))
                            })
                            .collect()
                    })
            }
            DevKind::Cargo => {
                // Offline, cargo retries for about 10 seconds without these.
                text(self.call(
                    &[
                        "--config",
                        "net.retry=0",
                        "--config",
                        "http.timeout=15",
                        "search",
                        "--limit",
                        &limit,
                        "--",
                        query,
                    ],
                    cancel,
                    false,
                ))?
                .map(|output| {
                    output
                        .lines()
                        .filter_map(cargo_search_hit)
                        .map(|(name, version, summary)| {
                            (name.into(), version.into(), summary.into())
                        })
                        .collect()
                })
            }
            // `gem search` takes a regular expression and ignores `--`, so only
            // plain gem names are sent, with their dots escaped.
            DevKind::Gem if gem_searchable(query) => {
                let pattern = query.replace('.', "\\.");
                text(self.call(&["search", "--remote", &pattern], cancel, false))?.map(|output| {
                    output
                        .lines()
                        .filter_map(gem_search_hit)
                        .map(|(name, version)| (name.into(), version.into(), String::new()))
                        .collect()
                })
            }
            // Packagist reports no version; `global require` takes the latest.
            // `global` searches the same COMPOSER_HOME repositories it installs from.
            DevKind::Composer => text(self.call(
                &["global", "search", "--format=json", "--", query],
                cancel,
                false,
            ))?
            .and_then(|json| serde_json::from_str::<Vec<ComposerSearchHit>>(&json).ok())
            .map(|hits| {
                hits.into_iter()
                    .take(REGISTRY_RESULTS)
                    .map(|hit| {
                        (
                            hit.name,
                            "latest".into(),
                            hit.description.unwrap_or_default(),
                        )
                    })
                    .collect()
            }),
            _ => None,
        })
    }
    /// The registry answer in the order searches list it. RubyGems lists
    /// matches alphabetically, so exact and prefix matches go first before
    /// keeping only the first few.
    fn rank(&self, query: &str, mut hits: Vec<RegistryHit>) -> Vec<RegistryHit> {
        if self.kind == DevKind::Gem {
            let lowered = query.to_ascii_lowercase();
            hits.sort_by_key(|(name, ..)| {
                let name = name.to_ascii_lowercase();
                if name == lowered {
                    0
                } else if name.starts_with(&lowered) {
                    1
                } else {
                    2
                }
            });
            hits.truncate(REGISTRY_RESULTS);
        }
        hits
    }
    /// An offer to install exactly `query` by name, for registries PkgDeck
    /// cannot search. It has no version, so searches never list it as a
    /// match; it only lets an exact `install NAME` reach the manager.
    fn exact_offer(&self, home: &std::path::Path, query: &str) -> Option<Package> {
        self.kind.valid_name(query).then(|| Package {
            id: PackageId {
                backend: self.kind.id().into(),
                name: query.into(),
                architecture: std::env::consts::ARCH.into(),
                scope: Scope::Environment {
                    path: home.to_path_buf(),
                },
                remote: None,
                reference: None,
            },
            display_name: query.into(),
            summary: format!("Install {query} with {}", self.kind.id()),
            installed_version: None,
            candidate_version: None,
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        })
    }
    /// Installed tools; `updates` also looks for newer versions, which costs
    /// a registry request per tool for Cargo and Bun and is slow for mise.
    fn inventory(
        &self,
        updates: Updates,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let home = self
            .home
            .clone()
            .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
        match self.kind {
            DevKind::Cargo => self.cargo_inventory(&home, updates, cancel),
            DevKind::Npm => self.npm_inventory(&home, cancel),
            DevKind::Pnpm => self.pnpm_inventory(&home, cancel),
            DevKind::Bun => self.bun_inventory(&home, updates, cancel),
            DevKind::Pip => self.pip_inventory(&home, cancel),
            DevKind::Pipx => self.pipx_inventory(&home, cancel),
            DevKind::Uv => self.uv_inventory(&home, cancel),
            DevKind::Mise => self.mise_inventory(&home, updates != Updates::None, cancel),
            DevKind::Composer => self.composer_inventory(&home, cancel),
            DevKind::Gem => self.gem_inventory(&home, cancel),
        }
    }
    fn upgrade(
        &self,
        target: &PackageId,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        // pnpm and Bun keep pinned ranges, so plain `update` would not move
        // explicitly versioned tools. Reinstalling at `@latest` is also what
        // the reported candidate reflects. Composer re-requires the package
        // for the same reason; pip reinstalls with `--upgrade`.
        let name = self.target(target)?;
        progress(Progress::Message(format!("Updating {name}…")));
        let home = self
            .home
            .clone()
            .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
        let result = match self.kind {
            DevKind::Cargo => self.cargo_reinstall(&home, name, cancel)?,
            DevKind::Npm => self.call(&["update", "--global", name], cancel, true)?,
            DevKind::Pnpm | DevKind::Bun => {
                let latest = format!("{name}@latest");
                self.call(&["add", "--global", &latest], cancel, true)?
            }
            DevKind::Pip => self.pip_call(&home, &["install", "--upgrade", name], cancel, true)?,
            DevKind::Pipx => self.call(&["upgrade", name], cancel, true)?,
            // uv keeps exact version pins, so plain `tool upgrade` would not
            // move explicitly versioned tools. Reinstalling at `@latest` is
            // also what the reported candidate reflects.
            DevKind::Uv => {
                let latest = format!("{name}@latest");
                self.call(&["tool", "install", "--force", &latest], cancel, true)?
            }
            // Within the configured range; `--bump` would rewrite the config.
            DevKind::Mise => self.call(&["upgrade", name], cancel, true)?,
            DevKind::Composer => self.call(
                &[
                    "global",
                    "require",
                    "--no-interaction",
                    "--no-progress",
                    name,
                ],
                cancel,
                true,
            )?,
            DevKind::Gem => self.call(
                &["update", "--user-install", "--no-document", name],
                cancel,
                true,
            )?,
        };
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
    fn upgrade_all(
        &self,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        // Cargo and pip have no registry upgrade-all command; refresh every
        // installed tool in place as the single per-backend transaction.
        if self.kind == DevKind::Npm {
            progress(Progress::Message(format!(
                "Updating {} packages…",
                self.kind.id()
            )));
            let result = self.call(&["update", "--global"], cancel, true)?;
            return Ok(OperationOutcome {
                cancellation_deferred: result.cancellation_deferred,
            });
        }
        if self.kind == DevKind::Pipx {
            progress(Progress::Message("Updating pipx packages…".into()));
            let result = self.call(&["upgrade-all"], cancel, true)?;
            return Ok(OperationOutcome {
                cancellation_deferred: result.cancellation_deferred,
            });
        }
        if self.kind == DevKind::Uv {
            // `tool upgrade --all` keeps exact pins like per-package upgrade,
            // so reinstall each outdated tool at `@latest` as the single
            // transaction.
            return self.each_outdated(cancel, &mut |name| {
                progress(Progress::Package(name.into()));
                progress(Progress::Message(format!("Updating {name} to latest…")));
                let latest = format!("{name}@latest");
                self.call(&["tool", "install", "--force", &latest], cancel, true)?;
                Ok(())
            });
        }
        if self.kind == DevKind::Mise {
            // Plain `mise upgrade` also covers tools of the project in the
            // working directory; name only the global tools. `mise upgrade`
            // finds which are outdated itself, so skip the update check.
            let home = self
                .home
                .clone()
                .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
            let names: Vec<String> = self
                .mise_inventory(&home, false, cancel)?
                .into_iter()
                .map(|package| package.package.id.name)
                .collect();
            if names.is_empty() {
                return Ok(OperationOutcome::default());
            }
            progress(Progress::Message("Updating global mise tools…".into()));
            let mut args = vec!["upgrade"];
            args.extend(names.iter().map(String::as_str));
            let result = self.call(&args, cancel, true)?;
            return Ok(OperationOutcome {
                cancellation_deferred: result.cancellation_deferred,
            });
        }
        if self.kind == DevKind::Composer {
            // `global update` respects pinned constraints, so re-require each
            // outdated package to reach the reported latest candidates.
            return self.each_outdated(cancel, &mut |name| {
                progress(Progress::Package(name.into()));
                progress(Progress::Message(format!("Updating {name}…")));
                self.call(
                    &[
                        "global",
                        "require",
                        "--no-interaction",
                        "--no-progress",
                        name,
                    ],
                    cancel,
                    true,
                )?;
                Ok(())
            });
        }
        if self.kind == DevKind::Gem {
            progress(Progress::Message("Updating RubyGems…".into()));
            let result = self.call(&["update", "--user-install", "--no-document"], cancel, true)?;
            return Ok(OperationOutcome {
                cancellation_deferred: result.cancellation_deferred,
            });
        }
        let home = self
            .home
            .clone()
            .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
        self.each_outdated(cancel, &mut |name| {
            progress(Progress::Package(name.into()));
            match self.kind {
                DevKind::Cargo => {
                    progress(Progress::Message(format!("Reinstalling {name}…")));
                    self.cargo_reinstall(&home, name, cancel)?;
                }
                DevKind::Pip => {
                    progress(Progress::Message(format!("Updating {name}…")));
                    self.pip_call(&home, &["install", "--upgrade", name], cancel, true)?;
                }
                _ => {
                    progress(Progress::Message(format!("Updating {name} to latest…")));
                    let latest = format!("{name}@latest");
                    self.call(&["add", "--global", &latest], cancel, true)?;
                }
            }
            Ok(())
        })
    }
    /// Runs `update` for each tool its registry has a newer version of, and
    /// only those: a tool installed from git or a folder has none, and
    /// reinstalling it by name would fetch whatever the registry has under
    /// that name. One tool failing doesn't stop the others; the first
    /// failure is reported once all were tried.
    fn each_outdated(
        &self,
        cancel: &Cancellation,
        update: &mut dyn FnMut(&str) -> Result<(), EngineError>,
    ) -> Result<OperationOutcome, EngineError> {
        let mut failed = None;
        for detail in self.inventory(Updates::All, cancel)? {
            if detail.package.update != UpdateAvailability::Available {
                continue;
            }
            match update(&detail.package.id.name) {
                Ok(()) => {}
                Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
                Err(error) => {
                    failed.get_or_insert(error);
                }
            }
        }
        failed.map_or(Ok(OperationOutcome::default()), Err)
    }
    /// Reinstalls a crates.io tool at its latest version with the features
    /// it was built with. Tools from git or a folder are refused: Cargo
    /// would look their name up on crates.io, where it can be another crate.
    fn cargo_reinstall(
        &self,
        home: &std::path::Path,
        name: &str,
        cancel: &Cancellation,
    ) -> Result<Completion, EngineError> {
        let id = self.kind.id();
        let list = self.cargo_list(cancel)?;
        let source = list
            .lines()
            .filter_map(|line| line.strip_suffix(':'))
            .find_map(|header| {
                let (tool, rest) = header.split_once(' ')?;
                (tool == name).then(|| rest.split_once(' ').map(|(_, source)| source))
            })
            .ok_or(EngineError::NotFound)?;
        if let Some(source) = source.filter(|&source| {
            !matches!(
                source,
                "(registry+https://github.com/rust-lang/crates.io-index)"
                    | "(registry+sparse+https://index.crates.io/)"
            )
        }) {
            let from = source.trim_start_matches('(').trim_end_matches(')');
            return Err(EngineError::Refused {
                backend: id.into(),
                refusal: Refusal::Explained(format!(
                    "{name} was installed from {from}, not crates.io. Update it the way you installed it."
                )),
            });
        }
        let mut args = vec!["install".to_owned(), "--force".to_owned()];
        args.extend(cargo_features(home, name));
        args.push(name.to_owned());
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        Ok(self.call(&args, cancel, true)?)
    }
}

/// How Cargo's own record (`.crates2.json` in its home) says `name` was
/// built: its features, as `cargo install` flags.
fn cargo_features(home: &std::path::Path, name: &str) -> Vec<String> {
    #[derive(Deserialize)]
    struct Record {
        #[serde(default)]
        installs: std::collections::BTreeMap<String, Install>,
    }
    #[derive(Deserialize)]
    struct Install {
        #[serde(default)]
        features: Vec<String>,
        #[serde(default)]
        all_features: bool,
        #[serde(default)]
        no_default_features: bool,
    }
    let Some(install) = std::fs::read_to_string(host_metadata_path(&home.join(".crates2.json")))
        .ok()
        .and_then(|text| serde_json::from_str::<Record>(&text).ok())
        .and_then(|record| {
            record
                .installs
                .into_iter()
                .find(|(key, _)| key.split(' ').next() == Some(name))
                .map(|(_, install)| install)
        })
    else {
        return vec![];
    };
    let mut flags = vec![];
    if install.all_features {
        flags.push("--all-features".into());
    }
    if install.no_default_features {
        flags.push("--no-default-features".into());
    }
    if !install.features.is_empty() {
        flags.push(format!("--features={}", install.features.join(",")));
    }
    flags
}

impl<T: Transport> Backend for DevTool<T> {
    fn id(&self) -> &str {
        self.kind.id()
    }
    /// Installed tools and install offers both use valid registry names.
    fn may_have(&self, name: &str) -> bool {
        self.kind.valid_name(name)
    }
    /// Exact names never query the open registries. npm, crates.io, RubyGems
    /// and Packagist accept any name, so a hit only proves some package has it,
    /// not that it is the tool meant; `install ripgrep` must not become
    /// ambiguous because npm has an unrelated `ripgrep`. An installed match is
    /// confirmed; otherwise the manager can only try the name. mise's registry
    /// lists the same tools other managers ship, so it is used as usual.
    fn lookup(&mut self, name: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        if self.kind == DevKind::Mise {
            return self.search(name, cancel);
        }
        let home = self
            .home
            .clone()
            .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
        let installed = self
            .inventory(Updates::None, cancel)?
            .into_iter()
            .map(|d| d.package)
            .find(|package| package.id.name == name);
        Ok(installed
            .or_else(|| self.exact_offer(&home, name))
            .into_iter()
            .collect())
    }
    fn capabilities(&self) -> &[Capability] {
        if matches!(self.kind, DevKind::Npm | DevKind::Pip | DevKind::Uv) {
            cleanup::DEV_CLEAN_CAPABILITIES
        } else {
            DEV_CAPABILITIES
        }
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        let home = match self.home(cancel) {
            Ok(home) => home,
            Err(ExecutionError::Disabled(reason)) => {
                return Ok(Availability::Unavailable(reason));
            }
            Err(error) => return Err(error.into()),
        };
        self.home = Some(home.clone());
        if matches!(self.kind, DevKind::Npm | DevKind::Pnpm) {
            // `root --global` above already ran the manager; a second Node
            // start-up for `--version` would prove nothing more.
            return Ok(Availability::Available);
        }
        if self.kind == DevKind::Pip {
            availability(self.transport.venv_pip(
                &home,
                &[OsString::from("--version")],
                cancel,
                false,
            ))
        } else {
            availability(self.call(&["--version"], cancel, false))
        }
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .inventory(Updates::All, cancel)?
            .into_iter()
            .map(|d| d.package)
            .collect())
    }
    /// Search filters installed tools by name. A valid query with no
    /// installed match resolves to a synthetic installable candidate so
    /// selection, confirmation, and installation keep working uniformly;
    /// details remain available only for installed tools.
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let home = self
            .home
            .clone()
            .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
        let lowered = query.to_ascii_lowercase();
        // Update checks for Cargo, Bun and mise reach the network for every
        // tool, so searches list installed tools without them.
        let inventory: Vec<Package> = self
            .inventory(Updates::None, cancel)?
            .into_iter()
            .map(|d| d.package)
            .collect();
        let mut results: Vec<Package> = inventory
            .iter()
            .filter(|package| {
                package.id.name.to_ascii_lowercase().contains(&lowered)
                    || package.display_name.to_ascii_lowercase().contains(&lowered)
            })
            .cloned()
            .collect();
        if self.kind == DevKind::Mise {
            // Registry matches can find an installed tool through an alias
            // (`op` for `1password`); those come back as the installed row.
            for package in self.mise_offers(&home, query, &inventory, cancel)? {
                if !results.iter().any(|found| found.id == package.id) {
                    results.push(package);
                }
            }
            // A backend-qualified tool (`cargo:ripgrep`) is not a registry
            // entry, so keep the exact name installable.
            if !results.iter().any(|package| package.id.name == query) {
                if let Some(offer) = self.exact_offer(&home, query) {
                    results.push(offer);
                }
            }
            return Ok(results);
        }
        for (name, version, summary) in self.cached_registry_hits(query, cancel)? {
            if !self.kind.valid_name(&name) || results.iter().any(|package| package.id.name == name)
            {
                continue;
            }
            if let Some(installed) = inventory.iter().find(|package| package.id.name == name) {
                results.push(installed.clone());
                continue;
            }
            results.push(Package {
                id: PackageId {
                    backend: self.kind.id().into(),
                    name: name.clone(),
                    architecture: std::env::consts::ARCH.into(),
                    scope: Scope::Environment { path: home.clone() },
                    remote: None,
                    reference: None,
                },
                display_name: name,
                summary,
                installed_version: None,
                candidate_version: Some(version),
                update: UpdateAvailability::Unknown,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
                adopt_with: None,
            });
        }
        // PyPI cannot be searched, and npm search ranks product names poorly,
        // so also offer the exact packages of known AI command-line tools that
        // match the query. One already installed comes back as its installed row.
        for tool in ai_catalog::matching(query) {
            let package = match self.kind {
                DevKind::Npm | DevKind::Pnpm | DevKind::Bun => tool.npm,
                DevKind::Pipx | DevKind::Uv => tool.pypi,
                _ => None,
            };
            let Some(name) = package.filter(|name| self.kind.valid_name(name)) else {
                continue;
            };
            if results.iter().any(|package| package.id.name == name) {
                continue;
            }
            if let Some(installed) = inventory.iter().find(|package| package.id.name == name) {
                results.push(installed.clone());
                continue;
            }
            results.push(Package {
                id: PackageId {
                    backend: self.kind.id().into(),
                    name: name.into(),
                    architecture: std::env::consts::ARCH.into(),
                    scope: Scope::Environment { path: home.clone() },
                    remote: None,
                    reference: None,
                },
                display_name: tool.product.into(),
                summary: format!("{} (AI command-line tool)", tool.product),
                installed_version: None,
                // Installing without a version takes the registry's latest.
                candidate_version: Some("latest".into()),
                update: UpdateAvailability::Unknown,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
                adopt_with: None,
            });
        }
        // A catalog alias must not hide the exact name the user typed:
        // `install amp --from npm` still tries the npm package `amp`.
        if !results.iter().any(|package| package.id.name == query) {
            results.extend(self.exact_offer(&home, query));
        }
        Ok(results)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        self.target(id)?;
        let find =
            |inventory: Vec<PackageDetails>| inventory.into_iter().find(|d| d.package.id == *id);
        if let Some(listed) = find(self.inventory(Updates::None, cancel)?) {
            // Cargo, Bun and mise check updates per tool, slowly or over the
            // network, so only an installed tool gets one, just for itself.
            // The other managers' listings already include updates.
            if !matches!(self.kind, DevKind::Cargo | DevKind::Bun | DevKind::Mise) {
                return Ok(listed);
            }
            return Ok(find(self.inventory(Updates::Only(&id.name), cancel)?).unwrap_or(listed));
        }
        // A search result that is not installed yet (a registry match) still
        // opens: find the same identity again. Unversioned exact-name offers
        // are not results, so they have no details.
        self.search(&id.name, cancel)?
            .into_iter()
            .find(|package| package.id == *id && package.candidate_version.is_some())
            .map(|package| PackageDetails {
                description: package.summary.clone(),
                homepage: package.homepages.first().cloned(),
                dependencies: vec![],
                package,
            })
            .ok_or(EngineError::NotFound)
    }
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        self.cleanup_cache(cancel)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        // Installed state is read again on every search, but a change can
        // follow a registry update, so later searches ask again.
        self.searches.clear();
        let id = self.kind.id();
        if operation.backend() != id {
            return Err(invalid(id, "foreign operation"));
        }
        let args: Vec<&str> = match operation {
            Operation::Refresh { .. } => return Err(self.unsupported(Capability::Refresh)),
            Operation::Install(target) => match self.kind {
                DevKind::Cargo => vec!["install", self.target(target)?],
                DevKind::Npm => vec!["install", "--global", self.target(target)?],
                DevKind::Pnpm | DevKind::Bun => vec!["add", "--global", self.target(target)?],
                DevKind::Pipx => vec!["install", self.target(target)?],
                DevKind::Uv => vec!["tool", "install", self.target(target)?],
                // Adds the tool to the global configuration at its latest version.
                DevKind::Mise => vec!["use", "--global", self.target(target)?],
                DevKind::Composer => vec![
                    "global",
                    "require",
                    "--no-interaction",
                    "--no-progress",
                    self.target(target)?,
                ],
                DevKind::Gem => vec![
                    "install",
                    "--user-install",
                    "--no-document",
                    self.target(target)?,
                ],
                DevKind::Pip => {
                    let name = self.target(target)?;
                    let home = self
                        .home
                        .clone()
                        .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
                    progress(Progress::Message(format!(
                        "Running {id}. If you cancel, PkgDeck waits for it to finish."
                    )));
                    let result = self.pip_call(&home, &["install", name], cancel, true)?;
                    return Ok(OperationOutcome {
                        cancellation_deferred: result.cancellation_deferred,
                    });
                }
            },
            Operation::Remove(target) => match self.kind {
                DevKind::Cargo => vec!["uninstall", self.target(target)?],
                DevKind::Npm => vec!["uninstall", "--global", self.target(target)?],
                DevKind::Pnpm | DevKind::Bun => vec!["remove", "--global", self.target(target)?],
                DevKind::Pipx => vec!["uninstall", self.target(target)?],
                DevKind::Uv => vec!["tool", "uninstall", self.target(target)?],
                // Drop the global request, then delete the versions no tracked
                // project config still needs; `unuse` alone keeps them.
                DevKind::Mise => {
                    let name = self.target(target)?;
                    progress(Progress::Message(format!(
                        "Running {id}. If you cancel, PkgDeck waits for it to finish."
                    )));
                    let unused = self.call(&["unuse", "--global", name], cancel, true)?;
                    // Cancelled while `unuse` ran: it finished, so stop there
                    // and report the deferral rather than a cancelled prune.
                    if unused.cancellation_deferred {
                        return Ok(OperationOutcome {
                            cancellation_deferred: true,
                        });
                    }
                    let result = self.call(&["prune", "--yes", name], cancel, true)?;
                    return Ok(OperationOutcome {
                        cancellation_deferred: result.cancellation_deferred,
                    });
                }
                DevKind::Composer => vec!["global", "remove", self.target(target)?],
                DevKind::Gem => vec![
                    "uninstall",
                    "--user-install",
                    "-x",
                    "-a",
                    self.target(target)?,
                ],
                DevKind::Pip => {
                    let name = self.target(target)?;
                    let home = self
                        .home
                        .clone()
                        .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
                    progress(Progress::Message(format!(
                        "Running {id}. If you cancel, PkgDeck waits for it to finish."
                    )));
                    let result = self.pip_call(&home, &["uninstall", "-y", name], cancel, true)?;
                    return Ok(OperationOutcome {
                        cancellation_deferred: result.cancellation_deferred,
                    });
                }
            },
            Operation::Upgrade(target) => {
                return self.upgrade(target, cancel, progress);
            }
            Operation::UpgradeAll { .. } => {
                return self.upgrade_all(cancel, progress);
            }
            Operation::Clean(target) => return self.clean_cache(target, cancel),
        };
        progress(Progress::Message(format!(
            "Running {id}. If you cancel, PkgDeck waits for it to finish."
        )));
        let result = self.call(&args, cancel, true)?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}
