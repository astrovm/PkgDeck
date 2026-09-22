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

impl<T: Transport> Flatpak<T> {
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
                    summary: format!(
                        "{} unused runtimes and extensions; applications and their data are kept",
                        entries.len()
                    ),
                    preview,
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
        let (present, preview) = match self.kind {
            DevKind::Uv => {
                let size: u64 = output
                    .trim()
                    .parse()
                    .map_err(|error| invalid("uv", error))?;
                (size > 0, format!("{size} bytes in the uv cache. Removes cached downloads and builds; installed tools and environments are kept."))
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
                (
                    counts.into_iter().any(|count| count.unwrap_or(0) > 0),
                    output,
                )
            }
            _ => (!output.trim().is_empty(), output),
        };
        Ok(if present {
            vec![CleanupItem {
            id: CleanupId { backend: self.kind.id().into(), key: key.into() },
            kind: CleanupKind::PackageCache,
            title: title.into(),
            summary: "Removes cached downloads; installed packages are kept. Future installs may download again.".into(),
            preview,
        }]
        } else {
            vec![]
        })
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
    pub(super) fn cleanup_apt_report(
        &self,
        cancel: &Cancellation,
        authenticated: bool,
    ) -> CleanupReport {
        let mut report = CleanupReport::default();
        for key in ["autoremove", "autoclean"] {
            // Unauthenticated `autoclean` always fails for unprivileged users
            // (`/var/cache/apt/archives/partial` is `_apt:root`), which would
            // bury the actionable `autoremove` plan under a permission error.
            // Skip the cache probe until the explicit authenticated "Check APT"
            // preview; it runs the same `--simulate` through the auth boundary.
            if key == "autoclean" && !authenticated {
                continue;
            }
            match self.cleanup_apt_task(key, cancel, authenticated && key == "autoclean") {
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
        authenticated: bool,
    ) -> Result<Option<CleanupItem>, EngineError> {
        let (args, title, kind): (&[&str], _, _) = match key {
            "autoremove" => (
                &[
                    "--simulate",
                    "-o",
                    "Debug::NoLocking=1",
                    "--purge",
                    "autoremove",
                ],
                "Unused dependencies",
                CleanupKind::OrphanDependencies,
            ),
            "autoclean" => (
                &["--simulate", "-o", "Debug::NoLocking=1", "autoclean"],
                "Obsolete package downloads",
                CleanupKind::PackageCache,
            ),
            _ => return Err(invalid("apt", "unknown cleanup task")),
        };
        // Authentication is only used by an explicit preview request or an already
        // confirmed cleanup. Both paths retain --simulate: no discovery mutates APT.
        let output = String::from_utf8(bytes(
            "apt",
            self.transport.system_manager(
                "apt-get",
                &args.iter().map(OsString::from).collect::<Vec<_>>(),
                cancel,
                authenticated,
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
            kind,
            title: title.into(),
            summary: if key == "autoremove" {
                format!(
                    "APT can remove {} unused package entries and leftover configuration",
                    actionable.len()
                )
            } else {
                format!("APT can remove {} cached package files", actionable.len())
            },
            preview: actionable.join("\n"),
        }))
    }
}
