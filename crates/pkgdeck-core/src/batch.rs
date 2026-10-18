//! One narrowly scoped authorization process for a confirmed native batch.
//! The elevated process derives its own command list from typed operations;
//! the frontend may only request each listed command once and in order.
//! Managers that don't share a lock run side by side, each in its own
//! [`lane`], and in order within it.
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
    io::{PipeReader, PipeWriter, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use crate::spawn::{Child, Io, Launch};

// 2: the runner forwards each line of a command's output as it runs.
// 3: commands in different lanes run at once; replies name their operation.
const PROTOCOL: u8 = 3;
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
        // A check refreshes only the user's own copy of the lists, so an
        // update refreshes the system's first, under the same approval.
        Operation::UpgradeAll { backend } if backend == "apt" => {
            return Ok(vec![
                apt_command(AptAction::Refresh)?,
                apt_command(AptAction::UpgradeAll)?,
            ]);
        }
        Operation::Upgrade(id) if id.backend == "apt" => {
            return Ok(vec![
                apt_command(AptAction::Refresh)?,
                apt_command(AptAction::Upgrade(apt_target(id)?))?,
            ]);
        }
        Operation::Install(id) if id.backend == "apt" => {
            apt_command(AptAction::Install(apt_target(id)?))?
        }
        Operation::Remove(id) if id.backend == "apt" => {
            apt_command(AptAction::Remove(apt_target(id)?))?
        }
        Operation::Clean(id) if id.backend == "apt" => match id.key.as_str() {
            "autoremove" => apt_command(AptAction::Autoremove)?,
            "clean" => apt_command(AptAction::Clean)?,
            _ => return Err(invalid("unknown APT cleanup task")),
        },
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
        Operation::UpgradeAll { backend } if backend == "fwupd" => {
            firmware_command("update", None)?
        }
        Operation::Upgrade(id) if id.backend == "fwupd" => {
            firmware_command("update", Some(&id.name))?
        }
        // Refreshes of these managers are reviewed and elevated. Snap has no
        // metadata-only refresh, so its refresh runs as the user (see backends).
        Operation::Refresh { backend }
            if matches!(backend.as_str(), "dnf" | "pacman" | "zypper") =>
        {
            manager_command(backend, "refresh", None)?
        }
        Operation::UpgradeAll { backend }
            if matches!(backend.as_str(), "dnf" | "pacman" | "zypper" | "snap") =>
        {
            manager_command(backend, "all", None)?
        }
        Operation::Install(id) | Operation::Remove(id) | Operation::Upgrade(id)
            if matches!(id.backend.as_str(), "dnf" | "pacman" | "zypper" | "snap") =>
        {
            let name = system_id(id, &id.backend)?;
            let verb = match operation {
                Operation::Install(_) => "install",
                Operation::Remove(_) => "remove",
                _ => "upgrade",
            };
            manager_command(&id.backend, verb, Some(name))?
        }
        _ => return Ok(vec![]),
    };
    Ok(vec![command])
}

/// What one runner session may do.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BatchMode {
    /// Changes someone reviewed and confirmed for this session.
    #[default]
    Reviewed,
    /// Refreshes and full upgrades of system managers only, for unattended
    /// updates under a saved approval. APT removes packages only with
    /// `removals`, which the person allowed in settings.
    UpgradeOnly { removals: bool },
}
/// The runner argument that selects [`BatchMode::UpgradeOnly`]. A saved
/// approval matches the runner's exact command line, this flag included.
pub const UPGRADE_ONLY: &str = "--upgrade-only";
/// Added after [`UPGRADE_ONLY`] when an update may remove packages.
pub const ALLOW_REMOVALS: &str = "--allow-removals";
/// System managers an upgrade-only runner updates.
pub const UPGRADE_ONLY_BACKENDS: &[&str] = &["apt", "flatpak", "dnf", "pacman", "zypper", "snap"];

/// Refuse any change the runner would make as root other than refreshes
/// and full upgrades of [`UPGRADE_ONLY_BACKENDS`]. Changes that run as the
/// user never reach the runner and pass. A per-package upgrade is refused:
/// `pacman -S --needed NAME` would install a package that is not there.
pub fn check_mode(mode: BatchMode, operations: &[Operation]) -> Result<(), ExecutionError> {
    if mode == BatchMode::Reviewed {
        return Ok(());
    }
    let allowed = operations.iter().all(|operation| match operation {
        Operation::Refresh { backend } | Operation::UpgradeAll { backend } => {
            UPGRADE_ONLY_BACKENDS.contains(&backend.as_str())
                || protected_commands(operation).is_ok_and(|commands| commands.is_empty())
        }
        _ => protected_commands(operation).is_ok_and(|commands| commands.is_empty()),
    });
    if allowed {
        Ok(())
    } else {
        Err(invalid(
            "unattended updates only refresh sources and update every package",
        ))
    }
}
/// The commands the runner itself runs. In upgrade-only mode without
/// removals APT aborts rather than remove anything (`--no-remove`); the
/// frontend still asks for the reviewed command, and the runner substitutes
/// this one.
fn runner_commands(
    mode: BatchMode,
    operations: &[Operation],
) -> Result<Vec<Vec<ProtectedCommand>>, ExecutionError> {
    check_mode(mode, operations)?;
    let mut commands = batch_commands(operations)?;
    if mode == (BatchMode::UpgradeOnly { removals: false }) {
        for (operation, commands) in operations.iter().zip(&mut commands) {
            if matches!(operation, Operation::UpgradeAll { backend } if backend == "apt") {
                // The upgrade, after the refresh.
                if let Some(command) = commands.last_mut() {
                    let verb = command.args.len() - 1;
                    command.args.insert(verb, "--no-remove".into());
                }
            }
        }
    }
    Ok(commands)
}

/// Where the run of changes from `index` that one native transaction
/// makes together ends: APT installs, removals or updates, and pacman
/// updates, one verb at a time. Updating pacman packages one by one is a
/// partial upgrade, which fails when packages move in lockstep, like
/// systemd and systemd-libs.
pub fn group_end(operations: &[Operation], index: usize) -> usize {
    let first = &operations[index];
    let mut end = index + 1;
    if joining(first).is_some() {
        while end < operations.len()
            && joining(&operations[end]).is_some()
            && operations[end].backend() == first.backend()
            && std::mem::discriminant(&operations[end]) == std::mem::discriminant(first)
        {
            end += 1;
        }
    }
    end
}

/// The package a change that can join a transaction is for.
fn joining(operation: &Operation) -> Option<&PackageId> {
    match operation {
        Operation::Install(id) | Operation::Remove(id) if id.backend == "apt" => Some(id),
        Operation::Upgrade(id) if matches!(id.backend.as_str(), "apt" | "pacman") => Some(id),
        _ => None,
    }
}

