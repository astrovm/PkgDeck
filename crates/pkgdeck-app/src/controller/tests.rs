use super::*;
use pkgdeck_core::fake::Fake;
#[test]
fn a_running_change_reads_as_what_it_is_doing() {
    for (title, running) in [
        ("Install Demo (Flatpak)", "Installing Demo (Flatpak)…"),
        ("Remove Demo (APT)", "Removing Demo (APT)…"),
        ("Update Beta App", "Updating Beta App…"),
        ("Refresh APT package lists", "Refreshing APT package lists…"),
        ("Clean Cargo downloads", "Cleaning Cargo downloads…"),
        ("Add Example repository", "Adding Example repository…"),
        ("Enable Example repository", "Enabling Example repository…"),
        (
            "Disable Example repository",
            "Disabling Example repository…",
        ),
        ("Change Example priority", "Changing Example priority…"),
        ("Open software sources", "Opening software sources…"),
        ("Update", "Updating…"),
        ("Working", "Working…"),
        ("", ""),
    ] {
        assert_eq!(running_label(title), running, "{title}");
    }
}
/// What `$pattern` binds in `$value`; any other value fails the test.
macro_rules! expect {
    ($value:expr, $pattern:pat => $bound:expr) => {
        match $value {
            $pattern => $bound,
            _ => panic!("expected {}", stringify!($pattern)),
        }
    };
}
/// A finished batch of updates: how each step ended and what it said.
fn ran(steps: &[(&str, Outcome, Said)]) -> Payload {
    Payload::Batch(Batch {
        noun: "updates",
        steps: steps
            .iter()
            .map(|(title, outcome, said)| Ended {
                title: (*title).into(),
                outcome: *outcome,
                said: said.clone(),
                kept: None,
            })
            .collect(),
        note: None,
    })
}
/// A batch where only how each step ended matters.
fn ended(outcomes: &[Outcome]) -> Payload {
    let steps: Vec<_> = outcomes
        .iter()
        .map(|outcome| match outcome {
            Outcome::Finished => ("Change", *outcome, Said::Completed),
            _ => ("Change", *outcome, because("It stopped.")),
        })
        .collect();
    ran(&steps)
}
fn because(reason: &str) -> Said {
    Said::Because(reason.into())
}
#[test]
fn write_progress_tracks_real_steps_and_transfer_totals() {
    let first = Operation::Refresh {
        backend: "apt".into(),
    };
    let second = Operation::Refresh {
        backend: "flatpak".into(),
    };
    let mut state = ProgressState::new(
        &Job::UpgradeAll(vec![first.clone(), second.clone()], None),
        Some(42),
        &Names::new(),
    );
    state.apply(&Event::Started(first.clone()));
    state.apply(&Event::Progress {
        operation: first.clone(),
        progress: Progress::Transfer {
            completed: 50,
            total: Some(100),
        },
    });
    let snapshot: Value = serde_json::from_str(&state.snapshot().to_string()).unwrap();
    assert_eq!(snapshot["activity_id"], 42);
    assert_eq!(snapshot["done"], 0);
    assert_eq!(snapshot["total"], 2);
    assert_eq!(snapshot["transfer_total"], 100);
    state.apply(&Event::Finished {
        operation: first,
        result: Ok(OperationOutcome::default()),
    });
    state.apply(&Event::Started(second));
    let snapshot: Value = serde_json::from_str(&state.snapshot().to_string()).unwrap();
    assert_eq!(snapshot["done"], 1);
    assert_eq!(snapshot["transfer_total"], Value::Null);
    assert_eq!(snapshot["label"], "Refreshing Flatpak package lists…");
    let repository = RepositoryAction {
        backend: "flatpak".into(),
        name: "synthetic".into(),
        scope: Scope::System,
        change: repositories::Change::Remove,
    };
    let state = ProgressState::new(
        &Job::Repositories(Some(repository.clone())),
        None,
        &Names::new(),
    );
    assert_eq!(state.label, "Remove synthetic repository");
    for (change, label) in [
        (
            repositories::Change::Add {
                url: "https://example.invalid/repo".into(),
            },
            "Add synthetic repository",
        ),
        (
            repositories::Change::SetEnabled { enabled: true },
            "Enable synthetic repository",
        ),
        (
            repositories::Change::SetEnabled { enabled: false },
            "Disable synthetic repository",
        ),
        (
            repositories::Change::SetPriority { priority: 5 },
            "Change synthetic priority",
        ),
        (repositories::Change::OpenEditor, "Open software sources"),
    ] {
        let action = RepositoryAction {
            change,
            ..repository.clone()
        };
        let state = ProgressState::new(&Job::Repositories(Some(action)), None, &Names::new());
        assert_eq!(state.label, label);
    }
    let mut state = ProgressState::new(
        &Job::Load("installed".into(), String::new()),
        None,
        &Names::new(),
    );
    assert_eq!(state.label, "Working");
    let before = state.snapshot().to_string();
    state.apply(&Event::Progress {
        operation: Operation::Refresh {
            backend: "apt".into(),
        },
        progress: Progress::Message("Reading lists".into()),
    });
    assert_eq!(state.snapshot().to_string(), before);
}
#[test]
fn file_urls_preserve_spaces_and_unicode_and_reject_remote_hosts() {
    assert_eq!(
        local_input_path("file:///tmp/Sample%20%C3%B1.AppImage").unwrap(),
        PathBuf::from("/tmp/Sample ñ.AppImage")
    );
    assert_eq!(
        local_input_path("file://localhost/tmp/Sample%20App.deb").unwrap(),
        PathBuf::from("/tmp/Sample App.deb")
    );
    assert!(local_input_path("file://remote/tmp/package.deb").is_err());
    assert!(local_input_path("file:///tmp/bad%00.deb").is_err());
    assert!(local_input_path("file:///tmp/bad%2Z.deb").is_err());
    assert!(local_input_path("file:///tmp/bad%FF.deb").is_err());
    assert!(local_input_path("relative.deb").is_err());
    assert!(local_input_path("https://example.org/package.deb").is_err());
    assert!(inspect_open_input("file:///tmp/bad%2Z.deb", &Cancellation::default()).is_err());
}
#[test]
fn direct_https_links_route_to_their_native_previews() {
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::var_os("PKGDECK_OPEN_LINK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("pkgdeck-open-links-{}", std::process::id()))
        });
    if std::env::var_os("PKGDECK_OPEN_LINK_CHILD").is_none() {
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let curl = base.join("curl");
        // A real bundle's header, written with the shell's own printf.
        let bundle: String =
            include_bytes!("../../../pkgdeck-core/src/backends/fixtures/synthetic.flatpak")
                .iter()
                .map(|byte| format!("\\{byte:03o}"))
                .collect();
        let script = format!("#!/bin/sh\nout=''\nfor arg; do\n  if [ \"$previous\" = '--output' ]; then out=\"$arg\"; fi\n  previous=\"$arg\"\ndone\ncase \"$arg\" in\n  *.flatpakref) printf '[Flatpak Ref]\\nName=org.example.Synthetic\\nUrl=https://example.invalid/repo\\n' ;;\n  *.flatpakrepo) printf '[Flatpak Repo]\\nName=synthetic\\nUrl=https://example.invalid/repo\\nGPGKey=c3ludGhldGlj\\n' ;;\n  *.flatpak) printf '{bundle}' > \"$out\" ;;\nesac\n");
        std::fs::write(&curl, script).unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "controller::tests::direct_https_links_route_to_their_native_previews",
                "--nocapture",
            ])
            .env("PKGDECK_OPEN_LINK_CHILD", "1")
            .env("PKGDECK_OPEN_LINK_DIR", &base)
            .env("PATH", &base)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        std::fs::remove_dir_all(base).unwrap();
        return;
    }
    let cancel = Cancellation::default();
    for link in [
        "https://example.invalid/synthetic.flatpakref",
        "flatpak+https://example.invalid/synthetic.flatpakref",
    ] {
        let preview = inspect_open_input(link, &cancel).unwrap();
        let opened = expect!(preview, Payload::OpenPackage(opened) => opened);
        assert_eq!(opened.package.id.name, "org.example.Synthetic");
    }
    let preview =
        inspect_open_input("https://example.invalid/synthetic.flatpakrepo", &cancel).unwrap();
    let import = expect!(preview, Payload::OpenRepository(import) => import);
    assert_eq!(import.name, "synthetic");
    let preview = inspect_open_input("https://example.invalid/synthetic.flatpak", &cancel).unwrap();
    let opened = expect!(preview, Payload::OpenPackage(opened) => opened);
    assert_eq!(opened.package.id.backend, "flatpak");
    // Named by the app inside, not the file.
    assert_eq!(opened.package.id.name, "org.example.Synthetic");
    assert_eq!(opened.package.candidate_version.as_deref(), Some("1.0"));
    assert!(inspect_open_input("https://example.invalid/unsupported.bin", &cancel).is_err());
}
#[test]
fn remote_appimage_confirmation_uses_the_filename() {
    let id = PackageId {
        backend: "appimage".into(),
        name: "https://example.invalid/downloads/Synthetic.AppImage?version=1".into(),
        architecture: "x86_64".into(),
        scope: Scope::Environment {
            path: PathBuf::from("/tmp/synthetic-appimages"),
        },
        remote: None,
        reference: Some("artifact:appimage:synthetic".into()),
    };
    let label = operation_label(&Operation::Install(id));
    assert!(label.contains("Install Synthetic.AppImage"));
    assert!(!label.contains("?version=1"));
}
#[test]
fn opening_repository_file_previews_before_any_write() {
    let base = std::env::temp_dir().join(format!("pkgdeck-open-repository-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let source = base.join("synthetic.sources");
    std::fs::write(&source, "Types: deb\nURIs: https://example.invalid/repo\nSuites: stable\nComponents: main\nSigned-By: /etc/apt/keyrings/synthetic.gpg\n").unwrap();
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .open_input(source.to_str().unwrap().into());
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && controller.confirmation().is_empty() {
        controller.as_mut().poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(controller.confirmation().to_string().contains("APT source"));
    assert!(matches!(
        controller.rust().pending,
        Some(Job::ImportRepository(_))
    ));
    controller.as_mut().confirm(false);
    assert!(source.exists());
    controller
        .as_mut()
        .open_input(source.to_str().unwrap().into());
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && controller.confirmation().is_empty() {
        controller.as_mut().poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(&source, "Types: deb\nURIs: https://example.invalid/changed\nSuites: stable\nComponents: main\nSigned-By: /etc/apt/keyrings/synthetic.gpg\n").unwrap();
    controller.as_mut().confirm(true);
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && *controller.busy() {
        controller.as_mut().poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(controller
        .status()
        .to_string()
        .contains("changed since preview"));
    let one_click = base.join("synthetic.ymp");
    std::fs::write(&one_click, "<metapackage><group/></metapackage>\n").unwrap();
    controller
        .as_mut()
        .open_input(one_click.to_str().unwrap().into());
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && controller.confirmation().is_empty() {
        controller.as_mut().poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(controller
        .confirmation()
        .to_string()
        .contains("Open native installer"));
    controller.as_mut().confirm(false);
    std::fs::remove_dir_all(base).unwrap();
}
#[test]
fn confirmed_flatpak_repository_reaches_the_native_manager() {
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::var_os("PKGDECK_CONTROLLER_REPO_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("pkgdeck-controller-repo-{}", std::process::id()))
        });
    if std::env::var_os("PKGDECK_CONTROLLER_REPO_CHILD").is_none() {
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let flatpak = base.join("flatpak");
        std::fs::write(
            &flatpak,
            format!(
                "#!/bin/sh\ncase \"$*\" in\n *remote-add*) printf 'added' > '{}' ;;\nesac\n",
                base.join("record").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&flatpak, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "controller::tests::confirmed_flatpak_repository_reaches_the_native_manager",
                "--nocapture",
            ])
            .env("PKGDECK_CONTROLLER_REPO_CHILD", "1")
            .env("PKGDECK_CONTROLLER_REPO_DIR", &base)
            .env("PATH", &base)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        std::fs::remove_dir_all(base).unwrap();
        return;
    }
    let source = base.join("synthetic.flatpakrepo");
    std::fs::write(
        &source,
        "[Flatpak Repo]\nName=synthetic\nUrl=https://example.invalid/repo\nGPGKey=c3ludGhldGlj\n",
    )
    .unwrap();
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .open_input(source.to_str().unwrap().into());
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && controller.confirmation().is_empty() {
        controller.as_mut().poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(controller
        .confirmation()
        .to_string()
        .contains("Add synthetic"));
    controller.as_mut().confirm(true);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && !base.join("record").exists() {
        controller.as_mut().poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::read_to_string(base.join("record")).unwrap(),
        "added"
    );
    assert!(source.exists());
}
#[test]
#[cfg(target_os = "linux")]
fn opening_local_deb_reads_metadata_without_installing_it() {
    let base = std::env::temp_dir().join(format!("pkgdeck-open-deb-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let control = base.join("staging/DEBIAN");
    std::fs::create_dir_all(&control).unwrap();
    std::fs::write(
            control.join("control"),
            "Package: pkgdeck-open-synthetic\nVersion: 1.2.3\nArchitecture: all\nMaintainer: PkgDeck tests <nobody@example.invalid>\nDescription: Synthetic local archive\n",
        )
        .unwrap();
    let archive = base.join("Synthetic package.deb");
    let status = std::process::Command::new("dpkg-deb")
        .arg("--build")
        .arg(base.join("staging"))
        .arg(&archive)
        .status()
        .unwrap();
    assert!(status.success());
    let preview = inspect_open_input(archive.to_str().unwrap(), &Cancellation::default());
    let opened = expect!(preview.unwrap(), Payload::OpenPackage(opened) => opened);
    let package = &opened.package;
    assert_eq!(package.id.backend, "apt");
    assert_eq!(package.id.name, "pkgdeck-open-synthetic");
    assert_eq!(package.candidate_version.as_deref(), Some("1.2.3"));
    assert!(package
        .id
        .reference
        .as_deref()
        .unwrap()
        .starts_with("local-deb:"));
    assert!(archive.exists());
    std::fs::remove_dir_all(base).unwrap();
}
#[test]
fn opening_appimage_previews_without_importing_until_confirmation() {
    let base = std::env::temp_dir().join(format!("pkgdeck-open-appimage-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let source = base.join("Sample ñ.AppImage");
    let mut elf = [0_u8; 64];
    elf[..4].copy_from_slice(b"\x7fELF");
    elf[4..7].copy_from_slice(&[2, 1, 1]);
    elf[8..11].copy_from_slice(b"AI\x02");
    // ELF machine: AArch64 (183) or x86-64 (62).
    let machine: u16 = [62, 183][usize::from(std::env::consts::ARCH == "aarch64")];
    elf[18..20].copy_from_slice(&machine.to_le_bytes());
    elf[20..24].copy_from_slice(&1_u32.to_le_bytes());
    elf[52..54].copy_from_slice(&64_u16.to_le_bytes());
    std::fs::write(&source, elf).unwrap();
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .open_input(source.to_str().unwrap().into());
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && controller.opened().is_empty() {
        controller.as_mut().poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    // It opens as a page first; nothing is asked until Install.
    let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
    assert_eq!(page["action"], "Install", "{page}");
    assert_eq!(page["location"], source.to_str().unwrap());
    assert!(controller.confirmation().is_empty());
    controller.as_mut().install_opened();
    let preview = controller.confirmation().to_string();
    let status = controller.status();
    assert!(
        preview.contains("Sample ñ"),
        "preview: {preview}; status: {status}"
    );
    assert!(preview.contains("Location:"), "{preview}");
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert!(
        !data["summary"].as_str().unwrap().contains("Other changes"),
        "{data}"
    );
    assert!(matches!(
        controller.rust().pending,
        Some(Job::Write(Operation::Install(_), _))
    ));
    controller.as_mut().confirm(false);
    assert!(source.exists());
    assert!(controller.confirmation().is_empty());
    std::fs::remove_dir_all(base).unwrap();
}
#[test]
fn opening_flatpak_reference_uses_confirmation_and_rejects_unsupported_input() {
    let base = std::env::temp_dir().join(format!("pkgdeck-open-flatpakref-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let source = base.join("Synthetic ñ.flatpakref");
    std::fs::write(
        &source,
        "[Flatpak Ref]\nName=org.example.Synthetic\nUrl=https://example.invalid/repo\n",
    )
    .unwrap();
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .open_input(base.join("queued-unsupported.txt").to_str().unwrap().into());
    controller
        .as_mut()
        .open_input(format!("file://{}", source.display()).into());
    assert!(controller
        .status()
        .to_string()
        .contains("The file will open when the current operation finishes"));
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && controller.opened().is_empty() {
        controller.as_mut().poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    controller.as_mut().install_opened();
    let preview = controller.confirmation().to_string();
    assert!(preview.contains("org.example.Synthetic"), "{preview}");
    assert!(
        preview.contains("Repository to add if needed:"),
        "{preview}"
    );
    assert!(matches!(
        controller.rust().pending,
        Some(Job::Write(Operation::Install(_), _))
    ));
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert_eq!(data["flatpak_ref_scope"], "user");
    controller.as_mut().set_open_flatpak_scope(true);
    assert!(controller.confirmation().to_string().contains("System"));
    assert!(matches!(
        &controller.rust().pending,
        Some(Job::Write(Operation::Install(id), _)) if id.scope == Scope::System
    ));
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert_eq!(data["flatpak_ref_scope"], "system");
    controller.as_mut().set_open_flatpak_scope(false);
    assert!(controller.confirmation().to_string().contains("User"));
    controller.as_mut().confirm(false);
    assert!(source.exists());

    controller
        .as_mut()
        .open_input(base.join("unsupported.txt").to_str().unwrap().into());
    // The deadline and the answer share one exit, so a slow runner
    // doesn't leave a line unrun.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        controller.as_mut().poll();
        if !controller.busy() || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(controller
        .status()
        .to_string()
        .to_lowercase()
        .contains("unsupported package format"));
    assert!(controller.confirmation().is_empty());
    controller
        .as_mut()
        .open_input("flatpak+http://example.invalid/app.flatpakref".into());
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        controller.as_mut().poll();
        if !controller.busy() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(controller.status().to_string().contains(".flatpakref"));
    assert!(controller.confirmation().is_empty());
    controller.as_mut().open_input(" ".into());
    assert_eq!(
        controller.status().to_string(),
        "Choose an installation file."
    );
    std::fs::remove_dir_all(base).unwrap();
}
#[test]
fn appimage_launch_settings_need_an_installed_appimage() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let mut apt = synthetic_package("tool", "Tool");
    apt.installed_version = Some("1".into());
    let mut gone = synthetic_package("/nonexistent/pkgdeck-test/demo.appimage", "Demo");
    gone.id.backend = "appimage".into();
    gone.id.scope = Scope::User { uid: 1000 };
    gone.installed_version = Some("1".into());
    controller.as_mut().rust_mut().packages = vec![apt, gone];
    // Anything else, or no row at all, has nothing to show or do.
    for index in [-1, 0, 5] {
        assert_eq!(
            controller.as_mut().app_launch_settings(index).to_string(),
            "{}"
        );
        assert_eq!(
            controller.as_mut().launch_app(index).to_string(),
            "Nothing to start."
        );
        assert_eq!(
            controller
                .as_mut()
                .save_app_launch_settings(index, "%U".into(), "[]".into())
                .to_string(),
            "Nothing to change."
        );
    }
    // An AppImage that's gone says so instead.
    let settings: Value =
        serde_json::from_str(&controller.as_mut().app_launch_settings(1).to_string()).unwrap();
    assert!(settings["error"].is_string(), "{settings}");
    assert!(!controller.as_mut().launch_app(1).is_empty());
    assert!(!controller
        .as_mut()
        .save_app_launch_settings(
            1,
            "%U".into(),
            r#"[{"name":" A ","value":"1"},{"name":"","value":"x"}]"#.into()
        )
        .is_empty());
}
#[test]
fn appimage_pages_read_change_and_launch_through_the_menu_entry() {
    let data = std::env::temp_dir().join(format!("pkgdeck-launch-data-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let root = data.join("pkgdeck/appimages");
    let applications = data.join("applications");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&applications).unwrap();
    // A managed AppImage that's a script, so launching it really runs.
    let digest = "ab".repeat(32);
    let name = format!("pkgdeck-{digest}.AppImage");
    let file = root.join(&name);
    let record = data.join("record");
    std::fs::write(
        &file,
        format!(
            "#!/bin/sh\nprintf '%s|%s\\n' \"$GREETING\" \"$*\" > '{}'\n",
            record.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    let entry = applications.join(format!("pkgdeck-{digest}.desktop"));
    std::fs::write(
        &entry,
        format!(
            "[Desktop Entry]\nName=Demo\nExec=\"{}\" --flag %U\n",
            file.display()
        ),
    )
    .unwrap();
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().appimage_data = Some(data.clone());
    let mut row = synthetic_package(&name, "Demo");
    row.id.backend = "appimage".into();
    row.id.scope = Scope::User {
        uid: rustix::process::getuid().as_raw(),
    };
    row.installed_version = Some("2.0".into());
    controller.as_mut().rust_mut().packages = vec![row];
    let settings: Value =
        serde_json::from_str(&controller.as_mut().app_launch_settings(0).to_string()).unwrap();
    assert_eq!(
        settings,
        json!({"arguments": "--flag %U", "environment": [], "editable": true})
    );
    // Blank variable names are dropped; the rest are saved trimmed.
    assert_eq!(
        controller
            .as_mut()
            .save_app_launch_settings(
                0,
                "--other %U".into(),
                r#"[{"name":" GREETING ","value":"hi"},{"name":"","value":"x"}]"#.into()
            )
            .to_string(),
        ""
    );
    let settings: Value =
        serde_json::from_str(&controller.as_mut().app_launch_settings(0).to_string()).unwrap();
    assert_eq!(settings["arguments"], "--other %U");
    assert_eq!(
        settings["environment"],
        json!([{"name": "GREETING", "value": "hi"}])
    );
    assert_eq!(controller.as_mut().launch_app(0).to_string(), "");
    let text = (0..200)
        .find_map(|_| {
            std::thread::sleep(Duration::from_millis(10));
            std::fs::read_to_string(&record)
                .ok()
                .filter(|text| text.ends_with('\n'))
        })
        .expect("the AppImage never started");
    assert_eq!(text, "hi|--other\n");
    // Its page shows the file it starts from.
    let shown: Value = serde_json::from_str(&controller.as_mut().app_file(0).to_string()).unwrap();
    assert_eq!(shown["path"], file.display().to_string());
    assert_eq!(shown["folder"], root.display().to_string());
    assert_eq!(shown["bytes"], std::fs::metadata(&file).unwrap().len());
    assert_eq!(shown["managed"], true);
    assert_eq!(shown["without_fuse"], false);
    assert_eq!(controller.as_mut().app_file(5).to_string(), "{}");
    // The page of a file PkgDeck just installed starts that install.
    assert_eq!(
        controller.as_mut().launch_opened().to_string(),
        "Nothing to start."
    );
    let mut opened = controller.rust().packages[0].clone();
    opened.id.name = data.join("Demo.AppImage").display().to_string();
    opened.id.reference = Some(digest.clone());
    controller.as_mut().rust_mut().opened_package = Some(opened);
    std::fs::remove_file(&record).unwrap();
    assert_eq!(controller.as_mut().launch_opened().to_string(), "");
    let text = (0..200)
        .find_map(|_| {
            std::thread::sleep(Duration::from_millis(10));
            std::fs::read_to_string(&record)
                .ok()
                .filter(|text| text.ends_with('\n'))
        })
        .expect("the AppImage never started");
    assert_eq!(text, "hi|--other\n");
    controller.as_mut().rust_mut().opened_package = None;
    // Where it updates from, saved for the next check.
    let source: Value =
        serde_json::from_str(&controller.as_mut().app_update_source(0).to_string()).unwrap();
    assert_eq!(
        source,
        json!({"github": null, "builtin": false, "editable": true})
    );
    assert!(!controller
        .as_mut()
        .save_app_update_source(0, "not a project".into())
        .is_empty());
    assert_eq!(
        controller
            .as_mut()
            .save_app_update_source(0, "example/demo".into())
            .to_string(),
        ""
    );
    let source: Value =
        serde_json::from_str(&controller.as_mut().app_update_source(0).to_string()).unwrap();
    assert_eq!(source["github"], "example/demo");
    assert_eq!(controller.as_mut().app_update_source(5).to_string(), "{}");
    assert_eq!(
        controller
            .as_mut()
            .save_app_update_source(5, "x/y".into())
            .to_string(),
        "Nothing to change."
    );
    // An AppImage that's gone says why.
    let mut gone = controller.rust().packages[0].clone();
    gone.id.name = "/nonexistent/pkgdeck-test/other.appimage".into();
    controller.as_mut().rust_mut().packages.push(gone);
    let gone: Value =
        serde_json::from_str(&controller.as_mut().app_update_source(1).to_string()).unwrap();
    assert!(gone["error"].is_string(), "{gone}");
    assert_eq!(controller.as_mut().app_file(1).to_string(), "{}");
    // Opened from a file that isn't an AppImage: nothing to start.
    let mut opened = controller.rust().packages[0].clone();
    opened.id.backend = "apt".into();
    opened.id.reference = Some(digest);
    controller.as_mut().rust_mut().opened_package = Some(opened);
    assert_eq!(
        controller.as_mut().launch_opened().to_string(),
        "Nothing to start."
    );
    // One the install didn't keep can't start.
    let mut opened = controller.rust().packages[0].clone();
    opened.id.reference = Some("cd".repeat(32));
    controller.as_mut().rust_mut().opened_package = Some(opened);
    assert!(!controller.as_mut().launch_opened().is_empty());
    std::fs::remove_dir_all(data).unwrap();
}
#[test]
fn previews_say_when_a_change_needs_a_second_look() {
    let package = synthetic_package("synthetic", "Synthetic");
    let install = Operation::Install(package.id.clone());
    let preview = |operation: &Operation, plan: Option<&TransactionPlan>| {
        confirmation_preview(operation, std::slice::from_ref(&package), &[], plan)
    };
    // Installing or updating just that app runs without asking.
    assert_eq!(preview(&install, None)["review"], false);
    assert_eq!(
        preview(&Operation::Upgrade(package.id.clone()), None)["review"],
        false
    );
    // Removing always asks, and says what may stay behind.
    let removal = preview(&Operation::Remove(package.id.clone()), None);
    assert_eq!(removal["review"], true);
    assert_eq!(
        removal["notes"],
        json!([
            "Other changes may be required.",
            "App data may remain after removal."
        ])
    );
    // A plan that changes other packages lists them.
    let change = |name: &str| PlannedChange {
        action: PlannedAction::Install,
        name: name.into(),
        installed_version: None,
        candidate_version: Some("1".into()),
    };
    let mut plan = TransactionPlan {
        operation: install.clone(),
        native_preview: String::new(),
        changes: vec![change("synthetic")],
        download_bytes: None,
        disk_bytes: None,
        restart_required: None,
        adopts: None,
    };
    let alone = preview(&install, Some(&plan));
    assert_eq!(alone["review"], false);
    assert_eq!(alone["changes"], json!([]));
    plan.changes.push(change("synthetic-library"));
    let more = preview(&install, Some(&plan));
    assert_eq!(more["review"], true);
    assert_eq!(more["notes"], json!(["1 other package will change"]));
    assert_eq!(more["changes"], json!(["Install synthetic-library (1)"]));
    // Moving an app already in place is reviewed too.
    plan.changes.truncate(1);
    plan.adopts = Some("/Applications/Synthetic.app".into());
    assert_eq!(preview(&install, Some(&plan))["review"], true);
    // So is anything that isn't one app.
    assert_eq!(
        confirmation_preview(
            &Operation::UpgradeAll {
                backend: "apt".into()
            },
            &[],
            &[],
            None
        )["review"],
        true
    );
}
#[test]
fn an_opened_file_already_installed_offers_update_or_reinstall() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    for (backend, installed, candidate, action) in [
        ("flatpak", None, "0.4.1", "Install"),
        ("flatpak", Some("0.4.1"), "0.4.1", "Reinstall"),
        ("flatpak", Some("0.3.2"), "0.4.1", "Update"),
        ("apt", Some("1.0"), "1.1", "Update"),
        ("apt", Some("1.1"), "1.1", ""),
    ] {
        let mut package = synthetic_package("org.example.Synthetic", "Synthetic");
        package.id.backend = backend.into();
        package.installed_version = installed.map(str::to_owned);
        package.candidate_version = Some(candidate.into());
        controller.as_mut().apply(Ok(opened_payload(package)));
        let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
        assert_eq!(page["action"], action, "{backend} {installed:?}");
        assert_eq!(page["package"]["installed"], json!(installed), "{backend}");
    }
}
#[test]
fn flatpak_links_start_where_the_last_one_was_installed() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let user = Scope::User {
        uid: rustix::process::getuid().as_raw(),
    };
    let mut link = synthetic_package("org.example.Demo", "Demo");
    link.id.backend = "flatpak".into();
    link.id.scope = user.clone();
    link.id.reference = Some(format!(
        "flatpakref:{}:https://example.invalid/Demo.flatpakref",
        "ab".repeat(32)
    ));
    link.installed_version = None;
    let scope_of = |controller: &mut Pin<&mut ffi::PackageController>, package: &Package| {
        controller
            .as_mut()
            .apply(Ok(opened_payload(package.clone())));
        let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
        assert_eq!(
            controller.rust().opened_package.as_ref().unwrap().id.scope,
            serde_json::from_value(page["package"]["scope"].clone()).unwrap()
        );
        page["package"]["scope_label"].as_str().unwrap().to_owned()
    };
    assert!(!controller.flatpak_links_for_system());
    assert!(scope_of(&mut controller, &link).starts_with("User"));
    controller.as_mut().set_flatpak_links_for_system(true);
    assert_eq!(scope_of(&mut controller, &link), "System");
    // The review starts there too.
    controller.as_mut().install_opened();
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert_eq!(data["flatpak_ref_scope"], "system");
    controller.as_mut().confirm(false);
    // An app that's already installed stays where it is.
    let mut installed = link.clone();
    installed.installed_version = Some("1.0".into());
    assert!(scope_of(&mut controller, &installed).starts_with("User"));
    // Other files keep their own scope.
    let mut bundle = link.clone();
    bundle.id.reference = Some(format!(
        "artifact:flatpak:{}::/tmp/Demo.flatpak",
        "ab".repeat(32)
    ));
    assert!(scope_of(&mut controller, &bundle).starts_with("User"));
    controller.as_mut().set_flatpak_links_for_system(false);
    assert!(scope_of(&mut controller, &link).starts_with("User"));
}
#[test]
fn an_opened_flatpak_installed_for_the_system_shows_installed_and_launch() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let source = "https://example.invalid/Demo.flatpakref";
    let reference = |kind: &str| {
        let mut package = synthetic_package("org.example.Demo", "Demo");
        package.id.backend = "flatpak".into();
        package.id.scope = Scope::User {
            uid: rustix::process::getuid().as_raw(),
        };
        package.id.reference = Some(format!("flatpakref:{}:{source}", "ab".repeat(32)));
        package.installed_version = None;
        // A reference names no version.
        package.candidate_version = None;
        package.summary = format!("Reference: {source}\n{kind}: org.example.Demo/x86_64/stable");
        package
    };
    let package = reference("Application");
    controller
        .as_mut()
        .apply(Ok(opened_payload(package.clone())));
    let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
    assert_eq!(page["launchable"], false);
    assert_eq!(
        controller.as_mut().launch_opened().to_string(),
        "Nothing to start."
    );
    // The review installs it for the whole system instead.
    controller.as_mut().install_opened();
    controller.as_mut().set_open_flatpak_scope(true);
    let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
    assert_eq!(page["package"]["scope_label"], "System");
    assert_eq!(page["action"], "Install");
    let installed = expect!(
        controller.rust().pending.clone(),
        Some(Job::Write(Operation::Install(installed), _)) => installed
    );
    assert_eq!(installed.scope, Scope::System);
    controller.as_mut().mark_opened_installed(
        &Job::Write(Operation::Install(installed.clone()), None),
        &Ok(Payload::Written(OperationOutcome::default())),
    );
    let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
    assert_eq!(page["action"], "", "{page}");
    assert_eq!(page["package"]["installed"], "");
    assert_eq!(page["launchable"], true);
    assert_eq!(controller.opened_flatpak(), Some(installed.clone()));
    // Launch starts it from where it went; one PkgDeck can't start says why.
    let mut elsewhere = installed;
    elsewhere.scope = Scope::Environment {
        path: "/opt/environment".into(),
    };
    controller
        .as_mut()
        .rust_mut()
        .opened_package
        .as_mut()
        .unwrap()
        .id = elsewhere;
    assert!(controller
        .as_mut()
        .launch_opened()
        .to_string()
        .contains("only system and user apps can start"));
    // A runtime is installed too, but nothing starts it.
    let runtime = reference("Runtime");
    controller
        .as_mut()
        .apply(Ok(opened_payload(runtime.clone())));
    controller.as_mut().mark_opened_installed(
        &Job::Write(Operation::Install(runtime.id.clone()), None),
        &Ok(Payload::Written(OperationOutcome::default())),
    );
    let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
    assert_eq!(page["action"], "");
    assert_eq!(page["launchable"], false);
    assert_eq!(controller.opened_flatpak(), None);
    // An app opened when it's already installed can start right away.
    let mut installed = package;
    installed.installed_version = Some("1.0".into());
    controller.as_mut().apply(Ok(opened_payload(installed)));
    let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
    assert_eq!(page["action"], "Reinstall");
    assert_eq!(page["launchable"], true);
}
#[test]
fn opened_appimages_start_from_their_file_until_installed() {
    let dir = std::env::temp_dir().join(format!("pkgdeck-run-file-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A header alone looks like an AppImage, but nothing can run it.
    let mut header = vec![0_u8; 64];
    header[..4].copy_from_slice(b"\x7fELF");
    header[4..7].copy_from_slice(&[2, 1, 1]);
    header[8..11].copy_from_slice(b"AI\x02");
    header[18..20].copy_from_slice(&62_u16.to_le_bytes());
    header[20..24].copy_from_slice(&1_u32.to_le_bytes());
    header[52..54].copy_from_slice(&64_u16.to_le_bytes());
    let file = dir.join("Demo.AppImage");
    std::fs::write(&file, &header).unwrap();
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().appimage_data = Some(dir.join("data"));
    let mut package = synthetic_package(&file.to_string_lossy(), "Demo");
    package.id.backend = "appimage".into();
    package.id.reference = Some("ab".repeat(32));
    package.installed_version = None;
    controller
        .as_mut()
        .apply(Ok(opened_payload(package.clone())));
    let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
    assert_eq!(page["runnable"], true);
    assert_eq!(page["launchable"], false);
    let error = controller.as_mut().launch_opened().to_string();
    assert!(error.contains("couldn't start it"), "{error}");
    // Once installed, the installed copy starts instead.
    controller.as_mut().mark_opened_installed(
        &Job::Write(Operation::Install(package.id.clone()), None),
        &Ok(Payload::Written(OperationOutcome::default())),
    );
    let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
    assert_eq!(page["runnable"], false);
    let error = controller.as_mut().launch_opened().to_string();
    assert!(!error.contains("couldn't start it"), "{error}");
    // Neither an installed copy nor a name that isn't a path runs as a file.
    let mut installed = package.clone();
    installed.installed_version = Some("1.0".into());
    let mut named = package;
    named.id.name = "Demo.AppImage".into();
    for other in [installed, named] {
        controller.as_mut().apply(Ok(opened_payload(other)));
        let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
        assert_eq!(page["runnable"], false);
    }
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn opened_files_show_a_page_until_back_and_installed_after_their_install() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    // Nothing open: Install does nothing.
    controller.as_mut().install_opened();
    assert!(controller.confirmation().is_empty());
    let mut package = synthetic_package("demo", "Demo");
    package.id.backend = "fixture".into();
    package.installed_version = None;
    package.candidate_version = Some("2.1.7".into());
    package.summary = "Flash images safely\nLocal archive: /home/user/Downloads/demo.deb".into();
    package.id.reference = Some(format!(
        "local-deb:{}:/home/user/Downloads/demo.deb",
        "ab".repeat(32)
    ));
    let details = PackageDetails {
        description: "A longer description.".into(),
        homepage: Some("https://example.invalid/demo".into()),
        dependencies: vec!["libc6".into()],
        package: package.clone(),
    };
    controller
        .as_mut()
        .apply(Ok(Payload::OpenPackage(Box::new(details))));
    let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
    assert_eq!(page["action"], "Install");
    assert_eq!(page["location"], "/home/user/Downloads/demo.deb");
    assert_eq!(page["package"]["summary"], "Flash images safely");
    assert_eq!(page["description"], "A longer description.");
    assert_eq!(page["homepage"], "https://example.invalid/demo");
    assert_eq!(page["dependencies"], json!(["libc6"]));
    // Nothing is asked until Install.
    assert!(controller.confirmation().is_empty());
    controller.as_mut().install_opened();
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert_eq!(data["review"], false);
    controller.as_mut().confirm(false);
    // Another app's install leaves the page as it is; its own marks it installed.
    let job = |id: &PackageId| Job::Write(Operation::Install(id.clone()), None);
    let other = synthetic_package("other", "Other").id;
    controller.as_mut().mark_opened_installed(
        &job(&other),
        &Ok(Payload::Written(OperationOutcome::default())),
    );
    controller
        .as_mut()
        .mark_opened_installed(&job(&package.id), &Err(EngineError::NotFound));
    let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
    assert_eq!(page["action"], "Install");
    controller.as_mut().mark_opened_installed(
        &job(&package.id),
        &Ok(Payload::Written(OperationOutcome::default())),
    );
    let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
    assert_eq!(page["action"], "");
    assert_eq!(page["package"]["installed"], "2.1.7");
    // Back closes it.
    controller.as_mut().close_opened();
    assert!(controller.opened().is_empty());
    controller.as_mut().install_opened();
    assert!(controller.confirmation().is_empty());
    controller.as_mut().mark_opened_installed(
        &job(&package.id),
        &Ok(Payload::Written(OperationOutcome::default())),
    );
    assert!(controller.opened().is_empty());
    // References show the link they came from, not the app's ID, and
    // their comment, if any, as the summary.
    let source = "https://example.invalid/demo.flatpakref";
    let mut link = package.clone();
    link.id.name = "org.example.Demo".into();
    link.id.reference = Some(format!("flatpakref:{}:{source}", "ab".repeat(32)));
    let page = |link: &Package| {
        opened_page(
            &PackageDetails {
                description: String::new(),
                homepage: None,
                dependencies: vec![],
                package: link.clone(),
            },
            "Install",
        )
    };
    link.summary = format!("Does demo things\nReference: {source}\nSigning key: included");
    let opened = page(&link);
    assert_eq!(opened["location"], source);
    assert_eq!(opened["package"]["summary"], "Does demo things");
    link.summary = format!("Reference: {source}\nSigning key: included");
    assert_eq!(page(&link)["package"]["summary"], "");
}
#[test]
fn appimage_install_previews_show_icon_and_description() {
    let mut package = synthetic_package("/home/user/Downloads/demo.appimage", "Demo App");
    package.id.backend = "appimage".into();
    package.id.scope = Scope::Environment {
        path: "/home/user/.local/share/pkgdeck/appimages".into(),
    };
    package.summary = "Does demo things".into();
    package.installed_version = None;
    package.candidate_version = Some("2.0".into());
    package.icon = Some("/icons/demo.png".into());
    let data = confirmation_preview(
        &Operation::Install(package.id.clone()),
        std::slice::from_ref(&package),
        &[],
        None,
    );
    assert_eq!(data["icon"], "/icons/demo.png");
    let summary = data["summary"].as_str().unwrap();
    assert!(summary.starts_with("Install Demo App\nAppImage"));
    assert!(summary.contains("\n2.0\nDoes demo things"));
    // AppImages carry everything they need.
    assert!(!summary.contains("Other changes may be required"));
}

#[test]
fn confirmation_preview_names_target_and_extra_native_changes() {
    let package: Package = serde_json::from_value(json!({
            "id": {"backend":"apt", "name":"anonymous", "architecture":"amd64", "scope":"system"},
            "display_name":"Anonymous App", "summary":"Synthetic", "installed_version":"1", "candidate_version":"2", "update":"available"
        })).unwrap();
    let operation = Operation::Upgrade(package.id.clone());
    let mut plan = TransactionPlan {
        operation: operation.clone(),
        native_preview: "synthetic".into(),
        changes: vec![
            PlannedChange {
                action: PlannedAction::Upgrade,
                name: "anonymous".into(),
                installed_version: Some("1".into()),
                candidate_version: Some("2".into()),
            },
            PlannedChange {
                action: PlannedAction::Remove,
                name: "old-library".into(),
                installed_version: Some("1".into()),
                candidate_version: None,
            },
        ],
        download_bytes: None,
        disk_bytes: None,
        restart_required: None,
        adopts: None,
    };
    let preview =
        confirmation_preview(&operation, std::slice::from_ref(&package), &[], Some(&plan));
    assert_eq!(preview["action"], "Update Anonymous App");
    assert!(preview["summary"]
        .as_str()
        .unwrap()
        .starts_with("Update Anonymous App\nAPT, System\n1 → 2"));
    assert!(!preview["summary"]
        .as_str()
        .unwrap()
        .contains("Architecture:"));
    assert!(preview["summary"]
        .as_str()
        .unwrap()
        .contains("1 other package will change"));
    assert!(preview["summary"]
        .as_str()
        .unwrap()
        .contains("Removes: old-library"));
    assert!(preview["details"]
        .as_str()
        .unwrap()
        .contains("Remove old-library (1)"));
    let body = preview["body"].as_str().unwrap();
    assert!(body.contains("Source: APT"));
    assert!(body.contains("Scope: System"));
    assert!(body.contains("Remove old-library (1)"));
    assert!(!body.contains("Install anonymous"));
    assert!(!body.contains("unknown"));
    plan.changes.push(PlannedChange {
        action: PlannedAction::Remove,
        name: "anonymous".into(),
        installed_version: Some("1".into()),
        candidate_version: None,
    });
    plan.download_bytes = Some(2048);
    plan.disk_bytes = Some(-512);
    plan.restart_required = Some(true);
    let preview =
        confirmation_preview(&operation, std::slice::from_ref(&package), &[], Some(&plan));
    let body = preview["body"].as_str().unwrap();
    assert!(body.contains("Selected: Update anonymous (1 → 2)"));
    assert!(body.contains("Remove anonymous (1)"));
    assert!(body.contains("Download: 2.0 KB, Disk: -512 bytes"));
    assert!(preview["summary"]
        .as_str()
        .unwrap()
        .contains("Restart required"));
    let mut environment_package = package.clone();
    environment_package.id.scope = Scope::Environment {
        path: std::path::PathBuf::from("/synthetic/env"),
    };
    let environment_operation = Operation::Upgrade(environment_package.id.clone());
    let environment =
        confirmation_preview(&environment_operation, &[environment_package], &[], None);
    assert!(environment["details"]
        .as_str()
        .unwrap()
        .contains("Location: /synthetic/env"));
    assert!(!environment["summary"]
        .as_str()
        .unwrap()
        .contains("/synthetic/env"));
    let unavailable = confirmation_preview(&operation, &[package], &[], None);
    assert!(!unavailable["body"].as_str().unwrap().contains("unknown"));
    assert!(unavailable["summary"]
        .as_str()
        .unwrap()
        .contains("Other changes may be required"));
    let long = serde_json::from_value::<Package>(json!({
            "id": {"backend":"apt", "name":"anonymous", "architecture":"amd64", "scope":"system"},
            "display_name":"An extremely long synthetic application name for narrow windows", "summary":"Synthetic", "installed_version":"1", "candidate_version":"2", "update":"available"
        })).unwrap();
    let preview = confirmation_preview(&operation, std::slice::from_ref(&long), &[], None);
    assert!(preview["action"].as_str().unwrap().chars().count() <= 36);
    assert!(preview["summary"]
        .as_str()
        .unwrap()
        .contains(&long.display_name));
}
#[test]
fn retry_merges_only_target_and_preserves_other_check_times() {
    let package = |backend: &str| -> Package {
        serde_json::from_value(json!({
                "id": {"backend":backend, "name":"anonymous", "architecture":"all", "scope":"system"},
                "display_name":"Anonymous", "summary":"Synthetic", "installed_version":"1", "candidate_version":"1", "update":"current"
            })).unwrap()
    };
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().packages = vec![package("apt")];
    controller
        .as_mut()
        .rust_mut()
        .last_success
        .insert("apt".into(), 123);
    controller.as_mut().set_report_state(encoded(json!({"phase":"partial", "successful_sources":["apt"], "failures":[{"source":"npm", "kind":"failed"}], "last_success":{"apt":123}})));
    controller.as_mut().apply(Ok(Payload::RetryPackages(
        "npm".into(),
        PackageReport {
            packages: vec![package("npm")],
            failures: vec![],
            successful_sources: vec!["npm".into()],
        },
    )));
    assert_eq!(controller.rust().packages.len(), 2);
    assert_eq!(controller.rust().last_success["apt"], 123);
    assert!(controller.rust().last_success["npm"] > 123);
    assert_eq!(
        serde_json::from_str::<Value>(&controller.report_state().to_string()).unwrap()["phase"],
        "complete"
    );
}
#[test]
fn partial_inventory_from_one_source_keeps_rows_and_reports_partial_state() {
    let package: Package = serde_json::from_value(json!({
            "id": {"backend":"macos-apps", "name":"/Applications/Visible.app", "architecture":"unknown", "scope":"system"},
            "display_name":"Visible", "summary":"Application", "installed_version":"1", "update":"unknown"
        })).unwrap();
    let failure = BackendFailure {
        backend: "macos-apps".into(),
        error: EngineError::InvalidResponse {
            backend: "macos-apps".into(),
            reason: "skipped folder /Applications/Restricted: permission denied".into(),
        },
    };
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .apply(Ok(Payload::Packages(PackageReport {
            packages: vec![package.clone()],
            failures: vec![failure],
            successful_sources: vec![],
        })));
    assert_eq!(controller.rust().packages, vec![package]);
    let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
    assert_eq!(state["phase"], "partial");
    assert!(state["failures"][0]["detail"]
        .as_str()
        .unwrap()
        .contains("/Applications/Restricted"));
    assert!(!controller.rust().last_success.contains_key("macos-apps"));
}
#[test]
fn failed_update_check_keeps_known_updates_and_retry_merges_new_ones() {
    let package = |backend: &str| -> Package {
        serde_json::from_value(json!({
                "id": {"backend":backend, "name":"synthetic-tool", "architecture":"all", "scope":"system"},
                "display_name":"Synthetic tool", "summary":"Synthetic", "installed_version":"1", "candidate_version":"2", "update":"available"
            }))
            .unwrap()
    };
    let failure = BackendFailure {
        backend: "codex".into(),
        error: EngineError::InvalidResponse {
            backend: "codex".into(),
            reason: "synthetic timeout".into(),
        },
    };
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().updates_view = true;
    controller
        .as_mut()
        .apply(Ok(Payload::Packages(PackageReport {
            packages: vec![package("apt")],
            failures: vec![failure.clone()],
            successful_sources: vec!["apt".into()],
        })));
    assert!(*controller.upgradable());
    assert_eq!(upgrade_plan(&controller.rust().packages).len(), 1);
    let apt_checked = controller.rust().last_success["apt"];

    controller.as_mut().apply(Ok(Payload::RetryFailedUpdates(
        vec!["codex".into()],
        PackageReport {
            packages: vec![],
            failures: vec![failure],
            successful_sources: vec![],
        },
    )));
    assert!(*controller.upgradable());
    assert_eq!(controller.rust().packages, vec![package("apt")]);
    assert_eq!(controller.rust().last_success["apt"], apt_checked);
    controller.as_mut().apply(Ok(Payload::UpgradePreview(
        vec![Operation::UpgradeAll {
            backend: "apt".into(),
        }],
        1,
        None,
    )));
    assert!(controller
        .confirmation()
        .to_string()
        .contains("Updates from it are not included"));
    controller.as_mut().confirm(false);

    controller.as_mut().apply(Ok(Payload::RetryFailedUpdates(
        vec!["codex".into()],
        PackageReport {
            packages: vec![package("codex")],
            failures: vec![],
            successful_sources: vec!["codex".into()],
        },
    )));
    assert_eq!(controller.rust().packages.len(), 2);
    assert_eq!(upgrade_plan(&controller.rust().packages).len(), 2);
    assert!(controller.rust().failures.is_empty());
    assert_eq!(controller.rust().last_success["apt"], apt_checked);
    assert!(controller.rust().last_success.contains_key("codex"));
}
#[test]
fn retry_jobs_return_only_the_requested_view_and_preview() {
    let package: Package = serde_json::from_value(json!({
        "id": {"backend":"fixture", "name":"anonymous", "architecture":"all", "scope":"system"},
        "display_name":"Anonymous", "summary":"Synthetic", "installed_version":"1",
        "candidate_version":"2", "update":"available"
    }))
    .unwrap();
    let mut engine = Engine::default();
    engine
        .register(Fixture {
            package: package.clone(),
            fail: false,
        })
        .unwrap();
    let cancel = Cancellation::default();
    for (view, query, count) in [
        ("Search", "anonymous", 1),
        ("Installed", "missing", 0),
        ("Updates", "missing", 1),
    ] {
        let mut replies = Vec::new();
        execute(
            &mut engine,
            Job::RetrySource(view.into(), query.into(), "fixture".into()),
            &cancel,
            &mut |reply| replies.push(reply),
        );
        assert_eq!(replies.len(), 1);
        let (source, report) = expect!(
            replies.pop().unwrap(),
            Reply::Done(Ok(Payload::RetryPackages(source, report))) => (source, report)
        );
        assert_eq!(source, "fixture");
        assert_eq!(report.packages.len(), count);
        assert_eq!(report.successful_sources, ["fixture"]);
    }
    let mut replies = Vec::new();
    execute(
        &mut engine,
        Job::RetryFailedUpdates(vec!["fixture".into()]),
        &cancel,
        &mut |reply| replies.push(reply),
    );
    assert!(
        matches!(replies.pop(), Some(Reply::Done(Ok(Payload::RetryFailedUpdates(sources, report))))
            if sources == ["fixture"] && report.packages == [package.clone()])
    );
    let mut replies = Vec::new();
    execute(
        &mut engine,
        Job::RetrySource("Clean".into(), "".into(), "fixture".into()),
        &cancel,
        &mut |reply| replies.push(reply),
    );
    assert!(
        matches!(replies.pop(), Some(Reply::Done(Ok(Payload::RetryCleanup(source, _)))) if source == "fixture")
    );
    execute(
        &mut engine,
        Job::RetrySource("Sources".into(), "".into(), "fixture".into()),
        &cancel,
        &mut |reply| replies.push(reply),
    );
    assert!(
        matches!(replies.pop(), Some(Reply::Done(Ok(Payload::RetrySources(source, sources)))) if source == "fixture" && sources.len() == 1)
    );
    execute(
        &mut engine,
        Job::PlanOperation(Operation::Install(package.id)),
        &cancel,
        &mut |reply| replies.push(reply),
    );
    assert!(matches!(
        replies.pop(),
        Some(Reply::Done(Ok(Payload::OperationPreview(
            Operation::Install(_),
            None
        ))))
    ));
}
#[test]
fn retry_cleanup_and_sources_replace_only_failed_source() {
    let item = |source: &str, name: &str| CleanupItem {
        id: CleanupId {
            backend: source.into(),
            key: name.into(),
        },
        kind: CleanupKind::OrphanDependencies,
        title: name.into(),
        summary: "Synthetic".into(),
        preview: "No changes".into(),
        bytes: None,
    };
    let source = |name: &str, available| Source {
        backend: name.into(),
        capabilities: vec![],
        availability: if available {
            Ok(Availability::Available)
        } else {
            Err(EngineError::Unavailable {
                backend: name.into(),
                reason: "Synthetic".into(),
            })
        },
    };
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .apply(Ok(Payload::Cleanup(CleanupReport {
            items: vec![item("apt", "keep"), item("npm", "old")],
            failures: vec![],
        })));
    controller.as_mut().apply(Ok(Payload::RetryCleanup(
        "npm".into(),
        CleanupReport {
            items: vec![item("npm", "new")],
            failures: vec![],
        },
    )));
    assert_eq!(
        controller
            .rust()
            .cleanup
            .iter()
            .map(|item| item.title.as_str())
            .collect::<Vec<_>>(),
        ["keep", "new"]
    );
    controller.as_mut().apply(Ok(Payload::Sources(vec![
        source("apt", true),
        source("npm", false),
    ])));
    controller.as_mut().apply(Ok(Payload::RetrySources(
        "npm".into(),
        vec![source("npm", true)],
    )));
    assert_eq!(controller.rust().sources.len(), 2);
    assert!(controller
        .rust()
        .sources
        .iter()
        .all(|source| source.availability.is_ok()));
}
#[test]
fn changed_plans_request_a_fresh_confirmation_for_the_same_targets() {
    let package: Package = serde_json::from_value(json!({
        "id": {"backend":"apt", "name":"anonymous", "architecture":"amd64", "scope":"system"},
        "display_name":"Anonymous", "summary":"Synthetic", "installed_version":"1",
        "candidate_version":"2", "update":"available"
    }))
    .unwrap();
    let operation = Operation::Upgrade(package.id.clone());
    let plan = TransactionPlan {
        operation: operation.clone(),
        native_preview: "Synthetic".into(),
        changes: vec![],
        download_bytes: None,
        disk_bytes: None,
        restart_required: None,
        adopts: None,
    };
    let drift = Err(EngineError::Refused {
        backend: "apt".into(),
        refusal: Refusal::PlanChanged,
    });
    let single = Job::Write(operation.clone(), Some(Box::new(plan)));
    assert!(
        matches!(repreview_changed_plan(&single, &drift, std::slice::from_ref(&package)), Some(Job::PlanOperation(next)) if next == operation)
    );
    let batch = Job::UpgradeAll(
        vec![operation.clone()],
        Some(AptUpgradePlan {
            preview: "Synthetic".into(),
            upgrades: vec!["anonymous".into()],
            installs: vec![],
            removals: vec![],
        }),
    );
    assert!(
        matches!(repreview_changed_plan(&batch, &drift, &[package]), Some(Job::PlanUpgrade(next, 1)) if next == vec![operation])
    );
    assert!(repreview_changed_plan(&single, &Err(EngineError::NotFound), &[]).is_none());
    assert!(repreview_changed_plan(&Job::Load("Updates".into(), "".into()), &drift, &[]).is_none());
}
/// A source whose plan is different every time it is asked.
struct Drifting(usize);
impl Backend for Drifting {
    fn id(&self) -> &str {
        "drifting"
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Install]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn operation_plan(
        &mut self,
        operation: &Operation,
        _: &Cancellation,
    ) -> Result<Option<TransactionPlan>, EngineError> {
        self.0 += 1;
        Ok(Some(TransactionPlan {
            operation: operation.clone(),
            native_preview: format!("Synthetic plan {}", self.0),
            changes: vec![],
            download_bytes: None,
            disk_bytes: None,
            restart_required: None,
            adopts: None,
        }))
    }
}
#[test]
fn a_single_change_whose_plan_changed_opens_for_review_again() {
    let mut engine = Engine::default();
    engine.register(Drifting(0)).unwrap();
    let cancel = Cancellation::default();
    let mut package = synthetic_package("synthetic", "Synthetic");
    package.id.backend = "drifting".into();
    let operation = Operation::Install(package.id);
    let plan = engine.plan_operation(&operation, &cancel).unwrap().unwrap();
    engine.remember_operation_plan(plan.clone());
    let job = Job::Write(operation.clone(), Some(Box::new(plan)));
    let result = expect!(
        run_job(&mut engine, job.clone(), &cancel).pop(),
        Some(Reply::Done(result)) => result
    );
    // The engine's own answer, not a made-up one, brings the review back.
    assert!(matches!(
        repreview_changed_plan(&job, &result, &[]),
        Some(Job::PlanOperation(next)) if next == operation
    ));
}
#[test]
fn retry_rejects_unknown_or_disabled_sources_without_starting_work() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .retry_source("Unknown".into(), "".into(), "apt".into());
    controller
        .as_mut()
        .retry_source("Updates".into(), "".into(), "unknown".into());
    controller.as_mut().rust_mut().source_filter = vec!["apt".into()];
    controller
        .as_mut()
        .retry_source("Updates".into(), "".into(), "npm".into());
    assert!(controller.rust().worker.is_none());
    assert!(controller.rust().queued.is_none());
}
#[test]
fn refresh_sources_reloads_only_the_listed_sources() {
    let package = |backend: &str| -> Package {
        serde_json::from_value(json!({
                "id": {"backend":backend, "name":"synthetic-tool", "architecture":"all", "scope":"system"},
                "display_name":"Synthetic tool", "summary":"Synthetic", "installed_version":"1", "candidate_version":"2", "update":"available"
            }))
            .unwrap()
    };
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().active_view = "Updates".into();
    controller.as_mut().rust_mut().packages = vec![package("npm")];

    // Unknown, empty, and filtered-out sources start nothing.
    controller.as_mut().refresh_sources("unknown,,".into());
    controller.as_mut().rust_mut().source_filter = vec!["apt".into()];
    controller.as_mut().refresh_sources("npm".into());
    assert!(controller.rust().worker.is_none());
    assert!(controller.rust().queued.is_none());
    controller.as_mut().rust_mut().source_filter.clear();

    // A foreground read keeps its place and nothing queues behind it.
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Load("Updates".into(), String::new()),
        vec![],
    ));
    controller.as_mut().refresh_sources("apt".into());
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(worker.job, Job::Load(..))
    ));
    assert!(controller.rust().queued.is_none());
    controller.as_mut().rust_mut().worker = None;

    // The refresh reads only apt and leaves npm's rows alone. It does
    // not plan upgrades, so nothing else starts after it lands.
    controller.as_mut().refresh_sources("apt".into());
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::RefreshSources(sources)
            if sources == &["apt"])
    ));
    settle(&mut controller);
    assert!(controller
        .rust()
        .packages
        .iter()
        .any(|p| p.id.backend == "npm"));
    assert!(controller.rust().queued.is_none());
    assert!(controller.rust().worker.is_none());
}
#[test]
fn apt_removals_appear_first_in_update_confirmation() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let plan = AptUpgradePlan {
        preview: "synthetic plan".into(),
        upgrades: vec!["synthetic".into()],
        installs: vec!["dependency".into()],
        removals: vec!["retired".into()],
    };
    controller.as_mut().apply(Ok(Payload::UpgradePreview(
        vec![Operation::UpgradeAll {
            backend: "apt".into(),
        }],
        1,
        Some(plan.clone()),
    )));
    let confirmation = controller.confirmation().to_string();
    assert!(confirmation.contains("Remove (1): retired"));
    let preview: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert!(preview["summary"]
        .as_str()
        .unwrap()
        .contains("Removes: retired"));
    assert!(preview["details"]
        .as_str()
        .unwrap()
        .contains("Remove (1): retired"));
    assert!(
        confirmation.find("Remove (1): retired") < confirmation.find("Update all APT packages")
    );
    assert!(
        matches!(&controller.rust().pending, Some(Job::UpgradeAll(_, Some(saved))) if *saved == plan)
    );
}
#[test]
fn background_inventory_populates_both_sections_without_foreground_changes() {
    // An Updates read refreshes indexes first, so it can fill Installed
    // too. A plain Installed read cannot stand in for Updates.
    for (read, updates_cached) in [("Updates", true), ("Installed", false)] {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        controller.as_mut().set_rows("original".into());
        let package: Package = serde_json::from_value(json!({
            "id": {"backend":"snap", "name":"synthetic", "architecture":"all", "scope":"system"},
            "display_name":"Synthetic", "summary":"Fixture", "installed_version":"1",
            "candidate_version":"2", "update":"available"
        }))
        .unwrap();
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                package,
                fail: false,
            })
            .unwrap();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            execute(
                &mut engine,
                Job::Load(read.into(), "".into()),
                &Cancellation::default(),
                &mut |reply| {
                    sender.send(reply).unwrap();
                },
            )
        });
        handle.join().unwrap();
        controller.as_mut().rust_mut().background = true;
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: Cancellation::default(),
            job: Job::Load(read.into(), "".into()),
        });
        controller.as_mut().poll();
        assert_eq!(controller.rows().to_string(), "original");
        assert!(!controller.busy());
        assert_eq!(
            controller
                .rust()
                .prefetched
                .contains_key(&cache_key("Updates", "", &[], false)),
            updates_cached,
            "{read}"
        );
        // Installed opens from the cache and keeps native updates.
        controller
            .as_mut()
            .load("Installed".into(), "".into(), "".into(), false, false);
        assert!(controller.rust().worker.is_none(), "{read}");
        assert_eq!(controller.rust().packages.len(), 1);
        assert_eq!(
            controller.rust().packages[0].update,
            UpdateAvailability::Available
        );
    }
}

#[test]
fn a_failed_update_refresh_does_not_warm_installed() {
    let installed = cache_key("Installed", "", &[], false);
    let updates = cache_key("Updates", "", &[], false);
    let mut report = PackageReport::default();
    report.failures.push(BackendFailure {
        backend: "homebrew".into(),
        error: EngineError::Cancelled,
    });
    for (refreshed, installed_cached) in [(true, false), (false, true)] {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller
            .as_mut()
            .warm_inventory(report.clone(), refreshed);
        let cached = &controller.rust().prefetched;
        assert_eq!(cached.contains_key(&installed), installed_cached);
        assert_eq!(cached.contains_key(&updates), refreshed);
    }
    // Without a window, nothing is kept for later.
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().set_window_open(false);
    controller
        .as_mut()
        .warm_inventory(PackageReport::default(), true);
    assert!(controller.rust().prefetched.is_empty());
}

#[test]
fn expired_snapshot_stays_visible_and_preempts_background_refresh() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let key = cache_key("Clean", "", &[], false);
    let mut cached = cached_view("cached cleanup");
    cached.loaded = Instant::now() - VIEW_TTL;
    controller
        .as_mut()
        .rust_mut()
        .view_cache
        .insert(key, cached);
    let (_sender, receiver) = mpsc::channel();
    let cancel = Cancellation::default();
    controller.as_mut().rust_mut().background = true;
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: cancel.clone(),
        job: Job::Load("Sources".into(), "".into()),
    });
    controller
        .as_mut()
        .load("Clean".into(), "".into(), "".into(), false, false);
    assert!(controller.rows().to_string().contains("cached cleanup"));
    assert!(cancel.requested());
    assert!(matches!(&controller.rust().queued, Some(Job::Load(view, _)) if view == "Clean"));
}

#[test]
fn a_section_still_loading_never_lands_on_an_empty_search() {
    for queued in [false, true] {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        let (release, gate) = mpsc::channel::<()>();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let _ = gate.recv();
            let mut report = PackageReport::default();
            report
                .packages
                .push(synthetic_package("installed-only", "installed-only"));
            report.successful_sources.push("apt".into());
            let _ = sender.send(Reply::Partial(report.clone()));
            let _ = sender.send(Reply::Done(Ok(Payload::Packages(report))));
        });
        let cancel = Cancellation::default();
        controller.as_mut().rust_mut().active_view = "Installed".into();
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle,
            receiver,
            cancel: cancel.clone(),
            job: Job::Load("Installed".into(), "".into()),
        });
        if queued {
            // Switched on to Updates first, which waits its turn.
            controller.as_mut().rust_mut().queued = Some(Job::Load("Updates".into(), "".into()));
        }
        controller.as_mut().set_busy(true);
        controller
            .as_mut()
            .load("Search".into(), "".into(), "".into(), false, false);
        assert!(cancel.requested());
        assert!(controller.rust().queued.is_none());
        assert!(!*controller.busy());
        release.send(()).unwrap();
        wait_until(&mut controller, |c| c.rust().worker.is_none());
        assert_eq!(controller.rows().to_string(), "[]", "queued: {queued}");
        assert!(!*controller.busy());
    }
}

#[test]
fn an_empty_search_stops_waiting_on_a_preload() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    // Installed waits on its preload: busy, with nothing running.
    controller.as_mut().rust_mut().active_view = "Installed".into();
    controller.as_mut().rust_mut().awaiting_prefetch = true;
    controller.as_mut().set_busy(true);
    controller
        .as_mut()
        .load("Search".into(), "".into(), "".into(), false, false);
    assert!(!*controller.busy());
    assert!(!controller.rust().awaiting_prefetch);
    assert_eq!(controller.rows().to_string(), "[]");
}

#[test]
fn an_empty_search_drops_section_retries() {
    // A retry waiting its turn never starts.
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    controller.as_mut().rust_mut().queued = Some(Job::RetrySource(
        "Installed".into(),
        "".into(),
        "apt".into(),
    ));
    controller
        .as_mut()
        .load("Search".into(), "".into(), "".into(), false, false);
    assert!(controller.rust().queued.is_none());
    // Update all's retry stops, and its rows stay off Search.
    let (sender, receiver) = mpsc::channel();
    let (release, gate) = mpsc::channel::<()>();
    let handle = thread::spawn(move || {
        let _ = gate.recv();
        let mut report = PackageReport::default();
        report
            .packages
            .push(synthetic_package("updates-only", "updates-only"));
        report.successful_sources.push("apt".into());
        let _ = sender.send(Reply::Done(Ok(Payload::RetryFailedUpdates(
            vec!["apt".into()],
            report,
        ))));
    });
    let cancel = Cancellation::default();
    controller.as_mut().rust_mut().active_view = "Updates".into();
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle,
        receiver,
        cancel: cancel.clone(),
        job: Job::RetryFailedUpdates(vec!["apt".into()]),
    });
    controller.as_mut().set_busy(true);
    controller
        .as_mut()
        .load("Search".into(), "".into(), "".into(), false, false);
    assert!(cancel.requested());
    assert!(!*controller.busy());
    release.send(()).unwrap();
    wait_until(&mut controller, |c| c.rust().worker.is_none());
    assert_eq!(controller.rows().to_string(), "[]");
}

#[test]
fn an_empty_search_is_idle_beside_background_checks() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    let (_sender, receiver) = mpsc::channel::<Reply>();
    let cancel = Cancellation::default();
    // Installed cancelled the background check to wait on its preload.
    cancel.cancel();
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel,
        job: Job::BackgroundUpdates(vec!["apt".into()]),
    });
    controller.as_mut().rust_mut().background = true;
    controller.as_mut().rust_mut().active_view = "Installed".into();
    controller.as_mut().rust_mut().awaiting_prefetch = true;
    controller.as_mut().set_busy(true);
    controller
        .as_mut()
        .load("Search".into(), "".into(), "".into(), false, false);
    assert!(!*controller.busy());
    assert!(controller.rust().worker.is_some());
}

#[test]
fn stale_snapshot_stays_whole_while_its_refresh_streams() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let key = cache_key("Installed", "", &[], false);
    let mut cached = cached_view("cached package");
    cached.expired = true;
    controller
        .as_mut()
        .rust_mut()
        .view_cache
        .insert(key, cached);
    let (_sender, receiver) = mpsc::channel();
    controller.as_mut().rust_mut().background = true;
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: Cancellation::default(),
        job: Job::Load("Sources".into(), "".into()),
    });
    controller
        .as_mut()
        .load("Installed".into(), "".into(), "".into(), false, false);
    assert!(controller.rows().to_string().contains("cached package"));
    assert!(controller.rust().hold_partials);
    // The refresh streams one backend's rows before the rest.
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Reply::Partial(PackageReport::default()))
        .unwrap();
    controller.as_mut().rust_mut().background = false;
    controller.as_mut().rust_mut().queued = None;
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| thread::sleep(Duration::from_millis(200))),
        receiver,
        cancel: Cancellation::default(),
        job: Job::Load("Installed".into(), "".into()),
    });
    controller.as_mut().poll();
    assert!(controller.rows().to_string().contains("cached package"));
    // A fresh visit streams partials as usual.
    controller
        .as_mut()
        .load("Search".into(), "query".into(), "".into(), false, false);
    assert!(!controller.rust().hold_partials);
}

/// A fresh directory for one test, removed when it ends.
struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pkgdeck-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn wait_for_file(path: &std::path::Path, contains: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !std::fs::read_to_string(path).is_ok_and(|text| text.contains(contains)) {
        assert!(Instant::now() < deadline, "never held {contains}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn installed(controller: &mut Pin<&mut ffi::PackageController>, name: &str) -> String {
    let package: Package = serde_json::from_value(json!({
        "id": {"backend":"apt", "name":name, "architecture":"all", "scope":"system"},
        "display_name":name, "summary":"Kept package", "installed_version":"1", "update":"current"
    }))
    .unwrap();
    controller.as_mut().rust_mut().packages = vec![package];
    controller
        .as_mut()
        .set_rows(json!([{"kind":"package","name":name}]).to_string().into());
    cache_key("Installed", "", &[], false)
}

#[test]
fn kept_sections_show_at_once_on_the_next_launch() {
    let dir = Scratch::new("kept-views");
    let path = dir.path().join("pkgdeck/views.json");
    let mut first = ffi::create_controller();
    let mut first = first.pin_mut();
    first
        .as_mut()
        .rust_mut()
        .open_view_store(Some(path.clone()));
    let key = installed(&mut first, "kept-tool");
    first.as_mut().stash_current(key.clone());
    wait_for_file(&path, "kept-tool");
    assert!(!path.with_extension("json.tmp").exists());

    let mut next = ffi::create_controller();
    let mut next = next.pin_mut();
    next.as_mut().rust_mut().open_view_store(Some(path));
    let cached = next.rust().view_cache.get(&key).unwrap();
    assert!(cached.stale());
    assert_eq!(cached.packages[0].id.name, "kept-tool");
    next.as_mut()
        .load("Installed".into(), "".into(), "".into(), false, false);
    assert!(next.rows().to_string().contains("kept-tool"));
    assert!(next
        .report_state()
        .to_string()
        .contains(r#""phase":"stale""#));
}

#[test]
fn searches_and_sources_are_not_kept() {
    let dir = Scratch::new("unkept-views");
    let path = dir.path().join("views.json");
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .rust_mut()
        .open_view_store(Some(path.clone()));
    installed(&mut controller, "searched-tool");
    controller
        .as_mut()
        .stash_current(cache_key("Search", "searched", &[], false));
    controller
        .as_mut()
        .stash_current(cache_key("Sources", "", &[], false));
    // A section with a failed source keeps its rows, not the failure.
    let key = installed(&mut controller, "partly-checked-tool");
    controller.as_mut().rust_mut().failures = vec![BackendFailure {
        backend: "snap".into(),
        error: EngineError::Execution(ExecutionError::Cancelled),
    }];
    controller.as_mut().stash_current(key);
    wait_for_file(&path, "partly-checked-tool");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("searched-tool"));
    assert!(!text.contains("Sources"));
    assert!(!text.contains("snap"));
}

#[test]
fn an_older_write_never_replaces_a_newer_one() {
    let dir = Scratch::new("ordered-views");
    let path = dir.path().join("views.json");
    let written = std::sync::Mutex::new(std::collections::HashMap::new());
    let stored = |key: &str, status: &str| StoredView {
        key: key.into(),
        packages: vec![],
        cleanup: vec![],
        rows: "[]".into(),
        status: status.into(),
        report_state: String::new(),
        upgradable: false,
        updates_view: false,
    };
    assert!(write_view(&path, stored("Installed", "newer"), 2, &written));
    assert!(!write_view(
        &path,
        stored("Installed", "older"),
        1,
        &written
    ));
    // Another section's older write still lands, beside the first.
    assert!(write_view(&path, stored("Updates", "other"), 1, &written));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("newer") && !text.contains("older"));
    assert!(text.contains("other"));
    // A file in the way of the directory is a failed write, not a crash.
    let blocked = dir.path().join("views.json").join("views.json");
    assert!(!write_view(
        &blocked,
        stored("Clean", "blocked"),
        3,
        &written
    ));
}

#[test]
fn a_closed_window_keeps_no_sections_in_memory_and_gets_them_back() {
    let dir = Scratch::new("window-views");
    let path = dir.path().join("views.json");
    let key = cache_key("Installed", "", &[], false);
    let view = StoredView {
        key: key.clone(),
        packages: vec![],
        cleanup: vec![],
        rows: "[]".into(),
        status: "kept".into(),
        report_state: String::new(),
        upgradable: false,
        updates_view: false,
    };
    let written = std::sync::Mutex::new(std::collections::HashMap::new());
    assert!(write_view(&path, view, 1, &written));
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .rust_mut()
        .open_view_store(Some(path.clone()));
    assert!(controller.rust().view_cache.get(&key).is_some());
    assert!(!controller.rust().prefetch.is_empty());

    controller.as_mut().set_window_open(false);
    let rust = controller.rust();
    assert!(rust.view_cache.entries.is_empty());
    assert!(rust.prefetch.is_empty());
    assert!(rust.engine.is_none());
    // Nothing left to load, so the tray can sleep.
    assert!(!rust.outstanding());
    assert!(!*controller.needs_poll());
    // Closing twice changes nothing.
    controller.as_mut().set_window_open(false);
    assert!(controller.rust().prefetch.is_empty());

    controller.as_mut().set_window_open(true);
    assert!(controller.rust().view_cache.get(&key).is_some());
    assert_eq!(controller.rust().prefetch, prefetch_views());
    // Opening twice changes nothing.
    controller.as_mut().set_window_open(true);
    assert!(controller.rust().view_cache.get(&key).is_some());
    // Sections already in memory aren't read again.
    controller.as_mut().set_window_open(false);
    let fresh = cache_key("Updates", "", &[], false);
    controller
        .as_mut()
        .rust_mut()
        .view_cache
        .insert(fresh.clone(), cached_view("loaded while closed"));
    controller.as_mut().set_window_open(true);
    let cache = &controller.rust().view_cache;
    assert!(cache.get(&fresh).is_some());
    assert!(cache.get(&key).is_none());
}

#[test]
fn a_new_copy_waits_out_the_last_ones_check() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().resume_checks(100);
    let schedule = &mut controller.as_mut().rust_mut().background_schedule;
    assert!(!schedule.ready(150, true, false, false, false, false));
}

#[test]
fn a_damaged_or_older_store_is_ignored() {
    let dir = Scratch::new("damaged-views");
    let key = cache_key("Installed", "", &[], false);
    for text in [
            "not json".to_owned(),
            json!({"version": "0.0.1", "views": [{"key": key, "packages": [], "cleanup": [], "rows": "[]", "status": "", "report_state": "", "upgradable": false, "updates_view": false}]}).to_string(),
        ] {
            let path = dir.path().join("views.json");
            std::fs::write(&path, text).unwrap();
            let mut controller = ffi::create_controller();
            let mut controller = controller.pin_mut();
            controller.as_mut().rust_mut().open_view_store(Some(path));
            assert!(controller.rust().view_cache.get(&key).is_none());
        }
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().open_view_store(None);
    assert!(controller.rust().view_store.is_none());
}

#[test]
fn update_all_retry_replaces_the_saved_updates_section() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let key = cache_key("Updates", "", &[], false);
    controller
        .as_mut()
        .rust_mut()
        .view_cache
        .insert(key.clone(), cached_view("before retry"));
    controller.as_mut().rust_mut().view_cache.expire();
    let package: Package = serde_json::from_value(json!({
            "id": {"backend":"apt", "name":"retried-tool", "architecture":"all", "scope":"system"},
            "display_name":"retried-tool", "summary":"Retried package", "installed_version":"1", "candidate_version":"2", "update":"available"
        }))
        .unwrap();
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Reply::Done(Ok(Payload::RetryFailedUpdates(
            vec!["apt".into()],
            PackageReport {
                packages: vec![package],
                failures: vec![],
                successful_sources: vec!["apt".into()],
            },
        ))))
        .unwrap();
    let handle = thread::spawn(|| {});
    while !handle.is_finished() {
        thread::yield_now();
    }
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle,
        receiver,
        cancel: Cancellation::default(),
        job: Job::RetryFailedUpdates(vec!["apt".into()]),
    });
    controller.as_mut().poll();
    let saved = controller.rust().view_cache.get(&key).unwrap();
    assert!(!saved.stale());
    assert!(saved.rows.to_string().contains("retried-tool"));
    assert!(!saved.rows.to_string().contains("before retry"));
}

#[test]
fn forced_reload_of_the_open_section_keeps_its_rows_until_complete() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().active_view = "Updates".into();
    controller.as_mut().set_rows(r#"[{"name":"shown"}]"#.into());
    let (_sender, receiver) = mpsc::channel();
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: Cancellation::default(),
        job: Job::Load("Updates".into(), "".into()),
    });
    controller
        .as_mut()
        .load("Updates".into(), "".into(), "".into(), false, true);
    assert!(controller.rust().hold_partials);
    // Reloading again before the refresh finishes still keeps them.
    assert_eq!(controller.rows().to_string(), "[]");
    controller
        .as_mut()
        .load("Updates".into(), "".into(), "".into(), false, true);
    assert!(controller.rust().hold_partials);
    controller.as_mut().rust_mut().active_view = "Installed".into();
    controller
        .as_mut()
        .load("Updates".into(), "".into(), "".into(), false, true);
    assert!(!controller.rust().hold_partials);
}

#[test]
fn confirmed_write_queued_over_background_worker_reports_busy() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let (_sender, receiver) = mpsc::channel();
    controller.as_mut().rust_mut().background = true;
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: Cancellation::default(),
        job: Job::Load("Sources".into(), "".into()),
    });
    controller.as_mut().rust_mut().pending = Some(Job::Write(
        Operation::Refresh {
            backend: "fixture".into(),
        },
        None,
    ));
    controller.as_mut().confirm(true);
    // The write waits for the background worker to wind down, but the
    // UI must already report busy so confirmed work is never read as
    // idle before it starts.
    assert!(matches!(
        controller.rust().confirmed_queue.front().map(|entry| &entry.job),
        Some(Job::Write(Operation::Refresh { backend }, _)) if backend == "fixture"
    ));
    assert!(*controller.busy());
}

#[test]
fn confirmed_queue_keeps_order_and_rejects_duplicate_or_conflicting_targets() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let (_sender, receiver) = mpsc::channel();
    let first = PackageId {
        backend: "fixture".into(),
        name: "first".into(),
        architecture: "all".into(),
        scope: Scope::System,
        remote: None,
        reference: None,
    };
    let second = PackageId {
        name: "second".into(),
        ..first.clone()
    };
    let third = PackageId {
        name: "third".into(),
        ..first.clone()
    };
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: Cancellation::default(),
        job: Job::Write(Operation::Install(first.clone()), None),
    });
    controller
        .as_mut()
        .accept_confirmed(Job::Write(Operation::Install(first.clone()), None));
    controller
        .as_mut()
        .accept_confirmed(Job::Write(Operation::Remove(first), None));
    assert!(controller.rust().confirmed_queue.is_empty());
    controller
        .as_mut()
        .accept_confirmed(Job::Write(Operation::Install(second.clone()), None));
    controller
        .as_mut()
        .accept_confirmed(Job::Write(Operation::Install(third.clone()), None));
    assert_eq!(controller.rust().confirmed_queue.len(), 2);
    assert!(
        matches!(&controller.rust().confirmed_queue[0].job, Job::Write(Operation::Install(id), _) if id == &second)
    );
    assert!(
        matches!(&controller.rust().confirmed_queue[1].job, Job::Write(Operation::Install(id), _) if id == &third)
    );
    controller.as_mut().cancel_queued();
    assert!(controller.rust().confirmed_queue.is_empty());
    let validating = Confirmed {
        job: Job::Write(Operation::Install(second.clone()), None),
        activity_id: None,
        cleanup_preview: vec![],
    };
    assert_eq!(
        queue_conflict(
            &Job::Write(Operation::Remove(second), None),
            None,
            &VecDeque::new(),
            Some(&validating),
            None,
        ),
        Some("A conflicting operation is already running or queued.")
    );
    let clean = CleanupId {
        backend: "fixture".into(),
        key: "cache".into(),
    };
    let active = Job::CleanAll(vec![Operation::Clean(clean.clone())]);
    assert_eq!(
        queue_conflict(
            &Job::Write(Operation::Clean(clean), None),
            Some(&active),
            &VecDeque::new(),
            None,
            None
        ),
        Some("This operation is already running or queued.")
    );
    let active = Job::UpgradeAll(
        vec![Operation::UpgradeAll {
            backend: "fixture".into(),
        }],
        None,
    );
    assert_eq!(
        queue_conflict(
            &Job::Write(Operation::Upgrade(third), None),
            Some(&active),
            &VecDeque::new(),
            None,
            None
        ),
        Some("A conflicting operation is already running or queued.")
    );
}

#[test]
fn cancelling_queued_revalidation_discards_a_late_preview() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let operation = Operation::Refresh {
        backend: "fixture".into(),
    };
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Reply::Done(Ok(Payload::OperationPreview(
            operation.clone(),
            None,
        ))))
        .unwrap();
    controller.as_mut().rust_mut().revalidating = Some(Confirmed {
        job: Job::Write(operation.clone(), None),
        activity_id: None,
        cleanup_preview: vec![],
    });
    let cancel = Cancellation::default();
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: cancel.clone(),
        job: Job::PlanOperation(operation),
    });
    controller.as_mut().cancel_queued();
    assert!(cancel.requested());
    controller.as_mut().poll();
    assert!(controller.rust().pending.is_none());
    assert!(controller.confirmation().is_empty());
}

#[test]
fn background_check_policy_and_results_leave_the_current_view_intact() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().set_rows("current view".into());
    controller
        .as_mut()
        .check_updates("apt".into(), false, false, false, false);
    controller
        .as_mut()
        .check_updates("apt".into(), true, true, false, false);
    controller
        .as_mut()
        .check_updates("apt".into(), true, false, true, false);
    assert!(controller.rust().worker.is_none());
    controller
        .as_mut()
        .check_updates("unknown".into(), true, false, false, false);
    assert!(controller.rust().worker.is_none());

    let package: Package = serde_json::from_value(json!({
        "id": {"backend":"apt", "name":"synthetic", "architecture":"amd64", "scope":"system"},
        "display_name":"Synthetic", "summary":"Fixture", "installed_version":"1",
        "candidate_version":"2", "update":"available"
    }))
    .unwrap();
    let report = PackageReport {
        packages: vec![package],
        failures: vec![],
        successful_sources: vec!["apt".into()],
    };
    controller.as_mut().finish_background_check(report.clone());
    let state: Value = serde_json::from_str(&controller.background_state().to_string()).unwrap();
    assert_eq!(state["available"], 1);
    assert_eq!(state["notify"], true);
    assert_eq!(controller.rows().to_string(), "current view");
    controller.as_mut().acknowledge_notification();
    let acknowledged = controller.notification_history().to_string();
    // A restart restores what was already announced.
    let mut restarted = ffi::create_controller();
    let mut restarted = restarted.pin_mut();
    restarted
        .as_mut()
        .restore_notification_history(acknowledged.as_str().into());
    assert_eq!(restarted.notification_history().to_string(), acknowledged);
    restarted.as_mut().finish_background_check(report.clone());
    let state: Value = serde_json::from_str(&restarted.background_state().to_string()).unwrap();
    assert_eq!(state["notify"], false);
    controller.as_mut().finish_background_check(report.clone());
    let state: Value = serde_json::from_str(&controller.background_state().to_string()).unwrap();
    assert_eq!(state["notify"], false);

    let mut changed = report;
    changed.packages[0].candidate_version = Some("3".into());
    controller.as_mut().finish_background_check(changed);
    let state: Value = serde_json::from_str(&controller.background_state().to_string()).unwrap();
    assert_eq!(state["notify"], true);

    controller.as_mut().finish_background_check(PackageReport {
        packages: vec![],
        failures: vec![BackendFailure {
            backend: "fixture".into(),
            error: EngineError::Cancelled,
        }],
        successful_sources: vec![],
    });
    let state: Value = serde_json::from_str(&controller.background_state().to_string()).unwrap();
    assert_eq!(state["notify"], false);
    assert_eq!(state["failures"][0]["source"], "fixture");
}

#[test]
fn writes_serve_cached_navigation_and_defer_new_reads_without_cancelling() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().view_cache.insert(
        cache_key("Installed", "", &[], false),
        cached_view("retained package"),
    );
    let (_sender, receiver) = mpsc::channel();
    let cancel = Cancellation::default();
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: cancel.clone(),
        job: Job::Write(
            Operation::Refresh {
                backend: "fixture".into(),
            },
            None,
        ),
    });
    controller
        .as_mut()
        .load("Installed".into(), "".into(), "".into(), false, false);
    assert!(controller.rows().to_string().contains("retained package"));
    assert!(!cancel.requested());
    assert!(
        matches!(&controller.rust().deferred_load, Some(Job::Load(view, _)) if view == "Installed")
    );
    controller
        .as_mut()
        .load("Search".into(), "new query".into(), "".into(), false, false);
    assert_eq!(controller.rows().to_string(), "[]");
    assert!(
        matches!(&controller.rust().deferred_load, Some(Job::Load(view, query)) if view == "Search" && query == "new query")
    );
    assert!(!cancel.requested());
}

#[test]
fn queued_previews_only_reuse_an_unchanged_confirmation() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let operation = Operation::Refresh {
        backend: "fixture".into(),
    };
    let queued = || Confirmed {
        job: Job::Write(operation.clone(), None),
        activity_id: None,
        cleanup_preview: vec![],
    };
    controller.as_mut().rust_mut().revalidating = Some(queued());
    controller
        .as_mut()
        .apply(Ok(Payload::OperationPreview(operation.clone(), None)));
    assert!(controller.rust().validated_confirmed.is_some());
    assert!(controller.confirmation().is_empty());
    controller.as_mut().rust_mut().validated_confirmed = None;
    controller.as_mut().rust_mut().revalidating = Some(queued());
    controller.as_mut().apply(Ok(Payload::OperationPreview(
        Operation::Refresh {
            backend: "changed".into(),
        },
        None,
    )));
    assert!(controller.rust().validated_confirmed.is_none());
    assert!(controller.confirmation().to_string().contains("changed"));
}

#[test]
fn queued_cleanup_requires_new_consent_when_its_native_preview_changes() {
    let item = CleanupItem {
        id: CleanupId {
            backend: "fixture".into(),
            key: "cache".into(),
        },
        kind: CleanupKind::OrphanDependencies,
        title: "Synthetic cleanup".into(),
        summary: "Fixture".into(),
        preview: "Remove one cached file".into(),
        bytes: None,
    };
    let operations = vec![Operation::Clean(item.id.clone())];
    let queued = || Confirmed {
        job: Job::CleanAll(operations.clone()),
        activity_id: None,
        cleanup_preview: vec![item.clone()],
    };
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().revalidating = Some(queued());
    controller.as_mut().apply(Ok(Payload::CleanPreview(
        operations.clone(),
        vec![item.clone()],
    )));
    assert!(controller.rust().validated_confirmed.is_some());
    assert!(controller.confirmation().is_empty());

    controller.as_mut().rust_mut().validated_confirmed = None;
    controller.as_mut().rust_mut().revalidating = Some(queued());
    let mut changed = item.clone();
    changed.preview = "Remove two cached files".into();
    controller
        .as_mut()
        .apply(Ok(Payload::CleanPreview(operations.clone(), vec![changed])));
    assert!(controller.rust().validated_confirmed.is_none());
    assert!(controller
        .confirmation()
        .to_string()
        .contains("Remove two cached files"));
    assert!(matches!(controller.rust().pending, Some(Job::CleanAll(_))));

    controller.as_mut().rust_mut().pending = None;
    controller.as_mut().set_confirmation(QString::default());
    controller.as_mut().rust_mut().revalidating = Some(queued());
    controller
        .as_mut()
        .apply(Ok(Payload::CleanPreview(operations, vec![])));
    assert!(controller.rust().pending.is_none());
    assert!(controller
        .status()
        .to_string()
        .contains("no longer available"));
}

#[test]
fn queued_writes_wait_for_a_read_worker_then_revalidate_each_job_kind() {
    let clean = Operation::Clean(CleanupId {
        backend: "fixture".into(),
        key: "cache".into(),
    });
    let jobs = [
        Job::Write(
            Operation::Refresh {
                backend: "fixture".into(),
            },
            None,
        ),
        Job::UpgradeAll(
            vec![Operation::UpgradeAll {
                backend: "fixture".into(),
            }],
            None,
        ),
        Job::CleanAll(vec![clean]),
    ];
    for (index, job) in jobs.into_iter().enumerate() {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        let (_sender, receiver) = mpsc::channel();
        let cancel = Cancellation::default();
        controller.as_mut().rust_mut().background = true;
        controller.as_mut().rust_mut().worker = Some(Worker {
            handle: thread::spawn(|| {}),
            receiver,
            cancel: cancel.clone(),
            job: Job::Load("Sources".into(), "".into()),
        });
        controller.as_mut().validate_confirmed(Confirmed {
            job,
            activity_id: None,
            cleanup_preview: vec![],
        });
        assert!(controller.rust().revalidating.is_some());
        assert!(cancel.requested());
        assert!(matches!(
            (index, &controller.rust().queued),
            (0, Some(Job::PlanOperation(_)))
                | (1, Some(Job::PlanUpgrade(_, _)))
                | (2, Some(Job::PlanCleanAll(_)))
        ));
    }
}

#[test]
fn background_update_job_reads_only_available_updates() {
    let package: Package = serde_json::from_value(json!({
        "id": {"backend":"fixture", "name":"synthetic", "architecture":"all", "scope":"system"},
        "display_name":"Synthetic", "summary":"Fixture", "installed_version":"1",
        "candidate_version":"2", "update":"available"
    }))
    .unwrap();
    let mut engine = Engine::default();
    engine
        .register(Fixture {
            package,
            fail: false,
        })
        .unwrap();
    let mut replies = vec![];
    execute(
        &mut engine,
        Job::BackgroundUpdates(vec!["fixture".into()]),
        &Cancellation::default(),
        &mut |reply| replies.push(reply),
    );
    assert!(matches!(
        replies.pop(),
        Some(Reply::Done(Ok(Payload::BackgroundUpdates(report))))
            if report.packages.len() == 1 && report.packages[0].id.name == "synthetic"
    ));
}

#[test]
fn background_worker_error_updates_failure_state_without_losing_known_count() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().set_background_state(encoded(json!({
        "last_check": 1, "available": 3, "failures": [], "notify": true
    })));
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Reply::Done(Err(EngineError::Unavailable {
            backend: "fixture".into(),
            reason: "temporarily offline".into(),
        })))
        .unwrap();
    drop(sender);
    controller.as_mut().rust_mut().background = true;
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: Cancellation::default(),
        job: Job::BackgroundUpdates(vec!["fixture".into()]),
    });
    controller.as_mut().poll();
    let state: Value = serde_json::from_str(&controller.background_state().to_string()).unwrap();
    assert_eq!(state["available"], 3);
    assert_eq!(state["notify"], false);
    assert_eq!(state["failures"][0]["kind"], "unavailable");
    assert!(state["last_check"].as_u64().unwrap() > 1);
    assert!(!controller.busy());

    controller
        .as_mut()
        .finish_background_error(&EngineError::Incomplete(vec![
            BackendFailure {
                backend: "apt".into(),
                error: EngineError::Unavailable {
                    backend: "apt".into(),
                    reason: "offline".into(),
                },
            },
            BackendFailure {
                backend: "flatpak".into(),
                error: EngineError::Cancelled,
            },
        ]));
    let state: Value = serde_json::from_str(&controller.background_state().to_string()).unwrap();
    assert_eq!(state["available"], 3);
    assert_eq!(state["failures"][0]["source"], "apt");
    assert_eq!(state["failures"][1]["kind"], "cancelled");
}

#[test]
fn autostart_only_persists_after_a_successful_file_change() {
    let path = std::env::temp_dir().join(format!(
        "pkgdeck-controller-autostart-{}",
        std::process::id()
    ));
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .use_autostart_path(path.join("autostart/app.desktop"));
    assert!(controller.as_mut().set_autostart(true));
    assert!(std::fs::read_to_string(path.join("autostart/app.desktop"))
        .unwrap()
        .contains("--background"));
    assert!(controller.as_mut().set_autostart(false));
    assert!(!path.join("autostart/app.desktop").exists());
    std::fs::write(path.join("blocked"), "synthetic").unwrap();
    controller.as_mut().rust_mut().autostart_path = Some(path.join("blocked/app.desktop"));
    assert!(!controller.as_mut().set_autostart(true));
    assert!(controller
        .status()
        .to_string()
        .contains("Autostart couldn't be changed"));
    controller.as_mut().rust_mut().autostart_path = None;
    assert!(!controller.as_mut().set_autostart(true));
    assert!(controller
        .status()
        .to_string()
        .contains("No user configuration directory"));
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn queued_activity_and_mixed_batch_results_use_exact_targets() {
    let path = std::env::temp_dir().join(format!(
        "pkgdeck-controller-activity-{}",
        std::process::id()
    ));
    let store = History::new(path.join("activity.json"));
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().activity_store =
        Some(ActivityStore::at(path.join("activity.json")));
    let first = Operation::Upgrade(PackageId {
        backend: "fixture".into(),
        name: "first".into(),
        architecture: "all".into(),
        scope: Scope::System,
        remote: None,
        reference: None,
    });
    let second = Operation::Upgrade(PackageId {
        backend: "fixture".into(),
        name: "second".into(),
        architecture: "all".into(),
        scope: Scope::User { uid: 1234 },
        remote: None,
        reference: None,
    });
    let operations = vec![first, second];
    let (_sender, receiver) = mpsc::channel();
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: Cancellation::default(),
        job: Job::Load("Sources".into(), "".into()),
    });
    controller
        .as_mut()
        .accept_confirmed(Job::UpgradeAll(operations.clone(), None));
    let entries = store.entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].operations, operations);
    assert_eq!(entries[0].state, State::Queued);
    // Activity is read on its own thread and published by poll().
    assert!(*controller.needs_poll());
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !controller.activity().to_string().contains("queued") {
        controller.as_mut().poll_activity();
        thread::sleep(Duration::from_millis(2));
    }
    assert!(controller.activity().to_string().contains("queued"));
    assert!(controller
        .activity()
        .to_string()
        .contains("Update first (fixture)"));
    controller.as_mut().cancel_queued();
    assert_eq!(store.entries().unwrap()[0].state, State::Cancelled);
    controller.as_mut().rust_mut().worker = None;

    let id = store
        .begin("gui", operations.clone(), vec![], State::Running)
        .unwrap();
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Reply::Done(Ok(ran(&[
            ("First", Outcome::Finished, Said::Completed),
            ("Second", Outcome::Failed, because("busy")),
        ]))))
        .unwrap();
    controller.as_mut().rust_mut().active_activity_id = Some(id);
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: Cancellation::default(),
        job: Job::UpgradeAll(operations.clone(), None),
    });
    controller.as_mut().poll();
    let entry = store
        .entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == id)
        .unwrap();
    assert_eq!(entry.state, State::Failed);
    assert_eq!(entry.outcomes, [Outcome::Finished, Outcome::Failed]);
    assert_eq!(entry.operations, operations);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn failed_queued_revalidation_records_failure_without_starting_a_write() {
    let path = std::env::temp_dir().join(format!(
        "pkgdeck-revalidation-activity-{}",
        std::process::id()
    ));
    let store = History::new(path.join("activity.json"));
    let operation = Operation::Refresh {
        backend: "fixture".into(),
    };
    let id = store
        .begin("gui", vec![operation.clone()], vec![], State::Queued)
        .unwrap();
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().activity_store =
        Some(ActivityStore::at(path.join("activity.json")));
    controller.as_mut().rust_mut().revalidating = Some(Confirmed {
        job: Job::Write(operation, None),
        activity_id: Some(id),
        cleanup_preview: vec![],
    });
    controller.as_mut().apply(Err(EngineError::NotFound));
    assert!(controller.rust().revalidating.is_none());
    assert!(controller.rust().worker.is_none());
    let entry = store.entries().unwrap().pop().unwrap();
    assert_eq!(entry.state, State::Failed);
    assert_eq!(entry.outcomes, [Outcome::Failed]);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn unavailable_confirmed_target_has_a_terminal_activity_result() {
    let path = std::env::temp_dir().join(format!(
        "pkgdeck-unavailable-activity-{}",
        std::process::id()
    ));
    let store = History::new(path.join("activity.json"));
    let operation = Operation::Refresh {
        backend: "missing-fixture".into(),
    };
    let id = store
        .begin("gui", vec![operation.clone()], vec![], State::Queued)
        .unwrap();
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().activity_store =
        Some(ActivityStore::at(path.join("activity.json")));
    controller.as_mut().start_confirmed(Confirmed {
        job: Job::Write(operation, None),
        activity_id: Some(id),
        cleanup_preview: vec![],
    });
    assert_eq!(store.entries().unwrap()[0].state, State::Running);
    wait_until(&mut controller, |controller| {
        controller.rust().worker.is_none()
    });
    let entry = store
        .entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == id)
        .unwrap();
    assert_eq!(entry.state, State::Failed);
    assert_eq!(entry.outcomes, [Outcome::Failed]);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn changed_native_plan_is_reviewed_before_a_deferred_page_load() {
    let operation = Operation::Refresh {
        backend: "missing-fixture".into(),
    };
    let plan = TransactionPlan {
        operation: operation.clone(),
        native_preview: "synthetic".into(),
        changes: vec![],
        download_bytes: None,
        disk_bytes: None,
        restart_required: None,
        adopts: None,
    };
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Reply::Done(Err(EngineError::Refused {
            backend: "missing-fixture".into(),
            refusal: Refusal::PlanChanged,
        })))
        .unwrap();
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: Cancellation::default(),
        job: Job::Write(operation.clone(), Some(Box::new(plan))),
    });
    controller.as_mut().rust_mut().deferred_load = Some(Job::Load("Search".into(), "query".into()));
    controller.as_mut().poll();
    assert!(
        matches!(controller.rust().worker.as_ref().map(|worker| &worker.job), Some(Job::PlanOperation(op)) if op == &operation)
    );
    assert!(
        matches!(&controller.rust().deferred_load, Some(Job::Load(view, query)) if view == "Search" && query == "query")
    );
    // The banner asks for another review instead of reporting a failure.
    let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
    assert_eq!(notice["kind"], "info");
    assert!(notice["title"]
        .as_str()
        .unwrap()
        .contains("Review them again"));
}

#[test]
fn selection_during_a_write_uses_retained_package_details() {
    let package: Package = serde_json::from_value(json!({
        "id": {"backend":"fixture", "name":"selected", "architecture":"all", "scope":"system"},
        "display_name":"Selected", "summary":"Synthetic details", "installed_version":"1",
        "candidate_version":"2", "update":"available"
    }))
    .unwrap();
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().packages = vec![package];
    let (_sender, receiver) = mpsc::channel();
    let cancel = Cancellation::default();
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: cancel.clone(),
        job: Job::Write(
            Operation::Refresh {
                backend: "fixture".into(),
            },
            None,
        ),
    });
    controller.as_mut().select(0);
    assert_eq!(
        controller.rust().selected.as_ref().unwrap().name,
        "selected"
    );
    assert!(controller
        .details()
        .to_string()
        .contains("Synthetic details"));
    assert!(!cancel.requested());
}

#[test]
fn label_helpers_cover_scopes_sources_and_cleanup_operations() {
    use std::path::PathBuf;
    assert_eq!(scope_label(&Scope::System), "System");
    assert_eq!(scope_label(&Scope::User { uid: 1000 }), "User 1000");
    assert_eq!(
        scope_label(&Scope::Environment {
            path: PathBuf::from("/tmp/fixture")
        }),
        "/tmp/fixture"
    );
    let unavailable = Source {
        backend: "apt".into(),
        capabilities: vec![],
        availability: Ok(Availability::Unavailable("locked".into())),
    };
    assert_eq!(source_status(&unavailable), "Unavailable: locked");
    let failed = Source {
        backend: "apt".into(),
        capabilities: vec![],
        availability: Err(EngineError::Cancelled),
    };
    assert_eq!(source_status(&failed), "Cancelled.");
    let clean = Operation::Clean(CleanupId {
        backend: "apt".into(),
        key: "autoremove".into(),
    });
    assert_eq!(operation_label(&clean), "Clean autoremove (APT)");
    assert!(confirmation_label(&clean, &[]).contains("Clean autoremove (APT)"));
    let firmware = |name: &str| Package {
        id: PackageId {
            backend: "fwupd".into(),
            name: name.into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        },
        display_name: name.into(),
        summary: "synthetic firmware".into(),
        installed_version: Some("1".into()),
        candidate_version: Some("2".into()),
        update: UpdateAvailability::Available,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
        adopt_with: None,
    };
    let other = firmware("other-device");
    let target = firmware("target-device");
    let label = confirmation_label(&Operation::Upgrade(target.id.clone()), &[other, target]);
    assert!(label.contains("target-device: synthetic firmware"));
    assert!(!label.contains("other-device: synthetic firmware"));
}
#[test]
fn jobs_route_to_their_own_sources_and_errors_have_a_kind() {
    let filter = vec!["filtered".to_owned()];
    let id = synthetic_package("editor", "Editor").id;
    let upgrade = |backend: &str| Operation::UpgradeAll {
        backend: backend.into(),
    };
    let clean = Operation::Clean(CleanupId {
        backend: "flatpak".into(),
        key: "unused".into(),
    });
    for (job, sources) in [
        (Job::RetryFailedUpdates(vec!["snap".into()]), vec!["snap"]),
        (Job::BackgroundUpdates(vec!["dnf".into()]), vec!["dnf"]),
        (Job::PlanCleanAll(vec![clean.clone()]), vec!["flatpak"]),
        (
            Job::ManifestPreview(PathBuf::from("inventory.json")),
            vec![],
        ),
        (
            Job::PlanUpgrade(vec![upgrade("flatpak"), upgrade("apt")], 2),
            vec!["apt"],
        ),
        (
            Job::PlanUpgrade(vec![upgrade("flatpak")], 1),
            vec!["filtered"],
        ),
        (Job::Details(id.clone()), vec!["apt"]),
    ] {
        assert_eq!(engine_source(&job, &filter), sources);
    }

    let install = Operation::Install(id.clone());
    assert!(same_target(&install, &Operation::Remove(id.clone())));
    assert!(same_target(&clean, &clean.clone()));
    assert!(!same_target(
        &clean,
        &Operation::Clean(CleanupId {
            backend: "flatpak".into(),
            key: "other".into(),
        })
    ));
    assert!(same_target(&upgrade("apt"), &install));
    assert!(!same_target(&clean, &install));

    let source = |availability| Source {
        backend: "flatpak".into(),
        capabilities: vec![],
        availability,
    };
    let restricted = source_row(&source(Ok(Availability::Unavailable(
        "disabled by the administrator".into(),
    ))));
    assert_eq!(restricted["availability_kind"], "restricted");
    let missing = source_row(&source(Ok(Availability::Unavailable(
        "not installed".into(),
    ))));
    assert_eq!(missing["availability_kind"], "unavailable");
    assert_eq!(restricted["available"], false);
    for (error, kind) in [
        (
            EngineError::Unsupported {
                backend: "flatpak".into(),
                capability: Capability::Search,
            },
            "unsupported",
        ),
        (
            EngineError::Execution(ExecutionError::AuthorizationDenied),
            "authorization",
        ),
        (
            EngineError::Execution(ExecutionError::AuthorizationCancelled),
            "authorization",
        ),
        (EngineError::Execution(ExecutionError::LockBusy), "locked"),
    ] {
        assert_eq!(failure_kind(&error), kind);
        let row = source_row(&source(Err(error)));
        assert_eq!(row["availability_kind"], kind);
        assert_eq!(row["check_failed"], true);
    }
    assert_eq!(
        plain_text("host command failed (exit 1):", None),
        "The package manager reported an error without details."
    );
    assert_eq!(
        plain_text("host command failed (exit 1):", Some("snap")),
        "Snap reported an error without details."
    );
    assert_eq!(
        plain_text("host command failed (exit 1): E: broken index", None),
        "Broken index."
    );
}
#[test]
fn removal_confirmation_lists_the_selected_and_other_native_changes() {
    let change = |action, name: &str, old: Option<&str>, new: Option<&str>| PlannedChange {
        action,
        name: name.into(),
        installed_version: old.map(Into::into),
        candidate_version: new.map(Into::into),
    };
    let mut editor = synthetic_package("editor", "");
    editor.candidate_version = Some("2".into());
    let remove = Operation::Remove(editor.id.clone());
    let plan = TransactionPlan {
        operation: remove.clone(),
        native_preview: String::new(),
        changes: vec![
            change(PlannedAction::Remove, "editor", Some("1"), Some("2")),
            change(PlannedAction::Install, "libnew", None, Some("3")),
            change(PlannedAction::Upgrade, "libsame", Some("4"), Some("4")),
            change(PlannedAction::Remove, "libgone", None, None),
        ],
        download_bytes: None,
        disk_bytes: None,
        restart_required: None,
        adopts: None,
    };
    let preview = confirmation_preview(&remove, &[editor.clone()], &[], Some(&plan));
    assert_eq!(preview["action"], "Remove editor");
    assert_eq!(
            preview["summary"],
            "Remove editor\nAPT, System\n1\n3 other packages will change\nRemoves: libgone\nApp data may remain after removal."
        );
    let body = preview["body"].as_str().unwrap();
    assert!(body.contains("Selected: Remove editor (1)"), "{body}");
    assert!(
        body.contains("Other changes:\nInstall libnew (3)\nUpdate libsame (4)\nRemove libgone"),
        "{body}"
    );
    // A source-wide change has no selected package among the plan.
    let clean = Operation::Clean(CleanupId {
        backend: "apt".into(),
        key: "autoremove".into(),
    });
    let preview = confirmation_preview(&clean, &[editor.clone()], &[], Some(&plan));
    assert!(preview["summary"]
        .as_str()
        .unwrap()
        .contains("4 other packages will change"));
    assert!(!preview["body"].as_str().unwrap().contains("Selected:"));
    // A Flatpak reference only offers a system or user scope.
    let mut reference = editor;
    reference.id.backend = "flatpak".into();
    reference.id.reference = Some("flatpakref:https://example.invalid/app.flatpakref".into());
    reference.id.scope = Scope::Environment {
        path: "/opt/flatpak".into(),
    };
    let install = Operation::Install(reference.id.clone());
    let preview = confirmation_preview(&install, &[reference], &[], None);
    assert_eq!(preview["flatpak_ref_scope"], "");
    assert!(preview["body"]
        .as_str()
        .unwrap()
        .contains("Location: /opt/flatpak"));
}
#[test]
fn activity_reads_report_an_unreadable_lock() {
    let temp = pkgdeck_tools::Temp::new();
    let file = temp.0.join("not-a-directory");
    std::fs::write(&file, "").unwrap();
    let error = read_activity(&file.join("activity.json")).unwrap_err();
    assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(read_activity(&temp.0.join("activity.json"))
        .unwrap()
        .is_empty());
}

#[test]
fn preempting_read_reports_busy_until_fresh_rows_land() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let (_sender, receiver) = mpsc::channel();
    controller.as_mut().rust_mut().background = true;
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: Cancellation::default(),
        job: Job::Load("Installed".into(), "".into()),
    });
    controller
        .as_mut()
        .load("Search".into(), "fixture".into(), "".into(), false, true);
    // The new view cancels the in-flight load and waits behind it; the
    // table is empty until it runs, so idle must not be reported.
    assert!(matches!(
        &controller.rust().queued,
        Some(Job::Load(view, query)) if view == "Search" && query == "fixture"
    ));
    assert!(*controller.busy());
}

#[test]
fn superseded_search_drops_late_successful_payloads() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().set_rows("new-query-pending".into());
    controller.as_mut().rust_mut().queued = Some(Job::Load("Search".into(), "new-query".into()));
    controller
        .as_mut()
        .apply(Ok(Payload::Packages(PackageReport::default())));
    assert_eq!(controller.rows().to_string(), "new-query-pending");
    controller.as_mut().apply(Ok(Payload::Sources(vec![])));
    assert_eq!(controller.rows().to_string(), "new-query-pending");
}

#[test]
fn sections_preload_from_startup_in_order() {
    let mut controller = Controller::default();
    assert_eq!(controller.active_view, "Search");
    let order: Vec<_> = std::iter::from_fn(|| controller.next_prefetch()).collect();
    assert_eq!(order, ["Installed", "Updates", "Clean", "Sources"]);
    assert!(controller.prefetch.is_empty());
    controller.invalidate_prefetch();
    assert_eq!(controller.prefetch.len(), 4);
}
fn synthetic_package(name: &str, display_name: &str) -> Package {
    Package {
        id: PackageId {
            backend: "apt".into(),
            name: name.into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        },
        display_name: display_name.into(),
        summary: "Synthetic package".into(),
        installed_version: Some("1".into()),
        candidate_version: Some("1".into()),
        update: UpdateAvailability::Unknown,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
        adopt_with: None,
    }
}
fn wait_until(
    controller: &mut Pin<&mut ffi::PackageController>,
    done: impl Fn(&ffi::PackageController) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !done(controller) {
        controller.as_mut().poll();
        thread::sleep(Duration::from_millis(5));
    }
    assert!(done(controller));
}
fn fake_prefetch(view: &str, key: String, replies: Vec<Reply>, stale: bool) -> PrefetchWorker {
    let (sender, receiver) = mpsc::channel();
    for reply in replies {
        sender.send(reply).unwrap();
    }
    PrefetchWorker {
        handle: thread::spawn(move || drop(sender)),
        receiver,
        cancel: Cancellation::default(),
        view: view.into(),
        key,
        stale,
        asked: vec![],
        answered: vec![],
        scope: (vec![], false, Instant::now()),
    }
}
#[test]
fn finished_changes_explain_what_happened() {
    use pkgdeck_core::process::{Completion, ExecutionError as E};
    let install = Operation::Install(synthetic_package("htop", "htop").id);
    let job = Job::Write(install.clone(), None);
    assert_eq!(operation_title(&install), "Install htop (APT)");
    assert_eq!(
        operation_title(&Operation::Refresh {
            backend: "apt".into()
        }),
        "Refresh APT package lists"
    );
    let success = write_notice(
        &job,
        &Ok(Payload::Written(OperationOutcome::default())),
        false,
        &Names::new(),
    );
    assert_eq!(
        success,
        json!({"kind": "success", "title": "Install htop (APT) finished",
                "operation": "install", "package": "htop", "source": "apt", "source_name": "APT",
                "undo": true, "undo_action": "remove",
                "target": {"source": "apt", "name": "htop", "architecture": "all",
                    "remote": null, "scope": "system", "reference": null}})
    );
    // A remembered display name is what people see.
    let names = Names::from([(
        synthetic_package("htop", "htop").id,
        "Process Viewer".to_owned(),
    )]);
    let named = write_notice(
        &job,
        &Ok(Payload::Written(OperationOutcome::default())),
        false,
        &names,
    );
    assert_eq!(named["title"], "Install Process Viewer (APT) finished");
    assert_eq!(named["package"], "Process Viewer");
    // Undo is offered only for a finished install or removal.
    let failed = write_notice(&job, &Err(EngineError::NotFound), false, &names);
    assert_eq!(failed["undo"], false);
    let removal = write_notice(
        &Job::Write(
            Operation::Remove(synthetic_package("htop", "htop").id),
            None,
        ),
        &Ok(Payload::Written(OperationOutcome::default())),
        false,
        &names,
    );
    assert_eq!(removal["undo_action"], "install");
    let update = write_notice(
        &Job::Write(
            Operation::Upgrade(synthetic_package("htop", "htop").id),
            None,
        ),
        &Ok(Payload::Written(OperationOutcome::default())),
        false,
        &names,
    );
    assert_eq!(update["operation"], "update");
    assert_eq!(update["undo"], false);
    let denied = write_notice(
        &job,
        &Err(E::AuthorizationDenied.into()),
        false,
        &Names::new(),
    );
    assert_eq!(denied["kind"], "error");
    assert_eq!(denied["title"], "Install htop (APT) failed");
    assert!(denied["detail"]
        .as_str()
        .unwrap()
        .contains("password prompt"));
    let sudo = write_notice(
        &job,
        &Err(E::AuthorizationDenied.into()),
        true,
        &Names::new(),
    );
    assert!(sudo["detail"].as_str().unwrap().contains("sudo login"));
    for (error, expected) in [
        (E::AuthorizationCancelled, "prompt was closed"),
        (E::LockBusy, "Another package manager is running"),
        (E::Interrupted, "interrupted"),
        (
            E::Failed(Completion {
                code: Some(100),
                signal: None,
                stdout: vec![],
                stderr: b"E: one\n\nE: two\n".to_vec(),
                truncated: false,
                cancellation_deferred: false,
            }),
            "APT couldn't run: one.",
        ),
        (
            E::Failed(Completion {
                code: Some(1),
                signal: None,
                stdout: vec![],
                stderr: vec![],
                truncated: false,
                cancellation_deferred: false,
            }),
            "APT reported an error without details.",
        ),
        (E::TimedOut, "took too long"),
    ] {
        let notice = write_notice(&job, &Err(error.into()), false, &Names::new());
        assert!(
            notice["detail"].as_str().unwrap().contains(expected),
            "{notice}"
        );
    }
    let cancelled = write_notice(&job, &Err(EngineError::Cancelled), false, &Names::new());
    assert_eq!(cancelled["kind"], "info");
    let batch = Job::UpgradeAll(
        vec![
            Operation::UpgradeAll {
                backend: "apt".into(),
            },
            Operation::UpgradeAll {
                backend: "flatpak".into(),
            },
        ],
        None,
    );
    let partial = write_notice(
        &batch,
        &Ok(ran(&[
            (
                "Update all APT packages",
                Outcome::Finished,
                Said::Completed,
            ),
            (
                "Update all Flatpak packages",
                Outcome::Failed,
                because("busy"),
            ),
        ])),
        false,
        &Names::new(),
    );
    // One failure names the source, without the step's long title.
    assert_eq!(partial["title"], "Flatpak didn't update");
    assert_eq!(partial["detail"], "Flatpak: busy");
    let cancelled_batch = write_notice(
        &batch,
        &Ok(ran(&[
            ("APT", Outcome::Finished, Said::Completed),
            ("Flatpak", Outcome::Cancelled, because("Cancelled.")),
        ])),
        false,
        &Names::new(),
    );
    assert_eq!(cancelled_batch["kind"], "info");
    assert_eq!(
        cancelled_batch["title"],
        "1 of 2 changes finished. The rest were cancelled."
    );
    let mixed = write_notice(
        &batch,
        &Ok(ran(&[
            ("APT", Outcome::Failed, because("busy")),
            ("Flatpak", Outcome::Cancelled, because("Cancelled.")),
        ])),
        false,
        &Names::new(),
    );
    assert_eq!(mixed["kind"], "error");
    let denied_batch = write_notice(
        &batch,
        &Ok(ran(&[
            ("APT", Outcome::Failed, Said::Denied),
            ("Flatpak", Outcome::Finished, Said::Completed),
        ])),
        false,
        &Names::new(),
    );
    assert_eq!(denied_batch["kind"], "error");
    assert!(denied_batch["detail"]
        .as_str()
        .unwrap()
        .contains("password prompt"));
    let denied_sudo = write_notice(
        &batch,
        &Ok(ran(&[("APT", Outcome::Failed, Said::Denied)])),
        true,
        &Names::new(),
    );
    assert!(denied_sudo["detail"]
        .as_str()
        .unwrap()
        .contains("sudo login"));
    // A failed preview says so; a superseded one stays quiet.
    let plan = Job::PlanOperation(install.clone());
    let prepared = preflight_notice(&plan, &E::LockBusy.into(), false, &Names::new()).unwrap();
    assert_eq!(prepared["title"], "Install htop (APT) couldn't be prepared");
    let denied_plan = preflight_notice(
        &Job::PlanUpgrade(vec![], 0),
        &E::AuthorizationDenied.into(),
        false,
        &Names::new(),
    )
    .unwrap();
    assert_eq!(denied_plan["title"], "The update couldn't be prepared");
    assert_eq!(
        preflight_notice(
            &Job::PlanCleanAll(vec![]),
            &E::TimedOut.into(),
            false,
            &Names::new()
        )
        .unwrap()["title"],
        "The cleanup couldn't be prepared"
    );
    assert!(preflight_notice(&plan, &EngineError::Cancelled, false, &Names::new()).is_none());
    assert!(preflight_notice(&job, &E::LockBusy.into(), false, &Names::new()).is_none());
    let all = write_notice(
        &batch,
        &Ok(ran(&[
            ("APT", Outcome::Finished, Said::Completed),
            ("Flatpak", Outcome::Finished, Said::Completed),
        ])),
        false,
        &Names::new(),
    );
    assert_eq!(all["title"], "2 changes finished");
    let firmware = write_notice(
        &job,
        &Ok(Payload::Batch(Batch {
            noun: "updates",
            steps: vec![Ended {
                title: "Update firmware".into(),
                outcome: Outcome::Finished,
                said: Said::Completed,
                kept: None,
            }],
            note: firmware_note(&[Operation::Upgrade(PackageId {
                backend: "fwupd".into(),
                ..synthetic_package("bios", "BIOS").id
            })]),
        })),
        false,
        &Names::new(),
    );
    assert_eq!(firmware["kind"], "success");
    assert!(firmware["detail"]
        .as_str()
        .unwrap()
        .contains("Restart or shut down"));
    let batch_done = write_notice(
        &batch,
        &Ok(ran(&[
            (
                "Update all packages from apt",
                Outcome::Finished,
                Said::Completed,
            ),
            (
                "Update all packages from flatpak",
                Outcome::Finished,
                Said::KeptAfterCancel,
            ),
        ])),
        false,
        &Names::new(),
    );
    assert_eq!(
            batch_done["detail"],
            "Update all packages from flatpak: Finished before it could be cancelled. Changes were kept."
        );
    let deferred = write_notice(
        &job,
        &Ok(Payload::Written(OperationOutcome {
            cancellation_deferred: true,
        })),
        false,
        &Names::new(),
    );
    assert!(deferred["detail"]
        .as_str()
        .unwrap()
        .contains("before it could be cancelled"));
    let repository = write_notice(
        &Job::Repositories(None),
        &Ok(Payload::Written(OperationOutcome::default())),
        false,
        &Names::new(),
    );
    assert_eq!(repository["title"], "The change finished");
    // The banner clears on request.
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().set_notice(encoded(denied));
    controller.as_mut().dismiss_notice();
    assert_eq!(controller.notice().to_string(), "{}");
}
#[test]
fn opening_a_package_never_makes_the_page_busy() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    controller.as_mut().rust_mut().source_filter = vec!["grok".into()];
    let mut package = synthetic_package("grok", "Grok");
    package.id.backend = "grok".into();
    controller.as_mut().rust_mut().packages = vec![package];
    controller.as_mut().select(0);
    // Details load beside the page: actions stay enabled meanwhile.
    assert!(controller.details().to_string().contains("Grok"));
    assert!(controller.rust().details_worker.is_some());
    assert!(controller.rust().worker.is_none());
    assert!(!*controller.busy());
}
#[test]
fn selecting_during_a_search_loads_details_alongside_it() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    // A standalone tool keeps both jobs cheap and read-only.
    controller.as_mut().rust_mut().source_filter = vec!["grok".into()];
    let mut package = synthetic_package("grok", "Grok");
    package.id.backend = "grok".into();
    controller.as_mut().rust_mut().packages = vec![package.clone()];
    controller
        .as_mut()
        .start(Job::Load("Search".into(), "zzz".into()));
    controller.as_mut().select(0);
    // The row shows at once; full details load on their own worker.
    assert!(controller.details().to_string().contains("Grok"));
    assert!(controller.rust().details_worker.is_some());
    assert!(controller.rust().queued.is_none());
    // Choosing that row again waits for the lookup already running.
    let cancel = controller
        .rust()
        .details_worker
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    controller.as_mut().select(0);
    assert!(!cancel.requested());
    assert!(controller
        .rust()
        .worker
        .as_ref()
        .is_some_and(|worker| { matches!(worker.job, Job::Load(..)) }));
    assert!(controller.rust().queued.is_none());
    // A reply that arrives while the next search is queued is kept, so
    // opening the package again does not query the manager a second time.
    controller.as_mut().rust_mut().queued = Some(Job::Load("Search".into(), "next".into()));
    controller.as_mut().set_details("{}".into());
    controller
        .as_mut()
        .apply(Ok(Payload::Details(Box::new(PackageDetails {
            package: package.clone(),
            description: "Full Grok details".into(),
            homepage: None,
            dependencies: vec![],
        }))));
    assert_eq!(controller.details().to_string(), "{}");
    assert!(controller
        .rust()
        .detail_cache
        .get(&package.id)
        .is_some_and(|cached| cached.to_string().contains("Full Grok details")));
    controller.as_mut().rust_mut().queued = None;
    controller.as_mut().select(0);
    assert!(controller
        .details()
        .to_string()
        .contains("Full Grok details"));
    // A reload drops the lookup so its reply can't refill the cache.
    controller.as_mut().rust_mut().invalidate_details();
    assert!(controller.rust().details_worker.is_none());
    // Selecting again replaces the pending lookup.
    controller.as_mut().select(0);
    wait_until(&mut controller, |c| {
        c.rust().details_worker.is_none() && c.rust().worker.is_none()
    });
    assert_eq!(controller.rust().selected.as_ref(), Some(&package.id));
}
#[test]
fn cancel_stops_the_parallel_details_lookup() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let package = synthetic_package("fixture", "Fixture");
    controller.as_mut().rust_mut().packages = vec![package.clone()];
    controller.as_mut().rust_mut().selected = Some(package.id.clone());
    let (sender, receiver) = mpsc::channel();
    let cancel = Cancellation::default();
    sender
        .send(Reply::DetailsPreview(Box::new(PackageDetails {
            package: package.clone(),
            description: "should not land".into(),
            homepage: None,
            dependencies: vec![],
        })))
        .unwrap();
    controller.as_mut().rust_mut().details_worker = Some(DetailsWorker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: cancel.clone(),
        id: package.id.clone(),
    });
    controller.as_mut().cancel();
    assert!(cancel.requested());
    assert!(controller.rust().details_worker.is_none());
    controller.as_mut().poll();
    assert!(controller.rust().detail_cache.is_empty());
    assert!(!controller.details().to_string().contains("should not land"));
    assert!(controller.status().to_string().contains("Cancelling"));
    // The channel was dropped with the worker, so nothing further can land.
    assert!(sender
        .send(Reply::Done(Ok(Payload::Details(Box::new(
            PackageDetails {
                package,
                description: "still should not land".into(),
                homepage: None,
                dependencies: vec![],
            }
        )))))
        .is_err());
}
#[test]
fn resting_on_a_row_loads_its_details_before_it_opens() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let first = fixture_row();
    let mut second = fixture_row();
    second.id.name = "other".into();
    controller.as_mut().rust_mut().packages = vec![first.clone(), second.clone()];
    let loading = |controller: &ffi::PackageController| {
        controller
            .rust()
            .details_worker
            .as_ref()
            .map(|worker| worker.id.clone())
    };
    controller.as_mut().warm_details(-1);
    assert_eq!(loading(&controller), None);
    controller.as_mut().warm_details(0);
    assert_eq!(loading(&controller), Some(first.id.clone()));
    controller.as_mut().warm_details(0);
    assert_eq!(loading(&controller), Some(first.id.clone()));
    // Moving on to another row looks that one up instead.
    controller.as_mut().warm_details(1);
    assert_eq!(loading(&controller), Some(second.id.clone()));
    // Opening it shows what the row says while the same lookup finishes.
    controller.as_mut().select(1);
    assert_eq!(loading(&controller), Some(second.id.clone()));
    let details: Value = serde_json::from_str(&controller.details().to_string()).unwrap();
    assert_eq!(details["more"], true);
    // The open package's lookup is never replaced.
    controller.as_mut().warm_details(0);
    assert_eq!(loading(&controller), Some(second.id.clone()));
    let started = Instant::now();
    while controller.rust().details_worker.is_some() {
        assert!(started.elapsed() < Duration::from_secs(10));
        controller.as_mut().poll();
        thread::sleep(Duration::from_millis(5));
    }
    let details: Value = serde_json::from_str(&controller.details().to_string()).unwrap();
    assert_ne!(details["more"], true);
    // Known, open, or behind a change: nothing to look up.
    controller
        .as_mut()
        .rust_mut()
        .detail_cache
        .insert(second.id.clone(), "{}".into());
    controller.as_mut().rust_mut().selected = None;
    controller.as_mut().warm_details(1);
    assert_eq!(loading(&controller), None);
    controller.as_mut().rust_mut().detail_cache.clear();
    controller.as_mut().rust_mut().selected = Some(second.id.clone());
    controller.as_mut().warm_details(1);
    assert_eq!(loading(&controller), None);
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Write(Operation::Install(first.id.clone()), None),
        vec![],
    ));
    controller.as_mut().warm_details(0);
    assert_eq!(loading(&controller), None);
    controller.as_mut().rust_mut().worker = None;
}
#[test]
fn parallel_details_failure_replaces_the_row_preview() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let package = synthetic_package("fixture", "Fixture");
    controller.as_mut().rust_mut().packages = vec![package.clone()];
    controller.as_mut().rust_mut().selected = Some(package.id.clone());
    controller.as_mut().set_details(encoded(json!({
        "package": package_row(&package, &[], None),
        "description": package.summary,
    })));
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Reply::Done(Err(EngineError::Unavailable {
            backend: "homebrew".into(),
            reason: "brew is not installed".into(),
        })))
        .unwrap();
    controller.as_mut().rust_mut().details_worker = Some(DetailsWorker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: Cancellation::default(),
        id: package.id.clone(),
    });
    controller.as_mut().poll();
    let details: Value = serde_json::from_str(&controller.details().to_string()).unwrap();
    assert!(details["description"]
        .as_str()
        .unwrap()
        .contains("Details couldn't be loaded"));
    assert!(details["description"]
        .as_str()
        .unwrap()
        .contains("brew is not installed"));
    assert_eq!(details["package"]["name"], "fixture");
    assert!(controller.rust().detail_cache.is_empty());
    // A cancelled lookup stays quiet and leaves the preview in place.
    controller.as_mut().set_details("{}".into());
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Reply::Done(Err(EngineError::Cancelled)))
        .unwrap();
    controller.as_mut().rust_mut().details_worker = Some(DetailsWorker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: Cancellation::default(),
        id: package.id.clone(),
    });
    controller.as_mut().poll();
    assert_eq!(controller.details().to_string(), "{}");
    // A failure for a package that left the list has no row to explain.
    controller.as_mut().rust_mut().packages.clear();
    let (sender, receiver) = mpsc::channel();
    sender
        .send(Reply::Done(Err(EngineError::NotFound)))
        .unwrap();
    controller.as_mut().rust_mut().details_worker = Some(DetailsWorker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: Cancellation::default(),
        id: package.id,
    });
    let status = controller.status().to_string();
    controller.as_mut().poll();
    assert_eq!(controller.details().to_string(), "{}");
    assert_eq!(controller.status().to_string(), status);
}
#[test]
fn finished_preloads_fill_sections_and_the_source_catalog() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    // A pending review blocks new preloads, so only these fakes run.
    controller.as_mut().rust_mut().pending = Some(Job::Load("Search".into(), String::new()));
    let mut inventory = PackageReport::default();
    inventory.packages.push(synthetic_package("warm", "warm"));
    let sources = vec![Source {
        backend: "apt".into(),
        capabilities: vec![Capability::Search],
        availability: Ok(Availability::Available),
    }];
    let key = cache_key("Sources", "", &[], false);
    controller.as_mut().rust_mut().prefetch_worker = Some(fake_prefetch(
        "Sources",
        key.clone(),
        vec![
            Reply::Inventory(inventory, true),
            Reply::Done(Ok(Payload::Sources(sources))),
        ],
        false,
    ));
    wait_until(&mut controller, |c| c.rust().prefetch_worker.is_none());
    assert!(controller.rust().prefetched.contains_key(&key));
    assert!(controller
        .rust()
        .prefetched
        .contains_key(&cache_key("Updates", "", &[], false)));
    assert!(controller.rust().catalog_checked);
    assert!(controller.source_catalog().to_string().contains("apt"));
    // A preload made stale by a write or filter change is dropped.
    controller.as_mut().rust_mut().prefetched.clear();
    let key = cache_key("Clean", "", &[], false);
    controller.as_mut().rust_mut().prefetch_worker = Some(fake_prefetch(
        "Clean",
        key.clone(),
        vec![Reply::Done(Ok(Payload::Cleanup(CleanupReport::default())))],
        true,
    ));
    wait_until(&mut controller, |c| c.rust().prefetch_worker.is_none());
    assert!(controller.rust().prefetched.is_empty());
}
#[test]
fn open_sections_are_reloaded_when_their_preload_gets_old() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().pending = Some(Job::Load("Search".into(), String::new()));
    controller.as_mut().rust_mut().prefetch.clear();
    let fresh = cached_view("fresh");
    let mut old = cached_view("old");
    old.loaded = Instant::now() - REWARM_AFTER;
    controller
        .as_mut()
        .rust_mut()
        .view_cache
        .insert(cache_key("Installed", "", &[], false), old);
    controller
        .as_mut()
        .rust_mut()
        .view_cache
        .insert(cache_key("Clean", "", &[], false), fresh);
    controller.as_mut().rust_mut().last_rewarm = Instant::now() - Duration::from_secs(60);
    controller.as_mut().poll();
    // Installed (old), Updates and Sources (never loaded) reload,
    // Installed first because it also fills Updates.
    let mut order = controller.rust().prefetch.clone();
    order.reverse();
    assert_eq!(order, ["Installed", "Updates", "Sources"]);
    // Checked at most every 30 seconds.
    controller.as_mut().rust_mut().prefetch.clear();
    controller.as_mut().poll();
    assert!(controller.rust().prefetch.is_empty());
}
#[test]
fn updates_refresh_in_the_background_only_as_often_as_checks_run() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().pending = Some(Job::Load("Search".into(), String::new()));
    controller.as_mut().set_check_interval(60);
    let key = cache_key("Updates", "", &[], false);
    for view in ["Installed", "Clean", "Sources"] {
        controller
            .as_mut()
            .rust_mut()
            .view_cache
            .insert(cache_key(view, "", &[], false), cached_view("fresh"));
    }
    // Older than the usual rewarm, younger than the hourly check.
    for (age, due) in [(REWARM_AFTER, false), (Duration::from_secs(60 * 60), true)] {
        let mut updates = cached_view("updates");
        updates.loaded = Instant::now() - age;
        controller
            .as_mut()
            .rust_mut()
            .view_cache
            .insert(key.clone(), updates);
        controller.as_mut().rust_mut().prefetch.clear();
        controller.as_mut().rust_mut().last_rewarm = Instant::now() - Duration::from_secs(60);
        controller.as_mut().poll();
        assert_eq!(
            controller.rust().prefetch,
            if due {
                vec!["Updates".to_owned()]
            } else {
                vec![]
            },
            "{age:?}"
        );
    }
}
#[test]
fn preloads_start_on_their_own_worker_and_report_back() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    // A standalone tool keeps detection cheap and read-only.
    controller.as_mut().rust_mut().source_filter = vec!["grok".into()];
    controller.as_mut().rust_mut().prefetch = vec!["Clean".into()];
    controller.as_mut().poll();
    assert!(controller.rust().prefetch_worker.is_some());
    assert!(controller.rust().worker.is_none());
    wait_until(&mut controller, |c| c.rust().prefetch_worker.is_none());
    assert!(
        controller
            .rust()
            .prefetched
            .contains_key(&cache_key("Clean", "", &["grok".into()], false))
            || controller.rust().prefetch_worker.is_none()
    );
}
#[test]
fn searches_reuse_a_recent_engine_and_keep_its_detection_time() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    let job = Job::Load("Search".into(), "anything".into());
    let born = Instant::now() - Duration::from_secs(5);
    let scope = engine_source(&job, &[]);
    controller.as_mut().rust_mut().engine = Some(Engine::default());
    controller.as_mut().rust_mut().engine_scope = Some((scope.clone(), false, born));
    controller.as_mut().start(job);
    wait_until(&mut controller, |c| c.rust().worker.is_none());
    let (sources, sudo, kept) = controller.rust().engine_scope.clone().unwrap();
    assert_eq!((sources, sudo, kept), (scope, false, born));
    assert!(controller.rust().engine.is_some());
    // Changing the sources forgets it.
    controller.as_mut().rust_mut().view_cache.insert(
        cache_key("Installed", "", &["apt".into()], false),
        cached_view("apt only"),
    );
    controller
        .as_mut()
        .load("Installed".into(), "".into(), "apt".into(), false, false);
    assert!(controller.rust().engine_scope.is_none());
    assert!(controller.rows().to_string().contains("apt only"));
}
#[test]
fn rows_stay_actionable_only_while_rows_load() {
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    let worker = |job| Worker {
        handle: thread::spawn(|| {}),
        receiver: mpsc::channel().1,
        cancel: Cancellation::default(),
        job,
    };
    let refresh = Operation::Refresh {
        backend: "apt".into(),
    };
    assert!(!controller.rust().only_reading());
    controller.as_mut().rust_mut().worker = Some(worker(Job::Load("Search".into(), "x".into())));
    controller.as_mut().sync_needs_poll();
    assert!(*controller.reading());
    // A change being prepared or queued blocks rows again.
    controller.as_mut().rust_mut().queued = Some(Job::PlanOperation(refresh.clone()));
    assert!(!controller.rust().only_reading());
    controller.as_mut().rust_mut().queued = None;
    controller.as_mut().rust_mut().worker = Some(worker(Job::Write(refresh, None)));
    controller.as_mut().sync_needs_poll();
    assert!(!*controller.reading());
    controller.as_mut().rust_mut().worker = None;
}
#[test]
fn preloads_reuse_one_checked_engine_and_pass_it_on() {
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    let scope = engine_source(&Job::Load("Installed".into(), String::new()), &[]);
    let born = Instant::now() - Duration::from_secs(5);
    controller.as_mut().rust_mut().engine = Some(Engine::default());
    controller.as_mut().rust_mut().engine_scope = Some((scope.clone(), false, born));
    controller
        .as_mut()
        .start_prefetch("Installed".into(), cache_key("Installed", "", &[], false));
    assert!(controller.rust().engine.is_none());
    wait_until(&mut controller, |c| c.rust().prefetch_worker.is_none());
    // Handed back with its first check time, for the next preload or search.
    assert_eq!(
        controller.rust().engine_scope,
        Some((scope.clone(), false, born))
    );
    assert!(controller.rust().engine.is_some());
    // An engine checked over a minute ago is checked again.
    let old = Instant::now() - VIEW_TTL;
    controller.as_mut().rust_mut().engine_scope = Some((scope.clone(), false, old));
    controller
        .as_mut()
        .start_prefetch("Updates".into(), cache_key("Updates", "", &[], false));
    wait_until(&mut controller, |c| c.rust().prefetch_worker.is_none());
    let (_, _, checked) = controller.rust().engine_scope.clone().unwrap();
    assert!(checked > old);
    // A preload made stale by a source change keeps nothing.
    controller.as_mut().rust_mut().engine = None;
    controller.as_mut().rust_mut().engine_scope = None;
    controller.as_mut().rust_mut().prefetch_worker = Some(fake_prefetch(
        "Clean",
        cache_key("Clean", "", &[], false),
        vec![Reply::Engine(Box::default())],
        true,
    ));
    wait_until(&mut controller, |c| c.rust().prefetch_worker.is_none());
    assert!(controller.rust().engine.is_none());
}
#[test]
fn a_better_name_from_details_updates_cached_sections_too() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let package = synthetic_package("org.example.app", "org.example.app");
    let rows = encoded(vec![
        json!({"name": "org.example.app", "display_name": "org.example.app"}),
    ]);
    controller.as_mut().rust_mut().packages = vec![package.clone()];
    controller.as_mut().set_rows(rows.clone());
    let mut cached = cached_view("unused");
    cached.packages = vec![package.clone()];
    cached.rows = rows;
    controller
        .as_mut()
        .rust_mut()
        .view_cache
        .insert(cache_key("Installed", "", &[], false), cached);
    let mut named = package;
    named.display_name = "Example App".into();
    controller
        .as_mut()
        .apply(Ok(Payload::Details(Box::new(PackageDetails {
            package: named,
            description: String::new(),
            homepage: None,
            dependencies: vec![],
        }))));
    assert!(controller.rows().to_string().contains("Example App"));
    let cached = controller
        .rust()
        .view_cache
        .get(&cache_key("Installed", "", &[], false))
        .unwrap();
    assert!(cached.rows.to_string().contains("Example App"));
    assert_eq!(cached.packages[0].display_name, "Example App");
}
#[test]
fn updates_wait_for_an_installed_preload_and_cancel_older_reads() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    let (release, gate) = mpsc::channel::<()>();
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let _ = gate.recv();
        let mut report = PackageReport::default();
        let mut package = synthetic_package("pending", "pending");
        package.update = UpdateAvailability::Available;
        package.candidate_version = Some("2".into());
        report.packages.push(package);
        report.successful_sources.push("apt".into());
        let _ = sender.send(Reply::Inventory(report.clone(), true));
        let _ = sender.send(Reply::Done(Ok(Payload::Packages(report))));
    });
    controller.as_mut().rust_mut().prefetch_worker = Some(PrefetchWorker {
        handle,
        receiver,
        cancel: Cancellation::default(),
        view: "Updates".into(),
        key: cache_key("Updates", "", &[], false),
        stale: false,
        asked: vec![],
        answered: vec![],
        scope: (vec![], false, Instant::now()),
    });
    // An older foreground read is cancelled and its reply ignored.
    controller
        .as_mut()
        .start(Job::Load("Search".into(), "zzz-no-such-package".into()));
    controller
        .as_mut()
        .load("Updates".into(), "".into(), "".into(), false, false);
    assert!(controller.rust().awaiting_prefetch);
    assert!(controller.rust().background);
    release.send(()).unwrap();
    wait_until(&mut controller, |c| !*c.busy() && c.rust().worker.is_none());
    assert!(controller.rows().to_string().contains("pending"));
}
#[test]
fn pending_sources_shrink_as_partials_arrive_and_clear_when_the_load_ends() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    let pending = |c: &ffi::PackageController| -> Vec<String> {
        serde_json::from_str(&c.pending_sources().to_string()).unwrap()
    };
    assert!(pending(&controller).is_empty());
    let (step, gate) = mpsc::channel::<()>();
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let asked = vec!["apt".to_string(), "gem".into(), "npm".into()];
        let _ = sender.send(Reply::Asked(asked));
        let _ = gate.recv();
        let mut report = PackageReport::default();
        report.packages.push(synthetic_package("fast", "fast"));
        report.successful_sources.push("apt".into());
        let _ = sender.send(Reply::Partial(report.clone()));
        let _ = gate.recv();
        report.failures.push(BackendFailure {
            backend: "npm".into(),
            error: EngineError::Cancelled,
        });
        let _ = sender.send(Reply::Partial(report.clone()));
        let _ = gate.recv();
        // Gem never answers, as a source skipped quietly would not.
        let _ = sender.send(Reply::Done(Ok(Payload::Packages(report))));
    });
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle,
        receiver,
        cancel: Cancellation::default(),
        job: Job::Load("Search".into(), "fast".into()),
    });
    controller.as_mut().set_busy(true);
    wait_until(&mut controller, |c| !c.rust().asked.is_empty());
    assert_eq!(pending(&controller), ["apt", "gem", "npm"]);
    step.send(()).unwrap();
    wait_until(&mut controller, |c| c.rust().asked.len() == 2);
    assert_eq!(pending(&controller), ["gem", "npm"]);
    assert!(controller.rows().to_string().contains("fast"));
    step.send(()).unwrap();
    wait_until(&mut controller, |c| c.rust().asked.len() == 1);
    assert_eq!(pending(&controller), ["gem"]);
    step.send(()).unwrap();
    wait_until(&mut controller, |c| c.rust().worker.is_none());
    assert!(pending(&controller).is_empty());
}
#[test]
fn pending_sources_clear_on_cancel_and_skip_background_loads() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    let pending = |c: &ffi::PackageController| c.pending_sources().to_string();
    let worker = |job: Job| {
        let (step, gate) = mpsc::channel::<()>();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let _ = sender.send(Reply::Asked(vec!["apt".into(), "npm".into()]));
            let _ = gate.recv();
            let mut report = PackageReport::default();
            report.successful_sources.push("apt".into());
            let _ = sender.send(Reply::Partial(report.clone()));
            let _ = gate.recv();
            let _ = sender.send(Reply::Done(Ok(Payload::Packages(report))));
        });
        let worker = Worker {
            handle,
            receiver,
            cancel: Cancellation::default(),
            job,
        };
        (worker, step)
    };
    // Cancelling stops showing what the load waited for, and a partial
    // that still arrives does not bring it back.
    let (running, step) = worker(Job::Load("Installed".into(), "".into()));
    controller.as_mut().rust_mut().worker = Some(running);
    wait_until(&mut controller, |c| !c.rust().asked.is_empty());
    assert_eq!(pending(&controller), r#"["apt","npm"]"#);
    controller.as_mut().cancel();
    assert_eq!(pending(&controller), "[]");
    step.send(()).unwrap();
    step.send(()).unwrap();
    wait_until(&mut controller, |c| c.rust().worker.is_none());
    assert_eq!(pending(&controller), "[]");
    // Another request replacing the load hides it at the next poll,
    // before the replaced load winds down.
    let (running, step) = worker(Job::Load("Search".into(), "vim".into()));
    controller.as_mut().rust_mut().worker = Some(running);
    wait_until(&mut controller, |c| !c.rust().asked.is_empty());
    controller.rust().worker.as_ref().unwrap().cancel.cancel();
    controller.as_mut().rust_mut().background = true;
    controller.as_mut().poll();
    assert_eq!(pending(&controller), "[]");
    assert!(controller.rust().worker.is_some());
    step.send(()).unwrap();
    step.send(()).unwrap();
    wait_until(&mut controller, |c| c.rust().worker.is_none());
    assert_eq!(pending(&controller), "[]");
    // A section reloading out of sight never lists its sources.
    let (running, step) = worker(Job::Load("Updates".into(), "".into()));
    controller.as_mut().rust_mut().worker = Some(running);
    controller.as_mut().rust_mut().background = true;
    step.send(()).unwrap();
    step.send(()).unwrap();
    wait_until(&mut controller, |c| c.rust().worker.is_none());
    assert_eq!(pending(&controller), "[]");
    assert!(controller.rust().asked.is_empty());
}
#[test]
fn a_section_waiting_on_its_preload_names_the_sources_still_loading() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    let pending = |c: &ffi::PackageController| c.pending_sources().to_string();
    let (step, gate) = mpsc::channel::<()>();
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let _ = sender.send(Reply::Asked(vec!["apt".into(), "brew".into()]));
        let _ = gate.recv();
        let _ = sender.send(Reply::Answered(vec!["brew".into()]));
        let _ = gate.recv();
        let mut report = PackageReport::default();
        report.successful_sources.push("brew".into());
        let _ = sender.send(Reply::Done(Ok(Payload::Packages(report))));
    });
    controller.as_mut().rust_mut().prefetch_worker = Some(PrefetchWorker {
        handle,
        receiver,
        cancel: Cancellation::default(),
        view: "Updates".into(),
        key: cache_key("Updates", "", &[], false),
        stale: false,
        asked: vec![],
        answered: vec![],
        scope: (vec![], false, Instant::now()),
    });
    // A preload nobody waits for stays quiet.
    wait_until(&mut controller, |c| {
        c.rust().prefetch_worker.as_ref().unwrap().asked.len() == 2
    });
    assert_eq!(pending(&controller), "[]");
    controller
        .as_mut()
        .load("Updates".into(), "".into(), "".into(), false, false);
    assert!(controller.rust().awaiting_prefetch);
    assert_eq!(pending(&controller), r#"["apt","brew"]"#);
    step.send(()).unwrap();
    wait_until(&mut controller, |c| {
        c.pending_sources().to_string() == r#"["apt"]"#
    });
    // Leaving the section stops naming them; coming back resumes.
    controller
        .as_mut()
        .load("Search".into(), "".into(), "".into(), false, false);
    assert_eq!(pending(&controller), "[]");
    controller
        .as_mut()
        .load("Updates".into(), "".into(), "".into(), false, false);
    assert_eq!(pending(&controller), r#"["apt"]"#);
    step.send(()).unwrap();
    wait_until(&mut controller, |c| c.rust().prefetch_worker.is_none());
    assert!(!controller.rust().awaiting_prefetch);
    assert_eq!(pending(&controller), "[]");
}
#[test]
fn a_stale_preload_names_nothing() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    let key = cache_key("Installed", "", &[], false);
    let mut worker = fake_prefetch("Installed", key, vec![], false);
    worker.asked = vec!["apt".into()];
    controller.as_mut().rust_mut().prefetch_worker = Some(worker);
    controller.as_mut().rust_mut().active_view = "Installed".into();
    controller.as_mut().rust_mut().awaiting_prefetch = true;
    controller.as_mut().show_pending();
    assert_eq!(controller.pending_sources().to_string(), r#"["apt"]"#);
    controller.as_mut().rust_mut().invalidate_prefetch();
    controller.as_mut().rust_mut().awaiting_prefetch = true;
    controller.as_mut().show_pending();
    assert_eq!(controller.pending_sources().to_string(), "[]");
}
#[test]
fn preloads_report_which_sources_answered_instead_of_rows() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .start_prefetch("Installed".into(), cache_key("Installed", "", &[], false));
    let worker = controller
        .as_mut()
        .rust_mut()
        .prefetch_worker
        .take()
        .unwrap();
    worker.handle.join().unwrap();
    let replies: Vec<_> = worker.receiver.try_iter().collect();
    assert!(matches!(&replies[0], Reply::Asked(asked) if asked == &["fixture"]));
    assert!(matches!(&replies[1], Reply::Answered(answered) if answered == &["fixture"]));
    assert!(!replies.iter().any(|r| matches!(r, Reply::Partial(_))));
}
#[test]
fn retrying_a_source_in_a_section_ignores_search_text() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    controller.as_mut().rust_mut().source_filter = vec!["grok".into()];
    controller
        .as_mut()
        .retry_source("Clean".into(), "leftover text".into(), "grok".into());
    assert!(matches!(
        controller.rust().worker.as_ref().map(|worker| &worker.job),
        Some(Job::RetrySource(view, query, _)) if view == "Clean" && query.is_empty()
    ));
    wait_until(&mut controller, |c| c.rust().worker.is_none());
}
#[test]
fn opening_a_preloading_section_waits_for_that_preload() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    let key = cache_key("Installed", "", &[], false);
    let (sender, receiver) = mpsc::channel();
    let (release, gate) = mpsc::channel::<()>();
    let handle = thread::spawn(move || {
        let _ = gate.recv();
        let mut report = PackageReport::default();
        report.packages.push(Package {
            id: PackageId {
                backend: "apt".into(),
                name: "preloaded".into(),
                architecture: "all".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: "preloaded".into(),
            summary: "Preloaded package".into(),
            installed_version: Some("1".into()),
            candidate_version: Some("1".into()),
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        });
        report.successful_sources.push("apt".into());
        let _ = sender.send(Reply::Done(Ok(Payload::Packages(report))));
    });
    controller.as_mut().rust_mut().prefetch_worker = Some(PrefetchWorker {
        handle,
        receiver,
        cancel: Cancellation::default(),
        view: "Installed".into(),
        key,
        stale: false,
        asked: vec![],
        answered: vec![],
        scope: (vec![], false, Instant::now()),
    });
    controller
        .as_mut()
        .load("Installed".into(), "".into(), "".into(), false, false);
    // No second query: the section waits for the preload in flight.
    assert!(controller.rust().worker.is_none());
    assert!(controller.rust().awaiting_prefetch);
    assert!(*controller.busy());
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && *controller.busy() {
        controller.as_mut().poll();
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!*controller.busy());
    assert!(controller.rows().to_string().contains("preloaded"));
    assert!(controller.rust().worker.is_none());
}
#[test]
fn typed_searches_never_evict_preloaded_sections() {
    let mut cache = ViewCache { entries: vec![] };
    cache.insert(
        cache_key("Installed", "", &[], false),
        cached_view("installed"),
    );
    for index in 0..100 {
        cache.insert(
            cache_key("Search", &format!("query {index}"), &[], false),
            cached_view("search"),
        );
    }
    assert!(cache.get(&cache_key("Installed", "", &[], false)).is_some());
    assert!(cache
        .get(&cache_key("Search", "query 99", &[], false))
        .is_some());
    assert_eq!(
        cache_key("Sources", "", &["apt".into()], false),
        cache_key("Sources", "", &[], false)
    );
}

#[test]
fn source_catalog_check_is_lazy_and_single_flight() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    assert!(controller.rust().catalog_worker.is_none());
    controller.as_mut().begin_catalog_check(|_| {
        vec![Source {
            backend: "apt".into(),
            capabilities: vec![Capability::Search],
            availability: Ok(Availability::Available),
        }]
    });
    controller
        .as_mut()
        .begin_catalog_check(|_| unreachable!("the check is single-flight"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !controller.rust().catalog_checked && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
        controller.as_mut().poll();
    }
    assert!(controller.rust().catalog_checked);
    let catalog: Value = serde_json::from_str(&controller.source_catalog().to_string()).unwrap();
    assert_eq!(catalog.as_array().unwrap().len(), 1);
    assert_eq!(catalog[0]["source"], "apt");
    assert_eq!(catalog[0]["availability_kind"], "available");
    assert!(!*controller.busy());
    assert!(controller.rust().worker.is_none());
    controller.as_mut().begin_catalog_check(|_| Vec::new());
    assert!(controller.rust().catalog_worker.is_none());
    // Try again: a fresh check replaces the catalog, and asking twice
    // while it runs starts it once.
    controller.as_mut().forget_catalog();
    let (release, wait) = mpsc::channel::<()>();
    controller.as_mut().begin_catalog_check(move |_| {
        let _ = wait.recv();
        vec![Source {
            backend: "apt".into(),
            capabilities: vec![Capability::Search],
            availability: Err(EngineError::Execution(ExecutionError::Invalid(
                "apt broke".into(),
            ))),
        }]
    });
    assert!(controller.rust().catalog_worker.is_some());
    controller.as_mut().forget_catalog();
    // Would empty the catalog, if it ran.
    controller.as_mut().begin_catalog_check(|_| Vec::new());
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !controller.rust().catalog_checked && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
        controller.as_mut().poll();
    }
    let catalog: Value = serde_json::from_str(&controller.source_catalog().to_string()).unwrap();
    assert_eq!(catalog[0]["check_failed"], true);
}

#[test]
fn dropping_controller_stops_preloads_and_details_too() {
    // A preload still checking for updates would otherwise outlive
    // PkgDeck, and its command would lose its pipe.
    let prefetch = fake_prefetch("updates", "key".into(), Vec::new(), false);
    let preload = prefetch.cancel.clone();
    let (sender, receiver) = mpsc::channel();
    let details = Cancellation::default();
    let mut controller = Controller::default();
    controller.prefetch_worker = Some(prefetch);
    controller.details_worker = Some(DetailsWorker {
        handle: std::thread::spawn(move || drop(sender)),
        receiver,
        cancel: details.clone(),
        id: synthetic_package("synthetic", "Synthetic").id,
    });
    drop(controller);
    assert!(preload.requested());
    assert!(details.requested());
}

#[test]
fn dropping_controller_waits_for_its_threads() {
    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done = finished.clone();
    let mut prefetch = fake_prefetch("updates", "key".into(), Vec::new(), false);
    prefetch.handle = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        done.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let mut controller = Controller::default();
    controller.prefetch_worker = Some(prefetch);
    drop(controller);
    assert!(finished.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn dropping_controller_cancels_source_catalog_check() {
    let cancel = Cancellation::default();
    let (sender, receiver) = mpsc::channel();
    let handle = std::thread::spawn(move || drop(sender));
    let mut controller = Controller::default();
    controller.catalog_worker = Some(CatalogWorker {
        handle,
        receiver,
        cancel: cancel.clone(),
    });
    drop(controller);
    assert!(cancel.requested());
}

#[test]
fn cleanup_rows_show_preview_and_require_confirmation() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let item = CleanupItem {
        id: CleanupId {
            backend: "apt".into(),
            key: "autoremove".into(),
        },
        kind: CleanupKind::OrphanDependencies,
        title: "Unused dependencies".into(),
        summary: "One package".into(),
        preview: "Remv synthetic-runtime [1.0]".into(),
        bytes: None,
    };
    controller
        .as_mut()
        .apply(Ok(Payload::Cleanup(CleanupReport {
            items: vec![item],
            failures: vec![],
        })));
    controller
        .as_mut()
        .stash_current(cache_key("Clean", "", &[], false));
    controller.as_mut().rust_mut().cleanup.clear();
    controller
        .as_mut()
        .load("Clean".into(), "".into(), "".into(), false, false);
    assert_eq!(controller.rust().cleanup.len(), 1);
    assert!(controller.rust().worker.is_none());
    assert!(controller.rows().to_string().contains("cleanup"));
    controller.as_mut().select(0);
    assert!(controller
        .details()
        .to_string()
        .contains("synthetic-runtime"));
    controller.as_mut().propose("clean".into(), 0);
    assert!(controller
        .confirmation()
        .to_string()
        .contains("Remv synthetic-runtime"));
    assert!(matches!(
        controller.rust().pending,
        Some(Job::Write(Operation::Clean(_), _))
    ));
    propose_every_cleanup(&mut controller);
    assert!(controller
        .confirmation()
        .to_string()
        .contains("Run 1 cleanup task?"));
    assert!(matches!(controller.rust().pending, Some(Job::CleanAll(_))));
    controller.as_mut().confirm(false);

    let failure = BackendFailure {
        backend: "homebrew".into(),
        error: EngineError::Unavailable {
            backend: "homebrew".into(),
            reason: "synthetic executable missing".into(),
        },
    };
    controller
        .as_mut()
        .apply(Ok(Payload::Cleanup(CleanupReport {
            items: vec![],
            failures: vec![failure],
        })));
    assert!(controller
        .status()
        .to_string()
        .contains("could not be checked"));
    assert!(controller.rows().to_string().contains("failure"));
    controller.as_mut().select(0);
    assert!(controller.details().to_string().contains("homebrew"));
    propose_every_cleanup(&mut controller);
    assert!(controller.confirmation().is_empty());
}
#[test]
fn repository_confirmation_and_refresh_invalidate_cached_state() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().change_repository("broken".into());
    assert!(controller
        .status()
        .to_string()
        .contains("This repository change isn't valid"));
    controller.as_mut().change_repository(
        r#"{"backend":"flatpak","name":"--all","scope":"system","action":"remove"}"#.into(),
    );
    assert!(controller
        .status()
        .to_string()
        .contains("invalid repository name"));
    let request = r#"{"backend":"flatpak","name":"fixture","scope":"system","action":"remove"}"#;
    controller.as_mut().change_repository(request.into());
    assert!(controller.confirmation().to_string().contains("System"));
    assert!(controller.confirmation().to_string().contains("fixture"));
    assert!(matches!(
        controller.rust().pending,
        Some(Job::Repositories(Some(_)))
    ));
    assert!(!*controller.busy());
    controller.as_mut().confirm(false);
    assert!(controller.rust().pending.is_none());
    assert!(controller.confirmation().to_string().is_empty());
    controller
        .as_mut()
        .rust_mut()
        .view_cache
        .insert("old".into(), cached_view("old"));
    let key = cache_key("Sources", "", &[], false);
    controller
        .as_mut()
        .rust_mut()
        .view_cache
        .insert(key, cached_view("cached sources"));
    controller
        .as_mut()
        .load("Sources".into(), "".into(), "".into(), false, false);
    assert!(!*controller.busy());
    assert!(controller.rows().to_string().contains("cached sources"));
    controller
        .as_mut()
        .apply(Ok(Payload::Repositories(repositories::Report {
            repositories: vec![
                repositories::Repository {
                    backend: "flatpak".into(),
                    name: "fixture".into(),
                    title: "Fixture".into(),
                    url: "https://example.invalid".into(),
                    scope: Scope::System,
                    enabled: true,
                    priority: Some(1),
                },
                repositories::Repository {
                    backend: "flatpak".into(),
                    name: "fixture-user".into(),
                    title: "Fixture user".into(),
                    url: "https://example.invalid".into(),
                    scope: Scope::User { uid: 1000 },
                    enabled: true,
                    priority: None,
                },
            ],
            errors: vec!["Synthetic partial failure".into()],
            features: repositories::Features::default(),
        })));
    let report: Value = serde_json::from_str(&controller.repositories().to_string()).unwrap();
    assert_eq!(report["repositories"][0]["scope"], "system");
    assert_eq!(report["repositories"][1]["scope"], "user");
    assert_eq!(report["errors"][0], "Synthetic partial failure.");
    assert_eq!(report["repositories"][0]["where"], "Flatpak, System");
    assert!(controller
        .rust()
        .view_cache
        .get("old")
        .is_some_and(CachedView::stale));
}
#[test]
fn restricted_sources_propose_removal_only_when_their_source_can_remove() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let mut app = synthetic_package("/Applications/Obsidian.app", "Obsidian");
    app.id.backend = "macos-apps".into();
    app.installed_version = Some("1.2.3".into());
    app.update = UpdateAvailability::Available;
    let mut tool = synthetic_package("codex", "Codex");
    tool.id.backend = "codex".into();
    tool.installed_version = Some("1.0".into());
    let mut missing = tool.clone();
    missing.installed_version = None;
    controller.as_mut().rust_mut().packages = vec![app, tool, missing];
    let proposes = |controller: &mut Pin<&mut ffi::PackageController>, action: &str, index: i32| {
        controller.as_mut().propose(action.into(), index);
        let proposed = controller.rust().pending.is_some();
        assert_eq!(proposed, !controller.confirmation().is_empty());
        proposed
    };
    // Until the catalog says a source can remove, nothing is proposed.
    for catalog in [
        "[]",
        "not json",
        r#"[{"source":"macos-apps","capabilities":["search","installed"]},
                {"source":"codex","capabilities":["installed","upgrade"]}]"#,
    ] {
        controller.as_mut().set_source_catalog(catalog.into());
        for index in [0, 1] {
            assert!(!proposes(&mut controller, "remove", index));
        }
    }
    controller.as_mut().set_source_catalog(
        r#"[{"source":"macos-apps","capabilities":["installed","remove"]},
                {"source":"codex","capabilities":["installed","remove","upgrade"]}]"#
            .into(),
    );
    // Inventory rows are still never installed or updated, and a source
    // PkgDeck never installs from offers no install.
    for action in ["install", "upgrade"] {
        assert!(!proposes(&mut controller, action, 0));
    }
    assert!(!proposes(&mut controller, "install", 2));
    // Only installed rows can be removed.
    assert!(!proposes(&mut controller, "remove", 2));
    for (index, backend) in [(0, "macos-apps"), (1, "codex")] {
        assert!(proposes(&mut controller, "remove", index));
        assert!(
            matches!(&controller.rust().pending, Some(Job::Write(Operation::Remove(id), _)) if id.backend == backend)
        );
    }
}
#[test]
fn ordinary_sources_offer_removal_without_asking_the_catalog() {
    assert!(removable("[]", "apt"));
    assert!(removable("not json", "homebrew"));
    assert!(!removable("[]", "fwupd"));
    assert!(!removable(r#"[{"source":"mas"}]"#, "mas"));
    assert!(!removable(
        r#"[{"source":"conda","capabilities":["remove"]}]"#,
        "mas"
    ));
    assert!(removable(
        r#"[{"source":"mas","capabilities":["remove"]}]"#,
        "mas"
    ));
}
#[test]
fn firmware_actions_require_confirmation_and_never_offer_removal() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let package = Package {
        id: PackageId {
            backend: "fwupd".into(),
            name: "synthetic-device".into(),
            architecture: "device".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        },
        display_name: "Synthetic BIOS".into(),
        summary: "Firmware, AC power required, Restart required".into(),
        installed_version: Some("1".into()),
        candidate_version: Some("2".into()),
        update: UpdateAvailability::Available,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
        adopt_with: None,
    };
    controller.as_mut().rust_mut().packages = vec![package.clone()];
    controller.as_mut().rust_mut().detail_cache.insert(
        package.id.clone(),
        encoded(json!({"description": "Synthetic cached firmware details"})),
    );
    controller.as_mut().select(0);
    assert!(controller
        .details()
        .to_string()
        .contains("Synthetic cached firmware details"));
    assert!(!*controller.busy());

    controller.as_mut().rust_mut().detail_cache.clear();
    let (_sender, receiver) = mpsc::channel();
    let cancelled = Cancellation::default();
    let mut previous = package.id.clone();
    previous.name = "previous-device".into();
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: cancelled.clone(),
        job: Job::Details(previous),
    });
    controller.as_mut().select(0);
    assert!(cancelled.requested());
    assert!(matches!(&controller.rust().queued, Some(Job::Details(id)) if *id == package.id));
    controller
        .as_mut()
        .rust_mut()
        .worker
        .take()
        .unwrap()
        .handle
        .join()
        .unwrap();
    controller.as_mut().rust_mut().queued = None;
    controller.as_mut().rust_mut().updates_view = true;
    controller.as_mut().set_upgradable(true);
    for action in ["install", "remove"] {
        controller.as_mut().propose(action.into(), 0);
        assert!(controller.rust().pending.is_none());
    }
    for action in ["upgrade", "upgrade-all"] {
        controller.as_mut().propose(action.into(), 0);
        let confirmation = controller.confirmation().to_string();
        assert!(confirmation.contains("Synthetic BIOS"));
        assert!(confirmation.contains("AC power required"));
        assert!(confirmation.contains("Restart required"));
        assert!(!*controller.writing());
        controller.as_mut().confirm(false);
    }
    let identity = json!([[
        package.id.backend,
        package.id.name,
        package.id.architecture,
        null,
        package.id.scope
    ]])
    .to_string();
    controller
        .as_mut()
        .propose_checked(identity.as_str().into());
    assert!(controller
        .confirmation()
        .to_string()
        .contains("Synthetic BIOS"));
    controller.as_mut().propose_checked("[]".into());
    assert!(controller.rust().pending.is_none());
    assert!(controller.status().to_string().contains("No selected"));
    controller.as_mut().apply(Ok(Payload::Batch(Batch {
        noun: "updates",
        steps: vec![Ended {
            title: "Update Synthetic BIOS".into(),
            outcome: Outcome::Finished,
            said: Said::Completed,
            kept: None,
        }],
        note: firmware_note(&[Operation::Upgrade(package.id.clone())]),
    })));
    assert!(controller
        .status()
        .to_string()
        .contains("Restart or shut down"));
    assert!(!controller.rust().packages.is_empty());
    assert!(!*controller.upgradable());
}
#[test]
fn container_pull_is_explicit_and_requires_a_stored_tag() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let package = Package {
        id: PackageId {
            backend: "docker".into(),
            name: "sha256:0123456789abcdef".into(),
            architecture: "x86_64".into(),
            scope: Scope::System,
            remote: Some("Docker daemon".into()),
            reference: Some("example/app:latest".into()),
        },
        display_name: "example/app:latest".into(),
        summary: "Tags: example/app:latest. 42MB".into(),
        installed_version: Some("0123456789ab".into()),
        candidate_version: None,
        update: UpdateAvailability::Unknown,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
        adopt_with: None,
    };
    controller.as_mut().rust_mut().packages = vec![package];
    controller.as_mut().propose("upgrade".into(), 0);
    assert!(matches!(
        &controller.rust().pending,
        Some(Job::Write(Operation::Upgrade(_), _))
    ));
    assert!(controller
        .confirmation()
        .to_string()
        .contains("example/app:latest"));
    controller.as_mut().confirm(false);
    controller.as_mut().rust_mut().packages[0].id.reference = None;
    controller.as_mut().propose("upgrade".into(), 0);
    assert!(controller.rust().pending.is_none());
}
#[test]
fn source_failures_and_busy_requests_preserve_the_active_operation() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .load("Sources".into(), "".into(), "unknown".into(), false, false);
    assert_eq!(controller.status().to_string(), "Unknown source.");
    assert!(!*controller.busy());
    controller
        .as_mut()
        .rust_mut()
        .failures
        .push(BackendFailure {
            backend: "fwupd".into(),
            error: EngineError::Unavailable {
                backend: "fwupd".into(),
                reason: "Synthetic daemon offline".into(),
            },
        });
    controller.as_mut().select(0);
    assert!(controller
        .details()
        .to_string()
        .contains("Firmware isn't available: synthetic daemon offline."));
    controller.as_mut().rust_mut().failures.clear();
    controller.as_mut().rust_mut().sources.push(Source {
        backend: "fwupd".into(),
        capabilities: vec![Capability::Refresh],
        availability: Ok(Availability::Unavailable("Synthetic unavailable".into())),
    });
    controller.as_mut().select(0);
    assert!(controller
        .details()
        .to_string()
        .contains("Synthetic unavailable"));
    controller.as_mut().propose("refresh".into(), 0);
    assert!(controller.rust().pending.is_none());
    let (sender, receiver) = mpsc::channel();
    let cancel = Cancellation::default();
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: cancel.clone(),
        job: Job::Repositories(Some(RepositoryAction {
            backend: "flatpak".into(),
            name: "fixture".into(),
            scope: Scope::System,
            change: repositories::Change::Remove,
        })),
    });
    controller.as_mut().load_repositories();
    controller.as_mut().change_repository("invalid".into());
    controller.as_mut().propose_checked("[]".into());
    controller
        .as_mut()
        .load("Sources".into(), "".into(), "".into(), false, true);
    assert!(!cancel.requested());
    assert!(controller.rust().queued.is_none());
    controller.as_mut().cancel();
    assert!(cancel.requested());
    assert!(controller
        .status()
        .to_string()
        .contains("Cancelling once the package manager finishes safely"));
    sender
        .send(Reply::Done(Ok(Payload::Repositories(
            repositories::Report::default(),
        ))))
        .unwrap_or_else(|_| panic!("worker channel closed"));
    controller.as_mut().poll();
    assert!(controller.rust().worker.is_none());
    assert!(!*controller.busy());
}
#[test]
fn streamed_details_preserve_progress_and_finish_cleanly() {
    let mut object = ffi::create_controller();
    let mut controller = object.pin_mut();
    let package = Package {
        id: PackageId {
            backend: "fwupd".into(),
            name: "synthetic-device".into(),
            architecture: "device".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        },
        display_name: "Device".into(),
        summary: "Firmware".into(),
        installed_version: Some("1".into()),
        candidate_version: Some("2".into()),
        update: UpdateAvailability::Available,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
        adopt_with: None,
    };
    controller.as_mut().rust_mut().packages = vec![package.clone()];
    controller.as_mut().rust_mut().selected = Some(package.id.clone());
    controller
        .as_mut()
        .set_rows(encoded(vec![package_row(&package, &[], None)]));
    let mut details = PackageDetails {
        package: package.clone(),
        description: "Synthetic firmware details".into(),
        homepage: None,
        dependencies: vec![],
    };
    details.package.display_name = "Synthetic BIOS".into();
    let (sender, receiver) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(move || {
            wait.recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
        }),
        receiver,
        cancel: Cancellation::default(),
        job: Job::Details(package.id.clone()),
    });
    controller.as_mut().set_busy(true);
    sender
        .send(Reply::DetailsPreview(Box::new(details.clone())))
        .unwrap_or_else(|_| panic!("closed channel"));
    sender
        .send(Reply::Progress("Loading device metadata".into()))
        .unwrap_or_else(|_| panic!("closed channel"));
    controller.as_mut().poll();
    assert_eq!(controller.status().to_string(), "Loading device metadata");
    assert!(controller
        .details()
        .to_string()
        .contains("Synthetic firmware details"));
    assert!(controller.rows().to_string().contains("Synthetic BIOS"));
    assert!(*controller.busy());
    release.send(()).unwrap();
    sender
        .send(Reply::Done(Ok(Payload::Details(Box::new(details)))))
        .unwrap_or_else(|_| panic!("closed channel"));
    controller.as_mut().poll();
    assert!(!*controller.busy());
    assert!(controller.rust().worker.is_none());
    controller
        .as_mut()
        .apply(Ok(Payload::Written(OperationOutcome {
            cancellation_deferred: true,
        })));
    assert!(controller
        .status()
        .to_string()
        .contains("Finished before it could be cancelled"));
}
#[test]
fn new_view_discards_superseded_reports_and_cancellation_errors() {
    let mut object = ffi::create_controller();
    let mut controller = object.pin_mut();
    let (_sender, receiver) = mpsc::channel();
    let cancel = Cancellation::default();
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: cancel.clone(),
        job: Job::Load("Sources".into(), "".into()),
    });
    controller
        .as_mut()
        .load("Updates".into(), "".into(), "".into(), false, true);
    assert!(cancel.requested());
    assert!(matches!(&controller.rust().queued, Some(Job::Load(view, _)) if view == "Updates"));
    let status = controller.status().to_string();
    controller.as_mut().apply(Err(EngineError::Cancelled));
    assert_eq!(controller.status().to_string(), status);
    controller
        .as_mut()
        .apply(Ok(Payload::Packages(PackageReport::default())));
    controller.as_mut().apply(Ok(Payload::Sources(vec![Source {
        backend: "fwupd".into(),
        capabilities: vec![],
        availability: Ok(Availability::Available),
    }])));
    assert!(controller.rust().sources.is_empty());
    assert_eq!(controller.rows().to_string(), "[]");
    controller
        .as_mut()
        .rust_mut()
        .worker
        .take()
        .unwrap()
        .handle
        .join()
        .unwrap();
    controller.as_mut().rust_mut().queued = None;
    controller.as_mut().apply(Err(EngineError::NotFound));
    assert!(controller
        .status()
        .to_string()
        .contains("No package matches"));
}
struct Fixture {
    package: Package,
    fail: bool,
}
impl Backend for Fixture {
    fn id(&self) -> &str {
        &self.package.id.backend
    }
    fn capabilities(&self) -> &[Capability] {
        &[
            Capability::Search,
            Capability::Installed,
            Capability::Details,
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
        Ok(vec![self.package.clone()])
    }
    fn installed(&mut self, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(vec![self.package.clone()])
    }
    fn details(&mut self, _: &PackageId, _: &Cancellation) -> Result<PackageDetails, EngineError> {
        Err(EngineError::NotFound)
    }
    fn execute(
        &mut self,
        _: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        progress(Progress::Message("Synthetic progress".into()));
        if self.fail {
            Err(pkgdeck_core::process::ExecutionError::AuthorizationDenied.into())
        } else {
            cancel.cancel();
            Ok(OperationOutcome {
                cancellation_deferred: true,
            })
        }
    }
}
#[test]
fn inventory_jobs_export_and_preview_exact_installed_identity() {
    let package: Package = serde_json::from_value(json!({
            "id":{"backend":"fixture", "name":"synthetic-editor", "architecture":"x86_64", "scope":"system"},
            "display_name":"Synthetic Editor", "summary":"Fixture", "installed_version":"1", "candidate_version":null, "update":"current"
        })).unwrap();
    let path = std::env::temp_dir().join(format!(
        "pkgdeck-gui-inventory-{}-{:?}.json",
        std::process::id(),
        std::thread::current().id()
    ));
    let mut engine = Engine::default();
    engine
        .register(Fixture {
            package: package.clone(),
            fail: false,
        })
        .unwrap();
    let mut replies = Vec::new();
    execute(
        &mut engine,
        Job::ManifestExport(path.clone(), vec![package.id.clone()]),
        &Cancellation::default(),
        &mut |reply| replies.push(reply),
    );
    assert!(matches!(
        replies.pop(),
        Some(Reply::Done(Ok(Payload::ManifestExport(1))))
    ));
    let saved = manifest::read(&path).unwrap();
    assert_eq!(saved.packages[0].name, "synthetic-editor");
    execute(
        &mut engine,
        Job::ManifestPreview(path.clone()),
        &Cancellation::default(),
        &mut |reply| replies.push(reply),
    );
    let preview = expect!(
        replies.pop(),
        Some(Reply::Done(Ok(Payload::ManifestPreview(preview)))) => preview
    );
    assert_eq!(
        preview.packages[0].status,
        manifest::PreviewStatus::AlreadyInstalled
    );
    execute(
        &mut engine,
        Job::ManifestExport(path.clone(), vec![]),
        &Cancellation::default(),
        &mut |reply| replies.push(reply),
    );
    assert!(matches!(replies.pop(), Some(Reply::Done(Err(_)))));
    std::fs::remove_file(path).unwrap();
}
/// A source whose installed list always fails and whose writes report
/// the given progress before succeeding.
struct Scripted {
    id: &'static str,
    progress: Vec<Progress>,
}
impl Backend for Scripted {
    fn id(&self) -> &str {
        self.id
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Installed, Capability::Upgrade]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn execute(
        &mut self,
        _: &Operation,
        _: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        self.progress.drain(..).for_each(progress);
        Ok(OperationOutcome::default())
    }
}
/// Homebrew Casks where updating `locked` stops at sudo's password.
struct PasswordCask {
    locked: &'static str,
}
impl Backend for PasswordCask {
    fn id(&self) -> &str {
        "homebrew-cask"
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Installed, Capability::Upgrade]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        _: &Cancellation,
        _: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        match operation {
                Operation::Upgrade(id) if id.name == self.locked => Err(
                    pkgdeck_core::process::ExecutionError::Failed(
                        pkgdeck_core::process::Completion {
                            code: Some(1),
                            signal: None,
                            stdout: vec![],
                            stderr: format!("sudo: a password is required\nError: {}: Failure while executing; `/usr/bin/sudo -E -- /bin/rm` exited with 1.\n", id.name).into_bytes(),
                            truncated: false,
                            cancellation_deferred: false,
                        },
                    )
                    .into(),
                ),
                _ => Ok(OperationOutcome::default()),
            }
    }
}
#[test]
fn an_automatic_run_remembers_casks_that_stopped_for_the_password() {
    let cask = |name: &str| {
        let mut package = synthetic_package(name, name);
        package.id.backend = "homebrew-cask".into();
        package
    };
    let (mail, editor) = (cask("mail-app"), cask("editor-app"));
    let job = Job::AutoUpgrade(
        vec![
            Operation::Upgrade(mail.id.clone()),
            Operation::Upgrade(editor.id.clone()),
        ],
        false,
    );
    let mut engine = Engine::default();
    engine
        .register(PasswordCask { locked: "mail-app" })
        .unwrap();
    let replies = run_job(&mut engine, job.clone(), &Cancellation::default());
    assert!(replies
        .iter()
        .any(|reply| matches!(reply, Reply::NeedsPassword(id) if *id == mail.id)));

    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().cask_versions = BTreeMap::from([
        (mail.id.clone(), "2".to_owned()),
        (editor.id.clone(), "5".to_owned()),
    ]);
    let replies = replies
        .into_iter()
        .filter(|reply| matches!(reply, Reply::NeedsPassword(_) | Reply::Done(_)))
        .collect();
    controller.as_mut().rust_mut().worker = Some(fake_worker(job, replies));
    controller.as_mut().poll();
    let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
    assert_eq!(notice["kind"], "info");
    assert_eq!(notice["title"], "mail-app needs your password to update");
    assert!(controller.rust().needs_password.skips("mail-app", "2"));
    assert!(!controller.rust().needs_password.skips("editor-app", "5"));
    assert!(controller.rust().password_casks.is_empty());
}
/// A standalone tool whose updates fail while `broken`.
struct BrokenTool {
    broken: bool,
}
impl Backend for BrokenTool {
    fn id(&self) -> &str {
        "anchor"
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Installed, Capability::Upgrade]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn execute(
        &mut self,
        _: &Operation,
        _: &Cancellation,
        _: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        if self.broken {
            Err(EngineError::Execution(
                pkgdeck_core::process::ExecutionError::Io(
                    "No such file or directory (os error 2)".into(),
                ),
            ))
        } else {
            Ok(OperationOutcome::default())
        }
    }
}
#[test]
fn failures_with_the_same_reason_share_one_line() {
    let clean = |key: &str| {
        Operation::Clean(CleanupId {
            backend: "dev-caches".into(),
            key: key.into(),
        })
    };
    let names = Names::new();
    let notice = |keys: &[&str], reasons: &[&str]| {
        let operations: Vec<_> = keys.iter().map(|key| clean(key)).collect();
        let steps: Vec<_> = reasons
            .iter()
            .map(|reason| ("Clean", Outcome::Failed, because(reason)))
            .collect();
        write_notice(&Job::CleanAll(operations), &Ok(ran(&steps)), false, &names)
    };
    let name = |key: &str| short_name(&clean(key), &names);
    let (busy, gone) = ("The disk is busy.", "It's gone.");
    let two = notice(&["a", "b"], &[busy, busy]);
    assert_eq!(two["title"], "2 of 2 changes failed");
    assert_eq!(
        two["detail"],
        format!("{} and {}: {busy}", name("a"), name("b"))
    );
    let three = notice(&["a", "b", "c"], &[busy, gone, busy]);
    assert_eq!(
        three["detail"],
        format!(
            "{} and {}: {busy}\n{}: {gone}",
            name("a"),
            name("c"),
            name("b")
        )
    );
    let many = notice(&["a", "b", "c", "d", "e"], &[busy; 5]);
    assert_eq!(
        many["detail"],
        format!("{}, {} and 3 more: {busy}", name("a"), name("b"))
    );
    let four = notice(&["a", "b", "c"], &[busy; 3]);
    assert_eq!(
        four["detail"],
        format!("{}, {} and {}: {busy}", name("a"), name("b"), name("c"))
    );
    // A reason that already names its change stays on its own line.
    let named = format!("{} couldn't run.", name("a"));
    let own = notice(&["a", "b"], &[&named, busy]);
    assert_eq!(own["detail"], format!("{named}\n{}: {busy}", name("b")));
}
#[test]
fn update_all_skips_updates_that_failed_until_they_work() {
    let available = |backend: &str, name: &str, display: &str, version: &str| {
        let mut package = synthetic_package(name, display);
        package.id.backend = backend.into();
        package.installed_version = Some("1".into());
        package.candidate_version = Some(version.into());
        package.update = UpdateAvailability::Available;
        package
    };
    let mail = available("homebrew-cask", "mail-app", "Mail App", "2");
    let mut tool = available("anchor", "anchor", "Anchor (AVM)", "2.0");
    tool.id.reference = Some("/home/fixture/.avm/bin/avm".into());
    let upgrade = |package: &Package| Operation::Upgrade(package.id.clone());
    let run = |controller: &mut Pin<&mut ffi::PackageController>, job: Job, broken: bool| {
        let mut engine = Engine::default();
        engine
            .register(PasswordCask { locked: "mail-app" })
            .unwrap();
        engine.register(BrokenTool { broken }).unwrap();
        let replies = run_job(&mut engine, job.clone(), &Cancellation::default())
            .into_iter()
            .filter(|reply| matches!(reply, Reply::NeedsPassword(_) | Reply::Done(_)))
            .collect();
        controller.as_mut().rust_mut().worker = Some(fake_worker(job, replies));
        controller.as_mut().poll();
    };
    let held = |controller: &Pin<&mut ffi::PackageController>| -> Vec<(String, String)> {
        serde_json::from_str::<Vec<Value>>(controller.held_updates().as_str())
            .unwrap()
            .iter()
            .map(|held| {
                (
                    held["name"].as_str().unwrap().to_owned(),
                    held["reason"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().packages = vec![mail.clone(), tool.clone()];
    controller.as_mut().rust_mut().updates_view = true;
    run(
        &mut controller,
        Job::UpgradeAll(vec![upgrade(&mail), upgrade(&tool)], None),
        true,
    );
    // Each failure is one short line named the way people know it.
    let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
    assert_eq!(notice["title"], "2 of 2 changes failed", "{notice}");
    let detail = notice["detail"].as_str().unwrap();
    assert!(!detail.contains("/home/fixture"), "{detail}");
    assert!(detail.contains("Anchor (AVM): "), "{detail}");
    assert!(detail.contains("Mail App: "), "{detail}");
    // The tool waits as a failure, the cask for the password.
    assert!(controller.rust().failed_updates.skips(&tool.id, "2.0"));
    assert!(!controller.rust().failed_updates.skips(&mail.id, "2"));
    assert!(controller.rust().needs_password.skips("mail-app", "2"));
    assert_eq!(
        held(&controller),
        [
            ("anchor".into(), "failed".into()),
            ("mail-app".into(), "password".into())
        ]
    );
    // A cask is known by its source and name, at the version it needed
    // the password for.
    let other = available("homebrew-cask", "other-app", "Other App", "7");
    let same_name = available("apt", "mail-app", "Mail App", "9");
    controller.as_mut().rust_mut().packages = vec![other, same_name, mail.clone(), tool.clone()];
    controller.as_mut().publish_held_updates();
    assert!(held(&controller).contains(&("mail-app".into(), "password".into())));
    controller.as_mut().rust_mut().packages = vec![mail.clone(), tool.clone()];
    // Update all leaves the failed one out, and asks for the cask.
    controller.as_mut().propose("upgrade-all".into(), -1);
    assert!(matches!(
        &controller.rust().pending,
        Some(Job::UpgradeAll(operations, _))
            if !operations.is_empty() && !operations.contains(&upgrade(&tool))
    ));
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    let summary = data["summary"].as_str().unwrap();
    assert!(
        summary.starts_with("Update 1 package") && !summary.starts_with("Update 1 packages"),
        "{summary}"
    );
    // With nothing else left, it tries it again.
    controller.as_mut().confirm(false);
    controller.as_mut().rust_mut().packages = vec![tool.clone()];
    controller.as_mut().propose("upgrade-all".into(), -1);
    assert!(matches!(
        &controller.rust().pending,
        Some(Job::UpgradeAll(operations, _)) if operations == &[upgrade(&tool)]
    ));
    controller.as_mut().confirm(false);
    // A newer version is tried again and isn't marked.
    let newer = available("anchor", "anchor", "Anchor (AVM)", "2.1");
    let newer = Package {
        id: tool.id.clone(),
        ..newer
    };
    controller.as_mut().rust_mut().packages = vec![newer.clone()];
    controller.as_mut().publish_held_updates();
    assert!(held(&controller).iter().all(|(name, _)| name != "anchor"));
    // One change that fails on its own is remembered too.
    run(&mut controller, Job::Write(upgrade(&tool), None), true);
    assert!(controller.rust().failed_updates.skips(&tool.id, "2.1"));
    // A batch that failed before any step ran blames none of them.
    controller.as_mut().rust_mut().packages = vec![mail.clone(), newer.clone()];
    let job = Job::UpgradeAll(vec![upgrade(&mail), upgrade(&tool)], None);
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        job,
        vec![Reply::Done(Err(EngineError::NotFound))],
    ));
    controller.as_mut().poll();
    assert!(!controller.rust().failed_updates.skips(&mail.id, "2"));
    controller.as_mut().rust_mut().packages = vec![mail.clone(), tool.clone()];
    // Once it works, it's forgotten, and so is the cask.
    run(&mut controller, Job::Write(upgrade(&tool), None), false);
    assert!(!controller.rust().failed_updates.skips(&tool.id, "2.0"));
    let mut engine = Engine::default();
    engine.register(PasswordCask { locked: "other" }).unwrap();
    let job = Job::Write(upgrade(&mail), None);
    let replies = run_job(&mut engine, job.clone(), &Cancellation::default());
    controller.as_mut().rust_mut().worker = Some(fake_worker(job, replies));
    controller.as_mut().poll();
    assert!(!controller.rust().needs_password.skips("mail-app", "2"));
    assert!(held(&controller).is_empty());
}
#[test]
fn one_failed_update_is_named_without_its_path() {
    let mut tool = synthetic_package("anchor", "Anchor (AVM)");
    tool.id.backend = "anchor".into();
    tool.id.reference = Some("/home/fixture/.avm/bin/avm".into());
    let operation = Operation::Upgrade(tool.id.clone());
    let names = Names::from([(tool.id.clone(), "Anchor (AVM)".to_owned())]);
    let title = operation_title(&operation);
    assert!(
        title.contains("/home/fixture"),
        "the step's title names the path"
    );
    let notice = write_notice(
        &Job::AutoUpgrade(vec![operation.clone()], false),
        &Ok(ran(&[(
            &title,
            Outcome::Failed,
            because("Anchor (AVM) couldn't run: No such file or directory (os error 2)."),
        )])),
        false,
        &names,
    );
    assert_eq!(notice["title"], "Anchor (AVM) didn't update");
    assert_eq!(
        notice["detail"],
        "Anchor (AVM) couldn't run: No such file or directory (os error 2)."
    );
    // A single change that fails on its own reads the same way.
    let single = write_notice(
        &Job::Write(operation, None),
        &Err(EngineError::NotFound),
        false,
        &names,
    );
    assert_eq!(single["title"], "Anchor (AVM) didn't update");
    // Anything else keeps saying what failed.
    let install = write_notice(
        &Job::Write(Operation::Install(tool.id.clone()), None),
        &Err(EngineError::NotFound),
        false,
        &names,
    );
    assert_eq!(
        install["title"],
        "Install Anchor (AVM) (Anchor (AVM)) failed"
    );
}
#[test]
fn a_failure_notice_goes_once_its_updates_are_done() {
    let mut tool = synthetic_package("anchor", "Anchor (AVM)");
    tool.id.backend = "anchor".into();
    tool.installed_version = Some("1".into());
    tool.candidate_version = Some("2.0".into());
    tool.update = UpdateAvailability::Available;
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().updates_view = true;
    let list = |controller: &mut Pin<&mut ffi::PackageController>, packages: Vec<Package>| {
        controller
            .as_mut()
            .apply(Ok(Payload::Packages(PackageReport {
                packages,
                failures: vec![],
                successful_sources: vec!["anchor".into()],
            })));
    };
    let fail = |controller: &mut Pin<&mut ffi::PackageController>, job: Job| {
        controller.as_mut().rust_mut().retry_job = Some(job);
        controller
            .as_mut()
            .set_notice(r#"{"kind":"error","title":"Anchor (AVM) didn't update"}"#.into());
    };
    // Still waiting: the notice stays.
    fail(
        &mut controller,
        Job::Write(Operation::Upgrade(tool.id.clone()), None),
    );
    list(&mut controller, vec![tool.clone()]);
    assert_ne!(controller.notice().to_string(), "{}");
    // Only the Updates list tells: another page doesn't list it.
    controller.as_mut().rust_mut().updates_view = false;
    list(&mut controller, vec![]);
    assert_ne!(controller.notice().to_string(), "{}");
    controller.as_mut().rust_mut().updates_view = true;
    // Updated somewhere else, so still listed but current: it goes.
    let mut current = tool.clone();
    current.update = UpdateAvailability::Current;
    list(&mut controller, vec![current]);
    assert_eq!(controller.notice().to_string(), "{}");
    assert!(controller.rust().retry_job.is_none());
    // A whole source can't be told apart by package, so it stays.
    fail(
        &mut controller,
        Job::UpgradeAll(
            vec![Operation::UpgradeAll {
                backend: "anchor".into(),
            }],
            None,
        ),
    );
    list(&mut controller, vec![]);
    assert_ne!(controller.notice().to_string(), "{}");
}
#[test]
fn a_background_check_forgets_failed_updates_that_are_done() {
    let package = |backend: &str, update: UpdateAvailability| {
        let mut package = synthetic_package(backend, backend);
        package.id.backend = backend.into();
        package.installed_version = Some("1".into());
        package.candidate_version = Some("2.0".into());
        package.update = update;
        package
    };
    let waiting = package("anchor", UpdateAvailability::Available);
    let done = package("foundry", UpdateAvailability::Current);
    let unchecked = package("nix", UpdateAvailability::Available);
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().failed_updates.remember(
        &[
            (waiting.id.clone(), "2.0".into()),
            (done.id.clone(), "2.0".into()),
            (unchecked.id.clone(), "2.0".into()),
        ],
        &[],
    );
    controller.as_mut().finish_background_check(PackageReport {
        packages: vec![waiting.clone(), done.clone()],
        failures: vec![BackendFailure {
            backend: "nix".into(),
            error: EngineError::NotFound,
        }],
        successful_sources: vec!["anchor".into(), "foundry".into()],
    });
    let failed = &controller.rust().failed_updates;
    // Still listed at that version: kept.
    assert!(failed.skips(&waiting.id, "2.0"));
    // Up to date now: forgotten.
    assert!(!failed.skips(&done.id, "2.0"));
    // Its source didn't answer: kept until it does.
    assert!(failed.skips(&unchecked.id, "2.0"));
}
#[test]
fn a_failed_cleanup_line_names_the_step() {
    let clean = Operation::Clean(CleanupId {
        backend: "apt".into(),
        key: "cache".into(),
    });
    let notice = write_notice(
        &Job::CleanAll(vec![clean.clone()]),
        &Ok(ran(&[(
            &operation_title(&clean),
            Outcome::Failed,
            because("disk full"),
        )])),
        false,
        &Names::new(),
    );
    assert_eq!(notice["title"], "Clean cache (APT) failed");
    assert_eq!(notice["detail"], "Clean cache (APT): disk full");
}
#[test]
fn a_refused_password_reads_as_plain_advice_on_its_row() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    let package = synthetic_package("htop", "htop");
    let upgrade = Operation::Upgrade(package.id.clone());
    finish_write(
        &mut controller,
        Job::UpgradeAll(vec![upgrade.clone()], None),
        Ok(ran(&[(
            &operation_title(&upgrade),
            Outcome::Failed,
            Said::Denied,
        )])),
    );
    let advice = plain_error(
        &EngineError::Execution(pkgdeck_core::process::ExecutionError::AuthorizationDenied),
        None,
        false,
    );
    assert_eq!(parsed(controller.failed_rows())[0]["reason"], advice);
}
/// Proposes every listed cleanup task, as Clean all does.
/// Checks every task, then cleans the checked ones.
fn propose_every_cleanup(controller: &mut Pin<&mut ffi::PackageController>) {
    let ids: Vec<_> = controller
        .rust()
        .cleanup
        .iter()
        .map(|item| item.id.clone())
        .collect();
    for id in &ids {
        let preferences = &mut controller.as_mut().rust_mut().cleanup_preferences;
        preferences.set_included(id.clone(), true);
    }
    controller
        .as_mut()
        .propose_clean_checked(encoded(json!(ids)));
}
/// Lets `job` finish with `result`, as its worker would.
fn finish_write(
    controller: &mut Pin<&mut ffi::PackageController>,
    job: Job,
    result: Result<Payload, EngineError>,
) {
    let (sender, receiver) = mpsc::channel();
    sender.send(Reply::Done(result)).unwrap();
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: Cancellation::default(),
        job,
    });
    wait_until(controller, |c| c.rust().worker.is_none());
}
fn parsed(text: &QString) -> Value {
    serde_json::from_str(&text.to_string()).unwrap()
}
#[test]
fn a_failed_step_marks_its_row_until_a_later_look_shows_it_done() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    let id = |key: &str| CleanupId {
        backend: "projects".into(),
        key: key.into(),
    };
    let (kept_id, gone_id) = (id("worktrees:/code/kept"), id("worktrees:/code/gone"));
    let (kept, gone) = (Operation::Clean(kept_id.clone()), Operation::Clean(gone_id));
    finish_write(
        &mut controller,
        Job::CleanAll(vec![kept.clone(), gone.clone()]),
        Ok(ran(&[
            (
                &operation_title(&kept),
                Outcome::Failed,
                because("Couldn't remove ~/kept: b is busy"),
            ),
            (
                &operation_title(&gone),
                Outcome::Failed,
                because("Couldn't remove ~/gone: it is busy"),
            ),
        ])),
    );
    let rows = parsed(controller.failed_rows());
    assert_eq!(
        rows[0],
        json!({"target": {"source": "projects", "cleanup_key": "worktrees:/code/kept"},
                "reason": "Couldn't remove ~/kept: b is busy"})
    );
    assert_eq!(rows.as_array().unwrap().len(), 2);
    let failure = parsed(controller.failure());
    assert_eq!(failure["kind"], "error");
    assert_eq!(failure["urgent"], false);
    assert_eq!(failure["retry"], true);
    // The toast going away leaves the failure and its Retry.
    controller.as_mut().dismiss_notice();
    assert_eq!(controller.notice().to_string(), "{}");
    assert_ne!(controller.failure().to_string(), "{}");
    assert!(controller.rust().retry_job.is_some());
    // Cleaned some other way: its row and, once both are, the failure go.
    let item = |id: &CleanupId| CleanupItem {
        id: id.clone(),
        kind: CleanupKind::Worktree,
        title: "worktrees".into(),
        summary: String::new(),
        preview: String::new(),
        bytes: None,
    };
    let look =
        |controller: &mut Pin<&mut ffi::PackageController>, items: Vec<CleanupItem>, failures| {
            controller
                .as_mut()
                .apply(Ok(Payload::Cleanup(CleanupReport { items, failures })));
        };
    look(&mut controller, vec![item(&kept_id)], vec![]);
    assert_eq!(
        parsed(controller.failed_rows()).as_array().unwrap().len(),
        1
    );
    assert_ne!(controller.failure().to_string(), "{}");
    // A source that couldn't be checked tells nothing.
    look(
        &mut controller,
        vec![],
        vec![BackendFailure {
            backend: "projects".into(),
            error: EngineError::NotFound,
        }],
    );
    assert_eq!(
        parsed(controller.failed_rows()).as_array().unwrap().len(),
        1
    );
    look(&mut controller, vec![], vec![]);
    assert_eq!(controller.failed_rows().to_string(), "[]");
    assert_eq!(controller.failure().to_string(), "{}");
    assert!(controller.rust().retry_job.is_none());
}
#[test]
fn a_cancelled_step_is_no_failure_and_a_removal_seen_gone_clears_its_row() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    let clean = Operation::Clean(CleanupId {
        backend: "projects".into(),
        key: "worktrees:/code/app".into(),
    });
    finish_write(
        &mut controller,
        Job::CleanAll(vec![clean]),
        Ok(ran(&[("Clean", Outcome::Cancelled, because("Cancelled."))])),
    );
    assert_eq!(controller.failed_rows().to_string(), "[]");
    assert_eq!(controller.failure().to_string(), "{}");
    let package = synthetic_package("htop", "htop");
    finish_write(
        &mut controller,
        Job::Write(Operation::Remove(package.id.clone()), None),
        Err(EngineError::NotFound),
    );
    assert_eq!(
        parsed(controller.failed_rows()).as_array().unwrap().len(),
        1
    );
    controller.as_mut().rust_mut().active_view = "Installed".into();
    let look = |controller: &mut Pin<&mut ffi::PackageController>, packages: Vec<Package>| {
        controller
            .as_mut()
            .apply(Ok(Payload::Packages(PackageReport {
                packages,
                failures: vec![],
                successful_sources: vec!["apt".into()],
            })));
    };
    // Still installed: the removal didn't happen.
    look(&mut controller, vec![package]);
    assert_eq!(
        parsed(controller.failed_rows()).as_array().unwrap().len(),
        1
    );
    // Gone from a complete look at Installed: the row and failure clear.
    look(&mut controller, vec![]);
    assert_eq!(controller.failed_rows().to_string(), "[]");
    assert_eq!(controller.failure().to_string(), "{}");
}
#[test]
fn unrelated_source_reports_keep_failed_changes_and_their_retry() {
    let package = synthetic_package("htop", "htop");
    let clean = Operation::Clean(CleanupId {
        backend: "projects".into(),
        key: "worktrees:/code/app".into(),
    });
    for operation in [
        Operation::Upgrade(package.id.clone()),
        Operation::Remove(package.id.clone()),
        clean,
    ] {
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        let cleaning = matches!(operation, Operation::Clean(_));
        let updating = matches!(operation, Operation::Upgrade(_));
        finish_write(
            &mut controller,
            Job::Write(operation.clone(), None),
            Err(EngineError::NotFound),
        );
        controller.as_mut().rust_mut().active_view = if cleaning {
            "Clean"
        } else if updating {
            "Updates"
        } else {
            "Installed"
        }
        .into();
        controller.as_mut().rust_mut().updates_view = updating;
        let look = |controller: &mut Pin<&mut ffi::PackageController>, source: &str| {
            controller.as_mut().rust_mut().source_filter = vec![source.into()];
            controller.as_mut().apply(Ok(if cleaning {
                Payload::Cleanup(CleanupReport {
                    items: vec![],
                    failures: vec![],
                })
            } else {
                Payload::Packages(PackageReport {
                    packages: vec![],
                    failures: vec![],
                    successful_sources: vec![source.into()],
                })
            }));
        };
        look(&mut controller, "npm");
        assert_eq!(
            parsed(controller.failed_rows()).as_array().unwrap().len(),
            1,
            "{operation:?}"
        );
        assert_ne!(controller.failure().to_string(), "{}", "{operation:?}");
        assert!(controller.rust().retry_job.is_some(), "{operation:?}");
        if !cleaning {
            controller.as_mut().rust_mut().source_filter.clear();
            controller
                .as_mut()
                .apply(Ok(Payload::Packages(PackageReport::default())));
            assert_eq!(
                parsed(controller.failed_rows()).as_array().unwrap().len(),
                1
            );
            assert!(controller.rust().retry_job.is_some());
        }
        look(&mut controller, operation.backend());
        assert_eq!(controller.failed_rows().to_string(), "[]");
        assert_eq!(controller.failure().to_string(), "{}");
        assert!(controller.rust().retry_job.is_none());
    }
}
#[test]
fn a_step_outside_a_package_or_task_is_named_by_its_title() {
    let refresh = Operation::Refresh {
        backend: "apt".into(),
    };
    assert_eq!(
        short_name(&refresh, &Names::new()),
        operation_title_named(&refresh, &Names::new())
    );
    assert!(!short_name(&refresh, &Names::new()).is_empty());
}
#[test]
fn a_refused_password_is_urgent_and_a_step_that_works_clears_its_row() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    let package = synthetic_package("htop", "htop");
    let install = Operation::Install(package.id.clone());
    finish_write(
        &mut controller,
        Job::Write(install.clone(), None),
        Err(EngineError::Execution(
            pkgdeck_core::process::ExecutionError::AuthorizationDenied,
        )),
    );
    assert_eq!(parsed(controller.failure())["urgent"], true);
    let rows = parsed(controller.failed_rows());
    assert_eq!(rows[0]["target"]["name"], "htop");
    assert!(!rows[0]["reason"].as_str().unwrap().is_empty());
    // Another failure replaces it; a later success leaves it alone.
    finish_write(
        &mut controller,
        Job::Write(install.clone(), None),
        Err(EngineError::NotFound),
    );
    assert_eq!(parsed(controller.failure())["urgent"], false);
    finish_write(
        &mut controller,
        Job::Write(Operation::Remove(synthetic_package("vim", "vim").id), None),
        Ok(Payload::Written(OperationOutcome::default())),
    );
    assert_ne!(controller.failure().to_string(), "{}");
    assert_eq!(
        parsed(controller.failed_rows()).as_array().unwrap().len(),
        1
    );
    // Installed after all: the row and the failure clear.
    let mut installed = package.clone();
    installed.installed_version = Some("1".into());
    controller
        .as_mut()
        .apply(Ok(Payload::Packages(PackageReport {
            packages: vec![installed],
            failures: vec![],
            successful_sources: vec!["apt".into()],
        })));
    assert_eq!(controller.failed_rows().to_string(), "[]");
    assert_eq!(controller.failure().to_string(), "{}");
    // Retrying runs it again and forgets the failure.
    finish_write(
        &mut controller,
        Job::Write(install.clone(), None),
        Err(EngineError::NotFound),
    );
    controller.as_mut().retry_change();
    assert_eq!(controller.failure().to_string(), "{}");
    // Once it works, its row clears.
    finish_write(
        &mut controller,
        Job::Write(install, None),
        Ok(Payload::Written(OperationOutcome::default())),
    );
    assert_eq!(controller.failed_rows().to_string(), "[]");
}
#[test]
fn a_failed_cleanup_is_named_by_its_row_not_its_key() {
    let item = CleanupItem {
        id: CleanupId {
            backend: "projects".into(),
            key: "worktrees:/home/user/code/app".into(),
        },
        kind: CleanupKind::Worktree,
        title: "app worktrees".into(),
        summary: "3 finished worktrees".into(),
        preview: "Removes these worktrees.".into(),
        bytes: None,
    };
    let clean = Operation::Clean(item.id.clone());
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().cleanup = vec![item];
    let names = controller.rust().names_for(std::slice::from_ref(&clean));
    // Remembered at confirmation, so a refresh that drops the row
    // still names it.
    controller.as_mut().rust_mut().names.extend(names);
    controller.as_mut().rust_mut().cleanup.clear();
    let names = controller.rust().names_for(std::slice::from_ref(&clean));
    let notice = write_notice(
        &Job::CleanAll(vec![clean.clone()]),
        &Ok(ran(&[(
            &operation_title(&clean),
            Outcome::Failed,
            because("Couldn't remove ~/.trees/b: b is busy"),
        )])),
        false,
        &names,
    );
    assert_eq!(notice["title"], "Clean app worktrees failed");
    assert_eq!(
        notice["detail"],
        "app worktrees: Couldn't remove ~/.trees/b: b is busy"
    );
    // Without its row, the key still says which one.
    assert_eq!(
        operation_title(&clean),
        "Clean worktrees:/home/user/code/app (Old projects)"
    );
}
#[test]
fn automatic_runs_skip_updates_that_failed() {
    let mut tool = synthetic_package("anchor", "Anchor (AVM)");
    tool.id.backend = "anchor".into();
    tool.installed_version = Some("1".into());
    tool.candidate_version = Some("2.0".into());
    tool.update = UpdateAvailability::Available;
    let dir = std::env::temp_dir().join(format!("pkgdeck-auto-failed-{}", std::process::id()));
    let mut failed = FailedUpdates::at(&dir.join("failed-updates.json"));
    let plan = |failed: &FailedUpdates, packages: &[Package]| {
        automatic_plan(packages, &NeedsPassword::default(), failed)
    };
    assert_eq!(
        plan(&failed, &[tool.clone()]),
        [Operation::Upgrade(tool.id.clone())]
    );
    failed.remember(&[(tool.id.clone(), "2.0".into())], &[]);
    // Automatic runs never fall back to trying it again.
    assert!(plan(&failed, &[tool.clone()]).is_empty());
    let mut newer = tool.clone();
    newer.candidate_version = Some("2.1".into());
    assert_eq!(
        plan(&failed, &[newer]),
        [Operation::Upgrade(tool.id.clone())]
    );
    // An update that failed to the same version isn't run when checked
    // with others either.
    let identity = |p: &Package| {
        json!([
            p.id.backend,
            p.id.name,
            p.id.architecture,
            p.id.remote,
            p.id.scope
        ])
    };
    let mut other = tool.clone();
    other.id.name = "foundry".into();
    let checked = plan_checked_upgrade(
        &[tool.clone(), other.clone()],
        &json!([identity(&tool), identity(&other)]).to_string(),
        &failed,
    );
    assert_eq!(checked.operations, [Operation::Upgrade(other.id.clone())]);
    let _ = std::fs::remove_dir_all(dir);
}
/// APT whose dry run removes `removals`, and whose writes succeed.
struct AptPlanFixture {
    removals: Vec<String>,
}
impl Backend for AptPlanFixture {
    fn id(&self) -> &str {
        "apt"
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Installed, Capability::Upgrade]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn apt_upgrade_plan(&mut self, _: &Cancellation) -> Result<AptUpgradePlan, EngineError> {
        Ok(AptUpgradePlan {
            preview: "Inst synthetic".into(),
            upgrades: vec!["synthetic".into()],
            installs: vec![],
            removals: self.removals.clone(),
        })
    }
    fn execute(
        &mut self,
        _: &Operation,
        _: &Cancellation,
        _: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        Ok(OperationOutcome::default())
    }
}
#[test]
fn automatic_updates_hold_apt_back_when_it_would_remove_packages() {
    let all = |backend: &str| Operation::UpgradeAll {
        backend: backend.into(),
    };
    for (removals, allowed, apt_outcome) in [
        (vec!["old-kernel".to_owned()], false, Outcome::Failed),
        (vec!["old-kernel".to_owned()], true, Outcome::Finished),
        (vec![], false, Outcome::Finished),
    ] {
        let mut engine = Engine::default();
        engine
            .register(AptPlanFixture {
                removals: removals.clone(),
            })
            .unwrap();
        engine
            .register(Scripted {
                id: "homebrew",
                progress: vec![],
            })
            .unwrap();
        let replies = run_job(
            &mut engine,
            Job::AutoUpgrade(vec![all("apt"), all("homebrew")], allowed),
            &Cancellation::default(),
        );
        let ran = expect!(
            replies.into_iter().last(),
            Some(Reply::Done(Ok(Payload::Batch(ran)))) => ran
        );
        let status = ran.text(false);
        assert_eq!(
            ran.outcomes().collect::<Vec<_>>(),
            [apt_outcome, Outcome::Finished]
        );
        assert_eq!(
            status.contains("it would remove 1 package. Review it with Update all."),
            apt_outcome == Outcome::Failed,
            "{status}"
        );
    }
}
#[test]
fn only_automatic_runs_skip_the_password() {
    use pkgdeck_core::batch::BatchMode;
    let all = |backend: &str| Operation::UpgradeAll {
        backend: backend.into(),
    };
    // Update all is started by the person, so it asks.
    let manual = Job::UpgradeAll(vec![all("apt"), all("homebrew")], None);
    assert_eq!(batch_mode(&manual), None);
    let mut id = synthetic_package("synthetic", "Synthetic").id;
    id.backend = "dnf".into();
    assert_eq!(batch_mode(&Job::Write(Operation::Upgrade(id), None)), None);
    // An automatic run always uses the mode it was queued with.
    for removals in [false, true] {
        let auto = Job::AutoUpgrade(vec![all("apt")], removals);
        assert_eq!(batch_mode(&auto), Some(BatchMode::UpgradeOnly { removals }));
    }
}
#[test]
fn update_all_leaves_apt_out_when_removals_are_off() {
    let all = |backend: &str| Operation::UpgradeAll {
        backend: backend.into(),
    };
    let plan = AptUpgradePlan {
        preview: "Remv old-kernel".into(),
        upgrades: vec!["linux".into()],
        installs: vec![],
        removals: vec!["old-kernel".into()],
    };
    let available = |backend: &str, name: &str| {
        let mut package = synthetic_package(name, name);
        package.id.backend = backend.into();
        package.update = UpdateAvailability::Available;
        package
    };
    for allowed in [true, false] {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().set_allow_removals(allowed);
        controller.as_mut().rust_mut().packages =
            vec![available("apt", "linux"), available("homebrew", "wget")];
        controller.as_mut().apply(Ok(Payload::UpgradePreview(
            vec![all("apt"), all("homebrew")],
            2,
            Some(plan.clone()),
        )));
        let confirmation = controller.confirmation().to_string();
        let pending = expect!(
            &controller.rust().pending,
            Some(Job::UpgradeAll(operations, apt)) => (operations.clone(), apt.clone())
        );
        if allowed {
            assert_eq!(
                pending,
                (vec![all("apt"), all("homebrew")], Some(plan.clone()))
            );
            assert!(confirmation.starts_with("Update all 2 listed packages?"));
        } else {
            assert_eq!(pending, (vec![all("homebrew")], None));
            assert!(confirmation.starts_with("Update all 1 listed package?"));
            assert!(confirmation.contains("APT is left out: it would remove old-kernel."));
        }
    }
    // With nothing else to update, nothing waits for confirmation.
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().set_allow_removals(false);
    controller
        .as_mut()
        .apply(Ok(Payload::UpgradePreview(vec![all("apt")], 1, Some(plan))));
    assert!(controller.rust().pending.is_none());
    assert!(controller
        .status()
        .to_string()
        .starts_with("APT is left out"));
}
#[test]
fn automatic_runs_download_macos_updates_in_one_step() {
    let available = |backend: &str, name: &str| {
        let mut package = synthetic_package(name, name);
        package.id.backend = backend.into();
        package.update = UpdateAvailability::Available;
        package
    };
    let all = |backend: &str| Operation::UpgradeAll {
        backend: backend.into(),
    };
    assert_eq!(
        automatic_plan(
            &[
                available("macos-updates", "Safari"),
                available("homebrew", "wget"),
                available("macos-updates", "macOS Tahoe 26.1"),
            ],
            &NeedsPassword::default(),
            &FailedUpdates::default()
        ),
        [all("homebrew"), all("macos-updates")]
    );
    assert_eq!(
        automatic_plan(
            &[available("homebrew", "wget")],
            &NeedsPassword::default(),
            &FailedUpdates::default()
        ),
        [all("homebrew")]
    );
}
#[test]
fn automatic_runs_skip_casks_that_needed_the_password() {
    let cask = |name: &str, version: &str| {
        let mut package = synthetic_package(name, name);
        package.id.backend = "homebrew-cask".into();
        package.installed_version = Some("1".into());
        package.candidate_version = Some(version.into());
        package.update = UpdateAvailability::Available;
        package
    };
    let (mail, editor) = (cask("mail-app", "2"), cask("editor-app", "5"));
    let upgrade = |package: &Package| Operation::Upgrade(package.id.clone());
    let dir = std::env::temp_dir().join(format!("pkgdeck-auto-casks-{}", std::process::id()));
    let mut needs = NeedsPassword::at(&dir.join("needs-password.json"));
    // One at a time, so one cask's password stops only that cask.
    assert_eq!(
        automatic_plan(
            &[mail.clone(), editor.clone()],
            &needs,
            &FailedUpdates::default()
        ),
        [upgrade(&mail), upgrade(&editor)]
    );
    needs.remember(&[("mail-app".into(), "2".into())], &[]);
    assert_eq!(
        automatic_plan(
            &[mail.clone(), editor.clone()],
            &needs,
            &FailedUpdates::default()
        ),
        [upgrade(&editor)]
    );
    assert_eq!(
        automatic_plan(&[cask("mail-app", "3")], &needs, &FailedUpdates::default()),
        [upgrade(&cask("mail-app", "3"))]
    );

    let job = Job::AutoUpgrade(vec![upgrade(&mail), upgrade(&editor)], false);
    let names = Names::from([(mail.id.clone(), "Mail App".to_owned())]);
    let batch = |outcomes: Vec<Outcome>| Ok(ended(&outcomes));
    let notice = password_notice(
        &job,
        &batch(vec![Outcome::Failed, Outcome::Finished]),
        std::slice::from_ref(&mail.id),
        &names,
    )
    .unwrap();
    assert_eq!(notice["kind"], "info");
    assert_eq!(notice["title"], "Mail App needs your password to update");
    // Any other failure keeps the error banner.
    assert!(password_notice(
        &job,
        &batch(vec![Outcome::Failed, Outcome::Failed]),
        std::slice::from_ref(&mail.id),
        &names,
    )
    .is_none());
    let both = password_notice(
        &job,
        &batch(vec![Outcome::Failed, Outcome::Failed]),
        &[mail.id.clone(), editor.id.clone()],
        &names,
    )
    .unwrap();
    assert_eq!(both["title"], "2 apps need your password to update");
    assert!(password_notice(
        &job,
        &batch(vec![Outcome::Finished, Outcome::Finished]),
        &[],
        &names,
    )
    .is_none());
    let manual = Job::UpgradeAll(vec![upgrade(&mail)], None);
    assert!(password_notice(
        &manual,
        &batch(vec![Outcome::Failed]),
        std::slice::from_ref(&mail.id),
        &names
    )
    .is_none());
    let _ = std::fs::remove_dir_all(dir);
}
#[test]
fn background_checks_queue_only_what_may_update_unattended() {
    let available = |backend: &str, name: &str| {
        let mut package = synthetic_package(name, name);
        package.id.backend = backend.into();
        package.update = UpdateAvailability::Available;
        package.candidate_version = Some("2".into());
        package
    };
    let report = PackageReport {
        packages: vec![
            available("homebrew", "wget"),
            // No saved approval here, so system managers wait.
            available("apt", "synthetic"),
            available("fwupd", "firmware"),
            // Its check failed, so it may have more than it listed.
            available("cargo", "ripgrep"),
        ],
        failures: vec![BackendFailure {
            backend: "cargo".into(),
            error: EngineError::Cancelled,
        }],
        successful_sources: vec!["homebrew".into(), "apt".into(), "fwupd".into()],
    };
    for automatic in [false, true] {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        controller.as_mut().set_auto_update(automatic);
        controller.as_mut().set_allow_removals(automatic);
        controller.as_mut().finish_background_check(report.clone());
        let state: Value =
            serde_json::from_str(&controller.background_state().to_string()).unwrap();
        // What automatic updates leave for you is announced either way.
        assert_eq!(state["notify"], true);
        assert_eq!(
            state["manual"],
            if automatic { json!(3) } else { Value::Null }
        );
        let queued = controller
            .rust()
            .validated_confirmed
            .as_ref()
            .map(|entry| entry.job.operations());
        assert!(controller
            .rust()
            .validated_confirmed
            .as_ref()
            .is_none_or(|entry| matches!(entry.job, Job::AutoUpgrade(_, removals) if removals)));
        if automatic {
            assert_eq!(
                queued,
                Some(vec![Operation::UpgradeAll {
                    backend: "homebrew".into()
                }])
            );
        } else {
            assert_eq!(queued, None);
        }
    }
    // Nothing starts while a change waits for confirmation.
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().set_auto_update(true);
    controller.as_mut().rust_mut().pending = Some(Job::Load("Search".into(), String::new()));
    controller.as_mut().finish_background_check(report);
    assert!(controller.rust().validated_confirmed.is_none());
}
#[test]
fn automatic_updates_leave_the_sources_you_picked_for_you() {
    let available = |backend: &str, name: &str| {
        let mut package = synthetic_package(name, name);
        package.id.backend = backend.into();
        package.update = UpdateAvailability::Available;
        package.candidate_version = Some("2".into());
        package
    };
    let report = |packages: Vec<Package>| PackageReport {
        packages,
        failures: vec![],
        successful_sources: vec!["homebrew".into(), "cargo".into()],
    };
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    controller.as_mut().set_auto_update(true);
    controller.as_mut().set_manual_sources(vec!["cargo".into()]);
    let both = report(vec![
        available("homebrew", "wget"),
        available("cargo", "ripgrep"),
        available("cargo", "bat"),
    ]);
    assert_eq!(controller.as_mut().schedule_auto_update(&both), Some(2));
    let queued = controller
        .rust()
        .validated_confirmed
        .as_ref()
        .map(|entry| entry.job.operations());
    assert_eq!(
        queued,
        Some(vec![Operation::UpgradeAll {
            backend: "homebrew".into()
        }])
    );
    // Only sources left for you: nothing runs, and the check tells.
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    controller.as_mut().set_auto_update(true);
    controller
        .as_mut()
        .set_manual_sources(vec!["homebrew".into()]);
    controller
        .as_mut()
        .finish_background_check(report(vec![available("homebrew", "wget")]));
    let state: Value = serde_json::from_str(&controller.background_state().to_string()).unwrap();
    assert!(controller.rust().validated_confirmed.is_none());
    assert_eq!(state["notify"], true);
    assert_eq!(state["manual"], Value::Null);
    // Everything installs on its own: the run tells when it's done.
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    controller.as_mut().set_auto_update(true);
    controller
        .as_mut()
        .finish_background_check(report(vec![available("homebrew", "wget")]));
    let state: Value = serde_json::from_str(&controller.background_state().to_string()).unwrap();
    assert!(controller.rust().validated_confirmed.is_some());
    assert_eq!(state["notify"], false);
    assert_eq!(state["manual"], 0);
}
/// Fakes that count only what `waits` asks for, like the real check.
fn on_battery(_: &pkgdeck_core::host::Host, waits: Waits) -> Option<Hold> {
    waits.battery.then_some(Hold::Battery)
}
fn on_metered(_: &pkgdeck_core::host::Host, waits: Waits) -> Option<Hold> {
    waits.metered.then_some(Hold::Metered)
}
fn building(_: &pkgdeck_core::host::Host, _: Waits) -> Option<Hold> {
    Some(Hold::Build)
}
#[test]
fn turned_off_waits_let_updates_run_and_cleaning_ignores_metered() {
    let mut wget = synthetic_package("wget", "wget");
    wget.id.backend = "homebrew".into();
    wget.update = UpdateAvailability::Available;
    wget.candidate_version = Some("2".into());
    let report = PackageReport {
        packages: vec![wget],
        failures: vec![],
        successful_sources: vec!["homebrew".into()],
    };
    for (hold, battery, metered, runs) in [
        (on_battery as fn(&_, _) -> _, true, true, false),
        (on_battery, false, true, true),
        (on_metered, true, true, false),
        (on_metered, true, false, true),
    ] {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().prefetch.clear();
        controller.as_mut().set_auto_update(true);
        controller.as_mut().set_waits(battery, metered);
        assert_eq!(controller.waits(), Waits { battery, metered });
        controller.as_mut().rust_mut().natives.hold = hold;
        let left = controller.as_mut().schedule_auto_update(&report);
        assert_eq!(left.is_some(), runs, "{battery} {metered}");
    }
    // Cleaning runs on a metered connection, and waits for the charger.
    let cleans = |hold: fn(&pkgdeck_core::host::Host, Waits) -> Option<Hold>, battery| {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        controller.as_mut().rust_mut().natives.hold = hold;
        controller.as_mut().set_waits(battery, true);
        controller
            .as_mut()
            .set_auto_clean(true, epoch_seconds() - 25 * 60 * 60);
        controller.as_mut().schedule_auto_clean();
        controller.rust().validated_confirmed.is_some()
    };
    assert!(cleans(on_metered, true));
    assert!(!cleans(on_battery, true));
    assert!(cleans(on_battery, false));
}
#[test]
fn automatic_updates_wait_for_power_and_builds_then_check_again() {
    let mut wget = synthetic_package("wget", "wget");
    wget.id.backend = "homebrew".into();
    wget.update = UpdateAvailability::Available;
    wget.candidate_version = Some("2".into());
    let report = PackageReport {
        packages: vec![wget],
        failures: vec![],
        successful_sources: vec!["homebrew".into()],
    };
    let state = |controller: &ffi::PackageController| -> Value {
        serde_json::from_str(&controller.background_state().to_string()).unwrap()
    };
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().prefetch.clear();
    controller.as_mut().set_auto_update(true);
    // The last check just started, so the next isn't due for a while.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(controller
        .as_mut()
        .rust_mut()
        .background_schedule
        .ready(now, true, false, false, false, false));

    // On battery, the updates wait and the check tells about them instead.
    controller.as_mut().rust_mut().natives.hold = on_battery;
    controller.as_mut().finish_background_check(report.clone());
    assert!(controller.rust().validated_confirmed.is_none());
    assert_eq!(state(&controller)["notify"], true);
    assert_eq!(state(&controller)["waiting"], "battery");

    // Plugged in during a build: still waiting, now for the build.
    controller.as_mut().rust_mut().natives.hold = building;
    controller
        .as_mut()
        .check_updates("homebrew".into(), true, false, false, false);
    assert!(controller.rust().worker.is_none());
    assert_eq!(state(&controller)["waiting"], "build");
    assert_eq!(state(&controller)["notify"], false);
    // Asking again with the same answer changes nothing.
    controller
        .as_mut()
        .check_updates("homebrew".into(), true, false, false, false);
    assert!(controller.rust().worker.is_none());
    assert_eq!(state(&controller)["waiting"], "build");

    // Once the build ends, PkgDeck checks again right away, but not
    // while a confirmation is open.
    controller.as_mut().rust_mut().natives.hold = never_hold;
    controller.as_mut().rust_mut().pending = Some(Job::CleanAll(vec![]));
    controller
        .as_mut()
        .check_updates("homebrew".into(), true, false, false, false);
    assert!(controller.rust().worker.is_none());
    controller.as_mut().rust_mut().pending = None;
    controller
        .as_mut()
        .check_updates("homebrew".into(), true, false, false, false);
    // The wait is over when that check finishes.
    assert_eq!(controller.rust().auto_held, Some(Hold::Build));
    assert_eq!(state(&controller)["waiting"], "build");
    assert!(matches!(
        controller.rust().worker.as_ref().map(|worker| &worker.job),
        Some(Job::BackgroundUpdates(sources)) if sources == &["homebrew"]
    ));
    settle(&mut controller);
    controller.as_mut().finish_background_check(report.clone());
    assert!(controller.rust().validated_confirmed.is_some());
    assert_eq!(state(&controller)["waiting"], Value::Null);

    // Turning automatic updates off forgets the wait, so nothing checks
    // before the interval.
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().set_auto_update(true);
    controller.as_mut().rust_mut().natives.hold = building;
    assert!(controller
        .as_mut()
        .rust_mut()
        .background_schedule
        .ready(now, true, false, false, false, false));
    controller.as_mut().finish_background_check(report);
    assert_eq!(controller.rust().auto_held, Some(Hold::Build));
    controller.as_mut().set_auto_update(false);
    assert_eq!(controller.rust().auto_held, None);
    controller.as_mut().rust_mut().natives.hold = never_hold;
    controller
        .as_mut()
        .check_updates("homebrew".into(), true, false, false, false);
    assert!(controller.rust().worker.is_none());
}
fn run_job(engine: &mut Engine, job: Job, cancel: &Cancellation) -> Vec<Reply> {
    let mut replies = Vec::new();
    execute(engine, job, cancel, &mut |reply| replies.push(reply));
    replies
}
#[test]
fn jobs_without_a_worker_engine_path_answer_directly() {
    let mut engine = Engine::default();
    engine
        .register(Scripted {
            id: "fwupd",
            progress: vec![
                Progress::Transfer {
                    completed: 5,
                    total: Some(10),
                },
                Progress::Transfer {
                    completed: 7,
                    total: None,
                },
                Progress::Package("synthetic-firmware".into()),
            ],
        })
        .unwrap();
    let cancel = Cancellation::default();
    let mut firmware = synthetic_package("synthetic-firmware", "Firmware").id;
    firmware.backend = "fwupd".into();
    let replies = run_job(
        &mut engine,
        Job::Write(Operation::Upgrade(firmware.clone()), None),
        &cancel,
    );
    let texts: Vec<_> = replies
        .iter()
        .filter_map(|reply| match reply {
            Reply::Progress(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        [
            "Transferring 5 of 10…",
            "Transferring 7…",
            "Updating synthetic-firmware…"
        ]
    );
    assert!(matches!(
        replies.last(),
        Some(Reply::Done(Ok(Payload::Batch(ran))))
            if ran.text(false).ends_with("Restart or shut down the device if the update asked for it.")
                && ran.outcomes().eq([Outcome::Finished])
    ));

    // A failed source makes an export incomplete, so nothing is written.
    let path = std::env::temp_dir().join(format!(
        "pkgdeck-gui-partial-export-{}.json",
        std::process::id()
    ));
    let replies = run_job(
        &mut engine,
        Job::ManifestExport(path.clone(), vec![]),
        &cancel,
    );
    assert!(matches!(
        replies.last(),
        Some(Reply::Done(Err(EngineError::Execution(ExecutionError::Invalid(reason)))))
            if reason == "some package sources could not be read; retry the export"
    ));
    assert!(!path.exists());

    let upgrade = |backend: &str| Operation::UpgradeAll {
        backend: backend.into(),
    };
    let replies = run_job(
        &mut engine,
        Job::PlanUpgrade(vec![upgrade("fwupd")], 3),
        &cancel,
    );
    assert!(matches!(
        replies.last(),
        Some(Reply::Done(Ok(Payload::UpgradePreview(operations, 3, None))))
            if operations == &[upgrade("fwupd")]
    ));
    let replies = run_job(
        &mut engine,
        Job::PlanUpgrade(vec![upgrade("apt")], 1),
        &cancel,
    );
    assert!(matches!(
        replies.last(),
        Some(Reply::Done(Err(EngineError::UnknownBackend(backend)))) if backend == "apt"
    ));
    let clean = vec![Operation::Clean(CleanupId {
        backend: "fwupd".into(),
        key: "cache".into(),
    })];
    let replies = run_job(&mut engine, Job::PlanCleanAll(clean.clone()), &cancel);
    assert!(matches!(
        replies.last(),
        Some(Reply::Done(Ok(Payload::CleanPreview(operations, items))))
            if operations == &clean && items.is_empty()
    ));
    for job in [
        Job::Repositories(None),
        Job::OpenInput("relative.deb".into()),
    ] {
        let replies = run_job(&mut engine, job, &cancel);
        assert!(matches!(
            replies.as_slice(),
            [Reply::Done(Err(
                EngineError::NotFound | EngineError::InvalidResponse { .. }
            ))]
        ));
    }

    let replies = run_job(
        &mut engine,
        Job::UpgradeAll(vec![Operation::Upgrade(firmware.clone())], None),
        &cancel,
    );
    assert!(matches!(
        replies.last(),
        Some(Reply::Done(Ok(Payload::Batch(ran))))
            if ran.steps[0].said == Said::Completed && ran.outcomes().eq([Outcome::Finished])
    ));
    cancel.cancel();
    let replies = run_job(
        &mut engine,
        Job::UpgradeAll(vec![Operation::Upgrade(firmware)], None),
        &cancel,
    );
    assert!(matches!(
        replies.last(),
        Some(Reply::Done(Ok(Payload::Batch(ran)))) if ran.outcomes().eq([Outcome::Cancelled])
    ));
}
#[test]
fn inventory_picker_rejects_remote_paths_and_bad_selection() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .export_inventory(QUrl::from("https://example.invalid/list.json"), "[]".into());
    assert!(controller.status().to_string().contains("local inventory"));
    controller
        .as_mut()
        .preview_inventory(QUrl::from("https://example.invalid/list.json"));
    assert!(controller.status().to_string().contains("local inventory"));
    let file = QUrl::from_local_file(&"/tmp/synthetic-list.json".into());
    controller.as_mut().export_inventory(file, "broken".into());
    assert!(controller
        .status()
        .to_string()
        .contains("Invalid package selection"));
    controller
        .as_mut()
        .apply(Ok(Payload::ManifestPreview(manifest::Preview {
            schema_version: manifest::SCHEMA_VERSION,
            packages: vec![],
        })));
    assert!(controller
        .manifest_preview()
        .to_string()
        .contains("packages"));
    controller.as_mut().apply(Ok(Payload::ManifestExport(2)));
    assert!(controller
        .status()
        .to_string()
        .contains("Exported 2 packages"));

    // A foreground picker request can preempt a background read without
    // touching the host while it waits for the worker to wind down.
    let (sender, receiver) = mpsc::channel();
    drop(sender);
    let cancellation = Cancellation::default();
    controller.as_mut().rust_mut().background = true;
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(|| {}),
        receiver,
        cancel: cancellation.clone(),
        job: Job::Load("Installed".into(), String::new()),
    });
    let local = QUrl::from_local_file(&"/tmp/synthetic-list.json".into());
    controller.as_mut().preview_inventory(local.clone());
    assert!(matches!(
        controller.rust().queued,
        Some(Job::ManifestPreview(_))
    ));
    assert!(cancellation.requested());
    controller.as_mut().export_inventory(local, "[]".into());
    assert!(matches!(
        controller.rust().queued,
        Some(Job::ManifestExport(_, _))
    ));
    controller
        .as_mut()
        .rust_mut()
        .worker
        .take()
        .unwrap()
        .handle
        .join()
        .unwrap();
    controller.as_mut().rust_mut().queued = None;
}
fn cached_view(name: &str) -> CachedView {
    CachedView {
        loaded: Instant::now(),
        expired: false,
        cleanup: vec![],
        packages: vec![],
        failures: vec![],
        sources: vec![],
        rows: format!(r#"[{{"name":"{name}"}}]"#).as_str().into(),
        status: "cached".into(),
        report_state: r#"{"phase":"cached"}"#.into(),
        upgradable: false,
        updates_view: false,
    }
}
#[test]
fn cache_keys_separate_views_queries_sources_and_elevation() {
    let apt = vec!["apt".to_string()];
    let all: Vec<String> = vec![];
    assert_eq!(
        cache_key("Installed", "", &all, false),
        cache_key("Installed", "", &all, false)
    );
    assert_ne!(
        cache_key("Installed", "", &all, false),
        cache_key("Updates", "", &all, false)
    );
    assert_ne!(
        cache_key("Search", "fire", &all, false),
        cache_key("Search", "firefox", &all, false)
    );
    assert_ne!(
        cache_key("Installed", "", &all, false),
        cache_key("Installed", "", &apt, false)
    );
    assert_ne!(
        cache_key("Installed", "", &all, false),
        cache_key("Installed", "", &all, true)
    );
}
#[test]
fn view_cache_replaces_evicts_oldest_and_expires() {
    let mut cache = ViewCache { entries: vec![] };
    assert!(cache.get("missing").is_none());
    cache.insert("a".into(), cached_view("a"));
    cache.insert("b".into(), cached_view("b"));
    assert!(cache.get("a").is_some());
    // Re-inserting a key replaces it without growing the cache.
    cache.insert("a".into(), cached_view("a2"));
    assert_eq!(cache.entries.len(), 2);
    assert!(cache.get("a").is_some());
    for i in 0..ViewCache::CAPACITY {
        cache.insert(format!("k{i}"), cached_view("x"));
    }
    assert_eq!(cache.entries.len(), ViewCache::CAPACITY);
    // Oldest ("b", then "a") evicted first.
    assert!(cache.get("b").is_none());
    assert!(cache.get("a").is_none());
    cache.expire();
    assert!(cache.entries.iter().all(|(_, view)| view.stale()));
    assert_eq!(cache.entries.len(), ViewCache::CAPACITY);
}
#[test]
fn friendly_detail_names_preserve_exact_identity_and_inventory_state() {
    let original: Package = serde_json::from_value(json!({
        "id": {"backend":"snap", "name":"player", "architecture":"all", "scope":"system"},
        "display_name":"player", "summary":"Local summary", "installed_version":"1",
        "candidate_version":"2", "update":"available"
    }))
    .unwrap();
    let mut packages = vec![original.clone()];
    let rows = serde_json::to_string(&vec![package_row(&original, &[], None)]).unwrap();
    let mut detail = original.clone();
    detail.display_name = "Friendly Player".into();
    detail.installed_version = None;
    detail.candidate_version = Some("99".into());
    let updated = update_detail_name(&mut packages, &rows, &detail).unwrap();
    assert_eq!(updated[0]["display_name"], "Friendly Player");
    assert_eq!(updated[0]["installed"], "1");
    assert_eq!(updated[0]["candidate"], "2");
    let mut expected = original.clone();
    expected.display_name = "Friendly Player".into();
    assert_eq!(packages, [expected]);
    assert!(update_detail_name(&mut packages, &rows, &detail).is_none());
    packages[0] = original.clone();
    detail.id.scope = Scope::User { uid: 1000 };
    assert!(update_detail_name(&mut packages, &rows, &detail).is_none());
    detail.id = original.id.clone();
    for malformed in ["broken", "[]", "[null]"] {
        assert!(update_detail_name(&mut packages, malformed, &detail).is_none());
        assert_eq!(packages[0], original);
    }
    detail.display_name.clear();
    assert!(update_detail_name(&mut packages, &rows, &detail).is_none());
}
#[test]
fn controller_version_tracks_package_metadata() {
    assert_eq!(
        Controller::default().version.to_string(),
        pkgdeck_core::VERSION
    );
}
#[test]
fn installed_view_filters_by_query_while_other_views_ignore_it() {
    fn load(engine: &mut Engine, view: &str, query: &str) -> PackageReport {
        let mut replies = vec![];
        execute(
            engine,
            Job::Load(view.into(), query.into()),
            &Cancellation::default(),
            &mut |r| replies.push(r),
        );
        expect!(replies.pop().unwrap(), Reply::Done(Ok(Payload::Packages(report))) => report)
    }
    let package = Package {
        id: PackageId {
            backend: "fixture".into(),
            name: "synthetic".into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        },
        display_name: "Synthetic".into(),
        summary: "Fixture".into(),
        installed_version: Some("1".into()),
        candidate_version: Some("2".into()),
        update: UpdateAvailability::Available,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
        adopt_with: None,
    };
    let mut engine = Engine::default();
    engine
        .register(Fixture {
            package: package.clone(),
            fail: false,
        })
        .unwrap();
    assert_eq!(load(&mut engine, "Installed", "").packages.len(), 1);
    assert_eq!(
        load(&mut engine, "Installed", "synthetic").packages.len(),
        1
    );
    assert_eq!(load(&mut engine, "Installed", "FIXTURE").packages.len(), 1);
    assert!(load(&mut engine, "Installed", "missing")
        .packages
        .is_empty());
    // Updates never narrows by the query; stale field text is ignored.
    assert_eq!(load(&mut engine, "Updates", "missing").packages.len(), 1);
    // Search must exclude install placeholders in every streamed report,
    // but retain genuine catalog entries with an empty version label.
    for candidate in [None, Some(""), Some("2")] {
        let mut offer = package.clone();
        offer.installed_version = None;
        offer.candidate_version = candidate.map(str::to_owned);
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                package: offer,
                fail: false,
            })
            .unwrap();
        let mut reports = Vec::new();
        execute(
            &mut engine,
            Job::Load("Search".into(), "synthetic".into()),
            &Cancellation::default(),
            &mut |reply| {
                if let Reply::Partial(report) | Reply::Done(Ok(Payload::Packages(report))) = reply {
                    reports.push(report);
                }
            },
        );
        assert!(reports.len() >= 2);
        assert!(reports
            .iter()
            .all(|report| report.packages.len() == usize::from(candidate.is_some())));
    }
}
#[test]
fn checked_identities_resolve_only_to_upgradable_packages() {
    fn package(name: &str, installed: bool, update: UpdateAvailability) -> Package {
        Package {
            id: PackageId {
                backend: "fixture".into(),
                name: name.into(),
                architecture: "all".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: name.into(),
            summary: "Fixture".into(),
            installed_version: installed.then(|| "1".into()),
            candidate_version: Some("2".into()),
            update,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        }
    }
    let packages = vec![
        package("upgradable", true, UpdateAvailability::Available),
        package("current", true, UpdateAvailability::Current),
        package("uninstalled", false, UpdateAvailability::Available),
    ];
    // Identity shape mirrors QML rowIdentity: [source, name, arch, remote, scope, reference].
    let row = |name: &str| {
        vec![
            serde_json::json!("fixture"),
            serde_json::json!(name),
            serde_json::json!("all"),
            serde_json::json!(null),
            serde_json::json!("system"),
            serde_json::json!(null),
        ]
    };
    let identities = serde_json::to_string(&vec![
        row("upgradable"),
        row("current"),
        row("uninstalled"),
        row("vanished"),
        vec![serde_json::json!("bogus")],
    ])
    .unwrap();
    let operations = checked_upgrades(&packages, &identities);
    assert_eq!(operations.len(), 1);
    assert!(matches!(&operations[0], Operation::Upgrade(id) if id.name == "upgradable"));
    assert!(checked_upgrades(&packages, "not json").is_empty());
    let mut runtime = package("org.example.Platform", true, UpdateAvailability::Available);
    runtime.id.reference = Some("runtime/org.example.Platform/all/stable".into());
    let mut identity = row("org.example.Platform");
    identity[5] = serde_json::json!(runtime.id.reference);
    let encoded = serde_json::to_string(&vec![identity.clone()]).unwrap();
    assert_eq!(
        checked_upgrades(&[runtime.clone()], &encoded),
        vec![Operation::Upgrade(runtime.id.clone())]
    );
    // A reference that is not text, or trailing parts, never matches.
    let mut numbered = identity.clone();
    numbered[5] = serde_json::json!(7);
    let mut longer = identity.clone();
    longer.push(serde_json::json!("extra"));
    let malformed = serde_json::to_string(&vec![numbered, longer]).unwrap();
    assert!(checked_upgrades(&[runtime.clone()], &malformed).is_empty());
    identity[5] = serde_json::json!("runtime/org.example.Platform/all/beta");
    assert!(
        checked_upgrades(&[runtime], &serde_json::to_string(&vec![identity]).unwrap()).is_empty()
    );
}
#[test]
fn rows_made_on_the_worker_match_rows_made_here() {
    let package = |name: &str| Package {
        id: PackageId {
            backend: "fixture".into(),
            name: name.into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        },
        display_name: name.into(),
        summary: "Fixture".into(),
        installed_version: Some("1".into()),
        candidate_version: Some("1".into()),
        update: UpdateAvailability::Current,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
        adopt_with: None,
    };
    let packages = vec![package("one"), package("two")];
    let made_here = serde_json::to_string(&package_rows(&packages)).unwrap();
    let mut cache = RowsCache::default();
    assert_eq!(cache.json(&packages), made_here);
    // Rows from a worker are used only for exactly the same packages.
    let mut prepared = PreparedRows::new(&packages);
    prepared.json = "[\"from the worker\"]".into();
    cache.insert(*prepared);
    assert_eq!(cache.json(&packages), "[\"from the worker\"]");
    let mut changed = packages.clone();
    changed[1].installed_version = Some("2".into());
    assert_ne!(cache.json(&changed), "[\"from the worker\"]");
    // Only the newest few are kept.
    for name in ["a", "b", "c", "d"] {
        cache.insert(*PreparedRows::new(&[package(name)]));
    }
    assert_eq!(cache.json(&packages), made_here);
    // A snapshot over the budget pushes every older one out, so one huge
    // list is held once and never four times.
    let mut huge = PreparedRows::new(&[package("huge")]);
    huge.json = "x".repeat(RowsCache::BUDGET);
    cache.insert(*huge);
    let mut small = PreparedRows::new(&[package("small")]);
    small.json = "[\"small\"]".into();
    cache.insert(*small);
    assert_eq!(cache.json(&[package("small")]), "[\"small\"]");
    assert_eq!(cache.0.len(), 1);
    // Failure rows go after the package rows.
    let failure = [json!({"kind": "failure"})];
    assert_eq!(
        append_rows("[]".into(), &failure),
        r#"[{"kind":"failure"}]"#
    );
    assert_eq!(
        append_rows("[1]".into(), &failure),
        r#"[1,{"kind":"failure"}]"#
    );
    assert_eq!(append_rows("[1]".into(), &[]), "[1]");
}
#[test]
fn a_batch_of_partials_shows_only_the_newest() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let row = |name: &str| {
        let mut row = fixture_row();
        row.id.name = name.into();
        row
    };
    let report = |names: &[&str]| PackageReport {
        packages: names.iter().map(|name| row(name)).collect(),
        failures: vec![],
        successful_sources: vec![],
    };
    let (worker, gate) = held_worker(
        Job::Load("Installed".into(), String::new()),
        vec![
            Reply::Rows(PreparedRows::new(&report(&["first"]).packages)),
            Reply::Partial(report(&["first"])),
            Reply::Rows(PreparedRows::new(&report(&["first", "second"]).packages)),
            Reply::Partial(report(&["first", "second"])),
        ],
    );
    controller.as_mut().rust_mut().worker = Some(worker);
    let shown = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = shown.clone();
    controller
        .as_mut()
        .on_rows_changed(move |_| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })
        .release();
    controller.as_mut().poll();
    assert_eq!(shown.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(controller.rows().to_string().contains("second"));
    drop(gate);
    settle(&mut controller);

    // A read that ended without its final report still shows its
    // newest rows.
    let worker = fake_worker(
        Job::Load("Installed".into(), String::new()),
        vec![
            Reply::Partial(report(&["third"])),
            Reply::Partial(report(&["third", "fourth"])),
        ],
    );
    while !worker.handle.is_finished() {
        thread::yield_now();
    }
    controller.as_mut().rust_mut().worker = Some(worker);
    controller.as_mut().poll();
    assert!(controller.rust().worker.is_none());
    assert!(controller.rows().to_string().contains("fourth"));
}
#[test]
fn huge_searches_keep_only_the_best_matches() {
    let package = |name: String| Package {
        id: PackageId {
            backend: "fixture".into(),
            name,
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        },
        display_name: String::new(),
        summary: "Fixture".into(),
        installed_version: None,
        candidate_version: Some("1".into()),
        update: UpdateAvailability::Unknown,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
        adopt_with: None,
    };
    let mut report = PackageReport {
        packages: (0..2000)
            .map(|i| package(format!("lib-{i:04}-v")))
            .chain([package("v".into()), package("vlc".into())])
            .collect(),
        ..PackageReport::default()
    };
    assert_eq!(keep_best_matches(&mut report, "V "), 2002);
    assert_eq!(report.packages.len(), SEARCH_ROW_LIMIT);
    // The exact name and a prefix match come first, as the list ranks.
    let names: Vec<_> = report.packages.iter().map(|p| p.id.name.as_str()).collect();
    assert_eq!(names[..3], ["v", "vlc", "lib-0000-v"]);
    // Smaller searches are left as they are.
    let mut small = PackageReport {
        packages: vec![package("vlc".into()), package("v".into())],
        ..PackageReport::default()
    };
    assert_eq!(keep_best_matches(&mut small, "v"), 2);
    let names: Vec<_> = small.packages.iter().map(|p| p.id.name.as_str()).collect();
    assert_eq!(names, ["vlc", "v"]);

    // The page says how many matched in all.
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().natives = Natives {
        warm_catalog: true,
        ..SYNTHETIC
    };
    controller
        .as_mut()
        .load("Search".into(), "v".into(), "".into(), false, false);
    settle(&mut controller);
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Load("Search".into(), "v".into()),
        vec![
            Reply::Matches(2002),
            Reply::Done(Ok(Payload::Packages(report))),
        ],
    ));
    controller.as_mut().poll();
    let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
    assert_eq!(state["matches"], 2002);
    // A new search starts counting again.
    controller
        .as_mut()
        .start(Job::Load("Search".into(), "vl".into()));
    assert_eq!(controller.rust().search_matches, 0);
    settle(&mut controller);
}
#[test]
fn checked_proposals_plan_selected_upgrades() {
    fn package(name: &str) -> Package {
        Package {
            id: PackageId {
                backend: "fixture".into(),
                name: name.into(),
                architecture: "all".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: name.into(),
            summary: "Fixture".into(),
            installed_version: Some("1".into()),
            candidate_version: Some("2".into()),
            update: UpdateAvailability::Available,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        }
    }
    // Identity shape mirrors QML rowIdentity: [source, name, arch, remote, scope, reference].
    let row = |name: &str| {
        serde_json::to_string(&vec![
            serde_json::json!("fixture"),
            serde_json::json!(name),
            serde_json::json!("all"),
            serde_json::json!(null),
            serde_json::json!("system"),
            serde_json::json!(null),
        ])
        .unwrap()
    };
    let packages = vec![package("upgradable")];
    // Stale identities resolve to nothing: empty confirmation, status set.
    let empty = plan_checked_upgrade(
        &packages,
        &format!("[{}]", row("vanished")),
        &FailedUpdates::default(),
    );
    assert!(empty.operations.is_empty());
    assert!(empty.confirmation.is_empty());
    assert_eq!(
        empty.status.as_deref(),
        Some("No selected packages can be updated.")
    );
    // One live identity plans a single-upgrade batch with confirmation.
    let plan = plan_checked_upgrade(
        &packages,
        &format!("[{}, {}]", row("upgradable"), row("vanished")),
        &FailedUpdates::default(),
    );
    assert_eq!(plan.operations.len(), 1);
    assert!(matches!(&plan.operations[0], Operation::Upgrade(id) if id.name == "upgradable"));
    assert!(plan.status.is_none());
    assert!(plan.confirmation.contains("Update 1 selected package?"));
    assert!(plan.confirmation.contains("Update upgradable"));
    // The dialog lists the package itself, with its versions, not a
    // repeated sentence.
    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0]["title"], "upgradable");
    assert_eq!(plan.items[0]["group"], "fixture");
    assert_eq!(plan.items[0]["from"], "1");
    assert_eq!(plan.items[0]["to"], "2");
    // Update all lists every package its sources would update.
    let all = update_items(
        &[Operation::UpgradeAll {
            backend: "fixture".into(),
        }],
        &[package("upgradable"), package("other")],
    );
    assert_eq!(all.len(), 2);
}
#[test]
fn an_apt_confirmation_says_only_what_its_rows_do_not() {
    let mut plan = AptUpgradePlan {
        preview: String::new(),
        upgrades: vec!["libavcodec62".into()],
        installs: vec![],
        removals: vec![],
    };
    assert_eq!(apt_extra(&plan), "");
    plan.installs = vec!["libnew1".into()];
    plan.removals = vec!["libold1".into(), "libold2".into()];
    assert_eq!(
        apt_extra(&plan),
        "\n\nInstall (1): libnew1\nRemove (2): libold1, libold2"
    );
    let build = |architecture: &str| Package {
        id: PackageId {
            backend: "apt".into(),
            name: "libavcodec62".into(),
            architecture: architecture.into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        },
        display_name: String::new(),
        summary: String::new(),
        installed_version: Some("1".into()),
        candidate_version: Some("2".into()),
        update: UpdateAvailability::Available,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
        adopt_with: None,
    };
    let items = update_items(
        &[Operation::UpgradeAll {
            backend: "apt".into(),
        }],
        &[build("amd64"), build("i386")],
    );
    assert_eq!(items[0]["title"], "libavcodec62 (amd64)");
    assert_eq!(items[1]["title"], "libavcodec62 (i386)");
    // Only updates become rows: a refresh or a removal in the batch adds none.
    let mixed = update_items(
        &[
            Operation::Refresh {
                backend: "apt".into(),
            },
            Operation::Remove(build("amd64").id),
            Operation::Upgrade(build("i386").id),
        ],
        &[build("amd64"), build("i386")],
    );
    assert_eq!(mixed.len(), 1);
    assert_eq!(mixed[0]["title"], "libavcodec62");
}
#[test]
fn a_cleanup_confirmation_lists_each_task_with_its_size_and_preview() {
    let item = |title: &str, bytes: Option<u64>| CleanupItem {
        id: CleanupId {
            backend: "fixture".into(),
            key: title.into(),
        },
        kind: CleanupKind::PackageCache,
        title: title.into(),
        summary: String::new(),
        preview: format!("Would empty {title}"),
        bytes,
    };
    let (cache, logs) = (item("cache", Some(2_000_000_000)), item("logs", None));
    let data = clean_confirmation(&[&cache, &logs]);
    let items = data["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["group"], "fixture");
    assert_eq!(items[0]["size"], "2.0 GB");
    assert_eq!(items[0]["preview"], "Would empty cache");
    assert!(items[1]["size"].is_null());
    assert!(data["details"].is_null());
    // The queued-revalidation path reads this as the confirmation text.
    assert_eq!(
        data["labels"].as_str().unwrap(),
        format!("{}\n\n{}", cleanup_label(&cache), cleanup_label(&logs))
    );
}
#[test]
fn failure_details_carry_backend_error_and_hint() {
    let failure = BackendFailure {
        backend: "npm".into(),
        error: EngineError::InvalidResponse {
            backend: "npm".into(),
            reason: "npm ls failed: boom".into(),
        },
    };
    let text = failure_details(&failure, false).to_string();
    assert!(text.contains("npm ls failed: boom"));
    assert!(text.contains("Sources view"));
}
#[test]
fn details_jobs_scope_their_own_backend() {
    let id = PackageId {
        backend: "homebrew".into(),
        name: "synthetic".into(),
        architecture: "all".into(),
        scope: Scope::System,
        remote: None,
        reference: None,
    };
    assert_eq!(
        engine_source(&Job::Details(id.clone()), &[]),
        vec!["homebrew".to_string()]
    );
    assert_eq!(
        engine_source(&Job::Details(id), &["apt".to_string()]),
        vec!["homebrew".to_string()]
    );
    assert_eq!(
        engine_source(
            &Job::Load("Installed".into(), "".into()),
            &["apt".to_string()]
        ),
        vec!["apt".to_string()]
    );
    assert_eq!(
        engine_source(&Job::Load("Installed".into(), "".into()), &[]),
        Vec::<String>::new()
    );
    assert_eq!(
        engine_source(
            &Job::RetrySource("Updates".into(), "".into(), "apt".into()),
            &["apt".into(), "npm".into()]
        ),
        vec!["apt".to_string()]
    );
}
#[test]
fn stale_details_replies_never_take_the_panel() {
    let id = PackageId {
        backend: "fixture".into(),
        name: "synthetic".into(),
        architecture: "all".into(),
        scope: Scope::System,
        remote: None,
        reference: None,
    };
    let mut other = id.clone();
    other.name = "other".into();
    let details = PackageDetails {
        package: Package {
            id: id.clone(),
            display_name: "Synthetic".into(),
            summary: "Fixture".into(),
            installed_version: None,
            candidate_version: None,
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        },
        description: String::new(),
        homepage: None,
        dependencies: vec![],
    };
    assert!(details_fresh(Some(&id), &details));
    assert!(!details_fresh(None, &details));
    assert!(!details_fresh(Some(&other), &details));
}
#[test]
fn loads_stream_partials_before_the_terminal_report() {
    let package = Package {
        id: PackageId {
            backend: "fixture".into(),
            name: "synthetic".into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        },
        display_name: "Synthetic".into(),
        summary: "Fixture".into(),
        installed_version: Some("1".into()),
        candidate_version: Some("2".into()),
        update: UpdateAvailability::Available,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
        adopt_with: None,
    };
    let mut engine = Engine::default();
    engine
        .register(Fixture {
            package: package.clone(),
            fail: false,
        })
        .unwrap();
    let mut replies = vec![];
    execute(
        &mut engine,
        Job::Load("Search".into(), "synthetic".into()),
        &Cancellation::default(),
        &mut |r| replies.push(r),
    );
    assert_eq!(replies.len(), 5);
    let asked = expect!(replies.remove(0), Reply::Asked(sources) => sources);
    assert_eq!(asked, ["fixture"]);
    // Each report says how many packages matched before it arrives.
    assert_eq!(
        expect!(replies.remove(0), Reply::Matches(count) => count),
        1
    );
    let partial = expect!(replies.remove(0), Reply::Partial(report) => report);
    assert_eq!(
        expect!(replies.remove(0), Reply::Matches(count) => count),
        1
    );
    let terminal = expect!(replies.remove(0), Reply::Done(Ok(Payload::Packages(report))) => report);
    assert_eq!(terminal, partial);
}
#[test]
fn installed_and_updates_loads_stream_filtered_partials() {
    let package = Package {
        id: PackageId {
            backend: "fixture".into(),
            name: "synthetic".into(),
            architecture: "all".into(),
            scope: Scope::System,
            remote: None,
            reference: None,
        },
        display_name: "Synthetic".into(),
        summary: "Fixture".into(),
        installed_version: Some("1".into()),
        candidate_version: Some("2".into()),
        update: UpdateAvailability::Available,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
        adopt_with: None,
    };
    let mut engine = Engine::default();
    engine
        .register(Fixture {
            package: package.clone(),
            fail: false,
        })
        .unwrap();
    // Both views stream one partial per backend, each carrying the
    // same view filter as the terminal report.
    for view in ["Installed", "Updates"] {
        let mut replies = vec![];
        execute(
            &mut engine,
            Job::Load(view.into(), "".into()),
            &Cancellation::default(),
            &mut |r| replies.push(r),
        );
        assert_eq!(replies.len(), 4);
        let asked = expect!(replies.remove(0), Reply::Asked(sources) => sources);
        assert_eq!(asked, ["fixture"]);
        let partial = expect!(replies.remove(0), Reply::Partial(report) => report);
        assert_eq!(partial.packages, vec![package.clone()]);
        assert!(matches!(
            replies.remove(0),
            Reply::Inventory(_, refreshed) if refreshed == (view == "Updates")
        ));
        let terminal =
            expect!(replies.remove(0), Reply::Done(Ok(Payload::Packages(report))) => report);
        assert_eq!(terminal, partial);
    }
}
#[test]
fn jobs_keep_source_identity_native_updates_and_typed_failures() {
    let id = PackageId {
        backend: "fixture".into(),
        name: "synthetic".into(),
        architecture: "all".into(),
        scope: Scope::System,
        remote: None,
        reference: None,
    };
    let package = Package {
        id: id.clone(),
        display_name: "Synthetic".into(),
        summary: "Fixture".into(),
        installed_version: Some("1".into()),
        candidate_version: Some("2".into()),
        update: UpdateAvailability::Available,
        icon: None,
        component_ids: vec![],
        homepages: vec![],
        adopt_with: None,
    };
    assert_eq!(package_row(&package, &[], None)["source"], "fixture");
    assert!(encoded(package_row(&package, &[], None))
        .to_string()
        .contains("synthetic"));
    let mut firmware = package.clone();
    firmware.id.backend = "fwupd".into();
    firmware.display_name = "Synthetic BIOS".into();
    firmware.summary = "Firmware, AC power required, Restart required".into();
    for tool in pkgdeck_core::backends::StandaloneTool::ALL {
        let mut standalone = package.clone();
        standalone.id.backend = tool.id().into();
        standalone.id.reference = Some("/synthetic/bin/tool".into());
        assert_eq!(
            upgrade_plan(&[standalone.clone()]),
            vec![Operation::Upgrade(standalone.id)]
        );
    }
    for backend in ["rustup", "pixi", "nix"] {
        let mut toolchain = package.clone();
        toolchain.id.backend = backend.into();
        assert_eq!(
            upgrade_plan(&[toolchain.clone()]),
            vec![Operation::Upgrade(toolchain.id)]
        );
    }
    let mixed = upgrade_plan(&[package.clone(), firmware.clone()]);
    assert!(mixed.contains(&Operation::Upgrade(firmware.id.clone())));
    let label = confirmation_label(
        &Operation::Upgrade(firmware.id.clone()),
        &[firmware.clone()],
    );
    assert!(label.contains("Synthetic BIOS"));
    assert!(label.contains("AC power required"));
    assert!(label.contains("Restart required"));
    let checked = plan_checked_upgrade(
        &[firmware.clone()],
        &json!([[
            firmware.id.backend,
            firmware.id.name,
            firmware.id.architecture,
            null,
            firmware.id.scope
        ]])
        .to_string(),
        &FailedUpdates::default(),
    );
    assert!(checked.confirmation.contains("Synthetic BIOS"));
    let mut firmware_engine = Engine::default();
    firmware_engine
        .register(Fixture {
            package: firmware.clone(),
            fail: false,
        })
        .unwrap();
    for job in [
        Job::Write(Operation::Upgrade(firmware.id.clone()), None),
        Job::UpgradeAll(vec![Operation::Upgrade(firmware.id.clone())], None),
    ] {
        let mut replies = vec![];
        execute(
            &mut firmware_engine,
            job,
            &Cancellation::default(),
            &mut |reply| replies.push(reply),
        );
        let status = expect!(
            replies.pop().unwrap(),
            Reply::Done(Ok(Payload::Batch(ran))) => ran.text(false)
        );
        assert!(status.contains("Restart or shut down"));
    }

    let mut other = package.clone();
    other.id.backend = "other-source".into();
    let mut current = package.clone();
    current.update = UpdateAvailability::Current;
    let mut uninstalled = package.clone();
    uninstalled.installed_version = None;
    let plan = upgrade_plan(&[
        package.clone(),
        package.clone(),
        other.clone(),
        current,
        uninstalled,
    ]);
    // One command for each source, however many packages it updates.
    assert_eq!(
        plan,
        vec![
            Operation::UpgradeAll {
                backend: id.backend.clone()
            },
            Operation::UpgradeAll {
                backend: other.id.backend.clone()
            }
        ]
    );
    for fail in [false, true] {
        let mut engine = Engine::default();
        engine
            .register(Fixture {
                package: package.clone(),
                fail,
            })
            .unwrap();
        for job in [
            Job::Load("Search".into(), "synthetic".into()),
            Job::Load("Installed".into(), "".into()),
            Job::Load("Updates".into(), "".into()),
            Job::Load("Sources".into(), "".into()),
            Job::Details(id.clone()),
            Job::Write(Operation::Upgrade(id.clone()), None),
            Job::UpgradeAll(
                vec![Operation::UpgradeAll {
                    backend: id.backend.clone(),
                }],
                None,
            ),
        ] {
            let mut replies = vec![];
            execute(
                &mut engine,
                job.clone(),
                &Cancellation::default(),
                &mut |r| replies.push(r),
            );
            match replies.pop().unwrap() {
                Reply::Done(Ok(Payload::Packages(report))) => {
                    assert_eq!(report.packages[0].id, id)
                }
                Reply::Done(Ok(Payload::Sources(sources))) => {
                    assert_eq!(sources[0].backend, "fixture")
                }
                Reply::Done(Ok(Payload::Batch(ran))) => {
                    let status = ran.text(false);
                    assert!(status.contains(if fail {
                        "Completed 0 of 1"
                    } else {
                        "Completed 1 of 1"
                    }));
                    assert!(status.contains("Update all fixture packages"));
                    assert!(status.contains(if fail { "administrator" } else { "cancel" }));
                }
                Reply::Done(Ok(Payload::Written(outcome))) => {
                    assert!(outcome.cancellation_deferred)
                }
                reply => {
                    let expected = if matches!(job, Job::Details(_)) {
                        EngineError::NotFound
                    } else {
                        pkgdeck_core::process::ExecutionError::AuthorizationDenied.into()
                    };
                    assert!(
                        matches!(&reply, Reply::Done(Err(error)) if *error == expected),
                        "missing terminal reply"
                    );
                }
            }
        }
    }
}
fn fake_worker(job: Job, replies: Vec<Reply>) -> Worker {
    let (sender, receiver) = mpsc::channel();
    for reply in replies {
        sender.send(reply).unwrap();
    }
    Worker {
        handle: thread::spawn(move || drop(sender)),
        receiver,
        cancel: Cancellation::default(),
        job,
    }
}
fn idle_controller() -> crate::qt::UniquePtr<ffi::PackageController> {
    let mut controller = ffi::create_controller();
    controller.pin_mut().rust_mut().prefetch.clear();
    controller
}
#[test]
fn source_and_scope_names_read_as_people_know_them() {
    for (id, name) in [
        ("apt", "APT"),
        ("dnf", "DNF"),
        ("pacman", "Pacman"),
        ("zypper", "Zypper"),
        ("snap", "Snap"),
        ("homebrew", "Homebrew"),
        ("homebrew-cask", "Homebrew Casks"),
        ("macos-apps", "macOS Applications"),
        ("mas", "Mac App Store"),
        ("aur", "AUR"),
        ("apk", "apk"),
        ("xbps", "XBPS"),
        ("system-image", "System image"),
        ("macports", "MacPorts"),
        ("rustup", "rustup"),
        ("nix", "Nix"),
        ("go", "Go"),
        ("dotnet", ".NET tools"),
        ("appimage", "AppImage"),
        ("flatpak", "Flatpak"),
        ("docker", "Docker images"),
        ("podman", "Podman images"),
        ("toolbox", "Toolbx containers"),
        ("distrobox", "Distrobox containers"),
        ("cargo", "Cargo"),
        ("npm", "npm"),
        ("pnpm", "pnpm"),
        ("bun", "Bun"),
        ("pip", "pip"),
        ("pipx", "pipx"),
        ("uv", "uv"),
        ("pixi", "pixi"),
        ("conda", "Conda"),
        ("composer", "Composer"),
        ("gem", "RubyGems"),
        ("fwupd", "Firmware"),
        ("codex", "Codex (standalone)"),
        ("claude", "Claude Code (standalone)"),
        ("grok", "Grok (standalone)"),
        ("opencode", "OpenCode (standalone)"),
        ("cursor", "Cursor CLI (standalone)"),
        ("copilot", "GitHub Copilot CLI (standalone)"),
        ("kiro", "Kiro CLI (standalone)"),
        ("antigravity", "Antigravity CLI (standalone)"),
        ("amp", "Amp (standalone)"),
        ("droid", "Factory Droid (standalone)"),
        ("solana", "Solana CLI (Agave)"),
        ("anchor", "Anchor (AVM)"),
        ("foundry", "Foundry (standalone)"),
        ("dev-caches", "Developer caches"),
        ("projects", "Old projects"),
        ("fixture", "fixture"),
    ] {
        assert_eq!(source_display_name(id), name);
    }
    // Every source the engine knows has a name of its own.
    for id in pkgdeck_core::backends::BACKEND_IDS {
        assert!(source_display_name(id) != *id || id.chars().all(|c| c.is_lowercase()));
    }
    assert_eq!(
        source_and_scope("flatpak", &Scope::User { uid: 1000 }),
        "Flatpak, User"
    );
    assert_eq!(source_and_scope("apt", &Scope::System), "APT, System");
    assert_eq!(
        scope_word(&Scope::Environment {
            path: PathBuf::from("/tmp/env")
        }),
        "/tmp/env"
    );
    let upgrade_all = Operation::UpgradeAll {
        backend: "flatpak".into(),
    };
    assert_eq!(operation_title(&upgrade_all), "Update all Flatpak packages");
    let mut id = synthetic_package("tool", "Tool").id;
    id.scope = Scope::User { uid: 1000 };
    let label = operation_label(&Operation::Remove(id.clone()));
    assert!(label.contains("Source: APT"));
    assert!(label.contains("Scope: User"));
    assert!(!label.contains("1000"));
    // A downloaded AppImage shows its file name, a Flatpak ref its ref.
    id.backend = "appimage".into();
    id.name = "https://example.invalid/dl/Tool.AppImage?x=1".into();
    id.reference = Some("artifact:sha".into());
    assert_eq!(package_name(&id), "Tool.AppImage");
    // A local AppImage's reference is its checksum, never shown.
    id.name = "/home/user/Downloads/tool.appimage".into();
    id.reference = Some("ab".repeat(32));
    assert_eq!(package_name(&id), "tool.appimage");
    id.backend = "flatpak".into();
    id.reference = Some("app/org.example.Tool/x86_64/stable".into());
    assert_eq!(package_name(&id), "app/org.example.Tool/x86_64/stable");
    // An App Store app shows its name, not its App Store ID.
    id.backend = "mas".into();
    id.name = "iMovie".into();
    id.reference = Some("408981434".into());
    assert_eq!(package_name(&id), "iMovie");
    assert_eq!(
        operation_title(&Operation::Upgrade(id)),
        "Update iMovie (Mac App Store)"
    );
}
#[test]
fn repository_text_names_sources_and_scopes() {
    let action = RepositoryAction {
        backend: "flatpak".into(),
        name: "flathub".into(),
        scope: Scope::System,
        change: repositories::Change::Remove,
    };
    assert_eq!(
        repository_label(&action),
        "Remove repository\nflathub, Flatpak, System"
    );
    for (change, text) in [
        (
            repositories::Change::Add {
                url: "https://example.invalid".into(),
            },
            "Add repository from https://example.invalid",
        ),
        (
            repositories::Change::SetEnabled { enabled: true },
            "Enable repository",
        ),
        (
            repositories::Change::SetEnabled { enabled: false },
            "Disable repository",
        ),
        (
            repositories::Change::SetPriority { priority: 5 },
            "Set repository priority to 5",
        ),
        (repositories::Change::OpenEditor, "Open Software Sources"),
    ] {
        let action = RepositoryAction {
            change,
            ..action.clone()
        };
        assert!(repository_label(&action).starts_with(text));
    }
    assert_eq!(
        repository_error("flatpak: host command failed (Some(1)): error: No remote refs\n"),
        "Flatpak couldn't run: No remote refs."
    );
    assert_eq!(
        repository_error("apt: listing failed"),
        "APT: Listing failed."
    );
    assert_eq!(
        repository_error("/etc/apt/sources.list: unreadable"),
        "/etc/apt/sources.list: unreadable."
    );
}
#[test]
fn failed_commands_keep_their_output_for_the_banner() {
    use pkgdeck_core::process::{Completion, ExecutionError as E};
    let failed = |stdout: &str, stderr: &str| {
        EngineError::Execution(E::Failed(Completion {
            code: Some(101),
            signal: None,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
            truncated: false,
            cancellation_deferred: false,
        }))
    };
    assert_eq!(
        raw_failure_output(&failed("built 3 crates\n", "error: linker failed\n")).as_deref(),
        Some("built 3 crates\nerror: linker failed")
    );
    assert_eq!(raw_failure_output(&failed("", "  \n")), None);
    assert_eq!(raw_failure_output(&EngineError::NotFound), None);
    // A long log keeps its end, where the reason is.
    let long = format!("{}\nthe reason", "x".repeat(FAILURE_OUTPUT_LIMIT * 2));
    let kept = raw_failure_output(&failed("", &long)).unwrap();
    assert!(kept.starts_with('…') && kept.ends_with("the reason"));
    // The cut moves forward off the middle of a character.
    let accents = format!("{}x", "é".repeat(FAILURE_OUTPUT_LIMIT));
    let kept = raw_failure_output(&failed("", &accents)).unwrap();
    assert!(kept.starts_with("…é") && kept.ends_with('x'));
}
#[test]
fn retry_runs_only_the_steps_that_failed() {
    let names = |job: &Option<Job>| match job {
        Some(Job::UpgradeAll(operations, _) | Job::CleanAll(operations)) => operations.len(),
        _ => 0,
    };
    let upgrade = |backend: &str| Operation::UpgradeAll {
        backend: backend.into(),
    };
    let job = Job::UpgradeAll(vec![upgrade("cargo"), upgrade("npm")], None);
    let partial = Ok(ended(&[Outcome::Finished, Outcome::Failed]));
    assert_eq!(names(&retry_job(&job, &partial)), 1);
    let none_failed = Ok(ended(&[Outcome::Finished, Outcome::Finished]));
    assert!(retry_job(&job, &none_failed).is_none());
    assert_eq!(names(&retry_job(&job, &Err(EngineError::NotFound))), 2);
    assert!(retry_job(&job, &Err(EngineError::Cancelled)).is_none());
    // A step that finished has nothing to retry.
    assert!(retry_job(&job, &Ok(Payload::Written(OperationOutcome::default()))).is_none());
    assert_eq!(names(&None), 0);
    // Cleanups retry the same way.
    let cache = |backend: &str| {
        Operation::Clean(CleanupId {
            backend: backend.into(),
            key: "cache".into(),
        })
    };
    let clean = Job::CleanAll(vec![cache("apt"), cache("npm")]);
    let one_failed = Ok(ended(&[Outcome::Failed, Outcome::Finished]));
    assert_eq!(names(&retry_job(&clean, &one_failed)), 1);
    assert!(retry_job(&clean, &none_failed).is_none());
}
#[test]
fn a_failed_change_offers_its_output_and_retry_once() {
    use pkgdeck_core::process::{Completion, ExecutionError as E};
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let row = fixture_row();
    controller.as_mut().rust_mut().packages = vec![row.clone()];
    let job = Job::Write(Operation::Upgrade(row.id.clone()), None);
    let failure = EngineError::Execution(E::Failed(Completion {
        code: Some(1),
        signal: None,
        stdout: vec![],
        stderr: b"the tool said no".to_vec(),
        truncated: false,
        cancellation_deferred: false,
    }));
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        job,
        vec![
            Reply::FailureOutput("Upgrade\nthe batch log".into()),
            Reply::Done(Err(failure)),
        ],
    ));
    controller.as_mut().poll();
    let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
    assert_eq!(notice["kind"], "error");
    assert_eq!(notice["output"], "Upgrade\nthe batch log");
    assert_eq!(notice["retry"], true);
    // Retry clears the banner and runs the change again, once.
    controller.as_mut().retry_change();
    assert_eq!(controller.notice().to_string(), "{}");
    assert!(controller.rust().worker.is_some());
    settle(&mut controller);
    let finished = controller.notice().to_string();
    assert!(finished.contains("\"success\""));
    controller.as_mut().retry_change();
    assert_eq!(controller.notice().to_string(), finished);
}
#[test]
fn engine_errors_become_one_plain_sentence() {
    use pkgdeck_core::process::{Completion, ExecutionError as E};
    let failed = |stderr: &str| {
        EngineError::Execution(E::Failed(Completion {
            code: Some(1),
            signal: None,
            stdout: vec![],
            stderr: stderr.as_bytes().to_vec(),
            truncated: false,
            cancellation_deferred: false,
        }))
    };
    let rustup = failed("error: rustup could not choose a version of cargo to run, because one wasn't specified explicitly, and no default is configured.\nhelp: run 'rustup default stable'\n");
    assert_eq!(
        plain_error(&rustup, Some("cargo"), false),
        "Cargo couldn't run: rustup has no default toolchain."
    );
    // Cargo names the dependency whose build failed.
    let openssl = failed("error: failed to run custom build command for `openssl-sys v0.9.100`\n\nCaused by:\n  Could not find openssl via pkg-config\n");
    assert_eq!(
            plain_error(&openssl, Some("cargo"), false),
            "Cargo couldn't run: building `openssl-sys v0.9.100` failed. It needs OpenSSL's development files (libssl-dev or openssl-devel) and pkg-config."
        );
    // The same crate failing for another reason gets no install advice.
    let old_openssl = failed("error: failed to run custom build command for `openssl-sys v0.6.7`\n\nCaused by:\n  process didn't exit successfully\n");
    assert_eq!(
            plain_error(&old_openssl, Some("cargo"), false),
            "Cargo couldn't run: building `openssl-sys v0.6.7` failed. Run the update in a terminal to read the build log."
        );
    let other = failed("error: failed to run custom build command for `ring v0.17.0`\n");
    assert_eq!(
            plain_error(&other, Some("cargo"), false),
            "Cargo couldn't run: building `ring v0.17.0` failed. Run the update in a terminal to read the build log."
        );
    // Flatpak names the remote and the reason, not the URL.
    let remote = failed("error: Unable to load summary from remote astrovm: While fetching https://flatpak.4st.li/repo/summary.idx: [35] SSL connect error\n");
    assert_eq!(
        plain_error(&remote, Some("flatpak"), false),
        "Flatpak couldn't run: it can't reach the remote astrovm (SSL connect error)."
    );
    // The same text reached through Display, with the debug exit code.
    assert_eq!(
        plain_text(&rustup.to_string(), Some("cargo")),
        "Cargo couldn't run: rustup has no default toolchain."
    );
    assert_eq!(
        plain_text(
            "host command failed (Some(100)): \nE: Unable to locate package x\n",
            None
        ),
        "Unable to locate package x."
    );
    assert_eq!(
        plain_text("host command failed (None): ", Some("apt")),
        "APT reported an error without details."
    );
    assert_eq!(
        plain_text("host command failed (None): ", None),
        "The package manager reported an error without details."
    );
    // Stdout is used when a tool reports only there.
    let quiet = EngineError::Execution(E::Failed(Completion {
        code: Some(1),
        signal: None,
        stdout: b"npm ERR! 404 Not Found\n".to_vec(),
        stderr: vec![],
        truncated: false,
        cancellation_deferred: false,
    }));
    assert_eq!(
        plain_error(&quiet, Some("npm"), false),
        "npm couldn't run: npm ERR! 404 Not Found."
    );
    assert_eq!(
        plain_error(&failed("   \n"), None, false),
        "The package manager reported an error."
    );
    assert_eq!(plain_error(&failed("boom"), None, false), "Boom.");
    let sudo_failed = failed(
        "Error: Failure while executing; `/usr/bin/sudo -E -- /bin/rm -r -f --` exited with 1.",
    );
    assert!(plain_error(&sudo_failed, Some("homebrew-cask"), false)
        .starts_with("Homebrew needed your administrator password"));
    assert!(plain_error(&sudo_failed, Some("npm"), false).starts_with("npm couldn't run"));
    // Running as root is explained, whatever layer said it.
    assert_eq!(
        plain_error(
            &E::Disabled("run the frontend as an unprivileged user".into()).into(),
            Some("apt"),
            false
        ),
        ROOT_MESSAGE
    );
    assert_eq!(
        plain_error(
            &EngineError::Unavailable {
                backend: "apt".into(),
                reason: "run the frontend as an unprivileged user".into()
            },
            None,
            false
        ),
        ROOT_MESSAGE
    );
    assert_eq!(
        plain_error(
            &EngineError::InvalidResponse {
                backend: "snap".into(),
                reason: "run the frontend as an unprivileged user".into()
            },
            None,
            false
        ),
        ROOT_MESSAGE
    );
    assert_eq!(
        plain_error(
            &EngineError::InvalidResponse {
                backend: "snap".into(),
                reason: "unexpected output Some(\"x\")".into()
            },
            None,
            false
        ),
        "Snap reported a problem: unexpected output x."
    );
    assert_eq!(
        plain_error(
            &EngineError::InvalidResponse {
                backend: "snap".into(),
                reason: " ".into()
            },
            None,
            false
        ),
        "Snap reported a problem."
    );
    // PkgDeck's own sentence needs no source in front.
    assert_eq!(
        plain_error(
            &EngineError::Refused {
                backend: "dev-caches".into(),
                refusal: Refusal::CleanupChanged,
            },
            None,
            false
        ),
        "This cleanup task changed or expired. Reload and review it again."
    );
    let skipped = EngineError::InvalidResponse {
        backend: "macos-apps".into(),
        reason: "skipped folder /Users/me/Applications/Locked: Permission denied (os error 13)"
            .into(),
    };
    assert_eq!(
    plain_error(&skipped, Some("macos-apps"), false),
    "Couldn't read /Users/me/Applications/Locked (permission denied). The other apps are still listed."
);
    assert_eq!(failure_kind(&skipped), "partial");
    // mas asks for the password itself, whatever PkgDeck's setting is.
    let denied = EngineError::Execution(pkgdeck_core::process::ExecutionError::AuthorizationDenied);
    for sudo in [false, true] {
        assert!(plain_error(&denied, Some("mas"), sudo).contains("pkd upgrade --from mas"));
    }
    let elsewhere = EngineError::InvalidResponse {
        backend: "snap".into(),
        reason: "skipped folder /x: denied".into(),
    };
    assert_eq!(failure_kind(&elsewhere), "failed");
    assert_eq!(
        plain_error(
            &EngineError::InvalidResponse {
                backend: "open".into(),
                reason: "Only local file URLs are supported.".into()
            },
            None,
            false
        ),
        "Only local file URLs are supported."
    );
    assert_eq!(
        plain_error(
            &EngineError::Unavailable {
                backend: "apt".into(),
                reason: "APT lists are missing".into()
            },
            None,
            false
        ),
        "APT isn't available: APT lists are missing."
    );
    assert_eq!(
        plain_error(
            &EngineError::Unsupported {
                backend: "fwupd".into(),
                capability: Capability::Install
            },
            None,
            false
        ),
        "Firmware can't do this."
    );
    assert_eq!(
        plain_error(&EngineError::UnknownBackend("x".into()), None, false),
        "Unknown backend: x."
    );
    assert_eq!(plain_error(&E::Cancelled.into(), None, false), "Cancelled.");
    assert_eq!(plain_text("", None), "Something went wrong.");
    assert_eq!(sentence("  "), "");
    assert_eq!(sentence("done!"), "Done!");
    assert_eq!(strip_debug("code Some(3 and more"), "code 3 and more");
    assert_eq!(strip_debug("exit (None) now"), "exit  now");
    assert_eq!(lower_first("APT lists"), "APT lists");
    assert_eq!(lower_first("Broken"), "broken");
    assert_eq!(meaningful_line("\n\n"), None);
    assert_eq!(
        meaningful_line(
            "WARNING: apt does not have a stable CLI interface.\nReading\nE: Held packages"
        ),
        Some("Held packages".into())
    );
    // A denial in a batch is worded for the chosen permission option.
    let denied = expect!(
        ran(&[
            ("Update x (APT)", Outcome::Failed, Said::Denied),
            ("Update y (APT)", Outcome::Finished, Said::KeptAfterCancel),
        ]),
        Payload::Batch(ran) => ran
    );
    assert_eq!(
            denied.text(true),
            format!(
                "Completed 1 of 2 updates.\nUpdate x (APT): {}\nUpdate y (APT): Finished before it could be cancelled. Changes were kept.",
                plain_error(&E::AuthorizationDenied.into(), None, true)
            )
        );
    assert_ne!(denied.text(true), denied.text(false));
}
#[test]
fn errors_shown_by_the_controller_are_plain() {
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().apply(Err(EngineError::Execution(
        pkgdeck_core::process::ExecutionError::Disabled(
            "run the frontend as an unprivileged user".into(),
        ),
    )));
    assert_eq!(controller.status().to_string(), ROOT_MESSAGE);
    let failure = BackendFailure {
            backend: "cargo".into(),
            error: EngineError::InvalidResponse {
                backend: "cargo".into(),
                reason: "host command failed (Some(1)): error: rustup could not choose a version of cargo to run".into(),
            },
        };
    controller
        .as_mut()
        .apply(Ok(Payload::Packages(PackageReport {
            packages: vec![synthetic_package("htop", "htop")],
            failures: vec![failure.clone()],
            successful_sources: vec!["apt".into()],
        })));
    let expected = "Cargo couldn't run: rustup has no default toolchain.";
    assert!(controller
        .status()
        .to_string()
        .contains(&format!("Cargo: {expected}")));
    let rows: Value = serde_json::from_str(&controller.rows().to_string()).unwrap();
    assert_eq!(rows[1]["summary"], expected);
    assert_eq!(rows[1]["display_name"], "Cargo");
    assert_eq!(rows[1]["source"], "cargo");
    let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
    assert_eq!(state["failures"][0]["detail"], expected);
    controller
        .as_mut()
        .apply(Ok(Payload::Cleanup(CleanupReport {
            items: vec![],
            failures: vec![failure],
        })));
    let rows: Value = serde_json::from_str(&controller.rows().to_string()).unwrap();
    assert_eq!(rows[0]["summary"], expected);
    let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
    assert_eq!(state["failures"][0]["detail"], expected);
    assert!(state["checked_at"].as_u64().unwrap() > 0);
}
#[test]
fn activity_reads_share_the_lock_and_never_write() {
    use rustix::fs::{flock, FlockOperation};
    let path = std::env::temp_dir().join(format!("pkgdeck-activity-read-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    let store = ActivityStore::at(path.join("activity.json"));
    // No history yet reads as empty, without creating anything.
    assert!(store.load().unwrap().is_empty());
    assert!(!path.exists());
    let operation = Operation::Refresh {
        backend: "apt".into(),
    };
    let id = store
        .begin("gui", vec![operation.clone()], vec![], State::Finished)
        .unwrap();
    // Another reader holding the lock shared does not block this read.
    let lock = std::fs::File::open(path.join("activity.lock")).unwrap();
    flock(&lock, FlockOperation::LockShared).unwrap();
    let modified = std::fs::metadata(path.join("activity.json"))
        .unwrap()
        .modified()
        .unwrap();
    let entries = store.load().unwrap();
    assert_eq!(entries[0].id, id);
    assert_eq!(
        std::fs::metadata(path.join("activity.json"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    drop(lock);
    // An entry whose owner died is marked interrupted once.
    store
        .begin("gui", vec![operation], vec![], State::Running)
        .unwrap();
    let mut raw: Vec<Value> =
        serde_json::from_slice(&std::fs::read(path.join("activity.json")).unwrap()).unwrap();
    raw[1]["owner_pid"] = json!(u32::MAX - 1);
    std::fs::write(
        path.join("activity.json"),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    assert_eq!(store.load().unwrap()[1].state, State::Interrupted);
    // A broken file reads as no history rather than failing.
    std::fs::write(path.join("activity.json"), b"not json").unwrap();
    assert!(read_activity(&path.join("activity.json"))
        .unwrap()
        .is_empty());
    // A directory where the file should be is an error.
    std::fs::remove_file(path.join("activity.json")).unwrap();
    std::fs::create_dir(path.join("activity.json")).unwrap();
    assert!(read_activity(&path.join("activity.json")).is_err());
    std::fs::remove_dir(path.join("activity.json")).unwrap();
    std::fs::remove_file(path.join("activity.lock")).unwrap();
    std::fs::create_dir(path.join("activity.lock")).unwrap();
    assert!(read_activity(&path.join("activity.json")).is_ok());
    std::fs::remove_dir_all(path).unwrap();
    let _ = Some(&store).map(|store| store.history.clone());
    assert!(ActivityStore::default_store().is_some() || std::env::var_os("HOME").is_none());
}
#[test]
fn activity_refreshes_off_the_gui_thread_and_repeats_when_asked_again() {
    let path = std::env::temp_dir().join(format!("pkgdeck-activity-worker-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    let store = ActivityStore::at(path.join("activity.json"));
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    // Without a store nothing starts.
    controller.as_mut().refresh_activity();
    assert!(controller.rust().activity_worker.is_none());
    controller.as_mut().poll();
    assert!(!*controller.needs_poll());
    controller.as_mut().rust_mut().activity_store = Some(store.clone());
    controller.as_mut().refresh_activity();
    assert!(*controller.needs_poll());
    // A second request while reading reads again afterwards.
    store
        .begin(
            "gui",
            vec![Operation::Refresh {
                backend: "flatpak".into(),
            }],
            vec![],
            State::Queued,
        )
        .unwrap();
    controller.as_mut().refresh_activity();
    assert!(controller.rust().activity_again);
    wait_until(&mut controller, |controller| {
        controller
            .activity()
            .to_string()
            .contains("Refresh Flatpak package lists")
            && controller.rust().activity_worker.is_none()
    });
    assert!(!controller.rust().activity_again);
    assert!(!*controller.needs_poll());
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn needs_poll_is_true_exactly_while_work_is_outstanding() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    // Startup preloads wait for poll().
    assert!(*controller.needs_poll());
    controller.as_mut().rust_mut().prefetch.clear();
    controller.as_mut().poll();
    assert!(!*controller.needs_poll());
    // A confirmation holds preloads back; declining lets them run.
    controller.as_mut().rust_mut().prefetch = vec!["Clean".into()];
    controller.as_mut().rust_mut().pending = Some(Job::CleanAll(vec![]));
    controller.as_mut().sync_needs_poll();
    assert!(!*controller.needs_poll());
    controller.as_mut().confirm(false);
    assert!(*controller.needs_poll());
    controller.as_mut().rust_mut().prefetch.clear();
    // Details start synchronously and flip it at once.
    controller.as_mut().rust_mut().source_filter = vec!["grok".into()];
    let mut package = synthetic_package("grok", "Grok");
    package.id.backend = "grok".into();
    controller.as_mut().rust_mut().packages = vec![package];
    controller.as_mut().poll();
    assert!(!*controller.needs_poll());
    controller.as_mut().select(0);
    assert!(*controller.needs_poll());
    wait_until(&mut controller, |controller| {
        controller.rust().details_worker.is_none()
    });
    controller.as_mut().poll();
    assert!(!*controller.needs_poll());
    // Cancelling with nothing running keeps it false.
    controller.as_mut().cancel();
    assert!(!*controller.needs_poll());
    // Each worker kind counts.
    let mut rust = Controller::default();
    rust.prefetch.clear();
    assert!(!rust.outstanding());
    rust.awaiting_prefetch = true;
    assert!(rust.outstanding());
    rust.awaiting_prefetch = false;
    rust.catalog_worker = Some(CatalogWorker {
        handle: thread::spawn(|| {}),
        receiver: mpsc::channel().1,
        cancel: Cancellation::default(),
    });
    assert!(rust.outstanding());
    let worker = rust.catalog_worker.take().unwrap();
    worker.handle.join().unwrap();
    rust.worker = Some(fake_worker(Job::Load("Clean".into(), "".into()), vec![]));
    assert!(rust.outstanding());
    assert!(rust.foreground_worker());
    rust.refreshing = true;
    assert!(!rust.foreground_worker());
    rust.worker.take().unwrap().handle.join().unwrap();
}
#[test]
fn returning_to_a_recent_section_publishes_it_at_once() {
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    let key = cache_key("Updates", "", &[], false);
    let mut cached = cached_view("recent update");
    cached.report_state = r#"{"phase":"complete","checked_at":1234}"#.into();
    controller
        .as_mut()
        .rust_mut()
        .view_cache
        .insert(key, cached);
    controller
        .as_mut()
        .load("Updates".into(), "".into(), "".into(), false, false);
    assert!(controller.rows().to_string().contains("recent update"));
    assert!(!*controller.busy());
    assert!(!*controller.refreshing());
    assert!(controller.rust().worker.is_none());
    let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
    assert_eq!(state["phase"], "cached");
    assert_eq!(state["checked_at"], 1234);
}
#[test]
fn returning_to_an_old_section_refreshes_quietly_behind_it() {
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    let key = cache_key("Installed", "", &[], false);
    let mut cached = cached_view("old package");
    cached.loaded = Instant::now() - VIEW_TTL;
    controller
        .as_mut()
        .rust_mut()
        .view_cache
        .insert(key.clone(), cached);
    // The previous section still reads in the background.
    controller.as_mut().rust_mut().background = true;
    controller.as_mut().rust_mut().worker =
        Some(fake_worker(Job::Load("Sources".into(), "".into()), vec![]));
    controller
        .as_mut()
        .load("Installed".into(), "".into(), "".into(), false, false);
    assert!(controller.rows().to_string().contains("old package"));
    assert!(!*controller.busy());
    assert!(*controller.refreshing());
    assert!(*controller.needs_poll());
    assert!(matches!(&controller.rust().queued, Some(Job::Load(view, _)) if view == "Installed"));
    // Asking for it again keeps the one reload.
    controller.as_mut().rust_mut().background = false;
    controller.as_mut().rust_mut().queued = None;
    let report = PackageReport {
        packages: vec![synthetic_package("fresh", "Fresh")],
        failures: vec![],
        successful_sources: vec!["apt".into()],
    };
    let reload = fake_worker(
        Job::Load("Installed".into(), "".into()),
        vec![Reply::Done(Ok(Payload::Packages(report)))],
    );
    let cancel = reload.cancel.clone();
    controller.as_mut().rust_mut().worker = Some(reload);
    controller
        .as_mut()
        .load("Installed".into(), "".into(), "".into(), false, false);
    assert!(!cancel.requested());
    assert!(*controller.refreshing());
    assert!(!*controller.busy());
    // The reload lands: fresh rows, spinner off, checked time recorded.
    wait_until(&mut controller, |controller| {
        controller.rust().worker.is_none()
    });
    assert!(controller.rows().to_string().contains("fresh"));
    assert!(!*controller.refreshing());
    assert!(!*controller.busy());
    let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
    assert!(state["checked_at"].as_u64().unwrap() > 0);
}
#[test]
fn a_users_request_replaces_a_quiet_refresh() {
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().active_view = "Installed".into();
    controller.as_mut().set_refreshing(true);
    let refresh = fake_worker(Job::Load("Installed".into(), "".into()), vec![]);
    let cancel = refresh.cancel.clone();
    controller.as_mut().rust_mut().worker = Some(refresh);
    let operation = Operation::Install(synthetic_package("htop", "htop").id);
    controller
        .as_mut()
        .start(Job::PlanOperation(operation.clone()));
    assert!(cancel.requested());
    assert!(!*controller.refreshing());
    assert!(*controller.busy());
    assert!(controller.rust().background);
    assert!(matches!(&controller.rust().queued, Some(Job::PlanOperation(op)) if *op == operation));
    // Guards that refused while a foreground read ran let requests
    // through to start() while only a quiet refresh runs.
    controller.as_mut().rust_mut().background = false;
    controller.as_mut().set_refreshing(true);
    controller.as_mut().load_repositories();
    assert!(matches!(
        &controller.rust().queued,
        Some(Job::Repositories(None))
    ));
}
#[test]
fn a_waited_for_preload_of_an_old_section_is_quiet() {
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    let key = cache_key("Clean", "", &[], false);
    let mut cached = cached_view("old cleanup");
    cached.expired = true;
    controller
        .as_mut()
        .rust_mut()
        .view_cache
        .insert(key.clone(), cached);
    controller.as_mut().rust_mut().prefetch_worker =
        Some(fake_prefetch("Clean", key, vec![], false));
    controller
        .as_mut()
        .load("Clean".into(), "".into(), "".into(), false, false);
    assert!(controller.rust().awaiting_prefetch);
    assert!(*controller.refreshing());
    assert!(!*controller.busy());
}
#[test]
fn a_preloaded_section_says_when_it_was_read() {
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    let key = cache_key("Updates", "", &[], false);
    let loaded = Instant::now() - Duration::from_secs(20);
    controller
        .as_mut()
        .rust_mut()
        .prefetched
        .insert(key, (loaded, Payload::Packages(PackageReport::default())));
    controller
        .as_mut()
        .load("Updates".into(), "".into(), "".into(), false, false);
    let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
    let checked = state["checked_at"].as_u64().unwrap();
    assert!(epoch_seconds() - checked >= 19);
    assert!(!*controller.busy());
    assert!(!*controller.refreshing());
    assert_eq!(epoch_seconds_at(Instant::now()), epoch_seconds());
    // A loading partial carries no checked time.
    controller
        .as_mut()
        .set_package_report_state(&PackageReport::default(), true);
    let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
    assert!(state.get("checked_at").is_none());
    controller.as_mut().apply(Ok(Payload::Sources(vec![])));
    let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
    assert!(state["checked_at"].as_u64().is_some());
}
#[test]
fn progress_names_the_rows_it_changes_and_how_far_it_is() {
    let first = synthetic_package("first", "First App").id;
    let second = synthetic_package("second", "Second App").id;
    let names = Names::from([(first.clone(), "First App".to_owned())]);
    let job = Job::UpgradeAll(
        vec![
            Operation::Upgrade(first.clone()),
            Operation::Upgrade(second.clone()),
            Operation::UpgradeAll {
                backend: "flatpak".into(),
            },
        ],
        None,
    );
    let mut state = ProgressState::new(&job, Some(7), &names);
    let snapshot: Value = serde_json::from_str(&state.snapshot().to_string()).unwrap();
    assert_eq!(snapshot["label"], "Updating First App (APT)…");
    assert_eq!(snapshot["targets"][0], target_row(&first));
    assert_eq!(snapshot["targets"][1]["name"], "second");
    assert_eq!(snapshot["sources"], json!(["flatpak"]));
    assert_eq!(snapshot["fraction"], 0.0);
    assert_eq!(snapshot["current"], Value::Null);
    assert_eq!(snapshot["action"], "");
    state.apply(&Event::Started(Operation::Upgrade(second.clone())));
    state.apply(&Event::Progress {
        operation: Operation::Upgrade(second.clone()),
        progress: Progress::Transfer {
            completed: 50,
            total: Some(100),
        },
    });
    let snapshot: Value = serde_json::from_str(&state.snapshot().to_string()).unwrap();
    assert_eq!(snapshot["current"], target_row(&second));
    assert_eq!(snapshot["current_source"], "apt");
    assert_eq!(snapshot["action"], "update");
    assert!((snapshot["fraction"].as_f64().unwrap() - 0.5 / 3.0).abs() < 1e-9);
    assert_eq!(snapshot["finished"], json!([]));
    // A finished package step is listed, a whole-source one is not.
    state.apply(&Event::Finished {
        operation: Operation::Upgrade(second.clone()),
        result: Ok(OperationOutcome::default()),
    });
    state.apply(&Event::Finished {
        operation: Operation::UpgradeAll {
            backend: "flatpak".into(),
        },
        result: Ok(OperationOutcome::default()),
    });
    let snapshot: Value = serde_json::from_str(&state.snapshot().to_string()).unwrap();
    assert_eq!(snapshot["finished"], json!([target_row(&second)]));
    // One change: its transfer is the fraction, unknown without one.
    let single = Job::Write(Operation::Install(first.clone()), None);
    let mut state = ProgressState::new(&single, None, &names);
    let snapshot: Value = serde_json::from_str(&state.snapshot().to_string()).unwrap();
    assert_eq!(snapshot["fraction"], Value::Null);
    assert_eq!(snapshot["current"], target_row(&first));
    assert_eq!(snapshot["action"], "install");
    state.apply(&Event::Progress {
        operation: Operation::Install(first.clone()),
        progress: Progress::Transfer {
            completed: 30,
            total: Some(40),
        },
    });
    assert_eq!(state.fraction(), Some(0.75));
    state.apply(&Event::Finished {
        operation: Operation::Install(first),
        result: Ok(OperationOutcome::default()),
    });
    assert_eq!(state.fraction(), Some(1.0));
}
#[test]
fn steps_of_different_sources_run_and_show_at_once() {
    let mut snap = synthetic_package("gnome-46-2404", "GNOME 46");
    snap.id.backend = "snap".into();
    snap.update = UpdateAvailability::Available;
    let mut apt = synthetic_package("wireplumber", "WirePlumber");
    apt.update = UpdateAvailability::Available;
    let (snaps, apt_update) = (
        Operation::UpgradeAll {
            backend: "snap".into(),
        },
        Operation::Upgrade(apt.id.clone()),
    );
    let flatpak = Operation::UpgradeAll {
        backend: "flatpak".into(),
    };
    let job = Job::UpgradeAll(
        vec![apt_update.clone(), snaps.clone(), flatpak.clone()],
        None,
    );
    let mut state =
        ProgressState::new(&job, None, &Names::new()).with_rows(&[apt.clone(), snap.clone()]);
    let snapshot = |state: &ProgressState| -> Value {
        serde_json::from_str(&state.snapshot().to_string()).unwrap()
    };
    let transfer = |operation: &Operation, completed| Event::Progress {
        operation: operation.clone(),
        progress: Progress::Transfer {
            completed,
            total: Some(100),
        },
    };
    state.apply(&Event::Started(apt_update.clone()));
    state.apply(&transfer(&apt_update, 50));
    // One step: the card shows its transfer.
    assert_eq!(snapshot(&state)["transfer_total"], 100);
    state.apply(&Event::Started(snaps.clone()));
    state.apply(&transfer(&snaps, 25));
    assert!(state.observe("gnome-46-2404 2 from Canonical refreshed"));
    let now = snapshot(&state);
    assert_eq!(now["label"], "Updating GNOME 46 and 1 more");
    assert_eq!(
        now["running"],
        json!([
            {"current": target_row(&apt.id), "source": "apt"},
            {"current": target_row(&snap.id), "source": "snap"},
        ])
    );
    assert_eq!(now["current"], target_row(&snap.id));
    // Two steps: no single transfer, but both count toward the bar.
    assert_eq!(now["transfer_total"], Value::Null);
    assert!((now["fraction"].as_f64().unwrap() - 0.75 / 3.0).abs() < 1e-9);
    state.apply(&Event::Finished {
        operation: snaps,
        result: Ok(OperationOutcome::default()),
    });
    // Back to what still runs.
    let now = snapshot(&state);
    assert_eq!(now["label"], "Updating wireplumber (APT)…");
    assert_eq!(now["current"], target_row(&apt.id));
    assert_eq!(now["current_source"], "apt");
    assert_eq!(now["transferred"], 50);
    assert_eq!(now["finished"], json!([target_row(&snap.id)]));
    // A step that finishes without having started moves nothing back.
    state.apply(&Event::Finished {
        operation: flatpak,
        result: Ok(OperationOutcome::default()),
    });
    assert_eq!(snapshot(&state)["current_source"], "apt");
    state.apply(&Event::Finished {
        operation: apt_update,
        result: Ok(OperationOutcome::default()),
    });
    let now = snapshot(&state);
    assert_eq!(now["running"], json!([]));
    assert_eq!(now["fraction"], 1.0);
    assert!(!state.observe("gnome-46-2404 2 from Canonical refreshed"));
}
#[test]
fn update_all_counts_and_follows_each_package_of_a_source() {
    let cask = |name: &str, shown: &str| {
        let mut package = synthetic_package(name, shown);
        package.id.backend = "homebrew-cask".into();
        package.update = UpdateAvailability::Available;
        package
    };
    let (alpha, beta, gamma) = (
        cask("alpha", "Alpha App"),
        cask("beta", "Beta App"),
        cask("gamma", "Gamma App"),
    );
    let mut current = cask("current", "Current");
    current.update = UpdateAvailability::Current;
    let firmware = {
        let mut package = synthetic_package("bios", "BIOS");
        package.id.backend = "fwupd".into();
        package.id
    };
    let casks = Operation::UpgradeAll {
        backend: "homebrew-cask".into(),
    };
    let flatpak = Operation::UpgradeAll {
        backend: "flatpak".into(),
    };
    let job = Job::UpgradeAll(
        vec![
            casks.clone(),
            flatpak.clone(),
            Operation::Upgrade(firmware.clone()),
        ],
        None,
    );
    let packages = [alpha.clone(), beta.clone(), gamma.clone(), current];
    let mut state = ProgressState::new(&job, None, &Names::new()).with_rows(&packages);
    let snapshot = |state: &ProgressState| -> Value {
        serde_json::from_str(&state.snapshot().to_string()).unwrap()
    };
    // Three casks, one Flatpak step without rows, and the firmware.
    assert_eq!(snapshot(&state)["total"], 5);
    assert_eq!(snapshot(&state)["targets"].as_array().unwrap().len(), 4);
    assert!(!state.observe("==> Upgrading beta"), "nothing runs yet");
    state.apply(&Event::Started(casks.clone()));
    assert!(!state.observe("==> Upgrading 3 outdated packages:"));
    assert!(!state.observe("==> Upgrading unlisted"));
    assert!(state.observe("==> Upgrading beta"));
    let now = snapshot(&state);
    assert_eq!(now["label"], "Updating Beta App…");
    assert_eq!(now["current"], target_row(&beta.id));
    assert_eq!(now["done"], 0);
    assert!(state.observe("==> Upgrading gamma"));
    assert_eq!(snapshot(&state)["finished"], json!([target_row(&beta.id)]));
    // Sources PkgDeck updates one by one report packages directly, by
    // the name people see as well.
    state.apply(&Event::Progress {
        operation: casks.clone(),
        progress: Progress::Package("Alpha App".into()),
    });
    assert_eq!(snapshot(&state)["done"], 2);
    state.apply(&Event::Finished {
        operation: casks,
        result: Ok(OperationOutcome::default()),
    });
    assert_eq!(snapshot(&state)["done"], 3);
    assert_eq!(snapshot(&state)["finished"].as_array().unwrap().len(), 3);
    state.apply(&Event::Started(flatpak.clone()));
    state.apply(&Event::Finished {
        operation: flatpak,
        result: Ok(OperationOutcome::default()),
    });
    assert_eq!(snapshot(&state)["done"], 4);
    state.apply(&Event::Started(Operation::Upgrade(firmware.clone())));
    assert!(
        !state.observe("==> Upgrading alpha"),
        "not a whole-source step"
    );
    state.apply(&Event::Finished {
        operation: Operation::Upgrade(firmware),
        result: Ok(OperationOutcome::default()),
    });
    assert_eq!(snapshot(&state)["done"], 5);
    assert_eq!(state.fraction(), Some(1.0));

    // The worker passes on only lines that name a package.
    let (sender, receiver) = crate::wake::channel();
    let follow = output_follower(vec!["homebrew-cask".into()], sender);
    follow("==> Downloading https://example.invalid/beta.zip");
    follow("==> Upgrading beta");
    assert!(matches!(
        receiver.try_iter().collect::<Vec<_>>().as_slice(),
        [Reply::Output(line)] if line == "==> Upgrading beta"
    ));
}
#[test]
fn batch_result_names_the_sources_and_rows_that_succeeded() {
    let first = synthetic_package("first", "First App").id;
    let second = synthetic_package("second", "Second App").id;
    let flatpak = Operation::UpgradeAll {
        backend: "flatpak".into(),
    };
    let job = Job::UpgradeAll(
        vec![
            Operation::Upgrade(first.clone()),
            Operation::Upgrade(second.clone()),
            flatpak.clone(),
        ],
        None,
    );
    let mut state = ProgressState::new(&job, None, &Names::new());
    let result = |state: &ProgressState| -> Value {
        serde_json::from_str(&state.batch_result().to_string()).unwrap()
    };
    assert_eq!(
        result(&state),
        json!({"sources": ["apt", "flatpak"], "finished": []})
    );
    state.apply(&Event::Started(Operation::Upgrade(first.clone())));
    state.apply(&Event::Finished {
        operation: Operation::Upgrade(first.clone()),
        result: Ok(OperationOutcome::default()),
    });
    // A failed row is done, but not a success, so it is not listed.
    state.apply(&Event::Started(Operation::Upgrade(second.clone())));
    state.apply(&Event::Finished {
        operation: Operation::Upgrade(second.clone()),
        result: Err(EngineError::InvalidResponse {
            backend: "apt".into(),
            reason: "failed".into(),
        }),
    });
    state.apply(&Event::Started(flatpak.clone()));
    state.apply(&Event::Finished {
        operation: flatpak,
        result: Ok(OperationOutcome::default()),
    });
    assert_eq!(
        result(&state),
        json!({"sources": ["apt", "flatpak"], "finished": [target_row(&first)]})
    );
    // A single change has no batch marks, so it names nothing.
    let single = Job::Write(Operation::Install(first.clone()), None);
    let mut state = ProgressState::new(&single, None, &Names::new());
    state.apply(&Event::Started(Operation::Install(first.clone())));
    state.apply(&Event::Finished {
        operation: Operation::Install(first),
        result: Ok(OperationOutcome::default()),
    });
    assert_eq!(result(&state), json!({"sources": [], "finished": []}));
}
#[test]
fn unpacking_another_package_does_not_prove_the_previous_update_succeeded() {
    let packages: Vec<_> = ["alpha", "beta"]
        .into_iter()
        .map(|name| {
            let mut package = synthetic_package(name, name);
            package.update = UpdateAvailability::Available;
            package
        })
        .collect();
    let operation = Operation::UpgradeAll {
        backend: "apt".into(),
    };
    let job = Job::UpgradeAll(vec![operation.clone()], None);
    for ok in [false, true] {
        let mut state = ProgressState::new(&job, None, &Names::new()).with_rows(&packages);
        state.apply(&Event::Started(operation.clone()));
        assert!(state.observe("Unpacking alpha:all (2) over (1) ..."));
        assert!(state.observe("Unpacking beta:all (2) over (1) ..."));
        assert!(state.succeeded.is_empty());
        state.apply(&Event::Finished {
            operation: operation.clone(),
            result: if ok {
                Ok(OperationOutcome::default())
            } else {
                Err(EngineError::NotFound)
            },
        });
        assert_eq!(state.succeeded.len(), if ok { 2 } else { 0 });
        assert_eq!(
            parsed(&state.batch_result())["finished"]
                .as_array()
                .unwrap()
                .len(),
            if ok { 2 } else { 0 }
        );
        assert_eq!(state.fraction(), Some(1.0));
    }
}
#[test]
fn followed_output_moves_the_running_update_all_on() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let mut package = synthetic_package("beta", "Beta App");
    package.id.backend = "homebrew-cask".into();
    package.update = UpdateAvailability::Available;
    let casks = Operation::UpgradeAll {
        backend: "homebrew-cask".into(),
    };
    let job = Job::UpgradeAll(vec![casks.clone()], None);
    let mut state =
        ProgressState::new(&job, None, &Names::new()).with_rows(std::slice::from_ref(&package));
    state.apply(&Event::Started(casks));
    controller.as_mut().rust_mut().progress_state = Some(state);
    let (sender, receiver) = mpsc::channel();
    let (release, hold) = mpsc::channel::<()>();
    sender
        .send(Reply::Output("==> Upgrading beta".into()))
        .unwrap();
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(move || {
            let _ = hold.recv();
            drop(sender);
        }),
        receiver,
        cancel: Cancellation::default(),
        job,
    });
    controller.as_mut().poll();
    let progress: Value = serde_json::from_str(&controller.progress().to_string()).unwrap();
    assert_eq!(progress["label"], "Updating Beta App…");
    assert_eq!(progress["current"], target_row(&package.id));
    drop(release);
    controller.as_mut().rust_mut().progress_state = None;
    controller.as_mut().follow_output("==> Upgrading beta");
    settle(&mut controller);
}
#[test]
fn undo_is_offered_only_where_it_is_safe() {
    let id = synthetic_package("tool", "Tool").id;
    assert_eq!(undo_action(&Operation::Install(id.clone())), Some("remove"));
    assert_eq!(undo_action(&Operation::Remove(id.clone())), Some("install"));
    assert_eq!(undo_action(&Operation::Upgrade(id.clone())), None);
    let mut local = id.clone();
    local.reference = Some("local-deb:/tmp/tool.deb".into());
    assert_eq!(undo_action(&Operation::Remove(local)), None);
    let mut appimage = id.clone();
    appimage.backend = "appimage".into();
    assert_eq!(undo_action(&Operation::Remove(appimage.clone())), None);
    assert_eq!(undo_action(&Operation::Install(appimage)), Some("remove"));
    // Removing from a source PkgDeck can't install from can't be undone.
    for backend in ["codex", "mas", "macos-apps"] {
        let mut restricted = id.clone();
        restricted.backend = backend.into();
        assert_eq!(undo_action(&Operation::Remove(restricted)), None);
    }
    let mut firmware = id;
    firmware.backend = "fwupd".into();
    assert_eq!(undo_action(&Operation::Install(firmware)), None);
    assert_eq!(
        undo_action(&Operation::Clean(CleanupId {
            backend: "apt".into(),
            key: "autoremove".into()
        })),
        None
    );
    // Several changes or none are never undone as one.
    let batch = notice_subject(
        &[
            Operation::Refresh {
                backend: "apt".into(),
            },
            Operation::Refresh {
                backend: "snap".into(),
            },
        ],
        &Names::new(),
        true,
    );
    assert_eq!(batch["operation"], "batch");
    assert_eq!(batch["undo"], false);
    assert_eq!(notice_subject(&[], &Names::new(), true)["operation"], "");
    let refresh = notice_subject(
        &[Operation::Refresh {
            backend: "snap".into(),
        }],
        &Names::new(),
        true,
    );
    assert_eq!(refresh["source_name"], "Snap");
    assert!(refresh.get("target").is_none());
    assert_eq!(job_backend(&[]), None);
    assert_eq!(
        operation_kind(&Operation::UpgradeAll {
            backend: "apt".into()
        }),
        "update_all"
    );
    assert_eq!(
        operation_kind(&Operation::Clean(CleanupId {
            backend: "apt".into(),
            key: "autoremove".into()
        })),
        "clean"
    );
}
#[test]
fn confirmed_changes_remember_display_names_for_their_results() {
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    let package = synthetic_package("htop", "Process Viewer");
    controller.as_mut().rust_mut().packages = vec![package.clone()];
    // The remembered names stay bounded.
    for index in 0..300 {
        let id = synthetic_package(&format!("old-{index}"), "Old").id;
        controller
            .as_mut()
            .rust_mut()
            .names
            .insert(id, "Old".into());
    }
    // A busy worker keeps the confirmed change queued.
    controller.as_mut().rust_mut().worker =
        Some(fake_worker(Job::Load("Sources".into(), "".into()), vec![]));
    controller
        .as_mut()
        .accept_confirmed(Job::Write(Operation::Remove(package.id.clone()), None));
    controller.as_mut().rust_mut().packages.clear();
    assert_eq!(controller.rust().names.len(), 1);
    assert_eq!(
        controller.rust().names.get(&package.id).map(String::as_str),
        Some("Process Viewer")
    );
    let notice = write_notice(
        &Job::Write(Operation::Remove(package.id.clone()), None),
        &Ok(Payload::Written(OperationOutcome::default())),
        false,
        &controller
            .rust()
            .names_for(&[Operation::Remove(package.id)]),
    );
    assert_eq!(notice["title"], "Remove Process Viewer (APT) finished");
    assert_eq!(notice["undo_action"], "install");
    controller.as_mut().cancel_queued();
    let worker = controller.as_mut().rust_mut().worker.take().unwrap();
    worker.handle.join().unwrap();
}
#[test]
fn opened_repositories_and_cleanups_confirm_in_plain_words() {
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().cleanup = vec![CleanupItem {
        id: CleanupId {
            backend: "apt".into(),
            key: "autoremove".into(),
        },
        title: "Unused packages".into(),
        summary: "Synthetic".into(),
        preview: "Removes 2 packages".into(),
        kind: CleanupKind::OrphanDependencies,
        bytes: None,
    }];
    propose_every_cleanup(&mut controller);
    assert!(controller
        .confirmation()
        .to_string()
        .contains("Unused packages (APT)"));
    controller.as_mut().change_repository(
        r#"{"backend":"flatpak","name":"flathub","scope":"system","action":"remove"}"#.into(),
    );
    assert!(controller
        .confirmation()
        .to_string()
        .contains("flathub, Flatpak, System"));
}

/// A Homebrew Casks stand-in: `adopts` is the app its install would
/// take over, `None` for an ordinary install.
struct CaskFixture {
    package: Package,
    adopts: Option<PathBuf>,
}
impl Backend for CaskFixture {
    fn id(&self) -> &str {
        "homebrew-cask"
    }
    fn capabilities(&self) -> &[Capability] {
        &[
            Capability::Search,
            Capability::Installed,
            Capability::Install,
        ]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn search(&mut self, _: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(vec![self.package.clone()])
    }
    fn operation_plan(
        &mut self,
        operation: &Operation,
        _: &Cancellation,
    ) -> Result<Option<TransactionPlan>, EngineError> {
        Ok(self.adopts.clone().map(|app| TransactionPlan {
            operation: operation.clone(),
            native_preview: "Obsidian is already in /Applications (version 1.2.3, signed by TEAM)."
                .into(),
            changes: vec![],
            download_bytes: None,
            disk_bytes: None,
            restart_required: None,
            adopts: Some(app),
        }))
    }
}
/// An AppImage source stand-in: the file it finds, and the copy its
/// install would move in (`None` once it's no longer installed elsewhere).
struct AppImageFixture {
    package: Package,
    adopts: Option<PathBuf>,
}
impl Backend for AppImageFixture {
    fn id(&self) -> &str {
        "appimage"
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Search, Capability::Install]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn search(&mut self, query: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
        if query == "/fail" {
            return Err(EngineError::NotFound);
        }
        Ok(vec![self.package.clone()])
    }
    fn operation_plan(
        &mut self,
        operation: &Operation,
        _: &Cancellation,
    ) -> Result<Option<TransactionPlan>, EngineError> {
        Ok(self.adopts.clone().map(|original| TransactionPlan {
                operation: operation.clone(),
                native_preview: "Moves ~/AppImages/demo.appimage into PkgDeck's folder and replaces its menu entries.".into(),
                changes: vec![],
                download_bytes: None,
                disk_bytes: None,
                restart_required: None,
                adopts: Some(original),
            }))
    }
}
fn external_appimage() -> Package {
    let mut app = synthetic_package("/home/user/AppImages/demo.appimage", "Demo");
    app.id.backend = "appimage".into();
    app.id.scope = Scope::User { uid: 1000 };
    app.summary = "~/AppImages/demo.appimage".into();
    app.adopt_with = Some("appimage".into());
    app
}
fn plan_appimage_adoption_with(adopts: Option<&str>, app: Package) -> Reply {
    let mut file = app.clone();
    file.id.scope = Scope::Environment {
        path: "/home/user/.local/share/pkgdeck/appimages".into(),
    };
    file.id.reference = Some("ab".repeat(32));
    file.summary = "Does demo things".into();
    let mut engine = Engine::default();
    engine
        .register(AppImageFixture {
            package: file,
            adopts: adopts.map(PathBuf::from),
        })
        .unwrap();
    let mut replies = Vec::new();
    execute(
        &mut engine,
        Job::PlanAdoption(Box::new(app)),
        &Cancellation::default(),
        &mut |reply| replies.push(reply),
    );
    replies.pop().unwrap()
}
#[test]
fn appimage_adoption_previews_the_move_the_appimage_source_plans() {
    let app = external_appimage();
    assert!(matches!(
        plan_appimage_adoption_with(Some(&app.id.name), app.clone()),
        Reply::Done(Ok(Payload::AdoptionPreview(file, Operation::Install(id), plan)))
            if file.id == id
                && id.name == app.id.name
                && plan.adopts.as_deref() == Some(std::path::Path::new(&app.id.name))
    ));
    // No longer installed elsewhere: nothing to take over.
    assert!(matches!(
        plan_appimage_adoption_with(None, app.clone()),
        Reply::Done(Err(EngineError::Refused { refusal: Refusal::Explained(reason), .. })) if reason.contains("isn't installed some other way anymore")
    ));
    // A failed read of the file is reported as is.
    let mut failing = app.clone();
    failing.id.name = "/fail".into();
    assert!(matches!(
        plan_appimage_adoption_with(Some("/fail"), failing),
        Reply::Done(Err(EngineError::NotFound))
    ));
    // Only rows PkgDeck can take over are planned.
    let mut managed = app;
    managed.adopt_with = None;
    assert!(matches!(
        plan_appimage_adoption_with(Some("/x"), managed),
        Reply::Done(Err(EngineError::NotFound))
    ));
}
#[test]
fn managing_every_appimage_reviews_them_once() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let demo = external_appimage();
    let mut other = external_appimage();
    other.id.name = "/home/user/AppImages/other.appimage".into();
    other.display_name = "Other".into();
    // Planned on a worker: what can move, and what can't any more.
    let mut file = demo.clone();
    file.id.reference = Some("ab".repeat(32));
    let mut engine = Engine::default();
    engine
        .register(AppImageFixture {
            package: file,
            adopts: Some(PathBuf::from(&demo.id.name)),
        })
        .unwrap();
    let plan_all = |engine: &mut Engine, cancel: &Cancellation| {
        let mut replies = Vec::new();
        execute(
            engine,
            Job::PlanAdoptAll(vec![demo.clone(), other.clone()]),
            cancel,
            &mut |reply| replies.push(reply),
        );
        replies.pop().unwrap()
    };
    assert!(matches!(
        plan_all(&mut engine, &Cancellation::default()),
        Reply::Done(Ok(Payload::AdoptAllPreview(plans, skipped)))
            if plans.len() == 1 && skipped == ["Other"]
    ));
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(matches!(
        plan_all(&mut engine, &cancelled),
        Reply::Done(Err(EngineError::Cancelled))
    ));
    // One review lists every move, then runs them all.
    let plan = |app: &Package, version: Option<&str>| {
        let operation = Operation::Install(app.id.clone());
        let plan = TransactionPlan {
            operation: operation.clone(),
            native_preview: "Moves it into PkgDeck's folder.".into(),
            changes: vec![PlannedChange {
                action: PlannedAction::Install,
                name: app.display_name.clone(),
                installed_version: None,
                candidate_version: version.map(str::to_owned),
            }],
            download_bytes: None,
            disk_bytes: None,
            restart_required: None,
            adopts: Some(PathBuf::from(&app.id.name)),
        };
        (app.clone(), operation, Box::new(plan))
    };
    controller.as_mut().rust_mut().packages = vec![demo.clone(), other.clone()];
    controller.as_mut().apply(Ok(Payload::AdoptAllPreview(
        vec![plan(&demo, Some("2.0")), plan(&other, None)],
        vec![],
    )));
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert_eq!(data["action"], "Manage all");
    assert_eq!(
        data["changes"],
        json!(["Move Demo 2.0 into PkgDeck", "Move Other into PkgDeck"])
    );
    assert!(data["summary"]
        .as_str()
        .unwrap()
        .starts_with("Manage 2 AppImages with PkgDeck"));
    controller.as_mut().confirm(true);
    let queued: Vec<Vec<Operation>> = controller
        .rust()
        .worker
        .iter()
        .map(|worker| worker.job.operations())
        .chain(
            controller
                .rust()
                .confirmed_queue
                .iter()
                .map(|entry| entry.job.operations()),
        )
        .collect();
    assert_eq!(queued.len(), 2, "{}", queued.len());
    settle(&mut controller);
    // One that couldn't move is named, and the count reads right.
    controller.as_mut().apply(Ok(Payload::AdoptAllPreview(
        vec![plan(&demo, Some("2.0"))],
        vec!["Other".into()],
    )));
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert!(data["summary"]
        .as_str()
        .unwrap()
        .starts_with("Manage 1 AppImage with PkgDeck"));
    assert!(data["notes"][1].as_str().unwrap().contains("Other"));
    controller.as_mut().confirm(false);
    // A later review never runs the moves an earlier one listed.
    controller.as_mut().apply(Ok(Payload::AdoptAllPreview(
        vec![plan(&demo, Some("2.0")), plan(&other, None)],
        vec![],
    )));
    controller.as_mut().rust_mut().pending = Some(Job::PlanCleanAll(vec![]));
    controller.as_mut().confirm(true);
    assert!(controller.rust().confirmed_queue.is_empty());
    assert!(!controller
        .rust()
        .worker
        .as_ref()
        .is_some_and(|worker| matches!(worker.job, Job::Write(..))));
    settle(&mut controller);
    // Just one: the usual review, which says it moves.
    controller.as_mut().apply(Ok(Payload::AdoptAllPreview(
        vec![plan(&demo, Some("2.0"))],
        vec![],
    )));
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert_eq!(data["action"], "Manage with PkgDeck");
    assert!(
        data.to_string().contains("Move Demo 2.0 into PkgDeck"),
        "{data}"
    );
    controller.as_mut().confirm(false);
    // None left to move.
    controller
        .as_mut()
        .apply(Ok(Payload::AdoptAllPreview(vec![], vec!["Demo".into()])));
    assert_eq!(controller.confirmation().to_string(), "");
    assert!(controller.status().to_string().contains("No AppImage"));
    // The button plans every AppImage installed some other way.
    controller.as_mut().propose("adopt-all".into(), -1);
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::PlanAdoptAll(apps) if apps.len() == 2)
    ));
    settle(&mut controller);
    controller.as_mut().rust_mut().packages = vec![];
    controller.as_mut().propose("adopt-all".into(), -1);
    assert!(controller.rust().worker.is_none());
    // A failed plan says what it was for.
    let notice = preflight_notice(
        &Job::PlanAdoptAll(vec![demo]),
        &EngineError::NotFound,
        false,
        &Names::default(),
    )
    .unwrap();
    assert!(notice["title"]
        .as_str()
        .unwrap()
        .starts_with("Managing your AppImages with PkgDeck"));
}
#[test]
fn appimage_adoption_confirms_as_manage_with_pkgdeck() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let app = external_appimage();
    controller.as_mut().rust_mut().packages = vec![app.clone()];
    let reply = plan_appimage_adoption_with(Some(&app.id.name), app.clone());
    assert!(matches!(
        &reply,
        Reply::Done(Ok(Payload::AdoptionPreview(..)))
    ));
    if let Reply::Done(result) = reply {
        controller.as_mut().apply(result);
    }
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert_eq!(data["action"], "Manage with PkgDeck");
    assert!(
        data["summary"]
            .as_str()
            .unwrap()
            .starts_with("Manage Demo with PkgDeck\nAppImage\nMoves ~/AppImages/demo.appimage"),
        "{data}"
    );
    assert!(matches!(
        &controller.rust().pending,
        Some(Job::Write(Operation::Install(_), Some(plan))) if plan.adopts.is_some()
    ));
    controller.as_mut().confirm(false);
    // Opening a file that's installed some other way previews the move.
    let mut opened = app.clone();
    opened.id.scope = Scope::Environment {
        path: "/home/user/.local/share/pkgdeck/appimages".into(),
    };
    controller
        .as_mut()
        .apply(Ok(opened_payload(opened.clone())));
    // Its page offers Manage, which previews the move.
    let page: Value = serde_json::from_str(&controller.opened().to_string()).unwrap();
    assert_eq!(page["action"], "Manage");
    controller.as_mut().install_opened();
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::PlanAdoption(found) if found.id == opened.id)
    ));
    settle(&mut controller);
    // A failed plan names PkgDeck, not Homebrew.
    let notice = preflight_notice(
        &Job::PlanAdoption(Box::new(app)),
        &EngineError::NotFound,
        false,
        &Names::default(),
    )
    .unwrap();
    assert!(
        notice["title"]
            .as_str()
            .unwrap()
            .starts_with("Managing Demo with PkgDeck"),
        "{notice}"
    );
}
fn adoptable_app() -> Package {
    let mut app = synthetic_package("/Applications/Obsidian.app", "Obsidian");
    app.id.backend = "macos-apps".into();
    app.id.architecture = "unknown".into();
    app.id.reference = Some(app.id.name.clone());
    app.installed_version = Some("1.2.3".into());
    app.adopt_with = Some("obsidian".into());
    app
}
fn obsidian_cask() -> Package {
    let mut cask = synthetic_package("obsidian", "Obsidian");
    cask.id.backend = "homebrew-cask".into();
    cask.id.architecture = std::env::consts::ARCH.into();
    cask.id.scope = Scope::Environment {
        path: "/opt/homebrew".into(),
    };
    cask.installed_version = None;
    cask.candidate_version = Some("1.2.3".into());
    cask
}
fn plan_adoption_with(cask: Package, adopts: Option<&str>, app: Package) -> Reply {
    let mut engine = Engine::default();
    engine
        .register(CaskFixture {
            package: cask,
            adopts: adopts.map(PathBuf::from),
        })
        .unwrap();
    let mut replies = Vec::new();
    execute(
        &mut engine,
        Job::PlanAdoption(Box::new(app)),
        &Cancellation::default(),
        &mut |reply| replies.push(reply),
    );
    assert_eq!(replies.len(), 1);
    replies.pop().unwrap()
}
#[test]
fn adoption_resolves_the_real_cask_and_previews_only_this_copy() {
    let cask = obsidian_cask();
    // The identity, scope and prefix come from the cask source.
    assert!(matches!(
        plan_adoption_with(
            cask.clone(),
            Some("/Applications/Obsidian.app"),
            adoptable_app(),
        ),
        Reply::Done(Ok(Payload::AdoptionPreview(found, Operation::Install(id), plan)))
            if *found == cask
                && id == cask.id
                && plan.adopts.as_deref()
                    == Some(std::path::Path::new("/Applications/Obsidian.app"))
    ));
    let refused = |reply: Reply, words: &str| {
        assert!(
            matches!(
                &reply,
                Reply::Done(Err(EngineError::Refused { backend, refusal: Refusal::Explained(reason) }))
                    if backend == "homebrew-cask"
                        && reason.contains(words)
                        && reason.ends_with("Nothing was changed.")
            ),
            "expected a refusal naming {words:?}"
        );
    };
    // An install that would add a copy elsewhere, or no copy at all.
    refused(
        plan_adoption_with(
            cask.clone(),
            Some("/Applications/Other/Obsidian.app"),
            adoptable_app(),
        ),
        "would install another copy instead of managing /Applications/Obsidian.app",
    );
    refused(
        plan_adoption_with(cask.clone(), None, adoptable_app()),
        "would install another copy",
    );
    // Homebrew already has the cask: nothing to hand over.
    let mut installed = cask.clone();
    installed.installed_version = Some("1.2.3".into());
    refused(
        plan_adoption_with(
            installed,
            Some("/Applications/Obsidian.app"),
            adoptable_app(),
        ),
        "already has the obsidian cask installed",
    );
    // An unnamed app is called by its cask.
    let mut installed = cask.clone();
    installed.installed_version = Some("1.2.3".into());
    let mut unnamed = adoptable_app();
    unnamed.display_name.clear();
    refused(
        plan_adoption_with(installed, Some("/Applications/Obsidian.app"), unnamed),
        "can't take over this copy of obsidian.",
    );
    // A cancelled lookup stops before any plan.
    let mut engine = Engine::default();
    engine
        .register(CaskFixture {
            package: cask.clone(),
            adopts: None,
        })
        .unwrap();
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(matches!(
        plan_adoption(&mut engine, &adoptable_app(), &cancel),
        Err(EngineError::Cancelled)
    ));
    // Only macOS inventory rows that name a cask are resolved.
    let mut plain = adoptable_app();
    plain.adopt_with = None;
    let mut foreign = adoptable_app();
    foreign.id.backend = "apt".into();
    for app in [plain, foreign] {
        assert!(matches!(
            plan_adoption_with(cask.clone(), Some("/Applications/Obsidian.app"), app),
            Reply::Done(Err(EngineError::NotFound))
        ));
    }
    assert_eq!(
        engine_source(&Job::PlanAdoption(Box::new(adoptable_app())), &[]),
        ["homebrew-cask"]
    );
    // The AppImage source takes over AppImages itself.
    let mut appimage = adoptable_app();
    appimage.id.backend = "appimage".into();
    appimage.adopt_with = Some("appimage".into());
    assert_eq!(
        engine_source(&Job::PlanAdoption(Box::new(appimage)), &[]),
        ["appimage"]
    );
}

#[test]
fn appimages_open_whatever_their_file_is_called() {
    let dir = std::env::temp_dir().join(format!("pkgdeck-appimage-magic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut header = vec![0_u8; 64];
    header[..4].copy_from_slice(b"\x7fELF");
    header[8..11].copy_from_slice(b"AI\x02");
    let named = dir.join("tool");
    std::fs::write(&named, &header).unwrap();
    assert!(is_appimage(&named));
    header[8] = 0;
    std::fs::write(&named, &header).unwrap();
    assert!(!is_appimage(&named));
    assert!(!is_appimage(&dir.join("missing")));
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn adoption_is_proposed_only_for_rows_a_cask_can_manage() {
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    let mut plain = adoptable_app();
    plain.adopt_with = None;
    let mut other = synthetic_package("obsidian", "Obsidian");
    other.adopt_with = Some("obsidian".into());
    controller.as_mut().rust_mut().packages = vec![adoptable_app(), plain, other];
    assert_eq!(
        package_row(&adoptable_app(), &[], None)["adopt_with"],
        "obsidian"
    );
    assert!(package_row(&controller.rust().packages[1], &[], None)["adopt_with"].is_null());
    // A quiet background read keeps the job queued, so no real Homebrew
    // is asked.
    controller.as_mut().rust_mut().background = true;
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Load("Installed".into(), "".into()),
        vec![],
    ));
    for index in [1, 2, 3, -1] {
        controller.as_mut().propose("adopt".into(), index);
        assert!(controller.rust().pending.is_none());
        assert!(!matches!(
            controller.rust().queued,
            Some(Job::PlanAdoption(_))
        ));
    }
    controller.as_mut().propose("adopt".into(), 0);
    assert!(matches!(
        &controller.rust().queued,
        Some(Job::PlanAdoption(app)) if **app == adoptable_app()
    ));
    // The read-only row still never offers its own writes.
    controller.as_mut().rust_mut().queued = None;
    for action in ["install", "remove", "upgrade"] {
        controller.as_mut().propose(action.into(), 0);
        assert!(controller.rust().pending.is_none());
        assert!(controller.rust().queued.is_none());
    }
    controller.as_mut().rust_mut().worker = None;
}
#[test]
fn adoption_confirms_in_plain_words_and_offers_no_undo() {
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().packages = vec![adoptable_app()];
    let cask = obsidian_cask();
    let operation = Operation::Install(cask.id.clone());
    let plan = TransactionPlan {
        operation: operation.clone(),
        native_preview: "Obsidian is already in /Applications (version 1.2.3, signed by TEAM)."
            .into(),
        changes: vec![],
        download_bytes: None,
        disk_bytes: None,
        restart_required: None,
        adopts: Some("/Applications/Obsidian.app".into()),
    };
    controller.as_mut().apply(Ok(Payload::AdoptionPreview(
        Box::new(cask.clone()),
        operation.clone(),
        Box::new(plan.clone()),
    )));
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert_eq!(data["action"], "Manage with Homebrew");
    let summary = data["summary"].as_str().unwrap();
    assert!(
        summary.starts_with("Manage Obsidian with Homebrew\nHomebrew Casks\n"),
        "{summary}"
    );
    assert!(summary.contains("already in /Applications"));
    assert!(controller
        .confirmation()
        .to_string()
        .contains("already in /Applications"));
    assert!(
        matches!(&controller.rust().pending, Some(Job::Write(op, Some(reviewed))) if *op == operation && **reviewed == plan)
    );
    assert_eq!(
        controller.rust().names.get(&cask.id).map(String::as_str),
        Some("Obsidian")
    );
    // Removing the cask afterwards would delete the app the user had.
    let job = Job::Write(operation.clone(), Some(Box::new(plan)));
    let notice = write_notice(
        &job,
        &Ok(Payload::Written(OperationOutcome::default())),
        false,
        &controller.rust().names,
    );
    assert_eq!(notice["kind"], "success");
    assert_eq!(notice["undo"], false);
    assert_eq!(notice["undo_action"], "");
    let plain = write_notice(
        &Job::Write(operation, None),
        &Ok(Payload::Written(OperationOutcome::default())),
        false,
        &controller.rust().names,
    );
    assert_eq!(plain["undo"], true);
    // A refused adoption explains itself before anything runs.
    let refusal = preflight_notice(
            &Job::PlanAdoption(Box::new(adoptable_app())),
            &EngineError::Refused {
                backend: "homebrew-cask".into(),
                refusal: Refusal::Explained("The obsidian cask would install another copy instead of managing /Applications/Obsidian.app. Nothing was changed.".into()),
            },
            false,
            &Names::new(),
        )
        .unwrap();
    assert_eq!(
        refusal["title"],
        "Managing Obsidian with Homebrew couldn't be prepared"
    );
    assert!(refusal["detail"].as_str().unwrap().contains("another copy"));
    controller.as_mut().confirm(false);
    assert!(controller.rust().pending.is_none());
}

/// A package from the synthetic system: installed, with an update.
fn fixture_row() -> Package {
    let mut package = synthetic_package("synthetic", "Synthetic");
    package.id.backend = "fixture".into();
    package.candidate_version = Some("2".into());
    package.update = UpdateAvailability::Available;
    package
}
fn fixture_engine(
    _: &[String],
    _: bool,
    _: Authorization,
    _: &Cancellation,
) -> Result<Engine, EngineError> {
    let mut engine = Engine::default();
    engine.register(Fixture {
        package: fixture_row(),
        fail: false,
    })?;
    Ok(engine)
}
fn missing_engine(
    sources: &[String],
    _: bool,
    _: Authorization,
    _: &Cancellation,
) -> Result<Engine, EngineError> {
    Err(EngineError::Unavailable {
        backend: sources.join(","),
        reason: "synthetic system has no managers".into(),
    })
}
/// A host with no tools on its PATH, so nothing native can run.
fn bare_host() -> pkgdeck_core::host::Host {
    pkgdeck_core::host::Host::new(
        pkgdeck_core::host::Runtime::Native,
        BTreeMap::from([("PATH".into(), "/nonexistent/pkgdeck-tests".into())]),
    )
}
/// A machine without package managers: every known source is absent,
/// and an unknown one is refused as the real engine refuses it.
fn no_engine(
    sources: &[String],
    _: bool,
    _: Authorization,
    _: &Cancellation,
) -> Result<Engine, EngineError> {
    match sources
        .iter()
        .find(|source| !pkgdeck_core::backends::BACKEND_IDS.contains(&source.as_str()))
    {
        Some(unknown) => Err(EngineError::UnknownBackend(unknown.clone())),
        None => Ok(Engine::default()),
    }
}
fn no_catalog() -> crate::metadata::Catalog {
    crate::metadata::Catalog::default()
}
fn offline(_: &str, _: &Cancellation) -> String {
    String::new()
}
/// No local catalog and no provider lookups, so details never leave the test.
const NO_METADATA: crate::metadata::Sources = crate::metadata::Sources {
    catalog: no_catalog,
    fetch: offline,
};
/// A machine without the helper: approving never reaches a password prompt.
fn no_approval(
    _: &pkgdeck_core::host::Host,
    _: bool,
    _: &Cancellation,
) -> Result<String, ExecutionError> {
    Err(ExecutionError::Disabled(
        "PkgDeck's system helper is not installed".into(),
    ))
}
/// The password was given: the approval names a synthetic helper.
fn synthetic_approval(
    _: &pkgdeck_core::host::Host,
    grant: bool,
    _: &Cancellation,
) -> Result<String, ExecutionError> {
    Ok(if grant { "synthetic-helper" } else { "" }.into())
}
pub(super) const NO_MANAGERS: Natives = Natives {
    engine: no_engine,
    host: bare_host,
    approve: no_approval,
    root: "/nonexistent/pkgdeck-tests",
    metadata: NO_METADATA,
    warm_catalog: false,
    hold: super::never_hold,
};
#[test]
fn sources_are_checked_again_on_request() {
    let mut controller = ffi::create_synthetic_controller(fixture_engine);
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().catalog_checked = true;
    controller.as_mut().check_sources();
    assert!(controller.rust().catalog_worker.is_none());
    controller.as_mut().recheck_sources();
    assert!(controller.rust().catalog_worker.is_some());
}
#[test]
fn synthetic_controllers_stay_off_the_system() {
    let mut controller = ffi::create_synthetic_controller(fixture_engine);
    let natives = controller.rust().natives;
    assert_eq!(natives.root, "/nonexistent/pkgdeck");
    assert!(!natives.warm_catalog);
    assert!((natives.metadata.fetch)("https://flathub.org/x", &Cancellation::default()).is_empty());
    let _ = (natives.metadata.catalog)();
    assert!((natives.approve)(&(natives.host)(), true, &Cancellation::default()).is_err());
    assert!(controller.rust().activity_store.is_none());
    let mut controller = controller.pin_mut();
    controller.as_mut().check_sources();
    settle(&mut controller);
    let catalog: Value = serde_json::from_str(&controller.source_catalog().to_string()).unwrap();
    assert_eq!(catalog[0]["source"], "fixture");
}
#[test]
fn production_workers_use_the_running_system() {
    assert_eq!(NATIVES.root, "/");
    let cancel = Cancellation::default();
    assert!(matches!(
        (NO_MANAGERS.engine)(&["missing-fixture".into()], false, Authorization::Polkit, &cancel),
        Err(EngineError::UnknownBackend(name)) if name == "missing-fixture"
    ));
    let empty = (NO_MANAGERS.engine)(&["apt".into()], true, Authorization::Polkit, &cancel);
    assert!(empty.unwrap().discover(&cancel).is_empty());
    // Every provider lookup from a test answers nothing.
    assert!((NO_MANAGERS.metadata.fetch)(
        "https://flathub.org/api/v2/appstream/org.example.App",
        &cancel
    )
    .is_empty());
}
const SYNTHETIC: Natives = Natives {
    engine: fixture_engine,
    host: bare_host,
    approve: no_approval,
    root: "/nonexistent/pkgdeck-tests",
    metadata: NO_METADATA,
    warm_catalog: false,
    hold: super::never_hold,
};
fn synthetic_controller() -> crate::qt::UniquePtr<ffi::PackageController> {
    let mut controller = idle_controller();
    controller.pin_mut().rust_mut().natives = SYNTHETIC;
    controller
}
fn settle(controller: &mut Pin<&mut ffi::PackageController>) {
    wait_until(controller, |controller| {
        controller.rust().worker.is_none()
            && controller.rust().details_worker.is_none()
            && controller.rust().catalog_worker.is_none()
    });
}
#[test]
fn background_checks_repositories_and_source_catalogs_use_the_system_workers() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .check_updates("apt,,flatpak".into(), true, false, false, true);
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::BackgroundUpdates(sources) if sources == &["apt", "flatpak"])
    ));
    assert!(!controller.busy());
    settle(&mut controller);
    let state: Value = serde_json::from_str(&controller.background_state().to_string()).unwrap();
    assert_eq!(state["available"], 1);

    controller.as_mut().load_repositories();
    settle(&mut controller);
    let repositories: Value = serde_json::from_str(&controller.repositories().to_string()).unwrap();
    assert_eq!(repositories["repositories"], json!([]));
    assert_eq!(controller.status().to_string(), "Repositories loaded.");

    controller.as_mut().check_sources();
    settle(&mut controller);
    let catalog: Value = serde_json::from_str(&controller.source_catalog().to_string()).unwrap();
    assert_eq!(catalog[0]["source"], "fixture");
    assert_eq!(catalog[0]["availability_kind"], "available");
    assert!(controller.rust().catalog_checked);
}
/// Answers details for its one package.
struct Described(Package);
impl Backend for Described {
    fn id(&self) -> &str {
        &self.0.id.backend
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Details]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn details(&mut self, _: &PackageId, _: &Cancellation) -> Result<PackageDetails, EngineError> {
        Ok(PackageDetails {
            package: self.0.clone(),
            description: "Described by the warm engine".into(),
            homepage: None,
            dependencies: vec!["libsynthetic".into()],
        })
    }
}
#[test]
fn details_reuse_a_warm_engine_and_fall_back_to_a_fresh_one() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let mut row = fixture_row();
    row.id.backend = "described".into();
    let mut warm = Engine::default();
    warm.register(Described(row.clone())).unwrap();
    assert_eq!(
        warm.discover(&Cancellation::default())[0].availability,
        Ok(Availability::Available)
    );
    controller.as_mut().rust_mut().packages = vec![row.clone()];
    controller.as_mut().rust_mut().selected = Some(row.id.clone());
    controller.as_mut().rust_mut().engine = Some(warm);
    controller.as_mut().start(Job::Details(row.id.clone()));
    settle(&mut controller);
    let details: Value = serde_json::from_str(&controller.details().to_string()).unwrap();
    assert_eq!(details["description"], "Described by the warm engine");
    assert_eq!(details["dependencies"], json!(["libsynthetic"]));
    // The warm engine goes back for the next lookup.
    assert!(controller.rust().engine.is_some());

    // The warm engine's own failure is reported as is.
    let mut warm = Engine::default();
    warm.register(Fixture {
        package: fixture_row(),
        fail: false,
    })
    .unwrap();
    controller.as_mut().rust_mut().engine = Some(warm);
    controller.as_mut().start(Job::Details(fixture_row().id));
    settle(&mut controller);
    assert_eq!(
        controller.status().to_string(),
        plain_error(&EngineError::NotFound, None, false)
    );
    assert!(controller.rust().engine.is_some());

    // A source it never detected needs a fresh engine.
    controller.as_mut().rust_mut().natives.engine = missing_engine;
    controller.as_mut().start(Job::Details(row.id.clone()));
    settle(&mut controller);
    assert!(controller
        .status()
        .to_string()
        .contains("synthetic system has no managers"));
}

/// A worker whose thread keeps running until the returned gate drops.
fn held_worker(job: Job, replies: Vec<Reply>) -> (Worker, mpsc::Sender<()>) {
    let (sender, receiver) = mpsc::channel();
    for reply in replies {
        sender.send(reply).unwrap();
    }
    let (gate, held) = mpsc::channel::<()>();
    let worker = Worker {
        handle: thread::spawn(move || {
            let _ = held.recv();
            drop(sender);
        }),
        receiver,
        cancel: Cancellation::default(),
        job,
    };
    (worker, gate)
}
fn failing_worker(job: Job) -> Worker {
    Worker {
        handle: thread::spawn(|| panic!("synthetic worker failure")),
        receiver: mpsc::channel().1,
        cancel: Cancellation::default(),
        job,
    }
}
fn fixture_details(description: &str) -> Box<PackageDetails> {
    Box::new(PackageDetails {
        package: fixture_row(),
        description: description.into(),
        homepage: None,
        dependencies: vec![],
    })
}
#[test]
fn background_workers_keep_catalogs_and_report_their_own_failure() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let source = Source {
        backend: "fixture".into(),
        capabilities: vec![],
        availability: Ok(Availability::Available),
    };
    controller.as_mut().rust_mut().background = true;
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Load("Sources".into(), String::new()),
        vec![
            Reply::Engine(Box::default()),
            Reply::Done(Ok(Payload::Sources(vec![source]))),
        ],
    ));
    controller.as_mut().poll();
    assert!(controller.rust().worker.is_none());
    assert!(controller.rust().catalog_checked);
    assert!(controller.source_catalog().to_string().contains("fixture"));
    // A quiet read never leaves its engine for later searches.
    assert!(controller.rust().engine.is_none());
    assert!(controller
        .rust()
        .prefetched
        .contains_key(&cache_key("Sources", "", &[], false)));

    // A quiet result for anything but a section is not kept.
    controller.as_mut().rust_mut().background = true;
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Details(fixture_row().id),
        vec![Reply::Done(Ok(Payload::Details(fixture_details("Quiet"))))],
    ));
    controller.as_mut().poll();
    assert!(!controller.details().to_string().contains("Quiet"));
    assert_eq!(controller.rust().prefetched.len(), 1);

    controller.as_mut().rust_mut().background = true;
    controller.as_mut().rust_mut().worker =
        Some(failing_worker(Job::BackgroundUpdates(vec!["apt".into()])));
    wait_until(&mut controller, |controller| {
        controller.rust().worker.is_none()
    });
    let state: Value = serde_json::from_str(&controller.background_state().to_string()).unwrap();
    assert_eq!(
        state["failures"],
        json!([{"source": "check", "kind": "failed"}])
    );

    let status = controller.status().to_string();
    controller.as_mut().rust_mut().worker =
        Some(failing_worker(Job::Load("Installed".into(), String::new())));
    wait_until(&mut controller, |controller| {
        controller.rust().worker.is_none()
    });
    assert_ne!(controller.status().to_string(), status);
    assert_eq!(
        controller.status().to_string(),
        "A background task failed. Try again."
    );
}
#[test]
fn running_and_finished_workers_show_details_previews() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let row = fixture_row();
    controller.as_mut().rust_mut().packages = vec![row.clone()];
    controller.as_mut().rust_mut().selected = Some(row.id.clone());
    let (worker, gate) = held_worker(
        Job::Load("Installed".into(), String::new()),
        vec![
            Reply::Partial(PackageReport {
                packages: vec![row.clone()],
                failures: vec![],
                successful_sources: vec![],
            }),
            Reply::DetailsPreview(fixture_details("While loading")),
            Reply::Engine(Box::default()),
        ],
    );
    controller.as_mut().rust_mut().worker = Some(worker);
    controller.as_mut().poll();
    assert!(controller.rust().worker.is_some());
    assert!(controller.details().to_string().contains("While loading"));
    assert!(controller.rows().to_string().contains("synthetic"));
    // A running worker's engine is not taken early.
    assert!(controller.rust().engine.is_none());
    drop(gate);
    settle(&mut controller);

    controller.as_mut().rust_mut().detail_cache.clear();
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Details(row.id.clone()),
        vec![
            Reply::DetailsPreview(fixture_details("Preview")),
            Reply::Done(Ok(Payload::Details(fixture_details("Finished")))),
        ],
    ));
    controller.as_mut().poll();
    assert!(controller.rust().worker.is_none());
    assert!(controller.details().to_string().contains("Finished"));

    // Flathub's description and screenshots are fetched after the first
    // look, which says more is coming and isn't kept for reopening.
    let mut flathub = fixture_details("");
    flathub.package.id.backend = "flatpak".into();
    flathub.package.id.name = "io.example.PreviewOnly".into();
    flathub.package.id.remote = Some("flathub".into());
    let more = |controller: &Pin<&mut ffi::PackageController>| {
        serde_json::from_str::<Value>(&controller.details().to_string()).unwrap()["more"] == true
    };
    controller.as_mut().rust_mut().packages = vec![flathub.package.clone()];
    controller.as_mut().rust_mut().selected = Some(flathub.package.id.clone());
    controller.as_mut().rust_mut().detail_cache.clear();
    let (worker, gate) = held_worker(
        Job::Details(flathub.package.id.clone()),
        vec![Reply::DetailsPreview(flathub.clone())],
    );
    controller.as_mut().rust_mut().worker = Some(worker);
    controller.as_mut().poll();
    assert!(more(&controller));
    assert!(controller.rust().detail_cache.is_empty());
    drop(gate);
    settle(&mut controller);
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Details(flathub.package.id.clone()),
        vec![Reply::Done(Ok(Payload::Details(flathub)))],
    ));
    controller.as_mut().poll();
    assert!(!more(&controller));
    assert_eq!(controller.rust().detail_cache.len(), 1);
}
#[test]
fn finished_reviews_notices_and_activity_follow_the_job() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let row = fixture_row();
    controller.as_mut().rust_mut().packages = vec![row.clone()];
    let install = Operation::Install(row.id.clone());

    // Only reviews are dropped after their confirmation was cancelled.
    controller.as_mut().rust_mut().discard_revalidation = true;
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Load("Installed".into(), String::new()),
        vec![Reply::Done(Ok(Payload::Packages(PackageReport {
            packages: vec![row.clone()],
            failures: vec![],
            successful_sources: vec!["fixture".into()],
        })))],
    ));
    controller.as_mut().poll();
    assert!(controller.rows().to_string().contains("synthetic"));
    assert!(!controller.rust().discard_revalidation);
    controller.as_mut().rust_mut().discard_revalidation = true;
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::PlanOperation(install.clone()),
        vec![Reply::Done(Ok(Payload::OperationPreview(
            install.clone(),
            None,
        )))],
    ));
    controller.as_mut().poll();
    assert!(controller.rust().pending.is_none());
    assert!(!controller.rust().discard_revalidation);

    // A review that cannot be prepared says so.
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::PlanOperation(install.clone()),
        vec![Reply::Done(Err(ExecutionError::AuthorizationDenied.into()))],
    ));
    controller.as_mut().poll();
    let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
    assert_eq!(notice["kind"], "error");
    assert_eq!(
        notice["title"],
        "Install synthetic (fixture) couldn't be prepared"
    );

    // A cancelled write records a cancelled outcome.
    let temp = pkgdeck_tools::Temp::new();
    let store = ActivityStore::at(temp.0.join("activity.json"));
    let upgrade = Operation::Upgrade(row.id.clone());
    let id = store
        .begin("gui", vec![upgrade.clone()], vec![], State::Running)
        .unwrap();
    controller.as_mut().rust_mut().activity_store = Some(store.clone());
    controller.as_mut().rust_mut().active_activity_id = Some(id);
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Write(upgrade.clone(), None),
        vec![Reply::Done(Err(EngineError::Cancelled))],
    ));
    controller.as_mut().poll();
    let entry = store.entries().unwrap().pop().unwrap();
    assert_eq!(entry.outcomes, [Outcome::Cancelled]);
    let finished = store
        .begin("gui", vec![upgrade.clone()], vec![], State::Running)
        .unwrap();
    controller.as_mut().rust_mut().active_activity_id = Some(finished);
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Write(upgrade.clone(), None),
        vec![Reply::Done(Ok(Payload::Written(
            OperationOutcome::default(),
        )))],
    ));
    controller.as_mut().poll();
    let entry = store.entries().unwrap().pop().unwrap();
    assert_eq!(
        (entry.id, entry.outcomes),
        (finished, vec![Outcome::Finished])
    );
    assert!(controller.rust().active_activity_id.is_none());

    // Without a history the finished write only forgets its entry.
    controller.as_mut().rust_mut().activity_store = None;
    controller.as_mut().rust_mut().active_activity_id = Some(finished + 1);
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Write(upgrade, None),
        vec![Reply::Done(Ok(Payload::Written(
            OperationOutcome::default(),
        )))],
    ));
    controller.as_mut().poll();
    assert!(controller.rust().active_activity_id.is_none());
    assert_eq!(store.entries().unwrap().len(), 2);
}
#[test]
fn retried_update_sources_plan_the_update_they_were_retried_for() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().active_view = "Updates".into();
    controller.as_mut().rust_mut().updates_view = true;
    let retried = |packages: Vec<Package>| {
        fake_worker(
            Job::RetryFailedUpdates(vec!["fixture".into()]),
            vec![Reply::Done(Ok(Payload::RetryFailedUpdates(
                vec!["fixture".into()],
                PackageReport {
                    packages,
                    failures: vec![],
                    successful_sources: vec!["fixture".into()],
                },
            )))],
        )
    };
    controller.as_mut().rust_mut().worker = Some(retried(vec![]));
    controller.as_mut().poll();
    assert_eq!(
        controller.status().to_string(),
        "No available updates after retrying source checks."
    );
    assert!(controller.rust().worker.is_none());

    controller.as_mut().rust_mut().worker = Some(retried(vec![fixture_row()]));
    controller.as_mut().poll();
    let upgrade = Operation::UpgradeAll {
        backend: "fixture".into(),
    };
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::PlanUpgrade(operations, 1) if operations == std::slice::from_ref(&upgrade))
    ));
    settle(&mut controller);
    assert!(matches!(
        &controller.rust().pending,
        Some(Job::UpgradeAll(operations, None)) if operations == &[upgrade]
    ));
}
#[test]
fn finished_workers_start_confirmed_reviewed_and_deferred_jobs() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let row = fixture_row();
    controller.as_mut().rust_mut().packages = vec![row.clone()];
    let upgrade = Operation::Upgrade(row.id.clone());
    let done = || {
        fake_worker(
            Job::Load("Search".into(), "synthetic".into()),
            vec![Reply::Done(Err(EngineError::Cancelled))],
        )
    };
    let confirmed = || Confirmed {
        job: Job::Write(upgrade.clone(), None),
        activity_id: None,
        cleanup_preview: vec![],
    };

    controller.as_mut().rust_mut().validated_confirmed = Some(confirmed());
    controller.as_mut().rust_mut().worker = Some(done());
    controller.as_mut().poll();
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::Write(operation, None) if *operation == upgrade)
    ));
    assert!(controller.writing());
    settle(&mut controller);
    let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
    assert_eq!(notice["kind"], "success");

    controller
        .as_mut()
        .rust_mut()
        .confirmed_queue
        .push_back(confirmed());
    controller.as_mut().rust_mut().worker = Some(done());
    controller.as_mut().poll();
    assert!(controller.rust().revalidating.is_some());
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::PlanOperation(operation) if *operation == upgrade)
    ));
    settle(&mut controller);

    controller.as_mut().rust_mut().deferred_load =
        Some(Job::Load("Installed".into(), String::new()));
    controller.as_mut().rust_mut().worker = Some(done());
    controller.as_mut().poll();
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::Load(view, _) if view == "Installed")
    ));
    settle(&mut controller);
    assert!(controller.rows().to_string().contains("synthetic"));
}

fn sized(backend: &str, key: &str, kind: CleanupKind, bytes: Option<u64>) -> CleanupItem {
    CleanupItem {
        id: CleanupId {
            backend: backend.into(),
            key: key.into(),
        },
        kind,
        title: format!("{key} task"),
        summary: "Synthetic cleanup".into(),
        preview: format!("Empties {key}"),
        bytes,
    }
}
struct UpdateCheckFixture {
    id: &'static str,
    index: bool,
    armed: bool,
    refreshed: bool,
    failure: bool,
    read_failure: bool,
    stop: bool,
    log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}
impl Backend for UpdateCheckFixture {
    fn id(&self) -> &str {
        self.id
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Installed, Capability::Refresh]
    }
    fn has_update_index(&self) -> bool {
        self.index
    }
    fn arm_update_check(&mut self, token: Option<u64>) {
        self.armed = token.is_some();
    }
    fn refresh_update_index(&mut self, _: &Cancellation) -> Result<(), EngineError> {
        if self.index && self.armed {
            self.log.lock().unwrap().push(format!("index:{}", self.id));
            if self.failure {
                return Err(ExecutionError::AuthorizationDenied.into());
            }
            self.refreshed = true;
        }
        Ok(())
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.log.lock().unwrap().push(format!("read:{}", self.id));
        if self.stop {
            cancel.cancel();
        }
        if self.read_failure {
            return Err(ExecutionError::Invalid("fixture read failed".into()).into());
        }
        let mut package = fixture_row();
        package.id.backend = self.id.into();
        package.update = if self.refreshed || self.id == "npm" {
            UpdateAvailability::Available
        } else {
            UpdateAvailability::Current
        };
        Ok(vec![package])
    }
    fn execute(
        &mut self,
        _: &Operation,
        cancel: &Cancellation,
        _: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("refresh:{}", self.id));
        if self.stop {
            cancel.cancel();
            return Err(EngineError::Cancelled);
        }
        if self.failure {
            return Err(ExecutionError::AuthorizationDenied.into());
        }
        self.refreshed = true;
        Ok(OperationOutcome::default())
    }
}
#[test]
fn refresh_explicit_update_checks_refresh_before_reading_and_keep_refresh_failures() {
    for failure in [false, true] {
        let log = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let mut engine = Engine::default();
        for id in ["apt", "npm"] {
            engine
                .register(UpdateCheckFixture {
                    index: false,
                    armed: false,
                    id,
                    refreshed: false,
                    failure: id == "apt" && failure,
                    read_failure: false,
                    stop: false,
                    log: log.clone(),
                })
                .unwrap();
        }
        let operations = vec![Operation::Refresh {
            backend: "apt".into(),
        }];
        let mut replies = vec![];
        execute(
            &mut engine,
            Job::CheckUpdates(operations.clone(), vec!["apt".into(), "npm".into()]),
            &Cancellation::default(),
            &mut |r| replies.push(r),
        );
        assert_eq!(log.lock().unwrap()[0], "refresh:apt");
        assert!(log.lock().unwrap().contains(&"read:npm".into()));
        assert!(!log.lock().unwrap().contains(&"refresh:npm".into()));
        let (report, ran) = expect!(replies.pop().unwrap(), Reply::Done(Ok(Payload::CheckedUpdates(report, ran))) => (report, ran));
        let outcomes: Vec<_> = ran.outcomes().collect();
        assert_eq!(report.packages.len(), if failure { 1 } else { 2 });
        assert_eq!(report.failures.len(), usize::from(failure));
        assert_eq!(report.successful_sources.contains(&"apt".into()), !failure);
        assert_eq!(
            outcomes,
            [if failure {
                Outcome::Failed
            } else {
                Outcome::Finished
            }]
        );
        if failure {
            assert_eq!(
                report.failures[0].error,
                EngineError::Execution(ExecutionError::AuthorizationDenied)
            );
            let retry = retry_job(
                &Job::CheckUpdates(operations, vec!["apt".into(), "npm".into()]),
                &Ok(Payload::Batch(ran)),
            );
            assert!(
                matches!(retry, Some(Job::CheckUpdates(ops, sources)) if ops.len() == 1 && sources == ["apt", "npm"])
            );
        }
    }
    // Registries without a refresh operation are still checked.
    let mut engine = Engine::default();
    let log = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    engine
        .register(UpdateCheckFixture {
            index: false,
            armed: false,
            id: "npm",
            refreshed: false,
            failure: false,
            read_failure: false,
            stop: false,
            log: log.clone(),
        })
        .unwrap();
    let mut replies = vec![];
    execute(
        &mut engine,
        Job::CheckUpdates(vec![], vec!["npm".into()]),
        &Cancellation::default(),
        &mut |r| replies.push(r),
    );
    assert_eq!(*log.lock().unwrap(), ["read:npm"]);
    assert!(
        matches!(replies.pop(), Some(Reply::Done(Ok(Payload::CheckedUpdates(report, ran)))) if report.packages.len() == 1 && ran.steps.is_empty())
    );
}
#[test]
fn refresh_explicit_checks_fetch_index_only_sources_once_and_preserve_their_failure() {
    for failure in [false, true] {
        let mut engine = Engine::default();
        let log = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        engine
            .register(UpdateCheckFixture {
                id: "npm",
                index: true,
                armed: false,
                refreshed: false,
                failure,
                read_failure: false,
                stop: false,
                log: log.clone(),
            })
            .unwrap();
        let mut replies = vec![];
        execute(
            &mut engine,
            Job::CheckUpdates(vec![], vec!["npm".into()]),
            &Cancellation::default(),
            &mut |r| replies.push(r),
        );
        let report = expect!(replies.pop().unwrap(), Reply::Done(Ok(Payload::CheckedUpdates(report, _))) => report);
        assert_eq!(report.failures.len(), usize::from(failure));
        assert_eq!(report.successful_sources.contains(&"npm".into()), !failure);
        assert_eq!(log.lock().unwrap().last().unwrap(), "read:npm");
        if !failure {
            assert_eq!(*log.lock().unwrap(), ["index:npm", "read:npm"]);
        }
    }
}

#[test]
fn refresh_failures_replace_only_the_same_sources_read_failure() {
    let mut engine = Engine::default();
    let log = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    for id in ["apt", "npm"] {
        engine
            .register(UpdateCheckFixture {
                id,
                index: false,
                armed: false,
                refreshed: false,
                failure: id == "apt",
                read_failure: true,
                stop: false,
                log: log.clone(),
            })
            .unwrap();
    }
    let mut replies = vec![];
    execute(
        &mut engine,
        Job::CheckUpdates(
            vec![Operation::Refresh {
                backend: "apt".into(),
            }],
            vec!["apt".into(), "npm".into()],
        ),
        &Cancellation::default(),
        &mut |r| replies.push(r),
    );
    let report = expect!(replies.pop().unwrap(), Reply::Done(Ok(Payload::CheckedUpdates(report, _))) => report);
    assert_eq!(report.failures.len(), 2);
    let apt = report.failures.iter().find(|f| f.backend == "apt").unwrap();
    assert_eq!(
        apt.error,
        EngineError::Execution(ExecutionError::AuthorizationDenied)
    );
    let npm = report.failures.iter().find(|f| f.backend == "npm").unwrap();
    assert!(
        matches!(&npm.error, EngineError::Execution(ExecutionError::Invalid(message)) if message == "fixture read failed")
    );
}

#[test]
fn refresh_failed_operations_do_not_claim_a_successful_check_and_keep_the_error_notice() {
    let mut controller = synthetic_controller();
    let mut ctl = controller.pin_mut();
    ctl.as_mut().rust_mut().active_view = "Installed".into();
    ctl.as_mut()
        .rust_mut()
        .last_success
        .insert("apt".into(), 123);
    ctl.as_mut().rust_mut().worker = Some(fake_worker(
        Job::CheckUpdates(
            vec![
                Operation::Refresh {
                    backend: "apt".into(),
                },
                Operation::Refresh {
                    backend: "flatpak".into(),
                },
            ],
            vec!["apt".into(), "flatpak".into()],
        ),
        vec![Reply::Done(Ok(Payload::CheckedUpdates(
            Box::default(),
            expect!(ended(&[Outcome::Failed, Outcome::Finished]), Payload::Batch(ran) => ran),
        )))],
    ));
    settle(&mut ctl);
    assert_eq!(ctl.rust().last_success["apt"], 123);
    assert!(ctl.rust().last_success["flatpak"] > 123);
    let notice: Value = serde_json::from_str(ctl.notice().as_str()).unwrap();
    assert_eq!(notice["kind"], "error");
    assert_eq!(notice["retry"], true);
}

#[test]
fn refresh_a_failed_check_without_a_report_is_not_reported_as_cancelled() {
    let mut controller = synthetic_controller();
    let mut ctl = controller.pin_mut();
    ctl.as_mut().rust_mut().worker = Some(fake_worker(
        Job::CheckUpdates(vec![], vec!["apt".into()]),
        vec![Reply::Done(Err(EngineError::Execution(
            ExecutionError::AuthorizationDenied,
        )))],
    ));
    settle(&mut ctl);
    let state: Value = serde_json::from_str(ctl.report_state().as_str()).unwrap();
    assert_ne!(state["phase"], "cancelled");
    let notice: Value = serde_json::from_str(ctl.notice().as_str()).unwrap();
    assert_eq!(notice["kind"], "error");
}

#[test]
fn refresh_checking_snap_and_macports_does_not_apply_their_software_updates() {
    let mut controller = synthetic_controller();
    let mut ctl = controller.pin_mut();
    ctl.as_mut().set_source_catalog(encoded(json!([
        {"source":"snap", "available":true, "capabilities":["refresh", "installed"]},
        {"source":"macports", "available":true, "capabilities":["refresh", "installed"]},
        {"source":"apt", "available":true, "capabilities":["refresh", "installed"]}
    ])));
    ctl.as_mut().check_for_updates("snap,macports,apt".into());
    let job = &ctl.rust().worker.as_ref().unwrap().job;
    let (ops, sources) = expect!(job, Job::CheckUpdates(ops, sources) => (ops, sources));
    // Snap's refresh only lists snaps. APT refreshes as the check reads
    // it, without a password, and MacPorts keeps its ports tree: its
    // refresh could update MacPorts.
    assert_eq!(
        ops,
        &[Operation::Refresh {
            backend: "snap".into()
        }]
    );
    assert_eq!(sources, &["apt", "macports", "snap"]);
    settle(&mut ctl);
}

#[test]
fn refresh_final_rows_are_prepared_on_the_worker_before_the_report_is_published() {
    fn engine(
        _: &[String],
        _: bool,
        _: Authorization,
        _: &Cancellation,
    ) -> Result<Engine, EngineError> {
        let mut engine = Engine::default();
        engine
            .register(UpdateCheckFixture {
                id: "npm",
                index: false,
                armed: false,
                refreshed: false,
                failure: false,
                read_failure: false,
                stop: false,
                log: std::sync::Arc::new(std::sync::Mutex::new(vec![])),
            })
            .unwrap();
        Ok(engine)
    }
    let mut controller = ffi::create_synthetic_controller(engine);
    let mut ctl = controller.pin_mut();
    ctl.as_mut().check_for_updates("npm".into());
    let worker = ctl.as_mut().rust_mut().worker.take().unwrap();
    worker.handle.join().unwrap();
    let replies: Vec<_> = worker.receiver.try_iter().collect();
    let rows = replies
        .iter()
        .position(|r| matches!(r, Reply::Rows(_)))
        .unwrap();
    let done = replies
        .iter()
        .position(|r| matches!(r, Reply::Done(Ok(Payload::CheckedUpdates(..)))))
        .unwrap();
    assert!(rows < done);
    let rows = expect!(&replies[rows], Reply::Rows(rows) => rows);
    assert!(rows.json.contains("npm"));
}

/// A source whose tool is missing: detection says it isn't installed.
fn not_installed(id: &str) -> Fake {
    Fake::new(id, &[Capability::Installed, Capability::Refresh])
        .unavailable(&format!("{id} not found"))
}
const NOT_INSTALLED: [&str; 25] = [
    "amp",
    "apk",
    "aur",
    "composer",
    "conda",
    "copilot",
    "cursor",
    "dnf",
    "dotnet",
    "droid",
    "gem",
    "kiro",
    "macos-apps",
    "macports",
    "mas",
    "mise",
    "nix",
    "pacman",
    "pip",
    "pixi",
    "system-image",
    "xbps",
    "zypper",
    "macos-updates",
    "oh-my-zsh",
];
#[test]
fn refresh_sources_that_are_not_installed_are_not_reported_as_failed_checks() {
    fn engine(
        _: &[String],
        _: bool,
        _: Authorization,
        _: &Cancellation,
    ) -> Result<Engine, EngineError> {
        let mut engine = Engine::default();
        for id in NOT_INSTALLED {
            engine.register(not_installed(id)).unwrap();
        }
        Ok(engine)
    }
    let mut controller = ffi::create_synthetic_controller(engine);
    let mut ctl = controller.pin_mut();
    ctl.as_mut()
        .check_for_updates(NOT_INSTALLED.join(",").into());
    settle(&mut ctl);
    let state: Value = serde_json::from_str(ctl.report_state().as_str()).unwrap();
    assert_eq!(state["phase"], "complete");
    assert_eq!(state["failures"], json!([]));
}
#[test]
fn refresh_skips_sources_that_cannot_run_here() {
    let mut controller = synthetic_controller();
    let mut ctl = controller.pin_mut();
    ctl.as_mut().set_source_catalog(encoded(json!([
        {"source":"macos-apps", "available":false, "capabilities":["installed"]},
        {"source":"pip", "available":false, "capabilities":["installed"]},
        {"source":"npm", "available":true, "capabilities":["installed"]}
    ])));
    ctl.as_mut().check_for_updates("macos-apps,npm,pip".into());
    let job = &ctl.rust().worker.as_ref().unwrap().job;
    let sources = expect!(job, Job::CheckUpdates(_, sources) => sources);
    assert_eq!(*sources, ["npm"]);
    settle(&mut ctl);
    // Nothing left to check: nothing starts.
    ctl.as_mut().check_for_updates("macos-apps,pip".into());
    assert!(ctl.rust().worker.is_none());
}
#[test]
fn refresh_a_real_error_still_reports_its_source_next_to_missing_ones() {
    fn engine(
        _: &[String],
        _: bool,
        _: Authorization,
        _: &Cancellation,
    ) -> Result<Engine, EngineError> {
        let mut engine = Engine::default();
        engine.register(not_installed("amp")).unwrap();
        engine
            .register(UpdateCheckFixture {
                id: "npm",
                index: false,
                armed: false,
                refreshed: false,
                failure: false,
                read_failure: true,
                stop: false,
                log: std::sync::Arc::new(std::sync::Mutex::new(vec![])),
            })
            .unwrap();
        Ok(engine)
    }
    let mut controller = ffi::create_synthetic_controller(engine);
    let mut ctl = controller.pin_mut();
    ctl.as_mut().check_for_updates("amp,npm".into());
    settle(&mut ctl);
    let state: Value = serde_json::from_str(ctl.report_state().as_str()).unwrap();
    assert_eq!(state["phase"], "failed");
    assert_eq!(state["failures"].as_array().unwrap().len(), 1);
    assert_eq!(state["failures"][0]["source"], "npm");
    assert_ne!(state["failures"][0]["kind"], "unavailable");
}

/// A source that is installed but broken: detection says it's unavailable
/// for a reason other than a missing tool.
#[test]
fn refresh_an_installed_but_broken_source_still_reports_a_failed_check() {
    fn engine(
        _: &[String],
        _: bool,
        _: Authorization,
        _: &Cancellation,
    ) -> Result<Engine, EngineError> {
        let mut engine = Engine::default();
        engine.register(not_installed("amp")).unwrap();
        engine
            .register(
                Fake::new("docker", &[Capability::Installed, Capability::Refresh])
                    .unavailable("Docker is installed but unavailable: daemon stopped"),
            )
            .unwrap();
        Ok(engine)
    }
    let mut controller = ffi::create_synthetic_controller(engine);
    let mut ctl = controller.pin_mut();
    ctl.as_mut().check_for_updates("amp,docker".into());
    settle(&mut ctl);
    let state: Value = serde_json::from_str(ctl.report_state().as_str()).unwrap();
    assert_eq!(state["phase"], "failed");
    assert_eq!(state["failures"].as_array().unwrap().len(), 1);
    assert_eq!(state["failures"][0]["source"], "docker");
}

#[test]
fn refresh_cancelling_an_explicit_check_never_reads_or_claims_fresh_results() {
    let mut engine = Engine::default();
    let log = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    engine
        .register(UpdateCheckFixture {
            index: false,
            armed: false,
            id: "apt",
            refreshed: false,
            failure: false,
            read_failure: false,
            stop: true,
            log: log.clone(),
        })
        .unwrap();
    let mut replies = vec![];
    execute(
        &mut engine,
        Job::CheckUpdates(
            vec![Operation::Refresh {
                backend: "apt".into(),
            }],
            vec!["apt".into()],
        ),
        &Cancellation::default(),
        &mut |r| replies.push(r),
    );
    assert_eq!(*log.lock().unwrap(), ["refresh:apt"]);
    assert!(
        matches!(replies.pop(), Some(Reply::Done(Ok(Payload::Batch(ran)))) if ran.outcomes().eq([Outcome::Cancelled]))
    );
}
#[test]
fn refresh_cancellation_during_the_read_keeps_the_old_rows_and_marks_the_check_stopped() {
    let log = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let mut engine = Engine::default();
    engine
        .register(UpdateCheckFixture {
            id: "npm",
            index: false,
            armed: false,
            refreshed: false,
            failure: false,
            read_failure: false,
            stop: true,
            log: log.clone(),
        })
        .unwrap();
    let cancel = Cancellation::default();
    let job = Job::CheckUpdates(vec![], vec!["npm".into()]);
    let mut replies = vec![];
    execute(&mut engine, job.clone(), &cancel, &mut |r| replies.push(r));
    assert_eq!(*log.lock().unwrap(), ["read:npm"]);
    assert!(
        matches!(replies.last(), Some(Reply::Done(Ok(Payload::Batch(ran)))) if ran.steps.is_empty())
    );
    let mut controller = synthetic_controller();
    let mut ctl = controller.pin_mut();
    ctl.as_mut()
        .set_rows("[{\"kind\":\"package\",\"name\":\"known-update\",\"source\":\"npm\"}]".into());
    let before = ctl.rows().to_string();
    let mut worker = fake_worker(job, replies);
    worker.cancel = cancel;
    ctl.as_mut().rust_mut().worker = Some(worker);
    settle(&mut ctl);
    assert_eq!(ctl.rows().to_string(), before);
    let state: Value = serde_json::from_str(ctl.report_state().as_str()).unwrap();
    assert_eq!(state["phase"], "cancelled");
    assert!(state.get("checked_at").is_none());
}

#[test]
fn refresh_checked_update_reports_apply_without_an_extra_read_and_refresh_jobs_describe_their_operations(
) {
    let operations = vec![
        Operation::Refresh {
            backend: "apt".into(),
        },
        Operation::Refresh {
            backend: "flatpak".into(),
        },
    ];
    assert_eq!(
        Job::CheckUpdates(operations.clone(), vec![]).operations(),
        operations
    );
    let mut controller = synthetic_controller();
    let mut ctl = controller.pin_mut();
    ctl.as_mut().apply(Ok(Payload::CheckedUpdates(
        Box::new(PackageReport {
            packages: vec![fixture_row()],
            failures: vec![],
            successful_sources: vec!["fixture".into()],
        }),
        Batch::default(),
    )));
    assert_eq!(ctl.rust().packages.len(), 1);
    assert!(ctl.rust().worker.is_none());
    assert!(retry_job(
        &Job::CheckUpdates(vec![], vec!["npm".into()]),
        &Ok(Payload::Batch(Batch::default()))
    )
    .is_none());
}

#[test]
fn refresh_explicit_check_source_selection_is_deduplicated_skips_unavailable_and_uses_only_refresh_capabilities(
) {
    // Exhaust every subset: selection, availability and refresh capability
    // are independent, and rereading registries must not depend on refresh.
    for chosen in 1..8_u8 {
        for available in 0..8_u8 {
            for refresh in 0..8_u8 {
                let ids = ["apt", "flatpak", "npm"];
                let mut ctl = synthetic_controller();
                let mut ctl = ctl.pin_mut();
                ctl.as_mut().set_source_catalog(encoded(json!(ids.iter().enumerate().map(|(i, source)| json!({"source":source, "available": available & (1<<i) != 0, "capabilities": if refresh & (1<<i) != 0 { vec!["refresh"] } else { vec!["installed"] }})).collect::<Vec<_>>())));
                let sources: Vec<_> = ids
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| chosen & (1 << i) != 0)
                    .map(|(_, id)| *id)
                    .collect();
                ctl.as_mut().check_for_updates(
                    format!("{},{}", sources.join(","), sources.join(",")).into(),
                );
                // Sources that can't run here are skipped.
                let checked: Vec<_> = ids
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| chosen & available & (1 << i) != 0)
                    .map(|(_, id)| *id)
                    .collect();
                assert_eq!(ctl.checking_updates(), !checked.is_empty());
                let Some(worker) = ctl.rust().worker.as_ref() else {
                    continue;
                };
                let job = &worker.job;
                let (ops, selected) =
                    expect!(job, Job::CheckUpdates(ops, selected) => (ops, selected));
                assert_eq!(selected, &checked);
                let expected: Vec<_> = ids
                    .iter()
                    .enumerate()
                    // APT refreshes as the check reads it instead.
                    .filter(|(i, id)| chosen & available & refresh & (1 << i) != 0 && **id != "apt")
                    .map(|(_, id)| Operation::Refresh {
                        backend: (*id).into(),
                    })
                    .collect();
                assert_eq!(ops, &expected);
                assert_eq!(engine_source(job, &[]), selected.clone());
                ctl.as_mut().check_for_updates("apt".into());
                assert!(
                    ctl.rust().confirmed_queue.is_empty(),
                    "a second click queued work"
                );
                settle(&mut ctl);
            }
        }
    }
    let mut ctl = synthetic_controller();
    let mut ctl = ctl.pin_mut();
    for invalid in ["", ",,", "unknown", "apt,unknown"] {
        ctl.as_mut().check_for_updates(invalid.into());
        assert!(ctl.rust().worker.is_none());
    }
    ctl.as_mut().rust_mut().pending = Some(Job::Write(
        Operation::Refresh {
            backend: "apt".into(),
        },
        None,
    ));
    ctl.as_mut().check_for_updates("apt".into());
    assert!(ctl.rust().worker.is_none());
}
#[test]
fn refresh_an_explicit_check_never_replaces_a_new_pages_rows_or_source_selection() {
    for view in ["Installed", "Search", "Updates"] {
        let mut controller = synthetic_controller();
        let mut ctl = controller.pin_mut();
        ctl.as_mut().rust_mut().active_view = view.into();
        ctl.as_mut().rust_mut().source_filter = vec!["npm".into()];
        ctl.as_mut().rust_mut().deferred_load = Some(Job::Load(view.into(), "new query".into()));
        ctl.as_mut().set_rows(
            "[{\"kind\":\"package\",\"name\":\"current-page\",\"source\":\"npm\"}]".into(),
        );
        let before = ctl.rows().to_string();
        let report = PackageReport {
            packages: vec![fixture_row()],
            successful_sources: vec!["apt".into()],
            failures: vec![],
        };
        ctl.as_mut().rust_mut().worker = Some(fake_worker(
            Job::CheckUpdates(vec![], vec!["apt".into()]),
            vec![Reply::Done(Ok(Payload::CheckedUpdates(
                Box::new(report),
                Batch::default(),
            )))],
        ));
        wait_until(&mut ctl, |ctl| !ctl.checking_updates());
        assert_eq!(
            ctl.rows().to_string(),
            before,
            "late Updates report replaced {view}"
        );
        assert!(ctl
            .rust()
            .view_cache
            .get(&cache_key("Updates", "", &["npm".into()], false))
            .is_none());
        settle(&mut ctl);
    }
}

#[test]
fn refresh_a_check_never_runs_a_refresh_that_needs_root() {
    // System managers refresh through PackageKit or the user's own copy
    // of their index as the check reads them (refresh_update_index).
    for (source, refreshes) in [
        ("apt", false),
        ("dnf", false),
        ("pacman", false),
        ("zypper", false),
        ("apk", false),
        ("xbps", false),
        ("macports", false),
        ("flatpak", true),
        ("snap", true),
        ("fwupd", true),
    ] {
        let mut controller = synthetic_controller();
        let mut ctl = controller.pin_mut();
        ctl.as_mut().set_source_catalog(encoded(
            json!([{"source":source, "available":true, "capabilities":["refresh"]}]),
        ));
        ctl.as_mut().check_for_updates(source.into());
        let job = &ctl.rust().worker.as_ref().unwrap().job;
        let ops = job.operations();
        if refreshes {
            assert_eq!(
                ops,
                &[Operation::Refresh {
                    backend: source.into()
                }],
                "{source} check should refresh first"
            );
        } else {
            assert!(ops.is_empty(), "{source} check refreshed");
        }
        assert!(
            ctl.rust().confirmed_queue.is_empty(),
            "{source} check queued work"
        );
        settle(&mut ctl);
    }
}

#[test]
fn refresh_checks_preempt_background_reads_and_are_identified_while_queued() {
    let mut controller = synthetic_controller();
    let mut ctl = controller.pin_mut();
    ctl.as_mut().set_source_catalog(encoded(
        json!([{"source":"flatpak", "available":true, "capabilities":["refresh"]}]),
    ));
    ctl.as_mut().rust_mut().worker =
        Some(fake_worker(Job::Load("Updates".into(), "".into()), vec![]));
    ctl.as_mut().rust_mut().background = true;
    ctl.as_mut()
        .check_for_updates(" flatpak , flatpak , ".into());
    assert!(ctl.checking_updates());
    assert_eq!(ctl.rust().confirmed_queue.len(), 1);
    assert!(ctl.rust().worker.as_ref().unwrap().cancel.requested());
    settle(&mut ctl);
    assert!(!ctl.checking_updates());
    assert!(ctl.rust().confirmed_queue.is_empty());
}

#[test]
fn refresh_a_finished_explicit_check_publishes_and_caches_its_final_report() {
    let mut controller = synthetic_controller();
    let mut ctl = controller.pin_mut();
    ctl.as_mut().rust_mut().active_view = "Updates".into();
    ctl.as_mut().rust_mut().updates_view = true;
    ctl.as_mut().rust_mut().source_filter = vec!["npm".into()];
    let mut package = fixture_row();
    package.id.backend = "npm".into();
    let report = PackageReport {
        packages: vec![package],
        successful_sources: vec!["npm".into()],
        failures: vec![],
    };
    ctl.as_mut().rust_mut().worker = Some(fake_worker(
        Job::CheckUpdates(vec![], vec!["npm".into()]),
        vec![Reply::Done(Ok(Payload::CheckedUpdates(
            Box::new(report),
            Batch::default(),
        )))],
    ));
    ctl.as_mut().set_busy(true);
    ctl.as_mut().set_writing(true);
    settle(&mut ctl);
    assert!(!ctl.busy());
    assert!(!ctl.writing());
    assert!(!ctl.checking_updates());
    assert_eq!(ctl.rust().packages.len(), 1);
    assert!(ctl.rust().queued.is_none());
    assert!(ctl.notice().is_empty() || ctl.notice().as_str() == "{}");
    let state: Value = serde_json::from_str(ctl.report_state().as_str()).unwrap();
    assert_eq!(state["phase"], "complete");
    assert!(state["checked_at"].as_u64().unwrap() > 0);
    assert!(ctl
        .rust()
        .view_cache
        .get(&cache_key("Updates", "", &["npm".into()], false))
        .is_some());
}

#[test]
fn a_refresh_batch_counts_source_refreshes() {
    let refresh = |backend: &str| Operation::Refresh {
        backend: backend.into(),
    };
    // Its status line counts refreshes.
    let ran = std::sync::Arc::default();
    let mut engine = Engine::default();
    engine
        .register(Cleaner {
            backend: "fixture",
            items: vec![],
            ran: std::sync::Arc::clone(&ran),
        })
        .unwrap();
    let finished = batch(
        &mut engine,
        &[refresh("fixture")],
        vec![],
        &Cancellation::default(),
        &mut |_| {},
    );
    let status = finished.text(false);
    // The fixture only cleans, so its refresh fails.
    assert!(
        status.starts_with("Completed 0 of 1 source refreshes."),
        "{status}"
    );
    assert!(finished.outcomes().eq([Outcome::Failed]));
    assert!(ran.lock().unwrap().is_empty());
}

#[test]
fn cleanup_shows_what_it_frees_and_clean_all_waits_for_data_to_be_checked() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let volumes = sized(
        "docker",
        "unused-volumes",
        CleanupKind::Data,
        Some(9_000_000_000),
    );
    let unknown = sized("apt", "clean", CleanupKind::PackageCache, None);
    let build = sized(
        "projects",
        "build:/home/me/app",
        CleanupKind::BuildFiles,
        Some(500_000_000),
    );
    let cargo = sized(
        "dev-caches",
        "cargo",
        CleanupKind::PackageCache,
        Some(2_000_000_000),
    );
    controller
        .as_mut()
        .apply(Ok(Payload::Cleanup(CleanupReport {
            items: vec![
                unknown.clone(),
                build.clone(),
                volumes.clone(),
                cargo.clone(),
            ],
            failures: vec![],
        })));
    // The biggest first, and tasks that can't tell at the end.
    assert_eq!(
        controller.rust().cleanup,
        [
            volumes.clone(),
            cargo.clone(),
            build.clone(),
            unknown.clone()
        ]
    );
    let rows: Value = serde_json::from_str(&controller.rows().to_string()).unwrap();
    assert_eq!(rows[1]["bytes"], 2_000_000_000u64);
    assert_eq!(rows[3]["bytes"], Value::Null);
    assert_eq!(
        controller.status().to_string(),
        "About 12 GB can be freed. Review a cleanup task before running it."
    );
    // One task tells its size.
    controller.as_mut().propose("clean".into(), 1);
    let confirmation = controller.confirmation().to_string();
    assert!(
        confirmation.contains("Frees about 2.0 GB."),
        "{confirmation}"
    );
    controller.as_mut().confirm(false);
    controller.as_mut().propose("clean".into(), 3);
    assert!(!controller.confirmation().to_string().contains("Frees"));
    controller.as_mut().confirm(false);
    // Clean all: every checked task, with what each frees. The volumes
    // wait to be checked.
    let ids: Vec<_> = controller
        .rust()
        .cleanup
        .iter()
        .map(|item| item.id.clone())
        .collect();
    controller
        .as_mut()
        .propose_clean_checked(encoded(json!(ids)));
    let pending = controller.rust().pending.as_ref();
    assert!(matches!(pending, Some(Job::CleanAll(_))));
    assert_eq!(
        pending.map(Job::operations).unwrap_or_default(),
        [&cargo, &build, &unknown].map(|item| Operation::Clean(item.id.clone()))
    );
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert_eq!(
        data["summary"],
        "Clean 3 tasks\nFrees about 2.5 GB.\nFinished tasks cannot be undone."
    );
    let body = data["body"].as_str().unwrap();
    assert!(body.starts_with("cargo task (Developer caches), about 2.0 GB\nEmpties cargo"));
    assert!(body.contains("clean task (APT)\nEmpties clean"));
    assert!(!body.contains("unused-volumes"));
    controller.as_mut().confirm(false);
    // Once checked, they go with the rest.
    propose_every_cleanup(&mut controller);
    assert_eq!(
        controller
            .rust()
            .pending
            .as_ref()
            .map(Job::operations)
            .unwrap_or_default(),
        [&volumes, &cargo, &build, &unknown].map(|item| Operation::Clean(item.id.clone()))
    );
    controller.as_mut().confirm(false);
    // Unchecked again: nothing for Clean all to do.
    controller
        .as_mut()
        .apply(Ok(Payload::Cleanup(CleanupReport {
            items: vec![volumes.clone()],
            failures: vec![],
        })));
    let preferences = &mut controller.as_mut().rust_mut().cleanup_preferences;
    preferences.set_included(volumes.id.clone(), false);
    controller
        .as_mut()
        .propose_clean_checked(encoded(json!([volumes.id])));
    assert!(controller.rust().pending.is_none());
    // Sizes that can't be told keep the plain status.
    controller
        .as_mut()
        .apply(Ok(Payload::Cleanup(CleanupReport {
            items: vec![unknown],
            failures: vec![],
        })));
    assert_eq!(
        controller.status().to_string(),
        "Review a cleanup task before running it."
    );
    // A plan that changed while queued is confirmed again, sizes included.
    let entry = Confirmed {
        job: Job::CleanAll(vec![Operation::Clean(cargo.id.clone())]),
        activity_id: None,
        cleanup_preview: vec![cargo.clone()],
    };
    controller.as_mut().rust_mut().revalidating = Some(entry);
    let mut changed = cargo.clone();
    changed.preview = "Empties more".into();
    controller.as_mut().apply(Ok(Payload::CleanPreview(
        vec![Operation::Clean(cargo.id.clone())],
        vec![changed],
    )));
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert_eq!(
        data["body"],
        "cargo task (Developer caches), about 2.0 GB\nEmpties more"
    );
    assert_eq!(controller.confirmation().to_string(), data["body"]);
}

/// Cleanup tasks that run as they're told, and say which ran.
struct Cleaner {
    backend: &'static str,
    items: Vec<CleanupItem>,
    ran: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}
impl Backend for Cleaner {
    fn id(&self) -> &str {
        self.backend
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Clean]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn cleanup(&mut self, _: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        Ok(self.items.clone())
    }
    fn execute(
        &mut self,
        operation: &Operation,
        _: &Cancellation,
        _: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        // "Clean KEY (Source)": only cleanups reach this source.
        let label = operation_label(operation);
        let key = label
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_owned();
        self.ran.lock().unwrap().push(key.clone());
        if key == "broken" {
            return Err(EngineError::Execution(ExecutionError::Io(
                "disk full".into(),
            )));
        }
        Ok(OperationOutcome::default())
    }
}
#[test]
fn automatic_cleaning_runs_every_checked_task_that_needs_no_password() {
    let ran = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let mut engine = Engine::default();
    engine
        .register(Cleaner {
            backend: "fixture",
            items: vec![
                sized("fixture", "cache", CleanupKind::PackageCache, Some(3_000)),
                sized(
                    "fixture",
                    "stopped-containers",
                    CleanupKind::Unused,
                    Some(1),
                ),
                sized("fixture", "worktrees", CleanupKind::Worktree, Some(1)),
                sized("fixture", "broken", CleanupKind::BuildFiles, Some(500)),
                sized("fixture", "images", CleanupKind::Unused, None),
            ],
            ran: ran.clone(),
        })
        .unwrap();
    let replies = run_job(
        &mut engine,
        Job::AutoClean {
            caches: true,
            worktrees: false,
            preferences: CleanupPreferences::default(),
        },
        &Cancellation::default(),
    );
    assert!(matches!(
        replies.last(),
        Some(Reply::Done(Ok(Payload::AutoCleaned {
            cleaned: 2,
            failed: 1,
            bytes: 3_000,
            ..
        })))
    ));
    let mut cleaned = ran.lock().unwrap().clone();
    cleaned.sort();
    // Stopped containers wait to be checked.
    assert_eq!(cleaned, ["broken", "cache", "images"]);
    // Once checked, they go too.
    let mut preferences = CleanupPreferences::default();
    preferences.set_included(
        CleanupId {
            backend: "fixture".into(),
            key: "stopped-containers".into(),
        },
        true,
    );
    ran.lock().unwrap().clear();
    let auto = Job::AutoClean {
        caches: true,
        worktrees: false,
        preferences,
    };
    run_job(&mut engine, auto, &Cancellation::default());
    let mut cleaned = ran.lock().unwrap().clone();
    cleaned.sort();
    assert_eq!(cleaned, ["broken", "cache", "images", "stopped-containers"]);
    // Nothing to clean: nothing runs.
    let mut idle = Engine::default();
    idle.register(Cleaner {
        backend: "fixture",
        items: vec![sized(
            "fixture",
            "worktrees",
            CleanupKind::Worktree,
            Some(1),
        )],
        ran: ran.clone(),
    })
    .unwrap();
    assert!(matches!(
        run_job(
            &mut idle,
            Job::AutoClean {
                caches: true,
                worktrees: false,
                preferences: CleanupPreferences::default()
            },
            &Cancellation::default()
        )
        .last(),
        Some(Reply::Done(Ok(Payload::AutoCleaned {
            cleaned: 0,
            failed: 0,
            bytes: 0,
            ..
        })))
    ));
    assert_eq!(ran.lock().unwrap().len(), 4);
    assert!(Job::AutoClean {
        caches: true,
        worktrees: false,
        preferences: CleanupPreferences::default()
    }
    .writes());
    assert!(Job::AutoClean {
        caches: true,
        worktrees: false,
        preferences: CleanupPreferences::default()
    }
    .operations()
    .is_empty());
}
#[test]
fn automatic_worktree_cleanup_is_independent_and_only_runs_project_tasks() {
    for caches in [false, true] {
        for worktrees in [false, true] {
            let ran = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
            let mut engine = Engine::default();
            for backend in ["projects", "fixture"] {
                engine
                    .register(Cleaner {
                        backend,
                        items: vec![
                            sized(backend, "cache", CleanupKind::BuildFiles, Some(10)),
                            sized(backend, "worktrees", CleanupKind::Worktree, Some(20)),
                            sized(backend, "volume", CleanupKind::Data, Some(30)),
                        ],
                        ran: ran.clone(),
                    })
                    .unwrap();
            }
            let replies = run_job(
                &mut engine,
                Job::AutoClean {
                    caches,
                    worktrees,
                    preferences: CleanupPreferences::default(),
                },
                &Cancellation::default(),
            );
            let cleaned = usize::from(caches) * 2 + usize::from(worktrees);
            assert!(
                matches!(replies.last(), Some(Reply::Done(Ok(Payload::AutoCleaned { cleaned: actual, failed: 0, .. }))) if *actual == cleaned)
            );
            let keys = ran.lock().unwrap();
            assert_eq!(
                keys.iter().filter(|key| *key == "worktrees").count(),
                usize::from(worktrees)
            );
            assert!(!keys.contains(&"volume".into()));
        }
    }
}

#[test]
fn checked_cleanup_uses_current_exact_keys_deduplicates_and_skips_unchecked_data() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let items = vec![
        sized("projects", "one", CleanupKind::Worktree, Some(10)),
        sized("projects", "two", CleanupKind::Worktree, Some(20)),
        sized("projects", "volume", CleanupKind::Data, Some(30)),
    ];
    controller
        .as_mut()
        .apply(Ok(Payload::Cleanup(CleanupReport {
            items: items.clone(),
            failures: vec![],
        })));
    let wanted = vec![
        items[1].id.clone(),
        items[1].id.clone(),
        items[2].id.clone(),
        CleanupId {
            backend: "projects".into(),
            key: "gone".into(),
        },
    ];
    controller
        .as_mut()
        .propose_clean_checked(serde_json::to_string(&wanted).unwrap().into());
    assert!(
        matches!(&controller.rust().pending, Some(Job::CleanAll(ops)) if ops == &vec![Operation::Clean(items[1].id.clone())])
    );
    assert!(controller
        .confirmation()
        .to_string()
        .contains("Run 1 cleanup task?"));
    for input in ["[]", "not json", r#"[{"backend":"other","key":"two"}]"#] {
        controller.as_mut().propose_clean_checked(input.into());
        assert!(controller.rust().pending.is_none());
        assert!(controller.confirmation().to_string().is_empty());
    }
}

#[test]
fn excluded_tasks_never_run_automatically_or_in_batches() {
    let ran = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let mut engine = Engine::default();
    let items = vec![
        sized("dev-caches", "sccache", CleanupKind::PackageCache, Some(10)),
        sized("dev-caches", "cargo", CleanupKind::PackageCache, Some(20)),
    ];
    engine
        .register(Cleaner {
            backend: "dev-caches",
            items: items.clone(),
            ran: ran.clone(),
        })
        .unwrap();
    let preferences = CleanupPreferences::default();
    run_job(
        &mut engine,
        Job::AutoClean {
            caches: true,
            worktrees: false,
            preferences: preferences.clone(),
        },
        &Cancellation::default(),
    );
    assert_eq!(*ran.lock().unwrap(), vec!["cargo"]);
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller
        .as_mut()
        .apply(Ok(Payload::Cleanup(CleanupReport {
            items: items.clone(),
            failures: vec![],
        })));
    let ids = serde_json::to_string(&items.iter().map(|item| item.id.clone()).collect::<Vec<_>>())
        .unwrap();
    controller
        .as_mut()
        .propose_clean_checked(ids.as_str().into());
    assert!(
        matches!(&controller.rust().pending, Some(Job::CleanAll(ops)) if ops == &vec![Operation::Clean(items[1].id.clone())])
    );
    let mut preferences = preferences;
    preferences.set_included(items[1].id.clone(), false);
    controller
        .as_mut()
        .set_cleanup_preferences(serde_json::to_string(&preferences).unwrap().into());
    controller
        .as_mut()
        .set_cleanup_preferences("bad json".into());
    controller
        .as_mut()
        .propose_clean_checked(ids.as_str().into());
    assert!(controller.rust().pending.is_none());
    ran.lock().unwrap().clear();
    run_job(
        &mut engine,
        Job::AutoClean {
            caches: true,
            worktrees: false,
            preferences,
        },
        &Cancellation::default(),
    );
    assert!(ran.lock().unwrap().is_empty());
    assert!(engine_source(
        &Job::AutoClean {
            caches: true,
            worktrees: false,
            preferences: CleanupPreferences::default()
        },
        &["apt".into()]
    )
    .is_empty());
}

#[test]
fn worktree_only_cleaning_respects_interval_power_and_pending_work() {
    for case in 0..5 {
        let mut controller = ffi::create_controller();
        let mut controller = controller.pin_mut();
        assert!(!controller.auto_clean_worktrees());
        controller.as_mut().set_auto_clean_worktrees(true);
        assert!(controller.auto_clean_worktrees());
        controller.as_mut().rust_mut().natives.hold = never_hold;
        let now = epoch_seconds();
        controller
            .as_mut()
            .set_auto_clean(false, now - 25 * 60 * 60);
        match case {
            0 => {}
            1 => controller
                .as_mut()
                .set_auto_clean(false, now - 23 * 60 * 60),
            2 => controller.as_mut().rust_mut().natives.hold = on_battery,
            3 => controller.as_mut().rust_mut().pending = Some(Job::CleanAll(vec![])),
            _ => controller.as_mut().set_auto_clean_worktrees(false),
        }
        controller.as_mut().schedule_auto_clean();
        let scheduled = controller
            .rust()
            .validated_confirmed
            .as_ref()
            .is_some_and(|entry| {
                matches!(
                    entry.job,
                    Job::AutoClean {
                        caches: false,
                        worktrees: true,
                        ..
                    }
                )
            });
        assert_eq!(scheduled, case == 0, "case {case}");
    }
}

#[test]
fn arbitrary_cleanup_selection_text_never_creates_an_operation() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    let item = sized("projects", "fixture-key", CleanupKind::Worktree, Some(1));
    controller
        .as_mut()
        .apply(Ok(Payload::Cleanup(CleanupReport {
            items: vec![item],
            failures: vec![],
        })));
    let mut seed = 12345u64;
    for length in 0..128 {
        let text: String = (0..length)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                char::from(32 + (seed % 95) as u8)
            })
            .collect();
        controller.as_mut().propose_clean_checked(text.into());
        assert!(controller.rust().pending.is_none());
    }
}

#[test]
fn automatic_updates_read_back_as_set() {
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    assert!(!controller.auto_update());
    controller.as_mut().set_auto_update(true);
    assert!(controller.auto_update());
    controller.as_mut().set_auto_update(false);
    assert!(!controller.auto_update());
}
#[test]
fn automatic_cleaning_waits_a_day_for_power_builds_and_other_work() {
    let state = |controller: &ffi::PackageController| -> Value {
        serde_json::from_str(&controller.background_state().to_string()).unwrap()
    };
    let empty = PackageReport {
        packages: vec![],
        failures: vec![],
        successful_sources: vec![],
    };
    let scheduled = |controller: &Pin<&mut ffi::PackageController>| {
        matches!(
            controller
                .rust()
                .validated_confirmed
                .as_ref()
                .map(|entry| &entry.job),
            Some(Job::AutoClean { .. })
        )
    };
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().natives.hold = never_hold;
    // Off by default.
    controller.as_mut().finish_background_check(empty.clone());
    assert!(!scheduled(&controller));
    let now = epoch_seconds();
    // Cleaned less than a day ago.
    controller.as_mut().set_auto_clean(true, now - 23 * 60 * 60);
    controller.as_mut().finish_background_check(empty.clone());
    assert!(!scheduled(&controller));
    // On battery or during a build.
    controller.as_mut().set_auto_clean(true, now - 25 * 60 * 60);
    controller.as_mut().rust_mut().natives.hold = on_battery;
    controller.as_mut().finish_background_check(empty.clone());
    assert!(!scheduled(&controller));
    controller.as_mut().rust_mut().natives.hold = never_hold;
    // Something else waits to run.
    for busy in 0..3 {
        match busy {
            0 => controller.as_mut().rust_mut().pending = Some(Job::CleanAll(vec![])),
            1 => {
                controller.as_mut().rust_mut().queued =
                    Some(Job::Load("Installed".into(), String::new()))
            }
            _ => controller
                .as_mut()
                .rust_mut()
                .confirmed_queue
                .push_back(Confirmed {
                    job: Job::CleanAll(vec![]),
                    activity_id: None,
                    cleanup_preview: vec![],
                }),
        }
        controller.as_mut().finish_background_check(empty.clone());
        assert!(!scheduled(&controller), "{busy}");
        controller.as_mut().rust_mut().pending = None;
        controller.as_mut().rust_mut().queued = None;
        controller.as_mut().rust_mut().confirmed_queue.clear();
    }
    controller.as_mut().finish_background_check(empty.clone());
    assert!(scheduled(&controller));
    // A second check while it waits changes nothing.
    controller.as_mut().finish_background_check(empty);
    assert!(scheduled(&controller));
    controller.as_mut().rust_mut().validated_confirmed = None;

    // What it did is kept for the window to tell and remember.
    controller.as_mut().apply(Ok(Payload::AutoCleaned {
        cleaned: 2,
        failed: 1,
        bytes: 3_000_000,
        tasks: vec![],
        started_at: 0,
    }));
    let after = controller.rust().auto_cleaned_at;
    assert!(after >= now);
    assert_eq!(state(&controller)["cleaned_at"], after);
    assert_eq!(
        state(&controller)["cleaned"],
        json!({"tasks": 2, "failed": 1, "bytes": 3_000_000})
    );
    assert_eq!(
        serde_json::from_str::<Value>(&controller.report_state().to_string()).unwrap()["phase"],
        "stale"
    );
    // Nothing cleaned leaves the lists alone.
    let mut quiet = ffi::create_controller();
    let mut quiet = quiet.pin_mut();
    quiet.as_mut().set_background_state("not json".into());
    quiet.as_mut().apply(Ok(Payload::AutoCleaned {
        cleaned: 0,
        failed: 0,
        bytes: 0,
        tasks: vec![],
        started_at: 0,
    }));
    assert_ne!(
        serde_json::from_str::<Value>(&quiet.report_state().to_string()).unwrap()["phase"],
        "stale"
    );
    assert_eq!(state(&quiet)["cleaned"]["tasks"], 0);
}
#[test]
fn automatic_cleaning_runs_between_checks_after_updates_and_holds() {
    let scheduled = |controller: &Pin<&mut ffi::PackageController>| {
        matches!(
            controller
                .rust()
                .validated_confirmed
                .as_ref()
                .map(|entry| &entry.job),
            Some(Job::AutoClean { .. })
        )
    };
    let mut controller = ffi::create_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().natives.hold = building;
    let now = epoch_seconds();
    controller.as_mut().set_auto_clean(true, now - 25 * 60 * 60);
    // The last check was just now, so none is due.
    controller.as_mut().resume_checks(now);
    let tick = |controller: &mut Pin<&mut ffi::PackageController>, enabled: bool| {
        controller
            .as_mut()
            .check_updates("apt".into(), enabled, false, false, false);
    };
    // A build doesn't hold cleaning back: each task that build uses waits
    // for it on its own.
    tick(&mut controller, true);
    assert!(scheduled(&controller));
    controller.as_mut().rust_mut().validated_confirmed = None;
    controller.as_mut().set_auto_clean(true, now - 25 * 60 * 60);
    // An automatic update is about to run.
    controller.as_mut().rust_mut().natives.hold = never_hold;
    controller.as_mut().rust_mut().validated_confirmed = Some(Confirmed {
        job: Job::AutoUpgrade(vec![], false),
        activity_id: None,
        cleanup_preview: vec![],
    });
    tick(&mut controller, true);
    assert!(matches!(
        controller
            .rust()
            .validated_confirmed
            .as_ref()
            .map(|c| &c.job),
        Some(Job::AutoUpgrade(..))
    ));
    // Without background checks, nothing runs on its own.
    controller.as_mut().rust_mut().validated_confirmed = None;
    tick(&mut controller, false);
    assert!(!scheduled(&controller));
    // The update is done: the next tick cleans, without waiting for a check.
    tick(&mut controller, true);
    assert!(scheduled(&controller));
    assert!(controller.rust().worker.is_none());
}
fn cleanup_item(key: &str) -> CleanupItem {
    CleanupItem {
        id: CleanupId {
            backend: "fixture".into(),
            key: key.into(),
        },
        kind: CleanupKind::OrphanDependencies,
        title: format!("Remove {key}"),
        summary: "Synthetic cleanup".into(),
        preview: format!("would remove {key}"),
        bytes: None,
    }
}
#[test]
fn open_reference_scope_changes_only_a_pending_flatpak_reference_install() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().set_open_flatpak_scope(false);
    assert!(controller.rust().pending.is_none());
    let mut native = fixture_row();
    native.id.backend = "apt".into();
    controller.as_mut().rust_mut().pending =
        Some(Job::Write(Operation::Install(native.id.clone()), None));
    controller.as_mut().set_open_flatpak_scope(false);
    assert!(matches!(
        &controller.rust().pending,
        Some(Job::Write(Operation::Install(id), None)) if *id == native.id
    ));
    let mut reference = native.clone();
    reference.id.backend = "flatpak".into();
    reference.id.reference = Some("flatpakref:https://example.invalid/app.flatpakref".into());
    controller.as_mut().rust_mut().pending =
        Some(Job::Write(Operation::Install(reference.id.clone()), None));
    // The same scope, or a row that is no longer listed, keeps the plan.
    controller.as_mut().set_open_flatpak_scope(true);
    controller.as_mut().set_open_flatpak_scope(false);
    assert!(matches!(
        &controller.rust().pending,
        Some(Job::Write(Operation::Install(id), None)) if id.scope == Scope::System
    ));
}
#[test]
fn activity_names_a_cleanup_task_after_it_is_gone() {
    let temp = pkgdeck_tools::Temp::new();
    let store = ActivityStore::at(temp.0.join("activity.json"));
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().activity_store = Some(store.clone());
    let (item, package) = (cleanup_item("orphans"), fixture_row());
    controller.as_mut().rust_mut().cleanup = vec![item.clone()];
    controller
        .as_mut()
        .rust_mut()
        .names
        .insert(package.id.clone(), "Fixture App".into());
    // Behind a running write, so it only queues.
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Write(Operation::Upgrade(package.id.clone()), None),
        vec![],
    ));
    let operations = vec![
        Operation::Clean(item.id.clone()),
        Operation::Install(package.id.clone()),
        Operation::Refresh {
            backend: "fixture".into(),
        },
    ];
    controller
        .as_mut()
        .accept_confirmed(Job::CleanAll(operations.clone()));
    let entry = store.entries().unwrap().pop().unwrap();
    assert_eq!(
        entry.names,
        [
            Some("Remove orphans".to_owned()),
            Some("Fixture App".to_owned()),
            None
        ]
    );
    // Cleaned, the task is gone, and a fresh start knows no names: Activity
    // still calls it what it was called.
    let labels = |entries: &[pkgdeck_core::activity::Entry], names: &Names| {
        let rows: Value = serde_json::from_str(&activity_rows(entries, names).to_string()).unwrap();
        rows[0]["labels"].clone()
    };
    assert_eq!(
        labels(&[entry.clone()], &Names::new()),
        json!([
            "Clean Remove orphans",
            "Install Fixture App (fixture)",
            "Refresh fixture package lists"
        ])
    );
    // A name known now goes first.
    let known = Names {
        cleanup: BTreeMap::from([(item.id.clone(), "Orphans".to_owned())]),
        ..Names::new()
    };
    assert_eq!(labels(&[entry.clone()], &known)[0], "Clean Orphans");
    // An entry from before names were kept goes by its key.
    let mut old = entry;
    old.names.clear();
    assert_eq!(labels(&[old], &Names::new())[0], "Clean orphans (fixture)");
}
#[test]
fn confirmed_changes_queue_behind_running_work_and_can_be_cancelled() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let items = vec![cleanup_item("orphans"), cleanup_item("cache")];
    controller.as_mut().rust_mut().cleanup = items.clone();
    let upgrade = Operation::Upgrade(fixture_row().id);
    // A running write is never cancelled for a queued change.
    controller.as_mut().rust_mut().worker =
        Some(fake_worker(Job::Write(upgrade.clone(), None), vec![]));
    controller
        .as_mut()
        .accept_confirmed(Job::CleanAll(vec![Operation::Clean(items[1].id.clone())]));
    assert!(!controller
        .rust()
        .worker
        .as_ref()
        .unwrap()
        .cancel
        .requested());
    assert_eq!(
        controller.rust().confirmed_queue[0].cleanup_preview,
        [items[1].clone()]
    );
    assert_eq!(controller.status().to_string(), "Operation queued.");
    controller.as_mut().rust_mut().worker = None;
    // Changes wait behind earlier queued ones even with nothing running.
    controller
        .as_mut()
        .accept_confirmed(Job::CleanAll(vec![Operation::Clean(items[0].id.clone())]));
    assert_eq!(controller.rust().confirmed_queue.len(), 2);
    assert!(controller.rust().worker.is_none());
    controller.as_mut().rust_mut().validated_confirmed = Some(Confirmed {
        job: Job::Write(upgrade.clone(), None),
        activity_id: None,
        cleanup_preview: vec![],
    });
    controller.as_mut().cancel_queued();
    assert!(controller.rust().confirmed_queue.is_empty());
    assert!(controller.rust().validated_confirmed.is_none());

    // A running write keeps a new request waiting, not queued.
    controller.as_mut().rust_mut().worker = Some(fake_worker(Job::Write(upgrade, None), vec![]));
    controller
        .as_mut()
        .start(Job::Load("Installed".into(), String::new()));
    assert!(controller.rust().queued.is_none());
    controller.as_mut().rust_mut().worker = None;

    // Source changes run without a separate review. The bare host has
    // no editor, so nothing opens.
    let action = RepositoryAction {
        backend: "apt".into(),
        name: "sources".into(),
        scope: Scope::System,
        change: repositories::Change::OpenEditor,
    };
    controller.as_mut().validate_confirmed(Confirmed {
        job: Job::Repositories(Some(action)),
        activity_id: None,
        cleanup_preview: vec![],
    });
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(worker.job, Job::Repositories(Some(_)))
    ));
    settle(&mut controller);
    assert!(controller.status().to_string().contains("unavailable"));
}
#[test]
fn reads_report_unsupported_sources_and_ignore_invalid_requests() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().set_package_report_state(
        &PackageReport {
            packages: vec![],
            failures: vec![BackendFailure {
                backend: "fixture".into(),
                error: EngineError::Unsupported {
                    backend: "fixture".into(),
                    capability: Capability::Installed,
                },
            }],
            successful_sources: vec![],
        },
        false,
    );
    let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
    assert_eq!(state["phase"], "unsupported");
    controller.as_mut().set_package_report_state(
        &PackageReport {
            packages: vec![],
            failures: vec![BackendFailure {
                backend: "fixture".into(),
                error: EngineError::Cancelled,
            }],
            successful_sources: vec![],
        },
        false,
    );
    let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
    assert_eq!(state["phase"], "failed");

    let status = controller.status().to_string();
    controller.as_mut().load(
        "Bogus".into(),
        String::new().as_str().into(),
        String::new().as_str().into(),
        false,
        false,
    );
    assert!(controller.rust().worker.is_none());
    assert_eq!(controller.status().to_string(), status);

    // Nothing starts beside a foreground read.
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Load("Installed".into(), String::new()),
        vec![],
    ));
    controller
        .as_mut()
        .retry_source("Installed".into(), "".into(), "flatpak".into());
    let path = std::env::temp_dir().join("pkgdeck-never-written.json");
    let url = QUrl::from_local_file(&path.to_string_lossy().as_ref().into());
    controller
        .as_mut()
        .export_inventory(url.clone(), "[]".into());
    controller.as_mut().preview_inventory(url);
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(worker.job, Job::Load(..))
    ));
    assert!(controller.rust().queued.is_none());
    controller.as_mut().rust_mut().worker = None;

    controller
        .as_mut()
        .retry_source("Search".into(), "synthetic".into(), "flatpak".into());
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::RetrySource(view, query, source)
            if view == "Search" && query == "synthetic" && source == "flatpak")
    ));
    settle(&mut controller);
    assert!(controller.rows().to_string().contains("synthetic"));
}
#[test]
fn reviewed_plans_travel_to_the_write_engine() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let row = fixture_row();
    controller.as_mut().rust_mut().packages = vec![row.clone()];
    controller.as_mut().rust_mut().cleanup = vec![cleanup_item("orphans")];
    let upgrade_all = Operation::UpgradeAll {
        backend: "fixture".into(),
    };
    let apt_plan = AptUpgradePlan {
        preview: "Inst synthetic".into(),
        upgrades: vec!["synthetic".into()],
        installs: vec![],
        removals: vec![],
    };
    controller
        .as_mut()
        .start(Job::UpgradeAll(vec![upgrade_all], Some(apt_plan)));
    settle(&mut controller);
    let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
    assert_eq!(notice["kind"], "success");
    let upgrade = Operation::Upgrade(row.id.clone());
    let plan = TransactionPlan {
        operation: upgrade.clone(),
        native_preview: "Upgrade synthetic".into(),
        changes: vec![],
        download_bytes: None,
        disk_bytes: None,
        restart_required: None,
        adopts: None,
    };
    controller
        .as_mut()
        .start(Job::Write(upgrade, Some(Box::new(plan))));
    settle(&mut controller);
    let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
    // The fixture now plans nothing, so the reviewed plan is stale: it
    // asks for a new review instead of failing.
    assert_eq!(notice["kind"], "info");
    assert!(notice["title"]
        .as_str()
        .unwrap()
        .contains("The planned changes are different now."));
}
#[test]
fn automatic_runs_use_their_batch_mode_and_report_what_they_did() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let operations = vec![Operation::UpgradeAll {
        backend: "fixture".into(),
    }];
    controller
        .as_mut()
        .start(Job::AutoUpgrade(operations.clone(), false));
    settle(&mut controller);
    let result: Value = serde_json::from_str(&controller.auto_update_result().to_string()).unwrap();
    assert_eq!(result, json!({"updated": 1, "failed": 0, "total": 1}));
    // A run that never reached its batch leaves the last result alone.
    controller.as_mut().set_auto_update_result("{}".into());
    controller.as_mut().rust_mut().natives = Natives {
        engine: missing_engine,
        ..SYNTHETIC
    };
    controller
        .as_mut()
        .start(Job::AutoUpgrade(operations, false));
    settle(&mut controller);
    assert_eq!(controller.auto_update_result().to_string(), "{}");
}
#[test]
fn a_finished_refresh_counts_as_a_successful_check() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let refresh = Operation::Refresh {
        backend: "fixture".into(),
    };
    controller.as_mut().start(Job::Write(refresh.clone(), None));
    settle(&mut controller);
    let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
    assert!(
        state["last_success"]["fixture"].as_u64().unwrap() > 0,
        "{state}"
    );
    // A batch records only the refreshes that finished.
    controller.as_mut().rust_mut().last_success.clear();
    controller
        .as_mut()
        .start(Job::UpgradeAll(vec![refresh], None));
    settle(&mut controller);
    assert!(controller.rust().last_success.contains_key("fixture"));
}
#[test]
fn a_change_that_replaces_pkgdeck_asks_for_a_restart() {
    let dir = std::env::temp_dir().join(format!("pkgdeck-self-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let program = dir.join("pkgdeck");
    let operations = vec![Operation::UpgradeAll {
        backend: "fixture".into(),
    }];
    for (job, expected) in [
        (Job::UpgradeAll(operations.clone(), None), "manual"),
        (Job::AutoUpgrade(operations.clone(), false), "automatic"),
    ] {
        std::fs::write(&program, "old").unwrap();
        let mut controller = synthetic_controller();
        let mut controller = controller.pin_mut();
        let install = pkgdeck_core::relaunch::Install::program(&program);
        controller.as_mut().rust_mut().install = Some(install.clone());
        // Nothing replaced yet.
        controller.as_mut().start(job.clone());
        settle(&mut controller);
        assert_eq!(controller.self_update().to_string(), "");
        std::fs::write(dir.join("new"), "new").unwrap();
        std::fs::rename(dir.join("new"), &program).unwrap();
        controller.as_mut().start(job);
        settle(&mut controller);
        assert_eq!(controller.self_update().to_string(), expected);
        // The updated copy starts once this test process exits.
        assert!(controller.as_mut().restart_app(true));
    }
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    assert!(!controller.as_mut().restart_app(false));
    // A Flatpak restarts through flatpak-spawn, missing outside one.
    controller.as_mut().rust_mut().install = pkgdeck_core::relaunch::Install::detect(
            &Default::default(),
            Some("[Instance]\napp-path=/var/lib/flatpak/app/io.github.astrovm.PkgDeck/x86_64/stable/abc/files\n"),
            std::path::Path::new("/app/bin/pkgdeck"),
        );
    assert!(!controller.as_mut().restart_app(false));
    assert!(controller
        .status()
        .to_string()
        .starts_with("PkgDeck couldn't restart:"));
}
#[test]
fn system_update_approval_is_saved_removed_and_explained() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().natives = Natives {
        approve: synthetic_approval,
        ..SYNTHETIC
    };
    let approval_idle =
        |controller: &ffi::PackageController| controller.rust().approval_worker.is_none();
    controller.as_mut().allow_system_updates(true);
    assert!(controller.needs_poll());
    wait_until(&mut controller, approval_idle);
    assert_eq!(controller.system_approval().to_string(), "synthetic-helper");
    assert_eq!(controller.approval_error().to_string(), "");
    controller.as_mut().allow_system_updates(false);
    wait_until(&mut controller, approval_idle);
    assert_eq!(controller.system_approval().to_string(), "");

    // Without the helper, the reason is shown and nothing is saved.
    controller.as_mut().rust_mut().natives = SYNTHETIC;
    controller.as_mut().allow_system_updates(true);
    wait_until(&mut controller, approval_idle);
    assert_eq!(controller.system_approval().to_string(), "");
    assert_eq!(
        controller.approval_error().to_string(),
        "PkgDeck's system helper is not installed"
    );

    // While the password prompt is open, another toggle waits and the
    // answer is read once it arrives.
    let (sender, receiver) = mpsc::channel();
    controller.as_mut().rust_mut().approval_worker = Some(ApprovalWorker {
        handle: thread::spawn(|| {}),
        receiver,
    });
    controller.as_mut().allow_system_updates(false);
    controller.as_mut().poll();
    assert!(controller.rust().approval_worker.is_some());
    assert_eq!(
        controller.approval_error().to_string(),
        "PkgDeck's system helper is not installed"
    );
    sender.send(Ok("later-helper".into())).unwrap();
    controller.as_mut().poll();
    assert!(controller.rust().approval_worker.is_none());
    assert_eq!(controller.system_approval().to_string(), "later-helper");
}
#[test]
fn approval_failures_read_as_one_sentence() {
    let failed = |stderr: &str| {
        ExecutionError::Failed(pkgdeck_core::process::Completion {
            code: Some(1),
            signal: None,
            stdout: vec![],
            stderr: stderr.as_bytes().to_vec(),
            truncated: false,
            cancellation_deferred: false,
        })
    };
    for (error, sentence) in [
        (
            ExecutionError::AuthorizationCancelled,
            "The password prompt was cancelled.",
        ),
        (
            ExecutionError::AuthorizationDenied,
            "Your password wasn't accepted, so nothing changed.",
        ),
        (
            ExecutionError::Disabled("Not installed".into()),
            "Not installed",
        ),
        (
            ExecutionError::Invalid("Unexpected user".into()),
            "Unexpected user",
        ),
        (
            failed(
                "polkit noise\npkgdeck-host-runner: this system's polkit has no rules folder\n\n",
            ),
            "this system's polkit has no rules folder",
        ),
        (
            failed(" \n"),
            "PkgDeck's helper could not save the setting.",
        ),
        (ExecutionError::TimedOut, "host read timed out"),
    ] {
        assert_eq!(approval_error(&error), sentence);
    }
}
#[test]
fn a_saved_approval_counts_only_for_the_helper_it_names() {
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    assert!(!controller.rust().approval_current());
    controller
        .as_mut()
        .restore_system_approval("some-other-helper".into());
    let current = controller.rust().approval_current();
    // The test binary has no root-owned helper beside it on Linux.
    #[cfg(not(target_os = "macos"))]
    assert!(!current);
    #[cfg(target_os = "macos")]
    let _ = current;
    // Nothing that may update unattended: no run is queued.
    controller.as_mut().set_auto_update(true);
    let mut firmware = synthetic_package("firmware", "Firmware");
    firmware.id.backend = "fwupd".into();
    firmware.update = UpdateAvailability::Available;
    firmware.candidate_version = Some("2".into());
    let report = PackageReport {
        packages: vec![firmware],
        failures: vec![],
        successful_sources: vec!["fwupd".into()],
    };
    assert_eq!(controller.as_mut().schedule_auto_update(&report), None);
    assert!(controller.rust().validated_confirmed.is_none());
}
#[test]
fn automatic_runs_without_apt_skip_its_plan_and_hold_it_when_planning_fails() {
    let all = |backend: &str| Operation::UpgradeAll {
        backend: backend.into(),
    };
    let mut engine = Engine::default();
    engine
        .register(Scripted {
            id: "homebrew",
            progress: vec![],
        })
        .unwrap();
    let cancel = Cancellation::default();
    let replies = run_job(
        &mut engine,
        Job::AutoUpgrade(vec![all("homebrew")], false),
        &cancel,
    );
    let outcomes = expect!(
        replies.into_iter().last(),
        Some(Reply::Done(Ok(Payload::Batch(ran)))) => ran.outcomes().collect::<Vec<_>>()
    );
    assert_eq!(outcomes, [Outcome::Finished]);
    // APT isn't there to plan, so it is held with the reason and the
    // rest still runs.
    let replies = run_job(
        &mut engine,
        Job::AutoUpgrade(vec![all("apt"), all("homebrew")], true),
        &cancel,
    );
    let outcomes = expect!(
        replies.into_iter().last(),
        Some(Reply::Done(Ok(Payload::Batch(ran)))) => ran.outcomes().collect::<Vec<_>>()
    );
    assert_eq!(outcomes, [Outcome::Failed, Outcome::Finished]);
    // A cleanup batch names its tasks as such.
    let replies = run_job(
        &mut engine,
        Job::CleanAll(vec![Operation::Clean(CleanupId {
            backend: "fwupd".into(),
            key: "cache".into(),
        })]),
        &cancel,
    );
    let status = expect!(
        replies.into_iter().last(),
        Some(Reply::Done(Ok(Payload::Batch(ran)))) => ran.text(false)
    );
    assert!(
        status.starts_with("Completed 0 of 1 cleanup tasks."),
        "{status}"
    );
}
#[test]
fn details_workers_replace_older_lookups_and_report_failures() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let first = fixture_row();
    let mut second = fixture_row();
    second.id.name = "second".into();
    controller.as_mut().rust_mut().packages = vec![first.clone(), second.clone()];
    controller.as_mut().select(0);
    let older = controller
        .rust()
        .details_worker
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    controller.as_mut().select(1);
    assert!(older.requested());
    // Choosing the package that is loading does not start it again.
    let current = controller
        .rust()
        .details_worker
        .as_ref()
        .unwrap()
        .cancel
        .clone();
    controller.as_mut().select(1);
    assert!(!current.requested());
    settle(&mut controller);
    let (worker, gate) = held_worker(Job::Details(first.id.clone()), vec![]);
    controller.as_mut().rust_mut().worker = Some(worker);
    controller.as_mut().select(0);
    assert!(!controller
        .rust()
        .worker
        .as_ref()
        .unwrap()
        .cancel
        .requested());
    assert!(controller.rust().queued.is_none());
    assert!(controller.rust().details_worker.is_none());
    drop(gate);
    settle(&mut controller);
    // The fixture has no details; the row preview stays with the reason.
    let details: Value = serde_json::from_str(&controller.details().to_string()).unwrap();
    assert_eq!(details["package"]["name"], "second");
    let status = controller.status().to_string();
    assert_eq!(status, "No package matches the selection.");

    controller.as_mut().rust_mut().natives.engine = missing_engine;
    controller.as_mut().select(0);
    settle(&mut controller);
    assert!(controller
        .status()
        .to_string()
        .contains("synthetic system has no managers"));

    // Replies other than details are ignored.
    let (sender, receiver) = mpsc::channel();
    sender.send(Reply::Engine(Box::default())).unwrap();
    controller.as_mut().rust_mut().details_worker = Some(DetailsWorker {
        handle: thread::spawn(move || drop(sender)),
        receiver,
        cancel: Cancellation::default(),
        id: first.id.clone(),
    });
    let before = controller.details().to_string();
    settle(&mut controller);
    assert_eq!(controller.details().to_string(), before);
}
#[test]
fn failed_preloads_leave_their_section_unloaded() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().natives.engine = missing_engine;
    let key = cache_key("Installed", "", &[], false);
    controller
        .as_mut()
        .start_prefetch("Installed".into(), key.clone());
    wait_until(&mut controller, |controller| {
        controller.rust().prefetch_worker.is_none()
    });
    assert!(!controller.rust().prefetched.contains_key(&key));
}

#[test]
fn proposals_review_native_plans_and_retry_failed_sources_first() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let row = fixture_row();
    let mut apt = fixture_row();
    apt.id.backend = "apt".into();
    controller.as_mut().rust_mut().packages = vec![row.clone()];
    // Updating everything is offered only in the Updates section.
    controller.as_mut().propose("upgrade-all".into(), -1);
    assert!(controller.rust().worker.is_none());
    assert!(controller.rust().pending.is_none());

    controller.as_mut().rust_mut().updates_view = true;
    controller.as_mut().rust_mut().failures = vec![BackendFailure {
        backend: "fixture".into(),
        error: EngineError::Cancelled,
    }];
    controller.as_mut().propose("upgrade-all".into(), -1);
    assert_eq!(
        controller.status().to_string(),
        "Retrying failed source checks before updating…"
    );
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::RetryFailedUpdates(sources) if sources == &["fixture"])
    ));
    settle(&mut controller);

    // APT updates and APT changes to one package get a native plan first.
    controller.as_mut().rust_mut().failures.clear();
    controller.as_mut().rust_mut().packages = vec![apt.clone()];
    controller.as_mut().propose("upgrade-all".into(), -1);
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::PlanUpgrade(operations, 1)
            if operations == &[Operation::UpgradeAll { backend: "apt".into() }])
    ));
    settle(&mut controller);
    controller.as_mut().rust_mut().packages = vec![apt.clone()];
    controller.as_mut().propose("remove".into(), 0);
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::PlanOperation(Operation::Remove(id)) if *id == apt.id)
    ));
    settle(&mut controller);

    // Other managers confirm without a native plan.
    controller.as_mut().rust_mut().packages = vec![row.clone()];
    controller.as_mut().propose("remove".into(), 0);
    assert!(controller.rust().worker.is_none());
    assert!(matches!(
        &controller.rust().pending,
        Some(Job::Write(Operation::Remove(id), None)) if *id == row.id
    ));

    let mut second = row.clone();
    second.id.name = "second".into();
    controller.as_mut().rust_mut().packages = vec![row, second];
    controller.as_mut().propose_checked(
            r#"[["fixture","synthetic","all",null,"system",null],["fixture","second","all",null,"system",null]]"#.into(),
        );
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert_eq!(
        data["summary"],
        "Update 2 selected packages\nIf one update fails, the others can still finish."
    );

    // The Software Sources editor opens without a confirmation.
    controller.as_mut().rust_mut().pending = None;
    controller.as_mut().change_repository(
        r#"{"backend":"apt","name":"sources","scope":"system","action":"open_editor"}"#.into(),
    );
    assert!(controller.rust().pending.is_none());
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::Repositories(Some(action))
            if matches!(action.change, repositories::Change::OpenEditor))
    ));
    settle(&mut controller);
    assert!(controller.status().to_string().contains("unavailable"));
}
#[test]
fn applied_results_change_only_what_they_describe() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let row = fixture_row();
    controller
        .as_mut()
        .apply(Ok(Payload::BackgroundUpdates(PackageReport {
            packages: vec![row.clone()],
            failures: vec![],
            successful_sources: vec!["fixture".into()],
        })));
    let state: Value = serde_json::from_str(&controller.background_state().to_string()).unwrap();
    assert_eq!(state["available"], 1);

    // An opened APT package is reviewed with its native plan.
    let mut apt = fixture_row();
    apt.id.backend = "apt".into();
    apt.installed_version = None;
    controller.as_mut().apply(Ok(opened_payload(apt.clone())));
    controller.as_mut().install_opened();
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::PlanOperation(Operation::Install(id)) if *id == apt.id)
    ));
    settle(&mut controller);

    // A queued update whose plan is unchanged may run.
    let operations = vec![Operation::UpgradeAll {
        backend: "fixture".into(),
    }];
    let queued = |job| Confirmed {
        job,
        activity_id: None,
        cleanup_preview: vec![],
    };
    controller.as_mut().rust_mut().revalidating =
        Some(queued(Job::UpgradeAll(operations.clone(), None)));
    controller
        .as_mut()
        .apply(Ok(Payload::UpgradePreview(operations.clone(), 1, None)));
    assert!(controller.rust().validated_confirmed.is_some());
    assert!(controller.rust().pending.is_none());
    controller.as_mut().rust_mut().validated_confirmed = None;

    // A changed one asks again, naming what could not be checked.
    controller.as_mut().rust_mut().revalidating = Some(queued(Job::UpgradeAll(vec![], None)));
    controller.as_mut().rust_mut().packages = vec![row.clone()];
    controller.as_mut().rust_mut().failures = ["apt", "dnf"]
        .map(|backend| BackendFailure {
            backend: backend.into(),
            error: EngineError::Cancelled,
        })
        .into();
    let plan = AptUpgradePlan {
        preview: String::new(),
        upgrades: vec!["synthetic".into()],
        installs: vec![],
        removals: vec![],
    };
    controller.as_mut().apply(Ok(Payload::UpgradePreview(
        operations.clone(),
        1,
        Some(plan.clone()),
    )));
    assert!(controller.rust().revalidating.is_none());
    assert!(controller.rust().validated_confirmed.is_none());
    assert!(matches!(
        &controller.rust().pending,
        Some(Job::UpgradeAll(pending, Some(reviewed))) if *pending == operations && *reviewed == plan
    ));
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert_eq!(
        data["summary"],
        "Update 1 package\n2 sources could not be checked. Updates from them are not included."
    );
    controller.as_mut().rust_mut().failures.clear();
    controller
        .as_mut()
        .apply(Ok(Payload::UpgradePreview(operations.clone(), 2, None)));
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert_eq!(
        data["summary"],
        "Update 2 packages\nIf one update fails, the others can still finish."
    );

    // A queued cleanup whose tasks changed is confirmed again.
    let items = vec![cleanup_item("orphans"), cleanup_item("cache")];
    let cleanups: Vec<_> = items
        .iter()
        .map(|item| Operation::Clean(item.id.clone()))
        .collect();
    controller.as_mut().rust_mut().pending = None;
    controller.as_mut().rust_mut().revalidating = Some(queued(Job::CleanAll(cleanups.clone())));
    controller
        .as_mut()
        .apply(Ok(Payload::CleanPreview(cleanups.clone(), items.clone())));
    let data: Value = serde_json::from_str(&controller.confirmation_data().to_string()).unwrap();
    assert_eq!(data["action"], "Clean 2 tasks");
    assert!(
        matches!(&controller.rust().pending, Some(Job::CleanAll(pending)) if *pending == cleanups)
    );
    // Without a queued cleanup a preview only refreshes nothing.
    controller.as_mut().rust_mut().pending = None;
    controller
        .as_mut()
        .apply(Ok(Payload::CleanPreview(cleanups, items)));
    assert!(controller.rust().pending.is_none());

    // Every source failing is a failed read.
    controller.as_mut().apply(Ok(Payload::Sources(vec![Source {
        backend: "fixture".into(),
        capabilities: vec![],
        availability: Err(EngineError::Cancelled),
    }])));
    let state: Value = serde_json::from_str(&controller.report_state().to_string()).unwrap();
    assert_eq!(state["phase"], "failed");

    // The details cache stays bounded.
    for index in 0..128 {
        let mut id = row.id.clone();
        id.name = format!("cached-{index}");
        controller
            .as_mut()
            .rust_mut()
            .detail_cache
            .insert(id, "{}".into());
    }
    controller
        .as_mut()
        .apply(Ok(Payload::Details(fixture_details("Fresh"))));
    assert_eq!(controller.rust().detail_cache.len(), 1);
    assert!(controller.rust().detail_cache.contains_key(&row.id));
}

fn described_engine(
    _: &[String],
    _: bool,
    _: Authorization,
    _: &Cancellation,
) -> Result<Engine, EngineError> {
    let mut row = fixture_row();
    row.id.backend = "described".into();
    let mut engine = Engine::default();
    engine.register(Described(row))?;
    Ok(engine)
}
#[test]
fn elevated_reads_and_opened_packages_use_their_own_workers() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    // An empty search only remembers the chosen sources and elevation.
    controller
        .as_mut()
        .load("Search".into(), "  ".into(), "apt".into(), true, false);
    assert!(controller.rust().worker.is_none());
    assert_eq!(controller.rust().source_filter, ["apt"]);
    assert!(controller.rust().sudo);

    controller
        .as_mut()
        .load("Installed".into(), "".into(), "".into(), true, true);
    assert!(matches!(
        &controller.rust().worker,
        Some(worker) if matches!(&worker.job, Job::Load(view, _) if view == "Installed")
    ));
    settle(&mut controller);
    assert!(controller.rows().to_string().contains("synthetic"));
    controller
        .as_mut()
        .start_prefetch("Updates".into(), cache_key("Updates", "", &[], true));
    wait_until(&mut controller, |controller| {
        controller.rust().prefetch_worker.is_none()
    });
    assert!(controller
        .rust()
        .prefetched
        .contains_key(&cache_key("Updates", "", &[], true)));

    // Details of an opened package arrive from the details worker.
    let mut row = fixture_row();
    row.id.backend = "described".into();
    controller.as_mut().rust_mut().natives.engine = described_engine;
    controller.as_mut().rust_mut().packages = vec![row];
    controller.as_mut().select(0);
    assert!(controller.rust().details_worker.is_some());
    settle(&mut controller);
    let details: Value = serde_json::from_str(&controller.details().to_string()).unwrap();
    assert_eq!(details["description"], "Described by the warm engine");
}
#[test]
fn rows_offer_installs_and_source_refreshes() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let mut row = fixture_row();
    row.installed_version = None;
    controller.as_mut().rust_mut().packages = vec![row.clone()];
    controller.as_mut().propose("install".into(), 0);
    assert!(matches!(
        &controller.rust().pending,
        Some(Job::Write(Operation::Install(id), None)) if *id == row.id
    ));
    controller.as_mut().rust_mut().sources = vec![Source {
        backend: "fixture".into(),
        capabilities: vec![Capability::Refresh],
        availability: Ok(Availability::Available),
    }];
    controller.as_mut().propose("refresh".into(), 0);
    assert!(matches!(
        &controller.rust().pending,
        Some(Job::Write(Operation::Refresh { backend }, None)) if backend == "fixture"
    ));
}

#[test]
fn running_worker_events_publish_progress_only_while_tracking_a_write() {
    let mut controller = synthetic_controller();
    let mut controller = controller.pin_mut();
    let operation = Operation::Refresh {
        backend: "apt".into(),
    };
    let job = Job::Write(operation.clone(), None);
    controller.as_mut().rust_mut().progress_state =
        Some(ProgressState::new(&job, Some(42), &Names::new()));
    let (sender, receiver) = mpsc::channel();
    let (release, hold) = mpsc::channel::<()>();
    // Keep the worker alive until after polling: a finished worker takes
    // a different path that clears progress instead of publishing it.
    controller.as_mut().rust_mut().worker = Some(Worker {
        handle: thread::spawn(move || {
            let _ = hold.recv();
        }),
        receiver,
        cancel: Cancellation::default(),
        job,
    });
    sender
        .send(Reply::ProgressEvent(Event::Started(operation.clone())))
        .unwrap();
    sender
        .send(Reply::ProgressEvent(Event::Progress {
            operation: operation.clone(),
            progress: Progress::Transfer {
                completed: 50,
                total: Some(100),
            },
        }))
        .unwrap();
    controller.as_mut().poll();
    let progress: Value = serde_json::from_str(&controller.progress().to_string()).unwrap();
    assert_eq!(progress["activity_id"], 42);
    assert_eq!(progress["label"], "Refreshing APT package lists…");
    assert_eq!(progress["transferred"], 50);
    assert_eq!(progress["transfer_total"], 100);

    // A late event without a tracked write leaves the visible snapshot.
    controller.as_mut().rust_mut().progress_state = None;
    sender
        .send(Reply::ProgressEvent(Event::Finished {
            operation,
            result: Ok(OperationOutcome::default()),
        }))
        .unwrap();
    controller.as_mut().poll();
    assert_eq!(
        serde_json::from_str::<Value>(&controller.progress().to_string()).unwrap(),
        progress
    );
    drop(release);
    drop(sender);
    settle(&mut controller);
    assert_eq!(controller.progress().to_string(), "{}");
}
/// Whether the build `BuildingCaches` waits for still runs.
static CARGO_BUILDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// A Cargo cache that a build uses until `CARGO_BUILDING` turns off.
struct BuildingCaches;
impl Backend for BuildingCaches {
    fn id(&self) -> &str {
        "dev-caches"
    }
    fn capabilities(&self) -> &[Capability] {
        &[Capability::Clean]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Available)
    }
    fn cleanup(&mut self, _: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        Ok(vec![sized(
            "dev-caches",
            "cargo",
            CleanupKind::PackageCache,
            Some(10),
        )])
    }
    fn execute(
        &mut self,
        _: &Operation,
        _: &Cancellation,
        _: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        if CARGO_BUILDING.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(EngineError::Refused {
                backend: "dev-caches".into(),
                refusal: Refusal::Building,
            });
        }
        Ok(OperationOutcome::default())
    }
}
fn building_caches(
    _: &[String],
    _: bool,
    _: Authorization,
    _: &Cancellation,
) -> Result<Engine, EngineError> {
    let mut engine = Engine::default();
    engine.register(BuildingCaches)?;
    Ok(engine)
}
#[test]
fn a_cleanup_a_build_uses_waits_and_cleans_once_the_build_ends() {
    let temp = pkgdeck_tools::Temp::new();
    let store = ActivityStore::at(temp.0.join("activity.json"));
    let item = sized("dev-caches", "cargo", CleanupKind::PackageCache, Some(10));
    // Automatic cleaning finds the cache in use.
    let mut engine =
        building_caches(&[], false, Authorization::Polkit, &Cancellation::default()).unwrap();
    let replies = run_job(
        &mut engine,
        Job::AutoClean {
            caches: true,
            worktrees: false,
            preferences: CleanupPreferences::default(),
        },
        &Cancellation::default(),
    );
    let payload = expect!(replies.into_iter().last(), Some(Reply::Done(Ok(payload))) => payload);
    assert!(matches!(
        &payload,
        Payload::AutoCleaned { cleaned: 0, failed: 0, bytes: 0, tasks, .. }
            if tasks == &[(item.clone(), Outcome::Waiting, Some(WAITS_FOR_BUILD.to_owned()))]
    ));
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().natives.engine = building_caches;
    controller.as_mut().rust_mut().activity_store = Some(store.clone());
    controller.as_mut().apply(Ok(payload));
    // Activity shows it, waiting and why.
    let automatic = store.entries().unwrap().pop().unwrap();
    assert_eq!(automatic.frontend, "auto");
    assert_eq!(automatic.state, State::Finished);
    assert_eq!(automatic.outcomes, [Outcome::Waiting]);
    assert_eq!(automatic.reasons, [Some(WAITS_FOR_BUILD.to_owned())]);
    assert_eq!(automatic.names, [Some("cargo task".to_owned())]);
    assert_eq!(controller.rust().names.cleanup[&item.id], "cargo task");

    // Cleaning it by hand meanwhile waits too, without failing its row.
    controller.as_mut().rust_mut().cleanup = vec![item.clone()];
    let clean = Operation::Clean(item.id.clone());
    let by_hand = store
        .begin("gui", vec![clean.clone()], vec![], State::Running)
        .unwrap();
    controller.as_mut().rust_mut().active_activity_id = Some(by_hand);
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::Write(clean.clone(), None),
        vec![Reply::Done(Err(EngineError::Refused {
            backend: "dev-caches".into(),
            refusal: Refusal::Building,
        }))],
    ));
    controller.as_mut().poll();
    let notice: Value = serde_json::from_str(&controller.notice().to_string()).unwrap();
    assert_eq!(notice["kind"], "info");
    assert_eq!(notice["title"], "Clean cargo task waits for a build");
    assert_eq!(notice["detail"], WAITS_FOR_BUILD);
    assert_eq!(controller.failure().to_string(), "{}");
    let entry = store.entries().unwrap().pop().unwrap();
    assert_eq!(
        (entry.id, entry.outcomes),
        (by_hand, vec![Outcome::Waiting])
    );
    assert_eq!(
        controller.rust().waiting_cleans,
        [WaitingClean {
            item: item.clone(),
            activity: vec![(automatic.id, 0), (by_hand, 0)],
        }]
    );

    // It isn't tried while something else waits to run.
    controller.as_mut().rust_mut().pending = Some(Job::CleanAll(vec![]));
    controller.as_mut().retry_waiting_cleans();
    assert!(controller.rust().worker.is_none());
    controller.as_mut().rust_mut().pending = None;
    let retrying = |controller: &ffi::PackageController| {
        matches!(
            controller.rust().worker.as_ref().map(|worker| &worker.job),
            Some(Job::RetryCleans(_))
        )
    };
    // Still building: it keeps waiting, and isn't tried again for a minute.
    controller.as_mut().retry_waiting_cleans();
    assert!(matches!(
        controller.rust().worker.as_ref().map(|worker| &worker.job),
        Some(Job::RetryCleans(items)) if items == std::slice::from_ref(&item)
    ));
    wait_until(&mut controller, |controller| !retrying(controller));
    assert_eq!(controller.rust().waiting_cleans.len(), 1);
    controller.as_mut().retry_waiting_cleans();
    assert!(!retrying(&controller));
    // The build ended: it cleans, and both runs say so.
    CARGO_BUILDING.store(false, std::sync::atomic::Ordering::SeqCst);
    controller.as_mut().rust_mut().cleans_tried_at = 0;
    wait_until(&mut controller, |controller| {
        controller.rust().worker.is_none()
    });
    controller.as_mut().retry_waiting_cleans();
    assert!(retrying(&controller));
    wait_until(&mut controller, |controller| {
        controller.rust().waiting_cleans.is_empty()
    });
    for entry in store.entries().unwrap() {
        assert_eq!(entry.outcomes, [Outcome::Finished], "{}", entry.id);
        assert!(entry.reasons.is_empty());
    }
    // Nothing waits, so nothing is tried.
    controller.as_mut().rust_mut().cleans_tried_at = 0;
    wait_until(&mut controller, |controller| {
        controller.rust().worker.is_none()
    });
    controller.as_mut().retry_waiting_cleans();
    assert!(!retrying(&controller));
}
#[test]
fn a_retry_settles_only_the_tasks_it_ran() {
    let temp = pkgdeck_tools::Temp::new();
    let store = ActivityStore::at(temp.0.join("activity.json"));
    let item = |key| sized("dev-caches", key, CleanupKind::PackageCache, None);
    let (cargo, gradle, bun) = (item("cargo"), item("gradle"), item("bun"));
    let run = store
        .record(
            "auto",
            [&cargo, &gradle, &bun]
                .iter()
                .map(|item| Operation::Clean(item.id.clone()))
                .collect(),
            vec![],
            1,
            vec![(Outcome::Waiting, Some(WAITS_FOR_BUILD.into())); 3],
        )
        .unwrap();
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().rust_mut().activity_store = Some(store.clone());
    let waiting = |item: &CleanupItem, index| WaitingClean {
        item: item.clone(),
        activity: vec![(run, index)],
    };
    controller.as_mut().rust_mut().keep_waiting(vec![
        waiting(&cargo, 0),
        waiting(&gradle, 1),
        waiting(&bun, 2),
    ]);
    let step = |outcome, kept: Option<&str>| Ended {
        title: String::new(),
        outcome,
        said: Said::Completed,
        kept: kept.map(str::to_owned),
    };
    // Gradle failed after all, Bun's retry was cancelled, and a task that
    // stopped waiting meanwhile is left alone.
    let other = item("go-build");
    controller.as_mut().finish_retried_cleans(
        &[gradle.clone(), bun.clone(), other],
        &Batch {
            noun: "cleanup tasks",
            steps: vec![
                step(Outcome::Failed, Some("Disk full.")),
                step(Outcome::Cancelled, None),
                step(Outcome::Finished, None),
            ],
            note: None,
        },
    );
    let entry = store.entries().unwrap().pop().unwrap();
    assert_eq!(
        entry.outcomes,
        [Outcome::Waiting, Outcome::Failed, Outcome::Waiting]
    );
    assert_eq!(entry.reasons[1].as_deref(), Some("Disk full."));
    assert_eq!(entry.state, State::Failed);
    let left: Vec<_> = controller
        .rust()
        .waiting_cleans
        .iter()
        .map(|waiting| waiting.item.id.key.as_str())
        .collect();
    assert_eq!(left, ["cargo", "bun"]);
    // A retry's result settles nothing when it reaches the foreground.
    controller
        .as_mut()
        .apply(Ok(Payload::RetriedCleans(Batch::default())));
    assert_eq!(controller.rust().waiting_cleans.len(), 2);
    // A task Clean no longer lists has no review to wait with.
    controller.as_mut().wait_for_builds(
        &Job::Write(Operation::Clean(item("gone").id), None),
        &[(Outcome::Waiting, Some(WAITS_FOR_BUILD.into()))],
        Some(run),
    );
    assert_eq!(controller.rust().waiting_cleans.len(), 2);
    // A failed retry keeps them waiting too.
    controller.as_mut().rust_mut().background = true;
    controller.as_mut().rust_mut().worker = Some(fake_worker(
        Job::RetryCleans(vec![cargo.clone()]),
        vec![Reply::Done(Err(EngineError::Unavailable {
            backend: "dev-caches".into(),
            reason: "synthetic".into(),
        }))],
    ));
    controller.as_mut().poll();
    assert_eq!(controller.rust().waiting_cleans.len(), 2);
    assert_eq!(controller.failure().to_string(), "{}");
    // Without a history, a cleaned task just stops waiting.
    controller.as_mut().rust_mut().activity_store = None;
    controller.as_mut().finish_retried_cleans(
        &[cargo],
        &Batch {
            noun: "cleanup tasks",
            steps: vec![step(Outcome::Finished, None)],
            note: None,
        },
    );
    assert_eq!(controller.rust().waiting_cleans.len(), 1);
}
#[test]
fn a_cleanup_left_alone_is_not_a_failure() {
    let building = EngineError::Refused {
        backend: "dev-caches".into(),
        refusal: Refusal::Building,
    };
    assert_eq!(
        ended_by(&building, "dev-caches"),
        (Outcome::Waiting, Some(WAITS_FOR_BUILD.into()))
    );
    let changed = EngineError::Refused {
        backend: "podman".into(),
        refusal: Refusal::CleanupChanged,
    };
    assert_eq!(
        ended_by(&changed, "podman"),
        (Outcome::Skipped, Some(Refusal::CleanupChanged.to_string()))
    );
    assert_eq!(
        ended_by(&EngineError::Cancelled, "apt"),
        (Outcome::Cancelled, None)
    );
    // What a tool printed stays out of what Activity keeps.
    let failed =
        EngineError::Execution(ExecutionError::Failed(pkgdeck_core::process::Completion {
            code: Some(1),
            signal: None,
            stdout: vec![],
            stderr: b"secret detail".to_vec(),
            truncated: false,
            cancellation_deferred: false,
        }));
    assert_eq!(
        ended_by(&failed, "npm"),
        (Outcome::Failed, Some("npm stopped with an error.".into()))
    );
    let confused = EngineError::InvalidResponse {
        backend: "snap".into(),
        reason: "secret detail".into(),
    };
    assert_eq!(
        ended_by(&confused, "snap"),
        (Outcome::Failed, Some("Snap reported a problem.".into()))
    );
    let (outcome, kept) = ended_by(&EngineError::NotFound, "projects");
    assert_eq!(outcome, Outcome::Failed);
    assert_eq!(
        kept,
        Some(plain_error(&EngineError::NotFound, Some("projects"), false))
    );
    // A batch says what waits after what failed, and keeps the reasons.
    let job = Job::CleanAll(vec![
        Operation::Clean(sized("dev-caches", "cargo", CleanupKind::PackageCache, None).id),
        Operation::Clean(sized("podman", "images", CleanupKind::Unused, None).id),
        Operation::Clean(sized("projects", "app", CleanupKind::BuildFiles, None).id),
    ]);
    let step = |title: &str, outcome, said: &str| Ended {
        title: title.into(),
        outcome,
        said: Said::Because(said.into()),
        kept: Some(said.into()),
    };
    let ran = Batch {
        noun: "cleanup tasks",
        steps: vec![
            step("Clean Cargo", Outcome::Waiting, WAITS_FOR_BUILD),
            step("Clean images", Outcome::Skipped, "It changed."),
            step("Clean app", Outcome::Failed, "Disk full."),
        ],
        note: None,
    };
    assert_eq!(
        ran.ended(),
        [
            (Outcome::Waiting, Some(WAITS_FOR_BUILD.into())),
            (Outcome::Skipped, Some("It changed.".into())),
            (Outcome::Failed, Some("Disk full.".into())),
        ]
    );
    let names = Names::default();
    let notice = write_notice(&job, &Ok(Payload::Batch(ran.clone())), false, &names);
    assert_eq!(notice["kind"], "error");
    assert_eq!(
        notice["detail"],
        format!(
            "Clean app (Old projects): Disk full.\nClean Cargo: {WAITS_FOR_BUILD}\nClean images: It changed."
        )
    );
    // Without a failure it says how many finished.
    let mut fine = ran;
    fine.steps.pop();
    let notice = write_notice(&job, &Ok(Payload::Batch(fine)), false, &names);
    assert_eq!(notice["kind"], "info");
    assert_eq!(notice["title"], "0 of 2 cleanup tasks finished");
}
#[test]
fn automatic_updates_during_a_build_hold_only_what_a_build_may_run() {
    let package = |backend: &str, name: &str| {
        let mut package = synthetic_package(name, name);
        package.id.backend = backend.into();
        package.update = UpdateAvailability::Available;
        package.candidate_version = Some("2".into());
        package
    };
    let report = |packages: Vec<Package>| PackageReport {
        successful_sources: packages.iter().map(|p| p.id.backend.clone()).collect(),
        packages,
        failures: vec![],
    };
    let state = |controller: &ffi::PackageController| -> Value {
        serde_json::from_str(&controller.background_state().to_string()).unwrap()
    };
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().set_auto_update(true);
    controller.as_mut().rust_mut().natives.hold = building;
    // A coding assistant updates; the tools a build may run wait.
    controller.as_mut().finish_background_check(report(vec![
        package("homebrew", "wget"),
        package("npm", "typescript"),
        package("codex", "codex"),
    ]));
    let confirmed = controller.rust().validated_confirmed.as_ref().unwrap();
    assert!(matches!(
        &confirmed.job,
        Job::AutoUpgrade(operations, _)
            if operations.iter().map(Operation::backend).collect::<Vec<_>>() == ["codex"]
    ));
    assert_eq!(controller.rust().auto_held, Some(Hold::Build));
    assert_eq!(state(&controller)["waiting"], "build");
    assert_eq!(
        state(&controller)["waiting_for"],
        json!(["Homebrew", "npm"])
    );
    // Only what a build never runs: nothing waits.
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().set_auto_update(true);
    controller.as_mut().rust_mut().natives.hold = building;
    controller
        .as_mut()
        .finish_background_check(report(vec![package("codex", "codex")]));
    assert!(controller.rust().validated_confirmed.is_some());
    assert_eq!(controller.rust().auto_held, None);
    assert_eq!(state(&controller)["waiting"], Value::Null);
    assert_eq!(state(&controller)["waiting_for"], json!([]));
    // Only tools: they all wait.
    let mut controller = idle_controller();
    let mut controller = controller.pin_mut();
    controller.as_mut().set_auto_update(true);
    controller.as_mut().rust_mut().natives.hold = building;
    controller
        .as_mut()
        .finish_background_check(report(vec![package("homebrew", "wget")]));
    assert!(controller.rust().validated_confirmed.is_none());
    assert_eq!(state(&controller)["waiting_for"], json!(["Homebrew"]));
}
