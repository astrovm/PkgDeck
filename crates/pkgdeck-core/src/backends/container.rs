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
    Capability::Clean,
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
    cleanup_images: Option<Vec<String>>,
    cleanup_build_cache: Option<Vec<String>>,
    /// The stopped containers, unused image tags and unused volumes the
    /// last preview listed, which a cleanup must find unchanged.
    cleanup_containers: Option<Vec<String>>,
    cleanup_unused_images: Option<Vec<String>>,
    cleanup_volumes: Option<Vec<String>>,
}

impl<T> Container<T> {
    pub fn docker(transport: T) -> Self {
        Self {
            transport,
            kind: ContainerKind::Docker,
            remote_offers: false,
            cleanup_images: None,
            cleanup_build_cache: None,
            cleanup_containers: None,
            cleanup_unused_images: None,
            cleanup_volumes: None,
        }
    }
    pub fn podman(transport: T) -> Self {
        Self {
            transport,
            kind: ContainerKind::Podman,
            remote_offers: false,
            cleanup_images: None,
            cleanup_build_cache: None,
            cleanup_containers: None,
            cleanup_unused_images: None,
            cleanup_volumes: None,
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

/// An image ID without its `sha256:` prefix, which Podman leaves out.
fn bare_id(id: &str) -> &str {
    id.strip_prefix("sha256:").unwrap_or(id)
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

    // Pin the default builder for both preview and execution; changing the user's
    // selected builder must never redirect an already approved cleanup.
    /// The reclaimable records and, when every one says, what they take.
    fn build_cache(
        &self,
        cancel: &Cancellation,
    ) -> Result<(Vec<String>, Option<u64>), EngineError> {
        let result = self.call(
            &["buildx", "du", "--builder", "default", "--format=json"],
            cancel,
            false,
        )?;
        if result.code != Some(0) || result.truncated {
            return Err(ExecutionError::Failed(result).into());
        }
        let text =
            String::from_utf8(result.stdout).map_err(|error| EngineError::InvalidResponse {
                backend: self.kind.id().into(),
                reason: error.to_string(),
            })?;
        let mut ids = Vec::new();
        let mut bytes = Some(0);
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let value: Value =
                serde_json::from_str(line).map_err(|error| EngineError::InvalidResponse {
                    backend: self.kind.id().into(),
                    reason: error.to_string(),
                })?;
            if value.get("Reclaimable").and_then(Value::as_bool) != Some(true)
                || value.get("Mutable").and_then(Value::as_bool) != Some(false)
                || matches!(field(&value, &["Type"]), Some("internal" | "frontend"))
            {
                continue;
            }
            let id = field(&value, &["ID"])
                .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric()))
                .ok_or_else(|| EngineError::InvalidResponse {
                    backend: self.kind.id().into(),
                    reason: "invalid build cache ID".into(),
                })?;
            // A byte count, or a size as `docker` prints one ("4.096kB").
            let size = match value.get("Size") {
                Some(Value::Number(size)) => size.as_u64(),
                Some(Value::String(size)) => crate::disk::parse_size(size),
                _ => None,
            };
            bytes = bytes.zip(size).map(|(total, size)| total + size);
            ids.push(id.to_owned());
        }
        ids.sort();
        ids.dedup();
        Ok((ids, bytes))
    }

    /// A read that must succeed in full, as text.
    fn text(&self, args: &[&str], cancel: &Cancellation) -> Result<String, EngineError> {
        let result = self.call(args, cancel, false)?;
        if result.code != Some(0) || result.truncated {
            return Err(ExecutionError::Failed(result).into());
        }
        String::from_utf8(result.stdout).map_err(|error| EngineError::InvalidResponse {
            backend: self.kind.id().into(),
            reason: error.to_string(),
        })
    }

    /// The sum of the byte counts `args` followed by `ids` prints, one per
    /// line, such as image or container sizes. Unknown when any is missing.
    fn total(&self, args: &[&str], ids: &[String], cancel: &Cancellation) -> Option<u64> {
        let mut args = args.to_vec();
        args.extend(ids.iter().map(String::as_str));
        self.text(&args, cancel)
            .ok()?
            .lines()
            .map(|line| line.trim().parse::<u64>().ok())
            .sum()
    }