/// The commands for a run [`group_end`] found, all for its first change.
fn group_commands(group: &[Operation]) -> Result<Vec<ProtectedCommand>, ExecutionError> {
    let ids = group.iter().filter_map(joining);
    if group[0].backend() == "pacman" {
        let mut command = manager_command("pacman", "upgrade", None)?;
        for id in ids {
            command.args.push(system_id(id, "pacman")?.into());
        }
        return Ok(vec![command]);
    }
    let action = match group[0] {
        Operation::Install(_) => AptAction::Install,
        Operation::Remove(_) => AptAction::Remove,
        _ => AptAction::Upgrade,
    };
    let actions = ids
        .map(|id| apt_target(id).map(action))
        .collect::<Result<Vec<_>, _>>()?;
    let args = AptAction::group_arguments(&actions)?
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned());
    let command = ProtectedCommand::new("apt-get", args);
    // Updates refresh the lists first, as one does on its own.
    Ok(if matches!(group[0], Operation::Upgrade(_)) {
        vec![apt_command(AptAction::Refresh)?, command]
    } else {
        vec![command]
    })
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
        let end = group_end(operations, index);
        if end - index > 1 {
            commands[index] = group_commands(&operations[index..end])?;
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
/// `verb` is `refresh`, `install`, `remove`, `upgrade` or `all`.
fn manager_command(
    backend: &str,
    verb: &str,
    name: Option<&str>,
) -> Result<ProtectedCommand, ExecutionError> {
    let fixed: &[&str] = match (backend, verb) {
        ("dnf", "refresh") => &["-y", "makecache"],
        ("dnf", "install") => &["-y", "install", "--"],
        ("dnf", "remove") => &["-y", "remove", "--"],
        ("dnf", "upgrade") => &["-y", "--refresh", "upgrade", "--"],
        ("dnf", "all") => &["-y", "--refresh", "upgrade"],
        ("pacman", "refresh") => &["-Sy", "--noconfirm"],
        ("pacman", "install") => &["-S", "--noconfirm", "--needed", "--"],
        ("pacman", "upgrade") => &["-Sy", "--noconfirm", "--needed", "--"],
        ("pacman", "remove") => &["-Rns", "--noconfirm", "--"],
        ("pacman", "all") => &["-Syu", "--noconfirm"],
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
        ("snap", "upgrade" | "all") => &["refresh"],
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
    Ready {
        protocol: u8,
    },
    /// A line the running command printed, sent before it completes.
    Output {
        operation: usize,
        line: String,
    },
    Completed {
        operation: usize,
        completion: WireCompletion,
    },
    /// The request for `operation` was refused, or its command couldn't
    /// run. The runner takes no more requests after this.
    Rejected {
        operation: usize,
        reason: String,
    },
}
/// A reply for one operation, as the step waiting for it gets it.
enum Reply {
    Output(String),
    Completed(WireCompletion),
    Rejected(String),
}
impl Response {
    /// The operation a reply is for, and the reply; `None` for Ready.
    fn reply(self) -> Option<(usize, Reply)> {
        match self {
            Self::Ready { .. } => None,
            Self::Output { operation, line } => Some((operation, Reply::Output(line))),
            Self::Completed {
                operation,
                completion,
            } => Some((operation, Reply::Completed(completion))),
            Self::Rejected { operation, reason } => Some((operation, Reply::Rejected(reason))),
        }
    }
}

/// Which package managers can't run at the same time: those that share a
/// lock share a lane, and a lane runs one change at a time, in order.
/// Managers this doesn't name share one lane, as every change did before.
pub fn lane(operation: &Operation) -> &'static str {
    match operation.backend() {
        "apt" => "apt",
        "flatpak" => "flatpak",
        "snap" => "snap",
        // AUR helpers build, then install through pacman.
        "pacman" | "aur" => "pacman",
        "dnf" => "dnf",
        "zypper" => "zypper",
        "apk" => "apk",
        "xbps" => "xbps",
        // One Homebrew lock covers formulae and casks.
        "homebrew" | "homebrew-cask" => "homebrew",
        "nix" => "nix",
        _ => "",
    }
}

/// The runs [`group_end`] finds in `operations`, as `start..end` ranges,
/// split by [`lane`] in the order each lane first appears.
pub fn lanes(operations: &[Operation]) -> Vec<Vec<std::ops::Range<usize>>> {
    let mut lanes: Vec<(&str, Vec<std::ops::Range<usize>>)> = vec![];
    let mut index = 0;
    while index < operations.len() {
        let end = group_end(operations, index);
        let (key, run) = (lane(&operations[index]), index..end);
        match lanes.iter_mut().find(|(lane, _)| *lane == key) {
            Some((_, runs)) => runs.push(run),
            None => lanes.push((key, vec![run])),
        }
        index = end;
    }
    lanes.into_iter().map(|(_, runs)| runs).collect()
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
/// The runner's replies, handed to the step waiting for each operation.
#[derive(Default)]
struct Replies {
    waiting: std::collections::BTreeMap<usize, std::sync::mpsc::Sender<Reply>>,
    /// The runner stopped answering; nothing more will come.
    closed: bool,
}
type SharedReplies = std::sync::Arc<std::sync::Mutex<Replies>>;
struct Session {
    child: std::sync::Mutex<Child>,
    input: std::sync::Mutex<Option<PipeWriter>>,
    replies: SharedReplies,
    reader: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
    commands: Vec<Vec<ProtectedCommand>>,
    next: std::sync::Mutex<Vec<usize>>,
}
thread_local! {
    static SESSION: RefCell<Option<std::sync::Arc<Session>>> = const { RefCell::new(None) };
    /// The operation this thread is running, see [`set_operation`].
    static CURRENT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        SESSION.with(|cell| {
            cell.borrow_mut().take();
        });
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        // The elevated runner may not accept signals from the frontend.
        // EOF must let it leave its request loop before we wait for it.
        // A step that panicked while holding one changes nothing here.
        use std::sync::PoisonError;
        self.input
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        // Also close a runner that sent a malformed Ready frame. A batch
        // error must never leave an elevated child waiting for more input.
        let child = self.child.get_mut().unwrap_or_else(PoisonError::into_inner);
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
        // Its output closed with it.
        if let Some(reader) = self
            .reader
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = reader.join();
        }
    }
}
/// Reads the runner's replies until it closes, handing each to the step
/// waiting for its operation.
fn read_replies(mut output: PipeReader, replies: SharedReplies) {
    let mut unexpected = false;
    while let Ok(bytes) = line(&mut output) {
        let Some((operation, reply)) = serde_json::from_slice::<Response>(&bytes)
            .ok()
            .and_then(Response::reply)
        else {
            unexpected = true;
            break;
        };
        let replies = replies.lock().expect("runner replies");
        if let Some(waiting) = replies.waiting.get(&operation) {
            let _ = waiting.send(reply);
        }
    }
    let mut replies = replies.lock().expect("runner replies");
    replies.closed = true;
    // Dropping every sender wakes whoever still waits.
    for waiting in std::mem::take(&mut replies.waiting).into_values() {
        if unexpected {
            let _ = waiting.send(Reply::Rejected("unexpected authorization response".into()));
        }
    }
}
impl Session {
    /// Runs the next approved command of `index`, which must be the one
    /// asked for, and waits for it.
    fn run(
        &self,
        index: usize,
        actual: &ProtectedCommand,
        cancel: &Cancellation,
    ) -> Result<Completion, ExecutionError> {
        let next = self.next.lock().expect("runner state").get(index).copied();
        let expected = next.and_then(|next| self.commands.get(index)?.get(next));
        let (Some(next), Some(expected)) = (next, expected) else {
            return Err(invalid("protected command outside approved batch"));
        };
        if expected != actual {
            return Err(invalid("protected command differs from approved batch"));
        }
        if cancel.requested() {
            return Err(ExecutionError::Cancelled);
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        {
            let mut replies = self.replies.lock().expect("runner replies");
            if replies.closed {
                return Err(closed());
            }
            if replies.waiting.contains_key(&index) {
                return Err(invalid("protected command already running"));
            }
            replies.waiting.insert(index, sender);
        }
        let result = (|| {
            send(
                self.input
                    .lock()
                    .expect("runner input")
                    .as_mut()
                    .ok_or_else(closed)?,
                &Request::Run {
                    operation: index,
                    command: next,
                },
            )?;
            loop {
                match receiver.recv().map_err(|_| closed())? {
                    Reply::Output(line) => cancel.observe(&line),
                    Reply::Completed(completion) => {
                        self.next.lock().expect("runner state")[index] += 1;
                        let mut result: Completion = completion.into();
                        result.cancellation_deferred = cancel.requested();
                        return Ok(result);
                    }
                    Reply::Rejected(reason) => return Err(invalid(&reason)),
                }
            }
        })();
        self.replies
            .lock()
            .expect("runner replies")
            .waiting
            .remove(&index);
        result
    }
}
fn closed() -> ExecutionError {
    ExecutionError::Io("authorization runner closed unexpectedly".into())
}
/// The batch's session, for other threads to run their part of it in.
#[derive(Clone)]
pub struct SharedSession(std::sync::Arc<Session>);
/// While held, this thread runs its commands through the shared session.
pub struct Entered;
impl Drop for Entered {
    fn drop(&mut self) {
        SESSION.with(|cell| {
            cell.borrow_mut().take();
        });
    }
}
/// The session this thread runs in, if any.
pub fn share() -> Option<SharedSession> {
    SESSION.with(|cell| cell.borrow().clone().map(SharedSession))
}
impl SharedSession {
    pub fn enter(&self) -> Entered {
        SESSION.with(|cell| *cell.borrow_mut() = Some(std::sync::Arc::clone(&self.0)));
        Entered
    }
}
fn flatpak_app_path(info: &str) -> Option<PathBuf> {
    let mut in_instance = false;
    let mut app_path = None;
    for line in info.lines() {
        if line.starts_with('[') {
            in_instance = line == "[Instance]";
        } else if in_instance {
            if let Some(path) = line.strip_prefix("app-path=") {
                if app_path.replace(path).is_some() {
                    return None;
                }
            }
        }
    }
    let path = PathBuf::from(app_path?);
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
        || path.file_name()? != "files"
    {
        return None;
    }
    Some(path)
}
fn bundled_runner_path(runtime: Runtime, executable: &Path, flatpak_info: &str) -> Option<PathBuf> {
    let package = if runtime == Runtime::Flatpak {
        flatpak_app_path(flatpak_info)?
    } else {
        executable.parent()?.parent()?.to_path_buf()
    };
    Some(package.join("libexec/pkgdeck-host-runner"))
}
fn find_runner(runtime: Runtime, executable: &Path, flatpak_info: &str) -> Option<PathBuf> {
    let bundled = bundled_runner_path(runtime, executable, flatpak_info)?;
    let executable_file = |path: &Path| {
        path.is_file() && rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_ok()
    };
    if executable_file(&bundled) {
        return Some(bundled);
    }
    if runtime == Runtime::Native {
        return [
            executable.with_file_name("pkgdeck-host-runner"),
            PathBuf::from("/usr/libexec/pkgdeck-host-runner"),
        ]
        .into_iter()
        .find(|path| executable_file(path));
    }
    None
}
pub(crate) fn runner_path(host: &Host) -> Option<PathBuf> {
    let info = if host.runtime == Runtime::Flatpak {
        fs::read_to_string("/.flatpak-info").ok()?
    } else {
        String::new()
    };
    let executable = fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    find_runner(host.runtime, &executable, &info)
}
fn appimage_launcher(path: &Path) -> Option<PathBuf> {
    let path = fs::canonicalize(path).ok()?;
    (path.is_file() && rustix::fs::access(&path, rustix::fs::Access::EXEC_OK).is_ok())
        .then_some(path)
}
pub fn begin(
    host: &Host,
    authorization: Authorization,
    mode: BatchMode,
    operations: &[Operation],
    cancel: &Cancellation,
) -> Result<Option<ScopeGuard>, ExecutionError> {
    if SESSION.with(|cell| cell.borrow().is_some()) {
        return Err(invalid("nested authorization batch"));
    }
    if cancel.requested() {
        return Err(ExecutionError::Cancelled);
    }
    check_mode(mode, operations)?;
    let commands = batch_commands(operations)?;
    if !commands.iter().any(|entry| !entry.is_empty()) {
        return Ok(None);
    }
    let appimage = if host.runtime == Runtime::AppImage {
        std::env::var_os("APPIMAGE").and_then(|path| appimage_launcher(Path::new(&path)))
    } else {
        None
    };
    let runner = appimage
        .map(|path| (path, true))
        .or_else(|| runner_path(host).map(|path| (path, false)))
        .or_else(|| self_runner(host, mode));
    start_runner(
        host,
        authorization,
        mode,
        runner,
        operations,
        commands,
        cancel,
    )
}
/// A debug build run from its build folder has no packaged runner, which
/// would ask for the password once per source. It starts itself as the
/// runner (`--batch-runner`) instead. Release builds never do.
#[cfg(debug_assertions)]
fn self_runner(host: &Host, mode: BatchMode) -> Option<(PathBuf, bool)> {
    let executable = fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    self_runner_for(mode, host.runtime, executable)
}
#[cfg(not(debug_assertions))]
fn self_runner(_: &Host, _: BatchMode) -> Option<(PathBuf, bool)> {
    None
}
#[cfg(debug_assertions)]
fn self_runner_for(
    mode: BatchMode,
    runtime: Runtime,
    executable: PathBuf,
) -> Option<(PathBuf, bool)> {
    // Only the window knows `--batch-runner`, not a test binary or `pkd`.
    (mode == BatchMode::Reviewed
        && runtime == Runtime::Native
        && executable.file_name()? == "pkgdeck")
        .then_some((executable, true))
}
/// Start the session through `runner`, the one [`begin`] found, if any.
fn start_runner(
    host: &Host,
    authorization: Authorization,
    mode: BatchMode,
    runner: Option<(PathBuf, bool)>,
    operations: &[Operation],
    commands: Vec<Vec<ProtectedCommand>>,
    cancel: &Cancellation,
) -> Result<Option<ScopeGuard>, ExecutionError> {
    match runner_command(host, authorization, mode, runner)? {
        Some(command) => begin_session(command, operations, commands, cancel),
        // Without a runner each command would ask for a password on its
        // own; nobody is there to answer during an unattended update.
        None if mode != BatchMode::Reviewed => Err(ExecutionError::Disabled(
            "unattended updates need PkgDeck's system helper".into(),
        )),
        None => Ok(None),
    }
}
/// The authorization prompt that starts `runner`: a packaged host runner,
/// or (with `true`) a program that runs it with `--batch-runner`, such as
/// an AppImage launcher or a debug `pkgdeck`.
fn runner_command(
    host: &Host,
    authorization: Authorization,
    mode: BatchMode,
    runner: Option<(PathBuf, bool)>,
) -> Result<Option<Launch>, ExecutionError> {
    let Some((path, appimage)) = runner else {
        return Ok(None);
    };
    let (program, mut args) = authorization.prefix(&path);
    if appimage {
        args.push("--batch-runner".into());
    }
    if let BatchMode::UpgradeOnly { removals } = mode {
        args.push(UPGRADE_ONLY.into());
        if removals {
            args.push(ALLOW_REMOVALS.into());
        }
    }
    let mut command = host.command(Path::new(program), &args)?;
    command.stderr(Io::Null).process_group();
    Ok(Some(command))
}
/// How long the user has to answer the authorization prompt.
const AUTHORIZATION_TIMEOUT: Duration = Duration::from_secs(120);
fn begin_session(
    command: impl Into<Launch>,
    operations: &[Operation],
    commands: Vec<Vec<ProtectedCommand>>,
    cancel: &Cancellation,
) -> Result<Option<ScopeGuard>, ExecutionError> {
    begin_session_within(
        command.into(),
        operations,
        commands,
        cancel,
        AUTHORIZATION_TIMEOUT,
    )
}
fn begin_session_within(
    mut command: Launch,
    operations: &[Operation],
    commands: Vec<Vec<ProtectedCommand>>,
    cancel: &Cancellation,
    timeout: Duration,
) -> Result<Option<ScopeGuard>, ExecutionError> {
    if SESSION.with(|cell| cell.borrow().is_some()) {
        return Err(invalid("nested authorization batch"));
    }
    command.stdin(Io::Piped).stdout(Io::Piped).stderr(Io::Null);
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
        return Err(authorization_start_error(child, error));
    }
    let mut output = output;
    let session = Session {
        child: std::sync::Mutex::new(child),
        input: std::sync::Mutex::new(Some(input)),
        replies: SharedReplies::default(),
        reader: std::sync::Mutex::new(None),
        next: std::sync::Mutex::new(vec![0; commands.len()]),
        commands,
    };
    let start = Instant::now();
    let flags = rustix::fs::fcntl_getfl(&output).map_err(|e| ExecutionError::Io(e.to_string()))?;
    rustix::fs::fcntl_setfl(&output, flags | rustix::fs::OFlags::NONBLOCK)
        .map_err(|e| ExecutionError::Io(e.to_string()))?;
    let mut frame = Vec::new();
    let ready = loop {
        if cancel.requested() {
            return Err(ExecutionError::AuthorizationCancelled);
        }
        if start.elapsed() > timeout {
            return Err(ExecutionError::AuthorizationDenied);
        }
        if let Some(status) = session.child.lock().expect("runner").try_wait()? {
            return Err(match status.code() {
                Some(126) => ExecutionError::AuthorizationCancelled,
                _ => ExecutionError::AuthorizationDenied,
            });
        }
        // The ready frame is short and written before any native command.
        // stdout is nonblocking so cancellation can dismiss a pending prompt.
        let mut chunk = [0; 8192];
        let count = match output.read(&mut chunk) {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                // Wake as soon as a partial ready frame has more bytes, while
                // bounding the wait so cancellation and child exit stay responsive.
                let mut fds = [rustix::event::PollFd::new(
                    &output,
                    rustix::event::PollFlags::IN,
                )];
                let timeout = rustix::event::Timespec {
                    tv_sec: 0,
                    tv_nsec: 20_000_000,
                };
                unless_interrupted(rustix::event::poll(&mut fds, Some(&timeout)))
                    .map_err(|error| ExecutionError::Io(error.to_string()))?;
                continue;
            }
            result => result?,
        };
        if count == 0 {
            return Err(ExecutionError::AuthorizationDenied);
        }
        frame.extend_from_slice(&chunk[..count]);
        if frame.len() > MAX_MESSAGE {
            return Err(invalid("authorization message too large"));
        }
        if let Some(end) = frame.iter().position(|byte| *byte == b'\n') {
            break serde_json::from_slice::<Response>(&frame[..end])
                .map_err(|e| ExecutionError::Io(e.to_string()))?;
        }
    };
    let flags = rustix::fs::fcntl_getfl(&output).map_err(|e| ExecutionError::Io(e.to_string()))?;
    rustix::fs::fcntl_setfl(&output, flags & !rustix::fs::OFlags::NONBLOCK)
        .map_err(|e| ExecutionError::Io(e.to_string()))?;
    if !matches!(ready, Response::Ready { protocol: PROTOCOL }) {
        return Err(ExecutionError::AuthorizationDenied);
    }
    let replies = std::sync::Arc::clone(&session.replies);
    *session.reader.lock().expect("runner reader") =
        Some(std::thread::spawn(move || read_replies(output, replies)));
    SESSION.with(|cell| {
        *cell.borrow_mut() = Some(std::sync::Arc::new(session));
        Ok(Some(ScopeGuard))
    })
}

