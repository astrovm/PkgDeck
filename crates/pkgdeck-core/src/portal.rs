//! Host commands from the Flatpak: the Flatpak portal's `HostCommand`, over
//! one session-bus connection the whole app shares.
//!
//! `flatpak-spawn --host` opens a new bus connection for every command. The
//! sandbox's bus proxy turns connections away when too many arrive at once
//! ("Can't find bus: … Resource temporarily unavailable"), which checking
//! every source at the same time did. One connection never piles up.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type;
use zbus::MatchRule;

const DESTINATION: &str = "org.freedesktop.Flatpak";
const PATH: &str = "/org/freedesktop/Flatpak/Development";
const INTERFACE: &str = "org.freedesktop.Flatpak.Development";
/// `HostCommand` flag: start from an empty environment.
const CLEAR_ENV: u32 = 1;

pub(crate) const UNAVAILABLE: &str = "Flatpak host bridge is unavailable";

/// Where host commands go: the shared session bus, or (in tests) a fake
/// portal, or nowhere.
#[derive(Clone)]
pub(crate) struct Portal(Route);
#[derive(Clone)]
enum Route {
    Session,
    #[cfg(test)]
    Fixed(Option<Arc<Bus>>),
}
impl std::fmt::Debug for Portal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            Route::Session => "Portal(session)",
            #[cfg(test)]
            Route::Fixed(_) => "Portal(fake)",
        })
    }
}

/// A program to run on the host. `env: None` keeps the environment the
/// host's Flatpak session helper has; otherwise the program gets exactly it.
#[derive(Clone, Debug)]
pub(crate) struct HostProgram {
    pub(crate) argv: Vec<OsString>,
    pub(crate) env: Option<BTreeMap<OsString, OsString>>,
    portal: Portal,
}

impl Portal {
    pub(crate) fn session() -> Self {
        Self(Route::Session)
    }

    /// The program, if host commands can run at all.
    pub(crate) fn program(
        &self,
        argv: Vec<OsString>,
        env: Option<BTreeMap<OsString, OsString>>,
    ) -> Option<HostProgram> {
        self.bus().ok()?;
        Some(HostProgram {
            argv,
            env,
            portal: self.clone(),
        })
    }

    fn bus(&self) -> io::Result<Arc<Bus>> {
        match &self.0 {
            Route::Session => shared(),
            #[cfg(test)]
            Route::Fixed(bus) => bus
                .clone()
                .filter(|bus| !bus.lost())
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, UNAVAILABLE)),
        }
    }
}

impl HostProgram {
    /// Start it with `stdio` as its standard input, output and error. With
    /// `retry`, a dropped bus connection is made again and the start tried
    /// again: fine for reads, but a write might have started already.
    pub(crate) fn start(&self, stdio: [OwnedFd; 3], retry: bool) -> io::Result<HostChild> {
        let mut attempt = 0;
        loop {
            let bus = self.portal.bus()?;
            match bus.start(&self.argv, self.env.as_ref(), &stdio) {
                Ok(pid) => {
                    return Ok(HostChild {
                        pid,
                        bus,
                        status: None,
                    })
                }
                Err(error) if retry && attempt < 3 && transient(&error) => {
                    attempt += 1;
                    if let zbus::Error::InputOutput(_) = error {
                        bus.lose();
                    }
                    std::thread::sleep(Duration::from_millis(50 << attempt));
                }
                Err(error) => return Err(io_error(error)),
            }
        }
    }
}

