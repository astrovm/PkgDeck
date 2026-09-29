//! Policy for read-only background update checks. The caller supplies a clock and
//! network state so scheduling can be tested without waiting or networking.
#[cfg(target_os = "linux")]
use crate::host::Host;
use crate::{
    engine::PackageReport,
    host::Runtime,
    package::{PackageId, UpdateAvailability},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

pub const CHECK_INTERVAL_SECONDS: u64 = 30 * 60;

#[derive(Default)]
pub struct Schedule {
    next_due: u64,
    notified: BTreeMap<String, BTreeSet<(PackageId, Option<String>)>>,
    pending: BTreeMap<String, BTreeSet<(PackageId, Option<String>)>>,
}
#[derive(Default, Serialize, Deserialize)]
struct NotificationHistory {
    #[serde(default)]
    notified: BTreeMap<String, BTreeSet<(PackageId, Option<String>)>>,
}
pub struct CheckResult {
    pub count: usize,
    pub failures: usize,
    pub notify: bool,
}
impl Schedule {
    pub fn restore_notifications(&mut self, encoded: &str) {
        if let Ok(history) = serde_json::from_str::<NotificationHistory>(encoded) {
            self.notified = history.notified;
        }
    }
    pub fn notification_history(&self) -> String {
        serde_json::to_string(&NotificationHistory {
            notified: self.notified.clone(),
        })
        .expect("notification history is serializable")
    }
    pub fn acknowledge_notification(&mut self) {
        for (source, updates) in std::mem::take(&mut self.pending) {
            self.notified.insert(source, updates);
        }
    }
    pub fn ready(
        &mut self,
        now: u64,
        enabled: bool,
        offline: bool,
        metered: bool,
        busy: bool,
        force: bool,
    ) -> bool {
        if !enabled && !force || offline || metered || busy || !force && now < self.next_due {
            return false;
        }
        self.next_due = now.saturating_add(CHECK_INTERVAL_SECONDS);
        true
    }
    pub fn complete(&mut self, report: &PackageReport) -> CheckResult {
        let mut current: BTreeMap<String, BTreeSet<_>> = report
            .successful_sources
            .iter()
            .map(|source| (source.clone(), BTreeSet::new()))
            .collect();
        for package in report
            .packages
            .iter()
            .filter(|package| package.update == UpdateAvailability::Available)
        {
            if let Some(updates) = current.get_mut(&package.id.backend) {
                updates.insert((package.id.clone(), package.candidate_version.clone()));
            }
        }
        let count = current.values().map(BTreeSet::len).sum();
        self.pending.clear();
        for (source, updates) in current {
            let notified = self.notified.entry(source.clone()).or_default();
            notified.retain(|update| updates.contains(update));
            if updates.iter().any(|update| !notified.contains(update)) {
                self.pending.insert(source, updates);
            }
        }
        CheckResult {
            count,
            failures: report.failures.len(),
            notify: !self.pending.is_empty(),
        }
    }
}

/// Where the start-at-login entry lives: an XDG autostart entry on Linux,
/// a per-user LaunchAgent on macOS.
pub fn autostart_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    return launch_agent_path_from(std::env::var_os("HOME"));
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return None;
    #[cfg(target_os = "linux")]
    {
        // In Flatpak, Host::current reads the host user's XDG paths through the
        // bridge, instead of the sandbox's private configuration directory.
        let host = Host::current();
        autostart_path_from(host.var("XDG_CONFIG_HOME"), host.var("HOME"))
    }
}
#[cfg(any(target_os = "linux", test))]
fn autostart_path_from(config: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = config
        .filter(|p| Path::new(p).is_absolute())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|p| Path::new(p).is_absolute())
                .map(|home| PathBuf::from(home).join(".config"))
        })?;
    Some(base.join("autostart/io.github.astrovm.PkgDeck.desktop"))
}
#[cfg(any(target_os = "macos", test))]
fn launch_agent_path_from(home: Option<OsString>) -> Option<PathBuf> {
    home.filter(|p| Path::new(p).is_absolute()).map(|home| {
        PathBuf::from(home).join("Library/LaunchAgents/io.github.astrovm.PkgDeck.plist")
    })
}
pub fn set_autostart(path: &Path, enabled: bool) -> io::Result<()> {
    let environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    let runtime = Runtime::detect(&environment, Path::new("/.flatpak-info").exists());
    let appimage = environment.get(&OsString::from("APPIMAGE")).map(Path::new);
    set_autostart_for(path, enabled, runtime, appimage)
}
fn set_autostart_for(
    path: &Path,
    enabled: bool,
    runtime: Runtime,
    appimage: Option<&Path>,
) -> io::Result<()> {
    if !enabled {
        if path.exists() {
            fs::remove_file(path)?;
        }
        return Ok(());
    }
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid autostart path"))?,
    )?;
    if path
        .extension()
        .is_some_and(|extension| extension == "plist")
    {
        let plist = launch_agent(&std::env::current_exe()?)?;
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        file.write_all(plist.as_bytes())?;
        return file.sync_all();
    }
    let executable = if matches!(runtime, Runtime::Native | Runtime::AppImage) {
        std::env::current_exe()?
    } else {
        PathBuf::new()
    };
    let exec = autostart_exec(runtime, &executable, appimage)?;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    file.write_all(format!("[Desktop Entry]\nType=Application\nName=PkgDeck\nExec={exec}\nIcon=io.github.astrovm.PkgDeck\nX-GNOME-Autostart-enabled=true\n").as_bytes())?;
    file.sync_all()
}

