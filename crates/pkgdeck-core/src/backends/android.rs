//! What the Android SDK keeps for emulators and nothing uses: system
//! images no emulator starts from, and emulators that can't start because
//! their settings are gone. Emulators made by Gradle count as users too.
use super::*;
use crate::{disk, host::Host};
use dev_caches::shown;
use std::{fs, path::Path};

/// How deep emulator settings are looked for: Gradle keeps its own
/// emulators a folder down.
const AVD_DEPTH: usize = 2;

fn var(host: &Host, name: &str) -> Option<PathBuf> {
    host.var(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// The SDK: `ANDROID_HOME`, `ANDROID_SDK_ROOT`, or where Android Studio
/// puts it.
fn sdk(host: &Host) -> Option<PathBuf> {
    var(host, "ANDROID_HOME")
        .or_else(|| var(host, "ANDROID_SDK_ROOT"))
        .or_else(|| {
            let home = var(host, "HOME")?;
            Some(if cfg!(target_os = "macos") {
                home.join("Library/Android/sdk")
            } else {
                home.join("Android/Sdk")
            })
        })
}

/// Where the emulator keeps its own settings, and where emulators live.
fn user_home(host: &Host) -> Option<PathBuf> {
    var(host, "ANDROID_USER_HOME").or_else(|| Some(var(host, "HOME")?.join(".android")))
}
fn avds(host: &Host) -> Option<PathBuf> {
    var(host, "ANDROID_AVD_HOME").or_else(|| Some(user_home(host)?.join("avd")))
}

/// `key=value` lines.
fn settings(path: &Path) -> Vec<(String, String)> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect()
}

/// Every emulator folder (`*.avd`), down to [`AVD_DEPTH`].
fn avd_folders(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut found = vec![];
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if path.extension().is_some_and(|extension| extension == "avd") {
            found.push(path);
        } else if depth + 1 < AVD_DEPTH {
            found.extend(avd_folders(&path, depth + 1));
        }
    }
    found
}

/// The system images emulators start from, as `android-36/google_apis/x86_64`.
fn images_in_use(host: &Host) -> Vec<String> {
    let Some(avds) = avds(host) else {
        return vec![];
    };
    avd_folders(&avds, 0)
        .iter()
        .flat_map(|avd| settings(&avd.join("config.ini")))
        .filter(|(key, _)| key.starts_with("image.sysdir."))
        .filter_map(|(_, value)| {
            value
                .trim_end_matches('/')
                .strip_prefix("system-images/")
                .map(str::to_owned)
        })
        .collect()
}

/// Installed system images, as `android-36/google_apis/x86_64`.
fn images(host: &Host) -> Vec<String> {
    let Some(root) = sdk(host).map(|sdk| sdk.join("system-images")) else {
        return vec![];
    };
    let children = |dir: &Path| -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        names.sort();
        names
    };
    let mut found = vec![];
    for api in children(&root) {
        for tag in children(&root.join(&api)) {
            for abi in children(&root.join(&api).join(&tag)) {
                found.push(format!("{api}/{tag}/{abi}"));
            }
        }
    }
    found
}

/// What the image's package calls itself: "Google APIs Intel x86_64 Atom
/// System Image for Android 36".
fn image_name(folder: &Path, image: &str) -> String {
    let package = fs::read_to_string(folder.join("package.xml")).unwrap_or_default();
    let tag = |name: &str| {
        let open = format!("<{name}>");
        let start = package.find(&open)? + open.len();
        let end = package[start..].find('<')? + start;
        Some(package[start..end].trim().to_owned()).filter(|text| !text.is_empty())
    };
    match (tag("display-name"), tag("api-level")) {
        (Some(name), Some(api)) => format!("{name} for Android {api}"),
        _ => format!("Android system image {}", image.replace('/', " ")),
    }
}

fn id(key: String) -> CleanupId {
    CleanupId {
        backend: "dev-caches".into(),
        key,
    }
}

/// The task for the system image `image`, when no emulator uses it.
pub(super) fn image_item(host: &Host, image: &str, cancel: &Cancellation) -> Option<CleanupItem> {
    let parts: Vec<_> = image.split('/').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || part.starts_with('.'))
        || images_in_use(host).iter().any(|used| used == image)
    {
        return None;
    }
    let folder = sdk(host)?.join("system-images").join(image);
    if !fs::symlink_metadata(&folder).is_ok_and(|metadata| metadata.is_dir()) {
        return None;
    }
    Some(CleanupItem {
        id: id(format!("android-image:{image}")),
        kind: CleanupKind::Unused,
        title: image_name(&folder, image),
        summary: "No emulator uses it".into(),
        preview: format!(
            "Deletes this system image. Android Studio downloads it again when an emulator needs it.\n{}",
            shown(host, &folder)
        ),
        bytes: Some(disk::usage(&folder, cancel)),
    })
}

/// An emulator that can't start: its folder has no settings, or its entry
/// points at a folder that's gone. The entry and the folder both go.
struct Broken {
    entry: Option<PathBuf>,
    folder: Option<PathBuf>,
}

