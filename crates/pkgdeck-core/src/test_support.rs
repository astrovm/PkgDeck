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
    // A writer that failed stops reading, so its reason comes first.
    finish(path, &output)?;
    written
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pkgdeck-test-support-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_written_executable_runs() {
        let dir = scratch("runs");
        let script = dir.join("script");
        write_executable(&script, "#!/bin/sh\necho written\n").unwrap();
        let copy = dir.join("copy");
        copy_executable(&script, &copy).unwrap();
        for program in [&script, &copy] {
            let output = Command::new(program).output().unwrap();
            assert_eq!(output.stdout, b"written\n");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_write_names_the_file_and_says_why() {
        let dir = scratch("fails");
        let missing = dir.join("missing").join("script");
        // Bigger than a pipe holds, so the shell is gone before it is all
        // sent: its exit status is still the reason given.
        let contents = format!("#!/bin/sh\n{}", "#".repeat(1 << 20));
        let error = write_executable(&missing, contents).unwrap_err();
        let message = error.to_string();
        assert!(message.starts_with(&format!("writing {}: exit status: ", missing.display())));
        // The shell's own reason follows, and names the file again.
        let path = missing.display().to_string();
        assert_eq!(message.matches(&path).count(), 2, "{message}");
        let error = copy_executable(dir.join("absent"), dir.join("copy")).unwrap_err();
        assert!(error.to_string().contains("absent"), "{error}");
        assert!(!dir.join("copy").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
