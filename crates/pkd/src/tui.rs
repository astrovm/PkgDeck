//! Terminal presentation only. Each job owns an engine on a worker thread.
use crate::cli::Args;
use crossterm::event::{self, Event as Input, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use pkgdeck_core::{engine::*, package::*, process::Cancellation};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Row, Table, TableState, Wrap},
};
use std::{io, sync::mpsc, thread, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq)]
enum View {
    Search,
    Installed,
    Updates,
    Sources,
}
#[derive(Clone, Debug)]
enum Job {
    Load(View, String),
    Details(PackageId),
    Write(Operation),
}
enum Reply {
    Packages(PackageReport),
    Sources(Vec<Source>),
    Details(Box<PackageDetails>),
    Written,
    Status(String),
    Failed(String),
}

fn perform(engine: &mut Engine, job: Job, cancel: &Cancellation, send: &mut dyn FnMut(Reply)) {
    match job {
        Job::Load(View::Sources, _) => send(Reply::Sources(engine.discover(cancel))),
        Job::Load(view, query) => {
            if view == View::Search {
                // Stream cumulative partials like the GUI; Updates stays
                // synchronous below so upgrade gating sees complete failures.
                engine.search_stream(&query, cancel, &mut |partial| {
                    send(Reply::Packages(partial))
                });
            } else {
                let mut report = engine.installed(cancel);
                if view == View::Updates {
                    report
                        .packages
                        .retain(|p| p.update == UpdateAvailability::Available);
                }
                send(Reply::Packages(report));
            }
        }
        Job::Details(id) => match engine.details(&id, cancel) {
            Ok(details) => send(Reply::Details(Box::new(details))),
            Err(e) => send(Reply::Failed(e.to_string())),
        },
        Job::Write(operation) => {
            let result = engine.execute(&operation, cancel, &mut |event| {
                if let Event::Progress { progress, .. } = event {
                    send(Reply::Status(match progress {
                        Progress::Message(message) => message,
                        Progress::Transfer { completed, total } => match total {
                            Some(total) => format!("Transferred {completed} of {total}"),
                            None => format!("Transferred {completed}"),
                        },
                    }));
                }
            });
            match result {
                Ok(outcome) => {
                    send(Reply::Written);
                    send(Reply::Status(
                        if outcome.cancellation_deferred {
                            "Completed after cancellation; native changes were not rolled back."
                        } else {
                            "Completed. Press r to reload package state."
                        }
                        .into(),
                    ));
                }
                Err(e) => send(Reply::Failed(e.to_string())),
            }
        }
    }
}

