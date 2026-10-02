//! Native package-manager, container image, Homebrew formula/cask, and local AppImage adapters.
mod adopt;
mod ai_catalog;
mod appimage;
mod apt_cli;
mod aur;
mod cleanup;
mod conda;
mod container;
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
pub use aur::Aur;
pub use conda::Conda;
pub use container::{Container, ContainerKind};
pub use dev_containers::DevContainers;
pub use dotnet::DotnetTools;
pub use firmware::Firmware;
pub use go_bin::GoBinaries;
pub use mac_apps::MacApps;
pub use macos_updates::MacUpdates;
pub use mas::MacAppStore;
pub use nix::Nix;
pub use oh_my_zsh::OhMyZsh;
pub use pixi::Pixi;
pub use rustup::Rustup;
use serde::Deserialize;
pub use standalone::{Standalone, StandaloneTool};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::PathBuf,
    time::Duration,
};
pub use system_image::SystemImage;

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
    /// Sandboxed host APT query; only the native transport implements it.
    /// Fixtures keep the default because they bypass host execution.
    fn apt_query_sandboxed(
        &self,
        _mode: &str,
        _query: &str,
        _arch: &str,
        _cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        Err(ExecutionError::Disabled("APT not found".into()))
    }
    /// Read one sanitized host environment value, if the fixture provides it.
    fn env(&self, _name: &str) -> Option<OsString> {
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

pub struct NativeTransport {
    pub host: Host,
    pub authorization: Authorization,
}

fn apt_query_executable(
    executable: &std::path::Path,
    built: Option<&str>,
) -> Result<PathBuf, ExecutionError> {
    let adjacent = executable.with_file_name("pkgdeck-apt-query");
    if adjacent.is_file() {
        return Ok(adjacent);
    }
    if let Some(path) = built.map(PathBuf::from).filter(|path| path.is_file()) {
        return Ok(path);
    }
    Err(ExecutionError::Disabled(
        "APT helper is missing. Rebuild with libapt-pkg-dev installed, or reinstall PkgDeck."
            .into(),
    ))
}

/// Everything the APT helper's answers depend on: installed state (dpkg
/// status and its pending journal), package lists, APT configuration,
/// sources, pins, and the machine id that phased updates use.
fn apt_watches() -> Vec<crate::cache::Watch> {
    use crate::cache::Watch;
    vec![
        Watch::file("/var/lib/dpkg/status"),
        Watch::tree("/var/lib/dpkg/updates", 1),
        Watch::file("/var/lib/dpkg/arch"),
        Watch::tree("/var/lib/apt/lists", 1),
        Watch::file("/var/lib/apt/extended_states"),
        Watch::tree("/etc/apt", 2),
        Watch::file("/etc/machine-id"),
    ]
}

/// What `flatpak search` answers from: the installation's AppStream data
/// (`appstream/<remote>/<arch>/active` flips on each refresh) and its
/// remote configuration.
fn flatpak_search_watches(installation: &std::path::Path) -> Vec<crate::cache::Watch> {
    use crate::cache::Watch;
    vec![
        Watch::tree(installation.join("appstream"), 3),
        Watch::file(installation.join("repo/config")),
        Watch::tree("/etc/flatpak", 2),
        Watch::file("/usr/bin/flatpak"),
    ]
}

/// The installation folder whose data `flatpak search` reads, or `None`
/// when it can't be known for certain (a relocated installation).
fn flatpak_installation(system: bool, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if var("FLATPAK_USER_DIR").is_some() || var("FLATPAK_SYSTEM_DIR").is_some() {
        return None;
    }
    if system {
        return Some(PathBuf::from("/var/lib/flatpak"));
    }
    let data = var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".local/share")))?;
    Some(data.join("flatpak"))
}

/// Locale settings that change translated names and descriptions.
fn locale_key(var: impl Fn(&str) -> Option<OsString>) -> Vec<String> {
    ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .map(|name| {
            var(name)
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect()
}

impl NativeTransport {
    /// Flatpak reads, with remote searches answered from `store` while the
    /// installation's AppStream data is unchanged.
    fn flatpak_cached(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
        system: bool,
        store: impl FnOnce() -> Option<crate::cache::Store>,
    ) -> Result<Completion, ExecutionError> {
        let run = || {
            self.host
                .flatpak(args, cancel, write, system, self.authorization)
        };
        // Remote search reads the local AppStream copy and takes about a
        // second per installation. Other reads are fast or remote.
        let search = !write
            && self.host.runtime == crate::host::Runtime::Native
            && args.get(1).is_some_and(|arg| arg == "search");
        let Some(installation) = search
            .then(|| flatpak_installation(system, |name| self.host.var(name)))
            .flatten()
        else {
            return run();
        };
        let args_text: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let args_text: Vec<&str> = args_text.iter().map(String::as_str).collect();
        // Names and descriptions come translated.
        let locale = locale_key(|name| self.host.var(name));
        let locale: Vec<&str> = locale.iter().map(String::as_str).collect();
        crate::cache::completion(
            store().as_ref(),
            "flatpak",
            "search",
            &args_text,
            &flatpak_search_watches(&installation),
            &locale,
            run,
        )
    }
}

impl Transport for NativeTransport {
    fn system_flatpak_writable(&self) -> bool {
        self.host.system_flatpak_available()
    }
    fn repository_editor_available(&self) -> bool {
        self.host.source_editor_available(self.authorization)
    }
    fn repository_editor(&self) -> Result<(), ExecutionError> {
        self.host.open_source_editor(self.authorization)
    }
    fn apt_write_group(
        &self,
        actions: &[AptAction],
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        self.host.apt_group(actions, self.authorization, cancel)
    }
    fn apt_query(
        &self,
        mode: &str,
        query: &str,
        arch: &str,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        if self.host.resolve("apt-get")?.is_none() {
            return Err(ExecutionError::Disabled("APT not found".into()));
        }
        if self.host.runtime == crate::host::Runtime::Flatpak {
            return self.apt_query_sandboxed(mode, query, arch, cancel);
        }
        let executable = apt_query_executable(
            &std::env::current_exe().map_err(|e| ExecutionError::Io(e.to_string()))?,
            option_env!("PKGDECK_BUILT_APT_QUERY"),
        )?;
        // The helper rebuilds APT's cache in memory on every run (about a
        // second). Its answer only depends on the APT and dpkg databases.
        let cacheable = mode != "detect" && self.host.var("APT_CONFIG").is_none();
        let store = cacheable.then(crate::cache::Store::user).flatten();
        let helper = crate::cache::fingerprint(&[crate::cache::Watch::file(&executable)], &[]);
        let result = crate::cache::completion(
            store.as_ref(),
            "apt",
            mode,
            &[query, arch],
            &apt_watches(),
            &[helper.as_deref().unwrap_or_default()],
            || {
                self.host.read(
                    &executable,
                    &[mode.into(), query.into(), arch.into()],
                    Limits {
                        timeout: Duration::from_secs(120),
                        output_bytes: 32 * 1024 * 1024,
                    },
                    cancel,
                )
            },
        )?;
        if result.code == Some(0) {
            Ok(result)
        } else {
            Err(ExecutionError::Failed(result))
        }
    }
    /// Sandboxed host APT query without interpreter payloads: drives the
    /// host's `dpkg-query`/`apt-cache` through the host bridge and assembles
    /// the same records as the native helper. Read-only; never writes cache.
    fn apt_query_sandboxed(
        &self,
        mode: &str,
        query: &str,
        arch: &str,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        fn finish(details: Vec<PackageDetails>) -> Result<Completion, ExecutionError> {
            Ok(Completion {
                code: Some(0),
                signal: None,
                stdout: serde_json::to_vec(&details)
                    .map_err(|error| ExecutionError::Io(error.to_string()))?,
                stderr: vec![],
                truncated: false,
                cancellation_deferred: false,
            })
        }
        let limits = Limits {
            timeout: Duration::from_secs(120),
            output_bytes: 32 * 1024 * 1024,
        };
        let missing = || ExecutionError::Disabled("APT not found".into());
        let dpkg = self.host.resolve("dpkg-query")?.ok_or_else(missing)?;
        let cache = self.host.resolve("apt-cache")?.ok_or_else(missing)?;
        let output = |executable: &std::path::Path, args: &[OsString]| {
            let result = self.host.read(executable, args, limits, cancel)?;
            if result.code == Some(0) && !result.truncated {
                Ok(String::from_utf8_lossy(&result.stdout).into_owned())
            } else {
                Err(ExecutionError::Failed(result))
            }
        };
        let installed_rows = || {
            output(
                &dpkg,
                &[
                    OsString::from("-W"),
                    OsString::from("-f${Package}\t${Architecture}\t${Version}\t${Status}\n"),
                ],
            )
            .map(|text| apt_cli::parse_dpkg_table(&text))
        };
        match mode {
            "detect" => finish(vec![]),
            "installed" => {
                let rows = installed_rows()?;
                if rows.is_empty() {
                    return finish(vec![]);
                }
                let mut names: Vec<OsString> = rows
                    .iter()
                    .map(|row| {
                        if row.arch == "all" {
                            OsString::from(&row.name)
                        } else {
                            OsString::from(format!("{}:{}", row.name, row.arch))
                        }
                    })
                    .collect();
                names.sort();
                names.dedup();
                let policy = output(&cache, &[&[OsString::from("policy")], &names[..]].concat())?;
                let show = output(&cache, &[&[OsString::from("show")], &names[..]].concat())?;
                finish(apt_cli::installed_packages(
                    &rows,
                    &apt_cli::parse_policy_dump(&policy),
                    &apt_cli::parse_show_dump(&show),
                ))
            }
            "search" => {
                // Literal-substring semantics without handing user text to the
                // native matcher: narrow server-side with escaped patterns,
                // one per word (apt-cache needs all of them), then filter on
                // name plus short description in Rust.
                let escape = |word: &str| -> OsString {
                    word.chars()
                        .flat_map(|char| {
                            if ".[{()*+?^$|\\".contains(char) {
                                vec!['\\', char]
                            } else {
                                vec![char]
                            }
                        })
                        .collect::<String>()
                        .into()
                };
                let mut args = vec![OsString::from("search")];
                args.extend(
                    query
                        .split(SEARCH_SEPARATORS)
                        .filter(|word| !word.is_empty())
                        .map(escape),
                );
                if args.len() == 1 {
                    args.push(escape(query));
                }
                let dump = output(&cache, &args)?;
                let candidates = apt_cli::parse_search_dump(&dump);
                let mut names: Vec<OsString> = candidates
                    .iter()
                    .filter(|(name, summary)| search_matches(&format!("{name} {summary}"), query))
                    .map(|(name, _)| OsString::from(name))
                    .collect();
                names.sort();
                names.dedup();
                if names.is_empty() {
                    return finish(vec![]);
                }
                let policy = output(&cache, &[&[OsString::from("policy")], &names[..]].concat())?;
                let show = output(&cache, &[&[OsString::from("show")], &names[..]].concat())?;
                finish(apt_cli::search_packages(
                    query,
                    &apt_cli::parse_search_dump(&dump),
                    &apt_cli::parse_policy_dump(&policy),
                    &apt_cli::parse_show_dump(&show),
                    &installed_rows()?,
                ))
            }
            "details" => {
                let target = if arch == "all" {
                    query.to_owned()
                } else {
                    format!("{query}:{arch}")
                };
                let policy = output(&cache, &[OsString::from("policy"), OsString::from(&target)])?;
                let show = output(&cache, &[OsString::from("show"), OsString::from(&target)])?;
                let parsed = apt_cli::parse_policy_dump(&policy);
                // An empty record lets the backend report NotFound exactly
                // like the native helper does for unknown identities.
                finish(
                    apt_cli::details_package(
                        query,
                        arch,
                        &parsed,
                        &apt_cli::parse_show_dump(&show),
                        &installed_rows()?,
                    )
                    .into_iter()
                    .collect(),
                )
            }
            _ => Err(ExecutionError::Invalid("unknown APT query".into())),
        }
    }
    fn apt_write(
        &self,
        action: AptAction,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        self.host.apt(action, self.authorization, cancel)
    }
    fn brew(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.host.brew_asking(
            args,
            cancel,
            write,
            matches!(self.authorization, Authorization::Polkit),
        )
    }
    fn flatpak(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
        system: bool,
    ) -> Result<Completion, ExecutionError> {
        self.flatpak_cached(args, cancel, write, system, crate::cache::Store::user)
    }
    fn flatpak_unused(&self, cancel: &Cancellation) -> Result<Completion, ExecutionError> {
        let _ = cancel;
        Err(ExecutionError::Disabled(
            "Flatpak cannot show an exact preview of unused runtimes".into(),
        ))
    }
    fn supports_flatpak_cleanup(&self) -> bool {
        false
    }
    fn docker_is_podman(&self) -> bool {
        self.host.docker_is_podman_shim()
    }
    fn env(&self, name: &str) -> Option<OsString> {
        self.host.var(name)
    }
    fn authorization(&self) -> Authorization {
        self.authorization
    }
    fn dev_tool(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        let label = match executable {
            "cargo" => "Cargo",
            "npm" => "npm",
            "pnpm" => "pnpm",
            "bun" => "Bun",
            "pipx" => "pipx",
            "uv" => "uv",
            "composer" => "Composer",
            "gem" => "RubyGems",
            "git" => "Git",
            other => other,
        };
        self.host.dev_tool(executable, label, args, cancel, write)
    }
    fn venv_pip(
        &self,
        venv: &std::path::Path,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.host.venv_pip(venv, args, cancel, write)
    }
    fn system_manager(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.host
            .system_manager(executable, args, cancel, write, self.authorization)
    }
    fn macos_tool(
        &self,
        executable: &std::path::Path,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.host.macos_tool(executable, args, cancel, write)
    }
    fn container(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        let label = if executable == "docker" {
            "Docker"
        } else {
            "Podman"
        };
        self.host
            .container_engine(executable, label, args, cancel, write)
    }
}

pub struct Apt<T = NativeTransport> {
    pub transport: T,
    desktop_entries: Option<std::collections::BTreeMap<String, PathBuf>>,
    components: Option<std::collections::BTreeMap<String, Vec<String>>>,
}
impl<T> Apt<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            desktop_entries: None,
            components: None,
        }
    }
}
pub struct Homebrew<T = NativeTransport> {
    pub transport: T,
    prefix: Option<PathBuf>,
    /// Set for an update check so formulae and casks share one `brew update`.
    update_check: Option<u64>,
    /// Search details already read, while this engine is reused.
    found: BrewFound,
}
pub struct HomebrewCask<T = NativeTransport> {
    pub transport: T,
    prefix: Option<PathBuf>,
    /// Checks and backs up an app already in the cask's place before
    /// Homebrew adopts it; `None` never adopts (fixture transports).
    adoption: Option<Box<dyn adopt::AdoptIo>>,
    /// Set for an update check so formulae and casks share one `brew update`.
    update_check: Option<u64>,
    /// Search details already read, while this engine is reused.
    found: BrewFound,
}
pub struct Flatpak<T = NativeTransport> {
    transport: T,
    /// Remotes the last installed query could not ask about updates. The
    /// rest were asked, so their rows are complete.
    skipped: Vec<EngineError>,
}
fn flatpak_offer_reference(reference: &str) -> Option<(&str, &str, &str)> {
    let mut parts = reference.split('/');
    let (name, arch, branch) = (parts.next()?, parts.next()?, parts.next()?);
    (flatpak_id(name) && flatpak_id(arch) && flatpak_id(branch) && parts.next().is_none())
        .then_some((name, arch, branch))
}
impl<T: Transport> Flatpak<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            skipped: vec![],
        }
    }
}
/// One `brew update` at a time across checks. Callers that share a token
/// already share one fetch; this only keeps a second check, or an explicit
/// refresh, from taking Homebrew's git lock at the same moment. The next
/// check still fetches. It does not reuse a finished result.
fn brew_update_turn() -> std::sync::MutexGuard<'static, ()> {
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
impl<T: Transport> Homebrew<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            prefix: None,
            update_check: None,
            found: BrewFound::default(),
        }
    }
}
impl<T: Transport> HomebrewCask<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            prefix: None,
            adoption: None,
            update_check: None,
            found: BrewFound::default(),
        }
    }
    /// Casks can take over apps someone installed themselves (macOS only).
    pub fn with_adoption(self) -> Self {
        #[cfg(target_os = "macos")]
        return Self {
            adoption: Some(Box::new(adopt::NativeAdopt(Host::current()))),
            ..self
        };
        #[cfg(not(target_os = "macos"))]
        self
    }
}

