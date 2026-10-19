//! Icons and AppStream ids for installed packages, read from the system's own metadata.
use super::*;

/// Local application icons, resolved without network access for installed
/// packages only. Remote catalog entries never carry icons: fetching them
/// would add a download cache with eviction semantics on top of every
/// backend. Names that cannot resolve keep the generic source icon.
fn icon_file(dir: &std::path::Path, stem: &str) -> Option<PathBuf> {
    for extension in ["png", "svg", "xpm"] {
        let candidate = dir.join(format!("{stem}.{extension}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Installed snaps expose their icon beside the desktop entry. `sysroot`
/// is `/` on a real system and a fixture directory in tests.
pub(super) fn snap_icon(sysroot: &std::path::Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return None;
    }
    icon_file(
        &sysroot.join("snap").join(name).join("current/meta/gui"),
        "icon",
    )
}

/// Installed Flatpak applications export icons per installation scope.
/// `export_roots` holds the `exports` directories, user scope first.
pub(super) fn flatpak_icon(export_roots: &[PathBuf], app_id: &str) -> Option<PathBuf> {
    if !flatpak_id(app_id) {
        return None;
    }
    for root in export_roots {
        for size in ["128x128", "64x64", "48x48", "32x32"] {
            let dir = root.join("share/icons/hicolor").join(size).join("apps");
            if let Some(icon) = icon_file(&dir, app_id) {
                return Some(icon);
            }
        }
    }
    None
}

/// Map installed Debian packages to their first desktop entry by scanning
/// the dpkg file lists once per query. File names are `<package>.list` or
/// `<package>:<arch>.list`; only GUI applications resolve.
pub(super) fn apt_desktop_map(
    info_dir: &std::path::Path,
) -> std::collections::BTreeMap<String, PathBuf> {
    let mut map = std::collections::BTreeMap::new();
    let entries = std::fs::read_dir(info_dir).into_iter().flatten().flatten();
    for entry in entries {
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = file.strip_suffix(".list") else {
            continue;
        };
        let package = stem.split_once(':').map_or(stem, |(name, _)| name);
        if package.is_empty() || map.contains_key(package) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        if let Some(desktop) = content.lines().find(|line| {
            line.strip_prefix("/usr/share/applications/")
                .is_some_and(|path| path.ends_with(".desktop"))
        }) {
            map.insert(package.to_owned(), PathBuf::from(desktop));
        }
    }
    map
}

/// Resolve a desktop entry's `Icon` value: absolute paths are used directly
/// while bare names search the icon theme. `home` selects the user theme
/// directory and is `None` when the sanitized environment hides it.
pub(super) fn desktop_icon(
    home: Option<&std::path::Path>,
    desktop: &std::path::Path,
) -> Option<PathBuf> {
    let content = std::fs::read_to_string(host_metadata_path(desktop)).ok()?;
    let name = content
        .lines()
        .find_map(|line| line.strip_prefix("Icon="))?
        .trim();
    themed_icon(home, name)
}

pub(super) fn host_metadata_path(path: &std::path::Path) -> PathBuf {
    static HOST: std::sync::OnceLock<Host> = std::sync::OnceLock::new();
    HOST.get_or_init(Host::current).filesystem_path(path)
}

/// Resolve an exact desktop/AppStream icon name without network requests.
pub fn themed_icon(home: Option<&std::path::Path>, name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    let path = PathBuf::from(name);
    if path.is_absolute() {
        let path = host_metadata_path(&path);
        return path.is_file().then_some(path);
    }
    if name.contains('/') {
        return None;
    }
    let mut dirs = vec![
        host_metadata_path(std::path::Path::new("/usr/local/share/icons")),
        host_metadata_path(std::path::Path::new("/usr/share/icons")),
    ];
    if let Some(home) = home {
        dirs.insert(0, home.join(".local/share/icons"));
    }
    for dir in &dirs {
        for size in ["128x128", "64x64", "48x48", "32x32", "scalable"] {
            if let Some(icon) = icon_file(&dir.join("hicolor").join(size).join("apps"), name) {
                return Some(icon);
            }
        }
    }
    icon_file(
        &host_metadata_path(std::path::Path::new("/usr/share/pixmaps")),
        name,
    )
}

/// Strip one trailing `.desktop` suffix so DEP-11 component ids
/// (`org.mozilla.firefox.desktop`), Flatpak app ids (`org.mozilla.firefox`),
/// and snap desktop basenames (`firefox.desktop`) share one namespace.
/// Empty stems are `None` so they never join unrelated rows.
pub(super) fn component_stem(id: &str) -> Option<String> {
    let stem = id.strip_suffix(".desktop").unwrap_or(id).trim();
    (!stem.is_empty()).then(|| stem.to_owned())
}

/// Map installed Debian packages to their AppStream component-id stems from
/// the DEP-11 YAML in `yaml_dir`. Decompressing it takes a few hundred
/// milliseconds on every list, so the map is kept in the user's cache.
pub(super) fn dep11_component_ids(
    yaml_dir: &std::path::Path,
) -> std::collections::BTreeMap<String, Vec<String>> {
    dep11_cached(crate::cache::Store::user().as_ref(), yaml_dir, dep11_scan)
}

/// [`dep11_component_ids`] answered from `store` while the folder and every
/// file its entries link to (APT's lists, refreshed in place) are unchanged.
pub(super) fn dep11_cached(
    store: Option<&crate::cache::Store>,
    yaml_dir: &std::path::Path,
    scan: impl FnOnce(&[PathBuf]) -> std::collections::BTreeMap<String, Vec<String>>,
) -> std::collections::BTreeMap<String, Vec<String>> {
    use crate::cache::Watch;
    let Ok(entries) = std::fs::read_dir(yaml_dir) else {
        return std::collections::BTreeMap::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "gz"))
        .collect();
    files.sort();
    let folder = std::fs::canonicalize(yaml_dir).unwrap_or_else(|_| yaml_dir.to_owned());
    let mut watches = vec![Watch::file(yaml_dir), Watch::tree(folder, 1)];
    // A dangling link is watched where it points, so its target appearing
    // counts as a change.
    watches.extend(files.iter().map(|file| {
        Watch::file(
            std::fs::canonicalize(file)
                .or_else(|_| std::fs::read_link(file).map(|target| yaml_dir.join(target)))
                .unwrap_or_else(|_| file.clone()),
        )
    }));
    let encoded = crate::cache::read_through(
        store,
        "dep11",
        "components",
        &[&yaml_dir.to_string_lossy()],
        &watches,
        &[],
        || serde_json::to_vec(&scan(&files)),
    )
    .unwrap_or_default();
    serde_json::from_slice(&encoded).unwrap_or_default()
}

/// Scan DEP-11 YAML (`<id>.yml.gz`) as a gzip line stream. Documents are
/// `---`-separated with top-level `ID:`/`Package:` scalars; a package can
/// own several components. Unreadable files map nothing.
pub(super) fn dep11_scan(files: &[PathBuf]) -> std::collections::BTreeMap<String, Vec<String>> {
    use std::io::{BufRead, BufReader};
    let mut map: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for path in files {
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        let decoder = BufReader::new(flate2::read::GzDecoder::new(file));
        let mut id: Option<String> = None;
        let mut package: Option<String> = None;
        let mut flush = |id: &mut Option<String>, package: &mut Option<String>| {
            if let (Some(id), Some(package)) = (id.take(), package.take()) {
                if let Some(stem) = component_stem(&id) {
                    map.entry(package).or_default().push(stem);
                }
            }
        };
        for line in decoder.lines() {
            let Ok(line) = line else {
                break;
            };
            if line == "---" {
                flush(&mut id, &mut package);
            } else if let Some(value) = line.strip_prefix("ID: ") {
                id = Some(value.trim().to_owned());
            } else if let Some(value) = line.strip_prefix("Package: ") {
                package = Some(value.trim().to_owned());
            }
        }
        flush(&mut id, &mut package);
    }
    for ids in map.values_mut() {
        ids.sort();
        ids.dedup();
    }
    map
}

/// Desktop-id stems shipped by an installed snap (`meta/gui/*.desktop`).
/// CLI-only snaps expose no desktop entry and group with nothing.
pub(super) fn snap_desktop_ids(sysroot: &std::path::Path, name: &str) -> Vec<String> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return vec![];
    }
    let gui = sysroot.join("snap").join(name).join("current/meta/gui");
    let Ok(entries) = std::fs::read_dir(&gui) else {
        return vec![];
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "desktop"))
        .filter_map(|entry| component_stem(&entry.file_name().to_string_lossy()))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}