/// A wait cut short by a signal is not an error; the caller waits again.
fn unless_interrupted(result: rustix::io::Result<usize>) -> rustix::io::Result<()> {
    match result {
        Ok(_) | Err(rustix::io::Errno::INTR) => Ok(()),
        Err(error) => Err(error),
    }
}

fn authorization_start_error(mut child: Child, error: ExecutionError) -> ExecutionError {
    // A helper can close stdin before its dismissal is observed. Treat that
    // startup race as an authorization result, not an I/O failure.
    let exit = child
        .try_wait()
        .ok()
        .flatten()
        .and_then(|status| status.code());
    let _ = child.kill();
    let _ = child.wait();
    match error {
        ExecutionError::Io(_) if exit == Some(126) => ExecutionError::AuthorizationCancelled,
        ExecutionError::Io(_) => ExecutionError::AuthorizationDenied,
        other => other,
    }
}

/// The operation this thread's next protected commands belong to.
pub fn set_operation(index: usize) {
    CURRENT.with(|current| current.set(index));
}
pub fn run_in_scope(
    executable: &Path,
    args: &[OsString],
    cancel: &Cancellation,
) -> Option<Result<Completion, ExecutionError>> {
    let session = SESSION.with(|cell| cell.borrow().clone())?;
    let actual = ProtectedCommand {
        program: executable.to_string_lossy().into_owned(),
        args: args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
    };
    Some(session.run(CURRENT.with(std::cell::Cell::get), &actual, cancel))
}

