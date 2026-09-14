//! Host execution boundary. Sandboxed formats fail closed until separately validated.
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
    pub fn detect(env: &BTreeMap<OsString, OsString>, flatpak_info: bool) -> Self {
        if env.contains_key(&OsString::from("SNAP")) {
            Self::Snap
        } else if flatpak_info || env.contains_key(&OsString::from("FLATPAK_ID")) {
            Self::Flatpak
        } else if env.contains_key(&OsString::from("APPIMAGE"))
            || env.contains_key(&OsString::from("APPDIR"))
        {
            Self::AppImage
        } else {
            Self::Native
        }
    }

    pub fn disabled_reason(self) -> Option<&'static str> {
        match self {
            Self::Flatpak => Some("Flatpak host execution is disabled: flatpak-spawn --host and org.freedesktop.Flatpak D-Bus permission require installed-format validation"),
            Self::Snap => Some("Snap host execution is disabled: strict confinement has no validated host package-manager bridge"),
            Self::Native | Self::AppImage => None,
        }
    }
}

/// A snapshot of the invoking user's environment, stripped of packaging/runtime injection.
#[derive(Clone, Debug)]
pub struct Host {
    pub runtime: Runtime,
    env: BTreeMap<OsString, OsString>,
    excluded: Option<PathBuf>,
}

pub const BACKENDS: &[(&str, &str)] = &[
    ("APT", "apt-get"),
    ("DNF", "dnf"),
    ("Pacman", "pacman"),
    ("Zypper", "zypper"),
    ("Flatpak", "flatpak"),
    ("Snap", "snap"),
    ("Homebrew", "brew"),
    ("Cargo", "cargo"),
    ("npm", "npm"),
    ("pnpm", "pnpm"),
    ("Bun", "bun"),
    ("pipx", "pipx"),
    ("uv", "uv"),
    ("Composer", "composer"),
    ("RubyGems", "gem"),
];

impl Host {
    pub fn current() -> Self {
        let env = std::env::vars_os().collect();
        let runtime = Runtime::detect(&env, Path::new("/.flatpak-info").exists());
        Self::new(runtime, env)
    }

