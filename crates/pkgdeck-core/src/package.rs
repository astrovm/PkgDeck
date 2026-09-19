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

/// Installed backends keyed by AppStream component id. Empty stems are
/// ignored so they never join unrelated rows.
fn installed_component_backends(
    packages: &[Package],
) -> std::collections::BTreeMap<&str, std::collections::BTreeSet<&str>> {
    let mut index = std::collections::BTreeMap::new();
    for package in packages {
        if package.installed_version.is_none() {
            continue;
        }
        for component in &package.component_ids {
            if component.is_empty() {
                continue;
            }
            index
                .entry(component.as_str())
                .or_insert_with(std::collections::BTreeSet::new)
                .insert(package.id.backend.as_str());
        }
    }
    index
}
fn backends_sharing(
    components: &[String],
    backend: &str,
    index: &std::collections::BTreeMap<&str, std::collections::BTreeSet<&str>>,
) -> Vec<String> {
    let mut backends = std::collections::BTreeSet::new();
    for component in components {
        if component.is_empty() {
            continue;
        }
        if let Some(set) = index.get(component.as_str()) {
            for other in set {
                if *other != backend {
                    backends.insert(*other);
                }
            }
        }
    }
    backends.into_iter().map(str::to_owned).collect()
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
/// package sharing one of its AppStream component ids. Only installed
/// packages group: remote catalog entries never join, and a backend with
/// several matching packages is listed once. Display-only: selection and
/// writes always address exact identities, never groups.
pub fn same_app_sources(packages: &[Package], id: &PackageId) -> Vec<String> {
    let Some(package) = packages.iter().find(|package| package.id == *id) else {
        return vec![];
    };
    backends_sharing(
        &package.component_ids,
        &id.backend,
        &installed_component_backends(packages),
    )
}
/// Per-row `same_app_sources` for a whole report, sharing one inverted index.
pub fn same_app_sources_all(packages: &[Package]) -> Vec<Vec<String>> {
    let index = installed_component_backends(packages);
    packages
        .iter()
        .map(|package| backends_sharing(&package.component_ids, &package.id.backend, &index))
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
