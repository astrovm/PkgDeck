//! Bounded Flatpak reference parsing for explicit user-opened files and links.
use crate::{
    engine::EngineError,
    host::Host,
    package::{Package, PackageId, Scope, UpdateAvailability},
    process::{Cancellation, Limits},
};
use sha2::{Digest, Sha256};
use std::{ffi::OsString, fs, io::Read, path::Path, time::Duration};

const MAX_BYTES: usize = 64 * 1024;
fn invalid(reason: impl ToString) -> EngineError {
    EngineError::InvalidResponse {
        backend: "flatpak".into(),
        reason: reason.to_string(),
    }
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
fn https_url(value: &str, suffix: &str) -> bool {
    let Some(rest) = value.strip_prefix("https://") else {
        return false;
    };
    let Some((authority, path)) = rest.split_once('/') else {
        return false;
    };
    !authority.is_empty()
        && !authority.contains('@')
        && !authority.contains('#')
        && !value.bytes().any(|b| b.is_ascii_whitespace() || b < 0x20)
        && path
            .split(['?', '#'])
            .next()
            .is_some_and(|path| path.ends_with(suffix))
}
fn reference_bytes(source: &str, cancel: &Cancellation) -> Result<Vec<u8>, EngineError> {
    if source.starts_with("https://") {
        if !https_url(source, ".flatpakref") {
            return Err(invalid("expected an HTTPS .flatpakref URL"));
        }
        let args = [
            "-q",
            "--silent",
            "--show-error",
            "--fail",
            "--proto",
            "=https",
            "--max-redirs",
            "0",
            "--max-filesize",
            "65536",
            "--connect-timeout",
            "5",
            "--max-time",
            "15",
            source,
        ]
        .map(OsString::from);
        let host = Host::current();
        let executable = host
            .resolve("curl")?
            .ok_or_else(|| invalid("curl is unavailable"))?;
        let result = host.read(
            &executable,
            &args,
            Limits {
                timeout: Duration::from_secs(18),
                output_bytes: MAX_BYTES,
            },
            cancel,
        )?;
        if result.code != Some(0) || result.truncated {
            return Err(invalid("could not read bounded HTTPS Flatpak reference"));
        }
        Ok(result.stdout)
    } else {
        let path = Path::new(source);
        if !path.is_absolute() || path.extension().is_none_or(|ext| ext != "flatpakref") {
            return Err(invalid("expected an absolute .flatpakref path"));
        }
        let metadata = fs::symlink_metadata(path).map_err(invalid)?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_BYTES as u64 {
            return Err(invalid(
                "expected a regular Flatpak reference no larger than 64 KiB",
            ));
        }
        let mut bytes = Vec::new();
        fs::File::open(path)
            .map_err(invalid)?
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(invalid)?;
        if bytes.len() > MAX_BYTES {
            return Err(invalid("Flatpak reference exceeds 64 KiB"));
        }
        Ok(bytes)
    }
}
fn inspect_bytes(source: &str, bytes: &[u8]) -> Result<Package, EngineError> {
    let hash = format!("{:x}", Sha256::digest(bytes));
    let text = std::str::from_utf8(bytes).map_err(invalid)?;
    let mut section = "";
    let mut fields = std::collections::BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
            continue;
        }
        if section == "Flatpak Ref" {
            if let Some((key, value)) = line.split_once('=') {
                fields.insert(key.trim(), value.trim());
            }
        }
    }
    let name = *fields
        .get("Name")
        .ok_or_else(|| invalid("Flatpak reference lacks Name"))?;
    let branch = fields.get("Branch").copied().unwrap_or("master");
    let url = *fields
        .get("Url")
        .ok_or_else(|| invalid("Flatpak reference lacks repository URL"))?;
    if !identifier(name) || !identifier(branch) || !https_url(url, "") {
        return Err(invalid("invalid Flatpak name, branch, or repository URL"));
    }
    let runtime_repo = fields.get("RuntimeRepo").copied().unwrap_or("");
    if !runtime_repo.is_empty() && !https_url(runtime_repo, ".flatpakrepo") {
        return Err(invalid("invalid runtime repository URL"));
    }
    let title = fields.get("Title").copied().unwrap_or(name);
    let runtime = match fields.get("IsRuntime").copied().unwrap_or("false") {
        "true" => true,
        "false" => false,
        _ => return Err(invalid("invalid IsRuntime value")),
    };
    let remote_name = fields
        .get("SuggestRemoteName")
        .copied()
        .unwrap_or("manager-selected");
    if remote_name != "manager-selected" && !identifier(remote_name) {
        return Err(invalid("invalid suggested Flatpak remote name"));
    }
    let signing_key = fields.get("GPGKey").is_some_and(|key| !key.is_empty());
    let arch = std::env::consts::ARCH;
    let scope = Scope::User {
        uid: rustix::process::getuid().as_raw(),
    };
    let summary = format!("Reference: {source}\n{}: {name}/{arch}/{branch}\nRepository to add if needed: {url}\nSuggested remote: {remote_name}\nSigning key: {}\nRuntime repository to add if needed: {}\nAdditional runtimes may be installed.", if runtime { "Runtime" } else { "Application" }, if signing_key { "included" } else { "not included" }, if runtime_repo.is_empty() { "none" } else { runtime_repo });
    Ok(Package {
        id: PackageId {
            backend: "flatpak".into(),
            name: name.into(),
            architecture: arch.into(),
            scope,
            remote: None,
            reference: Some(format!("flatpakref:{hash}:{source}")),
        },
        display_name: title.into(),
        summary,
        installed_version: None,
        candidate_version: Some(branch.into()),
        update: UpdateAvailability::Unknown,
        icon: None,
        component_ids: vec![name.into()],
        homepages: vec![],
    })
}
pub fn inspect(source: &str, cancel: &Cancellation) -> Result<Package, EngineError> {
    inspect_bytes(source, &reference_bytes(source, cancel)?)
}
pub fn verified_source(
    id: &PackageId,
    cancel: &Cancellation,
) -> Result<Option<Vec<u8>>, EngineError> {
    let Some(reference) = id
        .reference
        .as_deref()
        .and_then(|value| value.strip_prefix("flatpakref:"))
    else {
        return Ok(None);
    };
    let (expected, source) = reference
        .split_once(':')
        .ok_or_else(|| invalid("invalid Flatpak reference identity"))?;
    let bytes = reference_bytes(source, cancel)?;
    let current = inspect_bytes(source, &bytes)?;
    if current.id != *id
        || current
            .id
            .reference
            .as_deref()
            .is_none_or(|value| !value.starts_with(&format!("flatpakref:{expected}:")))
    {
        return Err(invalid("Flatpak reference changed since preview"));
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_valid_reference_and_rejects_bad_origins() {
        let base = std::env::temp_dir().join(format!("pkgdeck-flatpakref-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        let path = base.join("Example App.flatpakref");
        fs::write(&path, "[Flatpak Ref]\nName=org.example.App\nBranch=stable\nUrl=https://example.org/repo\nRuntimeRepo=https://example.org/runtime.flatpakrepo\n").unwrap();
        let cancel = Cancellation::default();
        let package = inspect(path.to_str().unwrap(), &cancel).unwrap();
        assert_eq!(package.id.name, "org.example.App");
        assert!(verified_source(&package.id, &cancel).unwrap().is_some());
        fs::write(
            &path,
            "[Flatpak Ref]\nName=org.example.Other\nUrl=https://example.org/repo\n",
        )
        .unwrap();
        assert!(verified_source(&package.id, &cancel).is_err());
        assert!(!https_url(
            "https://user@example.org/app.flatpakref",
            ".flatpakref"
        ));
        fs::remove_dir_all(base).unwrap();
    }
}
