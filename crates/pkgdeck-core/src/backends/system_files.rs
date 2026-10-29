//! Files the system keeps that nothing needs after a while: system logs
//! older than a week, and what crashed programs left for reporting. They
//! belong to root, so they're sized without a password and deleted with
//! one, by the exact commands in [`crate::batch::protected_commands`].
use super::*;

const CAPABILITIES: &[Capability] = &[Capability::Clean];
/// Smaller ones aren't worth a line.
const WORTH_CLEANING: u64 = 1024 * 1024;
/// How much of the system log stays, in journalctl's words and in days.
pub(crate) const KEEP_LOGS: &str = "1week";
const KEEP_LOG_DAYS: &str = "+7";
pub(crate) const COREDUMPS: &str = "/var/lib/systemd/coredump";
pub(crate) const CRASH_REPORTS: &str = "/var/crash";

/// One task: what `find` matches, how it's cleaned and how it's told.
struct Task {
    key: &'static str,
    title: &'static str,
    summary: &'static str,
    preview: &'static str,
    /// Where `find` looks, and what it matches there.
    find: &'static [&'static str],
}

const TASKS: &[Task] = &[
    Task {
        key: "journal",
        title: "Old system logs",
        summary: "Logs from more than a week ago",
        preview: "Deletes system logs older than a week. The last week stays.",
        find: &[
            "/var/log/journal",
            "-mindepth",
            "2",
            "-maxdepth",
            "2",
            "-type",
            "f",
            "-name",
            "*@*.journal*",
            "-mtime",
            KEEP_LOG_DAYS,
        ],
    },
    Task {
        key: "coredumps",
        title: "Crash dumps",
        summary: "Memory of programs that crashed, kept to debug them",
        preview: "Deletes every crash dump systemd kept.",
        find: &[
            COREDUMPS,
            "-mindepth",
            "1",
            "-maxdepth",
            "1",
            "-type",
            "f",
            "-name",
            "core.*",
        ],
    },
    Task {
        key: "crash-reports",
        title: "Crash reports",
        summary: "Reports of programs that crashed, waiting to be sent",
        preview: "Deletes every crash report waiting in /var/crash.",
        find: &[
            CRASH_REPORTS,
            "-mindepth",
            "1",
            "-maxdepth",
            "1",
            "-type",
            "f",
            "(",
            "-name",
            "*.crash",
            "-o",
            "-name",
            "*.upload",
            "-o",
            "-name",
            "*.uploaded",
            ")",
        ],
    },
];

/// The `find` arguments that delete what task `key` lists, for the
/// runner. `None` for any other key.
pub(crate) fn delete_args(key: &str) -> Option<Vec<String>> {
    let task = TASKS.iter().find(|task| task.key == key)?;
    if key == "journal" {
        return Some(vec![format!("--vacuum-time={KEEP_LOGS}")]);
    }
    let mut args: Vec<String> = task.find.iter().map(|arg| (*arg).to_owned()).collect();
    args.push("-delete".into());
    Some(args)
}

pub struct SystemFiles<T: Transport> {
    transport: T,
}