fn flatpak_id(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
fn flatpak_reference(reference: &str) -> Option<(&str, &str, &str, &str)> {
    let mut parts = reference.split('/');
    let (kind, name, arch, branch) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    (matches!(kind, "app" | "runtime")
        && flatpak_id(name)
        && flatpak_id(arch)
        && flatpak_id(branch)
        && parts.next().is_none())
    .then_some((kind, name, arch, branch))
}
impl<T: Transport> Flatpak<T> {
    fn call(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
        system: bool,
    ) -> Result<Completion, EngineError> {
        let result = self.transport.flatpak(
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            cancel,
            write,
            system,
        )?;
        if write {
            bytes("flatpak", result.clone())?;
        }
        Ok(result)
    }
    fn list(
        &self,
        cancel: &Cancellation,
        system: bool,
        updates: bool,
        skipped: &mut Vec<EngineError>,
    ) -> Result<Vec<Package>, EngineError> {
        let scope = if system {
            Scope::System
        } else {
            Scope::User {
                uid: rustix::process::getuid().as_raw(),
            }
        };
        let prefix = if system { "--system" } else { "--user" };
        let output = bytes(
            "flatpak",
            self.call(
                &[
                    prefix,
                    "list",
                    "--columns=application,arch,branch,version,description,origin,options,name",
                ],
                cancel,
                false,
                system,
            )?,
        )?;
        let text = String::from_utf8(output).map_err(|e| invalid("flatpak", e))?;
        let mut packages = Vec::new();
        let mut origins = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let fields: Vec<_> = line.split('\t').collect();
            if !(6..=8).contains(&fields.len())
                || !flatpak_id(fields[0])
                || !flatpak_id(fields[1])
                || !flatpak_id(fields[2])
                || (!fields[5].is_empty() && !flatpak_id(fields[5]))
            {
                return Err(invalid("flatpak", "invalid list metadata"));
            }
            let runtime = fields
                .get(6)
                .is_some_and(|options| options.split(',').any(|option| option.trim() == "runtime"));
            let kind = if runtime { "runtime" } else { "app" };
            let reference = format!("{kind}/{}/{}/{}", fields[0], fields[1], fields[2]);
            origins.push(fields[5].to_owned());
            packages.push(Package {
                id: PackageId {
                    backend: "flatpak".into(),
                    name: fields[0].into(),
                    architecture: fields[1].into(),
                    scope: scope.clone(),
                    remote: None,
                    reference: Some(reference),
                },
                display_name: fields
                    .get(7)
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(&fields[0])
                    .trim()
                    .into(),
                summary: if runtime {
                    format!("Runtime {} ({})", fields[2], fields[4])
                } else {
                    fields[4].into()
                },
                installed_version: Some(fields[3].into()),
                candidate_version: Some(fields[3].into()),
                update: UpdateAvailability::Current,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
                adopt_with: None,
            });
        }
        if packages.is_empty() || !updates {
            return Ok(packages);
        }
        // Flatpak compares commits, not version labels: rebuilds and runtimes
        // can have an update even when the version is unchanged or empty.
        let remote_updates = |remote: Option<&str>| -> Result<String, EngineError> {
            let mut args = vec![
                prefix,
                "remote-ls",
                "--updates",
                "--columns=ref,version,origin",
            ];
            args.extend(remote);
            let output = bytes("flatpak", self.call(&args, cancel, false, system)?)?;
            String::from_utf8(output).map_err(|e| invalid("flatpak", e))
        };
        let remotes: std::collections::BTreeSet<_> =
            origins.iter().filter(|origin| !origin.is_empty()).collect();
        let mut texts = Vec::new();
        match remote_updates(None) {
            Ok(text) => texts.push(text),
            // One unreachable remote makes flatpak refuse to list any. Ask
            // each remote on its own so the others still show their updates.
            Err(first) if !cancel.requested() && remotes.len() > 1 => {
                let mut failures = Vec::new();
                for remote in remotes {
                    match remote_updates(Some(remote)) {
                        Ok(text) => texts.push(text),
                        Err(error) => failures.push(error),
                    }
                }
                if texts.is_empty() || cancel.requested() {
                    return Err(first);
                }
                skipped.extend(failures);
            }
            Err(error) => return Err(error),
        }
        for updates in texts {
            for line in updates.lines().filter(|line| !line.trim().is_empty()) {
                let fields: Vec<_> = line.split('\t').collect();
                if fields.len() != 3
                    || flatpak_reference(fields[0]).is_none()
                    || !flatpak_id(fields[2])
                {
                    return Err(invalid("flatpak", "invalid update metadata"));
                }
                for (package, origin) in packages.iter_mut().zip(&origins) {
                    if package.id.reference.as_deref() == Some(fields[0]) && origin == fields[2] {
                        package.candidate_version = Some(fields[1].into());
                        package.update = UpdateAvailability::Available;
                    }
                }
            }
        }
        Ok(packages)
    }
    fn search_scope(
        &self,
        query: &str,
        cancel: &Cancellation,
        system: bool,
    ) -> Result<Vec<Package>, EngineError> {
        let scope = if system {
            Scope::System
        } else {
            Scope::User {
                uid: rustix::process::getuid().as_raw(),
            }
        };
        let prefix = if system { "--system" } else { "--user" };
        let output = bytes(
            "flatpak",
            self.call(
                &[
                    prefix,
                    "search",
                    "--columns=name,description,application,version,branch,remotes",
                    query,
                ],
                cancel,
                false,
                system,
            )?,
        )?;
        let output = String::from_utf8(output).map_err(|e| invalid("flatpak", e))?;
        // Without matches flatpak prints a translated notice such as
        // "No matches found" instead of rows. Every real row has tabs.
        if !output.contains('\t') {
            return Ok(vec![]);
        }
        output
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let fields: Vec<_> = line.split('\t').collect();
                let remote = fields
                    .get(5)
                    .and_then(|value| value.split(',').next())
                    .filter(|value| flatpak_id(value));
                if fields.len() != 6
                    || !flatpak_id(fields[2])
                    || !flatpak_id(fields[4])
                    || remote.is_none()
                {
                    return Err(invalid("flatpak", "invalid remote search metadata"));
                }
                Ok(Package {
                    id: PackageId {
                        backend: "flatpak".into(),
                        name: fields[2].into(),
                        architecture: std::env::consts::ARCH.into(),
                        scope: scope.clone(),
                        remote: remote.map(str::to_owned),
                        reference: Some(format!(
                            "{}/{}/{}",
                            fields[2],
                            std::env::consts::ARCH,
                            fields[4]
                        )),
                    },
                    display_name: fields[0].into(),
                    summary: fields[1].into(),
                    installed_version: None,
                    candidate_version: Some(fields[3].into()),
                    update: UpdateAvailability::Unknown,
                    icon: None,
                    component_ids: vec![],
                    homepages: vec![],
                    adopt_with: None,
                })
            })
            .collect()
    }
    fn target(&self, id: &PackageId) -> Result<(bool, &'static str), EngineError> {
        if id.backend != "flatpak" || !flatpak_id(&id.name) || !flatpak_id(&id.architecture) {
            return Err(invalid("flatpak", "foreign or invalid Flatpak identity"));
        }
        if let Some(reference) = &id.reference {
            let (name, arch, _) = flatpak_reference(reference)
                .map(|(_, name, arch, branch)| (name, arch, branch))
                .or_else(|| flatpak_offer_reference(reference))
                .ok_or_else(|| invalid("flatpak", "invalid native ref"))?;
            if name != id.name || arch != id.architecture {
                return Err(invalid(
                    "flatpak",
                    "native ref does not match package identity",
                ));
            }
        }
        match id.scope {
            Scope::System => Ok((true, "--system")),
            Scope::User { .. } => Ok((false, "--user")),
            _ => Err(invalid(
                "flatpak",
                "Flatpak packages must be user or system scoped",
            )),
        }
    }
}
impl<T: Transport> Backend for Flatpak<T> {
    fn id(&self) -> &str {
        "flatpak"
    }
    /// Flatpak ids are reverse-DNS names with at least one dot, and refs
    /// contain slashes, so a plain name such as `cowsay` never matches.
    fn may_have(&self, name: &str) -> bool {
        (name.contains('.') || name.contains('/'))
            && name
                .split('/')
                .all(|part| part.is_empty() || flatpak_id(part))
    }
    fn capabilities(&self) -> &[Capability] {
        if self.transport.supports_flatpak_cleanup() {
            CLEAN_CAPABILITIES
        } else {
            CAPABILITIES
        }
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        match self.call(
            &["--user", "remotes", "--columns=name"],
            cancel,
            false,
            false,
        ) {
            Ok(_) => Ok(Availability::Available),
            Err(EngineError::Execution(ExecutionError::Disabled(reason))) => {
                Ok(Availability::Unavailable(reason))
            }
            Err(e) => Err(e),
        }
    }
    fn query_errors(&self) -> Vec<EngineError> {
        self.skipped.clone()
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        if query.trim().is_empty() || query.starts_with('-') {
            return Err(invalid("flatpak", "expected a search term"));
        }
        self.skipped.clear();
        if flatpak_reference(query).is_some() {
            return self.installed(cancel).map(|packages| {
                packages
                    .into_iter()
                    .filter(|package| package.id.reference.as_deref() == Some(query))
                    .collect()
            });
        }
        // The user catalog is the primary source; a failing system query
        // (for example, a machine with no system remotes) must not fail
        // the whole search when user results are available.
        let mut result = self.search_scope(query, cancel, false)?;
        if let Ok(system) = self.search_scope(query, cancel, true) {
            result.extend(system);
        }
        // Offers in separate installations remain independently selectable.
        let mut seen = std::collections::BTreeSet::new();
        result.retain(|package| seen.insert(package.id.clone()));
        // Local inventory is enough to choose Install versus Remove. Search
        // must not fetch remote update metadata just to establish this state.
        let mut installed = self.list(cancel, false, false, &mut vec![])?;
        installed.extend(self.list(cancel, true, false, &mut vec![])?);
        let mut offers = Vec::new();
        for offer in result {
            let matches: Vec<_> =
                installed
                    .iter()
                    .filter(|package| {
                        package.id.scope == offer.id.scope
                            && package.id.reference.as_deref().and_then(|reference| {
                                reference.split_once('/').map(|(_, rest)| rest)
                            }) == offer.id.reference.as_deref()
                    })
                    .collect();
            if matches.is_empty() {
                offers.push(offer);
            } else {
                for package in matches {
                    let mut row = offer.clone();
                    row.id = package.id.clone();
                    row.installed_version = package.installed_version.clone();
                    offers.push(row);
                }
            }
        }
        offers.dedup_by(|a, b| a.id == b.id);
        Ok(offers)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let home = self.transport.env("HOME").map(PathBuf::from);
        let mut skipped = Vec::new();
        let mut result = self.list(cancel, false, true, &mut skipped)?;
        result.extend(self.list(cancel, true, true, &mut skipped)?);
        self.skipped = skipped;
        for package in &mut result {
            // Exported icons double as the installed check per scope; `list`
            // only reports user and system installations.
            let root = if package.id.scope == Scope::System {
                Some(PathBuf::from("/var/lib/flatpak/exports"))
            } else {
                home.as_ref()
                    .map(|home| home.join(".local/share/flatpak/exports"))
            };
            if let Some(root) = root {
                package.icon = flatpak_icon(&[root], &package.id.name);
            }
            // The Flatpak application id is itself the AppStream component id.
            if package
                .id
                .reference
                .as_deref()
                .is_some_and(|reference| reference.starts_with("app/"))
            {
                package.component_ids = vec![package.id.name.clone()];
            }
        }
        Ok(result)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        if id.remote.is_some() {
            return self
                .search(id.name.as_str(), cancel)?
                .into_iter()
                .find(|package| package.id == *id)
                .map(|package| PackageDetails {
                    description: package.summary.clone(),
                    homepage: None,
                    dependencies: vec![],
                    package,
                })
                .ok_or(EngineError::NotFound);
        }
        let package = self
            .installed(cancel)?
            .into_iter()
            .find(|p| p.id == *id)
            .ok_or(EngineError::NotFound)?;
        Ok(PackageDetails {
            description: package.summary.clone(),
            homepage: None,
            dependencies: vec![],
            package,
        })
    }
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        if !self.transport.supports_flatpak_cleanup() {
            return Err(self.unsupported(Capability::Clean));
        }
        self.cleanup_unused(cancel)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        if let Operation::Install(id) = operation {
            if id
                .reference
                .as_deref()
                .is_some_and(|value| value.starts_with("artifact:flatpak:"))
            {
                // The scope is checked before the bundle is fetched.
                let (system, scope) = match id.scope {
                    Scope::System => (true, "--system"),
                    Scope::User { uid } if uid == rustix::process::getuid().as_raw() => {
                        (false, "--user")
                    }
                    _ => return Err(invalid("flatpak", "invalid Flatpak scope")),
                };
                let missing = || invalid("flatpak", "missing bundle");
                let staged = crate::artifact::stage(id, cancel)?.ok_or_else(missing)?;
                progress(Progress::Message(format!(
                    "Installing Flatpak bundle {}.",
                    id.name
                )));
                let result = self.call(
                    &[
                        scope,
                        "install",
                        "--noninteractive",
                        "--assumeyes",
                        "--bundle",
                        &staged.path().to_string_lossy(),
                    ],
                    cancel,
                    true,
                    system,
                )?;
                return Ok(OperationOutcome {
                    cancellation_deferred: result.cancellation_deferred,
                });
            }
            if let Some(bytes) = crate::flatpak_ref::verified_source(id, cancel)? {
                // Verification accepts only system and current-user scopes.
                let (system, scope) = if id.scope == Scope::System {
                    (true, "--system")
                } else {
                    (false, "--user")
                };
                progress(Progress::Message(format!(
                    "Installing Flatpak reference for {}.",
                    id.name
                )));
                let temporary = std::env::temp_dir().join(format!(
                    "pkgdeck-flatpakref-{}-{}.flatpakref",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                ));
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)
                    .map_err(|e| invalid("flatpak", e))?;
                use std::io::Write;
                let written = file.write_all(&bytes);
                drop(file);
                let source = temporary.to_string_lossy().into_owned();
                let result = written
                    .map_err(|error| invalid("flatpak", error))
                    .and_then(|()| {
                        self.call(
                            &[
                                scope,
                                "install",
                                "--noninteractive",
                                "--assumeyes",
                                "--from",
                                &source,
                            ],
                            cancel,
                            true,
                            system,
                        )
                    });
                let _ = std::fs::remove_file(&temporary);
                let result = result?;
                return Ok(OperationOutcome {
                    cancellation_deferred: result.cancellation_deferred,
                });
            }
        }
        if let Operation::Clean(id) = operation {
            if !self.transport.supports_flatpak_cleanup() {
                return Err(self.unsupported(Capability::Clean));
            }
            return self.clean_unused(id, cancel);
        }
        let (id, verb) = match operation {
            Operation::Refresh { backend } if backend == "flatpak" => {
                for (system, scope) in [(false, "--user"), (true, "--system")] {
                    self.call(
                        &[
                            scope,
                            "update",
                            "--no-deploy",
                            "--noninteractive",
                            "--assumeyes",
                        ],
                        cancel,
                        true,
                        system,
                    )?;
                }
                return Ok(OperationOutcome::default());
            }
            Operation::Install(id) => (id, "install"),
            Operation::Remove(id) => (id, "uninstall"),
            Operation::Upgrade(id) => (id, "update"),
            Operation::UpgradeAll { backend } if backend == "flatpak" => {
                for (system, scope) in [(false, "--user"), (true, "--system")] {
                    self.call(
                        &[scope, "update", "--noninteractive", "--assumeyes"],
                        cancel,
                        true,
                        system,
                    )?;
                }
                progress(Progress::Message("Updated Flatpak packages.".into()));
                return Ok(OperationOutcome::default());
            }
            _ => return Err(invalid("flatpak", "foreign operation")),
        };
        let (system, scope) = self.target(id)?;
        progress(Progress::Message(format!(
            "Running Flatpak {verb} for {}.",
            id.name
        )));
        let reference = id.reference.as_deref().unwrap_or(&id.name);
        let mut args = vec![scope, verb, "--noninteractive", "--assumeyes"];
        if reference.starts_with("runtime/") {
            args.push("--runtime");
        } else if reference.starts_with("app/") || id.reference.is_none() {
            args.push("--app");
        }
        // Search metadata does not identify app versus runtime. Preserve its
        // name/architecture/branch and let Flatpak resolve the kind.
        if verb == "install" {
            args.push(id.remote.as_deref().unwrap_or("flathub"));
        }
        args.push(reference);
        let result = self.call(&args, cancel, true, system)?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
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

