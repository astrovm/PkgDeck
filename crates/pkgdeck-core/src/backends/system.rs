//! DNF, pacman, zypper, Snap, apk and XBPS: system managers that share one adapter.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ManagerKind {
    Dnf,
    Pacman,
    Zypper,
    Snap,
    Apk,
    Xbps,
    MacPorts,
}

impl ManagerKind {
    fn id(self) -> &'static str {
        match self {
            Self::Dnf => "dnf",
            Self::Pacman => "pacman",
            Self::Zypper => "zypper",
            Self::Snap => "snap",
            Self::Apk => "apk",
            Self::Xbps => "xbps",
            Self::MacPorts => "macports",
        }
    }
    fn executable(self) -> &'static str {
        match self {
            Self::Xbps => "xbps-query",
            Self::MacPorts => "port",
            _ => self.id(),
        }
    }
    /// XBPS splits writes across two commands.
    fn write_executable(self, operation: &Operation) -> &'static str {
        match (self, operation) {
            (Self::Xbps, Operation::Remove(_)) => "xbps-remove",
            (Self::Xbps, _) => "xbps-install",
            _ => self.executable(),
        }
    }
    /// Where the manager lists pending updates, without writing.
    fn update_args(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Self::Snap => ("snap", &["refresh", "--list"]),
            Self::Apk => ("apk", &["list", "--upgradable"]),
            // -C reads the cache a check just refreshed; without one, it's
            // asked again online.
            Self::Dnf => (
                "dnf",
                &[
                    "-C",
                    "--quiet",
                    "repoquery",
                    "--upgrades",
                    "--latest-limit",
                    "1",
                    "--queryformat",
                    "%{name}|%{arch}|%{version}-%{release}\n",
                ],
            ),
            Self::Pacman => ("pacman", &["-Qu"]),
            Self::Zypper => (
                "zypper",
                &["--quiet", "--xmlout", "--non-interactive", "list-updates"],
            ),
            // -M reads the repository index into memory; -n only reports.
            Self::Xbps => ("xbps-install", &["-Mun"]),
            Self::MacPorts => ("port", &["-q", "outdated"]),
        }
    }
    fn read_args(self, installed: bool, query: &str) -> Vec<OsString> {
        match (self, installed) {
            (Self::Dnf, true) => vec![
                "--quiet",
                "repoquery",
                "--latest-limit",
                "1",
                "--installed",
                "--queryformat",
                "%{name}|%{arch}|%{version}-%{release}|%{summary}\n",
            ],
            // -C reads the cached metadata instead of checking every
            // repository online first; a missing cache is retried without.
            // A bare name only matches that exact package, so the glob
            // finds names containing the query, ignoring case. Valid
            // queries never hold glob characters.
            (Self::Dnf, false) => {
                return [
                    "-C",
                    "--quiet",
                    "repoquery",
                    "--latest-limit",
                    "1",
                    "--queryformat",
                    "%{name}|%{arch}|%{version}-%{release}|%{summary}\n",
                ]
                .into_iter()
                .map(OsString::from)
                .chain([OsString::from(format!("*{query}*"))])
                .collect();
            }
            (Self::Pacman, true) => vec!["-Q"],
            (Self::Pacman, false) => vec!["-Ss", query],
            (Self::Zypper, true) => vec![
                "--xmlout",
                "search",
                "--installed-only",
                "--details",
                "--type",
                "package",
            ],
            (Self::Zypper, false) => vec![
                "--xmlout",
                "search",
                "--details",
                "--type",
                "package",
                query,
            ],
            (Self::Snap, true) => vec!["list"],
            (Self::Snap, false) => vec!["find", query],
            (Self::Apk, true) => vec!["list", "--installed"],
            (Self::Apk, false) => vec!["search", "-v", query],
            (Self::Xbps, true) => vec!["-l"],
            (Self::Xbps, false) => vec!["-Rs", query],
            (Self::MacPorts, true) => vec!["-q", "installed"],
            // Quiet mode would print only names; --line gives tab-separated
            // name, version, categories and description.
            (Self::MacPorts, false) => vec!["search", "--name", "--line", query],
        }
        .into_iter()
        .map(OsString::from)
        .collect()
    }
    /// The fixed arguments before the package name; `None` for cleanup,
    /// which these managers do not offer.
    fn write_args(self, operation: &Operation) -> Option<Vec<&'static str>> {
        Some(match (self, operation) {
            (Self::Dnf, Operation::Refresh { .. }) => vec!["-y", "makecache"],
            (Self::Dnf, Operation::Install(_)) => vec!["-y", "install", "--"],
            (Self::Dnf, Operation::Remove(_)) => vec!["-y", "remove", "--"],
            // A check refreshes only the user's own copy of each index, so
            // an update refreshes the system's first.
            (Self::Dnf, Operation::Upgrade(_)) => vec!["-y", "--refresh", "upgrade", "--"],
            (Self::Dnf, Operation::UpgradeAll { .. }) => vec!["-y", "--refresh", "upgrade"],
            (Self::Pacman, Operation::Refresh { .. }) => vec!["-Sy", "--noconfirm"],
            (Self::Pacman, Operation::Install(_)) => {
                vec!["-S", "--noconfirm", "--needed", "--"]
            }
            (Self::Pacman, Operation::Remove(_)) => vec!["-Rns", "--noconfirm", "--"],
            (Self::Pacman, Operation::Upgrade(_)) => {
                vec!["-Sy", "--noconfirm", "--needed", "--"]
            }
            (Self::Pacman, Operation::UpgradeAll { .. }) => vec!["-Syu", "--noconfirm"],
            (Self::Zypper, Operation::Refresh { .. }) => vec!["--non-interactive", "refresh"],
            (Self::Zypper, Operation::Install(_)) => vec![
                "--non-interactive",
                "install",
                "--auto-agree-with-licenses",
                "--",
            ],
            (Self::Zypper, Operation::Remove(_)) => vec!["--non-interactive", "remove", "--"],
            (Self::Zypper, Operation::Upgrade(_)) => vec!["--non-interactive", "update", "--"],
            (Self::Zypper, Operation::UpgradeAll { .. }) => vec!["--non-interactive", "update"],
            (Self::Snap, Operation::Install(_)) => vec!["install"],
            (Self::Snap, Operation::Remove(_)) => vec!["remove"],
            (Self::Snap, Operation::Upgrade(_)) => vec!["refresh"],
            (Self::Snap, Operation::UpgradeAll { .. }) => vec!["refresh"],
            (Self::Apk, Operation::Refresh { .. }) => vec!["update"],
            (Self::Apk, Operation::Install(_)) => vec!["add", "--"],
            (Self::Apk, Operation::Remove(_)) => vec!["del", "--"],
            (Self::Apk, Operation::Upgrade(_)) => vec!["upgrade", "--update-cache", "--"],
            (Self::Apk, Operation::UpgradeAll { .. }) => vec!["upgrade", "--update-cache"],
            (Self::Xbps, Operation::Refresh { .. }) => vec!["-S"],
            (Self::Xbps, Operation::Install(_) | Operation::Remove(_)) => vec!["-y"],
            (Self::Xbps, Operation::Upgrade(_) | Operation::UpgradeAll { .. }) => vec!["-Syu"],
            // -N never asks; MacPorts keeps each port's variants on upgrade.
            (Self::MacPorts, Operation::Refresh { .. }) => vec!["-N", "selfupdate"],
            (Self::MacPorts, Operation::Install(_)) => vec!["-N", "install"],
            (Self::MacPorts, Operation::Remove(_)) => vec!["-N", "uninstall"],
            (Self::MacPorts, Operation::Upgrade(_)) => vec!["-N", "upgrade"],
            (Self::MacPorts, Operation::UpgradeAll { .. }) => vec!["-N", "upgrade", "outdated"],
            // Snap's refresh is read-only and handled in `execute`.
            (Self::Snap, Operation::Refresh { .. }) | (_, Operation::Clean(_)) => return None,
        })
    }
}

