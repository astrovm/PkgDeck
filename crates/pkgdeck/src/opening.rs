//! One PkgDeck per person. A second launch hands what it was asked to open
//! (a file, a link, or nothing) to the running one over a local socket and
//! exits; the running one shows its window and opens it.

use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};

/// The longest input a second launch may send.
const LIMIT: usize = 8192;

/// The socket's path: in `$XDG_RUNTIME_DIR` (tests point it elsewhere to
/// stay away from an installed PkgDeck that is running), else the temporary
/// folder, named for the user.
pub fn socket_path(runtime: Option<String>, uid: u32) -> PathBuf {
    let directory = runtime
        .filter(|dir| dir.starts_with('/'))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    directory.join(format!("pkgdeck-open-{uid}"))
}

/// The first argument that is something to open: a path, a file link, or
/// an HTTPS or Flatpak link.
pub fn input_from(args: &[String]) -> String {
    args.iter()
        .find(|arg| {
            arg.starts_with('/')
                || arg.starts_with("file://")
                || arg.starts_with("https://")
                || arg.starts_with("flatpak+https://")
        })
        .cloned()
        .unwrap_or_default()
}

/// Sends `input` to the PkgDeck listening at `path`. True when it took it.
pub fn forward(path: &Path, input: &str) -> bool {
    if input.len() > LIMIT {
        return false;
    }
    let Ok(mut stream) = UnixStream::connect(path) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let mut bytes = input.as_bytes().to_vec();
    bytes.push(0);
    stream.write_all(&bytes).is_ok()
}

/// What the running PkgDeck hears: one input per second launch, "" when the
/// launch only asked to show the window.
pub struct Listener {
    pub inputs: mpsc::Receiver<String>,
    path: PathBuf,
}
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Either hand `input` to a running PkgDeck (`Ok(None)`), or become the one
/// that listens. `wake` runs after each input arrives. Without a socket
/// (`Err`) the window still opens, alone.
pub fn claim(
    path: &Path,
    input: &str,
    wake: impl Fn() + Send + 'static,
) -> Result<Option<Listener>, std::io::Error> {
    if forward(path, input) {
        return Ok(None);
    }
    let listener = match UnixListener::bind(path) {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            // Nobody answered: a PkgDeck that crashed left its socket.
            if forward(path, input) {
                return Ok(None);
            }
            std::fs::remove_file(path)?;
            UnixListener::bind(path)?
        }
        Err(error) => return Err(error),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    let (sender, inputs) = mpsc::channel();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut bytes = Vec::new();
            let _ = stream.take(LIMIT as u64 + 1).read_to_end(&mut bytes);
            let Some(end) = bytes.iter().position(|byte| *byte == 0) else {
                continue;
            };
            if sender
                .send(String::from_utf8_lossy(&bytes[..end]).into_owned())
                .is_err()
            {
                break;
            }
            wake();
        }
    });
    Ok(Some(Listener {
        inputs,
        path: path.to_owned(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pkgdeck-open-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("socket")
    }

    #[test]
    fn inputs_are_paths_and_links() {
        let args = |list: &[&str]| list.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        assert_eq!(input_from(&args(&["pkgdeck", "--smoke-test"])), "");
        assert_eq!(input_from(&args(&["pkgdeck", "/tmp/a.flatpakref", "/b"])), "/tmp/a.flatpakref");
        assert_eq!(input_from(&args(&["pkgdeck", "http://x"])), "");
        assert_eq!(
            input_from(&args(&["pkgdeck", "flatpak+https://dl.flathub.org/a"])),
            "flatpak+https://dl.flathub.org/a"
        );
        assert_eq!(socket_path(Some("/run/user/1".into()), 1), PathBuf::from("/run/user/1/pkgdeck-open-1"));
        assert!(socket_path(Some("relative".into()), 7).ends_with("pkgdeck-open-7"));
    }

    #[test]
    fn a_second_launch_hands_its_input_over() {
        let path = temp("handover");
        let (wake_send, woken) = mpsc::channel();
        let listener = claim(&path, "", move || {
            let _ = wake_send.send(());
        })
        .unwrap()
        .expect("first launch listens");
        assert!(claim(&path, "/tmp/Ñandú app.AppImage", || {}).unwrap().is_none());
        assert_eq!(
            listener.inputs.recv_timeout(Duration::from_secs(5)).unwrap(),
            "/tmp/Ñandú app.AppImage"
        );
        woken.recv_timeout(Duration::from_secs(5)).unwrap();
        // Showing the window alone sends nothing to open.
        assert!(forward(&path, ""));
        assert_eq!(listener.inputs.recv_timeout(Duration::from_secs(5)).unwrap(), "");
        // Too long to send.
        assert!(!forward(&path, &"x".repeat(LIMIT + 1)));
        // Bytes without an end are dropped.
        let mut raw = UnixStream::connect(&path).unwrap();
        raw.write_all(b"no end").unwrap();
        drop(raw);
        assert!(forward(&path, "after"));
        assert_eq!(listener.inputs.recv_timeout(Duration::from_secs(5)).unwrap(), "after");
        drop(listener);
        assert!(!path.exists());
    }

    #[test]
    fn a_stale_socket_is_replaced() {
        let path = temp("stale");
        drop(UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        let listener = claim(&path, "", || {}).unwrap();
        assert!(listener.is_some());
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        let path = PathBuf::from("/nonexistent-pkgdeck-dir/socket");
        assert!(claim(&path, "", || {}).is_err());
    }
}
