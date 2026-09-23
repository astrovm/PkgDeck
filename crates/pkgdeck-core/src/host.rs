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
            bridge: "/usr/bin/flatpak-spawn".into(),
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

    fn host_file(&self, path: &Path, executable: bool) -> Result<bool, ExecutionError> {
        if self.runtime != Runtime::Flatpak {
            return Ok(path.is_file()
                && (!executable || rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_ok()));
        }
        // Validate in the host namespace: sandbox symlinks and runtime files
        // cannot prove that a host executable exists or is runnable.
        for flag in if executable {
            &["-f", "-x"][..]
        } else {
            &["-f"][..]
        } {
            let command = self
                .flatpak_host_command(Path::new("/usr/bin/test"), &[(*flag).into(), path.into()])?;
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
        self.enabled()?;
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
                    && self.host_file(&candidate, true)?
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
        self.enabled()?;
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
        let mut host = self.clone();
        host.env.extend(
            env.iter()
                .map(|(key, value)| ((*key).into(), value.clone())),
        );
        let command = host.command(executable, args)?;
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
        let command = host.command(&path, args)?;
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
        if self.runtime != Runtime::Flatpak {
            if let Some(podman) = self.resolve("podman").ok().flatten() {
                if let (Ok(docker), Ok(podman)) =
                    (fs::canonicalize(&path), fs::canonicalize(&podman))
                {
                    if docker == podman {
                        return true;
                    }
                }
            }
        }
        // Wrapper script that execs podman; ELF binaries are never parsed.
        if fs::metadata(&probe)
            .ok()
            .is_some_and(|meta| meta.len() < 65536)
        {
            if let Ok(bytes) = fs::read(&probe) {
                let text = String::from_utf8_lossy(&bytes);
                if text.starts_with("#!")
                    && text.lines().any(|line| {
                        let line = line.trim();
                        line.starts_with("exec ")
                            && line.split_whitespace().any(|word| {
                                word.trim_matches('"').rsplit('/').next() == Some("podman")
                            })
                    })
                {
                    return true;
                }
            }
        }
        false
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
        if self
            .excluded
            .iter()
            .any(|root| venv.starts_with(root) || python.starts_with(root))
        {
            return Err(ExecutionError::Disabled(
                "pip virtual environment not found".to_string(),
            ));
        }
        if !self.host_file(&python, true)? {
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

    /// Execute one validated APT transaction with several exact targets.
    pub fn apt_group(
        &self,
        actions: &[AptAction],
        authorization: Authorization,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        self.enabled()?;
        if rustix::process::geteuid().is_root() {
            return Err(ExecutionError::Invalid(
                "run the frontend as an unprivileged user".into(),
            ));
        }
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
        if write && rustix::process::geteuid().is_root() {
            return Err(ExecutionError::Invalid(
                "run the frontend as an unprivileged user".into(),
            ));
        }
        self.enabled()?;
        let path = if write && system {
            if self.runtime == Runtime::Flatpak {
                let path = Path::new("/usr/bin/flatpak");
                if !self.host_file(path, true)? {
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
        // Remote catalog queries can download metadata on their first call.
        // Keep these cancellable, but allow more time and output than local reads.
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
}

impl Host {
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

    /// Open the distro editor as the current user; its native policy handles writes.
    pub fn open_source_editor(&self) -> Result<(), ExecutionError> {
        self.enabled()?;
        let path = ["/usr/bin/software-properties-qt", "/usr/bin/software-properties-gtk"]
            .into_iter().map(PathBuf::from).find(|path| self.host_file(path, true).unwrap_or(false))
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
            if !self.host_file(&path, true)? {
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
        let (program, mut prefixed) = authorization.prefix(executable);
        prefixed.extend(args.iter().cloned());
        let mut host = self.clone();
        host.env
            .insert("PATH".into(), "/usr/sbin:/usr/bin:/sbin:/bin".into());
        let command = host.command(Path::new(program), &prefixed)?;
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
        host.bridge = "/bin/true".into();
        for name in ["apt-get", "brew", "podman", "python3"] {
            let path = Path::new("/usr/bin").join(name);
            let command = host
                .command(&path, &["literal;$(no-shell)".into()])
                .unwrap();
            assert_eq!(command.get_program(), "/bin/true");
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
            .host_file(Path::new("/synthetic-host-executable"), true)
            .unwrap());
        host.bridge = "/bin/false".into();
        assert!(!host
            .host_file(Path::new("/synthetic-host-executable"), true)
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
        std::os::unix::fs::symlink("/bin/true", directory.join("brew")).unwrap();
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
        std::os::unix::fs::symlink("/bin/true", directory.join("flatpak")).unwrap();
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

#[cfg(test)]
mod apt_sandboxed_tests {
    use super::*;
    use crate::backends::{NativeTransport, Transport};
    use std::os::unix::fs::PermissionsExt;

    const BRIDGE: &str = r#"#!/bin/sh
exe=""
trailing=""
collect=0
for arg in "$@"; do
  if [ "$collect" = "0" ]; then
    if [ "$arg" = "--directory=/" ]; then collect=1; fi
    continue
  fi
  case "$arg" in --env=*) continue;; esac
  if [ -z "$exe" ]; then exe=$arg; else trailing="$trailing $arg"; fi
done
case "$exe" in
  /usr/bin/test)
    flag=""
    target=""
    for token in $trailing; do
      case "$token" in -*) flag=$token;; *) target=$token;; esac
    done
    if [ "$flag" = "-x" ]; then test -f "$target" -a -x "$target"; else test -f "$target"; fi
    exit $?;;
  *dpkg-query) cat "$(dirname "$exe")/dpkg.txt";;
  *apt-cache)
    case "$trailing" in
      *policy*) cat "$(dirname "$exe")/policy.txt";;
      *search*) cat "$(dirname "$exe")/search.txt";;
      *show*) cat "$(dirname "$exe")/show.txt";;
      *) exit 99;;
    esac;;
  *) exit 99;;
