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
            Self::Snap | Self::Native | Self::AppImage => None,
        }
    }
}

/// A snapshot of the invoking user's environment, stripped of packaging/runtime injection.
#[derive(Clone, Debug)]
pub struct Host {
    pub runtime: Runtime,
    env: BTreeMap<OsString, OsString>,
    excluded: Vec<PathBuf>,
}

pub const BACKENDS: &[(&str, &str)] = &[
    ("APT", "apt-get"),
    ("DNF", "dnf"),
    ("Pacman", "pacman"),
    ("Zypper", "zypper"),
    ("Flatpak", "flatpak"),
    ("Snap", "snap"),
    ("Homebrew", "brew"),
    ("Docker", "docker"),
    ("Podman", "podman"),
    ("Cargo", "cargo"),
    ("npm", "npm"),
    ("pnpm", "pnpm"),
    ("Bun", "bun"),
    ("pip (virtual environment)", "pip3"),
    ("pipx", "pipx"),
    ("uv", "uv"),
    ("Composer", "composer"),
    ("RubyGems", "gem"),
];

impl Host {
    pub fn current() -> Self {
        let mut env = std::env::vars_os().collect();
        let runtime = Runtime::detect(&env, Path::new("/.flatpak-info").exists());
        if runtime == Runtime::Flatpak {
            if let Some(host_env) = flatpak_host_environment() {
                env = host_env;
            }
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
            "XDG_CACHE_HOME",
            "SSH_AUTH_SOCK",
            "VIRTUAL_ENV",
            "PIPX_HOME",
            "PIPX_BIN_DIR",
            "UV_TOOL_DIR",
            "COMPOSER_HOME",
            "GEM_HOME",
            "CODEX_HOME",
            "CODEX_INSTALL_DIR",
            "CLAUDE_CONFIG_DIR",
            "GROK_BIN_DIR",
            "DISABLE_UPDATES",
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
        }
    }

    fn enabled(&self) -> Result<(), ExecutionError> {
        match self.runtime.disabled_reason() {
            Some(reason) => Err(ExecutionError::Disabled(reason.into())),
            None => Ok(()),
        }
    }