/// Package updates from `zypper --xmlout list-updates`, by name. Patches
/// and products are left out; their packages are listed too.
fn zypper_updates(xml: &str) -> BTreeMap<String, String> {
    xml.split("<update ")
        .skip(1)
        .filter_map(|entry| {
            let tag = entry.split_once('>').map_or(entry, |(tag, _)| tag);
            let attr = |key: &str| {
                tag.split_once(&format!(" {key}=\""))
                    .or_else(|| {
                        tag.strip_prefix(&format!("{key}=\""))
                            .map(|tail| ("", tail))
                    })
                    .and_then(|(_, tail)| tail.split_once('"'))
                    .map(|(value, _)| value)
            };
            (attr("kind")? == "package")
                .then(|| Some((attr("name")?.to_owned(), attr("edition")?.to_owned())))
                .flatten()
                .filter(|(name, edition)| !name.is_empty() && !edition.is_empty())
        })
        .collect()
}

/// `name-1.2.3-r0` into (`name`, `1.2.3-r0`).
fn apk_pkgver(pkgver: &str) -> Option<(&str, String)> {
    let mut parts = pkgver.rsplitn(3, '-');
    let (release, version, name) = (parts.next()?, parts.next()?, parts.next()?);
    (release.starts_with('r')
        && version.starts_with(|c: char| c.is_ascii_digit())
        && !name.is_empty())
    .then(|| (name, format!("{version}-{release}")))
}
/// `name-1.2.3_1` into (`name`, `1.2.3_1`).
fn xbps_pkgver(pkgver: &str) -> Option<(&str, String)> {
    let (name, version) = pkgver.rsplit_once('-')?;
    (version.contains('_') && !name.is_empty()).then(|| (name, version.to_owned()))
}

/// The snaps and revisions an old-revisions task removes, from its key:
/// `disabled-revisions:firefox=9113,core22=2955`. `None` for any other key,
/// or one with a name or revision Snap wouldn't make.
pub fn snap_revisions(key: &str) -> Option<Vec<(String, String)>> {
    let listed = key.strip_prefix("disabled-revisions:")?;
    let name = |name: &str| {
        !name.is_empty()
            && !name.starts_with('-')
            && name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    };
    let revision = |revision: &str| {
        let digits = revision.strip_prefix('x').unwrap_or(revision);
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    };
    listed
        .split(',')
        .map(|pair| {
            let (snap, number) = pair.split_once('=')?;
            (name(snap) && revision(number)).then(|| (snap.to_owned(), number.to_owned()))
        })
        .collect::<Option<Vec<_>>>()
        .filter(|revisions| !revisions.is_empty())
}

/// Revisions `snap list --all` marks disabled, with their versions: what
/// Snap keeps to go back to after an update.
/// Every installed snap by name, from `snap list --all`.
fn snap_names(listing: &str) -> std::collections::BTreeMap<String, String> {
    listing
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .map(|name| (name.to_owned(), name.to_owned()))
        .collect()
}

fn disabled_revisions(listing: &str) -> Vec<(String, String, String)> {
    listing
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            let [name, version, revision, .., notes] = fields.as_slice() else {
                return None;
            };
            notes.split(',').any(|note| note == "disabled").then(|| {
                (
                    (*name).to_owned(),
                    (*revision).to_owned(),
                    (*version).to_owned(),
                )
            })
        })
        .collect()
}

