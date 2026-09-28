//! Image-based Linux systems (Fedora Atomic desktops, CoreOS, bootc images).
//!
//! These update the whole OS as one deployment that applies on the next
//! restart, not package by package. The source shows the booted deployment,
//! an update that is waiting for a restart, and one that can be downloaded.
//! bootc and rpm-ostree can both manage the same machine: bootc's status is
//! read first when it reports a booted image, otherwise rpm-ostree's.
//! PkgDeck never restarts the computer.
use super::{bytes, invalid, NativeTransport, Transport};
use crate::{engine::*, package::*, process::*};
use serde_json::Value;
use std::ffi::OsString;

const ID: &str = "system-image";
const NAME: &str = "system";
const CAPABILITIES: &[Capability] = &[
    Capability::Installed,
    Capability::Details,
    Capability::Upgrade,
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tool {
    Bootc,
    RpmOstree,
}
impl Tool {
    fn executable(self) -> &'static str {
        match self {
            Self::Bootc => "bootc",
            Self::RpmOstree => "rpm-ostree",
        }
    }
    fn status_args(self) -> &'static [&'static str] {
        match self {
            Self::Bootc => &["status", "--format", "json"],
            Self::RpmOstree => &["status", "--json"],
        }
    }
}

/// One deployment, from either tool.
#[derive(Debug, Default, Clone, PartialEq)]
struct Deployment {
    source: String,
    version: Option<String>,
    packages: Vec<String>,
}

/// What the status says about this machine.
#[derive(Debug, Default, PartialEq)]
struct Status {
    booted: Deployment,
    /// Downloaded and applied on the next restart.
    staged: Option<Deployment>,
    rollback: Option<Deployment>,
    /// Found by the last update check, not downloaded yet.
    available: Option<String>,
}

fn text(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

fn bootc_entry(entry: &Value) -> Option<Deployment> {
    if entry.is_null() {
        return None;
    }
    Some(Deployment {
        source: text(&entry["image"]["image"]["image"]).unwrap_or_else(|| "bootc image".into()),
        version: text(&entry["image"]["version"]),
        packages: vec![],
    })
}

fn parse_bootc(value: &Value) -> Option<Status> {
    let status = &value["status"];
    let booted = bootc_entry(&status["booted"])?;
    Some(Status {
        available: text(&status["booted"]["cachedUpdate"]["version"])
            .or_else(|| text(&status["booted"]["cachedUpdate"]["imageDigest"])),
        staged: bootc_entry(&status["staged"]),
        rollback: bootc_entry(&status["rollback"]),
        booted,
    })
}

fn ostree_entry(entry: &Value) -> Deployment {
    let source = text(&entry["container-image-reference"])
        .or_else(|| text(&entry["origin"]))
        .unwrap_or_else(|| text(&entry["osname"]).unwrap_or_else(|| "ostree deployment".into()));
    Deployment {
        source,
        version: text(&entry["version"]),
        packages: entry["requested-packages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(text)
            .collect(),
    }
}

fn parse_rpm_ostree(value: &Value) -> Option<Status> {
    let deployments = value["deployments"].as_array()?;
    let booted_at = deployments.iter().position(|d| d["booted"] == true)?;
    let booted = ostree_entry(&deployments[booted_at]);
    // Deployments are listed in boot order: one before the booted deployment
    // is pending (staged for the next restart); one after it is the rollback.
    let staged = deployments[..booted_at].first().map(ostree_entry);
    let rollback = deployments.get(booted_at + 1).map(ostree_entry);
    Some(Status {
        available: text(&value["cached-update"]["version"]),
        booted,
        staged,
        rollback,
    })
}

pub struct SystemImage<T = NativeTransport> {
    transport: T,
    tool: Option<Tool>,
}

impl<T: Transport> SystemImage<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            tool: None,
        }
    }

    fn read(&self, tool: Tool, cancel: &Cancellation) -> Result<Option<Status>, EngineError> {
        let args: Vec<OsString> = tool.status_args().iter().map(OsString::from).collect();
        let output = bytes(
            ID,
            self.transport
                .system_manager(tool.executable(), &args, cancel, false)?,
        )?;
        let value: Value = serde_json::from_slice(&output).map_err(|error| invalid(ID, error))?;
        Ok(match tool {
            Tool::Bootc => parse_bootc(&value),
            Tool::RpmOstree => parse_rpm_ostree(&value),
        })
    }

    /// The tool that describes this machine, and what it says.
    fn status(&mut self, cancel: &Cancellation) -> Result<Option<(Tool, Status)>, EngineError> {
        for tool in [Tool::Bootc, Tool::RpmOstree] {
            match self.read(tool, cancel) {
                Ok(Some(status)) => {
                    self.tool = Some(tool);
                    return Ok(Some((tool, status)));
                }
                Ok(None) => {}
                Err(EngineError::Cancelled | EngineError::Execution(ExecutionError::Cancelled)) => {
                    return Err(EngineError::Cancelled)
                }
                // Not installed, or installed on a normal (not image-based)
                // system, where both tools refuse: try the next.
                Err(_) => {}
            }
        }
        Ok(None)
    }

    fn package(tool: Tool, status: &Status) -> Package {
        let version = status
            .booted
            .version
            .clone()
            .unwrap_or_else(|| "unknown".into());
        let (summary, candidate, update) = match (&status.staged, &status.available) {
            (Some(staged), _) => (
                format!(
                    "Restart to finish updating to {}",
                    staged.version.as_deref().unwrap_or("the new version")
                ),
                staged.version.clone(),
                UpdateAvailability::Current,
            ),
            (None, Some(available)) if Some(available) != status.booted.version.as_ref() => (
                format!("{} · update available", status.booted.source),
                Some(available.clone()),
                UpdateAvailability::Available,
            ),
            (None, _) => (
                status.booted.source.clone(),
                None,
                UpdateAvailability::Current,
            ),
        };
        Package {
            id: PackageId {
                backend: ID.into(),
                name: NAME.into(),
                architecture: std::env::consts::ARCH.into(),
                scope: Scope::System,
                remote: None,
                reference: Some(tool.executable().into()),
            },
            display_name: format!("System image ({})", tool.executable()),
            summary,
            installed_version: Some(version),
            candidate_version: candidate,
            update,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
        }
    }
}

