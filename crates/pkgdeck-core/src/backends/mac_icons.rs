//! The icons of Mac apps, read from their bundles. An `.icns` file holds one
//! picture at several sizes, as PNGs in anything `iconutil` made. The window
//! only draws PNGs, so the one it needs is saved on its own. Older files keep
//! their pictures in formats only macOS reads, so `sips` converts those.
use crate::{
    cache::Store,
    host::Host,
    process::{Cancellation, Limits},
};
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
/// As large as a converted icon needs, however large the original is.
const CONVERTED_PIXELS: &str = "256";

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

/// Write the `.icns` file `from` as a PNG at `to`, with `sips`, macOS's own
/// tool, at `tool`.
fn convert_with(tool: &Path, from: &Path, to: &Path, cancel: &Cancellation) -> bool {
    let args = [
        "-s".as_ref(),
        "format".as_ref(),
        "png".as_ref(),
        "-Z".as_ref(),
        CONVERTED_PIXELS.as_ref(),
        from.as_os_str(),
        "--out".as_ref(),
        to.as_os_str(),
    ]
    .map(ToOwned::to_owned);
    Host::current()
        .read(tool, &args, Limits::default(), cancel)
        .is_ok_and(|result| result.code == Some(0))
}

/// The saved PNG of the app at `bundle`, whose `Info.plist` is `plist`, kept
/// under `cache` until the app's icon changes. `convert` handles a file that
/// holds no PNG.
pub(super) fn bundle_icon(
    bundle: &Path,
    plist: &Value,
    cache: &Path,
    convert: impl Fn(&Path, &Path) -> bool,
) -> Option<PathBuf> {
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
    fs::create_dir_all(saved.parent()?).ok()?;
    // Whole, then renamed: the window may be reading the old one.
    let partial = saved.with_extension("partial.png");
    let written = match png_in_icns(&icns) {
        Some(png) => fs::write(&partial, png).is_ok(),
        None => {
            convert(&file, &partial)
                && fs::read(&partial).is_ok_and(|bytes| bytes.starts_with(PNG_MAGIC))
        }
    };
    if !written || fs::rename(&partial, &saved).is_err() {
        let _ = fs::remove_file(&partial);
        return None;
    }
    Some(saved)
}

/// [`bundle_icon`] with macOS's own converter.
fn bundle_icon_with_sips(
    bundle: &Path,
    plist: &Value,
    cache: &Path,
    cancel: &Cancellation,
) -> Option<PathBuf> {
    bundle_icon(bundle, plist, cache, |from, to| {
        convert_with(Path::new("/usr/bin/sips"), from, to, cancel)
    })
}

