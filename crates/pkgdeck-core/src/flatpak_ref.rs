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
fn reference_bytes(
    source: &str,
    cancel: &Cancellation,
    host: &Host,
) -> Result<Vec<u8>, EngineError> {
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
    let hash = hex::encode(Sha256::digest(bytes));
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
    inspect_bytes(source, &reference_bytes(source, cancel, &Host::current())?)
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
    let bytes = reference_bytes(source, cancel, &Host::current())?;
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
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn remote_reference_reads_bounded_https_without_following_redirects() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-flatpakref-https-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let script = base.join("curl");
        let calls = base.join("calls");
        let reference =
            b"[Flatpak Ref]\nName=org.example.Remote\nUrl=https://example.invalid/repo\n";
        fs::write(base.join("reference"), reference).unwrap();
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n/usr/bin/cat '{}'\n",
                calls.display(),
                base.join("reference").display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let host = Host::new(
            crate::host::Runtime::Native,
            [(OsString::from("PATH"), base.as_os_str().to_os_string())]
                .into_iter()
                .collect(),
        );
        let cancel = Cancellation::default();
        let url = "https://example.invalid/a.flatpakref";
        let bytes = reference_bytes(url, &cancel, &host).unwrap();
        assert_eq!(bytes, reference);
        let package = inspect_bytes(url, &bytes).unwrap();
        assert_eq!(package.id.name, "org.example.Remote");
        assert_eq!(package.candidate_version.as_deref(), Some("master"));
        let args = fs::read_to_string(&calls).unwrap();
        assert!(args.contains("--max-redirs\n0\n"));
        assert!(args.contains("--proto\n=https\n"));
        assert!(args.ends_with("https://example.invalid/a.flatpakref\n"));
        assert!(reference_bytes("https://example.invalid/a.txt", &cancel, &host).is_err());
        assert!(reference_bytes("http://example.invalid/a.flatpakref", &cancel, &host).is_err());
        assert!(reference_bytes("https://example.invalid", &cancel, &host).is_err());
        fs::write(&script, "#!/bin/sh\nexit 22\n").unwrap();
        assert!(reference_bytes(url, &cancel, &host).is_err());
        fs::remove_file(&script).unwrap();
        assert!(reference_bytes(url, &cancel, &host).is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn local_reference_rejects_symlinks_and_large_files_without_parsing() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-flatpakref-limits-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let source = base.join("reference.flatpakref");
        fs::write(
            &source,
            b"[Flatpak Ref]\nName=org.example.App\nUrl=https://example.invalid/repo\n",
        )
        .unwrap();
        let alias = base.join("alias.flatpakref");
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        let cancel = Cancellation::default();
        assert!(inspect(alias.to_str().unwrap(), &cancel).is_err());
        let oversized = fs::OpenOptions::new().write(true).open(&source).unwrap();
        oversized.set_len(MAX_BYTES as u64 + 1).unwrap();
        assert!(inspect(source.to_str().unwrap(), &cancel).is_err());
        fs::remove_dir_all(base).unwrap();
    }
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

    #[test]
    fn reference_metadata_rejects_invalid_identity_scope_and_repository() {
        let source = "/tmp/example.flatpakref";
        for body in [
            "[Flatpak Ref]\nUrl=https://example.invalid/repo\n",
            "[Flatpak Ref]\nName=org.example.App\n",
            "[Flatpak Ref]\nName=-bad\nUrl=https://example.invalid/repo\n",
            "[Flatpak Ref]\nName=org.example.App\nBranch=-bad\nUrl=https://example.invalid/repo\n",
            "[Flatpak Ref]\nName=org.example.App\nUrl=http://example.invalid/repo\n",
            "[Flatpak Ref]\nName=org.example.App\nUrl=https://example.invalid/repo\nRuntimeRepo=http://example.invalid/runtime.flatpakrepo\n",
            "[Flatpak Ref]\nName=org.example.App\nUrl=https://example.invalid/repo\nIsRuntime=perhaps\n",
            "[Flatpak Ref]\nName=org.example.App\nUrl=https://example.invalid/repo\nSuggestRemoteName=-bad\n",
        ] {
            assert!(inspect_bytes(source, body.as_bytes()).is_err(), "{body}");
        }
        assert!(inspect_bytes(source, &[0xff]).is_err());
        let runtime = inspect_bytes(source, b"[Flatpak Ref]\nName=org.example.Runtime\nUrl=https://example.invalid/repo\nIsRuntime=true\nGPGKey=synthetic\nSuggestRemoteName=example\n").unwrap();
        assert!(runtime.summary.contains("Runtime:"));
        assert!(runtime.summary.contains("Signing key: included"));
        assert!(runtime.summary.contains("Suggested remote: example"));
    }
}