fn readable(text: impl AsRef<str>) -> String {
    text.as_ref()
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

struct App {
    view: View,
    query: String,
    editing: bool,
    packages: Vec<Package>,
    sources: Vec<Source>,
    table: TableState,
    details: Option<PackageDetails>,
    confirmation: Option<Operation>,
    status: String,
    expanded: bool,
    scroll: u16,
    busy: bool,
    cancelled: bool,
    color: bool,
}
/// Honor NO_COLOR and dumb terminals, mirroring the CLI contract: styling is
/// chromatic only when explicitly allowed. Layout markers are unaffected.
fn colors_enabled() -> bool {
    colors_allowed(
        std::env::var_os("NO_COLOR").is_none(),
        std::env::var("TERM"),
    )
}
fn colors_allowed(no_color_unset: bool, term: Result<String, std::env::VarError>) -> bool {
    no_color_unset && term.is_ok_and(|term| term != "dumb")
}
impl Default for App {
    fn default() -> Self {
        Self {
            view: View::Search,
            query: String::new(),
            editing: false,
            packages: vec![],
            sources: vec![],
            table: TableState::default(),
            details: None,
            confirmation: None,
            expanded: false,
            scroll: 0,
            status: "Press / to search; 2 installed, 3 updates, 4 sources.".into(),
            busy: false,
            cancelled: false,
            color: true,
        }
    }
}
impl App {
    /// Strip foreground/background colors when the terminal opts out. Text
    /// attributes and layout are preserved so output stays structured.
    fn paint(&self, style: Style) -> Style {
        if self.color {
            style
        } else {
            Style {
                fg: None,
                bg: None,
                ..style
            }
        }
    }
    fn load(&mut self, view: View) -> Job {
        self.scroll = 0;
        self.view = view;
        self.packages.clear();
        self.sources.clear();
        self.details = None;
        self.table.select(None);
        Job::Load(view, self.query.clone())
    }
    fn reply(&mut self, reply: Reply) {
        match reply {
            Reply::Packages(report) => {
                self.status = if report.failures.is_empty() {
                    format!("{} packages", report.packages.len())
                } else {
                    report
                        .failures
                        .iter()
                        .map(|f| format!("{}: {}", f.backend, f.error))
                        .collect::<Vec<_>>()
                        .join("; ")
                };
                // Streaming partials re-sort rows around the selection:
                // follow the selected identity instead of the row index.
                let keep = self
                    .table
                    .selected()
                    .and_then(|i| self.packages.get(i))
                    .map(|p| p.id.clone());
                self.packages = report.packages;
                let selected = keep
                    .and_then(|id| self.packages.iter().position(|p| p.id == id))
                    .or((!self.packages.is_empty()).then_some(0));
                self.table.select(selected);
            }
            Reply::Sources(sources) => {
                self.sources = sources;
                self.table.select((!self.sources.is_empty()).then_some(0));
                self.status = "Select a source; u refreshes metadata only.".into();
            }
            Reply::Details(details) => {
                self.status = "Package details loaded.".into();
                self.details = Some(*details);
            }
            Reply::Written => {
                self.packages.clear();
                self.details = None;
                self.table.select(None);
            }
            Reply::Status(text) | Reply::Failed(text) => self.status = text,
        }
    }
    fn selected_details(&self) -> Option<Job> {
        self.table
            .selected()
            .and_then(|i| self.packages.get(i))
            .map(|p| Job::Details(p.id.clone()))
    }
    fn key(&mut self, key: KeyEvent) -> (bool, Option<Job>) {
        if key.kind != KeyEventKind::Press {
            return (false, None);
        }
        let escape = key.code == KeyCode::Esc
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL));
        if matches!(key.code, KeyCode::PageDown | KeyCode::PageUp) {
            self.scroll = if key.code == KeyCode::PageDown {
                self.scroll.saturating_add(3)
            } else {
                self.scroll.saturating_sub(3)
            };
            return (false, None);
        }
        if self.expanded {
            if escape || key.code == KeyCode::Char('e') {
                self.expanded = false;
                self.scroll = 0;
            }
            return (false, None);
        }
        if self.busy {
            if escape || key.code == KeyCode::Char('q') {
                self.cancelled = true;
                self.status =
                    "Cancellation requested; waiting for the native operation to finish safely."
                        .into();
            }
            return (false, None);
        }
        if self.confirmation.is_some() {
            let job = if key.code == KeyCode::Char('y') {
                self.confirmation.take().map(Job::Write)
            } else {
                if escape || key.code == KeyCode::Char('n') {
                    self.confirmation = None;
                }
                None
            };
            return (false, job);
        }
        if self.editing {
            match key.code {
                _ if escape => self.editing = false,
                KeyCode::Enter => {
                    self.editing = false;
                    if !self.query.trim().is_empty() {
                        return (false, Some(self.load(View::Search)));
                    }
                }
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.query.push(c);
                }
                _ => (),
            }
            return (false, None);
        }
        if escape || key.code == KeyCode::Char('q') {
            return (true, None);
        }
        let job = match key.code {
            KeyCode::Char('e') => {
                self.expanded = true;
                self.scroll = 0;
                None
            }
            KeyCode::Char('/') => {
                self.editing = true;
                None
            }
            KeyCode::Char('1') => {
                self.view = View::Search;
                self.packages.clear();
                self.sources.clear();
                self.details = None;
                self.table.select(None);
                self.editing = true;
                None
            }
            KeyCode::Char('2') => Some(self.load(View::Installed)),
            KeyCode::Char('3') => Some(self.load(View::Updates)),
            KeyCode::Char('4') => Some(self.load(View::Sources)),
            KeyCode::Char('r') => {
                if self.view != View::Search || !self.query.trim().is_empty() {
                    Some(self.load(self.view))
                } else {
                    None
                }
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Up | KeyCode::Char('k') => {
                let len = if self.view == View::Sources {
                    self.sources.len()
                } else {
                    self.packages.len()
                };
                if len > 0 {
                    let i = self.table.selected().unwrap_or(0);
                    self.table.select(Some(
                        if matches!(key.code, KeyCode::Down | KeyCode::Char('j')) {
                            (i + 1).min(len - 1)
                        } else {
                            i.saturating_sub(1)
                        },
                    ));
                    self.details = None;
                    self.scroll = 0;
                }
                None
            }
            KeyCode::Enter => self.selected_details(),
            KeyCode::Char('i' | 'd' | 'g' | 'u') => {
                self.scroll = 0;
                self.confirmation = self.table.selected().and_then(|i| {
                    if self.view == View::Sources {
                        self.sources
                            .get(i)
                            .filter(|s| {
                                key.code == KeyCode::Char('u')
                                    && s.availability == Ok(Availability::Available)
                                    && s.capabilities.contains(&Capability::Refresh)
                            })
                            .map(|s| Operation::Refresh {
                                backend: s.backend.clone(),
                            })
                    } else {
                        self.packages.get(i).and_then(|p| match key.code {
                            KeyCode::Char('i') if p.installed_version.is_none() => {
                                Some(Operation::Install(p.id.clone()))
                            }
                            KeyCode::Char('d') if p.installed_version.is_some() => {
                                Some(Operation::Remove(p.id.clone()))
                            }
                            KeyCode::Char('g') if p.update == UpdateAvailability::Available => {
                                Some(Operation::Upgrade(p.id.clone()))
                            }
                            _ => None,
                        })
                    }
                });
                None
            }
            _ => None,
        };
        (false, job)
    }
    fn draw(&mut self, frame: &mut ratatui::Frame) {
        if self.confirmation.is_some() || self.expanded {
            let text = self.confirmation.as_ref().map(|op| format!("Confirm {}\n\nNative dependency changes may follow.\n[!] Press y to confirm, n or Esc to go back.", crate::presentation::operation(op))).unwrap_or_else(|| self.status.clone());
            frame.render_widget(
                Paragraph::new(readable(text))
                    .wrap(Wrap { trim: false })
                    .scroll((self.scroll, 0))
                    .block(
                        Block::bordered()
                            .border_type(ratatui::widgets::BorderType::Rounded)
                            .border_style(
                                self.paint(Style::default().fg(Color::Rgb(104, 151, 207))),
                            )
                            .title("Status / confirmation - PgUp/PgDn scroll, Esc back"),
                    ),
                frame.area(),
            );
            return;
        }

        let areas = Layout::vertical([
            Constraint::Length(5),
            Constraint::Min(3),
            Constraint::Length(if frame.area().height >= 24 { 8 } else { 4 }),
            Constraint::Length(4),
            Constraint::Length(2),
        ])
        .split(frame.area());
        // The query only applies to Search; dim it elsewhere so a stale
        // string is not mistaken for a filter on other views.
        let query = Line::from(vec![
            Span::raw("1 / Search   2 [x] Installed   3 [^] Updates   4 [=] Sources\n\n/ "),
            Span::styled(
                format!("{}{}", self.query, if self.editing { "_" } else { "" }),
                if self.view == View::Search {
                    Style::default()
                } else {
                    Style::default().add_modifier(Modifier::DIM)
                },
            ),
        ]);
        frame.render_widget(
            Paragraph::new(query).block(
                Block::bordered()
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .border_style(self.paint(Style::default().fg(Color::Rgb(104, 151, 207))))
                    .title(format!("PkgDeck - {:?}", self.view)),
            ),
            areas[0],
        );
        let rows: Vec<Row> = if self.view == View::Sources {
            self.sources
                .iter()
                .map(|s| {
                    Row::new(vec![
                        readable(&s.backend),
                        readable(match &s.availability {
                            Ok(Availability::Available) => "Available".into(),
                            Ok(Availability::Unavailable(reason)) => {
                                format!("Unavailable: {reason}")
                            }
                            Err(error) => error.to_string(),
                        }),
                        s.capabilities
                            .iter()
                            .map(|c| format!("{c:?}"))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ])
                })
                .collect()
        } else {
            self.packages
                .iter()
                .map(|p| {
                    Row::new(vec![
                        format!(
                            "{} {}",
                            crate::presentation::package_marker(
                                p.installed_version.is_some(),
                                p.update == UpdateAvailability::Available
                            ),
                            readable(&p.id.name)
                        ),
                        readable(format!("{} / {}", p.id.backend, p.id.architecture)),
                        readable(format!(
                            "{} -> {}",
                            p.installed_version.as_deref().unwrap_or("not installed"),
                            p.candidate_version.as_deref().unwrap_or("unknown")
                        )),
                        readable(&p.summary),
                    ])
                })
                .collect()
        };
        let headers = if self.view == View::Sources {
            vec!["Source", "Availability", "Capabilities"]
        } else {
            vec!["Name", "Source / arch", "Installed -> candidate", "Summary"]
        };
        let wide = frame.area().width >= 100;
        let widths = if self.view == View::Sources {
            vec![
                Constraint::Percentage(20),
                Constraint::Percentage(35),
                Constraint::Percentage(45),
            ]
        } else if wide {
            vec![
                Constraint::Percentage(25),
                Constraint::Percentage(18),
                Constraint::Percentage(24),
                Constraint::Percentage(33),
            ]
        } else {
            vec![
                Constraint::Percentage(35),
                Constraint::Percentage(25),
                Constraint::Percentage(40),
            ]
        };
        frame.render_stateful_widget(
            Table::new(rows, widths)
                .column_spacing(2)
                .header(
                    Row::new(headers).style(
                        self.paint(
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ),
                )
                .block(
                    Block::bordered()
                        .border_type(ratatui::widgets::BorderType::Rounded)
                        .border_style(self.paint(Style::default().fg(Color::Rgb(104, 151, 207))))
                        .title(format!(
                            " Results · {} {} ",
                            if self.view == View::Sources {
                                self.sources.len()
                            } else {
                                self.packages.len()
                            },
                            if self.view == View::Sources {
                                "sources"
                            } else {
                                "packages"
                            }
                        )),
                )
                .row_highlight_style(
                    self.paint(
                        Style::default()
                            .bg(Color::Rgb(40, 62, 89))
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                )
                .highlight_symbol("> "),
            areas[1],
            &mut self.table,
        );
        let detail = if self.view == View::Sources {
            self.table
                .selected()
                .and_then(|i| self.sources.get(i))
                .map(|s| {
                    let status = match &s.availability {
                        Ok(Availability::Available) => "Available".into(),
                        Ok(Availability::Unavailable(reason)) => {
                            format!("Unavailable: {reason}")
                        }
                        Err(error) => error.to_string(),
                    };
                    let capabilities = s
                        .capabilities
                        .iter()
                        .map(|c| format!("{c:?}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{}\n{}\nCapabilities: {}", s.backend, status, capabilities)
                })
                .unwrap_or_else(|| "No source selected.".into())
        } else if let Some(d) = &self.details {
            format!(
                "{}\nScope: {}\n{}\nHomepage: {}\nDependencies: {}",
                d.package.id.name,
                crate::presentation::scope(&d.package.id.scope),
                d.description,
                d.homepage.as_deref().unwrap_or("unavailable"),
                d.dependencies.join(", ")
            )
        } else if let Some(p) = self.table.selected().and_then(|i| self.packages.get(i)) {
            format!(
                "{} | {} | Update: {:?}\n{}\nEnter: load full details",
                p.id.name,
                crate::presentation::scope(&p.id.scope),
                p.update,
                p.summary
            )
        } else {
            "Search for a package with /, or press 2 to browse installed packages.\nUse ↑/↓ to select a result and Enter for full details.".into()
        };
        frame.render_widget(
            Paragraph::new(readable(detail))
                .scroll((self.scroll, 0))
                .wrap(Wrap { trim: false })
                .block(
                    Block::bordered()
                        .border_type(ratatui::widgets::BorderType::Rounded)
                        .border_style(self.paint(Style::default().fg(Color::Rgb(104, 151, 207))))
                        .title("Package details"),
                ),
            areas[2],
        );
        let status = format!(
            "{}{}",
            if self.busy { "[...] Working: " } else { "" },
            self.status
        );
        frame.render_widget(
            Paragraph::new(readable(status))
                .wrap(Wrap { trim: false })
                .block(
                    Block::bordered()
                        .border_type(ratatui::widgets::BorderType::Rounded)
                        .border_style(self.paint(Style::default().fg(Color::Rgb(104, 151, 207))))
                        .title("Status / confirmation"),
                ),
            areas[3],
        );
        // Two fixed wordings keep wraps on item boundaries: the full ledger
        // needs 100 columns, while the compact ledger fits narrow terminals.
        let footer = if frame.area().width >= 100 {
            "/ search  arrows/j/k select  Enter details  i install  d remove  g upgrade\nu refresh source  r reload  e status  PgUp/PgDn details  Esc cancel/back  q quit"
        } else {
            "/ search  arrows select  Enter details  i install  d remove\ng upgrade  u refresh  r reload  e status  PgUp/PgDn  Esc back  q quit"
        };
        frame.render_widget(Paragraph::new(footer).wrap(Wrap { trim: false }), areas[4]);
    }
}

pub fn run(terminal: &mut ratatui::DefaultTerminal, args: &Args) -> io::Result<()> {
    let mut app = App {
        color: colors_enabled(),
        ..App::default()
    };
    let shutdown = Cancellation::default();
    let signal = signal_hook::flag::register(signal_hook::consts::SIGTERM, shutdown.flag())?;
    let mut worker: Option<(thread::JoinHandle<()>, mpsc::Receiver<Reply>, Cancellation)> = None;
    let result = (|| {
        let mut dirty = true;
        loop {
            if shutdown.requested() {
                if let Some((_, _, cancel)) = &worker {
                    cancel.cancel();
                } else {
                    break;
                }
            }
            if let Some((handle, receive, _)) = &worker {
                let replies: Vec<_> = receive.try_iter().collect();
                dirty |= !replies.is_empty();
                let complete = replies.iter().any(|r| !matches!(r, Reply::Status(_)));
                for reply in replies {
                    app.reply(reply);
                }
                if complete || handle.is_finished() {
                    let (handle, receive, _) = worker.take().unwrap();
                    if handle.join().is_err() {
                        app.status = "Backend worker failed.".into();
                    }
                    for reply in receive.try_iter() {
                        app.reply(reply);
                    }
                    app.busy = false;
                    dirty = true;
                }
            }
            if dirty {
                terminal.draw(|frame| app.draw(frame))?;
                dirty = false;
            }
            if !event::poll(Duration::from_millis(50))? {
                continue;
            }
            let input = event::read()?;
            dirty = true; // Input and resize events invalidate the frame; idle polls do not.
            if let Input::Key(key) = input {
                let (quit, job) = app.key(key);
                if quit {
                    break;
                }
                if let Some((_, _, cancel)) = &worker {
                    if app.cancelled {
                        cancel.cancel();
                    }
                }
                if let Some(job) = job {
                    let (send, receive) = mpsc::channel();
                    let cancel = Cancellation::default();
                    let token = cancel.clone();
                    let source = args.from.clone();
                    let arch = args.arch.clone();
                    let auth = args.auth;
                    app.busy = true;
                    app.cancelled = false;
                    app.status = "Loading…".into();
                    let handle = thread::spawn(move || {
                        let discover = matches!(job, Job::Load(View::Sources, _));
                        // Details address one backend directly, skipping every
                        // other backend's detection roundtrip.
                        let source = match &job {
                            Job::Details(id) => Some(id.backend.clone()),
                            _ => source,
                        };
                        let mut deliver = |mut reply| {
                            if let Reply::Packages(report) = &mut reply {
                                report.packages.retain(|p| {
                                    arch.as_ref().is_none_or(|a| a == &p.id.architecture)
                                });
                            }
                            let _ = send.send(reply);
                        };
                        match pkgdeck_core::backends::native_engine(
                            source.as_deref(),
                            discover,
                            auth.into(),
                            &token,
                        ) {
                            Ok(mut engine) => perform(&mut engine, job, &token, &mut deliver),
                            Err(e) => deliver(Reply::Failed(e.to_string())),
                        }
                    });
                    worker = Some((handle, receive, cancel));
                }
            }
        }
        Ok(())
    })();
    signal_hook::low_level::unregister(signal);
    // Never abandon a native write if terminal I/O fails.
    if let Some((handle, _, cancel)) = worker {
        cancel.cancel();
        let _ = handle.join();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use pkgdeck_core::process::ExecutionError;
    use ratatui::{backend::TestBackend, Terminal};
    fn package() -> Package {
        Package {
            id: PackageId {
                backend: "fixture".into(),
                name: "synthetic-tool".into(),
                architecture: "all".into(),
                scope: Scope::System,
                remote: None,
            },
            display_name: "Synthetic".into(),
            summary: "A synthetic package".into(),
            installed_version: None,
            candidate_version: Some("1".into()),
            update: UpdateAvailability::Current,
        }
    }
    struct Fixture {
        package: Package,
        deny: bool,
    }
    impl Backend for Fixture {
        fn id(&self) -> &str {
            "fixture"
        }
        fn capabilities(&self) -> &[Capability] {
            &[
                Capability::Search,
                Capability::Installed,
                Capability::Details,
                Capability::Install,
                Capability::Upgrade,
                Capability::Remove,
                Capability::Refresh,
            ]
        }
        fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
            Ok(Availability::Available)
        }
        fn search(&mut self, _: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
            Ok(vec![self.package.clone()])
        }
        fn installed(&mut self, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
            Ok(if self.package.installed_version.is_some() {
                vec![self.package.clone()]
            } else {
                vec![]
            })
        }
        fn details(
            &mut self,
            _: &PackageId,
            _: &Cancellation,
        ) -> Result<PackageDetails, EngineError> {
            if self.deny {
                return Err(EngineError::NotFound);
            }
            Ok(PackageDetails {
                package: self.package.clone(),
                description: "Full synthetic description".into(),
                homepage: None,
                dependencies: vec!["fixture-library".into()],
            })
        }
        fn execute(
            &mut self,
            op: &Operation,
            cancel: &Cancellation,
            progress: &mut dyn FnMut(Progress),
        ) -> Result<OperationOutcome, EngineError> {
            if self.deny {
                return Err(ExecutionError::AuthorizationDenied.into());
            }
            progress(Progress::Message("Native fixture operation".into()));
            match op {
                Operation::Install(_) | Operation::Upgrade(_) | Operation::UpgradeAll { .. } => {
                    self.package.installed_version = self.package.candidate_version.clone();
                    self.package.update = UpdateAvailability::Current;
                }
                Operation::Remove(_) => self.package.installed_version = None,
                Operation::Refresh { .. } => {
                    self.package.candidate_version = Some("2".into());
                    self.package.update = UpdateAvailability::Available;
                    cancel.cancel();
                }
            }
            Ok(OperationOutcome {
                cancellation_deferred: cancel.requested(),
            })
        }
    }
    fn key(app: &mut App, code: KeyCode) -> Option<Job> {
        app.key(KeyEvent::new(code, KeyModifiers::NONE)).1
    }
    fn drive(app: &mut App, engine: &mut Engine, code: KeyCode) {
        if let Some(job) = key(app, code) {
            perform(engine, job, &Cancellation::default(), &mut |r| app.reply(r));
        }
    }
    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect()
    }
    #[test]
    fn keyboard_lifecycle_uses_selected_identity_and_explicit_confirmation() {
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                package: package(),
                deny: false,
            })
            .unwrap();
        let mut app = App::default();
        key(&mut app, KeyCode::Char('/'));
        for c in "synthetic-tooX".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        key(&mut app, KeyCode::Backspace);
        key(&mut app, KeyCode::Char('l'));
        drive(&mut app, &mut engine, KeyCode::Enter);
        assert_eq!(app.query, "synthetic-tool");
        assert!(render(&mut app, 100, 30).contains("synthetic-tool"));
        drive(&mut app, &mut engine, KeyCode::Enter);
        assert!(render(&mut app, 100, 30).contains("Full synthetic description"));
        key(&mut app, KeyCode::Char('i'));
        assert!(render(&mut app, 100, 30).contains("Confirm Install"));
        key(&mut app, KeyCode::Char('n'));
        assert!(engine
            .installed(&Cancellation::default())
            .packages
            .is_empty());
        key(&mut app, KeyCode::Char('i'));
        drive(&mut app, &mut engine, KeyCode::Char('y'));
        assert!(app.status.starts_with("Completed"));
        drive(&mut app, &mut engine, KeyCode::Char('2'));
        assert_eq!(app.packages[0].installed_version.as_deref(), Some("1"));
        drive(&mut app, &mut engine, KeyCode::Char('4'));
        assert!(render(&mut app, 100, 30).contains("Available"));
        key(&mut app, KeyCode::Char('u'));
        drive(&mut app, &mut engine, KeyCode::Char('y'));
        assert!(app.status.contains("not rolled back"));
        drive(&mut app, &mut engine, KeyCode::Char('3'));
        assert_eq!(app.packages.len(), 1);
        key(&mut app, KeyCode::Char('g'));
        drive(&mut app, &mut engine, KeyCode::Char('y'));
        drive(&mut app, &mut engine, KeyCode::Char('3'));
        assert!(app.packages.is_empty());
        drive(&mut app, &mut engine, KeyCode::Char('2'));
        key(&mut app, KeyCode::Char('d'));
        drive(&mut app, &mut engine, KeyCode::Char('y'));
        drive(&mut app, &mut engine, KeyCode::Char('r'));
        assert!(app.packages.is_empty());
    }
    #[test]
    fn long_details_and_errors_scroll_without_emitting_terminal_controls() {
        let mut app = App::default();
        let mut p = package();
        p.summary = "unsafe\x1b[2Jtext".into();
        app.reply(Reply::Packages(PackageReport {
            packages: vec![p],
            failures: vec![],
        }));
        assert!(render(&mut app, 100, 30).contains("unsafe�[2Jtext"));
        app.reply(Reply::Failed("line1\nline2\nline3\nlast line".into()));
        key(&mut app, KeyCode::Char('e'));
        key(&mut app, KeyCode::PageDown);
        assert!(render(&mut app, 40, 5).contains("last line"));
        key(&mut app, KeyCode::PageUp);
        assert!(render(&mut app, 40, 5).contains("line1"));
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('i'));
        key(&mut app, KeyCode::PageDown);
        key(&mut app, KeyCode::PageUp);
        assert!(render(&mut app, 100, 30).contains("synthetic-tool"));
        key(&mut app, KeyCode::Esc);
        assert!(app.confirmation.is_none());
    }

    #[test]
    fn failures_cancellation_navigation_and_small_terminals() {
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                package: package(),
                deny: true,
            })
            .unwrap();
        let mut app = App::default();
        key(&mut app, KeyCode::Char('1'));
        key(&mut app, KeyCode::Enter);
        assert!(app.packages.is_empty());
        key(&mut app, KeyCode::Char('/'));
        key(&mut app, KeyCode::Esc);
        app.query = "synthetic".into();
        drive(&mut app, &mut engine, KeyCode::Char('r'));
        let mut other = package();
        other.id.backend = "other".into();
        app.packages.push(other);
        key(&mut app, KeyCode::Down);
        assert_eq!(app.table.selected(), Some(1));
        key(&mut app, KeyCode::Up);
        assert_eq!(app.table.selected(), Some(0));
        drive(&mut app, &mut engine, KeyCode::Enter);
        assert!(app.status.contains("no package"));
        key(&mut app, KeyCode::Char('i'));
        drive(&mut app, &mut engine, KeyCode::Char('y'));
        assert!(app.status.contains("authorization"));
        app.busy = true;
        key(&mut app, KeyCode::Char('q'));
        assert!(app.cancelled);
        assert!(key(&mut app, KeyCode::Char('y')).is_none());
        app.busy = false;
        let report = PackageReport {
            failures: vec![BackendFailure {
                backend: "fixture".into(),
                error: EngineError::Cancelled,
            }],
            ..Default::default()
        };
        app.reply(Reply::Packages(report));
        assert!(app.status.contains("cancelled"));
        for (w, h) in [(100, 30), (40, 16), (12, 5), (1, 1)] {
            render(&mut app, w, h);
        }
        for code in [KeyCode::Down, KeyCode::Char('d'), KeyCode::Char('z')] {
            key(&mut app, code);
        }
        let mut release = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert!(!app.key(release).0);
        assert!(
            app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
                .0
        );
    }

    #[test]
    fn color_opt_out_keeps_layout_without_chromatic_styles() {
        assert!(colors_allowed(true, Ok("xterm-256color".into())));
        assert!(!colors_allowed(false, Ok("xterm-256color".into())));
        assert!(!colors_allowed(true, Ok("dumb".into())));
        assert!(!colors_allowed(true, Err(std::env::VarError::NotPresent)));
        let styled = Style::default().fg(Color::Cyan);
        assert_eq!(App::default().paint(styled), styled);
        let plain = App {
            color: false,
            ..App::default()
        }
        .paint(styled);
        assert_eq!(plain.fg, None);
        assert_eq!(plain.bg, None);
        let mut app = App {
            color: false,
            ..App::default()
        };
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains("PkgDeck"));
        assert!(screen.contains("cancel/back"));
    }

    #[test]
    fn streaming_partials_keep_selection_on_identity() {
        fn report(names: &[&str]) -> PackageReport {
            PackageReport {
                packages: names
                    .iter()
                    .map(|name| {
                        let mut package = package();
                        package.id.name = (*name).into();
                        package.display_name = (*name).into();
                        package
                    })
                    .collect(),
                failures: vec![],
            }
        }
        let mut app = App::default();
        app.reply(Reply::Packages(report(&["alpha", "beta"])));
        assert_eq!(app.table.selected(), Some(0));
        key(&mut app, KeyCode::Down);
        assert_eq!(app.table.selected(), Some(1));
        // A partial inserting a row above follows the identity, not the index.
        app.reply(Reply::Packages(report(&["aaa", "alpha", "beta"])));
        assert_eq!(app.table.selected(), Some(2));
        assert_eq!(app.packages[2].id.name, "beta");
        // A vanished selection falls back to the first row, never a wrong one.
        app.reply(Reply::Packages(report(&["aaa"])));
        assert_eq!(app.table.selected(), Some(0));
    }

    #[test]
    fn sources_show_humanized_details_and_narrow_footer() {
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                package: package(),
                deny: false,
            })
            .unwrap();
        let mut app = App::default();
        drive(&mut app, &mut engine, KeyCode::Char('4'));
        assert_eq!(app.sources.len(), 1);
        key(&mut app, KeyCode::Down);
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains("Available"));
        assert!(!screen.contains("Ok("));
        assert!(screen.contains("Capabilities:"));
        assert!(!screen.contains("Capabilities: ["));
        let narrow = render(&mut app, 70, 24);
        assert!(narrow.contains("Esc back"));
        assert!(!narrow.contains("cancel/back"));
        let wide = render(&mut app, 120, 30);
        assert!(wide.contains("cancel/back"));
    }
}