/// Local application icons, resolved without network access for installed
/// packages only. Remote catalog entries never carry icons: fetching them
/// would add a download cache with eviction semantics on top of every
/// backend. Names that cannot resolve keep the generic source icon.
fn icon_file(dir: &std::path::Path, stem: &str) -> Option<PathBuf> {
    for extension in ["png", "svg", "xpm"] {
        let candidate = dir.join(format!("{stem}.{extension}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Installed snaps expose their icon beside the desktop entry. `sysroot`
/// is `/` on a real system and a fixture directory in tests.
fn snap_icon(sysroot: &std::path::Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return None;
    }
    icon_file(
        &sysroot.join("snap").join(name).join("current/meta/gui"),
        "icon",
    )
}

/// Installed Flatpak applications export icons per installation scope.
/// `export_roots` holds the `exports` directories, user scope first.
fn flatpak_icon(export_roots: &[PathBuf], app_id: &str) -> Option<PathBuf> {
    if !flatpak_id(app_id) {
        return None;
    }
    for root in export_roots {
        for size in ["128x128", "64x64", "48x48", "32x32"] {
            let dir = root.join("share/icons/hicolor").join(size).join("apps");
            if let Some(icon) = icon_file(&dir, app_id) {
                return Some(icon);
            }
        }
    }
    None
}

/// Map installed Debian packages to their first desktop entry by scanning
/// the dpkg file lists once per query. File names are `<package>.list` or
/// `<package>:<arch>.list`; only GUI applications resolve.
fn apt_desktop_map(info_dir: &std::path::Path) -> std::collections::BTreeMap<String, PathBuf> {
    let mut map = std::collections::BTreeMap::new();
    let entries = std::fs::read_dir(info_dir).into_iter().flatten().flatten();
    for entry in entries {
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = file.strip_suffix(".list") else {
            continue;
        };
        let package = stem.split_once(':').map_or(stem, |(name, _)| name);
        if package.is_empty() || map.contains_key(package) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        if let Some(desktop) = content.lines().find(|line| {
            line.strip_prefix("/usr/share/applications/")
                .is_some_and(|path| path.ends_with(".desktop"))
        }) {
            map.insert(package.to_owned(), PathBuf::from(desktop));
        }
    }
    map
}

/// Resolve a desktop entry's `Icon` value: absolute paths are used directly
/// while bare names search the icon theme. `home` selects the user theme
/// directory and is `None` when the sanitized environment hides it.
fn desktop_icon(home: Option<&std::path::Path>, desktop: &std::path::Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(host_metadata_path(desktop)).ok()?;
    let name = content
        .lines()
        .find_map(|line| line.strip_prefix("Icon="))?
        .trim();
    themed_icon(home, name)
}

fn host_metadata_path(path: &std::path::Path) -> PathBuf {
    static HOST: std::sync::OnceLock<Host> = std::sync::OnceLock::new();
    HOST.get_or_init(Host::current).filesystem_path(path)
}

/// Resolve an exact desktop/AppStream icon name without network requests.
pub fn themed_icon(home: Option<&std::path::Path>, name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    let path = PathBuf::from(name);
    if path.is_absolute() {
        let path = host_metadata_path(&path);
        return path.is_file().then_some(path);
    }
    if name.contains('/') {
        return None;
    }
    let mut dirs = vec![
        host_metadata_path(std::path::Path::new("/usr/local/share/icons")),
        host_metadata_path(std::path::Path::new("/usr/share/icons")),
    ];
    if let Some(home) = home {
        dirs.insert(0, home.join(".local/share/icons"));
    }
    for dir in &dirs {
        for size in ["128x128", "64x64", "48x48", "32x32", "scalable"] {
            if let Some(icon) = icon_file(&dir.join("hicolor").join(size).join("apps"), name) {
                return Some(icon);
            }
        }
    }
    icon_file(
        &host_metadata_path(std::path::Path::new("/usr/share/pixmaps")),
        name,
    )
}

/// Strip one trailing `.desktop` suffix so DEP-11 component ids
/// (`org.mozilla.firefox.desktop`), Flatpak app ids (`org.mozilla.firefox`),
/// and snap desktop basenames (`firefox.desktop`) share one namespace.
/// Empty stems are `None` so they never join unrelated rows.
fn component_stem(id: &str) -> Option<String> {
    let stem = id.strip_suffix(".desktop").unwrap_or(id).trim();
    (!stem.is_empty()).then(|| stem.to_owned())
}

/// Map installed Debian packages to their AppStream component-id stems from
/// the DEP-11 YAML in `yaml_dir`. Decompressing it takes a few hundred
/// milliseconds on every list, so the map is kept in the user's cache.
fn dep11_component_ids(
    yaml_dir: &std::path::Path,
) -> std::collections::BTreeMap<String, Vec<String>> {
    dep11_cached(crate::cache::Store::user().as_ref(), yaml_dir, dep11_scan)
}

/// [`dep11_component_ids`] answered from `store` while the folder and every
/// file its entries link to (APT's lists, refreshed in place) are unchanged.
fn dep11_cached(
    store: Option<&crate::cache::Store>,
    yaml_dir: &std::path::Path,
    scan: impl FnOnce(&[PathBuf]) -> std::collections::BTreeMap<String, Vec<String>>,
) -> std::collections::BTreeMap<String, Vec<String>> {
    use crate::cache::Watch;
    let Ok(entries) = std::fs::read_dir(yaml_dir) else {
        return std::collections::BTreeMap::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "gz"))
        .collect();
    files.sort();
    let folder = std::fs::canonicalize(yaml_dir).unwrap_or_else(|_| yaml_dir.to_owned());
    let mut watches = vec![Watch::file(yaml_dir), Watch::tree(folder, 1)];
    // A dangling link is watched where it points, so its target appearing
    // counts as a change.
    watches.extend(files.iter().map(|file| {
        Watch::file(
            std::fs::canonicalize(file)
                .or_else(|_| std::fs::read_link(file).map(|target| yaml_dir.join(target)))
                .unwrap_or_else(|_| file.clone()),
        )
    }));
    let encoded = crate::cache::read_through(
        store,
        "dep11",
        "components",
        &[&yaml_dir.to_string_lossy()],
        &watches,
        &[],
        || serde_json::to_vec(&scan(&files)),
    )
    .unwrap_or_default();
    serde_json::from_slice(&encoded).unwrap_or_default()
}

/// Scan DEP-11 YAML (`<id>.yml.gz`) as a gzip line stream. Documents are
/// `---`-separated with top-level `ID:`/`Package:` scalars; a package can
/// own several components. Unreadable files map nothing.
fn dep11_scan(files: &[PathBuf]) -> std::collections::BTreeMap<String, Vec<String>> {
    use std::io::{BufRead, BufReader};
    let mut map: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for path in files {
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        let decoder = BufReader::new(flate2::read::GzDecoder::new(file));
        let mut id: Option<String> = None;
        let mut package: Option<String> = None;
        let mut flush = |id: &mut Option<String>, package: &mut Option<String>| {
            if let (Some(id), Some(package)) = (id.take(), package.take()) {
                if let Some(stem) = component_stem(&id) {
                    map.entry(package).or_default().push(stem);
                }
            }
        };
        for line in decoder.lines() {
            let Ok(line) = line else {
                break;
            };
            if line == "---" {
                flush(&mut id, &mut package);
            } else if let Some(value) = line.strip_prefix("ID: ") {
                id = Some(value.trim().to_owned());
            } else if let Some(value) = line.strip_prefix("Package: ") {
                package = Some(value.trim().to_owned());
            }
        }
        flush(&mut id, &mut package);
    }
    for ids in map.values_mut() {
        ids.sort();
        ids.dedup();
    }
    map
}

/// Desktop-id stems shipped by an installed snap (`meta/gui/*.desktop`).
/// CLI-only snaps expose no desktop entry and group with nothing.
fn snap_desktop_ids(sysroot: &std::path::Path, name: &str) -> Vec<String> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return vec![];
    }
    let gui = sysroot.join("snap").join(name).join("current/meta/gui");
    let Ok(entries) = std::fs::read_dir(&gui) else {
        return vec![];
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "desktop"))
        .filter_map(|entry| component_stem(&entry.file_name().to_string_lossy()))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}
fn parse_apt_upgrade_plan(output: &[u8]) -> Result<AptUpgradePlan, EngineError> {
    let preview = std::str::from_utf8(output)
        .map_err(|e| invalid("apt", e))?
        .trim();
    let summary = preview
        .lines()
        .find(|line| {
            line.contains(" upgraded, ")
                && line.contains(" newly installed, ")
                && line.contains(" to remove")
        })
        .ok_or_else(|| invalid("apt", "simulation did not include a transaction summary"))?;
    let counts: Vec<_> = summary
        .split(',')
        .take(3)
        .map(|part| {
            part.split_whitespace()
                .next()
                .and_then(|n| n.parse::<usize>().ok())
        })
        .collect();
    let [Some(upgrades), Some(installs), Some(removals)] = counts.as_slice() else {
        return Err(invalid("apt", "invalid simulation counts"));
    };
    let mut plan = AptUpgradePlan {
        preview: preview.into(),
        upgrades: Vec::new(),
        installs: Vec::new(),
        removals: Vec::new(),
    };
    for line in preview.lines() {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("Inst") => {
                let name = fields
                    .next()
                    .ok_or_else(|| invalid("apt", "incomplete install action"))?;
                let existing = fields.next().is_some_and(|field| field.starts_with('['));
                if existing {
                    plan.upgrades.push(name.into());
                } else {
                    plan.installs.push(name.into());
                }
            }
            Some("Remv" | "Purg") => {
                let name = fields
                    .next()
                    .ok_or_else(|| invalid("apt", "incomplete removal action"))?;
                plan.removals.push(name.into());
            }
            _ => {}
        }
    }
    if plan.upgrades.len() != *upgrades
        || plan.installs.len() != *installs
        || plan.removals.len() != *removals
    {
        return Err(invalid(
            "apt",
            "simulation actions did not match its summary",
        ));
    }
    Ok(plan)
}
fn parse_apt_transaction_plan(
    operation: &Operation,
    output: &[u8],
) -> Result<TransactionPlan, EngineError> {
    let parsed = parse_apt_upgrade_plan(output)?;
    let mut changes = Vec::new();
    for line in parsed.preview.lines() {
        let mut fields = line.split_whitespace();
        let kind = fields.next();
        let Some(name) = fields.next() else { continue };
        let action = match kind {
            Some("Inst") if parsed.upgrades.iter().any(|item| item == name) => {
                PlannedAction::Upgrade
            }
            Some("Inst") => PlannedAction::Install,
            Some("Remv" | "Purg") => PlannedAction::Remove,
            _ => continue,
        };
        let tokens: Vec<_> = fields.collect();
        let installed_version = tokens
            .iter()
            .find(|field| field.starts_with('['))
            .map(|field| field.trim_matches(['[', ']']).to_owned());
        let candidate_version = if matches!(action, PlannedAction::Remove) {
            None
        } else {
            tokens
                .iter()
                .find(|field| field.starts_with('('))
                .map(|field| {
                    field
                        .trim_start_matches('(')
                        .trim_end_matches(')')
                        .to_owned()
                })
        };
        changes.push(PlannedChange {
            action,
            name: name.into(),
            installed_version,
            candidate_version,
        });
    }
    Ok(TransactionPlan {
        operation: operation.clone(),
        native_preview: parsed.preview,
        changes,
        download_bytes: None,
        disk_bytes: None,
        restart_required: None,
        adopts: None,
    })
}

