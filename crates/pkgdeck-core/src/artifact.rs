//! Explicitly opened package archives. Keep the reviewed digest across the
//! confirmation boundary and give native managers a private, immutable copy.
use crate::{
    engine::EngineError,
    host::Host,
    package::{Package, PackageId, Scope, UpdateAvailability},
    process::{self, Cancellation, Limits},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ASSERTION_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Deb,
    Rpm,
    Arch,
    Flatpak,
    Snap,
    AppImage,
}
impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::Arch => "arch",
            Self::Flatpak => "flatpak",
            Self::Snap => "snap",
            Self::AppImage => "appimage",
        }
    }
    fn backend(self) -> &'static str {
        match self {
            Self::Deb => "apt",
            Self::Rpm => "dnf",
            Self::Arch => "pacman",
            Self::Flatpak => "flatpak",
            Self::Snap => "snap",
            Self::AppImage => "appimage",
        }
    }
    fn suffix(self) -> &'static str {
        match self {
            Self::Deb => ".deb",
            Self::Rpm => ".rpm",
            Self::Arch => ".pkg.tar.zst",
            Self::Flatpak => ".flatpak",
            Self::Snap => ".snap",
            Self::AppImage => ".AppImage",
        }
    }
}
fn selected_backend(kind: Kind, host: &Host) -> Result<&'static str, EngineError> {
    if kind != Kind::Rpm {
        return Ok(kind.backend());
    }
    if host.resolve("dnf")?.is_some() {
        Ok("dnf")
    } else if host.resolve("zypper")?.is_some() {
        Ok("zypper")
    } else {
        Err(invalid("DNF or Zypper is required for RPM files"))
    }
}
pub fn kind(source: &str) -> Option<Kind> {
    let name = source.split(['?', '#']).next().unwrap_or(source);
    if name.ends_with(".deb") {
        Some(Kind::Deb)
    } else if name.ends_with(".rpm") {
        Some(Kind::Rpm)
    } else if [
        ".pkg.tar.zst",
        ".pkg.tar.xz",
        ".pkg.tar.gz",
        ".pkg.tar.bz2",
        ".pkg.tar.lz4",
    ]
    .iter()
    .any(|suffix| name.ends_with(suffix))
    {
        Some(Kind::Arch)
    } else if name.ends_with(".flatpak") {
        Some(Kind::Flatpak)
    } else if name.ends_with(".snap") {
        Some(Kind::Snap)
    } else if name.ends_with(".AppImage") {
        Some(Kind::AppImage)
    } else {
        None
    }
}
pub fn https_source(source: &str) -> bool {
    let Some(rest) = source.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty()
        && !host.contains('@')
        && !source.bytes().any(|byte| byte <= 0x20 || byte == 0x7f)
}
fn invalid(reason: impl ToString) -> EngineError {
    EngineError::InvalidResponse {
        backend: "open".into(),
        reason: reason.to_string(),
    }
}
fn temporary(suffix: &str) -> Result<PathBuf, EngineError> {
    let name = format!(
        "pkgdeck-input-{}-{}{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        suffix
    );
    let path = std::env::temp_dir().join(name);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(invalid)?;
    Ok(path)
}
pub struct Staged {
    path: PathBuf,
    assertion: Option<PathBuf>,
}
impl Staged {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn assertion(&self) -> Option<&Path> {
        self.assertion.as_deref()
    }
}
impl Drop for Staged {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        if let Some(path) = &self.assertion {
            let _ = fs::remove_file(path);
        }
    }
}
fn hash(path: &Path, cancel: &Cancellation) -> Result<String, EngineError> {
    let meta = fs::symlink_metadata(path).map_err(invalid)?;
    if !meta.file_type().is_file() || meta.len() < 8 || meta.len() > MAX_BYTES {
        return Err(invalid(
            "expected a regular package file no larger than 2 GiB",
        ));
    }
    let mut file = fs::File::open(path).map_err(invalid)?;
    let mut hash = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let count = file.read(&mut buffer).map_err(invalid)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        if size > MAX_BYTES {
            return Err(invalid("package file exceeds 2 GiB"));
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn download(
    source: &str,
    destination: &Path,
    max_bytes: u64,
    cancel: &Cancellation,
    host: &Host,
) -> Result<(), EngineError> {
    if !https_source(source) {
        return Err(invalid("expected an HTTPS package link"));
    }
    let curl = host
        .resolve("curl")?
        .ok_or_else(|| invalid("curl is unavailable"))?;
    let mut command = Command::new(curl);
    command
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-redirs",
            "5",
            "--max-filesize",
        ])
        .arg(max_bytes.to_string())
        .args(["--connect-timeout", "5", "--max-time", "300", "--output"])
        .arg(destination)
        .arg(source);
    let result = process::run(
        command,
        Limits {
            timeout: Duration::from_secs(305),
            output_bytes: 16 * 1024,
        },
        cancel,
        false,
    )?;
    if result.code != Some(0) || result.truncated {
        return Err(invalid("package download failed"));
    }
    Ok(())
}
fn copy(source: &Path, destination: &Path, cancel: &Cancellation) -> Result<(), EngineError> {
    let mut input = fs::File::open(source).map_err(invalid)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(destination)
        .map_err(invalid)?;
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let count = input.read(&mut buffer).map_err(invalid)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_BYTES {
            return Err(invalid("package file exceeds 2 GiB"));
        }
        output.write_all(&buffer[..count]).map_err(invalid)?;
    }
    output.sync_all().map_err(invalid)
}
fn materialize(
    source: &str,
    kind: Kind,
    cancel: &Cancellation,
    host: &Host,
) -> Result<Staged, EngineError> {
    let suffix = if kind == Kind::Arch {
        [
            ".pkg.tar.zst",
            ".pkg.tar.xz",
            ".pkg.tar.gz",
            ".pkg.tar.bz2",
            ".pkg.tar.lz4",
        ]
        .into_iter()
        .find(|suffix| {
            source
                .split(['?', '#'])
                .next()
                .unwrap_or(source)
                .ends_with(suffix)
        })
        .unwrap_or(kind.suffix())
    } else {
        kind.suffix()
    };
    let path = temporary(suffix)?;
    let mut staged = Staged {
        path,
        assertion: None,
    };
    if https_source(source) {
        download(source, &staged.path, MAX_BYTES, cancel, host)?;
    } else {
        let path = Path::new(source);
        if !path.is_absolute() {
            return Err(invalid("choose an absolute package path"));
        }
        let meta = fs::symlink_metadata(path).map_err(invalid)?;
        if !meta.file_type().is_file() {
            return Err(invalid("expected a regular package file"));
        }
        if meta.len() > MAX_BYTES {
            return Err(invalid("package file exceeds 2 GiB"));
        }
        copy(path, &staged.path, cancel)?;
    }
    if kind == Kind::Snap {
        let (base, suffix) = source
            .split_once(['?', '#'])
            .map_or((source, ""), |(base, _)| (base, &source[base.len()..]));
        let assertion_source = base
            .strip_suffix(".snap")
            .ok_or_else(|| invalid("invalid Snap filename"))?
            .to_owned()
            + ".assert"
            + suffix;
        let assertion = temporary(".assert")?;
        staged.assertion = Some(assertion.clone());
        if https_source(source) {
            download(
                &assertion_source,
                &assertion,
                MAX_ASSERTION_BYTES,
                cancel,
                host,
            )?;
        } else {
            let meta = fs::symlink_metadata(&assertion_source).map_err(invalid)?;
            if !meta.file_type().is_file() || meta.len() == 0 || meta.len() > MAX_ASSERTION_BYTES {
                return Err(invalid(
                    "Snap assertion must be a regular file smaller than 1 MiB",
                ));
            }
            copy(Path::new(&assertion_source), &assertion, cancel)?;
        }
        let meta = fs::metadata(&assertion).map_err(invalid)?;
        if meta.len() == 0 || meta.len() > MAX_ASSERTION_BYTES {
            return Err(invalid("Snap assertion must be smaller than 1 MiB"));
        }
    }
    Ok(staged)
}
fn metadata(
    executable: &str,
    args: Vec<OsString>,
    cancel: &Cancellation,
    host: &Host,
) -> Result<String, EngineError> {
    let executable = host
        .resolve(executable)?
        .ok_or_else(|| invalid("package metadata tool is unavailable"))?;
    let result = host.read(
        &executable,
        &args,
        Limits {
            timeout: Duration::from_secs(20),
            output_bytes: 64 * 1024,
        },
        cancel,
    )?;
    if result.code != Some(0) || result.truncated {
        return Err(invalid("could not inspect package metadata"));
    }
    String::from_utf8(result.stdout).map_err(invalid)
}
fn fields(text: &str, separator: char) -> Result<(&str, &str, &str, &str), EngineError> {
    let mut parts = text.trim().splitn(4, separator);
    let values = (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    );
    if values.0.is_empty() || values.1.is_empty() || values.2.is_empty() {
        return Err(invalid("package identity is incomplete"));
    }
    Ok(values)
}
pub fn inspect(source: &str, cancel: &Cancellation) -> Result<Package, EngineError> {
    inspect_with_host(source, cancel, &Host::current())
}
fn inspect_with_host(
    source: &str,
    cancel: &Cancellation,
    host: &Host,
) -> Result<Package, EngineError> {
    let kind = kind(source).ok_or_else(|| invalid("unsupported package format"))?;
    if source.starts_with("https://") && !https_source(source) {
        return Err(invalid("invalid HTTPS package link"));
    }
    let staged = materialize(source, kind, cancel, host)?;
    inspect_staged(&staged, source, kind, cancel, host)
}
fn inspect_staged(
    staged: &Staged,
    source: &str,
    kind: Kind,
    cancel: &Cancellation,
    host: &Host,
) -> Result<Package, EngineError> {
    let digest = hash(&staged.path, cancel)?;
    let (name, version, arch, summary) = match kind {
        Kind::Deb => {
            let package = crate::local_deb::inspect(&staged.path, cancel)?;
            (
                package.id.name,
                package.candidate_version.unwrap_or_default(),
                package.id.architecture,
                package
                    .summary
                    .lines()
                    .next()
                    .unwrap_or("Debian archive")
                    .to_owned(),
            )
        }
        Kind::Rpm => {
            let text = metadata(
                "rpm",
                vec![
                    "-qp".into(),
                    "--queryformat".into(),
                    "%{NAME}|%{VERSION}-%{RELEASE}|%{ARCH}|%{SUMMARY}".into(),
                    staged.path.as_os_str().to_os_string(),
                ],
                cancel,
                host,
            )?;
            let (n, v, a, s) = fields(&text, '|')?;
            (n.into(), v.into(), a.into(), s.into())
        }
        Kind::Arch => {
            let text = metadata(
                "pacman",
                vec![
                    "-Qip".into(),
                    "--".into(),
                    staged.path.as_os_str().to_os_string(),
                ],
                cancel,
                host,
            )?;
            let field = |name: &str| {
                text.lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .and_then(|(key, value)| (key.trim() == name).then_some(value.trim()))
                    })
                    .unwrap_or("")
            };
            let (name, version, arch, summary) = (
                field("Name"),
                field("Version"),
                field("Architecture"),
                field("Description"),
            );
            if name.is_empty() || version.is_empty() || arch.is_empty() {
                return Err(invalid("Arch package identity is incomplete"));
            }
            (name.into(), version.into(), arch.into(), summary.into())
        }
        Kind::Snap => {
            let text = metadata(
                "snap",
                vec!["info".into(), staged.path.as_os_str().to_os_string()],
                cancel,
                host,
            )?;
            let field = |name: &str| {
                text.lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .and_then(|(key, value)| (key.trim() == name).then_some(value.trim()))
                    })
                    .unwrap_or("")
            };
            let name = field("name");
            let version = field("version");
            if name.is_empty() {
                return Err(invalid("Snap package identity is incomplete"));
            }
            (
                name.into(),
                version.into(),
                std::env::consts::ARCH.into(),
                format!(
                    "{}\nMatching assertion provided; Snap verifies it during install",
                    field("summary")
                ),
            )
        }
        Kind::Flatpak | Kind::AppImage => {
            let name = source
                .split(['?', '#'])
                .next()
                .unwrap_or(source)
                .rsplit('/')
                .next()
                .unwrap_or("Package")
                .to_owned();
            if kind == Kind::AppImage {
                use crate::engine::Backend;
                let package = crate::backends::AppImage::native()
                    .search(&staged.path.to_string_lossy(), cancel)?
                    .pop()
                    .ok_or(EngineError::NotFound)?;
                (
                    source.to_owned(),
                    String::new(),
                    package.id.architecture,
                    format!("AppImage: {name}"),
                )
            } else {
                (
                    name,
                    String::new(),
                    std::env::consts::ARCH.into(),
                    "Flatpak bundle; Flatpak checks its contents during install".into(),
                )
            }
        }
    };
    let assertion_hash = staged
        .assertion()
        .map(|path| hash(path, cancel))
        .transpose()?
        .unwrap_or_default();
    let display_name = source
        .split(['?', '#'])
        .next()
        .unwrap_or(source)
        .rsplit('/')
        .next()
        .unwrap_or(&name)
        .to_owned();
    let scope = if kind == Kind::AppImage {
        crate::backends::AppImage::native().import_scope()
    } else if kind == Kind::Flatpak {
        Scope::User {
            uid: rustix::process::getuid().as_raw(),
        }
    } else {
        Scope::System
    };
    Ok(Package {
        id: PackageId {
            backend: selected_backend(kind, host)?.into(),
            name: name.clone(),
            architecture: arch,
            scope,
            remote: None,
            reference: Some(format!(
                "artifact:{}:{digest}:{assertion_hash}:{source}",
                kind.tag()
            )),
        },
        display_name,
        summary: format!("{summary}\nSource: {source}"),
        installed_version: None,
        candidate_version: (!version.is_empty()).then_some(version),
        update: UpdateAvailability::Unknown,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
    })
}
pub fn stage(id: &PackageId, cancel: &Cancellation) -> Result<Option<Staged>, EngineError> {
    stage_with_host(id, cancel, &Host::current())
}
fn stage_with_host(
    id: &PackageId,
    cancel: &Cancellation,
    host: &Host,
) -> Result<Option<Staged>, EngineError> {
    let Some(reference) = id
        .reference
        .as_deref()
        .and_then(|r| r.strip_prefix("artifact:"))
    else {
        return Ok(None);
    };
    let mut parts = reference.splitn(4, ':');
    let tag = parts.next().unwrap_or("");
    let expected = parts.next().unwrap_or("");
    let assertion_expected = parts.next().unwrap_or("");
    let source = parts.next().unwrap_or("");
    let kind = kind(source).ok_or_else(|| invalid("invalid package source"))?;
    if tag != kind.tag() || id.backend != selected_backend(kind, host)? || expected.len() != 64 {
        return Err(invalid("invalid package reference"));
    }
    let staged = materialize(source, kind, cancel, host)?;
    if hash(&staged.path, cancel)? != expected
        || staged
            .assertion()
            .map(|p| hash(p, cancel))
            .transpose()?
            .unwrap_or_default()
            != assertion_expected
    {
        return Err(invalid("package file changed since preview"));
    }
    let current = inspect_staged(&staged, source, kind, cancel, host)?;
    if current.id != *id {
        return Err(invalid("package identity changed since preview"));
    }
    Ok(Some(staged))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Runtime;
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    fn fixture_host(base: &Path) -> Host {
        let mut env = BTreeMap::new();
        env.insert(OsString::from("PATH"), base.as_os_str().to_os_string());
        Host::new(Runtime::Native, env)
    }
    fn tool(base: &Path, name: &str, script: &str) {
        let path = base.join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[test]
    fn routes_only_named_package_files() {
        for (name, expected) in [
            ("test.deb", Kind::Deb),
            ("test.rpm", Kind::Rpm),
            ("test.pkg.tar.zst", Kind::Arch),
            ("test.pkg.tar.xz", Kind::Arch),
            ("test.flatpak", Kind::Flatpak),
            ("test.snap", Kind::Snap),
            ("test.AppImage", Kind::AppImage),
        ] {
            assert_eq!(kind(name), Some(expected));
            assert_eq!(
                kind(&format!("https://example.invalid/{name}?download=1")),
                Some(expected)
            );
        }
        assert_eq!(kind("test.tar.gz"), None);
        assert_eq!(kind("test.sh"), None);
        assert!(!https_source("http://example.invalid/test.rpm"));
        assert!(!https_source("https://user@example.invalid/test.rpm"));
    }
    #[test]
    fn stages_regular_files_and_requires_snap_assertions() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-artifact-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let rpm = base.join("synthetic.rpm");
        fs::write(&rpm, b"synthetic package data").unwrap();
        let cancel = Cancellation::default();
        let stage =
            materialize(rpm.to_str().unwrap(), Kind::Rpm, &cancel, &Host::current()).unwrap();
        assert_eq!(fs::read(stage.path()).unwrap(), b"synthetic package data");
        fs::write(&rpm, b"changed package data").unwrap();
        assert_ne!(
            hash(stage.path(), &cancel).unwrap(),
            hash(&rpm, &cancel).unwrap()
        );
        let alias = base.join("alias.rpm");
        std::os::unix::fs::symlink(&rpm, &alias).unwrap();
        assert!(materialize(
            alias.to_str().unwrap(),
            Kind::Rpm,
            &cancel,
            &Host::current()
        )
        .is_err());
        let snap = base.join("synthetic.snap");
        fs::write(&snap, b"synthetic snap data").unwrap();
        assert!(materialize(
            snap.to_str().unwrap(),
            Kind::Snap,
            &cancel,
            &Host::current()
        )
        .is_err());
        let assertion = base.join("synthetic.assert");
        std::os::unix::fs::symlink(&rpm, &assertion).unwrap();
        assert!(materialize(
            snap.to_str().unwrap(),
            Kind::Snap,
            &cancel,
            &Host::current()
        )
        .is_err());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn reviewed_debian_archive_rejects_changed_bytes() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-artifact-deb-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let control = base.join("staging/DEBIAN");
        fs::create_dir_all(&control).unwrap();
        fs::write(control.join("control"), "Package: pkgdeck-artifact-test\nVersion: 1.0\nArchitecture: all\nMaintainer: Test <test@example.invalid>\nDescription: Synthetic archive\n").unwrap();
        let archive = base.join("sample.deb");
        assert!(Command::new("dpkg-deb")
            .args(["--build", "--root-owner-group"])
            .arg(base.join("staging"))
            .arg(&archive)
            .status()
            .unwrap()
            .success());
        let cancel = Cancellation::default();
        let package = inspect(archive.to_str().unwrap(), &cancel).unwrap();
        assert_eq!(package.id.name, "pkgdeck-artifact-test");
        assert!(stage(&package.id, &cancel).unwrap().is_some());
        fs::write(&archive, b"different archive").unwrap();
        assert!(stage(&package.id, &cancel).is_err());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn native_archive_metadata_and_digests_survive_preview() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-artifact-managers-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        tool(&base, "dnf", "exit 0");
        tool(
            &base,
            "rpm",
            "printf 'synthetic-rpm|1.2-3|x86_64|Synthetic RPM'",
        );
        tool(&base, "pacman", "printf 'Name : synthetic-arch\\nVersion : 2.0-1\\nArchitecture : x86_64\\nDescription : Synthetic Arch\\n'");
        tool(
            &base,
            "snap",
            "printf 'name: synthetic-snap\\nversion: 3.0\\nsummary: Synthetic Snap\\n'",
        );
        let host = fixture_host(&base);
        let cancel = Cancellation::default();
        for (file, expected_backend, expected_name) in [
            ("sample.rpm", "dnf", "synthetic-rpm"),
            ("sample.pkg.tar.xz", "pacman", "synthetic-arch"),
            ("sample.snap", "snap", "synthetic-snap"),
            ("sample.flatpak", "flatpak", "sample.flatpak"),
        ] {
            let path = base.join(file);
            fs::write(&path, b"synthetic package payload").unwrap();
            if file.ends_with(".snap") {
                fs::write(base.join("sample.assert"), b"synthetic signed assertion").unwrap();
            }
            let package = inspect_with_host(path.to_str().unwrap(), &cancel, &host).unwrap();
            assert_eq!(package.id.backend, expected_backend);
            assert_eq!(package.id.name, expected_name);
            let staged = stage_with_host(&package.id, &cancel, &host)
                .unwrap()
                .unwrap();
            assert_eq!(
                fs::read(staged.path()).unwrap(),
                b"synthetic package payload"
            );
            if file.ends_with(".snap") {
                assert!(staged.assertion().is_some());
            }
            fs::write(&path, b"changed package payload").unwrap();
            assert!(stage_with_host(&package.id, &cancel, &host).is_err());
        }
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn https_download_is_staged_and_revalidated() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-artifact-https-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let payload = base.join("payload");
        fs::write(&payload, b"synthetic remote package").unwrap();
        tool(&base, "dnf", "exit 0");
        tool(&base, "rpm", "printf 'remote-rpm|1.0-1|x86_64|Remote RPM'");
        tool(&base, "curl", &format!("output=''\nwhile [ \"$#\" -gt 0 ]; do\n if [ \"$1\" = '--output' ]; then shift; output=\"$1\"; fi\n shift\ndone\n/bin/cp '{}' \"$output\"", payload.display()));
        let host = fixture_host(&base);
        let cancel = Cancellation::default();
        let package =
            inspect_with_host("https://example.invalid/sample.rpm", &cancel, &host).unwrap();
        assert_eq!(package.id.name, "remote-rpm");
        assert!(stage_with_host(&package.id, &cancel, &host)
            .unwrap()
            .is_some());
        fs::write(&payload, b"different remote package").unwrap();
        assert!(stage_with_host(&package.id, &cancel, &host).is_err());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn rpm_uses_zypper_when_dnf_is_absent() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-artifact-zypper-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        tool(&base, "zypper", "exit 0");
        tool(
            &base,
            "rpm",
            "printf 'synthetic-rpm|1.0-1|x86_64|Synthetic RPM'",
        );
        let source = base.join("sample.rpm");
        fs::write(&source, b"synthetic rpm payload").unwrap();
        let host = fixture_host(&base);
        let cancel = Cancellation::default();
        let package = inspect_with_host(source.to_str().unwrap(), &cancel, &host).unwrap();
        assert_eq!(package.id.backend, "zypper");
        assert!(stage_with_host(&package.id, &cancel, &host)
            .unwrap()
            .is_some());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn remote_appimage_and_snap_keep_reviewed_bytes() {
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-artifact-remote-mixed-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let mut elf = [0_u8; 64];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4..7].copy_from_slice(&[2, 1, 1]);
        elf[8..11].copy_from_slice(b"AI\x02");
        let machine = if std::env::consts::ARCH == "aarch64" {
            183_u16
        } else {
            62_u16
        };
        elf[18..20].copy_from_slice(&machine.to_le_bytes());
        elf[20..24].copy_from_slice(&1_u32.to_le_bytes());
        elf[52..54].copy_from_slice(&64_u16.to_le_bytes());
        fs::write(base.join("appimage"), elf).unwrap();
        fs::write(base.join("snap"), b"synthetic remote snap payload").unwrap();
        fs::write(base.join("assert"), b"synthetic remote assertion").unwrap();
        tool(
            &base,
            "snap",
            "printf 'name: remote-snap\\nversion: 1.0\\nsummary: Remote Snap\\n'",
        );
        tool(&base, "curl", &format!("output=''\nsource=''\nwhile [ \"$#\" -gt 0 ]; do\n if [ \"$1\" = '--output' ]; then shift; output=\"$1\"; else source=\"$1\"; fi\n shift\ndone\ncase \"$source\" in\n *.AppImage) /bin/cp '{}' \"$output\" ;;\n *.assert) /bin/cp '{}' \"$output\" ;;\n *.snap) /bin/cp '{}' \"$output\" ;;\nesac", base.join("appimage").display(), base.join("assert").display(), base.join("snap").display()));
        let host = fixture_host(&base);
        let cancel = Cancellation::default();
        for (url, backend) in [
            ("https://example.invalid/Remote.AppImage", "appimage"),
            ("https://example.invalid/remote.snap", "snap"),
        ] {
            let package = inspect_with_host(url, &cancel, &host).unwrap();
            assert_eq!(package.id.backend, backend);
            assert!(stage_with_host(&package.id, &cancel, &host)
                .unwrap()
                .is_some());
        }
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn rejects_invalid_sources_metadata_and_changed_package_identity() {
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-artifact-rejections-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let host = fixture_host(&base);
        let cancel = Cancellation::default();
        assert!(selected_backend(Kind::Rpm, &host).is_err());
        assert!(materialize("relative.rpm", Kind::Rpm, &cancel, &host).is_err());
        assert!(download(
            "http://example.invalid/file.rpm",
            &base.join("unused"),
            MAX_BYTES,
            &cancel,
            &host
        )
        .is_err());
        let empty = base.join("empty.rpm");
        fs::write(&empty, b"").unwrap();
        assert!(hash(&empty, &cancel).is_err());
        let huge = base.join("huge.rpm");
        fs::File::create(&huge)
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(materialize(huge.to_str().unwrap(), Kind::Rpm, &cancel, &host).is_err());
        assert!(metadata("rpm", vec![], &cancel, &host).is_err());
        assert!(fields("name|version", '|').is_err());
        let source = base.join("synthetic.rpm");
        fs::write(&source, b"synthetic rpm payload").unwrap();
        tool(&base, "dnf", "exit 0");
        tool(
            &base,
            "rpm",
            "printf 'synthetic|1.0-1|x86_64|Synthetic RPM'",
        );
        let package = inspect_with_host(source.to_str().unwrap(), &cancel, &host).unwrap();
        let mut wrong_backend = package.id.clone();
        wrong_backend.backend = "zypper".into();
        assert!(stage_with_host(&wrong_backend, &cancel, &host).is_err());
        let mut wrong_reference = package.id.clone();
        wrong_reference.reference = wrong_reference
            .reference
            .map(|reference| reference.replacen("artifact:rpm:", "artifact:snap:", 1));
        assert!(stage_with_host(&wrong_reference, &cancel, &host).is_err());
        let interrupted = Cancellation::default();
        interrupted.cancel();
        assert!(hash(&source, &interrupted).is_err());
        assert!(materialize(source.to_str().unwrap(), Kind::Rpm, &interrupted, &host).is_err());
        assert!(
            inspect_with_host("https://user@example.invalid/synthetic.rpm", &cancel, &host)
                .is_err()
        );
        tool(&base, "rpm", "printf 'other|1.0-1|x86_64|Other RPM'");
        assert!(stage_with_host(&package.id, &cancel, &host).is_err());
        tool(&base, "rpm", "exit 1");
        assert!(metadata("rpm", vec![], &cancel, &host).is_err());
        tool(&base, "curl", "exit 1");
        assert!(download(
            "https://example.invalid/synthetic.rpm",
            &base.join("unused"),
            MAX_BYTES,
            &cancel,
            &host
        )
        .is_err());
        tool(&base, "pacman", "printf 'Name: missing-version\\n'");
        let arch = base.join("synthetic.pkg.tar.zst");
        fs::write(&arch, b"synthetic arch payload").unwrap();
        assert!(inspect_with_host(arch.to_str().unwrap(), &cancel, &host).is_err());
        tool(&base, "snap", "printf 'version: 1.0\\n'");
        let snap = base.join("synthetic.snap");
        fs::write(&snap, b"synthetic snap payload").unwrap();
        let assertion = base.join("synthetic.assert");
        fs::write(&assertion, b"synthetic assertion").unwrap();
        assert!(inspect_with_host(snap.to_str().unwrap(), &cancel, &host).is_err());
        fs::write(&assertion, vec![b'a'; 1024 * 1024 + 1]).unwrap();
        assert!(materialize(snap.to_str().unwrap(), Kind::Snap, &cancel, &host).is_err());
        let mut plain = package.id;
        plain.reference = None;
        assert!(stage_with_host(&plain, &cancel, &host).unwrap().is_none());
        fs::remove_dir_all(base).unwrap();
    }
}
