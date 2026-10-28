//! Caches developer tools keep in the home folder and fill again on their
//! own: Cargo's downloaded crates, Go's build and module caches, Gradle,
//! Maven, Yarn, Bun, sccache, CocoaPods and Xcode's build data. None of
//! these tools has a command that clears them, so PkgDeck empties the
//! folders itself, and never while a build that uses one is running.
use super::*;
use crate::{disk, hold::Toolchain, host::Host};
use std::path::Path;

const CAPABILITIES: &[Capability] = &[Capability::Clean];
/// Smaller caches aren't worth a line.
const WORTH_CLEANING: u64 = 1024 * 1024;

pub struct DevCaches {
    host: Host,
}

/// One cache: the folders it lives in and what clearing it means.
struct Cache {
    key: &'static str,
    title: &'static str,
    kind: CleanupKind,
    /// What comes back, and how.
    summary: &'static str,
    folders: Vec<PathBuf>,
    /// The builds that read or write it, so emptying it waits for them.
    builds: &'static [Toolchain],
}

impl DevCaches {
    pub fn new(host: Host) -> Self {
        Self { host }
    }

    fn caches(&self) -> Vec<Cache> {
        let var = |name: &str| {
            self.host
                .var(name)
                .and_then(|value| std::env::split_paths(&value).next())
                .filter(|path| path.is_absolute())
        };
        let Some(home) = var("HOME") else {
            return vec![];
        };
        let mac = cfg!(target_os = "macos");
        let caches = if mac {
            home.join("Library/Caches")
        } else {
            var("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache"))
        };
        let cargo = var("CARGO_HOME").unwrap_or_else(|| home.join(".cargo"));
        let gopath = var("GOPATH").unwrap_or_else(|| home.join("go"));
        let gradle = var("GRADLE_USER_HOME").unwrap_or_else(|| home.join(".gradle"));
        let mut all = vec![
            Cache {
                key: "cargo",
                title: "Cargo downloads",
                kind: CleanupKind::PackageCache,
                summary: "Cargo downloads crates again when a build needs them",
                folders: vec![cargo.join("registry"), cargo.join("git")],
                builds: &[Toolchain::Rust],
            },
            Cache {
                key: "go-build",
                title: "Go build cache",
                kind: CleanupKind::PackageCache,
                summary: "Go builds take longer until it fills again",
                folders: vec![var("GOCACHE").unwrap_or_else(|| caches.join("go-build"))],
                builds: &[Toolchain::Go],
            },
            Cache {
                key: "go-modules",
                title: "Go module downloads",
                kind: CleanupKind::PackageCache,
                summary: "Go downloads modules again when a build needs them",
                folders: vec![var("GOMODCACHE").unwrap_or_else(|| gopath.join("pkg/mod"))],
                builds: &[Toolchain::Go],
            },
            Cache {
                key: "gradle",
                title: "Gradle caches",
                kind: CleanupKind::PackageCache,
                summary: "Gradle downloads dependencies and versions of itself again when a build needs them",
                folders: vec![gradle.join("caches"), gradle.join("wrapper/dists")],
                builds: &[Toolchain::Gradle],
            },
            Cache {
                key: "maven",
                title: "Maven downloads",
                kind: CleanupKind::PackageCache,
                summary: "Maven downloads dependencies again. What you put there with mvn install comes back by building it again",
                folders: vec![home.join(".m2/repository")],
                builds: &[Toolchain::Maven],
            },
            Cache {
                key: "yarn",
                title: "Yarn cache",
                kind: CleanupKind::PackageCache,
                summary: "Yarn downloads packages again when an install needs them",
                folders: vec![
                    var("YARN_CACHE_FOLDER")
                        .unwrap_or_else(|| caches.join(if mac { "Yarn" } else { "yarn" })),
                    home.join(".yarn/berry/cache"),
                ],
                // Installs aren't builds, so nothing being built uses it.
                builds: &[],
            },
            Cache {
                key: "bun",
                title: "Bun cache",
                kind: CleanupKind::PackageCache,
                summary: "Bun downloads packages again when an install needs them",
                folders: vec![var("BUN_INSTALL")
                    .unwrap_or_else(|| home.join(".bun"))
                    .join("install/cache")],
                builds: &[],
            },
            Cache {
                key: "sccache",
                title: "sccache compiler cache",
                kind: CleanupKind::PackageCache,
                summary: "Builds take longer until it fills again",
                folders: vec![var("SCCACHE_DIR").unwrap_or_else(|| {
                    caches.join(if mac { "Mozilla.sccache" } else { "sccache" })
                })],
                builds: &[Toolchain::Rust, Toolchain::Native],
            },
        ];
        if mac {
            let xcode = home.join("Library/Developer/Xcode");
            all.extend([
                Cache {
                    key: "cocoapods",
                    title: "CocoaPods cache",
                    kind: CleanupKind::PackageCache,
                    summary: "CocoaPods downloads pods again when an install needs them",
                    folders: vec![caches.join("CocoaPods")],
                    builds: &[Toolchain::Apple],
                },
                Cache {
                    key: "xcode-derived-data",
                    title: "Xcode build data",
                    kind: CleanupKind::BuildFiles,
                    summary: "Xcode builds each project from scratch the next time",
                    folders: vec![xcode.join("DerivedData")],
                    builds: &[Toolchain::Apple],
                },
                Cache {
                    key: "xcode-device-support",
                    title: "Xcode device support files",
                    kind: CleanupKind::Unused,
                    summary: "Xcode copies them again from a device when you connect it",
                    folders: ["iOS", "watchOS", "tvOS", "visionOS"]
                        .iter()
                        .map(|os| xcode.join(format!("{os} DeviceSupport")))
                        .collect(),
                    builds: &[Toolchain::Apple],
                },
            ]);
        }
        all
    }

    /// The task for `cache`, when it has enough in it to be worth one.
    fn item(&self, cache: &Cache, cancel: &Cancellation) -> Option<CleanupItem> {
        let folders: Vec<_> = cache
            .folders
            .iter()
            .filter(|folder| folder.is_dir())
            .collect();
        let bytes: u64 = folders
            .iter()
            .filter_map(|folder| std::fs::canonicalize(folder).ok())
            .map(|folder| disk::usage(&folder, cancel))
            .sum();
        (bytes >= WORTH_CLEANING).then(|| CleanupItem {
            id: CleanupId {
                backend: "dev-caches".into(),
                key: cache.key.into(),
            },
            kind: cache.kind.clone(),
            title: cache.title.into(),
            summary: cache.summary.into(),
            preview: format!(
                "Empties these folders:\n{}",
                folders
                    .iter()
                    .map(|folder| shown(&self.host, folder))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            bytes: Some(bytes),
        })
    }
}

