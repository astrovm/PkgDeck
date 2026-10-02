//! macOS updates through `softwareupdate`: the pending updates Software
//! Update lists, and installing them.
//!
//! Update checks scan Apple's servers; every other query reads the last scan
//! (`--no-scan`), so opening a page never fetches. Installing needs root.
//! An update that needs a restart is only downloaded: on Apple silicon,
//! installing it needs the owner's password, which PkgDeck never handles,
//! and PkgDeck never restarts the Mac. System Settings finishes it.
//! Upgrades to a new major macOS version are left to System Settings.
//! Automatic updates only download (one fixed command the saved approval
//! names, see [`crate::unattended::macos`]); installing stays with the person.
use super::*;

const ID: &str = "macos-updates";
const CAPABILITIES: &[Capability] = &[
    Capability::Installed,
    Capability::Details,
    Capability::Upgrade,
];

pub struct MacUpdates<T = NativeTransport> {
    transport: T,
    update_check: Option<u64>,
}

/// One entry of `softwareupdate --list`.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Pending {
    label: String,
    title: String,
    version: String,
    restart: bool,
}

/// Labels go back to softwareupdate as arguments.
fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && !label.starts_with('-')
        && label.len() <= 256
        && label.chars().all(|c| !c.is_control())
}

/// Parse the listing:
///
/// ```text
/// * Label: macOS Tahoe 26.1-25B78
///     Title: macOS Tahoe 26.1, Version: 26.1, Size: 3141632K, Recommended: YES, Action: restart,
/// ```
fn parse(text: &str) -> Vec<Pending> {
    let mut pending = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(label) = line
            .trim_start()
            .strip_prefix("* Label: ")
            .or_else(|| line.trim_start().strip_prefix("- Label: "))
        else {
            continue;
        };
        let label = label.trim();
        let Some(details) = lines
            .peek()
            .and_then(|next| next.trim_start().strip_prefix("Title: "))
        else {
            continue;
        };
        lines.next();
        // "Title: a, Version: b, ...": a title may itself hold a comma, so
        // a piece only starts a field when it names a known one.
        let mut fields: Vec<(&str, String)> = vec![("Title", String::new())];
        for piece in details.split(", ") {
            let known = piece
                .split_once(": ")
                .filter(|(key, _)| matches!(*key, "Version" | "Size" | "Recommended" | "Action"));
            match known {
                Some((key, value)) => fields.push((key, value.trim_end_matches(',').into())),
                None => {
                    let (_, value) = fields.last_mut().expect("starts with the title");
                    if !value.is_empty() {
                        value.push_str(", ");
                    }
                    value.push_str(piece.trim_end_matches(','));
                }
            }
        }
        let field = |name: &str| {
            fields
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.trim().to_owned())
        };
        let title = field("Title").unwrap_or_default();
        if !valid_label(label) || title.is_empty() {
            continue;
        }
        pending.push(Pending {
            label: label.into(),
            title,
            version: field("Version").unwrap_or_default(),
            // "restart" or "shut down"; either way installing finishes later.
            restart: field("Action").is_some_and(|action| !action.is_empty()),
        });
    }
    pending
}

