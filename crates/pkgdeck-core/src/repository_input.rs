//! Preview explicit repository files before adding them to native managers.
use crate::{
    engine::EngineError,
    host::{Authorization, Host},
    process::{Cancellation, Limits},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Import {
    pub source: String,
    pub backend: String,
    pub suffix: String,
    pub name: String,
    pub description: String,
    digest: String,
}
fn invalid(reason: impl ToString) -> EngineError {
    EngineError::InvalidResponse {
        backend: "repositories".into(),
        reason: reason.to_string(),
    }
}
pub fn supported(source: &str) -> bool {
    [".flatpakrepo", ".repo", ".sources", ".list", ".ymp"]
        .iter()
        .any(|suffix| {
            source
                .split(['?', '#'])
                .next()
                .unwrap_or(source)
                .ends_with(suffix)
        })
}
fn source_bytes(source: &str, cancel: &Cancellation, host: &Host) -> Result<Vec<u8>, EngineError> {
    if source.starts_with("https://") {
        if !crate::artifact::https_source(source) {
            return Err(invalid("invalid HTTPS repository link"));
        }
        let curl = host
            .resolve("curl")?
            .ok_or_else(|| invalid("curl is unavailable"))?;
        let args = [
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
            "1048576",
            "--connect-timeout",
            "5",
            "--max-time",
            "20",
            source,
        ]
        .map(OsString::from);
        let result = host.read(
            &curl,
            &args,
            Limits {
                timeout: Duration::from_secs(25),
                output_bytes: MAX_BYTES,
            },
            cancel,
        )?;
        if result.code != Some(0) || result.truncated {
            return Err(invalid("could not read repository file"));
        }
        Ok(result.stdout)
    } else {
        let path = Path::new(source);
        if !path.is_absolute() {
            return Err(invalid("choose an absolute repository file"));
        }
        let meta = fs::symlink_metadata(path).map_err(invalid)?;
        if !meta.file_type().is_file() || meta.len() > MAX_BYTES as u64 {
            return Err(invalid(
                "expected a regular repository file smaller than 1 MiB",
            ));
        }
        let mut bytes = Vec::new();
        fs::File::open(path)
            .map_err(invalid)?
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(invalid)?;
        if bytes.len() > MAX_BYTES {
            return Err(invalid("repository file exceeds 1 MiB"));
        }
        Ok(bytes)
    }
}
fn value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .filter_map(|line| line.split_once(['=', ':']))
        .find_map(|(name, val)| (name.trim().eq_ignore_ascii_case(key)).then_some(val.trim()))
}
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
fn safe_repo_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-:".contains(&byte))
}
fn safe_repo_key(source: &str) -> bool {
    if crate::artifact::https_source(source) {
        return true;
    }
    source
        .strip_prefix("file:///etc/pki/rpm-gpg/")
        .is_some_and(|name| {
            let name = name
                .replace("$releasever", "release")
                .replace("$basearch", "arch")
                .replace("$arch", "arch");
            !name.is_empty()
                && !name.starts_with('.')
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
}
fn unique_keys(text: &str, keys: &[&str]) -> bool {
    keys.iter().all(|key| {
        text.lines()
            .filter_map(|line| line.split_once(['=', ':']))
            .filter(|(name, _)| name.trim().eq_ignore_ascii_case(key))
            .count()
            <= 1
    })
}
fn validate_repo_section(name: &str, section: &str) -> Result<(), EngineError> {
    let locations: Vec<_> = ["baseurl", "metalink", "mirrorlist"]
        .into_iter()
        .filter_map(|key| value(section, key))
        .collect();
    if !unique_keys(
        section,
        &["baseurl", "gpgcheck", "gpgkey", "mirrorlist", "metalink"],
    ) || locations.len() != 1
        || !safe_repo_name(name)
        || !crate::artifact::https_source(locations[0])
        || value(section, "gpgcheck") != Some("1")
        || value(section, "gpgkey")
            .is_some_and(|keys| keys.is_empty() || !keys.split_whitespace().all(safe_repo_key))
    {
        return Err(invalid(
            "every repository needs one HTTPS source and enabled GPG checks",
        ));
    }
    Ok(())
}
fn validate(
    suffix: &str,
    text: &str,
    host: &Host,
) -> Result<(String, String, String), EngineError> {
    if text.is_empty() || text.contains('\0') {
        return Err(invalid("empty or invalid repository file"));
    }
    match suffix {
        "flatpakrepo" => {
            if !text.lines().any(|line| line.trim() == "[Flatpak Repo]") {
                return Err(invalid("invalid Flatpak repository definition"));
            }
            let name =
                value(text, "Name").ok_or_else(|| invalid("Flatpak repository lacks a name"))?;
            let url =
                value(text, "Url").ok_or_else(|| invalid("Flatpak repository lacks a URL"))?;
            if !unique_keys(text, &["Name", "Url", "GPGKey"])
                || !safe_name(name)
                || !crate::artifact::https_source(url)
                || value(text, "GPGKey").is_none_or(str::is_empty)
            {
                return Err(invalid(
                    "Flatpak repository needs an HTTPS URL and signing key",
                ));
            }
            Ok(("flatpak".into(), name.into(), url.into()))
        }
        "repo" => {
            let sections: Vec<_> = text
                .split('\n')
                .enumerate()
                .filter_map(|(index, line)| {
                    line.trim()
                        .strip_prefix('[')
                        .and_then(|line| line.strip_suffix(']'))
                        .map(|name| (index, name))
                })
                .collect();
            let name = sections
                .first()
                .map(|(_, name)| *name)
                .ok_or_else(|| invalid("repository lacks an identifier"))?;
            for (position, (start, section_name)) in sections.iter().enumerate() {
                let end = sections
                    .get(position + 1)
                    .map_or(text.lines().count(), |(line, _)| *line);
                let section = text
                    .lines()
                    .skip(start + 1)
                    .take(end - start - 1)
                    .collect::<Vec<_>>()
                    .join("\n");
                validate_repo_section(section_name, &section)?;
            }
            let url = ["baseurl", "metalink", "mirrorlist"]
                .into_iter()
                .find_map(|key| value(text, key))
                .unwrap_or("");
            let backend = if host.resolve("dnf")?.is_some() {
                "dnf"
            } else if host.resolve("zypper")?.is_some() {
                "zypper"
            } else {
                return Err(invalid("DNF or Zypper is required for .repo files"));
            };
            Ok((backend.into(), name.into(), url.into()))
        }
        "sources" => {
            let uris = value(text, "URIs").ok_or_else(|| invalid("APT source lacks URIs"))?;
            if !text
                .split("\n\n")
                .filter(|stanza| !stanza.trim().is_empty())
                .all(|stanza| {
                    unique_keys(stanza, &["Types", "URIs", "Signed-By", "Trusted"])
                        && value(stanza, "Types").is_some()
                        && value(stanza, "URIs").is_some_and(|uris| {
                            uris.split_whitespace().all(crate::artifact::https_source)
                        })
                        && value(stanza, "Signed-By").is_some()
                        && !value(stanza, "Trusted")
                            .is_some_and(|value| value.eq_ignore_ascii_case("yes"))
                })
            {
                return Err(invalid("APT source needs HTTPS URIs and Signed-By"));
            }
            Ok(("apt".into(), "APT source".into(), uris.into()))
        }
        "list" => {
            let lines: Vec<_> = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .collect();
            if lines.is_empty()
                || lines.iter().any(|line| {
                    let lower = line.to_ascii_lowercase();
                    !line.starts_with("deb ") && !line.starts_with("deb-src ")
                        || !lower.contains("signed-by=")
                        || lower.contains("trusted=yes")
                        || lower.contains("allow-insecure=yes")
                        || !line.split_whitespace().any(crate::artifact::https_source)
                })
            {
                return Err(invalid("APT source lines need HTTPS and signed-by"));
            }
            Ok(("apt".into(), "APT source".into(), lines.join("\n")))
        }
        "ymp" => {
            if !text.contains("<metapackage")
                || text.contains("<!DOCTYPE")
                || text.contains("<!ENTITY")
            {
                return Err(invalid("invalid openSUSE One Click file"));
            }
            Ok((
                "zypper".into(),
                "One Click install".into(),
                "Opens in the native openSUSE installer".into(),
            ))
        }
        _ => Err(invalid("unsupported repository format")),
    }
}
pub fn inspect(source: &str, cancel: &Cancellation) -> Result<Import, EngineError> {
    inspect_with_host(source, cancel, &Host::current())
}
fn inspect_with_host(
    source: &str,
    cancel: &Cancellation,
    host: &Host,
) -> Result<Import, EngineError> {
    if !supported(source) {
        return Err(invalid("unsupported repository format"));
    }
    let suffix = source
        .split(['?', '#'])
        .next()
        .unwrap_or(source)
        .rsplit('.')
        .next()
        .unwrap_or("");
    let bytes = source_bytes(source, cancel, host)?;
    let text = std::str::from_utf8(&bytes).map_err(invalid)?;
    let (backend, name, description) = validate(suffix, text, host)?;
    Ok(Import {
        source: source.into(),
        backend,
        suffix: suffix.into(),
        name,
        description,
        digest: format!("{:x}", Sha256::digest(&bytes)),
    })
}
struct Staged(PathBuf);
impl Drop for Staged {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
pub fn apply(
    import: &Import,
    authorization: Authorization,
    cancel: &Cancellation,
) -> Result<(), EngineError> {
    apply_with_host(import, authorization, cancel, &Host::current())
}
fn apply_with_host(
    import: &Import,
    authorization: Authorization,
    cancel: &Cancellation,
    host: &Host,
) -> Result<(), EngineError> {
    let fresh = inspect_with_host(&import.source, cancel, host)?;
    if fresh.digest != import.digest || fresh.backend != import.backend || fresh.name != import.name
    {
        return Err(invalid("repository file changed since preview"));
    }
    let bytes = source_bytes(&import.source, cancel, host)?;
    if format!("{:x}", Sha256::digest(&bytes)) != import.digest {
        return Err(invalid("repository file changed during staging"));
    }
    let path = std::env::temp_dir().join(format!(
        "pkgdeck-repo-{}-{}.{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        import.suffix
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(invalid)?;
    use std::io::Write;
    file.write_all(&bytes).map_err(invalid)?;
    file.sync_all().map_err(invalid)?;
    drop(file);
    let staged = Staged(path);
    let result = match import.suffix.as_str() {
        "flatpakrepo" => host.flatpak(
            &[
                "--user".into(),
                "--noninteractive".into(),
                "remote-add".into(),
                "--from".into(),
                import.name.clone().into(),
                staged.0.as_os_str().to_os_string(),
            ],
            cancel,
            true,
            false,
            authorization,
        )?,
        "ymp" => host.one_click(&staged.0, cancel)?,
        _ => host.install_repository_file(
            &staged.0,
            &import.backend,
            &import.suffix,
            &import.digest,
            authorization,
            cancel,
        )?,
    };
    if result.code != Some(0) {
        return Err(crate::process::ExecutionError::Failed(result).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Runtime;
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    fn validate(suffix: &str, text: &str) -> Result<(String, String, String), EngineError> {
        super::validate(suffix, text, &Host::current())
    }
    #[test]
    fn requires_signed_apt_sources() {
        assert!(validate("sources", "Types: deb\nURIs: https://example.invalid/repo\nSuites: stable\nComponents: main\nSigned-By: /etc/apt/keyrings/example.gpg\n").is_ok());
        assert!(validate(
            "sources",
            "Types: deb\nURIs: http://example.invalid/repo\nSuites: stable\nComponents: main\n"
        )
        .is_err());
        assert!(validate(
            "list",
            "deb [trusted=yes] https://example.invalid stable main"
        )
        .is_err());
    }
    #[test]
    fn rejects_unsafe_flatpak_repository() {
        assert!(validate(
            "flatpakrepo",
            "[Flatpak Repo]\nName=flathub\nUrl=https://example.invalid/repo\nGPGKey=c3ludGhldGlj\n"
        )
        .is_ok());
        assert!(validate(
            "flatpakrepo",
            "[Flatpak Repo]\nName=bad;name\nUrl=https://example.invalid/repo\n"
        )
        .is_err());
        assert!(validate(
            "flatpakrepo",
            "[Flatpak Repo]\nName=unsigned\nUrl=https://example.invalid/repo\n"
        )
        .is_err());
        assert!(validate("flatpakrepo", "[Flatpak Repo]\nName=signed\nUrl=https://example.invalid/repo\nUrl=http://example.invalid/repo\nGPGKey=c3ludGhldGlj\n").is_err());
    }
    #[test]
    fn rejects_second_unsigned_repository() {
        assert!(validate_repo_section(
            "signed",
            "baseurl=https://example.invalid/signed\ngpgcheck=1"
        )
        .is_ok());
        assert!(validate_repo_section(
            "unsigned",
            "baseurl=https://example.invalid/unsigned\ngpgcheck=0"
        )
        .is_err());
        assert!(validate_repo_section("override", "baseurl=https://example.invalid/signed\nbaseurl=http://example.invalid/unsigned\ngpgcheck=1").is_err());
        assert!(validate_repo_section(
            "vendor:stable",
            "baseurl=https://example.invalid/repo\ngpgcheck=1\ngpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-synthetic"
        ).is_ok());
        assert!(validate_repo_section(
            "fedora:stable",
            "baseurl=https://example.invalid/repo\ngpgcheck=1\ngpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-fedora-$releasever-$basearch"
        ).is_ok());
        assert!(validate_repo_section(
            "fedora:stable",
            "metalink=https://example.invalid/metalink?repo=fedora-$releasever&arch=$basearch\ngpgcheck=1\ngpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-fedora-$releasever-$basearch"
        ).is_ok());
        assert!(validate_repo_section(
            "fedora:stable",
            "baseurl=https://example.invalid/repo\nmetalink=https://example.invalid/metalink\ngpgcheck=1"
        ).is_err());
        assert!(validate_repo_section(
            "vendor:stable",
            "baseurl=https://example.invalid/repo\ngpgcheck=1\ngpgkey=file:///tmp/untrusted-key"
        )
        .is_err());
        assert!(validate_repo_section(
            "vendor:stable",
            "baseurl=https://example.invalid/repo\ngpgcheck=1\ngpgkey=file:///etc/pki/rpm-gpg/../untrusted-key"
        ).is_err());
        assert!(validate_repo_section(
            "vendor:stable",
            "baseurl=https://example.invalid/repo\ngpgcheck=1\ngpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-$unknown"
        ).is_err());
    }
    #[test]
    fn previews_repository_formats_and_rejects_changed_files() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-repository-input-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let dnf = base.join("dnf");
        fs::write(&dnf, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&dnf, fs::Permissions::from_mode(0o755)).unwrap();
        let mut env = BTreeMap::new();
        env.insert(OsString::from("PATH"), base.as_os_str().to_os_string());
        let host = Host::new(Runtime::Native, env);
        let cancel = Cancellation::default();
        for (file, content, backend) in [
            ("synthetic.repo", "[synthetic]\nname=Synthetic\nbaseurl=https://example.invalid/repo\ngpgcheck=1\ngpgkey=https://example.invalid/key\n", "dnf"),
            ("synthetic.sources", "Types: deb\nURIs: https://example.invalid/repo\nSuites: stable\nComponents: main\nSigned-By: /etc/apt/keyrings/synthetic.gpg\n", "apt"),
            ("synthetic.list", "deb [signed-by=/etc/apt/keyrings/synthetic.gpg] https://example.invalid/repo stable main\n", "apt"),
            ("synthetic.flatpakrepo", "[Flatpak Repo]\nName=synthetic\nUrl=https://example.invalid/repo\nGPGKey=c3ludGhldGlj\n", "flatpak"),
            ("synthetic.ymp", "<metapackage><group/></metapackage>\n", "zypper"),
        ] {
            let path = base.join(file);
            fs::write(&path, content).unwrap();
            let imported = inspect_with_host(path.to_str().unwrap(), &cancel, &host).unwrap();
            assert_eq!(imported.backend, backend);
            assert!(imported.description.len() > 3);
            fs::write(&path, "changed after preview\n").unwrap();
            assert!(apply_with_host(&imported, Authorization::Polkit, &cancel, &host).is_err());
        }
        let alias = base.join("alias.sources");
        std::os::unix::fs::symlink(base.join("synthetic.sources"), &alias).unwrap();
        assert!(inspect_with_host(alias.to_str().unwrap(), &cancel, &host).is_err());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn previews_https_repository_without_shell_or_unbounded_output() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-repository-https-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let content = base.join("content");
        fs::write(
            &content,
            "[Flatpak Repo]\nName=remote\nUrl=https://example.invalid/repo\nGPGKey=c3ludGhldGlj\n",
        )
        .unwrap();
        let curl = base.join("curl");
        let pending_curl = base.join("curl.pending");
        fs::write(
            &pending_curl,
            format!("#!/bin/sh\n/bin/cat '{}'\n", content.display()),
        )
        .unwrap();
        // Publish the executable after closing its writer so parallel tests
        // never try to run a script while its inode is still being written.
        fs::set_permissions(&pending_curl, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&pending_curl, &curl).unwrap();
        let mut env = BTreeMap::new();
        env.insert(OsString::from("PATH"), base.as_os_str().to_os_string());
        let host = Host::new(Runtime::Native, env);
        let cancel = Cancellation::default();
        let imported =
            inspect_with_host("https://example.invalid/remote.flatpakrepo", &cancel, &host)
                .unwrap();
        assert_eq!(imported.name, "remote");
        fs::write(
            &content,
            "[Flatpak Repo]\nName=other\nUrl=https://example.invalid/repo\nGPGKey=c3ludGhldGlj\n",
        )
        .unwrap();
        assert!(apply_with_host(&imported, Authorization::Polkit, &cancel, &host).is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn adds_only_the_reviewed_flatpak_repository_bytes() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-flatpak-repository-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let source = base.join("synthetic.flatpakrepo");
        let contents = "[Flatpak Repo]\nName=synthetic\nUrl=https://example.invalid/repo\nGPGKey=c3ludGhldGlj\n";
        fs::write(&source, contents).unwrap();
        let record = base.join("record");
        let flatpak = base.join("flatpak");
        fs::write(
            &flatpak,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nfor arg; do :; done\n/bin/cat \"$arg\" >> '{}'\n",
                record.display(),
                record.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&flatpak, fs::Permissions::from_mode(0o755)).unwrap();
        let mut env = BTreeMap::new();
        env.insert(OsString::from("PATH"), base.as_os_str().to_os_string());
        let host = Host::new(Runtime::Native, env);
        let cancel = Cancellation::default();
        let import = inspect_with_host(source.to_str().unwrap(), &cancel, &host).unwrap();
        apply_with_host(&import, Authorization::Polkit, &cancel, &host).unwrap();
        let output = fs::read_to_string(record).unwrap();
        assert!(output.starts_with("--user\n--noninteractive\nremote-add\n--from\nsynthetic\n"));
        assert!(output.ends_with(contents));
        let staged = Path::new(output.lines().nth(5).unwrap());
        assert_ne!(staged, source);
        assert!(!staged.exists());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn rejects_unsafe_repository_files_before_preview() {
        let base = std::env::temp_dir().join(format!(
            "pkgdeck-repository-rejections-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let host = Host::current();
        let cancel = Cancellation::default();
        assert!(!supported("https://example.invalid/source.txt"));
        assert!(source_bytes("relative.sources", &cancel, &host).is_err());
        assert!(source_bytes(
            "https://user@example.invalid/synthetic.repo",
            &cancel,
            &host
        )
        .is_err());
        let source = base.join("synthetic.sources");
        fs::write(&source, "Types: deb\nURIs: https://example.invalid/repo\nSigned-By: /etc/apt/keyrings/example.gpg\n").unwrap();
        let alias = base.join("alias.sources");
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        assert!(source_bytes(alias.to_str().unwrap(), &cancel, &host).is_err());
        fs::write(&source, vec![b'a'; MAX_BYTES + 1]).unwrap();
        assert!(source_bytes(source.to_str().unwrap(), &cancel, &host).is_err());
        assert!(validate("flatpakrepo", "").is_err());
        assert!(validate("flatpakrepo", "[Flatpak Repo]\nName=synthetic\nUrl=https://example.invalid/repo\nGPGKey=key\nGPGKey=other\n").is_err());
        assert!(validate(
            "repo",
            "[bad]\nbaseurl=http://example.invalid/repo\ngpgcheck=1\n"
        )
        .is_err());
        assert!(validate("sources", "Types: deb\nURIs: https://example.invalid/repo\nSigned-By: /etc/apt/keyrings/example.gpg\n\nTypes: deb\nURIs: http://example.invalid/unsafe\nSigned-By: /etc/apt/keyrings/example.gpg\n").is_err());
        assert!(validate(
            "list",
            "deb [trusted=yes] https://example.invalid/repo stable main"
        )
        .is_err());
        assert!(validate("ymp", "<!DOCTYPE metapackage><metapackage/>").is_err());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn one_click_file_requires_the_native_opensuse_installer() {
        if Path::new("/usr/sbin/OneClickInstallUI").exists()
            || Path::new("/usr/bin/OneClickInstallUI").exists()
        {
            return;
        }
        let source = std::env::temp_dir().join(format!(
            "pkgdeck-one-click-input-{}.ymp",
            std::process::id()
        ));
        fs::write(&source, "<metapackage><group/></metapackage>\n").unwrap();
        let host = Host::current();
        let cancel = Cancellation::default();
        let import = inspect_with_host(source.to_str().unwrap(), &cancel, &host).unwrap();
        assert!(apply_with_host(&import, Authorization::Polkit, &cancel, &host).is_err());
        assert!(source.exists());
        fs::remove_file(source).unwrap();
    }
    #[test]
    fn repo_preview_selects_zypper_and_rejects_missing_or_failed_tools() {
        let base =
            std::env::temp_dir().join(format!("pkgdeck-repo-manager-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let source = base.join("synthetic.repo");
        fs::write(
            &source,
            "[synthetic]\nbaseurl=https://example.invalid/repo\ngpgcheck=1\n",
        )
        .unwrap();
        let mut env = BTreeMap::new();
        env.insert(OsString::from("PATH"), base.as_os_str().to_os_string());
        let host = Host::new(Runtime::Native, env.clone());
        let cancel = Cancellation::default();
        assert!(inspect_with_host(source.to_str().unwrap(), &cancel, &host).is_err());
        let zypper = base.join("zypper");
        fs::write(&zypper, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&zypper, fs::Permissions::from_mode(0o755)).unwrap();
        let import = inspect_with_host(source.to_str().unwrap(), &cancel, &host).unwrap();
        assert_eq!(import.backend, "zypper");
        fs::write(
            &source,
            "[synthetic]\nmetalink=https://example.invalid/metalink\ngpgcheck=1\n",
        )
        .unwrap();
        let import = inspect_with_host(source.to_str().unwrap(), &cancel, &host).unwrap();
        assert_eq!(import.description, "https://example.invalid/metalink");
        assert!(super::validate("unknown", "synthetic", &host).is_err());
        assert!(
            inspect_with_host(base.join("unknown.txt").to_str().unwrap(), &cancel, &host).is_err()
        );
        let curl = base.join("curl");
        fs::write(&curl, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            inspect_with_host("https://example.invalid/synthetic.repo", &cancel, &host).is_err()
        );
        fs::remove_dir_all(base).unwrap();
    }
}
