//! Rust toolchains managed by rustup, apart from the Cargo-installed tools.
//!
//! Each installed toolchain is a row: channels (stable, beta, nightly) update
//! within the channel, and pinned versions never do. Toolchain updates pass
//! `--no-self-update`; rustup's own update is a separate row.
use super::{bytes, invalid, NativeTransport, Transport};
use crate::{engine::*, package::*, process::*};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

const ID: &str = "rustup";
const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Upgrade,
    Capability::Clean,
];
/// How deep project folders are searched for toolchain pins.
const PIN_DEPTH: usize = 8;
/// The row for rustup itself.
const SELF: &str = "rustup";

pub struct Rustup<T = NativeTransport> {
    transport: T,
    /// Where projects are looked for, instead of the saved setting.
    folders: Option<Vec<PathBuf>>,
}

struct Toolchain {
    name: String,
    default: bool,
}

/// What `rustup check` says about one toolchain (or rustup itself).
#[derive(Debug, PartialEq)]
struct Check {
    installed: String,
    candidate: Option<String>,
}

/// Toolchain names rustup accepts: channels, versions, dates and targets.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// `1.98.1 (48a229cea 2026-09-01)` → `1.98.1`; `(unknown version)` → `unknown`.
fn version(text: &str) -> String {
    let text = text.trim();
    if text.starts_with('(') {
        return "unknown".into();
    }
    text.split_whitespace().next().unwrap_or(text).to_owned()
}

/// `stable-aarch64-apple-darwin - Update available : 1.89.0 (…) -> 1.98.1 (…)`
/// `beta-aarch64-apple-darwin - Up to date : 1.99.0-beta.2 (…)`
fn parse_check(text: &str) -> BTreeMap<String, Check> {
    text.lines()
        .filter_map(|line| {
            let (name, status) = line.split_once(" - ")?;
            // rustup 1.28 prints "Update available : …"; 1.29 "up to date: …".
            let (state, versions) = status.split_once(':')?;
            let check = if state.trim().eq_ignore_ascii_case("update available") {
                let (from, to) = versions.split_once(" -> ")?;
                Check {
                    installed: version(from),
                    candidate: Some(version(to)),
                }
            } else {
                Check {
                    installed: version(versions),
                    candidate: None,
                }
            };
            Some((name.trim().to_owned(), check))
        })
        .collect()
}

/// A toolchain name split into its version and the host it runs on:
/// `1.98.1-x86_64-unknown-linux-gnu` and `nightly-2026-05-01-…` are pinned
/// versions, `stable-…` a channel. `None` for a linked toolchain, which has
/// no version of its own.
fn split_version(name: &str) -> Option<(&str, &str)> {
    let dashes: Vec<_> = name.match_indices('-').map(|(at, _)| at).collect();
    let numbered = |part: &str| {
        let parts: Vec<_> = part.split('.').collect();
        (2..=3).contains(&parts.len())
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    };
    let first = *dashes.first()?;
    let channel = &name[..first];
    let end = if matches!(channel, "stable" | "beta" | "nightly") {
        // A dated one: `nightly-2026-05-01-…`.
        let dated = dashes.get(3).filter(|&&end| {
            let date = &name[first + 1..end];
            date.len() == 10 && date.bytes().all(|b| b.is_ascii_digit() || b == b'-')
        });
        dated.copied().unwrap_or(first)
    } else if numbered(channel) {
        first
    } else {
        return None;
    };
    Some((&name[..end], &name[end + 1..]))
}

/// The toolchain a `rust-toolchain.toml` or `rust-toolchain` file pins.
fn pinned(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("channel"))
        .and_then(|line| line.split_once('='))
        .map(|(_, value)| value.trim().trim_matches(|c| c == '"' || c == '\''));
    let channel = line.unwrap_or_else(|| text.trim());
    valid_name(channel).then(|| channel.to_owned())
}

