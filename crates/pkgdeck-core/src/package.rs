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
