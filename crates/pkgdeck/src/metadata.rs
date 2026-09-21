//! Optional local AppStream presentation metadata. Native package identities
//! remain authoritative; this catalog never supplies package operations.
use pkgdeck_core::{
    package::{Package, PackageId},
    process::Cancellation,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Screenshot {
    pub url: String,
    pub caption: String,
}
#[derive(Clone, Default)]
pub struct AppInfo {
    pub icon: Option<PathBuf>,
    pub name: String,
    pub description: String,
    pub homepage: Option<String>,
    pub screenshots: Vec<Screenshot>,
}
impl AppInfo {
    fn fill_missing(&mut self, other: &Self) {
        if self.icon.is_none() {
            self.icon.clone_from(&other.icon);
        }
        if self.name.is_empty() {
            self.name.clone_from(&other.name);
        }
        if self.description.is_empty() {
            self.description.clone_from(&other.description);
        }
        if self.homepage.is_none() {
            self.homepage.clone_from(&other.homepage);
        }
        if self.screenshots.is_empty() {
            self.screenshots.clone_from(&other.screenshots);
        }
    }
}
#[derive(Default)]
pub struct Catalog {
    metadata_dir: Option<PathBuf>,
    aliases: BTreeMap<String, String>,
    apps: BTreeMap<String, AppInfo>,
}
#[derive(Default)]
struct CatalogCache {
    generation: AtomicU64,
    snapshot: Mutex<Option<(u64, Arc<Catalog>)>>,
}
static CATALOG: CatalogCache = CatalogCache {
    generation: AtomicU64::new(0),
    snapshot: Mutex::new(None),
};
impl CatalogCache {
    fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
    fn cached(&self) -> Option<Arc<Catalog>> {
        let current = self.generation.load(Ordering::Acquire);
        self.snapshot
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(generation, _)| *generation == current)
            .map(|(_, catalog)| catalog.clone())
    }
    fn get(&self, load: impl FnOnce() -> Catalog) -> Arc<Catalog> {
        if let Some(catalog) = self.cached() {
            return catalog;
        }
        let generation = self.generation.load(Ordering::Acquire);
        // File reads and parsing run on the worker, outside the publication lock.
        let catalog = Arc::new(load());
        let mut snapshot = self.snapshot.lock().unwrap();
        if self.generation.load(Ordering::Acquire) == generation {
            *snapshot = Some((generation, catalog.clone()));
        }
        catalog
    }
}
pub fn invalidate() {
    CATALOG.invalidate();
    REMOTE.lock().unwrap().clear();
}
pub fn catalog() -> Arc<Catalog> {
    CATALOG.get(|| {
        Catalog::load(
            Path::new("/"),
            std::env::var_os("HOME").as_deref().map(Path::new),
        )
    })
}
pub fn cached_info(package: &Package) -> Option<AppInfo> {
    REMOTE
        .lock()
        .unwrap()
        .get(&package.id)
        .filter(|(generation, _)| *generation == CATALOG.generation.load(Ordering::Acquire))
        .map(|(_, info)| info.clone())
        .or_else(|| CATALOG.cached()?.find(package).cloned())
}
static REMOTE: Mutex<BTreeMap<PackageId, (u64, AppInfo)>> = Mutex::new(BTreeMap::new());
pub fn enrich(package: &mut Package) {
    catalog().enrich(package);
    if let Some(info) = cached_info(package).filter(|info| !info.name.is_empty()) {
        package.display_name = info.name;
    }
}
pub fn details(package: &mut Package, cancel: &Cancellation) {
    enrich(package);
    let generation = CATALOG.generation.load(Ordering::Acquire);
    if REMOTE
        .lock()
        .unwrap()
        .get(&package.id)
        .is_some_and(|(cached, _)| *cached == generation)
    {
        return;
    }
    let Some(info) = detail_info(
        package,
        cached_info(package).unwrap_or_default(),
        cancel,
        |url| {
            crate::network::ffi::fetch_metadata(
                &url.into(),
                &crate::network::LookupCancellation(cancel.clone()),
            )
            .to_string()
        },
    ) else {
        return;
    };
    if cancel.requested() || CATALOG.generation.load(Ordering::Acquire) != generation {
        return;
    }
    if !info.name.is_empty() {
        package.display_name.clone_from(&info.name);
    }
    let mut remote = REMOTE.lock().unwrap();
    if remote.len() >= 128 {
        remote.clear();
    }
    remote.insert(package.id.clone(), (generation, info));
}
fn detail_info(
    package: &Package,
    mut info: AppInfo,
    cancel: &Cancellation,
    fetch: impl FnOnce(&str) -> String,
) -> Option<AppInfo> {
    if cancel.requested() {
        return None;
    }
    // Searches stay local. Only an opened detail view may use its own provider.
    if info.screenshots.is_empty() {
        if let Some(url) = provider_url(package) {
            if let Some(remote) = provider_info(package, &fetch(&url)) {
                info.fill_missing(&remote);
            }
        }
    }
    (!cancel.requested()).then_some(info)
}
fn provider_url(package: &Package) -> Option<String> {
    let name = &package.id.name;
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
    {
        return None;
    }
    match package.id.backend.as_str() {
        "flatpak" if package.id.remote.as_deref() == Some("flathub") => {
            Some(format!("https://flathub.org/api/v2/appstream/{name}"))
        }
        "snap" => Some(format!("https://api.snapcraft.io/v2/snaps/info/{name}")),
        _ => None,
    }
}
fn description(value: &str) -> String {
    roxmltree::Document::parse(&format!("<description>{value}</description>"))
        .map(|doc| plain(doc.root_element()))
        .unwrap_or_else(|_| {
            if value.contains('<') {
                String::new()
            } else {
                value.into()
            }
        })
}
fn provider_info(package: &Package, text: &str) -> Option<AppInfo> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let (name, summary, body, homepage, shots) = match package.id.backend.as_str() {
        "flatpak" if value["id"].as_str() == Some(&package.id.name) => {
            let shots = value["screenshots"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|shot| {
                    let url = shot["sizes"]
                        .as_array()?
                        .iter()
                        .filter_map(|image| {
                            Some((
                                image["width"].as_u64().unwrap_or(0),
                                web_url(image["src"].as_str()?)?,
                            ))
                        })
                        .max_by_key(|(width, _)| *width)?
                        .1;
                    Some(Screenshot {
                        url,
                        caption: shot["caption"].as_str().unwrap_or("").into(),
                    })
                })
                .take(8)
                .collect();
            (
                &value["name"],
                &value["summary"],
                &value["description"],
                &value["urls"]["homepage"],
                shots,
            )
        }
        "snap" if value["snap"]["name"].as_str() == Some(&package.id.name) => {
            let snap = &value["snap"];
            let shots = snap["media"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|shot| shot["type"].as_str() == Some("screenshot"))
                .filter_map(|shot| {
                    Some(Screenshot {
                        url: web_url(shot["url"].as_str()?)?,
                        caption: String::new(),
                    })
                })
                .take(8)
                .collect();
            (
                &snap["title"],
                &snap["summary"],
                &snap["description"],
                &snap["store-url"],
                shots,
            )
        }
        _ => return None,
    };
    let mut info = AppInfo {
        name: name.as_str().unwrap_or("").into(),
        description: description(body.as_str().unwrap_or("")),
        homepage: homepage.as_str().and_then(web_url),
        screenshots: shots,
        icon: None,
    };
    if info.description.is_empty() {
        info.description = summary.as_str().unwrap_or("").into();
    }
    Some(info)
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
        let key = package
            .component_ids
            .iter()
            .find_map(|id| self.aliases.get(&format!("id:{}", stem(id))))
            .or_else(|| {
                self.aliases.get(&format!(
                    "source:{}:{}",
                    package.id.backend, package.id.name
                ))
            })
            .or_else(|| self.aliases.get(&format!("id:{}", stem(&package.id.name))))
            .or_else(|| {
                (package.id.backend != "flatpak")
                    .then(|| self.aliases.get(&format!("pkg:{}", package.id.name)))
                    .flatten()
            })?;
        self.apps.get(key)
    }
    pub fn enrich(&self, package: &mut Package) {
        if let Some(info) = self.find(package) {
            if package.icon.is_none() {
                package.icon.clone_from(&info.icon);
            }
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
        let Some(canonical) = keys
            .iter()
            .find_map(|key| self.aliases.get(key))
            .cloned()
            .or_else(|| keys.first().cloned())
        else {
            return;
        };
        if let Some(previous) = self.apps.get(&canonical) {
            info.fill_missing(previous);
        }
        self.apps.insert(canonical.clone(), info);
        for key in keys {
            self.aliases.insert(key, canonical.clone());
        }
    }
    fn resolve_icon(&self, name: &str, kind: &str) -> Option<PathBuf> {
        match kind {
            "remote" => web_url(name).map(PathBuf::from),
            "cached" => {
                if Path::new(name).components().count() != 1 || name == ".." {
                    return None;
                }
                let directory = self.metadata_dir.as_ref()?;
                for size in ["128x128", "64x64", "48x48"] {
                    let path = directory.join("icons").join(size).join(name);
                    if path.is_file() {
                        return Some(path);
                    }
                }
                None
            }
            "stock" | "local" => pkgdeck_core::backends::themed_icon(
                std::env::var_os("HOME").as_deref().map(Path::new),
                name,
            ),
            _ => None,
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
                    icon: component
                        .children()
                        .filter(|node| node.has_tag_name("icon"))
                        .find_map(|node| {
                            self.resolve_icon(
                                node.text()?,
                                node.attribute("type").unwrap_or("stock"),
                            )
                        }),
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
            let ids = std::iter::once(id.to_owned())
                .chain(
                    value["Launchable"]["desktop-id"]
                        .as_sequence()
                        .into_iter()
                        .flatten()
                        .filter_map(|id| id.as_str().map(str::to_owned)),
                )
                .collect();
            self.insert(
                ids,
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
                    icon: value["Icon"]["stock"]
                        .as_str()
                        .and_then(|name| self.resolve_icon(name, "stock"))
                        .or_else(|| {
                            value["Icon"]["cached"]
                                .as_sequence()
                                .into_iter()
                                .flatten()
                                .find_map(|icon| {
                                    self.resolve_icon(icon["name"].as_str()?, "cached")
                                })
                        }),
                    screenshots,
                },
            );
        }
    }
    fn desktop(&mut self, path: &Path) {
        let Ok(file) = fs::File::open(path) else {
            return;
        };
        let mut text = String::new();
        if file.take(1024 * 1024).read_to_string(&mut text).is_err() {
            return;
        }
        let mut fields = BTreeMap::new();
        let mut entry = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                entry = line == "[Desktop Entry]";
            } else if entry && !line.starts_with('#') {
                if let Some((key, value)) = line.split_once('=') {
                    fields.insert(key.trim(), value.trim());
                }
            }
        }
        if fields.get("Type") != Some(&"Application")
            || fields.get("Hidden") == Some(&"true")
            || fields.get("NoDisplay") == Some(&"true")
        {
            return;
        }
        let Some(name) = fields.get("Name").filter(|name| !name.is_empty()) else {
            return;
        };
        let Some(id) = path.file_stem().and_then(|name| name.to_str()) else {
            return;
        };
        let mut ids = vec![id.to_owned()];
        if let Some(flatpak) = fields.get("X-Flatpak") {
            ids.push((*flatpak).into());
        }
        self.insert(
            ids,
            vec![],
            AppInfo {
                icon: fields
                    .get("Icon")
                    .and_then(|name| self.resolve_icon(name, "stock")),
                name: (*name).into(),
                description: fields.get("Comment").unwrap_or(&"").to_string(),
                ..AppInfo::default()
            },
        );
        if let Some(snap) = fields.get("X-SnapInstanceName") {
            if let Some(key) = self.aliases.get(&format!("id:{id}")).cloned() {
                self.aliases.insert(format!("source:snap:{snap}"), key);
            }
        }
    }
    fn load(root: &Path, home: Option<&Path>) -> Self {
        let mut files = BTreeSet::new();
        let mut catalog = Self::default();
        let mut data_dirs = vec![
            root.join("usr/share"),
            root.join("usr/local/share"),
            root.join("var/lib/flatpak/exports/share"),
            root.join("var/lib/snapd/desktop"),
        ];
        if let Some(home) = home {
            data_dirs.push(home.join(".local/share"));
            data_dirs.push(home.join(".local/share/flatpak/exports/share"));
        }
        if root == Path::new("/") {
            if let Some(path) = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
            {
                data_dirs.push(path);
            }
            if let Some(paths) = std::env::var_os("XDG_DATA_DIRS") {
                data_dirs.extend(std::env::split_paths(&paths).filter(|p| p.is_absolute()));
            }
        }
        for dir in data_dirs {
            for file in entries(&dir.join("applications")) {
                if file
                    .extension()
                    .is_some_and(|extension| extension == "desktop")
                {
                    catalog.desktop(&file);
                }
            }
            collect_files(&dir.join("metainfo"), &mut files);
            collect_files(&dir.join("appdata"), &mut files);
        }
        for dir in [
            "usr/share/metainfo",
            "usr/share/appdata",
            "var/lib/swcatalog/xml",
            "var/cache/swcatalog/xml",
            "var/cache/swcatalog/yaml",
            "usr/share/swcatalog/xml",
            "usr/share/swcatalog/yaml",
            "var/cache/app-info/yaml",
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
                catalog.metadata_dir = path.parent().map(Path::to_path_buf);
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
        if [".xml", ".xml.gz", ".yml", ".yaml", ".yml.gz", ".yaml.gz"]
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

    #[test]
    fn appstream_icons_resolve_cached_remote_and_missing_assets() {
        let dir =
            std::env::temp_dir().join(format!("pkgdeck-catalog-icons-{}", std::process::id()));
        fs::create_dir_all(dir.join("icons/128x128")).unwrap();
        let icon = dir.join("icons/128x128/player.png");
        fs::write(&icon, "synthetic icon").unwrap();
        let mut catalog = Catalog {
            metadata_dir: Some(dir.clone()),
            ..Catalog::default()
        };
        catalog.xml(r#"<component type="desktop-application"><id>org.example.Player</id><name>Player</name><icon type="cached">player.png</icon></component>"#);
        let mut app = package("flatpak", "org.example.Player");
        catalog.enrich(&mut app);
        assert_eq!(app.icon, Some(icon));
        assert!(catalog.resolve_icon("../player.png", "cached").is_none());
        assert!(catalog.resolve_icon("missing.png", "cached").is_none());
        assert!(catalog
            .resolve_icon("http://example.invalid/icon.png", "remote")
            .is_none());
        assert_eq!(
            catalog.resolve_icon("https://example.invalid/icon.png", "remote"),
            Some(PathBuf::from("https://example.invalid/icon.png"))
        );
        fs::remove_dir_all(dir).unwrap();
    }

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
        assert!(catalog.apps.is_empty());
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
Launchable: {desktop-id: [editor-app.desktop]}
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
        assert_eq!(
            catalog.find(&package("apt", "editor-app")).unwrap().name,
            "Example Editor"
        );
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
        assert!(Catalog::load(&temp.0.join("absent"), None).apps.is_empty());
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
    fn refreshed_catalog_replaces_old_metadata_without_publishing_stale_loads() {
        let cache = CatalogCache::default();
        let first = cache.get(|| {
            let mut c = Catalog::default();
            c.xml(APP);
            c
        });
        assert_eq!(
            first.find(&package("apt", "player-bin")).unwrap().name,
            "Example Player"
        );
        cache.get(|| panic!("unchanged catalog must be reused"));
        cache.invalidate();
        assert!(cache.cached().is_none());
        let second = cache.get(|| {
            let mut c = Catalog::default();
            c.xml(&APP.replace("Example Player", "New Player"));
            c
        });
        assert_eq!(
            second.find(&package("apt", "player-bin")).unwrap().name,
            "New Player"
        );
        assert!(!Arc::ptr_eq(&first, &second));
        cache.invalidate();
        cache.get(|| {
            cache.invalidate();
            Catalog::default()
        });
        assert!(
            cache.cached().is_none(),
            "a refresh during parsing must not publish the superseded catalog"
        );
    }
    #[test]
    fn desktop_fallback_matches_exact_sources_and_appstream_enriches_all_aliases() {
        let temp = pkgdeck_tools::Temp::new();
        let dir = temp.0.join("var/lib/snapd/desktop/applications");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("player.desktop"), "[Desktop Entry]\nType=Application\nName=Desktop Player\nComment=Local description\nX-SnapInstanceName=player-snap\nX-Flatpak=org.example.Player\n[Desktop Action Other]\nName=Wrong action name\n").unwrap();
        for (name, text) in [
            ("hidden", "Type=Application\nHidden=true\nName=Hidden"),
            ("helper", "Type=Application\nNoDisplay=true\nName=Helper"),
            ("link", "Type=Link\nName=Link"),
            ("unnamed", "Type=Application"),
        ] {
            fs::write(
                dir.join(format!("{name}.desktop")),
                format!("[Desktop Entry]\n{text}"),
            )
            .unwrap();
        }
        let mut catalog = Catalog::load(&temp.0, None);
        assert_eq!(
            catalog.find(&package("snap", "player-snap")).unwrap().name,
            "Desktop Player"
        );
        assert!(catalog.find(&package("apt", "player-snap")).is_none());
        assert!(catalog.find(&package("apt", "hidden")).is_none());
        catalog.xml(APP);
        let info = catalog.find(&package("snap", "player-snap")).unwrap();
        assert_eq!(info.name, "Example Player");
        assert_eq!(
            info.screenshots.len(),
            1,
            "aliases must resolve the richer AppStream record"
        );
        catalog.desktop(&temp.0.join("absent.desktop"));
    }
    #[test]
    fn remote_providers_are_source_scoped_and_reject_foreign_identities() {
        let mut flatpak = package("flatpak", "org.example.Player");
        assert!(provider_url(&flatpak).is_none());
        flatpak.id.remote = Some("private".into());
        assert!(provider_url(&flatpak).is_none());
        flatpak.id.remote = Some("flathub".into());
        assert_eq!(
            provider_url(&flatpak).as_deref(),
            Some("https://flathub.org/api/v2/appstream/org.example.Player")
        );
        let json = r#"{"id":"org.example.Player","name":"Remote Player","summary":"Fallback","description":"<p>Play <em>media</em>.</p>","urls":{"homepage":"https://example.invalid"},"screenshots":[{"caption":"Library","sizes":[{"width":320,"src":"https://example.invalid/small.webp"},{"width":1280,"src":"https://example.invalid/large.webp"}]},{"sizes":[{"src":"file:///tmp/private"}]}]}"#;
        let info = provider_info(&flatpak, json).unwrap();
        assert_eq!(info.name, "Remote Player");
        assert_eq!(info.description, "Play media.");
        assert_eq!(
            info.screenshots[0].url,
            "https://example.invalid/large.webp"
        );
        assert_eq!(info.screenshots.len(), 1);
        assert!(provider_info(&package("flatpak", "foreign"), json).is_none());
        assert!(provider_info(&flatpak, "broken").is_none());
        let snap = package("snap", "player");
        assert_eq!(
            provider_url(&snap).as_deref(),
            Some("https://api.snapcraft.io/v2/snaps/info/player")
        );
        let info = provider_info(&snap, r#"{"snap":{"name":"player","title":"Snap Player","summary":"Summary","store-url":"https://example.invalid/store","media":[{"type":"icon","url":"https://example.invalid/icon"},{"type":"screenshot","url":"https://example.invalid/snap.png"}]}}"#).unwrap();
        assert_eq!(info.name, "Snap Player");
        assert_eq!(info.description, "Summary");
        assert_eq!(info.screenshots.len(), 1);
        assert!(
            provider_info(&package("snap", "other"), r#"{"snap":{"name":"player"}}"#).is_none()
        );
        for p in [
            package("snap", "../private"),
            package("snap", ""),
            package("npm", "player"),
            package("apt", "player"),
        ] {
            assert!(provider_url(&p).is_none());
        }
        assert_eq!(description("Rock & roll"), "Rock & roll");
        assert!(description("<broken>").is_empty());
    }
    #[test]
    fn detail_fallback_keeps_local_data_and_cancellation_stops_optional_requests() {
        let snap = package("snap", "player");
        let cancel = Cancellation::default();
        let local = AppInfo {
            name: "Local Player".into(),
            description: "Local description".into(),
            ..AppInfo::default()
        };
        let info = detail_info(&snap, local.clone(), &cancel, |_| r#"{"snap":{"name":"player","title":"Remote Player","media":[{"type":"screenshot","url":"https://example.invalid/player.png"}]}}"#.into()).unwrap();
        assert_eq!(info.name, "Local Player");
        assert_eq!(info.screenshots.len(), 1);
        detail_info(&snap, info, &cancel, |_| {
            panic!("complete local metadata needs no request")
        });
        let fallback =
            detail_info(&snap, local.clone(), &cancel, |_| "HTTP failure".into()).unwrap();
        assert_eq!(fallback.description, local.description);
        let unsupported = package("apt", "player");
        detail_info(&unsupported, local.clone(), &cancel, |_| {
            panic!("unsupported providers must stay local")
        });
        assert!(detail_info(&snap, local.clone(), &cancel, |_| {
            cancel.cancel();
            String::new()
        })
        .is_none());
        assert!(detail_info(&snap, local, &cancel, |_| panic!(
            "cancelled requests must not start"
        ))
        .is_none());
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
