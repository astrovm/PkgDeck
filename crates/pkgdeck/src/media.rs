//! App icons and screenshots from the web. Each HTTPS image downloads once
//! with curl, on its own thread, and stays on disk for a week under
//! `~/.cache/pkgdeck/media`, which is kept under 64 MB.

use eframe::egui::{
    self,
    load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError},
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

const WEEK: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const FILE_LIMIT: u64 = 16 * 1024 * 1024;
const CACHE_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Clone)]
enum Entry {
    Loading,
    Ready(Arc<[u8]>),
    Failed(String),
}

pub struct WebLoader {
    directory: Option<PathBuf>,
    curl: PathBuf,
    entries: Arc<Mutex<HashMap<String, Entry>>>,
}

/// The cached file for `url`.
fn cache_file(directory: &Path, url: &str) -> PathBuf {
    directory.join(hex::encode(Sha256::digest(url.as_bytes())))
}

fn fresh(path: &Path, now: SystemTime) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .is_ok_and(|modified| now.duration_since(modified).unwrap_or_default() < WEEK)
}

/// Drops expired files, then the oldest until the folder fits its limit.
pub fn trim(directory: &Path, now: SystemTime, limit: u64) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            let modified = meta.modified().ok()?;
            meta.is_file().then(|| (modified, meta.len(), entry.path()))
        })
        .collect();
    files.sort();
    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    for (modified, size, path) in files {
        let expired = now.duration_since(modified).unwrap_or_default() >= WEEK;
        if (expired || total > limit) && std::fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
}

/// Downloads `url` into `target`: HTTPS only, within 15 seconds and 16 MB.
fn download(curl: &Path, url: &str, target: &Path) -> Result<Vec<u8>, String> {
    let partial = target.with_extension("partial");
    let status = Command::new(curl)
        .args([
            "--silent",
            "--fail",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-time",
            "15",
            "--max-filesize",
        ])
        .arg(FILE_LIMIT.to_string())
        .arg("--output")
        .arg(&partial)
        .arg("--")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("couldn't download {url}"));
    }
    let bytes = std::fs::read(&partial).map_err(|error| error.to_string())?;
    let _ = std::fs::rename(&partial, target);
    Ok(bytes)
}

impl WebLoader {
    pub fn new(directory: Option<PathBuf>) -> Self {
        Self::with_curl(directory, "curl".into())
    }

    fn with_curl(directory: Option<PathBuf>, curl: PathBuf) -> Self {
        if let Some(directory) = &directory {
            let _ = std::fs::create_dir_all(directory);
            trim(directory, SystemTime::now(), CACHE_LIMIT);
        }
        Self {
            directory,
            curl,
            entries: Arc::default(),
        }
    }
}

impl BytesLoader for WebLoader {
    fn id(&self) -> &str {
        egui::generate_loader_id!(WebLoader)
    }

