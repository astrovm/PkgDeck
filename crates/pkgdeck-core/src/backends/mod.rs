//! Native package-manager, container image, Homebrew formula/cask, and local AppImage adapters.
mod adopt;
mod ai_catalog;
mod appimage;
mod appimage_contents;
mod appimage_github;
mod apt;
mod aur;
mod cleanup;
mod conda;
mod dev_tool;
mod flatpak;
mod homebrew;
mod icons;
mod native;
mod system;
#[cfg(test)]
mod tests;
pub(crate) use conda::compare_versions;
mod container;
mod desktop_exec;
mod dev_caches;
mod dev_containers;
mod dotnet;
mod firmware;
mod go_bin;
mod mac_apps;
mod macos_updates;
mod mas;
mod nix;
mod oh_my_zsh;
mod pixi;
mod projects;
mod rustup;
mod standalone;
mod system_image;
use crate::{
    engine::*,
    host::{AptAction, Authorization, Host},
    package::*,
    process::*,
};
pub use appimage::AppImage;
pub use apt::*;
pub use aur::Aur;
pub use conda::Conda;
pub use container::{Container, ContainerKind};
pub use dev_containers::DevContainers;
pub use dev_tool::*;
pub use dotnet::DotnetTools;
pub use firmware::Firmware;
pub use flatpak::*;
pub use go_bin::GoBinaries;
pub use homebrew::*;
pub use icons::*;
pub use mac_apps::MacApps;
pub use macos_updates::MacUpdates;
pub use mas::MacAppStore;
pub use native::*;
pub use nix::Nix;
pub use oh_my_zsh::OhMyZsh;
pub use pixi::Pixi;
pub use projects::{ProjectSettings, WorktreeProtection};
pub use rustup::Rustup;
use serde::Deserialize;
pub use standalone::{Standalone, StandaloneTool};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::PathBuf,
    time::Duration,
};
pub use system::*;
pub use system_image::SystemImage;

/// Start an installed Flatpak app, detached, as its menu entry would.
pub fn launch_flatpak(id: &PackageId) -> Result<(), EngineError> {
    let bridge = (Host::current().runtime == crate::host::Runtime::Flatpak)
        .then(|| std::path::Path::new("flatpak-spawn"));
    start_flatpak(id, "flatpak", bridge)
}

/// `launch_flatpak` with the `flatpak` program to run, so tests can stand in.
fn start_flatpak(
    id: &PackageId,
    program: &str,
    bridge: Option<&std::path::Path>,
) -> Result<(), EngineError> {
    let mut command = flatpak_launch_command(id)?;
    command.program = program.into();
    desktop_exec::spawn(&command, bridge).map_err(|error| EngineError::InvalidResponse {
        backend: "flatpak".into(),
        reason: format!("couldn't start it: {error}"),
    })
}

/// `flatpak run` for the installation `id` is in.
fn flatpak_launch_command(id: &PackageId) -> Result<desktop_exec::Command, EngineError> {
    let invalid = |reason: &str| EngineError::InvalidResponse {
        backend: "flatpak".into(),
        reason: reason.into(),
    };
    let installation = match id.scope {
        Scope::System => "--system",
        Scope::User { .. } => "--user",
        Scope::Environment { .. } => return Err(invalid("only system and user apps can start")),
    };
    if id.backend != "flatpak" || id.name.is_empty() || id.name.starts_with('-') {
        return Err(invalid("not a Flatpak app"));
    }
    let mut arguments = vec!["run".to_owned(), installation.to_owned()];
    if !id.architecture.is_empty() {
        arguments.push(format!("--arch={}", id.architecture));
    }
    arguments.push(id.name.clone());
    Ok(desktop_exec::Command {
        environment: Vec::new(),
        program: "flatpak".into(),
        arguments,
    })
}

const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Refresh,
    Capability::Upgrade,
];
const CLEAN_CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Refresh,
    Capability::Upgrade,
    Capability::Clean,
];

/// Explicit empty scope for callers whose empty source list means all sources.
pub const NO_BACKENDS: &str = "__none__";

