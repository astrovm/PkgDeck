//! Native host APT queries for sandboxed builds, using host packaging tools.
//!
//! When PkgDeck itself runs as a Flatpak it cannot use the bundled libapt
//! helper against the sandbox; instead it drives the host's `dpkg-query` and
//! `apt-cache` through the host bridge and assembles the same records the
//! native helper reports. Only read-only commands run, and every parse is a
//! pure function covered by fixture tests below. Spawning stays in the
//! transport glue, which passes command output into this module.
use crate::package::*;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// One installed-package row from `dpkg-query -W`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DpkgRow {
    pub name: String,
    pub arch: String,
    pub version: String,
    pub held: bool,
}

/// Parse `dpkg-query -W -f='${Package}\t${Architecture}\t${Version}\t${Status}\n'`.
/// Keeps rows whose status ends in `installed` with a want of `install`/`hold`.
pub(crate) fn parse_dpkg_table(text: &str) -> Vec<DpkgRow> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let mut fields = line.split('\t');
        let (Some(name), Some(arch), Some(version), Some(status)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if fields.next().is_some() || name.is_empty() || arch.is_empty() || version.is_empty() {
            continue;
        }
        let mut status = status.split_whitespace();
        let (Some(want), Some(state)) = (status.next(), status.last()) else {
            continue;
        };
        if state != "installed" || (want != "install" && want != "hold") {
            continue;
        }
        // dpkg may qualify a foreign-architecture Package field; the
        // architecture is represented separately in PackageId.
        let name = name.strip_suffix(&format!(":{arch}")).unwrap_or(name);
        rows.push(DpkgRow {
            name: name.into(),
            arch: arch.into(),
            version: version.into(),
            held: want == "hold",
        });
    }
    rows
}

/// Candidate state from `apt-cache policy` for one package stanza.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PolicyEntry {
    pub installed: Option<String>,
    pub candidate: Option<String>,
    /// Versions whose table row carries a `(phased N%)` marker below 100.
    pub phased: BTreeSet<String>,
}

/// Parse `apt-cache policy` output keyed by `(name, stanza architecture)`; a
/// `None` architecture means the stanza applied to the native architecture.
pub(crate) fn parse_policy_dump(text: &str) -> BTreeMap<(String, Option<String>), PolicyEntry> {
    let mut map = BTreeMap::new();
    let mut current: Option<(String, Option<String>)> = None;
    for line in text.lines() {
        if line.starts_with(char::is_whitespace) {
            let Some(key) = current.clone() else { continue };
            let entry: &mut PolicyEntry = map.entry(key).or_default();
            let trimmed = line.trim_start();
            if let Some(version) = trimmed.strip_prefix("Installed:").map(str::trim) {
                if version != "(none)" {
                    entry.installed = Some(version.into());
                }
            } else if let Some(version) = trimmed.strip_prefix("Candidate:").map(str::trim) {
                if version != "(none)" {
                    entry.candidate = Some(version.into());
                }
            } else {
                let row = trimmed.strip_prefix("*** ").unwrap_or(trimmed);
                let mut parts = row.split_whitespace();
                if let Some(version) = parts.next() {
                    let rest: Vec<_> = parts.collect();
                    if let Some(index) = rest.iter().position(|part| part.starts_with("(phased")) {
                        // APT prints `(phased 30%)` as two whitespace-separated
                        // fields; inspecting only the first marks even 100%
                        // candidates as phased.
                        let percent: String = rest[index..]
                            .iter()
                            .take(2)
                            .flat_map(|part| part.chars())
                            .filter(|char| char.is_ascii_digit())
                            .collect();
                        // An unreadable marker fails closed: the candidate is
                        // treated as phased rather than offered early.
                        if percent.is_empty()
                            || percent.parse::<u32>().is_ok_and(|percent| percent < 100)
                        {
                            entry.phased.insert(version.into());
                        }
                    }
                }
            }
            continue;
        }
        current = None;
        if let Some(header) = line.strip_suffix(':') {
            if header.is_empty() || header.contains(char::is_whitespace) {
                continue;
            }
            let (name, arch) = match header.split_once(':') {
                Some((name, arch)) if !name.is_empty() && !arch.is_empty() => {
                    (name, Some(arch.into()))
                }
                _ if !header.contains(':') => (header, None),
                _ => continue,
            };
            current = Some((name.into(), arch));
        }
    }
    map
}