    pub fn new(runtime: Runtime, source: BTreeMap<OsString, OsString>) -> Self {
        let excluded = source
            .get(&OsString::from("APPDIR"))
            .map(PathBuf::from)
            .map(|path| fs::canonicalize(&path).unwrap_or(path));
        let mut env = BTreeMap::new();
        // No LD_*, PYTHONPATH, NODE_OPTIONS, APT_CONFIG, shell startup files,
        // Qt plugins, or sandbox XDG directories are inherited by host tools.
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
            "XDG_CACHE_HOME",
            "SSH_AUTH_SOCK",
        ] {
            if let Some(value) = source.get(&OsString::from(name)) {
                env.insert(name.into(), value.clone());
            }
        }
        let path = source
            .get(&OsString::from("PATH"))
            .cloned()
            .unwrap_or_else(|| {
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into()
            });
        let dirs: Vec<_> = std::env::split_paths(&path)
            .filter(|p| {
                p.is_absolute() && !excluded.as_ref().is_some_and(|root| p.starts_with(root))
            })
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
        }
    }

    fn enabled(&self) -> Result<(), ExecutionError> {
        match self.runtime.disabled_reason() {
            Some(reason) => Err(ExecutionError::Disabled(reason.into())),
            None => Ok(()),
        }
    }

    /// Resolve on the host, never by running a shell or probing a packaged runtime.
    pub fn resolve(&self, name: &str) -> Result<Option<PathBuf>, ExecutionError> {
        self.enabled()?;
        if name.is_empty() || name.contains('/') {
            return Err(ExecutionError::Invalid(
                "expected an executable name".into(),
            ));
        }
        let path = &self.env[&OsString::from("PATH")];
        Ok(std::env::split_paths(path).find_map(|dir| {
            let candidate = dir.join(name);
            let target = fs::canonicalize(&candidate).ok()?;
            if self
                .excluded
                .as_ref()
                .is_some_and(|root| target.starts_with(root))
            {
                return None;
            }
            let metadata = fs::metadata(&target).ok()?;
            (metadata.is_file()
                && rustix::fs::access(&candidate, rustix::fs::Access::EXEC_OK).is_ok())
            .then_some(candidate)
        }))
    }

    fn command(&self, executable: &Path, args: &[OsString]) -> Result<Command, ExecutionError> {
        self.enabled()?;
        if !executable.is_absolute() {
            return Err(ExecutionError::Invalid(
                "host executable must be absolute".into(),
            ));
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

    /// Homebrew always runs as the invoking user, with automatic unrelated work disabled.
    pub fn brew(
        &self,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        self.enabled()?;
        if write && rustix::process::geteuid().is_root() {
            return Err(ExecutionError::Invalid(
                "run the frontend as an unprivileged user".into(),
            ));
        }
        let path = self
            .resolve("brew")?
            .ok_or_else(|| ExecutionError::Disabled("Homebrew not found".into()))?;
        let mut command = self.command(&path, args)?;
        for name in [
            "HOMEBREW_NO_AUTO_UPDATE",
            "HOMEBREW_NO_INSTALL_CLEANUP",
            "HOMEBREW_NO_ANALYTICS",
            "HOMEBREW_NO_ENV_HINTS",
            "HOMEBREW_NO_COLOR",
        ] {
            command.env(name, "1");
        }
        let result = process::run(
            command,
            Limits {
                timeout: std::time::Duration::from_secs(120),
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
        self.enabled()?;
        if rustix::process::geteuid().is_root() {
            return Err(ExecutionError::Invalid(
                "run the frontend as an unprivileged user".into(),
            ));
        }
        let result = self.privileged(
            Path::new("/usr/bin/apt-get"),
            &action.arguments()?,
            authorization,
            cancel,
        )?;
        classify_apt(result)
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
        self.enabled()?;
        if write && rustix::process::geteuid().is_root() {
            return Err(ExecutionError::Invalid(
                "run the frontend as an unprivileged user".into(),
            ));
        }
        let path = if write && system {
            // System writes cross an authorization boundary. Never derive this
            // executable from the invoking user's PATH.
            let path = PathBuf::from("/usr/bin/flatpak");
            if !path.is_file() {
                return Err(ExecutionError::Disabled("system Flatpak not found".into()));
            }
            path
        } else {
            self.resolve("flatpak")?
                .ok_or_else(|| ExecutionError::Disabled("Flatpak not found".into()))?
        };
        if write && system {
            self.privileged(&path, args, authorization, cancel)
        } else {
            self.run(&path, args, cancel, write)
        }
    }

    fn run(
        &self,
        executable: &Path,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        let command = self.command(executable, args)?;
        let result = process::run(command, Limits::default(), cancel, write)?;
        if result.code == Some(0) {
            Ok(result)
        } else {
            Err(ExecutionError::Failed(result))
        }
    }

    fn privileged(
        &self,
        executable: &Path,
        args: &[OsString],
        authorization: Authorization,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        let (program, mut prefixed) = authorization.prefix(executable);
        prefixed.extend(args.iter().cloned());
        let mut command = self.command(Path::new(program), &prefixed)?;
        command.env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
        process::run(command, Limits::default(), cancel, true)
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
    fn prefix(self, executable: &Path) -> (&'static str, Vec<OsString>) {
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

#[derive(Debug)]
pub enum AptAction {
    Refresh,
    Upgrade(String),
    Install(String),
    Remove(String),
}

impl AptAction {
    pub fn arguments(&self) -> Result<Vec<OsString>, ExecutionError> {
        if matches!(self, Self::Refresh) {
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
        let (operation, package) = match self {
            Self::Refresh => unreachable!(),
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