/// Backend ids accepted by `--from` and the source checklist. Adding a
/// backend means extending this list, the GUI `sourceIds`, and the CLI
/// value parser together.
pub const BACKEND_IDS: &[&str] = &[
    "fwupd",
    "apt",
    "dnf",
    "pacman",
    "aur",
    "zypper",
    "apk",
    "xbps",
    "snap",
    "system-image",
    "homebrew",
    "homebrew-cask",
    "macos-apps",
    "mas",
    "macos-updates",
    "macports",
    "appimage",
    "flatpak",
    "docker",
    "podman",
    "toolbox",
    "distrobox",
    "cargo",
    "rustup",
    "go",
    "dotnet",
    "npm",
    "pnpm",
    "bun",
    "pip",
    "pipx",
    "uv",
    "mise",
    "pixi",
    "conda",
    "nix",
    "composer",
    "gem",
    "oh-my-zsh",
    "codex",
    "claude",
    "grok",
    "opencode",
    "cursor",
    "copilot",
    "kiro",
    "antigravity",
    "amp",
    "droid",
    "solana",
    "anchor",
    "foundry",
    "dev-caches",
    "projects",
];

/// PkgDeck never installs from these sources: it updates what is already
/// there, and removes it only when the backend reports
/// [`Capability::Remove`].
pub fn never_installs(id: &str) -> bool {
    matches!(
        id,
        "fwupd"
            | "mas"
            | "macos-updates"
            | "conda"
            | "system-image"
            | "aur"
            | "toolbox"
            | "distrobox"
            | "oh-my-zsh"
    ) || StandaloneTool::ALL.iter().any(|tool| tool.id() == id)
}

/// Sources upgraded package by package: they have no single "upgrade
/// everything" command, so `pkd upgrade` without names lists each package.
pub fn per_package_upgrades(id: &str) -> bool {
    never_installs(id) || matches!(id, "pixi" | "rustup" | "nix")
}

/// System managers whose source refresh runs as root. A check never runs
/// one: it reads PackageKit's refresh or the user's own copy of the index
/// instead (see `refresh_update_index`), and an update refreshes first.
pub fn refresh_needs_root(id: &str) -> bool {
    matches!(
        id,
        "apt" | "dnf" | "pacman" | "zypper" | "apk" | "xbps" | "macports"
    )
}

/// The package a line of a manager's output says it is working on, when it
/// is one of the lines that name each package during an update: Homebrew's
/// "==> Upgrading firefox", APT's "Unpacking curl:amd64 (8.5) over ...",
/// DNF's "Upgrading htop-3.3.0-1.fc41.x86_64", and so on. Update all runs
/// one command per source and follows these to show each package.
pub fn output_package(backend: &str, line: &str) -> Option<String> {
    let word = |rest: &str| rest.split_whitespace().next().map(str::to_owned);
    let after = |marker: &str| line.find(marker).map(|at| &line[at + marker.len()..]);
    // name-version-release.arch, with an optional epoch in the version.
    let rpm = |nevra: String| {
        let base = nevra
            .rsplit_once('.')
            .map_or(nevra.as_str(), |(base, _)| base);
        let mut parts = base.rsplitn(3, '-');
        let (_, _, name) = (parts.next()?, parts.next()?, parts.next()?);
        Some(name.to_owned())
    };
    let name = match backend {
        "homebrew" | "homebrew-cask" => {
            let rest = line.strip_prefix("==> Upgrading ")?;
            // The summary heading ("Upgrading 3 outdated packages:") names none.
            if rest.trim().contains(char::is_whitespace) {
                return None;
            }
            word(rest)?.rsplit('/').next().map(str::to_owned)
        }
        "apt" => word(line.strip_prefix("Unpacking ")?)?
            .split(':')
            .next()
            .map(str::to_owned),
        // DNF 4: "  Upgrading   : htop-3.3.0-1.fc41.x86_64   1/2";
        // DNF 5: "[1/2] Upgrading htop-0:3.3.0-1.fc41.x86_64".
        "dnf" => match line.trim_start().strip_prefix("Upgrading") {
            Some(rest) if rest.trim_start().starts_with(':') => {
                rpm(word(rest.trim_start()[1..].trim_start())?)
            }
            _ => rpm(word(after("] Upgrading ")?)?),
        },
        "zypper" => rpm(word(after(") Installing: ")?)?),
        "pacman" => word(after(") upgrading ")?),
        "apk" => word(after(") Upgrading ")?),
        // "htop-3.3.0_1: unpacking ..."
        "xbps" => line
            .split_once(": unpacking")
            .and_then(|(pkgver, _)| pkgver.trim().rsplit_once('-'))
            .map(|(name, _)| name.to_owned()),
        "macports" => word(line.strip_prefix("--->  Installing ")?),
        // "Updating app/org.gnome.Maps/x86_64/stable" or a bare ID.
        "flatpak" => {
            let reference = word(line.trim_start().strip_prefix("Updating ")?)?;
            let mut parts = reference.split('/');
            let first = parts.next().unwrap_or_default();
            match parts.next() {
                Some(id) if first == "app" || first == "runtime" => Some(id.to_owned()),
                _ => Some(first.to_owned()),
            }
        }
        // "firefox 131.0 from Mozilla✓ refreshed"
        "snap" if line.trim_end().ends_with(" refreshed") => word(line),
        "pipx" => word(line.trim_start().strip_prefix("upgraded package ")?),
        "gem" => word(line.strip_prefix("Updating ")?).filter(|name| name != "installed"),
        // "==> Downloading Final Cut Pro (11.0)": App Store names have spaces.
        "mas" => line.strip_prefix("==> Downloading ").map(|rest| {
            rest.rsplit_once(" (")
                .map_or(rest, |(name, _)| name)
                .trim()
                .to_owned()
        }),
        _ => None,
    }?;
    (!name.is_empty()).then_some(name)
}