/// One version record from `apt-cache show`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ShowRecord {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub summary: String,
    pub description: String,
    pub homepage: Option<String>,
    pub depends: Vec<String>,
    pub phased_percent: Option<u32>,
}

/// Parse `apt-cache show` output in stanza order (newest records first).
pub(crate) fn parse_show_dump(text: &str) -> Vec<ShowRecord> {
    let mut records = Vec::new();
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;
    let mut flush = |fields: &mut Vec<(String, String)>| {
        if fields.is_empty() {
            return;
        }
        let get = |name: &str| {
            fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let name = get("Package").unwrap_or_default();
        let version = get("Version").unwrap_or_default();
        if name.is_empty() || version.is_empty() {
            fields.clear();
            return;
        }
        let summary = get("Description-en")
            .or_else(|| get("Description"))
            .map(|text| text.lines().next().unwrap_or("").into())
            .unwrap_or_default();
        let description = get("Description-en")
            .or_else(|| get("Description"))
            .unwrap_or_default();
        let homepage = get("Homepage").filter(|value| !value.trim().is_empty());
        let depends = get("Depends")
            .filter(|value| !value.is_empty())
            .into_iter()
            .collect();
        let phased_percent =
            get("Phased-Update-Percentage").and_then(|value| value.trim().parse().ok());
        records.push(ShowRecord {
            name,
            version,
            arch: get("Architecture").unwrap_or_default(),
            summary,
            description,
            homepage,
            depends,
            phased_percent,
        });
        fields.clear();
    };
    for line in text.lines() {
        if line.is_empty() {
            if let Some(field) = current.take() {
                fields.push(field);
            }
            flush(&mut fields);
            continue;
        }
        if line.starts_with(' ') {
            if let Some((_, value)) = current.as_mut() {
                let unfolded = line.strip_prefix(' ').unwrap_or(line);
                if unfolded == "." {
                    value.push('\n');
                } else {
                    value.push('\n');
                    value.push_str(unfolded);
                }
            }
            continue;
        }
        if let Some(field) = current.take() {
            fields.push(field);
        }
        match line.split_once(':') {
            Some((key, value)) if !key.contains(char::is_whitespace) => {
                current = Some((key.into(), value.trim_start().into()))
            }
            _ => current = None,
        }
    }
    if let Some(field) = current.take() {
        fields.push(field);
    }
    flush(&mut fields);
    records
}

/// First record for a package and version, preferring an architecture match.
pub(crate) fn select_record<'a>(
    records: &'a [ShowRecord],
    name: &str,
    version: &str,
    arch: &str,
) -> Option<&'a ShowRecord> {
    records
        .iter()
        .find(|record| record.name == name && record.version == version && record.arch == arch)
        .or_else(|| {
            records
                .iter()
                .find(|record| record.name == name && record.version == version)
        })
}

fn order_value(byte: Option<u8>) -> i32 {
    match byte {
        None => 0,
        Some(b'~') => -1,
        Some(char) if char.is_ascii_digit() => 0,
        Some(char) if char.is_ascii_alphabetic() => i32::from(char),
        Some(char) => i32::from(char) + 256,
    }
}

