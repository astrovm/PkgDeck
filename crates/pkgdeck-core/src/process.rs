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

/// Receives each line a package manager prints while it runs.
pub type OutputObserver = Arc<dyn Fn(&str) + Send + Sync>;

/// Cancels a running job, and can follow its output: it reaches every
/// command a job runs, so an observer set here sees their output live.
#[derive(Clone, Default)]
pub struct Cancellation {
    flag: Arc<AtomicBool>,
    output: Option<OutputObserver>,
}
impl std::fmt::Debug for Cancellation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cancellation")
            .field("requested", &self.requested())
            .field("observed", &self.output.is_some())
            .finish()
    }
}
impl Cancellation {
    /// Shared signal flag; handlers only request cancellation.
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.flag.clone()
    }
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }
    pub fn requested(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
    /// The same cancellation, with `observer` seeing every line of output
    /// the job's commands print, as they print it.
    pub fn with_output(&self, observer: OutputObserver) -> Self {
        Self {
            flag: self.flag.clone(),
            output: Some(observer),
        }
    }
    pub(crate) fn observe(&self, line: &str) {
        if let Some(observer) = &self.output {
            observer(line);
        }
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
/// marked as an error (`E:`, `error:`, `fatal:` …) that says more than the
/// marker (Nix prints a bare `error:` before the cause), else the first line that
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
    let bare = |line: &str| {
        line.trim_end_matches(|c: char| c == ':' || c.is_whitespace())
            .chars()
            .all(|c| c.is_ascii_alphabetic())
    };
    let line = lines
        .iter()
        .find(|line| starts(line, &ERRORS) && !bare(line))
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
    let flags = rustix::fs::fcntl_getfl(pipe)? | rustix::fs::OFlags::NONBLOCK;
    Ok(rustix::fs::fcntl_setfl(pipe, flags)?)
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
    command: Command,
    limits: Limits,
    cancel: &Cancellation,
    write: bool,
) -> Result<Completion, ExecutionError> {
    run_observed(command, limits, cancel, write, &mut |line| {
        cancel.observe(line)
    })
}

/// Like [`run`], and hands each complete line of standard output to
/// `on_line` as soon as it arrives, so a long command can be followed
/// (which package Homebrew is upgrading now, for one).
pub(crate) fn run_observed(
    mut command: Command,
    limits: Limits,
    cancel: &Cancellation,
    write: bool,
    on_line: &mut dyn FnMut(&str),
) -> Result<Completion, ExecutionError> {
    if cancel.requested() {
        return Err(ExecutionError::Cancelled);
    }
    let program = command.get_program().to_string_lossy().into_owned();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    // A program written just before it runs can briefly report "text file
    // busy" while another thread's fork still holds its write descriptor.
    // Nothing ran yet, so trying again is safe, even for writes.
    let mut busy_retries = 0;
    let mut child = loop {
        match command.spawn() {
            Ok(child) => break child,
            Err(error)
                if error.kind() == io::ErrorKind::ExecutableFileBusy && busy_retries < 20 =>
            {
                busy_retries += 1;
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(ExecutionError::Io(format!("spawn {program}: {error}"))),
        }
    };
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
    // Standard output up to here was already handed out line by line.
    let mut reported = 0;
    let mut report = |output: &[u8], last: bool| {
        while let Some(end) = output[reported..].iter().position(|byte| *byte == b'\n') {
            on_line(&String::from_utf8_lossy(&output[reported..reported + end]));
            reported += end + 1;
        }
        if last && reported < output.len() {
            on_line(&String::from_utf8_lossy(&output[reported..]));
        }
    };
    let supervise = (|| {
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        let cap = limits.output_bytes;
        let mut drain_both = |result: &mut Completion| -> io::Result<()> {
            drain(&mut stdout, &mut result.stdout, cap, &mut result.truncated)?;
            drain(&mut stderr, &mut result.stderr, cap, &mut result.truncated)
        };
        loop {
            drain_both(&mut result)?;
            report(&result.stdout, false);
            if let Some(status) = child.try_wait()? {
                reaped = true;
                drain_both(&mut result)?;
                report(&result.stdout, true);
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
        // The caller gets its answer now; the group is killed and reaped
        // beside it, so a stop never keeps the app busy.
        let pid = rustix::process::Pid::from_raw(child.id() as i32).expect("live child PID");
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::TERM);
        std::thread::spawn(move || stop_group(&mut child, stdout, stderr, STOP_GRACE));
    }
    supervise
}

/// How long a stopped command gets to exit before it is killed.
const STOP_GRACE: Duration = Duration::from_millis(500);

/// Finishes stopping a child's process group that was sent SIGTERM: SIGKILL
/// after `grace`. In the Flatpak the child is `flatpak-spawn --host`, which
/// passes SIGTERM on to the command it started on the host. SIGKILL can't be
/// passed on, so the host command would keep running with nobody reading
/// its output, and some (grok) crash when they finally print.
///
/// Its output keeps being read and dropped until it exits: a command that
/// prints while it stops would otherwise die from a broken pipe, which is
/// how grok crashed.
fn stop_group(
    child: &mut std::process::Child,
    mut stdout: impl Read,
    mut stderr: impl Read,
    grace: Duration,
) {
    use rustix::process::{kill_process_group, waitid, Pid, Signal, WaitId, WaitIdOptions};
    // The child is not reaped until the end, so its process-group ID
    // cannot be reused while it is signalled.
    let pid = Pid::from_raw(child.id() as i32).expect("live child PID");
    let exited = || {
        matches!(
            waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            ),
            Ok(Some(_))
        )
    };
    let mut drop_output = || {
        let _ = drain(&mut stdout, &mut Vec::new(), 0, &mut false);
        let _ = drain(&mut stderr, &mut Vec::new(), 0, &mut false);
    };
    let deadline = Instant::now() + grace;
    while !exited() && Instant::now() < deadline {
        drop_output();
        std::thread::sleep(Duration::from_millis(5));
    }
    // Whatever else is left in the group, such as a child that ignored
    // SIGTERM or the leader's own children.
    let _ = kill_process_group(pid, Signal::KILL);
    let _ = child.wait();
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
    struct Scripted(Vec<io::Result<&'static [u8]>>);
    impl Read for Scripted {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let bytes = self.0.remove(0)?;
            buffer[..bytes.len()].copy_from_slice(bytes);
            Ok(bytes.len())
        }
    }
    #[test]
    fn draining_retries_interrupts_caps_output_and_reports_read_errors() {
        let mut output = vec![];
        let mut truncated = false;
        let mut pipe = Scripted(vec![
            Err(io::ErrorKind::Interrupted.into()),
            Ok(b"abc"),
            Ok(b"def"),
            Err(io::ErrorKind::WouldBlock.into()),
        ]);
        drain(&mut pipe, &mut output, 4, &mut truncated).unwrap();
        assert_eq!(output, b"abcd");
        assert!(truncated);
        let mut broken = Scripted(vec![Err(io::ErrorKind::BrokenPipe.into())]);
        assert_eq!(
            drain(&mut broken, &mut output, 4, &mut truncated)
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
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
        let nix = "error:\n       … while calling the 'throw' builtin\n       error: Nixpkgs 26.11 has dropped support for x86_64-darwin.\n";
        assert_eq!(
            failure_summary(nix.as_bytes()).unwrap(),
            "error: Nixpkgs 26.11 has dropped support for x86_64-darwin."
        );
        // A bare marker alone is still better than nothing.
        assert_eq!(failure_summary(b"error:\n").unwrap(), "error:");
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
    /// Waits up to `limit` for `done`, and says whether it happened. It always
    /// sleeps once first, so no line depends on how fast the runner is.
    fn wait_for(limit: Duration, done: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            std::thread::sleep(Duration::from_millis(5));
            if done() || Instant::now() >= deadline {
                return done();
            }
        }
    }

    /// A read that runs past its limit, in a fresh folder for its marker files.
    fn stopped_read(name: &str, script: &str) -> (std::path::PathBuf, Duration) {
        let dir = std::env::temp_dir().join(format!("pkgdeck-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ready = dir.join("ready");
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script, "sh"]).arg(&dir);
        let start = Instant::now();
        let thread = std::thread::spawn(move || {
            let cancel = Cancellation::default();
            let stopper = cancel.clone();
            let watcher = std::thread::spawn(move || {
                wait_for(Duration::from_secs(10), || ready.exists());
                stopper.cancel();
            });
            let result = run(command, Limits::default(), &cancel, false);
            watcher.join().unwrap();
            result
        });
        assert_eq!(thread.join().unwrap(), Err(ExecutionError::Cancelled));
        (dir, start.elapsed())
    }

    #[test]
    fn a_stopped_read_is_asked_to_exit_before_it_is_killed() {
        // flatpak-spawn passes SIGTERM on to the host command; SIGKILL
        // would leave that command running on the host.
        let (dir, _) = stopped_read(
            "terminated",
            // It prints while stopping, as grok did: a closed pipe would
            // kill it before it gets to its marker.
            "trap 'echo stopping; echo stopping >&2; : > \"$1/terminated\"; exit 0' TERM; \
             : > \"$1/ready\"; \
             while :; do sleep 0.01; done",
        );
        // The trap runs beside the answer, before any SIGKILL.
        assert!(wait_for(STOP_GRACE, || dir.join("terminated").exists()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_stopped_read_that_ignores_sigterm_is_still_killed() {
        let (dir, elapsed) = stopped_read(
            "ignores-term",
            "trap '' TERM; echo $$ > \"$1/pid\"; : > \"$1/ready\"; while :; do sleep 0.01; done",
        );
        // The read answers at once; the kill follows after the grace.
        assert!(elapsed < STOP_GRACE, "{elapsed:?}");
        let pid = std::fs::read_to_string(dir.join("pid")).unwrap();
        let pid = rustix::process::Pid::from_raw(pid.trim().parse().unwrap()).unwrap();
        // Signal 0 only asks whether it is still there; macOS has no /proc.
        let alive = || rustix::process::test_kill_process(pid).is_ok();
        assert!(alive());
        assert!(wait_for(Duration::from_secs(5), || !alive()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn writes_defer_cancellation_and_timeout_until_native_completion() {
        let cancel = Cancellation::default();
        let other = cancel.clone();
        // Handshake with the child: it reports that it started, then waits for
        // the flag. Cancellation therefore always lands while the write runs,
        // however a loaded runner schedules either thread.
        let dir = std::env::temp_dir().join(format!("pkgdeck-deferred-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (ready, flag) = (dir.join("ready"), dir.join("cancelled"));
        let (started, marker) = (ready.clone(), flag.clone());
        let thread = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !started.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            other.cancel();
            std::fs::write(marker, b"").unwrap();
        });
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                ": > \"$1\"; while [ ! -e \"$2\" ]; do sleep 0.01; done; printf 'committed\n'",
                "sh",
            ])
            .arg(&ready)
            .arg(&flag);
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
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(result.code, Some(0));
        assert_eq!(result.stdout, b"committed\n");
        assert!(result.cancellation_deferred);
    }
    #[test]
    fn output_lines_arrive_while_the_program_runs() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf 'first\\n'; sleep 0.2; printf 'second\\nlast'"]);
        let started = Instant::now();
        let lines = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = lines.clone();
        let cancel = Cancellation::default().with_output(Arc::new(move |line: &str| {
            seen.lock()
                .unwrap()
                .push((line.to_owned(), started.elapsed()));
        }));
        assert!(format!("{cancel:?}").contains("observed: true"));
        let result = run(command, Limits::default(), &cancel, false).unwrap();
        let lines = lines.lock().unwrap();
        assert_eq!(result.stdout, b"first\nsecond\nlast");
        let names: Vec<_> = lines.iter().map(|(line, _)| line.as_str()).collect();
        assert_eq!(names, ["first", "second", "last"]);
        // The first line came before the program finished.
        assert!(lines[0].1 < lines[1].1);
        assert!(lines[1].1 - lines[0].1 >= Duration::from_millis(150));
    }
    #[test]
    fn a_program_still_open_for_writing_runs_once_it_is_closed() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("pkgdeck-busy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tool");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"#!/bin/sh\nprintf ran\n").unwrap();
        file.set_permissions(std::fs::Permissions::from_mode(0o755))
            .unwrap();
        // The open write descriptor makes exec fail with "text file busy"
        // until it closes.
        let holder = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            drop(file);
        });
        let result = run(
            Command::new(&path),
            Limits::default(),
            &Cancellation::default(),
            false,
        )
        .unwrap();
        holder.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(result.code, Some(0));
        assert_eq!(result.stdout, b"ran");
    }
}