/// Every toolchain the projects under `dir` pin.
fn pins(dir: &Path, depth: usize, device: u64, found: &mut Vec<String>, cancel: &Cancellation) {
    if cancel.requested() {
        return;
    }
    for name in ["rust-toolchain.toml", "rust-toolchain"] {
        if let Some(pin) = fs::read_to_string(dir.join(name))
            .ok()
            .and_then(|text| pinned(&text))
        {
            found.push(pin);
        }
    }
    if depth >= PIN_DEPTH {
        return;
    }
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let skipped = name.starts_with('.')
            || matches!(
                name.as_ref(),
                "target" | "node_modules" | "Library" | "build"
            );
        let path = entry.path();
        if !skipped
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
            && fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.dev() == device)
        {
            pins(&path, depth + 1, device, found, cancel);
        }
    }
}

impl<T: Transport> Rustup<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            folders: None,
        }
    }
    /// Look for pins in these folders rather than the saved ones.
    #[cfg(test)]
    fn with_folders(mut self, folders: Vec<PathBuf>) -> Self {
        self.folders = Some(folders);
        self
    }

    fn run(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<String, EngineError> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let output = match self.transport.dev_tool(ID, &args, cancel, write) {
            // `rustup check` exits 100 when updates exist.
            Err(ExecutionError::Failed(result)) if result.code == Some(100) => result.stdout,
            result => bytes(ID, result?)?,
        };
        String::from_utf8(output).map_err(|error| invalid(ID, error))
    }

    fn toolchains(&self, cancel: &Cancellation) -> Result<Vec<Toolchain>, EngineError> {
        Ok(self
            .run(&["toolchain", "list"], cancel, false)?
            .lines()
            .filter_map(|line| {
                let name = line.split_whitespace().next()?;
                (valid_name(name) && name != "no").then(|| Toolchain {
                    name: name.to_owned(),
                    // "(active)" depends on the directory rustup runs in, so
                    // only the default is reported.
                    default: line.contains("default"),
                })
            })
            .collect())
    }

    /// Toolchains of a pinned version no project pins, no folder
    /// overrides and that aren't the default.
    fn unpinned(&self, cancel: &Cancellation) -> Result<Vec<String>, EngineError> {
        let toolchains = self.toolchains(cancel)?;
        let Some(host) = toolchains
            .iter()
            .find(|toolchain| toolchain.default)
            .and_then(|toolchain| split_version(&toolchain.name))
            .map(|(_, host)| host.to_owned())
        else {
            return Ok(vec![]);
        };
        let folders = self
            .folders
            .clone()
            .unwrap_or_else(|| super::ProjectSettings::load(&crate::host::Host::current()).folders);
        let mut pinned = vec![];
        for folder in folders {
            if let Ok(metadata) = fs::metadata(&folder) {
                pins(&folder, 0, metadata.dev(), &mut pinned, cancel);
            }
        }
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        // "/code/app\t1.80.0-x86_64-unknown-linux-gnu"
        let overrides = self.run(&["override", "list"], cancel, false)?;
        Ok(toolchains
            .into_iter()
            .filter(|toolchain| !toolchain.default)
            .map(|toolchain| toolchain.name)
            .filter(|name| {
                split_version(name).is_some_and(|(version, on)| {
                    on == host
                        && !matches!(version, "stable" | "beta" | "nightly")
                        && !pinned.iter().any(|pin| pin == version || pin == name)
                })
            })
            .filter(|name| {
                !overrides
                    .lines()
                    .any(|line| line.split_whitespace().last() == Some(name.as_str()))
            })
            .collect())
    }

    /// Where `name` is installed, to measure it.
    fn folder(&self, name: &str) -> Option<PathBuf> {
        let home = self
            .transport
            .env("RUSTUP_HOME")
            .map(PathBuf::from)
            .or_else(|| Some(PathBuf::from(self.transport.env("HOME")?).join(".rustup")))
            .filter(|home| home.is_absolute())?;
        Some(home.join("toolchains").join(name))
    }

    fn cleanup_item(&self, name: &str, cancel: &Cancellation) -> CleanupItem {
        let bytes = self
            .folder(name)
            .filter(|folder| fs::symlink_metadata(folder).is_ok_and(|metadata| metadata.is_dir()))
            .map(|folder| crate::disk::usage(&folder, cancel));
        CleanupItem {
            id: CleanupId {
                backend: ID.into(),
                key: format!("toolchain:{name}"),
            },
            kind: CleanupKind::Unused,
            title: format!("Rust {name}"),
            summary: "No project pins this version".into(),
            preview: format!(
                "Uninstalls this toolchain. rustup installs it again when a project asks for it.\n{name}"
            ),
            bytes,
            restorable: false,
        }
    }

    fn package(name: &str, summary: String, check: Option<&Check>) -> Package {
        Package {
            id: PackageId {
                backend: ID.into(),
                name: name.into(),
                architecture: "unknown".into(),
                scope: Scope::User {
                    uid: rustix::process::getuid().as_raw(),
                },
                remote: None,
                reference: None,
            },
            display_name: name.into(),
            summary,
            installed_version: Some(
                check.map_or_else(|| "installed".into(), |c| c.installed.clone()),
            ),
            update: match check {
                Some(Check {
                    candidate: Some(_), ..
                }) => UpdateAvailability::Available,
                Some(_) => UpdateAvailability::Current,
                None => UpdateAvailability::Unknown,
            },
            candidate_version: check.and_then(|c| c.candidate.clone()),
            icon: None,
            component_ids: vec![],
            homepages: vec!["https://rust-lang.github.io/rustup/".into()],
            adopt_with: None,
        }
    }

    fn rows(&self, check: bool, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let checks = if check {
            parse_check(&self.run(&["check"], cancel, false)?)
        } else {
            BTreeMap::new()
        };
        let mut rows: Vec<Package> = self
            .toolchains(cancel)?
            .iter()
            .map(|toolchain| {
                let mut notes = vec!["Rust toolchain"];
                if toolchain.default {
                    notes.push("default");
                }
                // A pinned version is never updated, so check skips it.
                let pinned = toolchain.name.starts_with(|c: char| c.is_ascii_digit());
                let found = checks.get(&toolchain.name);
                let pinned_check;
                let check = match (found, pinned && check) {
                    (Some(found), _) => Some(found),
                    (None, true) => {
                        pinned_check = Check {
                            installed: toolchain
                                .name
                                .split('-')
                                .next()
                                .unwrap_or_default()
                                .to_owned(),
                            candidate: None,
                        };
                        Some(&pinned_check)
                    }
                    (None, false) => None,
                };
                Self::package(&toolchain.name, notes.join(" · "), check)
            })
            .collect();
        if let Some(own) = checks.get(SELF) {
            rows.push(Self::package(
                SELF,
                "The rustup toolchain manager".into(),
                Some(own),
            ));
        }
        Ok(rows)
    }
}

