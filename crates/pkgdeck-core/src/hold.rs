//! When automatic updates wait: on battery power, where they would drain
//! the battery, and while something is being built, where they could swap a
//! compiler or tool out from under the build.
use crate::{
    host::Host,
    process::{Cancellation, Limits},
};
use std::{ffi::OsString, path::Path, time::Duration};

/// Why automatic updates are waiting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hold {
    /// The computer runs on its battery.
    Battery,
    /// A compiler or build tool is running.
    Build,
}
impl Hold {
    /// The name the frontend shows a reason for.
    pub fn key(self) -> &'static str {
        match self {
            Self::Battery => "battery",
            Self::Build => "build",
        }
    }
}

/// Why automatic updates should wait right now, if they should.
pub fn current(host: &Host) -> Option<Hold> {
    first(on_battery(host), || building(host))
}

/// The battery comes first, as the cheaper check.
fn first(battery: bool, building: impl FnOnce() -> bool) -> Option<Hold> {
    if battery {
        return Some(Hold::Battery);
    }
    building().then_some(Hold::Build)
}

#[cfg(target_os = "macos")]
fn on_battery(host: &Host) -> bool {
    pmset_battery(host, Path::new("/usr/bin/pmset"))
}
#[cfg(any(target_os = "macos", test))]
fn pmset_battery(host: &Host, pmset: &Path) -> bool {
    read(host, pmset, &["-g", "batt"]).is_some_and(|status| drawing_from_battery(&status))
}
#[cfg(target_os = "linux")]
fn on_battery(_: &Host) -> bool {
    // The kernel's own view, which the Flatpak sandbox shares.
    discharging(Path::new("/sys/class/power_supply"))
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn on_battery(_: &Host) -> bool {
    false
}

/// Whether `pmset -g batt` says the Mac runs on its battery.
#[cfg(any(target_os = "macos", test))]
fn drawing_from_battery(status: &str) -> bool {
    status
        .lines()
        .next()
        .is_some_and(|line| line.contains("'Battery Power'"))
}

/// Whether one of the computer's own batteries is discharging. A battery
/// that is full or held at a charge limit reports something else, and the
/// batteries of a mouse or headset don't count.
#[cfg(any(target_os = "linux", test))]
fn discharging(supplies: &Path) -> bool {
    let read = |path: &Path| std::fs::read_to_string(path).unwrap_or_default();
    std::fs::read_dir(supplies)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .any(|supply| {
            read(&supply.join("type")).trim() == "Battery"
                && read(&supply.join("scope")).trim() != "Device"
                && read(&supply.join("status")).trim() == "Discharging"
        })
}

/// Whether something is being built right now.
pub(crate) fn building(host: &Host) -> bool {
    host.resolve("ps")
        .ok()
        .flatten()
        .is_some_and(|ps| listed_builds(host, &ps))
}
fn listed_builds(host: &Host, ps: &Path) -> bool {
    read(host, ps, &["-A", "-o", "args="]).is_some_and(|processes| builds_in(&processes))
}

fn read(host: &Host, program: &Path, args: &[&str]) -> Option<String> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let limits = Limits {
        timeout: Duration::from_secs(10),
        output_bytes: 4 * 1024 * 1024,
    };
    let done = host
        .read(program, &args, limits, &Cancellation::default())
        .ok()?;
    (done.code == Some(0)).then(|| String::from_utf8_lossy(&done.stdout).into_owned())
}

/// Programs that only run while something is being built.
const BUILDERS: &[&str] = &[
    "cargo",
    "rustc",
    "clippy-driver",
    "cc",
    "c++",
    "gcc",
    "g++",
    "clang",
    "clang++",
    "cc1",
    "cc1plus",
    "cc1obj",
    "collect2",
    "ld",
    "ld64",
    "ld.lld",
    "ld.gold",
    "lld",
    "mold",
    "make",
    "gmake",
    "ninja",
    "cmake",
    "xcodebuild",
    "swift-build",
    "swift-frontend",
    "swiftc",
    "javac",
    "kotlinc",
    "bazel",
    "bazelisk",
];
/// Tools that build only with some subcommands.
const BUILD_COMMANDS: &[(&str, &[&str])] = &[
    ("go", &["build", "install", "test", "vet"]),
    ("dotnet", &["build", "pack", "publish", "test"]),
];
/// The main classes of Java build clients, which run for as long as a
/// build does. Their daemons stay running between builds, so they don't
/// count.
const BUILD_CLASSES: &[&str] = &[
    "org.gradle.wrapper.GradleWrapperMain",
    "org.gradle.launcher.GradleMain",
    "org.codehaus.plexus.classworlds.launcher.Launcher",
];