fn broken(host: &Host, name: &str) -> Option<Broken> {
    if name.is_empty() || name.starts_with('.') || name.contains('/') {
        return None;
    }
    let avds = avds(host)?;
    let entry = avds.join(format!("{name}.ini"));
    let pointed = settings(&entry)
        .into_iter()
        .find(|(key, _)| key == "path")
        .map(|(_, path)| PathBuf::from(path));
    let folder = pointed.unwrap_or_else(|| avds.join(format!("{name}.avd")));
    let folder_there = fs::symlink_metadata(&folder).is_ok_and(|metadata| metadata.is_dir());
    if folder_there && folder.join("config.ini").is_file() {
        return None;
    }
    let found = Broken {
        entry: entry.is_file().then_some(entry),
        // Only a folder where emulators live, never one an entry points
        // anywhere else.
        folder: (folder_there && folder.parent() == Some(avds.as_path())).then_some(folder),
    };
    (found.entry.is_some() || found.folder.is_some()).then_some(found)
}

/// The task for the emulator `name`, when it can't start.
pub(super) fn avd_item(host: &Host, name: &str, cancel: &Cancellation) -> Option<CleanupItem> {
    let found = broken(host, name)?;
    let listed: Vec<_> = [&found.entry, &found.folder]
        .into_iter()
        .flatten()
        .map(|path| shown(host, path))
        .collect();
    Some(CleanupItem {
        id: id(format!("android-avd:{name}")),
        kind: CleanupKind::Unused,
        title: format!("Broken emulator {name}"),
        summary: "Its settings are missing, so it can't start".into(),
        preview: format!("Deletes what is left of it.\n{}", listed.join("\n")),
        bytes: Some(
            [&found.entry, &found.folder]
                .into_iter()
                .flatten()
                .map(|path| disk::usage(path, cancel))
                .sum(),
        ),
    })
}

