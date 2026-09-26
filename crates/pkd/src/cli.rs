use crate::live::Live;
use crate::session::Session;
use clap::{Parser, Subcommand, ValueEnum};
use pkgdeck_core::{
    activity::{History, Outcome, State},
    engine::*,
    host::{Authorization, Host},
    inspection::{audit, inspect_native, native_leftovers},
    manifest::{self, PreviewStatus},
    package::*,
    process::{Cancellation, ExecutionError},
};
use serde_json::{json, Value};
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version = pkgdeck_core::VERSION,
    about = "Search, install, update, and clean up packages from every package manager",
    after_help = EXAMPLES
)]
pub struct Args {
    /// Print machine-readable JSON instead of text.
    #[arg(long, global = true)]
    pub json: bool,
    /// Only use this source, such as apt or flatpak. Repeat to pick
    /// several; omit to use every available source. `pkd sources` lists
    /// them.
    #[arg(long, global = true, value_name = "SOURCE", hide_possible_values = true, value_parser = ["fwupd", "apt", "dnf", "pacman", "zypper", "snap", "homebrew", "homebrew-cask", "appimage", "flatpak", "docker", "podman", "cargo", "npm", "pnpm", "bun", "pip", "pipx", "uv", "composer", "gem", "codex", "claude", "grok", "opencode"])]
    pub from: Vec<String>,
    /// Pick a package architecture when the same name exists for several.
    #[arg(long, global = true)]
    pub arch: Option<String>,
    /// Pick the user or system installation.
    #[arg(long, global = true, value_enum)]
    pub scope: Option<InstallScope>,
    /// Approve changes without asking.
    #[arg(long, short = 'y', global = true)]
    pub yes: bool,
    /// How to get permission for system changes: an existing sudo login, or the desktop polkit prompt.
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
const EXAMPLES: &str = "\
Examples:
  pkd search vlc                  Find packages in every source
  pkd info cowsay                 Show one package
  pkd install cowsay              Install by exact name
  pkd install --from flatpak org.videolan.VLC
  pkd refresh && pkd upgrade      Refresh package lists, then update
  pkd list --from npm             List what one source installed
  pkd completions bash > ~/.local/share/bash-completion/completions/pkd";
#[derive(Subcommand)]
pub enum Commands {
    /// Search packages by name or description. Best matches come first.
    Search { query: String },
    /// Show details for one package, by exact name.
    Info { name: String },
    /// Install packages, by exact name.
    Install {
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Remove installed packages.
    Remove {
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Update the named packages, or everything if no names are given.
    Upgrade {
        names: Vec<String>,
        /// Allow an APT full upgrade to remove packages.
        #[arg(long)]
        allow_removals: bool,
    },
    /// Refresh package lists. Does not install updates; run `pkd upgrade` next.
    #[command(visible_alias = "update")]
    Refresh,
    /// List installed packages.
    List,
    /// List cleanup tasks, or run the ones you name.
    Clean {
        /// Task keys shown by `pkd clean`, such as apt:autoremove.
        targets: Vec<String>,
        /// Run every cleanup task.
        #[arg(long)]
        all: bool,
    },
    /// List package managers, whether they are available, and what they support.
    Sources,
    /// Check that PkgDeck can find and run your package managers.
    Doctor,
    /// List or manage repositories.
    Repos {
        #[command(subcommand)]
        command: Option<RepoCommand>,
    },
    /// Show which file runs for a command and which package installed it. Never runs the command.
    Inspect { command: String },
    /// Find apps installed more than once, and config files left behind by removed packages.
    Audit,
    /// Save your installed software to a file, or check a saved list on this machine.
    Inventory {
        #[command(subcommand)]
        command: InventoryCommand,
    },
    /// Print a shell completion script, for bash, zsh, or fish.
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}
/// Shells `pkd completions` can write a script for.
#[derive(Clone, Copy, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}
impl From<Shell> for clap_complete::Shell {
    fn from(shell: Shell) -> Self {
        match shell {
            Shell::Bash => Self::Bash,
            Shell::Zsh => Self::Zsh,
            Shell::Fish => Self::Fish,
        }
    }
}
/// Writes the completion script for `shell` to `out`.
fn completions(shell: Shell, out: &mut dyn Write) {
    use clap::CommandFactory;
    clap_complete::generate(
        clap_complete::Shell::from(shell),
        &mut Args::command(),
        "pkd",
        out,
    );
}
#[derive(Subcommand)]
pub enum InventoryCommand {
    /// Save the named installed packages, or all of them if no names are given.
    Export { path: PathBuf, names: Vec<String> },
    /// Check a saved list against this machine. Changes nothing.
    Preview { path: PathBuf },
}
#[derive(Subcommand)]
pub enum RepoCommand {
    /// List repositories.
    List,
    /// Add a Flatpak repository from an HTTPS .flatpakrepo URL.
    Add { name: String, url: String },
    /// Remove a Flatpak repository. Apps installed from it are kept.
    Remove { name: String },
    /// Enable a Flatpak or firmware repository.
    Enable { name: String },
    /// Disable a Flatpak or firmware repository.
    Disable { name: String },
    /// Set a Flatpak repository priority (0 to 9999, higher wins).
    Priority { name: String, priority: i32 },
    /// Open the Software Sources editor for APT.
    Edit,
}
impl Commands {
    /// Commands that change installed software and so need approval.
    /// Refreshing package lists changes nothing you would review, so it runs
    /// without asking, like the app's background refresh.
    fn writes(&self) -> bool {
        matches!(self, Self::Repos { command: Some(command) } if !matches!(command, RepoCommand::List))
            || matches!(
                self,
                Self::Install { .. } | Self::Remove { .. } | Self::Upgrade { .. }
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
    let message = crate::presentation::error_message(&error);
    (json!({"error": error, "message": message}), code)
}
/// What a typed package name is looked up for.
#[derive(Clone, Copy, PartialEq)]
enum Lookup {
    /// `info`: only packages a source confirmed have details.
    Details,
    /// `install`: an unverified registry offer counts when it is the only one.
    Install,
    /// `remove` and `upgrade NAME`: installed packages only.
    Installed,
}
/// The package a typed name picks. `offers` are registry sources that
/// could also try the name but did not confirm it exists; they never make
/// a confirmed match ambiguous. On `NotFound`, `offers` says where the user
/// could still try.
struct Selection {
    result: Result<PackageId, EngineError>,
    offers: Vec<String>,
}
fn select(
    engine: &mut Engine,
    args: &Args,
    name: &str,
    lookup: Lookup,
    cancel: &Cancellation,
) -> Selection {
    let report = if lookup == Lookup::Installed {
        engine.installed(cancel)
    } else {
        engine.lookup(name, cancel)
    };
    let selector = Selector {
        name: name.into(),
        // A single --from pins the backend; several restrict the engine to
        // that set and leave ambiguity resolution to the selector.
        backend: match args.from.as_slice() {
            [one] => Some(one.clone()),
            _ => None,
        },
        architecture: args.arch.clone(),
        scope: args.scope.map(InstallScope::native),
    };
    let result = report.select_confirmed(&selector, lookup == Lookup::Install);
    let offers = if lookup == Lookup::Install && selector.backend.is_none() {
        let chosen = result.as_ref().ok().map(|id| id.backend.as_str());
        report
            .offer_sources(&selector)
            .into_iter()
            .filter(|backend| Some(backend.as_str()) != chosen)
            .collect()
    } else {
        vec![]
    };
    Selection { result, offers }
}
/// "npm, pipx" → "--from npm or --from pipx".
fn from_flags(sources: &[String]) -> String {
    let flags: Vec<_> = sources.iter().map(|s| format!("--from {s}")).collect();
    match flags.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} or {b}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}
/// "npm, Cargo, and pipx", by display name.
fn source_list(sources: &[String]) -> String {
    let names: Vec<_> = sources
        .iter()
        .map(|s| pkgdeck_core::backends::display_name(s).to_string())
        .collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
pub fn dispatch(
    engine: &mut Engine,
    args: &Args,
    cancel: &Cancellation,
    confirm: &mut dyn FnMut(&[Operation]) -> bool,
    events: &mut dyn FnMut(Event),
) -> (Value, u8) {
    dispatch_with(engine, args, cancel, confirm, &mut |_| Ok(()), events)
}
/// `authorize` runs once approved changes are about to start, so a password
/// prompt comes after the review and never before it.
pub fn dispatch_with(
    engine: &mut Engine,
    args: &Args,
    cancel: &Cancellation,
    confirm: &mut dyn FnMut(&[Operation]) -> bool,
    authorize: &mut dyn FnMut(&[Operation]) -> Result<(), EngineError>,
    events: &mut dyn FnMut(Event),
) -> (Value, u8) {
    let command = args.command.as_ref().expect("CLI command");
    if args.scope.is_some()
        && matches!(
            command,
            Commands::Refresh
                | Commands::Sources
                | Commands::Doctor
                | Commands::Clean { .. }
                | Commands::Inventory {
                    command: InventoryCommand::Preview { .. }
                }
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
        Commands::Inventory { command } => {
            return match command {
                InventoryCommand::Export { path, names } => {
                    let mut report = engine.installed(cancel);
                    if !report.failures.is_empty() {
                        return failure(EngineError::Incomplete(report.failures));
                    }
                    report.packages.retain(|package| {
                        args.scope
                            .is_none_or(|scope| scope.native() == package.id.scope)
                            && args
                                .arch
                                .as_ref()
                                .is_none_or(|arch| arch == &package.id.architecture)
                    });
                    let selected: Result<Vec<_>, _> = names
                        .iter()
                        .map(|name| {
                            report.select(&Selector {
                                name: name.clone(),
                                backend: match args.from.as_slice() {
                                    [one] => Some(one.clone()),
                                    _ => None,
                                },
                                architecture: args.arch.clone(),
                                scope: args.scope.map(InstallScope::native),
                            })
                        })
                        .collect();
                    let selected = match selected {
                        Ok(selected) => selected,
                        Err(error) => return failure(error),
                    };
                    match manifest::export(&report.packages, &selected).and_then(|document| {
                        manifest::write_new(path, &document)?;
                        Ok(document.packages.len())
                    }) {
                        Ok(count) => (json!({"manifest_export":{"path":path,"packages":count}}), 0),
                        Err(error) => (json!({"error":error.to_string()}), 1),
                    }
                }
                InventoryCommand::Preview { path } => match manifest::read(path)
                    .and_then(|document| manifest::inspect(engine, &document, cancel))
                {
                    Ok(preview) => {
                        let incomplete = preview.packages.iter().any(|entry| {
                            matches!(entry.status, PreviewStatus::Unavailable)
                                && entry.reason.contains("could not be checked")
                        });
                        (
                            json!({"manifest_preview":preview}),
                            if incomplete { 8 } else { 0 },
                        )
                    }
                    Err(error) => (json!({"error":error.to_string()}), 1),
                },
            };
        }
        Commands::Info { name } => {
            return match select(engine, args, name, Lookup::Details, cancel)
                .result
                .and_then(|id| engine.details(&id, cancel))
            {
                Ok(details) => (json!(details), 0),
                Err(e) => failure(e),
            }
        }
        Commands::Inspect { command } => {
            let mut inventory = engine.installed(cancel);
            if args.from.is_empty() {
                inventory
                    .failures
                    .retain(|failure| !matches!(failure.error, EngineError::Unavailable { .. }));
            }
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
                json!({"error": "doctor only prints text; run it without --json"}),
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
    // A typed name no source confirmed, with the registries that could try it.
    let mut missing: Option<(String, Vec<String>)> = None;
    // Registry sources that could also try a name a catalog source matched.
    let mut alternatives: Vec<(Operation, Vec<String>)> = Vec::new();
    let planned = (|| -> Result<Vec<Operation>, EngineError> {
        match command {
            // Only sources that keep package lists can refresh them. Missing
            // managers are skipped unless they were asked for by name.
            Commands::Refresh => {
                let mut operations = Vec::new();
                for source in engine.discover(cancel) {
                    match source.availability {
                        Ok(Availability::Available)
                            if source.capabilities.contains(&Capability::Refresh) =>
                        {
                            operations.push(Operation::Refresh {
                                backend: source.backend,
                            })
                        }
                        Ok(Availability::Available) if !args.from.is_empty() => {
                            return Err(EngineError::Unsupported {
                                backend: source.backend,
                                capability: Capability::Refresh,
                            })
                        }
                        Ok(Availability::Unavailable(reason)) if !args.from.is_empty() => {
                            return Err(EngineError::Unavailable {
                                backend: source.backend,
                                reason,
                            })
                        }
                        Err(error) if !args.from.is_empty() => return Err(error),
                        _ => {}
                    }
                }
                Ok(operations)
            }
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
                let lookup = if matches!(command, Commands::Install { .. }) {
                    Lookup::Install
                } else {
                    Lookup::Installed
                };
                for name in names {
                    let selection = select(engine, args, name, lookup, cancel);
                    let id = match selection.result {
                        Ok(id) => id,
                        Err(error) => {
                            if matches!(error, EngineError::NotFound)
                                && !selection.offers.is_empty()
                            {
                                missing = Some((name.clone(), selection.offers));
                            }
                            return Err(error);
                        }
                    };
                    let operation = match command {
                        Commands::Install { .. } => Operation::Install(id),
                        Commands::Remove { .. } => Operation::Remove(id),
                        _ => Operation::Upgrade(id),
                    };
                    if !selection.offers.is_empty() {
                        alternatives.push((operation.clone(), selection.offers));
                    }
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
        Err(e) => {
            let (mut data, code) = failure(e);
            if let Some((name, offers)) = missing {
                data["message"] = json!(format!(
                    "No package named {name} was found. PkgDeck can't search {sources}, but they can try to install it by name: add {flags}.",
                    name = crate::presentation::clean(&name),
                    sources = source_list(&offers),
                    flags = from_flags(&offers)
                ));
                data["offers"] = json!(offers);
            }
            return (data, code);
        }
    };
    for (operation, offers) in alternatives {
        events(Event::Progress {
            operation,
            progress: Progress::Message(format!(
                "{} may also have this name. To use {} instead, add {}.",
                source_list(&offers),
                if offers.len() == 1 {
                    "it"
                } else {
                    "one of them"
                },
                from_flags(&offers)
            )),
        });
    }
    if let Some(operation) = operations.iter().find(
        |operation| matches!(operation, Operation::UpgradeAll { backend } if backend == "apt"),
    ) {
        let plan = match engine.plan_apt_upgrade(cancel) {
            Ok(plan) => plan,
            Err(error) => return failure(error),
        };
        events(Event::Progress {
            operation: operation.clone(),
            // Shown under "Update all apt packages"; empty sections add nothing.
            progress: Progress::Message(
                plan.summary()
                    .lines()
                    .filter(|line| !line.ends_with(": none"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
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
                json!({"error": "apt_removals_require_consent", "message": "This upgrade would remove packages. Review the plan, then add --allow-removals to approve.", "plan": plan}),
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
    if !operations.is_empty()
        && !args.yes
        && !matches!(command, Commands::Refresh)
        && !confirm(&operations)
    {
        return (
            json!({"error": "confirmation_declined", "message": "Cancelled. Nothing was changed."}),
            7,
        );
    }
    if !operations.is_empty() {
        if let Err(error) = authorize(&operations) {
            return failure(error);
        }
    }
    let history = History::default_store();
    let activity_id = history
        .as_ref()
        .and_then(|store| store.begin("cli", operations.clone(), State::Running).ok());
    let results = engine.execute_batch(&operations, cancel, events);
    if let (Some(store), Some(id)) = (&history, activity_id) {
        let outcomes = results
            .iter()
            .map(|result| match result {
                Ok(_) => Outcome::Finished,
                Err(EngineError::Cancelled) => Outcome::Cancelled,
                Err(_) => Outcome::Failed,
            })
            .collect();
        let _ = store.finish(id, outcomes);
    }
    let failed = results.iter().filter(|r| r.is_err()).count();
    let code = if failed == 0 {
        0
    } else if failed != results.len() {
        8
    } else {
        error_code(results.iter().find_map(|r| r.as_ref().err()).unwrap())
    };
    let operations = operations
        .into_iter()
        .zip(results)
        .map(|(operation, result)| match &result {
            Ok(_) => json!({"operation": operation, "result": result}),
            Err(error) => json!({
                "operation": operation,
                "result": result,
                "message": crate::presentation::error_message(error),
            }),
        })
        .collect::<Vec<_>>();
    (json!({ "operations": operations }), code)
}
fn color_for(stream: &impl IsTerminal) -> bool {
    stream.is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").is_ok_and(|term| term != "dumb")
}
/// `results_shown` means each change already got its own line on stderr, so
/// stdout only needs the closing summary.
fn emit(args: &Args, data: Value, code: u8, results_shown: bool) -> u8 {
    let text = if args.json {
        serde_json::to_string(&json!({"schema_version":1,"exit_code":code,"data":data}))
            .expect("serializable result")
    } else {
        let color = color_for(&io::stdout());
        if results_shown && data["operations"].is_array() {
            format!(
                "\n{}",
                crate::presentation::operations_summary(&data, color)
            )
        } else {
            let width = if io::stdout().is_terminal() {
                rustix::termios::tcgetwinsize(io::stdout())
                    .map_or(100, |size| usize::from(size.ws_col))
            } else {
                100
            };
            crate::presentation::human(&data, width, color)
        }
    };
    if writeln!(io::stdout().lock(), "{text}").is_err() {
        return 1;
    }
    code
}
/// What the spinner says while a command reads from package managers.
fn working_label(command: &Commands) -> Option<String> {
    Some(match command {
        Commands::Search { query } => {
            format!("Searching for {}", crate::presentation::clean(query))
        }
        Commands::Info { name } => format!("Looking up {}", crate::presentation::clean(name)),
        Commands::Inspect { command } => {
            format!("Inspecting {}", crate::presentation::clean(command))
        }
        Commands::List | Commands::Inventory { .. } => "Reading installed packages".into(),
        Commands::Audit => "Looking for duplicates and leftovers".into(),
        Commands::Sources => "Checking package managers".into(),
        Commands::Refresh => "Checking package managers".into(),
        Commands::Upgrade { names, .. } if names.is_empty() => "Checking for updates".into(),
        Commands::Install { .. } | Commands::Remove { .. } | Commands::Upgrade { .. } => {
            "Finding packages".into()
        }
        Commands::Clean { .. } => "Looking for cleanup tasks".into(),
        Commands::Repos { .. } => "Reading repositories".into(),
        Commands::Doctor | Commands::Completions { .. } => return None,
    })
}
fn repository_command(
    args: &Args,
    command: &Option<RepoCommand>,
    cancel: &Cancellation,
    live: &Live,
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
            live.clear();
            eprintln!("{}", crate::presentation::clean(label));
            crate::session::ask("Apply this repository change?", &mut io::stdin().lock())
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
            json!({"error": "choose one package manager with --from"}),
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
        return (
            json!({"error": "confirmation_declined", "message": "Cancelled. Nothing was changed."}),
            7,
        );
    }
    match repositories::apply(transport, &action, cancel) {
        Ok(()) => (json!({"message": "Repository operation completed"}), 0),
        Err(error) => failure(error),
    }
}
pub fn run(args: &Args) -> u8 {
    if let Some(Commands::Completions { shell }) = args.command {
        completions(shell, &mut io::stdout().lock());
        return 0;
    }
    if args.command.as_ref().is_some_and(Commands::writes)
        && !args.yes
        && (args.json || !io::stdin().is_terminal() || !io::stdout().is_terminal())
    {
        return emit(
            args,
            json!({"error":"confirmation_required","message":"add --yes to approve changes in JSON or non-interactive mode"}),
            2,
            false,
        );
    }
    let cancel = Cancellation::default();
    let signal = match signal_hook::flag::register(signal_hook::consts::SIGINT, cancel.flag()) {
        Ok(id) => id,
        Err(e) => return emit(args, json!({"error":e.to_string()}), 1, false),
    };
    let command = args.command.as_ref().expect("CLI command");
    // JSON output stays machine-only: no spinner or result lines on stderr.
    let live = if args.json {
        Live::off()
    } else {
        Live::new(color_for(&io::stderr()))
    };
    if !args.json {
        if let Some(label) = working_label(command) {
            live.status(label);
        }
    }
    if let Commands::Repos { command } = command {
        let (data, code) = repository_command(args, command, &cancel, &live);
        signal_hook::low_level::unregister(signal);
        drop(live);
        return emit(args, data, code, false);
    }
    // Native ownership databases can only identify these package managers.
    // Querying every unrelated inventory makes a simple PATH lookup slow.
    let inspection = matches!(command, Commands::Inspect { .. });
    let sources = if inspection {
        inspection_sources(&args.from)
    } else {
        args.from.clone()
    };
    let selected_engine = if inspection && sources.is_empty() {
        Ok(Engine::default())
    } else {
        pkgdeck_core::backends::native_engine(
            &sources,
            matches!(command, Commands::Sources),
            args.auth.into(),
            &cancel,
        )
    };
    let mut engine = match selected_engine {
        Ok(engine) => engine,
        Err(e) => {
            signal_hook::low_level::unregister(signal);
            drop(live);
            let (data, code) = failure(e);
            return emit(args, data, code, false);
        }
    };
    let session = Session::new(&live);
    let (data, code) = dispatch_with(
        &mut engine,
        args,
        &cancel,
        &mut |operations| session.confirm(operations, &mut io::stdin().lock()),
        &mut |operations| {
            if args.json {
                return Ok(());
            }
            if matches!(args.auth, Auth::Sudo) {
                crate::session::sudo_login(&live, operations, &cancel)?;
            }
            // A refresh changes nothing to review, so it just starts.
            session.start(operations, !matches!(command, Commands::Refresh));
            Ok(())
        },
        &mut |event| session.event(event),
    );
    let shown = session.results_shown();
    drop(session);
    signal_hook::low_level::unregister(signal);
    drop(live);
    emit(args, data, code, shown)
}

fn inspection_sources(requested: &[String]) -> Vec<String> {
    const NATIVE_OWNERS: [&str; 4] = ["apt", "dnf", "pacman", "zypper"];
    if requested.is_empty() {
        NATIVE_OWNERS
            .iter()
            .map(|source| (*source).into())
            .collect()
    } else {
        requested
            .iter()
            .filter(|source| NATIVE_OWNERS.contains(&source.as_str()))
            .cloned()
            .collect()
    }
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
            select(&mut engine, &system, "fixture", Lookup::Details, &cancel)
                .result
                .unwrap()
                .scope,
            Scope::System
        );
        let user = Args::try_parse_from(["pkd", "--scope", "user", "info", "fixture"]).unwrap();
        assert!(
            select(&mut engine, &user, "fixture", Lookup::Details, &cancel)
                .result
                .is_err()
        );
        assert!(Args::try_parse_from(["pkd", "--scope", "invalid", "info", "fixture"]).is_err());
        assert_eq!(
            call(&mut engine, &["--scope", "user", "upgrade"], true).0["operations"],
            json!([])
        );
        assert_eq!(call(&mut engine, &["--scope", "user", "update"], true).1, 2);
    }
    #[test]
    fn inventory_cli_exports_and_previews_exact_packages_without_writes() {
        let path = std::env::temp_dir().join(format!(
            "pkgdeck inventory test {} {:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        let path_text = path.to_str().unwrap();
        let mut installed = engine();
        let export_args =
            Args::try_parse_from(["pkd", "inventory", "export", path_text, "fixture"]).unwrap();
        let (exported, code) = dispatch(
            &mut installed,
            &export_args,
            &Cancellation::default(),
            &mut |_| panic!("inventory export must not request package confirmation"),
            &mut |_| {},
        );
        assert_eq!(code, 0, "{exported}");
        assert_eq!(exported["manifest_export"]["packages"], 1);
        assert_eq!(manifest::read(&path).unwrap().packages[0].name, "fixture");

        let mut target = Engine::default();
        target
            .register(Fixture {
                backend: "apt".into(),
                installed: false,
                fail: None,
                read_failure: None,
                verified: true,
            })
            .unwrap();
        let preview_args =
            Args::try_parse_from(["pkd", "inventory", "preview", path_text]).unwrap();
        let (preview, code) = dispatch(
            &mut target,
            &preview_args,
            &Cancellation::default(),
            &mut |_| panic!("inventory preview must not request package confirmation"),
            &mut |_| {},
        );
        assert_eq!(code, 0, "{preview}");
        assert_eq!(
            preview["manifest_preview"]["packages"][0]["status"],
            "installable"
        );
        let (_, duplicate_code) = dispatch(
            &mut engine(),
            &export_args,
            &Cancellation::default(),
            &mut |_| panic!("inventory export must not request confirmation"),
            &mut |_| {},
        );
        assert_eq!(duplicate_code, 1);
        let missing_args = Args::try_parse_from([
            "pkd",
            "inventory",
            "export",
            path_text,
            "missing-synthetic-package",
        ])
        .unwrap();
        let (_, missing_code) = dispatch(
            &mut engine(),
            &missing_args,
            &Cancellation::default(),
            &mut |_| panic!("inventory export must not request confirmation"),
            &mut |_| {},
        );
        assert_eq!(missing_code, 3);
        let mut failed_target = Engine::default();
        failed_target
            .register(Fixture {
                backend: "apt".into(),
                installed: false,
                fail: None,
                read_failure: Some(EngineError::InvalidResponse {
                    backend: "apt".into(),
                    reason: "synthetic catalog failure".into(),
                }),
                verified: true,
            })
            .unwrap();
        let (incomplete, incomplete_code) = dispatch(
            &mut failed_target,
            &preview_args,
            &Cancellation::default(),
            &mut |_| panic!("inventory preview must not request confirmation"),
            &mut |_| {},
        );
        assert_eq!(incomplete_code, 8);
        assert_eq!(
            incomplete["manifest_preview"]["packages"][0]["status"],
            "unavailable"
        );
        std::fs::remove_file(path).unwrap();
        let (_, absent_code) = dispatch(
            &mut engine(),
            &preview_args,
            &Cancellation::default(),
            &mut |_| panic!("inventory preview must not request confirmation"),
            &mut |_| {},
        );
        assert_eq!(absent_code, 1);
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
    fn help_completions_and_the_refresh_alias() {
        use clap::CommandFactory;
        // `update` stays as a visible alias of `refresh`.
        for name in ["refresh", "update"] {
            let args = Args::try_parse_from(["pkd", name]).unwrap();
            assert!(matches!(args.command, Some(Commands::Refresh)));
        }
        let mut command = Args::command();
        let help = command.render_long_help().to_string();
        assert!(
            help.contains("Examples:") && help.contains("pkd refresh"),
            "{help}"
        );
        assert!(help.contains("[alias: update]"), "{help}");
        assert!(!help.contains("homebrew-cask"), "{help}");
        // Everyday commands come first.
        let order: Vec<_> = command
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect();
        assert_eq!(&order[..4], ["search", "info", "install", "remove"]);
        for (shell, marker) in [
            (Shell::Bash, "complete -F"),
            (Shell::Zsh, "#compdef pkd"),
            (Shell::Fish, "complete -c pkd"),
        ] {
            let mut script = Vec::new();
            completions(shell, &mut script);
            let script = String::from_utf8(script).unwrap();
            assert!(
                script.contains(marker) && script.contains("refresh"),
                "{script}"
            );
        }
        assert!(Args::try_parse_from(["pkd", "completions", "powershell"]).is_err());
        assert_eq!(
            run(&Args::try_parse_from(["pkd", "completions", "bash"]).unwrap()),
            0
        );
        assert!(working_label(&Commands::Completions { shell: Shell::Fish }).is_none());
    }
    #[test]
    fn every_command_succeeds_and_writes_need_confirmation() {
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
            vec!["refresh"],
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
        assert_eq!(inspection_sources(&[]), ["apt", "dnf", "pacman", "zypper"]);
        assert_eq!(
            inspection_sources(&["flatpak".into(), "apt".into()]),
            ["apt"]
        );
        assert!(inspection_sources(&["flatpak".into()]).is_empty());
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
        let mut unavailable = Engine::default();
        unavailable
            .register(Fixture {
                backend: "dnf".into(),
                installed: false,
                fail: None,
                read_failure: Some(EngineError::Unavailable {
                    backend: "dnf".into(),
                    reason: "synthetic missing manager".into(),
                }),
                verified: true,
            })
            .unwrap();
        let (implicit, code) = call(
            &mut unavailable,
            &["inspect", "pkgdeck-fixture-missing"],
            false,
        );
        assert_eq!(code, 0);
        assert!(implicit["failures"].as_array().unwrap().is_empty());
        let (explicit, code) = call(
            &mut unavailable,
            &["--from", "dnf", "inspect", "pkgdeck-fixture-missing"],
            false,
        );
        assert_eq!(code, 8);
        assert_eq!(explicit["failures"].as_array().unwrap().len(), 1);
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
    fn registry_guesses_never_make_a_typed_name_ambiguous() {
        let guess = |backend: &str| Fixture {
            backend: backend.into(),
            installed: false,
            fail: None,
            read_failure: None,
            verified: false,
        };
        let mut engine = engine();
        engine.register(guess("cargo")).unwrap();
        engine.register(guess("npm")).unwrap();
        let args = Args::try_parse_from(["pkd", "install", "fixture"]).unwrap();
        let mut notes = vec![];
        let (data, code) = dispatch(
            &mut engine,
            &args,
            &Cancellation::default(),
            &mut |operations| {
                assert!(matches!(operations, [Operation::Install(id)] if id.backend == "apt"));
                true
            },
            &mut |event| {
                if let Event::Progress {
                    progress: Progress::Message(message),
                    ..
                } = event
                {
                    notes.push(message);
                }
            },
        );
        assert_eq!(code, 0, "{data}");
        assert!(
            notes.contains(
                &"Cargo and npm may also have this name. To use one of them instead, add --from cargo or --from npm."
                    .to_string()
            ),
            "{notes:?}"
        );
        assert_eq!(call(&mut engine, &["info", "fixture"], false).1, 0);
        let mut one = self::engine();
        one.register(guess("npm")).unwrap();
        let mut notes = vec![];
        let (_, code) = dispatch(
            &mut one,
            &args,
            &Cancellation::default(),
            &mut |_| true,
            &mut |event| {
                if let Event::Progress {
                    progress: Progress::Message(message),
                    ..
                } = event
                {
                    notes.push(message);
                }
            },
        );
        assert_eq!(code, 0);
        assert!(
            notes.contains(
                &"npm may also have this name. To use it instead, add --from npm.".to_string()
            ),
            "{notes:?}"
        );

        let mut guesses = Engine::default();
        for backend in ["cargo", "npm", "pipx"] {
            guesses.register(guess(backend)).unwrap();
        }
        let (data, code) = call(&mut guesses, &["install", "fixture"], true);
        assert_eq!(code, 3);
        assert_eq!(data["offers"], json!(["cargo", "npm", "pipx"]));
        assert_eq!(
            data["message"],
            "No package named fixture was found. PkgDeck can't search Cargo, npm, and pipx, but they can try to install it by name: add --from cargo, --from npm, or --from pipx."
        );
        let (data, code) = call(&mut guesses, &["info", "fixture"], false);
        assert_eq!(code, 3);
        assert!(data.get("offers").is_none());
        // A single guess is still a usable install target, and --from pins it.
        let mut single = Engine::default();
        single.register(guess("npm")).unwrap();
        let (data, code) = call(&mut single, &["install", "fixture"], true);
        assert_eq!(code, 0, "{data}");
        assert_eq!(
            call(
                &mut guesses,
                &["--from", "pipx", "install", "fixture"],
                true
            )
            .1,
            0
        );
        assert_eq!(from_flags(&["npm".into()]), "--from npm");
        assert_eq!(source_list(&["pipx".into()]), "pipx");
        assert_eq!(source_list(&[]), "");
        assert_eq!(from_flags(&[]), "");
    }

    #[test]
    fn every_reading_command_names_what_it_is_doing() {
        for (words, label) in [
            (vec!["search", "vim"], "Searching for vim"),
            (vec!["info", "vim"], "Looking up vim"),
            (vec!["inspect", "vim"], "Inspecting vim"),
            (vec!["list"], "Reading installed packages"),
            (
                vec!["inventory", "preview", "saved.json"],
                "Reading installed packages",
            ),
            (vec!["audit"], "Looking for duplicates and leftovers"),
            (vec!["sources"], "Checking package managers"),
            (vec!["update"], "Checking package managers"),
            (vec!["upgrade"], "Checking for updates"),
            (vec!["upgrade", "vim"], "Finding packages"),
            (vec!["install", "vim"], "Finding packages"),
            (vec!["remove", "vim"], "Finding packages"),
            (vec!["clean"], "Looking for cleanup tasks"),
            (vec!["repos"], "Reading repositories"),
        ] {
            let args = Args::try_parse_from(std::iter::once("pkd").chain(words)).unwrap();
            assert_eq!(
                working_label(args.command.as_ref().unwrap()).unwrap(),
                label
            );
        }
        let doctor = Args::try_parse_from(["pkd", "doctor"]).unwrap();
        assert!(working_label(doctor.command.as_ref().unwrap()).is_none());
    }
    struct NoRefresh(&'static str);
    impl Backend for NoRefresh {
        fn id(&self) -> &str {
            self.0
        }
        fn capabilities(&self) -> &[Capability] {
            &[Capability::Search, Capability::Installed]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
    }
    #[test]
    fn update_refreshes_only_capable_sources_without_asking() {
        let mut engine = engine();
        engine.register(NoRefresh("npm")).unwrap();
        let args = Args::try_parse_from(["pkd", "update"]).unwrap();
        assert!(!args.command.as_ref().unwrap().writes());
        let (data, code) = dispatch(
            &mut engine,
            &args,
            &Cancellation::default(),
            &mut |_| panic!("refreshing package lists must not ask for approval"),
            &mut |_| {},
        );
        assert_eq!(code, 0, "{data}");
        assert_eq!(
            data["operations"],
            json!([{"operation":{"refresh":{"backend":"apt"}},"result":{"Ok":{"cancellation_deferred":false}}}])
        );
        // Asking for a source that cannot refresh by name is still an error.
        assert_eq!(call(&mut engine, &["--from", "npm", "update"], true).1, 1);
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
