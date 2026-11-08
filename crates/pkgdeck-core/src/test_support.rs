//! Test executables written without this process holding them open.
//!
//! A test that creates a script and then runs it shares its file descriptors
//! with every other test thread. If one of those threads forks while the write
//! is still open, the child inherits that descriptor and `exec` fails with
//! "Text file busy" until the child exits. The writer here is a short-lived
//! child: it exits before this function returns, so a later `exec` cannot see
//! its descriptor.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Writes `contents` to `path` and marks it executable (`0o755`).
///
/// The calling process never opens `path` for writing.
pub fn write_executable(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    let mut child = Command::new("/bin/sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("the executable writer has no standard input"))?;
    let written = stdin.write_all(contents.as_ref());
    drop(stdin);
    let output = child.wait_with_output()?;
    written?;
    finish(path, &output)
}

/// Copies `from` to `to` and marks the copy executable (`0o755`).
///
/// The calling process never opens `to` for writing.
pub fn copy_executable(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let to = to.as_ref();
    let output = Command::new("/bin/sh")
        .args(["-c", "cp \"$1\" \"$2\" && chmod 755 \"$2\"", "sh"])
        .arg(from.as_ref())
        .arg(to)
        .output()?;
    finish(to, &output)
}

fn finish(path: &Path, output: &Output) -> io::Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    let mut message = format!("writing {}: {}", path.display(), output.status);
    if !detail.is_empty() {
        message.push(' ');
        message.push_str(detail);
    }
    Err(io::Error::other(message))
}
