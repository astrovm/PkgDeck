//! Docker and Podman image adapters using their structured CLI output.
use super::Transport;
use crate::{engine::*, package::*, process::*};
use serde_json::Value;
use std::{collections::BTreeMap, ffi::OsString};

const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Upgrade,
];
const IMAGE_FORMAT: &str = r#"{"ID":{{json .ID}},"Repository":{{json .Repository}},"Tag":{{json .Tag}},"Digest":{{json .Digest}},"Size":{{json .Size}},"CreatedSince":{{json .CreatedSince}}}"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainerKind {
    Docker,
    Podman,
}

impl ContainerKind {
    fn id(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Docker => "Docker",
            Self::Podman => "Podman",
        }
    }
    fn scope(self) -> Scope {
        match self {
            // Docker's system daemon owns the image store even when its socket
            // grants an unprivileged caller access.
            Self::Docker => Scope::System,
            Self::Podman => Scope::User {
                uid: rustix::process::getuid().as_raw(),
            },
        }
    }
    fn store(self) -> &'static str {
        match self {
            Self::Docker => "Docker daemon",
            Self::Podman => "rootless Podman storage",
        }
    }
}

pub struct Container<T> {
    transport: T,
    kind: ContainerKind,
    remote_offers: bool,
}

impl<T> Container<T> {
    pub fn docker(transport: T) -> Self {
        Self {
            transport,
            kind: ContainerKind::Docker,
            remote_offers: false,
        }
    }
    pub fn podman(transport: T) -> Self {
        Self {
            transport,
            kind: ContainerKind::Podman,
            remote_offers: false,
        }
    }
    pub fn with_remote_offers(mut self, enabled: bool) -> Self {
        self.remote_offers = enabled;
        self
    }
}

#[derive(Default)]
struct Image {
    id: String,
    references: Vec<String>,
    digests: Vec<String>,
    size: String,
    created: String,
}

fn field<'a>(value: &'a Value, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty() && *text != "<none>" && *text != "<none>:<none>")
}

