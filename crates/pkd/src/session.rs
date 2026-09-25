//! The interactive side of a change: review, approval, and live progress.
use crate::live::Live;
use crate::presentation::{self, Paint};
use pkgdeck_core::{
    engine::{EngineError, Event},
    package::{Operation, Progress},
    process::{Cancellation, ExecutionError},
};
use std::cell::{Cell, RefCell};
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

pub struct Session<'a> {
    live: &'a Live,
    color: bool,
    /// Print a ✓/✗ line per finished change. Off when stderr is not a
    /// terminal, where stdout carries the full result instead.
    show_results: bool,
    /// Details a backend reports while planning, such as the APT
    /// transaction, belong in the review. Messages after that are progress.
    notes: RefCell<Vec<(Operation, String)>>,
    reviewed: Cell<bool>,
    executing: Cell<bool>,
    position: Cell<usize>,
    total: Cell<usize>,
    shown: Cell<bool>,
}

impl<'a> Session<'a> {
    pub fn new(live: &'a Live) -> Self {
        Self {
            live,
            color: live.color(),
            show_results: live.animated(),
            notes: RefCell::default(),
            reviewed: Cell::new(false),
            executing: Cell::new(false),
            position: Cell::new(0),
            total: Cell::new(0),
            shown: Cell::new(false),
        }
    }
    /// True once each change got its own result line.
    pub fn results_shown(&self) -> bool {
        self.shown.get()
    }
    fn review(&self, operations: &[Operation]) {
        self.live.clear();
        eprintln!(
            "{}\n",
            presentation::plan(operations, &self.notes.borrow(), self.color)
        );
        self.reviewed.set(true);
    }
    /// Show the review and ask to apply it.
    pub fn confirm(&self, operations: &[Operation], input: &mut dyn BufRead) -> bool {
        self.review(operations);
        let count = operations.len();
        let question = if count == 1 {
            "Apply this change?".to_string()
        } else {
            format!("Apply these {count} changes?")
        };
        ask(&Paint(self.color).bold(&question), input)
    }
    /// Approved changes are about to start. With `--yes` nothing was
    /// reviewed yet, so show what runs; a refresh needs no review.
    pub fn start(&self, operations: &[Operation], review: bool) {
        if review && !self.reviewed.get() {
            self.review(operations);
        }
        self.total.set(operations.len());
        self.executing.set(true);
    }
    pub fn event(&self, event: Event) {
        match event {
            Event::Started(operation) => {
                self.position.set(self.position.get() + 1);
                let counter = if self.total.get() > 1 {
                    format!(" ({}/{})", self.position.get(), self.total.get())
                } else {
                    String::new()
                };
                self.live.status(format!(
                    "{}{counter}",
                    presentation::operation_progress(&operation)
                ));
            }
            Event::Progress {
                operation,
                progress: Progress::Message(message),
            } => {
                if self.executing.get() {
                    self.live.detail(&message);
                } else {
                    self.notes.borrow_mut().push((operation, message));
                }
            }
            Event::Progress {
                progress: Progress::Transfer { completed, total },
                ..
            } => self.live.detail(&transfer(completed, total)),
            Event::Finished { operation, result } => {
                if !self.show_results {
                    return;
                }
                let error = result.as_ref().err().map(presentation::error_message);
                let deferred = result
                    .as_ref()
                    .is_ok_and(|outcome| outcome.cancellation_deferred);
                self.live.line(&presentation::result_line(
                    &operation,
                    error.as_deref(),
                    deferred,
                    self.color,
                ));
                self.shown.set(true);
            }
        }
    }
}

fn transfer(completed: u64, total: Option<u64>) -> String {
    match total {
        Some(total) if total > 0 => format!("{}%", completed * 100 / total),
        _ => format!("{} KB", completed / 1024),
    }
}