/// Debian package version comparison: optional epoch, upstream part, then an
/// optional revision, each compared with tilde-aware ordering.
pub(crate) fn deb_cmp(left: &str, right: &str) -> Ordering {
    fn split_epoch(version: &str) -> (u64, &str) {
        match version.split_once(':') {
            Some((epoch, rest))
                if !epoch.is_empty() && epoch.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                (epoch.parse().unwrap_or(0), rest)
            }
            _ => (0, version),
        }
    }
    fn verrevcmp(mut left: &[u8], mut right: &[u8]) -> Ordering {
        while !left.is_empty() || !right.is_empty() {
            while left.first().is_some_and(|byte| !byte.is_ascii_digit())
                || right.first().is_some_and(|byte| !byte.is_ascii_digit())
            {
                let (head, tail) = match left.split_first() {
                    Some((byte, rest)) if !byte.is_ascii_digit() => (Some(*byte), rest),
                    _ => (None, left),
                };
                let (other, other_rest) = match right.split_first() {
                    Some((byte, rest)) if !byte.is_ascii_digit() => (Some(*byte), rest),
                    _ => (None, right),
                };
                if order_value(head).cmp(&order_value(other)) != Ordering::Equal {
                    return order_value(head).cmp(&order_value(other));
                }
                left = tail;
                right = other_rest;
            }
            while left.first() == Some(&b'0') {
                left = &left[1..];
            }
            while right.first() == Some(&b'0') {
                right = &right[1..];
            }
            let mut first_diff = Ordering::Equal;
            while left.first().is_some_and(|byte| byte.is_ascii_digit())
                && right.first().is_some_and(|byte| byte.is_ascii_digit())
            {
                if first_diff == Ordering::Equal {
                    first_diff = left[0].cmp(&right[0]);
                }
                left = &left[1..];
                right = &right[1..];
            }
            if left.first().is_some_and(|byte| byte.is_ascii_digit()) {
                return Ordering::Greater;
            }
            if right.first().is_some_and(|byte| byte.is_ascii_digit()) {
                return Ordering::Less;
            }
            if first_diff != Ordering::Equal {
                return first_diff;
            }
        }
        Ordering::Equal
    }
    let (left_epoch, left) = split_epoch(left);
    let (right_epoch, right) = split_epoch(right);
    if left_epoch != right_epoch {
        return left_epoch.cmp(&right_epoch);
    }
    fn split_revision(version: &str) -> (&str, &str) {
        match version.rfind('-') {
            Some(index) => (&version[..index], &version[index + 1..]),
            None => (version, ""),
        }
    }
    let (left_upstream, left_revision) = split_revision(left);
    let (right_upstream, right_revision) = split_revision(right);
    verrevcmp(left_upstream.as_bytes(), right_upstream.as_bytes()).then(verrevcmp(
        left_revision.as_bytes(),
        right_revision.as_bytes(),
    ))
}

fn policy_for<'a>(
    policy: &'a BTreeMap<(String, Option<String>), PolicyEntry>,
    name: &str,
    arch: &str,
) -> Option<&'a PolicyEntry> {
    policy
        .get(&(name.into(), Some(arch.into())))
        .or_else(|| policy.get(&(name.into(), None)))
}

fn update_state(
    installed: Option<&str>,
    candidate: Option<&str>,
    held: bool,
    phased: bool,
) -> UpdateAvailability {
    let upgradable = installed.is_some_and(|_| true)
        && candidate.is_some()
        && matches!(
            (installed, candidate),
            (Some(installed), Some(candidate))
                if deb_cmp(candidate, installed) == Ordering::Greater
        )
        && !held
        && !phased;
    if upgradable {
        UpdateAvailability::Available
    } else if installed.is_some() {
        UpdateAvailability::Current
    } else {
        UpdateAvailability::Unknown
    }
}