fn safe_id(value: &str) -> bool {
    let hex = value.strip_prefix("sha256:").unwrap_or(value);
    hex.len() >= 12 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn safe_reference(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.chars().any(char::is_whitespace)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/:@[]".contains(&byte))
}

fn short_id(id: &str) -> &str {
    let id = id.strip_prefix("sha256:").unwrap_or(id);
    id.get(..12).unwrap_or(id)
}

impl<T: Transport> Container<T> {
    fn call(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, EngineError> {
        Ok(self.transport.container(
            self.kind.id(),
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            cancel,
            write,
        )?)
    }

    fn inventory(&self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let result = self.call(
            &[
                "image",
                "ls",
                "--all",
                "--digests",
                "--no-trunc",
                "--format",
                IMAGE_FORMAT,
            ],
            cancel,
            false,
        )?;
        if result.code != Some(0) {
            return Err(ExecutionError::Failed(result).into());
        }
        let text =
            String::from_utf8(result.stdout).map_err(|error| EngineError::InvalidResponse {
                backend: self.kind.id().into(),
                reason: error.to_string(),
            })?;
        let mut images: BTreeMap<String, Image> = BTreeMap::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let value: Value =
                serde_json::from_str(line).map_err(|error| EngineError::InvalidResponse {
                    backend: self.kind.id().into(),
                    reason: format!("invalid image metadata: {error}"),
                })?;
            let id = field(&value, &["ID", "Id", "IDString"])
                .filter(|id| safe_id(id))
                .ok_or_else(|| EngineError::InvalidResponse {
                    backend: self.kind.id().into(),
                    reason: "image metadata has no valid immutable ID".into(),
                })?
                .to_owned();
            let image = images.entry(id.clone()).or_insert_with(|| Image {
                id,
                ..Image::default()
            });
            let repository = field(&value, &["Repository"]);
            let tag = field(&value, &["Tag"]);
            if let (Some(repository), Some(tag)) = (repository, tag) {
                let reference = format!("{repository}:{tag}");
                if safe_reference(&reference) && !image.references.contains(&reference) {
                    image.references.push(reference);
                }
            }
            if let Some(digest) = field(&value, &["Digest"]) {
                if !image.digests.iter().any(|existing| existing == digest) {
                    image.digests.push(digest.into());
                }
            }
            if image.size.is_empty() {
                image.size = field(&value, &["Size", "VirtualSize"])
                    .unwrap_or("Size unavailable")
                    .into();
            }
            if image.created.is_empty() {
                image.created = field(&value, &["CreatedSince", "CreatedAt", "Created"])
                    .unwrap_or("Creation time unavailable")
                    .into();
            }
        }
        Ok(images
            .into_values()
            .map(|mut image| {
                image.references.sort();
                image.digests.sort();
                let reference = image.references.first().cloned();
                let display_name = reference
                    .clone()
                    .unwrap_or_else(|| format!("Untagged image {}", short_id(&image.id)));
                let status = if image.references.is_empty() {
                    "Dangling".into()
                } else {
                    format!("Tags: {}", image.references.join(", "))
                };
                let digest = if image.digests.is_empty() {
                    String::new()
                } else {
                    format!(" · Digests: {}", image.digests.join(", "))
                };
                Package {
                    id: PackageId {
                        backend: self.kind.id().into(),
                        name: image.id.clone(),
                        architecture: std::env::consts::ARCH.into(),
                        scope: self.kind.scope(),
                        remote: Some(self.kind.store().into()),
                        reference,
                    },
                    display_name,
                    summary: format!("{status}{digest} · {} · {}", image.size, image.created),
                    installed_version: Some(short_id(&image.id).into()),
                    // A registry digest is not queried during inventory: claiming
                    // an update from a mutable tag would be misleading. Pull stays
                    // available as an explicit refresh action in the Installed view.
                    candidate_version: None,
                    update: UpdateAvailability::Unknown,
                    icon: None,
                    component_ids: vec![],
                    homepages: vec![],
                }
            })
            .collect())
    }

    fn validate_local(&self, id: &PackageId) -> Result<(), EngineError> {
        if id.backend != self.kind.id()
            || id.scope != self.kind.scope()
            || !safe_id(&id.name)
            || id
                .reference
                .as_deref()
                .is_some_and(|value| !safe_reference(value))
        {
            return Err(EngineError::InvalidResponse {
                backend: self.kind.id().into(),
                reason: "foreign or invalid container image identity".into(),
            });
        }
        Ok(())
    }
    fn validate_pull(&self, id: &PackageId) -> Result<(), EngineError> {
        if id.backend != self.kind.id()
            || id.scope != self.kind.scope()
            || id.reference.as_deref() != Some(id.name.as_str())
            || !safe_reference(&id.name)
        {
            return Err(EngineError::InvalidResponse {
                backend: self.kind.id().into(),
                reason: "foreign or invalid container pull reference".into(),
            });
        }
        Ok(())
    }
}

impl<T: Transport> Backend for Container<T> {
    fn id(&self) -> &str {
        self.kind.id()
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        match self.call(&["info", "--format", "{{json .}}"], cancel, false) {
            Ok(_) => Ok(Availability::Available),
            Err(EngineError::Execution(ExecutionError::Disabled(reason))) => {
                Ok(Availability::Unavailable(reason))
            }
            Err(EngineError::Execution(ExecutionError::Failed(result))) => {
                let diagnostic = if result.stderr.is_empty() {
                    &result.stdout
                } else {
                    &result.stderr
                };
                let diagnostic = String::from_utf8_lossy(diagnostic);
                let diagnostic = diagnostic.lines().next().unwrap_or("engine is not ready");
                Ok(Availability::Unavailable(format!(
                    "{} is installed but unavailable: {diagnostic}",
                    self.kind.label()
                )))
            }
            Err(error) => Err(error),
        }
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let query = query.trim();
        let folded = query.to_ascii_lowercase();
        let mut packages: Vec<_> = self
            .inventory(cancel)?
            .into_iter()
            .filter(|image| {
                image.id.name.to_ascii_lowercase().contains(&folded)
                    || image.display_name.to_ascii_lowercase().contains(&folded)
                    || image.summary.to_ascii_lowercase().contains(&folded)
            })
            .collect();
        if self.remote_offers
            && safe_reference(query)
            && !packages
                .iter()
                .any(|image| image.id.reference.as_deref() == Some(query))
        {
            packages.push(Package {
                id: PackageId {
                    backend: self.kind.id().into(),
                    name: query.into(),
                    architecture: std::env::consts::ARCH.into(),
                    scope: self.kind.scope(),
                    remote: Some("Container registry".into()),
                    reference: Some(query.into()),
                },
                display_name: query.into(),
                summary: format!("Pull image with {}", self.kind.label()),
                installed_version: None,
                candidate_version: Some("Registry".into()),
                update: UpdateAvailability::Unknown,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
            });
        }
        Ok(packages)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.inventory(cancel)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        if id.reference.as_deref() == Some(id.name.as_str()) {
            self.validate_pull(id)?;
            return Ok(PackageDetails {
                package: Package {
                    id: id.clone(),
                    display_name: id.name.clone(),
                    summary: format!("Pull image with {}", self.kind.label()),
                    installed_version: None,
                    candidate_version: Some("Registry".into()),
                    update: UpdateAvailability::Unknown,
                    icon: None,
                    component_ids: vec![],
                    homepages: vec![],
                },
                description: format!(
                    "Pull {} into {}. The container engine resolves the registry and verifies its native content digest.",
                    id.name,
                    self.kind.store()
                ),
                homepage: None,
                dependencies: vec![],
            });
        }
        self.validate_local(id)?;
        let package = self
            .inventory(cancel)?
            .into_iter()
            .find(|package| package.id == *id)
            .ok_or(EngineError::NotFound)?;
        let reference = package.id.reference.as_deref().unwrap_or("No tag");
        Ok(PackageDetails {
            description: format!(
                "{} image in {}. Immutable ID: {}. Pull reference: {reference}.",
                self.kind.label(),
                self.kind.store(),
                package.id.name
            ),
            homepage: None,
            dependencies: vec![],
            package,
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let id = match operation {
            Operation::Install(id) | Operation::Remove(id) | Operation::Upgrade(id) => id,
            _ => return Err(self.unsupported(operation.capability())),
        };
        if matches!(operation, Operation::Install(_)) {
            self.validate_pull(id)?;
        } else {
            self.validate_local(id)?;
        }
        let args = match operation {
            Operation::Remove(_) => vec!["image", "rm", id.name.as_str()],
            Operation::Install(_) | Operation::Upgrade(_) => {
                let reference =
                    id.reference
                        .as_deref()
                        .ok_or_else(|| EngineError::InvalidResponse {
                            backend: self.kind.id().into(),
                            reason: "a tagged image is required for pull".into(),
                        })?;
                vec!["pull", reference]
            }
            _ => unreachable!(),
        };
        progress(Progress::Message(match operation {
            Operation::Remove(_) if id.reference.is_none() => {
                format!(
                    "Removing dangling {} image {}",
                    self.kind.label(),
                    short_id(&id.name)
                )
            }
            Operation::Remove(_) => format!("Removing {} image", self.kind.label()),
            _ => format!(
                "Pulling {} with {}",
                id.reference.as_deref().unwrap(),
                self.kind.label()
            ),
        }));
        let result = self.call(&args, cancel, true)?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{backends::Transport, host::AptAction};
    use std::sync::{Arc, Mutex};

    type RecordedCall = (String, Vec<OsString>, bool);

    #[derive(Clone)]
    struct Fixture {
        output: Vec<u8>,
        code: Option<i32>,
        failure: Option<ExecutionError>,
        calls: Arc<Mutex<Vec<RecordedCall>>>,
    }
    impl Fixture {
        fn new(output: &str) -> Self {
            Self {
                output: output.as_bytes().into(),
                code: Some(0),
                failure: None,
                calls: Arc::new(Mutex::new(vec![])),
            }
        }
        fn bytes(output: &[u8]) -> Self {
            Self {
                output: output.into(),
                ..Self::new("")
            }
        }
        fn failed(error: ExecutionError) -> Self {
            Self {
                failure: Some(error),
                ..Self::new("")
            }
        }
        fn with_code(mut self, code: i32) -> Self {
            self.code = Some(code);
            self
        }
    }
    fn completed(output: &[u8], code: Option<i32>) -> Completion {
        Completion {
            code,
            signal: None,
            stdout: output.into(),
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
        }
    }
    impl Transport for Fixture {
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
        fn container(
            &self,
            executable: &str,
            args: &[OsString],
            cancel: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            if cancel.requested() {
                return Err(ExecutionError::Cancelled);
            }
            if let Some(error) = &self.failure {
                return Err(error.clone());
            }
            self.calls
                .lock()
                .unwrap()
                .push((executable.into(), args.into(), write));
            Ok(completed(
                if write || args.first().is_some_and(|arg| arg == "info") {
                    &[]
                } else {
                    &self.output
                },
                self.code,
            ))
        }
    }

    #[test]
    fn identities_and_references_reject_option_injection() {
        assert!(safe_id("sha256:0123456789abcdef"));
        assert!(!safe_id("--force"));
        assert!(safe_reference("registry.example/team/app:latest"));
        assert!(!safe_reference("--all"));
        assert!(!safe_reference("app name:latest"));
    }

    #[test]
    fn docker_inventory_groups_tags_by_immutable_id_and_removes_exactly() {
        let fixture = Fixture::new(
            r#"{"ID":"sha256:0123456789abcdef","Repository":"registry.example/app","Tag":"latest","Digest":"sha256:aaaa","Size":"42MB","CreatedSince":"2 days ago"}
{"ID":"sha256:0123456789abcdef","Repository":"registry.example/app","Tag":"stable","Digest":"sha256:aaaa","Size":"42MB","CreatedSince":"2 days ago"}
{"ID":"sha256:fedcba9876543210","Repository":"<none>","Tag":"<none>","Digest":"<none>","Size":"9MB","CreatedSince":"3 days ago"}"#,
        );
        let calls = fixture.calls.clone();
        let mut backend = Container::docker(fixture);
        let cancel = Cancellation::default();
        assert_eq!(backend.detect(&cancel).unwrap(), Availability::Available);
        let images = backend.installed(&cancel).unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].id.scope, Scope::System);
        assert_eq!(
            images[0].id.reference.as_deref(),
            Some("registry.example/app:latest")
        );
        assert!(images[0].summary.contains("registry.example/app:stable"));
        assert!(images[1].summary.starts_with("Dangling"));
        backend
            .execute(
                &Operation::Remove(images[1].id.clone()),
                &cancel,
                &mut |_| {},
            )
            .unwrap();
        let calls = calls.lock().unwrap();
        let (_, args, write) = calls.last().unwrap();
        assert!(*write);
        assert_eq!(args, &["image", "rm", "sha256:fedcba9876543210"]);
    }

    #[test]
    fn podman_pull_is_user_scoped_and_cancellation_is_typed() {
        let fixture = Fixture::new(
            r#"{"ID":"0123456789abcdef","Repository":"quay.io/team/app","Tag":"1","Digest":"sha256:bbbb","Size":"12MB","CreatedSince":"now"}"#,
        );
        let calls = fixture.calls.clone();
        let mut backend = Container::podman(fixture);
        let cancel = Cancellation::default();
        let image = backend.installed(&cancel).unwrap().remove(0);
        assert!(matches!(image.id.scope, Scope::User { .. }));
        backend
            .execute(&Operation::Upgrade(image.id), &cancel, &mut |_| {})
            .unwrap();
        let calls = calls.lock().unwrap();
        let (engine, args, write) = calls.last().unwrap();
        assert_eq!(engine, "podman");
        assert_eq!(args, &["pull", "quay.io/team/app:1"]);
        assert!(*write);
        drop(calls);
        cancel.cancel();
        assert_eq!(backend.installed(&cancel), Err(EngineError::Cancelled));
    }

    #[test]
    fn explicit_container_search_can_pull_a_new_reference_without_shells() {
        let fixture = Fixture::new("");
        let calls = fixture.calls.clone();
        let mut backend = Container::docker(fixture).with_remote_offers(true);
        let cancel = Cancellation::default();
        let offer = backend
            .search("registry.example/team/App:Preview", &cancel)
            .unwrap()
            .remove(0);
        assert!(offer.installed_version.is_none());
        assert_eq!(
            offer.id.reference.as_deref(),
            Some("registry.example/team/App:Preview")
        );
        assert!(backend
            .details(&offer.id, &cancel)
            .unwrap()
            .description
            .contains("registry.example/team/App:Preview"));
        backend
            .execute(&Operation::Install(offer.id), &cancel, &mut |_| {})
            .unwrap();
        let calls = calls.lock().unwrap();
        let (_, args, write) = calls.last().unwrap();
        assert_eq!(args, &["pull", "registry.example/team/App:Preview"]);
        assert!(*write);
    }

    #[test]
    fn detection_distinguishes_missing_stopped_and_broken_engines() {
        let cancel = Cancellation::default();
        let mut missing = Container::docker(Fixture::failed(ExecutionError::Disabled(
            "Docker not found".into(),
        )));
        assert_eq!(
            missing.detect(&cancel).unwrap(),
            Availability::Unavailable("Docker not found".into())
        );

        let mut stopped_result = completed(&[], Some(1));
        stopped_result.stderr = b"cannot connect to daemon\nmore detail".into();
        let mut stopped =
            Container::docker(Fixture::failed(ExecutionError::Failed(stopped_result)));
        assert!(matches!(
            stopped.detect(&cancel).unwrap(),
            Availability::Unavailable(reason)
                if reason == "Docker is installed but unavailable: cannot connect to daemon"
        ));

        let mut broken = Container::podman(Fixture::failed(ExecutionError::Io(
            "synthetic transport failure".into(),
        )));
        assert!(matches!(
            broken.detect(&cancel),
            Err(EngineError::Execution(ExecutionError::Io(message)))
                if message == "synthetic transport failure"
        ));
    }

    #[test]
    fn malformed_inventory_is_never_an_empty_success() {
        let cancel = Cancellation::default();
        let cases = [
            Fixture::new("").with_code(1),
            Fixture::bytes(&[0xff, 0xfe]),
            Fixture::new("not-json"),
            Fixture::new(r#"{"Repository":"example/app","Tag":"latest"}"#),
        ];
        for fixture in cases {
            let mut backend = Container::docker(fixture);
            assert!(backend.installed(&cancel).is_err());
        }
    }

    #[test]
    fn details_search_and_operations_preserve_exact_local_identity() {
        let fixture = Fixture::new(
            r#"{"ID":"sha256:0123456789abcdef","Repository":"example/app","Tag":"latest","Digest":"sha256:aaaa","Size":"42MB","CreatedSince":"2 days ago"}"#,
        );
        let calls = fixture.calls.clone();
        let mut backend = Container::docker(fixture).with_remote_offers(true);
        let cancel = Cancellation::default();
        assert!(backend.search("does-not-match", &cancel).unwrap().len() == 1);
        let packages = backend.search("example/app:latest", &cancel).unwrap();
        assert_eq!(
            packages.len(),
            1,
            "an installed exact tag is not offered twice"
        );
        let package = packages[0].clone();
        let details = backend.details(&package.id, &cancel).unwrap();
        assert!(details.description.contains("Immutable ID"));
        assert!(details.description.contains("example/app:latest"));

        let mut progress = vec![];
        backend
            .execute(
                &Operation::Remove(package.id.clone()),
                &cancel,
                &mut |message| progress.push(message),
            )
            .unwrap();
        assert!(
            matches!(&progress[0], Progress::Message(message) if message == "Removing Docker image")
        );
        let (_, args, write) = calls.lock().unwrap().last().unwrap().clone();
        assert_eq!(args, &["image", "rm", "sha256:0123456789abcdef"]);
        assert!(write);

        let mut untagged = package.id.clone();
        untagged.reference = None;
        assert!(matches!(
            backend.execute(&Operation::Upgrade(untagged), &cancel, &mut |_| {}),
            Err(EngineError::InvalidResponse { reason, .. })
                if reason == "a tagged image is required for pull"
        ));
        assert!(matches!(
            backend.execute(
                &Operation::Refresh {
                    backend: "docker".into()
                },
                &cancel,
                &mut |_| {}
            ),
            Err(EngineError::Unsupported { .. })
        ));
    }

    #[test]
    fn foreign_and_option_like_operations_fail_before_transport() {
        let mut backend = Container::podman(Fixture::new(""));
        let cancel = Cancellation::default();
        let base = PackageId {
            backend: "podman".into(),
            name: "0123456789abcdef".into(),
            architecture: "x86_64".into(),
            scope: Scope::User {
                uid: rustix::process::getuid().as_raw(),
            },
            remote: Some("rootless Podman storage".into()),
            reference: Some("example/app:latest".into()),
        };
        for id in [
            PackageId {
                backend: "docker".into(),
                ..base.clone()
            },
            PackageId {
                reference: Some("--all".into()),
                ..base.clone()
            },
            PackageId {
                name: "--force".into(),
                ..base
            },
        ] {
            assert!(backend
                .execute(&Operation::Remove(id), &cancel, &mut |_| {})
                .is_err());
        }
    }
}