/// Inventory sources: PkgDeck never installs or upgrades their rows, and they
/// never take part in resolving a name to install or upgrade. Removal still
/// reaches the backend, which refuses it unless it reports
/// [`Capability::Remove`].
pub fn inventory_only(id: &str) -> bool {
    id == "macos-apps"
}

/// The name people know a source by, for sentences and titles: "APT",
/// "Flatpak", "Homebrew Casks". Ids stay the machine vocabulary (`--from`,
/// JSON); unknown ids are returned unchanged.
pub fn display_name(id: &str) -> &str {
    match id {
        "fwupd" => "Firmware",
        "apt" => "APT",
        "dnf" => "DNF",
        "pacman" => "Pacman",
        "zypper" => "Zypper",
        "snap" => "Snap",
        "homebrew" => "Homebrew",
        "homebrew-cask" => "Homebrew Casks",
        "macos-apps" => "macOS Applications",
        "mas" => "Mac App Store",
        "macos-updates" => "macOS Updates",
        "macports" => "MacPorts",
        "aur" => "AUR",
        "xbps" => "XBPS",
        "system-image" => "System image",
        "nix" => "Nix",
        "go" => "Go",
        "dotnet" => ".NET tools",
        "oh-my-zsh" => "Oh My Zsh",
        "appimage" => "AppImage",
        "flatpak" => "Flatpak",
        "docker" => "Docker images",
        "podman" => "Podman images",
        "toolbox" => "Toolbx containers",
        "distrobox" => "Distrobox containers",
        "cargo" => "Cargo",
        "bun" => "Bun",
        "conda" => "Conda",
        "composer" => "Composer",
        "gem" => "RubyGems",
        "codex" => "Codex",
        "claude" => "Claude Code",
        "grok" => "Grok",
        "opencode" => "OpenCode",
        "cursor" => "Cursor CLI",
        "copilot" => "GitHub Copilot CLI",
        "kiro" => "Kiro CLI",
        "antigravity" => "Antigravity CLI",
        "amp" => "Amp",
        "droid" => "Factory Droid",
        "solana" => "Solana CLI (Agave)",
        "anchor" => "Anchor (AVM)",
        "foundry" => "Foundry",
        "dev-caches" => "Developer caches",
        "projects" => "Old projects",
        // npm, pnpm, pip, pipx, uv, mise, pixi, apk, and rustup are written in lower case.
        other => other,
    }
}

