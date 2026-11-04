//! AppImages that update from a GitHub project's releases: which project,
//! and which file of its newest release fits this computer.

use serde_json::Value;

/// One AppImage of a release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Asset {
    pub tag: String,
    pub name: String,
    pub url: String,
    pub size: u64,
    /// The SHA-256 GitHub publishes for the file, in hex, when it has one.
    pub sha256: Option<String>,
}

/// `owner/name` from `owner/name` or a `https://github.com/owner/name…`
/// link. Names keep to the characters GitHub allows.
pub(super) fn repository(input: &str) -> Option<String> {
    let input = input.trim().trim_end_matches('/');
    let path = input
        .strip_prefix("https://github.com/")
        .or_else(|| input.strip_prefix("http://github.com/"))
        .or_else(|| input.strip_prefix("github.com/"))
        .unwrap_or(input);
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let name = parts.next()?.trim_end_matches(".git");
    let valid = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    (valid(owner) && valid(name)).then(|| format!("{owner}/{name}"))
}

/// Whether a file name is an AppImage for `arch` (`x86_64` or `aarch64`).
/// Names that say no architecture count as x86_64, as projects publish them.
fn fits(name: &str, arch: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".appimage") {
        return false;
    }
    let says = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    let arm = says(&["aarch64", "arm64"]);
    let other = says(&["armv7", "armhf", "i386", "i686"]);
    match arch {
        "aarch64" => arm,
        _ => !arm && !other,
    }
}

/// The newest published release (no drafts or pre-releases) that has an
/// AppImage for `arch`, from GitHub's list of releases, newest first. A
/// release may have none: Obsidian publishes Android-only releases.
pub(super) fn newest(releases: &Value, arch: &str) -> Option<Asset> {
    releases.as_array()?.iter().find_map(|release| {
        if release["draft"].as_bool() != Some(false)
            || release["prerelease"].as_bool() != Some(false)
        {
            return None;
        }
        let tag = release["tag_name"].as_str()?;
        release["assets"].as_array()?.iter().find_map(|asset| {
            let name = asset["name"].as_str()?;
            let url = asset["browser_download_url"].as_str()?;
            (fits(name, arch) && url.starts_with("https://")).then(|| Asset {
                tag: tag.to_owned(),
                name: name.to_owned(),
                url: url.to_owned(),
                size: asset["size"].as_u64().unwrap_or(0),
                sha256: asset["digest"]
                    .as_str()
                    .and_then(|digest| digest.strip_prefix("sha256:"))
                    .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
                    .map(str::to_ascii_lowercase),
            })
        })
    })
}

/// A version or tag without a leading `v`, or the product name a project
/// with several products puts first (`cua-spaces-v0.20.0`), for comparing
/// the two.
pub(super) fn plain_version(version: &str) -> &str {
    let version = version.trim();
    let version = version
        .rmatch_indices("-v")
        .find(|(at, _)| version[at + 2..].starts_with(|c: char| c.is_ascii_digit()))
        .map_or(version, |(at, _)| &version[at + 1..]);
    version
        .strip_prefix('v')
        .or_else(|| version.strip_prefix('V'))
        .unwrap_or(version)
}

/// A version to put in order: semantic versions, and `1` or `1.2` read as
/// `1.0.0` and `1.2.0`.
pub(super) fn ordered(version: &str) -> Option<semver::Version> {
    let version = plain_version(version);
    semver::Version::parse(version).ok().or_else(|| {
        let parts: Vec<u64> = version
            .split('.')
            .map(|part| part.parse().ok())
            .collect::<Option<_>>()?;
        match parts[..] {
            [major] => Some(semver::Version::new(major, 0, 0)),
            [major, minor] => Some(semver::Version::new(major, minor, 0)),
            _ => None,
        }
    })
}

