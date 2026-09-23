use clap::{Parser, Subcommand, ValueEnum};
use pkgdeck_core::{
    engine::*,
    host::{Authorization, Host},
    inspection::{audit, inspect_native, native_leftovers},
    package::*,
    process::{Cancellation, ExecutionError},
};
use serde_json::{json, Value};
use std::io::{self, IsTerminal, Write};

#[derive(Parser)]
#[command(version = pkgdeck_core::VERSION, about = "PkgDeck package manager")]
pub struct Args {
    /// Emit machine-readable JSON instead of a human-readable table.
    #[arg(long, global = true)]
    pub json: bool,
    /// Restrict operations to the given sources. Repeatable; empty means
    /// every available source.
    #[arg(long, global = true, value_parser = ["fwupd", "apt", "dnf", "pacman", "zypper", "snap", "homebrew", "homebrew-cask", "appimage", "flatpak", "docker", "podman", "cargo", "npm", "pnpm", "bun", "pip", "pipx", "uv", "composer", "gem", "codex", "claude", "grok", "opencode"])]
    pub from: Vec<String>,
    /// Select the package architecture when a name is ambiguous.
    #[arg(long, global = true)]
    pub arch: Option<String>,
    /// Select the package installation (user or system).
    #[arg(long, global = true, value_enum)]
    pub scope: Option<InstallScope>,
    /// Approve native package and dependency changes without prompting.
    #[arg(long, short = 'y', global = true)]
    pub yes: bool,
    /// Authorize system changes using existing sudo credentials or host polkit.
    #[arg(long, global = true, value_enum, default_value = "sudo")]
    pub auth: Auth,
    #[command(subcommand)]
    pub command: Option<Commands>,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum InstallScope {
    User,
    System,
}
impl InstallScope {
    fn native(self) -> Scope {
        match self {
            Self::User => Scope::User {
                uid: rustix::process::getuid().as_raw(),
            },
            Self::System => Scope::System,
        }
    }
}
#[derive(Clone, Copy, ValueEnum)]
pub enum Auth {
    Sudo,
    Polkit,
}
impl From<Auth> for Authorization {
    fn from(value: Auth) -> Self {
        match value {
            Auth::Sudo => Self::SudoNonInteractive,
            Auth::Polkit => Self::Polkit,
        }
    }
}
#[derive(Subcommand)]
pub enum Commands {
    /// List or manage package repositories.
    Repos {
        #[command(subcommand)]
        command: Option<RepoCommand>,
    },
    /// Check host readiness and backend availability.
    Doctor,
    /// List package sources with availability and capabilities.
    Sources,
    /// Search packages by name or description, best matches first.
    Search { query: String },
    /// Show details for one exact package identifier.
    Info { name: String },
    /// Resolve a command in PATH and show manager-reported ownership without running it.
    Inspect { command: String },
    /// Show installed copies and manager-reported residual configuration.
    Audit,
    /// List installed packages.
    List,
    /// Install packages by exact identifier.
    Install {
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Remove installed packages.
    Remove {
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Refresh source metadata without upgrading packages.
    Update,
    /// Upgrade named packages, or all available updates if no names are supplied.
    Upgrade {
        names: Vec<String>,
        /// Explicitly approve removals in an APT full upgrade.
        #[arg(long)]
        allow_removals: bool,
    },
    /// Find safe manager-native cleanup plans, or run selected plan keys.
    Clean {
        /// Cleanup keys shown by `pkd clean` (for example apt:autoremove).
        targets: Vec<String>,
        /// Run every discovered cleanup plan.
        #[arg(long)]
        all: bool,
    },
}
#[derive(Subcommand)]
pub enum RepoCommand {
    /// List configured repositories and installation scopes.
    List,
    /// Add a Flatpak repository from an HTTPS .flatpakrepo URL.
    Add { name: String, url: String },
    /// Remove a Flatpak remote without forcing removal of installed apps.
    Remove { name: String },
    /// Enable a Flatpak or firmware remote.
    Enable { name: String },
    /// Disable a Flatpak or firmware remote.
    Disable { name: String },
    /// Set Flatpak remote priority (0–9999; higher is preferred).
    Priority { name: String, priority: i32 },
    /// Open the native APT Software Sources editor.
    Edit,
}
impl Commands {
    fn writes(&self) -> bool {
        matches!(self, Self::Repos { command: Some(command) } if !matches!(command, RepoCommand::List))
            || matches!(
                self,
                Self::Install { .. } | Self::Remove { .. } | Self::Update | Self::Upgrade { .. }
            )
            || matches!(self, Self::Clean { targets, all, .. } if *all || !targets.is_empty())
    }
}
fn error_code(error: &EngineError) -> u8 {
    match error {
        EngineError::NotFound => 3,
        EngineError::Ambiguous(_) | EngineError::Incomplete(_) => 4,
        EngineError::Cancelled
        | EngineError::Execution(
            ExecutionError::Cancelled | ExecutionError::AuthorizationCancelled,
        ) => 7,
        EngineError::Execution(ExecutionError::AuthorizationDenied) => 5,
        EngineError::Execution(ExecutionError::LockBusy) => 6,
        _ => 1,
    }
}
fn failure(error: EngineError) -> (Value, u8) {
    let code = error_code(&error);
    (json!({"error": error, "message": error.to_string()}), code)
}
fn select(
    engine: &mut Engine,
    args: &Args,
    name: &str,
    installed: bool,
    cancel: &Cancellation,
) -> Result<PackageId, EngineError> {
    let report = if installed {
        engine.installed(cancel)
    } else {
        engine.search(name, cancel)
    };
    report.select(&Selector {
        name: name.into(),
        // A single --from pins the backend; several restrict the engine to
        // that set and leave ambiguity resolution to the selector.
        backend: match args.from.as_slice() {
            [one] => Some(one.clone()),
            _ => None,
        },
        architecture: args.arch.clone(),
        scope: args.scope.map(InstallScope::native),
    })
}

pub fn dispatch(
    engine: &mut Engine,
    args: &Args,
    cancel: &Cancellation,
    confirm: &mut dyn FnMut(&[Operation]) -> bool,
    events: &mut dyn FnMut(Event),
) -> (Value, u8) {
    let command = args.command.as_ref().expect("CLI command");
    if args.scope.is_some()
        && matches!(
            command,
            Commands::Update | Commands::Sources | Commands::Doctor | Commands::Clean { .. }
        )
    {
        return (
            json!({"error": "--scope applies to package queries and operations, not source or cleanup operations"}),
            2,
        );
    }
    match command {
        Commands::Repos { .. } => {
            return (
                json!({"error": "repository commands use the native repository service"}),
                2,
            )
        }
        Commands::Sources => {
            let sources = engine.discover(cancel);
            let code = if sources.iter().any(|s| s.availability.is_err()) {
                1
            } else {
                0
            };
            return (json!({"sources": sources}), code);
        }
        Commands::Search { query } => {
            let mut report = engine.search(query, cancel);
            report.packages.retain(|p| {
                !unverified_search_offer(p)
                    && args.scope.is_none_or(|scope| scope.native() == p.id.scope)
            });
            rank_search_matches(&mut report.packages, query);
            let code = if report.failures.is_empty() { 0 } else { 8 };
            return (json!(report), code);
        }
        Commands::List => {
            let mut report = engine.installed(cancel);
            report
                .packages
                .retain(|p| args.scope.is_none_or(|scope| scope.native() == p.id.scope));
            let code = if report.failures.is_empty() { 0 } else { 8 };
            return (json!(report), code);
        }
        Commands::Info { name } => {
            return match select(engine, args, name, false, cancel)
                .and_then(|id| engine.details(&id, cancel))
            {
                Ok(details) => (json!(details), 0),
                Err(e) => failure(e),
            }
        }
        Commands::Inspect { command } => {
            let mut inventory = engine.installed(cancel);
            inventory
                .packages
                .retain(|p| args.scope.is_none_or(|scope| scope.native() == p.id.scope));
            let code = if inventory.failures.is_empty() { 0 } else { 8 };
            return match inspect_native(&Host::current(), command, &inventory.packages, cancel) {
                Ok(report) => (
                    json!({"inspection": report, "failures": inventory.failures}),
                    code,
                ),
                Err(error) => failure(error.into()),
            };
        }
        Commands::Audit => {
            let mut inventory = engine.installed(cancel);
            inventory
                .packages
                .retain(|p| args.scope.is_none_or(|scope| scope.native() == p.id.scope));
            let code = if inventory.failures.is_empty() { 0 } else { 8 };
            return (
                json!({"audit": audit(&inventory.packages, native_leftovers(&Host::current())), "failures": inventory.failures}),
                code,
            );
        }
        Commands::Doctor => {
            return (
                json!({"error": "doctor is a text-only diagnostic; omit --json"}),
                2,
            )
        }
        Commands::Clean { targets, all } if targets.is_empty() && !all => {
            let report = engine.cleanup(cancel);
            let code = if report
                .failures
                .iter()
                .any(|failure| !matches!(failure.error, EngineError::Unsupported { .. }))
            {
                8
            } else {
                0
            };
            return (json!(report), code);
        }
        _ => (),
    }
    let planned = (|| -> Result<Vec<Operation>, EngineError> {
        match command {
            Commands::Update => engine
                .discover(cancel)
                .into_iter()
                .map(|source| match source.availability? {
                    Availability::Available => Ok(Operation::Refresh {
                        backend: source.backend,
                    }),
                    Availability::Unavailable(reason) => Err(EngineError::Unavailable {
                        backend: source.backend,
                        reason,
                    }),
                })
                .collect(),
            Commands::Upgrade { names, .. } if names.is_empty() => {
                let report = engine.installed(cancel);
                if !report.failures.is_empty() {
                    return Err(EngineError::Incomplete(report.failures));
                }
                if args.arch.is_some() || args.scope.is_some() {
                    return Ok(report
                        .packages
                        .into_iter()
                        .filter(|p| {
                            p.update == UpdateAvailability::Available
                                && args.arch.as_ref().is_none_or(|a| a == &p.id.architecture)
                                && args.scope.is_none_or(|scope| scope.native() == p.id.scope)
                        })
                        .map(|p| Operation::Upgrade(p.id))
                        .collect());
                }
                let mut operations = Vec::new();
                for package in report
                    .packages
                    .into_iter()
                    .filter(|p| p.update == UpdateAvailability::Available)
                {
                    let operation = if pkgdeck_core::backends::update_only(&package.id.backend) {
                        Operation::Upgrade(package.id)
                    } else {
                        Operation::UpgradeAll {
                            backend: package.id.backend,
                        }
                    };
                    if !operations.contains(&operation) {
                        operations.push(operation);
                    }
                }
                Ok(operations)
            }
            Commands::Install { names }
            | Commands::Remove { names }
            | Commands::Upgrade { names, .. } => {
                let mut operations = Vec::new();
                for name in names {
                    let id = select(
                        engine,
                        args,
                        name,
                        !matches!(command, Commands::Install { .. }),
                        cancel,
                    )?;
                    let operation = match command {
                        Commands::Install { .. } => Operation::Install(id),
                        Commands::Remove { .. } => Operation::Remove(id),
                        _ => Operation::Upgrade(id),
                    };
                    if !operations.contains(&operation) {
                        operations.push(operation);
                    }
                }
                Ok(operations)
            }
            Commands::Clean { targets, all } => {
                let report = engine.cleanup(cancel);
                let hard_failures: Vec<_> = report
                    .failures
                    .into_iter()
                    .filter(|failure| !matches!(failure.error, EngineError::Unsupported { .. }))
                    .collect();
                if !hard_failures.is_empty() {
                    return Err(EngineError::Incomplete(hard_failures));
                }
                let requested: std::collections::BTreeSet<_> = targets.iter().cloned().collect();
                let operations: Vec<_> = report
                    .items
                    .into_iter()
                    .filter(|item| {
                        *all || requested.contains(&format!("{}:{}", item.id.backend, item.id.key))
                    })
                    .map(|item| Operation::Clean(item.id))
                    .collect();
                if !*all && operations.len() != requested.len() {
                    return Err(EngineError::NotFound);
                }
                Ok(operations)
            }
            _ => unreachable!(),
        }
    })();
    let operations = match planned {
        Ok(ops) => ops,
        Err(e) => return failure(e),
    };
    if let Some(operation) = operations.iter().find(
        |operation| matches!(operation, Operation::UpgradeAll { backend } if backend == "apt"),
    ) {
        let plan = match engine.plan_apt_upgrade(cancel) {
            Ok(plan) => plan,
            Err(error) => return failure(error),
        };
        events(Event::Progress {
            operation: operation.clone(),
            progress: Progress::Message(format!("APT transaction:\n{}", plan.summary())),
        });
        if !plan.removals.is_empty()
            && !matches!(
                command,
                Commands::Upgrade {
                    allow_removals: true,
                    ..
                }
            )
        {
            return (
                json!({"error": "apt_removals_require_consent", "message": "APT would remove packages; review the plan and pass --allow-removals to approve them.", "plan": plan}),
                2,
            );
        }
    }
    for operation in &operations {
        if let Operation::Upgrade(id) = operation {
            if id.backend == "fwupd" {
                match engine.details(id, cancel) {
                    Ok(details) => events(Event::Progress {
                        operation: operation.clone(),
                        progress: Progress::Message(format!(
                            "{}: {}",
                            details.package.display_name, details.package.summary
                        )),
                    }),
                    Err(error) => return failure(error),
                }
            }
        }
    }
    if !operations.is_empty() && !args.yes && !confirm(&operations) {
        return (json!({"error": "confirmation_declined"}), 7);
    }
    let results = engine.execute_batch(&operations, cancel, events);
    let failed = results.iter().filter(|r| r.is_err()).count();
    let code = if failed == 0 {
        0
    } else if failed != results.len() {
        8
    } else {
        error_code(results.iter().find_map(|r| r.as_ref().err()).unwrap())
    };
    (
        json!({"operations": operations.into_iter().zip(results).map(|(operation, result)| json!({"operation":operation,"result":result})).collect::<Vec<_>>()}),
        code,
    )
}
fn emit(args: &Args, data: Value, code: u8) -> u8 {
    let text = if args.json {
        serde_json::to_string(&json!({"schema_version":1,"exit_code":code,"data":data}))
            .expect("serializable result")
    } else {
        let terminal = io::stdout().is_terminal();
        let width = if terminal {
            rustix::termios::tcgetwinsize(io::stdout()).map_or(100, |size| usize::from(size.ws_col))
        } else {
            100
        };
        let color = terminal
            && std::env::var_os("NO_COLOR").is_none()
            && std::env::var("TERM").is_ok_and(|term| term != "dumb");
        crate::presentation::human(&data, width, color)
    };
    if writeln!(io::stdout().lock(), "{text}").is_err() {
        return 1;
    }
    code
}
fn repository_command(
    args: &Args,
    command: &Option<RepoCommand>,
    cancel: &Cancellation,
) -> (Value, u8) {
    use pkgdeck_core::{backends::NativeTransport, host::Host};
    let host = Host::current();
    if let Some(reason) = host.runtime.disabled_reason() {
        return (json!({"error": reason}), 1);
    }
    let transport = NativeTransport {
        host,
        authorization: args.auth.into(),
    };
    repository_dispatch(
        &transport,
        std::path::Path::new("/"),
        args,
        command,
        cancel,
        &mut |label| {
            eprintln!("{}", crate::presentation::clean(label));
            eprint!("Apply repository change? [y/N] ");
            let _ = io::stderr().flush();
            let mut answer = String::new();
            io::stdin().read_line(&mut answer).is_ok() && matches!(answer.trim(), "y" | "Y" | "yes")
        },
    )
}
fn repository_dispatch(
    transport: &impl pkgdeck_core::backends::Transport,
    root: &std::path::Path,
    args: &Args,
    command: &Option<RepoCommand>,
    cancel: &Cancellation,
    confirm: &mut dyn FnMut(&str) -> bool,
) -> (Value, u8) {
    use pkgdeck_core::repositories::{self, Action, Change};
    if matches!(command, None | Some(RepoCommand::List)) {
        let report = repositories::list_selected(
            transport,
            root,
            cancel,
            &args.from,
            args.scope.map(InstallScope::native).as_ref(),
        );
        let code = if report.errors.is_empty() { 0 } else { 8 };
        return (json!(report), code);
    }
    let [backend] = args.from.as_slice() else {
        return (
            json!({"error": "select one repository manager with --from"}),
            2,
        );
    };
    let (name, change) = match command.as_ref().unwrap() {
        RepoCommand::Add { name, url } => (name.clone(), Change::Add { url: url.clone() }),
        RepoCommand::Remove { name } => (name.clone(), Change::Remove),
        RepoCommand::Enable { name } => (name.clone(), Change::SetEnabled { enabled: true }),
        RepoCommand::Disable { name } => (name.clone(), Change::SetEnabled { enabled: false }),
        RepoCommand::Priority { name, priority } => (
            name.clone(),
            Change::SetPriority {
                priority: *priority,
            },
        ),
        RepoCommand::Edit => ("sources".into(), Change::OpenEditor),
        RepoCommand::List => unreachable!(),
    };
    let scope = args.scope.map(InstallScope::native).unwrap_or_else(|| {
        if backend == "flatpak" {
            InstallScope::User.native()
        } else {
            Scope::System
        }
    });
    let action = Action {
        backend: backend.clone(),
        name,
        scope,
        change,
    };
    if let Err(error) = action.validate() {
        return failure(error);
    }
    if !args.yes && !confirm(&action.label()) {
        return (json!({"error": "confirmation_declined"}), 7);
    }
    match repositories::apply(transport, &action, cancel) {
        Ok(()) => (json!({"message": "Repository operation completed"}), 0),
        Err(error) => failure(error),
    }
}
pub fn run(args: &Args) -> u8 {
    if args.command.as_ref().is_some_and(Commands::writes)
        && !args.yes
        && (args.json || !io::stdin().is_terminal() || !io::stdout().is_terminal())
    {
        return emit(
            args,
            json!({"error":"confirmation_required","message":"package changes require --yes in JSON or non-interactive mode"}),
            2,
        );
    }
    let cancel = Cancellation::default();
    let signal = match signal_hook::flag::register(signal_hook::consts::SIGINT, cancel.flag()) {
        Ok(id) => id,
        Err(e) => return emit(args, json!({"error":e.to_string()}), 1),
    };
    if let Some(Commands::Repos { command }) = &args.command {
        let (data, code) = repository_command(args, command, &cancel);
        signal_hook::low_level::unregister(signal);
        return emit(args, data, code);
    }
    let mut engine = match pkgdeck_core::backends::native_engine(
        &args.from,
        matches!(args.command, Some(Commands::Sources)),
        args.auth.into(),
        &cancel,
    ) {
        Ok(engine) => engine,
        Err(e) => {
            signal_hook::low_level::unregister(signal);
            let (data, code) = failure(e);
            return emit(args, data, code);
        }
    };
    let (data, code) = dispatch(
        &mut engine,
        args,
        &cancel,
        &mut |operations| {
            for operation in operations {
                eprintln!("{}", crate::presentation::operation(operation));
            }
            eprint!("Approve these operations and native dependency changes? [y/N] ");
            let _ = io::stderr().flush();
            let mut answer = String::new();
            io::stdin().read_line(&mut answer).is_ok() && matches!(answer.trim(), "y" | "Y" | "yes")
        },
        &mut |event| {
            if let Event::Progress {
                progress: Progress::Message(message),
                ..
            } = event
            {
                eprintln!("{}", crate::presentation::clean(&message));
            }
        },
    );
    signal_hook::low_level::unregister(signal);
    emit(args, data, code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pkgdeck_core::process::Completion;
    struct RepoFixture;
    impl pkgdeck_core::backends::Transport for RepoFixture {
        fn apt_query(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &Cancellation,
        ) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
        fn apt_write(
            &self,
            _: pkgdeck_core::host::AptAction,
            _: &Cancellation,
        ) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
        fn brew(
            &self,
            _: &[std::ffi::OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
        fn flatpak(
            &self,
            _: &[std::ffi::OsString],
            _: &Cancellation,
            write: bool,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            Ok(Completion {
                code: Some(0),
                signal: None,
                stdout: if write {
                    vec![]
                } else {
                    b"fixture\tFixture\thttps://example.invalid\t1\t\n".to_vec()
                },
                stderr: vec![],
                truncated: false,
                cancellation_deferred: false,
            })
        }
    }
    #[test]
    fn repository_cli_scopes_validation_and_confirmation() {
        let root = std::env::temp_dir().join(format!("pkgdeck-repos-cli-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let invoke = |words: &[&str], approve: bool| {
            let args =
                Args::try_parse_from(std::iter::once("pkd").chain(words.iter().copied())).unwrap();
            let Some(Commands::Repos { command }) = &args.command else {
                panic!()
            };
            repository_dispatch(
                &RepoFixture,
                &root,
                &args,
                command,
                &Cancellation::default(),
                &mut |_| approve,
            )
        };
        let (data, code) = invoke(&["repos"], false);
        assert_eq!(code, 0);
        assert_eq!(data["repositories"].as_array().unwrap().len(), 2);
        let (data, code) = invoke(
            &["--scope", "system", "--from", "flatpak", "repos", "list"],
            false,
        );
        assert_eq!(code, 0);
        assert_eq!(data["repositories"].as_array().unwrap().len(), 1);
        assert_eq!(data["repositories"][0]["scope"], "system");
        for words in [
            vec!["add", "fixture", "https://example.invalid/repo.flatpakrepo"],
            vec!["remove", "fixture"],
            vec!["enable", "fixture"],
            vec!["disable", "fixture"],
            vec!["priority", "fixture", "2"],
        ] {
            let mut args = vec!["--from", "flatpak", "repos"];
            args.extend(words);
            assert_eq!(invoke(&args, false).1, 7);
            assert_eq!(invoke(&args, true).1, 0);
        }
        assert_eq!(invoke(&["repos", "remove", "fixture"], true).1, 2);
        assert_ne!(
            invoke(
                &["--from", "flatpak", "repos", "priority", "fixture", "10000"],
                true
            )
            .1,
            0
        );
        assert_ne!(invoke(&["--from", "apt", "repos", "edit"], true).1, 0);
        assert_ne!(
            invoke(&["--from", "fwupd", "repos", "enable", "fixture"], true).1,
            0
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn standalone_update_all_uses_exact_installations() {
        for tool in pkgdeck_core::backends::StandaloneTool::ALL {
            let mut engine = Engine::default();
            engine
                .register(Fixture {
                    backend: tool.id().into(),
                    installed: true,
                    fail: None,
                    read_failure: None,
                    verified: true,
                })
                .unwrap();
            let args = Args::try_parse_from(["pkd", "--from", tool.id(), "upgrade"]).unwrap();
            let (_, code) = dispatch(
                &mut engine,
                &args,
                &Cancellation::default(),
                &mut |operations| {
                    assert_eq!(operations.len(), 1);
                    assert!(
                        matches!(&operations[0], Operation::Upgrade(id) if id.backend == tool.id())
                    );
                    false
                },
                &mut |_| {},
            );
            assert_eq!(code, 7);
        }
    }
    #[test]
    fn mixed_firmware_update_plan_pins_device_and_shows_requirements() {
        let mut engine = engine();
        engine
            .register(Fixture {
                backend: "fwupd".into(),
                installed: true,
                fail: None,
                read_failure: None,
                verified: true,
            })
            .unwrap();
        let args = Args::try_parse_from(["pkd", "upgrade"]).unwrap();
        let mut messages = vec![];
        let (data, code) = dispatch(
            &mut engine,
            &args,
            &Cancellation::default(),
            &mut |operations| {
                assert!(operations
                    .iter()
                    .any(|op| matches!(op, Operation::Upgrade(id) if id.backend == "fwupd")));
                assert!(operations
                    .iter()
                    .any(|op| matches!(op, Operation::UpgradeAll { backend } if backend == "apt")));
                true
            },
            &mut |event| {
                if let Event::Progress {
                    progress: Progress::Message(message),
                    ..
                } = event
                {
                    messages.push(message);
                }
            },
        );
        assert_eq!(code, 0, "{data}");
        assert!(messages
            .iter()
            .any(|message| message.contains("Fixture: Synthetic")));
    }
    #[test]
    fn named_package_scope_is_honored() {
        let cancel = Cancellation::default();
        let mut engine = engine();
        let system = Args::try_parse_from(["pkd", "--scope", "system", "info", "fixture"]).unwrap();
        assert_eq!(
            select(&mut engine, &system, "fixture", false, &cancel)
                .unwrap()
                .scope,
            Scope::System
        );
        let user = Args::try_parse_from(["pkd", "--scope", "user", "info", "fixture"]).unwrap();
        assert!(select(&mut engine, &user, "fixture", false, &cancel).is_err());
        assert!(Args::try_parse_from(["pkd", "--scope", "invalid", "info", "fixture"]).is_err());
        assert_eq!(
            call(&mut engine, &["--scope", "user", "upgrade"], true).0["operations"],
            json!([])
        );
        assert_eq!(call(&mut engine, &["--scope", "user", "update"], true).1, 2);
    }
    struct Fixture {
        backend: String,
        installed: bool,
        fail: Option<EngineError>,
        read_failure: Option<EngineError>,
        verified: bool,
    }
    impl Fixture {
        fn package(&self) -> Package {
            Package {
                id: PackageId {
                    backend: self.backend.clone(),
                    name: "fixture".into(),
                    architecture: "all".into(),
                    scope: Scope::System,
                    remote: None,
                    reference: None,
                },
                display_name: "Fixture".into(),
                summary: "Synthetic".into(),
                installed_version: self.installed.then(|| "1.0".into()),
                candidate_version: self.verified.then(|| "2.0".into()),
                update: UpdateAvailability::Available,
                icon: None,
                component_ids: vec![],
                homepages: vec![],
            }
        }
    }
    impl Backend for Fixture {
        fn id(&self) -> &str {
            &self.backend
        }
        fn capabilities(&self) -> &[Capability] {
            &[
                Capability::Search,
                Capability::Details,
                Capability::Installed,
                Capability::Install,
                Capability::Remove,
                Capability::Refresh,
                Capability::Upgrade,
                Capability::Clean,
            ]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn search(&mut self, _: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
            if let Some(error) = &self.read_failure {
                return Err(error.clone());
            }
            Ok(vec![self.package()])
        }
        fn installed(&mut self, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
            if let Some(error) = &self.read_failure {
                return Err(error.clone());
            }
            Ok(if self.installed {
                vec![self.package()]
            } else {
                vec![]
            })
        }
        fn details(
            &mut self,
            _: &PackageId,
            _: &Cancellation,
        ) -> Result<PackageDetails, EngineError> {
            Ok(PackageDetails {
                package: self.package(),
                description: "Synthetic".into(),
                homepage: None,
                dependencies: vec![],
            })
        }
        fn cleanup(&mut self, _: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
            if let Some(error) = &self.read_failure {
                return Err(error.clone());
            }
            Ok(vec![CleanupItem {
                id: CleanupId {
                    backend: self.backend.clone(),
                    key: "orphans".into(),
                },
                kind: CleanupKind::OrphanDependencies,
                title: "Synthetic cleanup".into(),
                summary: "One synthetic dependency".into(),
                preview: "synthetic-runtime".into(),
            }])
        }
        fn apt_upgrade_plan(&mut self, _: &Cancellation) -> Result<AptUpgradePlan, EngineError> {
            Ok(AptUpgradePlan {
                preview: "Inst fixture [1.0] (2.0 synthetic)".into(),
                upgrades: vec!["fixture".into()],
                installs: vec![],
                removals: if self.verified {
                    vec![]
                } else {
                    vec!["old-fixture".into()]
                },
            })
        }
        fn execute(
            &mut self,
            op: &Operation,
            _: &Cancellation,
            progress: &mut dyn FnMut(Progress),
        ) -> Result<OperationOutcome, EngineError> {
            if let Some(e) = &self.fail {
                return Err(e.clone());
            }
            self.installed = !matches!(op, Operation::Remove(_));
            progress(Progress::Message("synthetic write".into()));
            Ok(OperationOutcome::default())
        }
    }
    fn engine() -> Engine {
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                read_failure: None,
                verified: true,
                backend: "apt".into(),
                installed: true,
                fail: None,
            })
            .unwrap();
        engine
    }
    fn call(engine: &mut Engine, argv: &[&str], approve: bool) -> (Value, u8) {
        let args = Args::parse_from(std::iter::once("pkd").chain(argv.iter().copied()));
        dispatch(
            engine,
            &args,
            &Cancellation::default(),
            &mut |_| approve,
            &mut |_| {},
        )
    }
    #[test]
    fn commands_and_confirmation() {
        assert!(Args::try_parse_from(["pkd", "clean", "--authenticate"]).is_err());
        let mut engine = engine();
        for args in [
            vec!["sources"],
            vec!["search", "fixture"],
            vec!["list"],
            vec!["info", "fixture"],
            vec!["install", "fixture", "fixture"],
            vec!["upgrade", "fixture"],
            vec!["upgrade"],
            vec!["update"],
            vec!["remove", "fixture"],
            vec!["clean"],
            vec!["clean", "apt:orphans"],
        ] {
            assert_eq!(call(&mut engine, &args, true).1, 0, "{args:?}");
        }
        assert_eq!(call(&mut engine, &["install", "fixture"], false).1, 7);
        assert_eq!(call(&mut engine, &["clean", "apt:orphans"], false).1, 7);
        assert_eq!(call(&mut engine, &["clean", "missing:plan"], true).1, 3);
        let cleaned = call(&mut engine, &["clean", "--all"], true);
        assert_eq!(cleaned.1, 0);
        assert_eq!(
            cleaned.0["operations"][0]["operation"],
            json!({"clean":{"backend":"apt","key":"orphans"}})
        );
        assert_eq!(call(&mut engine, &["--scope", "user", "clean"], true).1, 2);
        assert_eq!(
            call(&mut engine, &["--yes", "install", "fixture"], false).1,
            0
        );
        assert_eq!(call(&mut engine, &["doctor"], false).1, 2);
        assert_eq!(call(&mut engine, &["info", "missing"], false).1, 3);
        assert_eq!(call(&mut engine, &["install", "missing"], true).1, 3);
        assert_eq!(
            call(&mut engine, &["--arch", "foreign", "upgrade"], true).0["operations"],
            json!([])
        );
        assert_eq!(
            call(&mut engine, &["upgrade"], true).0["operations"][0]["operation"],
            json!({"upgrade_all":{"backend":"apt"}})
        );
        let args = Args::parse_from(["pkd", "--auth", "polkit", "sources"]);
        assert!(matches!(
            Authorization::from(args.auth),
            Authorization::Polkit
        ));
        assert!(matches!(
            Authorization::from(Auth::Sudo),
            Authorization::SudoNonInteractive
        ));

        let mut failed_discovery = Engine::default();
        failed_discovery
            .register(Fixture {
                read_failure: Some(ExecutionError::TimedOut.into()),
                verified: true,
                backend: "apt".into(),
                installed: true,
                fail: None,
            })
            .unwrap();
        assert_eq!(call(&mut failed_discovery, &["clean"], true).1, 8);
        assert_ne!(call(&mut failed_discovery, &["clean", "--all"], true).1, 0);

        let mut failed_write = Engine::default();
        failed_write
            .register(Fixture {
                read_failure: None,
                verified: true,
                backend: "apt".into(),
                installed: true,
                fail: Some(ExecutionError::LockBusy.into()),
            })
            .unwrap();
        assert_ne!(call(&mut failed_write, &["clean", "--all"], true).1, 0);
    }
    #[test]
    fn inspection_commands_are_read_only_and_preserve_report_shapes() {
        let mut engine = engine();
        let (inspected, code) = call(&mut engine, &["inspect", "pkgdeck-fixture-missing"], false);
        assert_eq!(code, 0);
        assert_eq!(
            inspected["inspection"]["command"],
            "pkgdeck-fixture-missing"
        );
        assert!(inspected["inspection"]["resolved"].is_null());
        let (invalid, code) = call(&mut engine, &["inspect", "../outside"], false);
        assert_eq!(code, 1);
        assert!(invalid["error"].is_object());
        let (audited, code) = call(&mut engine, &["audit"], false);
        assert_eq!(code, 0);
        assert!(audited["audit"]["installed_copies"].is_array());
        assert_eq!(
            audited["audit"]["installed_copies"][0]["package"]["backend"],
            "apt"
        );
    }
    #[test]
    fn apt_removals_require_separate_cli_consent() {
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                backend: "apt".into(),
                installed: true,
                fail: None,
                read_failure: None,
                verified: false,
            })
            .unwrap();
        let (blocked, code) = call(&mut engine, &["--yes", "upgrade"], true);
        assert_eq!(code, 2);
        assert_eq!(blocked["error"], "apt_removals_require_consent");
        assert_eq!(blocked["plan"]["removals"], json!(["old-fixture"]));
        assert_eq!(
            call(&mut engine, &["--yes", "upgrade", "--allow-removals"], true).1,
            0
        );
    }
    #[test]
    fn source_ambiguity_partial_results_and_exit_codes() {
        let mut engine = engine();
        engine
            .register(Fixture {
                read_failure: None,
                verified: true,
                backend: "homebrew".into(),
                installed: true,
                fail: Some(ExecutionError::AuthorizationDenied.into()),
            })
            .unwrap();
        assert_eq!(call(&mut engine, &["install", "fixture"], true).1, 4);
        assert_eq!(
            call(&mut engine, &["--from", "apt", "install", "fixture"], true).1,
            0
        );
        assert_eq!(call(&mut engine, &["update"], true).1, 8);
        assert_eq!(
            call(
                &mut engine,
                &["--from", "homebrew", "install", "fixture"],
                true
            )
            .1,
            5
        );
        for (error, code) in [
            (EngineError::Cancelled, 7),
            (ExecutionError::AuthorizationCancelled.into(), 7),
            (ExecutionError::LockBusy.into(), 6),
            (ExecutionError::TimedOut.into(), 1),
            (EngineError::Incomplete(vec![]), 4),
        ] {
            assert_eq!(error_code(&error), code);
        }
    }

    #[test]
    fn search_hides_guesses_preserves_partial_results_and_keeps_explicit_install() {
        let mut engine = engine();
        engine
            .register(Fixture {
                backend: "cargo".into(),
                installed: false,
                fail: None,
                read_failure: None,
                verified: false,
            })
            .unwrap();
        let (report, code) = call(&mut engine, &["search", "fixture"], false);
        assert_eq!(code, 0);
        assert_eq!(report["packages"].as_array().unwrap().len(), 1);
        assert_eq!(report["packages"][0]["id"]["backend"], "apt");
        assert_eq!(
            call(
                &mut engine,
                &["--from", "cargo", "install", "fixture"],
                true
            )
            .1,
            0
        );
        engine
            .register(Fixture {
                backend: "flatpak".into(),
                installed: false,
                fail: None,
                read_failure: Some(ExecutionError::TimedOut.into()),
                verified: true,
            })
            .unwrap();
        let (report, code) = call(&mut engine, &["search", "fixture"], false);
        assert_eq!(code, 8);
        assert_eq!(report["packages"].as_array().unwrap().len(), 2);
        assert_eq!(report["failures"].as_array().unwrap().len(), 1);
        assert_eq!(report["failures"][0]["backend"], "flatpak");
        assert_eq!(call(&mut engine, &["upgrade"], true).1, 4);
    }

    #[test]
    fn upgrade_all_handles_empty_and_incomplete_installed_reports() {
        let mut empty = Engine::default();
        empty
            .register(Fixture {
                read_failure: None,
                verified: true,
                backend: "apt".into(),
                installed: false,
                fail: None,
            })
            .unwrap();
        assert_eq!(
            call(&mut empty, &["upgrade"], true).0["operations"],
            json!([])
        );
        let mut failed = Engine::default();
        failed
            .register(Fixture {
                read_failure: None,
                verified: true,
                backend: "apt".into(),
                installed: true,
                fail: Some(ExecutionError::TimedOut.into()),
            })
            .unwrap();
        assert_eq!(call(&mut failed, &["upgrade"], true).1, 1);
    }
}