/// Every unused system image and broken emulator.
pub(super) fn items(host: &Host, cancel: &Cancellation) -> Vec<CleanupItem> {
    let mut found: Vec<_> = images(host)
        .iter()
        .filter_map(|image| image_item(host, image, cancel))
        .collect();
    let mut names: Vec<_> = avds(host)
        .into_iter()
        .flat_map(|avds| fs::read_dir(avds).into_iter().flatten().flatten())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| {
            name.strip_suffix(".ini")
                .or_else(|| name.strip_suffix(".avd"))
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names.dedup();
    found.extend(names.iter().filter_map(|name| avd_item(host, name, cancel)));
    found
}

/// Delete what the task `key` names, as a fresh look finds it.
pub(super) fn remove(host: &Host, key: &str) -> Result<(), EngineError> {
    let failed = |path: &Path, error: std::io::Error| {
        EngineError::Execution(ExecutionError::Io(format!(
            "Couldn't remove {}: {error}",
            shown(host, path)
        )))
    };
    if let Some(image) = key.strip_prefix("android-image:") {
        let root = sdk(host).ok_or(EngineError::NotFound)?.join("system-images");
        let folder = root.join(image);
        disk::remove(&folder).map_err(|error| failed(&folder, error))?;
        // The API and tag folders, once they hold nothing else.
        for parent in folder.ancestors().skip(1).take(2) {
            if parent != root {
                let _ = fs::remove_dir(parent);
            }
        }
        return Ok(());
    }
    let name = key.strip_prefix("android-avd:").ok_or(EngineError::NotFound)?;
    let found = broken(host, name).ok_or(EngineError::NotFound)?;
    for path in [found.entry, found.folder].into_iter().flatten() {
        disk::remove(&path).map_err(|error| failed(&path, error))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Runtime;
    use std::collections::BTreeMap;

    fn scratch(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("pkgdeck-android-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }
    fn host(home: &Path, extra: &[(&str, &Path)]) -> Host {
        let mut env = BTreeMap::from([("HOME".into(), home.as_os_str().to_owned())]);
        for (name, value) in extra {
            env.insert(name.into(), value.as_os_str().to_owned());
        }
        Host::new(Runtime::Native, env)
    }
    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn image(sdk: &Path, image: &str, display: &str, api: &str) {
        let folder = sdk.join("system-images").join(image);
        write(
            &folder.join("package.xml"),
            &format!("<ns2:repository><localPackage path=\"x\"><type-details><api-level>{api}</api-level></type-details><display-name>{display}</display-name></localPackage></ns2:repository>"),
        );
        write(&folder.join("system.img"), &"x".repeat(8192));
    }
    fn avd(avds: &Path, name: &str, image: &str) {
        write(
            &avds.join(format!("{name}.ini")),
            &format!("avd.ini.encoding=UTF-8\npath={}\ntarget=android-36\n", avds.join(format!("{name}.avd")).display()),
        );
        write(
            &avds.join(format!("{name}.avd/config.ini")),
            &format!("hw.ramSize=2048\nimage.sysdir.1=system-images/{image}/\n"),
        );
    }

    #[test]
    fn images_no_emulator_uses_and_broken_emulators_are_offered_and_deleted() {
        let home = scratch("offer");
        let sdk = home.join(if cfg!(target_os = "macos") {
            "Library/Android/sdk"
        } else {
            "Android/Sdk"
        });
        let avds = home.join(".android/avd");
        image(&sdk, "android-36/google_apis/x86_64", "Google APIs Intel x86_64 Atom System Image", "36");
        image(&sdk, "android-36/google-tv/x86_64", "Google TV Intel x86_64 Atom System Image", "36");
        image(&sdk, "android-37.0/google_apis/x86_64", "Google APIs", "37.0");
        image(&sdk, "android-35/default/arm64-v8a", "", "");
        avd(&avds, "tv", "android-36/google-tv/x86_64");
        // Gradle's own emulators use images too.
        write(
            &avds.join("gradle-managed/dev.avd/config.ini"),
            "image.sysdir.1=system-images/android-37.0/google_apis/x86_64/\n",
        );
        // Left behind: a folder with no settings, and an entry for a
        // folder that's gone.
        write(&avds.join("ambient.avd/emulator-user.ini"), "window.x = 100\n");
        write(&avds.join("gone.ini"), &format!("path={}\n", avds.join("gone.avd").display()));
        // An entry that points elsewhere: only the entry goes.
        let elsewhere = home.join("elsewhere.avd");
        fs::create_dir_all(&elsewhere).unwrap();
        write(&avds.join("moved.ini"), &format!("path={}\n", elsewhere.display()));
        let host = host(&home, &[]);
        let cancel = Cancellation::default();
        let found = items(&host, &cancel);
        let keys: Vec<_> = found.iter().map(|item| item.id.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "android-image:android-35/default/arm64-v8a",
                "android-image:android-36/google_apis/x86_64",
                "android-avd:ambient",
                "android-avd:gone",
                "android-avd:moved",
            ]
        );
        assert_eq!(found[0].title, "Android system image android-35 default arm64-v8a");
        assert_eq!(
            found[1].title,
            "Google APIs Intel x86_64 Atom System Image for Android 36"
        );
        assert!(found[1].bytes.unwrap() >= 8192);
        assert!(found[1].preview.ends_with("/system-images/android-36/google_apis/x86_64"));
        assert_eq!(found[2].title, "Broken emulator ambient");
        assert_eq!(found[2].preview, "Deletes what is left of it.\n~/.android/avd/ambient.avd");
        assert_eq!(found[3].preview, "Deletes what is left of it.\n~/.android/avd/gone.ini");
        assert_eq!(found[4].preview, "Deletes what is left of it.\n~/.android/avd/moved.ini");
        for item in &found {
            remove(&host, &item.id.key).unwrap();
        }
        assert!(!sdk.join("system-images/android-36/google_apis").exists());
        assert!(!sdk.join("system-images/android-35").exists());
        assert!(sdk.join("system-images/android-36/google-tv/x86_64").exists());
        assert!(!avds.join("ambient.avd").exists() && !avds.join("gone.ini").exists());
        assert!(elsewhere.exists() && !avds.join("moved.ini").exists());
        assert!(avds.join("tv.avd/config.ini").exists());
        assert!(items(&host, &cancel).is_empty());
        // Gone, or never there, there's nothing to do.
        for key in ["android-avd:tv", "android-avd:../x", "android-avd:", "other"] {
            assert!(matches!(remove(&host, key), Err(EngineError::NotFound)), "{key}");
        }
        for image in ["android-36/google-tv/x86_64", "a/b", "a/../b", "a/b/.c"] {
            assert!(image_item(&host, image, &cancel).is_none(), "{image}");
        }
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn the_sdk_and_emulators_are_where_android_looks() {
        let home = Path::new("/home/me");
        let places = |extra: &[(&str, &Path)]| {
            let host = host(home, extra);
            (sdk(&host), avds(&host))
        };
        let default_sdk = if cfg!(target_os = "macos") {
            "/home/me/Library/Android/sdk"
        } else {
            "/home/me/Android/Sdk"
        };
        assert_eq!(
            places(&[]),
            (Some(default_sdk.into()), Some("/home/me/.android/avd".into()))
        );
        assert_eq!(
            places(&[
                ("ANDROID_SDK_ROOT", Path::new("/root-sdk")),
                ("ANDROID_USER_HOME", Path::new("/user")),
            ]),
            (Some("/root-sdk".into()), Some("/user/avd".into()))
        );
        assert_eq!(
            places(&[
                ("ANDROID_HOME", Path::new("/sdk")),
                ("ANDROID_SDK_ROOT", Path::new("/root-sdk")),
                ("ANDROID_AVD_HOME", Path::new("/avds")),
            ]),
            (Some("/sdk".into()), Some("/avds".into()))
        );
        let homeless = Host::new(Runtime::Native, BTreeMap::new());
        assert_eq!((sdk(&homeless), avds(&homeless)), (None, None));
        let cancel = Cancellation::default();
        assert!(items(&homeless, &cancel).is_empty());
        assert!(matches!(
            remove(&homeless, "android-image:a/b/c"),
            Err(EngineError::NotFound)
        ));
    }
}