    /// Containers that aren't running, by full ID, each with how to show it:
    /// "name (image)".
    fn stopped_containers(
        &self,
        cancel: &Cancellation,
    ) -> Result<Vec<(String, String)>, EngineError> {
        let text = self.text(
            &[
                "container",
                "ls",
                "--all",
                "--no-trunc",
                "--filter",
                "status=exited",
                "--filter",
                "status=created",
                "--format",
                "{{.ID}}\t{{.Names}}\t{{.Image}}",
            ],
            cancel,
        )?;
        let mut containers = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let mut fields = line.split('\t').map(str::trim);
            let id = fields.next().unwrap_or_default();
            if !safe_id(id) {
                return Err(EngineError::InvalidResponse {
                    backend: self.kind.id().into(),
                    reason: "invalid container ID".into(),
                });
            }
            let name = fields
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or(short_id(id));
            let shown = match fields.next().filter(|image| !image.is_empty()) {
                Some(image) => format!("{name} ({image})"),
                None => name.to_owned(),
            };
            containers.push((id.to_owned(), shown));
        }
        containers.sort();
        containers.dedup();
        Ok(containers)
    }

    /// The images some container, running or not, was made from.
    fn images_in_use(&self, cancel: &Cancellation) -> Result<Vec<String>, EngineError> {
        let containers = self.text(
            &["container", "ls", "--all", "--no-trunc", "--quiet"],
            cancel,
        )?;
        let containers: Vec<_> = containers
            .lines()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .collect();
        if containers.is_empty() {
            return Ok(vec![]);
        }
        if !containers.iter().all(|id| safe_id(id)) {
            return Err(EngineError::InvalidResponse {
                backend: self.kind.id().into(),
                reason: "invalid container ID".into(),
            });
        }
        let mut args = vec!["container", "inspect", "--format", "{{.Image}}"];
        args.extend(containers);
        Ok(self
            .text(&args, cancel)?
            .lines()
            .map(|id| bare_id(id.trim()).to_owned())
            .filter(|id| !id.is_empty())
            .collect())
    }

    /// Tagged images no container uses, as `repository:tag` with the image
    /// ID behind each. Untagged ones are the dangling-image task's.
    fn unused_images(&self, cancel: &Cancellation) -> Result<Vec<(String, String)>, EngineError> {
        let used = self.images_in_use(cancel)?;
        let text = self.text(
            &[
                "image",
                "ls",
                "--no-trunc",
                "--format",
                "{{.ID}}\t{{.Repository}}:{{.Tag}}\t{{.Digest}}",
                "--digests",
            ],
            cancel,
        )?;
        let mut images = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let mut fields = line.split('\t').map(str::trim);
            let (id, reference) = (
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default(),
            );
            if !safe_id(id) {
                return Err(EngineError::InvalidResponse {
                    backend: self.kind.id().into(),
                    reason: "invalid image ID".into(),
                });
            }
            // A newer pull takes the tag from the image before it, which
            // keeps its name and digest: it goes by those.
            let digest = fields.next().unwrap_or_default();
            let untagged = reference
                .strip_suffix(":<none>")
                .filter(|name| *name != "<none>" && digest.starts_with("sha256:"))
                .map(|name| format!("{name}@{digest}"));
            let reference = untagged.as_deref().unwrap_or(reference);
            if reference.contains("<none>")
                || !safe_reference(reference)
                || used.iter().any(|used| used == bare_id(id))
            {
                continue;
            }
            images.push((reference.to_owned(), id.to_owned()));
        }
        images.sort();
        images.dedup();
        Ok(images)
    }

    /// Volumes no container uses, by name.
    fn unused_volumes(&self, cancel: &Cancellation) -> Result<Vec<String>, EngineError> {
        let text = self.text(
            &["volume", "ls", "--quiet", "--filter", "dangling=true"],
            cancel,
        )?;
        let mut volumes = Vec::new();
        for name in text.lines().map(str::trim).filter(|name| !name.is_empty()) {
            if !name.starts_with(|c: char| c.is_ascii_alphanumeric())
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
            {
                return Err(EngineError::InvalidResponse {
                    backend: self.kind.id().into(),
                    reason: "invalid volume name".into(),
                });
            }
            volumes.push(name.to_owned());
        }
        volumes.sort();
        volumes.dedup();
        Ok(volumes)
    }

    fn dangling_images(&self, cancel: &Cancellation) -> Result<Vec<String>, EngineError> {
        let result = self.call(
            &[
                "image",
                "ls",
                "--filter",
                "dangling=true",
                "--no-trunc",
                "--quiet",
            ],
            cancel,
            false,
        )?;
        if result.code != Some(0) || result.truncated {
            return Err(ExecutionError::Failed(result).into());
        }
        let text =
            String::from_utf8(result.stdout).map_err(|error| EngineError::InvalidResponse {
                backend: self.kind.id().into(),
                reason: error.to_string(),
            })?;
        let mut ids = Vec::new();
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            if !safe_id(line) {
                return Err(EngineError::InvalidResponse {
                    backend: self.kind.id().into(),
                    reason: "invalid dangling image ID".into(),
                });
            }
            ids.push(line.to_owned());
        }
        ids.sort();
        ids.dedup();
        Ok(ids)
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
                    format!(". Digests: {}", image.digests.join(", "))
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
                    summary: format!(
                        "{status}{digest}. {}, created {}",
                        image.size, image.created
                    ),
                    installed_version: Some(short_id(&image.id).into()),
                    // A registry digest is not queried during inventory: claiming
                    // an update from a mutable tag would be misleading. Pull stays
                    // available as an explicit refresh action in the Installed view.
                    candidate_version: None,
                    update: UpdateAvailability::Unknown,
                    icon: None,
                    component_ids: vec![],
                    homepages: vec![],
                    adopt_with: None,
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

impl<T: Transport> Container<T> {
    /// Remove the stopped containers, unused images or unused volumes the
    /// reviewed preview listed that are still stopped or unused. Nothing is
    /// forced: a container that started, or an image or volume that came
    /// into use, stays. `None` for any other task.
    fn clean_unused(
        &mut self,
        key: &str,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Option<OperationOutcome>, EngineError> {
        let (planned, current, args, what) = match key {
            "stopped-containers" => (
                self.cleanup_containers.take(),
                self.stopped_containers(cancel)
                    .map(|found| found.into_iter().map(|(id, _)| id).collect::<Vec<_>>()),
                ["container", "rm"],
                "stopped containers",
            ),
            "unused-images" => (
                self.cleanup_unused_images.take(),
                self.unused_images(cancel).map(|found| {
                    found
                        .into_iter()
                        .map(|(reference, _)| reference)
                        .collect::<Vec<_>>()
                }),
                ["image", "rm"],
                "unused images",
            ),
            "unused-volumes" => (
                self.cleanup_volumes.take(),
                self.unused_volumes(cancel),
                ["volume", "rm"],
                "unused volumes",
            ),
            _ => return Ok(None),
        };
        let current = current?;
        let planned: Vec<_> = planned
            .ok_or(EngineError::NotFound)?
            .into_iter()
            .filter(|planned| current.contains(planned))
            .collect();
        if planned.is_empty() {
            return Err(EngineError::Refused {
                backend: self.kind.id().into(),
                refusal: Refusal::CleanupChanged,
            });
        }
        progress(Progress::Message(format!(
            "Removing {what} from {}…",
            self.kind.label()
        )));
        let mut args = args.to_vec();
        args.extend(planned.iter().map(String::as_str));
        let result = self.call(&args, cancel, true)?;
        if result.code != Some(0) {
            return Err(ExecutionError::Failed(result).into());
        }
        Ok(Some(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        }))
    }
}