/// A narrow transport seam lets adapter tests supply synthetic native responses.
pub trait Transport: Send {
    fn system_flatpak_writable(&self) -> bool {
        true
    }
    fn repository_editor_available(&self) -> bool {
        false
    }
    fn repository_editor(&self) -> Result<(), ExecutionError> {
        Err(ExecutionError::Disabled(
            "Software Sources editor unavailable".into(),
        ))
    }
    /// Refresh `source`'s lists through PackageKit, without a password.
    /// `Ok(false)` when PackageKit can't; fixtures never refresh there.
    fn packagekit_refresh(
        &self,
        source: &str,
        cancel: &Cancellation,
    ) -> Result<bool, ExecutionError> {
        let _ = (source, cancel);
        Ok(false)
    }
    /// The folder of `source`'s own copy of its index (see
    /// `private_index`), while it's newer than the system's.
    fn private_index(&self, source: &str) -> Option<PathBuf> {
        let _ = source;
        None
    }
    /// That folder, ready for a refresh and held until it's dropped.
    /// `None` when there's none.
    fn prepare_private_index(
        &self,
        source: &str,
        cancel: &Cancellation,
    ) -> Result<Option<crate::private_index::Copy>, ExecutionError> {
        let _ = (source, cancel);
        Ok(None)
    }
    /// Refresh an index the user owns: a read that can download for minutes.
    fn refresh_index(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        self.system_manager(executable, args, cancel, false)
    }
    // Fixtures only implement the managers their adapter drives; the native
    // transport overrides every one of these.
    fn apt_query(
        &self,
        _mode: &str,
        _query: &str,
        _arch: &str,
        _cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        Err(ExecutionError::Disabled("APT not found".into()))
    }
    fn apt_write(
        &self,
        _action: AptAction,
        _cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        Err(ExecutionError::Disabled("APT not found".into()))
    }
    fn apt_write_group(
        &self,
        _actions: &[AptAction],
        _cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        Err(ExecutionError::Disabled(
            "grouped APT transaction unavailable".into(),
        ))
    }
    fn brew(
        &self,
        _args: &[OsString],
        _cancel: &Cancellation,
        _write: bool,
    ) -> Result<Completion, ExecutionError> {
        Err(ExecutionError::Disabled("Homebrew not found".into()))
    }
    fn flatpak(
        &self,
        _args: &[OsString],
        _cancel: &Cancellation,
        _write: bool,
        _system: bool,
    ) -> Result<Completion, ExecutionError> {
        Err(ExecutionError::Disabled("Flatpak not found".into()))
    }
    /// An unused-ref preview is only supported by transports with an
    /// authoritative native libflatpak implementation. A CLI approximation
    /// must never authorize `uninstall --unused`.
    fn supports_flatpak_cleanup(&self) -> bool {
        true
    }
    /// Read-only libflatpak inventory; never runs an uninstall as a preview.
    fn flatpak_unused(&self, _cancel: &Cancellation) -> Result<Completion, ExecutionError> {
        Err(ExecutionError::Disabled(
            "Flatpak cannot show an exact preview of unused runtimes".into(),
        ))
    }
    /// True when the host `docker` executable is emulated by Podman.
    /// Fixtures keep the default; the native transport probes the host.
    fn docker_is_podman(&self) -> bool {
        false
    }
    /// Read one sanitized host environment value, if the fixture provides it.
    fn env(&self, _name: &str) -> Option<OsString> {
        None
    }
    /// Where Homebrew answers may be kept between runs, with everything
    /// they depend on. Fixtures keep none.
    fn brew_cache(&self) -> Option<(crate::cache::Store, Vec<crate::cache::Watch>)> {
        None
    }
    /// How system changes get permission; tools that call sudo themselves
    /// (AUR helpers) are pointed at the same prompt.
    fn authorization(&self) -> Authorization {
        Authorization::SudoNonInteractive
    }
    /// Run a user-scoped development tool (`cargo`, `npm`, `pnpm`, `bun`,
    /// `pipx`, `uv`, `composer`, `gem`). Fixtures override this one seam for
    /// every development manager except venv-pinned `pip` (see `venv_pip`).
    fn dev_tool(
        &self,
        executable: &str,
        _args: &[OsString],
        _cancel: &Cancellation,
        _write: bool,
    ) -> Result<Completion, ExecutionError> {
        Err(ExecutionError::Disabled(format!("{executable} not found")))
    }
    /// Run `<venv>/bin/python -m pip` for an explicitly selected virtual
    /// environment. Fixtures override this seam for `pip` tests.
    fn venv_pip(
        &self,
        venv: &std::path::Path,
        _args: &[OsString],
        _cancel: &Cancellation,
        _write: bool,
    ) -> Result<Completion, ExecutionError> {
        let _ = venv;
        Err(ExecutionError::Disabled("pip not found".into()))
    }
    fn system_manager(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        let _ = (args, cancel, write);
        Err(ExecutionError::Disabled(format!("{executable} not found")))
    }
    /// Run `/bin/ps` or `/usr/bin/trash` as the invoking user, for removing
    /// Mac apps. Fixtures override it; elsewhere nothing runs.
    fn macos_tool(
        &self,
        executable: &std::path::Path,
        _args: &[OsString],
        _cancel: &Cancellation,
        _write: bool,
    ) -> Result<Completion, ExecutionError> {
        Err(ExecutionError::Disabled(format!(
            "{} is unavailable",
            executable.display()
        )))
    }
    fn container(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.dev_tool(executable, args, cancel, write)
    }
}

