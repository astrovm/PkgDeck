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
    /// Shared signal flag; handlers only request cancellation.
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.0.clone()
    }
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

#[derive(serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct Completion {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    #[serde(serialize_with = "serialize_output")]
    pub stdout: Vec<u8>,
    #[serde(serialize_with = "serialize_output")]
    pub stderr: Vec<u8>,
    pub truncated: bool,
    /// Cancellation during a write is deferred until the native manager exits.
    pub cancellation_deferred: bool,
}

impl Completion {
    /// How the command ended, as the end of a plain sentence: "exited with
    /// code 1: error: …" or "was stopped by signal 9". The reason is the
    /// first meaningful stderr line; the full output stays in the value.
    pub fn outcome(&self) -> String {
        let ended = match (self.code, self.signal) {
            (Some(code), _) => format!("exited with code {code}"),
            (None, Some(signal)) => format!("was stopped by signal {signal}"),
            (None, None) => "was stopped".into(),
        };
        match failure_summary(&self.stderr) {
            Some(reason) => format!("{ended}: {reason}"),
            None => ended,
        }
    }
}

/// The stderr line that best explains a failed command: the first line
/// marked as an error (`E:`, `error:`, `fatal:` …), else the first line that
/// is not a warning or hint, else the first non-empty line. Long lines are
/// shortened and control characters never pass through.
pub fn failure_summary(stderr: &[u8]) -> Option<String> {
    const ERRORS: [&str; 4] = ["e:", "error", "fatal", "npm err"];
    const NOISE: [&str; 6] = ["w:", "warn", "npm warn", "n:", "note:", "hint:"];
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let starts = |line: &str, markers: &[&str]| {
        let line = line.to_ascii_lowercase();
        markers.iter().any(|marker| line.starts_with(marker))
    };
    let line = lines
        .iter()
        .find(|line| starts(line, &ERRORS))
        .or_else(|| lines.iter().find(|line| !starts(line, &NOISE)))
        .or_else(|| lines.first())?;
    let clean: String = line
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    const LIMIT: usize = 300;
    Some(if clean.chars().count() > LIMIT {
        format!("{}…", clean.chars().take(LIMIT).collect::<String>())
    } else {
        clean
    })
}

fn serialize_output<S: serde::Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&String::from_utf8_lossy(bytes))
}

#[derive(serde::Serialize, Clone, Debug, Eq, PartialEq)]
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
            Self::Failed(result) => write!(f, "the package manager {}", result.outcome()),
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
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|error| ExecutionError::Io(format!("spawn {program}: {error}")))?;
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
    fn failed(code: Option<i32>, signal: Option<i32>, stderr: &str) -> Completion {
        Completion {
            code,
            signal,
            stdout: vec![],
            stderr: stderr.as_bytes().to_vec(),
            truncated: false,
            cancellation_deferred: false,
        }
    }
    #[test]
    fn failures_read_as_one_plain_sentence() {
        let rustup = "error: rustup could not choose a version of cargo to run\nhelp: run 'rustup default stable'\n";
        assert_eq!(
            ExecutionError::Failed(failed(Some(1), None, rustup)).to_string(),
            "the package manager exited with code 1: error: rustup could not choose a version of cargo to run"
        );
        let apt = "WARNING: apt does not have a stable CLI interface.\n\nE: Unable to locate package x\nE: second\n";
        assert_eq!(
            failure_summary(apt.as_bytes()).unwrap(),
            "E: Unable to locate package x"
        );
        assert_eq!(
            failure_summary(b"warning: odd\nsomething broke\n").unwrap(),
            "something broke"
        );
        assert_eq!(
            failure_summary(b"warning: only\n").unwrap(),
            "warning: only"
        );
        assert_eq!(failure_summary(b"\n  \n"), None);
        assert_eq!(failure_summary(b"bad\x1b[31m").unwrap(), "bad [31m");
        let long = "x".repeat(400);
        assert_eq!(
            failure_summary(long.as_bytes()).unwrap().chars().count(),
            301
        );
        assert_eq!(failed(Some(2), None, "").outcome(), "exited with code 2");
        assert_eq!(
            failed(None, Some(9), "").outcome(),
            "was stopped by signal 9"
        );
        assert_eq!(failed(None, None, "").outcome(), "was stopped");
    }
    #[test]
    fn writes_defer_cancellation_and_timeout_until_native_completion() {
        let cancel = Cancellation::default();
        let other = cancel.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            other.cancel();
        });
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 0.1; printf 'committed\n'"]);
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
