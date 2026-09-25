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
/// Short imperative title for a planned change, such as "Install neovim".
pub fn operation_title(op: &Operation) -> String {
    match op {
        Operation::Refresh { backend } => format!("Refresh {}", clean(backend)),
        Operation::UpgradeAll { backend } => format!("Update all {} packages", clean(backend)),
        Operation::Clean(id) => format!("Clean up {}", clean(&id.key)),
        Operation::Install(id) => format!("Install {}", target(id)),
        Operation::Remove(id) => format!("Remove {}", target(id)),
        Operation::Upgrade(id) => format!("Update {}", target(id)),
    }
}
/// What PkgDeck is doing right now, for the spinner.
pub fn operation_progress(op: &Operation) -> String {
    match op {
        Operation::Refresh { backend } => format!("Refreshing {}", clean(backend)),
        Operation::UpgradeAll { backend } => format!("Updating all {} packages", clean(backend)),
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
        Operation::Clean(id) => clean(&id.backend),
        Operation::Install(id) | Operation::Remove(id) | Operation::Upgrade(id) => {
            let mut parts = vec![clean(&id.backend)];
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
                    field(inner, "backend"),
                    capability_verb(inner["capability"].as_str().unwrap_or_default())
                ),
                "Unavailable" => format!(
                    "{} isn't available: {}",
                    field(inner, "backend"),
                    field(inner, "reason")
                ),
                "InvalidResponse" => {
                    format!("{}: {}", field(inner, "backend"), field(inner, "reason"))
                }
                "Incomplete" => {
                    let failed: Vec<_> = inner
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(failure_text)
                        .collect();
                    format!(
                        "Some sources didn't answer, so PkgDeck won't guess which package you meant. {} Try again, or pick a source with --from.",
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
                let stderr = result["stderr"].as_str().unwrap_or_default();
                let tail: Vec<_> = stderr
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .collect();
                let tail = tail[tail.len().saturating_sub(3)..].join(" ");
                let code = result["code"]
                    .as_i64()
                    .map_or_else(|| "was stopped".into(), |code| format!("exited with code {code}"));
                if tail.is_empty() {
                    format!("The package manager {code}.")
                } else {
                    format!("The package manager {code}: {}", clean(&tail))
                }
            } else {
                value(error)
            }
        }
    }
}
fn failure_text(failure: &Value) -> String {
    match failure["backend"].as_str() {
        Some(backend) => {
            let text = error_text(&failure["error"]);
            // Many errors already start with their source; don't repeat it.
            if text.starts_with(&format!("{backend}:")) || text.starts_with(&format!("{backend} "))
            {
                text
            } else {
                format!("{}: {text}", clean(backend))
            }
        }
        None => value(failure),
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
        paint.green("Package lists are up to date.")
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
fn cell(text: &str, width: usize) -> String {
    let text = clean(text);
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
                .map(|row| clean(&row[i].0).width())
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
                .map(|failure| value(&failure["backend"]))
                .collect();
            output.push_str(&format!(
                "\n\n{}",
                paint.dim(&format!("No cleanup tasks in: {}", names.join(", ")))
            ));
        }
    } else if let Some(packages) = data["packages"].as_array() {
        let rows = packages
            .iter()
            .map(|p| {
                let installed = !p["installed_version"].is_null();
                let update = p["update"] == "available";
                let marker = package_marker(installed, update);
                let name = value(
                    if matches!(
                        p["id"]["backend"].as_str(),
                        Some("fwupd" | "docker" | "podman")
                    ) {
                        &p["display_name"]
                    } else {
                        &p["id"]["name"]
                    },
                );
                // Flatpak can install the same app per user and system-wide.
                let source = match (p["id"]["backend"].as_str(), scope_label(&p["id"]["scope"])) {
                    (Some("flatpak"), Some(scope)) => format!("flatpak ({scope})"),
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
                let marker_style: Style = if update {
                    Paint::cyan
                } else if installed {
                    Paint::green
                } else {
                    Paint::dim
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
            output.push_str(&format!(
                "\n\n{}{}\n{}",
                paint.bold(&format!(
                    "{count} package{}",
                    if count == 1 { "" } else { "s" }
                )),
                paint.dim(". Run `pkd info <name>` for details."),
                paint.dim("○ not installed   ● installed   ↑ update available")
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
                        output.push_str(&format!(
                            "\n  Owner of {}: {} {} ({})",
                            value(&owner["path"]),
                            value(&owner["manager"]),
                            value(&owner["native_name"]),
                            value(&owner["state"])
                        ));
                        if let Some(packages) = owner["packages"].as_array() {
                            for package in packages {
                                output.push_str(&format!(
                                    "\n    Exact copy: {} from {}, {}",
                                    value(&package["name"]),
                                    value(&package["backend"]),
                                    value(&package["scope"])
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
                            value(&copy["package"]["backend"]),
                            value(&copy["package"]["scope"]),
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
                    value(&row["manager"]),
                    value(&row["native_name"])
                ));
            }
        }
        output.push_str(&format!("\n{}", paint.dim(&value(&report["data_note"]))));
    } else if data.get("package").is_some() {
        let p = &data["package"];
        output.push_str(&paint.bold(&value(&p["id"]["name"])));
        let description = value(&data["description"]);
        if !data["description"].is_null() && !description.trim().is_empty() {
            output.push_str(&format!("\n{description}"));
        }
        output.push_str("\n\n");
        for (label, field) in [
            ("Source", &p["id"]["backend"]),
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
                "not installed".into()
            } else if field.is_null()
                || field.as_array().is_some_and(Vec::is_empty)
                || (label == "Update" && field == "unknown")
            {
                continue;
            } else {
                value(field)
            };
            let text = if label == "Update" && text == "available" {
                paint.cyan(&text)
            } else {
                text
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
                let availability = value(&s["availability"]);
                let (status, detail) = match availability.split_once(": ") {
                    Some((status, reason)) => (status.to_string(), reason.to_string()),
                    None => (availability.clone(), value(&s["capabilities"])),
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
        output.push_str(&table(
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
            // Each match with the flag that picks it.
            let flags: Vec<_> = matches
                .iter()
                .map(|id| format!("--from {}", value(&id["backend"])))
                .collect();
            let width = flags.iter().map(|flag| flag.width()).max().unwrap_or(0);
            for (id, flag) in matches.iter().zip(flags) {
                let mut parts = vec![value(&id["name"])];
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
        assert!(incomplete.contains("! apt: synthetic failure"));
        let ambiguous = human(
            &json!({"error":{"Ambiguous":[
                {"backend":"apt","name":"fixture","architecture":"amd64","scope":"system"},
                {"backend":"apt","name":"fixture","architecture":"i386","scope":"system"}
            ]},"message":"2 packages match; select a backend, architecture, or scope"}),
            80,
            false,
        );
        assert!(ambiguous.contains("--from apt  fixture, amd64, system"));
        assert!(ambiguous.contains("--from apt  fixture, i386, system"));
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
        assert!(output.contains("Exact copy: fixture from apt, system"));
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
            "Clean up autoremove  apt"
        );
        assert_eq!(
            operation(&Operation::UpgradeAll {
                backend: "homebrew".into(),
            }),
            "Update all homebrew packages"
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
        assert!(output.contains("No cleanup tasks in: snap"));
        assert!(output.contains("homebrew: synthetic failure"));

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
                "apt, i386",
            ),
            (
                Operation::Remove(id("flatpak", "x86_64", Scope::System)),
                "Remove tool",
                "Removing tool",
                "flatpak, system",
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
                "Refresh apt",
                "Refreshing apt",
                "",
            ),
            (
                Operation::UpgradeAll {
                    backend: "snap".into(),
                },
                "Update all snap packages",
                "Updating all snap packages",
                "",
            ),
            (
                Operation::Clean(pkgdeck_core::package::CleanupId {
                    backend: "apt".into(),
                    key: "autoclean".into(),
                }),
                "Clean up autoclean",
                "Cleaning up autoclean",
                "apt",
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
                "exited with code 100: E: one E: two",
            ),
            (failed(None, ""), "The package manager was stopped."),
            (json!({"Execution": {"Unknown": 1}}), "Unknown"),
            (
                json!({"Unavailable": {"backend": "dnf", "reason": "not found"}}),
                "dnf isn't available: not found",
            ),
            (
                json!({"InvalidResponse": {"backend": "apt", "reason": "odd"}}),
                "apt: odd",
            ),
            (
                json!({"Incomplete": [{"backend": "apt", "error": {"Execution": "TimedOut"}}]}),
                "won't guess which package you meant. apt: The package manager took too long to answer. Try again, or pick a source with --from.",
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
        assert!(human(&inspection, 80, false).contains("! dnf: The package manager took too long"));
        assert_eq!(error_text(&json!({"Execution": "Disabled"})), "Disabled");
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
        assert_eq!(failure_text(&failure), "flatpak: bad");
        let failure = json!({"backend": "dnf", "error": {"Unavailable": {"backend": "dnf", "reason": "missing"}}});
        assert_eq!(failure_text(&failure), "dnf isn't available: missing");
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
            (vec![op(json!({"Ok": {}}))], "Package lists are up to date."),
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
        assert!(uninstalled.contains("Installed  not installed"));
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
        assert!(output.contains("✗ Update all apt packages"), "{output}");
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