/// The connection every host command shares; a lost one is made again.
fn shared() -> io::Result<Arc<Bus>> {
    static SHARED: Mutex<Option<Arc<Bus>>> = Mutex::new(None);
    // Held while connecting, so commands starting together share one try.
    let mut slot = SHARED.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(bus) = slot.as_ref().filter(|bus| !bus.lost()) {
        return Ok(bus.clone());
    }
    // Outside a real sandbox the host's own portal would run commands for
    // a copy that only thinks it is a Flatpak.
    if !Path::new("/.flatpak-info").exists() {
        return Err(io::Error::new(io::ErrorKind::NotConnected, UNAVAILABLE));
    }
    let bus = connect().and_then(Bus::open).map_err(|error| {
        io::Error::new(
            io::ErrorKind::NotConnected,
            format!("{UNAVAILABLE}: {error}"),
        )
    })?;
    *slot = Some(bus.clone());
    Ok(bus)
}

fn connect() -> zbus::Result<Connection> {
    let mut attempt = 0;
    loop {
        match Connection::session() {
            Err(error) if attempt < 5 && transient(&error) => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(20 << attempt));
            }
            result => return result,
        }
    }
}

/// Whether trying again could work: the connection broke or was turned
/// away, or the portal didn't answer. A portal that answered with an error
/// (no such program) gives the same answer again.
fn transient(error: &zbus::Error) -> bool {
    match error {
        zbus::Error::InputOutput(_) => true,
        zbus::Error::MethodError(name, _, _) => matches!(
            name.as_str(),
            "org.freedesktop.DBus.Error.NoReply"
                | "org.freedesktop.DBus.Error.ServiceUnknown"
                | "org.freedesktop.DBus.Error.NameHasNoOwner"
                | "org.freedesktop.DBus.Error.LimitsExceeded"
                | "org.freedesktop.DBus.Error.Disconnected"
                | "org.freedesktop.DBus.Error.Timeout"
                | "org.freedesktop.DBus.Error.TimedOut"
        ),
        _ => false,
    }
}

fn io_error(error: zbus::Error) -> io::Error {
    match error {
        // "Failed to start command: Failed to execute child process …"
        zbus::Error::MethodError(_, Some(message), _) => io::Error::other(message),
        zbus::Error::InputOutput(error) => io::Error::new(error.kind(), error.to_string()),
        error => io::Error::other(error.to_string()),
    }
}

/// Lost the connection to the portal, so a command's end can't be known.
fn lost() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "lost the connection to the Flatpak host bridge",
    )
}

/// One connection to the portal and what it said about commands ending.
pub(crate) struct Bus {
    connection: Connection,
    state: Mutex<State>,
    changed: Condvar,
}
#[derive(Default)]
struct State {
    /// Wait statuses of commands that ended and nobody collected yet.
    ended: HashMap<u32, i32>,
    /// Commands nobody waits for any more; their end is dropped.
    abandoned: HashSet<u32>,
    lost: bool,
}

impl Bus {
    fn open(connection: Connection) -> zbus::Result<Arc<Self>> {
        // Listen before the first command, so no end is missed.
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .path(PATH)?
            .interface(INTERFACE)?
            .member("HostCommandExited")?
            .build();
        let ends = MessageIterator::for_match_rule(rule, &connection, Some(1024))?;
        let bus = Arc::new(Self {
            connection,
            state: Mutex::default(),
            changed: Condvar::new(),
        });
        let weak = Arc::downgrade(&bus);
        std::thread::Builder::new()
            .name("pkgdeck-host-commands".into())
            .spawn(move || Self::listen(ends, weak))
            .map_err(|error| zbus::Error::InputOutput(Arc::new(error)))?;
        Ok(bus)
    }

