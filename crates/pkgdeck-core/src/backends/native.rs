//! The real system: host commands, caches and watches every source reads through.
use super::*;

pub struct NativeTransport {
    pub host: Host,
    pub authorization: Authorization,
}

pub(super) fn apt_query_executable(
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
/// sources, pins, and the machine id that phased updates use. Inside the
/// Flatpak, the host's /etc is under /run/host.
pub(super) fn apt_watches(host: &Host, copy: Option<PathBuf>) -> Vec<crate::cache::Watch> {
    use crate::cache::Watch;
    let etc = |path: &str| host.filesystem_path(std::path::Path::new(path));
    vec![
        Watch::file("/var/lib/dpkg/status"),
        Watch::tree("/var/lib/dpkg/updates", 1),
        Watch::file("/var/lib/dpkg/arch"),
        Watch::tree("/var/lib/apt/lists", 1),
        Watch::file("/var/lib/apt/extended_states"),
        Watch::tree(etc("/etc/apt"), 2),
        Watch::file(etc("/etc/machine-id")),
    ]
    .into_iter()
    .chain(copy.map(|dir| Watch::tree(dir.join("lists"), 1)))
    .collect()
}

/// What the APT helper is told: inside the Flatpak, where the host's /etc
/// is; and a check's own copy of the lists while it's newer.
pub(super) fn apt_helper_env(flatpak: bool, lists: Option<PathBuf>) -> Vec<(&'static str, String)> {
    let host = flatpak.then(|| ("PKGDECK_APT_HOST", "/run/host".to_owned()));
    let lists = lists.map(|lists| ("PKGDECK_APT_LISTS", lists.to_string_lossy().into_owned()));
    host.into_iter().chain(lists).collect()
}

/// What `flatpak search` answers from: the installation's AppStream data
/// (`appstream/<remote>/<arch>/active` flips on each refresh) and its
/// remote configuration.
pub(super) fn flatpak_search_watches(installation: &std::path::Path) -> Vec<crate::cache::Watch> {
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
pub(super) fn flatpak_installation(
    system: bool,
    var: impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
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
pub(super) fn locale_key(var: impl Fn(&str) -> Option<OsString>) -> Vec<String> {
    ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .map(|name| {
            var(name)
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect()
}

/// What `pacman -Ss` answers from below `root`: the synced databases, the
/// local database for its "[installed]" marks, and the configuration that
/// names the repositories. `None` when the configuration moves the
/// databases or reads repositories from elsewhere.
fn pacman_search_watches(root: &std::path::Path) -> Option<Vec<crate::cache::Watch>> {
    use crate::cache::Watch;
    let conf = root.join("etc/pacman.conf");
    let text = std::fs::read_to_string(&conf).unwrap_or_default();
    let moved = text.lines().any(|line| {
        let (key, value) = line.split_once('=').unwrap_or((line, ""));
        match key.trim() {
            "DBPath" | "RootDir" => true,
            // Only files right in pacman.d, which is watched one level deep.
            "Include" => value
                .trim()
                .strip_prefix("/etc/pacman.d/")
                .is_none_or(|file| file.contains('/') || file.starts_with('.')),
            _ => false,
        }
    });
    (!moved).then(|| {
        vec![
            Watch::tree(root.join("var/lib/pacman/sync"), 1),
            // Every install, upgrade or removal adds or removes a
            // "name-version" folder here, which changes the folder itself.
            Watch::file(root.join("var/lib/pacman/local")),
            Watch::file(conf),
            Watch::tree(root.join("etc/pacman.d"), 1),
            Watch::file(root.join("usr/bin/pacman")),
        ]
    })
}

impl NativeTransport {
    /// System manager commands, with pacman searches answered from `store`
    /// while the databases below `root` are unchanged.
    fn system_manager_cached(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
        write: bool,
        root: &std::path::Path,
        store: impl FnOnce() -> Option<crate::cache::Store>,
    ) -> Result<Completion, ExecutionError> {
        let run = || {
            self.host
                .system_manager(executable, args, cancel, write, self.authorization)
        };
        // A search reads every synced database and takes about a second.
        let search = !write
            && self.host.runtime.sees_host_files()
            && executable == "pacman"
            && args.first().is_some_and(|arg| arg == "-Ss");
        let Some(watches) = search.then(|| pacman_search_watches(root)).flatten() else {
            return run();
        };
        let args_text: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let args_text: Vec<&str> = args_text.iter().map(String::as_str).collect();
        crate::cache::completion(
            store().as_ref(),
            "pacman",
            "search",
            &args_text,
            &watches,
            &[],
            run,
        )
    }
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
            && self.host.runtime.sees_host_files()
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
    #[cfg(target_os = "linux")]
    fn packagekit_refresh(
        &self,
        source: &str,
        cancel: &Cancellation,
    ) -> Result<bool, ExecutionError> {
        crate::packagekit::refresh(source, cancel)
    }
    fn private_index(&self, source: &str) -> Option<PathBuf> {
        crate::private_index::fresher(source)
    }
    fn prepare_private_index(
        &self,
        source: &str,
        cancel: &Cancellation,
    ) -> Result<Option<crate::private_index::Copy>, ExecutionError> {
        crate::private_index::prepare(source, cancel)
    }
    fn refresh_index(
        &self,
        executable: &str,
        args: &[OsString],
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        self.host.refresh_index(executable, args, cancel)
    }
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
        // Inside the Flatpak the bundled helper reads the host's /var as is,
        // and its /etc under /run/host.
        let env = apt_helper_env(
            self.host.runtime == crate::host::Runtime::Flatpak,
            self.private_index("apt").map(|dir| dir.join("lists")),
        );
        let env: Vec<(&str, &str)> = env
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let executable = apt_query_executable(
            &std::env::current_exe().map_err(|e| ExecutionError::Io(e.to_string()))?,
            option_env!("PKGDECK_BUILT_APT_QUERY"),
        )?;
        // The helper opens APT's cache on every run (half a second or more).
        // Its answer only depends on the APT and dpkg databases.
        let cacheable = mode != "detect" && self.host.var("APT_CONFIG").is_none();
        let store = cacheable.then(crate::cache::Store::user).flatten();
        let helper = crate::cache::fingerprint(&[crate::cache::Watch::file(&executable)], &[]);
        let result = crate::cache::completion(
            store.as_ref(),
            "apt",
            mode,
            &[query, arch],
            &apt_watches(&self.host, crate::private_index::dir("apt")),
            &[helper.as_deref().unwrap_or_default()],
            || {
                self.host.read_bundled(
                    &executable,
                    &[mode.into(), query.into(), arch.into()],
                    &env,
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
    fn brew_cache(&self) -> Option<(crate::cache::Store, Vec<crate::cache::Watch>)> {
        self.host.brew_cache()
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
        self.system_manager_cached(
            executable,
            args,
            cancel,
            write,
            std::path::Path::new("/"),
            crate::cache::Store::user,
        )
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

#[cfg(test)]
mod tests {
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
    fn explicit_empty_scope_has_no_cleanup_tasks_or_package_sources() {
        let cancel = Cancellation::default();
        let mut engine =
            native_engine(&[NO_BACKENDS.into()], false, Authorization::Polkit, &cancel).unwrap();
        assert!(engine.cleanup(&cancel).items.is_empty());
        assert!(engine.source_ids().is_empty());
    }

    #[test]
    fn automatic_engines_skip_missing_managers_without_probing_upfront() {
        let base = temp_dir("automatic");
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let probed = base.join("probed");
        // Cargo is present; its probe leaves a mark.
        script(
            &bin.join("cargo"),
            &format!("echo >> '{}'; exit 0", probed.display()),
        );
        let host = transport(&bin, &base).host;
        let cancel = Cancellation::default();
        let mut engine =
            native_engine_on(host, &[], false, Authorization::SudoNonInteractive, &cancel).unwrap();
        assert!(!probed.exists());
        // Its own query probes it.
        engine.search_backend("cargo", "fixture", &cancel);
        assert!(probed.exists());
        // A missing manager reads as never registered, as before.
        let missing = engine.search_backend("npm", "fixture", &cancel);
        assert!(missing.successful_sources.is_empty());
        assert_eq!(
            missing.failures[0].error,
            EngineError::UnknownBackend("npm".into())
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn the_native_transport_keeps_homebrew_answers_in_the_user_cache() {
        let base = temp_dir("user-cache");
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(base.join("cache/api")).unwrap();
        script(
            &bin.join("brew"),
            &format!("echo {}", base.join("cache").display()),
        );
        let native = transport(&bin, &base);
        assert_eq!(
            native.brew_cache().is_some(),
            crate::cache::Store::user().is_some()
        );
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
    fn pacman_searches_are_answered_from_the_cache_until_its_databases_change() {
        let base = temp_dir("pacman-search");
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(base.join("var/lib/pacman/sync")).unwrap();
        std::fs::create_dir_all(base.join("var/lib/pacman/local")).unwrap();
        std::fs::create_dir_all(base.join("etc")).unwrap();
        let conf = base.join("etc/pacman.conf");
        std::fs::write(
            &conf,
            "#DBPath = /elsewhere\n[core]\nInclude = /etc/pacman.d/mirrorlist\n",
        )
        .unwrap();
        let pacman = bin.join("pacman");
        script(&pacman, "printf 'core/first 1.0-1\\n    First\\n'");
        let native = transport(&bin, &base);
        let store = || Some(crate::cache::Store::new(base.join("cache")));
        let cancel = Cancellation::default();
        let read = |args: &[&str]| {
            let args: Vec<OsString> = args.iter().map(OsString::from).collect();
            let result =
                native.system_manager_cached("pacman", &args, &cancel, false, &base, store);
            String::from_utf8(result.unwrap().stdout).unwrap()
        };
        let search = ["-Ss", "first"];
        assert_eq!(read(&search), "core/first 1.0-1\n    First\n");
        script(&pacman, "printf 'core/first 2.0-1\\n    First\\n'");
        assert_eq!(read(&search), "core/first 1.0-1\n    First\n");
        // A refresh replaced a synced database: search again.
        std::fs::write(base.join("var/lib/pacman/sync/core.db"), "").unwrap();
        assert_eq!(read(&search), "core/first 2.0-1\n    First\n");
        // An install changes the "[installed]" marks.
        script(
            &pacman,
            "printf 'core/first 2.0-1 [installed]\\n    First\\n'",
        );
        assert_eq!(read(&search), "core/first 2.0-1\n    First\n");
        std::fs::create_dir(base.join("var/lib/pacman/local/first-2.0-1")).unwrap();
        assert_eq!(read(&search), "core/first 2.0-1 [installed]\n    First\n");
        // Other reads always run.
        let entries = || std::fs::read_dir(base.join("cache")).unwrap().count();
        let cached = entries();
        script(&pacman, "printf 'other\\n'");
        assert_eq!(read(&["-Q"]), "other\n");
        assert_eq!(entries(), cached);
        // Databases or repositories configured elsewhere are not watched.
        for line in [
            "DBPath = /srv/pacman/",
            "Include = /srv/repos.conf",
            "Include = /etc/pacman.d/repos/extra.conf",
            "Include = /etc/pacman.d/..",
        ] {
            std::fs::write(&conf, format!("[options]\n{line}\n")).unwrap();
            assert!(pacman_search_watches(&base).is_none());
            assert_eq!(read(&["-Ss", "other"]), "other\n");
            assert_eq!(entries(), cached);
        }
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

    #[test]
    fn apt_details_after_a_lookup_reuse_what_it_read_until_a_change() {
        /// Answers every query with the same rows, described by the mode
        /// that read them, and logs each helper run.
        #[derive(Clone, Default)]
        struct Helper(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
        fn done(stdout: Vec<u8>) -> Completion {
            Completion {
                code: Some(0),
                signal: None,
                stdout,
                stderr: vec![],
                truncated: false,
                cancellation_deferred: false,
            }
        }
        impl Transport for Helper {
            fn apt_query(
                &self,
                mode: &str,
                query: &str,
                arch: &str,
                _: &Cancellation,
            ) -> Result<Completion, ExecutionError> {
                self.0
                    .lock()
                    .unwrap()
                    .push(format!("{mode} {query} {arch}"));
                let rows: Vec<_> = ["amd64", "i386"]
                    .map(|arch| {
                        serde_json::json!({
                            "package": {
                                "id": {
                                    "backend": "apt", "name": "tool", "architecture": arch,
                                    "scope": "system", "remote": null, "reference": null,
                                },
                                "display_name": "tool", "summary": "A tool",
                                "installed_version": "1.0", "candidate_version": "1.0",
                                "update": "current", "icon": null, "component_ids": [], "homepages": [],
                            },
                            "description": format!("A tool, read by {mode}"),
                            "homepage": "https://example.invalid", "dependencies": [],
                        })
                    })
                    .into();
                Ok(done(serde_json::to_vec(&rows).unwrap()))
            }
            fn apt_write(
                &self,
                _: AptAction,
                _: &Cancellation,
            ) -> Result<Completion, ExecutionError> {
                self.0.lock().unwrap().push("write".into());
                Ok(done(vec![]))
            }
            fn apt_write_group(
                &self,
                _: &[AptAction],
                _: &Cancellation,
            ) -> Result<Completion, ExecutionError> {
                self.0.lock().unwrap().push("write group".into());
                Ok(done(vec![]))
            }
        }
        let base = temp_dir("apt-lookup");
        let icon = base.join("tool.png");
        std::fs::write(&icon, "png").unwrap();
        let desktop = base.join("tool.desktop");
        std::fs::write(
            &desktop,
            format!("[Desktop Entry]\nIcon={}\n", icon.display()),
        )
        .unwrap();
        let helper = Helper::default();
        let log = || std::mem::take(&mut *helper.0.lock().unwrap());
        let mut apt = Apt::new(helper.clone());
        apt.desktop_entries = Some([("tool".to_string(), desktop)].into());
        apt.components = Some([("tool".to_string(), vec!["org.example.Tool".into()])].into());
        let cancel = Cancellation::default();
        // One helper run finds every architecture of the exact name.
        let found = apt.lookup("tool", &cancel).unwrap();
        assert_eq!(log(), ["lookup tool "]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].icon.as_ref(), Some(&icon));
        let (amd64, i386) = (found[0].id.clone(), found[1].id.clone());
        // `pkd info`: details come from the lookup, dressed like a fresh read.
        let details = apt.details(&amd64, &cancel).unwrap();
        assert!(log().is_empty());
        assert_eq!(details.description, "A tool, read by lookup");
        assert_eq!(details.package.icon.as_ref(), Some(&icon));
        assert_eq!(details.package.component_ids, ["org.example.Tool"]);
        assert_eq!(details.package.homepages, ["https://example.invalid"]);
        // Read once: the next details ask the helper.
        let details = apt.details(&i386, &cancel).unwrap();
        assert_eq!(log(), ["details tool i386"]);
        assert_eq!(details.description, "A tool, read by details");
        // A change, alone or grouped, clears what the lookup read.
        apt.lookup("tool", &cancel).unwrap();
        apt.execute(&Operation::Remove(amd64.clone()), &cancel, &mut |_| {})
            .unwrap();
        apt.details(&amd64, &cancel).unwrap();
        assert_eq!(log(), ["lookup tool ", "write", "details tool amd64"]);
        apt.lookup("tool", &cancel).unwrap();
        apt.execute_group(
            &[Operation::Remove(amd64.clone()), Operation::Remove(i386)],
            &cancel,
            &mut |_| {},
        )
        .unwrap()
        .unwrap();
        apt.details(&amd64, &cancel).unwrap();
        assert_eq!(log(), ["lookup tool ", "write group", "details tool amd64"]);
        // Names APT can't have are never sent, and leave nothing behind.
        apt.lookup("tool", &cancel).unwrap();
        for name in ["Tool Name", "tool:amd64", "-o"] {
            assert!(apt.lookup(name, &cancel).unwrap().is_empty());
        }
        apt.details(&amd64, &cancel).unwrap();
        assert_eq!(log(), ["lookup tool ", "details tool amd64"]);
        std::fs::remove_dir_all(base).unwrap();
    }
}
