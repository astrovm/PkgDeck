//! Optional local AppStream presentation metadata. Native package identities
//! remain authoritative; this catalog never supplies package operations.
use pkgdeck_core::package::Package;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Screenshot {
    pub url: String,
    pub caption: String,
}
#[derive(Clone, Default)]
pub struct AppInfo {
    pub name: String,
    pub description: String,
    pub homepage: Option<String>,
    pub screenshots: Vec<Screenshot>,
}
#[derive(Default)]
pub struct Catalog(BTreeMap<String, Arc<AppInfo>>);
static CATALOG: OnceLock<Catalog> = OnceLock::new();

pub fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(|| {
        Catalog::load(
            Path::new("/"),
            std::env::var_os("HOME").as_deref().map(Path::new),
        )
    })
}
pub fn cached_info(package: &Package) -> Option<&'static AppInfo> {
    CATALOG.get()?.find(package)
}
fn web_url(value: &str) -> Option<String> {
    let value = value.trim();
    (value.starts_with("https://") && value.len() > 8 && !value.chars().any(char::is_whitespace))
        .then(|| value.to_owned())
}
fn image_url(value: &str, base: Option<&str>) -> Option<String> {
    let value = value.trim();
    if value.contains(':') {
        return web_url(value);
    }
    if value.is_empty() || value.starts_with("//") {
        return None;
    }
    let base = web_url(base?)?;
    web_url(&format!(
        "{}/{}",
        base.trim_end_matches('/'),
        value.trim_start_matches('/')
    ))
}
fn stem(value: &str) -> &str {
    value.strip_suffix(".desktop").unwrap_or(value)
}
fn plain(node: roxmltree::Node<'_, '_>) -> String {
    let mut text = String::new();
    for node in node.descendants() {
        if node.ancestors().any(|ancestor| {
            ancestor
                .attribute(("http://www.w3.org/XML/1998/namespace", "lang"))
                .is_some_and(|lang| lang != "en")
        }) {
            continue;
        }
        if node.is_text() {
            text.push_str(node.text().unwrap_or_default());
        } else if node.has_tag_name("p") || node.has_tag_name("li") || node.has_tag_name("br") {
            text.push(' ');
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    tag: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    node.children()
        .filter(|n| n.has_tag_name(tag))
        .find(|n| {
            n.attribute(("http://www.w3.org/XML/1998/namespace", "lang"))
                .is_none()
        })
        .or_else(|| {
            node.children().find(|n| {
                n.has_tag_name(tag)
                    && n.attribute(("http://www.w3.org/XML/1998/namespace", "lang")) == Some("en")
            })
        })
}
fn localized(value: &serde_yaml::Value) -> String {
    value
        .as_str()
        .or_else(|| value["C"].as_str())
        .or_else(|| value["en"].as_str())
        .unwrap_or("")
        .to_owned()
}
impl Catalog {
    pub fn find(&self, package: &Package) -> Option<&AppInfo> {
        if !matches!(
            package.id.backend.as_str(),
            "apt" | "dnf" | "pacman" | "zypper" | "flatpak" | "snap" | "appimage"
        ) {
            return None;
        }
        package
            .component_ids
            .iter()
            .chain(std::iter::once(&package.id.name))
            .find_map(|id| self.0.get(&format!("id:{}", stem(id))))
            .or_else(|| {
                (package.id.backend != "flatpak")
                    .then(|| self.0.get(&format!("pkg:{}", package.id.name)))
                    .flatten()
            })
            .map(Arc::as_ref)
    }
    pub fn enrich(&self, package: &mut Package) {
        if let Some(info) = self.find(package) {
            if !info.name.is_empty() {
                package.display_name.clone_from(&info.name);
            }
        }
    }
    fn insert(&mut self, ids: Vec<String>, packages: Vec<String>, mut info: AppInfo) {
        let keys: Vec<_> = ids
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(|id| format!("id:{}", stem(&id)))
            .chain(
                packages
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .map(|name| format!("pkg:{name}")),
            )
            .collect();
        if let Some(previous) = keys.iter().find_map(|key| self.0.get(key)) {
            if info.name.is_empty() {
                info.name.clone_from(&previous.name);
            }
            if info.description.is_empty() {
                info.description.clone_from(&previous.description);
            }
            if info.homepage.is_none() {
                info.homepage.clone_from(&previous.homepage);
            }
            if info.screenshots.is_empty() {
                info.screenshots.clone_from(&previous.screenshots);
            }
        }
        let info = Arc::new(info);
        for key in keys {
            self.0.insert(key, info.clone());
        }
    }
    fn xml(&mut self, text: &str) {
        let Ok(doc) = roxmltree::Document::parse(text) else {
            return;
        };
        let media_base = doc.root_element().attribute("media_baseurl");
        for component in doc.descendants().filter(|n| {
            n.has_tag_name("component") && n.attribute("type") == Some("desktop-application")
        }) {
            let Some(id) = child(component, "id").and_then(|n| n.text()) else {
                continue;
            };
            let mut ids = vec![id.to_owned()];
            ids.extend(
                component
                    .children()
                    .filter(|n| {
                        n.has_tag_name("launchable") && n.attribute("type") == Some("desktop-id")
                    })
                    .filter_map(|n| n.text().map(str::to_owned)),
            );
            let packages = component
                .children()
                .filter(|n| n.has_tag_name("pkgname"))
                .filter_map(|n| n.text().map(str::to_owned))
                .collect();
            let mut screenshots = Vec::new();
            for shot in component
                .descendants()
                .filter(|n| n.has_tag_name("screenshot"))
            {
                if let Some(url) = shot
                    .children()
                    .filter(|n| {
                        n.has_tag_name("image") && n.attribute("type").is_none_or(|t| t == "source")
                    })
                    .filter_map(|n| n.text().and_then(|url| image_url(url, media_base)))
                    .next()
                {
                    if !screenshots.iter().any(|s: &Screenshot| s.url == url) {
                        screenshots.push(Screenshot {
                            url,
                            caption: child(shot, "caption").map(plain).unwrap_or_default(),
                        });
                    }
                }
                if screenshots.len() == 8 {
                    break;
                }
            }
            self.insert(
                ids,
                packages,
                AppInfo {
                    name: child(component, "name").map(plain).unwrap_or_default(),
                    description: child(component, "description")
                        .map(plain)
                        .unwrap_or_default(),
                    homepage: component
                        .children()
                        .find(|n| n.has_tag_name("url") && n.attribute("type") == Some("homepage"))
                        .and_then(|n| n.text())
                        .and_then(web_url),
                    screenshots,
                },
            );
        }
    }
    fn yaml(&mut self, text: &str) {
        let mut media_base = None;
        for document in serde_yaml::Deserializer::from_str(text) {
            let Ok(value) = serde_yaml::Value::deserialize(document) else {
                break;
            };
            if value["File"].as_str() == Some("DEP-11") {
                media_base = value["MediaBaseUrl"].as_str().and_then(web_url);
                continue;
            }
            if value["Type"].as_str() != Some("desktop-application") {
                continue;
            }
            let Some(id) = value["ID"].as_str() else {
                continue;
            };
            let screenshots = value["Screenshots"]
                .as_sequence()
                .into_iter()
                .flatten()
                .filter_map(|shot| {
                    Some(Screenshot {
                        url: image_url(
                            shot["source-image"]["url"].as_str()?,
                            media_base.as_deref(),
                        )?,
                        caption: localized(&shot["caption"]),
                    })
                })
                .take(8)
                .collect();
            self.insert(
                vec![id.to_owned()],
                value["Package"]
                    .as_str()
                    .map(str::to_owned)
                    .into_iter()
                    .collect(),
                AppInfo {
                    name: localized(&value["Name"]),
                    // DEP-11 descriptions contain markup; render its text, never HTML.
                    description: roxmltree::Document::parse(&format!(
                        "<description>{}</description>",
                        localized(&value["Description"])
                    ))
                    .map(|d| plain(d.root_element()))
                    .unwrap_or_default(),
                    homepage: value["Url"]["homepage"].as_str().and_then(web_url),
                    screenshots,
                },
            );
        }
    }
    fn load(root: &Path, home: Option<&Path>) -> Self {
        let mut files = BTreeSet::new();
        for dir in [
            "usr/share/metainfo",
            "usr/share/appdata",
            "var/lib/swcatalog/xml",
            "var/cache/swcatalog/xml",
            "var/lib/app-info/xmls",
            "var/cache/app-info/xmls",
            "var/lib/apt/lists",
        ] {
            collect_files(&root.join(dir), &mut files);
        }
        let mut flatpak = vec![root.join("var/lib/flatpak/appstream")];
        if let Some(home) = home {
            collect_files(&home.join(".local/share/metainfo"), &mut files);
            flatpak.push(home.join(".local/share/flatpak/appstream"));
        }
        for path in flatpak {
            for remote in entries(&path) {
                for arch in entries(&remote) {
                    collect_files(&arch.join("active"), &mut files);
                }
            }
        }
        let mut catalog = Self::default();
        for path in files {
            if let Ok(file) = fs::File::open(&path) {
                let reader: Box<dyn Read> = if path.extension().is_some_and(|ext| ext == "gz") {
                    Box::new(flate2::read::GzDecoder::new(file))
                } else {
                    Box::new(file)
                };
                let mut text = String::new();
                // Bound decompression and allocation for optional metadata.
                if reader
                    .take(64 * 1024 * 1024 + 1)
                    .read_to_string(&mut text)
                    .is_err()
                    || text.len() > 64 * 1024 * 1024
                {
                    continue;
                }
                if path.to_string_lossy().contains(".xml") {
                    catalog.xml(&text);
                } else {
                    catalog.yaml(&text);
                }
            }
        }
        catalog
    }
}
fn entries(path: &Path) -> Vec<PathBuf> {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect()
}
fn collect_files(path: &Path, files: &mut BTreeSet<PathBuf>) {
    for file in entries(path) {
        if file.extension().is_some_and(|ext| ext == "gz") && file.with_extension("").is_file() {
            continue;
        }
        let name = file.to_string_lossy();
        if [".xml", ".xml.gz", ".yml.gz", ".yaml.gz"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
        {
            if let Ok(canonical) = file.canonicalize() {
                files.insert(canonical);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pkgdeck_core::package::{PackageId, Scope, UpdateAvailability};
    use std::io::Write;

    const APP: &str = r#"<component type="desktop-application">
      <id>org.example.Player</id><launchable type="desktop-id">player.desktop</launchable>
      <pkgname>player-bin</pkgname><name xml:lang="es">Reproductor</name><name>Example Player</name>
      <description><p>A <em>friendly</em> player.</p><p xml:lang="es">Traducción</p><p>Second paragraph.</p></description>
      <url type="homepage">https://example.invalid/player</url>
      <screenshots>
        <screenshot><caption>Library</caption><image type="thumbnail">https://example.invalid/thumb.png</image><image type="source">https://example.invalid/player.png</image></screenshot>
        <screenshot><image>https://example.invalid/player.png</image></screenshot>
        <screenshot><image>file:///etc/passwd</image></screenshot>
      </screenshots>
    </component>"#;
    fn package(backend: &str, name: &str) -> Package {
        Package {
            id: PackageId {
                backend: backend.into(),
                name: name.into(),
                architecture: "test".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: name.into(),
            summary: String::new(),
            installed_version: None,
            candidate_version: Some("1".into()),
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
        }
    }
    #[test]
    fn desktop_names_keep_native_identities_and_ignore_unrelated_packages() {
        let mut catalog = Catalog::default();
        catalog.xml(APP);
        for (backend, name) in [
            ("apt", "player-bin"),
            ("flatpak", "org.example.Player"),
            ("snap", "player"),
        ] {
            let mut p = package(backend, name);
            let identity = p.id.clone();
            catalog.enrich(&mut p);
            assert_eq!(p.display_name, "Example Player");
            assert_eq!(p.id, identity);
            let info = catalog.find(&p).unwrap();
            assert_eq!(info.description, "A friendly player. Second paragraph.");
            assert_eq!(
                info.homepage.as_deref(),
                Some("https://example.invalid/player")
            );
            assert_eq!(
                info.screenshots,
                [Screenshot {
                    url: "https://example.invalid/player.png".into(),
                    caption: "Library".into()
                }]
            );
        }
        for (backend, name) in [
            ("npm", "player"),
            ("apt", "libplayer"),
            ("flatpak", "player-bin"),
        ] {
            let mut p = package(backend, name);
            catalog.enrich(&mut p);
            assert_eq!(p.display_name, name);
            assert!(catalog.find(&p).is_none());
        }
        let mut p = package("dnf", "different-native-name");
        p.component_ids.push("org.example.Player.desktop".into());
        catalog.enrich(&mut p);
        assert_eq!(p.display_name, "Example Player");
    }
    #[test]
    fn optional_xml_metadata_is_resilient_and_screenshots_are_bounded() {
        let mut catalog = Catalog::default();
        catalog.xml("not xml");
        catalog.xml(r#"<components><component type="addon"><id>addon</id></component><component type="desktop-application"><name>No ID</name></component></components>"#);
        assert!(catalog.0.is_empty());
        catalog.xml(APP);
        catalog.xml(r#"<component type="desktop-application"><id>org.example.Player</id><name xml:lang="en">Updated name</name></component>"#);
        let info = catalog
            .find(&package("flatpak", "org.example.Player"))
            .unwrap();
        assert_eq!(info.name, "Updated name");
        assert!(!info.description.is_empty());
        assert!(info.homepage.is_some());
        assert_eq!(info.screenshots.len(), 1);
        let shots: String = (0..12)
            .map(|n| {
                format!("<screenshot><image>https://example.invalid/{n}.png</image></screenshot>")
            })
            .collect();
        catalog.xml(&format!(r#"<component type="desktop-application"><id>many</id><screenshots>{shots}</screenshots></component>"#));
        let info = catalog.find(&package("apt", "many")).unwrap();
        assert_eq!(info.screenshots.len(), 8);
        let mut p = package("apt", "many");
        catalog.enrich(&mut p);
        assert_eq!(p.display_name, "many");
    }
    #[test]
    fn dep11_names_descriptions_and_images_use_default_or_english_metadata() {
        let mut catalog = Catalog::default();
        catalog.yaml(
            r#"---
File: DEP-11
---
Type: addon
ID: ignored
---
Type: desktop-application
Name: missing id
---
Type: desktop-application
ID: org.example.Editor.desktop
Package: editor
Name: {C: Example Editor, es: Editor de ejemplo}
Description: {en: '<p>Edit <em>text</em>.</p>'}
Url: {homepage: 'https://example.invalid/editor'}
Screenshots:
  - source-image: {url: 'https://example.invalid/editor.png'}
    caption: {en: Editing}
  - source-image: {url: 'http://example.invalid/insecure.png'}
  - caption: No image
---
Type: desktop-application
ID: plain
Name: Plain Name
Description: '&invalid;'
---
[broken
"#,
        );
        let info = catalog.find(&package("apt", "editor")).unwrap();
        assert_eq!(info.name, "Example Editor");
        assert_eq!(info.description, "Edit text.");
        assert_eq!(
            info.homepage.as_deref(),
            Some("https://example.invalid/editor")
        );
        assert_eq!(
            info.screenshots,
            [Screenshot {
                url: "https://example.invalid/editor.png".into(),
                caption: "Editing".into()
            }]
        );
        let info = catalog.find(&package("flatpak", "plain")).unwrap();
        assert_eq!(info.name, "Plain Name");
        assert!(info.description.is_empty());
        assert!(info.screenshots.is_empty());
    }
    #[test]
    fn discovers_local_system_user_and_compressed_catalogs_without_network() {
        let temp = pkgdeck_tools::Temp::new();
        let home = temp.0.join("home/test");
        let system = temp.0.join("usr/share/metainfo");
        let remote = home.join(".local/share/flatpak/appstream/example/test/active");
        let dep11 = temp.0.join("var/lib/apt/lists");
        for dir in [&system, &remote, &dep11] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(system.join("player.xml"), APP).unwrap();
        // An uncompressed catalog takes precedence over its compressed copy.
        fs::write(system.join("player.xml.gz"), b"broken gzip").unwrap();
        fs::write(system.join("ignored.txt"), APP).unwrap();
        fs::write(system.join("bad.xml"), [0xff]).unwrap();
        fs::write(system.join("bad.xml.gz"), b"broken gzip").unwrap();
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gzip.write_all(
            APP.replace("org.example.Player", "org.example.Remote")
                .as_bytes(),
        )
        .unwrap();
        fs::write(remote.join("appstream.xml.gz"), gzip.finish().unwrap()).unwrap();
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gzip.write_all(b"Type: desktop-application\nID: org.example.Dep\nPackage: dep\nName: Dependency Browser\n").unwrap();
        fs::write(dep11.join("example.yml.gz"), gzip.finish().unwrap()).unwrap();
        let catalog = Catalog::load(&temp.0, Some(&home));
        assert_eq!(
            catalog.find(&package("apt", "player-bin")).unwrap().name,
            "Example Player"
        );
        assert!(catalog
            .find(&package("flatpak", "org.example.Remote"))
            .is_some());
        assert_eq!(
            catalog.find(&package("apt", "dep")).unwrap().name,
            "Dependency Browser"
        );
        assert!(Catalog::load(&temp.0.join("absent"), None).0.is_empty());
    }
    #[test]
    fn catalog_media_base_resolves_relative_screenshot_paths() {
        let mut catalog = Catalog::default();
        catalog.xml(r#"<components media_baseurl="https://example.invalid/media/"><component type="desktop-application"><id>relative</id><screenshots><screenshot><image>app/image.png</image></screenshot></screenshots></component></components>"#);
        assert_eq!(
            catalog
                .find(&package("flatpak", "relative"))
                .unwrap()
                .screenshots[0]
                .url,
            "https://example.invalid/media/app/image.png"
        );
        catalog.yaml("File: DEP-11\nMediaBaseUrl: https://example.invalid/media\n---\nType: desktop-application\nID: relative-yaml\nScreenshots:\n  - source-image: {url: 'app/yaml.png'}\n");
        assert_eq!(
            catalog
                .find(&package("flatpak", "relative-yaml"))
                .unwrap()
                .screenshots[0]
                .url,
            "https://example.invalid/media/app/yaml.png"
        );
        for (path, base) in [
            ("relative.png", None),
            ("", Some("https://example.invalid")),
            ("//elsewhere/image", Some("https://example.invalid")),
            ("relative.png", Some("file:///tmp")),
        ] {
            assert!(image_url(path, base).is_none());
        }
    }
    #[test]
    fn screenshot_urls_reject_local_files_and_invalid_addresses() {
        for url in [
            "",
            "https://",
            "file:///private",
            "data:image/png;base64,AAA",
            "https://example.invalid/a b",
            "http://example.invalid/a",
        ] {
            assert!(web_url(url).is_none());
        }
        assert_eq!(
            web_url(" https://example.invalid/a ").as_deref(),
            Some("https://example.invalid/a")
        );
    }
}