impl<T: Transport> Backend for SystemImage<T> {
    fn id(&self) -> &str {
        ID
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    #[cfg(not(target_os = "linux"))]
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Unavailable(
            "Image-based systems are Linux".into(),
        ))
    }
    #[cfg(target_os = "linux")]
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        Ok(match self.status(cancel)? {
            Some(_) => Availability::Available,
            None => Availability::Unavailable(
                "Not an image-based system (no bootc or rpm-ostree deployment)".into(),
            ),
        })
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .status(cancel)?
            .map(|(tool, status)| Self::package(tool, &status))
            .into_iter()
            .collect())
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let (tool, status) = self.status(cancel)?.ok_or(EngineError::NotFound)?;
        let package = Self::package(tool, &status);
        if package.id.name != id.name || id.backend != ID {
            return Err(EngineError::NotFound);
        }
        let describe = |label: &str, deployment: &Deployment| {
            let mut line = format!(
                "{label}: {} {}",
                deployment.source,
                deployment.version.as_deref().unwrap_or("")
            );
            if !deployment.packages.is_empty() {
                line.push_str(&format!(
                    "\nLayered packages: {}",
                    deployment.packages.join(", ")
                ));
            }
            line.trim_end().to_owned()
        };
        let mut description = vec![describe("Running", &status.booted)];
        if let Some(staged) = &status.staged {
            description.push(describe("After restart", staged));
        }
        if let Some(rollback) = &status.rollback {
            description.push(describe("Previous (rollback)", rollback));
        }
        description.push(format!(
            "Managed by {}. Updates download a new deployment that applies on the next restart; PkgDeck never restarts the computer.",
            tool.executable()
        ));
        Ok(PackageDetails {
            package,
            description: description.join("\n\n"),
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
            Operation::Upgrade(id) if id.backend == ID && id.name == NAME => {}
            Operation::UpgradeAll { backend } if backend == ID => {}
            Operation::Upgrade(_) => return Err(EngineError::NotFound),
            other => return Err(self.unsupported(other.capability())),
        }
        let (tool, before) = self.status(cancel)?.ok_or(EngineError::NotFound)?;
        if before.staged.is_some() {
            progress(Progress::Message(
                "An update is already waiting for a restart.".into(),
            ));
            return Ok(OperationOutcome::default());
        }
        progress(Progress::Message(format!(
            "Downloading the system update with {}. If you cancel, PkgDeck waits for it to finish.",
            tool.executable()
        )));
        let completion =
            self.transport
                .system_manager(tool.executable(), &["upgrade".into()], cancel, true)?;
        let deferred = completion.cancellation_deferred;
        bytes(ID, completion)?;
        let after = self
            .read(tool, &Cancellation::default())?
            .ok_or(EngineError::NotFound)?;
        progress(Progress::Message(match &after.staged {
            Some(staged) => format!(
                "Restart your computer to use {}. PkgDeck doesn't restart it for you.",
                staged.version.as_deref().unwrap_or("the update")
            ),
            None => "The system is up to date.".into(),
        }));
        Ok(OperationOutcome {
            cancellation_deferred: deferred,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::AptAction;
    use std::sync::{Arc, Mutex};

    const BOOTC_UPDATE: &str = r#"{"apiVersion":"org.containers.bootc/v1","kind":"BootcHost","spec":{"image":{"image":"quay.io/fedora/fedora-bootc:44","transport":"registry"}},
      "status":{"booted":{"image":{"image":{"image":"quay.io/fedora/fedora-bootc:44","transport":"registry"},"version":"44.20260920.0","imageDigest":"sha256:aaa"},
        "cachedUpdate":{"image":{"image":"quay.io/fedora/fedora-bootc:44","transport":"registry"},"version":"44.20260927.0","imageDigest":"sha256:bbb"},"pinned":false},
      "staged":null,"rollback":null,"type":"bootcHost"}}"#;
    const BOOTC_STAGED: &str = r#"{"status":{"booted":{"image":{"image":{"image":"quay.io/fedora/fedora-bootc:44"},"version":"44.20260920.0"}},
      "staged":{"image":{"image":{"image":"quay.io/fedora/fedora-bootc:44"},"version":"44.20260927.0"}},
      "rollback":{"image":{"image":{"image":"quay.io/fedora/fedora-bootc:44"},"version":"44.20260913.0"}}}}"#;
    const BOOTC_NOT_BOOTED: &str = r#"{"status":{"booted":null,"staged":null,"rollback":null}}"#;
    const RPM_OSTREE: &str = r#"{"deployments":[
        {"booted":false,"staged":true,"version":"44.20260927.0","origin":"fedora:fedora/44/x86_64/silverblue","requested-packages":["htop"],"osname":"fedora"},
        {"booted":true,"staged":false,"version":"44.20260920.0","container-image-reference":"ostree-image-signed:docker://quay.io/fedora-ostree-desktops/silverblue:44","requested-packages":["htop"],"osname":"fedora"},
        {"booted":false,"version":"44.20260913.0","origin":"fedora:fedora/44/x86_64/silverblue","osname":"fedora"}],
      "cached-update":null}"#;

    #[derive(Clone)]
    struct Fake {
        bootc: Option<&'static str>,
        ostree: Arc<Mutex<Option<&'static str>>>,
        calls: Arc<Mutex<Vec<String>>>,
        /// Every status read is cancelled.
        interrupted: bool,
        /// An upgrade finds nothing new and stages nothing.
        current: bool,
    }
    fn done(text: &str) -> Completion {
        Completion {
            code: Some(0),
            signal: None,
            stdout: text.as_bytes().to_vec(),
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
        }
    }
    impl Transport for Fake {
        fn apt_query(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &Cancellation,
        ) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
        fn apt_write(&self, _: AptAction, _: &Cancellation) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
        fn brew(
            &self,
            _: &[OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
        fn flatpak(
            &self,
            _: &[OsString],
            _: &Cancellation,
            _: bool,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
        fn system_manager(
            &self,
            executable: &str,
            args: &[OsString],
            _: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            self.calls
                .lock()
                .unwrap()
                .push(format!("{executable} {} write={write}", args.join(" ")));
            if self.interrupted {
                return Err(ExecutionError::Cancelled);
            }
            match (executable, write) {
                ("bootc", false) => self
                    .bootc
                    .map(done)
                    .ok_or_else(|| ExecutionError::Disabled("bootc not found".into())),
                ("rpm-ostree", false) => match *self.ostree.lock().unwrap() {
                    Some(json) => Ok(done(json)),
                    None => Err(ExecutionError::Failed(Completion {
                        code: Some(1),
                        stderr: b"error: This system was not booted via libostree.".to_vec(),
                        ..done("")
                    })),
                },
                ("rpm-ostree", true) => {
                    if !self.current {
                        *self.ostree.lock().unwrap() = Some(RPM_OSTREE);
                    }
                    Ok(done(""))
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    fn fake(bootc: Option<&'static str>, ostree: Option<&'static str>) -> Fake {
        Fake {
            bootc,
            ostree: Arc::new(Mutex::new(ostree)),
            calls: Arc::default(),
            interrupted: false,
            current: false,
        }
    }

    #[test]
    fn bootc_shows_available_and_staged_updates() {
        let mut image = SystemImage::new(fake(Some(BOOTC_UPDATE), None));
        let row = image.installed(&Cancellation::default()).unwrap().remove(0);
        assert_eq!(row.display_name, "System image (bootc)");
        assert_eq!(row.installed_version.as_deref(), Some("44.20260920.0"));
        assert_eq!(row.candidate_version.as_deref(), Some("44.20260927.0"));
        assert_eq!(row.update, UpdateAvailability::Available);
        let mut staged = SystemImage::new(fake(Some(BOOTC_STAGED), None));
        let row = staged
            .installed(&Cancellation::default())
            .unwrap()
            .remove(0);
        assert_eq!(row.summary, "Restart to finish updating to 44.20260927.0");
        assert_eq!(row.update, UpdateAvailability::Current);
        let details = staged.details(&row.id, &Cancellation::default()).unwrap();
        assert!(details
            .description
            .contains("Previous (rollback): quay.io/fedora/fedora-bootc:44 44.20260913.0"));
        assert!(details.description.contains("never restarts"));
    }

    #[test]
    fn rpm_ostree_when_bootc_has_no_booted_image() {
        let fake = fake(Some(BOOTC_NOT_BOOTED), Some(RPM_OSTREE));
        let mut image = SystemImage::new(fake);
        let row = image.installed(&Cancellation::default()).unwrap().remove(0);
        assert_eq!(row.display_name, "System image (rpm-ostree)");
        assert_eq!(row.installed_version.as_deref(), Some("44.20260920.0"));
        assert_eq!(row.summary, "Restart to finish updating to 44.20260927.0");
        let details = image.details(&row.id, &Cancellation::default()).unwrap();
        assert!(details.description.contains("Running: ostree-image-signed:docker://quay.io/fedora-ostree-desktops/silverblue:44 44.20260920.0\nLayered packages: htop"));
        assert!(details
            .description
            .contains("Previous (rollback): fedora:fedora/44/x86_64/silverblue 44.20260913.0"));
    }

    #[test]
    fn updates_stage_a_deployment_and_never_restart() {
        let current = r#"{"deployments":[{"booted":true,"version":"44.20260920.0","origin":"fedora:fedora/44/x86_64/silverblue"}],"cached-update":{"version":"44.20260927.0"}}"#;
        let fake = fake(None, Some(current));
        let mut image = SystemImage::new(fake.clone());
        let row = image.installed(&Cancellation::default()).unwrap().remove(0);
        assert_eq!(row.update, UpdateAvailability::Available);
        let mut messages = vec![];
        image
            .execute(
                &Operation::Upgrade(row.id.clone()),
                &Cancellation::default(),
                &mut |p| {
                    if let Progress::Message(m) = p {
                        messages.push(m)
                    }
                },
            )
            .unwrap();
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"rpm-ostree upgrade write=true".into()));
        assert!(messages
            .last()
            .unwrap()
            .contains("Restart your computer to use 44.20260927.0"));
        // A second update while one waits for a restart does nothing.
        fake.calls.lock().unwrap().clear();
        image
            .execute(
                &Operation::Upgrade(row.id),
                &Cancellation::default(),
                &mut |_| {},
            )
            .unwrap();
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains("write=true")));
    }

    #[test]
    fn ordinary_systems_are_unavailable() {
        let mut image = SystemImage::new(fake(None, None));
        let availability = image.detect(&Cancellation::default()).unwrap();
        if cfg!(target_os = "linux") {
            assert!(
                matches!(availability, Availability::Unavailable(reason) if reason.contains("Not an image-based system"))
            );
        } else {
            assert_eq!(
                availability,
                Availability::Unavailable("Image-based systems are Linux".into())
            );
        }
        assert!(image
            .installed(&Cancellation::default())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_cancelled_status_read_stops_instead_of_trying_the_next_tool() {
        let mut fake = fake(Some(BOOTC_UPDATE), Some(RPM_OSTREE));
        fake.interrupted = true;
        let mut image = SystemImage::new(fake.clone());
        assert!(matches!(
            image.installed(&Cancellation::default()),
            Err(EngineError::Cancelled)
        ));
        assert_eq!(fake.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn only_the_system_row_can_be_updated() {
        let mut image = SystemImage::new(fake(Some(BOOTC_UPDATE), None));
        if cfg!(target_os = "linux") {
            assert_eq!(
                image.detect(&Cancellation::default()).unwrap(),
                Availability::Available
            );
        }
        let row = image.installed(&Cancellation::default()).unwrap().remove(0);
        let mut other = row.id.clone();
        other.name = "kernel".into();
        let cancel = Cancellation::default();
        assert!(matches!(
            image.execute(&Operation::Upgrade(other.clone()), &cancel, &mut |_| {}),
            Err(EngineError::NotFound)
        ));
        assert!(matches!(
            image.details(&other, &cancel),
            Err(EngineError::NotFound)
        ));
        assert!(matches!(
            image.execute(&Operation::Remove(row.id.clone()), &cancel, &mut |_| {}),
            Err(EngineError::Unsupported { .. })
        ));
        assert!(matches!(
            image.execute(
                &Operation::UpgradeAll {
                    backend: "flatpak".into()
                },
                &cancel,
                &mut |_| {}
            ),
            Err(EngineError::Unsupported { .. })
        ));
        // Without an image-based system there is nothing to describe or update.
        let mut ordinary = SystemImage::new(fake(None, None));
        assert!(matches!(
            ordinary.details(&row.id, &cancel),
            Err(EngineError::NotFound)
        ));
        assert!(matches!(
            ordinary.execute(
                &Operation::UpgradeAll { backend: ID.into() },
                &cancel,
                &mut |_| {}
            ),
            Err(EngineError::NotFound)
        ));
    }

    #[test]
    fn sparse_status_still_names_the_deployment() {
        let bootc = parse_bootc(&serde_json::json!({"status": {"booted": {"image": {}}}})).unwrap();
        assert_eq!(bootc.booted.source, "bootc image");
        assert_eq!(bootc.booted.version, None);
        // The cached update is known by digest when it has no version.
        let digest = parse_bootc(&serde_json::json!({"status": {"booted": {
            "image": {"version": "44.1"},
            "cachedUpdate": {"imageDigest": "sha256:bbb"}
        }}}))
        .unwrap();
        assert_eq!(digest.available.as_deref(), Some("sha256:bbb"));
        let ostree = parse_rpm_ostree(&serde_json::json!({"deployments": [
            {"booted": true, "osname": "fedora"},
            {"booted": false, "requested-packages": ["htop", ""]}
        ]}))
        .unwrap();
        assert_eq!(ostree.booted.source, "fedora");
        let rollback = ostree.rollback.unwrap();
        assert_eq!(rollback.source, "ostree deployment");
        assert_eq!(rollback.packages, vec!["htop"]);
        assert!(
            parse_rpm_ostree(&serde_json::json!({"deployments": [{"booted": false}]})).is_none()
        );
        // An update check that found the running version is no update.
        let status = Status {
            booted: Deployment {
                source: "fedora".into(),
                version: Some("44.1".into()),
                packages: vec![],
            },
            available: Some("44.1".into()),
            ..Status::default()
        };
        let row = SystemImage::<Fake>::package(Tool::RpmOstree, &status);
        assert_eq!(row.update, UpdateAvailability::Current);
        assert_eq!(row.summary, "fedora");
    }

    #[test]
    fn an_update_that_stages_nothing_says_the_system_is_current() {
        let current = r#"{"deployments":[{"booted":true,"version":"44.20260920.0","origin":"fedora:fedora/44/x86_64/silverblue"}],"cached-update":null}"#;
        let mut fake = fake(None, Some(current));
        fake.current = true;
        let mut image = SystemImage::new(fake);
        let row = image.installed(&Cancellation::default()).unwrap().remove(0);
        let mut messages = vec![];
        image
            .execute(
                &Operation::Upgrade(row.id),
                &Cancellation::default(),
                &mut |p| {
                    if let Progress::Message(m) = p {
                        messages.push(m)
                    }
                },
            )
            .unwrap();
        assert_eq!(messages.last().unwrap(), "The system is up to date.");
    }
}
