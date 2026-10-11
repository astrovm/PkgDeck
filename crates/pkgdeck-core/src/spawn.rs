//! Starts a program here, or on the host when PkgDeck runs as a Flatpak,
//! and follows it the same way either way.
use std::ffi::OsStr;
use std::io::{self, PipeReader, PipeWriter};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};

#[cfg(target_os = "linux")]
use crate::portal::{HostChild, HostProgram};

/// What a program's standard input, output or error is connected to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Io {
    Inherit,
    Null,
    Piped,
}
impl Io {
    fn stdio(self) -> Stdio {
        match self {
            Self::Inherit => Stdio::inherit(),
            Self::Null => Stdio::null(),
            Self::Piped => Stdio::piped(),
        }
    }
}

/// A program ready to start.
pub(crate) struct Launch {
    target: Target,
    stdio: [Io; 3],
    group: bool,
    retry: bool,
}
enum Target {
    Local(Command),
    #[cfg(target_os = "linux")]
    Host(HostProgram),
}

impl From<Command> for Launch {
    fn from(command: Command) -> Self {
        Self::new(Target::Local(command))
    }
}

impl Launch {
    fn new(target: Target) -> Self {
        Self {
            target,
            stdio: [Io::Inherit; 3],
            group: false,
            retry: false,
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn host(program: HostProgram) -> Self {
        Self::new(Target::Host(program))
    }

    /// The program itself, as errors name it.
    pub(crate) fn program(&self) -> &OsStr {
        match &self.target {
            Target::Local(command) => command.get_program(),
            #[cfg(target_os = "linux")]
            Target::Host(program) => &program.argv[0],
        }
    }

    /// Its arguments, without the program.
    #[cfg(test)]
    pub(crate) fn args(&self) -> Vec<std::ffi::OsString> {
        match &self.target {
            Target::Local(command) => command.get_args().map(OsStr::to_owned).collect(),
            #[cfg(target_os = "linux")]
            Target::Host(program) => program.argv[1..].to_vec(),
        }
    }

    /// The host program, when it runs on the host.
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn host_program(&self) -> Option<&HostProgram> {
        match &self.target {
            Target::Host(program) => Some(program),
            Target::Local(_) => None,
        }
    }

    /// Add environment variables.
    pub(crate) fn envs<K, V>(&mut self, vars: impl IntoIterator<Item = (K, V)>) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        match &mut self.target {
            Target::Local(command) => {
                command.envs(vars);
            }
            #[cfg(target_os = "linux")]
            Target::Host(program) => {
                let env = program.env.get_or_insert_default();
                for (name, value) in vars {
                    env.insert(name.as_ref().to_owned(), value.as_ref().to_owned());
                }
            }
        }
        self
    }

    pub(crate) fn stdin(&mut self, io: Io) -> &mut Self {
        self.stdio[0] = io;
        self
    }
    pub(crate) fn stdout(&mut self, io: Io) -> &mut Self {
        self.stdio[1] = io;
        self
    }
    pub(crate) fn stderr(&mut self, io: Io) -> &mut Self {
        self.stdio[2] = io;
        self
    }

    /// Start it in its own process group, so stopping it stops what it
    /// started too. Host commands always get one.
    pub(crate) fn process_group(&mut self) -> &mut Self {
        self.group = true;
        self
    }

    /// Nothing breaks if it runs twice, so a host command whose start was
    /// lost on the way may be started again.
    pub(crate) fn retry_lost_start(&mut self) -> &mut Self {
        self.retry = true;
        self
    }

    pub(crate) fn spawn(&mut self) -> io::Result<Child> {
        match &mut self.target {
            Target::Local(command) => {
                let [stdin, stdout, stderr] = self.stdio;
                command
                    .stdin(stdin.stdio())
                    .stdout(stdout.stdio())
                    .stderr(stderr.stdio());
                if self.group {
                    command.process_group(0);
                }
                Ok(command.spawn()?.into())
            }
            #[cfg(target_os = "linux")]
            Target::Host(program) => {
                let ([stdin, stdout, stderr], remote) = host_stdio(self.stdio)?;
                let process = program.start(remote, self.retry)?;
                Ok(Child {
                    stdin: stdin.map(PipeWriter::from),
                    stdout: stdout.map(PipeReader::from),
                    stderr: stderr.map(PipeReader::from),
                    process: Process::Host(process),
                })
            }
        }
    }

    /// Run it and wait. Its streams are the app's own unless set.
    pub(crate) fn status(&mut self) -> io::Result<ExitStatus> {
        self.spawn()?.wait()
    }
}

/// The ends a host command gets, and the ends of its pipes this side keeps.
#[cfg(target_os = "linux")]
type HostStdio = ([Option<OwnedFd>; 3], [OwnedFd; 3]);

#[cfg(target_os = "linux")]
fn host_stdio(stdio: [Io; 3]) -> io::Result<HostStdio> {
    let mut local = [None, None, None];
    let mut remote = Vec::with_capacity(3);
    for (number, io) in stdio.into_iter().enumerate() {
        let input = number == 0;
        remote.push(match io {
            Io::Inherit => match number {
                0 => io::stdin().as_fd().try_clone_to_owned()?,
                1 => io::stdout().as_fd().try_clone_to_owned()?,
                _ => io::stderr().as_fd().try_clone_to_owned()?,
            },
            Io::Null => std::fs::OpenOptions::new()
                .read(input)
                .write(!input)
                .open("/dev/null")?
                .into(),
            Io::Piped => {
                let (reader, writer) = io::pipe()?;
                let (mine, theirs): (OwnedFd, OwnedFd) = if input {
                    (writer.into(), reader.into())
                } else {
                    (reader.into(), writer.into())
                };
                local[number] = Some(mine);
                theirs
            }
        });
    }
    let remote: [OwnedFd; 3] = remote.try_into().expect("three standard streams");
    Ok((local, remote))
}

/// A started program.
pub(crate) struct Child {
    pub(crate) stdin: Option<PipeWriter>,
    pub(crate) stdout: Option<PipeReader>,
    pub(crate) stderr: Option<PipeReader>,
    process: Process,
}
enum Process {
    Local(std::process::Child),
    #[cfg(target_os = "linux")]
    Host(HostChild),
}

impl From<std::process::Child> for Child {
    fn from(mut child: std::process::Child) -> Self {
        Self {
            stdin: child.stdin.take().map(|pipe| OwnedFd::from(pipe).into()),
            stdout: child.stdout.take().map(|pipe| OwnedFd::from(pipe).into()),
            stderr: child.stderr.take().map(|pipe| OwnedFd::from(pipe).into()),
            process: Process::Local(child),
        }
    }
}

impl Child {
    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match &mut self.process {
            Process::Local(child) => child.try_wait(),
            #[cfg(target_os = "linux")]
            Process::Host(child) => child.try_wait(),
        }
    }

    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        // Like std's wait: it gets no more input.
        drop(self.stdin.take());
        match &mut self.process {
            Process::Local(child) => child.wait(),
            #[cfg(target_os = "linux")]
            Process::Host(child) => child.wait(),
        }
    }

    pub(crate) fn kill(&mut self) -> io::Result<()> {
        match &mut self.process {
            Process::Local(child) => child.kill(),
            #[cfg(target_os = "linux")]
            Process::Host(child) => child.signal(rustix::process::Signal::KILL),
        }
    }

    /// Send `signal` to its process group: started with
    /// [`Launch::process_group`], or on the host.
    pub(crate) fn signal_group(&self, signal: rustix::process::Signal) -> io::Result<()> {
        match &self.process {
            Process::Local(child) => {
                let pid =
                    rustix::process::Pid::from_raw(child.id() as i32).expect("live child PID");
                Ok(rustix::process::kill_process_group(pid, signal)?)
            }
            #[cfg(target_os = "linux")]
            Process::Host(child) => child.signal(signal),
        }
    }

    /// Whether it ended, without collecting it: until then its process
    /// group's ID can't go to another program.
    pub(crate) fn ended(&self) -> bool {
        match &self.process {
            Process::Local(child) => {
                use rustix::process::{waitid, Pid, WaitId, WaitIdOptions};
                let pid = Pid::from_raw(child.id() as i32).expect("live child PID");
                matches!(
                    waitid(
                        WaitId::Pid(pid),
                        WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
                    ),
                    Ok(Some(_))
                )
            }
            #[cfg(target_os = "linux")]
            Process::Host(child) => child.ended(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn local_programs_get_the_streams_they_ask_for() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "read line; echo \"got $line\"; echo oops >&2; exit 3"]);
        let mut launch = Launch::from(command);
        assert_eq!(launch.program(), "/bin/sh");
        assert_eq!(launch.args()[0], "-c");
        let mut child = launch
            .stdin(Io::Piped)
            .stdout(Io::Piped)
            .stderr(Io::Piped)
            .process_group()
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"hi\n").unwrap();
        let mut output = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        let mut error = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut error)
            .unwrap();
        // It ends, and it is still there to collect. Each check waits
        // first, so the coverage of these lines doesn't depend on timing.
        let mut ended = false;
        for _ in 0..2000 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            ended = child.ended();
            if ended {
                break;
            }
        }
        assert!(ended);
        assert_eq!(child.wait().unwrap().code(), Some(3));
        assert_eq!((output.as_str(), error.as_str()), ("got hi\n", "oops\n"));
    }

    #[test]
    fn local_environment_reaches_the_program() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "printf %s \"$PKGDECK_SYNTHETIC\""])
            .env_clear();
        let mut child = Launch::from(command)
            .envs([("PKGDECK_SYNTHETIC", "value")])
            .stdout(Io::Piped)
            .spawn()
            .unwrap();
        let mut output = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        child.wait().unwrap();
        assert_eq!(output, "value");
        assert!(Launch::from(Command::new("/usr/bin/true"))
            .status()
            .unwrap()
            .success());
    }

    #[cfg(target_os = "linux")]
    mod host {
        use super::*;
        use crate::portal;
        use std::collections::BTreeMap;
        use std::path::Path;
        use std::time::{Duration, Instant};

        /// A fake host that runs the program here.
        fn host() -> (portal::Portal, std::path::PathBuf) {
            let dir = std::env::temp_dir().join(format!(
                "pkgdeck-portal-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let bridge = portal::fake_bridge(&dir);
            (portal::fake(&bridge), dir)
        }

        fn launch(portal: &portal::Portal, script: &str) -> Launch {
            let env = BTreeMap::from([("PKGDECK_SYNTHETIC".into(), "fixture".into())]);
            Launch::host(
                portal
                    .program(
                        vec!["/bin/sh".into(), "-c".into(), script.into()],
                        Some(env),
                    )
                    .unwrap(),
            )
        }

        #[test]
        fn host_programs_talk_through_their_pipes_and_report_how_they_ended() {
            let (portal, dir) = host();
            let mut child = launch(
                &portal,
                "read line; echo \"got $line $PKGDECK_SYNTHETIC\"; echo oops >&2; exit 4",
            )
            .stdin(Io::Piped)
            .stdout(Io::Piped)
            .stderr(Io::Piped)
            .spawn()
            .unwrap();
            child.stdin.take().unwrap().write_all(b"hi\n").unwrap();
            // The end of output arrives: no copy of the pipe stays open.
            let mut output = String::new();
            child
                .stdout
                .take()
                .unwrap()
                .read_to_string(&mut output)
                .unwrap();
            let mut error = String::new();
            child
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut error)
                .unwrap();
            assert_eq!(child.wait().unwrap().code(), Some(4));
            assert!(child.ended());
            // Collected once, it stays collected.
            assert_eq!(child.try_wait().unwrap().unwrap().code(), Some(4));
            assert_eq!(output, "got hi fixture\n");
            assert_eq!(error, "oops\n");

            let mut silent = launch(&portal, "echo hidden; echo hidden >&2");
            let status = silent.stdout(Io::Null).stderr(Io::Null).status();
            assert!(status.unwrap().success());
            std::fs::remove_dir_all(dir).unwrap();
        }

        #[test]
        fn host_programs_take_more_environment() {
            let (portal, dir) = host();
            let script = "printf %s \"$PKGDECK_SYNTHETIC $PKGDECK_MORE\"";
            let mut more = launch(&portal, script);
            more.envs([("PKGDECK_MORE", "extra")]);
            assert_eq!(more.args(), ["-c", script]);
            let mut child = more.stdout(Io::Piped).spawn().unwrap();
            let mut output = String::new();
            child
                .stdout
                .take()
                .unwrap()
                .read_to_string(&mut output)
                .unwrap();
            assert!(child.wait().unwrap().success());
            assert_eq!(output, "fixture extra");
            assert!(Launch::from(Command::new("/usr/bin/true"))
                .host_program()
                .is_none());
            std::fs::remove_dir_all(dir).unwrap();
        }

        #[test]
        fn many_host_programs_at_once_share_one_connection() {
            let (portal, dir) = host();
            let children: Vec<_> = (0..300)
                .map(|n| {
                    launch(&portal, &format!("exit {}", n % 7))
                        .stdout(Io::Piped)
                        .spawn()
                        .unwrap()
                })
                .collect();
            for (n, mut child) in children.into_iter().enumerate() {
                assert_eq!(child.wait().unwrap().code(), Some(n as i32 % 7));
            }
            std::fs::remove_dir_all(dir).unwrap();
        }

        #[test]
        fn host_programs_stop_on_a_signal_to_their_group() {
            let (portal, dir) = host();
            let mut child = launch(&portal, "sleep 30 & wait").spawn().unwrap();
            assert!(child.try_wait().unwrap().is_none());
            assert!(!child.ended());
            child.signal_group(rustix::process::Signal::TERM).unwrap();
            let status = child.wait().unwrap();
            assert_eq!(
                std::os::unix::process::ExitStatusExt::signal(&status),
                Some(15)
            );
            let mut stubborn = launch(&portal, "trap '' TERM; sleep 30").spawn().unwrap();
            let start = Instant::now();
            stubborn.kill().unwrap();
            assert!(stubborn.wait().is_ok());
            assert!(start.elapsed() < Duration::from_secs(10));
            std::fs::remove_dir_all(dir).unwrap();
        }

        #[test]
        fn a_program_the_host_cant_start_fails_to_spawn() {
            let (portal, dir) = host();
            let program = portal.program(vec!["/bin/true".into()], None).unwrap();
            // The fake host can't start anything once its bridge is gone.
            std::fs::remove_dir_all(dir).unwrap();
            let error = Launch::host(program).spawn().err().unwrap();
            assert!(
                error.to_string().starts_with("Failed to start command"),
                "{error}"
            );
            // No bridge, no host commands.
            let missing = portal::fake(Path::new("/pkgdeck-synthetic-missing-bridge"));
            assert!(missing.program(vec!["/bin/true".into()], None).is_none());
        }

        #[test]
        fn a_program_nobody_waits_for_leaves_nothing_behind() {
            let (portal, dir) = host();
            drop(launch(&portal, "exit 0").spawn().unwrap());
            let mut child = launch(&portal, "exit 5").spawn().unwrap();
            assert_eq!(child.wait().unwrap().code(), Some(5));
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