/// [`bundle_icon_with_sips`] in the user's cache.
pub(super) fn user_bundle_icon(
    bundle: &Path,
    plist: &Value,
    cancel: &Cancellation,
) -> Option<PathBuf> {
    bundle_icon_with_sips(bundle, plist, Store::user()?.dir(), cancel)
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
            self.read_with(plist, |_, _| false)
        }
        fn read_with(
            &self,
            plist: Value,
            convert: impl Fn(&Path, &Path) -> bool,
        ) -> Option<PathBuf> {
            bundle_icon(&self.path, &plist, &self.root.join("cache"), convert)
        }
        fn saved_files(&self) -> usize {
            fs::read_dir(self.root.join("cache/app-icons")).map_or(0, Iterator::count)
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
        assert!(!bundle.root.join("cache").exists());
    }

    #[test]
    fn a_name_only_reaches_the_apps_own_resources() {
        let bundle = Bundle::new("names");
        let icon = icns(&[entry(b"ic07", &png(b"one"))]);
        // Real files, so only the name can be what turns them down.
        bundle.icon(".icns", &icon);
        fs::write(bundle.path.join("Contents/Other.icns"), &icon).unwrap();
        assert_eq!(bundle.read(json!({"CFBundleIconFile": ""})), None);
        assert_eq!(bundle.read(json!({"CFBundleIconFile": "../Other"})), None);
    }

    #[test]
    fn an_icon_file_up_to_64_mib_is_read_and_a_larger_one_is_not() {
        const LIMIT: u64 = 64 * 1024 * 1024;
        let bundle = Bundle::new("limit");
        let plist = json!({"CFBundleIconFile": "Big"});
        // A real PNG, then an entry of nothing, to reach each size.
        let sized = |size: u64| {
            let mut bytes = icns(&[entry(b"ic07", &png(b"one"))]);
            let padding = size - bytes.len() as u64;
            bytes.extend(b"pad ");
            bytes.extend((padding as u32).to_be_bytes());
            let file = bundle.icon("Big.icns", &bytes);
            fs::File::options()
                .write(true)
                .open(file)
                .unwrap()
                .set_len(size)
                .unwrap();
            let _ = fs::remove_dir_all(bundle.root.join("cache"));
        };
        sized(LIMIT);
        assert!(bundle.read(plist.clone()).is_some());
        sized(LIMIT + 1);
        assert_eq!(bundle.read(plist), None);
    }

    #[test]
    fn an_icns_without_a_png_is_converted() {
        let bundle = Bundle::new("converted");
        let file = bundle.icon("Old.icns", &icns(&[entry(b"it32", b"old")]));
        let plist = json!({"CFBundleIconFile": "Old"});
        let saved = bundle
            .read_with(plist, |from, to| {
                assert_eq!(from, file);
                fs::write(to, png(b"converted")).is_ok()
            })
            .unwrap();
        assert_eq!(fs::read(&saved).unwrap(), png(b"converted"));
        assert_eq!(bundle.saved_files(), 1);
    }

    #[test]
    fn a_conversion_that_fails_saves_nothing() {
        let bundle = Bundle::new("unconverted");
        bundle.icon("Old.icns", &icns(&[entry(b"it32", b"old")]));
        let plist = json!({"CFBundleIconFile": "Old"});
        // The tool failed, after writing part of a file.
        let failed = |_: &Path, to: &Path| {
            fs::write(to, b"part").unwrap();
            false
        };
        assert_eq!(bundle.read_with(plist.clone(), failed), None);
        assert_eq!(bundle.saved_files(), 0);
        // The tool said it worked, but what it wrote isn't a PNG.
        let wrong = |_: &Path, to: &Path| fs::write(to, b"not a png").is_ok();
        assert_eq!(bundle.read_with(plist.clone(), wrong), None);
        assert_eq!(bundle.saved_files(), 0);
        // The tool said it worked and wrote nothing.
        assert_eq!(bundle.read_with(plist, |_, _| true), None);
        assert_eq!(bundle.saved_files(), 0);
    }

    #[test]
    fn what_sips_leaves_behind_is_a_png_or_nothing() {
        let bundle = Bundle::new("sips");
        bundle.icon("Old.icns", &icns(&[entry(b"it32", b"old")]));
        let found = bundle_icon_with_sips(
            &bundle.path,
            &json!({"CFBundleIconFile": "Old"}),
            &bundle.root.join("cache"),
            &Cancellation::default(),
        );
        // A Mac's `sips` makes a PNG even from these damaged bytes. Elsewhere
        // there's no `sips` at all.
        match found {
            Some(saved) => {
                assert!(fs::read(saved).unwrap().starts_with(PNG_MAGIC));
                assert_eq!(bundle.saved_files(), 1);
            }
            None => assert_eq!(bundle.saved_files(), 0),
        }
    }

    #[test]
    fn a_conversion_worked_only_if_the_tool_says_so() {
        let cancel = Cancellation::default();
        let convert =
            |tool: &str| convert_with(Path::new(tool), Path::new("a"), Path::new("b"), &cancel);
        assert!(convert("/usr/bin/true"));
        assert!(!convert("/usr/bin/false"));
        // A computer without the tool, such as anything but a Mac.
        assert!(!convert("/nonexistent/sips"));
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
            user_bundle_icon(
                Path::new("/nonexistent"),
                &json!({}),
                &Cancellation::default()
            ),
            None
        );
    }
}