    fn listen(ends: MessageIterator, bus: Weak<Self>) {
        for message in ends {
            let (Ok(message), Some(bus)) = (message, bus.upgrade()) else {
                break;
            };
            if let Ok((pid, status)) = message.body().deserialize::<(u32, u32)>() {
                bus.ended(pid, status as i32);
            }
        }
        if let Some(bus) = bus.upgrade() {
            bus.lose();
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn lost(&self) -> bool {
        self.state().lost
    }

    fn lose(&self) {
        self.state().lost = true;
        self.changed.notify_all();
    }

    fn ended(&self, pid: u32, status: i32) {
        let mut state = self.state();
        if !state.abandoned.remove(&pid) {
            state.ended.insert(pid, status);
        }
        drop(state);
        self.changed.notify_all();
    }

    fn start(
        &self,
        argv: &[OsString],
        env: Option<&BTreeMap<OsString, OsString>>,
        stdio: &[OwnedFd; 3],
    ) -> zbus::Result<u32> {
        // GLib reads these byte strings up to their NUL.
        let bytes = |text: &OsStr| {
            let mut bytes = text.as_bytes().to_vec();
            bytes.push(0);
            bytes
        };
        let argv: Vec<Vec<u8>> = argv.iter().map(|arg| bytes(arg)).collect();
        let fds: HashMap<u32, zbus::zvariant::Fd<'_>> = stdio
            .iter()
            .enumerate()
            .map(|(number, fd)| (number as u32, zbus::zvariant::Fd::from(fd)))
            .collect();
        let variables: HashMap<String, String> = env
            .into_iter()
            .flatten()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect();
        let flags = if env.is_some() { CLEAR_ENV } else { 0 };
        let reply = self.connection.call_method(
            Some(DESTINATION),
            PATH,
            Some(INTERFACE),
            "HostCommand",
            &(bytes(OsStr::new("/")), argv, fds, variables, flags),
        )?;
        reply.body().deserialize()
    }

    fn signal(&self, pid: u32, signal: i32) -> io::Result<()> {
        // Host commands lead their own process group, so the group is
        // the command and whatever it started.
        self.connection
            .call_method(
                Some(DESTINATION),
                PATH,
                Some(INTERFACE),
                "HostCommandSignal",
                &(pid, signal as u32, true),
            )
            .map(drop)
            .map_err(io_error)
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        // Ends the listening thread, which holds the connection too.
        let _ = self.connection.clone().close();
    }
}

/// A command running on the host.
pub(crate) struct HostChild {
    pid: u32,
    bus: Arc<Bus>,
    status: Option<ExitStatus>,
}

impl HostChild {
    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_none() {
            let mut state = self.bus.state();
            match state.ended.remove(&self.pid) {
                Some(status) => self.status = Some(ExitStatus::from_raw(status)),
                None if state.lost => return Err(lost()),
                None => {}
            }
        }
        Ok(self.status)
    }

    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        let mut state = self.bus.state();
        loop {
            if let Some(status) = state.ended.remove(&self.pid) {
                let status = ExitStatus::from_raw(status);
                self.status = Some(status);
                return Ok(status);
            }
            if state.lost {
                return Err(lost());
            }
            state = self
                .bus
                .changed
                .wait(state)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }

    /// Whether it ended (or can't be followed any more), without
    /// collecting its status.
    pub(crate) fn ended(&self) -> bool {
        let state = self.bus.state();
        self.status.is_some() || state.lost || state.ended.contains_key(&self.pid)
    }

    /// Send `signal` to its process group. Root's processes refuse it.
    pub(crate) fn signal(&self, signal: rustix::process::Signal) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        self.bus.signal(self.pid, signal.as_raw())
    }
}

impl Drop for HostChild {
    fn drop(&mut self) {
        if self.status.is_some() {
            return;
        }
        let mut state = self.bus.state();
        if state.ended.remove(&self.pid).is_none() && !state.lost {
            // A later command can get the same PID.
            state.abandoned.insert(self.pid);
        }
    }
}

