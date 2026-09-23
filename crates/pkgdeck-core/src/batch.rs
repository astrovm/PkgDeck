//! One narrowly scoped authorization process for a confirmed native batch.
//! The elevated process derives its own command list from typed operations;
//! the frontend may only request each listed command once and in order.
use crate::{
    host::{AptAction, Authorization, Host, Runtime},
    package::{Operation, PackageId, Scope},
    process::{self, Cancellation, Completion, ExecutionError, Limits},
};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    ffi::OsString,
    fs,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};

const PROTOCOL: u8 = 1;
const SYSTEM_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";
const MAX_MESSAGE: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProtectedCommand {
    pub program: String,
    pub args: Vec<String>,
}
impl ProtectedCommand {
    fn new(program: &str, args: impl IntoIterator<Item = String>) -> Self {
        Self {
            program: format!("/usr/bin/{program}"),
            args: args.into_iter().collect(),
        }
    }
}

fn invalid(reason: &str) -> ExecutionError {
    ExecutionError::Invalid(reason.into())
}
fn identifier(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+@-".contains(&b))
}
fn flatpak_identifier(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
fn system_id<'a>(id: &'a PackageId, backend: &str) -> Result<&'a str, ExecutionError> {
    if id.backend != backend || id.scope != Scope::System || !identifier(&id.name) {
        return Err(invalid("invalid system package identity"));
    }
    Ok(&id.name)
}
fn apt_target(id: &PackageId) -> Result<String, ExecutionError> {
    let name = system_id(id, "apt")?;
    let target = format!("{name}:{}", id.architecture);
    AptAction::Install(target.clone()).arguments()?;
    Ok(target)
}
fn apt_command(action: AptAction) -> Result<ProtectedCommand, ExecutionError> {
    Ok(ProtectedCommand::new(
        "apt-get",
        action
            .arguments()?
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned()),
    ))
}
fn flatpak_ref(id: &PackageId) -> Result<&str, ExecutionError> {
    if id.backend != "flatpak"
        || !flatpak_identifier(&id.name)
        || !flatpak_identifier(&id.architecture)
        || id.scope != Scope::System
    {
        return Err(invalid("invalid system Flatpak identity"));
    }
    if let Some(reference) = &id.reference {
        let fields: Vec<_> = reference.split('/').collect();
        let valid = match fields.as_slice() {
            [kind, name, arch, branch] => {
                matches!(*kind, "app" | "runtime")
                    && *name == id.name
                    && *arch == id.architecture
                    && flatpak_identifier(branch)
            }
            [name, arch, branch] => {
                *name == id.name && *arch == id.architecture && flatpak_identifier(branch)
            }
            _ => false,
        };
        if !valid {
            return Err(invalid("invalid native Flatpak ref"));
        }
        Ok(reference)
    } else {
        Ok(&id.name)
    }
}

