//! What the controller's properties say, read into plain Rust, and the
//! rules that turn them into what the window shows: which rows, in what
//! order, with what words. Nothing here draws, so all of it is tested
//! directly.

use pkgdeck_core::backends;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// The sections of the window.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Page {
    Search,
    Installed,
    Updates,
    Clean,
    Sources,
    Settings,
}
impl Page {
    pub const ALL: [Page; 6] = [
        Page::Search,
        Page::Installed,
        Page::Updates,
        Page::Clean,
        Page::Sources,
        Page::Settings,
    ];
    /// The name the controller and the sidebar use.
    pub fn name(self) -> &'static str {
        match self {
            Page::Search => "Search",
            Page::Installed => "Installed",
            Page::Updates => "Updates",
            Page::Clean => "Clean",
            Page::Sources => "Sources",
            Page::Settings => "Settings",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Page::Search => "search",
            Page::Installed => "installed",
            Page::Updates => "updates",
            Page::Clean => "remove",
            Page::Sources => "sources",
            Page::Settings => "settings",
        }
    }
    /// Whether this page shows a list of rows.
    pub fn lists(self) -> bool {
        self != Page::Settings
    }
    /// What a source must be able to do to take part in this page.
    pub fn capability(self) -> Option<&'static str> {
        match self {
            Page::Search => Some("search"),
            Page::Installed => Some("installed"),
            Page::Updates => Some("upgrade"),
            Page::Clean => Some("clean"),
            _ => None,
        }
    }
    /// Pages with a source filter.
    pub fn filters_sources(self) -> bool {
        self.capability().is_some()
    }
}

/// One row of the controller's `rows`: a package, a source, a cleanup task
/// or a source that failed.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Row {
    pub kind: String,
    pub name: String,
    pub display_name: String,
    pub source: String,
    pub architecture: String,
    pub remote: Option<String>,
    pub reference: Option<String>,
    pub scope: Value,
    pub scope_label: String,
    pub summary: String,
    /// Installed version; `Some("")` still means installed.
    pub installed: Option<String>,
    pub candidate: Option<String>,
    pub update: String,
    pub icon: Option<String>,
    pub same_app_from: Vec<String>,
    pub same_app_group: Option<String>,
    pub adopt_with: Option<String>,
    pub failure_kind: String,
    pub cleanup_key: String,
    pub cleanup_kind: String,
    pub preview: String,
    pub available: Option<bool>,
    pub availability_kind: String,
    pub check_failed: bool,
    pub capabilities: Vec<String>,
}

impl Row {
    pub fn is_package(&self) -> bool {
        self.kind == "package"
    }
    pub fn is_installed(&self) -> bool {
        self.installed.is_some()
    }
    pub fn has_update(&self) -> bool {
        self.update == "available"
    }
    /// The name people know it by.
    pub fn title(&self) -> &str {
        if self.kind == "source" {
            return backends::display_name(&self.source);
        }
        if self.display_name.trim().is_empty() {
            &self.name
        } else {
            &self.display_name
        }
    }
    /// What identifies a row across loads, as the controller reads it back
    /// in `propose_checked`.
    pub fn identity(&self) -> String {
        identity_of(&self.identity_value())
    }
    pub fn identity_value(&self) -> Value {
        json!([
            self.source,
            self.name,
            self.architecture,
            self.remote,
            self.scope,
            self.reference
        ])
    }
    /// A row that only a search found, with no version either way.
    pub fn fabricated(&self) -> bool {
        self.is_package() && self.installed.is_none() && self.candidate.is_none()
    }
    pub fn is_flatpak_system(&self) -> bool {
        self.scope == Value::String("system".into())
    }
}

/// The identity of an id-shaped object (a notice's `target`, a progress
/// row), which carries the same fields as a row.
pub fn identity_of_row_value(value: &Value) -> String {
    let field = |key: &str| value.get(key).cloned().unwrap_or(Value::Null);
    let text = |key: &str| match field(key) {
        Value::String(text) if !text.is_empty() => Value::String(text),
        _ => Value::Null,
    };
    identity_of(&json!([
        field("source"),
        field("name"),
        field("architecture"),
        text("remote"),
        field("scope"),
        text("reference")
    ]))
}

fn identity_of(value: &Value) -> String {
    // Empty remotes and references count as none, as they do in the QML.
    let mut value = value.clone();
    for index in [3, 5] {
        if value.get(index).and_then(Value::as_str) == Some("") {
            value[index] = Value::Null;
        }
    }
    value.to_string()
}

/// Parse a JSON property, or the default when it's empty or broken. A
/// field that is `null` reads as missing, so it takes its default.
pub fn parse<T: for<'de> Deserialize<'de> + Default>(text: &str) -> T {
    fn without_nulls(value: &mut Value) {
        match value {
            Value::Object(map) => {
                map.retain(|_, field| !field.is_null());
                map.values_mut().for_each(without_nulls);
            }
            Value::Array(items) => items.iter_mut().for_each(without_nulls),
            _ => {}
        }
    }
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return T::default();
    };
    without_nulls(&mut value);
    serde_json::from_value(value).unwrap_or_default()
}

