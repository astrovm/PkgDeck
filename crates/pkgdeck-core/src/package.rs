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

/// Sorted backend ids, other than `id`'s own backend, with an installed
/// package sharing one of its AppStream component ids. Only installed
/// packages group: remote catalog entries never join, and a backend with
/// several matching packages is listed once. Display-only: selection and
/// writes always address exact identities, never groups.
pub fn same_app_sources(packages: &[Package], id: &PackageId) -> Vec<String> {
    let own: Vec<&str> = packages
        .iter()
        .find(|package| package.id == *id)
        .map(|package| package.component_ids.iter().map(String::as_str).collect())
        .unwrap_or_default();
    if own.is_empty() {
        return vec![];
    }
    let mut backends: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for package in packages {
        if package.id.backend == id.backend || package.installed_version.is_none() {
            continue;
        }
        if package
            .component_ids
            .iter()
            .any(|component| own.contains(&component.as_str()))
        {
            backends.insert(package.id.backend.as_str());
        }
    }
    backends.into_iter().map(str::to_owned).collect()
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
}
