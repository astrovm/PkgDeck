//! The icons of Mac apps, read from their bundles. An `.icns` file holds one
//! picture at several sizes, as PNGs in anything `iconutil` made. The window
//! only draws PNGs, so the one it needs is saved on its own.
use crate::cache::Store;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

const MAX_ICNS_BYTES: u64 = 64 * 1024 * 1024;
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
/// 128 pixels first, the smallest that stays sharp where the window draws
/// icons at their largest. Then each larger size, in case that one is missing.
const SIZES: [&[u8; 4]; 6] = [b"ic07", b"ic13", b"ic08", b"ic14", b"ic09", b"ic10"];

/// The PNG to show from an `.icns` file, if it holds one.
fn png_in_icns(icns: &[u8]) -> Option<&[u8]> {
    let mut rest = icns.strip_prefix(b"icns")?.get(4..)?;
    let mut pictures: Vec<(&[u8], &[u8])> = Vec::new();
    while let Some((header, after)) = rest.split_at_checked(8) {
        let (kind, length) = header.split_at(4);
        let length = u32::from_be_bytes(length.try_into().ok()?) as usize;
        let (body, next) = after.split_at_checked(length.checked_sub(8)?)?;
        if body.starts_with(PNG_MAGIC) {
            pictures.push((kind, body));
        }
        rest = next;
    }
    SIZES
        .iter()
        .find_map(|size| pictures.iter().find(|(kind, _)| kind == *size))
        .or(pictures.first())
        .map(|(_, body)| *body)
}

/// The saved PNG of the app at `bundle`, whose `Info.plist` is `plist`, kept
/// under `cache` until the app's icon changes.
pub(super) fn bundle_icon(bundle: &Path, plist: &Value, cache: &Path) -> Option<PathBuf> {
    let name = plist.get("CFBundleIconFile")?.as_str()?;
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let extension = if name.ends_with(".icns") { "" } else { ".icns" };
    let file = bundle
        .join("Contents/Resources")
        .join(format!("{name}{extension}"));
    let metadata = fs::metadata(&file).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_ICNS_BYTES {
        return None;
    }
    let digest = Sha256::digest(bundle.to_string_lossy().as_bytes());
    let saved = cache
        .join("app-icons")
        .join(format!("{}.png", &hex::encode(digest)[..24]));
    let current = fs::metadata(&saved)
        .and_then(|icon| Ok(icon.modified()? >= metadata.modified()?))
        .unwrap_or(false);
    if current {
        return Some(saved);
    }
    let icns = fs::read(&file).ok()?;
    let png = png_in_icns(&icns)?;
    fs::create_dir_all(saved.parent()?).ok()?;
    // Whole, then renamed: the window may be reading the old one.
    let partial = saved.with_extension("partial");
    fs::write(&partial, png).ok()?;
    fs::rename(&partial, &saved).ok()?;
    Some(saved)
}