/// A source as `source_catalog` describes it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Source {
    pub source: String,
    pub name: String,
    pub summary: String,
    pub available: bool,
    pub availability_kind: String,
    pub check_failed: bool,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Failure {
    pub source: String,
    pub kind: String,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ReportState {
    pub phase: String,
    pub failures: Vec<Failure>,
    pub successful_sources: Vec<String>,
    pub last_success: HashMap<String, i64>,
    pub checked_at: Option<i64>,
    pub matches: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Screenshot {
    pub url: String,
    pub caption: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Cleanup {
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub preview: String,
}

/// `details`, and the `opened` page, which has the same fields.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Details {
    pub package: Option<Row>,
    pub description: String,
    pub homepage: String,
    pub publisher: String,
    pub license: String,
    pub dependencies: Vec<String>,
    pub screenshots: Option<Vec<Screenshot>>,
    pub more: bool,
    pub location: String,
    pub action: String,
    pub cleanup: Option<Cleanup>,
    pub failure: Option<Value>,
    pub hint: String,
    pub source: Option<String>,
    pub availability: String,
}
impl Details {
    /// The text the panel shows for a source, failure or cleanup.
    pub fn panel_text(&self, report: &ReportState) -> String {
        if let Some(cleanup) = &self.cleanup {
            return format!("{}\n\n{}", cleanup.summary, cleanup.preview)
                .trim()
                .to_owned();
        }
        if let Some(failure) = &self.failure {
            let error = failure.get("error").and_then(Value::as_str).unwrap_or("");
            return format!("{error}\n{}", self.hint).trim().to_owned();
        }
        if let Some(source) = &self.source {
            let when = report
                .last_success
                .get(source)
                .map(|epoch| short_datetime(*epoch))
                .unwrap_or_else(|| "No successful check yet".into());
            return format!("{}\nLast successful check: {when}", self.availability);
        }
        String::new()
    }
}

/// `notice`: what a change left to say.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Notice {
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub output: String,
    pub retry: bool,
    pub operation: String,
    pub target: Option<Value>,
    pub undo: bool,
    pub undo_action: String,
}
impl Notice {
    pub fn is_empty(&self) -> bool {
        self.title.is_empty() && self.kind.is_empty()
    }
    /// Success and information go in a toast; the rest in a banner.
    pub fn is_toast(&self) -> bool {
        matches!(self.kind.as_str(), "success" | "info")
    }
    pub fn can_undo(&self) -> bool {
        self.undo
            && matches!(self.undo_action.as_str(), "install" | "remove")
            && self.target.is_some()
    }
}

/// `progress`: the running change.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Progress {
    pub activity_id: Option<u64>,
    pub label: String,
    pub done: u64,
    pub total: u64,
    pub transferred: u64,
    pub transfer_total: Option<u64>,
    pub fraction: Option<f64>,
    pub targets: Vec<Value>,
    pub sources: Vec<String>,
    pub action: String,
    pub current: Option<Value>,
    pub finished: Vec<Value>,
    pub current_source: Option<String>,
}
impl Progress {
    /// How far along, 0 to 1, when that's known.
    pub fn bar(&self) -> Option<f32> {
        match self.transfer_total {
            Some(total) if total > 0 => {
                return Some((self.transferred as f32 / total as f32).clamp(0.0, 1.0))
            }
            _ => {}
        }
        if self.total > 1 {
            return Some((self.done as f32 / self.total as f32).clamp(0.0, 1.0));
        }
        self.fraction
            .map(|fraction| fraction.clamp(0.0, 1.0) as f32)
    }
    /// "45%" or "2 of 5".
    pub fn count(&self) -> String {
        match self.transfer_total {
            Some(total) if total > 0 => format!(
                "{}%",
                (self.transferred as f64 * 100.0 / total as f64)
                    .round()
                    .min(100.0)
            ),
            _ if self.total > 1 => format!("{} of {}", self.done, self.total),
            _ => String::new(),
        }
    }
    /// A row's own progress: `None` when the row isn't part of the change,
    /// `Some(None)` when it runs without a known amount.
    pub fn for_row(&self, identity: &str, source: &str) -> Option<Option<f32>> {
        let has = |list: &[Value]| list.iter().any(|id| identity_of_row_value(id) == identity);
        if !has(&self.targets) {
            return None;
        }
        if self.total <= 1 {
            return Some(self.fraction.map(|f| f as f32).or(self.bar()));
        }
        if has(&self.finished) {
            return Some(Some(1.0));
        }
        let running = match &self.current {
            Some(current) => identity_of_row_value(current) == identity,
            None => self.current_source.as_deref() == Some(source),
        };
        if running {
            let transfer = match self.transfer_total {
                Some(total) if total > 0 => Some(self.transferred as f32 / total as f32),
                _ => None,
            };
            return Some(transfer);
        }
        Some(Some(0.0))
    }
}