/// A LaunchAgent that opens PkgDeck in the background at login. Inside an
/// app bundle it goes through `open`, so macOS starts the app the usual way
/// (one copy, Dock and menu bar set up); outside one it runs the program.
fn launch_agent(executable: &Path) -> io::Result<String> {
    let path = executable
        .to_str()
        .filter(|value| executable.is_absolute() && !value.chars().any(char::is_control))
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid autostart executable")
        })?;
    let bundle = executable
        .ancestors()
        .find(|ancestor| {
            ancestor
                .extension()
                .is_some_and(|extension| extension == "app")
        })
        .and_then(Path::to_str);
    let arguments: Vec<&str> = match bundle {
        Some(bundle) => vec![
            "/usr/bin/open",
            "-g",
            "-j",
            "-a",
            bundle,
            "--args",
            "--background",
        ],
        None => vec![path, "--background"],
    };
    let escape = |value: &str| {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let arguments: String = arguments
        .iter()
        .map(|argument| format!("        <string>{}</string>\n", escape(argument)))
        .collect();
    Ok([
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
        "<plist version=\"1.0\">\n<dict>\n",
        "    <key>Label</key>\n    <string>io.github.astrovm.PkgDeck</string>\n",
        "    <key>ProgramArguments</key>\n    <array>\n",
        &arguments,
        "    </array>\n",
        "    <key>RunAtLoad</key>\n    <true/>\n",
        "    <key>LimitLoadToSessionType</key>\n    <string>Aqua</string>\n",
        "    <key>ProcessType</key>\n    <string>Interactive</string>\n",
        "</dict>\n</plist>\n",
    ]
    .concat())
}

