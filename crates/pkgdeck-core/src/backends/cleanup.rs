//! Read-only native cleanup previews. Execution only accepts fixed task keys.
use super::*;

pub(super) const DEV_CLEAN_CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Upgrade,
    Capability::Clean,
];

#[derive(Deserialize)]
struct UnusedRef {
    scope: String,
    reference: String,
    bytes: u64,
}

impl<T: Transport + Sync> Flatpak<T> {
    pub(super) fn cleanup_unused(
        &self,
        cancel: &Cancellation,
    ) -> Result<Vec<CleanupItem>, EngineError> {
        let refs: Vec<UnusedRef> =
            serde_json::from_slice(&bytes("flatpak", self.transport.flatpak_unused(cancel)?)?)
                .map_err(|error| invalid("flatpak", error))?;
        if refs.iter().any(|entry| {
            !matches!(entry.scope.as_str(), "user" | "system")
                || !flatpak_reference(&entry.reference)
                    .is_some_and(|(kind, _, _, _)| kind == "runtime")
        }) {
            return Err(invalid("flatpak", "invalid unused runtime inventory"));
        }
        Ok(["user", "system"]
            .into_iter()
            .filter_map(|scope| {
                let entries: Vec<_> = refs.iter().filter(|entry| entry.scope == scope).collect();
                if entries.is_empty() {
                    return None;
                }
                let preview = entries
                    .iter()
                    .map(|entry| format!("{} ({} bytes installed)", entry.reference, entry.bytes))
                    .collect::<Vec<_>>()
                    .join("\n");
                Some(CleanupItem {
                    id: CleanupId {
                        backend: "flatpak".into(),
                        key: format!("unused-{scope}"),
                    },
                    kind: CleanupKind::OrphanDependencies,
                    title: format!("Unused Flatpak runtimes ({scope})"),
                    summary: format!("{} unused runtimes and extensions", entries.len()),
                    preview,
                    bytes: Some(entries.iter().map(|entry| entry.bytes).sum()),
                    restorable: false,
                })
            })
            .collect())
    }