/// `path` with the home folder written as `~`.
pub(super) fn shown(host: &Host, path: &Path) -> String {
    match host
        .var("HOME")
        .and_then(|home| path.strip_prefix(home).ok().map(Path::to_owned))
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Refuse to delete what a running build may be reading or writing: one
/// with a toolchain in `uses`.
pub(super) fn not_while_building(
    host: &Host,
    backend: &str,
    uses: &[Toolchain],
) -> Result<(), EngineError> {
    if crate::hold::builds(host)
        .iter()
        .any(|toolchain| uses.contains(toolchain))
    {
        return Err(EngineError::Refused {
            backend: backend.into(),
            refusal: Refusal::Building,
        });
    }
    Ok(())
}

impl Backend for DevCaches {
    fn id(&self) -> &str {
        "dev-caches"
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, _cancel: &Cancellation) -> Result<Availability, EngineError> {
        Ok(if self.host.var("HOME").is_some() {
            Availability::Available
        } else {
            Availability::Unavailable("PkgDeck can't tell where your home folder is.".into())
        })
    }
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        // Measuring walks every file, so the caches are measured side by side.
        let caches = self.caches();
        let items = std::thread::scope(|scope| {
            let workers: Vec<_> = caches
                .iter()
                .map(|cache| scope.spawn(|| self.item(cache, cancel)))
                .collect();
            workers
                .into_iter()
                .filter_map(|worker| worker.join().expect("cache measuring panicked"))
                .collect()
        });
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        Ok(items)
    }
    fn cleanup_plan(
        &mut self,
        id: &CleanupId,
        cancel: &Cancellation,
    ) -> Result<CleanupItem, EngineError> {
        self.caches()
            .iter()
            .filter(|cache| id.backend == "dev-caches" && cache.key == id.key)
            .find_map(|cache| self.item(cache, cancel))
            .ok_or(EngineError::NotFound)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let Operation::Clean(id) = operation else {
            return Err(self.unsupported(operation.capability()));
        };
        let cache = self
            .caches()
            .into_iter()
            .find(|cache| id.backend == "dev-caches" && cache.key == id.key)
            .ok_or(EngineError::NotFound)?;
        not_while_building(&self.host, self.id(), cache.builds)?;
        progress(Progress::Message(format!("Emptying the {}…", cache.title)));
        for folder in &cache.folders {
            if cancel.requested() {
                return Err(EngineError::Cancelled);
            }
            let Ok(real) = std::fs::canonicalize(folder) else {
                continue;
            };
            disk::empty(&real).map_err(|error| {
                // Named as the preview names it, not by where a link leads.
                EngineError::Execution(ExecutionError::Io(format!(
                    "Couldn't empty {}: {error}",
                    shown(&self.host, folder)
                )))
            })?;
        }
        Ok(OperationOutcome {
            cancellation_deferred: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Runtime;
    use std::{collections::BTreeMap, fs};

    fn home(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("pkgdeck-caches-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }
    /// A machine whose home is `home`, and where nothing is being built:
    /// without `ps` on its PATH, nothing counts as a build.
    fn caches_in(home: &Path, extra: &[(&str, &Path)]) -> DevCaches {
        let mut env = BTreeMap::from([
            ("HOME".into(), home.as_os_str().to_owned()),
            ("PATH".into(), "/nonexistent/pkgdeck".into()),
        ]);
        for (name, value) in extra {
            env.insert(name.into(), value.as_os_str().to_owned());
        }
        DevCaches::new(Host::new(Runtime::Native, env))
    }
    fn fill(dir: &Path, bytes: usize) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("blob"), vec![7; bytes]).unwrap();
    }

    #[test]
    fn full_caches_are_offered_with_their_size_and_emptied_in_place() {
        let root = home("offer");
        let cargo = root.join("cargo-home");
        fill(&cargo.join("registry/cache"), 2 * 1024 * 1024);
        fill(&cargo.join("git/db"), 512 * 1024);
        // Too little to be worth a line.
        fill(&root.join(".bun/install/cache"), 1024);
        let mut backend = caches_in(&root, &[("CARGO_HOME", &cargo)]);
        let cancel = Cancellation::default();
        assert_eq!(backend.detect(&cancel).unwrap(), Availability::Available);
        let items = backend.cleanup(&cancel).unwrap();
        assert_eq!(items.len(), 1, "{items:?}");
        let cargo_item = &items[0];
        assert_eq!(cargo_item.id.key, "cargo");
        assert_eq!(
            cargo_item.preview,
            "Empties these folders:\n~/cargo-home/registry\n~/cargo-home/git"
        );
        assert!(cargo_item.bytes.unwrap() >= 2 * 1024 * 1024 + 512 * 1024);
        // The plan checked right before cleaning is the same one.
        assert_eq!(
            backend.cleanup_plan(&cargo_item.id, &cancel).unwrap(),
            *cargo_item
        );
        // A folder gone since the review is skipped.
        fs::remove_dir_all(cargo.join("git")).unwrap();
        let mut said = vec![];
        backend
            .execute(
                &Operation::Clean(cargo_item.id.clone()),
                &cancel,
                &mut |progress| said.push(progress),
            )
            .unwrap();
        assert_eq!(
            said,
            [Progress::Message("Emptying the Cargo downloads…".into())]
        );
        assert!(cargo.join("registry").is_dir() && !cargo.join("git").exists());
        assert_eq!(fs::read_dir(cargo.join("registry")).unwrap().count(), 0);
        assert!(cargo.exists());
        // Emptied, it isn't offered any more.
        assert!(matches!(
            backend.cleanup_plan(&cargo_item.id, &cancel),
            Err(EngineError::NotFound)
        ));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn every_cache_lives_where_its_tool_keeps_it() {
        let root = home("places");
        let backend = caches_in(&root, &[]);
        let folders: BTreeMap<_, _> = backend
            .caches()
            .into_iter()
            .map(|cache| {
                (
                    cache.key,
                    cache
                        .folders
                        .iter()
                        .map(|folder| shown(&backend.host, folder))
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let caches = if cfg!(target_os = "macos") {
            "~/Library/Caches"
        } else {
            "~/.cache"
        };
        assert_eq!(folders["cargo"], ["~/.cargo/registry", "~/.cargo/git"]);
        assert_eq!(folders["go-build"], [format!("{caches}/go-build")]);
        assert_eq!(folders["go-modules"], ["~/go/pkg/mod"]);
        assert_eq!(
            folders["gradle"],
            ["~/.gradle/caches", "~/.gradle/wrapper/dists"]
        );
        assert_eq!(folders["maven"], ["~/.m2/repository"]);
        assert_eq!(folders["bun"], ["~/.bun/install/cache"]);
        assert_eq!(folders["yarn"][1], "~/.yarn/berry/cache");
        assert!(folders["sccache"][0].starts_with(caches));
        assert_eq!(
            folders.contains_key("xcode-derived-data"),
            cfg!(target_os = "macos")
        );
        // Tool settings move them; a relative one is ignored.
        let moved = caches_in(
            &root,
            &[
                ("GOCACHE", Path::new("/tmp/gocache")),
                ("GOMODCACHE", Path::new("relative")),
                ("SCCACHE_DIR", Path::new("/tmp/sccache")),
            ],
        );
        let caches = moved.caches();
        let folder = |key: &str| {
            caches
                .iter()
                .find(|cache| cache.key == key)
                .unwrap()
                .folders[0]
                .clone()
        };
        assert_eq!(folder("go-build"), Path::new("/tmp/gocache"));
        assert_eq!(folder("go-modules"), root.join("go/pkg/mod"));
        assert_eq!(folder("sccache"), Path::new("/tmp/sccache"));
        // Without a home there is nothing to look at.
        let mut homeless = DevCaches::new(Host::new(Runtime::Native, BTreeMap::new()));
        assert!(homeless.caches().is_empty());
        assert!(matches!(
            homeless.detect(&Cancellation::default()),
            Ok(Availability::Unavailable(_))
        ));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unknown_tasks_failures_and_cancelling_change_nothing() {
        let root = home("refuse");
        fill(&root.join(".m2/repository"), 2 * 1024 * 1024);
        let mut backend = caches_in(&root, &[]);
        let cancel = Cancellation::default();
        let mut ignore = |_: Progress| {};
        for id in [
            CleanupId {
                backend: "dev-caches".into(),
                key: "everything".into(),
            },
            CleanupId {
                backend: "npm".into(),
                key: "maven".into(),
            },
        ] {
            assert!(matches!(
                backend.execute(&Operation::Clean(id.clone()), &cancel, &mut ignore),
                Err(EngineError::NotFound)
            ));
            assert!(matches!(
                backend.cleanup_plan(&id, &cancel),
                Err(EngineError::NotFound)
            ));
        }
        assert!(matches!(
            backend.execute(
                &Operation::Refresh {
                    backend: "dev-caches".into()
                },
                &cancel,
                &mut ignore
            ),
            Err(EngineError::Unsupported { .. })
        ));
        let maven = CleanupId {
            backend: "dev-caches".into(),
            key: "maven".into(),
        };
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(matches!(
            backend.execute(&Operation::Clean(maven.clone()), &cancelled, &mut ignore),
            Err(EngineError::Cancelled)
        ));
        assert!(matches!(
            backend.cleanup(&cancelled),
            Err(EngineError::Cancelled)
        ));
        assert!(root.join(".m2/repository/blob").exists());
        // A folder that can't be emptied says which one. Root may change
        // anything, so this only shows for everyone else.
        let repository = root.join(".m2/repository");
        fs::set_permissions(
            &repository,
            std::os::unix::fs::PermissionsExt::from_mode(0o500),
        )
        .unwrap();
        let result = backend.execute(&Operation::Clean(maven), &cancel, &mut ignore);
        fs::set_permissions(
            &repository,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        let root_user = rustix::process::geteuid().is_root();
        assert!(
            root_user || result.is_err_and(|error| error.to_string().contains("~/.m2/repository")),
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_build_holds_only_the_caches_it_uses() {
        let root = home("building");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        // A Gradle build, and the compiler it started.
        fs::write(
            bin.join("ps"),
            "#!/bin/sh\nprintf '10 1 java org.gradle.launcher.GradleMain build\\n11 10 javac Main.java\\n'\n",
        )
        .unwrap();
        fs::set_permissions(
            bin.join("ps"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fill(&root.join(".gradle/caches"), 2 * 1024 * 1024);
        fill(&root.join(".bun/install/cache"), 2 * 1024 * 1024);
        fill(&root.join(".cargo/registry"), 2 * 1024 * 1024);
        let mut backend = caches_in(&root, &[("PATH", &bin)]);
        let cancel = Cancellation::default();
        let clean = |backend: &mut DevCaches, key: &str| {
            let id = CleanupId {
                backend: "dev-caches".into(),
                key: key.into(),
            };
            backend.execute(&Operation::Clean(id), &cancel, &mut |_| {})
        };
        assert!(matches!(
            clean(&mut backend, "gradle"),
            Err(EngineError::Refused {
                refusal: Refusal::Building,
                ..
            })
        ));
        assert!(root.join(".gradle/caches/blob").exists());
        clean(&mut backend, "bun").unwrap();
        clean(&mut backend, "cargo").unwrap();
        assert!(!root.join(".bun/install/cache/blob").exists());
        assert!(!root.join(".cargo/registry/blob").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn paths_outside_the_home_are_shown_in_full() {
        let host = Host::new(
            Runtime::Native,
            BTreeMap::from([("HOME".into(), "/home/me".into())]),
        );
        assert_eq!(shown(&host, Path::new("/home/me")), "~");
        assert_eq!(shown(&host, Path::new("/home/me/.m2")), "~/.m2");
        assert_eq!(shown(&host, Path::new("/home/meme")), "/home/meme");
        assert_eq!(shown(&host, Path::new("/opt/cache")), "/opt/cache");
    }
}