/// "26.1" is the same major version as "26.0.1"; "27.0" is not.
fn major(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

impl<T: Transport> MacUpdates<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            update_check: None,
        }
    }
    fn macos_version(&self, cancel: &Cancellation) -> Result<String, EngineError> {
        let result =
            self.transport
                .system_manager("sw_vers", &["-productVersion".into()], cancel, false)?;
        Ok(String::from_utf8_lossy(&bytes(ID, result)?)
            .trim()
            .to_owned())
    }
    fn listing(&self, scan: bool, cancel: &Cancellation) -> Result<Vec<Pending>, EngineError> {
        let mut args: Vec<OsString> = vec!["--list".into()];
        if !scan {
            args.push("--no-scan".into());
        }
        let result = self
            .transport
            .system_manager("softwareupdate", &args, cancel, false)?;
        // "No new software available." goes to stderr with exit status 0.
        Ok(parse(&String::from_utf8_lossy(&bytes(ID, result)?)))
    }
    fn packages(&self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let pending = self.listing(false, cancel)?;
        if pending.is_empty() {
            return Ok(vec![]);
        }
        let current = self.macos_version(cancel)?;
        Ok(pending
            .into_iter()
            .filter_map(|update| {
                let system = update.title.starts_with("macOS ");
                if system && !update.version.is_empty() && major(&update.version) != major(&current)
                {
                    return None;
                }
                let summary = if update.restart {
                    "macOS update, restart required"
                } else {
                    "macOS update"
                };
                Some(Package {
                    id: PackageId {
                        backend: ID.into(),
                        name: update.label,
                        architecture: "macos".into(),
                        scope: Scope::System,
                        remote: None,
                        reference: None,
                    },
                    display_name: update.title,
                    summary: summary.into(),
                    // Only macOS itself has a version to update from.
                    installed_version: Some(if system {
                        current.clone()
                    } else {
                        String::new()
                    }),
                    candidate_version: Some(update.version).filter(|v| !v.is_empty()),
                    update: UpdateAvailability::Available,
                    icon: None,
                    component_ids: vec![],
                    homepages: vec![],
                    adopt_with: None,
                })
            })
            .collect())
    }
    fn update(
        &self,
        id: &PackageId,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        if id.backend != ID || !valid_label(&id.name) {
            return Err(EngineError::NotFound);
        }
        let package = self
            .packages(cancel)?
            .into_iter()
            .find(|package| package.id == *id)
            .ok_or(EngineError::NotFound)?;
        let restart = package.summary.contains("restart");
        let command = if restart { "--download" } else { "--install" };
        progress(Progress::Message(format!(
            "{} {}",
            if restart { "Downloading" } else { "Installing" },
            package.display_name
        )));
        let args = [command.into(), "--no-scan".into(), id.name.clone().into()];
        let result = self
            .transport
            .system_manager("softwareupdate", &args, cancel, true)?;
        let deferred = result.cancellation_deferred;
        let output = bytes(ID, result)?;
        if !output.is_empty() {
            progress(Progress::Message(String::from_utf8_lossy(&output).into()));
        }
        if restart {
            progress(Progress::Message(format!(
                "{} is downloaded. Restart from System Settings > General > Software Update to finish installing it.",
                package.display_name
            )));
        }
        Ok(OperationOutcome {
            cancellation_deferred: deferred,
        })
    }
}

impl<T: Transport> MacUpdates<T> {
    fn download_all(
        &self,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        if self.packages(cancel)?.is_empty() {
            return Ok(OperationOutcome::default());
        }
        let [_, args @ ..] = crate::unattended::macos::DOWNLOAD_UPDATES;
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let result = self
            .transport
            .system_manager("softwareupdate", &args, cancel, true)?;
        let deferred = result.cancellation_deferred;
        let output = bytes(ID, result)?;
        if !output.is_empty() {
            progress(Progress::Message(String::from_utf8_lossy(&output).into()));
        }
        progress(Progress::Message(
            "macOS updates are downloaded. Install them from PkgDeck or System Settings > General > Software Update.".into(),
        ));
        Ok(OperationOutcome {
            cancellation_deferred: deferred,
        })
    }
}

