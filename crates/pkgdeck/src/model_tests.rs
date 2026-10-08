use super::*;
use serde_json::json;

fn row(value: Value) -> Row {
    let mut base = json!({
        "kind": "package", "name": "app", "display_name": "", "source": "apt",
        "architecture": "x86_64", "scope": "system", "scope_label": "System",
        "installed": null, "candidate": null, "update": "unknown", "summary": "",
    });
    for (key, field) in value.as_object().unwrap() {
        base[key] = field.clone();
    }
    serde_json::from_value(base).unwrap()
}

#[test]
fn pages_name_their_icons_capabilities_and_filters() {
    for page in Page::ALL {
        assert!(!page.name().is_empty());
        assert!(!page.icon().is_empty());
        assert_eq!(page.lists(), page != Page::Settings);
        assert_eq!(page.filters_sources(), page.capability().is_some());
    }
    assert_eq!(Page::Updates.capability(), Some("upgrade"));
    assert_eq!(Page::Clean.capability(), Some("clean"));
    assert_eq!(Page::Installed.capability(), Some("installed"));
    assert_eq!(Page::Search.capability(), Some("search"));
    assert_eq!(Page::Sources.capability(), None);
}

#[test]
fn rows_know_their_title_identity_and_state() {
    let firefox = row(json!({"name": "org.mozilla.firefox", "display_name": "Firefox", "source": "flatpak", "installed": "", "remote": "", "reference": "app/org.mozilla.firefox/x86_64/stable"}));
    assert_eq!(firefox.title(), "Firefox");
    assert!(firefox.is_installed());
    assert!(!firefox.has_update());
    assert!(firefox.is_flatpak_system());
    // An empty remote counts as none, as notices and progress send it.
    let target = json!({"source": "flatpak", "name": "org.mozilla.firefox", "architecture": "x86_64", "remote": "", "scope": "system", "reference": "app/org.mozilla.firefox/x86_64/stable"});
    assert_eq!(identity_of_row_value(&target), firefox.identity());
    let blank = row(json!({"display_name": "  "}));
    assert_eq!(blank.title(), "app");
    let source = row(json!({"kind": "source", "source": "flatpak"}));
    assert_eq!(source.title(), "Flatpak");
    assert!(row(json!({})).fabricated());
    assert!(!row(json!({"candidate": "1"})).fabricated());
    assert!(!row(json!({"kind": "cleanup"})).fabricated());
    assert_eq!(parse::<Vec<Row>>("not json"), vec![]);
    assert_eq!(parse::<Vec<Row>>(""), vec![]);
    let details: Details = parse(r#"{"package": {"name": "a", "installed": null}, "publisher": null, "description": "d"}"#);
    assert_eq!(details.description, "d");
    assert_eq!(details.package.unwrap().installed, None);
    assert_eq!(parse::<Vec<Row>>(r#"[{"kind": 1}]"#), vec![]);
}

#[test]
fn panels_say_what_cleanups_failures_and_sources_are() {
    let report = ReportState {
        last_success: HashMap::from([("apt".into(), 0)]),
        ..ReportState::default()
    };
    let cleanup: Details = serde_json::from_value(json!({"cleanup": {"summary": "Frees 20 MB", "preview": "a\nb"}})).unwrap();
    assert_eq!(cleanup.panel_text(&report), "Frees 20 MB\n\na\nb");
    let failure: Details = serde_json::from_value(json!({"failure": {"backend": "snap", "error": "snapd is off"}, "hint": "Start it"})).unwrap();
    assert_eq!(failure.panel_text(&report), "snapd is off\nStart it");
    let source: Details = serde_json::from_value(json!({"source": "apt", "availability": "Available"})).unwrap();
    assert!(source.panel_text(&report).starts_with("Available\nLast successful check: "));
    let never: Details = serde_json::from_value(json!({"source": "dnf", "availability": "Available"})).unwrap();
    assert_eq!(never.panel_text(&report), "Available\nLast successful check: No successful check yet");
    assert_eq!(Details::default().panel_text(&report), "");
}

#[test]
fn notices_choose_toast_or_banner_and_undo() {
    assert!(Notice::default().is_empty());
    let success: Notice = serde_json::from_value(json!({"kind": "success", "title": "Installed", "undo": true, "undo_action": "remove", "target": {}})).unwrap();
    assert!(success.is_toast() && success.can_undo() && !success.is_empty());
    let info: Notice = serde_json::from_value(json!({"kind": "info", "title": "x", "undo": true, "undo_action": "upgrade", "target": {}})).unwrap();
    assert!(info.is_toast() && !info.can_undo());
    let error: Notice = serde_json::from_value(json!({"kind": "error", "title": "Failed", "undo": true, "undo_action": "install"})).unwrap();
    assert!(!error.is_toast() && !error.can_undo());
}

#[test]
fn progress_measures_bytes_then_steps_then_fractions() {
    let bytes = Progress { transferred: 50, transfer_total: Some(200), total: 3, done: 1, ..Progress::default() };
    assert_eq!(bytes.bar(), Some(0.25));
    assert_eq!(bytes.count(), "25%");
    let steps = Progress { total: 4, done: 1, transfer_total: Some(0), ..Progress::default() };
    assert_eq!(steps.bar(), Some(0.25));
    assert_eq!(steps.count(), "1 of 4");
    let fraction = Progress { total: 1, fraction: Some(1.5), ..Progress::default() };
    assert_eq!(fraction.bar(), Some(1.0));
    assert_eq!(fraction.count(), "");
    assert_eq!(Progress::default().bar(), None);
}

#[test]
fn progress_tells_each_row_how_far_it_is() {
    let id = |name: &str| json!({"source": "apt", "name": name, "architecture": "x86_64", "scope": "system"});
    let identity = |name: &str| identity_of_row_value(&id(name));
    let single = Progress { targets: vec![id("a")], total: 1, fraction: Some(0.5), ..Progress::default() };
    assert_eq!(single.for_row(&identity("a"), "apt"), Some(Some(0.5)));
    assert_eq!(single.for_row(&identity("b"), "apt"), None);
    let unknown = Progress { targets: vec![id("a")], total: 1, ..Progress::default() };
    assert_eq!(unknown.for_row(&identity("a"), "apt"), Some(None));
    let batch = Progress {
        targets: vec![id("a"), id("b"), id("c")],
        finished: vec![id("a")],
        current: Some(id("b")),
        total: 3,
        transferred: 1,
        transfer_total: Some(4),
        ..Progress::default()
    };
    assert_eq!(batch.for_row(&identity("a"), "apt"), Some(Some(1.0)));
    assert_eq!(batch.for_row(&identity("b"), "apt"), Some(Some(0.25)));
    assert_eq!(batch.for_row(&identity("c"), "apt"), Some(Some(0.0)));
    let by_source = Progress {
        targets: vec![id("a"), id("b")],
        current_source: Some("apt".into()),
        total: 2,
        ..Progress::default()
    };
    assert_eq!(by_source.for_row(&identity("a"), "apt"), Some(None));
}

#[test]
fn activity_entries_say_what_happened() {
    let entry = |value: Value| -> Activity { serde_json::from_value(value).unwrap() };
    let install = json!({"install": {"backend": "flatpak", "name": "org.kde.krita", "scope": "system"}});
    let queued = entry(json!({"id": 1, "state": "queued", "operations": [install]}));
    assert_eq!(queued.title(), "Install org.kde.krita (Flatpak, System)");
    assert_eq!(queued.result(), "Queued");
    assert_eq!(queued.tone(), Tone::Muted);
    assert!(queued.others().is_empty());
    for (state, result, tone) in [
        ("running", "In progress", Tone::Accent),
        ("authorizing", "In progress", Tone::Accent),
        ("finished", "Completed", Tone::Success),
        ("failed", "Failed", Tone::Danger),
        ("cancelled", "Cancelled", Tone::Muted),
        ("interrupted", "Interrupted", Tone::Muted),
        ("strange", "strange", Tone::Success),
    ] {
        let e = entry(json!({"state": state}));
        assert_eq!((e.result().as_str(), e.tone()), (result, tone), "{state}");
    }
    assert!(entry(json!({"state": "running"})).running());
    let one = |outcome: Value| entry(json!({"state": "finished", "outcomes": [outcome]})).result();
    assert_eq!(one(json!("failed")), "Failed");
    assert_eq!(one(json!("cancelled")), "Cancelled");
    assert_eq!(one(json!("succeeded")), "Completed");
    let all = entry(json!({"state": "finished", "outcomes": ["ok", "ok"], "labels": ["Update a", "Update b"]}));
    assert_eq!(all.result(), "All 2 completed");
    assert_eq!(all.title(), "Update a");
    assert_eq!(all.others(), ["Update b"]);
    let mixed = entry(json!({"state": "finished", "outcomes": ["ok", {"error": "x"}, "cancelled"]}));
    assert_eq!(mixed.result(), "1 completed, 1 failed, 1 cancelled");
    assert_eq!(mixed.tone(), Tone::Danger);
    assert_eq!(entry(json!({"log": "line"})).log_text(), "line");
    assert_eq!(entry(json!({"output": ["a", 2]})).log_text(), "a\n2");
    assert_eq!(entry(json!({})).log_text(), "");
    assert_eq!(entry(json!({})).title(), "Change");
    let two_ops = entry(json!({"operations": [install, {"clean": {"backend": "apt", "key": "cache"}}]}));
    assert_eq!(two_ops.others(), ["Clean cache (APT)"]);
}

#[test]
fn operations_read_in_plain_words() {
    let text = |value: Value| operation_text(&value);
    assert_eq!(text(json!({"upgrade_all": {"backend": "apt"}})), "Update all (APT)");
    assert_eq!(text(json!({"refresh": {"backend": "snap"}})), "Refresh (Snap)");
    assert_eq!(text(json!({"remove": {"backend": "npm", "name": "ts", "scope": {"user": {"uid": 1}}}})), "Remove ts (npm, User)");
    assert_eq!(text(json!({"upgrade": {"backend": "homebrew", "name": "node", "scope": {"environment": {"path": "/opt/homebrew"}}}})), "Update node (Homebrew, /opt/homebrew)");
    assert_eq!(text(json!({"other": {}})), "Change");
    assert_eq!(text(json!({"upgrade": {"name": "x", "scope": {"odd": 1}}})), "Update x");
    assert_eq!(text(json!([])), "Change");
}

#[test]
fn confirmations_name_their_apply_button() {
    let data = |action: &str| Confirmation { action: action.into(), ..Confirmation::default() };
    assert_eq!(data("Remove GIMP").verb(), "Remove");
    assert!(data("Remove GIMP").removes());
    assert_eq!(data("Install…").verb(), "Install");
    assert_eq!(data("").verb(), "Apply");
    assert!(!data("Update all").removes());
}

#[test]
fn automatic_update_results_read_as_a_sentence() {
    let result = |updated, failed| AutoUpdateResult { updated, failed, total: updated + failed }.message();
    assert_eq!(result(0, 0), "No updates installed");
    assert_eq!(result(1, 0), "Installed 1 update");
    assert_eq!(result(3, 2), "Installed 3 updates, 2 need your attention");
}

#[test]
fn source_lines_say_where_a_row_comes_from() {
    let line = |value: Value, merged| source_line(&row(value), merged);
    assert_eq!(line(json!({"name": "firefox-esr", "display_name": "Firefox ESR"}), false), "APT, System");
    assert_eq!(line(json!({"name": "node", "display_name": "Node.js", "source": "homebrew", "scope_label": "/opt/homebrew"}), false), "node, Homebrew, /opt/homebrew");
    assert_eq!(
        line(json!({"name": "org.gimp.GIMP", "display_name": "GIMP", "source": "flatpak", "remote": "flathub", "scope": {"user": {"uid": 1}}, "reference": "app/org.gimp.GIMP/x86_64/beta"}), false),
        "Flatpak, flathub, User, beta"
    );
    assert_eq!(line(json!({"source": "flatpak", "reference": "app/x/x86_64/stable"}), true), "Flatpak, System and user");
    assert_eq!(line(json!({"source": "appimage", "adopt_with": "appimage", "scope_label": ""}), false), "AppImage, not managed");
    assert_eq!(line(json!({"kind": "cleanup", "scope_label": "System"}), false), "APT");
    assert_eq!(line(json!({"source": "docker", "scope": {"user": {"uid": 1}}}), false), "Docker images, User");
}

#[test]
fn versions_read_by_kind_and_state() {
    let version = |value: Value| version_text(&row(value));
    assert_eq!(version(json!({"kind": "cleanup", "cleanup_kind": "orphan_dependencies"})), "Dependencies");
    assert_eq!(version(json!({"kind": "cleanup", "cleanup_kind": "duplicate_copy"})), "Extra copy");
    assert_eq!(version(json!({"kind": "cleanup", "cleanup_kind": "package_cache"})), "Cache");
    assert_eq!(version(json!({"kind": "failure"})), "Failed");
    assert_eq!(version(json!({"kind": "source", "available": true})), "Available");
    assert_eq!(version(json!({"kind": "source"})), "Unavailable");
    assert_eq!(version(json!({"update": "available", "installed": "1", "candidate": "2"})), "1 → 2");
    assert_eq!(version(json!({"update": "available", "installed": "2", "candidate": "2"})), "2");
    assert_eq!(version(json!({"update": "available", "installed": "1"})), "1");
    assert_eq!(version(json!({"update": "available"})), "Unknown");
    assert_eq!(version(json!({"installed": ""})), "Unknown");
    assert_eq!(version(json!({"installed": "3"})), "3");
    assert_eq!(version(json!({"candidate": "4"})), "4");
    assert_eq!(version(json!({})), "Unknown");
}

fn catalog() -> Vec<Source> {
    serde_json::from_value(json!([
        {"source": "apt", "available": true, "capabilities": ["search", "installed", "upgrade", "clean", "remove"]},
        {"source": "flatpak", "available": true, "capabilities": ["search", "installed"]},
        {"source": "homebrew-cask", "available": true},
        {"source": "fwupd", "available": true, "capabilities": ["upgrade"]},
        {"source": "macos-apps", "available": true, "capabilities": ["remove"]},
        {"source": "snap", "available": false, "summary": "snapd isn't running"},
    ]))
    .unwrap()
}

#[test]
fn the_catalog_answers_for_missing_sources_too() {
    let entry = catalog_entry(&[], "apt");
    assert_eq!(entry.availability_kind, "checking");
    assert_eq!(entry.summary, "Checking availability…");
    let entry = catalog_entry(&catalog(), "pacman");
    assert_eq!(entry.availability_kind, "platform");
    assert!(!entry.available);
    assert!(catalog_entry(&catalog(), "apt").available);
}

#[test]
fn row_actions_follow_source_page_and_state() {
    let cat = catalog();
    let action = |value: Value, page| row_action(&row(value), page, &cat);
    assert_eq!(action(json!({"kind": "cleanup"}), Page::Clean), Some(Action::Clean));
    assert_eq!(action(json!({"kind": "source"}), Page::Sources), None);
    assert_eq!(action(json!({"installed": "1"}), Page::Updates), Some(Action::Upgrade));
    assert_eq!(action(json!({"installed": "1"}), Page::Installed), Some(Action::Remove));
    assert_eq!(action(json!({}), Page::Search), Some(Action::Install));
    // Firmware never installs: it updates, or has nothing to do.
    assert_eq!(action(json!({"source": "fwupd", "installed": "1", "update": "available"}), Page::Installed), Some(Action::Upgrade));
    assert_eq!(action(json!({"source": "fwupd", "installed": "1"}), Page::Installed), None);
    let mac_adopt = action(json!({"source": "macos-apps", "installed": "1", "adopt_with": "firefox"}), Page::Installed);
    let mac_remove = action(json!({"source": "macos-apps", "installed": "1"}), Page::Installed);
    if cfg!(target_os = "macos") {
        assert_eq!(mac_adopt, Some(Action::Adopt));
    } else {
        assert_eq!(mac_adopt, Some(Action::Remove));
    }
    assert_eq!(mac_remove, Some(Action::Remove));
    assert_eq!(row_action(&row(json!({"source": "macos-apps"})), Page::Installed, &[]), None);
    for action in [Action::Install, Action::Remove, Action::Upgrade, Action::Clean, Action::Adopt] {
        assert!(!action.key().is_empty() && !action.label().is_empty() && !action.icon().is_empty());
        let _ = action.tone();
    }
    assert!(can_adopt(&row(json!({"source": "appimage", "adopt_with": "appimage"})), &cat));
    assert!(!can_adopt(&row(json!({"source": "appimage", "adopt_with": "other"})), &cat));
    assert!(can_adopt(&row(json!({"source": "homebrew", "adopt_with": "x"})), &cat));
    assert!(!can_adopt(&row(json!({"kind": "cleanup", "adopt_with": "x"})), &cat));
    assert!(!can_adopt(&row(json!({})), &cat));
}

#[test]
fn search_ranks_exact_then_prefix_then_substring_then_summary() {
    let rank = |value: Value| relevance(&row(value), " Fire ");
    assert_eq!(rank(json!({"name": "fire", "candidate": "1"})), 0);
    assert_eq!(rank(json!({"name": "org.x.fire", "source": "flatpak", "candidate": "1"})), 0);
    assert_eq!(rank(json!({"name": "firefox", "candidate": "1"})), 1);
    assert_eq!(rank(json!({"name": "campfire", "candidate": "1"})), 2);
    assert_eq!(rank(json!({"name": "x", "summary": "Fire starter", "candidate": "1"})), 3);
    assert_eq!(rank(json!({"name": "x", "summary": "Light a fire", "candidate": "1"})), 4);
    assert_eq!(rank(json!({"name": "x", "candidate": "1"})), 5);
    assert_eq!(rank(json!({"name": "x"})), 11);
}

fn names(rows: &[Row], items: &[Item]) -> Vec<String> {
    items.iter().map(|item| rows[item.raw].name.clone()).collect()
}

#[test]
fn visible_rows_filter_by_page_source_and_text() {
    let rows = vec![
        row(json!({"name": "zsh", "installed": "5", "summary": "Shell"})),
        row(json!({"name": "bash", "installed": "5", "source": "homebrew", "same_app_from": ["apt"]})),
        row(json!({"kind": "failure", "name": "snap", "source": "snap"})),
        row(json!({"kind": "cleanup", "name": "cache", "cleanup_kind": "package_cache"})),
        row(json!({"name": "ghost"})),
        row(json!({"kind": "source", "name": "apt", "source": "apt", "available": true})),
        row(json!({"kind": "source", "name": "snap", "source": "snap", "available": false})),
    ];
    let options = |page| ViewOptions { page: Some(page), ..ViewOptions::default() };
    assert_eq!(names(&rows, &visible(&rows, &options(Page::Clean))), ["cache"]);
    assert_eq!(names(&rows, &visible(&rows, &options(Page::Sources))), ["apt"]);
    let all = ViewOptions { show_unavailable: true, ..options(Page::Sources) };
    assert_eq!(names(&rows, &visible(&rows, &all)), ["apt", "snap"]);
    let search = ViewOptions { query: "s", ..options(Page::Search) };
    assert!(!names(&rows, &visible(&rows, &search)).contains(&"ghost".to_owned()));
    let apt = HashSet::from(["apt".to_owned()]);
    let only_apt = ViewOptions { sources: Some(&apt), ..options(Page::Installed) };
    assert_eq!(names(&rows, &visible(&rows, &only_apt)), ["zsh", "cache", "ghost", "apt"]);
    let filtered = ViewOptions { filter: "SHELL", ..options(Page::Installed) };
    assert_eq!(names(&rows, &visible(&rows, &filtered)), ["zsh"]);
    let duplicates = ViewOptions { duplicates_only: true, ..options(Page::Installed) };
    assert_eq!(names(&rows, &visible(&rows, &duplicates)), ["bash"]);
}

#[test]
fn visible_rows_sort_by_column_relevance_or_name() {
    let rows = vec![
        row(json!({"name": "b", "installed": "2", "summary": "zz", "candidate": "1"})),
        row(json!({"name": "a", "installed": "", "candidate": "3", "summary": "yy"})),
        row(json!({"name": "c", "installed": "1", "summary": "a match", "candidate": "1"})),
    ];
    let sorted = |column, ascending| {
        let options = ViewOptions { page: Some(Page::Updates), sort: Some((column, ascending)), ..ViewOptions::default() };
        names(&rows, &visible(&rows, &options))
    };
    assert_eq!(sorted(Column::Name, true), ["a", "b", "c"]);
    assert_eq!(sorted(Column::Name, false), ["c", "b", "a"]);
    assert_eq!(sorted(Column::Version, true), ["c", "b", "a"]);
    assert_eq!(sorted(Column::Summary, true), ["c", "a", "b"]);
    let options = ViewOptions { page: Some(Page::Search), query: "match", ..ViewOptions::default() };
    assert_eq!(names(&rows, &visible(&rows, &options))[0], "c");
    let sources = vec![
        row(json!({"kind": "source", "name": "z", "source": "zypper", "available": true})),
        row(json!({"kind": "source", "name": "a", "source": "apt", "available": false})),
        row(json!({"kind": "source", "name": "b", "source": "bun", "available": true})),
    ];
    let options = ViewOptions { page: Some(Page::Sources), show_unavailable: true, ..ViewOptions::default() };
    assert_eq!(names(&sources, &visible(&sources, &options)), ["a", "b", "z"]);
    let by_status = ViewOptions { sort: Some((Column::Summary, true)), ..options };
    assert_eq!(names(&sources, &visible(&sources, &by_status)), ["a", "b", "z"]);
    for column in [Column::Name, Column::Version, Column::Summary] {
        assert_eq!(Column::from_key(column.key()), Some(column));
    }
    assert_eq!(Column::from_key("status"), Some(Column::Summary));
    assert_eq!(Column::from_key("capabilities"), None);
}

#[test]
fn flatpaks_in_both_scopes_become_one_row() {
    let system = row(json!({"name": "org.gimp.GIMP", "source": "flatpak", "remote": "flathub", "installed": "2"}));
    let user = row(json!({"name": "org.gimp.GIMP", "source": "flatpak", "remote": "flathub", "scope": {"user": {"uid": 1}}}));
    let alone = row(json!({"name": "org.kde.krita", "source": "flatpak"}));
    let rows = vec![user.clone(), system.clone(), alone];
    let options = ViewOptions { page: Some(Page::Installed), ..ViewOptions::default() };
    let items = visible(&rows, &options);
    assert_eq!(items.len(), 2);
    let merged = items.iter().find(|item| !item.variants.is_empty()).unwrap();
    // System first, and the installed copy is chosen.
    assert_eq!(merged.variants, vec![1, 0]);
    assert_eq!(merged.raw, 1);
    let choices = HashMap::from([(flatpak_group(&user).unwrap(), user.identity())]);
    let chosen = ViewOptions { scope_choices: Some(&choices), ..options };
    let items = visible(&rows, &chosen);
    assert_eq!(items.iter().find(|item| !item.variants.is_empty()).unwrap().raw, 0);
    // Neither installed: System.
    let rows = vec![row(json!({"name": "x", "source": "flatpak", "scope": {"user": {"uid": 1}}})), row(json!({"name": "x", "source": "flatpak"}))];
    let items = visible(&rows, &ViewOptions { page: Some(Page::Search), query: "x", ..ViewOptions::default() });
    assert_eq!(items.len(), 0, "search drops rows with no version");
    let rows = vec![row(json!({"name": "x", "source": "flatpak", "scope": {"user": {"uid": 1}}, "candidate": "1"})), row(json!({"name": "x", "source": "flatpak", "candidate": "1"}))];
    let items = visible(&rows, &ViewOptions { page: Some(Page::Search), query: "x", ..ViewOptions::default() });
    assert_eq!(items[0].raw, 1);
    assert_eq!(flatpak_group(&row(json!({}))), None);
}

#[test]
fn the_same_app_from_several_sources_sits_together() {
    let rows = vec![
        row(json!({"name": "firefox", "display_name": "Firefox", "installed": "1", "same_app_group": "firefox"})),
        row(json!({"name": "htop", "installed": "1"})),
        row(json!({"name": "org.mozilla.firefox", "source": "flatpak", "installed": "1", "same_app_group": "firefox"})),
        row(json!({"name": "lonely", "installed": "1", "same_app_group": "lonely"})),
        row(json!({"name": "vim-gtk", "installed": "1", "same_app_group": "vim"})),
        row(json!({"name": "vim", "source": "homebrew", "installed": "1", "same_app_group": "vim"})),
    ];
    let items = visible(&rows, &ViewOptions { page: Some(Page::Installed), ..ViewOptions::default() });
    assert_eq!(names(&rows, &items), ["firefox", "org.mozilla.firefox", "htop", "lonely", "vim-gtk", "vim"]);
    let heading = items[0].group.as_ref().unwrap();
    assert_eq!(heading.title, "Firefox");
    assert_eq!(heading.sources, ["APT", "Flatpak"]);
    assert!(items[1].group.is_none() && items[3].group.is_none());
    // Without a display name, the shortest name titles the group.
    assert_eq!(items[4].group.as_ref().unwrap().title, "vim");
    // A filter that matches one member keeps the whole group.
    let filtered = visible(&rows, &ViewOptions { page: Some(Page::Installed), filter: "mozilla", ..ViewOptions::default() });
    assert_eq!(names(&rows, &filtered), ["firefox", "org.mozilla.firefox"]);
}

#[test]
fn sources_are_listed_grouped_and_matched_to_files() {
    let ids = all_sources(&serde_json::from_value::<Vec<Source>>(json!([{"source": "newcomer"}, {"source": "apt"}])).unwrap());
    assert_eq!(ids.last().unwrap(), "newcomer");
    assert_eq!(ids.iter().filter(|id| *id == "apt").count(), 1);
    assert_eq!(category("apt"), "System");
    assert_eq!(category("flatpak"), "Applications");
    assert_eq!(category("docker"), "Containers");
    assert_eq!(category("cargo"), "Developer tools");
    assert!(CATEGORIES.contains(&category("anything")));
    let patterns = file_patterns(&catalog());
    assert!(patterns.contains(&"deb") && patterns.contains(&"flatpakref"));
    assert!(!patterns.contains(&"snap"), "snap isn't available");
    let everything: Vec<Source> = ["appimage", "apt", "dnf", "zypper", "pacman", "flatpak", "snap"]
        .iter()
        .map(|id| Source { source: (*id).into(), available: true, ..Source::default() })
        .collect();
    let patterns = file_patterns(&everything);
    for expected in ["AppImage", "rpm", "zst", "snap", "ymp"] {
        assert!(patterns.contains(&expected), "{expected}");
    }
    assert_eq!(file_patterns(&[]).is_empty(), !cfg!(target_os = "linux"));
}

#[test]
fn the_search_hint_counts_what_can_search() {
    let enabled: HashSet<String> = ["apt", "flatpak", "snap"].iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(search_hint(&[], &enabled, ""), "Searches your sources as you type.");
    assert_eq!(search_hint(&[], &enabled, "Saved."), "Saved.");
    assert_eq!(search_hint(&catalog(), &enabled, ""), "Searches 2 sources as you type.");
    let only_apt = HashSet::from(["apt".to_owned()]);
    assert_eq!(search_hint(&catalog(), &only_apt, ""), "Searches APT as you type.");
    assert_eq!(search_hint(&catalog(), &HashSet::new(), ""), "No enabled source can search. Turn one on in Sources.");
}

#[test]
fn failure_titles_name_one_or_count_many() {
    let failure = |source: &str, kind: &str| Failure { source: source.into(), kind: kind.into(), detail: None };
    assert_eq!(failure_title(&[failure("snap", "unavailable")]), "Couldn't check Snap");
    assert_eq!(failure_title(&[failure("snap", "failed"), failure("apt", "partial")]), "Couldn't check 2 sources");
    assert_eq!(failure_title(&[failure("apt", "partial")]), "Some apps couldn't be read");
}

#[test]
fn times_sizes_and_intervals_read_naturally() {
    let now = 1_800_000_000;
    assert_eq!(checked_ago(now - 10, now), "Checked just now");
    assert_eq!(checked_ago(now + 10, now), "Checked just now");
    assert_eq!(checked_ago(now - 60, now), "Checked 1 minute ago");
    assert_eq!(checked_ago(now - 300, now), "Checked 5 minutes ago");
    assert_eq!(checked_ago(now - 3600, now), "Checked 1 hour ago");
    assert_eq!(checked_ago(now - 7300, now), "Checked 2 hours ago");
    assert!(checked_ago(now - 90_000, now).starts_with("Checked "));
    assert!(!short_datetime(now).is_empty());
    assert_eq!(short_datetime(i64::MAX), "");
    assert_eq!(time_or_date(now, now).len(), 5);
    assert_eq!(time_or_date(now - 10 * 86_400, now), short_datetime(now - 10 * 86_400));
    assert!(super::now() > 1_700_000_000);
    assert_eq!(duration(-5), "0 s");
    assert_eq!(duration(42), "42 s");
    assert_eq!(duration(185), "3 min 5 s");
    assert_eq!(duration(3720), "1 h 2 min");
    assert_eq!(size(820), "820 bytes");
    assert_eq!(size(4_200_000), "4.2 MB");
    assert_eq!(size(31_000_000), "31 MB");
    assert_eq!(size(2_000_000_000_000_000), "2000 TB");
    assert_eq!(INTERVALS[nearest_interval(30)].0, 30);
    assert_eq!(INTERVALS[nearest_interval(100)].0, 60);
    assert_eq!(INTERVALS[nearest_interval(99_999)].0, 10080);
    assert_eq!(INTERVALS[nearest_interval(-4)].0, 15);
}
