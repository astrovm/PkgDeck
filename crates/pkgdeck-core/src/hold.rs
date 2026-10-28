//! When automatic updates wait: on battery power, where they would drain
//! the battery, on a metered connection, where they would use up a data
//! plan, and while something is being built, where they could swap a
//! compiler or tool out from under the build.
use crate::{
    host::Host,
    process::{Cancellation, Limits},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::Path,
    time::Duration,
};

/// Why automatic updates are waiting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hold {
    /// The computer runs on its battery.
    Battery,
    /// The network connection is metered, like a phone's hotspot.
    Metered,
    /// A compiler or build tool is running.
    Build,
}
impl Hold {
    /// The name the frontend shows a reason for.
    pub fn key(self) -> &'static str {
        match self {
            Self::Battery => "battery",
            Self::Metered => "metered",
            Self::Build => "build",
        }
    }
}

/// Which reasons count. Builds always do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Waits {
    pub battery: bool,
    pub metered: bool,
}
impl Default for Waits {
    fn default() -> Self {
        Self {
            battery: true,
            metered: true,
        }
    }
}

/// Why automatic updates should wait right now, if they should, of the
/// reasons `waits` counts.
pub fn current(host: &Host, waits: Waits) -> Option<Hold> {
    first([
        (Hold::Battery, &|| waits.battery && on_battery(host)),
        (Hold::Metered, &|| waits.metered && metered(host)),
        (Hold::Build, &|| building(host)),
    ])
}

