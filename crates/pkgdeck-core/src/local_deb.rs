//! Read-only local Debian package inspection. APT still owns the transaction.
use crate::{
    engine::EngineError,
    host::Host,
    package::{Package, PackageDetails, PackageId, Scope, UpdateAvailability},
    process::{Cancellation, Limits},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: u64 = 1024 * 1024 * 1024;

fn invalid(reason: impl ToString) -> EngineError {
    EngineError::InvalidResponse {
        backend: "apt".into(),
        reason: reason.to_string(),
    }
}

fn digest(path: &Path, cancel: &Cancellation) -> Result<String, EngineError> {
    let metadata = fs::symlink_metadata(path).map_err(invalid)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_BYTES || metadata.len() < 8 {
        return Err(invalid(
            "expected a regular Debian archive no larger than 1 GiB",
        ));
    }
    let mut file = fs::File::open(path).map_err(invalid)?;
    let mut magic = [0; 8];
    file.read_exact(&mut magic).map_err(invalid)?;
    if &magic != b"!<arch>\n" {
        return Err(invalid("invalid Debian archive header"));
    }
    let mut hash = Sha256::new();
    hash.update(magic);
    // Bytes past the limit, if the file grows meanwhile, are never read.
    let mut file = file.take(MAX_BYTES - 8);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let count = file.read(&mut buffer).map_err(invalid)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hex::encode(hash.finalize()))
}

