//! Human-readable command-line presentation.
use pkgdeck_core::{
    engine::EngineError,
    package::{Operation, PackageId, Scope},
};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

pub fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
/// One-cell markers that read well with or without color.
pub fn package_marker(installed: bool, update: bool) -> &'static str {
    if update {
        "↑"
    } else if installed {
        "●"
    } else {
        "○"
    }
}
fn target(id: &PackageId) -> String {
    clean(id.reference.as_deref().unwrap_or(&id.name))
}
/// The name people know a source by ("APT", "Flatpak"), for sentences.
pub fn source_name(backend: &str) -> String {
    clean(pkgdeck_core::backends::display_name(backend))
}
/// "all APT packages", "all Docker images", "all firmware".
fn everything_from(backend: &str) -> String {
    let name = source_name(backend);
    match backend {
        "fwupd" => "all firmware".into(),
        _ if name.ends_with(" images") || name.ends_with(" Casks") => format!("all {name}"),
        _ => format!("all {name} packages"),
    }
}
/// Short imperative title for a planned change, such as "Install neovim".
pub fn operation_title(op: &Operation) -> String {
    match op {
        Operation::Refresh { backend } => format!("Refresh {}", source_name(backend)),
        Operation::UpgradeAll { backend } => format!("Update {}", everything_from(backend)),
        Operation::Clean(id) => format!("Clean up {}", clean(&id.key)),
        Operation::Install(id) => format!("Install {}", target(id)),
        Operation::Remove(id) => format!("Remove {}", target(id)),
        Operation::Upgrade(id) => format!("Update {}", target(id)),
    }
}
/// What PkgDeck is doing right now, for the spinner.
pub fn operation_progress(op: &Operation) -> String {
    match op {
        Operation::Refresh { backend } => format!("Refreshing {}", source_name(backend)),
        Operation::UpgradeAll { backend } => format!("Updating {}", everything_from(backend)),
        Operation::Clean(id) => format!("Cleaning up {}", clean(&id.key)),
        Operation::Install(id) => format!("Installing {}", target(id)),
        Operation::Remove(id) => format!("Removing {}", target(id)),
        Operation::Upgrade(id) => format!("Updating {}", target(id)),
    }
}
/// Where a change happens: the source, plus architecture and scope when they matter.
pub fn operation_context(op: &Operation) -> String {
    match op {
        Operation::Refresh { .. } | Operation::UpgradeAll { .. } => String::new(),
        Operation::Clean(id) => source_name(&id.backend),
        Operation::Install(id) | Operation::Remove(id) | Operation::Upgrade(id) => {
            let mut parts = vec![source_name(&id.backend)];
            // Architecture only tells native packages apart (amd64 vs i386).
            if matches!(id.backend.as_str(), "apt" | "dnf" | "pacman" | "zypper")
                && !matches!(id.architecture.as_str(), "" | "all" | "any" | "noarch")
            {
                parts.push(clean(&id.architecture));
            }
            match id.scope {
                Scope::User { .. } => parts.push("user".into()),
                Scope::System if id.backend == "flatpak" => parts.push("system".into()),
                _ => {}
            }
            parts.join(", ")
        }
    }
}
fn operation_icon(paint: Paint, op: &Operation) -> String {
    match op {
        Operation::Install(_) => paint.green("+"),
        Operation::Remove(_) => paint.red("-"),
        Operation::Upgrade(_) | Operation::UpgradeAll { .. } => paint.cyan("↑"),
        Operation::Refresh { .. } => paint.cyan("↻"),
        Operation::Clean(_) => paint.yellow("~"),
    }
}
/// Title plus context on one line, without color.
#[cfg(test)]
pub fn operation(op: &Operation) -> String {
    operation_line(Paint(false), op)
}
fn operation_line(paint: Paint, op: &Operation) -> String {
    let context = operation_context(op);
    if context.is_empty() {
        operation_title(op)
    } else {
        format!("{}  {}", operation_title(op), paint.dim(&context))
    }
}
/// The review shown before asking to apply changes. `notes` are extra
/// details for one change, such as an APT transaction preview.
pub fn plan(operations: &[Operation], notes: &[(Operation, String)], color: bool) -> String {
    let paint = Paint(color);
    let count = operations.len();
    let mut output = paint.bold(&format!(
        "{count} change{}",
        if count == 1 { "" } else { "s" }
    ));
    for op in operations {
        output.push_str(&format!(
            "\n  {} {}",
            operation_icon(paint, op),
            operation_line(paint, op)
        ));
        for (_, note) in notes.iter().filter(|(noted, _)| noted == op) {
            for line in note.lines().filter(|line| !line.trim().is_empty()) {
                output.push_str(&format!("\n      {}", paint.dim(&clean(line.trim()))));
            }
        }
    }
    output
}
fn capability_verb(capability: &str) -> String {
    match capability {
        "search" => "search".into(),
        "details" => "show package details".into(),
        "installed" => "list installed packages".into(),
        "install" => "install packages".into(),
        "remove" => "remove packages".into(),
        "refresh" => "refresh package lists".into(),
        "upgrade" => "update packages".into(),
        "clean" => "clean up".into(),
        other => clean(other),
    }
}
/// A plain-language error, with what to do next when there is something to do.
pub fn error_message(error: &EngineError) -> String {
    serde_json::to_value(error).map_or_else(|_| clean(&error.to_string()), |v| error_text(&v))
}
/// The same wording for an error already serialized in a report.
pub fn error_text(error: &Value) -> String {
    let field = |v: &Value, key: &str| clean(v[key].as_str().unwrap_or_default());
    match error {
        Value::String(kind) => match kind.as_str() {
            "Cancelled" => "Cancelled.".into(),
            "NotFound" => "No package has that exact name. Use `pkd search` to find it.".into(),
            other => clean(other),
        },
        Value::Object(fields) if fields.len() == 1 => {
            let (kind, inner) = fields.iter().next().expect("one field");
            match kind.as_str() {
                "Execution" => execution_text(inner),
                "Unsupported" => format!(
                    "{} can't {}.",
                    source_name(&field(inner, "backend")),
                    capability_verb(inner["capability"].as_str().unwrap_or_default())
                ),
                "Unavailable" => format!(
                    "{} isn't available: {}",
                    source_name(&field(inner, "backend")),
                    field(inner, "reason")
                ),
                "InvalidResponse" => {
                    format!(
                        "{}: {}",
                        source_name(&field(inner, "backend")),
                        field(inner, "reason")
                    )
                }
                "Incomplete" => {
                    let failed: Vec<_> = inner
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(failure_text)
                        .collect();
                    format!(
                        "Some sources didn't answer, so nothing was changed. {} Try again, or leave them out with --from.",
                        failed.join(" ")
                    )
                }
                "Ambiguous" => format!(
                    "{} packages have this name. Pick one with --from, --arch, or --scope.",
                    inner.as_array().map_or(0, Vec::len)
                ),
                _ => value(error),
            }
        }
        other => value(other),
    }
}
fn execution_text(error: &Value) -> String {
    match error.as_str() {
        Some("AuthorizationDenied") => "Administrator access was denied. Run it again and enter your password, or add --auth polkit.".into(),
        Some("AuthorizationCancelled") => "Administrator access was cancelled. Nothing was changed.".into(),
        Some("LockBusy") => "Another package manager is running. Wait for it to finish, then try again.".into(),
        Some("Interrupted") => "The package manager was interrupted. Check its state before trying again.".into(),
        Some("TimedOut") => "The package manager took too long to answer.".into(),
        Some("Cancelled") => "Cancelled.".into(),
        Some(other) => clean(other),
        None => {
            if let Some(text) = ["Invalid", "Io", "Disabled"]
                .iter()
                .find_map(|kind| error.get(*kind).and_then(Value::as_str))
            {
                clean(text)
            } else if let Some(result) = error.get("Failed") {
                format!("The package manager {}", outcome(result))
            } else {
                value(error)
            }
        }
    }
}
/// How a failed command ended, as the end of a sentence, from its
/// serialized result: "exited with code 1: error: …". Mirrors
/// `Completion::outcome` in the engine.
fn outcome(result: &Value) -> String {
    let ended = match (result["code"].as_i64(), result["signal"].as_i64()) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("was stopped by signal {signal}"),
        (None, None) => "was stopped".into(),
    };
    let stderr = result["stderr"].as_str().unwrap_or_default();
    match pkgdeck_core::process::failure_summary(stderr.as_bytes()) {
        Some(reason) => format!("{ended}: {reason}"),
        None => format!("{ended}."),
    }
}
/// A source's error as one sentence that names the source once, such as
/// "Cargo exited with code 1: error: rustup could not choose a version".
fn failure_text(failure: &Value) -> String {
    match failure["backend"].as_str() {
        Some(backend) => {
            let name = source_name(backend);
            if let Some(result) = failure["error"]["Execution"].get("Failed") {
                return format!("{name} {}", outcome(result));
            }
            let text = error_text(&failure["error"]);
            // Many errors already start with their source; don't repeat it.
            if text.starts_with(&format!("{name}:")) || text.starts_with(&format!("{name} ")) {
                text
            } else {
                format!("{name}: {text}")
            }
        }
        None => value(failure),
    }
}
/// Why a source's availability check failed, for `pkd sources`.
fn source_check_text(backend: &str, error: &Value) -> String {
    let name = source_name(backend);
    match error["Execution"].get("Failed") {
        Some(result) => {
            let stderr = result["stderr"].as_str().unwrap_or_default();
            match pkgdeck_core::process::failure_summary(stderr.as_bytes()) {
                Some(reason) => format!("Couldn't run {name}: {reason}"),
                None => format!("Couldn't run {name}: it {}", outcome(result)),
            }
        }
        None => format!("Couldn't check {name}: {}", error_text(error)),
    }
}
/// One finished change: a check or a cross, then the reason on its own line.
pub fn result_line(op: &Operation, error: Option<&str>, deferred: bool, color: bool) -> String {
    let paint = Paint(color);
    let mut line = match error {
        None => format!("{} {}", paint.green("✓"), operation_line(paint, op)),
        Some(_) => format!("{} {}", paint.red("✗"), operation_line(paint, op)),
    };
    if deferred {
        line.push_str(&paint.dim("  finished after you cancelled; changes were kept"));
    }
    if let Some(error) = error {
        line.push_str(&format!("\n  {}", paint.red(&clean(error))));
    }
    line
}
/// Closing line for a batch of changes.
pub fn operations_summary(data: &Value, color: bool) -> String {
    let paint = Paint(color);
    let operations = data["operations"].as_array().map_or(&[][..], Vec::as_slice);
    let total = operations.len();
    let failed = operations
        .iter()
        .filter(|item| item["result"].get("Err").is_some())
        .count();
    if total == 0 {
        paint.green("Nothing to do. Everything is up to date.")
    } else if failed == 0
        && operations
            .iter()
            .all(|item| item["operation"].get("refresh").is_some())
    {
        format!(
            "{} {}",
            paint.green("Package lists refreshed."),
            paint.dim("Run `pkd upgrade` to install updates.")
        )
    } else if failed == 0 {
        paint.green(&format!(
            "Done. {total} change{} applied.",
            if total == 1 { "" } else { "s" }
        ))
    } else if failed == total {
        paint.red(&format!(
            "{} failed.",
            if total == 1 {
                "The change".into()
            } else {
                format!("All {total} changes")
            }
        ))
    } else {
        paint.yellow(&format!(
            "{failed} of {total} changes failed. The rest were applied."
        ))
    }
}
fn value(value: &Value) -> String {
    match value {
        Value::String(s) => clean(s),
        Value::Null => "Unavailable".into(),
        Value::Array(items) => items.iter().map(self::value).collect::<Vec<_>>().join(", "),
        Value::Object(fields) if fields.len() == 1 && fields.contains_key("Ok") => {
            self::value(&fields["Ok"])
        }
        Value::Object(fields) => fields
            .iter()
            .map(|(k, v)| format!("{}: {}", clean(k), self::value(v)))
            .collect::<Vec<_>>()
            .join("; "),
        other => other.to_string(),
    }
}
/// Separates a cell's full text from the shorter form a narrow column uses.
const SHORT: char = '\u{1f}';
/// A cell that reads `long` when it fits and `short` when it doesn't.
fn with_short(long: &str, short: &str) -> String {
    format!("{long}{SHORT}{short}")
}
/// The full text of a cell, without its short form.
fn long_text(text: &str) -> &str {
    text.split(SHORT).next().unwrap_or(text)
}
fn cell(text: &str, width: usize) -> String {
    let text = match text.split_once(SHORT) {
        Some((long, short)) if clean(long).width() > width => clean(short),
        Some((long, _)) => clean(long),
        None => clean(text),
    };
    let mut output = String::new();
    let clipped = text.width() > width;
    let limit = width.saturating_sub(usize::from(clipped));
    for c in text.chars() {
        if format!("{output}{c}").width() > limit {
            break;
        }
        output.push(c);
    }
    if clipped && width > 0 {
        output.push('…');
    }
    let padding = width.saturating_sub(output.width());
    format!("{output}{}", " ".repeat(padding))
}
/// ANSI styling that disappears entirely when color is off.
#[derive(Clone, Copy)]
pub struct Paint(pub bool);
impl Paint {
    fn wrap(self, code: &str, text: &str) -> String {
        if self.0 && !text.is_empty() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.into()
        }
    }
    pub fn bold(self, text: &str) -> String {
        self.wrap("1", text)
    }
    pub fn dim(self, text: &str) -> String {
        self.wrap("2", text)
    }
    fn green(self, text: &str) -> String {
        self.wrap("32", text)
    }
    fn cyan(self, text: &str) -> String {
        self.wrap("36", text)
    }
    fn yellow(self, text: &str) -> String {
        self.wrap("33", text)
    }
    fn red(self, text: &str) -> String {
        self.wrap("31", text)
    }
}
/// Style applied to a padded table cell; padding stays outside the escape codes.
type Style = fn(Paint, &str) -> String;
fn plain(_: Paint, text: &str) -> String {
    text.into()
}
/// "● VLC (org.videolan.VLC)": the id after the name is dimmed.
fn with_dim_id(paint: Paint, text: &str, main: Style) -> String {
    match text.rfind(" (") {
        Some(split) => format!(
            "{}{}",
            main(paint, &text[..split]),
            paint.dim(&text[split..])
        ),
        None => main(paint, text),
    }
}
fn green_named(paint: Paint, text: &str) -> String {
    with_dim_id(paint, text, Paint::green)
}
fn cyan_named(paint: Paint, text: &str) -> String {
    with_dim_id(paint, text, Paint::cyan)
}
/// "system", "user", or the environment path.
fn scope_text(scope: &Value) -> String {
    match scope_label(scope) {
        Some(label) => label.into(),
        None => match scope["environment"]["path"].as_str() {
            Some(path) => format!("environment {}", clean(path)),
            None => value(scope),
        },
    }
}
/// The name to show for a package, and whether it ends with its id in
/// parentheses: apps show their app name ("VLC (org.videolan.VLC)"), and
/// firmware and container images, whose ids are opaque, only their name.
fn package_name(package: &Value) -> (String, bool) {
    let id = value(&package["id"]["name"]);
    let display = package["display_name"]
        .as_str()
        .map(clean)
        .filter(|name| !name.trim().is_empty());
    match (package["id"]["backend"].as_str(), display) {
        (Some("fwupd" | "docker" | "podman"), Some(display)) => (display, false),
        (_, Some(display)) if display != id => (format!("{display} ({id})"), true),
        _ => (id, false),
    }
}
fn table(headers: &[&str], rows: &[Vec<(String, Style)>], width: usize, paint: Paint) -> String {
    // Drop trailing columns on narrow terminals; full metadata remains in `info`/JSON.
    let columns = if width < 60 {
        2
    } else if width < 90 {
        3
    } else {
        headers.len()
    }
    .min(headers.len());
    let gap = 2;
    let available = width.saturating_sub(gap * (columns - 1));
    // Size each column to its content, capped so the last column keeps room;
    // the last column takes whatever is left.
    let natural: Vec<usize> = (0..columns)
        .map(|i| {
            rows.iter()
                .map(|row| clean(long_text(&row[i].0)).width())
                .chain([headers[i].width()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let cap = |i: usize| match columns {
        2 => available * 60 / 100,
        _ if i == 0 => available * 36 / 100,
        _ => available * 24 / 100,
    };
    let mut widths: Vec<usize> = (0..columns)
        .map(|i| natural[i].min(cap(i)).max(3))
        .collect();
    let fixed: usize = widths[..columns - 1].iter().sum();
    widths[columns - 1] = available
        .saturating_sub(fixed)
        .min(natural[columns - 1])
        .max(3);
    let render = |cells: &[(String, Style)], header: bool| {
        cells
            .iter()
            .take(columns)
            .zip(&widths)
            .enumerate()
            .map(|(i, ((text, style), w))| {
                let padded = cell(text, *w);
                let content = padded.trim_end();
                let styled = if header {
                    paint.dim(content)
                } else {
                    style(paint, content)
                };
                if i + 1 == columns {
                    styled
                } else {
                    format!("{styled}{}", " ".repeat(w - content.width()))
                }
            })
            .collect::<Vec<_>>()
            .join(&" ".repeat(gap))
            .trim_end()
            .to_string()
    };
    let header_cells: Vec<(String, Style)> = headers
        .iter()
        .map(|h| ((*h).to_string(), plain as Style))
        .collect();
    let rule = widths.iter().sum::<usize>() + gap * (columns - 1);
    let mut result = render(&header_cells, true);
    result.push('\n');
    result.push_str(&paint.dim(&"─".repeat(rule.min(width))));
    for row in rows {
        result.push('\n');
        result.push_str(&render(row, false));
    }
    result
}
/// One ownership record of an executable, in words: "Installed by cowsay
/// (APT)". Records about another file (a symlink target) name that file.
fn owner_line(owner: &Value, candidate: &Value) -> String {
    let manager = source_name(owner["manager"].as_str().unwrap_or_default());
    let native = value(&owner["native_name"]);
    let path = value(&owner["path"]);
    let file = if owner["path"].is_null() || owner["path"] == candidate["path"] {
        String::new()
    } else {
        format!(" as {path}")
    };
    match owner["state"].as_str() {
        Some("known") => format!("Installed by {native} ({manager}){file}"),
        Some("ambiguous") => format!(
            "Installed by {native} ({manager}){file}, which matches several installed packages:"
        ),
        Some("unmatched") => format!(
            "{manager} lists {path} under {native}, but {native} isn't in the installed list"
        ),
        _ => format!("No package manager claims {path}"),
    }
}
/// Below 60 columns a table keeps two columns. Here the rest of each row
/// is too useful to drop, so it wraps onto indented lines under the row.
fn table_with_notes(
    headers: &[&str],
    rows: &[Vec<(String, Style)>],
    width: usize,
    paint: Paint,
) -> String {
    if width >= 60 || headers.len() <= 2 {
        return table(headers, rows, width, paint);
    }
    let rendered = table(&headers[..2], rows, width, paint);
    let mut lines = rendered.lines();
    let mut output: Vec<String> = lines.by_ref().take(2).map(str::to_string).collect();
    for (line, row) in lines.zip(rows) {
        output.push(line.to_string());
        let note: Vec<_> = row[2..]
            .iter()
            .map(|(text, _)| clean(long_text(text)))
            .filter(|text| !text.trim().is_empty())
            .collect();
        for wrapped in wrap(&note.join("  "), width.saturating_sub(4).max(8)) {
            output.push(format!("    {}", paint.dim(&wrapped)));
        }
    }
    output.join("\n")
}
/// Greedy word wrap by display width; long words are cut.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if candidate.width() <= width {
            line = candidate;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        line = if word.width() > width {
            cell(word, width).trim_end().to_string()
        } else {
            word.to_string()
        };
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}
fn failure_line(paint: Paint, detail: &str) -> String {
    format!("\n{} {detail}", paint.yellow("!"))
}
/// "already_installed" reads as "Already installed".
fn humanize(token: &str) -> String {
    let text = token.replace('_', " ");
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}
fn scope_label(scope: &Value) -> Option<&'static str> {
    match scope {
        Value::String(s) if s == "system" => Some("system"),
        Value::Object(fields) if fields.contains_key("user") => Some("user"),
        _ => None,
    }
}
pub fn human(data: &Value, width: usize, color: bool) -> String {
    let width = width.clamp(24, 160);
    let paint = Paint(color);
    let mut output = String::new();
    if let Some(items) = data["items"].as_array() {
        if items.is_empty() {
            output.push_str(&paint.green("Nothing to clean."));
        } else {
            let rows = items
                .iter()
                .map(|item| {
                    vec![
                        (
                            format!(
                                "{}:{}",
                                value(&item["id"]["backend"]),
                                value(&item["id"]["key"])
                            ),
                            Paint::bold as Style,
                        ),
                        (value(&item["title"]), plain as Style),
                        (value(&item["summary"]), Paint::dim as Style),
                    ]
                })
                .collect::<Vec<_>>();
            output.push_str(&table(&["KEY", "TASK", "SUMMARY"], &rows, width, paint));
            output.push_str(&format!(
                "\n\n{}",
                paint.dim("Run one with `pkd clean <key>`, or all with `pkd clean --all`. Add --json to see each preview.")
            ));
        }
        let failures = data["failures"].as_array().map_or(&[][..], Vec::as_slice);
        let (unsupported, failed): (Vec<_>, Vec<_>) = failures.iter().partition(|failure| {
            failure["error"].get("Unsupported").is_some()
                || failure["error"].get("unsupported").is_some()
        });
        for failure in failed {
            output.push_str(&failure_line(paint, &failure_text(failure)));
        }
        if !unsupported.is_empty() {
            let names: Vec<_> = unsupported
                .iter()
                .map(|failure| source_name(failure["backend"].as_str().unwrap_or_default()))
                .collect();
            output.push_str(&format!(
                "\n\n{}",
                paint.dim(&format!("No cleanup tasks in: {}", names.join(", ")))
            ));
        }
    } else if let Some(packages) = data["packages"].as_array() {
        // Which of the three states appear, for a legend without noise.
        let mut states = [false; 3];
        let rows = packages
            .iter()
            .map(|p| {
                let installed = !p["installed_version"].is_null();
                let update = p["update"] == "available";
                let marker = package_marker(installed, update);
                states[if update { 2 } else { usize::from(installed) }] = true;
                let (name, named) = package_name(p);
                // Flatpak can install the same app per user and system-wide.
                // Narrow columns use "flatpak·sys" rather than cutting it.
                let source = match (p["id"]["backend"].as_str(), scope_label(&p["id"]["scope"])) {
                    (Some("flatpak"), Some(scope)) => with_short(
                        &format!("flatpak ({scope})"),
                        &format!("flatpak·{}", if scope == "system" { "sys" } else { scope }),
                    ),
                    _ => value(&p["id"]["backend"]),
                };
                let version = if update && installed && !p["candidate_version"].is_null() {
                    format!(
                        "{} → {}",
                        value(&p["installed_version"]),
                        value(&p["candidate_version"])
                    )
                } else {
                    value(if installed {
                        &p["installed_version"]
                    } else {
                        &p["candidate_version"]
                    })
                };
                let marker_style: Style = match (update, installed, named) {
                    (true, _, true) => cyan_named,
                    (true, _, false) => Paint::cyan,
                    (false, true, true) => green_named,
                    (false, true, false) => Paint::green,
                    (false, false, _) => Paint::dim,
                };
                vec![
                    (format!("{marker} {name}"), marker_style),
                    (source, Paint::dim as Style),
                    (
                        version,
                        if update {
                            Paint::cyan as Style
                        } else {
                            plain as Style
                        },
                    ),
                    (value(&p["summary"]), Paint::dim as Style),
                ]
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            let failed = data["failures"]
                .as_array()
                .is_some_and(|failures| !failures.is_empty());
            output.push_str(if failed {
                "No packages returned from checked sources."
            } else {
                "No packages found."
            });
        } else {
            output.push_str(&table(
                &["NAME", "SOURCE", "VERSION", "SUMMARY"],
                &rows,
                width,
                paint,
            ));
            let count = rows.len();
            let legend: Vec<_> = ["○ not installed", "● installed", "↑ update available"]
                .into_iter()
                .zip(states)
                .filter_map(|(entry, seen)| seen.then_some(entry))
                .collect();
            output.push_str(&format!(
                "\n\n{}{}\n{}",
                paint.bold(&format!(
                    "{count} package{}",
                    if count == 1 { "" } else { "s" }
                )),
                paint.dim(". Run `pkd info <name>` for details."),
                paint.dim(&legend.join("   "))
            ));
        }
        if let Some(failures) = data["failures"].as_array() {
            for failure in failures {
                output.push_str(&failure_line(paint, &failure_text(failure)));
            }
        }
    } else if let Some(export) = data.get("manifest_export") {
        output.push_str(&paint.green(&format!(
            "Exported {} to {}",
            match export["packages"].as_u64() {
                Some(1) => "1 package".to_string(),
                count => format!("{} packages", count.unwrap_or_default()),
            },
            value(&export["path"])
        )));
    } else if let Some(preview) = data.get("manifest_preview") {
        let rows = preview["packages"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|entry| {
                vec![
                    (value(&entry["package"]["name"]), Paint::bold as Style),
                    (value(&entry["package"]["backend"]), Paint::dim as Style),
                    (humanize(&value(&entry["status"])), plain as Style),
                    (value(&entry["reason"]), Paint::dim as Style),
                ]
            })
            .collect::<Vec<_>>();
        output.push_str(&table(
            &["PACKAGE", "SOURCE", "STATUS", "DETAIL"],
            &rows,
            width,
            paint,
        ));
        for entry in preview["packages"].as_array().into_iter().flatten() {
            for proposed in entry["proposed_changes"].as_array().into_iter().flatten() {
                output.push_str(&format!("\n  {}", value(&proposed["detail"])));
            }
        }
    } else if let Some(report) = data.get("inspection") {
        for (label, field) in [
            ("Command", &report["command"]),
            ("Resolved", &report["resolved"]),
            ("Environment", &report["environment"]),
            ("PATH", &report["path"]),
        ] {
            output.push_str(&format!(
                "{}  {}\n",
                paint.dim(&format!("{label:>11}")),
                value(field)
            ));
        }
        if let Some(candidates) = report["candidates"].as_array() {
            for candidate in candidates {
                output.push_str(&format!(
                    "\n{} {}",
                    paint.bold(&value(&candidate["path"])),
                    paint.dim(&format!("[{}]", value(&candidate["state"])))
                ));
                if !candidate["target"].is_null() {
                    output.push_str(&format!(" → {}", value(&candidate["target"])));
                }
                if let Some(owners) = candidate["owners"].as_array() {
                    for owner in owners {
                        output.push_str(&format!("\n  {}", owner_line(owner, candidate)));
                        if let Some(packages) = owner["packages"].as_array() {
                            for package in packages {
                                output.push_str(&format!(
                                    "\n    Package: {} from {}, {}",
                                    value(&package["name"]),
                                    source_name(package["backend"].as_str().unwrap_or_default()),
                                    scope_text(&package["scope"])
                                ));
                            }
                        }
                    }
                }
            }
        }
        output.push_str(&format!(
            "\n\n{}",
            paint.dim(&value(&report["ownership_note"]))
        ));
    } else if let Some(report) = data.get("audit") {
        if let Some(groups) = report["groups"].as_array() {
            output.push_str(&paint.bold(&match groups.len() {
                0 => "No apps installed more than once.\n".to_string(),
                1 => "1 app installed more than once\n".to_string(),
                count => format!("{count} apps installed more than once\n"),
            }));
            for group in groups {
                output.push_str(&format!("\n{}\n", paint.bold(&value(&group["key"]))));
                if let Some(copies) = group["copies"].as_array() {
                    for copy in copies {
                        output.push_str(&format!(
                            "  {} from {}, {}, version {}\n",
                            value(&copy["package"]["name"]),
                            source_name(copy["package"]["backend"].as_str().unwrap_or_default()),
                            scope_text(&copy["package"]["scope"]),
                            value(&copy["installed_version"])
                        ));
                    }
                }
            }
        }
        if let Some(leftovers) = report["leftovers"].as_array() {
            output.push_str(&paint.bold(&match leftovers.len() {
                0 => "\nNo leftover files from removed packages.\n".to_string(),
                1 => "\n1 leftover file from removed packages\n".to_string(),
                count => format!("\n{count} leftover files from removed packages\n"),
            }));
            for row in leftovers {
                output.push_str(&format!(
                    "  {} ({} bytes, from {} package {})\n",
                    value(&row["path"]),
                    value(&row["size_bytes"]),
                    source_name(row["manager"].as_str().unwrap_or_default()),
                    value(&row["native_name"])
                ));
            }
        }
        output.push_str(&format!("\n{}", paint.dim(&value(&report["data_note"]))));
    } else if data.get("package").is_some() {
        let p = &data["package"];
        let (name, named) = package_name(p);
        output.push_str(&if named {
            with_dim_id(paint, &name, Paint::bold)
        } else {
            paint.bold(&name)
        });
        let description = value(&data["description"]);
        if !data["description"].is_null() && !description.trim().is_empty() {
            output.push_str(&format!("\n{description}"));
        }
        output.push_str("\n\n");
        let source = p["id"]["backend"]
            .as_str()
            .map(|backend| Value::String(source_name(backend)))
            .unwrap_or(Value::Null);
        for (label, field) in [
            ("Source", &source),
            ("Reference", &p["id"]["reference"]),
            ("Architecture", &p["id"]["architecture"]),
            ("Scope", &p["id"]["scope"]),
            ("Installed", &p["installed_version"]),
            ("Available", &p["candidate_version"]),
            ("Update", &p["update"]),
            ("Homepage", &data["homepage"]),
            ("Dependencies", &data["dependencies"]),
        ] {
            // A missing installed version means not installed; other empty
            // fields carry no information and are left out.
            let text = if label == "Installed" && field.is_null() {
                "no".into()
            } else if field.is_null()
                || field.as_array().is_some_and(Vec::is_empty)
                || (label == "Update" && field == "unknown")
            {
                continue;
            } else if label == "Scope" {
                scope_text(field)
            } else if label == "Update" && field == "current" {
                "up to date".into()
            } else if label == "Update" && field == "available" {
                paint.cyan(&match p["candidate_version"].as_str() {
                    Some(version) => format!("available ({})", clean(version)),
                    None => "available".into(),
                })
            } else {
                value(field)
            };
            output.push_str(&format!("{}  {text}\n", paint.dim(&format!("{label:>12}"))));
        }
    } else if let Some(repositories) = data["repositories"].as_array() {
        let rows = repositories
            .iter()
            .map(|r| {
                let enabled = r["enabled"] == true;
                vec![
                    (
                        format!("{} {}", if enabled { "●" } else { "○" }, value(&r["title"])),
                        if enabled {
                            plain as Style
                        } else {
                            Paint::dim as Style
                        },
                    ),
                    (value(&r["backend"]), Paint::dim as Style),
                    (
                        scope_label(&r["scope"]).map_or_else(|| value(&r["scope"]), str::to_string),
                        Paint::dim as Style,
                    ),
                    (value(&r["url"]), Paint::dim as Style),
                ]
            })
            .collect::<Vec<_>>();
        output.push_str(&table(
            &["REPOSITORY", "SOURCE", "SCOPE", "URL"],
            &rows,
            width,
            paint,
        ));
        if let Some(errors) = data["errors"].as_array() {
            for error in errors {
                output.push_str(&failure_line(paint, &value(error)));
            }
        }
    } else if let Some(sources) = data["sources"].as_array() {
        // Usable sources first; the rest explain why they are unavailable.
        let available = |s: &&Value| value(&s["availability"]) == "available";
        let rows = sources
            .iter()
            .filter(available)
            .chain(sources.iter().filter(|s| !available(s)))
            .map(|s| {
                let (status, detail) = match s["availability"].get("Err") {
                    // The check itself failed: say why in one sentence.
                    Some(error) => (
                        "failed".to_string(),
                        source_check_text(s["backend"].as_str().unwrap_or_default(), error),
                    ),
                    None => {
                        let availability = value(&s["availability"]);
                        match availability.split_once(": ") {
                            Some((status, reason)) => (status.to_string(), reason.to_string()),
                            None => (availability.clone(), value(&s["capabilities"])),
                        }
                    }
                };
                let available = status == "available";
                vec![
                    (
                        value(&s["backend"]),
                        if available {
                            Paint::bold as Style
                        } else {
                            Paint::dim as Style
                        },
                    ),
                    (
                        status,
                        if available {
                            Paint::green as Style
                        } else {
                            Paint::dim as Style
                        },
                    ),
                    (detail, Paint::dim as Style),
                ]
            })
            .collect::<Vec<_>>();
        output.push_str(&table_with_notes(
            &["SOURCE", "STATUS", "DETAILS"],
            &rows,
            width,
            paint,
        ));
    } else if let Some(operations) = data["operations"].as_array() {
        for item in operations {
            let op: Operation =
                serde_json::from_value(item["operation"].clone()).expect("typed operation");
            let error = item["result"].get("Err").map(|error| {
                item["message"]
                    .as_str()
                    .map_or_else(|| error_text(error), str::to_string)
            });
            let deferred = item["result"]["Ok"]["cancellation_deferred"] == true;
            output.push_str(&result_line(&op, error.as_deref(), deferred, color));
            output.push('\n');
        }
        if !operations.is_empty() {
            output.push('\n');
        }
        output.push_str(&operations_summary(data, color));
    } else if data["error"] == "confirmation_declined" {
        output.push_str(&paint.dim(&value(&data["message"])));
    } else {
        output.push_str(&format!(
            "{} {}",
            paint.red("✗"),
            value(data.get("message").unwrap_or(&data["error"]))
        ));
        if let Some(matches) = data["error"]["Ambiguous"].as_array() {
            // Each match with the flags that pick it: --from, plus --arch or
            // --scope when the same source has more than one match.
            let flags: Vec<_> = matches
                .iter()
                .map(|id| {
                    let same: Vec<_> = matches
                        .iter()
                        .filter(|other| other["backend"] == id["backend"])
                        .collect();
                    let mut flags = vec![format!("--from {}", value(&id["backend"]))];
                    if same
                        .iter()
                        .any(|other| other["architecture"] != id["architecture"])
                    {
                        flags.push(format!("--arch {}", value(&id["architecture"])));
                    }
                    let scope = scope_label(&id["scope"]);
                    if scope.is_some()
                        && same
                            .iter()
                            .any(|other| scope_label(&other["scope"]) != scope)
                    {
                        flags.push(format!("--scope {}", scope.unwrap_or_default()));
                    }
                    flags.join(" ")
                })
                .collect();
            let width = flags.iter().map(|flag| flag.width()).max().unwrap_or(0);
            for (id, flag) in matches.iter().zip(flags) {
                // Flags can't tell apart matches that differ only by
                // reference, such as two Flatpak branches. The reference
                // selects the exact one, so show it in place of the name.
                let twin = matches.iter().any(|other| {
                    other != id
                        && other["backend"] == id["backend"]
                        && other["architecture"] == id["architecture"]
                        && scope_label(&other["scope"]) == scope_label(&id["scope"])
                });
                let name = match id["reference"].as_str() {
                    Some(reference) if twin && !reference.is_empty() => clean(reference),
                    _ => value(&id["name"]),
                };
                let mut parts = vec![name];
                if matches!(
                    id["backend"].as_str(),
                    Some("apt" | "dnf" | "pacman" | "zypper")
                ) {
                    parts.push(value(&id["architecture"]));
                }
                parts.extend(scope_label(&id["scope"]).map(str::to_string));
                output.push_str(&format!(
                    "\n  {}{}  {}",
                    paint.bold(&flag),
                    " ".repeat(width - flag.width()),
                    parts.join(", ")
                ));
            }
        }
    }
    if data.get("inspection").is_some() || data.get("audit").is_some() {
        if let Some(failures) = data["failures"].as_array() {
            for failure in failures {
                output.push_str(&failure_line(paint, &failure_text(failure)));
            }
        }
    }
    output.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn portable_inventory_summary_shows_status_and_proposed_change() {
        let exported = human(
            &json!({"manifest_export":{"path":"synthetic.json", "packages":2}}),
            100,
            false,
        );
        assert!(exported.contains("Exported 2 packages to synthetic.json"));
        let preview = human(
            &json!({"manifest_preview":{"packages":[
                {"package":{"name":"org.example.App", "backend":"flatpak"},
                 "status":"ambiguous", "reason":"choose a repository",
                 "proposed_changes":[{"kind":"repository_addition", "detail":"Review flathub"}]}
            ]}}),
            100,
            false,
        );
        assert!(preview.contains("org.example.App"));
        assert!(preview.contains("Ambiguous"));
        assert_eq!(humanize("already_installed"), "Already installed");
        assert_eq!(humanize(""), "");
        let one = human(
            &json!({"manifest_export":{"path":"one.json", "packages":1}}),
            100,
            false,
        );
        assert!(one.contains("Exported 1 package to one.json"));
        assert!(preview.contains("Review flathub"));
    }
    #[test]
    fn repository_and_firmware_labels_are_human_readable() {
        let output = human(
            &json!({"repositories":[
            {"title":"Synthetic", "backend":"flatpak", "scope":"system", "enabled":true, "url":"https://example.invalid"},
            {"title":"Disabled", "backend":"fwupd", "scope":"system", "enabled":false, "url":"https://example.invalid"}
        ], "errors":["Synthetic failure"]}),
            120,
            false,
        );
        assert!(output.contains("● Synthetic"));
        assert!(output.contains("○ Disabled"));
        assert!(output.contains("system"));
        assert!(output.contains("Synthetic failure"));
        let output = human(
            &json!({"packages":[{"id":{"name":"opaque-id","backend":"fwupd"},"display_name":"Synthetic BIOS", "installed_version":"1"}]}),
            120,
            false,
        );
        assert!(output.contains("Synthetic BIOS"));
        assert!(!output.contains("opaque-id"));
    }
    #[test]
    fn tables_fit_narrow_unicode_terminals_and_strip_control_sequences() {
        let data = json!({"packages":[{"id":{"name":"工具-package-with-a-long-name", "backend":"fixture"}, "candidate_version":"2", "summary":"hello\u{001b}[31m\nworld"}],"failures":[]});
        for width in [24, 60, 100] {
            let output = human(&data, width, false);
            assert!(!output.contains('\u{001b}'));
            for line in output.lines().take(3) {
                assert!(line.width() <= width);
            }
            assert!(output.contains("fixture"));
        }
        let colored = human(&data, 100, true);
        assert!(colored.contains("\x1b[2mNAME\x1b[0m"));
        assert!(!colored.contains("PkgDeck"));
        assert_eq!(cell("a", 0), "");
    }
    #[test]
    fn package_states_have_portable_markers() {
        assert_eq!(package_marker(false, false), "○");
        assert_eq!(package_marker(true, false), "●");
        assert_eq!(package_marker(true, true), "↑");
        let output = human(
            &json!({"packages":[{"id":{"name":"fixture","backend":"apt"},"installed_version":"1","update":"available"}]}),
            100,
            false,
        );
        assert!(output.contains("↑ fixture"));
        assert!(output.contains("↑ update available"));
        assert!(!output.contains('\u{1b}'));
    }
    #[test]
    fn empty_results_and_ambiguous_choices_are_actionable() {
        let empty = human(&json!({"packages": [], "failures": []}), 80, false);
        assert!(empty.contains("No packages found."));
        assert!(!empty.contains("NAME"));
        assert!(!empty.contains("Not installed"));
        let incomplete = human(
            &json!({"packages": [], "failures": [{"backend":"apt", "error":"synthetic failure"}]}),
            80,
            false,
        );
        assert!(incomplete.contains("No packages returned from checked sources."));
        assert!(incomplete.contains("! APT: synthetic failure"));
        let ambiguous = human(
            &json!({"error":{"Ambiguous":[
                {"backend":"apt","name":"fixture","architecture":"amd64","scope":"system"},
                {"backend":"apt","name":"fixture","architecture":"i386","scope":"system"}
            ]},"message":"2 packages match; select a backend, architecture, or scope"}),
            80,
            false,
        );
        assert!(ambiguous.contains("--from apt --arch amd64  fixture, amd64, system"));
        assert!(ambiguous.contains("--from apt --arch i386   fixture, i386, system"));
        let scopes = human(
            &json!({"error":{"Ambiguous":[
                {"backend":"flatpak","name":"org.example.App","architecture":"x86_64","scope":"system"},
                {"backend":"flatpak","name":"org.example.App","architecture":"x86_64","scope":{"user":{"uid":1000}}}
            ]},"message":"2 packages match"}),
            100,
            false,
        );
        assert!(
            scopes.contains("--from flatpak --scope system  org.example.App, system"),
            "{scopes}"
        );
        assert!(
            scopes.contains("--from flatpak --scope user    org.example.App, user"),
            "{scopes}"
        );
        let branches = human(
            &json!({"error":{"Ambiguous":[
                {"backend":"flatpak","name":"org.example.App","architecture":"x86_64","scope":"system","reference":"org.example.App/x86_64/stable"},
                {"backend":"flatpak","name":"org.example.App","architecture":"x86_64","scope":"system","reference":"org.example.App/x86_64/beta"}
            ]},"message":"2 packages match"}),
            100,
            false,
        );
        assert!(
            branches.contains("--from flatpak  org.example.App/x86_64/stable, system"),
            "{branches}"
        );
        assert!(
            branches.contains("--from flatpak  org.example.App/x86_64/beta, system"),
            "{branches}"
        );
        let mixed = human(
            &json!({"error":{"Ambiguous":[
                {"backend":"apt","name":"htop","architecture":"amd64","scope":"system"},
                {"backend":"pipx","name":"htop","architecture":"x86_64","scope":{"environment":{"path":"/venv"}}}
            ]},"message":"2 packages match"}),
            80,
            false,
        );
        assert!(
            mixed.contains("--from apt   htop, amd64, system"),
            "{mixed}"
        );
        assert!(
            mixed.contains("--from pipx  htop") && !mixed.contains("/venv"),
            "{mixed}"
        );
    }
    #[test]
    fn inspection_and_audit_render_exact_read_only_evidence() {
        let command = json!({"inspection":{"command":"tool","environment":"Native host","path":"/first:/second",
            "resolved":"/first/tool","ownership_note":"Unknown remains unknown.","candidates":[
                {"path":"/first/tool","target":"/target/tool","state":"executable","owners":[
                    {"manager":"apt","native_name":"fixture:amd64","state":"known","packages":[
                        {"backend":"apt","name":"fixture","scope":"system"}]}]}]}});
        let output = human(&command, 100, false);
        assert!(output.contains("Resolved  /first/tool"));
        assert!(output.contains("/target/tool"));
        assert!(
            output.contains("Installed by fixture:amd64 (APT)"),
            "{output}"
        );
        assert!(
            output.contains("    Package: fixture from APT, system"),
            "{output}"
        );
        let audited = json!({"audit":{"groups":[{"key":"fixture","copies":[{"package":{"backend":"apt","name":"fixture","scope":"system"},"installed_version":"1"}]}],
            "leftovers":[{"manager":"apt","native_name":"old-fixture","path":"/etc/old.conf","size_bytes":4}],
            "data_note":"Unknown data remains unknown."}});
        let output = human(&audited, 100, false);
        assert!(output.contains("1 app installed more than once"));
        assert!(output.contains("1 leftover file from removed packages"));
        let empty = human(
            &json!({"audit":{"groups":[],"leftovers":[],"data_note":"Note."}}),
            100,
            false,
        );
        assert!(empty.contains("No apps installed more than once."));
        assert!(empty.contains("No leftover files from removed packages."));
        assert!(output.contains("/etc/old.conf"));
        assert!(output.contains("Unknown data remains unknown."));
    }
    #[test]
    fn cleanup_plans_and_failures_are_concise_and_actionable() {
        assert_eq!(
            operation(&Operation::Clean(pkgdeck_core::package::CleanupId {
                backend: "apt".into(),
                key: "autoremove".into(),
            })),
            "Clean up autoremove  APT"
        );
        assert_eq!(
            operation(&Operation::UpgradeAll {
                backend: "homebrew".into(),
            }),
            "Update all Homebrew packages"
        );
        let plans = json!({
            "items": [{
                "id": {"backend": "apt", "key": "autoremove"},
                "title": "Unused dependencies",
                "summary": "One package"
            }],
            "failures": [
                {"backend": "snap", "error": {"unsupported": {"capability": "clean"}}},
                {"backend": "homebrew", "error": "synthetic failure"}
            ]
        });
        let output = human(&plans, 100, false);
        assert!(output.contains("apt:autoremove"));
        assert!(output.contains("Unused dependencies"));
        assert!(output.contains("pkd clean --all"));
        assert!(output.contains("No cleanup tasks in: Snap"));
        assert!(output.contains("Homebrew: synthetic failure"));

        let empty = human(&json!({"items": [], "failures": []}), 80, false);
        assert!(empty.contains("Nothing to clean"));
    }
    #[test]
    fn package_table_shows_the_installed_version_when_present() {
        let output = human(
            &json!({"packages":[
                {"id":{"name":"installed","backend":"apt"},"installed_version":"1.2.3","candidate_version":"9.9.9"},
                {"id":{"name":"available","backend":"apt"},"candidate_version":"4.5.6"}
            ]}),
            100,
            false,
        );
        assert!(output.contains("1.2.3"));
        assert!(output.contains("4.5.6"));
        assert!(!output.contains("9.9.9"));
    }
    #[test]
    fn every_change_kind_has_a_title_progress_and_context() {
        let id = |backend: &str, arch: &str, scope: Scope| PackageId {
            backend: backend.into(),
            name: "tool".into(),
            architecture: arch.into(),
            scope,
            remote: None,
            reference: None,
        };
        let cases = [
            (
                Operation::Install(id("apt", "i386", Scope::System)),
                "Install tool",
                "Installing tool",
                "APT, i386",
            ),
            (
                Operation::Remove(id("flatpak", "x86_64", Scope::System)),
                "Remove tool",
                "Removing tool",
                "Flatpak, system",
            ),
            (
                Operation::Upgrade(id("npm", "all", Scope::User { uid: 1 })),
                "Update tool",
                "Updating tool",
                "npm, user",
            ),
            (
                Operation::Refresh {
                    backend: "apt".into(),
                },
                "Refresh APT",
                "Refreshing APT",
                "",
            ),
            (
                Operation::UpgradeAll {
                    backend: "snap".into(),
                },
                "Update all Snap packages",
                "Updating all Snap packages",
                "",
            ),
            (
                Operation::Clean(pkgdeck_core::package::CleanupId {
                    backend: "apt".into(),
                    key: "autoclean".into(),
                }),
                "Clean up autoclean",
                "Cleaning up autoclean",
                "APT",
            ),
        ];
        let operations: Vec<_> = cases.iter().map(|case| case.0.clone()).collect();
        for (operation, title, progress, context) in &cases {
            assert_eq!(operation_title(operation), *title);
            assert_eq!(operation_progress(operation), *progress);
            assert_eq!(operation_context(operation), *context);
        }
        let notes = [(operations[4].clone(), "Update (1): snapd\n".to_string())];
        for color in [false, true] {
            let review = plan(&operations, &notes, color);
            assert!(review.contains("6 changes"), "{review}");
            assert!(review.contains("Update (1): snapd"), "{review}");
            assert!(plan(&operations[..1], &[], color).contains("1 change"));
            let done = result_line(&operations[0], None, false, color);
            assert!(done.contains("Install tool"));
            let failed = result_line(&operations[1], Some("busy"), true, color);
            assert!(failed.contains("busy") && failed.contains("after you cancelled"));
        }
    }
    #[test]
    fn errors_read_as_plain_language_with_next_steps() {
        let failed = |code: Option<i64>, stderr: &str| json!({"Execution": {"Failed": {"code": code, "stderr": stderr}}});
        for (error, expected) in [
            (json!("Cancelled"), "Cancelled."),
            (json!("NotFound"), "Use `pkd search`"),
            (json!("Other"), "Other"),
            (
                json!({"Execution": "AuthorizationCancelled"}),
                "was cancelled",
            ),
            (
                json!({"Execution": "LockBusy"}),
                "Another package manager is running",
            ),
            (json!({"Execution": "Interrupted"}), "was interrupted"),
            (json!({"Execution": "TimedOut"}), "took too long"),
            (json!({"Execution": "Cancelled"}), "Cancelled."),
            (json!({"Execution": {"Invalid": "bad input"}}), "bad input"),
            (json!({"Execution": {"Io": "broken pipe"}}), "broken pipe"),
            (
                failed(Some(100), "E: one\n\nE: two\n"),
                "The package manager exited with code 100: E: one",
            ),
            (failed(None, ""), "The package manager was stopped."),
            (json!({"Execution": {"Unknown": 1}}), "Unknown"),
            (
                json!({"Unavailable": {"backend": "dnf", "reason": "not found"}}),
                "DNF isn't available: not found",
            ),
            (
                json!({"InvalidResponse": {"backend": "apt", "reason": "odd"}}),
                "APT: odd",
            ),
            (
                json!({"Incomplete": [{"backend": "apt", "error": {"Execution": "TimedOut"}}]}),
                "Some sources didn't answer, so nothing was changed. APT: The package manager took too long to answer. Try again, or leave them out with --from.",
            ),
            (json!({"Ambiguous": [{}, {}]}), "2 packages have this name"),
            (json!({"UnknownBackend": "x"}), "UnknownBackend: x"),
            (json!(7), "7"),
        ] {
            let text = error_text(&error);
            assert!(text.contains(expected), "{error} -> {text}");
        }
        for (capability, verb) in [
            ("search", "search"),
            ("details", "show package details"),
            ("installed", "list installed packages"),
            ("install", "install packages"),
            ("remove", "remove packages"),
            ("upgrade", "update packages"),
            ("clean", "clean up"),
            ("other", "other"),
        ] {
            assert_eq!(capability_verb(capability), verb);
        }
        assert_eq!(error_message(&EngineError::Cancelled), "Cancelled.");
        assert_eq!(failure_text(&json!("plain")), "plain");
    }
    #[test]
    fn scopes_updates_unavailable_sources_and_source_errors_render() {
        let packages = json!({"packages": [
            {"id": {"name": "org.example.App", "backend": "flatpak", "scope": {"user": {"uid": 1}}},
             "installed_version": "1", "candidate_version": "2", "update": "available"},
            {"id": {"name": "org.example.App", "backend": "flatpak", "scope": "system"},
             "installed_version": "1"}
        ]});
        let output = human(&packages, 120, true);
        assert!(output.contains("flatpak (user)") && output.contains("flatpak (system)"));
        assert!(output.contains("1 → 2"));
        let sources = json!({"sources": [
            {"backend": "apt", "availability": {"Ok": "available"}, "capabilities": ["search"]},
            {"backend": "dnf", "availability": {"Ok": {"unavailable": "not installed"}}, "capabilities": []}
        ]});
        let output = human(&sources, 120, true);
        assert!(output.contains("not installed"), "{output}");
        let repositories = json!({"repositories": [], "errors": ["Synthetic error"]});
        assert!(human(&repositories, 120, false).contains("! Synthetic error"));
        let details =
            json!({"package": {"id": {"name": "tool", "backend": "apt"}, "update": "available"}});
        assert!(human(&details, 80, true).contains("available"));
        let inspection = json!({"inspection": {"command": "tool", "candidates": []},
            "failures": [{"backend": "dnf", "error": {"Execution": "TimedOut"}}]});
        assert!(human(&inspection, 80, false).contains("! DNF: The package manager took too long"));
        assert_eq!(error_text(&json!({"Execution": "Disabled"})), "Disabled");
    }
    #[test]
    fn plain_wording_display_names_and_narrow_layouts() {
        // Ownership records read as sentences in every state.
        let candidate = json!({"path": "/usr/games/cowsay"});
        let owner = |state: &str, path: &str| json!({"manager": "apt", "native_name": "cowsay", "state": state, "path": path});
        for (owner, expected) in [
            (
                owner("known", "/usr/games/cowsay"),
                "Installed by cowsay (APT)",
            ),
            (
                owner("known", "/usr/share/cowsay"),
                "Installed by cowsay (APT) as /usr/share/cowsay",
            ),
            (
                owner("ambiguous", "/usr/games/cowsay"),
                "Installed by cowsay (APT), which matches several installed packages:",
            ),
            (
                owner("unmatched", "/usr/games/cowsay"),
                "APT lists /usr/games/cowsay under cowsay, but cowsay isn't in the installed list",
            ),
            (
                owner("unknown", "/usr/games/cowsay"),
                "No package manager claims /usr/games/cowsay",
            ),
        ] {
            assert_eq!(owner_line(&owner, &candidate), expected);
        }
        // Display names lead; the id follows when it differs.
        let named = |backend: &str, display: &str| json!({"id": {"name": "org.example.App", "backend": backend}, "display_name": display});
        assert_eq!(
            package_name(&named("flatpak", "Example")),
            ("Example (org.example.App)".into(), true)
        );
        assert_eq!(
            package_name(&named("fwupd", "System Firmware")),
            ("System Firmware".into(), false)
        );
        assert_eq!(
            package_name(&named("flatpak", " ")),
            ("org.example.App".into(), false)
        );
        // Info: the display name, plain states and the new version.
        let details = json!({"package": {"id": {"name": "org.example.App", "backend": "flatpak",
            "scope": {"user": {"uid": 1000}}}, "display_name": "Example", "installed_version": "1",
            "candidate_version": "2", "update": "available"}});
        let output = human(&details, 80, false);
        for expected in [
            "Example (org.example.App)",
            "Source  Flatpak",
            "Scope  user",
            "Update  available (2)",
        ] {
            assert!(output.contains(expected), "{output}");
        }
        let current = json!({"package": {"id": {"name": "tool", "backend": "apt", "scope": "system"},
            "installed_version": "1", "update": "current"}});
        let output = human(&current, 80, true);
        assert!(
            output.contains("up to date") && output.contains("system"),
            "{output}"
        );
        assert_eq!(
            scope_text(&json!({"environment": {"path": "/opt/tools"}})),
            "environment /opt/tools"
        );
        // Narrow tables abbreviate Flatpak scopes; the legend lists only
        // states that appear.
        let packages = json!({"packages": [
            {"id": {"name": "org.example.App", "backend": "flatpak", "scope": "system"},
             "display_name": "Example", "installed_version": "1"},
            {"id": {"name": "org.example.App", "backend": "flatpak", "scope": {"user": {"uid": 1}}},
             "installed_version": "1", "candidate_version": "2", "update": "available"}
        ]});
        let narrow = human(&packages, 30, false);
        assert!(narrow.contains("flatpak·sys"), "{narrow}");
        assert!(narrow.contains("flatpak·user"), "{narrow}");
        assert!(
            narrow.contains("● installed   ↑ update available"),
            "{narrow}"
        );
        assert!(!narrow.contains("not installed"), "{narrow}");
        let colored = human(&packages, 120, true);
        assert!(colored.contains("flatpak (system)"), "{colored}");
        assert!(colored.contains("Example"), "{colored}");
        // Sources: a failed check says why, and narrow terminals keep the
        // details on an indented line.
        let sources = json!({"sources": [
            {"backend": "apt", "availability": {"Err": {"Execution": {"Failed": {"code": 100,
                "stderr": "W: noise\nE: The lock is held\n"}}}}, "capabilities": []},
            {"backend": "dnf", "availability": {"Err": {"Execution": {"Failed": {"code": 1,
                "stderr": ""}}}}, "capabilities": []},
            {"backend": "snap", "availability": {"Err": {"Execution": "TimedOut"}}, "capabilities": []},
            {"backend": "npm", "availability": {"Ok": "available"},
             "capabilities": ["search", "details", "installed", "install", "remove", "upgrade"]}
        ]});
        let wide = human(&sources, 120, false);
        for expected in [
            "failed",
            "Couldn't run APT: E: The lock is held",
            "Couldn't run DNF: it exited with code 1.",
            "Couldn't check Snap: The package manager took too long",
        ] {
            assert!(wide.contains(expected), "{wide}");
        }
        let narrow = human(&sources, 40, false);
        assert!(narrow.contains("\n    Couldn't run APT:"), "{narrow}");
        assert!(narrow.lines().all(|line| line.width() <= 40), "{narrow}");
        assert!(narrow.contains("upgrade"), "{narrow}");
        // Words wrap at the width; a word that can't fit is cut to it.
        assert_eq!(wrap("one two three", 8), ["one two", "three"]);
        assert_eq!(wrap("abcdefghij xy", 5), ["abcd…", "xy"]);
        assert!(wrap("   ", 5).is_empty());
        // A short form replaces the long one only when the long one can't fit.
        let both = with_short("flatpak (system)", "flatpak·sys");
        assert_eq!(cell(&both, 20).trim_end(), "flatpak (system)");
        assert_eq!(cell(&both, 12).trim_end(), "flatpak·sys");
        assert_eq!(long_text(&both), "flatpak (system)");
        // Removing everything from a source names it plainly.
        assert_eq!(everything_from("apt"), "all APT packages");
        assert_eq!(everything_from("docker"), "all Docker images");
        assert_eq!(everything_from("fwupd"), "all firmware");
    }
    #[test]
    fn sources_list_usable_ones_first_and_errors_name_their_source_once() {
        let sources = json!({"sources": [
            {"backend": "bun", "availability": {"Ok": {"unavailable": "Bun not found"}}, "capabilities": []},
            {"backend": "apt", "availability": {"Ok": "available"}, "capabilities": ["search"]}
        ]});
        let output = human(&sources, 120, false);
        assert!(
            output.find("apt").unwrap() < output.find("bun").unwrap(),
            "{output}"
        );
        let failure = json!({"backend": "flatpak", "error": {"InvalidResponse": {"backend": "flatpak", "reason": "bad"}}});
        assert_eq!(failure_text(&failure), "Flatpak: bad");
        let failure = json!({"backend": "dnf", "error": {"Unavailable": {"backend": "dnf", "reason": "missing"}}});
        assert_eq!(failure_text(&failure), "DNF isn't available: missing");
        let repositories = json!({"repositories": [
            {"title": "Flathub", "backend": "flatpak", "scope": {"user": {"uid": 1000}}, "enabled": true, "url": "https://dl.flathub.org/repo/"}
        ]});
        let output = human(&repositories, 120, false);
        assert!(
            output.contains("user") && !output.contains("uid"),
            "{output}"
        );
    }
    #[test]
    fn batch_summaries_count_what_happened() {
        let op =
            |result: Value| json!({"operation": {"refresh": {"backend": "apt"}}, "result": result});
        let install = |result: Value| json!({"operation": {"install": {"name": "tool", "backend": "apt", "architecture": "all", "scope": "system"}}, "result": result});
        for (operations, expected) in [
            (vec![], "Nothing to do"),
            (vec![op(json!({"Ok": {}}))], "Package lists refreshed."),
            (
                vec![op(json!({"Ok": {}}))],
                "Run `pkd upgrade` to install updates.",
            ),
            (
                vec![install(json!({"Ok": {}})), install(json!({"Ok": {}}))],
                "Done. 2 changes applied.",
            ),
            (vec![install(json!({"Err": "x"}))], "The change failed."),
            (
                vec![install(json!({"Err": "x"})), install(json!({"Err": "y"}))],
                "All 2 changes failed.",
            ),
            (
                vec![install(json!({"Ok": {}})), install(json!({"Err": "y"}))],
                "1 of 2 changes failed",
            ),
        ] {
            for color in [false, true] {
                let summary = operations_summary(&json!({"operations": operations}), color);
                assert!(summary.contains(expected), "{summary}");
            }
        }
    }
    #[test]
    fn details_failures_and_operations_are_readable() {
        let id = json!({"name":"synthetic", "backend":"fixture", "architecture":"all", "scope":"system"});
        assert!(human(
            &json!({"package":{"id":id}, "description":"Test", "dependencies":["library"]}),
            80,
            false
        )
        .contains("Dependencies  library"));
        let uninstalled = human(
            &json!({"package":{"id":{"name":"synthetic","backend":"fixture","architecture":"all","scope":"system"},"installed_version":null}}),
            80,
            false,
        );
        assert!(uninstalled.contains("Installed  no"));
        let installed = human(
            &json!({"package":{"id":{"name":"synthetic","backend":"fixture","architecture":"all","scope":"system"},"installed_version":"1.0"}}),
            80,
            false,
        );
        assert!(installed.contains("Installed  1.0"));
        assert!(human(&json!({"sources":[{"backend":"fixture","availability":{"Ok":"available"},"capabilities":["search"]}]}), 100, false).contains("available"));
        assert!(human(
            &json!({"packages":[],"failures":[{"backend":"fixture","error":"offline"}]}),
            80,
            false
        )
        .contains("fixture: offline"));
        let ops = json!({"operations":[{"operation":{"install":id},"result":{"Ok":{"cancellation_deferred":true}}},{"operation":{"remove":id},"result":{"Err":"denied"}},{"operation":{"upgrade":id},"result":{"Ok":{}}},{"operation":{"refresh":{"backend":"fixture"}},"result":{"Ok":{}}}]});
        let output = human(&ops, 80, false);
        for expected in [
            "✓ Install synthetic  fixture",
            "✗ Remove synthetic",
            "  denied",
            "✓ Update synthetic",
            "✓ Refresh fixture",
            "after you cancelled",
            "1 of 4 changes failed",
        ] {
            assert!(output.contains(expected), "{output}");
        }
        let denied = json!({"operations":[{"operation":{"upgrade_all":{"backend":"apt"}},"result":{"Err":{"Execution":"AuthorizationDenied"}}}]});
        let output = human(&denied, 80, false);
        assert!(output.contains("✗ Update all APT packages"), "{output}");
        assert!(
            output.contains("Administrator access was denied"),
            "{output}"
        );
        assert!(output.contains("The change failed."), "{output}");
        assert_eq!(
            error_text(&json!({"Unsupported":{"backend":"npm","capability":"refresh"}})),
            "npm can't refresh package lists."
        );
        assert!(human(&json!({"operations":[]}), 80, false).contains("Nothing to do"));
        assert!(human(&json!({"error":"declined"}), 80, false).contains("✗ declined"));
        assert!(human(&json!({"message":"Retry", "error":1}), 80, false).contains("Retry"));
        assert_eq!(value(&json!(2)), "2");
    }
}
