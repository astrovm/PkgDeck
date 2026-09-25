//! Human-readable command-line presentation.
use pkgdeck_core::package::{Operation, Scope};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

pub fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
/// ASCII markers remain legible with stock terminal fonts, including NO_COLOR.
pub fn package_marker(installed: bool, update: bool) -> &'static str {
    if update {
        "[^]"
    } else if installed {
        "[x]"
    } else {
        "[ ]"
    }
}
pub fn scope(scope: &Scope) -> String {
    match scope {
        Scope::System => "System".into(),
        Scope::User { uid } => format!("User {uid}"),
        Scope::Environment { path } => path.display().to_string(),
    }
}
pub fn operation(op: &Operation) -> String {
    let (verb, id) = match op {
        Operation::Refresh { backend } => {
            return format!("Refresh metadata from {}", clean(backend))
        }
        Operation::UpgradeAll { backend } => {
            return format!("Upgrade all packages from {}", clean(backend))
        }
        Operation::Clean(id) => {
            return format!("Clean {} with {}", clean(&id.key), clean(&id.backend))
        }
        Operation::Install(id) => ("Install", id),
        Operation::Remove(id) => ("Remove", id),
        Operation::Upgrade(id) => ("Upgrade", id),
    };
    format!(
        "{verb} {}\n  Source: {} · Architecture: {} · Scope: {}",
        clean(id.reference.as_deref().unwrap_or(&id.name)),
        clean(&id.backend),
        clean(&id.architecture),
        clean(&scope(&id.scope))
    )
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
struct Paint(bool);
impl Paint {
    fn wrap(self, code: &str, text: &str) -> String {
        if self.0 && !text.is_empty() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.into()
        }
    }
    fn bold(self, text: &str) -> String {
        self.wrap("1", text)
    }
    fn dim(self, text: &str) -> String {
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
fn failure_line(paint: Paint, label: &str, detail: &str) -> String {
    format!("\n{} {detail}", paint.yellow(&format!("[!] {label}")))
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
        if let Some(failures) = data["failures"].as_array() {
            for failure in failures {
                let unsupported = failure["error"].get("Unsupported").is_some()
                    || failure["error"].get("unsupported").is_some();
                output.push_str(&if unsupported {
                    format!("\n{} {}", paint.dim("[-] Unsupported:"), value(failure))
                } else {
                    failure_line(paint, "Failed:", &value(failure))
                });
            }
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
                "\n\n{} {}\n{}",
                paint.bold(&format!(
                    "{count} package{}",
                    if count == 1 { "" } else { "s" }
                )),
                paint.dim("· run `pkd info <name>` for details"),
                paint.dim("[ ] Not installed   [x] Installed   [^] Update available")
            ));
        }
        if let Some(failures) = data["failures"].as_array() {
            for failure in failures {
                output.push_str(&failure_line(paint, "Source failed:", &value(failure)));
            }
        }
    } else if let Some(export) = data.get("manifest_export") {
        output.push_str(&paint.green(&format!(
            "Exported {} packages to {}",
            value(&export["packages"]),
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
                    (value(&entry["status"]), plain as Style),
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
                                    "\n    Exact copy: {} · {} · {}",
                                    value(&package["backend"]),
                                    value(&package["name"]),
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
            output.push_str(&paint.bold(&format!("{} known duplicate groups\n", groups.len())));
            for group in groups {
                output.push_str(&format!("\n{}\n", paint.bold(&value(&group["key"]))));
                if let Some(copies) = group["copies"].as_array() {
                    for copy in copies {
                        output.push_str(&format!(
                            "  {} · {} · {} · {}\n",
                            value(&copy["package"]["backend"]),
                            value(&copy["package"]["name"]),
                            value(&copy["package"]["scope"]),
                            value(&copy["installed_version"])
                        ));
                    }
                }
            }
        }
        if let Some(leftovers) = report["leftovers"].as_array() {
            output.push_str(&paint.bold(&format!(
                "\n{} manager-reported residual files\n",
                leftovers.len()
            )));
            for row in leftovers {
                output.push_str(&format!(
                    "  {} · {} · {} · {} bytes\n",
                    value(&row["manager"]),
                    value(&row["native_name"]),
                    value(&row["path"]),
                    value(&row["size_bytes"])
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
                        format!(
                            "{} {}",
                            if enabled { "[x]" } else { "[ ]" },
                            value(&r["title"])
                        ),
                        if enabled {
                            plain as Style
                        } else {
                            Paint::dim as Style
                        },
                    ),
                    (value(&r["backend"]), Paint::dim as Style),
                    (value(&r["scope"]), Paint::dim as Style),
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
                output.push_str(&failure_line(paint, "", &value(error)));
            }
        }
    } else if let Some(sources) = data["sources"].as_array() {
        let rows = sources
            .iter()
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
        if operations.is_empty() {
            output.push_str("Nothing to do. No package changes are needed.");
        }
        for item in operations {
            let op: Operation =
                serde_json::from_value(item["operation"].clone()).expect("typed operation");
            output.push_str(&operation(&op));
            if let Some(error) = item["result"].get("Err") {
                output.push_str(&format!(
                    "\n  {} {}\n",
                    paint.red("[!] Failed:"),
                    value(error)
                ));
            } else {
                output.push_str(&format!("\n  {}", paint.green("[OK] Completed")));
                if item["result"]["Ok"]["cancellation_deferred"] == true {
                    output.push_str(" after cancellation was requested; changes were kept");
                }
                output.push('\n');
            }
        }
    } else if data["error"] == "confirmation_declined" {
        output.push_str(&paint.dim(&value(&data["message"])));
    } else {
        output.push_str(&format!(
            "{} {}",
            paint.red("[!] Error:"),
            value(data.get("message").unwrap_or(&data["error"]))
        ));
        if let Some(matches) = data["error"]["Ambiguous"].as_array() {
            for id in matches {
                output.push_str(&format!(
                    "\n  {} · {} · {} · {}",
                    value(&id["backend"]),
                    value(&id["name"]),
                    value(&id["architecture"]),
                    value(&id["scope"])
                ));
            }
        }
    }
    if data.get("inspection").is_some() || data.get("audit").is_some() {
        if let Some(failures) = data["failures"].as_array() {
            for failure in failures {
                output.push_str(&failure_line(paint, "Source failed:", &value(failure)));
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
        assert!(preview.contains("ambiguous"));
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
        assert!(output.contains("[x] Synthetic"));
        assert!(output.contains("[ ] Disabled"));
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
        assert_eq!(package_marker(false, false), "[ ]");
        assert_eq!(package_marker(true, false), "[x]");
        assert_eq!(package_marker(true, true), "[^]");
        let output = human(
            &json!({"packages":[{"id":{"name":"fixture","backend":"apt"},"installed_version":"1","update":"available"}]}),
            100,
            false,
        );
        assert!(output.contains("[^] fixture"));
        assert!(output.contains("[^] Update available"));
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
        assert!(incomplete.contains("Source failed"));
        let ambiguous = human(
            &json!({"error":{"Ambiguous":[
                {"backend":"apt","name":"fixture","architecture":"amd64","scope":"system"},
                {"backend":"apt","name":"fixture","architecture":"i386","scope":"system"}
            ]},"message":"2 packages match; select a backend, architecture, or scope"}),
            80,
            false,
        );
        assert!(ambiguous.contains("apt · fixture · amd64 · system"));
        assert!(ambiguous.contains("apt · fixture · i386 · system"));
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
        assert!(output.contains("Exact copy: apt · fixture · system"));
        let audited = json!({"audit":{"groups":[{"key":"fixture","copies":[{"package":{"backend":"apt","name":"fixture","scope":"system"},"installed_version":"1"}]}],
            "leftovers":[{"manager":"apt","native_name":"old-fixture","path":"/etc/old.conf","size_bytes":4}],
            "data_note":"Unknown data remains unknown."}});
        let output = human(&audited, 100, false);
        assert!(output.contains("1 known duplicate groups"));
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
            "Clean autoremove with apt"
        );
        assert_eq!(
            operation(&Operation::UpgradeAll {
                backend: "homebrew".into(),
            }),
            "Upgrade all packages from homebrew"
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
        assert!(output.contains("Unsupported:"));
        assert!(output.contains("Failed:"));

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
        .contains("Source failed:"));
        let ops = json!({"operations":[{"operation":{"install":id},"result":{"Ok":{"cancellation_deferred":true}}},{"operation":{"remove":id},"result":{"Err":"denied"}},{"operation":{"upgrade":id},"result":{"Ok":{}}},{"operation":{"refresh":{"backend":"fixture"}},"result":{"Ok":{}}}]});
        let output = human(&ops, 80, false);
        for expected in [
            "Install synthetic",
            "Remove synthetic",
            "Upgrade synthetic",
            "Refresh metadata",
            "Failed: denied",
            "after cancellation",
            "Scope: System",
        ] {
            assert!(output.contains(expected));
        }
        assert!(human(&json!({"operations":[]}), 80, false).contains("Nothing to do"));
        assert!(human(&json!({"error":"declined"}), 80, false).contains("Error: declined"));
        assert!(human(&json!({"message":"Retry", "error":1}), 80, false).contains("Retry"));
        assert_eq!(value(&json!(2)), "2");
        assert_eq!(scope(&Scope::User { uid: 42 }), "User 42");
        assert_eq!(
            scope(&Scope::Environment {
                path: "/synthetic".into()
            }),
            "/synthetic"
        );
    }
}
