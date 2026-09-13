//! Frontend-independent package identities and metadata. Versions are backend-defined.
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Scope {
    System,
    User { uid: u32 },
    Environment { path: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PackageId {
    pub backend: String,
    pub name: String,
    pub architecture: String,
    pub scope: Scope,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateAvailability {
    Unknown,
    Current,
    Available,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Package {
    pub id: PackageId,
    pub display_name: String,
    pub summary: String,
    pub installed_version: Option<String>,
    pub candidate_version: Option<String>,
    /// Backends decide upgrade availability using their own version semantics.
    pub update: UpdateAvailability,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageDetails {
    pub package: Package,
    pub description: String,
    pub homepage: Option<String>,
    pub dependencies: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Capability {
    Search,
    Details,
    Installed,
    Install,
    Remove,
    Refresh,
    Upgrade,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Availability {
    Available,
    Unavailable(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Operation {
    /// Refresh metadata only; never upgrades installed packages.
    Refresh {
        backend: String,
    },
    Install(PackageId),
    Remove(PackageId),
    Upgrade(PackageId),
}

impl Operation {
    pub fn backend(&self) -> &str {
        match self {
            Self::Refresh { backend } => backend,
            Self::Install(id) | Self::Remove(id) | Self::Upgrade(id) => &id.backend,
        }
    }
    pub fn capability(&self) -> Capability {
        match self {
            Self::Refresh { .. } => Capability::Refresh,
            Self::Install(_) => Capability::Install,
            Self::Remove(_) => Capability::Remove,
            Self::Upgrade(_) => Capability::Upgrade,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Progress {
    Message(String),
    Transfer { completed: u64, total: Option<u64> },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OperationOutcome {
    /// A native write completed after cancellation was requested; it was not rolled back.
    pub cancellation_deferred: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selector {
    /// Exact backend package identifier, not its display name or a fuzzy search term.
    pub name: String,
    pub backend: Option<String>,
    pub architecture: Option<String>,
    pub scope: Option<Scope>,
}
