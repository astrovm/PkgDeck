//! Whether what an opened file or link installs is installed already, and
//! at which version, asked of the manager that would install it.
use crate::{
    host::Host,
    package::{Package, PackageId, Scope},
    process::{Cancellation, Limits},
};
use std::{cmp::Ordering, ffi::OsString, time::Duration};

/// The opened page's button once `package`'s installed copy is known:
/// Install, or Update for a newer one. Flatpak replaces any copy, so it also
/// offers Reinstall and Downgrade; other managers only install newer files,
/// so an installed one has nothing to press.
pub fn action(package: &Package) -> &'static str {
    let Some(installed) = package.installed_version.as_deref() else {
        return "Install";
    };
    let order = package
        .candidate_version
        .as_deref()
        .filter(|candidate| !candidate.is_empty() && !installed.is_empty())
        .map(|candidate| compare(candidate, installed));
    match (order, package.id.backend == "flatpak") {
        (Some(Ordering::Greater), _) => "Update",
        (Some(Ordering::Less), true) => "Downgrade",
        (_, true) => "Reinstall",
        _ => "",
    }
}

/// Versions as package managers write them: an optional `epoch:` first,
/// then dot and dash separated numbers and words.
pub fn compare(left: &str, right: &str) -> Ordering {
    fn epoch(version: &str) -> (u64, &str) {
        version
            .split_once(':')
            .and_then(|(epoch, rest)| Some((epoch.parse().ok()?, rest)))
            .unwrap_or((0, version))
    }
    let ((left_epoch, left), (right_epoch, right)) = (epoch(left), epoch(right));
    left_epoch
        .cmp(&right_epoch)
        .then_with(|| crate::backends::compare_versions(left, right))
}

/// The installed version of `id`'s package, `Some("")` when it is installed
/// at no known version, and `None` when it isn't installed or the manager
/// can't say.
pub fn version(id: &PackageId, cancel: &Cancellation) -> Option<String> {
    version_with_host(id, cancel, &Host::current())
}

