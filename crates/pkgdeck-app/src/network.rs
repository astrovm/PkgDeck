//! Downloads a provider's app metadata (JSON) with curl, as the engine
//! downloads everything else: HTTPS only, within 4 seconds and 2 MB, and
//! stopped as soon as the lookup is cancelled.

use pkgdeck_core::process::Cancellation;
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

const LIMIT: usize = 2 * 1024 * 1024;

/// The document at `url`, or "" for any other scheme, a failure, or a
/// cancellation.
pub fn fetch(url: &str, cancel: &Cancellation) -> String {
    fetch_with(Path::new("curl"), url, cancel)
}

fn fetch_with(curl: &Path, url: &str, cancel: &Cancellation) -> String {
    if cancel.requested() || !url.starts_with("https://") {
        return String::new();
    }
    let mut command = Command::new(curl);
    command
        .args([
            "--silent",
            "--fail",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
        ])
        .args(["--max-time", "4", "--max-filesize", &LIMIT.to_string()])
        .args(["--header", "Accept: application/json"]);
    if url.starts_with("https://api.snapcraft.io/") {
        command.args(["--header", "Snap-Device-Series: 16"]);
    }
    let Ok(mut child) = command
        .arg("--")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return String::new();
    };
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = (&mut stdout).take(LIMIT as u64 + 1).read_to_end(&mut bytes);
        bytes
    });
    let status = loop {
        if cancel.requested() {
            let _ = child.kill();
            let _ = child.wait();
            return String::new();
        }
        match child.try_wait() {
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            finished => break finished.ok().flatten(),
        }
    };
    let bytes = reader.join().unwrap_or_default();
    if !status.is_some_and(|status| status.success()) || bytes.len() > LIMIT {
        return String::new();
    }
    String::from_utf8(bytes).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    /// A curl that prints `body`, then exits with `code` after `delay`.
    fn fake(dir: &Path, name: &str, body: &str, code: i32, delay: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(
            &path,
            format!("#!/bin/sh\nprintf '%s' \"$*\" > '{}.args'\nprintf '%s' '{body}'\nsleep {delay}\nexit {code}\n", path.display()),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn fetches_https_documents_only_and_stops_when_cancelled() {
        let temp = pkgdeck_tools::Temp::new();
        let cancel = Cancellation::default();
        let ok = fake(&temp.0, "ok", "{\"name\":\"Krita\"}", 0, "0");
        assert_eq!(
            fetch_with(
                &ok,
                "https://flathub.org/api/v2/appstream/org.kde.krita",
                &cancel
            ),
            "{\"name\":\"Krita\"}"
        );
        let args = fs::read_to_string(temp.0.join("ok.args")).unwrap();
        assert!(
            args.contains("--proto =https") && args.contains("Accept: application/json"),
            "{args}"
        );
        assert!(!args.contains("Snap-Device-Series"));
        // The Snap Store answers only with its device series.
        fetch_with(
            &ok,
            "https://api.snapcraft.io/v2/snaps/info/firefox",
            &cancel,
        );
        assert!(fs::read_to_string(temp.0.join("ok.args"))
            .unwrap()
            .contains("Snap-Device-Series: 16"));
        // Never fetched: plain HTTP, local files.
        assert_eq!(fetch_with(&ok, "http://127.0.0.1:9/app.json", &cancel), "");
        assert_eq!(fetch_with(&ok, "file:///etc/hostname", &cancel), "");
        // A failed download, an answer that isn't text, a missing curl.
        let failed = fake(&temp.0, "failed", "partial", 22, "0");
        assert_eq!(fetch_with(&failed, "https://example.invalid/", &cancel), "");
        let binary = temp.0.join("binary");
        fs::write(&binary, "#!/bin/sh\nprintf '\\377\\376'\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(fetch_with(&binary, "https://example.invalid/", &cancel), "");
        assert_eq!(
            fetch_with(&temp.0.join("missing"), "https://example.invalid/", &cancel),
            ""
        );
        // Too big: cut at 2 MB and dropped.
        let big = temp.0.join("big");
        fs::write(
            &big,
            "#!/bin/sh\nhead -c 2097153 /dev/zero | tr '\\0' 'a'\n",
        )
        .unwrap();
        fs::set_permissions(&big, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(fetch_with(&big, "https://example.invalid/", &cancel), "");
        // Cancelled before, or while, it runs.
        let slow = fake(&temp.0, "slow", "late", 0, "5");
        let waiting = cancel.clone();
        let started = std::time::Instant::now();
        let worker = thread::spawn(move || fetch_with(&slow, "https://example.invalid/", &waiting));
        thread::sleep(Duration::from_millis(100));
        cancel.cancel();
        assert_eq!(worker.join().unwrap(), "");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(fetch_with(&ok, "https://example.invalid/", &cancel), "");
        assert_eq!(fetch("https://example.invalid/", &cancel), "");
    }
}