/// A test transport for managers that only run development tools: the fake
/// implements `dev_tool` (and `env` if it needs one), and every system
/// package manager seam refuses.
#[cfg(test)]
pub(crate) mod dev_tools {
    use super::*;

    pub(crate) trait DevTool: Send {
        fn dev_tool(
            &self,
            executable: &str,
            args: &[OsString],
            cancel: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError>;
        fn env(&self, _name: &str) -> Option<OsString> {
            None
        }
    }

    pub(crate) struct DevTools<F>(pub F);

    fn refused() -> ExecutionError {
        ExecutionError::Disabled("only development tools run here".into())
    }

    impl<F: DevTool> Transport for DevTools<F> {
        fn apt_query(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &Cancellation,
        ) -> Result<Completion, ExecutionError> {
            Err(refused())
        }
        fn apt_write(&self, _: AptAction, _: &Cancellation) -> Result<Completion, ExecutionError> {
            Err(refused())
        }
        fn brew(
            &self,
            _: &[OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            Err(refused())
        }
        fn flatpak(
            &self,
            _: &[OsString],
            _: &Cancellation,
            _: bool,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            Err(refused())
        }
        fn env(&self, name: &str) -> Option<OsString> {
            self.0.env(name)
        }
        fn dev_tool(
            &self,
            executable: &str,
            args: &[OsString],
            cancel: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            self.0.dev_tool(executable, args, cancel, write)
        }
    }

    #[test]
    fn only_development_tools_run() {
        struct Echo;
        impl DevTool for Echo {
            fn dev_tool(
                &self,
                executable: &str,
                args: &[OsString],
                _: &Cancellation,
                write: bool,
            ) -> Result<Completion, ExecutionError> {
                Ok(Completion {
                    code: Some(0),
                    signal: None,
                    stdout: format!("{executable} {args:?} {write}").into_bytes(),
                    stderr: vec![],
                    truncated: false,
                    cancellation_deferred: false,
                })
            }
        }
        let transport = DevTools(Echo);
        let cancel = Cancellation::default();
        let ran = transport
            .dev_tool("go", &["version".into()], &cancel, true)
            .unwrap();
        assert_eq!(ran.stdout, b"go [\"version\"] true");
        assert_eq!(transport.env("HOME"), None);
        let refusals = [
            transport.apt_query("search", "x", "amd64", &cancel),
            transport.apt_write(AptAction::Refresh, &cancel),
            transport.brew(&[], &cancel, false),
            transport.flatpak(&[], &cancel, false, false),
        ];
        for refusal in refusals {
            assert!(matches!(refusal, Err(ExecutionError::Disabled(_))));
        }
    }
}

fn invalid(backend: &str, reason: impl ToString) -> EngineError {
    EngineError::InvalidResponse {
        backend: backend.into(),
        reason: reason.to_string(),
    }
}
fn bytes(backend: &str, result: Completion) -> Result<Vec<u8>, EngineError> {
    if result.code != Some(0) {
        return Err(ExecutionError::Failed(result).into());
    }
    if result.truncated {
        return Err(invalid(backend, "metadata exceeded output limit"));
    }
    Ok(result.stdout)
}
fn availability(result: Result<Completion, ExecutionError>) -> Result<Availability, EngineError> {
    match result {
        Ok(_) => Ok(Availability::Available),
        Err(ExecutionError::Disabled(reason)) => Ok(Availability::Unavailable(reason)),
        Err(error) => Err(error.into()),
    }
}

/// Whether an `Unavailable` reason means the tool isn't installed. Missing
/// tools report `"<tool> not found"`; an installed but broken source reports
/// why it's broken, so it doesn't match.
pub fn is_missing_tool_reason(reason: &str) -> bool {
    reason
        .strip_suffix(" not found")
        .is_some_and(|tool| !tool.is_empty() && !tool.contains(' '))
}

#[cfg(test)]
mod missing_tool_reason_tests {
    use super::is_missing_tool_reason;