pub struct SystemManager<T = NativeTransport> {
    kind: ManagerKind,
    transport: T,
    /// Rows the last exact lookup read. Details are a search of the same
    /// name, so the details that follow it (`pkd info`) come from here.
    /// Read once; changes clear it.
    looked_up: Vec<Package>,
    /// Set during an update check, which refreshes the lists first.
    update_check: Option<u64>,
}
pub type Dnf<T = NativeTransport> = SystemManager<T>;
pub type Pacman<T = NativeTransport> = SystemManager<T>;
pub type Zypper<T = NativeTransport> = SystemManager<T>;
pub type Snap<T = NativeTransport> = SystemManager<T>;
impl<T: Transport> SystemManager<T> {
    fn new(kind: ManagerKind, transport: T) -> Self {
        Self {
            kind,
            transport,
            looked_up: vec![],
            update_check: None,
        }
    }
    pub fn dnf(transport: T) -> Self {
        Self::new(ManagerKind::Dnf, transport)
    }
    pub fn pacman(transport: T) -> Self {
        Self::new(ManagerKind::Pacman, transport)
    }
    pub fn zypper(transport: T) -> Self {
        Self::new(ManagerKind::Zypper, transport)
    }
    pub fn snap(transport: T) -> Self {
        Self::new(ManagerKind::Snap, transport)
    }
    pub fn apk(transport: T) -> Self {
        Self::new(ManagerKind::Apk, transport)
    }
    pub fn xbps(transport: T) -> Self {
        Self::new(ManagerKind::Xbps, transport)
    }
    pub fn macports(transport: T) -> Self {
        Self::new(ManagerKind::MacPorts, transport)
    }
    fn call(
        &self,
        args: Vec<OsString>,
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, EngineError> {
        self.call_with(self.kind.executable(), args, cancel, write)
    }
    fn call_with(
        &self,
        executable: &str,
        args: Vec<OsString>,
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, EngineError> {
        Ok(self
            .transport
            .system_manager(executable, &args, cancel, write)?)
    }
    /// Pending updates by package name. An unavailable listing leaves every
    /// row's update status as it was rather than failing the inventory.
    pub(super) fn updates(
        &self,
        cancel: &Cancellation,
    ) -> Result<BTreeMap<String, String>, EngineError> {
        let (executable, args) = self.kind.update_args();
        let mut args: Vec<OsString> = args.iter().map(OsString::from).collect();
        // A check's own copy of the index, while it's newer.
        if let Some(dir) = self.transport.private_index(self.kind.id()) {
            let dir = dir.into_os_string();
            match self.kind {
                ManagerKind::Pacman => args.extend(["--dbpath".into(), dir]),
                ManagerKind::Zypper => {
                    args.splice(0..0, ["--cache-dir".into(), dir, "--no-refresh".into()]);
                }
                ManagerKind::Apk => {
                    args.splice(0..0, ["--cache-dir".into(), dir]);
                }
                _ => {}
            }
        }
        let result = match self.call_with(executable, args.clone(), cancel, false) {
            Err(EngineError::Execution(ExecutionError::Failed(_))) if args[0] == "-C" => {
                self.call_with(executable, args[1..].to_vec(), cancel, false)
            }
            result => result,
        };
        let text = match result {
            Ok(done) => String::from_utf8_lossy(&bytes(self.kind.id(), done)?).into_owned(),
            Err(EngineError::Cancelled | EngineError::Execution(ExecutionError::Cancelled)) => {
                return Err(EngineError::Cancelled)
            }
            Err(error) if matches!(self.kind, ManagerKind::Snap) => return Err(error),
            Err(_) => return Ok(BTreeMap::new()),
        };
        if matches!(self.kind, ManagerKind::Zypper) {
            return Ok(zypper_updates(&text));
        }
        Ok(text
            .lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                match self.kind {
                    ManagerKind::Dnf => {
                        let fields: Vec<&str> = line.split('|').collect();
                        match fields[..] {
                            [name, _, version] if !version.is_empty() => {
                                Some((name, version.to_owned()))
                            }
                            _ => None,
                        }
                    }
                    // "name 1.0-1 -> 1.1-1", with " [ignored]" after the
                    // ones pacman.conf holds back: an upgrade skips those.
                    ManagerKind::Pacman if fields.len() == 4 && fields[2] == "->" => {
                        Some((fields[0], fields[3].to_owned()))
                    }
                    ManagerKind::Snap => Some((*fields.first()?, (*fields.get(1)?).to_owned())),
                    ManagerKind::Apk => apk_pkgver(fields.first()?),
                    ManagerKind::Xbps if fields.get(1) == Some(&"update") => {
                        xbps_pkgver(fields.first()?)
                    }
                    ManagerKind::MacPorts if fields.get(2) == Some(&"<") => {
                        Some((*fields.first()?, (*fields.get(3)?).to_owned()))
                    }
                    _ => None,
                }
                .map(|(name, version)| (name.to_owned(), version))
            })
            .collect())
    }
    fn valid_name(&self, name: &str) -> bool {
        !name.is_empty()
            && !name.starts_with('-')
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+@-".contains(&b))
    }
    fn package(
        &self,
        name: &str,
        arch: &str,
        version: &str,
        summary: &str,
    ) -> Result<Package, EngineError> {
        if !self.valid_name(name) || arch.is_empty() || version.is_empty() {
            return Err(invalid(self.kind.id(), "invalid package metadata"));
        }
        Ok(Package {
            id: PackageId {
                backend: self.kind.id().into(),
                name: name.into(),
                architecture: arch.into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: name.into(),
            summary: summary.into(),
            installed_version: None,
            candidate_version: Some(version.into()),
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        })
    }
    fn parse(&self, value: Vec<u8>, installed: bool) -> Result<Vec<Package>, EngineError> {
        let text = String::from_utf8(value).map_err(|e| invalid(self.kind.id(), e))?;
        let mut packages = Vec::new();
        match self.kind {
            ManagerKind::Dnf => {
                for line in text.lines().filter(|line| !line.is_empty()) {
                    let fields: Vec<_> = line.splitn(4, '|').collect();
                    if fields.len() != 4 {
                        return Err(invalid("dnf", "invalid repoquery metadata"));
                    }
                    let mut package = self.package(fields[0], fields[1], fields[2], fields[3])?;
                    if installed {
                        package.installed_version = Some(fields[2].into());
                        package.update = UpdateAvailability::Current;
                    }
                    packages.push(package);
                }
            }
            ManagerKind::Pacman => {
                let mut lines = text.lines().peekable();
                while let Some(line) = lines.next() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let header = line.split_whitespace().collect::<Vec<_>>();
                    let (name, version) = if installed {
                        (header.first().copied(), header.get(1).copied())
                    } else {
                        (
                            header
                                .first()
                                .and_then(|n| n.split_once('/').map(|(_, n)| n)),
                            header.get(1).copied(),
                        )
                    };
                    let (Some(name), Some(version)) = (name, version) else {
                        return Err(invalid("pacman", "invalid package metadata"));
                    };
                    let summary = if installed {
                        "Installed package"
                    } else {
                        lines.next().map(str::trim).unwrap_or("")
                    };
                    let mut package =
                        self.package(name, std::env::consts::ARCH, version, summary)?;
                    if installed {
                        package.installed_version = Some(version.into());
                        package.update = UpdateAvailability::Current;
                    } else {
                        // Search marks installed packages "[installed]", or
                        // "[installed: 1.0-1]" when another version is.
                        package.installed_version = line
                            .rsplit_once(" [installed")
                            .and_then(|(_, tail)| tail.strip_suffix(']'))
                            .map(|tail| tail.strip_prefix(": ").unwrap_or(version).into());
                    }
                    packages.push(package);
                }
            }
            ManagerKind::Zypper => {
                for entry in text.split("<solvable ").skip(1) {
                    let attr = |key: &str| {
                        entry
                            .split_once(&format!("{key}=\""))
                            .and_then(|(_, tail)| tail.split_once('"'))
                            .map(|(value, _)| value)
                    };
                    let (Some(name), Some(version), Some(arch)) =
                        (attr("name"), attr("edition"), attr("arch"))
                    else {
                        return Err(invalid("zypper", "invalid XML metadata"));
                    };
                    let mut package =
                        self.package(name, arch, version, attr("summary").unwrap_or(""))?;
                    if installed {
                        package.installed_version = Some(version.into());
                        package.update = UpdateAvailability::Current;
                    }
                    packages.push(package);
                }
            }
            ManagerKind::Apk | ManagerKind::Xbps | ManagerKind::MacPorts => {
                for line in text.lines().filter(|line| !line.trim().is_empty()) {
                    let Some((name, version, arch, summary)) = self.parse_line(line, installed)
                    else {
                        continue;
                    };
                    let mut package = self.package(name, &arch, &version, &summary)?;
                    if installed {
                        package.installed_version = Some(version);
                        package.update = UpdateAvailability::Current;
                    }
                    packages.push(package);
                }
            }
            ManagerKind::Snap => {
                for line in text.lines().skip(1).filter(|line| !line.trim().is_empty()) {
                    let fields = line.split_whitespace().collect::<Vec<_>>();
                    if fields.len() < 2 {
                        return Err(invalid("snap", "invalid list metadata"));
                    }
                    // `snap find` prints Name Version Publisher Notes
                    // Summary; the store summary is what matched the query,
                    // so keep it for ranking instead of a placeholder.
                    // `snap list` has no summary column.
                    let summary: String = if installed {
                        "Snap package".into()
                    } else {
                        fields
                            .get(4..)
                            .map(|tail| tail.join(" "))
                            .unwrap_or_default()
                    };
                    let mut package =
                        self.package(fields[0], std::env::consts::ARCH, fields[1], &summary)?;
                    if installed {
                        package.installed_version = Some(fields[1].into());
                        package.update = UpdateAvailability::Current;
                    }
                    packages.push(package);
                }
            }
        }
        Ok(packages)
    }
    /// One apk, XBPS or MacPorts listing line: name, version, architecture
    /// and summary. Lines that are not package entries return `None`.
    fn parse_line<'a>(
        &self,
        line: &'a str,
        installed: bool,
    ) -> Option<(&'a str, String, String, String)> {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let host = std::env::consts::ARCH.to_owned();
        match (self.kind, installed) {
            // name-1.2.3-r0 aarch64 {origin} (license) [installed]
            (ManagerKind::Apk, true) => {
                let (name, version) = apk_pkgver(fields.first()?)?;
                Some((
                    name,
                    version,
                    fields.get(1).map_or(host, |a| (*a).to_owned()),
                    String::new(),
                ))
            }
            // name-1.2.3-r0 - description
            (ManagerKind::Apk, false) => {
                let (pkgver, summary) = line.split_once(" - ").unwrap_or((line, ""));
                let (name, version) = apk_pkgver(pkgver.trim())?;
                Some((name, version, host, summary.trim().to_owned()))
            }
            // ii name-1.2.3_1   description
            (ManagerKind::Xbps, true) => {
                let (name, version) = xbps_pkgver(fields.get(1)?)?;
                Some((name, version, host, fields.get(2..)?.join(" ")))
            }
            // [-] name-1.2.3_1  description ([*] when installed)
            (ManagerKind::Xbps, false) => {
                let (name, version) = xbps_pkgver(fields.get(1)?)?;
                Some((name, version, host, fields.get(2..)?.join(" ")))
            }
            // "  name @1.2.3_0+variant (active)"; inactive versions stay
            // installed but are not what runs, so only active ones are rows.
            (ManagerKind::MacPorts, true) => {
                if !line.contains("(active)") {
                    return None;
                }
                let version = fields.get(1)?.strip_prefix('@')?;
                Some((
                    fields.first()?,
                    version.to_owned(),
                    host,
                    "MacPorts port".into(),
                ))
            }
            // MacPorts search, the remaining kind parsed here:
            // name <tab> version <tab> categories <tab> description
            _ => {
                let columns: Vec<&str> = line.split('\t').map(str::trim).collect();
                let [name, version, _, summary, ..] = columns[..] else {
                    // Not a result line, such as "No match for … found".
                    return None;
                };
                Some((
                    name,
                    version.trim_start_matches('@').to_owned(),
                    host,
                    summary.to_owned(),
                ))
            }
        }
    }
    fn query(
        &self,
        installed: bool,
        query: &str,
        cancel: &Cancellation,
    ) -> Result<Vec<Package>, EngineError> {
        // Several words ("balena etcher") can't go to the manager as one
        // name: ask for the longest word, then keep what matches them all.
        let words: Vec<&str> = query
            .split(SEARCH_SEPARATORS)
            .filter(|word| !word.is_empty())
            .collect();
        let narrowed = !installed && !self.valid_name(query);
        let term = if narrowed {
            // Text that starts like an option is never a search.
            if query.starts_with('-')
                || words.is_empty()
                || !words.iter().all(|word| self.valid_name(word))
            {
                return Err(invalid(self.kind.id(), "invalid package query"));
            }
            words
                .iter()
                .max_by_key(|word| word.len())
                .copied()
                .unwrap_or(query)
        } else {
            query
        };
        let mut packages = self.query_term(installed, term, cancel)?;
        if narrowed {
            packages.retain(|package| {
                search_matches(&format!("{} {}", package.id.name, package.summary), query)
            });
        }
        Ok(packages)
    }
    fn query_term(
        &self,
        installed: bool,
        query: &str,
        cancel: &Cancellation,
    ) -> Result<Vec<Package>, EngineError> {
        let args = self.kind.read_args(installed, query);
        let output = match self.call(args.clone(), cancel, false) {
            // Without cached metadata (never refreshed, or cleaned), DNF's
            // cache-only mode fails; ask again and let it fetch.
            Err(EngineError::Execution(ExecutionError::Failed(_)))
                if args.first().is_some_and(|arg| arg == "-C") =>
            {
                self.call(args[1..].to_vec(), cancel, false)
            }
            // pacman exits 1 when nothing matches.
            Err(EngineError::Execution(ExecutionError::Failed(result)))
                if matches!(self.kind, ManagerKind::Pacman)
                    && result.code == Some(1)
                    && result.stdout.is_empty() =>
            {
                return Ok(vec![]);
            }
            result => result,
        };
        self.parse(bytes(self.kind.id(), output?)?, installed)
    }
    /// Drops packages no repository has (AUR builds): they are the AUR
    /// source's rows. Without synced databases every package looks foreign;
    /// then Pacman keeps them all.
    fn drop_foreign(
        &self,
        packages: &mut Vec<Package>,
        cancel: &Cancellation,
    ) -> Result<(), EngineError> {
        let foreign = match self.call(vec!["-Qmq".into()], cancel, false) {
            // pacman -Q exits 1 when nothing matches: no foreign packages.
            Err(EngineError::Execution(ExecutionError::Failed(result)))
                if result.code == Some(1) && result.stdout.is_empty() =>
            {
                String::new()
            }
            result => {
                String::from_utf8(bytes("pacman", result?)?).map_err(|e| invalid("pacman", e))?
            }
        };
        let foreign: std::collections::BTreeSet<&str> = foreign.lines().map(str::trim).collect();
        if foreign.len() < packages.len() {
            packages.retain(|package| !foreign.contains(package.id.name.as_str()));
        }
        Ok(())
    }
    fn target(&self, id: &PackageId) -> Result<String, EngineError> {
        if id.backend != self.kind.id() || id.scope != Scope::System || !self.valid_name(&id.name) {
            return Err(invalid(
                self.kind.id(),
                "foreign or invalid package identity",
            ));
        }
        Ok(id.name.clone())
    }
    /// Local snap icons resolve straight from the filesystem, which doubles
    /// as the installed check: remote rows only match a file when an
    /// installed snap shares the name, which is the same application.
    fn snap_listing(&self, cancel: &Cancellation) -> Result<String, EngineError> {
        String::from_utf8(bytes(
            "snap",
            self.call(vec!["list".into(), "--all".into()], cancel, false)?,
        )?)
        .map_err(|error| invalid("snap", error))
    }
    /// Where snaps keep their data: `~/snap`, once the home is known.
    fn data_folders(&self) -> Option<app_data::DataFolders> {
        (self.kind == ManagerKind::Snap)
            .then(|| app_data::home(&self.transport))
            .flatten()
            .map(|home| app_data::DataFolders::snap(&home))
    }
    /// The revisions Snap keeps to go back to, as one task.
    fn old_revisions(&self, listing: &str) -> Option<CleanupItem> {
        let mut revisions = disabled_revisions(listing);
        revisions.sort();
        let key = format!(
            "disabled-revisions:{}",
            revisions
                .iter()
                .map(|(name, revision, _)| format!("{name}={revision}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        // Only what Snap would name goes in the key the runner reads.
        if revisions.is_empty() || snap_revisions(&key).is_none() {
            return None;
        }
        let sizes: Vec<u64> = revisions
            .iter()
            .filter_map(|(name, revision, _)| {
                std::fs::metadata(format!("/var/lib/snapd/snaps/{name}_{revision}.snap")).ok()
            })
            .map(|metadata| metadata.len())
            .collect();
        let count = revisions.len();
        Some(CleanupItem {
            id: CleanupId {
                backend: "snap".into(),
                key,
            },
            kind: CleanupKind::Unused,
            title: "Old Snap revisions".into(),
            summary: format!(
                "{count} older version{} kept after updates",
                if count == 1 { "" } else { "s" }
            ),
            preview: format!(
                "Removes these older versions. Each snap keeps the one it runs, and can't go back to these after.\n{}",
                revisions
                    .iter()
                    .map(|(name, revision, version)| format!("{name} {version} (revision {revision})"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            bytes: (!sizes.is_empty()).then(|| sizes.iter().sum()),
            restorable: false,
        })
    }
    fn snap_icon(&self, package: &mut Package) {
        if matches!(self.kind, ManagerKind::Snap) {
            package.icon = snap_icon(std::path::Path::new("/"), &package.id.name);
        }
    }
    /// Desktop-id stems shipped by an installed snap. CLI-only snaps expose
    /// no desktop entry and group with nothing.
    fn snap_components(&self, package: &mut Package) {
        if matches!(self.kind, ManagerKind::Snap) {
            package.component_ids = snap_desktop_ids(std::path::Path::new("/"), &package.id.name);
        }
    }
}
impl<T: Transport> Backend for SystemManager<T> {
    fn id(&self) -> &str {
        self.kind.id()
    }
    fn has_update_index(&self) -> bool {
        matches!(
            self.kind,
            ManagerKind::Dnf | ManagerKind::Pacman | ManagerKind::Zypper | ManagerKind::Apk
        )
    }
    fn arm_update_check(&mut self, token: Option<u64>) {
        self.update_check = token;
    }
    fn refresh_update_index(&mut self, cancel: &Cancellation) -> Result<(), EngineError> {
        if self.update_check.is_none() || self.refresh_without_password(cancel)? {
            return Ok(());
        }
        // Without PackageKit, the check fills its own copy of the index.
        let Some(copy) = self
            .transport
            .prepare_private_index(self.kind.id(), cancel)?
        else {
            return Ok(());
        };
        let dir = copy.dir.clone().into_os_string();
        let (executable, args): (&str, Vec<OsString>) = match self.kind {
            // The way checkupdates does it: under fakeroot, pacman fills a
            // database the user owns.
            ManagerKind::Pacman => (
                "fakeroot",
                vec![
                    "--".into(),
                    "pacman".into(),
                    "-Sy".into(),
                    "--dbpath".into(),
                    dir,
                    "--logfile".into(),
                    "/dev/null".into(),
                ],
            ),
            // zypper refreshes the copy's metadata as it lists updates.
            ManagerKind::Zypper => (
                "zypper",
                vec![
                    "--cache-dir".into(),
                    dir,
                    "--quiet".into(),
                    "--non-interactive".into(),
                    "list-updates".into(),
                ],
            ),
            ManagerKind::Apk => (
                "apk",
                vec![
                    "--cache-dir".into(),
                    dir,
                    "--update-cache".into(),
                    "list".into(),
                    "--upgradable".into(),
                ],
            ),
            // Nothing else keeps one.
            _ => return Ok(()),
        };
        self.looked_up.clear();
        match self.transport.refresh_index(executable, &args, cancel) {
            // Without fakeroot, updates stay as the system last saw them.
            Err(ExecutionError::Disabled(_)) if executable == "fakeroot" => Ok(()),
            result => bytes(self.kind.id(), result?).map(|_| ()),
        }
    }
    fn refresh_without_password(&mut self, cancel: &Cancellation) -> Result<bool, EngineError> {
        let refreshed = match self.kind {
            // DNF run as the user keeps its own cache, the one PkgDeck reads,
            // and refreshes it without root. Upgrades refresh the system's.
            ManagerKind::Dnf => {
                let args = ["--refresh", "--quiet", "makecache"].map(OsString::from);
                bytes("dnf", self.transport.refresh_index("dnf", &args, cancel)?)?;
                true
            }
            ManagerKind::Pacman | ManagerKind::Zypper => {
                self.transport.packagekit_refresh(self.kind.id(), cancel)?
            }
            _ => false,
        };
        if refreshed {
            self.looked_up.clear();
        }
        Ok(refreshed)
    }
    fn capabilities(&self) -> &[Capability] {
        if self.kind == ManagerKind::Snap {
            CLEAN_CAPABILITIES
        } else {
            CAPABILITIES
        }
    }
    fn may_have(&self, name: &str) -> bool {
        self.valid_name(name)
    }
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        if self.kind != ManagerKind::Snap {
            return Err(self.unsupported(Capability::Clean));
        }
        let listing = self.snap_listing(cancel)?;
        let mut items = self.old_revisions(&listing).into_iter().collect::<Vec<_>>();
        if let Some(folders) = self.data_folders() {
            items.extend(folders.leftovers(&snap_names(&listing), cancel)?);
        }
        Ok(items)
    }
    fn cleanup_plan(
        &mut self,
        id: &CleanupId,
        cancel: &Cancellation,
    ) -> Result<CleanupItem, EngineError> {
        if self.kind == ManagerKind::Snap && id.backend == "snap" && is_data_key(&id.key) {
            let folders = self.data_folders().ok_or(EngineError::NotFound)?;
            let listing = self.snap_listing(cancel)?;
            return folders.plan(&id.key, &snap_names(&listing), cancel);
        }
        self.cleanup(cancel)?
            .into_iter()
            .find(|item| item.id == *id)
            .ok_or(EngineError::NotFound)
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        match self.query(true, "", cancel) {
            Ok(_) => Ok(Availability::Available),
            Err(EngineError::Execution(ExecutionError::Disabled(reason))) => {
                Ok(Availability::Unavailable(reason))
            }
            Err(error) => Err(error),
        }
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let mut packages = self.query(false, query, cancel)?;
        if matches!(self.kind, ManagerKind::Pacman) {
            // Its search rows already carry the installed version.
            return Ok(packages);
        }
        let installed = self.query(true, "", cancel)?;
        let versions: std::collections::BTreeMap<_, _> = installed
            .iter()
            .map(|package| {
                (
                    (package.id.name.as_str(), package.id.architecture.as_str()),
                    &package.installed_version,
                )
            })
            .collect();
        for package in &mut packages {
            if let Some(version) =
                versions.get(&(package.id.name.as_str(), package.id.architecture.as_str()))
            {
                package.installed_version = (*version).clone();
            }
        }
        Ok(packages)
    }
    fn lookup(&mut self, name: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.looked_up.clear();
        let packages = self.search(name, cancel)?;
        self.looked_up = packages.clone();
        Ok(packages)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let mut packages = self.query(true, "", cancel)?;
        if matches!(self.kind, ManagerKind::Pacman) {
            self.drop_foreign(&mut packages, cancel)?;
        }
        let updates = self.updates(cancel)?;
        for package in &mut packages {
            self.snap_icon(package);
            self.snap_components(package);
            if let Some(candidate) = updates.get(&package.id.name) {
                package.candidate_version = Some(candidate.clone());
                package.update = UpdateAvailability::Available;
            }
        }
        Ok(packages)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let name = self.target(id)?;
        let looked_up = std::mem::take(&mut self.looked_up)
            .into_iter()
            .find(|package| package.id == *id);
        // Search merges the installed version into catalog rows.
        let mut package = match looked_up {
            Some(package) => package,
            None => self
                .search(&name, cancel)?
                .into_iter()
                .find(|package| package.id == *id)
                .ok_or(EngineError::NotFound)?,
        };
        self.snap_icon(&mut package);
        self.snap_components(&mut package);
        let data = package
            .installed_version
            .is_some()
            .then(|| self.data_folders())
            .flatten()
            .and_then(|folders| folders.data(&name, cancel));
        Ok(PackageDetails {
            description: package.summary.clone(),
            homepage: None,
            dependencies: vec![],
            package,
            data,
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        // A change can make what a lookup read out of date.
        self.looked_up.clear();
        if operation.backend() != self.kind.id() {
            return Err(invalid(self.kind.id(), "foreign operation"));
        }
        let artifact = match operation {
            Operation::Install(id)
                if id
                    .reference
                    .as_deref()
                    .is_some_and(|value| value.starts_with("artifact:")) =>
            {
                if !matches!(
                    self.kind,
                    ManagerKind::Dnf
                        | ManagerKind::Pacman
                        | ManagerKind::Zypper
                        | ManagerKind::Snap
                ) {
                    return Err(invalid(self.kind.id(), "unsupported local archive"));
                }
                crate::artifact::stage(id, cancel)?
            }
            _ => None,
        };
        if let Some(staged) = artifact.as_ref() {
            let path = staged.path().as_os_str().to_os_string();
            let args: Vec<OsString> = match self.kind {
                ManagerKind::Dnf => vec!["-y".into(), "install".into(), "--".into(), path],
                ManagerKind::Pacman => vec!["-U".into(), "--noconfirm".into(), "--".into(), path],
                ManagerKind::Zypper => vec![
                    "--non-interactive".into(),
                    "install".into(),
                    "--auto-agree-with-licenses".into(),
                    "--".into(),
                    path,
                ],
                // Snap: the other managers were refused before staging.
                _ => {
                    let assertion = staged
                        .assertion()
                        .ok_or_else(|| invalid("snap", "matching assertion is required"))?;
                    bytes(
                        "snap",
                        self.call(
                            vec!["ack".into(), assertion.as_os_str().to_os_string()],
                            cancel,
                            true,
                        )?,
                    )?;
                    vec!["install".into(), path]
                }
            };
            progress(Progress::Message(format!(
                "Installing local {} package…",
                self.kind.id()
            )));
            let result = self.call(args, cancel, true)?;
            bytes(self.kind.id(), result.clone())?;
            return Ok(OperationOutcome {
                cancellation_deferred: result.cancellation_deferred,
            });
        }
        // Snap has no metadata-only refresh. `snap refresh` upgrades every snap,
        // so the refresh runs the read-only listing instead.
        if matches!(
            (self.kind, operation),
            (ManagerKind::Snap, Operation::Refresh { .. })
        ) {
            progress(Progress::Message("Reading the Snap package list…".into()));
            bytes(
                "snap",
                self.call_with(
                    "snap",
                    vec![OsString::from("refresh"), OsString::from("--list")],
                    cancel,
                    false,
                )?,
            )?;
            return Ok(OperationOutcome::default());
        }
        if let (ManagerKind::Snap, Operation::Clean(id)) = (self.kind, operation) {
            if is_data_key(&id.key) {
                let folders = self.data_folders().ok_or(EngineError::NotFound)?;
                let installed = snap_names(&self.snap_listing(cancel)?);
                return folders.trash(
                    &id.key,
                    &installed,
                    app_data::trash(&self.transport),
                    cancel,
                    progress,
                );
            }
            let revisions = snap_revisions(&id.key).ok_or(EngineError::NotFound)?;
            progress(Progress::Message("Removing older Snap versions…".into()));
            let mut deferred = false;
            for (name, revision) in revisions {
                if cancel.requested() {
                    return Err(EngineError::Cancelled);
                }
                let args = vec![
                    "remove".into(),
                    name.into(),
                    format!("--revision={revision}").into(),
                ];
                let result = self.call(args, cancel, true)?;
                deferred |= result.cancellation_deferred;
                bytes("snap", result)?;
            }
            return Ok(OperationOutcome {
                cancellation_deferred: deferred,
            });
        }
        let Some(fixed) = self.kind.write_args(operation) else {
            return Err(self.unsupported(Capability::Clean));
        };
        let name = match operation {
            Operation::Install(id) | Operation::Remove(id) | Operation::Upgrade(id) => {
                Some(self.target(id)?)
            }
            _ => None,
        };
        let mut args: Vec<OsString> = fixed.into_iter().map(OsString::from).collect();
        if let Some(name) = name {
            args.push(name.into());
        }
        progress(Progress::Message(format!(
            "Running {}. If you cancel, PkgDeck waits for it to finish.",
            self.kind.id()
        )));
        let result = self.call_with(self.kind.write_executable(operation), args, cancel, true)?;
        bytes(self.kind.id(), result.clone())?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
    /// pacman updates what an update needs, but not the installed packages
    /// that need the exact version it replaces, like systemd-sysvcompat
    /// needing systemd=262: those join, and then what needs them.
    fn lockstep(
        &mut self,
        operations: &[Operation],
        cancel: &Cancellation,
    ) -> Result<Vec<Operation>, EngineError> {
        if !matches!(self.kind, ManagerKind::Pacman) {
            return Ok(vec![]);
        }
        let mut chosen: std::collections::BTreeSet<String> = operations
            .iter()
            .filter_map(|operation| match operation {
                Operation::Upgrade(id) if id.backend == "pacman" => Some(id.name.clone()),
                _ => None,
            })
            .collect();
        if chosen.is_empty() {
            return Ok(vec![]);
        }
        let updates = self.updates(cancel)?;
        let mut args: Vec<OsString> = vec!["-Qi".into(), "--".into()];
        args.extend(
            updates
                .keys()
                .filter(|name| !chosen.contains(*name) && self.valid_name(name))
                .map(OsString::from),
        );
        if args.len() == 2 {
            return Ok(vec![]);
        }
        let text = String::from_utf8_lossy(&bytes("pacman", self.call(args, cancel, false)?)?)
            .into_owned();
        let installed = pacman_pins(&text);
        let mut joined = vec![];
        loop {
            let before = joined.len();
            for PacmanPins {
                name,
                architecture,
                pins,
            } in &installed
            {
                let breaks = pins.iter().any(|(dependency, version)| {
                    chosen.contains(dependency)
                        && updates
                            .get(dependency)
                            .is_some_and(|candidate| !same_pacman_version(candidate, version))
                });
                if breaks && updates.contains_key(name) && chosen.insert(name.clone()) {
                    joined.push(Operation::Upgrade(PackageId {
                        backend: "pacman".into(),
                        name: name.clone(),
                        architecture: architecture.clone(),
                        scope: Scope::System,
                        remote: None,
                        reference: None,
                    }));
                }
            }
            if joined.len() == before {
                return Ok(joined);
            }
        }
    }
    /// pacman updates every selected package in one transaction, so
    /// packages that move in lockstep update together.
    fn execute_group(
        &mut self,
        operations: &[Operation],
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Option<Result<Vec<OperationOutcome>, EngineError>> {
        if !matches!(self.kind, ManagerKind::Pacman) || operations.len() < 2 {
            return None;
        }
        self.looked_up.clear();
        let grouped = (|| {
            let fixed = self.kind.write_args(&operations[0]).unwrap_or_default();
            let mut names = vec![];
            for operation in operations {
                match operation {
                    Operation::Upgrade(id) if id.backend == "pacman" => {
                        names.push(self.target(id)?);
                    }
                    _ => return Err(invalid("pacman", "mixed pacman batch")),
                }
            }
            // Names the packages that joined the ones chosen, too.
            progress(Progress::Message(format!(
                "Updating {} in one transaction…",
                and_list(&names)
            )));
            let mut args: Vec<OsString> = fixed.into_iter().map(OsString::from).collect();
            args.extend(names.into_iter().map(OsString::from));
            let result = self.call_with("pacman", args, cancel, true)?;
            bytes("pacman", result.clone())?;
            Ok(vec![
                OperationOutcome {
                    cancellation_deferred: result.cancellation_deferred,
                };
                operations.len()
            ])
        })();
        Some(grouped)
    }
}

/// A package in `pacman -Qi`'s answer.
#[derive(Debug, PartialEq)]
pub(super) struct PacmanPins {
    pub(super) name: String,
    pub(super) architecture: String,
    /// The exact versions it depends on (`systemd=262`), by name.
    pub(super) pins: Vec<(String, String)>,
}

/// Each package in `pacman -Qi`'s answer.
pub(super) fn pacman_pins(text: &str) -> Vec<PacmanPins> {
    let mut packages = vec![];
    let mut fields: Vec<(String, String)> = vec![];
    let mut finish = |fields: &mut Vec<(String, String)>| {
        let field = |key: &str| {
            fields
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        if let Some(name) = field("Name") {
            let pins = field("Depends On")
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|dependency| dependency.split_once('='))
                // `>=` and `<=` aren't exact.
                .filter(|(name, _)| !name.ends_with(['<', '>']))
                .map(|(name, version)| (name.to_owned(), version.to_owned()))
                .collect();
            let architecture = field("Architecture").unwrap_or_default().to_owned();
            packages.push(PacmanPins {
                name: name.to_owned(),
                architecture,
                pins,
            });
        }
        fields.clear();
    };
    for line in text.lines() {
        if line.trim().is_empty() {
            finish(&mut fields);
        } else if line.starts_with(char::is_whitespace) {
            // A long value goes on below.
            if let Some((_, value)) = fields.last_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
        } else if let Some((key, value)) = line.split_once(" : ") {
            fields.push((key.trim().to_owned(), value.trim().to_owned()));
        }
    }
    finish(&mut fields);
    packages
}

/// `a`, `a and b`, or `a, b, and c`.
pub(super) fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// Whether `candidate`, a full version, is the one `pinned` names. A pin
/// without a release (`262`) matches every release of that version.
pub(super) fn same_pacman_version(candidate: &str, pinned: &str) -> bool {
    candidate == pinned
        || (!pinned.contains('-')
            && candidate
                .rsplit_once('-')
                .is_some_and(|(version, _)| version == pinned))
}
