//! Sanitized host execution boundary, including the Flatpak host bridge.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::process::{self, Cancellation, Completion, ExecutionError, Limits};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Runtime {
    Native,
    AppImage,
    Flatpak,
    Snap,
}

impl Runtime {
    /// The runtime this program runs in.
    pub fn detect(env: &BTreeMap<OsString, OsString>, flatpak_info: bool) -> Self {
        Self::detect_for(env, flatpak_info, std::env::current_exe().ok().as_deref())
    }
    /// [`detect`](Self::detect) for the program at `executable`. Everything
    /// an AppImage starts inherits its variables, such as a terminal and
    /// what runs in it, so they only count when this program is inside the
    /// AppImage's folder.
    pub fn detect_for(
        env: &BTreeMap<OsString, OsString>,
        flatpak_info: bool,
        executable: Option<&Path>,
    ) -> Self {
        let appdir = env
            .get(&OsString::from("APPDIR"))
            .map(PathBuf::from)
            .map(|dir| fs::canonicalize(&dir).unwrap_or(dir));
        if env.contains_key(&OsString::from("SNAP")) {
            Self::Snap
        } else if flatpak_info || env.contains_key(&OsString::from("FLATPAK_ID")) {
            Self::Flatpak
        } else if appdir
            .zip(executable)
            .is_some_and(|(dir, executable)| executable.starts_with(dir))
        {
            Self::AppImage
        } else {
            Self::Native
        }
    }

    /// Whether host tools and their files are seen as they are, so their
    /// answers can be kept: true outside a sandbox.
    pub fn sees_host_files(self) -> bool {
        matches!(self, Self::Native | Self::AppImage)
    }

    pub fn disabled_reason(self) -> Option<&'static str> {
        None
    }
}

/// A snapshot of the invoking user's environment, stripped of packaging/runtime injection.
#[derive(Clone, Debug)]
pub struct Host {
    pub runtime: Runtime,
    env: BTreeMap<OsString, OsString>,
    excluded: Vec<PathBuf>,
    bridge: PathBuf,
}

/// Why a write refuses to start when the frontend itself runs as root.
pub const ROOT_REFUSAL: &str = "PkgDeck can't make changes when it runs as root. Run it as your normal user; it asks for permission when needed.";

/// Writes refuse to run as root (see [`ROOT_REFUSAL`]).
pub(crate) fn refuse_root(write: bool) -> Result<(), ExecutionError> {
    refuse_root_as(write, rustix::process::geteuid().is_root())
}
fn refuse_root_as(write: bool, root: bool) -> Result<(), ExecutionError> {
    if write && root {
        return Err(ExecutionError::Invalid(ROOT_REFUSAL.into()));
    }
    Ok(())
}

/// Points sudo at the password dialog bundled with the macOS app, so a
/// Homebrew cask that needs administrator access (Docker Desktop removing
/// its helper tools, for one) can ask for the password without a terminal.
/// A prompt the person set up themselves wins; other builds have no helper.
fn use_askpass(env: &mut BTreeMap<OsString, OsString>, executable: Option<&Path>) {
    let helper = executable
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|contents| contents.join("Resources/pkgdeck-askpass"))
        .filter(|helper| helper.is_file());
    if let Some(helper) = helper {
        env.entry("SUDO_ASKPASS".into())
            .or_insert_with(|| helper.into_os_string());
    }
}

#[cfg(test)]
mod askpass_tests {
    use super::*;
    #[test]
    fn homebrew_asks_for_passwords_with_the_bundled_dialog() {
        let bundle = std::env::temp_dir()
            .join(format!("pkgdeck-askpass-{}", std::process::id()))
            .join("PkgDeck.app/Contents");
        fs::create_dir_all(bundle.join("MacOS")).unwrap();
        fs::create_dir_all(bundle.join("Resources")).unwrap();
        let executable = bundle.join("MacOS/pkgdeck");
        let mut env = BTreeMap::new();
        use_askpass(&mut env, Some(&executable));
        assert!(env.is_empty(), "no helper, no prompt");
        let helper = bundle.join("Resources/pkgdeck-askpass");
        fs::write(&helper, "#!/bin/sh\n").unwrap();
        use_askpass(&mut env, Some(&executable));
        assert_eq!(
            env.get(&OsString::from("SUDO_ASKPASS")),
            Some(&helper.clone().into_os_string())
        );
        let mut own = BTreeMap::from([(
            OsString::from("SUDO_ASKPASS"),
            OsString::from("/usr/local/bin/mine"),
        )]);
        use_askpass(&mut own, Some(&executable));
        assert_eq!(
            own.get(&OsString::from("SUDO_ASKPASS")),
            Some(&OsString::from("/usr/local/bin/mine"))
        );
        let mut none = BTreeMap::new();
        use_askpass(&mut none, None);
        assert!(none.is_empty());
        fs::remove_dir_all(bundle.parent().unwrap().parent().unwrap()).unwrap();
    }
}

pub const BACKENDS: &[(&str, &str)] = &[
    ("APT", "apt-get"),
    ("DNF", "dnf"),
    ("Pacman", "pacman"),
    ("Zypper", "zypper"),
    ("apk", "apk"),
    ("XBPS", "xbps-install"),
    ("MacPorts", "port"),
    ("Nix", "nix"),
    ("Flatpak", "flatpak"),
    ("Snap", "snap"),
    ("Firmware (fwupd)", "fwupdmgr"),
    ("Mac App Store (mas)", "mas"),
    ("Homebrew", "brew"),
    ("Docker", "docker"),
    ("Podman", "podman"),
    ("Toolbx", "toolbox"),
    ("Distrobox", "distrobox"),
    ("Cargo", "cargo"),
    ("rustup", "rustup"),
    ("Go", "go"),
    (".NET", "dotnet"),
    ("npm", "npm"),
    ("pnpm", "pnpm"),
    ("Bun", "bun"),
    ("pip (virtual environment)", "pip3"),
    ("pipx", "pipx"),
    ("uv", "uv"),
    ("mise", "mise"),
    ("pixi", "pixi"),
    ("conda", "conda"),
    ("Composer", "composer"),
    ("RubyGems", "gem"),
];

fn repository_install_args(
    source: &Path,
    backend: &str,
    suffix: &str,
    digest: &str,
) -> Result<Vec<OsString>, ExecutionError> {
    if !source.is_absolute()
        || !source.is_file()
        || digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ExecutionError::Invalid("invalid repository file".into()));
    }
    let directory = match (backend, suffix) {
        ("apt", "sources" | "list") => "/etc/apt/sources.list.d",
        ("dnf", "repo") => "/etc/yum.repos.d",
        ("zypper", "repo") => "/etc/zypp/repos.d",
        _ => {
            return Err(ExecutionError::Invalid(
                "unsupported repository format".into(),
            ))
        }
    };
    let destination = Path::new(directory).join(format!("pkgdeck-{digest}.{suffix}"));
    Ok(vec![
        "--mode=0644".into(),
        "--no-target-directory".into(),
        "--".into(),
        source.as_os_str().to_os_string(),
        destination.as_os_str().to_os_string(),
    ])
}

