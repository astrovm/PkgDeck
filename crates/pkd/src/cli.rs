use clap::{Parser, Subcommand, ValueEnum};
use pkgdeck_core::{
    engine::*,
    host::Authorization,
    package::*,
    process::{Cancellation, ExecutionError},
};
use serde_json::{json, Value};
use std::io::{self, IsTerminal, Write};

#[derive(Parser)]
#[command(version = pkgdeck_core::VERSION, about = "PkgDeck package manager")]
pub struct Args {
    #[arg(long, global = true)]
    pub json: bool,
    /// Restrict operations to one source.
    #[arg(long, global = true, value_parser = ["apt", "dnf", "pacman", "zypper", "snap", "homebrew", "appimage", "flatpak"])]
    pub from: Option<String>,
    #[arg(long, global = true)]
    pub arch: Option<String>,
    /// Approve native package and dependency changes without prompting.
    #[arg(long, short = 'y', global = true)]
    pub yes: bool,
    #[arg(long, global = true, value_enum, default_value = "sudo")]
    pub auth: Auth,
    #[command(subcommand)]
    pub command: Option<Commands>,
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
    Doctor,
    Sources,
    Search {
        query: String,
    },
    Info {
        name: String,
    },
    List,
    Install {
        #[arg(required = true)]
        names: Vec<String>,
    },
    Remove {
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Refresh source metadata without upgrading packages.
    Update,
    /// Upgrade named packages, or all available updates if no names are supplied.
    Upgrade {
        names: Vec<String>,
    },
}
impl Commands {
    fn writes(&self) -> bool {
        matches!(
            self,
            Self::Install { .. } | Self::Remove { .. } | Self::Update | Self::Upgrade { .. }
        )
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
        backend: args.from.clone(),
        architecture: args.arch.clone(),
        scope: None,
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
    match command {
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
            let report = engine.search(query, cancel);
            let code = if report.failures.is_empty() { 0 } else { 8 };
            return (json!(report), code);
        }
        Commands::List => {
            let report = engine.installed(cancel);
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
        Commands::Doctor => {
            return (
                json!({"error": "doctor is a text-only diagnostic; omit --json"}),
                2,
            )
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
            Commands::Upgrade { names } if names.is_empty() => {
                let report = engine.installed(cancel);
                if !report.failures.is_empty() {
                    return Err(EngineError::Incomplete(report.failures));
                }
                if args.arch.is_some() {
                    return Ok(report
                        .packages
                        .into_iter()
                        .filter(|p| {
                            p.update == UpdateAvailability::Available
                                && args.arch.as_ref().is_some_and(|a| a == &p.id.architecture)
                        })
                        .map(|p| Operation::Upgrade(p.id))
                        .collect());
                }
                Ok(report
                    .packages
                    .into_iter()
                    .filter(|p| p.update == UpdateAvailability::Available)
                    .map(|p| p.id.backend)
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .map(|backend| Operation::UpgradeAll { backend })
                    .collect())
            }
            Commands::Install { names }
            | Commands::Remove { names }
            | Commands::Upgrade { names } => {
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
            _ => unreachable!(),
        }
    })();
    let operations = match planned {
        Ok(ops) => ops,
        Err(e) => return failure(e),
    };
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
            crossterm::terminal::size().map_or(100, |(w, _)| usize::from(w))
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
    let mut engine = match pkgdeck_core::backends::native_engine(
        args.from.as_deref(),
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
    struct Fixture {
        backend: String,
        installed: bool,
        fail: Option<EngineError>,
    }
    impl Fixture {
        fn package(&self) -> Package {
            Package {
                id: PackageId {
                    backend: self.backend.clone(),
                    name: "fixture".into(),
                    architecture: "all".into(),
                    scope: Scope::System,
                },
                display_name: "Fixture".into(),
                summary: "Synthetic".into(),
                installed_version: self.installed.then(|| "1.0".into()),
                candidate_version: Some("2.0".into()),
                update: UpdateAvailability::Available,
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
            ]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn search(&mut self, _: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
            Ok(vec![self.package()])
        }
        fn installed(&mut self, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
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
        ] {
            assert_eq!(call(&mut engine, &args, true).1, 0, "{args:?}");
        }
        assert_eq!(call(&mut engine, &["install", "fixture"], false).1, 7);
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
    }
    #[test]
    fn source_ambiguity_partial_results_and_exit_codes() {
        let mut engine = engine();
        engine
            .register(Fixture {
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
    fn upgrade_all_handles_empty_and_incomplete_installed_reports() {
        let mut empty = Engine::default();
        empty
            .register(Fixture {
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
                backend: "apt".into(),
                installed: true,
                fail: Some(ExecutionError::TimedOut.into()),
            })
            .unwrap();
        assert_eq!(call(&mut failed, &["upgrade"], true).1, 1);
    }
}
