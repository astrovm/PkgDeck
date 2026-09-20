//! Frontend-independent package identities and metadata. Versions are backend-defined.
use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    System,
    User { uid: u32 },
    Environment { path: PathBuf },
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PackageId {
    pub backend: String,
    pub name: String,
    pub architecture: String,
    pub scope: Scope,
    /// Source-specific repository identity, such as a Flatpak remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateAvailability {
    Unknown,
    Current,
    Available,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Package {
    pub id: PackageId,
    pub display_name: String,
    pub summary: String,
    pub installed_version: Option<String>,
    pub candidate_version: Option<String>,
    /// Backends decide upgrade availability using their own version semantics.
    pub update: UpdateAvailability,
    /// Local icon file for installed packages, resolved without network
    /// access where the backend can provide one. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<PathBuf>,
    /// AppStream component ids (desktop-id stems) identifying the same
    /// application across managers, e.g. `org.mozilla.firefox` for the APT,
    /// Flatpak, and Snap Firefox builds. Display-only grouping key: writes
    /// always address exact backend identities. Empty when unknown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component_ids: Vec<String>,
    /// Upstream homepages (raw URLs) identifying the same application
    /// across managers where AppStream has no entry, e.g. a CLI tool
    /// installed from both APT and Homebrew. Same display-only contract
    /// as component ids; normalized only while grouping. Empty when unknown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub homepages: Vec<String>,
}
/// Normalize a homepage for grouping: case-insensitive, no scheme, no
/// `www.` prefix, no query/fragment, no trailing slash. Empty or
/// unparseable values are `None` so they never join unrelated rows.
pub fn normalize_homepage(url: &str) -> Option<String> {
    let url = url.trim().to_lowercase();
    let url = url.split(['?', '#']).next().unwrap_or("");
    let url = url.split("://").last().unwrap_or("");
    let url = url.trim_end_matches('/');
    let url = url.strip_prefix("www.").unwrap_or(url);
    (!url.is_empty()).then(|| url.to_owned())
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct PackageDetails {
    pub package: Package,
    pub description: String,
    pub homepage: Option<String>,
    pub dependencies: Vec<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Search,
    Details,
    Installed,
    Install,
    Remove,
    Refresh,
    Upgrade,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Unavailable(String),
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Refresh metadata only; never upgrades installed packages.
    Refresh {
        backend: String,
    },
    Install(PackageId),
    Remove(PackageId),
    Upgrade(PackageId),
    /// Upgrade every package managed by one backend in a single transaction.
    UpgradeAll {
        backend: String,
    },
}

impl Operation {
    pub fn backend(&self) -> &str {
        match self {
            Self::Refresh { backend } | Self::UpgradeAll { backend } => backend,
            Self::Install(id) | Self::Remove(id) | Self::Upgrade(id) => &id.backend,
        }
    }
    pub fn capability(&self) -> Capability {
        match self {
            Self::Refresh { .. } => Capability::Refresh,
            Self::Install(_) => Capability::Install,
            Self::Remove(_) => Capability::Remove,
            Self::Upgrade(_) | Self::UpgradeAll { .. } => Capability::Upgrade,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Progress {
    Message(String),
    Transfer { completed: u64, total: Option<u64> },
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct OperationOutcome {
    /// A native write completed after cancellation was requested; it was not rolled back.
    pub cancellation_deferred: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Selector {
    /// Exact backend package identifier, not its display name or a fuzzy search term.
    pub name: String,
    pub backend: Option<String>,
    pub architecture: Option<String>,
    pub scope: Option<Scope>,
}

/// Grouping keys for one package: AppStream component ids plus normalized
/// homepages, each in its own namespace so a stem can never collide with a
/// URL. Empty values are ignored so they never join unrelated rows.
fn app_keys(components: &[String], homepages: &[String]) -> Vec<String> {
    let mut keys: Vec<String> = components
        .iter()
        .filter(|id| !id.is_empty())
        .map(|id| format!("c:{id}"))
        .collect();
    keys.extend(
        homepages
            .iter()
            .filter_map(|url| normalize_homepage(url))
            .map(|url| format!("h:{url}")),
    );
    keys
}
/// Installed backends keyed by grouping key (see [`app_keys`]).
fn installed_component_backends(
    packages: &[Package],
) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
    let mut index = std::collections::BTreeMap::new();
    for package in packages {
        if package.installed_version.is_none() {
            continue;
        }
        for key in app_keys(&package.component_ids, &package.homepages) {
            index
                .entry(key)
                .or_insert_with(std::collections::BTreeSet::new)
                .insert(package.id.backend.clone());
        }
    }
    index
}
fn backends_sharing(
    components: &[String],
    homepages: &[String],
    backend: &str,
    index: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
) -> Vec<String> {
    let mut backends = std::collections::BTreeSet::new();
    for key in app_keys(components, homepages) {
        if let Some(set) = index.get(&key) {
            for other in set {
                if other != backend {
                    backends.insert(other.clone());
                }
            }
        }
    }
    backends.into_iter().collect()
}
/// Best-match-first ordering for search results, shared by the terminal
/// frontends (the GUI ranks in QML for live keystrokes; same rule, see
/// Browser.qml relevanceScore): exact name, name prefix, name substring,
/// summary prefix, summary substring, then the rest. Unverifiable offers
/// (no version either way) sink below verified rows, and name/source
/// ties break deterministically. Display-only: engine order is untouched.
pub fn rank_search_matches(packages: &mut [Package], query: &str) {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return;
    }
    packages.sort_by(|a, b| {
        fabricated(a)
            .cmp(&fabricated(b))
            .then(score(a, &query).cmp(&score(b, &query)))
            .then(a.id.name.cmp(&b.id.name))
            .then(a.id.backend.cmp(&b.id.backend))
    });
}
fn fabricated(package: &Package) -> bool {
    package.installed_version.is_none() && package.candidate_version.is_none()
}
fn score(package: &Package, query: &str) -> u8 {
    let name = package.id.name.to_lowercase();
    let summary = package.summary.to_lowercase();
    if name == query {
        0
    } else if name.starts_with(query) {
        1
    } else if name.contains(query) {
        2
    } else if summary.starts_with(query) {
        3
    } else if summary.contains(query) {
        4
    } else {
        5
    }
}
/// Sorted backend ids, other than `id`'s own backend, with an installed
/// package sharing one of its AppStream component ids or normalized
/// homepages. Only installed packages group: remote catalog entries never
/// join, and a backend with several matching packages is listed once.
/// Display-only: selection and writes always address exact identities,
/// never groups.
pub fn same_app_sources(packages: &[Package], id: &PackageId) -> Vec<String> {
    let Some(package) = packages.iter().find(|package| package.id == *id) else {
        return vec![];
    };
    let index = installed_component_backends(packages);
    backends_sharing(
        &package.component_ids,
        &package.homepages,
        &id.backend,
        &index,
    )
}
/// Per-row `same_app_sources` for a whole report, sharing one inverted index.
pub fn same_app_sources_all(packages: &[Package]) -> Vec<Vec<String>> {
    let index = installed_component_backends(packages);
    packages
        .iter()
        .map(|package| {
            backends_sharing(
                &package.component_ids,
                &package.homepages,
                &package.id.backend,
                &index,
            )
        })
        .collect()
}

/// Stable display-group key for each package. A key is returned only when it
/// links installed packages from more than one backend; callers can therefore
/// group related rows without changing their exact package identities.
pub fn same_app_group_keys_all(packages: &[Package]) -> Vec<Option<String>> {
    let index = installed_component_backends(packages);
    let mut labels: std::collections::BTreeMap<String, String> = index
        .iter()
        .filter(|(_, backends)| backends.len() > 1)
        .map(|(key, _)| (key.clone(), key.clone()))
        .collect();
    // A package carrying both a component id and a homepage joins those
    // namespaces. Propagate the smallest key until transitive links settle.
    loop {
        let mut changed = false;
        for package in packages {
            let keys: Vec<_> = app_keys(&package.component_ids, &package.homepages)
                .into_iter()
                .filter(|key| labels.contains_key(key))
                .collect();
            let Some(label) = keys.iter().filter_map(|key| labels.get(key)).min().cloned() else {
                continue;
            };
            for key in keys {
                if labels.get(&key).is_some_and(|current| current > &label) {
                    labels.insert(key, label.clone());
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    packages
        .iter()
        .map(|package| {
            if package.installed_version.is_none() {
                return None;
            }
            app_keys(&package.component_ids, &package.homepages)
                .into_iter()
                .filter_map(|key| labels.get(&key).cloned())
                .min()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn package(backend: &str, name: &str, installed: bool, components: &[&str]) -> Package {
        Package {
            id: PackageId {
                backend: backend.into(),
                name: name.into(),
                architecture: "amd64".into(),
                scope: Scope::System,
                remote: None,
            },
            display_name: name.into(),
            summary: String::new(),
            installed_version: installed.then(|| "1".into()),
            candidate_version: Some("2".into()),
            update: UpdateAvailability::Available,
            icon: None,
            component_ids: components.iter().map(ToString::to_string).collect(),
            homepages: vec![],
        }
    }
    fn firefox_id() -> PackageId {
        PackageId {
            backend: "apt".into(),
            name: "firefox".into(),
            architecture: "amd64".into(),
            scope: Scope::System,
            remote: None,
        }
    }
    #[test]
    fn same_app_groups_installed_packages_by_component() {
        let packages = vec![
            package("apt", "firefox", true, &["firefox"]),
            package(
                "flatpak",
                "org.mozilla.firefox",
                true,
                &["org.mozilla.firefox", "firefox"],
            ),
            package("snap", "firefox", true, &["firefox"]),
            package("apt", "unrelated", true, &["unrelated"]),
        ];
        assert_eq!(
            same_app_sources(&packages, &firefox_id()),
            vec!["flatpak".to_string(), "snap".to_string()]
        );
    }
    #[test]
    fn same_app_ignores_remote_same_backend_and_unknown() {
        let packages = vec![
            package("apt", "firefox", true, &["firefox"]),
            // Remote catalog entries never join a group.
            package("flatpak", "org.mozilla.firefox", false, &["firefox"]),
            // A second apt row is the same backend, not another source.
            package("apt", "firefox-esr", true, &["firefox"]),
            // No shared component id.
            package("snap", "chromium", true, &["chromium"]),
        ];
        assert!(same_app_sources(&packages, &firefox_id()).is_empty());
        // Unknown identities and component-less packages group with nothing.
        assert!(same_app_sources(
            &packages,
            &PackageId {
                backend: "apt".into(),
                name: "missing".into(),
                architecture: "amd64".into(),
                scope: Scope::System,
                remote: None,
            }
        )
        .is_empty());
        assert!(same_app_sources(
            &packages,
            &PackageId {
                backend: "snap".into(),
                name: "chromium".into(),
                architecture: "amd64".into(),
                scope: Scope::System,
                remote: None,
            }
        )
        .is_empty());
    }
    #[test]
    fn rank_search_matches_orders_exact_prefix_substring_summary() {
        fn row(name: &str, summary: &str) -> Package {
            let mut package = package("apt", name, false, &[]);
            package.summary = summary.into();
            package.candidate_version = Some("1".into());
            package
        }
        let mut packages = vec![
            row("zzz", "fire starter"),
            row("x-fire-helper", "helper"),
            row("firefox", "browser"),
            row("fire", "exact"),
        ];
        rank_search_matches(&mut packages, "fire");
        let names: Vec<_> = packages.iter().map(|p| p.id.name.as_str()).collect();
        assert_eq!(names, vec!["fire", "firefox", "x-fire-helper", "zzz"]);
        // Empty queries keep engine order.
        let mut same = packages.clone();
        rank_search_matches(&mut same, "   ");
        assert_eq!(same, packages);
    }
    #[test]
    fn rank_search_matches_sinks_unverified_guesses() {
        let mut guess = package("npm", "fire", false, &[]);
        guess.candidate_version = None;
        let mut real = package("apt", "zzz", false, &[]);
        real.summary = "fire starter".into();
        real.candidate_version = Some("1".into());
        let mut packages = vec![guess, real];
        rank_search_matches(&mut packages, "fire");
        assert_eq!(packages[0].id.backend, "apt");
        assert_eq!(packages[1].id.backend, "npm");
    }
    #[test]
    fn normalize_homepage_strips_scheme_www_and_trailing_slash() {
        assert_eq!(
            normalize_homepage("https://github.com/sharkdp/bat/"),
            Some("github.com/sharkdp/bat".into())
        );
        assert_eq!(
            normalize_homepage("http://www.libreoffice.org/discover/writer/"),
            Some("libreoffice.org/discover/writer".into())
        );
        assert_eq!(
            normalize_homepage("https://gnu.org/software/libidn/#libidn2"),
            Some("gnu.org/software/libidn".into())
        );
        assert_eq!(normalize_homepage(""), None);
        assert_eq!(normalize_homepage("   "), None);
        assert_eq!(normalize_homepage("https://"), None);
    }
    #[test]
    fn same_app_groups_by_shared_homepage() {
        let mut apt = package("apt", "bat", true, &[]);
        apt.homepages = vec!["https://github.com/sharkdp/bat".into()];
        let mut brew = package("homebrew", "bat", true, &[]);
        brew.homepages = vec!["https://github.com/sharkdp/bat/".into()];
        let mut other = package("apt", "other", true, &[]);
        other.homepages = vec!["https://example.invalid/other".into()];
        let packages = vec![apt, brew, other];
        let apt_id = PackageId {
            backend: "apt".into(),
            name: "bat".into(),
            architecture: "amd64".into(),
            scope: Scope::System,
            remote: None,
        };
        // Trailing-slash variants still match after normalization.
        assert_eq!(
            same_app_sources(&packages, &apt_id),
            vec!["homebrew".to_string()]
        );
        // Components and homepages union: either key groups.
        let mut snap = package("snap", "bat", true, &["bat"]);
        snap.homepages = vec!["https://github.com/sharkdp/bat".into()];
        let mut packages = packages;
        packages.push(snap);
        assert_eq!(
            same_app_sources(&packages, &apt_id),
            vec!["homebrew".to_string(), "snap".to_string()]
        );
        let groups = same_app_group_keys_all(&packages);
        assert!(groups[0].is_some());
        assert_eq!(groups[0], groups[1]);
        assert_eq!(groups[0], groups[3]);
        assert_eq!(groups[2], None);
    }
    #[test]
    fn empty_component_stems_never_group() {
        let packages = vec![
            package("apt", "one", true, &[""]),
            package("flatpak", "two", true, &[""]),
        ];
        assert!(same_app_sources(
            &packages,
            &PackageId {
                backend: "apt".into(),
                name: "one".into(),
                architecture: "amd64".into(),
                scope: Scope::System,
                remote: None,
            }
        )
        .is_empty());
        assert!(same_app_sources_all(&packages).iter().all(Vec::is_empty));
    }
}