/// One entry of `activity`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Activity {
    pub id: u64,
    pub frontend: String,
    pub operations: Vec<Value>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub state: String,
    pub outcomes: Vec<Value>,
    pub labels: Vec<String>,
    pub log: Value,
    pub output: Value,
}
impl Activity {
    /// What it changed, in people's words.
    pub fn title(&self) -> String {
        self.labels
            .first()
            .cloned()
            .or_else(|| self.operations.first().map(operation_text))
            .unwrap_or_else(|| "Change".into())
    }
    pub fn others(&self) -> Vec<String> {
        if self.labels.len() > 1 {
            return self.labels[1..].to_vec();
        }
        self.operations.iter().skip(1).map(operation_text).collect()
    }
    fn outcome_counts(&self) -> (usize, usize, usize) {
        let mut counts = (0, 0, 0);
        for outcome in &self.outcomes {
            let text = outcome.to_string().to_lowercase();
            if text.contains("fail") || text.contains("error") {
                counts.1 += 1;
            } else if text.contains("cancel") {
                counts.2 += 1;
            } else {
                counts.0 += 1;
            }
        }
        counts
    }
    pub fn result(&self) -> String {
        let (done, failed, cancelled) = self.outcome_counts();
        match self.outcomes.len() {
            0 => match self.state.as_str() {
                "queued" => "Queued",
                "running" | "authorizing" => "In progress",
                "finished" => "Completed",
                "failed" => "Failed",
                "cancelled" => "Cancelled",
                "interrupted" => "Interrupted",
                other => other,
            }
            .to_owned(),
            1 if failed == 1 => "Failed".into(),
            1 if cancelled == 1 => "Cancelled".into(),
            1 => "Completed".into(),
            total if done == total => format!("All {total} completed"),
            _ => [
                (done, "completed"),
                (failed, "failed"),
                (cancelled, "cancelled"),
            ]
            .into_iter()
            .filter(|(count, _)| *count > 0)
            .map(|(count, word)| format!("{count} {word}"))
            .collect::<Vec<_>>()
            .join(", "),
        }
    }
    pub fn tone(&self) -> Tone {
        let (_, failed, _) = self.outcome_counts();
        match self.state.as_str() {
            _ if failed > 0 || self.state == "failed" => Tone::Danger,
            "cancelled" | "interrupted" | "queued" => Tone::Muted,
            "running" | "authorizing" => Tone::Accent,
            _ => Tone::Success,
        }
    }
    pub fn running(&self) -> bool {
        matches!(self.state.as_str(), "running" | "authorizing")
    }
    pub fn log_text(&self) -> String {
        let text = |value: &Value| match value {
            Value::String(text) => text.clone(),
            Value::Array(lines) => lines
                .iter()
                .map(|line| {
                    line.as_str()
                        .map_or_else(|| line.to_string(), str::to_owned)
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        let log = text(&self.log);
        if log.is_empty() {
            text(&self.output)
        } else {
            log
        }
    }
}

/// "Install Firefox (Flatpak, System)": an operation as the drawer names it.
pub fn operation_text(operation: &Value) -> String {
    let Some((kind, id)) = operation.as_object().and_then(|map| map.iter().next()) else {
        return "Change".into();
    };
    let verb = match kind.as_str() {
        "install" => "Install",
        "remove" => "Remove",
        "upgrade" => "Update",
        "upgrade_all" => "Update all",
        "refresh" => "Refresh",
        "clean" => "Clean",
        _ => "Change",
    };
    let name = id
        .get("name")
        .or_else(|| id.get("key"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let backend = id.get("backend").and_then(Value::as_str).unwrap_or("");
    let mut place = vec![];
    if !backend.is_empty() {
        place.push(backends::display_name(backend).to_owned());
    }
    match id.get("scope") {
        Some(Value::String(scope)) if scope == "system" => place.push("System".into()),
        Some(Value::Object(scope)) if scope.contains_key("user") => place.push("User".into()),
        Some(Value::Object(scope)) => {
            if let Some(path) = scope
                .get("environment")
                .and_then(|env| env.get("path"))
                .and_then(Value::as_str)
            {
                place.push(path.into());
            }
        }
        _ => {}
    }
    let mut text = verb.to_owned();
    if !name.is_empty() {
        text = format!("{text} {name}");
    }
    if !place.is_empty() {
        text = format!("{text} ({})", place.join(", "));
    }
    text
}

/// `confirmation_data`: the change that waits for a yes.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Confirmation {
    pub action: String,
    pub body: String,
    pub summary: String,
    pub details: String,
    pub flatpak_ref_scope: String,
    pub icon: Option<String>,
    pub review: Option<bool>,
    pub notes: Vec<String>,
    pub changes: Vec<String>,
}
impl Confirmation {
    /// The apply button's word: "Install", "Remove", "Update"…
    pub fn verb(&self) -> String {
        self.action
            .split_whitespace()
            .next()
            .unwrap_or("Apply")
            .trim_end_matches(['?', '…'])
            .to_owned()
    }
    pub fn removes(&self) -> bool {
        self.verb() == "Remove"
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Repository {
    pub backend: String,
    pub name: String,
    pub title: String,
    pub url: String,
    pub scope: String,
    pub enabled: bool,
    pub priority: Option<i64>,
    pub source_name: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct RepositoryFeatures {
    pub flatpak_user: bool,
    pub flatpak_system: bool,
    pub apt_editor: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Repositories {
    pub repositories: Vec<Repository>,
    pub errors: Vec<String>,
    pub features: RepositoryFeatures,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct BackgroundState {
    pub last_check: Option<i64>,
    pub available: u64,
    pub failures: Vec<Failure>,
    pub notify: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct AutoUpdateResult {
    pub updated: u64,
    pub failed: u64,
    pub total: u64,
}
impl AutoUpdateResult {
    pub fn message(&self) -> String {
        let installed = match self.updated {
            0 => "No updates installed".to_owned(),
            1 => "Installed 1 update".to_owned(),
            count => format!("Installed {count} updates"),
        };
        match self.failed {
            0 => installed,
            failed => format!("{installed}, {failed} need your attention"),
        }
    }
}

/// An installed AppImage's file, from `app_file`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppFile {
    pub path: String,
    pub bytes: Option<u64>,
    pub modified: Option<i64>,
    pub managed: bool,
    pub without_fuse: bool,
    pub folder: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Variable {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct LaunchSettings {
    pub arguments: String,
    pub environment: Vec<Variable>,
    pub editable: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct UpdateSource {
    pub github: Option<String>,
    pub builtin: bool,
    pub editable: bool,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Words

pub fn source_name(id: &str) -> &str {
    backends::display_name(id)
}

fn simplified(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// "Flatpak, flathub, System": where a row comes from.
pub fn source_line(row: &Row, merged_scopes: bool) -> String {
    let mut parts: Vec<String> = vec![];
    let names_differ = !row.display_name.is_empty()
        && simplified(&row.display_name) != simplified(&row.name)
        && !matches!(
            row.source.as_str(),
            "flatpak" | "macos-apps" | "mas" | "appimage"
        )
        // A standalone tool is named after its source already.
        && source_name(&row.source) != row.display_name;
    if names_differ && row.is_package() {
        parts.push(row.name.clone());
    }
    parts.push(source_name(&row.source).to_owned());
    if let Some(remote) = row.remote.as_deref().filter(|r| !r.is_empty()) {
        parts.push(remote.to_owned());
    }
    if merged_scopes {
        parts.push("System and user".into());
    } else if matches!(row.source.as_str(), "flatpak" | "docker" | "podman") {
        parts.push(
            if row.is_flatpak_system() {
                "System"
            } else {
                "User"
            }
            .into(),
        );
    } else if !row.scope_label.is_empty() && row.kind == "package" {
        parts.push(row.scope_label.clone());
    }
    if row.source == "flatpak" {
        if let Some(branch) = row
            .reference
            .as_deref()
            .and_then(|reference| reference.rsplit('/').next())
            .filter(|branch| !branch.is_empty() && *branch != "stable")
        {
            parts.push(branch.to_owned());
        }
    }
    if row.adopt_with.as_deref() == Some("appimage") && row.source == "appimage" {
        parts.push("not managed".into());
    }
    parts.dedup();
    parts.retain(|part| !part.is_empty());
    parts.join(", ")
}

pub fn cleanup_type(kind: &str) -> &'static str {
    match kind {
        "orphan_dependencies" => "Dependencies",
        "duplicate_copy" => "Extra copy",
        _ => "Cache",
    }
}

/// The version column's text.
pub fn version_text(row: &Row) -> String {
    let known = |value: &Option<String>| value.clone().filter(|v| !v.is_empty());
    match row.kind.as_str() {
        "cleanup" => cleanup_type(&row.cleanup_kind).into(),
        "failure" => "Failed".into(),
        "source" => if row.available.unwrap_or(false) {
            "Available"
        } else {
            "Unavailable"
        }
        .into(),
        _ if row.has_update() => match (known(&row.installed), known(&row.candidate)) {
            (Some(old), Some(new)) if old != new => format!("{old} → {new}"),
            (_, Some(new)) => new,
            (Some(old), None) => old,
            _ => "Unknown".into(),
        },
        _ if row.is_installed() => known(&row.installed).unwrap_or_else(|| "Unknown".into()),
        _ => known(&row.candidate).unwrap_or_else(|| "Unknown".into()),
    }
}

/// What the catalog says about a source; missing ones are still being
/// checked, or don't run here.
pub fn catalog_entry(catalog: &[Source], id: &str) -> Source {
    catalog
        .iter()
        .find(|source| source.source == id)
        .cloned()
        .unwrap_or_else(|| Source {
            source: id.into(),
            name: source_name(id).into(),
            summary: if catalog.is_empty() {
                "Checking availability…".into()
            } else {
                "Unsupported on this platform".into()
            },
            availability_kind: if catalog.is_empty() {
                "checking".into()
            } else {
                "platform".into()
            },
            ..Source::default()
        })
}

pub fn can_remove(row: &Row, catalog: &[Source]) -> bool {
    row.is_package()
        && row.is_installed()
        && ((row.source != "macos-apps" && !backends::never_installs(&row.source))
            || catalog_entry(catalog, &row.source)
                .capabilities
                .iter()
                .any(|capability| capability == "remove"))
}

pub fn can_adopt(row: &Row, catalog: &[Source]) -> bool {
    let Some(with) = row.adopt_with.as_deref() else {
        return false;
    };
    if !row.is_package() {
        return false;
    }
    match row.source.as_str() {
        "appimage" => with == "appimage",
        "macos-apps" => {
            cfg!(target_os = "macos") && catalog_entry(catalog, "homebrew-cask").available
        }
        _ => true,
    }
}

/// What a row's button does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Install,
    Remove,
    Upgrade,
    Clean,
    Adopt,
}
impl Action {
    /// The word `propose` takes.
    pub fn key(self) -> &'static str {
        match self {
            Action::Install => "install",
            Action::Remove => "remove",
            Action::Upgrade => "upgrade",
            Action::Clean => "clean",
            Action::Adopt => "adopt",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Action::Install => "Install",
            Action::Remove => "Remove",
            Action::Upgrade => "Update",
            Action::Clean => "Clean",
            Action::Adopt => "Manage",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Action::Upgrade => "updates",
            Action::Install | Action::Adopt => "install",
            Action::Remove | Action::Clean => "remove",
        }
    }
    pub fn tone(self) -> Tone {
        match self {
            Action::Upgrade | Action::Adopt => Tone::Accent,
            Action::Install => Tone::Success,
            Action::Remove | Action::Clean => Tone::Danger,
        }
    }
}

pub fn row_action(row: &Row, page: Page, catalog: &[Source]) -> Option<Action> {
    if row.kind == "cleanup" {
        return Some(Action::Clean);
    }
    if !row.is_package() {
        return None;
    }
    if row.source == "macos-apps" {
        return (can_adopt(row, catalog).then_some(Action::Adopt))
            .or_else(|| can_remove(row, catalog).then_some(Action::Remove));
    }
    if backends::never_installs(&row.source) {
        return if row.has_update() {
            Some(Action::Upgrade)
        } else if can_remove(row, catalog) {
            Some(Action::Remove)
        } else {
            None
        };
    }
    if page == Page::Updates {
        Some(Action::Upgrade)
    } else if row.is_installed() {
        Some(Action::Remove)
    } else {
        Some(Action::Install)
    }
}

/// A colour role, which the theme turns into a colour.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tone {
    Accent,
    Success,
    Danger,
    Warning,
    Muted,
}

// ---------------------------------------------------------------------------
// Which rows, in what order

/// A visible row: where it is in `rows`, the other Flatpak copies it
/// stands for, and the heading of the same-app group it starts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub raw: usize,
    /// Raw indexes of the System and User copies, System first, when this
    /// row stands for both.
    pub variants: Vec<usize>,
    pub group: Option<GroupHeading>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroupHeading {
    pub title: String,
    pub sources: Vec<String>,
}

/// How the list is sorted, when the person chose a column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Column {
    Name,
    Version,
    Summary,
}
impl Column {
    pub fn key(self) -> &'static str {
        match self {
            Column::Name => "name",
            Column::Version => "version",
            Column::Summary => "summary",
        }
    }
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "name" => Some(Column::Name),
            "version" => Some(Column::Version),
            "summary" | "status" => Some(Column::Summary),
            _ => None,
        }
    }
}

/// Everything the visible rows depend on besides the rows themselves.
#[derive(Clone, Debug, Default)]
pub struct ViewOptions<'a> {
    pub page: Option<Page>,
    pub query: &'a str,
    pub sources: Option<&'a HashSet<String>>,
    pub filter: &'a str,
    pub duplicates_only: bool,
    pub show_unavailable: bool,
    pub sort: Option<(Column, bool)>,
    pub scope_choices: Option<&'a HashMap<String, String>>,
}

/// How well a row matches a search: lower is better.
pub fn relevance(row: &Row, query: &str) -> u32 {
    let q = query.trim().to_lowercase();
    let name = row.name.to_lowercase();
    let display = row.display_name.to_lowercase();
    let summary = row.summary.to_lowercase();
    let last_segment = name.rsplit('.').next().unwrap_or(&name);
    let score = if name == q || display == q || (row.source == "flatpak" && last_segment == q) {
        0
    } else if name.starts_with(&q) || display.starts_with(&q) {
        1
    } else if name.contains(&q) || display.contains(&q) {
        2
    } else if summary.starts_with(&q) {
        3
    } else if summary.contains(&q) {
        4
    } else {
        5
    };
    score + if row.fabricated() { 6 } else { 0 }
}

fn filter_matches(row: &Row, filter: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    filter.is_empty()
        || format!(
            "{} {} {} {}",
            row.name, row.display_name, row.summary, row.source
        )
        .to_lowercase()
        .contains(&filter)
}

fn sort_key(row: &Row, column: Column) -> String {
    match column {
        Column::Name => row.title().to_lowercase(),
        Column::Version => row
            .installed
            .clone()
            .filter(|v| !v.is_empty())
            .or_else(|| row.candidate.clone())
            .unwrap_or_default()
            .to_lowercase(),
        Column::Summary => {
            if row.kind == "source" {
                row.available.unwrap_or(false).to_string()
            } else {
                row.summary.to_lowercase()
            }
        }
    }
}

/// The Flatpak group a row merges into: the same app from the same remote
/// and branch, in another scope.
pub fn flatpak_group(row: &Row) -> Option<String> {
    (row.source == "flatpak" && row.is_package()).then(|| {
        json!([
            row.name,
            row.architecture,
            row.remote.clone().unwrap_or_default(),
            row.reference.clone().unwrap_or_default()
        ])
        .to_string()
    })
}

/// The rows the list shows, in order.
pub fn visible(rows: &[Row], options: &ViewOptions) -> Vec<Item> {
    let page = options.page;
    let mut kept: Vec<usize> = (0..rows.len())
        .filter(|&index| {
            let row = &rows[index];
            if row.kind == "failure" {
                return false;
            }
            match page {
                Some(Page::Search) => !row.fabricated(),
                Some(Page::Clean) => row.kind == "cleanup",
                Some(Page::Sources) => {
                    row.kind == "source"
                        && (options.show_unavailable || row.available.unwrap_or(false))
                }
                _ => true,
            }
        })
        .filter(|&index| {
            page == Some(Page::Sources)
                || options
                    .sources
                    .is_none_or(|sources| sources.contains(&rows[index].source))
        })
        .collect();
    if page == Some(Page::Installed) {
        let groups_matching: HashSet<&str> = kept
            .iter()
            .map(|&index| &rows[index])
            .filter(|row| filter_matches(row, options.filter))
            .filter_map(|row| row.same_app_group.as_deref())
            .collect();
        kept.retain(|&index| {
            let row = &rows[index];
            (filter_matches(row, options.filter)
                || row
                    .same_app_group
                    .as_deref()
                    .is_some_and(|group| groups_matching.contains(group)))
                && (!options.duplicates_only || !row.same_app_from.is_empty())
        });
    }
    let by_name = |a: &Row, b: &Row| {
        a.title()
            .to_lowercase()
            .cmp(&b.title().to_lowercase())
            .then_with(|| a.source.cmp(&b.source))
    };
    if let Some((column, ascending)) = options.sort {
        kept.sort_by(|&a, &b| {
            let (a, b) = (&rows[a], &rows[b]);
            let order = sort_key(a, column)
                .cmp(&sort_key(b, column))
                .then_with(|| by_name(a, b));
            if ascending {
                order
            } else {
                order.reverse()
            }
        });
    } else if page == Some(Page::Sources) {
        // Enabled managers first, so the ones in use are easy to find.
        let disabled = |row: &Row| options.sources.is_some_and(|s| !s.contains(&row.source));
        kept.sort_by(|&a, &b| {
            let (a, b) = (&rows[a], &rows[b]);
            disabled(a).cmp(&disabled(b)).then_with(|| by_name(a, b))
        });
    } else if page == Some(Page::Search) && !options.query.trim().is_empty() {
        kept.sort_by(|&a, &b| {
            relevance(&rows[a], options.query)
                .cmp(&relevance(&rows[b], options.query))
                .then_with(|| by_name(&rows[a], &rows[b]))
        });
    }
    let mut items: Vec<Item> = kept
        .into_iter()
        .map(|raw| Item {
            raw,
            ..Item::default()
        })
        .collect();
    if matches!(page, Some(Page::Search) | Some(Page::Installed)) {
        items = merge_flatpak_scopes(rows, items, options.scope_choices);
    }
    let group = page == Some(Page::Installed)
        || (page == Some(Page::Search)
            && !options.query.trim().is_empty()
            && options.sort.is_none());
    if group {
        items = group_same_apps(rows, items);
    }
    items
}

/// One row for a Flatpak installed both for the system and the user.
fn merge_flatpak_scopes(
    rows: &[Row],
    items: Vec<Item>,
    choices: Option<&HashMap<String, String>>,
) -> Vec<Item> {
    let mut members: HashMap<String, Vec<usize>> = HashMap::new();
    for item in &items {
        if let Some(group) = flatpak_group(&rows[item.raw]) {
            members.entry(group).or_default().push(item.raw);
        }
    }
    let mut done: HashSet<String> = HashSet::new();
    let mut merged = vec![];
    for item in items {
        let Some(group) = flatpak_group(&rows[item.raw]) else {
            merged.push(item);
            continue;
        };
        let copies = &members[&group];
        let mixed = copies.iter().any(|&raw| rows[raw].is_flatpak_system())
            && copies.iter().any(|&raw| !rows[raw].is_flatpak_system());
        if !mixed {
            merged.push(item);
            continue;
        }
        if !done.insert(group.clone()) {
            continue;
        }
        let mut variants = copies.clone();
        variants.sort_by_key(|&raw| !rows[raw].is_flatpak_system());
        let chosen = choices
            .and_then(|choices| choices.get(&group))
            .and_then(|identity| {
                variants
                    .iter()
                    .find(|&&raw| rows[raw].identity() == *identity)
            })
            .or_else(|| variants.iter().find(|&&raw| rows[raw].is_installed()))
            .copied()
            .unwrap_or(variants[0]);
        merged.push(Item {
            raw: chosen,
            variants,
            group: None,
        });
    }
    merged
}

/// Rows of the same app from different sources, together, under a heading.
fn group_same_apps(rows: &[Row], items: Vec<Item>) -> Vec<Item> {
    let mut members: HashMap<&str, Vec<usize>> = HashMap::new();
    for (position, item) in items.iter().enumerate() {
        if let Some(group) = rows[item.raw].same_app_group.as_deref() {
            members.entry(group).or_default().push(position);
        }
    }
    let mut placed = vec![false; items.len()];
    let mut grouped = Vec::with_capacity(items.len());
    for position in 0..items.len() {
        if placed[position] {
            continue;
        }
        let group = rows[items[position].raw].same_app_group.as_deref();
        match group
            .and_then(|group| members.get(group))
            .filter(|m| m.len() >= 2)
        {
            Some(positions) => {
                let first = &rows[items[positions[0]].raw];
                let title = if first.display_name.is_empty() {
                    positions
                        .iter()
                        .map(|&p| rows[items[p].raw].name.as_str())
                        .min_by_key(|name| name.len())
                        .unwrap_or_default()
                        .to_owned()
                } else {
                    first.display_name.clone()
                };
                let mut sources: Vec<String> = vec![];
                for &p in positions {
                    let name = source_name(&rows[items[p].raw].source).to_owned();
                    if !sources.contains(&name) {
                        sources.push(name);
                    }
                }
                for (n, &p) in positions.iter().enumerate() {
                    placed[p] = true;
                    let mut item = items[p].clone();
                    item.group = (n == 0).then(|| GroupHeading {
                        title: title.clone(),
                        sources: sources.clone(),
                    });
                    grouped.push(item);
                }
            }
            None => {
                placed[position] = true;
                grouped.push(items[position].clone());
            }
        }
    }
    grouped
}

// ---------------------------------------------------------------------------
// Sources

/// Every source id, the known ones first in their usual order.
pub fn all_sources(catalog: &[Source]) -> Vec<String> {
    let mut ids: Vec<String> = backends::BACKEND_IDS
        .iter()
        .map(|id| (*id).to_owned())
        .collect();
    for source in catalog {
        if !ids.contains(&source.source) {
            ids.push(source.source.clone());
        }
    }
    ids
}

pub fn category(id: &str) -> &'static str {
    match id {
        "apt" | "dnf" | "pacman" | "aur" | "zypper" | "apk" | "xbps" | "macports"
        | "macos-updates" | "system-image" | "fwupd" => "System",
        "snap" | "homebrew" | "homebrew-cask" | "macos-apps" | "mas" | "appimage" | "flatpak" => {
            "Applications"
        }
        "docker" | "podman" => "Containers",
        _ => "Developer tools",
    }
}
pub const CATEGORIES: [&str; 4] = ["System", "Applications", "Developer tools", "Containers"];

#[cfg(target_os = "linux")]
const AWAITING_CATALOG: &[&str] = &["AppImage", "appimage"];
#[cfg(not(target_os = "linux"))]
const AWAITING_CATALOG: &[&str] = &[];

/// File name patterns the open dialog offers, by the sources available.
pub fn file_patterns(catalog: &[Source]) -> Vec<&'static str> {
    if catalog.is_empty() {
        // Before the catalog arrives, Linux still offers AppImages.
        return AWAITING_CATALOG.to_vec();
    }
    let available = |id: &str| catalog.iter().any(|s| s.source == id && s.available);
    let mut patterns = vec![];
    if available("appimage") {
        patterns.extend(["AppImage", "appimage"]);
    }
    if available("apt") {
        patterns.extend(["deb", "sources", "list"]);
    }
    if available("dnf") || available("zypper") {
        patterns.extend(["rpm", "repo"]);
    }
    if available("pacman") {
        patterns.extend(["zst", "xz", "gz", "bz2", "lz4"]);
    }
    if available("flatpak") {
        patterns.extend(["flatpak", "flatpakref", "flatpakrepo"]);
    }
    if available("snap") {
        patterns.push("snap");
    }
    if available("zypper") {
        patterns.push("ymp");
    }
    patterns
}

/// The line under the empty Search page.
pub fn search_hint(catalog: &[Source], enabled: &HashSet<String>, saved: &str) -> String {
    if catalog.is_empty() {
        return if saved.is_empty() {
            "Searches your sources as you type.".into()
        } else {
            saved.into()
        };
    }
    let searchable: Vec<&Source> = catalog
        .iter()
        .filter(|s| {
            s.available
                && enabled.contains(&s.source)
                && s.capabilities.iter().any(|c| c == "search")
        })
        .collect();
    match searchable.as_slice() {
        [] => "No enabled source can search. Turn one on in Sources.".into(),
        [one] => format!("Searches {} as you type.", source_name(&one.source)),
        many => format!("Searches {} sources as you type.", many.len()),
    }
}

/// "Couldn't check Snap", or several, or apps that couldn't be read.
pub fn failure_title(failures: &[Failure]) -> String {
    if !failures.is_empty() && failures.iter().all(|f| f.kind == "partial") {
        return "Some apps couldn't be read".into();
    }
    match failures {
        [one] => format!("Couldn't check {}", source_name(&one.source)),
        many => format!("Couldn't check {} sources", many.len()),
    }
}

// ---------------------------------------------------------------------------
// Time

fn local(epoch: i64) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(epoch, 0).single()
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// "Oct 8, 14:05".
pub fn short_datetime(epoch: i64) -> String {
    local(epoch)
        .map(|time| time.format("%b %-d, %H:%M").to_string())
        .unwrap_or_default()
}

/// The time alone when it was today, else the date too.
pub fn time_or_date(epoch: i64, now: i64) -> String {
    match (local(epoch), local(now)) {
        (Some(time), Some(today)) if time.date_naive() == today.date_naive() => {
            time.format("%H:%M").to_string()
        }
        _ => short_datetime(epoch),
    }
}

/// "Checked just now", "Checked 5 minutes ago"…
pub fn checked_ago(epoch: i64, now: i64) -> String {
    let seconds = (now - epoch).max(0);
    match seconds {
        0..60 => "Checked just now".into(),
        60..3600 => {
            plural(seconds / 60, "minute").map_or_else(String::new, |t| format!("Checked {t} ago"))
        }
        3600..86400 => {
            plural(seconds / 3600, "hour").map_or_else(String::new, |t| format!("Checked {t} ago"))
        }
        _ => format!("Checked {}", short_datetime(epoch)),
    }
}

fn plural(count: i64, noun: &str) -> Option<String> {
    Some(match count {
        1 => format!("1 {noun}"),
        count => format!("{count} {noun}s"),
    })
}

/// "42 s", "3 min 5 s", "1 h 2 min".
pub fn duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} min {} s", seconds / 60, seconds % 60),
        _ => format!("{} h {} min", seconds / 3600, (seconds % 3600) / 60),
    }
}

/// Bytes in base 1000: "820 bytes", "4.2 MB", "31 MB".
pub fn size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64;
    let mut unit = "bytes";
    for next in ["KB", "MB", "GB", "TB"] {
        if value < 1000.0 {
            break;
        }
        value /= 1000.0;
        unit = next;
    }
    if value < 10.0 {
        format!("{value:.1} {unit}")
    } else {
        format!("{value:.0} {unit}")
    }
}

/// How often background checks run, and how each is said.
pub const INTERVALS: [(i32, &str); 10] = [
    (15, "15 minutes"),
    (30, "30 minutes"),
    (60, "1 hour"),
    (180, "3 hours"),
    (360, "6 hours"),
    (720, "12 hours"),
    (1440, "1 day"),
    (2880, "2 days"),
    (4320, "3 days"),
    (10080, "1 week"),
];
pub fn nearest_interval(minutes: i32) -> usize {
    INTERVALS
        .iter()
        .enumerate()
        .min_by_key(|(_, (step, _))| (step - minutes).abs())
        .map(|(index, _)| index)
        .unwrap_or(1)
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