impl<T: Transport> Apt<T> {
    fn simulated_operation(
        &self,
        operation: &Operation,
        cancel: &Cancellation,
    ) -> Result<Option<TransactionPlan>, EngineError> {
        let (verb, id) = match operation {
            Operation::Install(id) | Operation::Upgrade(id) => ("install", id),
            Operation::Remove(id) => ("remove", id),
            _ => return Ok(None),
        };
        let artifact = if matches!(operation, Operation::Install(_)) {
            crate::artifact::stage(id, cancel)?
        } else {
            None
        };
        let target = if let Some(staged) = artifact.as_ref() {
            staged.path().display().to_string()
        } else if matches!(operation, Operation::Install(_)) {
            crate::local_deb::verified_path(id, cancel)?
                .map_or_else(|| self.target(id), |path| Ok(path.display().to_string()))?
        } else {
            self.target(id)?
        };
        let args = [
            OsString::from("--simulate"),
            OsString::from("-o"),
            OsString::from("Debug::NoLocking=1"),
            OsString::from(verb),
            OsString::from(target),
        ];
        let output = bytes(
            "apt",
            self.transport
                .system_manager("apt-get", &args, cancel, false)?,
        )?;
        parse_apt_transaction_plan(operation, &output).map(Some)
    }
    fn simulated_upgrade(&self, cancel: &Cancellation) -> Result<AptUpgradePlan, EngineError> {
        let args = ["--simulate", "-o", "Debug::NoLocking=1", "dist-upgrade"].map(OsString::from);
        let output = bytes(
            "apt",
            self.transport
                .system_manager("apt-get", &args, cancel, false)?,
        )?;
        parse_apt_upgrade_plan(&output)
    }
    fn query(
        &self,
        mode: &str,
        query: &str,
        arch: &str,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        serde_json::from_slice(&bytes(
            "apt",
            self.transport.apt_query(mode, query, arch, cancel)?,
        )?)
        .map_err(|e| invalid("apt", e))
    }
    fn target(&self, id: &PackageId) -> Result<String, EngineError> {
        if id.backend != "apt" || id.scope != Scope::System {
            return Err(invalid("apt", "foreign identity or scope"));
        }
        let target = format!("{}:{}", id.name, id.architecture);
        AptAction::Install(target.clone()).arguments()?;
        Ok(target)
    }
    /// Installed packages mapped to their desktop entries, scanned once per
    /// backend lifetime. Backend instances are rebuilt for every load, so the
    /// cache never outlives the query that populated it; repeated selections
    /// reuse it instead of re-reading every dpkg file list.
    fn desktop_entries(&mut self) -> &std::collections::BTreeMap<String, PathBuf> {
        self.desktop_entries
            .get_or_insert_with(|| apt_desktop_map(std::path::Path::new("/var/lib/dpkg/info")))
    }
    /// Installed packages mapped to their AppStream component-id stems,
    /// scanned once per backend lifetime like the desktop entries above.
    fn component_map(&mut self) -> &std::collections::BTreeMap<String, Vec<String>> {
        self.components.get_or_insert_with(|| {
            dep11_component_ids(std::path::Path::new("/var/lib/app-info/yaml"))
        })
    }
}
impl<T: Transport> Backend for Apt<T> {
    fn id(&self) -> &str {
        "apt"
    }
    fn capabilities(&self) -> &[Capability] {
        CLEAN_CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        availability(self.transport.apt_query("detect", "", "", cancel))
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .query("search", query, "", cancel)?
            .into_iter()
            .map(|mut d| {
                let home = self.transport.env("HOME").map(PathBuf::from);
                if let Some(desktop) = self.desktop_entries().get(&d.package.id.name).cloned() {
                    d.package.icon = desktop_icon(home.as_deref(), &desktop);
                }
                d.package
            })
            .collect())
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let home = self.transport.env("HOME").map(PathBuf::from);
        let packages = self.query("installed", "", "", cancel)?;
        Ok(packages
            .into_iter()
            .map(|mut d| {
                if let Some(desktop) = self.desktop_entries().get(&d.package.id.name).cloned() {
                    d.package.icon = desktop_icon(home.as_deref(), &desktop);
                }
                d.package.component_ids = self
                    .component_map()
                    .get(&d.package.id.name)
                    .cloned()
                    .unwrap_or_default();
                // The native helper already reports the control-file
                // homepage for every package: reuse it as a grouping key
                // so CLI tools (no AppStream entry) group across managers.
                d.package.homepages = d.homepage.clone().into_iter().collect();
                d.package
            })
            .collect())
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        self.target(id)?;
        let mut details = self
            .query("details", &id.name, &id.architecture, cancel)?
            .into_iter()
            .find(|d| d.package.id == *id)
            .ok_or(EngineError::NotFound)?;
        if details.package.installed_version.is_some() {
            let home = self.transport.env("HOME").map(PathBuf::from);
            if let Some(desktop) = self.desktop_entries().get(&id.name).cloned() {
                details.package.icon = desktop_icon(home.as_deref(), &desktop);
            }
            details.package.component_ids = self
                .component_map()
                .get(&id.name)
                .cloned()
                .unwrap_or_default();
            details.package.homepages = details.homepage.clone().into_iter().collect();
        }
        Ok(details)
    }
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        let report = self.cleanup_report(cancel);
        if let Some(failure) = report.failures.into_iter().next() {
            Err(failure.error)
        } else {
            Ok(report.items)
        }
    }
    fn cleanup_report(&mut self, cancel: &Cancellation) -> CleanupReport {
        self.cleanup_apt_report(cancel)
    }
    fn cleanup_plan(
        &mut self,
        id: &CleanupId,
        cancel: &Cancellation,
    ) -> Result<CleanupItem, EngineError> {
        if id.backend != "apt" {
            return Err(invalid("apt", "foreign cleanup task"));
        }
        self.cleanup_apt_task(&id.key, cancel)?
            .ok_or(EngineError::NotFound)
    }
    fn apt_upgrade_plan(&mut self, cancel: &Cancellation) -> Result<AptUpgradePlan, EngineError> {
        self.simulated_upgrade(cancel)
    }
    fn operation_plan(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
    ) -> Result<Option<TransactionPlan>, EngineError> {
        self.simulated_operation(operation, cancel)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let staged_artifact = match operation {
            Operation::Install(id) => crate::artifact::stage(id, cancel)?,
            _ => None,
        };
        let staged = match operation {
            Operation::Install(id) => crate::local_deb::stage(id, cancel)?,
            _ => None,
        };
        let action = match operation {
            Operation::Refresh { backend } if backend == "apt" => AptAction::Refresh,
            Operation::Install(id) => {
                if let Some(archive) = staged_artifact.as_ref() {
                    AptAction::InstallLocal(archive.path().to_owned())
                } else if let Some(archive) = staged.as_ref() {
                    AptAction::InstallLocal(archive.path().to_owned())
                } else {
                    AptAction::Install(self.target(id)?)
                }
            }
            Operation::Remove(id) => AptAction::Remove(self.target(id)?),
            Operation::Upgrade(id) => AptAction::Upgrade(self.target(id)?),
            Operation::UpgradeAll { backend } if backend == "apt" => AptAction::UpgradeAll,
            Operation::Clean(id) if id.backend == "apt" && id.key == "autoremove" => {
                AptAction::Autoremove
            }
            Operation::Clean(id) if id.backend == "apt" && id.key == "autoclean" => {
                AptAction::Autoclean
            }
            _ => return Err(invalid("apt", "foreign operation")),
        };
        progress(Progress::Message(
            "Running apt-get. If you cancel, PkgDeck waits for APT to finish.".into(),
        ));
        let result = self.transport.apt_write(action, cancel)?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
    fn execute_group(
        &mut self,
        operations: &[Operation],
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Option<Result<Vec<OperationOutcome>, EngineError>> {
        if operations.len() < 2 {
            return None;
        }
        let grouped = (|| {
            let actions = operations
                .iter()
                .map(|operation| match operation {
                    Operation::Install(id) => Ok(AptAction::Install(self.target(id)?)),
                    Operation::Remove(id) => Ok(AptAction::Remove(self.target(id)?)),
                    Operation::Upgrade(id) => Ok(AptAction::Upgrade(self.target(id)?)),
                    _ => Err(invalid("apt", "mixed APT batch")),
                })
                .collect::<Result<Vec<_>, EngineError>>()?;
            AptAction::group_arguments(&actions)?;
            progress(Progress::Message(format!(
                "Updating {} APT packages in one transaction.",
                actions.len()
            )));
            let result = self.transport.apt_write_group(&actions, cancel)?;
            Ok(vec![
                OperationOutcome {
                    cancellation_deferred: result.cancellation_deferred
                };
                actions.len()
            ])
        })();
        Some(grouped)
    }
}

#[derive(Deserialize)]
struct FormulaReport {
    formulae: Vec<Formula>,
}
#[derive(Deserialize)]
struct Formula {
    full_name: String,
    desc: Option<String>,
    homepage: String,
    versions: Versions,
    revision: u64,
    installed: Vec<Installed>,
    linked_keg: Option<String>,
    outdated: bool,
    dependencies: Vec<String>,
}
#[derive(Deserialize)]
struct Versions {
    stable: Option<String>,
}
#[derive(Deserialize)]
struct Installed {
    version: String,
}
fn formula_name(name: &str) -> bool {
    let segments: Vec<_> = name.split('/').collect();
    (segments.len() == 1 || segments.len() == 3)
        && segments.iter().all(|s| {
            !s.is_empty()
                && s.starts_with(|c: char| c.is_ascii_alphanumeric())
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"@+_.-".contains(&b))
                && *s != "."
                && *s != ".."
        })
}
/// Cask tokens share the formula policy: bare tokens and tap-qualified full
/// tokens, including versioned `@` tokens (for example `firefox@esr`).
fn cask_token(name: &str) -> bool {
    formula_name(name)
}
/// Names per `brew info` call. Each call starts Homebrew again, which is
/// most of its time, so a few big calls beat many small ones. Output stays
/// well under the read limit (about 3 MB per 1,000 casks).
const BREW_INFO_BATCH: usize = 1000;
/// Packages a search already read from `brew info`, by name. `None` marks a
/// name with no row here, such as a cask this system can't install. Typing
/// one more letter only narrows a search, so most of it is answered from
/// here. Changes and update checks clear it.
#[derive(Default)]
struct BrewFound(std::collections::HashMap<String, Option<Package>>);
impl BrewFound {
    /// `names` in order, reading the ones not seen yet with `read`.
    fn search(
        &mut self,
        names: &[&str],
        mut read: impl FnMut(&[&str]) -> Result<Vec<Package>, EngineError>,
    ) -> Result<Vec<Package>, EngineError> {
        let missing: Vec<_> = names
            .iter()
            .copied()
            .filter(|name| !self.0.contains_key(*name))
            .collect();
        for chunk in missing.chunks(BREW_INFO_BATCH) {
            let mut read = read(chunk)?
                .into_iter()
                .map(|package| (package.id.name.clone(), package))
                .collect::<std::collections::HashMap<_, _>>();
            for name in chunk {
                self.0.insert((*name).to_owned(), read.remove(*name));
            }
        }
        Ok(names
            .iter()
            .filter_map(|name| self.0.get(*name).cloned().flatten())
            .collect())
    }
    fn clear(&mut self) {
        self.0.clear();
    }
}
#[derive(Deserialize)]
struct CaskReport {
    casks: Vec<Cask>,
}
#[derive(Deserialize)]
struct Cask {
    full_token: String,
    name: Vec<String>,
    desc: Option<String>,
    homepage: String,
    version: String,
    installed: Option<String>,
    outdated: bool,
    #[serde(default)]
    depends_on: serde_json::Value,
    #[serde(default)]
    artifacts: Vec<serde_json::Value>,
}
/// Artifacts only macOS can install. On Linux, `brew info` describes each cask
/// for the running system: a Mac-only cask declares `depends_on: macos` or keeps
/// one of these artifacts, while a Linux cask ships AppImages, binaries or fonts.
const MACOS_ONLY_ARTIFACTS: &[&str] = &[
    "app",
    "pkg",
    "suite",
    "installer",
    "prefpane",
    "qlplugin",
    "mdimporter",
    "dictionary",
    "colorpicker",
    "input_method",
    "internet_plugin",
    "keyboard_layout",
    "screen_saver",
    "service",
    "audio_unit_plugin",
    "vst_plugin",
    "vst3_plugin",
];
fn cask_installs_here(cask: &Cask) -> bool {
    cfg!(target_os = "macos")
        || (cask.depends_on.get("macos").is_none()
            && !cask.artifacts.iter().any(|artifact| {
                artifact.as_object().is_some_and(|kinds| {
                    kinds
                        .keys()
                        .any(|kind| MACOS_ONLY_ARTIFACTS.contains(&kind.as_str()))
                })
            }))
}
/// Homebrew 6.0 added AppImage artifacts and Linux variations to casks; earlier
/// releases only had preliminary Linux cask support.
fn linux_casks_supported(version: &str) -> bool {
    version
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("Homebrew "))
        .and_then(|version| version.split(['.', '-']).next())
        .and_then(|major| major.parse::<u32>().ok())
        .is_some_and(|major| major >= 6)
}
impl<T: Transport> Homebrew<T> {
    fn call(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, EngineError> {
        Ok(self.transport.brew(
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            cancel,
            write,
        )?)
    }
    fn parse(&self, result: Completion) -> Result<Vec<PackageDetails>, EngineError> {
        let report: FormulaReport = serde_json::from_slice(&bytes("homebrew", result)?)
            .map_err(|e| invalid("homebrew", e))?;
        let prefix = self
            .prefix
            .as_ref()
            .ok_or_else(|| invalid("homebrew", "prefix not detected"))?;
        report
            .formulae
            .into_iter()
            .map(|f| {
                if !formula_name(&f.full_name) {
                    return Err(invalid("homebrew", "invalid formula name"));
                }
                let candidate = f.versions.stable.map(|v| {
                    if f.revision == 0 {
                        v
                    } else {
                        format!("{v}_{}", f.revision)
                    }
                });
                let installed = f.linked_keg.or_else(|| {
                    f.installed
                        .iter()
                        .find(|i| Some(&i.version) == candidate.as_ref())
                        .or_else(|| f.installed.last())
                        .map(|i| i.version.clone())
                });
                Ok(PackageDetails {
                    package: Package {
                        id: PackageId {
                            backend: "homebrew".into(),
                            name: f.full_name.clone(),
                            architecture: std::env::consts::ARCH.into(),
                            scope: Scope::Environment {
                                path: prefix.clone(),
                            },
                            remote: None,
                            reference: None,
                        },
                        display_name: f.full_name,
                        summary: f.desc.clone().unwrap_or_default(),
                        update: if f.outdated {
                            UpdateAvailability::Available
                        } else if installed.is_some() {
                            UpdateAvailability::Current
                        } else {
                            UpdateAvailability::Unknown
                        },
                        installed_version: installed,
                        candidate_version: candidate,
                        icon: None,
                        component_ids: vec![],
                        homepages: if f.homepage.is_empty() {
                            vec![]
                        } else {
                            vec![f.homepage.clone()]
                        },
                        adopt_with: None,
                    },
                    description: f.desc.unwrap_or_default(),
                    homepage: (!f.homepage.is_empty()).then_some(f.homepage),
                    dependencies: f.dependencies,
                })
            })
            .collect()
    }
    fn target<'a>(&self, id: &'a PackageId) -> Result<&'a str, EngineError> {
        if id.backend != "homebrew"
            || id.architecture != std::env::consts::ARCH
            || !formula_name(&id.name)
            || self
                .prefix
                .as_ref()
                .is_none_or(|p| id.scope != (Scope::Environment { path: p.clone() }))
        {
            return Err(invalid(
                "homebrew",
                "foreign identity or invalid formula name",
            ));
        }
        Ok(&id.name)
    }
}
impl<T: Transport> Backend for Homebrew<T> {
    fn id(&self) -> &str {
        "homebrew"
    }
    fn has_update_index(&self) -> bool {
        true
    }
    fn arm_update_check(&mut self, token: Option<u64>) {
        self.update_check = token;
    }
    fn refresh_update_index(&mut self, cancel: &Cancellation) -> Result<(), EngineError> {
        let Some(token) = self.update_check else {
            return Ok(());
        };
        self.found.clear();
        // Formulae and casks both enter here. The first `brew update` wins;
        // the other waits and reuses its result. A different check waits its
        // turn, then fetches again.
        crate::engine::once_per_check(token, || {
            let _turn = brew_update_turn();
            self.call(&["update"], cancel, true)?;
            Ok(())
        })
    }
    fn capabilities(&self) -> &[Capability] {
        CLEAN_CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        let result = self.transport.brew(&["--prefix".into()], cancel, false);
        if let Ok(result) = &result {
            let value = String::from_utf8(bytes("homebrew", result.clone())?)
                .map_err(|e| invalid("homebrew", e))?;
            let path = PathBuf::from(value.trim());
            if !path.is_absolute() {
                return Err(invalid("homebrew", "prefix must be absolute"));
            }
            self.prefix = Some(path);
        }
        availability(result)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let data = bytes("homebrew", self.call(&["formulae"], cancel, false)?)?;
        let names = String::from_utf8(data).map_err(|e| invalid("homebrew", e))?;
        let matches: Vec<_> = names.lines().filter(|n| search_matches(n, query)).collect();
        if matches.iter().any(|n| !formula_name(n)) {
            return Err(invalid("homebrew", "invalid formula name"));
        }
        let mut found = std::mem::take(&mut self.found);
        let packages = found.search(&matches, |chunk| {
            let mut args = vec!["info", "--json=v2", "--formula", "--"];
            args.extend(chunk);
            Ok(self
                .parse(self.call(&args, cancel, false)?)?
                .into_iter()
                .map(|d| d.package)
                .collect())
        });
        self.found = found;
        packages
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .parse(self.call(
                &["info", "--json=v2", "--formula", "--installed"],
                cancel,
                false,
            )?)?
            .into_iter()
            .map(|d| d.package)
            .collect())
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let name = self.target(id)?;
        self.parse(self.call(
            &["info", "--json=v2", "--formula", "--", name],
            cancel,
            false,
        )?)?
        .into_iter()
        .find(|d| d.package.id == *id)
        .ok_or(EngineError::NotFound)
    }
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        fn plan(result: Completion) -> Result<Option<String>, EngineError> {
            let output = String::from_utf8(bytes("homebrew", result)?)
                .map_err(|error| invalid("homebrew", error))?;
            let output = output.trim();
            Ok((!output.is_empty()).then(|| output.to_owned()))
        }
        let mut items = Vec::new();
        if let Some(preview) = plan(self.call(&["autoremove", "--dry-run"], cancel, false)?)? {
            items.push(CleanupItem {
                id: CleanupId {
                    backend: "homebrew".into(),
                    key: "autoremove".into(),
                },
                kind: CleanupKind::OrphanDependencies,
                title: "Unused Homebrew dependencies".into(),
                summary: "Formulae that no installed package needs anymore".into(),
                preview,
            });
        }
        if let Some(preview) = plan(self.call(&["cleanup", "--dry-run"], cancel, false)?)? {
            items.push(CleanupItem {
                id: CleanupId {
                    backend: "homebrew".into(),
                    key: "cleanup".into(),
                },
                kind: CleanupKind::PackageCache,
                title: "Old Homebrew downloads and versions".into(),
                summary: "Old files that brew cleanup would remove".into(),
                preview,
            });
        }
        Ok(items)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        // A change can make what searches read out of date.
        self.found.clear();
        let args = match operation {
            Operation::Refresh { backend } if backend == "homebrew" => vec!["update"],
            Operation::Install(id) => vec!["install", "--formula", "--", self.target(id)?],
            Operation::Remove(id) => {
                vec!["uninstall", "--formula", "--force", "--", self.target(id)?]
            }
            Operation::Upgrade(id) => vec!["upgrade", "--formula", "--", self.target(id)?],
            Operation::UpgradeAll { backend } if backend == "homebrew" => {
                vec!["upgrade", "--formula"]
            }
            Operation::Clean(id) if id.backend == "homebrew" && id.key == "autoremove" => {
                vec!["autoremove"]
            }
            Operation::Clean(id) if id.backend == "homebrew" && id.key == "cleanup" => {
                vec!["cleanup"]
            }
            _ => return Err(invalid("homebrew", "foreign operation")),
        };
        progress(Progress::Message(
            "Running brew. If you cancel, PkgDeck waits for it to finish.".into(),
        ));
        let result = if args == ["update"] {
            let _turn = brew_update_turn();
            self.call(&args, cancel, true)?
        } else {
            self.call(&args, cancel, true)?
        };
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}

impl<T: Transport> HomebrewCask<T> {
    fn call(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, EngineError> {
        Ok(self.transport.brew(
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            cancel,
            write,
        )?)
    }
    /// `installable_only` drops casks this system cannot install; searches use
    /// it on Linux, while installed casks always stay listed.
    fn parse(
        &self,
        result: Completion,
        installable_only: bool,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let report: CaskReport = serde_json::from_slice(&bytes("homebrew-cask", result)?)
            .map_err(|e| invalid("homebrew-cask", e))?;
        let prefix = self
            .prefix
            .as_ref()
            .ok_or_else(|| invalid("homebrew-cask", "prefix not detected"))?;
        report
            .casks
            .into_iter()
            .filter(|c| !installable_only || cask_installs_here(c))
            .map(|c| {
                if !cask_token(&c.full_token) {
                    return Err(invalid("homebrew-cask", "invalid cask token"));
                }
                let installed = c.installed.filter(|v| !v.is_empty());
                let display = c
                    .name
                    .iter()
                    .find(|n| !n.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| c.full_token.clone());
                Ok(PackageDetails {
                    package: Package {
                        id: PackageId {
                            backend: "homebrew-cask".into(),
                            name: c.full_token.clone(),
                            architecture: std::env::consts::ARCH.into(),
                            scope: Scope::Environment {
                                path: prefix.clone(),
                            },
                            remote: None,
                            reference: None,
                        },
                        display_name: display,
                        summary: c.desc.clone().unwrap_or_default(),
                        update: if c.outdated {
                            UpdateAvailability::Available
                        } else if installed.is_some() {
                            UpdateAvailability::Current
                        } else {
                            UpdateAvailability::Unknown
                        },
                        installed_version: installed,
                        candidate_version: Some(c.version),
                        icon: None,
                        component_ids: vec![],
                        homepages: if c.homepage.is_empty() {
                            vec![]
                        } else {
                            vec![c.homepage.clone()]
                        },
                        adopt_with: None,
                    },
                    description: c.desc.unwrap_or_default(),
                    homepage: (!c.homepage.is_empty()).then_some(c.homepage),
                    dependencies: vec![],
                })
            })
            .collect()
    }
    fn target<'a>(&self, id: &'a PackageId) -> Result<&'a str, EngineError> {
        if id.backend != "homebrew-cask"
            || id.architecture != std::env::consts::ARCH
            || !cask_token(&id.name)
            || self
                .prefix
                .as_ref()
                .is_none_or(|p| id.scope != (Scope::Environment { path: p.clone() }))
        {
            return Err(invalid(
                "homebrew-cask",
                "foreign identity or invalid cask token",
            ));
        }
        Ok(&id.name)
    }
}
impl<T: Transport> HomebrewCask<T> {
    /// A checked adoption when an app is already in the cask's place.
    fn adoption_plan(
        &self,
        token: &str,
        cancel: &Cancellation,
    ) -> Result<Option<adopt::Plan>, EngineError> {
        let Some(io) = &self.adoption else {
            return Ok(None);
        };
        let info: serde_json::Value = serde_json::from_slice(&bytes(
            "homebrew-cask",
            self.call(&["info", "--json=v2", "--cask", "--", token], cancel, false)?,
        )?)
        .map_err(|e| invalid("homebrew-cask", e))?;
        adopt::plan(io.as_ref(), &info["casks"][0], cancel)
    }
}
impl<T: Transport> Backend for HomebrewCask<T> {
    /// Installing over an app already in place adopts it; the preview says
    /// so, and names the version and publisher that were checked.
    fn operation_plan(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
    ) -> Result<Option<TransactionPlan>, EngineError> {
        let Operation::Install(id) = operation else {
            return Ok(None);
        };
        let token = self.target(id)?;
        Ok(self
            .adoption_plan(token, cancel)?
            .map(|plan| TransactionPlan {
                operation: operation.clone(),
                native_preview: plan.preview(),
                changes: vec![PlannedChange {
                    action: PlannedAction::Install,
                    name: token.into(),
                    installed_version: Some(plan.version.clone()),
                    candidate_version: Some(plan.version.clone()),
                }],
                download_bytes: None,
                disk_bytes: None,
                restart_required: None,
                adopts: Some(plan.app.clone()),
            }))
    }
    fn id(&self) -> &str {
        "homebrew-cask"
    }
    fn has_update_index(&self) -> bool {
        true
    }
    fn arm_update_check(&mut self, token: Option<u64>) {
        self.update_check = token;
    }
    fn refresh_update_index(&mut self, cancel: &Cancellation) -> Result<(), EngineError> {
        let Some(token) = self.update_check else {
            return Ok(());
        };
        self.found.clear();
        crate::engine::once_per_check(token, || {
            let _turn = brew_update_turn();
            self.call(&["update"], cancel, true)?;
            Ok(())
        })
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        let result = self.transport.brew(&["--prefix".into()], cancel, false);
        if let Ok(result) = &result {
            let value = String::from_utf8(bytes("homebrew-cask", result.clone())?)
                .map_err(|e| invalid("homebrew-cask", e))?;
            let path = PathBuf::from(value.trim());
            if !path.is_absolute() {
                return Err(invalid("homebrew-cask", "prefix must be absolute"));
            }
            self.prefix = Some(path);
            if !cfg!(target_os = "macos") {
                let version = bytes("homebrew-cask", self.call(&["--version"], cancel, false)?)?;
                if !linux_casks_supported(&String::from_utf8_lossy(&version)) {
                    return Ok(Availability::Unavailable(
                        "Homebrew Casks on Linux need Homebrew 6.0 or later".into(),
                    ));
                }
            }
        }
        availability(result)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let data = bytes("homebrew-cask", self.call(&["casks"], cancel, false)?)?;
        let names = String::from_utf8(data).map_err(|e| invalid("homebrew-cask", e))?;
        let matches: Vec<_> = names
            .lines()
            .filter(|n| !n.trim().is_empty() && search_matches(n, query))
            .collect();
        if matches.iter().any(|n| !cask_token(n)) {
            return Err(invalid("homebrew-cask", "invalid cask token"));
        }
        let mut found = std::mem::take(&mut self.found);
        let packages = found.search(&matches, |chunk| {
            let mut args = vec!["info", "--json=v2", "--cask", "--"];
            args.extend(chunk);
            Ok(self
                .parse(self.call(&args, cancel, false)?, true)?
                .into_iter()
                .map(|d| d.package)
                .collect())
        });
        self.found = found;
        packages
    }
    /// An exact name is looked up directly, which also finds casks in a tap
    /// that `brew casks` hasn't listed yet, and skips reading every cask.
    fn lookup(&mut self, name: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        if !cask_token(name) {
            return Ok(vec![]);
        }
        match self.call(&["info", "--json=v2", "--cask", "--", name], cancel, false) {
            Ok(result) => Ok(self
                .parse(result, true)?
                .into_iter()
                .map(|details| details.package)
                .collect()),
            // brew exits 1 for a name that is no cask.
            Err(EngineError::Execution(ExecutionError::Failed(result)))
                if result.code == Some(1) =>
            {
                Ok(vec![])
            }
            Err(error) => Err(error),
        }
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .parse(
                self.call(
                    &["info", "--json=v2", "--cask", "--installed"],
                    cancel,
                    false,
                )?,
                false,
            )?
            .into_iter()
            .map(|d| d.package)
            .collect())
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let name = self.target(id)?;
        self.parse(
            self.call(&["info", "--json=v2", "--cask", "--", name], cancel, false)?,
            false,
        )?
        .into_iter()
        .find(|d| d.package.id == *id)
        .ok_or(EngineError::NotFound)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        // A change can make what searches read out of date.
        self.found.clear();
        let args = match operation {
            // brew update refreshes formulae and casks together; both Homebrew
            // backends run it so each Refresh honestly refreshes its metadata.
            Operation::Refresh { backend } if backend == "homebrew-cask" => vec!["update"],
            Operation::Install(id) => {
                let token = self.target(id)?;
                if let (Some(io), Some(plan)) = (&self.adoption, self.adoption_plan(token, cancel)?)
                {
                    return adopt::run(
                        io.as_ref(),
                        &plan,
                        cancel,
                        progress,
                        &mut || {
                            self.call(&["install", "--cask", "--adopt", "--", token], cancel, true)
                        },
                        // brew list rejects a tap cask's full name; brew info
                        // takes both and reports the installed version.
                        &mut || {
                            self.call(
                                &["info", "--json=v2", "--cask", "--", token],
                                &Cancellation::default(),
                                false,
                            )
                            .ok()
                            .and_then(|done| {
                                serde_json::from_slice::<serde_json::Value>(&done.stdout).ok()
                            })
                            .is_some_and(|info| {
                                info["casks"][0]["installed"]
                                    .as_str()
                                    .is_some_and(|version| !version.is_empty())
                            })
                        },
                    );
                }
                vec!["install", "--cask", "--", token]
            }
            Operation::Remove(id) => {
                vec!["uninstall", "--cask", "--force", "--", self.target(id)?]
            }
            Operation::Upgrade(id) => vec!["upgrade", "--cask", "--", self.target(id)?],
            Operation::UpgradeAll { backend } if backend == "homebrew-cask" => {
                vec!["upgrade", "--cask"]
            }
            _ => return Err(invalid("homebrew-cask", "foreign operation")),
        };
        progress(Progress::Message(
            "Running brew. If you cancel, PkgDeck waits for it to finish.".into(),
        ));
        let result = if args == ["update"] {
            let _turn = brew_update_turn();
            self.call(&args, cancel, true)?
        } else {
            self.call(&args, cancel, true)?
        };
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}

