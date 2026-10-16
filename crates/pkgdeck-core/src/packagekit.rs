//! Package lists refreshed through PackageKit, which needs no password.
//!
//! Refreshing APT's, Pacman's or Zypper's lists runs as root. PackageKit's
//! daemon already is, and polkit lets anyone at the computer refresh
//! through it (`org.freedesktop.packagekit.system-sources-refresh` is
//! allowed for active sessions). So a check refreshes there first, and
//! only asks for the password where PackageKit can't do it: not installed,
//! driving another manager, or not allowed without one (over SSH).
use std::sync::mpsc;
use std::time::{Duration, Instant};

use futures_lite::StreamExt;
use zbus::message::Type;
use zbus::wire::{OwnedObjectPath, OwnedVariant};
use zbus::MatchRule;
use zbus::{Connection, MessageStream};

use crate::process::{Cancellation, Completion, ExecutionError};

const DESTINATION: &str = "org.freedesktop.PackageKit";
const PATH: &str = "/org/freedesktop/PackageKit";
const INTERFACE: &str = "org.freedesktop.PackageKit";
const TRANSACTION: &str = "org.freedesktop.PackageKit.Transaction";
const REFRESH_ACTION: &str = "org.freedesktop.packagekit.system-sources-refresh";
/// `CanAuthorize`'s answer when no password is needed.
const AUTHORIZED: u32 = 1;
/// `Finished`'s exit codes.
const SUCCESS: u32 = 1;
const CANCELLED: u32 = 3;
/// A slow mirror can take minutes, but a refresh never takes this long.
const TIMEOUT: Duration = Duration::from_secs(600);
/// How long a daemon that ignores `Cancel` is waited on.
const CANCEL_GRACE: Duration = Duration::from_millis(if cfg!(test) { 200 } else { 5000 });

/// PackageKit's name for the backend that drives `source`, if it has one.
pub fn backend_for(source: &str) -> Option<&'static str> {
    match source {
        "apt" => Some("apt"),
        "pacman" => Some("alpm"),
        "zypper" => Some("zypp"),
        _ => None,
    }
}

/// Refresh the lists of `source` through PackageKit on the system bus.
/// `Ok(false)` when PackageKit can't do it without a password; the caller
/// then refreshes the usual way.
pub fn refresh(source: &str, cancel: &Cancellation) -> Result<bool, ExecutionError> {
    // In a container, the bus is the host's, and so are its lists.
    let contained = ["/run/.containerenv", "/.dockerenv"]
        .iter()
        .any(|marker| std::path::Path::new(marker).exists());
    refresh_with(source, cancel, contained, || {
        zbus::block_on(Connection::system())
    })
}
/// [`refresh`], with the bus from `connect` unless `contained`.
fn refresh_with(
    source: &str,
    cancel: &Cancellation,
    contained: bool,
    connect: impl FnOnce() -> zbus::Result<Connection>,
) -> Result<bool, ExecutionError> {
    match (backend_for(source), contained) {
        (Some(backend), false) => match connect() {
            Ok(bus) => refresh_on(&bus, backend, cancel, TIMEOUT),
            Err(_) => Ok(false),
        },
        _ => Ok(false),
    }
}