    #[test]
    fn missing_tools_match_and_broken_installs_do_not() {
        assert!(is_missing_tool_reason("APT not found"));
        assert!(is_missing_tool_reason("mas not found"));
        assert!(!is_missing_tool_reason(" not found"));
        assert!(!is_missing_tool_reason("pipx home not found"));
        assert!(!is_missing_tool_reason(
            "Docker is installed but unavailable: daemon stopped"
        ));
        assert!(!is_missing_tool_reason(
            "mas 6.3.0 is too old; PkgDeck needs mas 7 or newer"
        ));
    }
}

/// Missing optional managers are omitted from automatic queries, but explicit selections
/// and source discovery retain their unavailability. Detection failures are never hidden.
/// Automatic engines detect each source inside its own query, so a slow probe
/// holds back only that source.
///
/// `sources` selects backends by id: empty means every available backend,
/// while a non-empty set registers exactly those members (unavailable ones
/// included, so discovery and explicit queries report their status).
pub fn native_engine(
    sources: &[String],
    discover: bool,
    authorization: Authorization,
    cancel: &Cancellation,
) -> Result<Engine, EngineError> {
    native_engine_on(Host::current(), sources, discover, authorization, cancel)
}
fn native_engine_on(
    host: Host,
    sources: &[String],
    discover: bool,
    authorization: Authorization,
    cancel: &Cancellation,
) -> Result<Engine, EngineError> {
    if sources == [NO_BACKENDS] {
        return Ok(Engine::default());
    }
    if let Some(unknown) = sources.iter().find(|s| !BACKEND_IDS.contains(&s.as_str())) {
        return Err(EngineError::UnknownBackend(unknown.into()));
    }
    let allowed = |id: &str| sources.is_empty() || sources.iter().any(|s| s == id);
    let explicit = !sources.is_empty();
    let mut engine = Engine::default();
    engine.enable_batch_authorization(host.clone(), authorization);
    let transport = || NativeTransport {
        host: host.clone(),
        authorization,
    };
    // Every candidate in registration order, with whether a listing or an
    // explicit selection probes it now. Detection spawns native tools (a Node
    // or Python start-up each), so the probes run concurrently instead of one
    // after another. Sources whose query runs detection anyway are not probed.
    let mut candidates: Vec<(Box<dyn Backend>, bool)> = Vec::new();
    if allowed("apt") {
        candidates.push((Box::new(Apt::new(transport())), true));
    }
    if allowed("homebrew") {
        candidates.push((Box::new(Homebrew::new(transport())), true));
    }
    // Homebrew 6+ installs Linux casks too (AppImages, binaries and fonts).
    if allowed("homebrew-cask") {
        candidates.push((
            Box::new(HomebrewCask::new(transport()).with_adoption()),
            true,
        ));
    }
    if allowed("macos-apps") {
        candidates.push((Box::new(MacApps::native(transport())), true));
    }
    if allowed("mas") {
        candidates.push((Box::new(MacAppStore::new(transport())), true));
    }
    if allowed("macos-updates") {
        candidates.push((Box::new(MacUpdates::new(transport())), true));
    }
    for (backend, make) in [
        (
            "dnf",
            SystemManager::dnf as fn(NativeTransport) -> SystemManager,
        ),
        ("pacman", SystemManager::pacman),
        ("zypper", SystemManager::zypper),
        ("snap", SystemManager::snap),
        ("apk", SystemManager::apk),
        ("xbps", SystemManager::xbps),
        ("macports", SystemManager::macports),
    ] {
        if allowed(backend) {
            candidates.push((Box::new(make(transport())), false));
        }
    }
    if allowed("fwupd") {
        candidates.push((Box::new(Firmware::new(transport())), true));
    }
    if allowed("dev-caches") {
        candidates.push((Box::new(dev_caches::DevCaches::new(host.clone())), true));
    }
    if allowed("projects") {
        candidates.push((Box::new(projects::Projects::new(host.clone())), true));
    }
    if allowed("appimage") {
        // Detection only checks the OS, so probing is free and keeps a
        // Linux-only source out of automatic queries on macOS.
        candidates.push((Box::new(AppImage::native()), true));
    }
    for tool in StandaloneTool::ALL {
        if allowed(tool.id()) {
            candidates.push((Box::new(Standalone::native(tool)), true));
        }
    }
    if allowed("flatpak") {
        // Like every other optional manager, an absent Flatpak stays out of
        // automatic queries instead of failing each one; explicit selections
        // and discovery still report its status.
        candidates.push((Box::new(Flatpak::new(transport())), true));
    }
    for (id, make) in [
        (
            "docker",
            Container::docker as fn(NativeTransport) -> Container<NativeTransport>,
        ),
        ("podman", Container::podman),
    ] {
        if allowed(id) {
            // A Podman compatibility wrapper does not represent a second
            // image store. Leave it out of automatic and multi-source views;
            // an explicit Docker-only selection still explains why it cannot
            // be queried.
            if id == "docker" && host.docker_is_podman_shim() && !(explicit && sources.len() == 1) {
                continue;
            }
            candidates.push((
                Box::new(make(transport()).with_remote_offers(explicit && sources.len() == 1)),
                true,
            ));
        }
    }
    for (id, make) in [
        ("cargo", DevTool::cargo as fn(NativeTransport) -> DevTool),
        ("npm", DevTool::npm),
        ("pnpm", DevTool::pnpm),
        ("bun", DevTool::bun),
        ("pip", DevTool::pip),
        ("pipx", DevTool::pipx),
        ("uv", DevTool::uv),
        ("mise", DevTool::mise),
        ("composer", DevTool::composer),
        ("gem", DevTool::gem),
    ] {
        if allowed(id) {
            candidates.push((Box::new(make(transport())), false));
        }
    }
    if allowed("pixi") {
        candidates.push((Box::new(Pixi::new(transport())), false));
    }
    if allowed("conda") {
        // Detection picks conda, mamba, or micromamba, so it always runs.
        candidates.push((Box::new(Conda::new(transport())), true));
    }
    if allowed("rustup") {
        candidates.push((Box::new(Rustup::new(transport())), false));
    }
    if allowed("oh-my-zsh") {
        candidates.push((Box::new(OhMyZsh::new(transport())), false));
    }
    if allowed("nix") {
        candidates.push((Box::new(Nix::new(transport())), false));
    }
    if allowed("go") {
        candidates.push((Box::new(GoBinaries::new(transport())), false));
    }
    if allowed("dotnet") {
        candidates.push((Box::new(DotnetTools::new(transport())), false));
    }
    // Both check the platform first, so probing is cheap elsewhere.
    if allowed("aur") {
        candidates.push((Box::new(Aur::new(transport())), true));
    }
    // Detection checks the platform first, so probing is cheap elsewhere.
    for (id, make) in [
        (
            "toolbox",
            DevContainers::toolbox as fn(NativeTransport) -> DevContainers<NativeTransport>,
        ),
        ("distrobox", DevContainers::distrobox),
    ] {
        if allowed(id) {
            candidates.push((Box::new(make(transport())), true));
        }
    }
    if allowed("system-image") {
        candidates.push((Box::new(SystemImage::new(transport())), true));
    }
    if !(discover || explicit) {
        for (backend, _) in candidates {
            engine.register_optional(backend)?;
        }
        return Ok(engine);
    }
    type Probed = (Box<dyn Backend>, Option<Result<Availability, EngineError>>);
    let probed: Vec<Probed> = std::thread::scope(|scope| {
        let workers: Vec<_> = candidates
            .into_iter()
            .map(|(mut backend, probe)| {
                scope.spawn(move || {
                    let status = probe.then(|| backend.detect(cancel));
                    (backend, status)
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("detection worker panicked"))
            .collect()
    });
    for (backend, status) in probed {
        let id = backend.id().to_string();
        // A detected backend keeps what detection learned (such as a
        // manager's home), so the following query skips its own probe.
        if let Some(status) = status {
            engine.note_detected(id, status);
        }
        engine.register_boxed(backend)?;
    }
    Ok(engine)
}
