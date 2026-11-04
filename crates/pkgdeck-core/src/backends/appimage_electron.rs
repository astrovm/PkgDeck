//! AppImages built with electron-builder say where their updates come from in
//! `resources/app-update.yml`, and don't carry AppImage's own update address.
//! That feed names the newest release and its checksum.

use super::appimage_github;

/// Where an app's releases are listed.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct Feed {
    /// The folder of the list, without a trailing slash.
    base: String,
    channel: String,
}

/// The AppImage of the newest release.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct Release {
    pub version: String,
    pub name: String,
    pub url: String,
    pub size: u64,
    /// SHA-512 of the file, in base64.
    pub sha512: String,
}

/// A `key: value` line, without the quotes YAML allows around the value.
fn value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let value = line.strip_prefix(key)?.strip_prefix(':')?.trim();
    let value = value
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .or_else(|| {
            value
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
        })
        .unwrap_or(value);
    (!value.is_empty()).then_some(value)
}

fn https(url: &str) -> bool {
    url.starts_with("https://")
        && url
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b"\"'<>\\^`{|}".contains(&b))
}

impl Feed {
    /// Reads an `app-update.yml`. Providers that need an account or a custom
    /// server (S3, Keygen and the like) have no feed PkgDeck can read.
    pub fn parse(config: &str) -> Option<Self> {
        let field = |key: &str| config.lines().find_map(|line| value(line, key));
        let channel = field("channel").unwrap_or("latest");
        if !channel
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            || channel.starts_with('.')
        {
            return None;
        }
        let base = match field("provider")? {
            "generic" => field("url")
                .filter(|url| https(url))?
                .trim_end_matches('/')
                .to_owned(),
            "github" => {
                let project = format!("{}/{}", field("owner")?, field("repo")?);
                let project = appimage_github::repository(&project)?;
                format!("https://github.com/{project}/releases/latest/download")
            }
            _ => return None,
        };
        Some(Self {
            base,
            channel: channel.to_owned(),
        })
    }

    /// The list of releases for `architecture`, named the way
    /// electron-updater names it.
    pub fn url(&self, architecture: &str) -> String {
        let suffix = if architecture == "aarch64" {
            "-arm64"
        } else {
            ""
        };
        format!("{}/{}-linux{suffix}.yml", self.base, self.channel)
    }

    /// Reads the list. Its files are links, or names beside the list.
    pub fn release(&self, list: &str) -> Option<Release> {
        let version = list.lines().find_map(|line| value(line, "version"))?;
        let mut files: Vec<(&str, Option<&str>, u64)> = vec![];
        for line in list.lines() {
            let field = line.trim_start();
            if let Some(url) = field.strip_prefix("- ").and_then(|item| value(item, "url")) {
                files.push((url, None, 0));
            } else if line.starts_with(' ') {
                // Top-level keys repeat these for one of the files.
                if let Some(file) = files.last_mut() {
                    if let Some(sha512) = value(field, "sha512") {
                        file.1 = Some(sha512);
                    } else if let Some(size) = value(field, "size") {
                        file.2 = size.parse().unwrap_or(0);
                    }
                }
            }
        }
        let (file, sha512, size) = files
            .into_iter()
            .find(|(url, ..)| url.to_ascii_lowercase().ends_with(".appimage"))?;
        let sha512 = sha512?;
        let name = file.rsplit('/').next()?;
        let url = if file.contains("://") {
            https(file).then(|| file.to_owned())?
        } else if name == file && name != ".." {
            format!("{}/{name}", self.base)
        } else {
            return None;
        };
        Some(Release {
            version: version.to_owned(),
            name: name.to_owned(),
            url,
            size,
            sha512: sha512.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feeds_come_from_providers_anyone_can_read() {
        let generic = Feed::parse(
            "provider: generic\nurl: https://example.invalid/app/\nchannel: latest\nupdaterCacheDirName: app-updater\n",
        )
        .unwrap();
        assert_eq!(
            generic.url("x86_64"),
            "https://example.invalid/app/latest-linux.yml"
        );
        assert_eq!(
            generic.url("aarch64"),
            "https://example.invalid/app/latest-linux-arm64.yml"
        );
        let github = Feed::parse("owner: example\nrepo: 'app'\nprovider: github\n").unwrap();
        assert_eq!(
            github.url("x86_64"),
            "https://github.com/example/app/releases/latest/download/latest-linux.yml"
        );
        let beta = Feed::parse("provider: generic\nurl: https://example.invalid\nchannel: beta\n");
        assert_eq!(
            beta.unwrap().url("x86_64"),
            "https://example.invalid/beta-linux.yml"
        );
        for config in [
            "",
            "provider: s3\nbucket: app\n",
            "provider: generic\n",
            "provider: generic\nurl: http://example.invalid\n",
            "provider: generic\nurl: https://example.invalid/a b\n",
            "provider: generic\nurl: https://example.invalid\nchannel: ../up\n",
            "provider: github\nowner: example\n",
            "provider: github\nowner: ../x\nrepo: app\n",
        ] {
            assert_eq!(Feed::parse(config), None, "{config}");
        }
    }

    fn feed() -> Feed {
        Feed::parse("provider: generic\nurl: https://example.invalid/app\n").unwrap()
    }

    #[test]
    fn a_release_is_its_appimage_with_its_checksum() {
        // The shape electron-builder writes, with another package listed first.
        let list = "version: 2.1.0\nfiles:\n  - url: https://example.invalid/app_2.1.0_amd64.deb\n    sha512: debsum\n    size: 5\n  - url: 'https://example.invalid/App-2.1.0-x86_64.AppImage'\n    sha512: imagesum==\n    size: 1000\n    blockMapSize: 9\npath: https://example.invalid/app_2.1.0_amd64.deb\nsha512: debsum\nreleaseDate: '2026-11-03T20:31:24.659Z'\n";
        assert_eq!(
            feed().release(list),
            Some(Release {
                version: "2.1.0".into(),
                name: "App-2.1.0-x86_64.AppImage".into(),
                url: "https://example.invalid/App-2.1.0-x86_64.AppImage".into(),
                size: 1000,
                sha512: "imagesum==".into(),
            })
        );
        // A bare name lives beside the list.
        let beside = feed()
            .release("version: 2.1.0\nfiles:\n  - url: App.AppImage\n    sha512: sum\n")
            .unwrap();
        assert_eq!(beside.url, "https://example.invalid/app/App.AppImage");
        assert_eq!(beside.size, 0);
        for list in [
            "not a list",
            "files:\n  - url: App.AppImage\n    sha512: sum\n",
            "version: 2.1.0\n",
            "version: 2.1.0\nfiles:\n  - url: App.deb\n    sha512: sum\n",
            "version: 2.1.0\nfiles:\n  - url: App.AppImage\n",
            "version: 2.1.0\nfiles:\n  - url: http://example.invalid/App.AppImage\n    sha512: sum\n",
            "version: 2.1.0\nfiles:\n  - url: ../App.AppImage\n    sha512: sum\n",
        ] {
            assert_eq!(feed().release(list), None, "{list}");
        }
    }
}