fn autostart_exec(
    runtime: Runtime,
    executable: &Path,
    appimage: Option<&Path>,
) -> io::Result<String> {
    match runtime {
        Runtime::Flatpak => Ok("flatpak run io.github.astrovm.PkgDeck --background".into()),
        Runtime::Snap => Ok("snap run pkgdeck --background".into()),
        Runtime::Native | Runtime::AppImage => {
            let path = if runtime == Runtime::AppImage {
                appimage
                    .filter(|path| path.is_absolute())
                    .unwrap_or(executable)
            } else {
                executable
            };
            let value = path
                .to_str()
                .filter(|value| path.is_absolute() && !value.chars().any(char::is_control))
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "invalid autostart executable")
                })?;
            // Desktop Entry Exec has its own quoting and string escaping rules.
            let mut quoted = String::from("\"");
            for character in value.chars() {
                match character {
                    '%' => quoted.push_str("%%"),
                    '\\' => quoted.push_str("\\\\\\\\"),
                    '"' => quoted.push_str("\\\\\""),
                    '$' => quoted.push_str("\\\\$"),
                    '`' => quoted.push_str("\\\\`"),
                    other => quoted.push(other),
                }
            }
            quoted.push('"');
            Ok(format!("{quoted} --background"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schedule_skips_offline_metered_busy_and_resume_bursts() {
        let mut schedule = Schedule::default();
        assert!(!schedule.ready(10, false, false, false, false, false));
        assert!(!schedule.ready(10, true, true, false, false, false));
        assert!(!schedule.ready(10, true, false, true, false, false));
        assert!(!schedule.ready(10, true, false, false, true, false));
        assert!(schedule.ready(10, true, false, false, false, false));
        assert!(!schedule.ready(20, true, false, false, false, false));
        assert!(schedule.ready(
            10 + CHECK_INTERVAL_SECONDS,
            true,
            false,
            false,
            false,
            false
        ));
        assert!(!schedule.ready(
            10 + CHECK_INTERVAL_SECONDS + 1,
            true,
            true,
            false,
            false,
            true
        ));
    }
    #[test]
    fn first_check_notifies_once_and_history_survives_restart() {
        let mut schedule = Schedule::default();
        let mut report = PackageReport::default();
        report.successful_sources.push("apt".into());
        assert!(!schedule.complete(&report).notify);
        let package = serde_json::from_value(serde_json::json!({
            "id": {"backend":"apt", "name":"synthetic", "architecture":"amd64", "scope":"system"},
            "display_name":"Synthetic", "summary":"Test", "installed_version":"1", "candidate_version":"2", "update":"available"
        })).unwrap();
        report.packages.push(package);
        assert!(schedule.complete(&report).notify);
        assert!(schedule.complete(&report).notify); // Delivery has not been acknowledged.
        schedule.acknowledge_notification();
        assert!(!schedule.complete(&report).notify);
        let history = schedule.notification_history();
        let mut restarted = Schedule::default();
        restarted.restore_notifications(&history);
        assert!(!restarted.complete(&report).notify);
        report.packages[0].candidate_version = Some("3".into());
        assert!(restarted.complete(&report).notify);
        restarted.acknowledge_notification();
        report.packages.clear();
        assert!(!restarted.complete(&report).notify); // Removal is not an alert.
        assert!(!restarted.notification_history().is_empty());
    }
    #[test]
    fn failed_source_does_not_silence_successful_source_or_erase_its_history() {
        let mut schedule = Schedule::default();
        let mut report = PackageReport::default();
        let package = serde_json::from_value(serde_json::json!({
            "id": {"backend":"apt", "name":"synthetic", "architecture":"amd64", "scope":"system"},
            "display_name":"Synthetic", "summary":"Test", "installed_version":"1", "candidate_version":"2", "update":"available"
        })).unwrap();
        report.packages.push(package);
        report.successful_sources.push("apt".into());
        report.failures.push(crate::engine::BackendFailure {
            backend: "other".into(),
            error: crate::engine::EngineError::Cancelled,
        });
        assert!(schedule.complete(&report).notify);
        schedule.acknowledge_notification();
        report.packages.clear();
        report.successful_sources.clear();
        report.failures[0].backend = "apt".into();
        assert!(!schedule.complete(&report).notify);
        report.failures.clear();
        report.successful_sources.push("apt".into());
        report.packages.push(serde_json::from_value(serde_json::json!({
            "id": {"backend":"apt", "name":"synthetic", "architecture":"amd64", "scope":"system"},
            "display_name":"Synthetic", "summary":"Test", "installed_version":"1", "candidate_version":"2", "update":"available"
        })).unwrap());
        assert!(!schedule.complete(&report).notify);
    }
    #[test]
    fn autostart_is_reversible() {
        let path = std::env::temp_dir()
            .join(format!("pkgdeck-autostart-{}", std::process::id()))
            .join("app.desktop");
        set_autostart(&path, true).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("--background"));
        set_autostart(&path, false).unwrap();
        assert!(!path.exists());
        set_autostart(&path, false).unwrap();
        set_autostart_for(&path, true, Runtime::Snap, None).unwrap();
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("\nExec=snap run pkgdeck --background\n"));
        let blocked = path.join("app.desktop");
        assert!(set_autostart(&blocked, true).is_err());
        let agent = path.with_file_name("io.github.astrovm.PkgDeck.plist");
        set_autostart(&agent, true).unwrap();
        let plist = fs::read_to_string(&agent).unwrap();
        assert!(plist.contains("<string>--background</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n    <true/>"));
        // A file where the folder should be blocks the write.
        assert!(set_autostart(&agent.join("blocked.plist"), true).is_err());
        set_autostart(&agent, false).unwrap();
        assert!(!agent.exists());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn launch_agent_opens_the_bundle_in_the_background() {
        let bundled = launch_agent(Path::new(
            "/Applications/Pkg & <Deck>.app/Contents/MacOS/pkgdeck",
        ))
        .unwrap();
        assert!(bundled.contains(
            "        <string>/usr/bin/open</string>\n        <string>-g</string>\n        <string>-j</string>\n        <string>-a</string>\n        <string>/Applications/Pkg &amp; &lt;Deck&gt;.app</string>\n        <string>--args</string>\n        <string>--background</string>\n"
        ));
        let loose = launch_agent(Path::new("/opt/pkgdeck/bin/pkgdeck")).unwrap();
        assert!(loose.contains(
            "        <string>/opt/pkgdeck/bin/pkgdeck</string>\n        <string>--background</string>\n"
        ));
        assert!(launch_agent(Path::new("relative/pkgdeck")).is_err());
        assert!(launch_agent(Path::new("/tmp/app\nstart")).is_err());
    }

    #[test]
    fn autostart_uses_a_launchable_package_command_and_escapes_paths() {
        let native = autostart_exec(
            Runtime::Native,
            Path::new("/opt/Pkg Deck/app%$\"`\\bin"),
            None,
        )
        .unwrap();
        assert_eq!(
            native,
            "\"/opt/Pkg Deck/app%%\\\\$\\\\\"\\\\`\\\\\\\\bin\" --background"
        );
        assert_eq!(
            autostart_exec(
                Runtime::AppImage,
                Path::new("/tmp/extracted/AppRun"),
                Some(Path::new("/home/user/Pkg Deck.AppImage"))
            )
            .unwrap(),
            "\"/home/user/Pkg Deck.AppImage\" --background"
        );
        assert_eq!(
            autostart_exec(Runtime::Flatpak, Path::new(""), None).unwrap(),
            "flatpak run io.github.astrovm.PkgDeck --background"
        );
        assert_eq!(
            autostart_exec(Runtime::Snap, Path::new(""), None).unwrap(),
            "snap run pkgdeck --background"
        );
        assert!(autostart_exec(Runtime::Native, Path::new("relative/app"), None).is_err());
        assert!(autostart_exec(Runtime::Native, Path::new("/tmp/app\nstart"), None).is_err());
    }

    #[test]
    fn autostart_path_uses_absolute_user_config_and_falls_back_to_home() {
        let suffix = Path::new("autostart/io.github.astrovm.PkgDeck.desktop");
        let agent = Path::new("Library/LaunchAgents/io.github.astrovm.PkgDeck.plist");
        let expected = if cfg!(target_os = "macos") {
            agent
        } else {
            suffix
        };
        assert!(
            autostart_path().is_some_and(|path| path.ends_with(expected))
                || !cfg!(any(target_os = "linux", target_os = "macos"))
        );
        assert_eq!(
            launch_agent_path_from(Some("/Users/fixture".into())),
            Some(PathBuf::from(
                "/Users/fixture/Library/LaunchAgents/io.github.astrovm.PkgDeck.plist"
            ))
        );
        assert_eq!(launch_agent_path_from(Some("relative".into())), None);
        assert_eq!(
            autostart_path_from(
                Some("/home/fixture/.config-alt".into()),
                Some("/home/fixture".into())
            ),
            Some(Path::new("/home/fixture/.config-alt").join(suffix))
        );
        assert_eq!(
            autostart_path_from(Some("relative/config".into()), Some("/home/fixture".into())),
            Some(Path::new("/home/fixture/.config").join(suffix))
        );
        assert_eq!(
            autostart_path_from(None, Some("relative/home".into())),
            None
        );
    }
}