    /// Read one sanitized environment value (for example `HOME`) without
    /// exposing the whole map. Tool homes derive from this, never from PATH.
    pub fn var(&self, name: &str) -> Option<OsString> {
        self.env.get(&OsString::from(name)).cloned()
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
            if self.excluded.iter().any(|root| target.starts_with(root)) {
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

    /// Run an already identified standalone installation as its owner. The
    /// adapter supplies fixed updater arguments, never a user shell command.
    pub(crate) fn standalone_write(
        &self,
        executable: &Path,
        args: &[OsString],
        env: &[(&str, OsString)],
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        if rustix::process::geteuid().is_root() {
            return Err(ExecutionError::Invalid(
                "run the frontend as an unprivileged user".into(),
            ));
        }
        let mut command = self.command(executable, args)?;
        command.envs(env.iter().map(|(key, value)| (key, value)));
        process::run(
            command,
            Limits {
                timeout: std::time::Duration::from_secs(300),
                output_bytes: 1024 * 1024,
            },
            cancel,
            true,
        )
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
        self.enabled()?;
        if write && rustix::process::geteuid().is_root() {
            return Err(ExecutionError::Invalid(
                "run the frontend as an unprivileged user".into(),
            ));
        }
        let path = self
            .resolve(executable)?
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
        self.enabled()?;
        if write && rustix::process::geteuid().is_root() {
            return Err(ExecutionError::Invalid(
                "run the frontend as an unprivileged user".into(),
            ));
        }
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
        self.enabled()?;
        if write && rustix::process::geteuid().is_root() {
            return Err(ExecutionError::Invalid(
                "run the frontend as an unprivileged user".into(),
            ));
        }
        if !venv.is_absolute() {
            return Err(ExecutionError::Invalid(
                "virtual environment path must be absolute".into(),
            ));
        }
        let python = venv.join("bin/python");
        let meta = fs::symlink_metadata(&python).map_err(|_| {
            ExecutionError::Disabled("pip virtual environment not found".to_string())
        })?;
        if !meta.file_type().is_symlink() && !meta.is_file() {
            return Err(ExecutionError::Disabled(
                "pip virtual environment not found".to_string(),
            ));
        }
        if fs::metadata(venv.join("pyvenv.cfg"))
            .map(|marker| !marker.is_file())
            .unwrap_or(true)
        {
            return Err(ExecutionError::Disabled(
                "pip virtual environment not found".to_string(),
            ));
        }
        if self
            .excluded
            .iter()
            .any(|root| venv.starts_with(root) || python.starts_with(root))
        {
            return Err(ExecutionError::Disabled(
                "pip virtual environment not found".to_string(),
            ));
        }
        if rustix::fs::access(&python, rustix::fs::Access::EXEC_OK).is_err() {
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
        if write && rustix::process::geteuid().is_root() {
            return Err(ExecutionError::Invalid(
                "run the frontend as an unprivileged user".into(),
            ));
        }
        if self.runtime == Runtime::Flatpak {
            let path = Path::new("/usr/bin/flatpak");
            let (program, mut full) = if write && system {
                let (program, prefixed) = authorization.prefix(path);
                (Path::new(program), prefixed)
            } else {
                (path, Vec::new())
            };
            full.extend(args.iter().cloned());
            let command = self.flatpak_host_command(program, &full)?;
            let result = process::run(command, Limits::default(), cancel, write)?;
            return if result.code == Some(0) {
                Ok(result)
            } else {
                Err(ExecutionError::Failed(result))
            };
        }
        self.enabled()?;
        let path = if write && system {
            // System writes cross an authorization boundary. Never derive this
            // executable from the invoking user's PATH.
            system_flatpak_path(Path::new("/usr/bin/flatpak"))?
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

    fn flatpak_host_command(
        &self,
        executable: &Path,
        args: &[OsString],
    ) -> Result<Command, ExecutionError> {
        self.flatpak_host_command_with_bridge(Path::new("/usr/bin/flatpak-spawn"), executable, args)
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
            // The bridge runs as the host user. Let libc/GLib resolve that
            // user's home and XDG stores instead of carrying sandbox paths
            // across the boundary. Session variables such as the D-Bus and
            // runtime-directory addresses still need to be forwarded.
            if matches!(
                name.to_str(),
                Some("HOME" | "XDG_CONFIG_HOME" | "XDG_DATA_HOME" | "XDG_CACHE_HOME")
            ) {
                continue;
            }
            let mut assignment = OsString::from("--env=");
            assignment.push(name);
            assignment.push("=");
            assignment.push(value);
            command.arg(assignment);
        }
        command.arg(executable).args(args);
        Ok(command)
    }

    /// Open the distro editor as the current user; its native policy handles writes.
    pub fn open_source_editor(&self) -> Result<(), ExecutionError> {
        self.enabled()?;
        let path = ["/usr/bin/software-properties-qt", "/usr/bin/software-properties-gtk"]
            .into_iter().map(PathBuf::from).find(|path| path.is_file())
            .ok_or_else(|| ExecutionError::Disabled("Install software-properties-qt or software-properties-gtk to manage APT repositories".into()))?;
        let mut child = self
            .command(&path, &[])?
            .spawn()
            .map_err(|error| ExecutionError::Io(error.to_string()))?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
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
        self.enabled()?;
        if write && rustix::process::geteuid().is_root() {
            return Err(ExecutionError::Invalid(
                "run the frontend as an unprivileged user".into(),
            ));
        }
        let path = if write {
            let path = PathBuf::from("/usr/bin").join(executable);
            if !path.is_file() {
                return Err(ExecutionError::Disabled(format!("{executable} not found")));
            }
            path
        } else {
            self.resolve(executable)?
                .ok_or_else(|| ExecutionError::Disabled(format!("{executable} not found")))?
        };
        if write {
            self.privileged(&path, args, authorization, cancel)
        } else {
            let timeout = if matches!(executable, "snap" | "fwupdmgr") {
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
    let output = Command::new(bridge)
        .args(["--host", "/usr/bin/env", "-0"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut env = BTreeMap::new();
    for entry in output.stdout.split(|byte| *byte == 0) {
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
    fn bridge_commands_pin_the_host_program_and_forward_only_sanitized_values() {
        let host = Host::new(
            Runtime::Flatpak,
            [
                ("HOME".into(), "/home/fixture".into()),
                ("PATH".into(), "/usr/bin".into()),
                (
                    "XDG_CONFIG_HOME".into(),
                    "/home/fixture/.var/app/io.github.astrovm.PkgDeck/config".into(),
                ),
                (
                    "XDG_DATA_HOME".into(),
                    "/home/fixture/.var/app/io.github.astrovm.PkgDeck/data".into(),
                ),
                ("XDG_CACHE_HOME".into(), "/home/fixture/.cache-alt".into()),
                ("XDG_RUNTIME_DIR".into(), "/run/user/1000".into()),
            ]
            .into(),
        );
        let command = host
            .flatpak_host_command_with_bridge(
                Path::new("/bin/true"),
                Path::new("/usr/bin/flatpak"),
                &["--user".into(), "list".into()],
            )
            .unwrap();
        assert_eq!(command.get_program(), "/bin/true");
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
        assert!(!args.iter().any(|arg| arg.starts_with("--env=HOME=")));
        assert!(!args
            .iter()
            .any(|arg| arg.starts_with("--env=XDG_CACHE_HOME=")));
        assert!(!args
            .iter()
            .any(|arg| arg.starts_with("--env=XDG_CONFIG_HOME=")));
        assert!(!args
            .iter()
            .any(|arg| arg.starts_with("--env=XDG_DATA_HOME=")));
        assert!(args.ends_with(&["/usr/bin/flatpak".into(), "--user".into(), "list".into()]));
        assert!(matches!(
            host.flatpak_host_command_with_bridge(
                Path::new("/bin/true"),
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
            system_flatpak_path(Path::new("/bin/true")).unwrap(),
            Path::new("/bin/true")
        );
        assert!(matches!(
            system_flatpak_path(Path::new("/missing-system-flatpak")),
            Err(ExecutionError::Disabled(_))
        ));
    }

    #[test]
    fn host_environment_probe_parses_nul_records_and_rejects_failures() {
        let path =
            std::env::temp_dir().join(format!("pkgdeck-flatpak-spawn-{}", std::process::id()));
        fs::write(
            &path,
            b"#!/bin/sh\nprintf 'HOME=/home/fixture\\0PATH=/usr/bin\\0'\n",
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let env = flatpak_host_environment_from(&path).unwrap();
        assert_eq!(
            env.get(&OsString::from("HOME")),
            Some(&"/home/fixture".into())
        );
        assert_eq!(env.get(&OsString::from("PATH")), Some(&"/usr/bin".into()));
        fs::write(&path, b"#!/bin/sh\nexit 7\n").unwrap();
        assert!(flatpak_host_environment_from(&path).is_none());
        fs::write(&path, b"#!/bin/sh\nprintf malformed\n").unwrap();
        assert!(flatpak_host_environment_from(&path).is_none());
        fs::remove_file(path).unwrap();
        assert!(flatpak_host_environment_from(Path::new("/missing-flatpak-spawn")).is_none());
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
    UpgradeAll,
    Install(String),
    Remove(String),
    Autoremove,
    Autoclean,
}

impl AptAction {
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
                return Ok(["--assume-yes", "-o", "DPkg::Lock::Timeout=0", "upgrade"]
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