/// Commands are generated only from typed identities and fixed manager verbs.
/// User-scoped commands have no entry in the elevated plan.
pub fn protected_commands(operation: &Operation) -> Result<Vec<ProtectedCommand>, ExecutionError> {
    let command = match operation {
        Operation::Refresh { backend } if backend == "apt" => apt_command(AptAction::Refresh)?,
        Operation::UpgradeAll { backend } if backend == "apt" => {
            apt_command(AptAction::UpgradeAll)?
        }
        Operation::Install(id) if id.backend == "apt" => {
            apt_command(AptAction::Install(apt_target(id)?))?
        }
        Operation::Remove(id) if id.backend == "apt" => {
            apt_command(AptAction::Remove(apt_target(id)?))?
        }
        Operation::Upgrade(id) if id.backend == "apt" => {
            apt_command(AptAction::Upgrade(apt_target(id)?))?
        }
        Operation::Clean(id) if id.backend == "apt" => match id.key.as_str() {
            "autoremove" => apt_command(AptAction::Autoremove)?,
            "autoclean" => apt_command(AptAction::Autoclean)?,
            _ => return Err(invalid("unknown APT cleanup task")),
        },
        Operation::Refresh { backend } if backend == "flatpak" => ProtectedCommand::new(
            "flatpak",
            [
                "--system",
                "update",
                "--no-deploy",
                "--noninteractive",
                "--assumeyes",
            ]
            .map(str::to_owned),
        ),
        Operation::UpgradeAll { backend } if backend == "flatpak" => ProtectedCommand::new(
            "flatpak",
            ["--system", "update", "--noninteractive", "--assumeyes"].map(str::to_owned),
        ),
        Operation::Clean(id) if id.backend == "flatpak" && id.key == "unused-system" => {
            ProtectedCommand::new(
                "flatpak",
                [
                    "--system",
                    "uninstall",
                    "--unused",
                    "--noninteractive",
                    "--assumeyes",
                ]
                .map(str::to_owned),
            )
        }
        Operation::Install(id) | Operation::Remove(id) | Operation::Upgrade(id)
            if id.backend == "flatpak" && id.scope == Scope::System =>
        {
            let reference = flatpak_ref(id)?;
            let verb = match operation {
                Operation::Install(_) => "install",
                Operation::Remove(_) => "uninstall",
                _ => "update",
            };
            let mut args = vec![
                "--system".into(),
                verb.into(),
                "--noninteractive".into(),
                "--assumeyes".into(),
            ];
            if reference.starts_with("runtime/") {
                args.push("--runtime".into());
            } else if reference.starts_with("app/") || id.reference.is_none() {
                args.push("--app".into());
            }
            if verb == "install" {
                let remote = id.remote.as_deref().unwrap_or("flathub");
                if !flatpak_identifier(remote) {
                    return Err(invalid("invalid Flatpak remote"));
                }
                args.push(remote.into());
            }
            args.push(reference.into());
            ProtectedCommand::new("flatpak", args)
        }
        Operation::Refresh { backend } if backend == "fwupd" => firmware_command("refresh", None)?,
        Operation::UpgradeAll { backend } if backend == "fwupd" => {
            firmware_command("update", None)?
        }
        Operation::Upgrade(id) if id.backend == "fwupd" => {
            firmware_command("update", Some(&id.name))?
        }
        Operation::Refresh { backend } | Operation::UpgradeAll { backend }
            if matches!(backend.as_str(), "dnf" | "pacman" | "zypper" | "snap") =>
        {
            manager_command(backend, operation, None)?
        }
        Operation::Install(id) | Operation::Remove(id) | Operation::Upgrade(id)
            if matches!(id.backend.as_str(), "dnf" | "pacman" | "zypper" | "snap") =>
        {
            let name = system_id(id, &id.backend)?;
            manager_command(&id.backend, operation, Some(name))?
        }
        _ => return Ok(vec![]),
    };
    Ok(vec![command])
}

fn apt_group_action(operation: &Operation) -> Result<Option<AptAction>, ExecutionError> {
    match operation {
        Operation::Install(id) if id.backend == "apt" => {
            Ok(Some(AptAction::Install(apt_target(id)?)))
        }
        Operation::Remove(id) if id.backend == "apt" => {
            Ok(Some(AptAction::Remove(apt_target(id)?)))
        }
        Operation::Upgrade(id) if id.backend == "apt" => {
            Ok(Some(AptAction::Upgrade(apt_target(id)?)))
        }
        _ => Ok(None),
    }
}

pub fn batch_commands(
    operations: &[Operation],
) -> Result<Vec<Vec<ProtectedCommand>>, ExecutionError> {
    let mut commands = operations
        .iter()
        .map(protected_commands)
        .collect::<Result<Vec<_>, _>>()?;
    let mut index = 0;
    while index < operations.len() {
        let Some(first) = apt_group_action(&operations[index])? else {
            index += 1;
            continue;
        };
        let mut actions = vec![first];
        let mut end = index + 1;
        while end < operations.len()
            && std::mem::discriminant(&operations[end])
                == std::mem::discriminant(&operations[index])
        {
            let Some(action) = apt_group_action(&operations[end])? else {
                break;
            };
            actions.push(action);
            end += 1;
        }
        if actions.len() > 1 {
            let args = AptAction::group_arguments(&actions)?
                .into_iter()
                .map(|arg| arg.to_string_lossy().into_owned());
            commands[index] = vec![ProtectedCommand::new("apt-get", args)];
            for entry in &mut commands[index + 1..end] {
                entry.clear();
            }
        }
        index = end;
    }
    Ok(commands)
}

