//! Read-only local Debian package inspection. APT still owns the transaction.
use crate::{
    engine::EngineError,
    host::Host,
    package::{Package, PackageId, Scope, UpdateAvailability},
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
    let mut total = 8_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        let count = file.read(&mut buffer).map_err(invalid)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_BYTES {
            return Err(invalid("Debian archive exceeds 1 GiB"));
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn field<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        line.strip_prefix(name)
            .and_then(|value| value.strip_prefix(':'))
            .map(str::trim)
    })
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
    if !path.is_absolute() || path.extension().is_none_or(|ext| ext != "deb") {
        return Err(invalid("expected an absolute .deb path"));
    }
    let hash = digest(path, cancel)?;
    let host = Host::current();
    let executable = host
        .resolve("dpkg-deb")?
        .ok_or_else(|| invalid("dpkg-deb is unavailable"))?;
    let result = host.read(
        &executable,
        &[OsString::from("--field"), path.as_os_str().to_os_string()],
        Limits {
            timeout: Duration::from_secs(15),
            output_bytes: 64 * 1024,
        },
        cancel,
    )?;
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
    Ok(Package {
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
        homepages: vec![],
    })
}

pub fn verified_path(
    id: &PackageId,
    cancel: &Cancellation,
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
    let current = inspect(&path, cancel)?;
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
    let Some(source) = verified_path(id, cancel)? else {
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
    let mut input = fs::File::open(&source).map_err(invalid)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(invalid)?;
    let staged = StagedArchive { path };
    let result = (|| {
        let mut hash = Sha256::new();
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
                return Err(invalid("Debian archive exceeds 1 GiB"));
            }
            hash.update(&buffer[..count]);
            output.write_all(&buffer[..count]).map_err(invalid)?;
        }
        output.sync_all().map_err(invalid)?;
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        if format!("{:x}", hash.finalize()) != expected {
            return Err(invalid("Debian archive changed during staging"));
        }
        Ok(())
    })();
    result?;
    Ok(Some(staged))
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn inspects_synthetic_deb_and_rejects_changes_after_preview() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-local-deb-valid-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let control = base.join("staging/DEBIAN");
        fs::create_dir_all(&control).unwrap();
        fs::write(control.join("control"), "Package: pkgdeck-synthetic\nVersion: 1.2.3\nArchitecture: all\nMaintainer: PkgDeck tests <nobody@example.invalid>\nDescription: Synthetic fixture\n").unwrap();
        let archive = base.join("Synthetic package.deb");
        let status = Command::new("dpkg-deb")
            .arg("--build")
            .arg(base.join("staging"))
            .arg(&archive)
            .status()
            .unwrap();
        assert!(status.success());
        let cancel = Cancellation::default();
        let package = inspect(&archive, &cancel).unwrap();
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
}