/// The first reason that applies, cheapest checks first.
fn first(checks: [(Hold, &dyn Fn() -> bool); 3]) -> Option<Hold> {
    checks
        .into_iter()
        .find(|(_, applies)| applies())
        .map(|(hold, _)| hold)
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

/// Whether NetworkManager says the connection is metered, or guesses it
/// is. The host's busctl asks it, since the Flatpak sandbox can't reach the
/// system bus.
#[cfg(target_os = "linux")]
fn metered(host: &Host) -> bool {
    host.resolve("busctl")
        .ok()
        .flatten()
        .is_some_and(|busctl| busctl_metered(host, &busctl))
}
#[cfg(not(target_os = "linux"))]
fn metered(_: &Host) -> bool {
    false
}
#[cfg(any(target_os = "linux", test))]
fn busctl_metered(host: &Host, busctl: &Path) -> bool {
    read(
        host,
        busctl,
        &[
            "--system",
            "get-property",
            "org.freedesktop.NetworkManager",
            "/org/freedesktop/NetworkManager",
            "org.freedesktop.NetworkManager",
            "Metered",
        ],
    )
    .is_some_and(|reply| metered_reply(&reply))
}

/// NetworkManager's metered values: 1 is yes and 3 a guess of yes. 0 is
/// unknown, 2 no and 4 a guess of no.
#[cfg(any(target_os = "linux", test))]
fn metered_reply(reply: &str) -> bool {
    matches!(reply.trim(), "u 1" | "u 3")
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

/// What a running build builds with, which decides the caches and projects
/// it may be reading or writing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Toolchain {
    Rust,
    Gradle,
    Maven,
    Go,
    Dotnet,
    /// Xcode and Swift.
    Apple,
    /// Compilers, linkers and build tools no other build started, which
    /// could be building anything.
    Native,
}

/// Whether automatic updates from `backend` wait while something is being
/// built. Apps, container images and coding assistants can't change what a
/// build runs; every other source can swap a compiler, an SDK or a library
/// out from under it.
pub fn waits_for_builds(backend: &str) -> bool {
    !matches!(
        backend,
        "flatpak"
            | "appimage"
            | "docker"
            | "podman"
            | "oh-my-zsh"
            | "codex"
            | "claude"
            | "grok"
            | "opencode"
            | "cursor"
            | "copilot"
            | "kiro"
            | "antigravity"
            | "amp"
            | "droid"
    )
}

/// Whether something is being built right now.
pub(crate) fn building(host: &Host) -> bool {
    !builds(host).is_empty()
}
/// The toolchains of everything being built right now.
pub(crate) fn builds(host: &Host) -> BTreeSet<Toolchain> {
    host.resolve("ps")
        .ok()
        .flatten()
        .map_or_else(BTreeSet::new, |ps| listed_builds(host, &ps))
}
fn listed_builds(host: &Host, ps: &Path) -> BTreeSet<Toolchain> {
    read(host, ps, &["-A", "-o", "pid=,ppid=,args="])
        .map_or_else(BTreeSet::new, |processes| builds_in(&processes))
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

/// Programs that only run while something is being built, with their
/// toolchain when they have one of their own.
const TOOLCHAIN_BUILDERS: &[(&str, Toolchain)] = &[
    ("cargo", Toolchain::Rust),
    ("rustc", Toolchain::Rust),
    ("clippy-driver", Toolchain::Rust),
    ("xcodebuild", Toolchain::Apple),
    ("swift-build", Toolchain::Apple),
    ("swift-frontend", Toolchain::Apple),
    ("swiftc", Toolchain::Apple),
];
/// Programs that only run while something is being built, for whatever
/// started them. Started by nothing that names a toolchain, they count as
/// [`Toolchain::Native`].
const BUILDERS: &[&str] = &[
    "cc", "c++", "gcc", "g++", "clang", "clang++", "cc1", "cc1plus", "cc1obj", "collect2", "ld",
    "ld64", "ld.lld", "ld.gold", "lld", "mold", "make", "gmake", "ninja", "cmake", "bazel",
    "bazelisk",
];
/// Java compilers, which Gradle and Maven builds both run.
const JVM_BUILDERS: &[&str] = &["javac", "kotlinc"];
/// Tools that build only with some subcommands. Their other commands still
/// name the toolchain of what they start, like `go run` compiling with cgo.
const BUILD_COMMANDS: &[(&str, Toolchain, &[&str])] = &[
    ("go", Toolchain::Go, &["build", "install", "test", "vet"]),
    (
        "dotnet",
        Toolchain::Dotnet,
        &["build", "pack", "publish", "test"],
    ),
];
/// The main classes of Java build clients, which run for as long as a
/// build does.
const BUILD_CLASSES: &[(&str, Toolchain)] = &[
    ("org.gradle.wrapper.GradleWrapperMain", Toolchain::Gradle),
    ("org.gradle.launcher.GradleMain", Toolchain::Gradle),
    (
        "org.codehaus.plexus.classworlds.launcher.Launcher",
        Toolchain::Maven,
    ),
];
/// Build daemons wait between builds, so they aren't builds themselves.
/// What they start is part of their toolchain's build.
const DAEMON_CLASSES: &[(&str, Toolchain)] = &[(
    "org.gradle.launcher.daemon.bootstrap.GradleDaemon",
    Toolchain::Gradle,
)];

/// What one command line says: the toolchain it names, if any, and
/// whether it builds, as `Some` of the toolchains it counts as when
/// nothing that started it names one.
fn read_process(args: &str) -> (Option<Toolchain>, Option<&'static [Toolchain]>) {
    const NATIVE: &[Toolchain] = &[Toolchain::Native];
    const JVM: &[Toolchain] = &[Toolchain::Gradle, Toolchain::Maven];
    let mut words = args.split_whitespace();
    let program = words.next().unwrap_or_default();
    let name = program.rsplit('/').next().unwrap_or_default();
    let command = words.clone().next().unwrap_or_default();
    if let Some((_, toolchain)) = TOOLCHAIN_BUILDERS.iter().find(|(tool, _)| *tool == name) {
        return (Some(*toolchain), Some(NATIVE));
    }
    if let Some((_, toolchain, commands)) = BUILD_COMMANDS.iter().find(|(tool, ..)| *tool == name) {
        return (
            Some(*toolchain),
            commands.contains(&command).then_some(NATIVE),
        );
    }
    if BUILDERS.contains(&name) {
        return (None, Some(NATIVE));
    }
    if JVM_BUILDERS.contains(&name) {
        return (None, Some(JVM));
    }
    let words: Vec<&str> = words.collect();
    let class = |classes: &[(&str, Toolchain)]| {
        classes
            .iter()
            .find(|(class, _)| words.contains(class))
            .map(|(_, toolchain)| *toolchain)
    };
    match (class(BUILD_CLASSES), class(DAEMON_CLASSES)) {
        (Some(toolchain), _) => (Some(toolchain), Some(NATIVE)),
        (None, daemon) => (daemon, None),
    }
}

/// The toolchains a process list builds with. Each line is a process ID,
/// its parent's, and its command line. A build tool counts for the nearest
/// process up its tree that names a toolchain, itself included: the linker
/// `rustc` starts is part of a Rust build.
fn builds_in(processes: &str) -> BTreeSet<Toolchain> {
    let mut parents = BTreeMap::new();
    let mut named = BTreeMap::new();
    let mut building = vec![];
    for line in processes.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (fields.next(), fields.next()) else {
            continue;
        };
        let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) else {
            continue;
        };
        let args = fields.collect::<Vec<_>>().join(" ");
        let (toolchain, builds) = read_process(&args);
        parents.insert(pid, ppid);
        if let Some(toolchain) = toolchain {
            named.insert(pid, toolchain);
        }
        if let Some(otherwise) = builds {
            building.push((pid, otherwise));
        }
    }
    let mut toolchains = BTreeSet::new();
    for (pid, otherwise) in building {
        // A list taken while processes come and go can loop; a tree this
        // deep is one.
        let named = std::iter::successors(Some(pid), |pid| parents.get(pid).copied())
            .take(64)
            .find_map(|pid| named.get(&pid).copied());
        match named {
            Some(toolchain) => {
                toolchains.insert(toolchain);
            }
            None => toolchains.extend(otherwise),
        }
    }
    toolchains
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Runtime;
    use std::fs;
    use Toolchain::*;

    /// Whether one command line, started by `launchd`, is a build.
    fn builds_alone(line: &str) -> BTreeSet<Toolchain> {
        builds_in(&format!("1 0 /sbin/launchd\n20 1 {line}\n"))
    }

    #[test]
    fn builds_are_compilers_build_tools_and_java_build_clients() {
        for (line, toolchain) in [
            (
                "/Users/me/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo build --release",
                Rust,
            ),
            ("rustc --crate-name pkgdeck --edition=2021 src/lib.rs", Rust),
            ("/usr/lib/gcc/x86_64-linux-gnu/13/cc1 -quiet main.c", Native),
            (
                "/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild -scheme App",
                Apple,
            ),
            ("ninja -C build", Native),
            ("go test ./...", Go),
            ("/usr/local/go/bin/go build -o app .", Go),
            ("dotnet build App.sln", Dotnet),
            (
                "/usr/bin/java -Xmx64m -classpath gradle-wrapper.jar org.gradle.wrapper.GradleWrapperMain assembleDebug",
                Gradle,
            ),
            (
                "/usr/bin/java -classpath plexus-classworlds.jar org.codehaus.plexus.classworlds.launcher.Launcher package",
                Maven,
            ),
        ] {
            assert_eq!(builds_alone(line), BTreeSet::from([toolchain]), "{line}");
        }
        // A Java compiler nothing else explains could be either JVM build's.
        assert_eq!(
            builds_alone("javac Main.java"),
            BTreeSet::from([Gradle, Maven])
        );
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
            assert_eq!(builds_alone(line), BTreeSet::new(), "{line}");
        }
    }

    #[test]
    fn a_build_tool_belongs_to_the_build_that_started_it() {
        let tree = |lines: &[&str]| builds_in(&lines.join("\n"));
        // rustc links with cc, a build script runs make: all one Rust build.
        assert_eq!(
            tree(&[
                "1 0 /sbin/init",
                "10 1 bash",
                "11 10 cargo build",
                "12 11 rustc --crate-name app",
                "13 12 cc -o app main.o",
                "14 11 target/debug/build/zstd-sys/build-script-build",
                "15 14 make -j4",
            ]),
            BTreeSet::from([Rust])
        );
        // What a Gradle daemon compiles is Gradle's, though the daemon
        // between builds is no build at all.
        assert_eq!(
            tree(&[
                "1 0 /sbin/init",
                "30 1 java org.gradle.launcher.daemon.bootstrap.GradleDaemon 9.8",
                "31 30 cmake --build app/.cxx",
                "32 30 javac Main.java",
            ]),
            BTreeSet::from([Gradle])
        );
        assert_eq!(
            tree(&[
                "1 0 /sbin/init",
                "30 1 java org.gradle.launcher.daemon.bootstrap.GradleDaemon 9.8"
            ]),
            BTreeSet::new()
        );
        // `go run` doesn't build, but the C compiler cgo starts for it does.
        assert_eq!(
            tree(&["1 0 /sbin/init", "40 1 go run .", "41 40 gcc -c x.c"]),
            BTreeSet::from([Go])
        );
        // Two builds at once count both; a compiler alone counts as native.
        assert_eq!(
            tree(&[
                "1 0 /sbin/init",
                "11 1 cargo build",
                "50 1 /usr/bin/make all",
            ]),
            BTreeSet::from([Rust, Native])
        );
        // Lines that aren't processes are skipped, and a loop in a list
        // taken while processes change still ends.
        assert_eq!(
            tree(&["PID PPID COMMAND", "x 1 cargo", "60 61 cc a.c", "61 60 sh"]),
            BTreeSet::from([Native])
        );
        assert_eq!(tree(&["7"]), BTreeSet::new());
    }

    #[test]
    fn the_battery_is_the_first_reason_to_wait() {
        let checks = |battery: bool, metered: bool, build: bool| {
            first([
                (Hold::Battery, &move || battery),
                (Hold::Metered, &move || metered),
                (Hold::Build, &move || build),
            ])
        };
        assert_eq!(checks(true, true, true), Some(Hold::Battery));
        assert_eq!(checks(false, true, true), Some(Hold::Metered));
        assert_eq!(checks(false, false, true), Some(Hold::Build));
        assert_eq!(checks(false, false, false), None);
        // A later check doesn't run once an earlier one applies.
        assert_eq!(
            first([
                (Hold::Battery, &|| true),
                (Hold::Metered, &|| unreachable!()),
                (Hold::Build, &|| unreachable!()),
            ]),
            Some(Hold::Battery)
        );
    }

    #[test]
    fn a_metered_or_likely_metered_connection_holds_updates() {
        assert!(metered_reply("u 1\n"));
        assert!(metered_reply("u 3\n"));
        for reply in ["u 0\n", "u 2\n", "u 4\n", "", "garbage"] {
            assert!(!metered_reply(reply), "{reply}");
        }
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
        let _ = current(&Host::current(), Waits::default());
        assert_eq!(Hold::Battery.key(), "battery");
        assert_eq!(Hold::Metered.key(), "metered");
        assert_eq!(Hold::Build.key(), "build");
        // Without ps or busctl, nothing counts as a build or metered.
        let bare = Host::new(
            Runtime::Native,
            BTreeMap::from([("PATH".into(), "/nonexistent/pkgdeck".into())]),
        );
        assert!(!building(&bare));
        assert!(!metered(&bare));
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
        // busctl gets asked for NetworkManager's property.
        let hotspot = tool(
            &dir,
            "hotspot",
            "[ \"$*\" = '--system get-property org.freedesktop.NetworkManager /org/freedesktop/NetworkManager org.freedesktop.NetworkManager Metered' ] && echo 'u 3'",
        );
        let home = tool(&dir, "home", "echo 'u 4'");
        assert!(busctl_metered(&host, &hotspot));
        assert!(!busctl_metered(&host, &home));
        assert!(!busctl_metered(&host, &broken));
        // ps gets asked for each process's parent too.
        let build = tool(
            &dir,
            "build",
            "[ \"$*\" = '-A -o pid=,ppid=,args=' ] && echo '1 0 /sbin/launchd' && echo '9 1 cargo build'",
        );
        let idle = tool(&dir, "idle", "echo '1 0 /sbin/launchd'");
        assert_eq!(listed_builds(&host, &build), BTreeSet::from([Rust]));
        assert_eq!(listed_builds(&host, &idle), BTreeSet::new());
        assert_eq!(listed_builds(&host, &broken), BTreeSet::new());
        // A long process list is read whole.
        let long = tool(&dir, "long", "head -c 3000000 /dev/zero | tr '\\0' a");
        assert_eq!(
            read(&host, &long, &[]).map(|text| text.len()),
            Some(3_000_000)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_sources_a_build_may_run_wait_for_builds() {
        for backend in ["apt", "snap", "brew", "cargo", "npm", "rustup", "fwupd"] {
            assert!(waits_for_builds(backend), "{backend}");
        }
        for backend in [
            "flatpak",
            "appimage",
            "podman",
            "docker",
            "oh-my-zsh",
            "codex",
            "claude",
        ] {
            assert!(!waits_for_builds(backend), "{backend}");
        }
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
        let nothing_else = Waits {
            battery: false,
            metered: false,
        };
        let held = (building(&host), current(&host, nothing_else));
        child.kill().unwrap();
        child.wait().unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert!(held.0);
        // Builds count whatever else is turned off.
        assert_eq!(held.1, Some(Hold::Build));
    }
}