#[cfg(test)]
mod repository_install_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn repository_destinations_are_fixed_and_content_addressed() {
        let source =
            std::env::temp_dir().join(format!("pkgdeck-repo-install-{}", std::process::id()));
        std::fs::write(&source, b"synthetic source").unwrap();
        let digest = "a".repeat(64);
        for (backend, suffix, destination) in [
            ("apt", "sources", "/etc/apt/sources.list.d"),
            ("apt", "list", "/etc/apt/sources.list.d"),
            ("dnf", "repo", "/etc/yum.repos.d"),
            ("zypper", "repo", "/etc/zypp/repos.d"),
        ] {
            let args = repository_install_args(&source, backend, suffix, &digest).unwrap();
            assert_eq!(args[0], "--mode=0644");
            assert_eq!(args[2], "--");
            assert_eq!(args[3], source.as_os_str());
            assert_eq!(
                Path::new(&args[4]).parent().unwrap(),
                Path::new(destination)
            );
            assert_eq!(
                Path::new(&args[4]).file_name().unwrap().to_str().unwrap(),
                format!("pkgdeck-{digest}.{suffix}")
            );
        }
        assert!(repository_install_args(&source, "apt", "repo", &digest).is_err());
        assert!(repository_install_args(&source, "apt", "sources", "unsafe").is_err());
        assert!(
            repository_install_args(Path::new("relative.sources"), "apt", "sources", &digest)
                .is_err()
        );
        std::fs::remove_file(source).unwrap();
    }
    #[test]
    fn reviewed_repository_write_passes_only_fixed_install_arguments() {
        let source =
            std::env::temp_dir().join(format!("pkgdeck-reviewed-source-{}", std::process::id()));
        std::fs::write(&source, b"synthetic source").unwrap();
        let host = Host::new(Runtime::Native, BTreeMap::new());
        let digest = "b".repeat(64);
        let result = host
            .install_repository_file_with(&source, "apt", "sources", &digest, |args| {
                assert_eq!(args[0], "--mode=0644");
                assert_eq!(args[1], "--no-target-directory");
                assert_eq!(args[2], "--");
                assert_eq!(args[3], source.as_os_str());
                assert_eq!(
                    args[4],
                    OsString::from(format!("/etc/apt/sources.list.d/pkgdeck-{digest}.sources"))
                );
                Ok(Completion {
                    code: Some(0),
                    signal: None,
                    stdout: vec![],
                    stderr: vec![],
                    truncated: false,
                    cancellation_deferred: false,
                })
            })
            .unwrap();
        assert_eq!(result.code, Some(0));
        assert!(host
            .install_repository_file_with(&source, "apt", "repo", &digest, |_| panic!(
                "must not run an unsupported write"
            ))
            .is_err());
        assert!(host
            .install_repository_file(
                &source,
                "apt",
                "repo",
                &digest,
                Authorization::Polkit,
                &Cancellation::default(),
            )
            .is_err());
        std::fs::remove_file(source).unwrap();
    }
    #[test]
    fn one_click_hands_the_reviewed_path_to_the_native_ui() {
        let root = std::env::temp_dir().join(format!(
            "pkgdeck-one-click-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("synthetic.ymp");
        let record = root.join("record");
        let executable = root.join("OneClickInstallUI");
        let prepared = root.join("OneClickInstallUI.tmp");
        std::fs::write(&source, b"<metapackage/>").unwrap();
        std::fs::write(
            &prepared,
            format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", record.display()),
        )
        .unwrap();
        std::fs::set_permissions(&prepared, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(prepared, &executable).unwrap();
        let host = Host::new(Runtime::Native, BTreeMap::new());
        let cancel = Cancellation::default();
        let installed = Path::new("/usr/sbin/OneClickInstallUI").exists()
            || Path::new("/usr/bin/OneClickInstallUI").exists();
        let one_click = || host.one_click(&source, &cancel);
        assert!(installed || matches!(one_click(), Err(ExecutionError::Disabled(_))));
        assert!(host
            .one_click_with_candidates(&source, &cancel, &[Path::new("/missing/OneClickInstallUI")])
            .is_err());
        let result = host
            .one_click_with_candidates(&source, &cancel, &[&executable])
            .unwrap();
        assert_eq!(result.code, Some(0));
        assert_eq!(
            std::fs::read_to_string(record).unwrap(),
            source.to_str().unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

impl Host {
    pub fn current() -> Self {
        let mut env = std::env::vars_os().collect();
        let runtime = Runtime::detect(&env, Path::new("/.flatpak-info").exists());
        if runtime == Runtime::Flatpak {
            // Never reinterpret sandbox HOME/XDG/PATH as host configuration.
            env = flatpak_host_environment().unwrap_or_default();
        }
        Self::new(runtime, env)
    }

    pub fn new(runtime: Runtime, source: BTreeMap<OsString, OsString>) -> Self {
        let excluded = ["APPDIR", "SNAP"]
            .into_iter()
            .filter_map(|name| source.get(&OsString::from(name)))
            .map(PathBuf::from)
            .map(|path| fs::canonicalize(&path).unwrap_or(path))
            .collect::<Vec<_>>();
        let mut env = BTreeMap::new();
        // No LD_*, PYTHONPATH, NODE_OPTIONS, APT_CONFIG, shell startup files,
        // or Qt plugin variables are inherited by host tools.
        // The second list carries explicit manager homes/selections only.
        for name in [
            "HOME",
            "USER",
            "LOGNAME",
            "TERM",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XAUTHORITY",
            "DBUS_SESSION_BUS_ADDRESS",
            "XDG_RUNTIME_DIR",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_DATA_DIRS",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
            "SSH_AUTH_SOCK",
            "VIRTUAL_ENV",
            "PIPX_HOME",
            "PIPX_BIN_DIR",
            "UV_TOOL_DIR",
            "MISE_DATA_DIR",
            "MISE_CONFIG_DIR",
            "MISE_GLOBAL_CONFIG_FILE",
            "MISE_INSTALLS_DIR",
            "MISE_STATE_DIR",
            "MISE_CACHE_DIR",
            // Go and .NET: where their tools install and which proxy to use.
            "GOBIN",
            "GOPATH",
            "GOROOT",
            "GOPROXY",
            "DOTNET_ROOT",
            "DOTNET_CLI_HOME",
            // A custom Homebrew location (command ownership) and cache
            // (when cached Homebrew listings expire).
            "HOMEBREW_PREFIX",
            "HOMEBREW_CACHE",
            // pixi and the conda family: their homes, and the manager a
            // `conda init` shell names.
            "PIXI_HOME",
            "CONDA_EXE",
            "MAMBA_EXE",
            "MAMBA_ROOT_PREFIX",
            "CONDARC",
            "CONDA_ENVS_PATH",
            "CONDA_PKGS_DIRS",
            // pnpm refuses global commands when PNPM_HOME is set in the
            // session but missing here; Cargo and Bun read theirs too.
            "PNPM_HOME",
            "CARGO_HOME",
            "RUSTUP_HOME",
            "BUN_INSTALL",
            "COMPOSER_HOME",
            "GEM_HOME",
            "CODEX_HOME",
            "CODEX_INSTALL_DIR",
            "CLAUDE_CONFIG_DIR",
            "GROK_BIN_DIR",
            "AVM_HOME",
            "FOUNDRY_DIR",
            "DISABLE_UPDATES",
            // Oh My Zsh's checkout and its custom plugins and themes.
            "ZSH",
            "ZSH_CUSTOM",
        ] {
            if let Some(value) = source.get(&OsString::from(name)) {
                env.insert(name.into(), value.clone());
            }
        }
        // A first `dotnet` run would otherwise print a welcome banner, send
        // telemetry, create an HTTPS development certificate and edit the
        // user's PATH setup, none of which a tool listing should do.
        for (name, value) in [
            ("DOTNET_CLI_TELEMETRY_OPTOUT", "1"),
            ("DOTNET_NOLOGO", "1"),
            ("DOTNET_GENERATE_ASPNET_CERTIFICATE", "false"),
            ("DOTNET_ADD_GLOBAL_TOOLS_TO_PATH", "false"),
            // Git fails instead of waiting for a password nobody can type.
            ("GIT_TERMINAL_PROMPT", "0"),
        ] {
            env.insert(name.into(), value.into());
        }
        let path = source
            .get(&OsString::from("PATH"))
            .cloned()
            .unwrap_or_else(|| {
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into()
            });
        let dirs: Vec<_> = std::env::split_paths(&path)
            .filter(|p| p.is_absolute() && !excluded.iter().any(|root| p.starts_with(root)))
            .collect();
        env.insert(
            "PATH".into(),
            std::env::join_paths(dirs).expect("PATH entries came from split_paths"),
        );
        env.insert("LC_ALL".into(), "C".into());
        env.insert("LANG".into(), "C".into());
        Self {
            runtime,
            env,
            excluded,
            bridge: "/usr/bin/flatpak-spawn".into(),
        }
    }

    /// Point Flatpak host commands at a fake `flatpak-spawn`.
    #[cfg(test)]
    pub(crate) fn set_bridge_for_tests(&mut self, bridge: &Path) {
        self.bridge = bridge.to_owned();
    }

    /// Read one sanitized environment value (for example `HOME`) without
    /// exposing the whole map. Tool homes derive from this, never from PATH.
    pub fn var(&self, name: &str) -> Option<OsString> {
        self.env.get(&OsString::from(name)).cloned()
    }

    /// Translate a host path for local filesystem inspection inside Flatpak.
    /// Executable arguments always retain their original host paths.
    pub fn filesystem_path(&self, path: &Path) -> PathBuf {
        if self.runtime == Runtime::Flatpak
            && ["/usr", "/etc", "/bin", "/sbin", "/lib", "/lib32", "/lib64"]
                .iter()
                .any(|root| path.starts_with(root))
        {
            Path::new("/run/host").join(path.strip_prefix("/").unwrap())
        } else {
            path.to_owned()
        }
    }

    /// Whether `path` is an executable file on the host.
    fn host_file(&self, path: &Path) -> Result<bool, ExecutionError> {
        if self.runtime != Runtime::Flatpak {
            return Ok(
                path.is_file() && rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_ok()
            );
        }
        // Validate in the host namespace: sandbox symlinks and runtime files
        // cannot prove that a host executable exists or is runnable.
        for flag in ["-f", "-x"] {
            let command =
                self.flatpak_host_command(Path::new("/usr/bin/test"), &[flag.into(), path.into()])?;
            let result = process::run(command, Limits::default(), &Cancellation::default(), false)?;
            match result.code {
                Some(0) => {}
                Some(1) => return Ok(false),
                _ => return Err(ExecutionError::Failed(result)),
            }
        }
        Ok(true)
    }

    /// Resolve on the host, never by running a shell or probing a packaged runtime.
    pub fn resolve(&self, name: &str) -> Result<Option<PathBuf>, ExecutionError> {
        if name.is_empty() || name.contains('/') {
            return Err(ExecutionError::Invalid(
                "expected an executable name".into(),
            ));
        }
        let path = &self.env[&OsString::from("PATH")];
        if self.runtime == Runtime::Flatpak {
            for dir in std::env::split_paths(path) {
                let candidate = dir.join(name);
                // A cheap mapped filesystem check avoids a host round trip
                // for absent directories; final validation is always remote.
                if fs::symlink_metadata(self.filesystem_path(&candidate)).is_ok()
                    && self.host_file(&candidate)?
                {
                    return Ok(Some(candidate));
                }
            }
            return Ok(None);
        }
        Ok(std::env::split_paths(path).find_map(|dir| {
            let candidate = dir.join(name);
            let target = fs::canonicalize(&candidate).ok()?;
            if self.excluded.iter().any(|root| target.starts_with(root)) {
                return None;
            }
            let metadata = fs::metadata(&target).ok()?;
            (metadata.is_file()
                && rustix::fs::access(&candidate, rustix::fs::Access::EXEC_OK).is_ok())
            .then_some(candidate)
        }))
    }

    pub(crate) fn command(
        &self,
        executable: &Path,
        args: &[OsString],
    ) -> Result<Command, ExecutionError> {
        if !executable.is_absolute() {
            return Err(ExecutionError::Invalid(
                "host executable must be absolute".into(),
            ));
        }
        if self.runtime == Runtime::Flatpak {
            return self.flatpak_host_command(executable, args);
        }
        let mut command = Command::new(executable);
        command
            .args(args)
            .env_clear()
            .envs(&self.env)
            .current_dir("/");
        Ok(command)
    }

    /// Unprivileged bounded execution; arguments are never interpreted by a shell.
    pub fn read(
        &self,
        executable: &Path,
        args: &[OsString],
        limits: Limits,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        process::run(self.command(executable, args)?, limits, cancel, false)
    }

    /// Run a program PkgDeck ships, such as the APT helper. Inside the
    /// Flatpak it runs in the sandbox next to PkgDeck, never on the host,
    /// with only the locale and `env`: host variables don't fit the sandbox.
    pub fn read_bundled(
        &self,
        executable: &Path,
        args: &[OsString],
        env: &[(&str, &str)],
        limits: Limits,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        if self.runtime != Runtime::Flatpak {
            let mut command = self.command(executable, args)?;
            command.envs(env.iter().copied());
            return process::run(command, limits, cancel, false);
        }
        let mut command = Command::new(executable);
        command
            .args(args)
            .env_clear()
            .envs(self.env.iter().filter(|(name, _)| {
                matches!(
                    name.to_str(),
                    Some("LANG" | "LANGUAGE" | "LC_ALL" | "LC_MESSAGES")
                )
            }))
            .envs(env.iter().copied())
            .current_dir("/");
        process::run(command, limits, cancel, false)
    }

    /// The fixed macOS programs app removal runs as the invoking user:
    /// `/bin/ps` to see which apps are open and `/usr/bin/trash` to move an
    /// app the user owns to their Trash. Nothing else runs through here.
    pub fn macos_tool(
        &self,
        executable: &Path,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        refuse_root(write)?;
        if !matches!(executable.to_str(), Some("/bin/ps" | "/usr/bin/trash")) {
            return Err(ExecutionError::Invalid(format!(
                "{} is not a macOS tool PkgDeck runs",
                executable.display()
            )));
        }
        process::run(
            self.command(executable, args)?,
            Limits {
                timeout: std::time::Duration::from_secs(60),
                output_bytes: 4 * 1024 * 1024,
            },
            cancel,
            write,
        )
    }

    /// Run an already identified standalone installation as its owner. The
    /// adapter supplies fixed updater arguments, never a user shell command.
    pub(crate) fn standalone_write(
        &self,
        executable: &Path,
        args: &[OsString],
        env: &[(&str, OsString)],
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        refuse_root(true)?;
        let mut host = self.clone();
        host.env.extend(
            env.iter()
                .map(|(key, value)| ((*key).into(), value.clone())),
        );
        let command = host.command(executable, args)?;
        process::run(
            command,
            Limits {
                // AVM builds solana-verify from source during an Anchor
                // update, which takes minutes on slower machines.
                timeout: std::time::Duration::from_secs(900),
                output_bytes: 1024 * 1024,
            },
            cancel,
            true,
        )
    }

    /// Homebrew always runs as the invoking user, with automatic unrelated work disabled.
    /// Update checks call `brew update` themselves. `HOMEBREW_NO_AUTO_UPDATE` stays
    /// set so `info` and `upgrade` do not fetch on their own.
    pub fn brew(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.brew_asking(args, cancel, write, true)
    }
    /// [`Self::brew`], with `ask` saying whether a cask's sudo may show the
    /// password dialog. Automatic updates and the CLI pass false: sudo then
    /// runs only under a saved approval, or asks in the CLI's terminal.
    pub fn brew_asking(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
        ask: bool,
    ) -> Result<Completion, ExecutionError> {
        refuse_root(write)?;
        let path = self
            .resolve("brew")?
            .ok_or_else(|| ExecutionError::Disabled("Homebrew not found".into()))?;
        let mut host = self.clone();
        for name in [
            "HOMEBREW_NO_AUTO_UPDATE",
            "HOMEBREW_NO_INSTALL_CLEANUP",
            "HOMEBREW_NO_ANALYTICS",
            "HOMEBREW_NO_ENV_HINTS",
            "HOMEBREW_NO_COLOR",
        ] {
            host.env.insert(name.into(), "1".into());
        }
        if ask {
            use_askpass(&mut host.env, std::env::current_exe().ok().as_deref());
        }
        let run = || {
            let command = host.command(&path, args)?;
            process::run(
                command,
                Limits {
                    timeout: std::time::Duration::from_secs(120),
                    output_bytes: 32 * 1024 * 1024,
                },
                cancel,
                write,
            )
        };
        // The installed listings take seconds of Homebrew's own start-up, and
        // their answer only changes with what is installed, fetched cask and
        // formula data, taps, or Homebrew itself.
        let text: Vec<&str> = args.iter().filter_map(|arg| arg.to_str()).collect();
        let source = match text.as_slice() {
            ["info", "--json=v2", "--cask", "--installed"] => Some("homebrew-cask"),
            ["info", "--json=v2", "--installed"] => Some("homebrew"),
            _ => None,
        };
        let result = match source.filter(|_| !write && self.runtime.sees_host_files()) {
            Some(source) => crate::cache::completion(
                crate::cache::Store::user().as_ref(),
                source,
                "brew",
                &text,
                &self.brew_watches(&path, self.brew_cache_folder().as_deref()),
                &[],
                run,
            )?,
            None => run()?,
        };
        if result.code == Some(0) {
            Ok(result)
        } else {
            Err(ExecutionError::Failed(result))
        }
    }

    /// Where Homebrew answers may be kept between runs, and everything they
    /// depend on. `None` in a sandbox, without a cache, or without Homebrew.
    /// Homebrew says where its data is, since `brew.env` files can move it.
    pub fn brew_cache(&self) -> Option<(crate::cache::Store, Vec<crate::cache::Watch>)> {
        if !self.runtime.sees_host_files() {
            return None;
        }
        let brew = self.resolve("brew").ok().flatten()?;
        let asked = self
            .brew_asking(&["--cache".into()], &Cancellation::default(), false, false)
            .ok()?;
        let cache = PathBuf::from(String::from_utf8_lossy(&asked.stdout).trim());
        Some((
            crate::cache::Store::user()?,
            self.brew_details_watches(&brew, &cache)?,
        ))
    }

    /// Everything Homebrew package details depend on, with `cache` being
    /// Homebrew's cache folder. `None` when its fetched data isn't there, so
    /// a wrong folder turns keeping off instead of keeping stale details.
    pub(crate) fn brew_details_watches(
        &self,
        brew: &Path,
        cache: &Path,
    ) -> Option<Vec<crate::cache::Watch>> {
        if !cache.is_absolute() || !cache.join("api").is_dir() {
            return None;
        }
        Some(self.brew_watches(brew, Some(cache))).filter(|watches| !watches.is_empty())
    }

    /// Homebrew's cache folder as its default or `HOMEBREW_CACHE` puts it.
    fn brew_cache_folder(&self) -> Option<PathBuf> {
        self.var("HOMEBREW_CACHE")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                let home = self.var("HOME").map(PathBuf::from)?;
                Some(if cfg!(target_os = "macos") {
                    home.join("Library/Caches/Homebrew")
                } else {
                    self.var("XDG_CACHE_HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| home.join(".cache"))
                        .join("Homebrew")
                })
            })
    }

    /// Everything a Homebrew installed-package listing depends on, with
    /// `cache` being Homebrew's cache folder.
    pub(crate) fn brew_watches(
        &self,
        brew: &Path,
        cache: Option<&Path>,
    ) -> Vec<crate::cache::Watch> {
        use crate::cache::Watch;
        let brew = fs::canonicalize(brew).unwrap_or_else(|_| brew.to_owned());
        // bin/brew lives in the Homebrew repository, which is the prefix on
        // Apple Silicon and a Homebrew folder inside it elsewhere.
        let Some(repository) = brew.parent().and_then(Path::parent).map(Path::to_owned) else {
            return vec![];
        };
        let prefix = if repository
            .file_name()
            .is_some_and(|name| name == "Homebrew")
        {
            repository
                .parent()
                .map_or_else(|| repository.clone(), Path::to_owned)
        } else {
            repository.clone()
        };
        let mut watches = vec![
            Watch::tree(prefix.join("Caskroom"), 2),
            Watch::tree(prefix.join("Cellar"), 2),
            // Which installed version is linked.
            Watch::tree(prefix.join("var/homebrew/linked"), 1),
            Watch::tree(repository.join("Library/Taps"), 4),
            Watch::file(repository.join(".git/HEAD")),
        ];
        watches.extend(cache.map(|cache| Watch::tree(cache.join("api"), 2)));
        watches
    }

    /// Official installers put Cargo, rustup, Nix, pixi and the conda family outside
    /// the PATH a desktop app starts with. Only their fixed per-user locations, or the
    /// executable a `conda init` shell names, are tried.
    fn user_install(&self, name: &str) -> Result<Option<PathBuf>, ExecutionError> {
        let path = |key: &str| {
            self.var(key)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let home = path("HOME");
        let mut candidates = vec![];
        match name {
            "pixi" => {
                candidates.extend(path("PIXI_HOME").map(|dir| dir.join("bin/pixi")));
                candidates.extend(home.as_ref().map(|home| home.join(".pixi/bin/pixi")));
            }
            "conda" | "mamba" => {
                candidates.extend(path("CONDA_EXE").map(|exe| exe.with_file_name(name)));
                for dir in ["miniforge3", "miniconda3", "anaconda3", "mambaforge"] {
                    candidates.extend(
                        home.as_ref()
                            .map(|home| home.join(dir).join("bin").join(name)),
                    );
                }
            }
            // `go install` writes to GOBIN, else GOPATH/bin, else ~/go/bin.
            "go" => {
                candidates.extend(path("GOROOT").map(|dir| dir.join("bin/go")));
                candidates.push("/usr/local/go/bin/go".into());
                candidates.push("/opt/homebrew/bin/go".into());
            }
            "dotnet" => {
                candidates.extend(path("DOTNET_ROOT").map(|dir| dir.join("dotnet")));
                candidates.extend(home.as_ref().map(|home| home.join(".dotnet/dotnet")));
                candidates.push("/usr/local/share/dotnet/dotnet".into());
            }
            // rustup puts cargo and its own binary in the same folder.
            "cargo" | "rustup" => {
                candidates.extend(path("CARGO_HOME").map(|dir| dir.join("bin").join(name)));
                candidates.extend(home.as_ref().map(|home| home.join(".cargo/bin").join(name)));
            }
            // Single-user installs link into the profile; multi-user ones
            // into the default profile.
            "nix" => {
                candidates.extend(home.as_ref().map(|home| home.join(".nix-profile/bin/nix")));
                candidates.push("/nix/var/nix/profiles/default/bin/nix".into());
            }
            "micromamba" => {
                candidates.extend(path("MAMBA_EXE"));
                candidates.extend(home.as_ref().map(|home| home.join(".local/bin/micromamba")));
            }
            _ => {}
        }
        for candidate in candidates {
            if candidate.file_name().is_some_and(|file| file == name)
                && self.host_file(&candidate)?
            {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    /// Development tools always run as the invoking user with a bounded
    /// output cap; arguments are never interpreted by a shell. `label` names
    /// the manager for disabled diagnostics.
    pub fn dev_tool(
        &self,
        executable: &str,
        label: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        refuse_root(write)?;
        let path = match self.resolve(executable)? {
            Some(path) => Some(path),
            None => self.user_install(executable)?,
        }
        .ok_or_else(|| ExecutionError::Disabled(format!("{label} not found")))?;
        let command = self.command(&path, args)?;
        let result = process::run(
            command,
            Limits {
                timeout: std::time::Duration::from_secs(300),
                output_bytes: 32 * 1024 * 1024,
            },
            cancel,
            write,
        )?;
        if result.code == Some(0) {
            Ok(result)
        } else {
            Err(ExecutionError::Failed(result))
        }
    }

    /// True when the `docker` executable is emulated by Podman (the
    /// `/usr/bin/docker` wrapper script or a symlink to `podman`). The Docker
    /// backend stays unregistered on such hosts so Podman remains the single
    /// source of truth for the shared image store.
    pub fn docker_is_podman_shim(&self) -> bool {
        let path = match self.resolve("docker").ok().flatten() {
            Some(path) => path,
            None => return false,
        };
        let probe = self.filesystem_path(&path);
        // Symlink directly to the podman binary.
        if fs::read_link(&probe)
            .ok()
            .is_some_and(|target| target.file_name().is_some_and(|name| name == "podman"))
        {
            return true;
        }
        // Same file as podman (hardlink or chained symlinks). Path
        // canonicalization is meaningless across the Flatpak host boundary.
        let canonical = |path: &Path| fs::canonicalize(path).ok();
        if self.runtime != Runtime::Flatpak
            && self.resolve("podman").ok().flatten().is_some_and(|podman| {
                canonical(&podman).is_some_and(|p| canonical(&path) == Some(p))
            })
        {
            return true;
        }
        // Wrapper script that execs podman; ELF binaries are never parsed.
        let script = fs::metadata(&probe)
            .ok()
            .filter(|meta| meta.len() < 65536)
            .and_then(|_| fs::read(&probe).ok());
        script.is_some_and(|bytes| {
            let text = String::from_utf8_lossy(&bytes);
            text.starts_with("#!")
                && text.lines().any(|line| {
                    let line = line.trim();
                    line.starts_with("exec ")
                        && line
                            .split_whitespace()
                            .any(|word| word.trim_matches('"').rsplit('/').next() == Some("podman"))
                })
        })
    }

    /// Run Docker or Podman as the invoking user. Reads use a short deadline so
    /// a stopped daemon cannot stall every package view; pulls and removals keep
    /// the normal deferred-cancellation write contract.
    pub fn container_engine(
        &self,
        executable: &str,
        label: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        refuse_root(write)?;
        let path = self
            .resolve(executable)?
            .ok_or_else(|| ExecutionError::Disabled(format!("{label} not found")))?;
        let command = self.command(&path, args)?;
        let result = process::run(
            command,
            Limits {
                timeout: if write {
                    std::time::Duration::from_secs(300)
                } else {
                    std::time::Duration::from_secs(8)
                },
                output_bytes: 32 * 1024 * 1024,
            },
            cancel,
            write,
        )?;
        if result.code == Some(0) {
            Ok(result)
        } else {
            Err(ExecutionError::Failed(result))
        }
    }

    /// pip inside an explicitly selected virtual environment. `venv` must be
    /// the environment root; `<venv>/bin/python -m pip` is the only entry
    /// point so system pip is never touched. User-scoped like `dev_tool`.
    /// The interpreter symlink is deliberately never resolved: venv discovery
    /// uses `argv[0]`'s directory, and resolving it would escape into the
    /// base interpreter's site packages. A `pyvenv.cfg` marker is required.
    pub fn venv_pip(
        &self,
        venv: &Path,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        refuse_root(write)?;
        if !venv.is_absolute() {
            return Err(ExecutionError::Invalid(
                "virtual environment path must be absolute".into(),
            ));
        }
        let python = venv.join("bin/python");
        let meta = fs::symlink_metadata(self.filesystem_path(&python)).map_err(|_| {
            ExecutionError::Disabled("pip virtual environment not found".to_string())
        })?;
        if !meta.file_type().is_symlink() && !meta.is_file() {
            return Err(ExecutionError::Disabled(
                "pip virtual environment not found".to_string(),
            ));
        }
        if fs::metadata(self.filesystem_path(&venv.join("pyvenv.cfg")))
            .map(|marker| !marker.is_file())
            .unwrap_or(true)
        {
            return Err(ExecutionError::Disabled(
                "pip virtual environment not found".to_string(),
            ));
        }
        let canonical_venv =
            fs::canonicalize(self.filesystem_path(venv)).unwrap_or_else(|_| venv.to_owned());
        if self.excluded.iter().any(|root| {
            canonical_venv.starts_with(root) || venv.starts_with(root) || python.starts_with(root)
        }) {
            return Err(ExecutionError::Disabled(
                "pip virtual environment not found".to_string(),
            ));
        }
        if !self.host_file(&python)? {
            return Err(ExecutionError::Disabled(
                "pip virtual environment not found".to_string(),
            ));
        }
        let mut full = vec![OsString::from("-m"), OsString::from("pip")];
        full.extend(args.iter().cloned());
        let command = self.command(&python, &full)?;
        let result = process::run(
            command,
            Limits {
                timeout: std::time::Duration::from_secs(300),
                output_bytes: 32 * 1024 * 1024,
            },
            cancel,
            write,
        )?;
        if result.code == Some(0) {
            Ok(result)
        } else {
            Err(ExecutionError::Failed(result))
        }
    }

    /// APT-only authorization boundary.
    /// User-scoped tools never enter this path, and the frontend itself must not be root.
    pub fn apt(
        &self,
        action: AptAction,
        authorization: Authorization,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        refuse_root(true)?;
        let result = self.privileged(
            Path::new("/usr/bin/apt-get"),
            &action.arguments()?,
            authorization,
            cancel,
        )?;
        classify_apt(result)
    }

    /// Execute one validated APT transaction with several exact targets.
    pub fn apt_group(
        &self,
        actions: &[AptAction],
        authorization: Authorization,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        refuse_root(true)?;
        classify_apt(self.privileged(
            Path::new("/usr/bin/apt-get"),
            &AptAction::group_arguments(actions)?,
            authorization,
            cancel,
        )?)
    }

    /// Runs Flatpak from the invoking user's sanitized environment. System writes
    /// use the same non-interactive authorization boundary as APT.
    pub fn flatpak(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
        system: bool,
        authorization: Authorization,
    ) -> Result<Completion, ExecutionError> {
        refuse_root(write)?;
        let path = if write && system {
            if self.runtime == Runtime::Flatpak {
                let path = Path::new("/usr/bin/flatpak");
                if !self.host_file(path)? {
                    return Err(ExecutionError::Disabled("system Flatpak not found".into()));
                }
                path.to_owned()
            } else {
                system_flatpak_path(Path::new("/usr/bin/flatpak"))?
            }
        } else {
            self.resolve("flatpak")?
                .ok_or_else(|| ExecutionError::Disabled("Flatpak not found".into()))?
        };
        if write && system {
            return self.privileged(&path, args, authorization, cancel);
        }
        let command = self.command(&path, args)?;
        let result = process::run(command, FLATPAK_QUERY_LIMITS, cancel, write)?;
        if result.code == Some(0) {
            Ok(result)
        } else {
            Err(ExecutionError::Failed(result))
        }
    }
}

// Remote catalog queries can download metadata on their first call. Keep
// them cancellable, but allow more time and output than local reads.
const FLATPAK_QUERY_LIMITS: Limits = Limits {
    timeout: std::time::Duration::from_secs(120),
    output_bytes: 32 * 1024 * 1024,
};

#[cfg(test)]
mod flatpak_limit_tests {
    use super::*;

    #[test]
    fn flatpak_queries_allow_more_time_and_output_than_plain_reads() {
        let plain = Limits::default();
        assert!(FLATPAK_QUERY_LIMITS.timeout > plain.timeout);
        assert!(FLATPAK_QUERY_LIMITS.output_bytes > plain.output_bytes);
    }
}

impl Host {
    pub fn system_flatpak_available(&self) -> bool {
        cfg!(target_os = "linux")
            && self
                .host_file(Path::new("/usr/bin/flatpak"))
                .unwrap_or(false)
    }

    fn flatpak_host_command(
        &self,
        executable: &Path,
        args: &[OsString],
    ) -> Result<Command, ExecutionError> {
        self.flatpak_host_command_with_bridge(&self.bridge, executable, args)
    }
    fn flatpak_host_command_with_bridge(
        &self,
        bridge: &Path,
        executable: &Path,
        args: &[OsString],
    ) -> Result<Command, ExecutionError> {
        if !executable.is_absolute() {
            return Err(ExecutionError::Invalid(
                "host executable must be absolute".into(),
            ));
        }
        if !bridge.is_file() {
            return Err(ExecutionError::Disabled(
                "Flatpak host bridge is unavailable".into(),
            ));
        }
        let mut command = Command::new(bridge);
        command.args(["--host", "--clear-env", "--directory=/"]);
        for (name, value) in &self.env {
            let mut assignment = OsString::from("--env=");
            assignment.push(name);
            assignment.push("=");
            assignment.push(value);
            command.arg(assignment);
        }
        command.arg(executable).args(args);
        Ok(command)
    }

    fn source_editor_path(
        &self,
        authorization: Authorization,
    ) -> Result<Option<PathBuf>, ExecutionError> {
        let authenticator = match authorization {
            Authorization::Polkit => Path::new("/usr/bin/pkexec"),
            Authorization::SudoNonInteractive => Path::new("/usr/bin/sudo"),
        };
        self.source_editor_path_with(
            authenticator,
            Path::new("/usr/bin/env"),
            &[
                Path::new("/usr/bin/software-properties-qt"),
                Path::new("/usr/bin/software-properties-gtk"),
            ],
        )
    }

    fn source_editor_path_with(
        &self,
        authenticator: &Path,
        env: &Path,
        editors: &[&Path],
    ) -> Result<Option<PathBuf>, ExecutionError> {
        if !cfg!(target_os = "linux")
            || self.resolve("apt-get")?.is_none()
            || !self.host_file(authenticator)?
            || !self.host_file(env)?
            || self.var("DISPLAY").is_none()
        {
            return Ok(None);
        }
        for path in editors {
            if self.host_file(path)? {
                return Ok(Some((*path).to_owned()));
            }
        }
        Ok(None)
    }

    pub fn source_editor_available(&self, authorization: Authorization) -> bool {
        self.source_editor_path(authorization)
            .ok()
            .flatten()
            .is_some()
    }

    fn source_editor_command(
        &self,
        authorization: Authorization,
        editor: &Path,
    ) -> Result<Command, ExecutionError> {
        let authenticator = match authorization {
            Authorization::Polkit => Path::new("/usr/bin/pkexec"),
            Authorization::SudoNonInteractive => Path::new("/usr/bin/sudo"),
        };
        let mut args = match authorization {
            Authorization::Polkit => vec!["--disable-internal-agent".into()],
            Authorization::SudoNonInteractive => vec!["-n".into(), "--".into()],
        };
        args.push(Path::new("/usr/bin/env").as_os_str().to_owned());
        for name in ["DISPLAY", "XAUTHORITY"] {
            if let Some(value) = self.var(name) {
                let mut assignment = OsString::from(format!("{name}="));
                assignment.push(value);
                args.push(assignment);
            }
        }
        args.push(editor.as_os_str().to_owned());
        self.command(authenticator, &args)
    }

    fn run_source_editor(mut command: Command) -> Result<(), ExecutionError> {
        let status = command
            .status()
            .map_err(|error| ExecutionError::Io(error.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(ExecutionError::Io(format!(
                "Software Sources exited with {status}"
            )))
        }
    }

    /// Run the distro editor with the authentication its desktop entry expects.
    pub fn open_source_editor(&self, authorization: Authorization) -> Result<(), ExecutionError> {
        let path = self.source_editor_path(authorization)?.ok_or_else(|| {
            ExecutionError::Disabled("APT Software Sources editor is unavailable".into())
        })?;
        Self::run_source_editor(self.source_editor_command(authorization, &path)?)
    }

    /// Install one reviewed repository definition under a content-addressed
    /// filename. The caller validates both bytes and kind before this boundary.
    pub(crate) fn install_repository_file(
        &self,
        source: &Path,
        backend: &str,
        suffix: &str,
        digest: &str,
        authorization: Authorization,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        self.install_repository_file_with(source, backend, suffix, digest, |args| {
            self.privileged(Path::new("/usr/bin/install"), args, authorization, cancel)
        })
    }

    fn install_repository_file_with(
        &self,
        source: &Path,
        backend: &str,
        suffix: &str,
        digest: &str,
        execute: impl FnOnce(&[OsString]) -> Result<Completion, ExecutionError>,
    ) -> Result<Completion, ExecutionError> {
        refuse_root(true)?;
        let args = repository_install_args(source, backend, suffix, digest)?;
        execute(&args)
    }

    /// The distro's One Click installer owns its own preview and authorization.
    pub(crate) fn one_click(
        &self,
        source: &Path,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        self.one_click_with_candidates(
            source,
            cancel,
            &[
                Path::new("/usr/sbin/OneClickInstallUI"),
                Path::new("/usr/bin/OneClickInstallUI"),
            ],
        )
    }

    fn one_click_with_candidates(
        &self,
        source: &Path,
        cancel: &Cancellation,
        candidates: &[&Path],
    ) -> Result<Completion, ExecutionError> {
        let executable = candidates
            .iter()
            .map(|path| path.to_path_buf())
            .find(|path| self.host_file(path).unwrap_or(false))
            .ok_or_else(|| {
                ExecutionError::Disabled("openSUSE One Click installer is unavailable".into())
            })?;
        process::run(
            self.command(&executable, &[source.as_os_str().to_os_string()])?,
            Limits::default(),
            cancel,
            true,
        )
    }

    /// Runs a distro package manager. Writes always use a fixed system path before
    /// entering the authorization boundary, never an executable found in PATH.
    pub fn system_manager(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
        authorization: Authorization,
    ) -> Result<Completion, ExecutionError> {
        refuse_root(write)?;
        let path = if write {
            // Privileged writes only ever run the manager from its fixed
            // system location, never whatever PATH finds first.
            let dirs: &[&str] = match executable {
                "apk" => &["/sbin", "/usr/sbin"],
                "port" => &["/opt/local/bin"],
                "softwareupdate" => &["/usr/sbin"],
                // Moves a system-owned app to the Trash (see mac_apps.rs).
                "mv" => &["/bin"],
                _ => &["/usr/bin"],
            };
            let mut found = None;
            for dir in dirs {
                let path = Path::new(dir).join(executable);
                if self.host_file(&path)? {
                    found = Some(path);
                    break;
                }
            }
            found.ok_or_else(|| ExecutionError::Disabled(format!("{executable} not found")))?
        } else {
            match self.resolve(executable)? {
                Some(path) => path,
                // MacPorts' folder is rarely on a shell's PATH.
                None if executable == "port"
                    && self.host_file(Path::new("/opt/local/bin/port"))? =>
                {
                    PathBuf::from("/opt/local/bin/port")
                }
                None if executable == "softwareupdate"
                    && self.host_file(Path::new("/usr/sbin/softwareupdate"))? =>
                {
                    PathBuf::from("/usr/sbin/softwareupdate")
                }
                None => return Err(ExecutionError::Disabled(format!("{executable} not found"))),
            }
        };
        if write {
            self.privileged(&path, args, authorization, cancel)
        } else {
            let timeout = if executable == "softwareupdate" {
                // Its listing asks Apple's servers and can take minutes.
                std::time::Duration::from_secs(300)
            } else if matches!(executable, "snap" | "fwupdmgr") {
                std::time::Duration::from_secs(60)
            } else {
                Limits::default().timeout
            };
            let command = self.command(&path, args)?;
            let result = process::run(
                command,
                Limits {
                    timeout,
                    ..Limits::default()
                },
                cancel,
                false,
            )?;
            if result.code == Some(0) {
                Ok(result)
            } else {
                Err(ExecutionError::Failed(result))
            }
        }
    }

    fn privileged(
        &self,
        executable: &Path,
        args: &[OsString],
        authorization: Authorization,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        if let Some(result) = crate::batch::run_in_scope(executable, args, cancel) {
            return result;
        }
        // macOS has no polkit; its system prompt is the administrator
        // password dialog.
        #[cfg(target_os = "macos")]
        if self.runtime == Runtime::Native && matches!(authorization, Authorization::Polkit) {
            return self.macos_administrator(executable, args, cancel);
        }
        let (program, mut prefixed) = authorization.prefix(executable);
        prefixed.extend(args.iter().cloned());
        let mut host = self.clone();
        host.env
            .insert("PATH".into(), "/usr/sbin:/usr/bin:/sbin:/bin".into());
        let command = host.command(Path::new(program), &prefixed)?;
        process::run(command, Limits::default(), cancel, true)
    }
}

#[cfg(target_os = "macos")]
impl Host {
    /// Run one command as root after macOS's own administrator password
    /// dialog. Every argument is shell-quoted and then escaped for AppleScript,
    /// so no argument can change the command.
    fn macos_administrator(
        &self,
        executable: &Path,
        args: &[OsString],
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        let script = administrator_script(executable, args)?;
        let mut host = self.clone();
        host.env
            .insert("PATH".into(), "/usr/sbin:/usr/bin:/sbin:/bin".into());
        let osascript = Path::new("/usr/bin/osascript");
        let command = host.command(osascript, &["-e".into(), script.into()])?;
        administrator_result(process::run(command, Limits::default(), cancel, true)?)
    }
}

/// AppleScript error -128 is the password dialog's Cancel button.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn administrator_result(result: Completion) -> Result<Completion, ExecutionError> {
    if result.code != Some(0) && String::from_utf8_lossy(&result.stderr).contains("(-128)") {
        return Err(ExecutionError::AuthorizationCancelled);
    }
    Ok(result)
}

/// One shell command line with every word single-quoted.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn shell_command(
    executable: &Path,
    args: &[OsString],
) -> Result<String, ExecutionError> {
    let quote = |arg: &std::ffi::OsStr| -> Result<String, ExecutionError> {
        let text = arg
            .to_str()
            .filter(|text| !text.contains('\0'))
            .ok_or_else(|| {
                ExecutionError::Invalid("administrator command arguments must be text".into())
            })?;
        Ok(format!("'{}'", text.replace('\'', "'\\''")))
    };
    Ok(std::iter::once(executable.as_os_str())
        .chain(args.iter().map(OsString::as_os_str))
        .map(quote)
        .collect::<Result<Vec<_>, _>>()?
        .join(" "))
}

/// An AppleScript string literal.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn applescript_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `do shell script "'/opt/local/bin/port' '-N' 'install' 'xz'" …`
#[cfg(any(target_os = "macos", test))]
pub(crate) fn administrator_script(
    executable: &Path,
    args: &[OsString],
) -> Result<String, ExecutionError> {
    Ok(format!(
        "do shell script {} with administrator privileges without altering line endings",
        applescript_string(&shell_command(executable, args)?)
    ))
}

#[cfg(test)]
mod brew_cache_tests {
    use super::*;

    /// A cached Homebrew listing is dropped when an install, a tap or
    /// fetched Homebrew data changes, on both Homebrew layouts.
    #[test]
    fn brew_listings_expire_when_homebrew_changes() {
        for nested in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "pkgdeck-brew-cache-{}-{nested}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            let repository = if nested {
                root.join("Homebrew")
            } else {
                root.clone()
            };
            fs::create_dir_all(repository.join("bin")).unwrap();
            fs::write(repository.join("bin/brew"), "").unwrap();
            fs::create_dir_all(root.join("Caskroom")).unwrap();
            fs::create_dir_all(repository.join("Library/Taps/me/homebrew-tools/Casks")).unwrap();
            let env = [
                ("HOME", root.join("home")),
                ("HOMEBREW_CACHE", root.join("cache")),
                ("PATH", "/usr/bin:/bin".into()),
            ]
            .into_iter()
            .map(|(key, value)| (OsString::from(key), value.into_os_string()))
            .collect();
            let host = Host::new(Runtime::Native, env);
            let watches = host.brew_watches(
                &repository.join("bin/brew"),
                host.brew_cache_folder().as_deref(),
            );
            let now = || crate::cache::fingerprint(&watches, &[]).unwrap();
            let before = now();
            assert_eq!(before, now(), "nothing changed");
            fs::create_dir_all(root.join("Caskroom/firefox/1.0")).unwrap();
            let installed = now();
            assert_ne!(before, installed, "an installed cask");
            fs::write(
                repository.join("Library/Taps/me/homebrew-tools/Casks/tool.rb"),
                "cask",
            )
            .unwrap();
            let tapped = now();
            assert_ne!(installed, tapped, "a tap's cask");
            fs::create_dir_all(root.join("cache/api")).unwrap();
            fs::write(root.join("cache/api/cask.jws.json"), "{}").unwrap();
            assert_ne!(tapped, now(), "fetched cask data");
            fs::remove_dir_all(&root).unwrap();
        }
    }
}

#[cfg(test)]
mod administrator_tests {
    use super::*;

    /// The administrator command survives quotes, spaces and backslashes:
    /// the real shell gets back exactly the arguments, and on macOS the real
    /// AppleScript parser gets back exactly the shell line.
    #[test]
    fn administrator_commands_quote_every_argument() {
        let args: Vec<OsString> = [
            "%s|",
            "it's",
            "\"quoted\" \\ back",
            "$(touch /tmp/x)",
            "a b",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let line = shell_command(Path::new("/usr/bin/printf"), &args).unwrap();
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&line)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "it's|\"quoted\" \\ back|$(touch /tmp/x)|a b|"
        );
        assert!(shell_command(Path::new("/bin/echo"), &["a\0b".into()]).is_err());
        let script = administrator_script(Path::new("/usr/bin/printf"), &args).unwrap();
        assert!(
            script.starts_with(r#"do shell script "'/usr/bin/printf' '%s|' 'it'\\''s'"#),
            "{script}"
        );
        assert!(script.ends_with("with administrator privileges without altering line endings"));
        if cfg!(target_os = "macos") {
            let out = std::process::Command::new("/usr/bin/osascript")
                .arg("-e")
                .arg(format!("return {}", applescript_string(&line)))
                .output()
                .unwrap();
            assert_eq!(
                String::from_utf8_lossy(&out.stdout).trim_end_matches('\n'),
                line
            );
        }
    }
}

fn flatpak_host_environment() -> Option<BTreeMap<OsString, OsString>> {
    flatpak_host_environment_from(Path::new("/usr/bin/flatpak-spawn"))
}

fn system_flatpak_path(path: &Path) -> Result<PathBuf, ExecutionError> {
    if !path.is_file() {
        return Err(ExecutionError::Disabled("system Flatpak not found".into()));
    }
    Ok(path.to_owned())
}

fn flatpak_host_environment_from(bridge: &Path) -> Option<BTreeMap<OsString, OsString>> {
    let mut command = Command::new(bridge);
    command.args(["--host", "/usr/bin/env", "-0"]);
    let output = process::run(command, Limits::default(), &Cancellation::default(), false).ok()?;
    if output.code != Some(0) || output.truncated {
        return None;
    }
    parse_flatpak_host_environment(&output.stdout)
}

fn parse_flatpak_host_environment(output: &[u8]) -> Option<BTreeMap<OsString, OsString>> {
    let mut env = BTreeMap::new();
    for entry in output.split(|byte| *byte == 0) {
        if entry.is_empty() {
            continue;
        }
        let split = entry.iter().position(|byte| *byte == b'=')?;
        use std::os::unix::ffi::OsStringExt;
        env.insert(
            OsString::from_vec(entry[..split].to_vec()),
            OsString::from_vec(entry[split + 1..].to_vec()),
        );
    }
    Some(env)
}

#[cfg(test)]
mod flatpak_bridge_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn flatpak_paths_and_general_commands_use_the_host_namespace() {
        let mut host = Host::new(
            Runtime::Flatpak,
            [
                ("PATH".into(), "/usr/bin".into()),
                ("HOME".into(), "/home/fixture".into()),
            ]
            .into(),
        );
        host.bridge = "/usr/bin/true".into();
        for name in ["apt-get", "brew", "podman", "python3"] {
            let path = Path::new("/usr/bin").join(name);
            let command = host
                .command(&path, &["literal;$(no-shell)".into()])
                .unwrap();
            assert_eq!(command.get_program(), "/usr/bin/true");
            let args: Vec<_> = command.get_args().collect();
            assert_eq!(args[args.len() - 2], path.as_os_str());
            assert_eq!(
                args.last().unwrap(),
                &OsString::from("literal;$(no-shell)").as_os_str()
            );
        }
        assert_eq!(
            host.filesystem_path(Path::new("/etc/apt")),
            Path::new("/run/host/etc/apt")
        );
        assert_eq!(
            host.filesystem_path(Path::new("/usr/share/icons")),
            Path::new("/run/host/usr/share/icons")
        );
        assert_eq!(
            host.filesystem_path(Path::new("/home/fixture/tool")),
            Path::new("/home/fixture/tool")
        );
        assert_eq!(
            Host::new(Runtime::Native, BTreeMap::new()).filesystem_path(Path::new("/etc/apt")),
            Path::new("/etc/apt")
        );
        assert!(host
            .host_file(Path::new("/synthetic-host-executable"))
            .unwrap());
        host.bridge = "/usr/bin/false".into();
        assert!(!host
            .host_file(Path::new("/synthetic-host-executable"))
            .unwrap());
    }

    #[test]
    fn authorization_environment_is_forwarded_to_host_not_bridge() {
        let mut host = Host::new(
            Runtime::Flatpak,
            [("PATH".into(), "/home/fixture/bin".into())].into(),
        );
        host.bridge = "/bin/echo".into();
        let result = host
            .privileged(
                Path::new("/usr/bin/apt-get"),
                &["--simulate".into()],
                Authorization::SudoNonInteractive,
                &Cancellation::default(),
            )
            .unwrap();
        let output = String::from_utf8(result.stdout).unwrap();
        assert!(output.contains("--env=PATH=/usr/sbin:/usr/bin:/sbin:/bin"));
        assert!(output.contains("/usr/bin/sudo -n -- /usr/bin/apt-get --simulate"));
        assert!(!output.contains("/home/fixture/bin"));
        // Homebrew policy must reach the host process, not merely the
        // flatpak-spawn wrapper's environment.
        let directory =
            std::env::temp_dir().join(format!("pkgdeck-bridge-brew-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::os::unix::fs::symlink("/usr/bin/true", directory.join("brew")).unwrap();
        host.env.insert("PATH".into(), directory.as_os_str().into());
        let result = host
            .brew(&["--version".into()], &Cancellation::default(), false)
            .unwrap();
        assert!(host.resolve("pkgdeck-synthetic-missing").unwrap().is_none());
        std::fs::create_dir(directory.join("bin")).unwrap();
        std::fs::write(directory.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        std::os::unix::fs::symlink("/usr/bin/python3", directory.join("bin/python")).unwrap();
        let pip = host
            .venv_pip(
                &directory,
                &["list".into()],
                &Cancellation::default(),
                false,
            )
            .unwrap();
        assert!(String::from_utf8(pip.stdout)
            .unwrap()
            .contains("/bin/python -m pip list"));
        std::os::unix::fs::symlink("/usr/bin/true", directory.join("flatpak")).unwrap();
        let flatpak = host
            .flatpak(
                &["--user".into(), "list".into()],
                &Cancellation::default(),
                false,
                false,
                Authorization::Polkit,
            )
            .unwrap();
        assert!(String::from_utf8(flatpak.stdout)
            .unwrap()
            .contains("/flatpak --user list"));
        std::fs::remove_dir_all(directory).unwrap();
        let output = String::from_utf8(result.stdout).unwrap();
        assert!(output.contains("--env=HOMEBREW_NO_AUTO_UPDATE=1"));
        assert!(output.contains("--env=HOMEBREW_NO_INSTALL_CLEANUP=1"));
        assert!(output.contains("--env=HOMEBREW_NO_ANALYTICS=1"));
    }

    #[test]
    fn system_writes_use_fixed_host_paths_and_fail_closed_when_unavailable() {
        let mut host = Host::new(
            Runtime::Flatpak,
            [("PATH".into(), "/home/fixture/untrusted-bin".into())].into(),
        );
        host.bridge = "/bin/echo".into();
        let cancel = Cancellation::default();
        let flatpak = host
            .flatpak(
                &["--system".into(), "update".into()],
                &cancel,
                true,
                true,
                Authorization::SudoNonInteractive,
            )
            .unwrap();
        let output = String::from_utf8(flatpak.stdout).unwrap();
        assert!(output.contains("/usr/bin/sudo -n -- /usr/bin/flatpak --system update"));
        assert!(!output.contains("/home/fixture/untrusted-bin/flatpak"));

        let manager = host
            .system_manager(
                "dnf",
                &["install".into(), "synthetic-package".into()],
                &cancel,
                true,
                Authorization::Polkit,
            )
            .unwrap();
        let output = String::from_utf8(manager.stdout).unwrap();
        assert!(output.contains("/usr/bin/dnf install synthetic-package"));
        assert!(!output.contains("/home/fixture/untrusted-bin/dnf"));
        // Removing a system-owned app moves it with the system's own mv.
        let moved = host
            .system_manager(
                "mv",
                &["-n".into(), "a".into(), "b".into()],
                &cancel,
                true,
                Authorization::SudoNonInteractive,
            )
            .unwrap();
        assert!(String::from_utf8(moved.stdout)
            .unwrap()
            .contains("/usr/bin/sudo -n -- /bin/mv -n a b"));
        // App removal runs only its two fixed programs as the user.
        let ps = host
            .macos_tool(Path::new("/bin/ps"), &["-ax".into()], &cancel, false)
            .unwrap();
        assert!(String::from_utf8(ps.stdout)
            .unwrap()
            .contains("/bin/ps -ax"));
        assert!(matches!(
            host.macos_tool(Path::new("/bin/rm"), &[], &cancel, true),
            Err(ExecutionError::Invalid(reason)) if reason.contains("/bin/rm")
        ));

        host.bridge = "/usr/bin/false".into();
        assert!(matches!(
            host.flatpak(&[], &cancel, true, true, Authorization::Polkit),
            Err(ExecutionError::Disabled(reason)) if reason == "system Flatpak not found"
        ));
        assert!(matches!(
            host.system_manager("dnf", &[], &cancel, true, Authorization::Polkit),
            Err(ExecutionError::Disabled(reason)) if reason == "dnf not found"
        ));
    }

    #[test]
    fn source_editor_requires_an_apt_desktop_session() {
        let host = Host::new(Runtime::Native, BTreeMap::new());
        assert!(!host.source_editor_available(Authorization::Polkit));
        assert!(matches!(
            host.open_source_editor(Authorization::Polkit),
            Err(ExecutionError::Disabled(reason)) if reason.contains("APT Software Sources")
        ));
    }

    #[test]
    fn source_editor_reports_the_child_result() {
        assert!(Host::run_source_editor(Command::new("/usr/bin/true")).is_ok());
        let mut rejected = Command::new("/bin/sh");
        rejected.args(["-c", "exit 7"]);
        assert!(matches!(
            Host::run_source_editor(rejected),
            Err(ExecutionError::Io(message)) if message.contains("exit status: 7")
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn source_editor_requires_real_helpers_and_chooses_an_available_editor() {
        let directory = std::env::temp_dir().join(format!(
            "pkgdeck-editor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let executable = |name: &str| {
            let path = directory.join(name);
            std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        executable("apt-get");
        let authenticator = executable("pkexec");
        let env = executable("env");
        let first = directory.join("software-properties-qt");
        let second = executable("software-properties-gtk");
        let host = Host::new(
            Runtime::Native,
            [
                ("PATH".into(), directory.as_os_str().to_owned()),
                ("DISPLAY".into(), ":91".into()),
            ]
            .into(),
        );
        assert_eq!(
            host.source_editor_path_with(&authenticator, &env, &[&first, &second])
                .unwrap(),
            Some(second)
        );
        let first = executable("software-properties-qt");
        assert_eq!(
            host.source_editor_path_with(
                &authenticator,
                &env,
                &[&first, &directory.join("missing")]
            )
            .unwrap(),
            Some(first.clone())
        );
        std::fs::remove_file(&authenticator).unwrap();
        assert!(host
            .source_editor_path_with(&authenticator, &env, &[&first])
            .unwrap()
            .is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn system_flatpak_write_availability_uses_the_host_namespace() {
        let mut host = Host::new(Runtime::Flatpak, BTreeMap::new());
        host.bridge = "/usr/bin/true".into();
        assert_eq!(host.system_flatpak_available(), cfg!(target_os = "linux"));
        host.bridge = "/usr/bin/false".into();
        assert!(!host.system_flatpak_available());
    }

    #[test]
    fn source_editor_pins_privileged_helpers_despite_untrusted_path() {
        let host = Host::new(
            Runtime::Native,
            [
                ("PATH".into(), "/tmp/untrusted-bin:/usr/bin".into()),
                ("DISPLAY".into(), ":91".into()),
                ("XAUTHORITY".into(), "/tmp/synthetic-auth".into()),
            ]
            .into(),
        );
        for (authorization, expected_program, prefix) in [
            (
                Authorization::Polkit,
                "/usr/bin/pkexec",
                "--disable-internal-agent",
            ),
            (Authorization::SudoNonInteractive, "/usr/bin/sudo", "-n"),
        ] {
            let command = host
                .source_editor_command(authorization, Path::new("/tmp/synthetic-editor"))
                .unwrap();
            assert_eq!(command.get_program(), expected_program);
            let args = command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            assert_eq!(args[0], prefix);
            assert!(args.contains(&"/usr/bin/env".into()));
            assert!(args.contains(&"DISPLAY=:91".into()));
            assert!(args.contains(&"XAUTHORITY=/tmp/synthetic-auth".into()));
            assert_eq!(args.last().unwrap(), "/tmp/synthetic-editor");
            assert!(!args.iter().any(|arg| arg.contains("/tmp/untrusted-bin")));
        }
    }

    #[test]
    fn bridge_commands_pin_the_host_program_and_forward_only_sanitized_values() {
        let host = Host::new(
            Runtime::Flatpak,
            [
                ("HOME".into(), "/home/fixture".into()),
                ("PATH".into(), "/usr/bin".into()),
                (
                    "XDG_CONFIG_HOME".into(),
                    "/home/fixture/.config-custom".into(),
                ),
                (
                    "XDG_DATA_HOME".into(),
                    "/home/fixture/.local/share-custom".into(),
                ),
                ("XDG_DATA_DIRS".into(), "/opt/apps/share:/usr/share".into()),
                ("XDG_CACHE_HOME".into(), "/home/fixture/.cache-alt".into()),
                ("XDG_RUNTIME_DIR".into(), "/run/user/1000".into()),
            ]
            .into(),
        );
        let command = host
            .flatpak_host_command_with_bridge(
                Path::new("/usr/bin/true"),
                Path::new("/usr/bin/flatpak"),
                &["--user".into(), "list".into()],
            )
            .unwrap();
        assert_eq!(command.get_program(), "/usr/bin/true");
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.starts_with(&[
            "--host".into(),
            "--clear-env".into(),
            "--directory=/".into()
        ]));
        assert!(args.contains(&"--env=XDG_RUNTIME_DIR=/run/user/1000".into()));
        assert!(args.contains(&"--env=HOME=/home/fixture".into()));
        assert!(args
            .iter()
            .any(|arg| arg.starts_with("--env=XDG_CACHE_HOME=")));
        assert!(args
            .iter()
            .any(|arg| arg.starts_with("--env=XDG_CONFIG_HOME=")));
        assert!(args
            .iter()
            .any(|arg| arg.starts_with("--env=XDG_DATA_HOME=")));
        assert!(args.contains(&"--env=XDG_DATA_DIRS=/opt/apps/share:/usr/share".into()));
        assert!(args.ends_with(&["/usr/bin/flatpak".into(), "--user".into(), "list".into()]));
        assert!(matches!(
            host.flatpak_host_command_with_bridge(
                Path::new("/usr/bin/true"),
                Path::new("flatpak"),
                &[]
            ),
            Err(ExecutionError::Invalid(_))
        ));
        assert!(matches!(
            host.flatpak_host_command(Path::new("flatpak"), &[]),
            Err(ExecutionError::Invalid(_))
        ));
        assert!(matches!(
            host.flatpak_host_command_with_bridge(
                Path::new("/missing-flatpak-spawn"),
                Path::new("/usr/bin/flatpak"),
                &[]
            ),
            Err(ExecutionError::Disabled(_))
        ));
        assert_eq!(
            system_flatpak_path(Path::new("/usr/bin/true")).unwrap(),
            Path::new("/usr/bin/true")
        );
        assert!(matches!(
            system_flatpak_path(Path::new("/missing-system-flatpak")),
            Err(ExecutionError::Disabled(_))
        ));
    }

    #[test]
    fn host_environment_probe_parses_nul_records_and_rejects_failures() {
        let env = parse_flatpak_host_environment(b"HOME=/home/fixture\0PATH=/usr/bin\0").unwrap();
        assert_eq!(
            env.get(&OsString::from("HOME")),
            Some(&"/home/fixture".into())
        );
        assert_eq!(env.get(&OsString::from("PATH")), Some(&"/usr/bin".into()));
        assert!(parse_flatpak_host_environment(b"malformed").is_none());

        let path =
            std::env::temp_dir().join(format!("pkgdeck-flatpak-spawn-{}", std::process::id()));
        fs::write(&path, b"#!/bin/sh\nexit 7\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(flatpak_host_environment_from(&path).is_none());
        fs::remove_file(path).unwrap();
        assert!(flatpak_host_environment_from(Path::new("/missing-flatpak-spawn")).is_none());
    }
}

#[cfg(test)]
mod docker_shim_tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn host_with_bin(entries: &[(&str, &str)]) -> (Host, PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "pkgdeck-docker-shim-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        for (name, body) in entries {
            let path = root.join("bin").join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut env = BTreeMap::new();
        env.insert("PATH".into(), root.join("bin").into());
        (Host::new(Runtime::Native, env), root)
    }

    #[test]
    fn docker_symlinked_to_podman_is_a_shim() {
        let (host, root) = host_with_bin(&[("podman", "#!/bin/sh\nexit 0\n")]);
        symlink(root.join("bin/podman"), root.join("bin/docker")).unwrap();
        assert!(host.docker_is_podman_shim());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn docker_wrapper_script_execing_podman_is_a_shim() {
        let (host, root) = host_with_bin(&[
            ("podman", "#!/bin/sh\nexit 0\n"),
            ("docker", "#!/bin/sh\nexec /usr/bin/podman \"$@\"\n"),
        ]);
        assert!(host.docker_is_podman_shim());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn real_docker_and_missing_docker_are_not_shims() {
        let (host, root) = host_with_bin(&[
            ("podman", "#!/bin/sh\nexit 0\n"),
            (
                "docker",
                "#!/bin/sh\n# podman is also installed, but this is a separate Docker CLI\necho Docker version 99.0\n",
            ),
        ]);
        assert!(!host.docker_is_podman_shim());
        std::fs::remove_dir_all(root).unwrap();
        let (lonely, root) = host_with_bin(&[("podman", "#!/bin/sh\nexit 0\n")]);
        assert!(!lonely.docker_is_podman_shim());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Authorization {
    /// Uses an existing desktop polkit agent; never reads passwords from application pipes.
    Polkit,
    /// Requires an existing sudo grant; never prompts or changes sudo policy.
    SudoNonInteractive,
}

impl Authorization {
    pub(crate) fn prefix(self, executable: &Path) -> (&'static str, Vec<OsString>) {
        match self {
            Self::Polkit => (
                "/usr/bin/pkexec",
                vec!["--disable-internal-agent".into(), executable.into()],
            ),
            Self::SudoNonInteractive => (
                "/usr/bin/sudo",
                vec!["-n".into(), "--".into(), executable.into()],
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub enum AptAction {
    Refresh,
    Upgrade(String),
    UpgradeAll,
    Install(String),
    InstallLocal(PathBuf),
    Remove(String),
    Autoremove,
    Autoclean,
}

impl AptAction {
    pub fn group_arguments(actions: &[Self]) -> Result<Vec<OsString>, ExecutionError> {
        let Some(first) = actions.first() else {
            return Err(ExecutionError::Invalid("empty APT transaction".into()));
        };
        let kind = std::mem::discriminant(first);
        if !matches!(first, Self::Install(_) | Self::Remove(_) | Self::Upgrade(_))
            || actions
                .iter()
                .any(|action| std::mem::discriminant(action) != kind)
        {
            return Err(ExecutionError::Invalid(
                "mixed APT transaction verbs".into(),
            ));
        }
        let mut arguments = first.arguments()?;
        for action in &actions[1..] {
            let next = action.arguments()?;
            arguments.push(next.last().expect("validated target").clone());
        }
        Ok(arguments)
    }
    pub fn arguments(&self) -> Result<Vec<OsString>, ExecutionError> {
        let (operation, package) = match self {
            Self::Refresh => {
                return Ok([
                    "--assume-yes",
                    "-o",
                    "DPkg::Lock::Timeout=0",
                    "-o",
                    "APT::Update::Error-Mode=any",
                    "update",
                ]
                .map(OsString::from)
                .to_vec());
            }
            Self::UpgradeAll => {
                return Ok([
                    "--assume-yes",
                    "-o",
                    "DPkg::Lock::Timeout=0",
                    "dist-upgrade",
                ]
                .map(OsString::from)
                .to_vec());
            }
            Self::Autoremove => {
                return Ok([
                    "--assume-yes",
                    "-o",
                    "DPkg::Lock::Timeout=0",
                    "--purge",
                    "autoremove",
                ]
                .map(OsString::from)
                .to_vec());
            }
            Self::Autoclean => {
                return Ok(["--assume-yes", "-o", "DPkg::Lock::Timeout=0", "autoclean"]
                    .map(OsString::from)
                    .to_vec());
            }
            Self::InstallLocal(path) => {
                if !path.is_absolute() || path.extension().is_none_or(|ext| ext != "deb") {
                    return Err(ExecutionError::Invalid(
                        "expected an absolute .deb path".into(),
                    ));
                }
                return Ok(vec![
                    "--assume-yes".into(),
                    "-o".into(),
                    "DPkg::Lock::Timeout=0".into(),
                    "--no-remove".into(),
                    "--".into(),
                    "install".into(),
                    path.as_os_str().to_os_string(),
                ]);
            }
            Self::Upgrade(package) => ("install", package),
            Self::Install(package) => ("install", package),
            Self::Remove(package) => ("remove", package),
        };
        let name = package
            .split_once(':')
            .map_or(package.as_str(), |(name, _)| name);
        let arch_valid = package.split_once(':').is_none_or(|(_, arch)| {
            !arch.is_empty()
                && arch
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        });
        if !arch_valid
            || name.len() < 2
            || !name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"+.-".contains(&b))
        {
            return Err(ExecutionError::Invalid(
                "expected a Debian package name, not a path or option".into(),
            ));
        }
        let mut args = vec!["--assume-yes", "-o", "DPkg::Lock::Timeout=0"];
        if operation == "install" {
            args.push("--no-remove");
        }
        if matches!(self, Self::Upgrade(_)) {
            args.push("--only-upgrade");
        }
        args.extend(["--", operation, package]);
        Ok(args.into_iter().map(OsString::from).collect())
    }
}

pub fn classify_apt(result: Completion) -> Result<Completion, ExecutionError> {
    let stderr = String::from_utf8_lossy(&result.stderr);
    match result.code {
        Some(0) => Ok(result),
        Some(126) => Err(ExecutionError::AuthorizationCancelled),
        Some(127) => Err(ExecutionError::AuthorizationDenied),
        _ if stderr.contains("sudo:") => Err(ExecutionError::AuthorizationDenied),
        _ if stderr.contains("Could not get lock") || stderr.contains("Unable to acquire") => {
            Err(ExecutionError::LockBusy)
        }
        _ if stderr.contains("dpkg was interrupted") => Err(ExecutionError::Interrupted),
        None => Err(ExecutionError::Interrupted),
        _ => Err(ExecutionError::Failed(result)),
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn scratch(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "pkgdeck-host-{name}-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }
    fn executable(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn host(runtime: Runtime, env: &[(&str, &Path)]) -> Host {
        Host::new(
            runtime,
            env.iter()
                .map(|(key, value)| (OsString::from(key), value.as_os_str().to_owned()))
                .collect(),
        )
    }
    fn cancelled() -> Cancellation {
        let cancel = Cancellation::default();
        cancel.cancel();
        cancel
    }

    #[test]
    fn bundled_programs_run_in_the_flatpak_sandbox_with_only_the_locale() {
        let root = scratch("bundled");
        let tool = root.join("tool");
        executable(&tool, "env | sort");
        let env = [("HOME", root.as_path())];
        let run = |host: Host| {
            let result = host
                .read_bundled(
                    &tool,
                    &[],
                    &[("PKGDECK_APT_HOST", "/run/host")],
                    Limits::default(),
                    &Cancellation::default(),
                )
                .unwrap();
            String::from_utf8(result.stdout).unwrap()
        };
        // Run here, not through flatpak-spawn, which this machine lacks.
        let sandboxed = run(host(Runtime::Flatpak, &env));
        // Host tools always read the C locale; so does the helper.
        assert!(sandboxed.contains("LANG=C\n"));
        assert!(sandboxed.contains("PKGDECK_APT_HOST=/run/host\n"));
        assert!(!sandboxed.contains("HOME="));
        // A native build runs it like any host program.
        let native = run(host(Runtime::Native, &env));
        assert!(native.contains("HOME="));
        assert!(native.contains("PKGDECK_APT_HOST=/run/host\n"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_writes_as_root_are_refused() {
        assert!(matches!(
            refuse_root_as(true, true),
            Err(ExecutionError::Invalid(reason)) if reason == ROOT_REFUSAL
        ));
        assert!(refuse_root_as(false, true).is_ok());
        assert!(refuse_root_as(true, false).is_ok());
    }

    #[test]
    fn a_failing_host_probe_is_an_error_not_a_missing_file() {
        let root = scratch("probe");
        let bridge = root.join("spawn");
        executable(&bridge, "exit 2");
        let mut host = host(Runtime::Flatpak, &[]);
        host.bridge = bridge;
        assert!(matches!(
            host.host_file(Path::new("/usr/bin/flatpak")),
            Err(ExecutionError::Failed(result)) if result.code == Some(2)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn homebrew_listings_go_through_the_cache_and_keep_errors() {
        let root = scratch("brew");
        executable(&root.join("bin/brew"), "exit 0");
        let host = host(
            Runtime::Native,
            &[("PATH", &root.join("bin")), ("HOME", &root)],
        );
        for args in [
            &["info", "--json=v2", "--installed"][..],
            &["info", "--json=v2", "--cask", "--installed"],
        ] {
            let args: Vec<OsString> = args.iter().map(OsString::from).collect();
            assert!(matches!(
                host.brew(&args, &cancelled(), false),
                Err(ExecutionError::Cancelled)
            ));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn homebrew_answers_are_kept_only_where_homebrew_keeps_its_data() {
        let root = scratch("brew-kept");
        let cache = root.join("cache");
        let env = [
            ("PATH", root.join("bin")),
            ("HOME", root.clone()),
            ("HOMEBREW_CACHE", cache.clone()),
        ];
        let env: Vec<(&str, &Path)> = env.iter().map(|(k, v)| (*k, v.as_path())).collect();
        assert!(host(Runtime::Native, &env).brew_cache().is_none());
        // Like brew, which also reads HOMEBREW_CACHE from brew.env files.
        executable(&root.join("bin/brew"), "echo \"$HOMEBREW_CACHE\"");
        assert!(host(Runtime::Flatpak, &env).brew_cache().is_none());
        // A cache folder without fetched data, or no answer at all.
        assert!(host(Runtime::Native, &env).brew_cache().is_none());
        assert!(host(Runtime::Native, &env[..2]).brew_cache().is_none());
        fs::create_dir_all(cache.join("api")).unwrap();
        let cellar = fs::canonicalize(&root).unwrap().join("Cellar");
        let watches = host(Runtime::Native, &env)
            .brew_cache()
            .map(|(_, watches)| watches)
            .unwrap_or_default();
        let watched = |path: &Path| watches.iter().any(|watch| watch.path == path);
        assert_eq!(watched(&cellar), crate::cache::Store::user().is_some());
        assert_eq!(
            watched(&cache.join("api")),
            crate::cache::Store::user().is_some()
        );
        // Homebrew failing keeps nothing.
        executable(&root.join("bin/brew"), "exit 1");
        assert!(host(Runtime::Native, &env).brew_cache().is_none());
        // Nor does a Homebrew outside a repository.
        let host = host(Runtime::Native, &env);
        assert!(host
            .brew_details_watches(Path::new("/brew"), &cache)
            .is_none());
        assert!(host
            .brew_details_watches(Path::new("/brew"), Path::new("cache"))
            .is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn homebrew_watches_need_a_repository_and_follow_the_cache_folder() {
        let host_without_home = host(Runtime::Native, &[]);
        assert!(host_without_home
            .brew_watches(Path::new("/brew"), None)
            .is_empty());
        assert_eq!(
            host_without_home
                .brew_watches(Path::new("/synthetic/bin/brew"), None)
                .len(),
            5
        );
        let root = scratch("brew-cache");
        let home = root.join("home");
        let xdg = root.join("xdg");
        let cache_watch = |host: &Host| {
            host.brew_watches(
                Path::new("/synthetic/bin/brew"),
                host.brew_cache_folder().as_deref(),
            )
            .last()
            .unwrap()
            .path
            .clone()
        };
        let plain = host(Runtime::Native, &[("HOME", &home)]);
        let custom = host(
            Runtime::Native,
            &[("HOME", &home), ("XDG_CACHE_HOME", &xdg)],
        );
        if cfg!(target_os = "macos") {
            assert_eq!(
                cache_watch(&plain),
                home.join("Library/Caches/Homebrew/api")
            );
            assert_eq!(
                cache_watch(&custom),
                home.join("Library/Caches/Homebrew/api")
            );
        } else {
            assert_eq!(cache_watch(&plain), home.join(".cache/Homebrew/api"));
            assert_eq!(cache_watch(&custom), xdg.join("Homebrew/api"));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn official_installer_locations_are_found_off_path() {
        let root = scratch("user-install");
        let home = root.join("home");
        let tools = root.join("tools");
        for (name, variable, under_variable, under_home) in [
            ("pixi", "PIXI_HOME", "bin/pixi", Some(".pixi/bin/pixi")),
            ("go", "GOROOT", "bin/go", None),
            ("dotnet", "DOTNET_ROOT", "dotnet", Some(".dotnet/dotnet")),
            (
                "rustup",
                "CARGO_HOME",
                "bin/rustup",
                Some(".cargo/bin/rustup"),
            ),
            ("cargo", "CARGO_HOME", "bin/cargo", Some(".cargo/bin/cargo")),
            ("nix", "", "", Some(".nix-profile/bin/nix")),
        ] {
            let from_variable = tools.join(name).join(under_variable);
            if !variable.is_empty() {
                executable(&from_variable, "exit 0");
                let host = host(
                    Runtime::Native,
                    &[(variable, &tools.join(name)), ("HOME", &home)],
                );
                assert_eq!(host.user_install(name).unwrap(), Some(from_variable));
            }
            if let Some(relative) = under_home {
                executable(&home.join(relative), "exit 0");
                let host = host(Runtime::Native, &[("HOME", &home)]);
                assert_eq!(host.user_install(name).unwrap(), Some(home.join(relative)));
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn user_tools_stop_before_running_when_cancelled() {
        let root = scratch("cancel");
        let bin = root.join("bin");
        executable(&bin.join("tool"), "exit 0");
        let host = host(Runtime::Native, &[("PATH", &bin), ("HOME", &root)]);
        assert!(matches!(
            host.dev_tool("tool", "Tool", &[], &cancelled(), false),
            Err(ExecutionError::Cancelled)
        ));
        assert!(matches!(
            host.system_manager("tool", &[], &cancelled(), false, Authorization::Polkit),
            Err(ExecutionError::Cancelled)
        ));
        assert!(matches!(
            host.container_engine("tool", "Tool", &[], &cancelled(), false),
            Err(ExecutionError::Cancelled)
        ));
        let venv = root.join("venv");
        executable(&venv.join("bin/python"), "echo broken >&2; exit 3");
        fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        assert!(matches!(
            host.venv_pip(&venv, &["list".into()], &cancelled(), false),
            Err(ExecutionError::Cancelled)
        ));
        assert!(matches!(
            host.venv_pip(&venv, &["list".into()], &Cancellation::default(), false),
            Err(ExecutionError::Failed(result)) if result.code == Some(3) && result.stderr == b"broken\n"
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn docker_is_a_shim_only_when_it_is_podman() {
        let root = scratch("docker");
        let bin = root.join("bin");
        let host = host(Runtime::Native, &[("PATH", &bin)]);
        executable(&root.join("podman"), "exit 0");
        symlink(root.join("podman"), root.join("alias")).unwrap();
        fs::create_dir_all(&bin).unwrap();
        symlink(root.join("podman"), bin.join("podman")).unwrap();
        // A chain of links that ends at podman.
        symlink(root.join("alias"), bin.join("docker")).unwrap();
        assert!(host.docker_is_podman_shim());
        fs::remove_file(bin.join("docker")).unwrap();
        // A large binary is never read as a script.
        fs::write(bin.join("docker"), vec![b'#'; 70_000]).unwrap();
        fs::set_permissions(bin.join("docker"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!host.docker_is_podman_shim());
        fs::remove_file(bin.join("podman")).unwrap();
        executable(&bin.join("docker"), "exec /usr/bin/podman \"$@\"");
        assert!(host.docker_is_podman_shim());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn privileged_apt_and_repository_writes_use_fixed_programs() {
        let root = scratch("privileged");
        let mut host = host(Runtime::Flatpak, &[]);
        host.bridge = "/bin/echo".into();
        let cancel = Cancellation::default();
        let apt = host
            .apt(
                AptAction::Install("fixture".into()),
                Authorization::SudoNonInteractive,
                &cancel,
            )
            .unwrap();
        assert!(
            String::from_utf8_lossy(&apt.stdout).contains("/usr/bin/sudo -n -- /usr/bin/apt-get")
        );
        let source = root.join("fixture.sources");
        fs::write(&source, "Types: deb\n").unwrap();
        let digest = "c".repeat(64);
        let installed = host
            .install_repository_file(
                &source,
                "apt",
                "sources",
                &digest,
                Authorization::Polkit,
                &cancel,
            )
            .unwrap();
        let output = String::from_utf8_lossy(&installed.stdout).into_owned();
        assert!(output.contains("/usr/bin/pkexec --disable-internal-agent /usr/bin/install"));
        assert!(output.contains(&format!("pkgdeck-{digest}.sources")));
        host.bridge = root.join("missing-spawn");
        assert!(matches!(
            host.apt_group(
                &[AptAction::Install("fixture".into())],
                Authorization::Polkit,
                &cancel
            ),
            Err(ExecutionError::Disabled(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn macports_is_found_in_its_own_folder_off_path() {
        let root = scratch("port");
        let bridge = root.join("spawn");
        executable(&bridge, "echo \"$@\"");
        let mut host = host(Runtime::Flatpak, &[("PATH", &root.join("empty"))]);
        host.bridge = bridge;
        let result = host
            .system_manager(
                "port",
                &["installed".into()],
                &Cancellation::default(),
                false,
                Authorization::Polkit,
            )
            .unwrap();
        assert!(
            String::from_utf8_lossy(&result.stdout).ends_with("/opt/local/bin/port installed\n")
        );
        // softwareupdate lives in /usr/sbin, which PATH often leaves out.
        let result = host
            .system_manager(
                "softwareupdate",
                &["--list".into()],
                &Cancellation::default(),
                false,
                Authorization::Polkit,
            )
            .unwrap();
        assert!(
            String::from_utf8_lossy(&result.stdout).ends_with("/usr/sbin/softwareupdate --list\n")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn administrator_dialog_cancel_is_reported() {
        let completion = |code, stderr: &str| Completion {
            code: Some(code),
            signal: None,
            stdout: vec![],
            stderr: stderr.as_bytes().to_vec(),
            truncated: false,
            cancellation_deferred: false,
        };
        assert!(matches!(
            administrator_result(completion(1, "execution error: User canceled. (-128)")),
            Err(ExecutionError::AuthorizationCancelled)
        ));
        assert_eq!(
            administrator_result(completion(1, "other failure"))
                .unwrap()
                .code,
            Some(1)
        );
        assert_eq!(
            administrator_result(completion(0, "")).unwrap().code,
            Some(0)
        );
    }

    #[test]
    fn host_environment_comes_from_the_bridge() {
        let root = scratch("environment");
        let bridge = root.join("spawn");
        executable(&bridge, "printf 'HOME=/home/fixture\\0PATH=/usr/bin\\0'");
        let env = flatpak_host_environment_from(&bridge).unwrap();
        assert_eq!(env[&OsString::from("HOME")], "/home/fixture");
        assert_eq!(env[&OsString::from("PATH")], "/usr/bin");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn source_editor_opens_through_the_host_bridge() {
        let root = scratch("editor");
        let record = root.join("record");
        let bridge = root.join("spawn");
        executable(&bridge, &format!("echo \"$@\" >> '{}'", record.display()));
        executable(&root.join("bin/apt-get"), "exit 0");
        let mut host = host(
            Runtime::Flatpak,
            &[("PATH", &root.join("bin")), ("DISPLAY", Path::new(":91"))],
        );
        host.bridge = bridge;
        assert!(host
            .source_editor_path_with(Path::new("/usr/bin/pkexec"), Path::new("/usr/bin/env"), &[])
            .unwrap()
            .is_none());
        host.open_source_editor(Authorization::Polkit).unwrap();
        let calls = fs::read_to_string(&record).unwrap();
        assert!(calls.lines().last().unwrap().ends_with(
            "/usr/bin/pkexec --disable-internal-agent /usr/bin/env DISPLAY=:91 /usr/bin/software-properties-qt"
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