/// Whether a process list, one command line per line, has a build in it.
fn builds_in(processes: &str) -> bool {
    processes.lines().any(|line| {
        let mut words = line.split_whitespace();
        let program = words.next().unwrap_or_default();
        let name = program.rsplit('/').next().unwrap_or_default();
        let command = words.clone().next().unwrap_or_default();
        BUILDERS.contains(&name)
            || BUILD_COMMANDS
                .iter()
                .any(|(tool, commands)| *tool == name && commands.contains(&command))
            || words.any(|word| BUILD_CLASSES.contains(&word))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Runtime;
    use std::{collections::BTreeMap, fs};

    #[test]
    fn builds_are_compilers_build_tools_and_java_build_clients() {
        for line in [
            "/Users/me/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo build --release",
            "rustc --crate-name pkgdeck --edition=2021 src/lib.rs",
            "/usr/lib/gcc/x86_64-linux-gnu/13/cc1 -quiet main.c",
            "/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild -scheme App",
            "ninja -C build",
            "go test ./...",
            "/usr/local/go/bin/go build -o app .",
            "dotnet build App.sln",
            "/usr/bin/java -Xmx64m -classpath gradle-wrapper.jar org.gradle.wrapper.GradleWrapperMain assembleDebug",
        ] {
            assert!(builds_in(&format!("/sbin/launchd\n{line}\n")), "{line}");
        }
        for line in [
            "",
            "[kworker/u8:2-events]",
            "/Applications/Android Studio.app/Contents/MacOS/studio",
            // A dev server or an app, not a build.
            "go run ./cmd/server",
            "dotnet App.dll",
            // Build daemons wait between builds.
            "/usr/bin/java -Xmx2g -cp gradle-daemon.jar org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.10",
            "/Users/me/.cargo/bin/sccache",
            "/usr/bin/makemkv",
            // Other tools with a build or test command.
            "npm test",
            "docker build .",
        ] {
            assert!(!builds_in(line), "{line}");
        }
    }

    #[test]
    fn the_battery_is_the_first_reason_to_wait() {
        assert_eq!(first(true, || unreachable!()), Some(Hold::Battery));
        assert_eq!(first(false, || true), Some(Hold::Build));
        assert_eq!(first(false, || false), None);
    }

    #[test]
    fn pmset_says_when_the_mac_runs_on_battery() {
        assert!(drawing_from_battery(
            "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t64%; discharging; 3:10 remaining present: true\n"
        ));
        assert!(!drawing_from_battery(
            "Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t80%; AC attached; not charging present: true\n"
        ));
        // A Mac without a battery, or pmset saying nothing.
        assert!(!drawing_from_battery("Now drawing from 'AC Power'\n"));
        assert!(!drawing_from_battery(""));
    }

    #[test]
    fn only_a_discharging_system_battery_means_battery_power() {
        let root = std::env::temp_dir().join(format!("pkgdeck-power-{}", std::process::id()));
        let supply = |name: &str, files: &[(&str, &str)]| {
            let dir = root.join(name);
            fs::create_dir_all(&dir).unwrap();
            for (file, text) in files {
                fs::write(dir.join(file), format!("{text}\n")).unwrap();
            }
        };
        // A desktop: no supplies at all, or only mains power.
        assert!(!discharging(&root.join("missing")));
        supply("AC", &[("type", "Mains"), ("online", "1")]);
        assert!(!discharging(&root));
        // A wireless mouse running down doesn't put the computer on battery.
        supply(
            "hidpp_battery_0",
            &[
                ("type", "Battery"),
                ("scope", "Device"),
                ("status", "Discharging"),
            ],
        );
        assert!(!discharging(&root));
        // Plugged in, full or held at a charge limit.
        supply("BAT0", &[("type", "Battery"), ("status", "Not charging")]);
        assert!(!discharging(&root));
        supply("BAT0", &[("type", "Battery"), ("status", "Discharging")]);
        assert!(discharging(&root));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_running_system_answers_without_failing() {
        // Whatever this machine is doing, asking never fails.
        let _ = current(&Host::current());
        assert_eq!(Hold::Battery.key(), "battery");
        assert_eq!(Hold::Build.key(), "build");
        // Without ps, nothing counts as a build.
        let bare = Host::new(
            Runtime::Native,
            BTreeMap::from([("PATH".into(), "/nonexistent/pkgdeck".into())]),
        );
        assert!(!building(&bare));
        assert_eq!(read(&bare, Path::new("/nonexistent/pkgdeck/ps"), &[]), None);
    }

    /// A shell script in a fresh folder, run as a stand-in for a tool.
    fn tool(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn power_and_builds_are_read_from_what_the_tools_print() {
        let dir = std::env::temp_dir().join(format!("pkgdeck-hold-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let host = Host::current();
        let battery = tool(&dir, "battery", "echo \"Now drawing from 'Battery Power'\"");
        let plugged = tool(&dir, "plugged", "echo \"Now drawing from 'AC Power'\"");
        let broken = tool(
            &dir,
            "broken",
            "echo \"Now drawing from 'Battery Power'\"; exit 1",
        );
        assert!(pmset_battery(&host, &battery));
        assert!(!pmset_battery(&host, &plugged));
        // A tool that fails says nothing, whatever it printed.
        assert!(!pmset_battery(&host, &broken));
        assert_eq!(read(&host, &broken, &[]), None);
        let build = tool(&dir, "build", "echo /sbin/launchd; echo cargo build");
        let idle = tool(&dir, "idle", "echo /sbin/launchd");
        assert!(listed_builds(&host, &build));
        assert!(!listed_builds(&host, &idle));
        // A long process list is read whole.
        let long = tool(&dir, "long", "head -c 3000000 /dev/zero | tr '\\0' a");
        assert_eq!(
            read(&host, &long, &[]).map(|text| text.len()),
            Some(3_000_000)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_running_compiler_holds_updates() {
        // A program named like a compiler, as ps lists it.
        let dir = std::env::temp_dir().join(format!("pkgdeck-rustc-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let rustc = dir.join("rustc");
        let _ = fs::remove_file(&rustc);
        std::os::unix::fs::symlink("/bin/sleep", &rustc).unwrap();
        let mut child = std::process::Command::new(&rustc)
            .arg("30")
            .spawn()
            .unwrap();
        let host = Host::current();
        let held = (building(&host), current(&host));
        child.kill().unwrap();
        child.wait().unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert!(held.0);
        assert!(held.1.is_some());
    }
}