fn firmware_command(verb: &str, id: Option<&str>) -> Result<ProtectedCommand, ExecutionError> {
    let mut args = vec![
        "--assume-yes".into(),
        "--no-reboot-check".into(),
        "--no-unreported-check".into(),
        verb.into(),
    ];
    if let Some(id) = id {
        if id.len() != 40 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid("invalid firmware device ID"));
        }
        args.push(id.into());
    }
    Ok(ProtectedCommand::new("fwupdmgr", args))
}
fn manager_command(
    backend: &str,
    operation: &Operation,
    name: Option<&str>,
) -> Result<ProtectedCommand, ExecutionError> {
    let verb = match operation {
        Operation::Refresh { .. } => "refresh",
        Operation::Install(_) => "install",
        Operation::Remove(_) => "remove",
        Operation::Upgrade(_) => "upgrade",
        Operation::UpgradeAll { .. } => "all",
        _ => return Err(invalid("unsupported system manager operation")),
    };
    let fixed: &[&str] = match (backend, verb) {
        ("dnf", "refresh") => &["-y", "makecache"],
        ("dnf", "install") => &["-y", "install", "--"],
        ("dnf", "remove") => &["-y", "remove", "--"],
        ("dnf", "upgrade") => &["-y", "upgrade", "--"],
        ("dnf", "all") => &["-y", "upgrade"],
        ("pacman", "refresh") => &["-Sy", "--noconfirm"],
        ("pacman", "install" | "upgrade") => &["-S", "--noconfirm", "--needed", "--"],
        ("pacman", "remove") => &["-Rns", "--noconfirm", "--"],
        ("pacman", "all") => &["-Su", "--noconfirm"],
        ("zypper", "refresh") => &["--non-interactive", "refresh"],
        ("zypper", "install") => &[
            "--non-interactive",
            "install",
            "--auto-agree-with-licenses",
            "--",
        ],
        ("zypper", "remove") => &["--non-interactive", "remove", "--"],
        ("zypper", "upgrade") => &["--non-interactive", "update", "--"],
        ("zypper", "all") => &["--non-interactive", "update"],
        ("snap", "refresh" | "upgrade" | "all") => &["refresh"],
        ("snap", "install") => &["install"],
        ("snap", "remove") => &["remove"],
        _ => return Err(invalid("unsupported system manager")),
    };
    let mut args = fixed
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect::<Vec<_>>();
    if let Some(name) = name {
        args.push(name.into());
    }
    Ok(ProtectedCommand::new(backend, args))
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Request {
    Start {
        protocol: u8,
        operations: Vec<Operation>,
    },
    Run {
        operation: usize,
        command: usize,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Response {
    Ready { protocol: u8 },
    Completed { completion: WireCompletion },
    Rejected { reason: String },
}
#[derive(Serialize, Deserialize)]
struct WireCompletion {
    code: Option<i32>,
    signal: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
    cancellation_deferred: bool,
}
impl From<Completion> for WireCompletion {
    fn from(value: Completion) -> Self {
        Self {
            code: value.code,
            signal: value.signal,
            stdout: value.stdout,
            stderr: value.stderr,
            truncated: value.truncated,
            cancellation_deferred: value.cancellation_deferred,
        }
    }
}
impl From<WireCompletion> for Completion {
    fn from(value: WireCompletion) -> Self {
        Self {
            code: value.code,
            signal: value.signal,
            stdout: value.stdout,
            stderr: value.stderr,
            truncated: value.truncated,
            cancellation_deferred: value.cancellation_deferred,
        }
    }
}

fn send<T: Serialize>(pipe: &mut impl Write, value: &T) -> Result<(), ExecutionError> {
    let mut bytes = serde_json::to_vec(value).map_err(|e| ExecutionError::Io(e.to_string()))?;
    if bytes.len() > MAX_MESSAGE {
        return Err(invalid("authorization message too large"));
    }
    bytes.push(b'\n');
    pipe.write_all(&bytes)?;
    pipe.flush()?;
    Ok(())
}
fn line(pipe: &mut impl Read) -> Result<Vec<u8>, ExecutionError> {
    let mut line = Vec::new();
    let mut byte = [0];
    loop {
        match pipe.read(&mut byte)? {
            0 => {
                return Err(ExecutionError::Io(
                    "authorization runner closed unexpectedly".into(),
                ))
            }
            _ if byte[0] == b'\n' => return Ok(line),
            _ => {
                line.push(byte[0]);
                if line.len() > MAX_MESSAGE {
                    return Err(invalid("authorization message too large"));
                }
            }
        }
    }
}

pub struct ScopeGuard;
struct Session {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    commands: Vec<Vec<ProtectedCommand>>,
    current: usize,
    next: Vec<usize>,
}
thread_local! { static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) }; }

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        SESSION.with(|cell| {
            if let Some(mut session) = cell.borrow_mut().take() {
                drop(session.input);
                // A native write has already completed when the scope ends.
                if session.child.try_wait().ok().flatten().is_none() {
                    let _ = session.child.kill();
                }
                let _ = session.child.wait();
            }
        });
    }
}
fn root_owned(path: &Path) -> bool {
    let Ok(canonical) = fs::canonicalize(path) else {
        return false;
    };
    let Ok(metadata) = fs::metadata(&canonical) else {
        return false;
    };
    metadata.is_file()
        && canonical.ancestors().skip(1).all(|parent| {
            fs::metadata(parent).is_ok_and(|info| info.uid() == 0 && info.mode() & 0o022 == 0)
        })
        && metadata.uid() == 0
        && metadata.mode() & 0o022 == 0
}
fn runner_path(host: &Host) -> Option<PathBuf> {
    let system = PathBuf::from("/usr/libexec/pkgdeck-host-runner");
    if host.runtime == Runtime::Flatpak {
        // A bundled sandbox executable is not a trusted host executable.
        if root_owned(Path::new("/run/host/usr/libexec/pkgdeck-host-runner")) {
            return Some(system);
        }
        return None;
    }
    if root_owned(&system) {
        return Some(system);
    }
    if host.runtime == Runtime::Snap {
        let snap = std::env::var_os("SNAP")?;
        let path = PathBuf::from(snap).join("usr/libexec/pkgdeck-host-runner");
        if fs::canonicalize(&path).is_ok_and(|canonical| canonical.starts_with("/snap/"))
            && root_owned(&path)
        {
            return Some(path);
        }
    }
    None
}
pub fn begin(
    host: &Host,
    authorization: Authorization,
    operations: &[Operation],
    cancel: &Cancellation,
) -> Result<Option<ScopeGuard>, ExecutionError> {
    if SESSION.with(|cell| cell.borrow().is_some()) {
        return Err(invalid("nested authorization batch"));
    }
    if cancel.requested() {
        return Err(ExecutionError::Cancelled);
    }
    let commands = batch_commands(operations)?;
    if !commands.iter().any(|entry| !entry.is_empty()) {
        return Ok(None);
    }
    let Some(path) = runner_path(host) else {
        return Ok(None);
    };
    let (program, args) = authorization.prefix(&path);
    let mut command = host.command(Path::new(program), &args)?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|e| ExecutionError::Io(e.to_string()))?;
    let mut input = child.stdin.take().expect("piped stdin");
    let output = child.stdout.take().expect("piped stdout");
    if let Err(error) = send(
        &mut input,
        &Request::Start {
            protocol: PROTOCOL,
            operations: operations.to_vec(),
        },
    ) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let mut session = Session {
        child,
        input,
        output,
        next: vec![0; commands.len()],
        commands,
        current: 0,
    };
    let start = Instant::now();
    let flags =
        rustix::fs::fcntl_getfl(&session.output).map_err(|e| ExecutionError::Io(e.to_string()))?;
    rustix::fs::fcntl_setfl(&session.output, flags | rustix::fs::OFlags::NONBLOCK)
        .map_err(|e| ExecutionError::Io(e.to_string()))?;
    let mut frame = Vec::new();
    let ready = loop {
        if cancel.requested() {
            let _ = session.child.kill();
            let _ = session.child.wait();
            return Err(ExecutionError::AuthorizationCancelled);
        }
        if start.elapsed() > Duration::from_secs(120) {
            let _ = session.child.kill();
            let _ = session.child.wait();
            return Err(ExecutionError::AuthorizationDenied);
        }
        if let Some(status) = session.child.try_wait()? {
            return Err(match status.code() {
                Some(126) => ExecutionError::AuthorizationCancelled,
                _ => ExecutionError::AuthorizationDenied,
            });
        }
        // The ready frame is short and written before any native command.
        // stdout is nonblocking so cancellation can dismiss a pending prompt.
        let mut chunk = [0; 256];
        match session.output.read(&mut chunk) {
            Ok(0) => return Err(ExecutionError::AuthorizationDenied),
            Ok(count) => {
                frame.extend_from_slice(&chunk[..count]);
                if frame.len() > MAX_MESSAGE {
                    return Err(invalid("authorization message too large"));
                }
                if let Some(end) = frame.iter().position(|byte| *byte == b'\n') {
                    break serde_json::from_slice::<Response>(&frame[..end])
                        .map_err(|e| ExecutionError::Io(e.to_string()))?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(error) => return Err(error.into()),
        }
    };
    let flags =
        rustix::fs::fcntl_getfl(&session.output).map_err(|e| ExecutionError::Io(e.to_string()))?;
    rustix::fs::fcntl_setfl(&session.output, flags & !rustix::fs::OFlags::NONBLOCK)
        .map_err(|e| ExecutionError::Io(e.to_string()))?;
    if !matches!(ready, Response::Ready { protocol: PROTOCOL }) {
        return Err(ExecutionError::AuthorizationDenied);
    }
    SESSION.with(|cell| {
        *cell.borrow_mut() = Some(session);
        Ok(Some(ScopeGuard))
    })
}

pub fn set_operation(index: usize) {
    SESSION.with(|cell| {
        if let Some(session) = cell.borrow_mut().as_mut() {
            session.current = index;
        }
    });
}
pub fn run_in_scope(
    executable: &Path,
    args: &[OsString],
    cancel: &Cancellation,
) -> Option<Result<Completion, ExecutionError>> {
    SESSION.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let session = borrow.as_mut()?;
        let index = session.current;
        let Some(next) = session.next.get(index).copied() else {
            return Some(Err(invalid("protected command outside approved batch")));
        };
        let Some(expected) = session
            .commands
            .get(index)
            .and_then(|commands| commands.get(next))
        else {
            return Some(Err(invalid("protected command outside approved batch")));
        };
        let actual = ProtectedCommand {
            program: executable.to_string_lossy().into_owned(),
            args: args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        };
        if expected != &actual {
            return Some(Err(invalid(
                "protected command differs from approved batch",
            )));
        }
        if cancel.requested() {
            return Some(Err(ExecutionError::Cancelled));
        }
        let result = (|| {
            send(
                &mut session.input,
                &Request::Run {
                    operation: index,
                    command: next,
                },
            )?;
            let response: Response = serde_json::from_slice(&line(&mut session.output)?)
                .map_err(|e| ExecutionError::Io(e.to_string()))?;
            match response {
                Response::Completed { completion } => {
                    session.next[index] += 1;
                    let mut result: Completion = completion.into();
                    result.cancellation_deferred = cancel.requested();
                    Ok(result)
                }
                Response::Rejected { reason } => Err(invalid(&reason)),
                _ => Err(invalid("unexpected authorization response")),
            }
        })();
        Some(result)
    })
}