fn field<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        line.strip_prefix(name)
            .and_then(|value| value.strip_prefix(':'))
            .map(str::trim)
    })
}
/// A Debian `Description`'s extended text: the indented lines after the
/// first, with ` .` marking an empty line between paragraphs.
fn extended_description(text: &str) -> String {
    let mut lines = text
        .lines()
        .skip_while(|line| !line.starts_with("Description:"));
    lines.next();
    let mut out = String::new();
    for line in lines.take_while(|line| line.starts_with(' ') || line.starts_with('\t')) {
        let line = line.trim();
        if line == "." {
            out.push_str("\n\n");
        } else {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push(' ');
            }
            out.push_str(line);
        }
    }
    out.trim().to_owned()
}
fn package_name(value: &str) -> bool {
    value.len() >= 2
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"+.-".contains(&byte)
        })
}
fn valid_architecture(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
fn valid_version(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+.:~-".contains(&byte))
}

pub fn inspect(path: &Path, cancel: &Cancellation) -> Result<Package, EngineError> {
    inspect_details(path, cancel).map(|details| details.package)
}
/// The archive's package with what its control file says about it: the
/// extended description, homepage and dependencies.
pub fn inspect_details(path: &Path, cancel: &Cancellation) -> Result<PackageDetails, EngineError> {
    inspect_with(path, cancel, &Host::current())
}
fn inspect_with(
    path: &Path,
    cancel: &Cancellation,
    host: &Host,
) -> Result<PackageDetails, EngineError> {
    if !path.is_absolute() || path.extension().is_none_or(|ext| ext != "deb") {
        return Err(invalid("expected an absolute .deb path"));
    }
    let hash = digest(path, cancel)?;
    let executable = host
        .resolve("dpkg-deb")?
        .ok_or_else(|| invalid("dpkg-deb is unavailable"))?;
    let args = [OsString::from("--field"), path.as_os_str().to_os_string()];
    let limits = Limits {
        timeout: Duration::from_secs(15),
        output_bytes: 64 * 1024,
    };
    let result = host.read(&executable, &args, limits, cancel)?;
    if result.code != Some(0) || result.truncated {
        return Err(invalid("dpkg-deb could not read bounded control metadata"));
    }
    let metadata = String::from_utf8(result.stdout).map_err(invalid)?;
    let name = field(&metadata, "Package").ok_or_else(|| invalid("missing Debian package name"))?;
    let version = field(&metadata, "Version").ok_or_else(|| invalid("missing Debian version"))?;
    let architecture =
        field(&metadata, "Architecture").ok_or_else(|| invalid("missing Debian architecture"))?;
    if !package_name(name) || !valid_version(version) || !valid_architecture(architecture) {
        return Err(invalid("invalid Debian package identity"));
    }
    let summary = field(&metadata, "Description")
        .unwrap_or("Local Debian archive")
        .to_owned();
    let homepage = field(&metadata, "Homepage")
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
        .map(str::to_owned);
    let dependencies = field(&metadata, "Depends")
        .map(|depends| {
            depends
                .split(',')
                .map(str::trim)
                .filter(|dependency| !dependency.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let package = Package {
        id: PackageId {
            backend: "apt".into(),
            name: name.into(),
            architecture: architecture.into(),
            scope: Scope::System,
            remote: None,
            reference: Some(format!("local-deb:{hash}:{}", path.display())),
        },
        display_name: name.into(),
        summary: format!("{summary}\nLocal archive: {}", path.display()),
        installed_version: None,
        candidate_version: Some(version.into()),
        update: UpdateAvailability::Unknown,
        icon: None,
        component_ids: vec![],
        homepages: homepage.iter().cloned().collect(),
        adopt_with: None,
    };
    Ok(PackageDetails {
        description: extended_description(&metadata),
        homepage,
        dependencies,
        package,
    })
}

pub fn verified_path(
    id: &PackageId,
    cancel: &Cancellation,
) -> Result<Option<PathBuf>, EngineError> {
    verified_path_with(id, cancel, &Host::current())
}
fn verified_path_with(
    id: &PackageId,
    cancel: &Cancellation,
    host: &Host,
) -> Result<Option<PathBuf>, EngineError> {
    let Some(reference) = id
        .reference
        .as_deref()
        .and_then(|value| value.strip_prefix("local-deb:"))
    else {
        return Ok(None);
    };
    let (expected, path) = reference
        .split_once(':')
        .ok_or_else(|| invalid("invalid local archive reference"))?;
    let path = PathBuf::from(path);
    if digest(&path, cancel)? != expected {
        return Err(invalid("Debian archive changed since preview"));
    }
    let current = inspect_with(&path, cancel, host)?.package;
    if current.id != *id {
        return Err(invalid("Debian package identity changed since preview"));
    }
    Ok(Some(path))
}

/// Hold a private copy of the reviewed bytes for the entire APT transaction.
/// A changed source cannot be substituted between revalidation and apt-get.
pub struct StagedArchive {
    path: PathBuf,
}
impl StagedArchive {
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for StagedArchive {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
pub fn stage(id: &PackageId, cancel: &Cancellation) -> Result<Option<StagedArchive>, EngineError> {
    stage_with(id, cancel, &Host::current())
}
fn stage_with(
    id: &PackageId,
    cancel: &Cancellation,
    host: &Host,
) -> Result<Option<StagedArchive>, EngineError> {
    let Some(source) = verified_path_with(id, cancel, host)? else {
        return Ok(None);
    };
    let expected = id
        .reference
        .as_deref()
        .and_then(|value| value.strip_prefix("local-deb:"))
        .and_then(|value| value.split_once(':'))
        .map(|(hash, _)| hash)
        .ok_or_else(|| invalid("invalid local archive reference"))?;
    let path = std::env::temp_dir().join(format!(
        "pkgdeck-deb-{}-{}.deb",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let input = fs::File::open(&source).map_err(invalid)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(invalid)?;
    let staged = StagedArchive { path };
    copy_reviewed(input, &mut output, expected, cancel)?;
    output.sync_all().map_err(invalid)?;
    Ok(Some(staged))
}

/// Copy at most the size limit, and only if the bytes still match `expected`.
fn copy_reviewed(
    input: impl Read,
    output: &mut impl Write,
    expected: &str,
    cancel: &Cancellation,
) -> Result<(), EngineError> {
    let mut input = input.take(MAX_BYTES);
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let count = input.read(&mut buffer).map_err(invalid)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        output.write_all(&buffer[..count]).map_err(invalid)?;
    }
    if cancel.requested() {
        return Err(EngineError::Cancelled);
    }
    if hex::encode(hash.finalize()) != expected {
        return Err(invalid("Debian archive changed during staging"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use std::process::Command;
    #[test]
    fn rejects_malformed_archives_and_symlinks() {
        let base = std::env::temp_dir().join(format!("pkgdeck-local-deb-{}", std::process::id()));
        let _ = fs::create_dir_all(&base);
        let archive = base.join("package with spaces.deb");
        fs::write(&archive, b"not a debian archive").unwrap();
        assert!(inspect(&archive, &Cancellation::default()).is_err());
        let alias = base.join("alias.deb");
        std::os::unix::fs::symlink(&archive, &alias).unwrap();
        assert!(inspect(&alias, &Cancellation::default()).is_err());
        assert!(inspect(Path::new("relative.deb"), &Cancellation::default()).is_err());
        let cancelled = Cancellation::default();
        cancelled.cancel();
        let header_only = base.join("header.deb");
        fs::write(&header_only, b"!<arch>\n").unwrap();
        assert_eq!(
            digest(&header_only, &cancelled),
            Err(EngineError::Cancelled)
        );
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn inspects_synthetic_deb_and_rejects_changes_after_preview() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-local-deb-valid-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let control = base.join("staging/DEBIAN");
        fs::create_dir_all(&control).unwrap();
        fs::write(control.join("control"), "Package: pkgdeck-synthetic\nVersion: 1.2.3\nArchitecture: all\nMaintainer: PkgDeck tests <nobody@example.invalid>\nDepends: libc6 (>= 2.34), zlib1g\nHomepage: https://example.invalid/synthetic\nDescription: Synthetic fixture\n A fixture that\n spans lines.\n .\n Second paragraph.\n").unwrap();
        let archive = base.join("Synthetic package.deb");
        let status = Command::new("dpkg-deb")
            .arg("--build")
            .arg(base.join("staging"))
            .arg(&archive)
            .status()
            .unwrap();
        assert!(status.success());
        let cancel = Cancellation::default();
        let details = inspect_details(&archive, &cancel).unwrap();
        assert_eq!(
            details.description,
            "A fixture that spans lines.\n\nSecond paragraph."
        );
        assert_eq!(
            details.homepage.as_deref(),
            Some("https://example.invalid/synthetic")
        );
        assert_eq!(details.dependencies, ["libc6 (>= 2.34)", "zlib1g"]);
        let package = inspect(&archive, &cancel).unwrap();
        assert_eq!(package, details.package);
        assert_eq!(package.homepages, ["https://example.invalid/synthetic"]);
        assert_eq!(package.id.name, "pkgdeck-synthetic");
        assert_eq!(package.candidate_version.as_deref(), Some("1.2.3"));
        assert_eq!(
            verified_path(&package.id, &cancel).unwrap(),
            Some(archive.clone())
        );
        let mut file = fs::OpenOptions::new().append(true).open(&archive).unwrap();
        use std::io::Write;
        file.write_all(b"changed").unwrap();
        assert!(verified_path(&package.id, &cancel).is_err());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn reviewed_archives_are_read_through_dpkg_deb_and_staged_unchanged() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-local-deb-host-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        let tool = base.join("dpkg-deb");
        fs::write(
            &tool,
            format!(
                "#!/bin/sh\ndir='{}'\n[ -e \"$dir/fail\" ] && exit 2\n/bin/cat \"$dir/control\"\n[ -e \"$dir/mutate\" ] && printf x >> \"$2\"\nexit 0\n",
                base.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let control = |text: &str| fs::write(base.join("control"), text).unwrap();
        control("Package: synthetic\nVersion: 1.0\nArchitecture: all\nDescription: Synthetic\n");
        let archive = base.join("synthetic.deb");
        fs::write(&archive, b"!<arch>\nsynthetic payload").unwrap();
        let host = Host::new(
            crate::host::Runtime::Native,
            [(OsString::from("PATH"), base.as_os_str().to_owned())].into(),
        );
        let cancel = Cancellation::default();
        let package = inspect_with(&archive, &cancel, &host).unwrap().package;
        assert_eq!(package.id.name, "synthetic");
        assert_eq!(package.candidate_version.as_deref(), Some("1.0"));
        let staged = stage_with(&package.id, &cancel, &host).unwrap().unwrap();
        assert_eq!(
            fs::read(staged.path()).unwrap(),
            b"!<arch>\nsynthetic payload"
        );
        drop(staged);
        control("Package: other\nVersion: 1.0\nArchitecture: all\n");
        assert!(matches!(
            verified_path_with(&package.id, &cancel, &host),
            Err(EngineError::InvalidResponse { reason, .. }) if reason.contains("identity changed")
        ));
        control("Package: Not-Valid\nVersion: 1.0\nArchitecture: all\n");
        assert!(matches!(
            inspect_with(&archive, &cancel, &host),
            Err(EngineError::InvalidResponse { reason, .. }) if reason == "invalid Debian package identity"
        ));
        control("Package: synthetic\nVersion: 1.0\nArchitecture: all\nDescription: Synthetic\n");
        // The archive changes right after its last check.
        fs::write(base.join("mutate"), "").unwrap();
        assert!(matches!(
            stage_with(&package.id, &cancel, &host),
            Err(EngineError::InvalidResponse { reason, .. }) if reason.contains("changed during staging")
        ));
        fs::write(base.join("fail"), "").unwrap();
        assert!(matches!(
            inspect_with(&archive, &cancel, &host),
            Err(EngineError::InvalidResponse { reason, .. }) if reason.contains("could not read")
        ));
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn staging_copies_stop_when_cancelled() {
        struct CancelAtEnd<'a>(&'a [u8], &'a Cancellation);
        impl Read for CancelAtEnd<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let count = self.0.read(buffer)?;
                if count == 0 {
                    self.1.cancel();
                }
                Ok(count)
            }
        }
        let expected = hex::encode(Sha256::digest(b"payload"));
        let mut output = Vec::new();
        copy_reviewed(
            &b"payload"[..],
            &mut output,
            &expected,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(output, b"payload");
        let late = Cancellation::default();
        assert_eq!(
            copy_reviewed(
                CancelAtEnd(b"payload", &late),
                &mut Vec::new(),
                &expected,
                &late
            ),
            Err(EngineError::Cancelled)
        );
        assert_eq!(
            copy_reviewed(&b"payload"[..], &mut Vec::new(), &expected, &late),
            Err(EngineError::Cancelled)
        );
    }
}