/// [`bundle_icon`] in the user's cache.
pub(super) fn user_bundle_icon(bundle: &Path, plist: &Value) -> Option<PathBuf> {
    bundle_icon(bundle, plist, Store::user()?.dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut bytes = kind.to_vec();
        bytes.extend((body.len() as u32 + 8).to_be_bytes());
        bytes.extend(body);
        bytes
    }

    fn icns(entries: &[Vec<u8>]) -> Vec<u8> {
        let body = entries.concat();
        let mut bytes = b"icns".to_vec();
        bytes.extend((body.len() as u32 + 8).to_be_bytes());
        bytes.extend(body);
        bytes
    }

    fn png(tag: &[u8]) -> Vec<u8> {
        [PNG_MAGIC, tag].concat()
    }

    #[test]
    fn the_128_pixel_png_comes_first_then_the_next_sizes_up() {
        let old = entry(b"it32", b"not a png");
        let small = entry(b"ic11", &png(b"32"));
        let medium = entry(b"ic07", &png(b"128"));
        let large = entry(b"ic09", &png(b"512"));
        let all = icns(&[old.clone(), large.clone(), small.clone(), medium]);
        assert_eq!(png_in_icns(&all), Some(png(b"128").as_slice()));
        let big = icns(&[old.clone(), small.clone(), large]);
        assert_eq!(png_in_icns(&big), Some(png(b"512").as_slice()));
        // Any PNG beats none.
        let tiny = icns(&[old.clone(), small]);
        assert_eq!(png_in_icns(&tiny), Some(png(b"32").as_slice()));
        assert_eq!(png_in_icns(&icns(&[old])), None);
    }

    #[test]
    fn a_damaged_icns_has_no_png() {
        assert_eq!(png_in_icns(b"not an icon"), None);
        assert_eq!(png_in_icns(b"icns"), None);
        // An entry too short to hold its own header.
        let mut short = b"icns\0\0\0\0ic07".to_vec();
        short.extend(4u32.to_be_bytes());
        assert_eq!(png_in_icns(&short), None);
        // An entry longer than the file.
        let mut cut = b"icns\0\0\0\0ic07".to_vec();
        cut.extend(64u32.to_be_bytes());
        cut.extend(png(b"x"));
        assert_eq!(png_in_icns(&cut), None);
        // Stray bytes after the last whole entry change nothing.
        let mut trailing = icns(&[entry(b"ic07", &png(b"128"))]);
        trailing.extend(b"xx");
        assert_eq!(png_in_icns(&trailing), Some(png(b"128").as_slice()));
    }

    struct Bundle {
        root: PathBuf,
        path: PathBuf,
    }

    impl Bundle {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("pkgdeck-mac-icons-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let path = root.join("Fixture.app");
            fs::create_dir_all(path.join("Contents/Resources")).unwrap();
            Self { root, path }
        }
        fn icon(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let file = self.path.join("Contents/Resources").join(name);
            fs::write(&file, bytes).unwrap();
            file
        }
        fn read(&self, plist: Value) -> Option<PathBuf> {
            bundle_icon(&self.path, &plist, &self.root.join("cache"))
        }
    }

    impl Drop for Bundle {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_bundle_icon_is_saved_as_its_png() {
        let bundle = Bundle::new("saved");
        bundle.icon("AppIcon.icns", &icns(&[entry(b"ic07", &png(b"one"))]));
        // With or without the extension, as Info.plist files have it.
        for name in ["AppIcon", "AppIcon.icns"] {
            let saved = bundle.read(json!({"CFBundleIconFile": name})).unwrap();
            assert_eq!(fs::read(&saved).unwrap(), png(b"one"));
            assert_eq!(saved.extension().unwrap(), "png");
        }
    }

    #[test]
    fn a_saved_icon_lasts_until_the_app_has_a_newer_one() {
        let bundle = Bundle::new("newer");
        let file = bundle.icon("AppIcon.icns", &icns(&[entry(b"ic07", &png(b"one"))]));
        let plist = json!({"CFBundleIconFile": "AppIcon"});
        let saved = bundle.read(plist.clone()).unwrap();
        // Nothing reads the app's file again while the saved one is current.
        fs::write(&saved, png(b"kept")).unwrap();
        let later = fs::metadata(&saved).unwrap().modified().unwrap();
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(later - std::time::Duration::from_secs(60))
            .unwrap();
        assert_eq!(bundle.read(plist.clone()), Some(saved.clone()));
        assert_eq!(fs::read(&saved).unwrap(), png(b"kept"));
        // An update brings a newer file.
        bundle.icon("AppIcon.icns", &icns(&[entry(b"ic07", &png(b"two"))]));
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(later + std::time::Duration::from_secs(60))
            .unwrap();
        assert_eq!(bundle.read(plist), Some(saved.clone()));
        assert_eq!(fs::read(&saved).unwrap(), png(b"two"));
    }

    #[test]
    fn apps_without_a_usable_icon_have_none() {
        let bundle = Bundle::new("none");
        assert_eq!(bundle.read(json!({})), None);
        assert_eq!(bundle.read(json!({"CFBundleIconFile": 1})), None);
        assert_eq!(bundle.read(json!({"CFBundleIconFile": ""})), None);
        assert_eq!(
            bundle.read(json!({"CFBundleIconFile": "../../Other"})),
            None
        );
        // Named but missing, or only a folder of that name.
        assert_eq!(bundle.read(json!({"CFBundleIconFile": "Missing"})), None);
        fs::create_dir(bundle.path.join("Contents/Resources/Folder.icns")).unwrap();
        assert_eq!(bundle.read(json!({"CFBundleIconFile": "Folder"})), None);
        // Too big to read.
        let file = bundle.icon("Huge.icns", b"");
        fs::File::options()
            .write(true)
            .open(file)
            .unwrap()
            .set_len(MAX_ICNS_BYTES + 1)
            .unwrap();
        assert_eq!(bundle.read(json!({"CFBundleIconFile": "Huge"})), None);
        // Holds no PNG: nothing is saved.
        bundle.icon("Old.icns", &icns(&[entry(b"it32", b"old")]));
        assert_eq!(bundle.read(json!({"CFBundleIconFile": "Old"})), None);
        assert!(!bundle.root.join("cache").exists());
    }

    #[test]
    fn a_cache_that_cant_be_written_leaves_the_app_without_an_icon() {
        let bundle = Bundle::new("unwritable");
        bundle.icon("AppIcon.icns", &icns(&[entry(b"ic07", &png(b"one"))]));
        let plist = json!({"CFBundleIconFile": "AppIcon"});
        // A file where the cache's folder should be.
        fs::write(bundle.root.join("cache"), b"").unwrap();
        assert_eq!(bundle.read(plist), None);
    }

    #[test]
    fn the_user_cache_holds_nothing_without_an_icon_to_save() {
        assert_eq!(
            user_bundle_icon(Path::new("/nonexistent"), &json!({})),
            None
        );
    }
}
