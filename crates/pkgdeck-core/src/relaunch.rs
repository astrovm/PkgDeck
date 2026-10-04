//! Restarting PkgDeck after it updated itself.
//!
//! At startup [`Install::current`] notes what is running: the program file,
//! the Mac app bundle, or the Flatpak or Snap revision. After a change,
//! [`Install::updated`] tells whether a new copy took its place, and
//! [`Install::relaunch`] starts that copy once this process has exited
//! (PkgDeck runs one copy at a time, so the new one must wait).
use crate::host::Runtime;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Waits for the process named by `$0` to exit, then runs the arguments.
const WAIT_FOR_PID: &str = "while kill -0 \"$0\" 2>/dev/null; do sleep 0.2; done; exec \"$@\"";
/// The Flatpak sandbox's processes aren't visible on the host, so the host
/// waits until no PkgDeck instance is left.
const WAIT_FOR_FLATPAK: &str = "while flatpak ps --columns=application 2>/dev/null | grep -qx \"$0\"; do sleep 0.3; done; exec flatpak run \"$0\" \"$@\"";

/// A file as found on disk: replacing it changes the inode, and a Homebrew
/// keg upgrade moves where its link points.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Stamp {
    canonical: PathBuf,
    device: u64,
    inode: u64,
    modified: i64,
}
fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(Stamp {
        canonical: path.canonicalize().ok()?,
        device: metadata.dev(),
        inode: metadata.ino(),
        modified: metadata.mtime(),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Kind {
    /// A Mac app bundle, started again through `open`.
    Bundle { bundle: PathBuf, program: PathBuf },
    /// A program at a path the new copy appears at.
    Program { path: PathBuf },
    /// `active` is the host's link to the deployed commit.
    Flatpak { active: PathBuf, commit: String },
    /// `current` is the link to the installed revision.
    Snap { current: PathBuf, revision: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Install {
    kind: Kind,
    stamp: Option<Stamp>,
}

impl Install {
    /// The running copy, or `None` when PkgDeck can't tell where it lives.
    pub fn current() -> Option<Self> {
        let env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        let flatpak_info = std::fs::read_to_string("/.flatpak-info").ok();
        Self::detect(
            &env,
            flatpak_info.as_deref(),
            &std::env::current_exe().ok()?,
        )
    }
    /// The copy described by an environment, the sandbox's `/.flatpak-info`
    /// when there is one, and the running program's path.
    pub fn detect(
        env: &BTreeMap<OsString, OsString>,
        flatpak_info: Option<&str>,
        executable: &Path,
    ) -> Option<Self> {
        let var = |name: &str| env.get(&OsString::from(name)).cloned();
        let kind = match Runtime::detect_for(env, flatpak_info.is_some(), Some(executable)) {
            Runtime::Flatpak => {
                // app-path=/var/lib/flatpak/app/ID/ARCH/BRANCH/COMMIT/files
                let path = flatpak_info?
                    .lines()
                    .find_map(|line| line.strip_prefix("app-path="))?;
                let commit_dir = Path::new(path.trim()).parent()?;
                Kind::Flatpak {
                    active: commit_dir.parent()?.join("active"),
                    commit: commit_dir.file_name()?.to_str()?.into(),
                }
            }
            Runtime::Snap => Kind::Snap {
                current: Path::new("/snap").join(var("SNAP_NAME")?).join("current"),
                revision: var("SNAP_REVISION")?.into_string().ok()?,
            },
            Runtime::AppImage => Kind::Program {
                path: PathBuf::from(var("APPIMAGE")?),
            },
            Runtime::Native => native(executable),
        };
        let stamp = match &kind {
            Kind::Bundle { program, .. } => stamp(program),
            Kind::Program { path } => stamp(path),
            _ => None,
        };
        Some(Self { kind, stamp })
    }

    /// A program at `path`, as it is now.
    pub fn program(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self {
            stamp: stamp(&path),
            kind: Kind::Program { path },
        }
    }

    /// Whether a new copy replaced the running one.
    pub fn updated(&self) -> bool {
        self.updated_with(|path| host_link(Path::new("flatpak-spawn"), path))
    }
    fn updated_with(&self, host_link: impl Fn(&Path) -> Option<PathBuf>) -> bool {
        match &self.kind {
            Kind::Bundle { program: path, .. } | Kind::Program { path } => {
                let now = stamp(path);
                now.is_some() && now != self.stamp
            }
            Kind::Flatpak { active, commit } => host_link(active).is_some_and(|target| {
                target
                    .file_name()
                    .is_some_and(|name| name != commit.as_str())
            }),
            Kind::Snap { current, revision } => std::fs::read_link(current)
                .is_ok_and(|target| target.as_os_str() != revision.as_str()),
        }
    }

    /// Start the new copy once this process exits, in the background (to
    /// the tray or menu bar) when `background`.
    pub fn relaunch(&self, background: bool) -> io::Result<()> {
        let (program, args) = self.waiter(std::process::id(), background);
        spawn_detached(&program, &args)
    }
    /// The command that waits for process `pid` and starts the new copy.
    fn waiter(&self, pid: u32, background: bool) -> (PathBuf, Vec<OsString>) {
        let flag = background.then(|| OsString::from("--background"));
        let shell = |launch: Vec<OsString>| {
            let mut args: Vec<OsString> =
                vec!["-c".into(), WAIT_FOR_PID.into(), pid.to_string().into()];
            args.extend(launch);
            args.extend(flag.clone());
            (PathBuf::from("/bin/sh"), args)
        };
        match &self.kind {
            Kind::Bundle { bundle, .. } => {
                let mut open: Vec<OsString> = vec!["/usr/bin/open".into()];
                if background {
                    // Neither brought to the front nor shown.
                    open.extend(["-g".into(), "-j".into()]);
                }
                open.extend(["-a".into(), bundle.into()]);
                if background {
                    open.push("--args".into());
                }
                shell(open)
            }
            Kind::Program { path } => shell(vec![path.into()]),
            Kind::Snap { current, .. } => {
                let name = current
                    .parent()
                    .and_then(Path::file_name)
                    .unwrap_or_default();
                shell(vec![Path::new("/snap/bin").join(name).into()])
            }
            Kind::Flatpak { .. } => {
                let mut args: Vec<OsString> = vec![
                    "--host".into(),
                    "setsid".into(),
                    "/bin/sh".into(),
                    "-c".into(),
                    WAIT_FOR_FLATPAK.into(),
                    crate::APP_ID.into(),
                ];
                args.extend(flag);
                (PathBuf::from("flatpak-spawn"), args)
            }
        }
    }
}

/// A Mac app runs from `NAME.app/Contents/MacOS`. A Homebrew formula runs
/// from its versioned keg, so the new copy appears behind the `opt` link.
fn native(executable: &Path) -> Kind {
    let real = executable
        .canonicalize()
        .unwrap_or_else(|_| executable.to_path_buf());
    if let Some(bundle) = real
        .parent()
        .filter(|dir| dir.ends_with("Contents/MacOS"))
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|bundle| bundle.extension().is_some_and(|ext| ext == "app"))
    {
        return Kind::Bundle {
            bundle: bundle.into(),
            program: real.clone(),
        };
    }
    let parts: Vec<_> = real.components().collect();
    if let Some(cellar) = parts.iter().rposition(|part| part.as_os_str() == "Cellar") {
        if let [name, _version, rest @ ..] = &parts[cellar + 1..] {
            if !rest.is_empty() {
                let prefix: PathBuf = parts[..cellar].iter().collect();
                let mut path = prefix.join("opt").join(name);
                path.extend(rest);
                return Kind::Program { path };
            }
        }
    }
    Kind::Program { path: real }
}

/// Where a host link points, read through `flatpak-spawn --host`.
fn host_link(spawn: &Path, path: &Path) -> Option<PathBuf> {
    let output = Command::new(spawn)
        .arg("--host")
        .arg("readlink")
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    (output.status.success() && !text.trim().is_empty()).then(|| PathBuf::from(text.trim()))
}

/// Started in its own process group with no terminal, so quitting PkgDeck
/// doesn't take it along.
fn spawn_detached(program: &Path, args: &[OsString]) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<OsString, OsString> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect()
    }
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pkgdeck-relaunch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }
    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_replaced_program_counts_as_updated_and_an_untouched_one_does_not() {
        let dir = scratch("program");
        let program = dir.join("pkgdeck");
        fs::write(&program, "old").unwrap();
        let install = Install::detect(&env(&[]), None, &program).unwrap();
        assert_eq!(Install::program(&program), install);
        assert!(!install.updated());
        // Package managers write the new file and move it into place.
        fs::write(dir.join("new"), "new").unwrap();
        fs::rename(dir.join("new"), &program).unwrap();
        assert!(install.updated());
        // A removed copy can't be started again.
        fs::remove_file(&program).unwrap();
        assert!(!install.updated());
        let (shell, args) = install.waiter(42, false);
        assert_eq!(shell, Path::new("/bin/sh"));
        assert_eq!(
            strings(&args),
            ["-c", WAIT_FOR_PID, "42", program.to_str().unwrap()]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_keg_is_followed_through_its_opt_link() {
        let dir = scratch("keg");
        for version in ["1.0", "1.1"] {
            let bin = dir.join(format!("Cellar/pkgdeck/{version}/bin"));
            fs::create_dir_all(&bin).unwrap();
            fs::write(bin.join("pkgdeck"), version).unwrap();
        }
        fs::create_dir_all(dir.join("opt")).unwrap();
        let link = dir.join("opt/pkgdeck");
        std::os::unix::fs::symlink(dir.join("Cellar/pkgdeck/1.0"), &link).unwrap();
        let running = dir.join("Cellar/pkgdeck/1.0/bin/pkgdeck");
        let install = Install::detect(&env(&[]), None, &running).unwrap();
        assert_eq!(
            install.kind,
            Kind::Program {
                path: dir.join("opt/pkgdeck/bin/pkgdeck")
            }
        );
        assert!(!install.updated());
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(dir.join("Cellar/pkgdeck/1.1"), &link).unwrap();
        assert!(install.updated());
        // A path that only looks like a keg is the program itself.
        assert_eq!(
            native(Path::new("/nowhere/Cellar/pkgdeck/1.0")),
            Kind::Program {
                path: "/nowhere/Cellar/pkgdeck/1.0".into()
            }
        );
        assert_eq!(
            native(Path::new("/nowhere/Cellar/pkgdeck")),
            Kind::Program {
                path: "/nowhere/Cellar/pkgdeck".into()
            }
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_mac_app_reopens_through_open_hidden_when_it_was() {
        let dir = scratch("bundle");
        let macos = dir.join("PkgDeck.app/Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        fs::write(macos.join("pkgdeck"), "old").unwrap();
        let install = Install::detect(&env(&[]), None, &macos.join("pkgdeck")).unwrap();
        let bundle = dir.join("PkgDeck.app");
        let bundle = bundle.to_str().unwrap();
        assert!(!install.updated());
        let (_, args) = install.waiter(7, true);
        assert_eq!(
            strings(&args),
            [
                "-c",
                WAIT_FOR_PID,
                "7",
                "/usr/bin/open",
                "-g",
                "-j",
                "-a",
                bundle,
                "--args",
                "--background"
            ]
        );
        let (_, args) = install.waiter(7, false);
        assert_eq!(strings(&args)[3..], ["/usr/bin/open", "-a", bundle]);
        // The app replaced as a whole is a new program file.
        fs::remove_dir_all(dir.join("PkgDeck.app")).unwrap();
        fs::create_dir_all(&macos).unwrap();
        fs::write(macos.join("pkgdeck"), "new").unwrap();
        assert!(install.updated());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sandboxed_copies_compare_revisions_and_restart_through_their_runtime() {
        let info = "[Application]\nname=io.github.astrovm.PkgDeck\n\n[Instance]\napp-path=/var/lib/flatpak/app/io.github.astrovm.PkgDeck/x86_64/stable/abc123/files\n";
        let flatpak =
            Install::detect(&env(&[]), Some(info), Path::new("/app/bin/pkgdeck")).unwrap();
        assert_eq!(
            flatpak.kind,
            Kind::Flatpak {
                active: "/var/lib/flatpak/app/io.github.astrovm.PkgDeck/x86_64/stable/active"
                    .into(),
                commit: "abc123".into(),
            }
        );
        // Outside a sandbox the host can't be asked, so nothing changed.
        assert!(!flatpak.updated());
        assert!(flatpak.updated_with(|_| Some("def456".into())));
        assert!(!flatpak.updated_with(|_| Some("abc123".into())));
        let (program, args) = flatpak.waiter(1, true);
        assert_eq!(program, Path::new("flatpak-spawn"));
        assert_eq!(
            strings(&args),
            [
                "--host",
                "setsid",
                "/bin/sh",
                "-c",
                WAIT_FOR_FLATPAK,
                crate::APP_ID,
                "--background"
            ]
        );
        assert!(Install::detect(
            &env(&[]),
            Some("[Instance]\n"),
            Path::new("/app/bin/pkgdeck")
        )
        .is_none());

        let snap = Install::detect(
            &env(&[
                ("SNAP", "/snap/pkgdeck/12"),
                ("SNAP_NAME", "pkgdeck"),
                ("SNAP_REVISION", "12"),
            ]),
            None,
            Path::new("/snap/pkgdeck/12/bin/pkgdeck"),
        )
        .unwrap();
        assert!(!snap.updated());
        let (_, args) = snap.waiter(3, false);
        assert_eq!(strings(&args)[3..], ["/snap/bin/pkgdeck"]);
        assert!(Install::detect(&env(&[("SNAP", "/snap/x/1")]), None, Path::new("/x")).is_none());

        let mounted = env(&[
            ("APPIMAGE", "/home/me/PkgDeck.AppImage"),
            ("APPDIR", "/tmp/.mount"),
        ]);
        let appimage =
            Install::detect(&mounted, None, Path::new("/tmp/.mount/usr/bin/pkgdeck")).unwrap();
        assert_eq!(
            appimage.kind,
            Kind::Program {
                path: "/home/me/PkgDeck.AppImage".into()
            }
        );
        // Started from another AppImage, such as a terminal: not that one.
        let inherited = Install::detect(&mounted, None, Path::new("/usr/bin/pkgdeck")).unwrap();
        assert_eq!(inherited.kind, native(Path::new("/usr/bin/pkgdeck")));
    }

    #[test]
    fn host_links_are_read_through_the_spawn_helper() {
        let dir = scratch("host");
        let spawn = dir.join("spawn");
        fs::write(
            &spawn,
            "#!/bin/sh\n[ \"$1 $2\" = '--host readlink' ] && echo \"$3-target\"\n",
        )
        .unwrap();
        fs::set_permissions(&spawn, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert_eq!(
            host_link(&spawn, Path::new("/x/active")),
            Some(PathBuf::from("/x/active-target"))
        );
        fs::write(&spawn, "#!/bin/sh\nexit 1\n").unwrap();
        assert_eq!(host_link(&spawn, Path::new("/x/active")), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_link_to_another_revision_is_an_update() {
        let dir = scratch("snap");
        let current = dir.join("current");
        std::os::unix::fs::symlink("13", &current).unwrap();
        let install = |revision: &str| Install {
            kind: Kind::Snap {
                current: current.clone(),
                revision: revision.into(),
            },
            stamp: None,
        };
        assert!(install("12").updated());
        assert!(!install("13").updated());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_relauncher_outlives_this_process_and_runs_the_program() {
        let dir = scratch("spawn");
        let marker = dir.join("started");
        let program = dir.join("program");
        fs::write(
            &program,
            format!("#!/bin/sh\necho \"$@\" > '{}'\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(
            &program,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        let install = Install::detect(&env(&[]), None, &program).unwrap();
        // Waiting on this test's process, the program starts only after the
        // test run ends; a missing program fails to spawn here.
        install.relaunch(true).unwrap();
        assert!(spawn_detached(&dir.join("missing"), &[]).is_err());
        spawn_detached(&program, &["now".into()]).unwrap();
        for _ in 0..200 {
            if fs::read_to_string(&marker).is_ok_and(|text| text == "now\n") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(fs::read_to_string(&marker).unwrap(), "now\n");
        assert!(Install::current().is_some());
    }
}