/// Entry point for the root-owned binary. No executable path or argument list
/// is read from the frontend after its typed plan has been validated.
pub fn serve() -> Result<(), ExecutionError> {
    if !rustix::process::geteuid().is_root() {
        return Err(invalid("host runner requires root"));
    }
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    serve_protocol(&mut input, &mut output, |expected| {
        let mut native = Command::new(&expected.program);
        native
            .args(&expected.args)
            .env_clear()
            .env("PATH", SYSTEM_PATH)
            .env("LC_ALL", "C")
            .current_dir("/");
        process::run(native, Limits::default(), &Cancellation::default(), true)
    })
}
fn serve_protocol(
    mut input: &mut impl Read,
    mut output: &mut impl Write,
    mut run: impl FnMut(&ProtectedCommand) -> Result<Completion, ExecutionError>,
) -> Result<(), ExecutionError> {
    let start: Request =
        serde_json::from_slice(&line(&mut input)?).map_err(|e| invalid(&e.to_string()))?;
    let operations = match start {
        Request::Start {
            protocol: PROTOCOL,
            operations,
        } if !operations.is_empty() && operations.len() <= 1024 => operations,
        _ => return Err(invalid("invalid authorization protocol or plan")),
    };
    let commands = batch_commands(&operations)?;
    if !commands.iter().any(|entry| !entry.is_empty()) {
        return Err(invalid("empty protected plan"));
    }
    send(&mut output, &Response::Ready { protocol: PROTOCOL })?;
    let mut next = vec![0; commands.len()];
    let mut last_operation = 0;
    while let Ok(bytes) = line(&mut input) {
        let request: Request =
            serde_json::from_slice(&bytes).map_err(|e| invalid(&e.to_string()))?;
        let Request::Run { operation, command } = request else {
            return Err(invalid("runner already initialized"));
        };
        if operation < last_operation
            || operation >= commands.len()
            || command != next[operation]
            || command >= commands[operation].len()
        {
            send(
                &mut output,
                &Response::Rejected {
                    reason: "command outside approved batch".into(),
                },
            )?;
            return Err(invalid("command outside approved batch"));
        }
        last_operation = operation;
        next[operation] += 1;
        let expected = &commands[operation][command];
        let completion = run(expected)?;
        send(
            &mut output,
            &Response::Completed {
                completion: completion.into(),
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn id(backend: &str, name: &str) -> PackageId {
        PackageId {
            backend: backend.into(),
            name: name.into(),
            architecture: "amd64".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        }
    }
    fn input(requests: &[Request]) -> Vec<u8> {
        let mut input = Vec::new();
        for request in requests {
            send(&mut input, request).unwrap();
        }
        input
    }
    fn completion() -> Completion {
        Completion {
            code: Some(0),
            signal: None,
            stdout: b"done".to_vec(),
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
        }
    }

    #[test]
    fn mixed_manager_scope_runs_exact_typed_commands_once() {
        let operations = vec![
            Operation::Install(id("apt", "synthetic-pkg")),
            Operation::Upgrade(id("snap", "synthetic-snap")),
            Operation::Upgrade(id("flatpak", "org.example.Test")),
        ];
        let requests = [
            Request::Start {
                protocol: PROTOCOL,
                operations: operations.clone(),
            },
            Request::Run {
                operation: 0,
                command: 0,
            },
            Request::Run {
                operation: 1,
                command: 0,
            },
            Request::Run {
                operation: 2,
                command: 0,
            },
        ];
        let mut seen = Vec::new();
        let mut output = Vec::new();
        serve_protocol(&mut Cursor::new(input(&requests)), &mut output, |command| {
            seen.push(command.clone());
            Ok(completion())
        })
        .unwrap();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].program, "/usr/bin/apt-get");
        assert_eq!(seen[1].program, "/usr/bin/snap");
        assert_eq!(seen[2].program, "/usr/bin/flatpak");
        let replies = output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<Response>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(replies.len(), 4);
        assert!(matches!(replies[0], Response::Ready { protocol: PROTOCOL }));
    }

    #[test]
    fn consecutive_apt_targets_are_one_approved_native_command() {
        let operations = vec![
            Operation::Install(id("apt", "synthetic-one")),
            Operation::Install(id("apt", "synthetic-two")),
        ];
        let commands = batch_commands(&operations).unwrap();
        assert_eq!(commands[0].len(), 1);
        assert!(commands[1].is_empty());
        assert!(commands[0][0]
            .args
            .ends_with(&["synthetic-one:amd64".into(), "synthetic-two:amd64".into()]));
        let requests = [
            Request::Start {
                protocol: PROTOCOL,
                operations,
            },
            Request::Run {
                operation: 0,
                command: 0,
            },
        ];
        let mut calls = 0;
        serve_protocol(&mut Cursor::new(input(&requests)), &mut Vec::new(), |_| {
            calls += 1;
            Ok(completion())
        })
        .unwrap();
        assert_eq!(calls, 1);
    }

    #[test]
    fn duplicate_or_out_of_order_request_cannot_repeat_a_write() {
        let operations = vec![Operation::Refresh {
            backend: "apt".into(),
        }];
        let requests = [
            Request::Start {
                protocol: PROTOCOL,
                operations,
            },
            Request::Run {
                operation: 0,
                command: 0,
            },
            Request::Run {
                operation: 0,
                command: 0,
            },
        ];
        let mut writes = 0;
        let result = serve_protocol(&mut Cursor::new(input(&requests)), &mut Vec::new(), |_| {
            writes += 1;
            Ok(completion())
        });
        assert!(matches!(result, Err(ExecutionError::Invalid(_))));
        assert_eq!(writes, 1);
    }

    #[test]
    fn invalid_plan_is_rejected_before_ready_or_write() {
        let operations = vec![Operation::Install(id("apt", "--purge"))];
        let mut output = Vec::new();
        let mut writes = 0;
        let result = serve_protocol(
            &mut Cursor::new(input(&[Request::Start {
                protocol: PROTOCOL,
                operations,
            }])),
            &mut output,
            |_| {
                writes += 1;
                Ok(completion())
            },
        );
        assert!(matches!(result, Err(ExecutionError::Invalid(_))));
        assert!(output.is_empty());
        assert_eq!(writes, 0);
    }

    #[test]
    fn user_flatpak_never_enters_protected_plan() {
        let mut user = id("flatpak", "org.example.Test");
        user.scope = Scope::User { uid: 1000 };
        assert!(protected_commands(&Operation::Install(user))
            .unwrap()
            .is_empty());
    }
}
