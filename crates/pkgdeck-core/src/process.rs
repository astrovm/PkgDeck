//! Bounded process supervision. Writes finish under the native manager's lock.
use std::io::{self, Read};
use std::os::fd::AsFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn requested(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub timeout: Duration,
    pub output_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            output_bytes: 128 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Completion {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
    /// Cancellation during a write is deferred until the native manager exits.
    pub cancellation_deferred: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionError {
    Disabled(String),
    Invalid(String),
    Io(String),
    Cancelled,
    TimedOut,
    AuthorizationCancelled,
    AuthorizationDenied,
    LockBusy,
    Interrupted,
    Failed(Completion),
}
impl std::fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled(s) | Self::Invalid(s) | Self::Io(s) => f.write_str(s),
            Self::Cancelled => f.write_str("operation cancelled before completion"),
            Self::TimedOut => f.write_str("host read timed out"),
            Self::AuthorizationCancelled => f.write_str("authorization cancelled"),
            Self::AuthorizationDenied => f.write_str("authorization denied or unavailable"),
            Self::LockBusy => {
                f.write_str("APT is busy; wait for the native manager, never remove lock files")
            }
            Self::Interrupted => {
                f.write_str("APT was interrupted; inspect native package state before retrying")
            }
            Self::Failed(result) => write!(
                f,
                "host command failed ({:?}): {}",
                result.code,
                String::from_utf8_lossy(&result.stderr)
            ),
        }
    }
}
impl std::error::Error for ExecutionError {}
impl From<io::Error> for ExecutionError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

fn nonblocking(pipe: &impl AsFd) -> io::Result<()> {
    let flags = rustix::fs::fcntl_getfl(pipe)?;
    Ok(rustix::fs::fcntl_setfl(
        pipe,
        flags | rustix::fs::OFlags::NONBLOCK,
    )?)
}

fn drain(
    pipe: &mut impl Read,
    output: &mut Vec<u8>,
    limit: usize,
    truncated: &mut bool,
) -> io::Result<()> {
    let mut buffer = [0; 4096];
    // Bound work per tick even if a child continuously produces output.
    for _ in 0..32 {
        match pipe.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                let keep = n.min(limit.saturating_sub(output.len()));
                output.extend_from_slice(&buffer[..keep]);
                *truncated |= keep < n;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

pub(crate) fn run(
    mut command: Command,
    limits: Limits,
    cancel: &Cancellation,
    write: bool,
) -> Result<Completion, ExecutionError> {
    if cancel.requested() {
        return Err(ExecutionError::Cancelled);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let mut stdout = child.stdout.take().expect("stdout was piped");
    let mut stderr = child.stderr.take().expect("stderr was piped");
    let start = Instant::now();
    let mut result = Completion {
        code: None,
        signal: None,
        stdout: vec![],
        stderr: vec![],
        truncated: false,
        cancellation_deferred: false,
    };
    let mut reaped = false;
    let supervise = (|| {
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        loop {
            drain(
                &mut stdout,
                &mut result.stdout,
                limits.output_bytes,
                &mut result.truncated,
            )?;
            drain(
                &mut stderr,
                &mut result.stderr,
                limits.output_bytes,
                &mut result.truncated,
            )?;
            if let Some(status) = child.try_wait()? {
                reaped = true;
                drain(
                    &mut stdout,
                    &mut result.stdout,
                    limits.output_bytes,
                    &mut result.truncated,
                )?;
                drain(
                    &mut stderr,
                    &mut result.stderr,
                    limits.output_bytes,
                    &mut result.truncated,
                )?;
                result.code = status.code();
                result.signal = status.signal();
                result.cancellation_deferred = write && cancel.requested();
                return Ok(result);
            }
            if !write {
                if cancel.requested() {
                    return Err(ExecutionError::Cancelled);
                }
                if start.elapsed() >= limits.timeout {
                    return Err(ExecutionError::TimedOut);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    })();
    if supervise.is_err() && !reaped {
        // The child has not been reaped, so its process-group ID cannot be reused.
        let pid = rustix::process::Pid::from_raw(child.id() as i32).expect("live child PID");
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        let _ = child.wait();
    }
    supervise
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn writes_defer_cancellation_and_timeout_until_native_completion() {
        let cancel = Cancellation::default();
        let other = cancel.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            other.cancel();
        });
        let mut command = Command::new("/usr/bin/python3");
        command.args(["-c", "import time; time.sleep(0.1); print('committed')"]);
        let result = run(
            command,
            Limits {
                timeout: Duration::ZERO,
                ..Limits::default()
            },
            &cancel,
            true,
        )
        .unwrap();
        thread.join().unwrap();
        assert_eq!(result.code, Some(0));
        assert_eq!(result.stdout, b"committed\n");
        assert!(result.cancellation_deferred);
    }
}