/// Entry point for the elevated binary. No executable path or argument list
/// is read from the frontend after its typed plan has been validated.
pub fn serve(mode: BatchMode) -> Result<(), ExecutionError> {
    serve_as(
        rustix::process::geteuid().is_root(),
        mode,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout(),
    )
}
fn serve_as(
    root: bool,
    mode: BatchMode,
    input: &mut impl Read,
    output: &mut (impl Write + Send),
) -> Result<(), ExecutionError> {
    if !root {
        return Err(invalid("host runner requires root"));
    }
    serve_protocol(mode, input, output, run_protected)
}
/// Run one approved command with a fixed system environment, handing each
/// line it prints to `on_line`.
fn run_protected(
    expected: &ProtectedCommand,
    on_line: &mut dyn FnMut(&str),
) -> Result<Completion, ExecutionError> {
    let mut native = Command::new(&expected.program);
    native
        .args(&expected.args)
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .env("LC_ALL", "C")
        .current_dir("/");
    process::run_observed(
        native,
        Limits::default(),
        &Cancellation::default(),
        true,
        on_line,
    )
}
/// Runs one approved command, handing each line it prints to the second
/// argument. Commands in different lanes run at once.
trait RunProtected:
    Fn(&ProtectedCommand, &mut dyn FnMut(&str)) -> Result<Completion, ExecutionError> + Sync
{
}
impl<
        F: Fn(&ProtectedCommand, &mut dyn FnMut(&str)) -> Result<Completion, ExecutionError> + Sync,
    > RunProtected for F
{
}
/// What the runner has let start, so each command runs once, in order in
/// its lane, and one at a time there.
struct Approved {
    next: Vec<usize>,
    /// The last operation each lane started, while it's running.
    lanes: std::collections::BTreeMap<&'static str, (usize, bool)>,
    /// Why a command couldn't run; no more start after one.
    failed: Option<ExecutionError>,
}
impl Approved {
    fn start(
        &mut self,
        commands: &[Vec<ProtectedCommand>],
        key: &'static str,
        operation: usize,
        command: usize,
    ) -> bool {
        let lane = self.lanes.get(key).copied();
        let ok = self.failed.is_none()
            && operation < commands.len()
            && command == self.next[operation]
            && command < commands[operation].len()
            && lane.is_none_or(|(last, running)| !running && last <= operation);
        if ok {
            self.next[operation] += 1;
            self.lanes.insert(key, (operation, true));
        }
        ok
    }
}
fn serve_protocol(
    mode: BatchMode,
    mut input: &mut impl Read,
    output: &mut (impl Write + Send),
    run: impl RunProtected,
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
    let commands = runner_commands(mode, &operations)?;
    if !commands.iter().any(|entry| !entry.is_empty()) {
        return Err(invalid("empty protected plan"));
    }
    send(output, &Response::Ready { protocol: PROTOCOL })?;
    let output = std::sync::Mutex::new(output);
    let reply = |response: &Response| send(&mut **output.lock().expect("runner output"), response);
    let state = std::sync::Mutex::new(Approved {
        next: vec![0; commands.len()],
        lanes: Default::default(),
        failed: None,
    });
    let (operations, commands, run, reply, approved) =
        (&operations, &commands, &run, &reply, &state);
    std::thread::scope(|scope| {
        while let Ok(bytes) = line(&mut input) {
            let request: Request =
                serde_json::from_slice(&bytes).map_err(|e| invalid(&e.to_string()))?;
            let Request::Run { operation, command } = request else {
                return Err(invalid("runner already initialized"));
            };
            let key = operations.get(operation).map_or("", lane);
            if !approved
                .lock()
                .expect("runner state")
                .start(commands, key, operation, command)
            {
                reply(&Response::Rejected {
                    operation,
                    reason: "command outside approved batch".into(),
                })?;
                return Err(invalid("command outside approved batch"));
            }
            scope.spawn(move || {
                let expected = &commands[operation][command];
                let completion = run(expected, &mut |line| {
                    // The frontend follows progress from these; a lost line
                    // is not worth failing the command over.
                    let _ = reply(&Response::Output {
                        operation,
                        line: line.to_owned(),
                    });
                });
                let mut state = approved.lock().expect("runner state");
                state.lanes.insert(key, (operation, false));
                let sent = match completion {
                    Ok(completion) => reply(&Response::Completed {
                        operation,
                        completion: completion.into(),
                    }),
                    Err(error) => {
                        let sent = reply(&Response::Rejected {
                            operation,
                            reason: error.to_string(),
                        });
                        state.failed.get_or_insert(error);
                        sent
                    }
                };
                if let Err(error) = sent {
                    state.failed.get_or_insert(error);
                }
            });
        }
        Ok(())
    })?;
    match state.into_inner().expect("runner state").failed {
        Some(error) => Err(error),
        None => Ok(()),
    }
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
    /// Runs nothing; records every command the runner was asked to run.
    #[derive(Default)]
    struct Recorder(std::sync::Mutex<Vec<ProtectedCommand>>);
    impl Recorder {
        fn run(&self) -> impl RunProtected + '_ {
            |command, _| {
                self.0.lock().unwrap().push(command.clone());
                Ok(completion())
            }
        }
        fn calls(&self) -> Vec<ProtectedCommand> {
            self.0.lock().unwrap().clone()
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
        let seen = std::sync::Mutex::new(Vec::new());
        let mut output = Vec::new();
        serve_protocol(
            BatchMode::Reviewed,
            &mut Cursor::new(input(&requests)),
            &mut output,
            |command, on_line| {
                seen.lock().unwrap().push(command.clone());
                on_line(&format!("Running {}", command.program));
                Ok(completion())
            },
        )
        .unwrap();
        // Three lanes, so in any order.
        let mut seen: Vec<_> = seen
            .into_inner()
            .unwrap()
            .into_iter()
            .map(|command| command.program)
            .collect();
        seen.sort();
        assert_eq!(
            seen,
            ["/usr/bin/apt-get", "/usr/bin/flatpak", "/usr/bin/snap"]
        );
        let replies = output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<Response>(line).unwrap())
            .collect::<Vec<_>>();
        // Ready, then each command's output line before its completion,
        // both naming its operation.
        assert_eq!(replies.len(), 7);
        assert!(matches!(replies[0], Response::Ready { protocol: PROTOCOL }));
        for (operation, program) in [(0, "apt-get"), (1, "snap"), (2, "flatpak")] {
            let output = replies.iter().position(|reply| {
                matches!(reply, Response::Output { operation: at, line }
                    if *at == operation && *line == format!("Running /usr/bin/{program}"))
            });
            let completed = replies.iter().position(|reply| {
                matches!(reply, Response::Completed { operation: at, .. } if *at == operation)
            });
            assert!(output.unwrap() < completed.unwrap());
        }
    }

    #[test]
    fn upgrade_only_runners_refresh_and_update_everything_and_nothing_else() {
        let all = |backend: &str| Operation::UpgradeAll {
            backend: backend.into(),
        };
        let refresh = |backend: &str| Operation::Refresh {
            backend: backend.into(),
        };
        let allowed: Vec<_> = UPGRADE_ONLY_BACKENDS
            .iter()
            .flat_map(|backend| [refresh(backend), all(backend)])
            .collect();
        assert_eq!(
            check_mode(BatchMode::UpgradeOnly { removals: false }, &allowed),
            Ok(())
        );
        // Changes that run as the user never reach the runner.
        let mut user = id("homebrew", "wget");
        user.scope = Scope::User { uid: 1000 };
        let user_changes = [all("homebrew"), all("cargo"), Operation::Upgrade(user)];
        assert_eq!(
            check_mode(BatchMode::UpgradeOnly { removals: false }, &user_changes),
            Ok(())
        );
        for refused in [
            Operation::Install(id("apt", "synthetic")),
            Operation::Remove(id("dnf", "synthetic")),
            // `pacman -S --needed NAME` would install a missing package.
            Operation::Upgrade(id("pacman", "synthetic")),
            all("fwupd"),
            Operation::Upgrade(id("flatpak", "org.example.App")),
            Operation::Clean(crate::package::CleanupId {
                backend: "apt".into(),
                key: "autoremove".into(),
            }),
        ] {
            assert!(
                check_mode(
                    BatchMode::UpgradeOnly { removals: false },
                    &[all("apt"), refused.clone()]
                )
                .is_err(),
                "{refused:?}"
            );
            assert_eq!(check_mode(BatchMode::Reviewed, &[refused]), Ok(()));
        }

        // APT never removes anything in this mode; the frontend still asks
        // for the reviewed command by index.
        let operations = vec![refresh("apt"), all("apt"), all("dnf")];
        let reviewed = batch_commands(&operations).unwrap();
        let runner =
            runner_commands(BatchMode::UpgradeOnly { removals: false }, &operations).unwrap();
        assert_eq!(runner[0], reviewed[0]);
        assert_eq!(runner[2], reviewed[2]);
        // The update refreshes the lists first, as reviewed.
        assert_eq!(runner[1][0], reviewed[1][0]);
        assert_eq!(runner[1][0].args.last().unwrap(), "update");
        assert_eq!(
            runner[1][1].args,
            [
                "--assume-yes",
                "-o",
                "DPkg::Lock::Timeout=0",
                "--no-remove",
                "dist-upgrade"
            ]
        );
        let requests = [
            Request::Start {
                protocol: PROTOCOL,
                operations: operations.clone(),
            },
            Request::Run {
                operation: 1,
                command: 0,
            },
        ];
        let recorder = Recorder::default();
        serve_protocol(
            BatchMode::UpgradeOnly { removals: false },
            &mut Cursor::new(input(&requests)),
            &mut Vec::new(),
            recorder.run(),
        )
        .unwrap();
        assert_eq!(recorder.calls(), [runner[1][0].clone()]);

        // With removals allowed, APT runs the reviewed command as is.
        let removing =
            runner_commands(BatchMode::UpgradeOnly { removals: true }, &operations).unwrap();
        assert_eq!(removing, reviewed);

        // An install never starts, even before the first command.
        let mut output = Vec::new();
        let recorder = Recorder::default();
        assert!(serve_protocol(
            BatchMode::UpgradeOnly { removals: false },
            &mut Cursor::new(input(&[Request::Start {
                protocol: PROTOCOL,
                operations: vec![Operation::Install(id("apt", "synthetic"))],
            }])),
            &mut output,
            recorder.run(),
        )
        .is_err());
        assert!(output.is_empty() && recorder.calls().is_empty());
    }

    #[test]
    fn upgrade_only_sessions_start_the_runner_with_its_flag_or_not_at_all() {
        let host = Host::new(Runtime::Native, Default::default());
        let runner = PathBuf::from("/usr/libexec/pkgdeck-host-runner");
        let command = runner_command(
            &host,
            Authorization::Polkit,
            BatchMode::UpgradeOnly { removals: false },
            Some((runner, false)),
        )
        .unwrap()
        .unwrap();
        let args: Vec<_> = command
            .args()
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--disable-internal-agent",
                "/usr/libexec/pkgdeck-host-runner",
                "--upgrade-only"
            ]
        );
        let command = runner_command(
            &host,
            Authorization::Polkit,
            BatchMode::UpgradeOnly { removals: true },
            Some((PathBuf::from("/usr/libexec/pkgdeck-host-runner"), false)),
        )
        .unwrap()
        .unwrap();
        assert!(command
            .args()
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .eq([
                "--disable-internal-agent",
                "/usr/libexec/pkgdeck-host-runner",
                "--upgrade-only",
                "--allow-removals"
            ]));
        let cancel = Cancellation::default();
        let refused = begin(
            &host,
            Authorization::Polkit,
            BatchMode::UpgradeOnly { removals: false },
            &[Operation::Install(id("apt", "synthetic"))],
            &cancel,
        );
        assert!(matches!(refused, Err(ExecutionError::Invalid(_))));
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
        let calls = std::sync::atomic::AtomicUsize::new(0);
        serve_protocol(
            BatchMode::Reviewed,
            &mut Cursor::new(input(&requests)),
            &mut Vec::new(),
            |_, _| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(completion())
            },
        )
        .unwrap();
        assert_eq!(calls.into_inner(), 1);
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
        let recorder = Recorder::default();
        let result = serve_protocol(
            BatchMode::Reviewed,
            &mut Cursor::new(input(&requests)),
            &mut Vec::new(),
            recorder.run(),
        );
        assert!(matches!(result, Err(ExecutionError::Invalid(_))));
        assert_eq!(recorder.calls().len(), 1);
    }

    #[test]
    fn invalid_plan_is_rejected_before_ready_or_write() {
        let operations = vec![Operation::Install(id("apt", "--purge"))];
        let mut output = Vec::new();
        let recorder = Recorder::default();
        let result = serve_protocol(
            BatchMode::Reviewed,
            &mut Cursor::new(input(&[Request::Start {
                protocol: PROTOCOL,
                operations,
            }])),
            &mut output,
            recorder.run(),
        );
        assert!(matches!(result, Err(ExecutionError::Invalid(_))));
        assert!(output.is_empty());
        assert!(recorder.calls().is_empty());
    }

    #[test]
    fn user_flatpak_never_enters_protected_plan() {
        let mut user = id("flatpak", "org.example.Test");
        user.scope = Scope::User { uid: 1000 };
        assert!(protected_commands(&Operation::Install(user))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn supported_system_operations_build_only_fixed_manager_verbs() {
        use crate::package::CleanupId;
        let targets = [
            (
                Operation::Refresh {
                    backend: "apt".into(),
                },
                "apt-get",
                "update",
            ),
            (
                Operation::UpgradeAll {
                    backend: "apt".into(),
                },
                "apt-get",
                "upgrade",
            ),
            (
                Operation::Install(id("apt", "synthetic")),
                "apt-get",
                "install",
            ),
            (
                Operation::Remove(id("apt", "synthetic")),
                "apt-get",
                "remove",
            ),
            (
                Operation::Upgrade(id("apt", "synthetic")),
                "apt-get",
                "install",
            ),
            (
                Operation::Clean(CleanupId {
                    backend: "apt".into(),
                    key: "autoremove".into(),
                }),
                "apt-get",
                "autoremove",
            ),
            (
                Operation::Clean(CleanupId {
                    backend: "apt".into(),
                    key: "clean".into(),
                }),
                "apt-get",
                "clean",
            ),
            (
                Operation::UpgradeAll {
                    backend: "flatpak".into(),
                },
                "flatpak",
                "update",
            ),
            (
                Operation::Clean(CleanupId {
                    backend: "flatpak".into(),
                    key: "unused-system".into(),
                }),
                "flatpak",
                "uninstall",
            ),
            (
                Operation::Install(id("flatpak", "org.example.Test")),
                "flatpak",
                "install",
            ),
            (
                Operation::Remove(id("flatpak", "org.example.Test")),
                "flatpak",
                "uninstall",
            ),
            (
                Operation::Upgrade(id("flatpak", "org.example.Test")),
                "flatpak",
                "update",
            ),
            (
                Operation::UpgradeAll {
                    backend: "fwupd".into(),
                },
                "fwupdmgr",
                "update",
            ),
            (
                Operation::Upgrade(id("fwupd", &"a".repeat(40))),
                "fwupdmgr",
                "update",
            ),
        ];
        for (operation, binary, verb) in targets {
            let mut commands = protected_commands(&operation).unwrap();
            // APT updates refresh the system's lists first: a check only
            // refreshed the user's own copy.
            if binary == "apt-get"
                && matches!(
                    operation,
                    Operation::Upgrade(_) | Operation::UpgradeAll { .. }
                )
            {
                assert_eq!(commands.len(), 2, "{operation:?}");
                assert_eq!(commands[0], apt_command(AptAction::Refresh).unwrap());
                commands.remove(0);
            }
            assert_eq!(commands.len(), 1, "{operation:?}");
            assert_eq!(commands[0].program, format!("/usr/bin/{binary}"));
            assert!(
                commands[0]
                    .args
                    .iter()
                    .any(|arg| arg == verb || (verb == "upgrade" && arg == "dist-upgrade")),
                "{operation:?}"
            );
        }
        // Routine checks never elevate a refresh; only the explicit,
        // reviewed refresh of these managers does.
        for backend in ["dnf", "pacman", "zypper"] {
            for operation in [
                Operation::Refresh {
                    backend: backend.into(),
                },
                Operation::UpgradeAll {
                    backend: backend.into(),
                },
                Operation::Install(id(backend, "synthetic")),
                Operation::Remove(id(backend, "synthetic")),
                Operation::Upgrade(id(backend, "synthetic")),
            ] {
                let commands = protected_commands(&operation).unwrap();
                assert_eq!(commands.len(), 1, "{operation:?}");
                assert_eq!(commands[0].program, format!("/usr/bin/{backend}"));
                assert!(commands[0].args.iter().all(|part| !part.contains('\n')));
            }
        }
    }

    #[test]
    fn malformed_identities_and_unknown_tasks_fail_before_runner_ready() {
        use crate::package::CleanupId;
        let mut wrong_scope = id("apt", "synthetic");
        wrong_scope.scope = Scope::User { uid: 1000 };
        let mut bad_flatpak = id("flatpak", "org.example.Test");
        bad_flatpak.reference = Some("app/other/x86_64/stable".into());
        let mut bad_remote = id("flatpak", "org.example.Test");
        bad_remote.remote = Some("--command".into());
        for operation in [
            Operation::Install(wrong_scope),
            Operation::Install(id("apt", "--purge")),
            Operation::Install(id("dnf", "--command")),
            Operation::Install(bad_flatpak),
            Operation::Install(bad_remote),
            Operation::Upgrade(id("fwupd", "bad-device")),
            Operation::Clean(CleanupId {
                backend: "apt".into(),
                key: "unknown".into(),
            }),
        ] {
            assert!(protected_commands(&operation).is_err(), "{operation:?}");
        }
        let mut runtime = id("flatpak", "org.example.Runtime");
        runtime.reference = Some("runtime/org.example.Runtime/amd64/stable".into());
        let command = protected_commands(&Operation::Install(runtime))
            .unwrap()
            .pop()
            .unwrap();
        assert!(command.args.contains(&"--runtime".into()));
        assert!(!command.args.contains(&"--app".into()));
    }

    #[test]
    fn authorization_protocol_rejects_empty_restarts_and_out_of_plan_indices() {
        let recorder = Recorder::default();
        let run = recorder.run();
        for request in [
            Request::Start {
                protocol: PROTOCOL,
                operations: vec![],
            },
            Request::Start {
                protocol: PROTOCOL + 1,
                operations: vec![Operation::Refresh {
                    backend: "apt".into(),
                }],
            },
            Request::Start {
                protocol: PROTOCOL,
                operations: vec![Operation::Refresh {
                    backend: "unknown".into(),
                }],
            },
        ] {
            let mut output = Vec::new();
            assert!(serve_protocol(
                BatchMode::Reviewed,
                &mut Cursor::new(input(&[request])),
                &mut output,
                &run
            )
            .is_err());
            assert!(output.is_empty());
        }
        for invalid_run in [
            Request::Run {
                operation: 1,
                command: 0,
            },
            Request::Run {
                operation: 0,
                command: 1,
            },
            Request::Start {
                protocol: PROTOCOL,
                operations: vec![Operation::Refresh {
                    backend: "apt".into(),
                }],
            },
        ] {
            let requests = [
                Request::Start {
                    protocol: PROTOCOL,
                    operations: vec![Operation::Refresh {
                        backend: "apt".into(),
                    }],
                },
                invalid_run,
            ];
            let mut output = Vec::new();
            assert!(serve_protocol(
                BatchMode::Reviewed,
                &mut Cursor::new(input(&requests)),
                &mut output,
                &run
            )
            .is_err());
        }
        drop(run);
        assert!(recorder.calls().is_empty());
    }

    #[test]
    fn scoped_session_accepts_only_the_next_approved_command() {
        // The child implements just the framed protocol. No native package
        // tool is started; this exercises the frontend side of the boundary.
        let script = r#"IFS= read -r start
printf '%s\n' '{"type":"ready","protocol":3}'
while IFS= read -r request; do
  printf '%s\n' '{"type":"output","operation":0,"line":"Unpacking synthetic:amd64 (2) over (1) ..."}'
  printf '%s\n' '{"type":"completed","operation":0,"completion":{"code":0,"signal":null,"stdout":[100,111,110,101],"stderr":[],"truncated":false,"cancellation_deferred":false}}'
done"#;
        let mut child = Command::new("/bin/sh");
        child.arg("-c").arg(script);
        let operations = [Operation::Install(id("apt", "synthetic"))];
        let approved = batch_commands(&operations).unwrap();
        let cancel = Cancellation::default();
        let scope = begin_session(child, &operations, approved.clone(), &cancel)
            .unwrap()
            .unwrap();
        assert!(matches!(
            begin_session(
                Command::new("/bin/true"),
                &operations,
                approved.clone(),
                &cancel
            ),
            Err(ExecutionError::Invalid(_))
        ));
        set_operation(0);
        let command = &approved[0][0];
        let args = command.args.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(matches!(
            run_in_scope(Path::new("/usr/bin/apt-get"), &["--purge".into()], &cancel),
            Some(Err(ExecutionError::Invalid(_)))
        ));
        // Output lines reach the job's observer while the command runs.
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = lines.clone();
        let observed = cancel.with_output(std::sync::Arc::new(move |line: &str| {
            seen.lock().unwrap().push(line.to_owned());
        }));
        let result = run_in_scope(Path::new(&command.program), &args, &observed)
            .unwrap()
            .unwrap();
        assert_eq!(result.stdout, b"done");
        assert_eq!(
            *lines.lock().unwrap(),
            ["Unpacking synthetic:amd64 (2) over (1) ..."]
        );
        assert!(matches!(
            run_in_scope(Path::new(&command.program), &args, &cancel),
            Some(Err(ExecutionError::Invalid(_)))
        ));
        set_operation(10);
        assert!(matches!(
            run_in_scope(Path::new(&command.program), &args, &cancel),
            Some(Err(ExecutionError::Invalid(_)))
        ));
        drop(scope);
        assert!(run_in_scope(Path::new(&command.program), &args, &cancel).is_none());
    }

    #[test]
    fn threads_run_their_steps_through_one_session_at_once() {
        // The runner answers only once both steps asked, the later one first.
        let script = r#"IFS= read -r start
printf '%s\n' '{"type":"ready","protocol":3}'
IFS= read -r first
IFS= read -r second
for request in "$second" "$first"; do
  case $request in *'"operation":0'*) step=0 ;; *) step=1 ;; esac
  printf '{"type":"output","operation":%s,"line":"step %s"}\n' $step $step
  printf '{"type":"completed","operation":%s,"completion":{"code":0,"signal":null,"stdout":[],"stderr":[],"truncated":false,"cancellation_deferred":false}}\n' $step
done"#;
        let mut child = Command::new("/bin/sh");
        child.arg("-c").arg(script);
        let operations = [
            Operation::Install(id("apt", "synthetic")),
            Operation::Upgrade(PackageId {
                remote: Some("flathub".into()),
                ..id("flatpak", "org.example.Test")
            }),
        ];
        let approved = batch_commands(&operations).unwrap();
        let cancel = Cancellation::default();
        let scope = begin_session(child, &operations, approved.clone(), &cancel)
            .unwrap()
            .unwrap();
        let shared = share().unwrap();
        let seen = std::thread::scope(|threads| {
            let steps = (0..2)
                .map(|step| {
                    let (shared, command, cancel) = (&shared, &approved[step][0], &cancel);
                    threads.spawn(move || {
                        let entered = shared.enter();
                        set_operation(step);
                        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
                        let seen = lines.clone();
                        let observed =
                            cancel.with_output(std::sync::Arc::new(move |line: &str| {
                                seen.lock().unwrap().push(line.to_owned());
                            }));
                        let args = command.args.iter().map(OsString::from).collect::<Vec<_>>();
                        let code = run_in_scope(Path::new(&command.program), &args, &observed)
                            .unwrap()
                            .unwrap()
                            .code;
                        let lines = lines.lock().unwrap().clone();
                        // Leaving it, this thread runs nothing through it.
                        drop(entered);
                        assert!(share().is_none());
                        (code, lines)
                    })
                })
                .collect::<Vec<_>>();
            steps
                .into_iter()
                .map(|step| step.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            seen,
            [
                (Some(0), vec!["step 0".to_owned()]),
                (Some(0), vec!["step 1".to_owned()])
            ]
        );
        drop(scope);
        assert!(share().is_none());
    }

    #[test]
    fn a_step_runs_once_at_a_time_and_not_after_the_runner_closed() {
        // The runner answers once it has both steps' requests, then leaves.
        let script = r#"IFS= read -r start
printf '%s\n' '{"type":"ready","protocol":3}'
IFS= read -r first
IFS= read -r second
for step in 0 1; do
  printf '{"type":"completed","operation":%s,"completion":{"code":0,"signal":null,"stdout":[],"stderr":[],"truncated":false,"cancellation_deferred":false}}\n' $step
done"#;
        let mut child = Command::new("/bin/sh");
        child.arg("-c").arg(script);
        let operations = [
            Operation::Install(id("apt", "synthetic")),
            Operation::Upgrade(PackageId {
                remote: Some("flathub".into()),
                ..id("flatpak", "org.example.Test")
            }),
        ];
        let approved = batch_commands(&operations).unwrap();
        let cancel = Cancellation::default();
        let scope = begin_session(child, &operations, approved.clone(), &cancel)
            .unwrap()
            .unwrap();
        let session = share().unwrap().0;
        let args = |step: usize| {
            approved[step][0]
                .args
                .iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        };
        let program = |step: usize| PathBuf::from(&approved[step][0].program);
        let wait_for = |done: &dyn Fn(&Replies) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !done(&session.replies.lock().unwrap()) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        std::thread::scope(|threads| {
            let first = threads.spawn(|| {
                let _entered = SharedSession(session.clone()).enter();
                set_operation(0);
                run_in_scope(&program(0), &args(0), &cancel).unwrap()
            });
            wait_for(&|replies| replies.waiting.contains_key(&0));
            // The same step again, while it runs.
            set_operation(0);
            assert!(matches!(
                run_in_scope(&program(0), &args(0), &cancel),
                Some(Err(ExecutionError::Invalid(reason))) if reason.contains("already running")
            ));
            set_operation(1);
            assert!(run_in_scope(&program(1), &args(1), &cancel)
                .unwrap()
                .is_ok());
            assert!(first.join().unwrap().is_ok());
        });
        wait_for(&|replies| replies.closed);
        set_operation(1);
        assert!(matches!(
            run_in_scope(&program(1), &args(1), &cancel),
            Some(Err(ExecutionError::Invalid(_)))
        ));
        // A step it still had to run finds it gone.
        let flatpak = &approved[1][0];
        *session.next.lock().unwrap() = vec![0, 0];
        set_operation(1);
        assert!(matches!(
            run_in_scope(Path::new(&flatpak.program), &args(1), &cancel),
            Some(Err(ExecutionError::Io(reason))) if reason.contains("closed unexpectedly")
        ));
        drop(scope);
    }

    #[test]
    fn ending_a_session_stops_a_runner_that_would_keep_going() {
        let pid = std::env::temp_dir().join(format!("pkgdeck-lingering-{}", std::process::id()));
        // It ignores the end of its input, and TERM.
        let script = format!(
            r#"trap '' TERM
printf '%s' $$ >'{}'
IFS= read -r start
printf '%s\n' '{{"type":"ready","protocol":3}}'
while :; do sleep 1 </dev/null >/dev/null 2>&1; done"#,
            pid.display()
        );
        let mut child = Command::new("/bin/sh");
        child.arg("-c").arg(script);
        let operations = [Operation::Install(id("apt", "synthetic"))];
        let approved = batch_commands(&operations).unwrap();
        let scope = begin_session(child, &operations, approved, &Cancellation::default())
            .unwrap()
            .unwrap();
        let runner = fs::read_to_string(&pid).unwrap();
        let _ = fs::remove_file(&pid);
        let runner = rustix::process::Pid::from_raw(runner.parse().unwrap()).unwrap();
        let alive = || rustix::process::test_kill_process(runner).is_ok();
        assert!(alive());
        drop(scope);
        // One still running is stopped before the test fails.
        let kill = || rustix::process::kill_process(runner, rustix::process::Signal::KILL);
        let lingering = alive();
        let _ = lingering.then(kill);
        assert!(!lingering);
    }

    #[test]
    fn scoped_session_can_cancel_a_pending_authorization() {
        let mut child = Command::new("/bin/sh");
        child.arg("-c").arg(
            "IFS= read -r start; sleep 2; printf '%s\\n' '{\"type\":\"ready\",\"protocol\":3}'",
        );
        let operations = [Operation::Refresh {
            backend: "apt".into(),
        }];
        let commands = batch_commands(&operations).unwrap();
        let cancel = Cancellation::default();
        let trigger = cancel.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            trigger.cancel();
        });
        let result = begin_session(child, &operations, commands, &cancel);
        thread.join().unwrap();
        assert!(matches!(
            result,
            Err(ExecutionError::AuthorizationCancelled)
        ));
    }

    #[test]
    fn absent_host_runner_keeps_unprivileged_or_invalid_batches_outside_scope() {
        use std::collections::BTreeMap;
        let host = Host::new(Runtime::Native, BTreeMap::new());
        let cancel = Cancellation::default();
        let user = Operation::Install(PackageId {
            scope: Scope::User { uid: 1000 },
            ..id("flatpak", "org.example.Test")
        });
        assert!(begin(
            &host,
            Authorization::Polkit,
            BatchMode::Reviewed,
            &[user],
            &cancel
        )
        .unwrap()
        .is_none());
        assert!(begin(
            &host,
            Authorization::Polkit,
            BatchMode::Reviewed,
            &[Operation::Refresh {
                backend: "apt".into()
            }],
            &cancel
        )
        .unwrap()
        .is_none());
        let flatpak = Host::new(Runtime::Flatpak, BTreeMap::new());
        assert!(runner_path(&flatpak).is_none());
        // A debug window without a packaged runner runs as its own, so one
        // password covers a reviewed batch; nothing else ever does.
        #[cfg(debug_assertions)]
        {
            let window = PathBuf::from("/work/target/debug/pkgdeck");
            let upgrade_only = BatchMode::UpgradeOnly { removals: false };
            assert_eq!(
                self_runner_for(BatchMode::Reviewed, Runtime::Native, window.clone()),
                Some((window.clone(), true))
            );
            assert_eq!(
                self_runner_for(upgrade_only, Runtime::Native, window.clone()),
                None
            );
            assert_eq!(
                self_runner_for(BatchMode::Reviewed, Runtime::Flatpak, window),
                None
            );
            for other in [
                "/work/target/debug/pkd",
                "/work/target/debug/deps/batch-1a2b",
            ] {
                assert_eq!(
                    self_runner_for(BatchMode::Reviewed, Runtime::Native, other.into()),
                    None
                );
            }
        }
        assert!(begin(
            &host,
            Authorization::Polkit,
            BatchMode::Reviewed,
            &[Operation::Install(id("apt", "--purge"))],
            &cancel
        )
        .is_err());
        cancel.cancel();
        assert!(matches!(
            begin(
                &host,
                Authorization::Polkit,
                BatchMode::Reviewed,
                &[Operation::Refresh {
                    backend: "apt".into()
                }],
                &cancel
            ),
            Err(ExecutionError::Cancelled)
        ));
    }

    #[test]
    fn framing_rejects_truncation_and_oversized_messages() {
        assert!(matches!(
            line(&mut Cursor::new(Vec::<u8>::new())),
            Err(ExecutionError::Io(_))
        ));
        assert!(matches!(
            line(&mut Cursor::new(vec![b'x'; MAX_MESSAGE + 1])),
            Err(ExecutionError::Invalid(_))
        ));
        assert!(matches!(
            send(&mut Vec::new(), &"x".repeat(MAX_MESSAGE + 1)),
            Err(ExecutionError::Invalid(_))
        ));
        let mut output = Vec::new();
        let recorder = Recorder::default();
        assert!(matches!(
            serve_protocol(
                BatchMode::Reviewed,
                &mut Cursor::new(b"not json\n"),
                &mut output,
                recorder.run()
            ),
            Err(ExecutionError::Invalid(_))
        ));
        assert!(recorder.calls().is_empty());
    }

    #[test]
    fn authorization_session_rejects_wrong_ready_and_dismissed_prompts() {
        let operations = [Operation::Refresh {
            backend: "apt".into(),
        }];
        let commands = batch_commands(&operations).unwrap();
        for script in [
            // A runner speaking another protocol version is refused.
            "IFS= read -r start; printf '%s\\n' '{\"type\":\"ready\",\"protocol\":2}'; sleep 1",
            "IFS= read -r start; printf '%s\\n' '{\"type\":\"rejected\",\"operation\":0,\"reason\":\"no\"}'; sleep 1",
            "exit 126",
        ] {
            let mut child = Command::new("/bin/sh");
            child.arg("-c").arg(script);
            assert!(matches!(
                begin_session(child, &operations, commands.clone(), &Cancellation::default()),
                Err(ExecutionError::AuthorizationDenied | ExecutionError::AuthorizationCancelled)
            ));
        }
    }

    #[test]
    fn helper_exit_before_ready_is_cancelled_only_for_a_dismissed_prompt() {
        let operations = [Operation::Refresh {
            backend: "apt".into(),
        }];
        let commands = batch_commands(&operations).unwrap();
        // A lingering child keeps stdout open, so the helper's exit status
        // is observed instead of end of output.
        for (code, cancelled) in [(126, true), (1, false)] {
            let mut child = Command::new("/bin/sh");
            child.arg("-c").arg(format!(
                "IFS= read -r start; sleep 2 </dev/null & exit {code}"
            ));
            let result = begin_session(
                child,
                &operations,
                commands.clone(),
                &Cancellation::default(),
            );
            assert!(matches!(
                (result, cancelled),
                (Err(ExecutionError::AuthorizationCancelled), true)
                    | (Err(ExecutionError::AuthorizationDenied), false)
            ));
        }
    }

    #[test]
    fn authorization_start_failure_uses_helper_exit_status() {
        for (script, cancelled) in [("exit 126", true), ("exit 1", false)] {
            let mut child = Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .spawn()
                .unwrap();
            child.wait().unwrap();
            let result =
                authorization_start_error(child.into(), ExecutionError::Io("closed pipe".into()));
            assert!(matches!(result, ExecutionError::AuthorizationCancelled) == cancelled);
        }
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 126")
            .spawn()
            .unwrap();
        child.wait().unwrap();
        assert!(matches!(
            authorization_start_error(child.into(), ExecutionError::Invalid("synthetic".into())),
            ExecutionError::Invalid(reason) if reason == "synthetic"
        ));
    }

    #[test]
    fn scoped_session_preserves_native_rejection_and_cancel_before_dispatch() {
        let script = r#"IFS= read -r start
printf '%s\n' '{"type":"ready","protocol":3}'
while IFS= read -r request; do
  printf '%s\n' '{"type":"rejected","operation":0,"reason":"synthetic rejection"}'
done"#;
        let operations = [Operation::Refresh {
            backend: "apt".into(),
        }];
        let commands = batch_commands(&operations).unwrap();
        let mut child = Command::new("/bin/sh");
        child.arg("-c").arg(script);
        let cancel = Cancellation::default();
        let scope = begin_session(child, &operations, commands.clone(), &cancel)
            .unwrap()
            .unwrap();
        set_operation(0);
        let command = &commands[0][0];
        let args = command.args.iter().map(OsString::from).collect::<Vec<_>>();
        let result = run_in_scope(Path::new(&command.program), &args, &cancel).unwrap();
        assert!(
            matches!(result, Err(ExecutionError::Invalid(reason)) if reason == "synthetic rejection")
        );
        cancel.cancel();
        assert!(matches!(
            run_in_scope(Path::new(&command.program), &args, &cancel),
            Some(Err(ExecutionError::Cancelled))
        ));
        drop(scope);
    }

    #[test]
    fn flatpak_runner_uses_its_own_deployment_for_system_and_user_installs() {
        let commit = "a".repeat(64);
        for base in ["/var/lib/flatpak", "/home/user/.local/share/flatpak"] {
            let path = format!("{base}/app/io.github.astrovm.PkgDeck/x86_64/master/{commit}/files");
            let info = format!("top=level\n[Instance]\nbranch=master\napp-path={path}\n");
            assert_eq!(
                bundled_runner_path(Runtime::Flatpak, Path::new("/app/bin/pkd"), &info),
                Some(Path::new(&path).join("libexec/pkgdeck-host-runner"))
            );
            assert!(flatpak_app_path(&format!("{info}app-path={path}\n")).is_none());
        }
        for invalid in [
            "relative/files".to_owned(),
            "/tmp/../files".to_owned(),
            "/tmp/other".to_owned(),
        ] {
            assert!(flatpak_app_path(&format!("[Instance]\napp-path={invalid}\n")).is_none());
        }
    }

    #[test]
    fn user_owned_packaged_runners_are_available_to_every_linux_format() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("pkgdeck-bundled-runner-{}", std::process::id()));
        let runner = root.join("libexec/pkgdeck-host-runner");
        fs::create_dir_all(runner.parent().unwrap()).unwrap();
        fs::write(&runner, b"synthetic runner").unwrap();
        fs::set_permissions(&runner, fs::Permissions::from_mode(0o755)).unwrap();
        for runtime in [Runtime::Native, Runtime::AppImage, Runtime::Snap] {
            assert_eq!(
                find_runner(runtime, &root.join("bin/pkd"), ""),
                Some(runner.clone())
            );
        }
        let files = root
            .join("app/io.github.astrovm.PkgDeck/x86_64/master")
            .join("a".repeat(64))
            .join("files");
        let flatpak_runner = files.join("libexec/pkgdeck-host-runner");
        fs::create_dir_all(flatpak_runner.parent().unwrap()).unwrap();
        fs::write(&flatpak_runner, b"synthetic runner").unwrap();
        fs::set_permissions(&flatpak_runner, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            find_runner(
                Runtime::Flatpak,
                Path::new("/app/bin/pkd"),
                &format!("[Instance]\napp-path={}\n", files.display())
            ),
            Some(flatpak_runner)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn appimage_launches_its_bundled_runner_from_a_fresh_mount() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("pkgdeck-appimage-runner-{}", std::process::id()));
        let launcher = root.join("AppRun");
        let runner = root.join("usr/libexec/pkgdeck-host-runner");
        fs::create_dir_all(runner.parent().unwrap()).unwrap();
        fs::write(
            &launcher,
            include_str!("../../../packaging/appimage/AppRun"),
        )
        .unwrap();
        fs::write(&runner, b"#!/bin/sh\nprintf 'runner:%s\n' \"$APPDIR\"\n").unwrap();
        for path in [&launcher, &runner] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            appimage_launcher(&launcher),
            Some(fs::canonicalize(&launcher).unwrap())
        );
        // Another test's fork can still hold a just-written script open,
        // which makes exec fail with "Text file busy". It clears at once.
        let run = || Command::new(&launcher).arg("--batch-runner").output();
        let busy = |result: &std::io::Result<std::process::Output>| {
            result.as_ref().err().map(std::io::Error::kind)
                == Some(std::io::ErrorKind::ExecutableFileBusy)
        };
        let output = std::iter::repeat_with(|| {
            let result = run();
            std::thread::sleep(Duration::from_millis(if busy(&result) { 20 } else { 0 }));
            result
        })
        .take(50)
        .find(|result| !busy(result))
        .expect("the script stayed busy")
        .unwrap();
        assert!(output.status.success());
        assert_eq!(
            output.stdout,
            format!("runner:{}\n", root.display()).as_bytes()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scoped_grouped_apt_write_runs_the_confirmed_targets_once() {
        use std::collections::BTreeMap;
        let script = r#"IFS= read -r start
printf '%s\n' '{"type":"ready","protocol":3}'
while IFS= read -r request; do
  printf '%s\n' '{"type":"completed","operation":0,"completion":{"code":0,"signal":null,"stdout":[100,111,110,101],"stderr":[],"truncated":false,"cancellation_deferred":false}}'
done"#;
        let operations = [
            Operation::Install(id("apt", "one")),
            Operation::Install(id("apt", "two")),
        ];
        let commands = batch_commands(&operations).unwrap();
        assert!(commands[1].is_empty());
        let mut child = Command::new("/bin/sh");
        child.arg("-c").arg(script);
        let cancel = Cancellation::default();
        let scope = begin_session(child, &operations, commands, &cancel)
            .unwrap()
            .unwrap();
        set_operation(0);
        let host = Host::new(Runtime::Native, BTreeMap::new());
        let result = host
            .apt_group(
                &[
                    AptAction::Install("one:amd64".into()),
                    AptAction::Install("two:amd64".into()),
                ],
                Authorization::Polkit,
                &cancel,
            )
            .unwrap();
        assert_eq!(result.code, Some(0));
        assert_eq!(result.stdout, b"done");
        set_operation(1);
        drop(scope);
    }

    /// The APT backend itself, run through a session whose runner approves
    /// what `batch_commands` planned and logs each command it's asked for.
    #[cfg(target_os = "linux")]
    #[test]
    fn apt_updates_send_the_refresh_then_the_update_in_one_session() {
        // The host must have APT for its commands to be built.
        if Path::new("/usr/bin/apt-get").exists() {
            apt_session();
        }
    }
    #[cfg(target_os = "linux")]
    fn apt_session() {
        use crate::engine::Backend;
        use std::collections::BTreeMap;
        let log = std::env::temp_dir().join(format!("pkgdeck-apt-session-{}", std::process::id()));
        let script = format!(
            r#"IFS= read -r start
printf '%s\n' '{{"type":"ready","protocol":3}}'
while IFS= read -r request; do
  printf '%s\n' "$request" >>'{}'
  printf '%s\n' '{{"type":"completed","operation":0,"completion":{{"code":0,"signal":null,"stdout":[],"stderr":[],"truncated":false,"cancellation_deferred":false}}}}'
done"#,
            log.display()
        );
        let session = |operations: &[Operation]| {
            let _ = fs::remove_file(&log);
            let mut child = Command::new("/bin/sh");
            child.arg("-c").arg(&script);
            let commands = batch_commands(operations).unwrap();
            let scope = begin_session(child, operations, commands, &Cancellation::default())
                .unwrap()
                .unwrap();
            set_operation(0);
            scope
        };
        let requests = || fs::read_to_string(&log).unwrap_or_default();
        let mut apt = crate::backends::Apt::new(crate::backends::NativeTransport {
            host: Host::new(Runtime::Native, BTreeMap::new()),
            authorization: Authorization::Polkit,
        });
        let cancel = Cancellation::default();
        let all = Operation::UpgradeAll {
            backend: "apt".into(),
        };
        let scope = session(std::slice::from_ref(&all));
        apt.prepare(&all, &cancel, &mut |_| {}).unwrap();
        apt.execute(&all, &cancel, &mut |_| {}).unwrap();
        drop(scope);
        assert_eq!(
            requests(),
            "{\"type\":\"run\",\"operation\":0,\"command\":0}\n{\"type\":\"run\",\"operation\":0,\"command\":1}\n"
        );
        // Grouped updates: one refresh, then the group.
        let updates = [
            Operation::Upgrade(id("apt", "one")),
            Operation::Upgrade(id("apt", "two")),
        ];
        let scope = session(&updates);
        apt.prepare(&updates[0], &cancel, &mut |_| {}).unwrap();
        apt.execute_group(&updates, &cancel, &mut |_| {})
            .unwrap()
            .unwrap();
        drop(scope);
        assert_eq!(requests().lines().count(), 2);
        // Skipping the refresh is refused before anything runs as root.
        let scope = session(std::slice::from_ref(&all));
        assert!(apt.execute(&all, &cancel, &mut |_| {}).is_err());
        drop(scope);
        assert_eq!(requests(), "");
        let _ = fs::remove_file(&log);
    }

    #[test]
    fn apt_grouping_stops_at_other_backends_and_preserves_each_verb() {
        let operations = [
            Operation::Remove(id("apt", "one")),
            Operation::Remove(id("apt", "two")),
            Operation::Remove(id("snap", "third")),
            Operation::Upgrade(id("apt", "four")),
            Operation::Upgrade(id("apt", "five")),
            Operation::Install(id("apt", "six")),
        ];
        let commands = batch_commands(&operations).unwrap();
        assert!(commands[1].is_empty());
        assert_eq!(commands[2][0].program, "/usr/bin/snap");
        assert!(commands[4].is_empty());
        assert_eq!(commands[5][0].program, "/usr/bin/apt-get");
        assert!(commands[0][0]
            .args
            .ends_with(&["one:amd64".into(), "two:amd64".into()]));
        // One refresh before both updates.
        assert_eq!(commands[3][0], apt_command(AptAction::Refresh).unwrap());
        assert!(commands[3][1]
            .args
            .ends_with(&["four:amd64".into(), "five:amd64".into()]));
        assert_eq!(commands[0].len(), 1);
        assert_eq!(commands[5].len(), 1);
        assert_eq!(
            commands[0][0]
                .args
                .iter()
                .filter(|value| *value == "remove")
                .count(),
            1
        );
    }

    #[test]
    fn pacman_updates_run_in_one_transaction_and_nothing_else_joins_them() {
        let operations = [
            Operation::Upgrade(id("pacman", "systemd")),
            Operation::Upgrade(id("pacman", "systemd-libs")),
            Operation::Upgrade(id("pacman", "systemd-sysvcompat")),
            Operation::Install(id("pacman", "jq")),
            Operation::Install(id("pacman", "curl")),
            Operation::Upgrade(id("dnf", "other")),
            Operation::Upgrade(id("pacman", "zlib")),
            Operation::Remove(id("pacman", "one")),
            Operation::Remove(id("pacman", "two")),
        ];
        let commands = batch_commands(&operations).unwrap();
        let mut together = manager_command("pacman", "upgrade", None).unwrap();
        together
            .args
            .extend(["systemd", "systemd-libs", "systemd-sysvcompat"].map(String::from));
        assert_eq!(commands[0], [together]);
        assert!(commands[1].is_empty() && commands[2].is_empty());
        // Installs, removals, other sources and a lone update keep their own.
        for (index, commands) in commands.iter().enumerate().skip(3) {
            assert_eq!(*commands, protected_commands(&operations[index]).unwrap());
        }
        assert_eq!(
            (0..operations.len())
                .map(|index| group_end(&operations, index))
                .collect::<Vec<_>>(),
            [3, 3, 3, 4, 5, 6, 7, 8, 9]
        );
        // A name that isn't one is refused for the whole run.
        let bad = [
            Operation::Upgrade(id("pacman", "systemd")),
            Operation::Upgrade(id("pacman", "--sysroot=/")),
        ];
        assert!(batch_commands(&bad).is_err());
    }

    #[test]
    fn managers_that_share_a_lock_share_a_lane() {
        let operations = [
            Operation::Upgrade(id("apt", "one")),
            Operation::Upgrade(id("apt", "two")),
            Operation::UpgradeAll {
                backend: "flatpak".into(),
            },
            Operation::Upgrade(id("homebrew", "jq")),
            Operation::Refresh {
                backend: "apt".into(),
            },
            Operation::Upgrade(id("homebrew-cask", "firefox")),
            Operation::Upgrade(id("rustup", "stable")),
            Operation::Upgrade(id("aur", "yay")),
            Operation::Upgrade(id("pip", "black")),
            Operation::Upgrade(id("pacman", "zlib")),
            Operation::Refresh {
                backend: "flatpak".into(),
            },
        ];
        assert_eq!(
            lanes(&operations),
            [
                vec![0..2, 4..5],
                vec![2..3, 10..11],
                vec![3..4, 5..6],
                // Every manager without a lane of its own shares one.
                vec![6..7, 8..9],
                vec![7..8, 9..10],
            ]
        );
        for backend in ["snap", "dnf", "zypper", "apk", "xbps", "nix"] {
            let refresh = Operation::Refresh {
                backend: backend.into(),
            };
            assert_eq!(lane(&refresh), backend);
        }
        assert!(lanes(&[]).is_empty());
    }

    /// Output that tells `sender` once the runner refuses something. The
    /// runner holds its state while it writes that, so whatever it reads next
    /// sees the refusal.
    struct Watched(Vec<u8>, std::sync::mpsc::Sender<()>);
    impl Write for Watched {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(bytes);
            if bytes.windows(8).any(|word| word == b"rejected") {
                let _ = self.1.send(());
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    /// Input that hands out `later` only once `ready` says so.
    struct Paced(
        Cursor<Vec<u8>>,
        std::sync::mpsc::Receiver<()>,
        Option<Vec<u8>>,
    );
    impl Read for Paced {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let count = self.0.read(buf)?;
            match self.2.take() {
                Some(later) if count == 0 => {
                    let _ = self.1.recv_timeout(Duration::from_secs(10));
                    self.0 = Cursor::new(later);
                    self.0.read(buf)
                }
                later => {
                    self.2 = later;
                    Ok(count)
                }
            }
        }
    }

    #[test]
    fn the_runner_runs_lanes_at_once_and_each_lane_in_order() {
        let operations = vec![
            Operation::Refresh {
                backend: "apt".into(),
            },
            Operation::Refresh {
                backend: "dnf".into(),
            },
            Operation::Install(id("apt", "jq")),
        ];
        let run = |operation: usize| Request::Run {
            operation,
            command: 0,
        };
        let start = || Request::Start {
            protocol: PROTOCOL,
            operations: operations.clone(),
        };
        // apt and dnf meet: each finishes only once both started.
        let (started, ready) = (std::sync::Mutex::new(0), std::sync::Condvar::new());
        let met = std::sync::Mutex::new(vec![]);
        serve_protocol(
            BatchMode::Reviewed,
            &mut Cursor::new(input(&[start(), run(0), run(1)])),
            &mut Vec::new(),
            |_, _| {
                let mut count = started.lock().unwrap();
                *count += 1;
                ready.notify_all();
                let (count, wait) = ready
                    .wait_timeout_while(count, Duration::from_secs(10), |count| *count < 2)
                    .unwrap();
                drop(count);
                met.lock().unwrap().push(!wait.timed_out());
                Ok(completion())
            },
        )
        .unwrap();
        assert_eq!(*met.lock().unwrap(), [true, true]);
        // A lane runs one at a time: apt's second change can't start while
        // its first runs, which only ends once the second was refused.
        let (sender, refused) = std::sync::mpsc::channel();
        let refused = std::sync::Mutex::new(refused);
        let mut output = Watched(vec![], sender);
        let recorder = Recorder::default();
        let result = serve_protocol(
            BatchMode::Reviewed,
            &mut Cursor::new(input(&[start(), run(0), run(2)])),
            &mut output,
            |command, _| {
                recorder.0.lock().unwrap().push(command.clone());
                let _ = refused
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10));
                Ok(completion())
            },
        );
        assert!(matches!(result, Err(ExecutionError::Invalid(_))));
        assert_eq!(recorder.calls().len(), 1);
        let output = String::from_utf8(output.0).unwrap();
        assert!(
            output.contains(
                r#"{"type":"rejected","operation":2,"reason":"command outside approved batch"}"#
            ),
            "{output}"
        );
        assert!(
            output.contains(r#"{"type":"completed","operation":0,"#),
            "{output}"
        );
        // ... and in order: once apt's second change ran, its first can't.
        let recorder = Recorder::default();
        let result = serve_protocol(
            BatchMode::Reviewed,
            &mut Cursor::new(input(&[start(), run(2), run(0)])),
            &mut Vec::new(),
            recorder.run(),
        );
        assert!(matches!(result, Err(ExecutionError::Invalid(_))));
        assert_eq!(recorder.calls().len(), 1);
    }

    #[test]
    fn after_a_command_fails_to_run_the_runner_starts_no_more() {
        let first = input(&[
            Request::Start {
                protocol: PROTOCOL,
                operations: vec![
                    Operation::Refresh {
                        backend: "apt".into(),
                    },
                    Operation::Refresh {
                        backend: "dnf".into(),
                    },
                ],
            },
            Request::Run {
                operation: 0,
                command: 0,
            },
        ]);
        // dnf's request comes in only once apt's failure was sent.
        let later = input(&[Request::Run {
            operation: 1,
            command: 0,
        }]);
        let (sender, failed) = std::sync::mpsc::channel();
        let mut output = Watched(vec![], sender);
        let recorder = Recorder::default();
        let result = serve_protocol(
            BatchMode::Reviewed,
            &mut Paced(Cursor::new(first), failed, Some(later)),
            &mut output,
            |command, _| {
                recorder.0.lock().unwrap().push(command.clone());
                Err(ExecutionError::Io("no such program".into()))
            },
        );
        assert!(matches!(result, Err(ExecutionError::Invalid(_))));
        assert_eq!(recorder.calls().len(), 1);
        let output = String::from_utf8(output.0).unwrap();
        assert!(
            output.contains(r#"{"type":"rejected","operation":0,"reason":"no such program"}"#),
            "{output}"
        );
        assert!(
            output.contains(
                r#"{"type":"rejected","operation":1,"reason":"command outside approved batch"}"#
            ),
            "{output}"
        );
    }

    #[test]
    fn flatpak_refs_validate_short_form_and_reject_malformed_scope() {
        let mut app = id("flatpak", "org.example.Test");
        app.architecture = "x86_64".into();
        app.reference = Some("org.example.Test/x86_64/stable".into());
        assert_eq!(flatpak_ref(&app).unwrap(), "org.example.Test/x86_64/stable");
        app.reference = Some("org.example.Other/x86_64/stable".into());
        assert!(flatpak_ref(&app).is_err());
        app.reference = Some("org.example.Test/x86_64".into());
        assert!(flatpak_ref(&app).is_err());
        app.reference = None;
        app.scope = Scope::User { uid: 1000 };
        assert!(flatpak_ref(&app).is_err());
        assert!(manager_command("unsupported", "refresh", None).is_err());
        // `snap refresh` upgrades every snap, so a refresh never maps to it.
        assert!(manager_command("snap", "refresh", None).is_err());
    }

    #[test]
    fn metadata_refreshes_of_flatpak_fwupd_and_snap_never_elevate() {
        for backend in ["flatpak", "fwupd", "snap"] {
            let refresh = Operation::Refresh {
                backend: backend.into(),
            };
            assert!(
                protected_commands(&refresh).unwrap().is_empty(),
                "{backend}"
            );
        }
        // Refreshing needs no approval, so an unattended run may do it too.
        let refreshes = UPGRADE_ONLY_BACKENDS
            .iter()
            .map(|backend| Operation::Refresh {
                backend: (*backend).into(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            check_mode(BatchMode::UpgradeOnly { removals: false }, &refreshes),
            Ok(())
        );
    }

    #[test]
    fn protocol_rejects_malformed_followup_and_propagates_native_failure() {
        let start = Request::Start {
            protocol: PROTOCOL,
            operations: vec![Operation::Refresh {
                backend: "apt".into(),
            }],
        };
        let mut malformed = input(&[start]);
        malformed.extend_from_slice(b"not-json\n");
        let mut output = Vec::new();
        let recorder = Recorder::default();
        assert!(matches!(
            serve_protocol(
                BatchMode::Reviewed,
                &mut Cursor::new(malformed),
                &mut output,
                recorder.run()
            ),
            Err(ExecutionError::Invalid(_))
        ));
        assert!(recorder.calls().is_empty());
        assert!(matches!(
            serde_json::from_slice::<Response>(output.split(|b| *b == b'\n').next().unwrap())
                .unwrap(),
            Response::Ready { .. }
        ));

        let requests = [
            Request::Start {
                protocol: PROTOCOL,
                operations: vec![Operation::Refresh {
                    backend: "apt".into(),
                }],
            },
            Request::Run {
                operation: 0,
                command: 0,
            },
        ];
        let mut output = Vec::new();
        assert!(matches!(
            serve_protocol(
                BatchMode::Reviewed,
                &mut Cursor::new(input(&requests)),
                &mut output,
                |_, _| Err(ExecutionError::AuthorizationDenied)
            ),
            Err(ExecutionError::AuthorizationDenied)
        ));
        // The frontend hears why, for that operation.
        let replies: Vec<Response> = output
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert!(matches!(
            &replies[..],
            [
                Response::Ready { .. },
                Response::Rejected { operation: 0, .. }
            ]
        ));
    }

    #[test]
    fn session_rejects_unexpected_completion_and_oversized_ready() {
        let operations = [Operation::Refresh {
            backend: "apt".into(),
        }];
        let commands = batch_commands(&operations).unwrap();
        let mut oversized = Command::new("/bin/sh");
        oversized.arg("-c").arg(format!(
            "IFS= read -r start; printf '%{}s\\n' x; sleep 1",
            MAX_MESSAGE + 1
        ));
        assert!(matches!(
            begin_session(
                oversized,
                &operations,
                commands.clone(),
                &Cancellation::default()
            ),
            Err(ExecutionError::Invalid(_))
        ));

        let mut child = Command::new("/bin/sh");
        child.arg("-c").arg("IFS= read -r start; printf '%s\\n' '{\"type\":\"ready\",\"protocol\":3}'; IFS= read -r request; printf '%s\\n' '{\"type\":\"ready\",\"protocol\":3}'");
        let cancel = Cancellation::default();
        let scope = begin_session(child, &operations, commands.clone(), &cancel)
            .unwrap()
            .unwrap();
        let command = &commands[0][0];
        let args = command.args.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(matches!(
            run_in_scope(Path::new(&command.program), &args, &cancel),
            Some(Err(ExecutionError::Invalid(_)))
        ));
        drop(scope);
    }

    #[test]
    fn nested_batch_is_rejected_before_starting_another_runner() {
        use std::collections::BTreeMap;
        let operations = [Operation::Refresh {
            backend: "apt".into(),
        }];
        let commands = batch_commands(&operations).unwrap();
        let mut child = Command::new("/bin/sh");
        child.arg("-c").arg("IFS= read -r start; printf '%s\\n' '{\"type\":\"ready\",\"protocol\":3}'; IFS= read -r request");
        let cancel = Cancellation::default();
        let scope = begin_session(child, &operations, commands, &cancel)
            .unwrap()
            .unwrap();
        let host = Host::new(Runtime::Native, BTreeMap::new());
        assert!(matches!(
            begin(
                &host,
                Authorization::Polkit,
                BatchMode::Reviewed,
                &operations,
                &cancel
            ),
            Err(ExecutionError::Invalid(_))
        ));
        drop(scope);
    }

    #[test]
    fn runner_fails_closed_when_ready_channel_closes_or_authorization_fails() {
        let operations = [Operation::Refresh {
            backend: "apt".into(),
        }];
        let commands = batch_commands(&operations).unwrap();
        for script in [
            "IFS= read -r start; exec 1>&-; sleep 1",
            "IFS= read -r start; exit 1",
        ] {
            let mut child = Command::new("/bin/sh");
            child.arg("-c").arg(script);
            assert!(matches!(
                begin_session(
                    child,
                    &operations,
                    commands.clone(),
                    &Cancellation::default()
                ),
                Err(ExecutionError::AuthorizationDenied)
            ));
        }
    }

    #[test]
    fn runner_stops_before_dispatch_when_ready_cannot_be_written() {
        let start = Request::Start {
            protocol: PROTOCOL,
            operations: vec![Operation::Refresh {
                backend: "apt".into(),
            }],
        };
        let recorder = Recorder::default();
        let mut full: &mut [u8] = &mut [];
        let result = serve_protocol(
            BatchMode::Reviewed,
            &mut Cursor::new(input(&[start])),
            &mut full,
            recorder.run(),
        );
        assert!(matches!(result, Err(ExecutionError::Io(_))));
        assert!(recorder.calls().is_empty());
    }

    /// Room for exactly the Ready frame; every later reply fails to write.
    fn ready_only() -> Vec<u8> {
        let mut ready = Vec::new();
        send(&mut ready, &Response::Ready { protocol: PROTOCOL }).unwrap();
        vec![0; ready.len()]
    }

    #[test]
    fn runner_stops_when_a_reply_cannot_be_written() {
        let refresh = || Request::Start {
            protocol: PROTOCOL,
            operations: vec![Operation::Refresh {
                backend: "apt".into(),
            }],
        };
        // The rejection of an out-of-plan request cannot be delivered.
        let recorder = Recorder::default();
        let mut buffer = ready_only();
        let result = serve_protocol(
            BatchMode::Reviewed,
            &mut Cursor::new(input(&[
                refresh(),
                Request::Run {
                    operation: 0,
                    command: 1,
                },
            ])),
            &mut &mut buffer[..],
            recorder.run(),
        );
        assert!(matches!(result, Err(ExecutionError::Io(_))));
        assert!(recorder.calls().is_empty());
        // Nor can a finished command's completion, after it ran once.
        let mut buffer = ready_only();
        let result = serve_protocol(
            BatchMode::Reviewed,
            &mut Cursor::new(input(&[
                refresh(),
                Request::Run {
                    operation: 0,
                    command: 0,
                },
            ])),
            &mut &mut buffer[..],
            recorder.run(),
        );
        assert!(matches!(result, Err(ExecutionError::Io(_))));
        assert_eq!(recorder.calls().len(), 1);
    }

    #[test]
    fn only_root_serves_the_protocol() {
        let invalid_plan = input(&[Request::Start {
            protocol: PROTOCOL,
            operations: vec![],
        }]);
        let mut input = Cursor::new(invalid_plan);
        let mut output = Vec::new();
        assert!(matches!(
            serve_as(false, BatchMode::Reviewed, &mut input, &mut output),
            Err(ExecutionError::Invalid(reason)) if reason == "host runner requires root"
        ));
        assert_eq!(input.position(), 0);
        // As root the request is read and validated before anything runs.
        assert!(matches!(
            serve_as(true, BatchMode::Reviewed, &mut input, &mut output),
            Err(ExecutionError::Invalid(reason)) if reason.contains("invalid authorization protocol")
        ));
        assert!(output.is_empty());
        // A root test process would read this process's stdin, so skip it.
        assert!(
            rustix::process::geteuid().is_root()
                || matches!(
                    serve(BatchMode::Reviewed),
                    Err(ExecutionError::Invalid(reason)) if reason == "host runner requires root"
                )
        );
    }

    #[test]
    fn approved_commands_run_with_a_fixed_system_environment() {
        let mut lines = Vec::new();
        let result = run_protected(
            &ProtectedCommand {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "printf '%s|%s|%s' \"$PATH\" \"$LC_ALL\" \"$PWD\"".into(),
                ],
            },
            &mut |line| lines.push(line.to_owned()),
        )
        .unwrap();
        assert_eq!(result.code, Some(0));
        assert_eq!(result.stdout, format!("{SYSTEM_PATH}|C|/").as_bytes());
        assert_eq!(lines, [format!("{SYSTEM_PATH}|C|/")]);
    }

    #[test]
    fn waits_for_authorization_retry_interrupts_and_time_out() {
        assert_eq!(unless_interrupted(Ok(1)), Ok(()));
        assert_eq!(unless_interrupted(Err(rustix::io::Errno::INTR)), Ok(()));
        assert_eq!(
            unless_interrupted(Err(rustix::io::Errno::BADF)),
            Err(rustix::io::Errno::BADF)
        );
        let operations = [Operation::Refresh {
            backend: "apt".into(),
        }];
        let mut child = Command::new("/bin/sh");
        child.arg("-c").arg("IFS= read -r start; sleep 5");
        let started = Instant::now();
        assert!(matches!(
            begin_session_within(
                child.into(),
                &operations,
                batch_commands(&operations).unwrap(),
                &Cancellation::default(),
                Duration::ZERO
            ),
            Err(ExecutionError::AuthorizationDenied)
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_runner_that_closes_its_input_at_once_is_a_denial() {
        // A plan larger than a pipe buffer cannot be written to a helper
        // that stopped reading.
        let operations = vec![
            Operation::Refresh {
                backend: "apt".into(),
            };
            4096
        ];
        let commands = batch_commands(&operations).unwrap();
        let mut child = Command::new("/bin/sh");
        child.arg("-c").arg("exec 0<&-; sleep 5");
        assert!(matches!(
            begin_session(child, &operations, commands, &Cancellation::default()),
            Err(ExecutionError::AuthorizationDenied)
        ));
    }

    #[test]
    fn a_runner_that_stops_reading_requests_fails_the_command() {
        let operations = [Operation::Refresh {
            backend: "apt".into(),
        }];
        let commands = batch_commands(&operations).unwrap();
        let mut child = Command::new("/bin/sh");
        child.arg("-c").arg(
            "IFS= read -r start; exec 0<&-; printf '%s\\n' '{\"type\":\"ready\",\"protocol\":3}'; sleep 5",
        );
        let cancel = Cancellation::default();
        let scope = begin_session(child, &operations, commands.clone(), &cancel)
            .unwrap()
            .unwrap();
        set_operation(0);
        let command = &commands[0][0];
        let args = command.args.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(matches!(
            run_in_scope(Path::new(&command.program), &args, &cancel),
            Some(Err(ExecutionError::Io(_)))
        ));
        drop(scope);
    }

    #[test]
    fn runner_commands_start_the_runner_through_the_authorization_prompt() {
        use std::collections::BTreeMap;
        let host = Host::new(Runtime::Native, BTreeMap::new());
        let runner = PathBuf::from("/opt/pkgdeck/libexec/pkgdeck-host-runner");
        assert!(
            runner_command(&host, Authorization::Polkit, BatchMode::Reviewed, None)
                .unwrap()
                .is_none()
        );
        let args = |command: &Launch| {
            command
                .args()
                .into_iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        let command = runner_command(
            &host,
            Authorization::Polkit,
            BatchMode::Reviewed,
            Some((runner.clone(), true)),
        )
        .unwrap()
        .unwrap();
        assert_eq!(command.program(), "/usr/bin/pkexec");
        assert_eq!(
            args(&command),
            [
                "--disable-internal-agent",
                "/opt/pkgdeck/libexec/pkgdeck-host-runner",
                "--batch-runner"
            ]
        );
        let command = runner_command(
            &host,
            Authorization::SudoNonInteractive,
            BatchMode::Reviewed,
            Some((runner, false)),
        )
        .unwrap()
        .unwrap();
        assert_eq!(command.program(), "/usr/bin/sudo");
        assert_eq!(
            args(&command),
            ["-n", "--", "/opt/pkgdeck/libexec/pkgdeck-host-runner"]
        );
        // Outside Flatpak an AppImage without its launcher has no packaged
        // runner beside the executable either.
        // (Inside a real AppImage its launcher would be used instead.)
        let appimage = Host::new(Runtime::AppImage, BTreeMap::new());
        let refresh = [Operation::Refresh {
            backend: "apt".into(),
        }];
        assert!(
            std::env::var_os("APPIMAGE").is_some()
                || begin(
                    &appimage,
                    Authorization::Polkit,
                    BatchMode::Reviewed,
                    &refresh,
                    &Cancellation::default()
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(
            find_runner(Runtime::AppImage, Path::new("/nonexistent/bin/pkd"), ""),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_found_runner_starts_and_a_missing_one_refuses_unattended_batches() {
        let refresh = [Operation::Refresh {
            backend: "apt".into(),
        }];
        let commands = || batch_commands(&refresh).unwrap();
        let cancel = Cancellation::default();
        let upgrade_only = BatchMode::UpgradeOnly { removals: false };
        // A bridge that exits at once: the runner never says it is ready.
        let mut flatpak = Host::new(Runtime::Flatpak, Default::default());
        flatpak.set_bridge_for_tests(Path::new("/usr/bin/true"));
        let runner = PathBuf::from("/opt/pkgdeck/libexec/pkgdeck-host-runner");
        assert!(start_runner(
            &flatpak,
            Authorization::Polkit,
            upgrade_only,
            Some((runner, false)),
            &refresh,
            commands(),
            &cancel,
        )
        .is_err());
        assert!(SESSION.with(|cell| cell.borrow().is_none()));
        let native = Host::new(Runtime::Native, Default::default());
        assert!(matches!(
            start_runner(
                &native,
                Authorization::Polkit,
                upgrade_only,
                None,
                &refresh,
                commands(),
                &cancel,
            ),
            Err(ExecutionError::Disabled(reason)) if reason.contains("system helper")
        ));
        assert!(start_runner(
            &native,
            Authorization::Polkit,
            BatchMode::Reviewed,
            None,
            &refresh,
            commands(),
            &cancel,
        )
        .unwrap()
        .is_none());
    }
}