esac
"#;

    fn sandboxed_transport() -> (NativeTransport, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "pkgdeck-apt-sandbox-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for name in ["apt-get", "dpkg-query", "apt-cache", "fake-spawn"] {
            let path = bin.join(name);
            let body = if name == "fake-spawn" {
                BRIDGE.into()
            } else {
                "#!/bin/sh\nexit 0\n".to_owned()
            };
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(
            bin.join("dpkg.txt"),
            "bash\tamd64\t5.3-2ubuntu1\tinstall ok installed\n\
             oldpkg\tamd64\t1.0\thold ok installed\n",
        )
        .unwrap();
        std::fs::write(
            bin.join("policy.txt"),
            "bash:\n  Installed: 5.3-2ubuntu1\n  Candidate: 5.3-2ubuntu1\n  Version table:\n *** 5.3-2ubuntu1 500\n\
             oldpkg:\n  Installed: 1.0\n  Candidate: 2.0\n  Version table:\n     2.0 500\n *** 1.0 500\n",
        )
        .unwrap();
        std::fs::write(
            bin.join("search.txt"),
            "bash - Bourne Again SHell\noldpkg - Old package\n",
        )
        .unwrap();
        std::fs::write(
            bin.join("show.txt"),
            "Package: bash\nArchitecture: amd64\nVersion: 5.3-2ubuntu1\nDescription-en: Bourne Again SHell\n\n\
             Package: oldpkg\nArchitecture: amd64\nVersion: 2.0\nDescription-en: Old package\n",
        )
        .unwrap();
        let mut env = BTreeMap::new();
        env.insert("PATH".into(), bin.as_os_str().into());
        let host = Host {
            runtime: Runtime::Flatpak,
            env,
            excluded: Vec::new(),
            bridge: bin.join("fake-spawn"),
        };
        (
            NativeTransport {
                host,
                authorization: Authorization::Polkit,
            },
            root,
        )
    }

    fn records(completion: Completion) -> Vec<serde_json::Value> {
        assert_eq!(completion.code, Some(0));
        let details: Vec<crate::package::PackageDetails> =
            serde_json::from_slice(&completion.stdout).unwrap();
        details
            .into_iter()
            .map(|details| serde_json::to_value(details).unwrap())
            .collect()
    }

    #[test]
    fn sandboxed_modes_match_native_records() {
        let (transport, root) = sandboxed_transport();
        let cancel = Cancellation::default();
        let installed = transport
            .apt_query("installed", "", "", &cancel)
            .map(records)
            .unwrap();
        assert_eq!(installed.len(), 2);
        assert_eq!(installed[0]["package"]["id"]["name"], "bash");
        assert_eq!(installed[0]["package"]["update"], "current");
        // Held packages never report an upgrade end to end.
        assert_eq!(installed[1]["package"]["id"]["name"], "oldpkg");
        assert_eq!(installed[1]["package"]["update"], "current");
        let found = transport
            .apt_query("search", "bash", "", &cancel)
            .map(records)
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["package"]["id"]["name"], "bash");
        let details = transport
            .apt_query("details", "bash", "amd64", &cancel)
            .map(records)
            .unwrap();
        assert_eq!(details.len(), 1);
        assert_eq!(details[0]["package"]["summary"], "Bourne Again SHell");
        // Unknown identities and architectures read as empty records, which
        // the backend reports as NotFound like the native helper.
        assert!(transport
            .apt_query("details", "bash", "i386", &cancel)
            .map(records)
            .unwrap()
            .is_empty());
        assert!(transport
            .apt_query("details", "ghost", "amd64", &cancel)
            .map(records)
            .unwrap()
            .is_empty());
        assert!(transport
            .apt_query("detect", "", "", &cancel)
            .map(records)
            .unwrap()
            .is_empty());
        assert!(matches!(
            transport.apt_query("erase", "", "", &cancel),
            Err(ExecutionError::Invalid(_))
        ));
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