fn version_with_host(id: &PackageId, cancel: &Cancellation, host: &Host) -> Option<String> {
    let name = id.name.as_str();
    if name.is_empty() || name.starts_with('-') {
        return None;
    }
    let read = |tool: &str, args: &[&str]| -> Option<String> {
        let path = host.resolve(tool).ok()??;
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let limits = Limits {
            timeout: Duration::from_secs(10),
            output_bytes: 4 * 1024 * 1024,
        };
        let result = host.read(&path, &args, limits, cancel).ok()?;
        (result.code == Some(0) && !result.truncated)
            .then(|| String::from_utf8(result.stdout).ok())
            .flatten()
    };
    let version = match id.backend.as_str() {
        "apt" => read(
            "dpkg-query",
            &["-W", "-f", "${db:Status-Status} ${Version}\n", "--", name],
        )?
        .lines()
        .find_map(|line| line.strip_prefix("installed "))
        .map(str::to_owned),
        "dnf" | "zypper" => read(
            "rpm",
            &["-q", "--queryformat", "%{VERSION}-%{RELEASE}\n", "--", name],
        )?
        .lines()
        .next()
        .map(str::to_owned),
        "pacman" => {
            let text = read("pacman", &["-Q", "--", name])?;
            let mut fields = text.lines().next()?.split_whitespace();
            (fields.next() == Some(name))
                .then(|| fields.next().map(str::to_owned))
                .flatten()
        }
        "snap" => {
            let text = read("snap", &["list", "--", name])?;
            let mut fields = text.lines().nth(1)?.split_whitespace();
            (fields.next() == Some(name))
                .then(|| fields.next().map(str::to_owned))
                .flatten()
        }
        "flatpak" => {
            let scope = match id.scope {
                Scope::System => "--system",
                Scope::User { .. } => "--user",
                _ => return None,
            };
            let text = read("flatpak", &[scope, "list", "--columns=application,version"])?;
            text.lines().find_map(|line| {
                let (app, version) = line.split_once('\t').unwrap_or((line, ""));
                (app == name).then(|| version.trim().to_owned())
            })
        }
        _ => None,
    }?;
    Some(version.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Runtime;
    use crate::package::UpdateAvailability;

    fn package(backend: &str, installed: Option<&str>, candidate: Option<&str>) -> Package {
        Package {
            id: id(backend, "tool", Scope::System),
            display_name: "tool".into(),
            summary: String::new(),
            installed_version: installed.map(str::to_owned),
            candidate_version: candidate.map(str::to_owned),
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        }
    }

    #[test]
    fn an_installed_copy_turns_install_into_update_or_reinstall() {
        for (backend, installed, candidate, expected) in [
            ("apt", None, Some("1.0"), "Install"),
            ("flatpak", None, None, "Install"),
            ("apt", Some("1.0-1"), Some("1.1-1"), "Update"),
            ("apt", Some("1.0-1"), Some("1.0-1"), ""),
            ("apt", Some("1.1-1"), Some("1.0-1"), ""),
            ("snap", Some(""), Some("2"), ""),
            ("dnf", Some("1"), None, ""),
            ("flatpak", Some("0.3.2"), Some("0.4.1"), "Update"),
            ("flatpak", Some("0.4.1"), Some("0.4.1"), "Reinstall"),
            ("flatpak", Some("0.4.1"), Some("0.3.2"), "Downgrade"),
            ("flatpak", Some("0.4.1"), None, "Reinstall"),
            ("flatpak", Some(""), Some("1"), "Reinstall"),
        ] {
            assert_eq!(
                action(&package(backend, installed, candidate)),
                expected,
                "{backend} {installed:?} {candidate:?}"
            );
        }
    }

    #[test]
    fn versions_compare_by_epoch_then_parts() {
        for (left, right, expected) in [
            ("1.10", "1.9", Ordering::Greater),
            ("0.4.1", "0.4.1", Ordering::Equal),
            ("1.0", "1.0.0", Ordering::Equal),
            ("1:1.0", "2.0", Ordering::Greater),
            ("1.0-2", "1.0-10", Ordering::Less),
            ("1.0rc1", "1.0", Ordering::Less),
            ("x:1.0", "1.0", Ordering::Less),
        ] {
            assert_eq!(compare(left, right), expected, "{left} {right}");
            assert_eq!(compare(right, left), expected.reverse(), "{right} {left}");
        }
    }
    use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::Path};

    fn id(backend: &str, name: &str, scope: Scope) -> PackageId {
        PackageId {
            backend: backend.into(),
            name: name.into(),
            architecture: "x86_64".into(),
            scope,
            remote: None,
            reference: None,
        }
    }

    /// A host whose only tool is `name`, printing `output` and exiting
    /// `code`, and which logs its arguments to `args`.
    fn host(base: &Path, name: &str, output: &str, code: i32) -> Host {
        let _ = fs::remove_dir_all(base);
        fs::create_dir_all(base).unwrap();
        let tool = base.join(name);
        fs::write(
            &tool,
            format!(
                "#!/bin/sh\nfor arg; do printf '%s\\n' \"$arg\"; done > \"${{0%/*}}/args\"\n\
                 printf '%s' '{output}'\nexit {code}\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let mut env = BTreeMap::new();
        env.insert(OsString::from("PATH"), base.as_os_str().to_os_string());
        Host::new(Runtime::Native, env)
    }

    #[test]
    fn each_manager_says_what_it_has_installed() {
        let base = std::env::temp_dir().join(format!("pkgdeck-installed-{}", std::process::id()));
        let cancel = Cancellation::default();
        let user = Scope::User { uid: 1000 };
        for (backend, tool, output, scope, expected, args) in [
            (
                "apt",
                "dpkg-query",
                "config-files 0.9\ninstalled 1.2-3\n",
                Scope::System,
                Some("1.2-3"),
                "-W|-f|${db:Status-Status} ${Version}\n|--|tool",
            ),
            (
                "apt",
                "dpkg-query",
                "config-files 0.9\n",
                Scope::System,
                None,
                "",
            ),
            (
                "dnf",
                "rpm",
                "1.2-3.fc41\n",
                Scope::System,
                Some("1.2-3.fc41"),
                "-q|--queryformat|%{VERSION}-%{RELEASE}\n|--|tool",
            ),
            ("zypper", "rpm", "4-1\n", Scope::System, Some("4-1"), ""),
            (
                "pacman",
                "pacman",
                "tool 1.2-3\n",
                Scope::System,
                Some("1.2-3"),
                "-Q|--|tool",
            ),
            ("pacman", "pacman", "other 1.2-3\n", Scope::System, None, ""),
            (
                "snap",
                "snap",
                "Name  Version  Rev\ntool  158.0-2  9057\n",
                Scope::System,
                Some("158.0-2"),
                "list|--|tool",
            ),
            (
                "snap",
                "snap",
                "Name  Version  Rev\n",
                Scope::System,
                None,
                "",
            ),
            (
                "flatpak",
                "flatpak",
                "org.example.Other\t1\ntool\t0.4.1\n",
                user.clone(),
                Some("0.4.1"),
                "--user|list|--columns=application,version",
            ),
            (
                "flatpak",
                "flatpak",
                "tool\t\n",
                Scope::System,
                Some(""),
                "--system|list|--columns=application,version",
            ),
            ("flatpak", "flatpak", "tool\n", user.clone(), Some(""), ""),
            ("flatpak", "flatpak", "other\t1\n", user.clone(), None, ""),
        ] {
            let host = host(&base, tool, output, 0);
            let found = version_with_host(&id(backend, "tool", scope), &cancel, &host);
            assert_eq!(found.as_deref(), expected, "{backend} {output:?}");
            if !args.is_empty() {
                let logged = fs::read_to_string(base.join("args")).unwrap();
                assert_eq!(
                    logged.trim_end_matches('\n'),
                    args.replace('|', "\n"),
                    "{backend}"
                );
            }
        }
        // Not installed, a failing or missing tool, or a name that reads as
        // an option: not installed as far as anyone can tell.
        let host = host(&base, "dpkg-query", "", 1);
        assert_eq!(
            version_with_host(&id("apt", "tool", Scope::System), &cancel, &host),
            None
        );
        for (backend, name, scope) in [
            ("rpm-less", "tool", Scope::System),
            ("dnf", "tool", Scope::System),
            ("apt", "--help", Scope::System),
            ("apt", "", Scope::System),
            ("appimage", "tool", Scope::System),
            (
                "flatpak",
                "tool",
                Scope::Environment {
                    path: "/env".into(),
                },
            ),
        ] {
            assert_eq!(
                version_with_host(&id(backend, name, scope), &cancel, &host),
                None
            );
        }
        fs::remove_dir_all(&base).unwrap();
    }
}