impl<T: Transport> Backend for MacUpdates<T> {
    fn id(&self) -> &str {
        ID
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        // sw_vers exists only on macOS.
        availability(self.transport.system_manager(
            "sw_vers",
            &["-productVersion".into()],
            cancel,
            false,
        ))
    }
    fn has_update_index(&self) -> bool {
        true
    }
    fn arm_update_check(&mut self, token: Option<u64>) {
        self.update_check = token;
    }
    /// Only an update check asks Apple's servers; the scan is saved for the
    /// `--no-scan` listings that follow.
    fn refresh_update_index(&mut self, cancel: &Cancellation) -> Result<(), EngineError> {
        if self.update_check.is_some() {
            self.listing(true, cancel)?;
        }
        Ok(())
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.packages(cancel)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let package = self
            .packages(cancel)?
            .into_iter()
            .find(|package| package.id == *id)
            .ok_or(EngineError::NotFound)?;
        Ok(PackageDetails {
            description: format!(
                "{}. Software Update label: {}",
                package.summary, package.id.name
            ),
            package,
            homepage: None,
            dependencies: vec![],
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        match operation {
            Operation::Upgrade(id) => self.update(id, cancel, progress),
            Operation::UpgradeAll { backend } if backend == ID => {
                self.download_all(cancel, progress)
            }
            _ => Err(self.unsupported(operation.capability())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const LISTING: &str = "Software Update Tool\n\nFinding available software\nSoftware Update found the following new or updated software:\n* Label: macOS Tahoe 26.1-25B78\n\tTitle: macOS Tahoe 26.1, Version: 26.1, Size: 3141632K, Recommended: YES, Action: restart, \n* Label: Safari26.1TahoeAuto-26.1\n\tTitle: Safari, Version: 26.1, Size: 210000K, Recommended: YES, \n* Label: Command Line Tools for Xcode, Beta-26.1\n\tTitle: Command Line Tools for Xcode, Beta, Version: 26.1, Size: 751657K, Recommended: YES, \n* Label: macOS Next 27.0-26A1\n\tTitle: macOS Next 27.0, Version: 27.0, Size: 9000000K, Recommended: YES, Action: restart, \n";

    /// Program, arguments, and whether it was a write.
    type Call = (String, Vec<String>, bool);

    #[derive(Clone, Default)]
    struct Fake {
        listing: Arc<Mutex<String>>,
        calls: Arc<Mutex<Vec<Call>>>,
    }
    impl Transport for Fake {
        fn system_manager(
            &self,
            executable: &str,
            args: &[OsString],
            _: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            let text: Vec<String> = args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            self.calls
                .lock()
                .unwrap()
                .push((executable.into(), text, write));
            let stdout = match executable {
                "sw_vers" => "26.0.1\n".into(),
                "softwareupdate" if !write => self.listing.lock().unwrap().clone(),
                _ => "Done.\n".into(),
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

    #[test]
    fn the_listing_keeps_titles_with_commas_and_marks_restarts() {
        let pending = parse(LISTING);
        assert_eq!(pending.len(), 4);
        assert_eq!(pending[0].label, "macOS Tahoe 26.1-25B78");
        assert!(pending[0].restart);
        assert!(!pending[1].restart);
        assert_eq!(pending[2].title, "Command Line Tools for Xcode, Beta");
        assert_eq!(pending[2].version, "26.1");
        assert!(parse("No new software available.\n").is_empty());
        // A label that would read as an option is never kept, and neither
        // is one without its details line.
        assert!(parse("* Label: --all\n\tTitle: x, Version: 1,\n").is_empty());
        assert!(parse("* Label: lonely\n* Label: next\n").is_empty());
    }

    #[test]
    fn an_empty_listing_has_no_rows_and_details_name_the_label() {
        let fake = Fake::default();
        let mut backend = MacUpdates::new(fake.clone());
        assert_eq!(backend.id(), ID);
        assert_eq!(backend.capabilities(), CAPABILITIES);
        assert!(backend.has_update_index());
        assert!(matches!(
            backend.detect(&Cancellation::default()),
            Ok(Availability::Available)
        ));
        assert!(backend
            .installed(&Cancellation::default())
            .unwrap()
            .is_empty());
        // Nothing listed, so macOS's version wasn't even read.
        assert!(
            fake.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(program, _, _)| program == "sw_vers")
                .count()
                == 1
        );
        *fake.listing.lock().unwrap() = LISTING.into();
        let row = backend
            .installed(&Cancellation::default())
            .unwrap()
            .remove(1);
        let details = backend.details(&row.id, &Cancellation::default()).unwrap();
        assert!(details.description.contains("Safari26.1TahoeAuto-26.1"));
        let gone = PackageId {
            name: "gone".into(),
            ..row.id.clone()
        };
        assert!(matches!(
            backend.details(&gone, &Cancellation::default()),
            Err(EngineError::NotFound)
        ));
        // Only updates; a label that reads as an option never runs.
        let mut option = row.id.clone();
        option.name = "--all".into();
        let mut seen = 0;
        let mut record = |_: Progress| seen += 1;
        let ok: Vec<bool> = [
            Operation::Upgrade(row.id.clone()),
            Operation::Remove(row.id.clone()),
            Operation::Upgrade(option),
        ]
        .iter()
        .map(|operation| {
            backend
                .execute(operation, &Cancellation::default(), &mut record)
                .is_ok()
        })
        .collect();
        assert_eq!(ok, [true, false, false]);
        assert_eq!(seen, 2, "installing Safari: what runs, then its output");
    }

    #[test]
    fn rows_skip_major_upgrades_and_only_checks_scan() {
        let fake = Fake::default();
        *fake.listing.lock().unwrap() = LISTING.into();
        let mut backend = MacUpdates::new(fake.clone());
        let rows = backend.installed(&Cancellation::default()).unwrap();
        let names: Vec<&str> = rows.iter().map(|row| row.display_name.as_str()).collect();
        assert_eq!(
            names,
            [
                "macOS Tahoe 26.1",
                "Safari",
                "Command Line Tools for Xcode, Beta"
            ]
        );
        assert_eq!(rows[0].installed_version.as_deref(), Some("26.0.1"));
        assert_eq!(rows[1].installed_version.as_deref(), Some(""));
        assert!(fake.calls.lock().unwrap().iter().all(|(_, args, _)| args
            .first()
            .map(String::as_str)
            != Some("--list")
            || args.contains(&"--no-scan".to_owned())));
        backend
            .refresh_update_index(&Cancellation::default())
            .unwrap();
        assert_eq!(fake.calls.lock().unwrap().len(), 2, "unarmed: no scan");
        backend.arm_update_check(Some(1));
        backend
            .refresh_update_index(&Cancellation::default())
            .unwrap();
        assert_eq!(
            fake.calls.lock().unwrap().last().unwrap().1,
            ["--list".to_owned()]
        );
    }

    #[test]
    fn restart_updates_download_and_others_install_as_root() {
        let fake = Fake::default();
        *fake.listing.lock().unwrap() = LISTING.into();
        let mut backend = MacUpdates::new(fake.clone());
        let rows = backend.installed(&Cancellation::default()).unwrap();
        let mut messages = vec![];
        let mut record = |progress| {
            if let Progress::Message(text) = progress {
                messages.push(text);
            }
        };
        for row in &rows[..2] {
            backend
                .execute(
                    &Operation::Upgrade(row.id.clone()),
                    &Cancellation::default(),
                    &mut record,
                )
                .unwrap();
        }
        let unknown = PackageId {
            name: "gone".into(),
            ..rows[0].id.clone()
        };
        assert!(matches!(
            backend.execute(
                &Operation::Upgrade(unknown),
                &Cancellation::default(),
                &mut record
            ),
            Err(EngineError::NotFound)
        ));
        let writes: Vec<Vec<String>> = fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, _, write)| *write)
            .map(|(_, args, _)| args.clone())
            .collect();
        assert_eq!(
            writes,
            [
                vec!["--download", "--no-scan", "macOS Tahoe 26.1-25B78"],
                vec!["--install", "--no-scan", "Safari26.1TahoeAuto-26.1"],
            ]
        );
        assert!(messages.iter().any(|m| m.contains("System Settings")));
    }

    #[test]
    fn downloading_everything_runs_the_approved_command_only_when_needed() {
        let fake = Fake::default();
        let mut backend = MacUpdates::new(fake.clone());
        let all = Operation::UpgradeAll { backend: ID.into() };
        let mut messages = vec![];
        let mut record = |progress| {
            if let Progress::Message(text) = progress {
                messages.push(text);
            }
        };
        backend
            .execute(&all, &Cancellation::default(), &mut record)
            .unwrap();
        let writes = |fake: &Fake| {
            fake.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, _, write)| *write)
                .map(|(program, args, _)| {
                    std::iter::once(format!("/usr/sbin/{program}"))
                        .chain(args.iter().cloned())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        assert!(writes(&fake).is_empty(), "nothing pending");
        *fake.listing.lock().unwrap() = LISTING.into();
        backend
            .execute(&all, &Cancellation::default(), &mut record)
            .unwrap();
        assert_eq!(writes(&fake), [crate::unattended::macos::DOWNLOAD_UPDATES]);
        assert!(messages.iter().any(|m| m.contains("downloaded")));
    }
}