/// A yes/no question on stderr; anything but an explicit yes means no.
pub fn ask(question: &str, input: &mut dyn BufRead) -> bool {
    eprint!("{question} [y/N] ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    input.read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Package managers whose changes need administrator access in this batch.
pub fn protected_sources(operations: &[Operation]) -> Vec<String> {
    let Ok(commands) = pkgdeck_core::batch::batch_commands(operations) else {
        return vec![];
    };
    let mut sources: Vec<String> = Vec::new();
    for (operation, commands) in operations.iter().zip(commands) {
        let backend = operation.backend().to_string();
        if !commands.is_empty() && !sources.contains(&backend) {
            sources.push(backend);
        }
    }
    sources
}

/// `sudo -n` only works with a cached login, so in a terminal ask sudo for
/// the password before changes start. sudo reads it, not PkgDeck.
pub fn sudo_login(
    live: &Live,
    operations: &[Operation],
    cancel: &Cancellation,
) -> Result<(), EngineError> {
    use pkgdeck_core::host::{Host, Runtime};
    let sources = protected_sources(operations);
    if sources.is_empty()
        || !io::stdin().is_terminal()
        || !io::stderr().is_terminal()
        || !matches!(Host::current().runtime, Runtime::Native | Runtime::AppImage)
    {
        return Ok(());
    }
    sudo_prompt(live, &sources, cancel, Path::new("/usr/bin/sudo"))
}

fn sudo_prompt(
    live: &Live,
    sources: &[String],
    cancel: &Cancellation,
    sudo: &Path,
) -> Result<(), EngineError> {
    let cached = std::process::Command::new(sudo)
        .args(["-n", "-v"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if cached {
        return Ok(());
    }
    live.clear();
    eprintln!(
        "{}",
        Paint(live.color()).dim(&format!(
            "Administrator access is needed for {}.",
            sources.join(", ")
        ))
    );
    let granted = std::process::Command::new(sudo)
        .args(["-v", "-p", "Password for %p: "])
        .status()
        .is_ok_and(|status| status.success());
    if granted {
        Ok(())
    } else if cancel.requested() {
        Err(ExecutionError::AuthorizationCancelled.into())
    } else {
        Err(ExecutionError::AuthorizationDenied.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pkgdeck_core::package::{OperationOutcome, PackageId, Scope};

    fn apt(name: &str) -> Operation {
        Operation::Install(PackageId {
            backend: "apt".into(),
            name: name.into(),
            architecture: "amd64".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        })
    }
    fn homebrew() -> Operation {
        Operation::UpgradeAll {
            backend: "homebrew".into(),
        }
    }

    #[test]
    fn review_notes_approval_and_progress_follow_the_batch() {
        let live = Live::forced(false);
        let mut session = Session::new(&live);
        session.show_results = true;
        let operations = [apt("fixture"), homebrew()];
        // Planning messages become review notes, not progress.
        session.event(Event::Progress {
            operation: operations[0].clone(),
            progress: Progress::Message("Update (1): fixture".into()),
        });
        assert_eq!(session.notes.borrow().len(), 1);
        assert!(!session.confirm(&operations, &mut &b"\n"[..]));
        assert!(session.confirm(&operations, &mut &b"YES\n"[..]));
        assert!(session.confirm(&operations[..1], &mut &b"y\n"[..]));
        session.start(&operations, true);
        assert_eq!(session.total.get(), 2);
        session.event(Event::Started(operations[0].clone()));
        session.event(Event::Progress {
            operation: operations[0].clone(),
            progress: Progress::Message("Unpacking fixture".into()),
        });
        assert_eq!(session.notes.borrow().len(), 1);
        session.event(Event::Progress {
            operation: operations[0].clone(),
            progress: Progress::Transfer {
                completed: 512,
                total: Some(1024),
            },
        });
        session.event(Event::Finished {
            operation: operations[0].clone(),
            result: Ok(OperationOutcome {
                cancellation_deferred: true,
            }),
        });
        session.event(Event::Started(operations[1].clone()));
        session.event(Event::Finished {
            operation: operations[1].clone(),
            result: Err(ExecutionError::LockBusy.into()),
        });
        assert!(session.results_shown());
        assert_eq!(session.position.get(), 2);
    }

    #[test]
    fn unattended_changes_are_shown_once_and_refreshes_skip_review() {
        let live = Live::forced(true);
        let session = Session::new(&live);
        session.start(&[homebrew()], false);
        assert!(!session.reviewed.get());
        session.start(&[homebrew()], true);
        assert!(session.reviewed.get());
        // Without a terminal, stdout carries the results instead.
        let quiet = Live::new(false);
        let session = Session::new(&quiet);
        session.event(Event::Finished {
            operation: homebrew(),
            result: Ok(OperationOutcome::default()),
        });
        assert!(!session.results_shown() || quiet.animated());
    }

    #[test]
    fn transfer_progress_reads_as_percent_or_size() {
        assert_eq!(transfer(50, Some(200)), "25%");
        assert_eq!(transfer(4096, None), "4 KB");
        assert_eq!(transfer(4096, Some(0)), "4 KB");
    }

    #[test]
    fn only_system_changes_need_administrator_access() {
        assert_eq!(
            protected_sources(&[apt("one"), apt("two"), homebrew()]),
            ["apt"]
        );
        assert!(protected_sources(&[homebrew()]).is_empty());
        // Tests never run in a terminal, so no prompt is attempted.
        let live = Live::new(false);
        assert!(sudo_login(&live, &[apt("one")], &Cancellation::default()).is_ok());
        assert!(sudo_login(&live, &[homebrew()], &Cancellation::default()).is_ok());
    }

    #[test]
    fn sudo_prompt_uses_a_cached_login_or_asks_once() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("pkd-sudo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = |name: &str, script: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let live = Live::new(false);
        let sources = ["apt".to_string()];
        let cancel = Cancellation::default();
        let cached = fake("cached", "exit 0");
        assert!(sudo_prompt(&live, &sources, &cancel, &cached).is_ok());
        let asks = fake("asks", r#"[ "$1" = -n ] && exit 1; exit 0"#);
        assert!(sudo_prompt(&live, &sources, &cancel, &asks).is_ok());
        let denied = fake("denied", "exit 1");
        assert_eq!(
            sudo_prompt(&live, &sources, &cancel, &denied),
            Err(ExecutionError::AuthorizationDenied.into())
        );
        cancel.cancel();
        assert_eq!(
            sudo_prompt(&live, &sources, &cancel, &denied),
            Err(ExecutionError::AuthorizationCancelled.into())
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
