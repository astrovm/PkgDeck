//! Policy for opt-in, read-only update checks. The caller supplies a clock and
//! network state so scheduling can be tested without waiting or networking.
#[cfg(target_os = "linux")]
use crate::host::Host;
use crate::{
    engine::PackageReport,
    host::Runtime,
    package::{PackageId, UpdateAvailability},
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

pub const CHECK_INTERVAL_SECONDS: u64 = 30 * 60;

#[derive(Default)]
pub struct Schedule {
    next_due: u64,
    last_complete_set: Option<Vec<(PackageId, Option<String>)>>,
}
pub struct CheckResult {
    pub count: usize,
    pub failures: usize,
    pub changed: bool,
}
impl Schedule {
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
        let mut updates: Vec<_> = report
            .packages
            .iter()
            .filter(|package| package.update == UpdateAvailability::Available)
            .map(|package| (package.id.clone(), package.candidate_version.clone()))
            .collect();
        updates.sort();
        updates.dedup();
        let changed = report.failures.is_empty()
            && self
                .last_complete_set
                .as_ref()
                .is_some_and(|previous| previous != &updates);
        if report.failures.is_empty() {
            self.last_complete_set = Some(updates.clone());
        }
        CheckResult {
            count: updates.len(),
            failures: report.failures.len(),
            changed,
        }
    }
}

pub fn autostart_path() -> Option<PathBuf> {
    #[cfg(not(target_os = "linux"))]
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
pub fn set_autostart(path: &Path, enabled: bool) -> io::Result<()> {
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
    let environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    let runtime = Runtime::detect(&environment, Path::new("/.flatpak-info").exists());
    let executable = if matches!(runtime, Runtime::Native | Runtime::AppImage) {
        std::env::current_exe()?
    } else {
        PathBuf::new()
    };
    let appimage = environment.get(&OsString::from("APPIMAGE")).map(Path::new);
    let exec = autostart_exec(runtime, &executable, appimage)?;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    file.write_all(format!("[Desktop Entry]\nType=Application\nName=PkgDeck\nExec={exec}\nIcon=io.github.astrovm.PkgDeck\nX-GNOME-Autostart-enabled=true\n").as_bytes())?;
    file.sync_all()
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
    fn changed_complete_set_notifies_once_but_partial_does_not_replace_baseline() {
        let mut schedule = Schedule::default();
        let mut report = PackageReport::default();
        assert!(!schedule.complete(&report).changed);
        assert!(!schedule.complete(&report).changed);
        let package = serde_json::from_value(serde_json::json!({
            "id": {"backend":"apt", "name":"synthetic", "architecture":"amd64", "scope":"system"},
            "display_name":"Synthetic", "summary":"Test", "installed_version":"1", "candidate_version":"2", "update":"available"
        })).unwrap();
        report.packages.push(package);
        assert!(schedule.complete(&report).changed);
        assert!(!schedule.complete(&report).changed);
        // A failed source cannot prove that the update set shrank.
        report.packages.clear();
        report.failures.push(crate::engine::BackendFailure {
            backend: "fixture".into(),
            error: crate::engine::EngineError::Cancelled,
        });
        assert!(!schedule.complete(&report).changed);
        report.failures.clear();
        assert!(schedule.complete(&report).changed);
        assert!(!schedule.complete(&report).changed);
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
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
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
        if cfg!(target_os = "linux") {
            assert!(autostart_path().unwrap().ends_with(suffix));
        } else {
            assert!(autostart_path().is_none());
        }
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