impl<T: Transport> SystemFiles<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    /// The task, when there is enough to clean. A folder that isn't there
    /// has nothing in it.
    fn item(&self, task: &Task, cancel: &Cancellation) -> Result<Option<CleanupItem>, EngineError> {
        let mut args: Vec<OsString> = task.find.iter().map(OsString::from).collect();
        args.extend(["-printf".into(), "%b\\n".into()]);
        let listed = match self.transport.system_manager("find", &args, cancel, false) {
            Ok(listed) => listed,
            Err(ExecutionError::Failed(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let blocks = String::from_utf8(bytes("system-files", listed)?)
            .map_err(|error| invalid("system-files", error))?
            .lines()
            .map(|line| line.trim().parse::<u64>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| invalid("system-files", error))?;
        let total = blocks.iter().sum::<u64>() * 512;
        Ok((total >= WORTH_CLEANING).then(|| CleanupItem {
            id: CleanupId {
                backend: "system-files".into(),
                key: task.key.into(),
            },
            kind: CleanupKind::PackageCache,
            title: task.title.into(),
            summary: task.summary.into(),
            preview: format!(
                "{} {} file{}.",
                task.preview,
                blocks.len(),
                if blocks.len() == 1 { "" } else { "s" }
            ),
            bytes: Some(total),
        }))
    }
}

impl<T: Transport> Backend for SystemFiles<T> {
    fn id(&self) -> &str {
        "system-files"
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, _cancel: &Cancellation) -> Result<Availability, EngineError> {
        Ok(if cfg!(target_os = "linux") {
            Availability::Available
        } else {
            Availability::Unavailable("Only Linux keeps these.".into())
        })
    }
    fn cleanup(&mut self, cancel: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        let mut items = vec![];
        for task in TASKS {
            items.extend(self.item(task, cancel)?);
        }
        Ok(items)
    }
    fn cleanup_plan(
        &mut self,
        id: &CleanupId,
        cancel: &Cancellation,
    ) -> Result<CleanupItem, EngineError> {
        let task = TASKS
            .iter()
            .find(|task| id.backend == "system-files" && task.key == id.key)
            .ok_or(EngineError::NotFound)?;
        self.item(task, cancel)?.ok_or(EngineError::NotFound)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let Operation::Clean(id) = operation else {
            return Err(self.unsupported(operation.capability()));
        };
        let args = delete_args(&id.key)
            .filter(|_| id.backend == "system-files")
            .ok_or(EngineError::NotFound)?;
        let plan = self.cleanup_plan(id, cancel)?;
        progress(Progress::Message(format!("Deleting {}…", plan.title.to_lowercase())));
        let program = if id.key == "journal" { "journalctl" } else { "find" };
        let args: Vec<OsString> = args.into_iter().map(OsString::from).collect();
        let result = self.transport.system_manager(program, &args, cancel, true)?;
        bytes("system-files", result.clone())?;
        Ok(OperationOutcome {
            cancellation_deferred: result.cancellation_deferred,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Answers each `find` with block counts, by the folder it looks in.
    #[derive(Clone, Default)]
    struct Disk {
        listed: Vec<(&'static str, Result<&'static str, ()>)>,
        ran: Arc<Mutex<Vec<String>>>,
    }
    impl Transport for Disk {
        fn system_manager(
            &self,
            executable: &str,
            args: &[OsString],
            _: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            let line = std::iter::once(executable.to_owned())
                .chain(args.iter().map(|arg| arg.to_string_lossy().into_owned()))
                .collect::<Vec<_>>()
                .join(" ");
            self.ran
                .lock()
                .unwrap()
                .push(if write { format!("root {line}") } else { line });
            if write {
                return Ok(completion(0, ""));
            }
            let folder = args[0].to_string_lossy();
            match self.listed.iter().find(|(listed, _)| *listed == folder) {
                Some((_, Ok(blocks))) => Ok(completion(0, blocks)),
                Some((_, Err(()))) => Err(ExecutionError::Disabled("find not found".into())),
                None => Err(ExecutionError::Failed(completion(1, ""))),
            }
        }
    }
    fn completion(code: i32, stdout: &str) -> Completion {
        Completion {
            code: Some(code),
            signal: None,
            stdout: stdout.as_bytes().to_vec(),
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
        }
    }

    #[test]
    fn old_logs_and_crashes_are_sized_without_root_and_deleted_with_it() {
        let disk = Disk {
            // Two big logs, one tiny crash dump, no report folder.
            listed: vec![
                ("/var/log/journal", Ok("40960\n40960\n")),
                (COREDUMPS, Ok("8\n")),
            ],
            ..Disk::default()
        };
        let mut backend = SystemFiles::new(disk.clone());
        let cancel = Cancellation::default();
        assert_eq!(
            backend.detect(&cancel).unwrap() == Availability::Available,
            cfg!(target_os = "linux")
        );
        let items = backend.cleanup(&cancel).unwrap();
        assert_eq!(items.len(), 1, "{items:?}");
        let logs = &items[0];
        assert_eq!(logs.id.key, "journal");
        assert_eq!(logs.bytes, Some(2 * 40960 * 512));
        assert_eq!(
            logs.preview,
            "Deletes system logs older than a week. The last week stays. 2 files."
        );
        assert!(!logs.automatic(), "it needs a password");
        assert_eq!(backend.cleanup_plan(&logs.id, &cancel).unwrap(), *logs);
        let mut said = vec![];
        backend
            .execute(&Operation::Clean(logs.id.clone()), &cancel, &mut |progress| {
                said.push(progress)
            })
            .unwrap();
        assert_eq!(said, [Progress::Message("Deleting old system logs…".into())]);
        let ran = disk.ran.lock().unwrap().clone();
        assert_eq!(
            ran.last().unwrap(),
            "root journalctl --vacuum-time=1week"
        );
        assert!(ran[0].starts_with("find /var/log/journal -mindepth 2 -maxdepth 2 -type f -name *@*.journal* -mtime +7 -printf"));
        // Too little, or nothing there, is no task.
        for key in ["coredumps", "crash-reports", "other"] {
            let id = CleanupId {
                backend: "system-files".into(),
                key: key.into(),
            };
            assert!(matches!(backend.cleanup_plan(&id, &cancel), Err(EngineError::NotFound)));
            assert!(matches!(
                backend.execute(&Operation::Clean(id), &cancel, &mut |_| {}),
                Err(EngineError::NotFound)
            ));
        }
        assert!(matches!(
            backend.execute(
                &Operation::Refresh {
                    backend: "system-files".into()
                },
                &cancel,
                &mut |_| {}
            ),
            Err(EngineError::Unsupported { .. })
        ));
    }

    #[test]
    fn crashes_are_deleted_by_find_and_a_missing_find_is_an_error() {
        let disk = Disk {
            listed: vec![
                (COREDUMPS, Ok("4096\n")),
                (CRASH_REPORTS, Ok("2048\n2048\n")),
                ("/var/log/journal", Ok("")),
            ],
            ..Disk::default()
        };
        let mut backend = SystemFiles::new(disk.clone());
        let cancel = Cancellation::default();
        let items = backend.cleanup(&cancel).unwrap();
        let keys: Vec<_> = items.iter().map(|item| item.id.key.as_str()).collect();
        assert_eq!(keys, ["coredumps", "crash-reports"]);
        assert_eq!(items[0].preview, "Deletes every crash dump systemd kept. 1 file.");
        for item in &items {
            backend
                .execute(&Operation::Clean(item.id.clone()), &cancel, &mut |_| {})
                .unwrap();
            let ran = disk.ran.lock().unwrap().last().unwrap().clone();
            assert_eq!(
                ran,
                format!("root find {}", delete_args(&item.id.key).unwrap().join(" "))
            );
        }
        assert!(delete_args("coredumps").unwrap().ends_with(&["-delete".into()]));
        assert_eq!(delete_args("other"), None);
        let broken = Disk {
            listed: vec![("/var/log/journal", Err(()))],
            ..Disk::default()
        };
        assert!(SystemFiles::new(broken).cleanup(&cancel).is_err());
        let garbled = Disk {
            listed: vec![("/var/log/journal", Ok("lots\n"))],
            ..Disk::default()
        };
        assert!(SystemFiles::new(garbled).cleanup(&cancel).is_err());
    }
}