    fn load(&self, ctx: &egui::Context, uri: &str) -> BytesLoadResult {
        if !uri.starts_with("https://") {
            return Err(LoadError::NotSupported);
        }
        let mut entries = self.entries.lock().expect("media lock");
        match entries.get(uri) {
            Some(Entry::Ready(bytes)) => {
                return Ok(BytesPoll::Ready {
                    size: None,
                    bytes: Bytes::Shared(bytes.clone()),
                    mime: None,
                })
            }
            Some(Entry::Failed(error)) => return Err(LoadError::Loading(error.clone())),
            Some(Entry::Loading) => return Ok(BytesPoll::Pending { size: None }),
            None => {}
        }
        entries.insert(uri.to_owned(), Entry::Loading);
        drop(entries);
        let (url, entries, ctx) = (uri.to_owned(), self.entries.clone(), ctx.clone());
        let (directory, curl) = (self.directory.clone(), self.curl.clone());
        std::thread::spawn(move || {
            let cached = directory.as_ref().map(|dir| cache_file(dir, &url));
            let result = match &cached {
                Some(file) if fresh(file, SystemTime::now()) => {
                    std::fs::read(file).map_err(|error| error.to_string())
                }
                Some(file) => download(&curl, &url, file),
                None => {
                    let file = std::env::temp_dir()
                        .join(format!("pkgdeck-media-{}", std::process::id()))
                        .join(hex::encode(Sha256::digest(url.as_bytes())));
                    let _ = std::fs::create_dir_all(file.parent().expect("parent"));
                    let result = download(&curl, &url, &file);
                    let _ = std::fs::remove_file(&file);
                    result
                }
            };
            let entry = match result {
                Ok(bytes) => Entry::Ready(bytes.into()),
                Err(error) => Entry::Failed(error),
            };
            entries.lock().expect("media lock").insert(url, entry);
            ctx.request_repaint();
        });
        Ok(BytesPoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        self.entries.lock().expect("media lock").remove(uri);
    }

    fn forget_all(&self) {
        self.entries.lock().expect("media lock").clear();
    }

    fn byte_size(&self) -> usize {
        self.entries
            .lock()
            .expect("media lock")
            .values()
            .map(|entry| match entry {
                Entry::Ready(bytes) => bytes.len(),
                _ => 0,
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pkgdeck-media-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn trimming_drops_expired_then_oldest() {
        let dir = temp("trim");
        let now = SystemTime::now();
        for (name, age, size) in [("old", 8, 10), ("a", 2, 40), ("b", 1, 40), ("c", 0, 40)] {
            let path = dir.join(name);
            std::fs::write(&path, vec![0u8; size]).unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(now - Duration::from_secs(age * 24 * 60 * 60))
                .unwrap();
        }
        trim(&dir, now, 100);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["b", "c"]);
        trim(&dir.join("missing"), now, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn wait_ready(loader: &WebLoader, ctx: &egui::Context, url: &str) -> BytesLoadResult {
        for _ in 0..500 {
            match loader.load(ctx, url) {
                Ok(BytesPoll::Pending { .. }) => std::thread::sleep(Duration::from_millis(10)),
                other => return other,
            }
        }
        panic!("never loaded");
    }

    #[test]
    fn images_download_once_and_come_from_disk_after() {
        let dir = temp("download");
        // A stand-in for curl that writes its URL to the --output file.
        let curl = dir.join("curl");
        std::fs::write(
            &curl,
            "#!/bin/sh\nwhile [ \"$1\" != --output ]; do shift; done\nout=$2; shift 3\n\
             case \"$1\" in *fail*) exit 22;; esac\necho \"$1\" >> \"$out.count\"; printf %s \"$1\" > \"$out\"\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cache = dir.join("cache");
        let loader = WebLoader::with_curl(Some(cache.clone()), curl.clone());
        let ctx = egui::Context::default();
        let url = "https://example.invalid/icon.png";
        let Ok(BytesPoll::Ready { bytes, .. }) = wait_ready(&loader, &ctx, url) else {
            panic!("not ready");
        };
        assert_eq!(&*bytes, url.as_bytes());
        assert_eq!(loader.byte_size(), url.len());
        // A second loader reads the file without downloading.
        let again = WebLoader::with_curl(Some(cache.clone()), curl.clone());
        assert!(matches!(wait_ready(&again, &ctx, url), Ok(BytesPoll::Ready { .. })));
        let count = cache_file(&cache, url).with_extension("partial.count");
        assert_eq!(std::fs::read_to_string(count).unwrap().lines().count(), 1);
        // Failures are remembered until forgotten.
        assert!(matches!(
            wait_ready(&loader, &ctx, "https://example.invalid/fail.png"),
            Err(LoadError::Loading(_))
        ));
        loader.forget("https://example.invalid/fail.png");
        loader.forget_all();
        assert_eq!(loader.byte_size(), 0);
        // Other schemes belong to other loaders.
        assert!(matches!(loader.load(&ctx, "file:///x.png"), Err(LoadError::NotSupported)));
        assert!(matches!(loader.load(&ctx, "http://x/y.png"), Err(LoadError::NotSupported)));
        // Without a cache folder images still load.
        let uncached = WebLoader::with_curl(None, curl);
        assert!(matches!(wait_ready(&uncached, &ctx, url), Ok(BytesPoll::Ready { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