/// Whether `theirs` is newer than `ours`: in order when both read as versions,
/// else whenever they differ.
pub(super) fn newer(theirs: &str, ours: &str) -> bool {
    let numbers = |version: &str| -> Option<Vec<u64>> {
        plain_version(version)
            .split('.')
            .map(|part| part.parse().ok())
            .collect()
    };
    if let (Some(mut theirs), Some(mut ours)) = (numbers(theirs), numbers(ours)) {
        // electron-builder adds a build number inside the AppImage: 0.20.0.130
        // is release 0.20.0, and its releases only name the 0.20.0.
        if theirs.len() <= 3 && ours.len() > 3 {
            ours.truncate(3);
        }
        let length = theirs.len().max(ours.len());
        theirs.resize(length, 0);
        ours.resize(length, 0);
        return theirs > ours;
    }
    match (ordered(theirs), ordered(ours)) {
        (Some(theirs), Some(ours)) => theirs > ours,
        _ => plain_version(theirs) != plain_version(ours),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repositories_come_from_names_or_links() {
        for input in [
            "obsidianmd/obsidian-releases",
            " https://github.com/obsidianmd/obsidian-releases/ ",
            "https://github.com/obsidianmd/obsidian-releases/releases/latest",
            "http://github.com/obsidianmd/obsidian-releases.git",
            "github.com/obsidianmd/obsidian-releases",
        ] {
            assert_eq!(
                repository(input).as_deref(),
                Some("obsidianmd/obsidian-releases"),
                "{input}"
            );
        }
        for input in [
            "",
            "owner",
            "owner/",
            "../x",
            "a b/c",
            "https://gitlab.com/a/b",
        ] {
            assert_eq!(repository(input), None, "{input}");
        }
    }

    #[test]
    fn files_fit_their_architecture() {
        assert!(fits("Obsidian-1.13.7.AppImage", "x86_64"));
        assert!(!fits("Obsidian-1.13.7-arm64.AppImage", "x86_64"));
        assert!(fits("Obsidian-1.13.7-arm64.AppImage", "aarch64"));
        assert!(fits("Trezor-Suite-26.9.3-linux-x86_64.AppImage", "x86_64"));
        assert!(!fits(
            "Trezor-Suite-26.9.3-linux-x86_64.AppImage",
            "aarch64"
        ));
        assert!(!fits("app-armhf.AppImage", "x86_64"));
        assert!(!fits(
            "Trezor-Suite-26.9.3-linux-x86_64.AppImage.asc",
            "x86_64"
        ));
        assert!(!fits("app.zsync", "x86_64"));
    }

    #[test]
    fn the_newest_release_with_a_fitting_appimage_wins() {
        let asset = |name: &str, digest: Value| json!({"name": name, "browser_download_url": format!("https://example.invalid/{name}"), "size": 10, "digest": digest});
        let release = |tag: &str, draft: bool, prerelease: bool, assets: Vec<Value>| json!({"tag_name": tag, "draft": draft, "prerelease": prerelease, "assets": assets});
        let sha = "AB".repeat(32);
        let releases = json!([
            release(
                "v1.14.0",
                true,
                false,
                vec![asset("App-1.14.0.AppImage", Value::Null)]
            ),
            release(
                "v1.14.0-beta",
                false,
                true,
                vec![asset("App-1.14.0-beta.AppImage", Value::Null)]
            ),
            release(
                "v1.13.8",
                false,
                false,
                vec![asset("App-1.13.8.apk", Value::Null)]
            ),
            release(
                "v1.13.7",
                false,
                false,
                vec![
                    asset("App-1.13.7-arm64.AppImage", json!("sha256:00")),
                    asset("App-1.13.7.AppImage", json!(format!("sha256:{sha}"))),
                ]
            ),
        ]);
        let found = newest(&releases, "x86_64").unwrap();
        assert_eq!(found.tag, "v1.13.7");
        assert_eq!(found.name, "App-1.13.7.AppImage");
        assert_eq!(found.url, "https://example.invalid/App-1.13.7.AppImage");
        assert_eq!(found.size, 10);
        assert_eq!(found.sha256, Some("ab".repeat(32)));
        // A digest that isn't a SHA-256 is ignored.
        assert_eq!(newest(&releases, "aarch64").unwrap().sha256, None);
        assert_eq!(newest(&json!([]), "x86_64"), None);
        assert_eq!(newest(&json!({"message": "Not Found"}), "x86_64"), None);
        // Plain HTTP links are never offered.
        let insecure = json!([{"tag_name": "v1", "draft": false, "prerelease": false,
            "assets": [{"name": "App.AppImage", "browser_download_url": "http://example.invalid/App.AppImage"}]}]);
        assert_eq!(newest(&insecure, "x86_64"), None);
    }

    #[test]
    fn short_versions_are_put_in_order() {
        assert_eq!(ordered("v2"), Some(semver::Version::new(2, 0, 0)));
        assert_eq!(ordered("2.1"), Some(semver::Version::new(2, 1, 0)));
        assert_eq!(ordered("v2.1.3"), Some(semver::Version::new(2, 1, 3)));
        assert!(ordered("1.0.0-beta.2").is_some());
        assert_eq!(ordered("1.2.3.4"), None);
        assert_eq!(ordered("build-7"), None);
    }
    #[test]
    fn newer_versions_read_the_way_releases_name_them() {
        for (theirs, ours, expected) in [
            ("2.1.0", "2.0", true),
            ("2.0", "2.0.0", false),
            ("2.0.0", "2.0", false),
            ("1.9.0", "2.0", false),
            // Numbers, not text.
            ("0.10.0", "0.9.0", true),
            // A build number inside the file isn't a newer release.
            ("0.20.0", "0.20.0.130", false),
            ("0.19.9", "0.20.0.130", false),
            ("0.20.1", "0.20.0.130", true),
            // Four numbers on both sides all count.
            ("1.2.3.5", "1.2.3.4", true),
            ("1.2.3.4", "1.2.3.5", false),
            // One tag for each product of a project.
            ("cua-spaces-v0.20.0", "0.20.0.130", false),
            ("cua-spaces-v0.21.0", "0.20.0.130", true),
            // Pre-releases come before their release.
            ("2.0.0", "2.0.0-beta.1", true),
            ("2.0.0-beta.1", "2.0.0", false),
            // What doesn't read as a version counts when it differs.
            ("build-7", "2.0", true),
            ("build-7", "build-7", false),
        ] {
            assert_eq!(newer(theirs, ours), expected, "{theirs} over {ours}");
        }
        assert_eq!(plain_version(" v1.2 "), "1.2");
        assert_eq!(plain_version("my-v-app"), "my-v-app");
    }

    #[test]
    fn versions_compare_without_a_leading_v() {
        assert_eq!(plain_version("v1.13.7"), "1.13.7");
        assert_eq!(plain_version(" V2 "), "2");
        assert_eq!(plain_version("26.9.3"), "26.9.3");
    }
}