    pub(super) fn clean_unused(
        &self,
        id: &CleanupId,
        cancel: &Cancellation,
    ) -> Result<OperationOutcome, EngineError> {
        let system = match (id.backend.as_str(), id.key.as_str()) {
            ("flatpak", "unused-user") => false,
            ("flatpak", "unused-system") => true,
            _ => return Err(invalid("flatpak", "unknown cleanup task")),
        };
        // Recompute the native dependency/pinning decision immediately before writing.
        if !self
            .cleanup_unused(cancel)?
            .iter()
            .any(|item| item.id == *id)
        {
            return Err(EngineError::NotFound);
        }
        let scope = if system { "--system" } else { "--user" };
        let result = self.call(
            &[
                scope,
                "uninstall",
                "--unused",
                "--noninteractive",
                "--assumeyes",
            ],
            cancel,
            true,
            system,
        )?;
        bytes("flatpak", result.clone())?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}

impl<T: Transport> DevTool<T> {
    fn cache_call(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, EngineError> {
        if self.kind == DevKind::Pip {
            let home = self
                .home
                .as_ref()
                .ok_or_else(|| invalid("pip", "manager home not detected"))?;
            Ok(self.pip_call(home, args, cancel, write)?)
        } else {
            Ok(self.call(args, cancel, write)?)
        }
    }

    pub(super) fn cleanup_cache(
        &self,
        cancel: &Cancellation,
    ) -> Result<Vec<CleanupItem>, EngineError> {
        let (args, key, title): (&[&str], _, _) = match self.kind {
            DevKind::Npm => (&["cache", "ls"], "cache", "npm package cache"),
            DevKind::Pip => (&["cache", "info"], "cache", "pip package cache"),
            DevKind::Uv => (
                &["cache", "size", "--output-format", "machine"],
                "cache",
                "uv package cache",
            ),
            _ => return Err(self.unsupported(Capability::Clean)),
        };
        let output = String::from_utf8(bytes(
            self.kind.id(),
            self.cache_call(args, cancel, false)?,
        )?)
        .map_err(|error| invalid(self.kind.id(), error))?;
        let (present, preview, size) = match self.kind {
            DevKind::Uv => {
                let size: u64 = output
                    .trim()
                    .parse()
                    .map_err(|error| invalid("uv", error))?;
                (size > 0, "Everything uv has cached".into(), Some(size))
            }
            DevKind::Pip => {
                let counts: Vec<_> = output
                    .lines()
                    .filter_map(|line| line.strip_prefix("Number of "))
                    .filter_map(|line| line.split_once(": "))
                    .map(|(_, value)| value.trim().parse::<u64>())
                    .collect();
                if counts.len() != 2 || counts.iter().any(Result::is_err) {
                    return Err(invalid("pip", "unrecognized cache inventory"));
                }
                // "Package index page cache size: 36.3 MB", "Wheels size: 10 kB".
                let size = output
                    .lines()
                    .filter_map(|line| line.split_once("size: "))
                    .map(|(_, size)| crate::disk::parse_size(size))
                    .sum();
                (
                    counts.into_iter().any(|count| count.unwrap_or(0) > 0),
                    output,
                    size,
                )
            }
            _ => {
                let size = self.npm_cache_usage(cancel);
                (!output.trim().is_empty(), output, size)
            }
        };
        Ok(if present {
            vec![CleanupItem {
                id: CleanupId {
                    backend: self.kind.id().into(),
                    key: key.into(),
                },
                kind: CleanupKind::PackageCache,
                title: title.into(),
                summary: "Clear cached downloads".into(),
                preview,
                bytes: size,
                restorable: false,
            }]
        } else {
            vec![]
        })
    }

    /// The space npm's cache takes: `npm cache clean` empties `_cacache`
    /// in the folder `npm config get cache` names.
    fn npm_cache_usage(&self, cancel: &Cancellation) -> Option<u64> {
        let output = bytes(
            "npm",
            self.cache_call(&["config", "get", "cache"], cancel, false)
                .ok()?,
        )
        .ok()?;
        let dir = PathBuf::from(String::from_utf8(output).ok()?.trim());
        dir.is_absolute()
            .then(|| crate::disk::usage(&dir.join("_cacache"), cancel))
    }

    pub(super) fn clean_cache(
        &self,
        id: &CleanupId,
        cancel: &Cancellation,
    ) -> Result<OperationOutcome, EngineError> {
        if id.backend != self.kind.id() || id.key != "cache" {
            return Err(invalid(self.kind.id(), "unknown cleanup task"));
        }
        let args: &[&str] = match self.kind {
            DevKind::Npm => &["cache", "clean", "--force"],
            DevKind::Pip => &["cache", "purge"],
            DevKind::Uv => &["cache", "clean"],
            _ => return Err(self.unsupported(Capability::Clean)),
        };
        if self.cleanup_cache(cancel)?.is_empty() {
            return Err(EngineError::NotFound);
        }
        let result = self.cache_call(args, cancel, true)?;
        bytes(self.kind.id(), result.clone())?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}

impl<T: Transport> Apt<T> {
    pub(super) fn cleanup_apt_report(&self, cancel: &Cancellation) -> CleanupReport {
        let mut report = CleanupReport::default();
        for key in ["autoremove", "clean"] {
            match self.cleanup_apt_task(key, cancel) {
                Ok(Some(item)) => report.items.push(item),
                Ok(None) => {}
                Err(error) => report.failures.push(BackendFailure {
                    backend: "apt".into(),
                    error,
                }),
            }
        }
        report
    }

    pub(super) fn cleanup_apt_task(
        &self,
        key: &str,
        cancel: &Cancellation,
    ) -> Result<Option<CleanupItem>, EngineError> {
        if key == "clean" {
            return self.cleanup_apt_cache(cancel);
        }
        if key != "autoremove" {
            return Err(invalid("apt", "unknown cleanup task"));
        }
        let args = [
            "--simulate",
            "-o",
            "Debug::NoLocking=1",
            "--purge",
            "autoremove",
        ];
        let output = String::from_utf8(bytes(
            "apt",
            self.transport.system_manager(
                "apt-get",
                &args.iter().map(OsString::from).collect::<Vec<_>>(),
                cancel,
                false,
            )?,
        )?)
        .map_err(|error| invalid("apt", error))?;
        let actionable: Vec<_> = output
            .lines()
            .filter(|line| {
                line.starts_with("Remv ") || line.starts_with("Purg ") || line.starts_with("Del ")
            })
            .map(str::trim)
            .collect();
        if actionable.is_empty() {
            return Ok(None);
        }
        Ok(Some(CleanupItem {
            id: CleanupId {
                backend: "apt".into(),
                key: key.into(),
            },
            kind: CleanupKind::OrphanDependencies,
            title: "Unused dependencies".into(),
            summary: format!("{} removable entries", actionable.len()),
            bytes: self.installed_size(&actionable, cancel),
            preview: actionable.join("\n"),
            restorable: false,
        }))
    }

    /// What the packages on "Remv name [version]" lines take installed,
    /// as dpkg recorded it in KiB.
    fn installed_size(&self, lines: &[&str], cancel: &Cancellation) -> Option<u64> {
        let mut args: Vec<OsString> =
            vec!["-W".into(), "-f=${Installed-Size}\\n".into(), "--".into()];
        args.extend(
            lines
                .iter()
                .filter_map(|line| line.split_whitespace().nth(1))
                .map(OsString::from),
        );
        let output = self
            .transport
            .system_manager("dpkg-query", &args, cancel, false)
            .ok()?;
        let text = String::from_utf8(bytes("apt", output).ok()?).ok()?;
        text.lines()
            .map(|line| line.trim().parse::<u64>().ok().map(|kib| kib * 1024))
            .sum()
    }

    fn cleanup_apt_cache(&self, cancel: &Cancellation) -> Result<Option<CleanupItem>, EngineError> {
        // `apt-get clean` empties the download cache, so what it frees is
        // the size of the downloads there. They are readable without a
        // password; only archives/partial is root's.
        let config = String::from_utf8(bytes(
            "apt",
            self.transport.system_manager(
                "apt-config",
                &[
                    "shell",
                    "ROOT",
                    "Dir",
                    "CACHE",
                    "Dir::Cache",
                    "ARCHIVES",
                    "Dir::Cache::archives",
                ]
                .map(OsString::from),
                cancel,
                false,
            )?,
        )?)
        .map_err(|error| invalid("apt", error))?;
        let value = |key: &str| {
            config
                .lines()
                .find_map(|line| {
                    line.strip_prefix(&format!("{key}='"))
                        .and_then(|value| value.strip_suffix('\''))
                })
                .filter(|value| !value.is_empty() && !value.contains('\''))
                .ok_or_else(|| invalid("apt", format!("missing {key} cache path")))
        };
        let archives = PathBuf::from(value("ROOT")?)
            .join(value("CACHE")?)
            .join(value("ARCHIVES")?);
        let entries = bytes(
            "apt",
            self.transport.system_manager(
                "find",
                &[
                    archives.into_os_string(),
                    "-maxdepth".into(),
                    "1".into(),
                    "-type".into(),
                    "f".into(),
                    "-name".into(),
                    "*.deb".into(),
                    "-printf".into(),
                    "%s\\n".into(),
                ],
                cancel,
                false,
            )?,
        )?;
        let sizes = String::from_utf8(entries)
            .map_err(|error| invalid("apt", error))?
            .lines()
            .map(|line| line.trim().parse::<u64>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| invalid("apt", error))?;
        if sizes.is_empty() {
            return Ok(None);
        }
        let count = sizes.len();
        Ok(Some(CleanupItem {
            id: CleanupId {
                backend: "apt".into(),
                key: "clean".into(),
            },
            kind: CleanupKind::PackageCache,
            title: "APT download cache".into(),
            summary: format!(
                "{count} cached download{}",
                if count == 1 { "" } else { "s" }
            ),
            preview: "Deletes every cached download. A package you reinstall downloads again."
                .into(),
            bytes: Some(sizes.iter().sum()),
            restorable: false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers commands whose line starts with a known prefix; any other
    /// command is not found.
    struct Fake(Vec<(&'static str, &'static [u8])>);

    impl Fake {
        fn answer(&self, executable: &str, args: &[OsString]) -> Result<Completion, ExecutionError> {
            let line = std::iter::once(executable.to_owned())
                .chain(args.iter().map(|arg| arg.to_string_lossy().into_owned()))
                .collect::<Vec<_>>()
                .join(" ");
            let (_, output) = self
                .0
                .iter()
                .find(|(prefix, _)| line.starts_with(prefix))
                .ok_or_else(|| ExecutionError::Disabled(format!("{line}: not found")))?;
            Ok(Completion {
                code: Some(0),
                signal: None,
                stdout: output.to_vec(),
                stderr: vec![],
                truncated: false,
                cancellation_deferred: false,
            })
        }
    }

    impl Transport for Fake {
        fn dev_tool(
            &self,
            executable: &str,
            args: &[OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            self.answer(executable, args)
        }
        fn venv_pip(
            &self,
            _: &std::path::Path,
            args: &[OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            self.answer("pip", args)
        }
        fn system_manager(
            &self,
            executable: &str,
            args: &[OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            self.answer(executable, args)
        }
    }

    fn reason(error: EngineError) -> String {
        error.to_string()
    }

    #[test]
    fn pip_cache_needs_a_virtual_environment() {
        let error = DevTool::pip(Fake(vec![]))
            .cleanup_cache(&Cancellation::default())
            .unwrap_err();
        assert!(reason(error).contains("manager home not detected"));
    }

    #[test]
    fn pip_cache_size_adds_every_size_line() {
        let mut pip = DevTool::pip(Fake(vec![(
            "pip cache info",
            b"Package index page cache size: 2 MB\nNumber of HTTP files: 3\nWheels size: 10 kB\nNumber of locally built wheels: 0\n",
        )]));
        pip.home = Some(PathBuf::from("/venv"));
        let items = pip.cleanup_cache(&Cancellation::default()).unwrap();
        assert_eq!(items[0].bytes, Some(2_010_000));
    }

    #[test]
    fn cache_inventories_must_be_text() {
        let cancel = Cancellation::default();
        let error = DevTool::uv(Fake(vec![("uv cache size", b"\xff")]))
            .cleanup_cache(&cancel)
            .unwrap_err();
        assert!(reason(error).contains("invalid utf-8"));
        for (command, key) in [
            ("apt-get --simulate", "autoremove"),
            ("apt-config shell", "clean"),
        ] {
            let error = Apt::new(Fake(vec![(command, b"\xff")]))
                .cleanup_apt_task(key, &cancel)
                .unwrap_err();
            assert!(reason(error).contains("invalid utf-8"), "{command}");
        }
        let error = Apt::new(Fake(vec![
            (
                "apt-config shell",
                b"ROOT='/'\nCACHE='var/cache/apt/'\nARCHIVES='archives/'\n",
            ),
            ("find /var/cache/apt/archives", b"\xff"),
        ]))
        .cleanup_apt_task("clean", &cancel)
        .unwrap_err();
        assert!(reason(error).contains("invalid utf-8"));
    }

    #[test]
    fn npm_cache_size_is_its_cacache_folder() {
        let cancel = Cancellation::default();
        let dir = std::env::temp_dir().join(format!("pkgdeck-npm-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("_cacache")).unwrap();
        std::fs::write(dir.join("_cacache/entry"), [0_u8; 4096]).unwrap();
        let config = format!("{}\n", dir.display()).leak().as_bytes();
        let items = DevTool::npm(Fake(vec![
            ("npm cache ls", b"make-fetch-happen:request-cache:x\n"),
            ("npm config get cache", config),
        ]))
        .cleanup_cache(&cancel)
        .unwrap();
        assert!(items[0].bytes.is_some_and(|bytes| bytes >= 4096), "{items:?}");
        // Without the folder the size is unknown, not zero.
        let items = DevTool::npm(Fake(vec![(
            "npm cache ls",
            b"make-fetch-happen:request-cache:x\n",
        )]))
        .cleanup_cache(&cancel)
        .unwrap();
        assert_eq!(items[0].bytes, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