impl<T: Transport> Backend for Container<T> {
    fn id(&self) -> &str {
        self.kind.id()
    }
    /// A container that started, or an image or volume that came into use
    /// or was removed, while the list was reviewed leaves the rest still
    /// safe to remove.
    fn cleanup_still_reviewed(&self, reviewed: &CleanupItem, current: &CleanupItem) -> bool {
        if reviewed == current {
            return true;
        }
        // The first line says what happens to them, the rest list them.
        fn said(preview: &str) -> Option<&str> {
            preview.split_once('\n').map(|(said, _)| said)
        }
        fn listed(preview: &str) -> Vec<&str> {
            preview
                .split_once('\n')
                .map_or(vec![], |(_, listed)| listed.lines().collect())
        }
        matches!(
            current.id.key.as_str(),
            "dangling-images" | "stopped-containers" | "unused-images" | "unused-volumes"
        ) && said(&reviewed.preview).is_some()
            && said(&reviewed.preview) == said(&current.preview)
            && only_fewer(reviewed, current, listed)
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        // A Podman-provided `docker` wrapper shares Podman's image store.
        // Registering both would duplicate every image and fail Docker-only
        // probes (for example `buildx du --builder`); stay unregistered so
        // Podman remains the single source of truth. Explicit `--from docker`
        // still surfaces this status instead of failing each query.
        if self.kind == ContainerKind::Docker && self.transport.docker_is_podman() {
            return Ok(Availability::Unavailable(
                "The docker command on this system is Podman. Use the Podman source instead."
                    .into(),
            ));
        }
        // `version` answers from the client and, for Docker, the daemon; `info`
        // also computes storage statistics and takes seconds on large stores.
        match self.call(&["version", "--format", "{{json .}}"], cancel, false) {
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
                adopt_with: None,
            });
        }
        Ok(packages)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.inventory(cancel)
    }
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        let (backend, label) = (self.kind.id(), self.kind.label());
        let item =
            |key: &str, kind, title: String, summary: String, preview: String, bytes| CleanupItem {
                id: CleanupId {
                    backend: backend.into(),
                    key: key.into(),
                },
                kind,
                title,
                summary,
                preview,
                bytes,
            };
        self.cleanup_images = None;
        let images = self.dangling_images(cancel)?;
        let mut items = if images.is_empty() {
            vec![]
        } else {
            vec![item(
                "dangling-images",
                CleanupKind::PackageCache,
                format!("Dangling {label} images"),
                format!("{} untagged images in {}", images.len(), self.kind.store()),
                format!("Removes these untagged images. Images used by containers are kept. Shared layers may remain.\n{}", images.join("\n")),
                self.total(&["image", "inspect", "--format", "{{.Size}}"], &images, cancel),
            )]
        };
        self.cleanup_images = Some(images);
        // The newer tasks are extras: one that fails to list leaves the
        // others, and only cancelling stops them all.
        fn optional<V>(result: Result<V, EngineError>) -> Result<Option<V>, EngineError> {
            match result {
                Err(EngineError::Cancelled) => Err(EngineError::Cancelled),
                other => Ok(other.ok()),
            }
        }
        self.cleanup_containers = None;
        if let Some(containers) = optional(self.stopped_containers(cancel))? {
            let ids: Vec<_> = containers.iter().map(|(id, _)| id.clone()).collect();
            if !ids.is_empty() {
                items.push(item(
                    "stopped-containers",
                    CleanupKind::Unused,
                    format!("Stopped {label} containers"),
                    format!("{} containers that aren't running", ids.len()),
                    format!("Removes these containers and the files they changed. Running containers are kept.\n{}", containers.iter().map(|(_, shown)| shown.as_str()).collect::<Vec<_>>().join("\n")),
                    self.total(&["container", "inspect", "--size", "--format", "{{.SizeRw}}"], &ids, cancel),
                ));
            }
            self.cleanup_containers = Some(ids);
        }
        self.cleanup_unused_images = None;
        if let Some(unused) = optional(self.unused_images(cancel))? {
            let references: Vec<_> = unused
                .iter()
                .map(|(reference, _)| reference.clone())
                .collect();
            if !references.is_empty() {
                let mut ids: Vec<_> = unused.iter().map(|(_, id)| id.clone()).collect();
                ids.sort();
                ids.dedup();
                items.push(item(
                    "unused-images",
                    CleanupKind::Unused,
                    format!("Unused {label} images"),
                    format!("{} images no container uses", references.len()),
                    format!("Removes these images. They come back with a pull or a build. Shared layers may remain.\n{}", references.join("\n")),
                    self.total(&["image", "inspect", "--format", "{{.Size}}"], &ids, cancel),
                ));
            }
            self.cleanup_unused_images = Some(references);
        }
        self.cleanup_volumes = None;
        if let Some(volumes) = optional(self.unused_volumes(cancel))? {
            if !volumes.is_empty() {
                items.push(item(
                    "unused-volumes",
                    CleanupKind::Data,
                    format!("Unused {label} volumes"),
                    format!("{} volumes no container uses", volumes.len()),
                    format!("Deletes these volumes and everything in them, such as databases. This can't be undone.\n{}", volumes.join("\n")),
                    None,
                ));
            }
            self.cleanup_volumes = Some(volumes);
        }
        self.cleanup_build_cache = None;
        // Buildx is an optional Docker plugin. Older installations retain image
        // cleanup without claiming unsupported cache cleanup is available, and
        // a failing cache probe (for example a Podman-backed `docker` whose
        // `buildx version` succeeds but rejects `--builder`) must never
        // discard the dangling-image plan either.
        if self.kind == ContainerKind::Docker
            && self
                .call(&["buildx", "version"], cancel, false)
                .is_ok_and(|result| result.code == Some(0))
        {
            match self.build_cache(cancel) {
                Ok((ids, bytes)) => {
                    if !ids.is_empty() {
                        items.push(item(
                            "build-cache",
                            CleanupKind::PackageCache,
                            "Docker build cache".into(),
                            format!("{} unused records in the default builder", ids.len()),
                            format!("Removes unused build cache from the default builder. Later builds may take longer. Some shared storage may remain.\n{}", ids.join("\n")),
                            bytes,
                        ));
                    }
                    self.cleanup_build_cache = Some(ids);
                }
                Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
                Err(_) => {}
            }
        }
        Ok(items)
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
                    adopt_with: None,
                },
                description: format!(
                    "Pull {} into {}. The container engine downloads it and verifies its digest.",
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
        if let Operation::Clean(id) = operation {
            if id.backend == self.kind.id()
                && id.key == "build-cache"
                && self.kind == ContainerKind::Docker
            {
                let planned = self
                    .cleanup_build_cache
                    .clone()
                    .ok_or(EngineError::NotFound)?;
                if planned.is_empty() || self.build_cache(cancel)?.0 != planned {
                    return Err(EngineError::Refused {
                        backend: self.kind.id().into(),
                        refusal: Refusal::CleanupChanged,
                    });
                }
                self.cleanup_build_cache = None;
                let mut deferred = false;
                for record in planned {
                    let filter = format!("id={record}");
                    let result = self.call(
                        &[
                            "buildx",
                            "prune",
                            "--builder",
                            "default",
                            "--force",
                            "--filter",
                            &filter,
                            "--filter",
                            "immutable=true",
                            "--filter",
                            "inuse=false",
                        ],
                        cancel,
                        true,
                    )?;
                    if result.code != Some(0) {
                        return Err(ExecutionError::Failed(result).into());
                    }
                    deferred |= result.cancellation_deferred;
                }
                return Ok(OperationOutcome {
                    cancellation_deferred: deferred,
                });
            }
            if id.backend == self.kind.id() {
                if let Some(outcome) = self.clean_unused(&id.key, cancel, progress)? {
                    return Ok(outcome);
                }
            }
            if id.backend != self.kind.id() || id.key != "dangling-images" {
                return Err(EngineError::NotFound);
            }
            let planned = self.cleanup_images.clone().ok_or(EngineError::NotFound)?;
            let current = self.dangling_images(cancel)?;
            if current != planned || planned.is_empty() {
                return Err(EngineError::Refused {
                    backend: self.kind.id().into(),
                    refusal: Refusal::CleanupChanged,
                });
            }
            self.cleanup_images = None;
            progress(Progress::Message(format!(
                "Removing dangling {} images…",
                self.kind.label()
            )));
            let mut args = vec!["image", "rm"];
            args.extend(planned.iter().map(String::as_str));
            let result = self.call(&args, cancel, true)?;
            if result.code != Some(0) {
                return Err(ExecutionError::Failed(result).into());
            }
            return Ok(OperationOutcome {
                cancellation_deferred: result.cancellation_deferred,
            });
        }
        let (id, pull) = match operation {
            Operation::Remove(id) => (id, false),
            Operation::Install(id) | Operation::Upgrade(id) => (id, true),
            _ => return Err(self.unsupported(operation.capability())),
        };
        if matches!(operation, Operation::Install(_)) {
            self.validate_pull(id)?;
        } else {
            self.validate_local(id)?;
        }
        let (args, message) = if pull {
            let reference =
                id.reference
                    .as_deref()
                    .ok_or_else(|| EngineError::InvalidResponse {
                        backend: self.kind.id().into(),
                        reason: "a tagged image is required for pull".into(),
                    })?;
            (
                vec!["pull", reference],
                format!("Pulling {reference} with {}", self.kind.label()),
            )
        } else if id.reference.is_none() {
            (
                vec!["image", "rm", id.name.as_str()],
                format!(
                    "Removing dangling {} image {}",
                    self.kind.label(),
                    short_id(&id.name)
                ),
            )
        } else {
            (
                vec!["image", "rm", id.name.as_str()],
                format!("Removing {} image", self.kind.label()),
            )
        };
        progress(Progress::Message(message));
        let result = self.call(&args, cancel, true)?;
        if result.code != Some(0) {
            return Err(ExecutionError::Failed(result).into());
        }
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::Transport;
    use std::sync::{Arc, Mutex};

    type RecordedCall = (String, Vec<OsString>, bool);

    #[derive(Clone)]
    struct Fixture {
        output: Vec<u8>,
        build_cache: Option<String>,
        code: Option<i32>,
        failure: Option<ExecutionError>,
        calls: Arc<Mutex<Vec<RecordedCall>>>,
        shim: bool,
        fail_du: bool,
        cancel_du: bool,
        /// Replaces the `buildx du` answer while `build_cache` stays planned.
        du: Option<Completion>,
        write_failure: Option<ExecutionError>,
        write_code: Option<i32>,
        /// Answers to reads by how their arguments start. Container,
        /// volume and size reads answer nothing unless set here.
        answers: Vec<(String, String)>,
        /// Replaces the answer to reads starting this way, failures too.
        replies: Vec<(String, Result<Completion, ExecutionError>)>,
    }
    impl Fixture {
        fn new(output: &str) -> Self {
            Self {
                output: output.as_bytes().into(),
                build_cache: None,
                code: Some(0),
                failure: None,
                calls: Arc::new(Mutex::new(vec![])),
                shim: false,
                fail_du: false,
                cancel_du: false,
                du: None,
                write_failure: None,
                write_code: None,
                answers: vec![],
                replies: vec![],
            }
        }
        fn answer(mut self, args: &str, output: &str) -> Self {
            self.answers.push((args.into(), output.into()));
            self
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
        fn docker_is_podman(&self) -> bool {
            self.shim
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
            if args == ["buildx", "version"] && self.build_cache.is_none() {
                return Err(ExecutionError::Disabled("buildx unavailable".into()));
            }
            if self.cancel_du && args.len() > 2 && args[0] == "buildx" && args[1] == "du" {
                // Cancellation arriving mid-discovery aborts instead of
                // degrading to a partial plan.
                cancel.cancel();
                return Err(ExecutionError::Cancelled);
            }
            if self.fail_du && args.len() > 2 && args[0] == "buildx" && args[1] == "du" {
                // A Podman-backed `docker` answers `buildx version` but
                // rejects Docker-only probe flags.
                return Err(ExecutionError::Failed(completed(
                    b"Error: unknown flag: --builder",
                    Some(125),
                )));
            }
            if let Some(du) = self
                .du
                .as_ref()
                .filter(|_| args.len() > 2 && args[1] == "du")
            {
                return Ok(du.clone());
            }
            self.calls
                .lock()
                .unwrap()
                .push((executable.into(), args.into(), write));
            if write {
                if let Some(error) = &self.write_failure {
                    return Err(error.clone());
                }
            }
            let joined = args
                .iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            if !write {
                if let Some((_, reply)) = self
                    .replies
                    .iter()
                    .find(|(prefix, _)| joined.starts_with(prefix.as_str()))
                {
                    return reply.clone();
                }
                if let Some((_, output)) = self
                    .answers
                    .iter()
                    .find(|(prefix, _)| joined.starts_with(prefix.as_str()))
                {
                    return Ok(completed(output.as_bytes(), self.code));
                }
                if [
                    "container ",
                    "volume ",
                    "image inspect",
                    "image ls --no-trunc --format",
                ]
                .iter()
                .any(|prefix| joined.starts_with(prefix))
                {
                    return Ok(completed(&[], self.code));
                }
            }
            Ok(completed(
                if write || args.first().is_some_and(|arg| arg == "version") {
                    &[]
                } else if args.first().is_some_and(|arg| arg == "buildx") {
                    self.build_cache.as_deref().unwrap_or("").as_bytes()
                } else {
                    &self.output
                },
                self.write_code.filter(|_| write).or(self.code),
            ))
        }
    }
    fn ignore(_: Progress) {}

    #[test]
    fn docker_podman_shim_is_unavailable_to_queries() {
        let cancel = Cancellation::default();
        let mut shim = Fixture::new("");
        shim.shim = true;
        let mut backend = Container::docker(shim);
        assert!(matches!(
            backend.detect(&cancel),
            Ok(Availability::Unavailable(reason))
                if reason.contains("Podman")
        ));
        // The Podman backend on the same host is unaffected.
        let mut podman = Container::podman(Fixture::new(""));
        assert_eq!(podman.detect(&cancel).unwrap(), Availability::Available);
    }

    #[test]
    fn image_writes_reject_nonzero_manager_completion() {
        for kind in [ContainerKind::Docker, ContainerKind::Podman] {
            let mut backend = match kind {
                ContainerKind::Docker => Container::docker(Fixture::new("").with_code(23)),
                ContainerKind::Podman => Container::podman(Fixture::new("").with_code(23)),
            };
            let scope = kind.scope();
            let local = PackageId {
                backend: kind.id().into(),
                name: "sha256:0123456789abcdef".into(),
                architecture: std::env::consts::ARCH.into(),
                scope: scope.clone(),
                remote: Some(kind.store().into()),
                reference: Some("registry.example/app:tag".into()),
            };
            let offer = PackageId {
                backend: kind.id().into(),
                name: "registry.example/app:tag".into(),
                architecture: std::env::consts::ARCH.into(),
                scope,
                remote: Some("Container registry".into()),
                reference: Some("registry.example/app:tag".into()),
            };
            for operation in [
                Operation::Install(offer),
                Operation::Upgrade(local.clone()),
                Operation::Remove(local),
            ] {
                assert!(matches!(
                    backend.execute(&operation, &Cancellation::default(), &mut |_| {}),
                    Err(EngineError::Execution(ExecutionError::Failed(result)))
                        if result.code == Some(23)
                ));
            }
        }
    }

    #[test]
    fn docker_build_cache_probe_failure_keeps_dangling_images() {
        let mut fixture = Fixture::new("sha256:0123456789abcdef\n");
        fixture.build_cache = Some(String::new());
        fixture.fail_du = true;
        let mut backend = Container::docker(fixture);
        let cancel = Cancellation::default();
        let plans = backend.cleanup(&cancel).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].id.key, "dangling-images");
        // The cache plan was never offered, so it cannot execute.
        assert!(backend
            .execute(
                &Operation::Clean(CleanupId {
                    backend: "docker".into(),
                    key: "build-cache".into()
                }),
                &cancel,
                &mut ignore
            )
            .is_err());
        backend
            .execute(&Operation::Clean(plans[0].id.clone()), &cancel, &mut |_| {})
            .unwrap();
    }

    #[test]
    fn docker_build_cache_cancellation_aborts_discovery() {
        let mut fixture = Fixture::new("sha256:0123456789abcdef\n");
        fixture.build_cache = Some(String::new());
        fixture.cancel_du = true;
        let mut backend = Container::docker(fixture);
        assert_eq!(
            backend.cleanup(&Cancellation::default()),
            Err(EngineError::Cancelled)
        );
    }

    #[test]
    fn docker_build_cache_pins_builder_and_filters_each_reviewed_record() {
        let mut fixture = Fixture::new("");
        fixture.build_cache = Some(
            r#"{"ID":"cache1","Reclaimable":true,"Mutable":false,"Type":"regular"}
{"ID":"busy","Reclaimable":false,"Mutable":false}
{"ID":"mutable","Reclaimable":true,"Mutable":true}
{"ID":"frontend","Reclaimable":true,"Mutable":false,"Type":"frontend"}"#
                .into(),
        );
        let calls = fixture.calls.clone();
        let mut backend = Container::docker(fixture);
        let cancel = Cancellation::default();
        let plan = backend.cleanup(&cancel).unwrap().remove(0);
        assert_eq!(plan.id.key, "build-cache");
        assert!(plan.summary.starts_with("1 unused"));
        backend
            .execute(&Operation::Clean(plan.id.clone()), &cancel, &mut |_| {})
            .unwrap();
        let writes: Vec<_> = calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.2)
            .cloned()
            .collect();
        assert_eq!(writes.len(), 1);
        assert_eq!(
            writes[0].1,
            [
                "buildx",
                "prune",
                "--builder",
                "default",
                "--force",
                "--filter",
                "id=cache1",
                "--filter",
                "immutable=true",
                "--filter",
                "inuse=false"
            ]
        );
        backend.cleanup(&cancel).unwrap();
        backend.transport.build_cache = Some(String::new());
        assert!(backend
            .execute(&Operation::Clean(plan.id), &cancel, &mut |_| {})
            .is_err());
        assert_eq!(
            calls.lock().unwrap().iter().filter(|call| call.2).count(),
            1
        );
    }

    #[test]
    fn cleanup_revalidates_exact_images_and_never_forces_removal() {
        for kind in [ContainerKind::Docker, ContainerKind::Podman] {
            let fixture = Fixture::new("sha256:0123456789abcdef\nsha256:0123456789abcdef\n");
            let calls = fixture.calls.clone();
            let mut backend = Container::docker(fixture);
            backend.kind = kind;
            let cancel = Cancellation::default();
            let plans = backend.cleanup(&cancel).unwrap();
            assert_eq!(plans.len(), 1);
            assert!(plans[0].summary.starts_with("1 untagged"));
            backend
                .execute(&Operation::Clean(plans[0].id.clone()), &cancel, &mut ignore)
                .unwrap();
            let calls = calls.lock().unwrap();
            // Listed for the preview and again right before removing.
            let listed: Vec<_> = calls
                .iter()
                .filter(|call| {
                    call.1[0] == "image" && call.1.iter().any(|arg| arg == "dangling=true")
                })
                .collect();
            assert_eq!(listed.len(), 2);
            assert_eq!(listed[0].1, listed[1].1);
            let last = calls.last().unwrap();
            assert_eq!(last.1, ["image", "rm", "sha256:0123456789abcdef"]);
            assert_eq!(calls.iter().filter(|call| call.2).count(), 1);
        }
    }

    #[test]
    fn cleanup_rejects_stale_missing_foreign_and_invalid_plans() {
        let cancel = Cancellation::default();
        let mut backend = Container::docker(Fixture::new("0123456789abcdef\n"));
        let id = CleanupId {
            backend: "docker".into(),
            key: "dangling-images".into(),
        };
        assert!(backend
            .execute(&Operation::Clean(id.clone()), &cancel, &mut |_| {})
            .is_err());
        backend.cleanup(&cancel).unwrap();
        backend.transport.output = b"fedcba9876543210\n".to_vec();
        assert!(backend
            .execute(&Operation::Clean(id.clone()), &cancel, &mut |_| {})
            .is_err());
        assert!(backend
            .transport
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| !call.2));
        let foreign = CleanupId {
            backend: "podman".into(),
            ..id
        };
        assert!(backend
            .execute(&Operation::Clean(foreign), &cancel, &mut |_| {})
            .is_err());
        for fixture in [
            Fixture::new("--force"),
            Fixture::bytes(&[0xff]),
            Fixture::new("").with_code(1),
        ] {
            assert!(Container::docker(fixture).cleanup(&cancel).is_err());
        }
        assert!(Container::docker(Fixture::new(""))
            .cleanup(&cancel)
            .unwrap()
            .is_empty());
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
                &mut ignore
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

    const STOPPED: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const RUNNING: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const USED: &str = "3333333333333333333333333333333333333333333333333333333333333333";
    const SPARE: &str = "4444444444444444444444444444444444444444444444444444444444444444";

    /// A store with a stopped container, a running one, an image each uses,
    /// a spare image, an untagged one and a volume nobody mounts.
    fn busy_store() -> Fixture {
        Fixture::new("")
            .answer(
                "container ls --all --no-trunc --filter",
                &format!("{STOPPED}\tweb\tnginx:1\n"),
            )
            .answer("container ls --all --no-trunc --quiet", &format!("{STOPPED}\n{RUNNING}\n"))
            .answer("container inspect --size", "4096\n")
            // Docker names images with their prefix, Podman without.
            .answer("container inspect --format", &format!("sha256:{USED}\n{USED}\n"))
            .answer(
                "image ls --no-trunc --format",
                &format!("sha256:{USED}\tnginx:1\nsha256:{SPARE}\tredis:7\nsha256:{SPARE}\tredis:latest\nsha256:{STOPPED}\t<none>:<none>\n"),
            )
            .answer("image inspect --format", "1000000\n")
            .answer("volume ls", "pgdata\n")
    }

    #[test]
    fn stopped_containers_unused_images_and_volumes_are_offered_with_sizes() {
        let cancel = Cancellation::default();
        let mut backend = Container::podman(busy_store());
        let plans = backend.cleanup(&cancel).unwrap();
        let keys: Vec<_> = plans.iter().map(|plan| plan.id.key.as_str()).collect();
        assert_eq!(
            keys,
            ["stopped-containers", "unused-images", "unused-volumes"]
        );
        let (containers, images, volumes) = (&plans[0], &plans[1], &plans[2]);
        assert_eq!(containers.kind, CleanupKind::Unused);
        assert!(
            containers.preview.ends_with("\nweb (nginx:1)"),
            "{}",
            containers.preview
        );
        assert_eq!(containers.bytes, Some(4096));
        // The image a container uses stays, whichever way its ID is written.
        assert!(
            images.preview.ends_with("\nredis:7\nredis:latest"),
            "{}",
            images.preview
        );
        // Two tags of one image count its size once.
        assert_eq!(images.bytes, Some(1_000_000));
        assert_eq!(images.summary, "2 images no container uses");
        // Volumes may hold the only copy of something: never in Clean all,
        // and with no size to tell.
        assert_eq!(volumes.kind, CleanupKind::Data);
        assert!(!volumes.kind.in_clean_all());
        assert!(CleanupKind::Unused.in_clean_all());
        assert_eq!(volumes.bytes, None);
        let calls = backend.transport.calls.clone();
        for (plan, removal) in [
            (containers, vec!["container", "rm", STOPPED]),
            (images, vec!["image", "rm", "redis:7", "redis:latest"]),
            (volumes, vec!["volume", "rm", "pgdata"]),
        ] {
            backend
                .execute(&Operation::Clean(plan.id.clone()), &cancel, &mut ignore)
                .unwrap();
            let last = calls.lock().unwrap().last().unwrap().clone();
            assert_eq!(last.1, removal);
            assert!(last.2, "removals write");
            // Each reviewed list is used once.
            assert!(matches!(
                backend.execute(&Operation::Clean(plan.id.clone()), &cancel, &mut ignore),
                Err(EngineError::NotFound)
            ));
        }
    }

    #[test]
    fn an_older_pull_that_lost_its_tag_goes_by_its_digest() {
        let cancel = Cancellation::default();
        let digest = format!("sha256:{SPARE}");
        let mut backend = Container::podman(
            Fixture::new("")
                .answer("container ls --all --no-trunc --quiet", "")
                .answer(
                    "image ls --no-trunc --format",
                    &format!("sha256:{USED}\trust:<none>\t{digest}\nsha256:{SPARE}\tlocal:<none>\t<none>\nsha256:{STOPPED}\t<none>:<none>\t{digest}\n"),
                )
                .answer("image inspect --format", "1000000\n"),
        );
        let plans = backend.cleanup(&cancel).unwrap();
        let images = plans
            .iter()
            .find(|plan| plan.id.key == "unused-images")
            .unwrap();
        // One without a digest has nothing to go by, and one without a
        // name is a dangling image.
        assert!(
            images.preview.ends_with(&format!("\nrust@{digest}")),
            "{}",
            images.preview
        );
        assert_eq!(images.summary, "1 images no container uses");
        backend
            .execute(&Operation::Clean(images.id.clone()), &cancel, &mut ignore)
            .unwrap();
        let calls = backend.transport.calls.lock().unwrap().clone();
        let listing = calls
            .iter()
            .find(|call| {
                call.1
                    .starts_with(&["image".into(), "ls".into(), "--no-trunc".into()])
            })
            .unwrap();
        assert_eq!(listing.1.last(), Some(&OsString::from("--digests")));
        assert_eq!(
            calls.last().unwrap().1,
            ["image", "rm", &format!("rust@{digest}")].map(OsString::from)
        );
    }

    #[test]
    fn unused_things_that_changed_since_the_review_are_kept() {
        let cancel = Cancellation::default();
        for (key, prefix, now) in [
            (
                "stopped-containers",
                "container ls --all --no-trunc --filter",
                "",
            ),
            ("unused-images", "image ls --no-trunc --format", ""),
            ("unused-volumes", "volume ls", ""),
        ] {
            let mut backend = Container::docker(busy_store());
            backend.cleanup(&cancel).unwrap();
            backend
                .transport
                .answers
                .insert(0, (prefix.into(), now.into()));
            let error = backend
                .execute(
                    &Operation::Clean(CleanupId {
                        backend: "docker".into(),
                        key: key.into(),
                    }),
                    &cancel,
                    &mut ignore,
                )
                .unwrap_err();
            assert!(error.to_string().contains("changed"), "{key}: {error}");
            assert!(backend
                .transport
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| !call.2));
        }
        // A removal the engine refuses, such as a container that started.
        let mut refused = busy_store();
        refused.write_code = Some(1);
        let mut backend = Container::docker(refused);
        backend.cleanup(&cancel).unwrap();
        assert!(matches!(
            backend.execute(
                &Operation::Clean(CleanupId {
                    backend: "docker".into(),
                    key: "stopped-containers".into(),
                }),
                &cancel,
                &mut ignore,
            ),
            Err(EngineError::Execution(ExecutionError::Failed(_)))
        ));
    }

    #[test]
    fn a_list_that_got_shorter_since_the_review_removes_what_is_left() {
        let cancel = Cancellation::default();
        let mut backend = Container::podman(busy_store());
        let reviewed = backend.cleanup(&cancel).unwrap().remove(1);
        // redis:latest came into use.
        backend.transport.answers.insert(
            0,
            (
                "image ls --no-trunc --format".into(),
                format!("sha256:{USED}\tnginx:1\nsha256:{SPARE}\tredis:7\n"),
            ),
        );
        let current = backend.cleanup(&cancel).unwrap().remove(1);
        assert_ne!(reviewed, current);
        assert!(backend.cleanup_still_reviewed(&reviewed, &current));
        assert!(backend.cleanup_still_reviewed(&current, &current));
        // A longer list, or a different one, needs a new review.
        assert!(!backend.cleanup_still_reviewed(&current, &reviewed));
        let reworded = CleanupItem {
            preview: current.preview.replacen("Removes", "Deletes", 1),
            ..current.clone()
        };
        assert!(!backend.cleanup_still_reviewed(&reviewed, &reworded));
        let other = CleanupItem {
            id: CleanupId {
                backend: "podman".into(),
                key: "build-cache".into(),
            },
            ..current.clone()
        };
        let other_reviewed = CleanupItem {
            id: other.id.clone(),
            ..reviewed.clone()
        };
        assert!(!backend.cleanup_still_reviewed(&other_reviewed, &other));
        // Dangling images removed by something else leave the rest.
        let dangling = |preview: &str| CleanupItem {
            id: CleanupId {
                backend: "podman".into(),
                key: "dangling-images".into(),
            },
            preview: preview.into(),
            ..current.clone()
        };
        assert!(backend.cleanup_still_reviewed(
            &dangling("Removes these untagged images.\naaa\nbbb"),
            &dangling("Removes these untagged images.\nbbb")
        ));
        // A preview with no list can't be told apart.
        let unlisted = CleanupItem {
            preview: "Removes these images.".into(),
            ..current.clone()
        };
        assert!(!backend.cleanup_still_reviewed(&reviewed, &unlisted));
        backend
            .execute(&Operation::Clean(current.id.clone()), &cancel, &mut ignore)
            .unwrap();
        let last = backend
            .transport
            .calls
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(last.1, ["image", "rm", "redis:7"]);
        // One that showed up after the last check isn't removed.
        let mut backend = Container::podman(busy_store());
        backend.cleanup(&cancel).unwrap();
        backend
            .transport
            .answers
            .insert(0, ("volume ls".into(), "pgdata\nother\n".into()));
        backend
            .execute(
                &Operation::Clean(CleanupId {
                    backend: "podman".into(),
                    key: "unused-volumes".into(),
                }),
                &cancel,
                &mut ignore,
            )
            .unwrap();
        let last = backend
            .transport
            .calls
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(last.1, ["volume", "rm", "pgdata"]);
    }

    #[test]
    fn a_list_that_fails_or_looks_wrong_leaves_the_other_tasks() {
        let cancel = Cancellation::default();
        for (prefix, output) in [
            ("volume ls", "-rf\n"),
            ("volume ls", "data base\n"),
            (
                "container ls --all --no-trunc --filter",
                "not-an-id\tweb\tnginx\n",
            ),
            ("container ls --all --no-trunc --quiet", "not-an-id\n"),
            ("image ls --no-trunc --format", "not-an-id\tredis:7\n"),
            (
                "image ls --no-trunc --format",
                &[0xffu8 as char].iter().collect::<String>(),
            ),
        ] {
            let mut backend = Container::docker(busy_store());
            backend
                .transport
                .answers
                .insert(0, (prefix.into(), output.into()));
            let plans = backend.cleanup(&cancel).unwrap();
            assert_eq!(plans.len(), 2, "{prefix}: {output}");
        }
        // Without sizes, a task still shows, with no size.
        let mut backend = Container::docker(busy_store());
        backend
            .transport
            .answers
            .insert(0, ("image inspect --format".into(), "lots\n".into()));
        backend
            .transport
            .answers
            .insert(0, ("container inspect --size".into(), "".into()));
        let plans = backend.cleanup(&cancel).unwrap();
        assert_eq!(plans[0].bytes, Some(0));
        assert_eq!(plans[1].bytes, None);
        // A store that answers nothing fails the whole check.
        let mut backend = Container::docker(busy_store());
        backend.transport.code = Some(1);
        assert!(backend.cleanup(&cancel).is_err());
        // No containers: every tagged image is unused.
        let mut backend = Container::docker(busy_store());
        backend.transport.answers.insert(
            0,
            ("container ls --all --no-trunc --quiet".into(), "".into()),
        );
        let plans = backend.cleanup(&cancel).unwrap();
        assert!(plans[1]
            .preview
            .ends_with("\nnginx:1\nredis:7\nredis:latest"));
        // A list that can't be read, or isn't text, leaves the other tasks.
        for (prefix, reply, left) in [
            (
                "container ls --all --no-trunc --filter",
                completed(b"", Some(1)),
                ["unused-images", "unused-volumes"],
            ),
            (
                "container ls --all --no-trunc --quiet",
                completed(b"", Some(1)),
                ["stopped-containers", "unused-volumes"],
            ),
            (
                "container inspect --format",
                completed(b"", Some(1)),
                ["stopped-containers", "unused-volumes"],
            ),
            (
                "image ls --no-trunc --format",
                Completion {
                    truncated: true,
                    ..completed(b"", Some(0))
                },
                ["stopped-containers", "unused-volumes"],
            ),
            (
                "volume ls",
                completed(&[0xff, 0xfe], Some(0)),
                ["stopped-containers", "unused-images"],
            ),
        ] {
            let mut backend = Container::docker(busy_store());
            backend.transport.replies.push((prefix.into(), Ok(reply)));
            let plans = backend.cleanup(&cancel).unwrap();
            let keys: Vec<_> = plans.iter().map(|plan| plan.id.key.as_str()).collect();
            assert_eq!(keys, left, "{prefix}");
        }
        // A stopped container shows its name alone without an image, and
        // its short ID without a name.
        let mut backend = Container::docker(busy_store());
        backend.transport.answers.insert(
            0,
            (
                "container ls --all --no-trunc --filter".into(),
                format!("{STOPPED}\tweb\t\n{RUNNING}\t\t\n"),
            ),
        );
        let plans = backend.cleanup(&cancel).unwrap();
        let preview = &plans[0].preview;
        assert!(preview.ends_with("\nweb\n222222222222"), "{preview}");
        // Cancelling during an extra task stops them all.
        let mut backend = Container::docker(busy_store());
        backend
            .transport
            .replies
            .push(("volume ls".into(), Err(ExecutionError::Cancelled)));
        assert!(matches!(
            backend.cleanup(&cancel),
            Err(EngineError::Cancelled)
        ));
        // Cancelling stops every task.
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(matches!(
            Container::docker(busy_store()).cleanup(&cancelled),
            Err(EngineError::Cancelled)
        ));
    }

    #[test]
    fn build_cache_sizes_add_up_however_docker_writes_them() {
        let mut fixture = Fixture::new("");
        fixture.build_cache = Some(
            r#"{"ID":"one","Reclaimable":true,"Mutable":false,"Size":2048}
{"ID":"two","Reclaimable":true,"Mutable":false,"Size":"4.096kB"}"#
                .into(),
        );
        let plans = Container::docker(fixture.clone())
            .cleanup(&Cancellation::default())
            .unwrap();
        assert_eq!(plans[0].bytes, Some(6144));
        fixture.build_cache = Some(
            r#"{"ID":"one","Reclaimable":true,"Mutable":false,"Size":2048}
{"ID":"two","Reclaimable":true,"Mutable":false}"#
                .into(),
        );
        let plans = Container::docker(fixture)
            .cleanup(&Cancellation::default())
            .unwrap();
        assert_eq!(plans[0].bytes, None);
    }

    fn cache_plan(fixture: Fixture) -> (Container<Fixture>, CleanupId) {
        let mut fixture = fixture;
        fixture.build_cache = Some(r#"{"ID":"cache1","Reclaimable":true,"Mutable":false}"#.into());
        let mut backend = Container::docker(fixture);
        let plan = backend.cleanup(&Cancellation::default()).unwrap().remove(0);
        assert_eq!(plan.id.key, "build-cache");
        (backend, plan.id)
    }

    #[test]
    fn build_cache_revalidation_rejects_failed_and_malformed_answers() {
        let cancel = Cancellation::default();
        let cases: [(Completion, &str); 4] = [
            (completed(b"", Some(1)), ""),
            (completed(&[0xff], Some(0)), "invalid utf-8"),
            (completed(b"not json", Some(0)), "expected ident"),
            (
                completed(
                    br#"{"ID":"bad id!","Reclaimable":true,"Mutable":false}"#,
                    Some(0),
                ),
                "invalid build cache ID",
            ),
        ];
        for (answer, reason) in cases {
            let (mut backend, id) = cache_plan(Fixture::new(""));
            backend.transport.du = Some(answer);
            let error = backend
                .execute(&Operation::Clean(id), &cancel, &mut ignore)
                .unwrap_err();
            if reason.is_empty() {
                assert!(matches!(
                    error,
                    EngineError::Execution(ExecutionError::Failed(result)) if result.code == Some(1)
                ));
            } else {
                assert!(
                    matches!(
                        &error,
                        EngineError::InvalidResponse { backend, reason: got }
                            if backend == "docker" && got.contains(reason)
                    ),
                    "{error:?}"
                );
            }
            assert!(backend
                .transport
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| !call.2));
        }
        // A truncated answer is never trusted either.
        let (mut backend, id) = cache_plan(Fixture::new(""));
        let mut truncated = completed(
            br#"{"ID":"cache1","Reclaimable":true,"Mutable":false}"#,
            Some(0),
        );
        truncated.truncated = true;
        backend.transport.du = Some(truncated);
        assert!(matches!(
            backend.execute(&Operation::Clean(id), &cancel, &mut ignore),
            Err(EngineError::Execution(ExecutionError::Failed(_)))
        ));
    }

    #[test]
    fn build_cache_prune_failures_are_reported() {
        let cancel = Cancellation::default();
        let (mut backend, id) = cache_plan(Fixture::new(""));
        backend.transport.write_failure = Some(ExecutionError::Io("prune failed".into()));
        assert_eq!(
            backend.execute(&Operation::Clean(id), &cancel, &mut ignore),
            Err(EngineError::Execution(ExecutionError::Io(
                "prune failed".into()
            )))
        );
        let (mut backend, id) = cache_plan(Fixture::new(""));
        backend.transport.write_code = Some(1);
        assert!(matches!(
            backend.execute(&Operation::Clean(id), &cancel, &mut ignore),
            Err(EngineError::Execution(ExecutionError::Failed(result))) if result.code == Some(1)
        ));
        // The approved plan was consumed, so it cannot be retried blindly.
        let id = CleanupId {
            backend: "docker".into(),
            key: "build-cache".into(),
        };
        assert_eq!(
            backend.execute(&Operation::Clean(id), &cancel, &mut ignore),
            Err(EngineError::NotFound)
        );
    }

    #[test]
    fn dangling_image_removal_failures_are_reported() {
        let cancel = Cancellation::default();
        let mut fixture = Fixture::new("sha256:0123456789abcdef\n");
        fixture.write_code = Some(2);
        let mut backend = Container::podman(fixture);
        let plan = backend.cleanup(&cancel).unwrap().remove(0);
        assert_eq!(plan.title, "Dangling Podman images");
        let mut messages = vec![];
        let result = backend.execute(&Operation::Clean(plan.id), &cancel, &mut |message| {
            messages.push(message)
        });
        assert!(matches!(
            result,
            Err(EngineError::Execution(ExecutionError::Failed(result))) if result.code == Some(2)
        ));
        assert_eq!(
            messages,
            [Progress::Message("Removing dangling Podman images…".into())]
        );
        // A failing engine never yields an empty cleanup plan.
        assert_eq!(
            Container::docker(Fixture::failed(ExecutionError::Io("down".into()))).cleanup(&cancel),
            Err(EngineError::Execution(ExecutionError::Io("down".into())))
        );
    }

    #[test]
    fn detection_falls_back_to_stdout_for_the_diagnostic() {
        let cancel = Cancellation::default();
        let quiet = completed(b"Error: podman socket missing\nhint", Some(125));
        let mut backend = Container::podman(Fixture::failed(ExecutionError::Failed(quiet)));
        assert_eq!(
            backend.detect(&cancel).unwrap(),
            Availability::Unavailable(
                "Podman is installed but unavailable: Error: podman socket missing".into()
            )
        );
        let silent = completed(b"", Some(1));
        let mut backend = Container::docker(Fixture::failed(ExecutionError::Failed(silent)));
        assert_eq!(
            backend.detect(&cancel).unwrap(),
            Availability::Unavailable(
                "Docker is installed but unavailable: engine is not ready".into()
            )
        );
    }

    #[test]
    fn pulls_require_a_matching_safe_reference_and_report_progress() {
        let cancel = Cancellation::default();
        let fixture = Fixture::new("");
        let calls = fixture.calls.clone();
        let mut backend = Container::docker(fixture);
        let offer = PackageId {
            backend: "docker".into(),
            name: "registry.example/app:tag".into(),
            architecture: std::env::consts::ARCH.into(),
            scope: Scope::System,
            remote: Some("Container registry".into()),
            reference: Some("registry.example/app:tag".into()),
        };
        for bad in [
            PackageId {
                backend: "podman".into(),
                ..offer.clone()
            },
            PackageId {
                reference: Some("registry.example/other:tag".into()),
                ..offer.clone()
            },
            PackageId {
                name: "-registry".into(),
                reference: Some("-registry".into()),
                ..offer.clone()
            },
        ] {
            assert_eq!(
                backend.execute(&Operation::Install(bad.clone()), &cancel, &mut ignore),
                Err(EngineError::InvalidResponse {
                    backend: "docker".into(),
                    reason: "foreign or invalid container pull reference".into(),
                })
            );
            if bad.reference.as_deref() == Some(bad.name.as_str()) {
                assert!(backend.details(&bad, &cancel).is_err());
            }
        }
        assert!(calls.lock().unwrap().is_empty());
        let mut messages = vec![];
        backend
            .execute(&Operation::Install(offer), &cancel, &mut |message| {
                messages.push(message)
            })
            .unwrap();
        assert_eq!(
            messages,
            [Progress::Message(
                "Pulling registry.example/app:tag with Docker".into()
            )]
        );
    }

    #[test]
    fn removing_an_untagged_image_names_its_short_id() {
        let cancel = Cancellation::default();
        let mut backend = Container::docker(Fixture::new(""));
        let id = PackageId {
            backend: "docker".into(),
            name: "sha256:fedcba9876543210ffff".into(),
            architecture: std::env::consts::ARCH.into(),
            scope: Scope::System,
            remote: Some("Docker daemon".into()),
            reference: None,
        };
        let mut messages = vec![];
        backend
            .execute(&Operation::Remove(id), &cancel, &mut |message| {
                messages.push(message)
            })
            .unwrap();
        assert_eq!(
            messages,
            [Progress::Message(
                "Removing dangling Docker image fedcba987654".into()
            )]
        );
    }
}