fn build_details(
    name: &str,
    arch: &str,
    installed: Option<String>,
    candidate: Option<String>,
    held: bool,
    phased: bool,
    record: Option<&ShowRecord>,
) -> PackageDetails {
    let summary = record
        .map(|record| record.summary.clone())
        .filter(|summary| !summary.is_empty())
        .unwrap_or_else(|| name.into());
    PackageDetails {
        package: Package {
            id: PackageId {
                backend: "apt".into(),
                name: name.into(),
                architecture: arch.into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: name.into(),
            summary,
            installed_version: installed.clone(),
            candidate_version: candidate.clone(),
            update: update_state(installed.as_deref(), candidate.as_deref(), held, phased),
            icon: None,
            component_ids: vec![],
            homepages: vec![],
        },
        description: record
            .map(|record| record.description.clone())
            .unwrap_or_default(),
        homepage: record.and_then(|record| record.homepage.clone()),
        dependencies: record
            .map(|record| record.depends.clone())
            .filter(|depends| !depends.is_empty())
            .unwrap_or_default(),
    }
}

fn phased_candidate(
    entry: &PolicyEntry,
    candidate: Option<&str>,
    record: Option<&ShowRecord>,
) -> bool {
    candidate.is_some_and(|candidate| entry.phased.contains(candidate))
        || record
            .and_then(|record| record.phased_percent)
            .is_some_and(|percent| percent != 100)
}

/// Assemble `installed` records from one `dpkg-query`, one batched
/// `apt-cache policy`, and one batched `apt-cache show` output.
pub(crate) fn installed_packages(
    dpkg: &[DpkgRow],
    policy: &BTreeMap<(String, Option<String>), PolicyEntry>,
    show: &[ShowRecord],
) -> Vec<PackageDetails> {
    let mut details = Vec::new();
    for row in dpkg {
        let entry = policy_for(policy, &row.name, &row.arch);
        let candidate = entry.and_then(|entry| entry.candidate.clone());
        let chosen = candidate.clone().or(Some(row.version.clone()));
        let record = chosen
            .as_deref()
            .and_then(|version| select_record(show, &row.name, version, &row.arch));
        let phased = entry.map(|entry| phased_candidate(entry, candidate.as_deref(), record));
        details.push(build_details(
            &row.name,
            &row.arch,
            Some(row.version.clone()),
            candidate,
            row.held,
            phased.unwrap_or(false),
            record,
        ));
    }
    details.sort_by(|left: &PackageDetails, right: &PackageDetails| {
        left.package.id.cmp(&right.package.id)
    });
    details
}

/// Parse `apt-cache search` output (`name - summary` per line).
pub(crate) fn parse_search_dump(text: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for line in text.lines() {
        match line.split_once(" - ") {
            Some((name, summary)) if !name.is_empty() && !name.contains(char::is_whitespace) => {
                rows.push((name.into(), summary.into()))
            }
            _ => {
                let name = line.trim();
                if !name.is_empty() && !name.contains(char::is_whitespace) {
                    rows.push((name.into(), String::new()));
                }
            }
        }
    }
    rows
}

/// Assemble `search` records: callers narrow `search_dump` matches with the
/// same substring rule as the native helper, then supply batched policy/show
/// output for the surviving names only.
pub(crate) fn search_packages(
    needle: &str,
    candidates: &[(String, String)],
    policy: &BTreeMap<(String, Option<String>), PolicyEntry>,
    show: &[ShowRecord],
    installed_rows: &[DpkgRow],
) -> Vec<PackageDetails> {
    let folded = needle.to_lowercase();
    let mut seen = BTreeSet::new();
    let mut details = Vec::new();
    for (name, summary) in candidates {
        if !format!("{name} {summary}").to_lowercase().contains(&folded) {
            continue;
        }
        // One identity per name: prefer the native-arch stanza, mirroring the
        // helper's single-version choice of candidate over installed state.
        let stanzas: Vec<(&(String, Option<String>), &PolicyEntry)> =
            policy.iter().filter(|((id, _), _)| *id == *name).collect();
        for ((_, arch), entry) in stanzas {
            let installed = entry.installed.clone();
            let candidate = entry.candidate.clone();
            if installed.is_none() && candidate.is_none() {
                continue;
            }
            // Resolve the display architecture: stanza-qualified names carry
            // it, native stanzas inherit the installed row or candidate record.
            let resolved = arch.clone().or_else(|| {
                candidate
                    .as_deref()
                    .and_then(|version| {
                        show.iter()
                            .find(|record| record.name == *name && record.version == version)
                    })
                    .map(|record| record.arch.clone())
                    .filter(|arch| !arch.is_empty())
            });
            let Some(arch) = resolved else { continue };
            if !seen.insert((name.clone(), arch.clone())) {
                continue;
            }
            let chosen = candidate.clone().or(installed.clone());
            let record = chosen
                .as_deref()
                .and_then(|version| select_record(show, name, version, &arch));
            let phased = phased_candidate(entry, candidate.as_deref(), record);
            let held = installed_rows
                .iter()
                .any(|row| row.name == *name && row.arch == arch && row.held);
            details.push(build_details(
                name, &arch, installed, candidate, held, phased, record,
            ));
        }
    }
    details.sort_by(|left: &PackageDetails, right: &PackageDetails| {
        left.package.id.cmp(&right.package.id)
    });
    details
}

/// Assemble one `details` record for an exact name/architecture identity.
pub(crate) fn details_package(
    name: &str,
    arch: &str,
    policy: &BTreeMap<(String, Option<String>), PolicyEntry>,
    show: &[ShowRecord],
    installed_rows: &[DpkgRow],
) -> Option<PackageDetails> {
    let entry = policy_for(policy, name, arch)?;
    let stanza_arch = policy
        .get(&(name.into(), Some(arch.into())))
        .map(|_| arch.to_owned())
        .or_else(|| {
            entry
                .candidate
                .as_deref()
                .and_then(|version| {
                    show.iter()
                        .find(|record| record.name == name && record.version == version)
                })
                .map(|record| record.arch.clone())
                .filter(|candidate| candidate == arch)
        })?;
    if stanza_arch != arch {
        return None;
    }
    let installed = entry.installed.clone();
    let candidate = entry.candidate.clone();
    if installed.is_none() && candidate.is_none() {
        return None;
    }
    let chosen = candidate.clone().or(installed.clone());
    let record = chosen
        .as_deref()
        .and_then(|version| select_record(show, name, version, arch));
    let phased = phased_candidate(entry, candidate.as_deref(), record);
    let held = installed_rows
        .iter()
        .any(|row| row.name == name && row.arch == arch && row.held);
    Some(build_details(
        name, arch, installed, candidate, held, phased, record,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debian_versions_compare_epochs_tildes_and_revisions() {
        assert_eq!(deb_cmp("1", "2"), Ordering::Less);
        assert_eq!(deb_cmp("2", "1"), Ordering::Greater);
        assert_eq!(deb_cmp("1.0", "1.0"), Ordering::Equal);
        assert_eq!(deb_cmp("1.0~beta1", "1.0"), Ordering::Less);
        assert_eq!(deb_cmp("1.0", "1.0~beta1"), Ordering::Greater);
        assert_eq!(deb_cmp("1:1.0", "2.0"), Ordering::Greater);
        assert_eq!(deb_cmp("2.0", "1:1.0"), Ordering::Less);
        assert_eq!(deb_cmp("1.0", "1.0-1"), Ordering::Less);
        assert_eq!(deb_cmp("1.0-1", "1.0-2"), Ordering::Less);
        assert_eq!(deb_cmp("1.0-10", "1.0-2"), Ordering::Greater);
        assert_eq!(deb_cmp("1.02", "1.2"), Ordering::Equal);
        assert_eq!(deb_cmp("5.3-2ubuntu1", "5.3-2ubuntu1"), Ordering::Equal);
        assert_eq!(
            deb_cmp("6.6.4-0ubuntu1", "6.6.6-0ubuntu0.1"),
            Ordering::Less
        );
        assert_eq!(deb_cmp("1.0a", "1.0"), Ordering::Greater);
    }

    #[test]
    fn dpkg_rows_keep_installed_and_hold_states() {
        let rows = parse_dpkg_table(
            "bash\tamd64\t5.3-2ubuntu1\tinstall ok installed\n\
             held-pkg\tamd64\t1.0\thold ok installed\n\
             removed\tamd64\t1.0\tdeinstall ok config-files\n\
             foreign:i386\ti386\t3.0\tinstall ok installed\n\
             half\tamd64\t1.0\tinstall ok unpacked\n\
             broken\n\
             empty-ver\tamd64\t\tinstall ok installed\n",
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "bash");
        assert!(!rows[0].held);
        assert!(rows[1].held);
        assert_eq!(rows[2].name, "foreign");
        assert_eq!(rows[2].arch, "i386");
    }

    #[test]
    fn policy_parses_arch_stanzas_phasing_and_none_markers() {
        let policy = parse_policy_dump(
            "bash:\n  Installed: 5.3-2ubuntu1\n  Candidate: 5.3-2ubuntu1\n  Version table:\n *** 5.3-2ubuntu1 500\n        500 https://example.invalid amd64 Packages\n        100 /var/lib/dpkg/status\n\
             drkonqi:\n  Installed: 6.6.4-0ubuntu1\n  Candidate: 6.6.6-0ubuntu0.1\n  Version table:\n     6.6.6-0ubuntu0.1 500 (phased 0%)\n        500 https://example.invalid amd64 Packages\n *** 6.6.4-0ubuntu1 500\n        100 /var/lib/dpkg/status\n\
              libfoo:i386:\n  Installed: (none)\n  Candidate: 1.0\n  Version table:\n     1.0 500 (phased 100%)\n",
        );
        let bash = &policy[&("bash".into(), None)];
        assert_eq!(bash.installed.as_deref(), Some("5.3-2ubuntu1"));
        assert_eq!(bash.candidate.as_deref(), Some("5.3-2ubuntu1"));
        assert!(bash.phased.is_empty());
        let drkonqi = &policy[&("drkonqi".into(), None)];
        assert!(drkonqi.phased.contains("6.6.6-0ubuntu0.1"));
        let i386 = &policy[&("libfoo".into(), Some("i386".into()))];
        assert_eq!(i386.installed, None);
        assert_eq!(i386.candidate.as_deref(), Some("1.0"));
        assert!(i386.phased.is_empty());
    }

    #[test]
    fn show_parses_records_dependencies_and_phasing() {
        let records = parse_show_dump(
            "Package: bash\nArchitecture: amd64\nVersion: 5.3-2ubuntu1\n\
             Depends: base-files (>= 2.1.12), debianutils (>= 5.6-0.1)\n\
             Homepage: https://example.invalid/shell\n\
             Description-en: GNU Bourne Again SHell\n Bash is an interpreter.\n .\n It conforms.\n\n\
             Package: old\nArchitecture: all\nVersion: 0.1\nDescription-en: Old tool\n",
        );
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].summary, "GNU Bourne Again SHell");
        assert!(records[0].description.contains("It conforms."));
        assert_eq!(
            records[0].homepage.as_deref(),
            Some("https://example.invalid/shell")
        );
        assert_eq!(
            records[0].depends,
            ["base-files (>= 2.1.12), debianutils (>= 5.6-0.1)"]
        );
        assert_eq!(records[1].homepage, None);
        assert!(records[1].depends.is_empty());
        assert_eq!(
            select_record(&records, "bash", "5.3-2ubuntu1", "amd64")
                .unwrap()
                .summary,
            "GNU Bourne Again SHell"
        );
        assert!(select_record(&records, "bash", "9.9", "amd64").is_none());
    }

    #[test]
    fn installed_joins_dpkg_policy_and_show() {
        let dpkg = parse_dpkg_table(
            "bash\tamd64\t5.3-2ubuntu1\tinstall ok installed\n\
             held-pkg\tamd64\t1.0\thold ok installed\n\
             phased-pkg\tamd64\t1.0\tinstall ok installed\n",
        );
        let policy = parse_policy_dump(
            "bash:\n  Installed: 5.3-2ubuntu1\n  Candidate: 5.3-2ubuntu1\n  Version table:\n *** 5.3-2ubuntu1 500\n\
             held-pkg:\n  Installed: 1.0\n  Candidate: 2.0\n  Version table:\n     2.0 500\n *** 1.0 500\n\
             phased-pkg:\n  Installed: 1.0\n  Candidate: 2.0\n  Version table:\n     2.0 500 (phased 30%)\n *** 1.0 500\n",
        );
        let show = parse_show_dump(
            "Package: bash\nArchitecture: amd64\nVersion: 5.3-2ubuntu1\nDescription-en: Shell\n\n\
             Package: held-pkg\nArchitecture: amd64\nVersion: 2.0\nDescription-en: Held\n\n\
             Package: phased-pkg\nArchitecture: amd64\nVersion: 2.0\nDescription-en: Phased\n",
        );
        let details = installed_packages(&dpkg, &policy, &show);
        assert_eq!(details.len(), 3);
        assert_eq!(details[0].package.update, UpdateAvailability::Current);
        assert_eq!(details[0].package.summary, "Shell");
        // Held packages never report an upgrade even when a newer candidate exists.
        assert_eq!(details[1].package.update, UpdateAvailability::Current);
        // Phased candidates below full rollout are not offered either.
        assert_eq!(details[2].package.update, UpdateAvailability::Current);
        assert_eq!(details[0].package.id.scope, Scope::System);
    }

    #[test]
    fn installed_reports_available_upgrades() {
        let dpkg = parse_dpkg_table("up\tamd64\t1.0\tinstall ok installed\n");
        let policy = parse_policy_dump(
            "up:\n  Installed: 1.0\n  Candidate: 2.0\n  Version table:\n     2.0 500\n *** 1.0 500\n",
        );
        let show = parse_show_dump(
            "Package: up\nArchitecture: amd64\nVersion: 2.0\nDescription-en: New\n",
        );
        let details = installed_packages(&dpkg, &policy, &show);
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].package.update, UpdateAvailability::Available);
        assert_eq!(details[0].package.candidate_version.as_deref(), Some("2.0"));
    }

    #[test]
    fn batched_show_records_never_cross_package_names() {
        let rows = parse_dpkg_table(
            "alpha\tamd64\t1.0\tinstall ok installed\n\
             beta\tamd64\t1.0\tinstall ok installed\n",
        );
        let policy = parse_policy_dump(
            "alpha:\n  Installed: 1.0\n  Candidate: 1.0\n\
             beta:\n  Installed: 1.0\n  Candidate: 1.0\n",
        );
        let show = parse_show_dump(
            "Package: alpha\nArchitecture: amd64\nVersion: 1.0\nDescription: Alpha description\n\n\
             Package: beta\nArchitecture: amd64\nVersion: 1.0\nDescription: Beta description\n",
        );
        let installed = installed_packages(&rows, &policy, &show);
        assert_eq!(installed[0].package.summary, "Alpha description");
        assert_eq!(installed[1].package.summary, "Beta description");
        let search = search_packages(
            "beta",
            &[
                ("alpha".into(), "Alpha".into()),
                ("beta".into(), "Beta".into()),
            ],
            &policy,
            &show,
            &rows,
        );
        assert_eq!(search.len(), 1);
        assert_eq!(search[0].package.summary, "Beta description");
        assert_eq!(
            details_package("beta", "amd64", &policy, &show, &rows)
                .unwrap()
                .package
                .summary,
            "Beta description"
        );
    }

    #[test]
    fn search_matches_names_and_summaries_without_shells() {
        let candidates = parse_search_dump("cafe - CAFÉ player\ninstalled-only - Installed only\n");
        let policy = parse_policy_dump(
            "cafe:\n  Installed: (none)\n  Candidate: 2.0\n  Version table:\n     2.0 500\n\
             installed-only:\n  Installed: 1.0\n  Candidate: (none)\n  Version table:\n *** 1.0 100\n",
        );
        let show = parse_show_dump(
            "Package: cafe\nArchitecture: amd64\nVersion: 2.0\nDescription-en: CAFÉ player\n\n\
             Package: installed-only\nArchitecture: amd64\nVersion: 1.0\nDescription-en: Installed only\n",
        );
        let found = search_packages("café", &candidates, &policy, &show, &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].package.id.name, "cafe");
        assert_eq!(found[0].package.update, UpdateAvailability::Unknown);
        let missing = search_packages("nothing-matches", &candidates, &policy, &show, &[]);
        assert!(missing.is_empty());
    }

    #[test]
    fn details_requires_exact_name_and_architecture() {
        let policy = parse_policy_dump(
            "synthetic:\n  Installed: 1.0\n  Candidate: 2.0\n  Version table:\n     2.0 500\n *** 1.0 500\n",
        );
        let show = parse_show_dump(
            "Package: synthetic\nArchitecture: arm64\nVersion: 2.0\nDescription-en: Synthetic\n",
        );
        let found = details_package("synthetic", "arm64", &policy, &show, &[]).unwrap();
        assert_eq!(found.package.candidate_version.as_deref(), Some("2.0"));
        assert_eq!(found.package.summary, "Synthetic");
        assert!(details_package("synthetic", "i386", &policy, &show, &[]).is_none());
        assert!(details_package("other", "arm64", &policy, &show, &[]).is_none());
    }

    #[test]
    fn details_without_any_version_is_not_found() {
        let policy = parse_policy_dump("ghost:\n  Installed: (none)\n  Candidate: (none)\n");
        assert!(details_package("ghost", "amd64", &policy, &[], &[]).is_none());
    }

    #[test]
    fn held_packages_are_not_offered_as_updates_in_search_or_details() {
        let rows = parse_dpkg_table("held\tamd64\t1.0\thold ok installed\n");
        let policy = parse_policy_dump(
            "held:\n  Installed: 1.0\n  Candidate: 2.0\n  Version table:\n     2.0 500\n *** 1.0 100\n",
        );
        let show = parse_show_dump(
            "Package: held\nArchitecture: amd64\nVersion: 2.0\nDescription: Held package\n",
        );
        let search = search_packages(
            "held",
            &[("held".into(), "Held package".into())],
            &policy,
            &show,
            &rows,
        );
        assert_eq!(search[0].package.update, UpdateAvailability::Current);
        let details = details_package("held", "amd64", &policy, &show, &rows).unwrap();
        assert_eq!(details.package.update, UpdateAvailability::Current);
    }
}