/// [`refresh`] over `bus`, giving up after `timeout`.
pub(crate) fn refresh_on(
    bus: &Connection,
    backend: &str,
    cancel: &Cancellation,
    timeout: Duration,
) -> Result<bool, ExecutionError> {
    if !can_refresh(bus, backend) {
        return Ok(false);
    }
    let transaction: OwnedObjectPath = call(bus, PATH, INTERFACE, "CreateTransaction", &())?;
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(TRANSACTION)
        .path(transaction.as_str())
        .build()
        .map_err(failed)?;
    let mut signals = zbus::block_on(MessageStream::for_match_rule(
        rule,
        bus,
        std::num::NonZeroUsize::new(16),
    ))
    .map_err(failed)?;
    // Without a password; a prompt would wait for nobody.
    call::<_, ()>(
        bus,
        transaction.as_str(),
        TRANSACTION,
        "SetHints",
        &(vec!["interactive=false"],),
    )?;
    let (sender, outcome) = mpsc::channel();
    std::thread::spawn(move || {
        let mut details = String::new();
        // Ends when the bus closes, which drops `sender`.
        while let Some(Ok(message)) = zbus::block_on(signals.next()) {
            let header = message.header();
            match header.member().map(|member| member.as_str()) {
                Some("ErrorCode") => {
                    if let Ok((_, text)) = message.body().deserialize::<(u32, String)>() {
                        details = text;
                    }
                }
                Some("Finished") => {
                    let exit = message
                        .body()
                        .deserialize::<(u32, u32)>()
                        .map(|(exit, _)| exit);
                    let _ = sender.send((exit.unwrap_or(0), details));
                    return;
                }
                _ => {}
            }
        }
    });
    call::<_, ()>(
        bus,
        transaction.as_str(),
        TRANSACTION,
        "RefreshCache",
        &(false,),
    )?;
    let deadline = Instant::now() + timeout;
    let mut stopping = false;
    let mut cancelled_at = Instant::now();
    // Why a refresh PackageKit was asked to stop ended.
    let stopped = || {
        if cancel.requested() {
            ExecutionError::Cancelled
        } else {
            ExecutionError::TimedOut
        }
    };
    loop {
        match outcome.recv_timeout(Duration::from_millis(50)) {
            Ok((SUCCESS, _)) => return Ok(true),
            Ok(_) if stopping || cancel.requested() => return Err(stopped()),
            Ok((CANCELLED, _)) => return Err(ExecutionError::Cancelled),
            Ok((_, details)) => return Err(failure(details)),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(failure("PackageKit stopped answering".into()))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let late = Instant::now() >= deadline;
        if (cancel.requested() || late) && !stopping {
            stopping = true;
            cancelled_at = Instant::now();
            let _ = call::<_, ()>(bus, transaction.as_str(), TRANSACTION, "Cancel", &());
        }
        // A daemon that ignores Cancel isn't waited on for long.
        if stopping && cancelled_at.elapsed() >= CANCEL_GRACE {
            return Err(stopped());
        }
    }
}

/// Whether PackageKit is there, drives `backend`, and may refresh without
/// asking for a password.
fn can_refresh(bus: &Connection, backend: &str) -> bool {
    let name = call::<_, OwnedVariant>(
        bus,
        PATH,
        "org.freedesktop.DBus.Properties",
        "Get",
        &(INTERFACE, "BackendName"),
    )
    .ok()
    .and_then(|value| String::try_from(value).ok());
    name.as_deref() == Some(backend)
        && call::<_, u32>(bus, PATH, INTERFACE, "CanAuthorize", &(REFRESH_ACTION,))
            .is_ok_and(|answer| answer == AUTHORIZED)
}

fn call<B, R>(
    bus: &Connection,
    path: &str,
    interface: &str,
    method: &str,
    body: &B,
) -> Result<R, ExecutionError>
where
    B: serde::Serialize + zbus::wire::DynamicType,
    R: for<'d> zbus::wire::DynamicDeserialize<'d>,
{
    zbus::block_on(bus.call_method(Some(DESTINATION), path, Some(interface), method, body))
        .and_then(|reply| reply.body().deserialize::<R>())
        .map_err(failed)
}

fn failed(error: impl std::fmt::Display) -> ExecutionError {
    failure(error.to_string())
}

/// A refresh that ended badly, with what PackageKit said about it.
fn failure(details: String) -> ExecutionError {
    ExecutionError::Failed(Completion {
        code: None,
        signal: None,
        stdout: vec![],
        stderr: format!("PackageKit: {details}").into_bytes(),
        truncated: false,
        cancellation_deferred: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};
    use zbus::connection::Builder;

    /// How the fake daemon behaves.
    #[derive(Clone)]
    struct Daemon {
        backend: &'static str,
        authorize: u32,
        /// What `RefreshCache` ends with: the exit code and an error, or no
        /// answer at all.
        finish: Option<(u32, Option<&'static str>)>,
        /// Whether `Cancel` ends the refresh.
        honours_cancel: bool,
        /// A method that answers with an error.
        fails: &'static str,
        /// Whether the daemon goes away once the refresh starts.
        hangs_up: bool,
        /// How long the refresh takes.
        takes: Duration,
    }

    impl Default for Daemon {
        fn default() -> Self {
            Self {
                backend: "apt",
                authorize: AUTHORIZED,
                finish: Some((SUCCESS, None)),
                honours_cancel: true,
                fails: "",
                hangs_up: false,
                takes: Duration::ZERO,
            }
        }
    }

    /// A client connected to a fake PackageKit, and the calls it got.
    fn serve(daemon: Daemon) -> (Connection, Arc<Mutex<Vec<String>>>) {
        let (client, server) = UnixStream::pair().unwrap();
        let guid = zbus::Guid::generate();
        let server = std::thread::spawn(move || {
            zbus::block_on(Builder::unix_stream(server).server(guid).p2p().build()).unwrap()
        });
        let client = zbus::block_on(Builder::unix_stream(client).p2p().build()).unwrap();
        let server = server.join().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let mut messages = MessageStream::from(&server);
        std::thread::spawn(move || {
            const TID: &str = "/1_abc";
            let finish = |exit: u32| {
                let _ = zbus::block_on(server.emit_signal(
                    None::<()>,
                    TID,
                    TRANSACTION,
                    "Finished",
                    &(exit, 5u32),
                ));
            };
            let calls = std::iter::from_fn(|| zbus::block_on(messages.next()))
                .map_while(Result::ok)
                .filter(|message| message.header().message_type() == Type::MethodCall);
            for message in calls {
                let header = message.header();
                let member = header.member().unwrap().to_string();
                seen.lock().unwrap().push(member.clone());
                if member == daemon.fails {
                    let _ = zbus::block_on(server.reply_error(
                        &header,
                        "org.freedesktop.DBus.Error.Failed",
                        &"refused",
                    ));
                    continue;
                }
                if member == "RefreshCache" && daemon.hangs_up {
                    let _ = zbus::block_on(server.reply(&header, &()));
                    // Dropping the last handle closes the connection.
                    return;
                }
                let _ = match member.as_str() {
                    "Get" => zbus::block_on(
                        server.reply(&header, &zbus::wire::Variant::from(daemon.backend)),
                    ),
                    "CanAuthorize" => zbus::block_on(server.reply(&header, &daemon.authorize)),
                    "CreateTransaction" => zbus::block_on(
                        server.reply(&header, &zbus::wire::ObjectPath::try_from(TID).unwrap()),
                    ),
                    "RefreshCache" => {
                        let replied = zbus::block_on(server.reply(&header, &()));
                        // Progress the client doesn't follow.
                        let _ = zbus::block_on(server.emit_signal(
                            None::<()>,
                            TID,
                            TRANSACTION,
                            "ItemProgress",
                            &("id", 1u32, 50u32),
                        ));
                        std::thread::sleep(daemon.takes);
                        if let Some((exit, error)) = daemon.finish {
                            if let Some(error) = error {
                                let _ = zbus::block_on(server.emit_signal(
                                    None::<()>,
                                    TID,
                                    TRANSACTION,
                                    "ErrorCode",
                                    &(7u32, error),
                                ));
                            }
                            finish(exit);
                        }
                        replied
                    }
                    "Cancel" => {
                        let replied = zbus::block_on(server.reply(&header, &()));
                        if daemon.honours_cancel {
                            finish(CANCELLED);
                        }
                        replied
                    }
                    _ => zbus::block_on(server.reply(&header, &())),
                };
            }
        });
        (client, calls)
    }

    fn calls(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        seen.lock().unwrap().clone()
    }

    #[test]
    fn a_refresh_runs_without_asking_and_succeeds() {
        let (bus, seen) = serve(Daemon::default());
        let cancel = Cancellation::default();
        assert_eq!(refresh_on(&bus, "apt", &cancel, TIMEOUT), Ok(true));
        assert_eq!(
            calls(&seen),
            [
                "Get",
                "CanAuthorize",
                "CreateTransaction",
                "SetHints",
                "RefreshCache"
            ]
        );
    }

    #[test]
    fn another_manager_or_a_password_leaves_it_to_the_caller() {
        let cancel = Cancellation::default();
        let (bus, seen) = serve(Daemon {
            backend: "dnf",
            ..Daemon::default()
        });
        assert_eq!(refresh_on(&bus, "apt", &cancel, TIMEOUT), Ok(false));
        assert_eq!(calls(&seen), ["Get"]);
        // Over SSH polkit would ask (3), or refuse (2).
        for answer in [0, 2, 3] {
            let (bus, seen) = serve(Daemon {
                authorize: answer,
                ..Daemon::default()
            });
            assert_eq!(refresh_on(&bus, "apt", &cancel, TIMEOUT), Ok(false));
            assert_eq!(calls(&seen), ["Get", "CanAuthorize"]);
        }
    }

    #[test]
    fn a_failed_refresh_says_why() {
        let (bus, _) = serve(Daemon {
            finish: Some((2, Some("Failed to fetch http://mirror/Release"))),
            ..Daemon::default()
        });
        let error = refresh_on(&bus, "apt", &Cancellation::default(), TIMEOUT).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Failed to fetch http://mirror/Release"),
            "{error}"
        );
    }

    #[test]
    fn cancelling_stops_the_refresh() {
        let (bus, seen) = serve(Daemon {
            finish: None,
            ..Daemon::default()
        });
        let cancel = Cancellation::default();
        let flag = cancel.flag();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        assert_eq!(
            refresh_on(&bus, "apt", &cancel, TIMEOUT),
            Err(ExecutionError::Cancelled)
        );
        assert_eq!(calls(&seen).last().map(String::as_str), Some("Cancel"));
    }

    #[test]
    fn a_refresh_that_never_ends_times_out() {
        let (bus, seen) = serve(Daemon {
            finish: None,
            honours_cancel: false,
            ..Daemon::default()
        });
        let started = Instant::now();
        let result = refresh_on(
            &bus,
            "apt",
            &Cancellation::default(),
            Duration::from_millis(50),
        );
        assert_eq!(result, Err(ExecutionError::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(calls(&seen).contains(&"Cancel".to_owned()));
    }

    #[test]
    fn a_refresh_stopped_for_taking_too_long_says_so() {
        let (bus, _) = serve(Daemon {
            finish: None,
            ..Daemon::default()
        });
        let result = refresh_on(
            &bus,
            "apt",
            &Cancellation::default(),
            Duration::from_millis(50),
        );
        assert_eq!(result, Err(ExecutionError::TimedOut));
    }

    #[test]
    fn outside_a_container_the_system_bus_is_asked() {
        let cancel = Cancellation::default();
        let fake = || Ok(serve(Daemon::default()).0);
        assert_eq!(refresh_with("apt", &cancel, false, fake), Ok(true));
        // In a container the bus is the host's, and so are its lists.
        let never = || -> zbus::Result<Connection> { panic!("asked the bus") };
        assert_eq!(refresh_with("apt", &cancel, true, never), Ok(false));
        assert_eq!(refresh_with("flatpak", &cancel, false, never), Ok(false));
        let absent = || Err(zbus::Error::Unsupported);
        assert_eq!(refresh_with("apt", &cancel, false, absent), Ok(false));
        // Sources PackageKit doesn't refresh never reach the bus.
        assert_eq!(refresh("flatpak", &cancel), Ok(false));
    }

    #[test]
    fn a_refused_step_or_a_refresh_cancelled_elsewhere_ends_it() {
        let cancel = Cancellation::default();
        for step in ["SetHints", "RefreshCache"] {
            let (bus, _) = serve(Daemon {
                fails: step,
                ..Daemon::default()
            });
            let error = refresh_on(&bus, "apt", &cancel, TIMEOUT).unwrap_err();
            assert!(error.to_string().contains("refused"), "{step}: {error}");
        }
        // Someone else cancelled it, say in another app.
        let (bus, _) = serve(Daemon {
            finish: Some((CANCELLED, None)),
            ..Daemon::default()
        });
        assert_eq!(
            refresh_on(&bus, "apt", &cancel, TIMEOUT),
            Err(ExecutionError::Cancelled)
        );
    }

    #[test]
    fn a_slow_refresh_is_waited_for_until_its_deadline() {
        let (bus, seen) = serve(Daemon {
            takes: CANCEL_GRACE * 2,
            ..Daemon::default()
        });
        let cancel = Cancellation::default();
        assert_eq!(refresh_on(&bus, "apt", &cancel, TIMEOUT), Ok(true));
        assert!(!calls(&seen).contains(&"Cancel".to_string()));
    }

    #[test]
    fn a_daemon_that_hangs_up_is_a_failure() {
        let (bus, _) = serve(Daemon {
            hangs_up: true,
            ..Daemon::default()
        });
        let error = refresh_on(&bus, "apt", &Cancellation::default(), TIMEOUT).unwrap_err();
        assert!(error.to_string().contains("stopped answering"), "{error}");
    }

    #[test]
    fn a_daemon_that_goes_away_is_a_failure() {
        let (bus, _) = serve(Daemon {
            finish: Some((6, None)),
            ..Daemon::default()
        });
        assert!(matches!(
            refresh_on(&bus, "apt", &Cancellation::default(), TIMEOUT),
            Err(ExecutionError::Failed(_))
        ));
    }

    #[test]
    fn only_managers_packagekit_drives_have_a_backend() {
        assert_eq!(backend_for("apt"), Some("apt"));
        assert_eq!(backend_for("pacman"), Some("alpm"));
        assert_eq!(backend_for("zypper"), Some("zypp"));
        for source in ["dnf", "apk", "xbps", "snap", "flatpak", "homebrew"] {
            assert_eq!(backend_for(source), None, "{source}");
            assert_eq!(refresh(source, &Cancellation::default()), Ok(false));
        }
    }
}
