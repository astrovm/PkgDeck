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
fn table(headers: &[&str], rows: &[Vec<String>], width: usize) -> String {
    // Drop trailing columns on narrow terminals; full metadata remains in `info`/JSON.
    let columns = if width < 60 {
        2
    } else if width < 90 {
        3
    } else {
        headers.len()
    }
    .min(headers.len());
    let available = width.saturating_sub(2 * (columns - 1));
    let shares = match columns {
        2 => vec![65, 35],
        3 => vec![34, 26, 40],
        _ => vec![24, 14, 18, 44],
    };
    let mut widths: Vec<_> = shares.iter().map(|share| available * share / 100).collect();
    widths[columns - 1] += available - widths.iter().sum::<usize>();
    let line = |cells: &[String]| {
        cells
            .iter()
            .zip(&widths)
            .map(|(text, w)| cell(text, *w))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let mut result = line(&headers.iter().map(|s| (*s).into()).collect::<Vec<_>>());
    result.push('\n');
    result.push_str(&"─".repeat(width));
    for row in rows {
        result.push('\n');
        result.push_str(&line(row));
    }
    result
}
pub fn human(data: &Value, width: usize, color: bool) -> String {
    let width = width.clamp(24, 160);
    let heading = if color {
        "\x1b[1;34mPkgDeck\x1b[0m"
    } else {
        "PkgDeck"
    };
    let mut output = format!("{heading}\n\n");
    if let Some(items) = data["items"].as_array() {
        if items.is_empty() {
            output.push_str("Nothing to clean.");
        } else {
            let rows = items
                .iter()
                .map(|item| {
                    vec![
                        format!(
                            "{}:{}",
                            value(&item["id"]["backend"]),
                            value(&item["id"]["key"])
                        ),
                        value(&item["title"]),
                        value(&item["summary"]),
                    ]
                })
                .collect::<Vec<_>>();
            output.push_str(&table(&["KEY", "CLEANUP", "SUMMARY"], &rows, width));
            output.push_str("\n\nReview a plan with --json. Run selected keys with `pkd clean <key>` or every plan with `pkd clean --all`.");
        }
        if let Some(failures) = data["failures"].as_array() {
            for failure in failures {
                let unsupported = failure["error"].get("Unsupported").is_some()
                    || failure["error"].get("unsupported").is_some();
                output.push_str(&format!(
                    "\n{} {}",
                    if unsupported {
                        "[-] Unsupported:"
                    } else {
                        "[!] Failed:"
                    },
                    value(failure)
                ));
            }
        }
    } else if let Some(packages) = data["packages"].as_array() {
        let rows = packages
            .iter()
            .map(|p| {
                vec![
                    format!(
                        "{} {}",
                        package_marker(
                            !p["installed_version"].is_null(),
                            p["update"] == "available"
                        ),
                        value(
                            if matches!(
                                p["id"]["backend"].as_str(),
                                Some("fwupd" | "docker" | "podman")
                            ) {
                                &p["display_name"]
                            } else {
                                &p["id"]["name"]
                            }
                        )
                    ),
                    value(&p["id"]["backend"]),
                    value(if p["installed_version"].is_null() {
                        &p["candidate_version"]
                    } else {
                        &p["installed_version"]
                    }),
                    value(&p["summary"]),
                ]
            })
            .collect::<Vec<_>>();
        output.push_str(&table(
            &["NAME", "SOURCE", "VERSION", "SUMMARY"],
            &rows,
            width,
        ));
        output.push_str(&format!(
            "\n\n{} packages · Use pkd info <name> for details.\n[ ] Not installed   [x] Installed   [^] Update available",
            rows.len()
        ));
        if let Some(failures) = data["failures"].as_array() {
            for failure in failures {
                output.push_str(&format!("\n[!] Source failed: {}", value(failure)));
            }
        }
    } else if let Some(export) = data.get("manifest_export") {
        output.push_str(&format!(
            "Exported {} packages to {}",
            value(&export["packages"]),
            value(&export["path"])
        ));
    } else if let Some(preview) = data.get("manifest_preview") {
        let rows = preview["packages"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|entry| {
                vec![
                    value(&entry["package"]["name"]),
                    value(&entry["package"]["backend"]),
                    value(&entry["status"]),
                    value(&entry["reason"]),
                ]
            })
            .collect::<Vec<_>>();
        output.push_str(&table(
            &["PACKAGE", "SOURCE", "STATUS", "DETAIL"],
            &rows,
            width,
        ));
        for entry in preview["packages"].as_array().into_iter().flatten() {
            for proposed in entry["proposed_changes"].as_array().into_iter().flatten() {
                output.push_str(&format!("\n  {}", value(&proposed["detail"])));
            }
        }
    } else if let Some(report) = data.get("inspection") {
        output.push_str(&format!(
            "Command: {}\nEnvironment: {}\nPATH: {}\nResolved: {}\n",
            value(&report["command"]),
            value(&report["environment"]),
            value(&report["path"]),
            value(&report["resolved"])
        ));
        if let Some(candidates) = report["candidates"].as_array() {
            for candidate in candidates {
                output.push_str(&format!(
                    "\n{} [{}]",
                    value(&candidate["path"]),
                    value(&candidate["state"])
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
        output.push_str(&format!("\n\n{}", value(&report["ownership_note"])));
    } else if let Some(report) = data.get("audit") {
        if let Some(groups) = report["groups"].as_array() {
            output.push_str(&format!("{} known duplicate groups\n", groups.len()));
            for group in groups {
                output.push_str(&format!("\n{}\n", value(&group["key"])));
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
            output.push_str(&format!(
                "\n{} manager-reported residual files\n",
                leftovers.len()
            ));
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
        output.push_str(&format!("\n{}", value(&report["data_note"])));
    } else if data.get("package").is_some() {
        let p = &data["package"];
        output.push_str(&format!(
            "{}\n{}\n\n",
            value(&p["id"]["name"]),
            value(&data["description"])
        ));
        for (label, field) in [
            ("Source", &p["id"]["backend"]),
            ("Reference", &p["id"]["reference"]),
            ("Architecture", &p["id"]["architecture"]),
            ("Scope", &p["id"]["scope"]),
            ("Installed", &p["installed_version"]),
            ("Candidate", &p["candidate_version"]),
            ("Update", &p["update"]),
            ("Homepage", &data["homepage"]),
            ("Dependencies", &data["dependencies"]),
        ] {
            if label == "Reference" && field.is_null() {
                continue;
            }
            // A missing installed version means not installed, matching the
            // state legend; every other null stays a plain Unavailable.
            let text = if label == "Installed" && field.is_null() {
                "not installed".into()
            } else {
                value(field)
            };
            output.push_str(&format!("{label:>12}  {text}\n"));
        }
    } else if let Some(repositories) = data["repositories"].as_array() {
        let rows = repositories
            .iter()
            .map(|r| {
                vec![
                    format!(
                        "{} {}",
                        if r["enabled"] == true { "[x]" } else { "[ ]" },
                        value(&r["title"])
                    ),
                    value(&r["backend"]),
                    value(&r["scope"]),
                    value(&r["url"]),
                ]
            })
            .collect::<Vec<_>>();
        output.push_str(&table(
            &["REPOSITORY", "SOURCE", "SCOPE", "URL"],
            &rows,
            width,
        ));
        if let Some(errors) = data["errors"].as_array() {
            for error in errors {
                output.push_str(&format!("\n[!] {}", value(error)));
            }
        }
    } else if let Some(sources) = data["sources"].as_array() {
        let rows = sources
            .iter()
            .map(|s| {
                vec![
                    value(&s["backend"]),
                    value(&s["availability"]),
                    value(&s["capabilities"]),
                ]
            })
            .collect::<Vec<_>>();
        output.push_str(&table(
            &["SOURCE", "AVAILABILITY", "CAPABILITIES"],
            &rows,
            width,
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
                output.push_str(&format!("\n  [!] Failed: {}\n", value(error)));
            } else {
                output.push_str("\n  [OK] Completed");
                if item["result"]["Ok"]["cancellation_deferred"] == true {
                    output.push_str(" after cancellation; native changes were not rolled back");
                }
                output.push('\n');
            }
        }
    } else {
        output.push_str(&format!(
            "[!] Error: {}",
            value(data.get("message").unwrap_or(&data["error"]))
        ));
    }
    if data.get("inspection").is_some() || data.get("audit").is_some() {
        if let Some(failures) = data["failures"].as_array() {
            for failure in failures {
                output.push_str(&format!("\n[!] Source failed: {}", value(failure)));
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
            for line in output.lines().skip(2).take(3) {
                assert!(line.width() <= width);
            }
            assert!(output.contains("fixture"));
        }
        assert!(human(&data, 100, true).starts_with("\x1b[1;34mPkgDeck\x1b[0m"));
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
    fn inspection_and_audit_render_exact_read_only_evidence() {
        let command = json!({"inspection":{"command":"tool","environment":"Native host","path":"/first:/second",
            "resolved":"/first/tool","ownership_note":"Unknown remains unknown.","candidates":[
                {"path":"/first/tool","target":"/target/tool","state":"executable","owners":[
                    {"manager":"apt","native_name":"fixture:amd64","state":"known","packages":[
                        {"backend":"apt","name":"fixture","scope":"system"}]}]}]}});
        let output = human(&command, 100, false);
        assert!(output.contains("Resolved: /first/tool"));
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
        assert!(output.contains("Review a plan with --json"));
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