/// A portal on a private connection that runs `bridge` here, given the
/// arguments `flatpak-spawn` would get, so tests can fake the host.
#[cfg(test)]
pub(crate) fn fake(bridge: &Path) -> Portal {
    if !bridge.is_file() {
        return Portal(Route::Fixed(None));
    }
    let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
    let guid = zbus::Guid::generate();
    let server = std::thread::spawn(move || {
        zbus::blocking::connection::Builder::async_io_unix_stream(server)
            .server(guid)
            .unwrap()
            .p2p()
            .build()
            .unwrap()
    });
    let client = zbus::blocking::connection::Builder::async_io_unix_stream(client)
        .p2p()
        .build()
        .unwrap();
    let server = server.join().unwrap();
    // Listening before the first call: a message nobody listens for yet is
    // dropped.
    let calls = MessageIterator::from(&server);
    let bridge = bridge.to_owned();
    std::thread::spawn(move || fake::serve(&server, calls, &bridge));
    Portal(Route::Fixed(Some(Bus::open(client).unwrap())))
}

#[cfg(test)]
mod fake {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    type HostCommand = (
        Vec<u8>,
        Vec<Vec<u8>>,
        HashMap<u32, zbus::zvariant::OwnedFd>,
        HashMap<String, String>,
        u32,
    );

    pub(super) fn serve(connection: &Connection, calls: MessageIterator, bridge: &Path) {
        for message in calls {
            let Ok(message) = message else { break };
            let header = message.header();
            if header.message_type() != Type::MethodCall {
                continue;
            }
            let replied = match header.member().map(|member| member.as_str()) {
                Some("HostCommand") => {
                    let call: HostCommand = message.body().deserialize().unwrap();
                    match start(connection, bridge, call) {
                        Ok(pid) => connection.reply(&header, &pid),
                        Err(error) => connection.reply_error(
                            &header,
                            "org.freedesktop.DBus.Error.Failed",
                            &format!("Failed to start command: {error}"),
                        ),
                    }
                }
                Some("HostCommandSignal") => {
                    let (pid, signal, _): (u32, u32, bool) = message.body().deserialize().unwrap();
                    let pid = rustix::process::Pid::from_raw(pid as i32).unwrap();
                    let signal = rustix::process::Signal::from_named_raw(signal as i32).unwrap();
                    match rustix::process::kill_process_group(pid, signal) {
                        Ok(()) => connection.reply(&header, &()),
                        Err(error) => connection.reply_error(
                            &header,
                            "org.freedesktop.DBus.Error.Failed",
                            &error.to_string(),
                        ),
                    }
                }
                _ => continue,
            };
            if replied.is_err() {
                break;
            }
        }
    }

    fn start(connection: &Connection, bridge: &Path, call: HostCommand) -> io::Result<u32> {
        let (directory, argv, mut fds, env, flags) = call;
        let text = |mut bytes: Vec<u8>| {
            assert_eq!(bytes.pop(), Some(0), "byte strings end in NUL");
            OsString::from(OsStr::from_bytes(&bytes))
        };
        let mut command = Command::new(bridge);
        command.arg("--host");
        if flags & CLEAR_ENV != 0 {
            command.arg("--clear-env");
        }
        let mut directory_arg = OsString::from("--directory=");
        directory_arg.push(text(directory));
        command.arg(directory_arg);
        let env: BTreeMap<_, _> = env.into_iter().collect();
        for (name, value) in env {
            command.arg(format!("--env={name}={value}"));
        }
        command.args(argv.into_iter().map(text));
        let mut stdio = |number: u32| {
            fds.remove(&number)
                .map(|fd| Stdio::from(OwnedFd::from(fd)))
                .unwrap_or_else(Stdio::null)
        };
        command
            .stdin(stdio(0))
            .stdout(stdio(1))
            .stderr(stdio(2))
            .process_group(0);
        let mut child = command.spawn()?;
        // The pipe ends this side had close with the command, as on the
        // real host: the caller only sees the end of output when it exits.
        drop(command);
        let pid = child.id();
        let connection = connection.clone();
        std::thread::spawn(move || {
            let status = child.wait().unwrap();
            let _ = connection.emit_signal(
                None::<&str>,
                PATH,
                INTERFACE,
                "HostCommandExited",
                &(pid, status.into_raw() as u32),
            );
        });
        Ok(pid)
    }
}