impl<T: Transport> Backend for Rustup<T> {
    fn id(&self) -> &str {
        ID
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        match self
            .transport
            .dev_tool(ID, &["--version".into()], cancel, false)
        {
            Ok(_) => Ok(Availability::Available),
            Err(ExecutionError::Disabled(reason)) => Ok(Availability::Unavailable(reason)),
            Err(ExecutionError::Cancelled) => Err(EngineError::Cancelled),
            Err(error) => Err(error.into()),
        }
    }
    fn may_have(&self, name: &str) -> bool {
        valid_name(name)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let query = query.to_lowercase();
        let mut rows: Vec<Package> = self
            .rows(false, cancel)?
            .into_iter()
            .filter(|row| row.id.name.contains(&query))
            .collect();
        // A channel or version that isn't installed can be.
        let known = ["stable", "beta", "nightly"].contains(&query.as_str())
            || query.starts_with(|c: char| c.is_ascii_digit());
        if known
            && valid_name(&query)
            && !rows.iter().any(|row| {
                row.id.name == query || row.id.name.split('-').next() == Some(query.as_str())
            })
        {
            let mut offer =
                Self::package(&query, format!("Install the {query} Rust toolchain"), None);
            offer.installed_version = None;
            rows.push(offer);
        }
        Ok(rows)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.rows(true, cancel)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let package = self
            .rows(true, cancel)?
            .into_iter()
            .find(|row| row.id == *id)
            .ok_or(EngineError::NotFound)?;
        let description = if id.name == SELF {
            "rustup itself. Updating it runs `rustup self update`.".to_owned()
        } else {
            format!(
                "{}. Updates stay on this toolchain's channel and never update rustup itself; pinned versions don't change.",
                package.summary
            )
        };
        Ok(PackageDetails {
            package,
            description,
            homepage: Some("https://rust-lang.github.io/rustup/".into()),
            dependencies: vec![],
            data: None,
        })
    }
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        Ok(self
            .unpinned(cancel)?
            .iter()
            .map(|name| self.cleanup_item(name, cancel))
            .collect())
    }
    fn cleanup_plan(
        &mut self,
        id: &CleanupId,
        cancel: &Cancellation,
    ) -> Result<CleanupItem, EngineError> {
        let name = id
            .key
            .strip_prefix("toolchain:")
            .filter(|_| id.backend == ID)
            .ok_or(EngineError::NotFound)?;
        if !self
            .unpinned(cancel)?
            .iter()
            .any(|unpinned| unpinned == name)
        {
            return Err(EngineError::NotFound);
        }
        Ok(self.cleanup_item(name, cancel))
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        if let Operation::Clean(id) = operation {
            let plan = self.cleanup_plan(id, cancel)?;
            let name = id.key.trim_start_matches("toolchain:");
            progress(Progress::Message(format!("Uninstalling {}…", plan.title)));
            let args = ["toolchain", "uninstall", "--", name].map(OsString::from);
            let completion = self.transport.dev_tool(ID, &args, cancel, true)?;
            let deferred = completion.cancellation_deferred;
            bytes(ID, completion)?;
            return Ok(OperationOutcome {
                cancellation_deferred: deferred,
            });
        }
        let (id, args): (&PackageId, Vec<&str>) = match operation {
            Operation::Upgrade(id) if id.name == SELF => (id, vec!["self", "update"]),
            Operation::Upgrade(id) => (id, vec!["update", "--no-self-update", "--", &id.name]),
            Operation::Install(id) => (
                id,
                vec![
                    "toolchain",
                    "install",
                    "--profile",
                    "minimal",
                    "--no-self-update",
                    "--",
                    &id.name,
                ],
            ),
            Operation::Remove(id) => {
                let toolchains = self.toolchains(cancel)?;
                let found = toolchains
                    .iter()
                    .find(|t| t.name == id.name)
                    .ok_or(EngineError::NotFound)?;
                if found.default {
                    return Err(invalid(ID, format!(
                        "{} is the default toolchain; pick another default (rustup default <toolchain>) before removing it",
                        id.name
                    )));
                }
                (id, vec!["toolchain", "uninstall", "--", &id.name])
            }
            other => return Err(self.unsupported(other.capability())),
        };
        if id.backend != ID || !valid_name(&id.name) {
            return Err(invalid(ID, "foreign or invalid toolchain"));
        }
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        progress(Progress::Message(format!(
            "Running rustup {}…",
            args.join(" ")
        )));
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let completion = self.transport.dev_tool(ID, &args, cancel, true)?;
        let deferred = completion.cancellation_deferred;
        bytes(ID, completion)?;
        // Verify with a fresh read; writes may finish after cancellation.
        let after = self.toolchains(&Cancellation::default())?;
        let listed = after
            .iter()
            .any(|t| t.name == id.name || t.name.split('-').next() == Some(id.name.as_str()));
        let ok = match operation {
            Operation::Install(_) => listed,
            Operation::Remove(_) => !after.iter().any(|t| t.name == id.name),
            _ => id.name == SELF || listed,
        };
        if !ok {
            return Err(invalid(
                ID,
                format!(
                    "rustup finished, but {} is not in the expected state",
                    id.name
                ),
            ));
        }
        Ok(OperationOutcome {
            cancellation_deferred: deferred,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Fake {
        list: Arc<Mutex<String>>,
        calls: Arc<Mutex<Vec<String>>>,
        /// How `rustup --version` fails, if it does.
        missing: Option<fn() -> ExecutionError>,
    }
    fn done(text: &str, code: i32) -> Completion {
        Completion {
            code: Some(code),
            signal: None,
            stdout: text.as_bytes().to_vec(),
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
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
            assert_eq!(executable, "rustup");
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            let line = args.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            let mut list = self.list.lock().unwrap();
            match args[0].as_str() {
                "--version" => self
                    .missing
                    .map_or_else(|| Ok(done("rustup 1.29.1", 0)), |error| Err(error())),
                "toolchain" if args[1] == "list" => Ok(done(&list, 0)),
                "override" => Ok(done(
                    "/code/app\t1.81.0-aarch64-apple-darwin\n",
                    0,
                )),
                // Newer rustup exits 100 when something can be updated.
                "check" => Err(ExecutionError::Failed(done(
                    "stable-aarch64-apple-darwin - Update available : 1.89.0 (29483883e 2025-08-04) -> 1.98.1 (48a229cea 2026-09-01)\n\
                     beta-aarch64-apple-darwin - Up to date : 1.99.0-beta.2 (abc 2026-09-20)\n\
                     nightly-aarch64-apple-darwin - up to date: 1.100.0-nightly (def 2026-09-27)\n\
                     rustup - update available : 1.28.2 -> 1.29.1\n",
                    100,
                ))),
                "toolchain" if args[1] == "install" => {
                    list.push_str("nightly-aarch64-apple-darwin\n");
                    Ok(done("", 0))
                }
                "toolchain" if args[1] == "uninstall" => {
                    let name = args.last().unwrap();
                    *list = list
                        .lines()
                        .filter(|line| !line.starts_with(name.as_str()))
                        .map(|line| format!("{line}\n"))
                        .collect();
                    Ok(done("", 0))
                }
                other => {
                    assert!(matches!(other, "update" | "self"), "unexpected {other}");
                    Ok(done("", 0))
                }
            }
        }
    }
    fn fake() -> Fake {
        Fake {
            list: Arc::new(Mutex::new(
                "stable-aarch64-apple-darwin (default)\nbeta-aarch64-apple-darwin\n1.80.0-aarch64-apple-darwin (active)\n".into(),
            )),
            calls: Arc::default(),
            missing: None,
        }
    }

    #[test]
    fn versions_no_project_pins_are_offered_and_uninstalled() {
        let root = std::env::temp_dir().join(format!("pkgdeck-rustup-pins-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |path: &Path, text: &str| {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(
            &root.join("code/app/rust-toolchain.toml"),
            "[toolchain]\nchannel = \"1.98.1\"\ncomponents = [\"clippy\"]\n",
        );
        write(
            &root.join("code/old/rust-toolchain"),
            "nightly-2026-05-01\n",
        );
        // Pins that don't count: in build output, hidden, too deep, or not
        // a pin.
        write(
            &root.join("code/app/target/x/rust-toolchain.toml"),
            "channel = \"1.97.0\"",
        );
        write(
            &root.join("code/deep/1/2/3/4/5/6/7/8/rust-toolchain.toml"),
            "channel = \"1.97.0\"",
        );
        write(
            &root.join("code/.hidden/rust-toolchain.toml"),
            "channel = \"1.96.0\"",
        );
        write(
            &root.join("code/odd/rust-toolchain.toml"),
            "[toolchain]\nprofile = \"minimal\"\n",
        );
        let fake = fake();
        *fake.list.lock().unwrap() = [
            "stable-aarch64-apple-darwin (default)",
            "beta-aarch64-apple-darwin",
            "nightly-aarch64-apple-darwin",
            "1.98.1-aarch64-apple-darwin",
            "1.97.0-aarch64-apple-darwin",
            "1.96.0-aarch64-apple-darwin",
            "1.81.0-aarch64-apple-darwin",
            "nightly-2026-05-01-aarch64-apple-darwin",
            "nightly-2026-04-01-aarch64-apple-darwin",
            "1.95.0-sbpf-solana-v1.57",
            "1.90.0-x86_64-apple-darwin",
            "",
        ]
        .join("\n");
        let installed = root.join("rustup/toolchains/1.97.0-aarch64-apple-darwin");
        write(&installed.join("bin/rustc"), &"x".repeat(4096));
        #[derive(Clone)]
        struct Homed(Fake, PathBuf);
        impl Transport for Homed {
            fn dev_tool(
                &self,
                executable: &str,
                args: &[OsString],
                cancel: &Cancellation,
                write: bool,
            ) -> Result<Completion, ExecutionError> {
                self.0.dev_tool(executable, args, cancel, write)
            }
            fn env(&self, name: &str) -> Option<OsString> {
                (name == "RUSTUP_HOME").then(|| self.1.clone().into_os_string())
            }
        }
        let mut rustup = Rustup::new(Homed(fake.clone(), root.join("rustup")))
            .with_folders(vec![root.join("code"), root.join("missing")]);
        let cancel = Cancellation::default();
        let items = rustup.cleanup(&cancel).unwrap();
        let keys: Vec<_> = items.iter().map(|item| item.id.key.as_str()).collect();
        // The other host's, the linked one, channels, pins and overrides stay.
        assert_eq!(
            keys,
            [
                "toolchain:1.97.0-aarch64-apple-darwin",
                "toolchain:1.96.0-aarch64-apple-darwin",
                "toolchain:nightly-2026-04-01-aarch64-apple-darwin",
            ]
        );
        assert_eq!(items[0].title, "Rust 1.97.0-aarch64-apple-darwin");
        assert!(items[0].bytes.unwrap() >= 4096);
        assert_eq!(items[1].bytes, None);
        assert_eq!(
            rustup.cleanup_plan(&items[0].id, &cancel).unwrap(),
            items[0]
        );
        let mut said = vec![];
        rustup
            .execute(
                &Operation::Clean(items[0].id.clone()),
                &cancel,
                &mut |progress| said.push(progress),
            )
            .unwrap();
        assert_eq!(
            said,
            [Progress::Message(
                "Uninstalling Rust 1.97.0-aarch64-apple-darwin…".into()
            )]
        );
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"toolchain uninstall -- 1.97.0-aarch64-apple-darwin".to_owned()));
        for key in [
            "toolchain:1.97.0-aarch64-apple-darwin",
            "toolchain:1.98.1-aarch64-apple-darwin",
            "toolchain:stable-aarch64-apple-darwin",
            "other",
        ] {
            let id = CleanupId {
                backend: ID.into(),
                key: key.into(),
            };
            assert!(matches!(
                rustup.cleanup_plan(&id, &cancel),
                Err(EngineError::NotFound)
            ));
        }
        // Without a default there's no telling which host is this one.
        *fake.list.lock().unwrap() = "1.97.0-aarch64-apple-darwin\n".into();
        assert!(rustup.cleanup(&cancel).unwrap().is_empty());
        let cancelled = Cancellation::default();
        cancelled.cancel();
        *fake.list.lock().unwrap() = "stable-aarch64-apple-darwin (default)\n".into();
        assert!(matches!(
            rustup.cleanup(&cancelled),
            Err(EngineError::Cancelled)
        ));
        assert_eq!(
            split_version("1.99-x86_64-unknown-linux-gnu"),
            Some(("1.99", "x86_64-unknown-linux-gnu"))
        );
        assert_eq!(split_version("custom"), None);
        assert_eq!(split_version("1.x-host"), None);
        assert_eq!(pinned("1.80.0"), Some("1.80.0".into()));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn toolchains_channels_pins_and_rustup_itself() {
        let fake = fake();
        let mut rustup = Rustup::new(fake.clone());
        let rows = rustup.installed(&Cancellation::default()).unwrap();
        let summary: Vec<(&str, &str, Option<&str>)> = rows
            .iter()
            .map(|r| {
                (
                    r.id.name.as_str(),
                    r.installed_version.as_deref().unwrap(),
                    r.candidate_version.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("stable-aarch64-apple-darwin", "1.89.0", Some("1.98.1")),
                ("beta-aarch64-apple-darwin", "1.99.0-beta.2", None),
                ("1.80.0-aarch64-apple-darwin", "1.80.0", None),
                ("rustup", "1.28.2", Some("1.29.1")),
            ]
        );
        assert_eq!(rows[0].summary, "Rust toolchain · default");
        assert_eq!(rows[2].update, UpdateAvailability::Current);
        rustup
            .execute(
                &Operation::Upgrade(rows[0].id.clone()),
                &Cancellation::default(),
                &mut drop::<Progress>,
            )
            .unwrap();
        rustup
            .execute(
                &Operation::Upgrade(rows[3].id.clone()),
                &Cancellation::default(),
                &mut drop::<Progress>,
            )
            .unwrap();
        let calls = fake.calls.lock().unwrap().clone();
        assert!(calls.contains(&"update --no-self-update -- stable-aarch64-apple-darwin".into()));
        assert!(calls.contains(&"self update".into()));
    }

    #[test]
    fn check_output_from_old_and_new_rustup() {
        let checks = parse_check(
            "stable-x86_64-unknown-linux-gnu - Update available : 1.97.0 (a 2026-08-01) -> 1.98.1 (b 2026-09-01)\n\
             beta-x86_64-unknown-linux-gnu - up to date: 1.99.0-beta.2 (c 2026-09-20)\n\
             nightly-x86_64-unknown-linux-gnu - update available: (unknown version) -> 1.100.0-nightly (d 2026-09-27)\n\
             rustup - update available : 1.28.2 -> 1.29.1\n",
        );
        assert_eq!(
            checks["stable-x86_64-unknown-linux-gnu"]
                .candidate
                .as_deref(),
            Some("1.98.1")
        );
        assert_eq!(
            checks["beta-x86_64-unknown-linux-gnu"],
            Check {
                installed: "1.99.0-beta.2".into(),
                candidate: None
            }
        );
        assert_eq!(
            checks["nightly-x86_64-unknown-linux-gnu"].installed,
            "unknown"
        );
        assert_eq!(checks["rustup"].candidate.as_deref(), Some("1.29.1"));
    }

    #[test]
    fn a_full_toolchain_name_finds_only_the_installed_row() {
        let mut rustup = Rustup::new(fake());
        let found = rustup
            .search("1.80.0-aarch64-apple-darwin", &Cancellation::default())
            .unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].installed_version.is_some());
    }

    #[test]
    fn install_offers_and_removal_rules() {
        let fake = fake();
        let mut rustup = Rustup::new(fake.clone());
        let offers = rustup.search("nightly", &Cancellation::default()).unwrap();
        assert_eq!(offers.len(), 1);
        assert!(unverified_search_offer(&offers[0]));
        assert!(rustup
            .search("stable", &Cancellation::default())
            .unwrap()
            .iter()
            .all(|r| r.installed_version.is_some()));
        rustup
            .execute(
                &Operation::Install(offers[0].id.clone()),
                &Cancellation::default(),
                &mut drop::<Progress>,
            )
            .unwrap();
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"toolchain install --profile minimal --no-self-update -- nightly".into()));
        let rows = rustup.installed(&Cancellation::default()).unwrap();
        // The default toolchain is never removed out from under Cargo.
        let error = rustup
            .execute(
                &Operation::Remove(rows[0].id.clone()),
                &Cancellation::default(),
                &mut drop::<Progress>,
            )
            .unwrap_err();
        assert!(error.to_string().contains("default toolchain"), "{error}");
        rustup
            .execute(
                &Operation::Remove(rows[1].id.clone()),
                &Cancellation::default(),
                &mut drop::<Progress>,
            )
            .unwrap();
        assert!(!fake.list.lock().unwrap().contains("beta"));
        let mut bad = rows[1].id.clone();
        bad.name = "--help".into();
        assert!(rustup
            .execute(
                &Operation::Install(bad),
                &Cancellation::default(),
                &mut drop::<Progress>
            )
            .is_err());
    }

    #[test]
    fn availability_follows_the_rustup_command() {
        let detect = |missing: Option<fn() -> ExecutionError>| {
            Rustup::new(Fake { missing, ..fake() }).detect(&Cancellation::default())
        };
        assert_eq!(detect(None).unwrap(), Availability::Available);
        assert_eq!(
            detect(Some(|| ExecutionError::Disabled("rustup not found".into()))).unwrap(),
            Availability::Unavailable("rustup not found".into())
        );
        assert!(matches!(
            detect(Some(|| ExecutionError::Cancelled)),
            Err(EngineError::Cancelled)
        ));
        assert!(matches!(
            detect(Some(|| ExecutionError::TimedOut)),
            Err(EngineError::Execution(ExecutionError::TimedOut))
        ));
        let rustup = Rustup::new(fake());
        assert!(rustup.may_have("1.80.0-aarch64-apple-darwin") && !rustup.may_have("--help"));
        assert_eq!(rustup.capabilities(), CAPABILITIES);
    }

    #[test]
    fn details_explain_toolchain_and_self_updates() {
        let mut rustup = Rustup::new(fake());
        let cancel = Cancellation::default();
        let rows = rustup.installed(&cancel).unwrap();
        let stable = rustup.details(&rows[0].id, &cancel).unwrap();
        assert!(stable
            .description
            .starts_with("Rust toolchain · default. Updates stay on this toolchain's channel"));
        let own = rustup.details(&rows[3].id, &cancel).unwrap();
        assert!(own.description.contains("rustup self update"));
        let mut absent = rows[0].id.clone();
        absent.name = "nightly-aarch64-apple-darwin".into();
        assert!(matches!(
            rustup.details(&absent, &cancel),
            Err(EngineError::NotFound)
        ));
    }

    #[test]
    fn operations_refuse_before_running_and_verify_after() {
        let fake = fake();
        let mut rustup = Rustup::new(fake.clone());
        let cancel = Cancellation::default();
        let rows = rustup.installed(&cancel).unwrap();
        let beta = rows[1].id.clone();
        let mut run = |operation: Operation, cancel: &Cancellation| {
            rustup.execute(&operation, cancel, &mut drop::<Progress>)
        };
        assert!(matches!(
            run(Operation::UpgradeAll { backend: ID.into() }, &cancel),
            Err(EngineError::Unsupported { .. })
        ));
        let mut absent = beta.clone();
        absent.name = "nightly".into();
        assert!(matches!(
            run(Operation::Remove(absent), &cancel),
            Err(EngineError::NotFound)
        ));
        let mut foreign = beta.clone();
        foreign.backend = "cargo".into();
        let error = run(Operation::Upgrade(foreign), &cancel).unwrap_err();
        assert!(error.to_string().contains("foreign or invalid toolchain"));
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(matches!(
            run(Operation::Upgrade(beta.clone()), &cancelled),
            Err(EngineError::Cancelled)
        ));
        // Nothing ran for any refusal.
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("update") || call.contains("uninstall")));
        // An update of a toolchain that then isn't listed is an error.
        *fake.list.lock().unwrap() = "stable-aarch64-apple-darwin (default)\n".into();
        let error = run(Operation::Upgrade(beta), &cancel).unwrap_err();
        assert!(
            error.to_string().contains("not in the expected state"),
            "{error}"
        );
    }

    /// The fake answers only rustup commands it knows.
    #[test]
    fn the_fake_refuses_every_other_manager() {
        let fake = fake();
        let cancel = Cancellation::default();
        let refused = |call: &dyn Fn() -> Result<Completion, ExecutionError>| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)).is_err()
        };
        assert!(refused(&|| fake.dev_tool("cargo", &[], &cancel, false)));
        assert!(refused(&|| fake.dev_tool(
            "rustup",
            &["frobnicate".into()],
            &cancel,
            true
        )));
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| call == "frobnicate"));
    }
}