#[derive(Clone, Copy)]
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
    /// Where the manager lists pending updates, if it can without writing.
    fn update_args(self) -> Option<(&'static str, &'static [&'static str])> {
        match self {
            Self::Apk => Some(("apk", &["list", "--upgradable"])),
            // -M reads the repository index into memory; -n only reports.
            Self::Xbps => Some(("xbps-install", &["-Mun"])),
            Self::MacPorts => Some(("port", &["-q", "outdated"])),
            _ => None,
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
            (Self::Dnf, false) => vec![
                "--quiet",
                "repoquery",
                "--latest-limit",
                "1",
                "--queryformat",
                "%{name}|%{arch}|%{version}-%{release}|%{summary}\n",
                query,
            ],
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
            (Self::Dnf, Operation::Upgrade(_)) => vec!["-y", "upgrade", "--"],
            (Self::Dnf, Operation::UpgradeAll { .. }) => vec!["-y", "upgrade"],
            (Self::Pacman, Operation::Refresh { .. }) => vec!["-Sy", "--noconfirm"],
            (Self::Pacman, Operation::Install(_)) => {
                vec!["-S", "--noconfirm", "--needed", "--"]
            }
            (Self::Pacman, Operation::Remove(_)) => vec!["-Rns", "--noconfirm", "--"],
            (Self::Pacman, Operation::Upgrade(_)) => {
                vec!["-S", "--noconfirm", "--needed", "--"]
            }
            (Self::Pacman, Operation::UpgradeAll { .. }) => vec!["-Su", "--noconfirm"],
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
            (Self::Snap, Operation::Refresh { .. }) => vec!["refresh"],
            (Self::Snap, Operation::Install(_)) => vec!["install"],
            (Self::Snap, Operation::Remove(_)) => vec!["remove"],
            (Self::Snap, Operation::Upgrade(_)) => vec!["refresh"],
            (Self::Snap, Operation::UpgradeAll { .. }) => vec!["refresh"],
            (Self::Apk, Operation::Refresh { .. }) => vec!["update"],
            (Self::Apk, Operation::Install(_)) => vec!["add", "--"],
            (Self::Apk, Operation::Remove(_)) => vec!["del", "--"],
            (Self::Apk, Operation::Upgrade(_)) => vec!["upgrade", "--"],
            (Self::Apk, Operation::UpgradeAll { .. }) => vec!["upgrade"],
            (Self::Xbps, Operation::Refresh { .. }) => vec!["-S"],
            (Self::Xbps, Operation::Install(_) | Operation::Remove(_)) => vec!["-y"],
            (Self::Xbps, Operation::Upgrade(_) | Operation::UpgradeAll { .. }) => vec!["-yu"],
            // -N never asks; MacPorts keeps each port's variants on upgrade.
            (Self::MacPorts, Operation::Refresh { .. }) => vec!["-N", "selfupdate"],
            (Self::MacPorts, Operation::Install(_)) => vec!["-N", "install"],
            (Self::MacPorts, Operation::Remove(_)) => vec!["-N", "uninstall"],
            (Self::MacPorts, Operation::Upgrade(_)) => vec!["-N", "upgrade"],
            (Self::MacPorts, Operation::UpgradeAll { .. }) => vec!["-N", "upgrade", "outdated"],
            (_, Operation::Clean(_)) => return None,
        })
    }
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

pub struct SystemManager<T = NativeTransport> {
    kind: ManagerKind,
    transport: T,
}
pub type Dnf<T = NativeTransport> = SystemManager<T>;
pub type Pacman<T = NativeTransport> = SystemManager<T>;
pub type Zypper<T = NativeTransport> = SystemManager<T>;
pub type Snap<T = NativeTransport> = SystemManager<T>;
impl<T: Transport> SystemManager<T> {
    fn new(kind: ManagerKind, transport: T) -> Self {
        Self { kind, transport }
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
    fn updates(&self, cancel: &Cancellation) -> Result<BTreeMap<String, String>, EngineError> {
        let Some((executable, args)) = self.kind.update_args() else {
            return Ok(BTreeMap::new());
        };
        let result = self.call_with(
            executable,
            args.iter().map(OsString::from).collect(),
            cancel,
            false,
        );
        let text = match result {
            Ok(done) => String::from_utf8_lossy(&bytes(self.kind.id(), done)?).into_owned(),
            Err(EngineError::Cancelled | EngineError::Execution(ExecutionError::Cancelled)) => {
                return Err(EngineError::Cancelled)
            }
            Err(_) => return Ok(BTreeMap::new()),
        };
        Ok(text
            .lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                match self.kind {
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
        if !installed && !self.valid_name(query) {
            return Err(invalid(self.kind.id(), "invalid package query"));
        }
        let args = self.kind.read_args(installed, query);
        let mut packages = self.parse(
            bytes(self.kind.id(), self.call(args, cancel, false)?)?,
            installed,
        )?;
        if installed && matches!(self.kind, ManagerKind::Pacman) {
            // Packages no repository has (AUR builds) are the AUR source's
            // rows. Without synced databases every package looks foreign;
            // then Pacman keeps them all.
            let foreign = match self.call(vec!["-Qmq".into()], cancel, false) {
                // pacman -Q exits 1 when nothing matches: no foreign packages.
                Err(EngineError::Execution(ExecutionError::Failed(result)))
                    if result.code == Some(1) && result.stdout.is_empty() =>
                {
                    String::new()
                }
                result => String::from_utf8(bytes("pacman", result?)?)
                    .map_err(|e| invalid("pacman", e))?,
            };
            let foreign: std::collections::BTreeSet<&str> =
                foreign.lines().map(str::trim).collect();
            if foreign.len() < packages.len() {
                packages.retain(|package| !foreign.contains(package.id.name.as_str()));
            }
        }
        Ok(packages)
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
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn may_have(&self, name: &str) -> bool {
        self.valid_name(name)
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
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let mut packages = self.query(true, "", cancel)?;
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
        // Search merges the installed version into catalog rows.
        let mut package = self
            .search(&name, cancel)?
            .into_iter()
            .find(|package| package.id == *id)
            .ok_or(EngineError::NotFound)?;
        self.snap_icon(&mut package);
        self.snap_components(&mut package);
        Ok(PackageDetails {
            description: package.summary.clone(),
            homepage: None,
            dependencies: vec![],
            package,
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
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
                "Installing local {} package.",
                self.kind.id()
            )));
            let result = self.call(args, cancel, true)?;
            bytes(self.kind.id(), result.clone())?;
            return Ok(OperationOutcome {
                cancellation_deferred: result.cancellation_deferred,
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
}

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
enum DevKind {
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
    fn id(self) -> &'static str {
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
fn dev_name(name: &str) -> bool {
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

/// PEP 508 bare project names for `pip`/`pipx`/`uv`: no options, paths,
/// URLs, extras, or version specifiers.
fn python_name(name: &str) -> bool {
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
fn composer_name(name: &str) -> bool {
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

/// RubyGems bare gem names.
fn gem_name(name: &str) -> bool {
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
    kind: DevKind,
    transport: T,
    home: Option<PathBuf>,
}
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
    fn call(
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
    fn pip_call(
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
    fn cargo_inventory(
        &self,
        home: &std::path::Path,
        updates: Updates,
        cancel: &Cancellation,
    ) -> Result<Vec<PackageDetails>, EngineError> {
        let id = self.kind.id();
        let output = bytes(id, self.call(&["install", "--list"], cancel, false)?)?;
        let text = String::from_utf8(output).map_err(|e| invalid(id, e))?;
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
            if !updates.covers(&package.id.name) {
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
    fn uv_entry(line: &str) -> Option<(String, String, Option<String>)> {
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
    fn gem_filename(file: &str) -> Option<(String, String)> {
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
    fn gem_version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
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
    fn gem_outdated(line: &str) -> Option<(String, String)> {
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
    ) -> Result<Vec<(String, String, String)>, EngineError> {
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
            DevKind::Npm | DevKind::Pnpm | DevKind::Bun => {
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
                    .into_iter()
                    .flatten()
                    .filter_map(|hit| {
                        Some((hit.name, hit.version?, hit.description.unwrap_or_default()))
                    })
                    .collect()
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
                .into_iter()
                .flat_map(|output| {
                    output
                        .lines()
                        .filter_map(cargo_search_hit)
                        .map(|(name, version, summary)| {
                            (name.into(), version.into(), summary.into())
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
            }
            // `gem search` takes a regular expression and ignores `--`, so only
            // plain gem names are sent, with their dots escaped.
            DevKind::Gem if gem_name(query) && query.len() >= 3 => {
                let pattern = query.replace('.', "\\.");
                let mut hits: Vec<(String, String, String)> =
                    text(self.call(&["search", "--remote", &pattern], cancel, false))?
                        .into_iter()
                        .flat_map(|output| {
                            output
                                .lines()
                                .filter_map(gem_search_hit)
                                .map(|(name, version)| (name.into(), version.into(), String::new()))
                                .collect::<Vec<_>>()
                        })
                        .collect();
                // RubyGems lists matches alphabetically, so rank exact and
                // prefix matches first before keeping only the first few.
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
                hits
            }
            // Packagist reports no version; `global require` takes the latest.
            // `global` searches the same COMPOSER_HOME repositories it installs from.
            DevKind::Composer => text(self.call(
                &["global", "search", "--format=json", "--", query],
                cancel,
                false,
            ))?
            .and_then(|json| serde_json::from_str::<Vec<ComposerSearchHit>>(&json).ok())
            .into_iter()
            .flatten()
            .take(REGISTRY_RESULTS)
            .map(|hit| {
                (
                    hit.name,
                    "latest".into(),
                    hit.description.unwrap_or_default(),
                )
            })
            .collect(),
            _ => vec![],
        })
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
        progress(Progress::Message(format!("Upgrading {name}.")));
        let home = self
            .home
            .clone()
            .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
        let result = match self.kind {
            DevKind::Cargo => self.call(&["install", "--force", name], cancel, true)?,
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
                "Updating {} packages.",
                self.kind.id()
            )));
            let result = self.call(&["update", "--global"], cancel, true)?;
            return Ok(OperationOutcome {
                cancellation_deferred: result.cancellation_deferred,
            });
        }
        if self.kind == DevKind::Pipx {
            progress(Progress::Message("Upgrading pipx packages.".into()));
            let result = self.call(&["upgrade-all"], cancel, true)?;
            return Ok(OperationOutcome {
                cancellation_deferred: result.cancellation_deferred,
            });
        }
        if self.kind == DevKind::Uv {
            // `tool upgrade --all` keeps exact pins like per-package upgrade,
            // so reinstall every tool at `@latest` as the single transaction.
            for package in self.inventory(Updates::None, cancel)? {
                let name = &package.package.id.name;
                progress(Progress::Package(name.clone()));
                progress(Progress::Message(format!("Upgrading {name} to latest.")));
                let latest = format!("{name}@latest");
                self.call(&["tool", "install", "--force", &latest], cancel, true)?;
            }
            return Ok(OperationOutcome::default());
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
            progress(Progress::Message("Upgrading global mise tools.".into()));
            let mut args = vec!["upgrade"];
            args.extend(names.iter().map(String::as_str));
            let result = self.call(&args, cancel, true)?;
            return Ok(OperationOutcome {
                cancellation_deferred: result.cancellation_deferred,
            });
        }
        if self.kind == DevKind::Composer {
            // `global update` respects pinned constraints, so re-require each
            // installed package to reach the reported latest candidates.
            for package in self.inventory(Updates::None, cancel)? {
                let name = &package.package.id.name;
                progress(Progress::Package(name.clone()));
                progress(Progress::Message(format!("Upgrading {name}.")));
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
            }
            return Ok(OperationOutcome::default());
        }
        if self.kind == DevKind::Gem {
            progress(Progress::Message("Updating RubyGems.".into()));
            let result = self.call(&["update", "--user-install", "--no-document"], cancel, true)?;
            return Ok(OperationOutcome {
                cancellation_deferred: result.cancellation_deferred,
            });
        }
        for package in self.inventory(Updates::None, cancel)? {
            let name = &package.package.id.name;
            if self.kind == DevKind::Cargo {
                progress(Progress::Package(name.clone()));
                progress(Progress::Message(format!("Reinstalling {name}.")));
                self.call(&["install", "--force", name], cancel, true)?;
            } else if self.kind == DevKind::Pip {
                let home = self
                    .home
                    .clone()
                    .ok_or_else(|| invalid(self.kind.id(), "manager home not detected"))?;
                progress(Progress::Package(name.clone()));
                progress(Progress::Message(format!("Upgrading {name}.")));
                self.pip_call(&home, &["install", "--upgrade", name], cancel, true)?;
            } else {
                progress(Progress::Package(name.clone()));
                progress(Progress::Message(format!("Upgrading {name} to latest.")));
                let latest = format!("{name}@latest");
                self.call(&["add", "--global", &latest], cancel, true)?;
            }
        }
        Ok(OperationOutcome::default())
    }
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
        for (name, version, summary) in self.registry_hits(query, cancel)? {
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

/// Missing optional managers are omitted from automatic queries, but explicit selections
/// and source discovery retain their unavailability. Detection failures are never hidden.
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
    // Every candidate in registration order, with whether it is probed now.
    // Detection spawns native tools (a Node or Python start-up each), so the
    // probes run concurrently instead of one after another.
    let mut candidates: Vec<(Box<dyn Backend>, bool)> = Vec::new();
    // Listed or explicitly selected sources register without a probe when
    // their query runs detection anyway.
    let probe_unless_listed = !(discover || explicit);
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
            candidates.push((Box::new(make(transport())), probe_unless_listed));
        }
    }
    if allowed("fwupd") {
        candidates.push((Box::new(Firmware::new(transport())), true));
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
            candidates.push((Box::new(make(transport())), probe_unless_listed));
        }
    }
    if allowed("pixi") {
        candidates.push((Box::new(Pixi::new(transport())), probe_unless_listed));
    }
    if allowed("conda") {
        // Detection picks conda, mamba, or micromamba, so it always runs.
        candidates.push((Box::new(Conda::new(transport())), true));
    }
    if allowed("rustup") {
        candidates.push((Box::new(Rustup::new(transport())), probe_unless_listed));
    }
    if allowed("oh-my-zsh") {
        candidates.push((Box::new(OhMyZsh::new(transport())), probe_unless_listed));
    }
    if allowed("nix") {
        candidates.push((Box::new(Nix::new(transport())), probe_unless_listed));
    }
    if allowed("go") {
        candidates.push((Box::new(GoBinaries::new(transport())), probe_unless_listed));
    }
    if allowed("dotnet") {
        candidates.push((Box::new(DotnetTools::new(transport())), probe_unless_listed));
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
        match status {
            // Missing optional managers are left out of automatic queries.
            Some(Ok(Availability::Unavailable(_))) if !(discover || explicit) => continue,
            // A detected backend keeps what detection learned (such as a
            // manager's home), so the following query skips its own probe.
            Some(status) => engine.note_detected(id, status),
            None => {}
        }
        engine.register_boxed(backend)?;
    }
    Ok(engine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::Path,
        sync::{Arc, Mutex},
    };

    #[test]
    fn linux_searches_only_offer_casks_linux_can_install() {
        // Shapes `brew info --json=v2 --cask` reports on Linux (Homebrew 7.0.6).
        let cask = |depends_on: serde_json::Value, artifacts: serde_json::Value| {
            serde_json::from_value::<Cask>(serde_json::json!({
                "full_token": "fixture", "name": ["Fixture"], "desc": null,
                "homepage": "", "version": "1", "installed": null, "outdated": false,
                "depends_on": depends_on, "artifacts": artifacts,
            }))
            .unwrap()
        };
        let obsidian = cask(
            serde_json::json!({}),
            serde_json::json!([{"app_image": ["Obsidian.AppImage"]}]),
        );
        let codex = cask(
            serde_json::json!({}),
            serde_json::json!([{"binary": ["codex"]}, {"zap": []}]),
        );
        let font = cask(
            serde_json::json!({}),
            serde_json::json!([{"font": ["Fira.ttf"]}]),
        );
        let rectangle = cask(
            serde_json::json!({"macos": {">=": ["14"]}}),
            serde_json::json!([{"app": ["Rectangle.app"]}]),
        );
        let undeclared_app = cask(
            serde_json::json!({}),
            serde_json::json!([{"app": ["Tool.app"]}]),
        );
        let pkg = cask(
            serde_json::json!({}),
            serde_json::json!([{"pkg": ["Tool.pkg"]}]),
        );
        for installable in [&obsidian, &codex, &font] {
            assert!(cask_installs_here(installable));
        }
        for mac_only in [&rectangle, &undeclared_app, &pkg] {
            assert_eq!(cask_installs_here(mac_only), cfg!(target_os = "macos"));
        }
        // Older reports without these fields still parse.
        assert!(serde_json::from_value::<Cask>(serde_json::json!({
            "full_token": "old", "name": [], "desc": null, "homepage": "",
            "version": "1", "installed": null, "outdated": false
        }))
        .is_ok());
    }

    #[test]
    fn linux_casks_need_homebrew_6() {
        assert!(linux_casks_supported("Homebrew 7.0.6\n"));
        assert!(linux_casks_supported(
            "Homebrew 6.0.0-12-gabcdef\nHomebrew/core"
        ));
        assert!(!linux_casks_supported("Homebrew 5.1.2\n"));
        assert!(!linux_casks_supported("Homebrew 4.6.20\n"));
        assert!(!linux_casks_supported("brew: command not found\n"));
        assert!(!linux_casks_supported(""));
    }

    #[test]
    fn upgrade_everything_lists_packages_where_there_is_no_single_command() {
        for id in [
            "pixi", "rustup", "nix", "aur", "mas", "conda", "fwupd", "codex",
        ] {
            assert!(per_package_upgrades(id), "{id}");
        }
        for id in ["apt", "homebrew", "npm", "mise"] {
            assert!(!per_package_upgrades(id), "{id}");
        }
    }

    #[test]
    fn every_source_has_a_display_name() {
        for id in BACKEND_IDS {
            assert!(!display_name(id).is_empty());
        }
        assert_eq!(display_name("apt"), "APT");
        assert_eq!(display_name("homebrew-cask"), "Homebrew Casks");
        assert_eq!(display_name("npm"), "npm");
        assert_eq!(display_name("unknown-source"), "unknown-source");
        let renamed = BACKEND_IDS
            .iter()
            .filter(|id| display_name(id) != **id)
            .count();
        assert_eq!(renamed, BACKEND_IDS.len() - 9);
    }

    #[test]
    fn exact_lookups_skip_sources_that_cannot_have_the_name() {
        let transport = || NativeTransport {
            host: Host::current(),
            authorization: Authorization::Polkit,
        };
        let flatpak = Flatpak::new(transport());
        assert!(!flatpak.may_have("cowsay"));
        assert!(flatpak.may_have("org.videolan.VLC"));
        assert!(flatpak.may_have("app/org.videolan.VLC/x86_64/stable"));
        assert!(!flatpak.may_have("bad name.x"));
        let npm = DevTool::npm(transport());
        assert!(npm.may_have("@scope/tool"));
        assert!(!npm.may_have("org.example/App/x86_64"));
        let snap = SystemManager::snap(transport());
        assert!(snap.may_have("cowsay"));
        assert!(!snap.may_have("app/org.example.App"));
        assert!(AppImage::native().may_have("anything at all"));
    }

    #[test]
    fn cache_keys_cover_relocations_and_locale() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            }
        };
        assert_eq!(
            flatpak_installation(true, env(&[])),
            Some(PathBuf::from("/var/lib/flatpak"))
        );
        assert_eq!(
            flatpak_installation(false, env(&[("HOME", "/home/me")])),
            Some(PathBuf::from("/home/me/.local/share/flatpak"))
        );
        assert_eq!(
            flatpak_installation(
                false,
                env(&[("HOME", "/home/me"), ("XDG_DATA_HOME", "/data")])
            ),
            Some(PathBuf::from("/data/flatpak"))
        );
        assert_eq!(
            flatpak_installation(
                false,
                env(&[("HOME", "/home/me"), ("XDG_DATA_HOME", "relative")])
            ),
            Some(PathBuf::from("/home/me/.local/share/flatpak"))
        );
        assert_eq!(flatpak_installation(false, env(&[])), None);
        assert_eq!(
            flatpak_installation(true, env(&[("FLATPAK_SYSTEM_DIR", "/srv/flatpak")])),
            None
        );
        assert_eq!(
            flatpak_installation(false, env(&[("FLATPAK_USER_DIR", "/srv/mine")])),
            None
        );
        assert_eq!(
            locale_key(env(&[("LANG", "de_DE.UTF-8")])),
            ["", "", "", "de_DE.UTF-8"]
        );
        assert!(
            flatpak_search_watches(std::path::Path::new("/var/lib/flatpak"))
                .iter()
                .any(|watch| watch.path.ends_with("appstream"))
        );
        assert!(apt_watches()
            .iter()
            .any(|watch| watch.path == std::path::Path::new("/var/lib/dpkg/status")));
    }

    #[test]
    fn apt_dist_upgrade_preview_classifies_actions() {
        let plan = parse_apt_upgrade_plan(b"Reading package lists...\n1 upgraded, 1 newly installed, 2 to remove and 0 not upgraded.\nInst old [1.0] (2.0 Ubuntu:stable [amd64])\nInst dependency (1.0 Ubuntu:stable [amd64])\nRemv retired [1.0]\nPurg obsolete [1.0]\nConf old (2.0 Ubuntu:stable [amd64])\n").unwrap();
        assert_eq!(plan.upgrades, ["old"]);
        assert_eq!(plan.installs, ["dependency"]);
        assert_eq!(plan.removals, ["retired", "obsolete"]);
        assert!(plan.summary().contains("Remove (2): retired, obsolete"));
        assert!(parse_apt_upgrade_plan(
            b"1 upgraded, 0 newly installed, 0 to remove and 0 not upgraded.\nInst\n"
        )
        .is_err());
        assert!(parse_apt_upgrade_plan(
            b"0 upgraded, 0 newly installed, 1 to remove and 0 not upgraded.\n"
        )
        .is_err());
        assert!(parse_apt_upgrade_plan(&[0xff]).is_err());
    }

    #[test]
    fn apt_single_action_preview_keeps_exact_target_and_extra_removal() {
        let operation = Operation::Install(PackageId {
            backend: "apt".into(),
            name: "anonymous".into(),
            architecture: "amd64".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        });
        let plan = parse_apt_transaction_plan(&operation, b"0 upgraded, 2 newly installed, 1 to remove and 0 not upgraded.\nInst anonymous (2.0 Ubuntu:stable [amd64])\nInst dependency (1.0 Ubuntu:stable [amd64])\nRemv obsolete [1.0]\n").unwrap();
        assert_eq!(plan.operation, operation);
        assert_eq!(plan.changes.len(), 3);
        assert_eq!(plan.changes[0].action, PlannedAction::Install);
        assert_eq!(plan.changes[0].candidate_version.as_deref(), Some("2.0"));
        assert_eq!(plan.changes[2].action, PlannedAction::Remove);
        assert_eq!(plan.changes[2].installed_version.as_deref(), Some("1.0"));
        assert!(plan.download_bytes.is_none());
    }

    #[test]
    fn apt_helper_supports_cargo_builds_and_relocated_bundles() {
        let root = std::env::temp_dir().join(format!("pkgdeck-apt-helper-{}", std::process::id()));
        std::fs::create_dir_all(root.join("bundle")).unwrap();
        let executable = root.join("bundle/pkgdeck");
        let built = root.join("cargo-helper");
        let adjacent = root.join("bundle/pkgdeck-apt-query");
        assert!(matches!(
            apt_query_executable(&executable, None),
            Err(ExecutionError::Disabled(message)) if message.contains("APT helper is missing")
        ));
        assert!(apt_query_executable(&executable, built.to_str()).is_err());
        std::fs::write(&built, "synthetic helper").unwrap();
        assert_eq!(
            apt_query_executable(&executable, built.to_str()).unwrap(),
            built
        );
        std::fs::write(&adjacent, "packaged helper").unwrap();
        assert_eq!(
            apt_query_executable(&executable, built.to_str()).unwrap(),
            adjacent
        );
        std::fs::remove_file(&built).unwrap();
        assert_eq!(
            apt_query_executable(&executable, built.to_str()).unwrap(),
            adjacent
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn python_php_ruby_and_node_names_accept_registry_forms_only() {
        for name in [
            "cowsay", "requests", "Pillow", "foo-bar", "foo_bar", "foo.bar", "a", "a1",
        ] {
            assert!(python_name(name), "{name}");
            assert!(gem_name(name), "{name}");
        }
        for name in [
            "",
            "-evil",
            "evil-",
            "evil_",
            "evil.",
            "../evil",
            "evil/thing",
            "https://example.invalid/tool.tar.gz",
            "name@latest",
            "name==1.0",
            "name;evil",
            "white space",
            "évil",
        ] {
            assert!(!python_name(name), "{name}");
            assert!(!gem_name(name), "{name}");
        }
        assert!(python_name(&"a".repeat(214)));
        assert!(!python_name(&"a".repeat(215)));
        for name in ["psr/log", "monolog/monolog", "vendor123/pkg-name_1.2"] {
            assert!(composer_name(name), "{name}");
        }
        for name in [
            "",
            "monolog",
            "/package",
            "vendor/",
            "Vendor/Package",
            "-vendor/package",
            "vendor//package",
            "vendor/pack age",
            "vendor/package/extra",
        ] {
            assert!(!composer_name(name), "{name}");
        }
        assert!(!dev_name(""));
        assert!(!dev_name("@scope"));
        assert!(!dev_name("@scope/tool/extra"));
        assert!(dev_name("@scope/tool"));
        assert!(!dev_name("."));
        assert!(!dev_name(".."));
    }

    #[test]
    fn uv_and_gem_parsers_reject_malformed_lines() {
        assert_eq!(
            DevTool::<NativeTransport>::uv_entry("cowsay v6.0"),
            Some(("cowsay".into(), "6.0".into(), None))
        );
        assert_eq!(
            DevTool::<NativeTransport>::uv_entry("cowsay v6.0 [latest: 6.1]"),
            Some(("cowsay".into(), "6.0".into(), Some("6.1".into())))
        );
        for line in [
            "", "   ", "-", "- cowsay", "nodash", "name ", "name v", "name 6.0",
        ] {
            assert_eq!(DevTool::<NativeTransport>::uv_entry(line), None, "{line}");
        }
        assert_eq!(
            DevTool::<NativeTransport>::gem_filename("cowsay-0.3.0.gemspec"),
            Some(("cowsay".into(), "0.3.0".into()))
        );
        assert_eq!(
            DevTool::<NativeTransport>::gem_filename("foo-bar-1.0.gemspec"),
            Some(("foo-bar".into(), "1.0".into()))
        );
        for file in [
            "nodash.gemspec",
            "README",
            "name-.gemspec",
            "name-x.gemspec",
            "-1.0.gemspec",
        ] {
            assert_eq!(
                DevTool::<NativeTransport>::gem_filename(file),
                None,
                "{file}"
            );
        }
        assert_eq!(
            DevTool::<NativeTransport>::gem_outdated("cowsay (0.3.0 < 0.4.0)"),
            Some(("cowsay".into(), "0.4.0".into()))
        );
        assert_eq!(
            DevTool::<NativeTransport>::gem_outdated("rake (13.0.0, 13.3.0 < 13.4.2)"),
            Some(("rake".into(), "13.4.2".into()))
        );
        for line in [
            "",
            "cowsay",
            "cowsay (0.3.0)",
            "--evil (1.0 < 2.0)",
            "-x (1 < 2)",
        ] {
            assert_eq!(
                DevTool::<NativeTransport>::gem_outdated(line),
                None,
                "{line}"
            );
        }
        use std::cmp::Ordering::*;
        assert_eq!(
            DevTool::<NativeTransport>::gem_version_cmp("1.0", "1.0"),
            Equal
        );
        assert_eq!(
            DevTool::<NativeTransport>::gem_version_cmp("0.2.0", "0.3.0"),
            Less
        );
        assert_eq!(
            DevTool::<NativeTransport>::gem_version_cmp("13.4.2", "13.0.0"),
            Greater
        );
        assert_eq!(
            DevTool::<NativeTransport>::gem_version_cmp("1.0", "1.0.0"),
            Less
        );
        assert_eq!(
            DevTool::<NativeTransport>::gem_version_cmp("1.0.0", "1.0"),
            Greater
        );
        assert_eq!(
            DevTool::<NativeTransport>::gem_version_cmp("1.0.a", "1.0.b"),
            Less
        );
    }

    #[test]
    fn component_stems_share_one_namespace() {
        assert_eq!(
            component_stem("firefox.desktop"),
            Some("firefox".to_string())
        );
        assert_eq!(
            component_stem("org.mozilla.firefox.desktop"),
            Some("org.mozilla.firefox".to_string())
        );
        assert_eq!(
            component_stem("org.mozilla.firefox"),
            Some("org.mozilla.firefox".to_string())
        );
        assert_eq!(component_stem("plain"), Some("plain".to_string()));
        assert_eq!(component_stem(""), None);
        assert_eq!(component_stem(".desktop"), None);
        assert_eq!(component_stem("   "), None);
    }

    #[test]
    fn dep11_maps_packages_to_component_stems() {
        use std::io::Write;
        let base = std::env::temp_dir().join(format!("pkgdeck-dep11-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(
                b"---\nFile: DEP-11\nVersion: '1.0'\n---\nType: desktop-application\nID: org.mozilla.firefox.desktop\nPackage: firefox\n---\nType: desktop-application\nID: firefox.desktop\nPackage: firefox\n---\nType: desktop-application\nID: org.chromium.Chromium.desktop\nPackage: chromium\nDescription:\n  C: >-\n    ID: not-a-component\n---\nType: desktop-application\nID: .desktop\nPackage: emptyid\n",
            )
            .unwrap();
        std::fs::write(
            base.join("test_dep11_Components-amd64.yml.gz"),
            encoder.finish().unwrap(),
        )
        .unwrap();
        // Non-gzipped and corrupt files never contribute.
        std::fs::write(base.join("notes.txt"), "ID: fake.desktop\nPackage: fake\n").unwrap();
        std::fs::write(base.join("broken.yml.gz"), b"not gzip data").unwrap();
        std::os::unix::fs::symlink(base.join("absent"), base.join("dangling.yml.gz")).unwrap();
        let map = dep11_cached(None, &base, dep11_scan);
        assert_eq!(
            map.get("firefox"),
            Some(&vec![
                "firefox".to_string(),
                "org.mozilla.firefox".to_string()
            ])
        );
        assert_eq!(
            map.get("chromium"),
            Some(&vec!["org.chromium.Chromium".to_string()])
        );
        assert!(!map.contains_key("fake"));
        assert!(!map.contains_key("emptyid"));
        assert!(dep11_component_ids(&base.join("missing")).is_empty());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn dep11_maps_are_cached_until_the_data_changes() {
        use std::io::Write;
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-dep11-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let lists = base.join("lists");
        let yaml = base.join("yaml");
        std::fs::create_dir_all(&lists).unwrap();
        std::fs::create_dir_all(&yaml).unwrap();
        let gz = |body: &str| {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(body.as_bytes()).unwrap();
            encoder.finish().unwrap()
        };
        // Like AppStream's APT hook: the folder links into APT's lists.
        let main = lists.join("main_dep11_Components-amd64.yml.gz");
        std::fs::write(
            &main,
            gz("---\nID: org.example.Tool.desktop\nPackage: tool\n"),
        )
        .unwrap();
        std::os::unix::fs::symlink(&main, yaml.join("main.yml.gz")).unwrap();
        let store = crate::cache::Store::new(base.join("cache"));
        let scans = std::cell::Cell::new(0);
        let get = || {
            dep11_cached(Some(&store), &yaml, |files| {
                scans.set(scans.get() + 1);
                dep11_scan(files)
            })
        };
        let tool = |stems: &[&str]| {
            std::collections::BTreeMap::from([(
                "tool".to_string(),
                stems
                    .iter()
                    .map(|stem| stem.to_string())
                    .collect::<Vec<_>>(),
            )])
        };
        assert_eq!(get(), tool(&["org.example.Tool"]));
        assert_eq!(get(), tool(&["org.example.Tool"]));
        assert_eq!(scans.get(), 1);
        // `apt update` rewrites the list behind the unchanged link.
        std::fs::write(
            &main,
            gz("---\nID: org.example.Tool.desktop\nPackage: tool\n---\nID: tool.desktop\nPackage: tool\n"),
        )
        .unwrap();
        assert_eq!(get(), tool(&["org.example.Tool", "tool"]));
        assert_eq!(scans.get(), 2);
        // A link whose list appears later counts as a change too.
        let universe = lists.join("universe_dep11_Components-amd64.yml.gz");
        std::os::unix::fs::symlink(&universe, yaml.join("universe.yml.gz")).unwrap();
        assert_eq!(get(), tool(&["org.example.Tool", "tool"]));
        assert_eq!(get(), tool(&["org.example.Tool", "tool"]));
        assert_eq!(scans.get(), 3);
        std::fs::write(
            &universe,
            gz("---\nID: org.example.Other\nPackage: other\n"),
        )
        .unwrap();
        assert_eq!(get()["other"], vec!["org.example.Other".to_string()]);
        assert_eq!(scans.get(), 4);
        // Without a store (root or PKGDECK_NO_CACHE) every call scans.
        dep11_cached(None, &yaml, |files| {
            scans.set(scans.get() + 1);
            dep11_scan(files)
        });
        assert_eq!(scans.get(), 5);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn snap_desktop_ids_list_gui_entries() {
        let base = std::env::temp_dir().join(format!("pkgdeck-snapids-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let gui = base.join("snap/spot/current/meta/gui");
        std::fs::create_dir_all(&gui).unwrap();
        std::fs::write(gui.join("spot.desktop"), "[Desktop Entry]\n").unwrap();
        std::fs::write(gui.join("spot-settings.desktop"), "[Desktop Entry]\n").unwrap();
        std::fs::write(gui.join("icon.png"), "png").unwrap();
        assert_eq!(
            snap_desktop_ids(&base, "spot"),
            vec!["spot".to_string(), "spot-settings".to_string()]
        );
        assert!(snap_desktop_ids(&base, "missing").is_empty());
        assert!(snap_desktop_ids(&base, "../evil").is_empty());
        assert!(snap_desktop_ids(&base, "").is_empty());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn local_icons_resolve_without_network_access() {
        let base = std::env::temp_dir().join(format!("pkgdeck-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // Snap layout: <sysroot>/snap/<name>/current/meta/gui/icon.<ext>.
        let gui = base.join("snap/hello/current/meta/gui");
        std::fs::create_dir_all(&gui).unwrap();
        std::fs::write(gui.join("icon.svg"), "<svg/>").unwrap();
        assert_eq!(snap_icon(&base, "hello"), Some(gui.join("icon.svg")));
        assert_eq!(snap_icon(&base, "missing"), None);
        assert_eq!(snap_icon(&base, "../evil"), None);
        assert_eq!(snap_icon(&base, ""), None);
        // Flatpak layout: <exports>/share/icons/hicolor/<size>/apps/<id>.<ext>.
        let exports = base.join("exports");
        let apps = exports.join("share/icons/hicolor/64x64/apps");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(apps.join("org.example.App.png"), "png").unwrap();
        assert_eq!(
            flatpak_icon(std::slice::from_ref(&exports), "org.example.App"),
            Some(apps.join("org.example.App.png"))
        );
        assert_eq!(
            flatpak_icon(std::slice::from_ref(&exports), "missing.app"),
            None
        );
        assert_eq!(flatpak_icon(&[exports], "--evil"), None);
        assert_eq!(flatpak_icon(&[], "org.example.App"), None);
        // Debian layout: <info>/<pkg>[_<arch>].list naming desktop entries.
        let info = base.join("info");
        std::fs::create_dir_all(&info).unwrap();
        std::fs::write(
            info.join("brave-browser.list"),
            "/usr/bin/brave\n/usr/share/applications/brave-browser.desktop\n",
        )
        .unwrap();
        std::fs::write(info.join("plain.list"), "/usr/bin/plain\n").unwrap();
        // An unreadable list is skipped.
        std::os::unix::fs::symlink(info.join("absent"), info.join("ghost.list")).unwrap();
        // Non-list files never name a package, and duplicate entries for
        // one package resolve exactly once.
        std::fs::write(info.join("README"), "inventory\n").unwrap();
        std::fs::write(
            info.join("brave-browser:amd64.list"),
            "/usr/bin/brave\n/usr/share/applications/other.desktop\n",
        )
        .unwrap();
        let map = apt_desktop_map(&info);
        // Either list may win the readdir order, but duplicates resolve once.
        let brave = map.get("brave-browser").unwrap();
        assert!(
            brave.ends_with("brave-browser.desktop") || brave.ends_with("other.desktop"),
            "{brave:?}"
        );
        assert_eq!(
            map.values()
                .filter(|p| {
                    p.ends_with("brave-browser.desktop") || p.ends_with("other.desktop")
                })
                .count(),
            1
        );
        assert!(!map.contains_key("plain"));
        assert!(!map.contains_key("README"));
        assert!(!map.contains_key("ghost"));
        assert_eq!(
            map.values()
                .filter(|p| p.ends_with("brave-browser.desktop"))
                .count()
                + map
                    .values()
                    .filter(|p| p.ends_with("other.desktop"))
                    .count(),
            1
        );
        // A bare Icon= name resolves through the theme; absolute paths
        // resolve directly when the file exists.
        let real = base.join("real-icon.png");
        std::fs::write(&real, "png").unwrap();
        let desktop = base.join("myapp.desktop");
        std::fs::write(
            &desktop,
            format!("[Desktop Entry]\nName=Mine\nIcon={}\n", real.display()),
        )
        .unwrap();
        assert_eq!(desktop_icon(None, &desktop), Some(real));
        std::fs::write(&desktop, "[Desktop Entry]\nName=Mine\n").unwrap();
        assert_eq!(desktop_icon(None, &desktop), None);
        std::fs::write(&desktop, "[Desktop Entry]\nName=Mine\nIcon=\n").unwrap();
        assert_eq!(desktop_icon(None, &desktop), None);
        std::fs::write(
            &desktop,
            "[Desktop Entry]\nName=Mine\nIcon=/nowhere/icon.png\n",
        )
        .unwrap();
        assert_eq!(desktop_icon(None, &desktop), None);
        // Relative names with a slash never touch the filesystem.
        std::fs::write(&desktop, "[Desktop Entry]\nName=Mine\nIcon=subdir/icon\n").unwrap();
        assert_eq!(desktop_icon(None, &desktop), None);
        // Unique names miss every theme directory including pixmaps.
        std::fs::write(
            &desktop,
            "[Desktop Entry]\nName=Mine\nIcon=pkgdeck-definitely-missing-icon\n",
        )
        .unwrap();
        assert_eq!(desktop_icon(None, &desktop), None);
        // The user theme directory wins over the system ones.
        let home = base.join("home");
        let theme = home.join(".local/share/icons/hicolor/48x48/apps");
        std::fs::create_dir_all(&theme).unwrap();
        std::fs::write(theme.join("pkgdeck-fixture-icon.png"), "png").unwrap();
        std::fs::write(
            &desktop,
            "[Desktop Entry]\nName=Mine\nIcon=pkgdeck-fixture-icon\n",
        )
        .unwrap();
        assert_eq!(
            desktop_icon(Some(&home), &desktop),
            Some(theme.join("pkgdeck-fixture-icon.png"))
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn update_output_names_the_package_each_manager_is_on() {
        for (backend, line, name) in [
            ("homebrew", "==> Upgrading openssl@3", Some("openssl@3")),
            (
                "homebrew-cask",
                "==> Upgrading astrovm/pkgdeck/pkgdeck",
                Some("pkgdeck"),
            ),
            ("homebrew-cask", "==> Upgrading 3 outdated packages:", None),
            ("homebrew", "  3.6.4 -> 3.6.4_1", None),
            (
                "apt",
                "Unpacking curl:amd64 (8.5.0-2) over (8.4.0-1) ...",
                Some("curl"),
            ),
            ("apt", "Setting up curl:amd64 (8.5.0-2) ...", None),
            (
                "dnf",
                "  Upgrading        : htop-3.3.0-1.fc41.x86_64        1/2",
                Some("htop"),
            ),
            (
                "dnf",
                "[1/4] Upgrading htop-0:3.3.0-1.fc41.x86_64 100% |",
                Some("htop"),
            ),
            ("dnf", "  Upgrading        :", None),
            ("dnf", "Upgrading:", None),
            (
                "zypper",
                "(1/2) Installing: vim-9.1.0-1.1.x86_64 [...done]",
                Some("vim"),
            ),
            ("zypper", "(1/2) Installing: broken [...done]", None),
            (
                "pacman",
                "(1/3) upgrading linux-firmware          [####]",
                Some("linux-firmware"),
            ),
            (
                "apk",
                "(2/5) Upgrading musl (1.2.4-r2 -> 1.2.5-r0)",
                Some("musl"),
            ),
            ("xbps", "htop-3.3.0_1: unpacking ...", Some("htop")),
            ("xbps", "nodash: unpacking ...", None),
            ("macports", "--->  Installing wget @1.24.5_0", Some("wget")),
            (
                "flatpak",
                "Updating app/org.gnome.Maps/x86_64/stable",
                Some("org.gnome.Maps"),
            ),
            (
                "flatpak",
                "Updating org.freedesktop.Platform",
                Some("org.freedesktop.Platform"),
            ),
            (
                "snap",
                "firefox 131.0 from Mozilla✓ refreshed",
                Some("firefox"),
            ),
            ("snap", "All snaps up to date.", None),
            (
                "pipx",
                "upgraded package black from 24.1 to 24.2 (location: ...)",
                Some("black"),
            ),
            ("gem", "Updating rake", Some("rake")),
            ("gem", "Updating installed gems", None),
            (
                "mas",
                "==> Downloading Final Cut Pro (11.0)",
                Some("Final Cut Pro"),
            ),
            ("mas", "==> Downloading Xcode", Some("Xcode")),
            ("npm", "changed 3 packages in 2s", None),
        ] {
            assert_eq!(
                output_package(backend, line).as_deref(),
                name,
                "{backend}: {line}"
            );
        }
        assert_eq!(output_package("flatpak", "Updating "), None);
        assert_eq!(output_package("mas", "==> Downloading  "), None);
    }

    const CODE_APP: &str = "/Applications/Visual Studio Code.app";
    const CODE_COMMAND: &str =
        "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code";

    /// `brew` answering for a few casks; any other name is no cask.
    #[derive(Clone, Default)]
    struct CaskBrew {
        version: &'static str,
        installed: Arc<Mutex<bool>>,
        calls: Arc<Mutex<Vec<String>>>,
    }
    impl Transport for CaskBrew {
        fn brew(
            &self,
            args: &[OsString],
            _: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            let line = args
                .iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            self.calls.lock().unwrap().push(line.clone());
            let installed = (*self.installed.lock().unwrap()).then_some("1.139.1");
            let cask = |token: &str, artifacts: serde_json::Value| {
                serde_json::json!({"casks": [{
                    "full_token": token, "name": [token], "desc": null,
                    "homepage": "https://example.com", "version": "1.139.1",
                    "installed": installed, "outdated": false, "auto_updates": true,
                    "artifacts": artifacts,
                }]})
                .to_string()
            };
            let stdout = match line.as_str() {
                "--prefix" => "/home/linuxbrew/.linuxbrew\n".into(),
                "--version" => format!("Homebrew {}\n", self.version),
                "casks" => "codex\ncodex-cli\nvisual-studio-code\n".into(),
                "info --json=v2 --cask -- visual-studio-code" => cask(
                    "visual-studio-code",
                    serde_json::json!([
                        {"app": ["Visual Studio Code.app"], "target": CODE_APP},
                        {"binary": [CODE_COMMAND], "target": "/home/linuxbrew/.linuxbrew/bin/code"},
                    ]),
                ),
                "info --json=v2 --cask -- codex" => {
                    cask("codex", serde_json::json!([{"binary": ["codex"]}]))
                }
                "info --json=v2 --cask -- codex-cli" => {
                    cask("codex-cli", serde_json::json!([{"binary": ["codex"]}]))
                }
                "info --json=v2 --cask -- codex codex-cli" => {
                    let mut both: serde_json::Value = serde_json::from_str(&cask(
                        "codex",
                        serde_json::json!([{"binary": ["codex"]}]),
                    ))
                    .unwrap();
                    let other: serde_json::Value = serde_json::from_str(&cask(
                        "codex-cli",
                        serde_json::json!([{"binary": ["codex"]}]),
                    ))
                    .unwrap();
                    both["casks"]
                        .as_array_mut()
                        .unwrap()
                        .push(other["casks"][0].clone());
                    both.to_string()
                }
                "upgrade --cask" => {
                    assert!(write);
                    String::new()
                }
                "install --cask --adopt -- visual-studio-code" => {
                    assert!(write);
                    *self.installed.lock().unwrap() = true;
                    String::new()
                }
                _ => {
                    return Err(ExecutionError::Failed(Completion {
                        code: Some(1),
                        signal: None,
                        stdout: vec![],
                        stderr: b"Error: No available cask".to_vec(),
                        truncated: false,
                        cancellation_deferred: false,
                    }))
                }
            };
            Ok(Completion {
                code: Some(0),
                signal: None,
                stdout: stdout.into_bytes(),
                stderr: vec![],
                truncated: false,
                cancellation_deferred: false,
            })
        }
    }

    /// A signed copy of the app already in place, with the cask's command
    /// already linked where Homebrew would link it.
    /// The second field makes the app vanish once it is backed up, as when
    /// Homebrew fails halfway through adopting it.
    struct Adoptable(Arc<Mutex<Vec<&'static str>>>, bool);
    impl adopt::AdoptIo for Adoptable {
        fn occupied(&self, path: &Path) -> bool {
            path == Path::new(CODE_APP) || path.ends_with("bin/code")
        }
        fn is_folder(&self, path: &Path) -> bool {
            path == Path::new(CODE_APP) && !(self.1 && self.0.lock().unwrap().contains(&"backup"))
        }
        fn is_file(&self, path: &Path) -> bool {
            path == Path::new(CODE_COMMAND)
        }
        fn links_to(&self, link: &Path, source: &Path) -> bool {
            link.starts_with("/home/linuxbrew") && source == Path::new(CODE_COMMAND)
        }
        fn plist(&self, _: &Path, _: &Cancellation) -> Result<serde_json::Value, EngineError> {
            Ok(serde_json::json!({
                "CFBundleIdentifier": "com.microsoft.VSCode",
                "CFBundleExecutable": "Electron",
                "CFBundleShortVersionString": "1.139.1",
            }))
        }
        fn team(&self, _: &Path, _: &Cancellation) -> Result<Option<String>, EngineError> {
            Ok(Some("UBF8T346G9".into()))
        }
        fn architectures(&self, _: &Path, _: &Cancellation) -> Result<Vec<String>, EngineError> {
            Ok(vec!["x86_64".into(), "arm64".into()])
        }
        fn backup(&self, app: &Path, _: &Cancellation) -> Result<PathBuf, EngineError> {
            self.0.lock().unwrap().push("backup");
            Ok(Path::new("/backups/1").join(app.file_name().unwrap()))
        }
        fn restore(&self, _: &Path, _: &Path, _: &Cancellation) -> Result<(), EngineError> {
            self.0.lock().unwrap().push("restore");
            Ok(())
        }
        fn discard(&self, _: &Path) {
            self.0.lock().unwrap().push("discard");
        }
    }

    #[test]
    fn cask_search_reads_spaces_as_the_dashes_in_tokens() {
        let mut casks = HomebrewCask::new(CaskBrew {
            version: "7.0.6",
            ..CaskBrew::default()
        });
        let cancel = Cancellation::default();
        assert_eq!(casks.detect(&cancel).unwrap(), Availability::Available);
        // A binary-only cask, so Linux searches offer it too.
        for query in ["codex cli", "Codex_CLI", "codex-cli"] {
            let found = casks.search(query, &cancel).unwrap();
            assert_eq!(
                found.iter().map(|p| p.id.name.as_str()).collect::<Vec<_>>(),
                ["codex-cli"],
                "{query}"
            );
        }
        assert!(casks.search("codex x", &cancel).unwrap().is_empty());
    }

    #[test]
    fn narrower_cask_searches_reuse_what_brew_already_read() {
        let brew = CaskBrew {
            version: "7.0.6",
            ..CaskBrew::default()
        };
        let mut casks = HomebrewCask::new(brew.clone());
        let cancel = Cancellation::default();
        casks.detect(&cancel).unwrap();
        let names = |found: Vec<Package>| found.into_iter().map(|p| p.id.name).collect::<Vec<_>>();
        let reads = || {
            brew.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| call.starts_with("info "))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(casks.search("codex", &cancel).unwrap()),
            ["codex", "codex-cli"]
        );
        assert_eq!(reads(), ["info --json=v2 --cask -- codex codex-cli"]);
        // One more letter: answered without asking brew again.
        assert_eq!(
            names(casks.search("codex-c", &cancel).unwrap()),
            ["codex-cli"]
        );
        assert_eq!(reads().len(), 1);
        // A change may install or update any of them, so they're read again.
        let _ = casks.execute(
            &Operation::Refresh {
                backend: "homebrew-cask".into(),
            },
            &cancel,
            &mut |_| {},
        );
        assert_eq!(
            names(casks.search("codex-c", &cancel).unwrap()),
            ["codex-cli"]
        );
        assert_eq!(
            reads().last().unwrap(),
            "info --json=v2 --cask -- codex-cli"
        );
    }

    #[test]
    fn a_cask_install_adopts_the_copy_already_in_place() {
        let brew = CaskBrew {
            version: "7.0.6",
            ..CaskBrew::default()
        };
        let log = Arc::new(Mutex::new(vec![]));
        let mut casks = HomebrewCask::new(brew.clone());
        casks.adoption = Some(Box::new(Adoptable(log.clone(), false)));
        let cancel = Cancellation::default();
        assert_eq!(casks.detect(&cancel).unwrap(), Availability::Available);
        let id = PackageId {
            backend: "homebrew-cask".into(),
            name: "visual-studio-code".into(),
            architecture: std::env::consts::ARCH.into(),
            scope: Scope::Environment {
                path: "/home/linuxbrew/.linuxbrew".into(),
            },
            remote: None,
            reference: None,
        };
        let mut messages = vec![];
        casks
            .execute(&Operation::Install(id.clone()), &cancel, &mut |progress| {
                if let Progress::Message(message) = progress {
                    messages.push(message);
                }
            })
            .unwrap();
        assert!(brew
            .calls
            .lock()
            .unwrap()
            .contains(&"install --cask --adopt -- visual-studio-code".into()));
        assert_eq!(*log.lock().unwrap(), vec!["backup", "discard"]);
        // The preview says the copy in place is adopted, with what was checked.
        let plan = casks
            .operation_plan(&Operation::Install(id.clone()), &cancel)
            .unwrap()
            .unwrap();
        assert!(
            plan.native_preview
                .starts_with("Visual Studio Code is already in /Applications (version"),
            "{}",
            plan.native_preview
        );
        assert!(plan.native_preview.contains("brew install --cask --adopt"));
        assert_eq!(
            plan.adopts.as_deref(),
            Some(std::path::Path::new("/Applications/Visual Studio Code.app"))
        );
        // Without an app in place there is no special preview.
        let mut plain = HomebrewCask::new(brew.clone());
        plain.detect(&cancel).unwrap();
        assert!(plain
            .operation_plan(&Operation::Install(id.clone()), &cancel)
            .unwrap()
            .is_none());
        assert_eq!(
            messages.last().unwrap(),
            "Homebrew now manages Visual Studio Code."
        );
    }

    #[test]
    fn a_failed_cask_adoption_puts_the_app_back() {
        let brew = CaskBrew {
            version: "7.0.6",
            ..CaskBrew::default()
        };
        let log = Arc::new(Mutex::new(vec![]));
        let mut casks = HomebrewCask::new(brew.clone());
        casks.adoption = Some(Box::new(Adoptable(log.clone(), true)));
        let cancel = Cancellation::default();
        casks.detect(&cancel).unwrap();
        let id = |name: &str| PackageId {
            backend: "homebrew-cask".into(),
            name: name.into(),
            architecture: std::env::consts::ARCH.into(),
            scope: Scope::Environment {
                path: "/home/linuxbrew/.linuxbrew".into(),
            },
            remote: None,
            reference: None,
        };
        let error = casks
            .execute(
                &Operation::Install(id("visual-studio-code")),
                &cancel,
                &mut |_| {},
            )
            .unwrap_err();
        assert!(error.to_string().contains("PkgDeck put it back"), "{error}");
        assert_eq!(*log.lock().unwrap(), vec!["backup", "restore", "discard"]);
        // A cask brew doesn't know fails before anything is touched.
        assert!(matches!(
            casks.operation_plan(&Operation::Install(id("unknown-cask")), &cancel),
            Err(EngineError::Execution(ExecutionError::Failed(_)))
        ));
        assert_eq!(log.lock().unwrap().len(), 3);
        // So does a lookup brew answers with a failing exit status.
        struct Refusing;
        impl Transport for Refusing {
            fn brew(
                &self,
                _: &[OsString],
                _: &Cancellation,
                _: bool,
            ) -> Result<Completion, ExecutionError> {
                Ok(Completion {
                    code: Some(1),
                    signal: None,
                    stdout: vec![],
                    stderr: vec![],
                    truncated: false,
                    cancellation_deferred: false,
                })
            }
        }
        let mut refusing = HomebrewCask::new(Refusing);
        refusing.adoption = Some(Box::new(Adoptable(log.clone(), false)));
        refusing.prefix = Some("/opt/homebrew".into());
        let mut code = id("visual-studio-code");
        code.scope = Scope::Environment {
            path: "/opt/homebrew".into(),
        };
        assert!(matches!(
            refusing.operation_plan(&Operation::Install(code), &cancel),
            Err(EngineError::Execution(ExecutionError::Failed(result))) if result.code == Some(1)
        ));
        assert_eq!(log.lock().unwrap().len(), 3);
    }

    #[test]
    fn exact_cask_names_are_looked_up_directly() {
        let brew = CaskBrew {
            version: "7.0.6",
            ..CaskBrew::default()
        };
        let mut casks = HomebrewCask::new(brew.clone());
        let cancel = Cancellation::default();
        casks.detect(&cancel).unwrap();
        let found = casks.lookup("codex", &cancel).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id.name, "codex");
        // Not a cask, or not a cask name at all: nothing, without an error.
        assert!(casks.lookup("no-such-cask", &cancel).unwrap().is_empty());
        let calls = brew.calls.lock().unwrap().len();
        assert!(casks.lookup("--help", &cancel).unwrap().is_empty());
        assert_eq!(brew.calls.lock().unwrap().len(), calls);
        casks
            .execute(
                &Operation::UpgradeAll {
                    backend: "homebrew-cask".into(),
                },
                &cancel,
                &mut |_| {},
            )
            .unwrap();
        assert!(brew
            .calls
            .lock()
            .unwrap()
            .contains(&"upgrade --cask".into()));
    }

    #[test]
    fn linux_casks_wait_for_homebrew_6() {
        let mut casks = HomebrewCask::new(CaskBrew {
            version: "5.1.2",
            ..CaskBrew::default()
        });
        let availability = casks.detect(&Cancellation::default()).unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(availability, Availability::Available);
        } else {
            assert_eq!(
                availability,
                Availability::Unavailable(
                    "Homebrew Casks on Linux need Homebrew 6.0 or later".into()
                )
            );
        }
    }
}

#[cfg(test)]
mod native_transport_tests {
    use super::*;
    use crate::host::Runtime;
    use std::{os::unix::fs::PermissionsExt, path::Path};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pkgdeck-native-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    /// An executable shell script, renamed into place so no writer still
    /// holds it open when it runs.
    fn script(path: &Path, body: &str) {
        let staging = path.with_extension("staging");
        std::fs::write(&staging, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(staging, path).unwrap();
    }
    /// A transport whose host sees only `bin` on PATH and `home` as HOME.
    fn transport(bin: &Path, home: &Path) -> NativeTransport {
        NativeTransport {
            host: Host::new(
                Runtime::Native,
                [
                    ("PATH".into(), bin.as_os_str().to_owned()),
                    ("HOME".into(), home.as_os_str().to_owned()),
                ]
                .into(),
            ),
            authorization: Authorization::SudoNonInteractive,
        }
    }
    fn records(result: Result<Completion, ExecutionError>) -> Vec<PackageDetails> {
        serde_json::from_slice(&result.unwrap().stdout).unwrap()
    }

    #[test]
    fn host_probes_report_what_the_sanitized_path_offers() {
        let base = temp_dir("probes");
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let native = transport(&bin, &base);
        let cancel = Cancellation::default();
        assert!(matches!(
            native.authorization(),
            Authorization::SudoNonInteractive
        ));
        // No apt-get on PATH: no Software Sources editor.
        assert!(!native.repository_editor_available());
        assert!(matches!(
            native.repository_editor(),
            Err(ExecutionError::Disabled(_))
        ));
        assert_eq!(
            native.system_flatpak_writable(),
            cfg!(target_os = "linux") && Path::new("/usr/bin/flatpak").is_file()
        );
        // The CLI cannot preview unused runtimes exactly, so it never offers it.
        assert!(!native.supports_flatpak_cleanup());
        assert!(matches!(
            native.flatpak_unused(&cancel),
            Err(ExecutionError::Disabled(_))
        ));
        // An empty transaction is refused before anything runs.
        assert!(matches!(
            native.apt_write_group(&[], &cancel),
            Err(ExecutionError::Invalid(_))
        ));
        // `docker` is Podman's compatibility wrapper only when it links there.
        assert!(!native.docker_is_podman());
        script(&bin.join("podman"), "exit 0");
        std::os::unix::fs::symlink(bin.join("podman"), bin.join("docker")).unwrap();
        assert!(native.docker_is_podman());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn native_engine_leaves_out_a_docker_that_is_podman() {
        let base = temp_dir("engine");
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        script(&bin.join("podman"), "exit 0");
        std::os::unix::fs::symlink(bin.join("podman"), bin.join("docker")).unwrap();
        let host = transport(&bin, &base).host;
        let cancel = Cancellation::default();
        let sources = |ids: &[&str]| {
            native_engine_on(
                host.clone(),
                &ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
                false,
                Authorization::SudoNonInteractive,
                &cancel,
            )
            .unwrap()
            .discover(&cancel)
            .into_iter()
            .map(|source| source.backend)
            .collect::<Vec<_>>()
        };
        assert_eq!(sources(&["docker", "podman"]), ["podman"]);
        // Asked for alone, Docker stays so it can explain itself.
        assert_eq!(sources(&["docker"]), ["docker"]);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn remote_flatpak_searches_are_answered_from_the_cache_until_appstream_changes() {
        let base = temp_dir("flatpak-search");
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let flatpak = bin.join("flatpak");
        script(&flatpak, "printf 'first\\tresult\\n'");
        let native = transport(&bin, &base);
        let store = || Some(crate::cache::Store::new(base.join("cache")));
        let cancel = Cancellation::default();
        let search = [
            "--user",
            "search",
            "--columns=name,description,application,version,branch,remotes",
            "editor",
        ]
        .map(OsString::from);
        let stdout = |result: Result<Completion, ExecutionError>| {
            String::from_utf8(result.unwrap().stdout).unwrap()
        };
        assert_eq!(
            stdout(native.flatpak_cached(&search, &cancel, false, false, store)),
            "first\tresult\n"
        );
        script(&flatpak, "printf 'second\\tresult\\n'");
        assert_eq!(
            stdout(native.flatpak_cached(&search, &cancel, false, false, store)),
            "first\tresult\n"
        );
        // The installation's AppStream data changed: search again.
        std::fs::create_dir_all(base.join(".local/share/flatpak/appstream/flathub")).unwrap();
        assert_eq!(
            stdout(native.flatpak_cached(&search, &cancel, false, false, store)),
            "second\tresult\n"
        );
        // Other reads always run.
        let cached = || std::fs::read_dir(base.join("cache")).unwrap().count();
        let entries = cached();
        let list = ["--user", "list"].map(OsString::from);
        assert_eq!(
            stdout(native.flatpak_cached(&list, &cancel, false, false, store)),
            "second\tresult\n"
        );
        assert_eq!(cached(), entries);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn apt_helper_rejects_unknown_query_modes() {
        let base = temp_dir("apt-helper");
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        script(&bin.join("apt-get"), "exit 0");
        let native = transport(&bin, &base);
        let error = native
            .apt_query("erase", "", "", &Cancellation::default())
            .unwrap_err();
        // Built with libapt-pkg, the helper refuses the mode itself;
        // elsewhere there is no helper to run.
        let refused = matches!(&error, ExecutionError::Failed(result) if result.code == Some(2));
        let missing = matches!(&error, ExecutionError::Disabled(reason) if reason.starts_with("APT helper is missing"));
        assert!(refused || missing, "{error:?}");
        // A cancelled query never starts the helper.
        let cancel = Cancellation::default();
        cancel.cancel();
        let error = native.apt_query("detect", "", "", &cancel).unwrap_err();
        let cancelled = matches!(error, ExecutionError::Cancelled);
        assert_eq!(cancelled, refused, "{error:?}");
        std::fs::remove_dir_all(base).unwrap();
    }

    /// `dpkg-query` and `apt-cache` stand-ins; `apt-cache` logs its
    /// arguments beside itself.
    fn apt_tools(dpkg: &str, search: &str, cache_exit: i32) -> (PathBuf, NativeTransport) {
        let base = temp_dir("apt-sandbox");
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        script(&bin.join("dpkg-query"), &format!("printf '{dpkg}'"));
        script(
            &bin.join("apt-cache"),
            &format!(
                "printf '%s\\n' \"$*\" >> \"$0.log\"\ncase \"$1\" in\n\
                 policy) printf 'tool:\\n  Installed: 1.0\\n  Candidate: 1.0\\n  Version table:\\n *** 1.0 500\\n';;\n\
                 show) printf 'Package: tool\\nArchitecture: all\\nVersion: 1.0\\nDescription-en: A tool\\n\\n';;\n\
                 search) printf '{search}';;\nesac\nexit {cache_exit}"
            ),
        );
        let native = transport(&bin, &base);
        (base, native)
    }
    fn cache_log(base: &Path) -> String {
        std::fs::read_to_string(base.join("bin/apt-cache.log")).unwrap_or_default()
    }

    #[test]
    fn sandboxed_apt_queries_handle_empty_and_architecture_independent_rows() {
        let cancel = Cancellation::default();
        // No installed rows: no apt-cache round trips.
        let (base, native) = apt_tools("", "", 0);
        assert!(records(native.apt_query_sandboxed("installed", "", "", &cancel)).is_empty());
        assert_eq!(cache_log(&base), "");
        std::fs::remove_dir_all(base).unwrap();
        // Architecture-independent packages are named without a suffix.
        let (base, native) = apt_tools(
            "tool\\tall\\t1.0\\tinstall ok installed\\n",
            "tool - A tool\\n",
            0,
        );
        let installed = records(native.apt_query_sandboxed("installed", "", "", &cancel));
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].package.id.name, "tool");
        assert_eq!(cache_log(&base), "policy tool\nshow tool\n");
        let details = records(native.apt_query_sandboxed("details", "tool", "all", &cancel));
        assert_eq!(details.len(), 1);
        assert!(cache_log(&base).ends_with("policy tool\nshow tool\n"));
        // Pattern characters are escaped for apt-cache and matched literally.
        assert!(records(native.apt_query_sandboxed("search", "c++", "", &cancel)).is_empty());
        assert!(cache_log(&base).ends_with("search c\\+\\+\n"));
        std::fs::remove_dir_all(base).unwrap();
        // Each word narrows apt-cache, and separators don't have to match.
        let (base, native) = apt_tools("", "kdeconnect - Phone integration\\n", 0);
        native
            .apt_query_sandboxed("search", "KDE connect", "", &cancel)
            .unwrap();
        assert!(cache_log(&base).starts_with("search KDE connect\npolicy kdeconnect\n"));
        // A search of only separators is still sent, escaped.
        native
            .apt_query_sandboxed("search", ".", "", &cancel)
            .unwrap();
        assert!(cache_log(&base).contains("search \\.\n"));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn sandboxed_apt_queries_report_failed_reads() {
        let (base, native) = apt_tools("", "", 1);
        assert!(matches!(
            native.apt_query_sandboxed("details", "tool", "amd64", &Cancellation::default()),
            Err(ExecutionError::Failed(result)) if result.code == Some(1)
        ));
        assert_eq!(cache_log(&base), "policy tool:amd64\n");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn installed_apt_packages_show_their_desktop_icons() {
        #[derive(Clone)]
        struct Helper(String);
        impl Transport for Helper {
            fn apt_query(
                &self,
                _: &str,
                _: &str,
                _: &str,
                _: &Cancellation,
            ) -> Result<Completion, ExecutionError> {
                Ok(Completion {
                    code: Some(0),
                    signal: None,
                    stdout: self.0.clone().into_bytes(),
                    stderr: vec![],
                    truncated: false,
                    cancellation_deferred: false,
                })
            }
        }
        let base = temp_dir("apt-icons");
        let icon = base.join("tool.png");
        std::fs::write(&icon, "png").unwrap();
        let desktop = base.join("tool.desktop");
        std::fs::write(
            &desktop,
            format!("[Desktop Entry]\nIcon={}\n", icon.display()),
        )
        .unwrap();
        let id = PackageId {
            backend: "apt".into(),
            name: "tool".into(),
            architecture: "amd64".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        };
        let row = serde_json::json!([{
            "package": {
                "id": id, "display_name": "tool", "summary": "A tool",
                "installed_version": "1.0", "candidate_version": "1.0",
                "update": "current", "icon": null, "component_ids": [], "homepages": [],
            },
            "description": "A tool", "homepage": "https://example.invalid", "dependencies": [],
        }]);
        let mut apt = Apt::new(Helper(row.to_string()));
        apt.desktop_entries = Some([("tool".to_string(), desktop)].into());
        apt.components = Some([("tool".to_string(), vec!["org.example.Tool".into()])].into());
        let cancel = Cancellation::default();
        assert_eq!(
            apt.search("tool", &cancel).unwrap()[0].icon.as_ref(),
            Some(&icon)
        );
        assert_eq!(
            apt.installed(&cancel).unwrap()[0].icon.as_ref(),
            Some(&icon)
        );
        let details = apt.details(&id, &cancel).unwrap();
        assert_eq!(details.package.icon.as_ref(), Some(&icon));
        assert_eq!(details.package.component_ids, ["org.example.Tool"]);
        assert_eq!(details.package.homepages, ["https://example.invalid"]);
        std::fs::remove_dir_all(base).unwrap();
    }
}
