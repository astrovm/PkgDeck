//! Unattended updates: which sources may update with nobody watching, and
//! the saved approval that lets system managers do so without a password.
//!
//! The approval is opt-in and narrow. On Linux it is a polkit rule that lets
//! one user, in an active local session, start PkgDeck's helper with
//! [`UPGRADE_ONLY`] and nothing else; that helper only refreshes sources and
//! updates every package (see [`crate::batch::check_mode`]). The rule names
//! the helper's own root-owned path, so a program the user can replace never
//! gets it. On macOS it is a sudoers entry that lets the user's sudo run
//! without a password: Homebrew casks and the App Store run sudo themselves
//! for whatever their installers need, so nothing narrower covers them.
//! Changes the person starts still ask: the macOS app shows its own
//! administrator dialog and the CLI asks sudo for the password first.
use crate::{
    batch::{self, BatchMode, ALLOW_REMOVALS, UPGRADE_ONLY},
    host::{Host, Runtime},
    process::{self, Cancellation, ExecutionError, Limits},
};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Output,
};

/// How a source may be updated with nobody watching.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unattended {
    /// Runs as the user, so its updates need no approval.
    User,
    /// Needs root, so it updates only under the saved approval.
    Approved,
    /// Never updated unattended: firmware (power and restarts),
    /// inventories, and system managers the helper doesn't run.
    Never,
}

pub fn unattended(backend: &str) -> Unattended {
    match backend {
        id if batch::UPGRADE_ONLY_BACKENDS.contains(&id)
            || matches!(id, "macports" | "mas" | "macos-updates") =>
        {
            Unattended::Approved
        }
        "fwupd" | "macos-apps" | "system-image" | "apk" | "xbps" | "aur" | "toolbox"
        | "distrobox" => Unattended::Never,
        _ => Unattended::User,
    }
}

/// Runner argument that saves the approval for the user who ran pkexec.
pub const GRANT: &str = "--allow-unattended-updates";
/// Runner argument that removes that user's approval.
pub const REVOKE: &str = "--forbid-unattended-updates";
const FLATPAK_PREFIX: &str = "/var/lib/flatpak/app/io.github.astrovm.PkgDeck/";
const SNAP_PREFIX: &str = "/snap/pkgdeck/";
const RUNNER: &str = "pkgdeck-host-runner";
const RULES_DIR: &str = "/etc/polkit-1/rules.d";

fn invalid(reason: impl Into<String>) -> ExecutionError {
    ExecutionError::Invalid(reason.into())
}

/// The helper's entry point: a reviewed batch, an upgrade-only batch, or
/// saving or removing the approval.
pub fn run_runner(args: &[OsString]) -> Result<(), ExecutionError> {
    match args {
        [] => batch::serve(BatchMode::Reviewed),
        [flag] if flag == UPGRADE_ONLY => batch::serve(BatchMode::UpgradeOnly { removals: false }),
        [flag, removals] if flag == UPGRADE_ONLY && removals == ALLOW_REMOVALS => {
            batch::serve(BatchMode::UpgradeOnly { removals: true })
        }
        [flag] if flag == GRANT => save_rule(true),
        [flag] if flag == REVOKE => save_rule(false),
        _ => Err(invalid("unknown runner arguments")),
    }
}

/// Which helper paths the approval covers. Flatpak and Snap give each
/// release a new folder, so the rule covers the app's own root-owned folder
/// there; any other install is matched exactly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunnerMatch {
    Exact(String),
    Prefix(String),
}
impl RunnerMatch {
    pub fn for_runner(path: &Path) -> Result<Self, ExecutionError> {
        let text = path
            .to_str()
            .filter(|text| safe_path(text) && text.ends_with(&format!("/{RUNNER}")))
            .ok_or_else(|| invalid("PkgDeck's helper has an unexpected path"))?;
        Ok(
            match [FLATPAK_PREFIX, SNAP_PREFIX]
                .into_iter()
                .find(|prefix| text.starts_with(prefix))
            {
                Some(prefix) => Self::Prefix(prefix.into()),
                None => Self::Exact(text.into()),
            },
        )
    }
    /// A short description saved with the setting, to tell whether the
    /// helper found now is still the approved one.
    pub fn key(&self) -> String {
        match self {
            Self::Exact(path) => path.clone(),
            Self::Prefix(prefix) => format!("{prefix}*"),
        }
    }
    fn test(&self) -> String {
        match self {
            Self::Exact(path) => format!("program == \"{path}\""),
            Self::Prefix(prefix) => format!(
                "program.indexOf(\"{prefix}\") == 0 && program.indexOf(\"/../\") < 0 && program.slice(-{}) == \"/{RUNNER}\"",
                RUNNER.len() + 1
            ),
        }
    }
}
/// Paths and user names are written into a JavaScript rule, so only plain
/// characters are accepted.
fn safe_path(text: &str) -> bool {
    text.starts_with('/')
        && !text.contains("/../")
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._+-".contains(&b))
}
fn safe_user(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// The polkit rule that lets `user` start the upgrade-only helper.
pub fn polkit_rule(user: &str, runner: &RunnerMatch) -> Result<String, ExecutionError> {
    if !safe_user(user) {
        return Err(invalid("unexpected user name"));
    }
    Ok(format!(
        r#"// Written by PkgDeck: "Allow automatic updates without a password" is on for {user}.
// It lets PkgDeck's helper refresh sources and update every system package
// for {user}, in an active local session, and nothing else. Turn the setting
// off in PkgDeck to remove this file.
polkit.addRule(function (action, subject) {{
    if (action.id != "org.freedesktop.policykit.exec" || subject.user != "{user}" ||
        !subject.local || !subject.active) {{
        return polkit.Result.NOT_HANDLED;
    }}
    var program = action.lookup("program");
    var command = action.lookup("command_line");
    if ({test} &&
        (command == program + " {UPGRADE_ONLY}" ||
         command == program + " {UPGRADE_ONLY} {ALLOW_REMOVALS}")) {{
        return polkit.Result.YES;
    }}
    return polkit.Result.NOT_HANDLED;
}});
"#,
        test = runner.test()
    ))
}
pub fn rule_path(dir: &Path, user: &str) -> PathBuf {
    dir.join(format!("49-pkgdeck-unattended-{user}.rules"))
}

/// True when `path` and every folder above it belong to root and nobody
/// else can write them, so no other user can swap the program.
pub fn root_owned(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    path.ancestors().all(|part| {
        std::fs::symlink_metadata(part)
            .is_ok_and(|meta| meta.uid() == 0 && meta.mode() & 0o022 == 0)
    })
}

/// Save or remove the rule, as root. The user is the one pkexec (or sudo)
/// ran for, never a name the caller passes.
fn save_rule(grant: bool) -> Result<(), ExecutionError> {
    save_rule_as(
        grant,
        rustix::process::geteuid().is_root(),
        ["PKEXEC_UID", "SUDO_UID"]
            .into_iter()
            .find_map(|name| std::env::var(name).ok()),
        user_name,
        helper_path,
        root_owned,
        Path::new(RULES_DIR),
        &mut io::stdout().lock(),
    )
}
/// The login name of `uid`, as `id` prints it.
fn user_name(uid: u32) -> io::Result<Output> {
    std::process::Command::new("/usr/bin/id")
        .args(["-nu", &uid.to_string()])
        .env_clear()
        .output()
}
/// The helper itself, with every link resolved.
fn helper_path() -> io::Result<PathBuf> {
    std::fs::canonicalize(std::env::current_exe()?)
}
#[allow(clippy::too_many_arguments)]
fn save_rule_as(
    grant: bool,
    root: bool,
    uid: Option<String>,
    name_of: fn(u32) -> io::Result<Output>,
    runner: fn() -> io::Result<PathBuf>,
    trusted: fn(&Path) -> bool,
    dir: &Path,
    out: &mut impl Write,
) -> Result<(), ExecutionError> {
    if !root {
        return Err(invalid("host runner requires root"));
    }
    let uid = uid
        .and_then(|uid| uid.parse::<u32>().ok())
        .filter(|uid| *uid != 0)
        .ok_or_else(|| invalid("run this through pkexec as the person allowing it"))?;
    let output = name_of(uid)?;
    let user = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || !safe_user(&user) {
        return Err(invalid("unexpected user name"));
    }
    let path = rule_path(dir, &user);
    if !grant {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        };
    }
    let runner = runner()?;
    if !trusted(&runner) {
        return Err(invalid(
            "This copy of PkgDeck can't do this. Use the system Flatpak, the Snap or your distro's package.",
        ));
    }
    let runner = RunnerMatch::for_runner(&runner)?;
    let rule = polkit_rule(&user, &runner)?;
    if !dir.is_dir() {
        return Err(invalid("this system's polkit has no rules folder"));
    }
    let temporary = dir.join(format!(".pkgdeck-unattended-{user}.tmp"));
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::remove_file(&temporary);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&temporary)?;
        file.write_all(rule.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, &path)?;
    // The caller saves this to tell later whether the helper it finds is
    // still the approved one.
    writeln!(out, "{}", runner.key())?;
    Ok(())
}

/// The approval key for the helper this frontend would start, or None when
/// it has none or the helper isn't root-owned.
pub fn current_runner(host: &Host) -> Option<String> {
    runner_key(host.runtime, &batch::runner_path(host)?)
}
/// A Flatpak's own folder is root-owned on the host even where the sandbox
/// can't tell, so only other installs are checked.
fn runner_key(runtime: Runtime, path: &Path) -> Option<String> {
    (runtime == Runtime::Flatpak || root_owned(path))
        .then(|| RunnerMatch::for_runner(path).ok())
        .flatten()
        .map(|runner| runner.key())
}

/// Save (`grant`) or remove the approval. Linux asks polkit for the
/// password once; macOS shows its administrator dialog. Returns the key
/// [`current_runner`] reports while the approval still applies.
pub fn set_approval(
    host: &Host,
    grant: bool,
    cancel: &Cancellation,
) -> Result<String, ExecutionError> {
    #[cfg(target_os = "macos")]
    {
        let _ = host;
        macos::set_approval(grant, cancel)
    }
    #[cfg(not(target_os = "macos"))]
    {
        polkit_approval(host, batch::runner_path(host), grant, cancel)
    }
}
/// Run `runner` through pkexec to save or remove the rule.
#[cfg(any(not(target_os = "macos"), test))]
fn polkit_approval(
    host: &Host,
    runner: Option<PathBuf>,
    grant: bool,
    cancel: &Cancellation,
) -> Result<String, ExecutionError> {
    let runner = runner.ok_or_else(|| {
        ExecutionError::Disabled("PkgDeck's system helper is not installed".into())
    })?;
    let (program, mut args) = crate::host::Authorization::Polkit.prefix(&runner);
    args.push(if grant { GRANT } else { REVOKE }.into());
    let command = host.command(Path::new(program), &args)?;
    polkit_result(process::run(command, Limits::default(), cancel, true))
}
/// pkexec exits with 126 when the password dialog is dismissed.
#[cfg(any(not(target_os = "macos"), test))]
fn polkit_result(
    result: Result<process::Completion, ExecutionError>,
) -> Result<String, ExecutionError> {
    let result = result?;
    match result.code {
        Some(0) => Ok(String::from_utf8_lossy(&result.stdout).trim().to_owned()),
        Some(126) => Err(ExecutionError::AuthorizationCancelled),
        _ => Err(ExecutionError::Failed(result)),
    }
}

/// macOS: a sudoers entry for the user. Casks, the App Store, MacPorts and
/// macOS updates all reach root through sudo, casks with whatever programs
/// their installers name, so the entry can't list commands.
#[cfg(any(target_os = "macos", test))]
pub mod macos {
    use super::*;

    pub const KEY: &str = "sudoers:automatic-updates";

    pub fn sudoers_line(user: &str) -> Result<String, ExecutionError> {
        if !safe_user(user) {
            return Err(invalid("unexpected user name"));
        }
        // SETENV: Homebrew passes its environment with `sudo -E`.
        Ok(format!("{user} ALL=(root) NOPASSWD:SETENV: ALL\n"))
    }
    /// sudo skips files whose names contain a dot.
    pub fn sudoers_path(user: &str) -> PathBuf {
        Path::new("/private/etc/sudoers.d")
            .join(format!("pkgdeck-unattended-{}", user.replace('.', "_")))
    }
    /// The shell script run as root: check the entry with visudo before it
    /// goes in place, so a mistake can never break sudo.
    pub fn script(user: &str, grant: bool) -> Result<String, ExecutionError> {
        let target = sudoers_path(user);
        let target = target.to_str().expect("built from checked text");
        if !grant {
            return Ok(format!("/bin/rm -f '{target}'"));
        }
        let line = sudoers_line(user)?;
        Ok(format!(
            "set -e; umask 077; /bin/mkdir -p /private/etc/sudoers.d; \
             tmp=$(/usr/bin/mktemp /private/etc/sudoers.d/.pkgdeck.XXXXXX); \
             trap '/bin/rm -f \"$tmp\"' EXIT; \
             /usr/bin/printf '%s' '{line}' > \"$tmp\"; \
             /usr/sbin/visudo -cqf \"$tmp\"; \
             /usr/sbin/chown root:wheel \"$tmp\"; /bin/chmod 0440 \"$tmp\"; \
             /bin/mv -f \"$tmp\" '{target}'; trap - EXIT"
        ))
    }

    #[cfg(target_os = "macos")]
    pub fn set_approval(grant: bool, cancel: &Cancellation) -> Result<String, ExecutionError> {
        let user = std::process::Command::new("/usr/bin/id")
            .arg("-un")
            .output()?;
        let user = String::from_utf8_lossy(&user.stdout).trim().to_owned();
        let args = ["-c".into(), script(&user, grant)?.into()];
        let shell = crate::host::shell_command(Path::new("/bin/sh"), &args)?;
        let apple = format!(
            "do shell script {} with administrator privileges",
            crate::host::applescript_string(&shell)
        );
        let mut command = std::process::Command::new("/usr/bin/osascript");
        command
            .args(["-e", &apple])
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .current_dir("/");
        approval_result(process::run(command, Limits::default(), cancel, true))
    }
    /// What the administrator dialog's script reported.
    pub(crate) fn approval_result(
        result: Result<process::Completion, ExecutionError>,
    ) -> Result<String, ExecutionError> {
        let result = crate::host::administrator_result(result?)?;
        if result.code == Some(0) {
            Ok(KEY.into())
        } else {
            Err(ExecutionError::Failed(result))
        }
    }

    /// Whether sudo runs a command now without asking. `sudo -l` can't
    /// tell: it lists commands that still need a password.
    #[cfg(target_os = "macos")]
    pub fn approved() -> bool {
        std::process::Command::new("/usr/bin/sudo")
            .args(["-n", "--", "/usr/bin/true"])
            .env_clear()
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_are_sorted_by_what_unattended_updates_may_do() {
        for id in [
            "apt",
            "flatpak",
            "dnf",
            "pacman",
            "zypper",
            "snap",
            "macports",
            "mas",
            "macos-updates",
        ] {
            assert_eq!(unattended(id), Unattended::Approved, "{id}");
        }
        for id in ["fwupd", "macos-apps", "apk", "xbps", "aur", "toolbox"] {
            assert_eq!(unattended(id), Unattended::Never, "{id}");
        }
        for id in [
            "homebrew",
            "homebrew-cask",
            "cargo",
            "npm",
            "oh-my-zsh",
            "rustup",
        ] {
            assert_eq!(unattended(id), Unattended::User, "{id}");
        }
    }

    #[test]
    fn runner_paths_match_exactly_or_by_their_app_folder() {
        let exact = RunnerMatch::for_runner(Path::new("/usr/libexec/pkgdeck-host-runner")).unwrap();
        assert_eq!(
            exact,
            RunnerMatch::Exact("/usr/libexec/pkgdeck-host-runner".into())
        );
        assert_eq!(exact.key(), "/usr/libexec/pkgdeck-host-runner");
        let flatpak = RunnerMatch::for_runner(Path::new(
            "/var/lib/flatpak/app/io.github.astrovm.PkgDeck/x86_64/stable/0123abcd/files/libexec/pkgdeck-host-runner",
        ))
        .unwrap();
        assert_eq!(flatpak, RunnerMatch::Prefix(FLATPAK_PREFIX.into()));
        assert_eq!(flatpak.key(), format!("{FLATPAK_PREFIX}*"));
        let snap = RunnerMatch::for_runner(Path::new(
            "/snap/pkgdeck/42/usr/libexec/pkgdeck-host-runner",
        ))
        .unwrap();
        assert_eq!(snap, RunnerMatch::Prefix(SNAP_PREFIX.into()));
        for bad in [
            "/usr/libexec/other",
            "relative/pkgdeck-host-runner",
            "/opt/a b/pkgdeck-host-runner",
            "/opt/x\"y/pkgdeck-host-runner",
            "/opt/../pkgdeck-host-runner",
        ] {
            assert!(RunnerMatch::for_runner(Path::new(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_polkit_rule_allows_only_the_upgrade_only_helper_for_one_user() {
        let rule = polkit_rule(
            "astro",
            &RunnerMatch::Exact("/usr/libexec/pkgdeck-host-runner".into()),
        )
        .unwrap();
        assert!(rule.contains(r#"subject.user != "astro""#));
        assert!(rule.contains("!subject.local || !subject.active"));
        assert!(rule.contains(r#"program == "/usr/libexec/pkgdeck-host-runner""#));
        assert!(rule.contains(r#"command == program + " --upgrade-only" ||"#));
        assert!(rule.contains(r#"command == program + " --upgrade-only --allow-removals")"#));
        assert!(rule.contains("polkit.Result.YES"));
        let prefix = polkit_rule("astro", &RunnerMatch::Prefix(FLATPAK_PREFIX.into())).unwrap();
        assert!(prefix.contains(&format!(r#"program.indexOf("{FLATPAK_PREFIX}") == 0"#)));
        assert!(prefix.contains(r#"program.slice(-20) == "/pkgdeck-host-runner""#));
        for user in ["", "-root", "a\"b", "a b", &"x".repeat(65)] {
            assert!(
                polkit_rule(user, &RunnerMatch::Exact("/x/pkgdeck-host-runner".into())).is_err()
            );
        }
        assert_eq!(
            rule_path(Path::new(RULES_DIR), "astro"),
            Path::new("/etc/polkit-1/rules.d/49-pkgdeck-unattended-astro.rules")
        );
    }

    #[test]
    fn root_ownership_is_required_all_the_way_up() {
        assert!(root_owned(Path::new("/")));
        assert!(!root_owned(&std::env::temp_dir().join("pkgdeck-not-there")));
        let mine = std::env::current_exe().unwrap();
        // Test binaries live in the build folder, which the user owns.
        assert!(!rustix::process::geteuid().is_root() && !root_owned(&mine));
    }

    #[test]
    fn the_runner_refuses_unknown_arguments_and_approval_without_root() {
        assert!(matches!(
            run_runner(&["--other".into()]),
            Err(ExecutionError::Invalid(_))
        ));
        for extra in ["extra", UPGRADE_ONLY] {
            assert!(matches!(
                run_runner(&[UPGRADE_ONLY.into(), extra.into()]),
                Err(ExecutionError::Invalid(_))
            ));
        }
        // Tests never run as root, so each of these stops at the root check.
        for args in [
            vec![GRANT],
            vec![REVOKE],
            vec![UPGRADE_ONLY, ALLOW_REMOVALS],
        ] {
            let args = args.into_iter().map(OsString::from).collect::<Vec<_>>();
            assert_eq!(run_runner(&args), Err(invalid("host runner requires root")));
        }
    }

    fn named(code: i32, name: &str) -> io::Result<Output> {
        use std::os::unix::process::ExitStatusExt;
        Ok(Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: format!("{name}\n").into_bytes(),
            stderr: Vec::new(),
        })
    }
    fn astro(_: u32) -> io::Result<Output> {
        named(0, "astro")
    }
    fn spaced(_: u32) -> io::Result<Output> {
        named(0, "a b")
    }
    fn unknown(_: u32) -> io::Result<Output> {
        named(1, "astro")
    }
    fn missing_id(_: u32) -> io::Result<Output> {
        Err(io::Error::other("no id"))
    }
    fn installed() -> io::Result<PathBuf> {
        Ok("/usr/libexec/pkgdeck-host-runner".into())
    }
    fn misnamed() -> io::Result<PathBuf> {
        Ok("/usr/libexec/other".into())
    }
    fn gone() -> io::Result<PathBuf> {
        Err(io::Error::other("no helper"))
    }
    fn yes(_: &Path) -> bool {
        true
    }
    fn no(_: &Path) -> bool {
        false
    }

    #[test]
    fn the_rule_is_saved_only_for_the_pkexec_user_and_a_root_owned_helper() {
        let dir = std::env::temp_dir().join(format!("pkgdeck-rules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let save = |grant, uid: Option<&str>, name_of, runner, trusted, dir: &Path| {
            let mut out = Vec::new();
            let result = save_rule_as(
                grant,
                true,
                uid.map(Into::into),
                name_of,
                runner,
                trusted,
                dir,
                &mut out,
            );
            result.map(|()| String::from_utf8(out).unwrap())
        };
        let mut out = Vec::new();
        assert_eq!(
            save_rule_as(true, false, None, astro, installed, yes, &dir, &mut out),
            Err(invalid("host runner requires root"))
        );
        for caller in [None, Some("0"), Some("abc")] {
            assert_eq!(
                save(true, caller, astro, installed, yes, &dir),
                Err(invalid("run this through pkexec as the person allowing it")),
                "{caller:?}"
            );
        }
        for name_of in [spaced, unknown] {
            assert_eq!(
                save(true, Some("1000"), name_of, installed, yes, &dir),
                Err(invalid("unexpected user name"))
            );
        }
        assert_eq!(
            save(true, Some("1000"), missing_id, installed, yes, &dir),
            Err(ExecutionError::Io("no id".into()))
        );
        assert_eq!(
            save(true, Some("1000"), astro, gone, yes, &dir),
            Err(ExecutionError::Io("no helper".into()))
        );
        assert!(matches!(
            save(true, Some("1000"), astro, installed, no, &dir),
            Err(ExecutionError::Invalid(reason)) if reason.contains("can't do this")
        ));
        assert!(save(true, Some("1000"), astro, misnamed, yes, &dir).is_err());
        assert_eq!(
            save(
                true,
                Some("1000"),
                astro,
                installed,
                yes,
                &dir.join("missing")
            ),
            Err(invalid("this system's polkit has no rules folder"))
        );

        // A temporary file left by an interrupted save is replaced.
        let temporary = dir.join(".pkgdeck-unattended-astro.tmp");
        std::fs::write(&temporary, "stale").unwrap();
        assert_eq!(
            save(true, Some("1000"), astro, installed, yes, &dir).unwrap(),
            "/usr/libexec/pkgdeck-host-runner\n"
        );
        let path = rule_path(&dir, "astro");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            polkit_rule(
                "astro",
                &RunnerMatch::for_runner(&installed().unwrap()).unwrap()
            )
            .unwrap()
        );
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o133, 0, "{mode:o}");
        }
        assert!(!temporary.exists());

        // Removing it works once, and again when it's already gone.
        for _ in 0..2 {
            assert_eq!(
                save(false, Some("1000"), astro, gone, no, &dir),
                Ok(String::new())
            );
            assert!(!path.exists());
        }
        std::fs::create_dir(&path).unwrap();
        assert!(matches!(
            save(false, Some("1000"), astro, gone, no, &dir),
            Err(ExecutionError::Io(_))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_rule_names_the_real_user_and_helper() {
        let output = user_name(rustix::process::getuid().as_raw()).unwrap();
        assert!(output.status.success());
        assert!(safe_user(String::from_utf8_lossy(&output.stdout).trim()));
        assert!(helper_path().unwrap().is_absolute());
    }

    #[test]
    fn only_a_root_owned_or_flatpak_helper_has_a_key() {
        let flatpak = format!("{FLATPAK_PREFIX}x86_64/stable/0123abcd/files/libexec/{RUNNER}");
        assert_eq!(
            runner_key(Runtime::Flatpak, Path::new(&flatpak)),
            Some(format!("{FLATPAK_PREFIX}*"))
        );
        assert_eq!(
            runner_key(Runtime::Flatpak, Path::new("/usr/libexec/other")),
            None
        );
        // The test binary lives in a folder the user owns.
        let mine = std::env::current_exe().unwrap();
        assert_eq!(runner_key(Runtime::Native, &mine), None);
        // A PkgDeck installed on this machine may have a helper of its own.
        let host = Host::new(Runtime::Native, Default::default());
        assert_eq!(
            current_runner(&host),
            batch::runner_path(&host).and_then(|path| runner_key(Runtime::Native, &path))
        );
    }

    #[test]
    fn saving_the_approval_runs_nothing_once_cancelled() {
        let cancel = Cancellation::default();
        cancel.cancel();
        let host = Host::new(Runtime::Native, Default::default());
        for grant in [true, false] {
            assert!(matches!(
                set_approval(&host, grant, &cancel),
                Err(ExecutionError::Disabled(_) | ExecutionError::Cancelled)
            ));
        }
        assert!(matches!(
            polkit_approval(&host, None, true, &cancel),
            Err(ExecutionError::Disabled(_))
        ));
        let runner = PathBuf::from("/usr/libexec/pkgdeck-host-runner");
        assert_eq!(
            polkit_approval(&host, Some(runner), false, &cancel),
            Err(ExecutionError::Cancelled)
        );
    }

    fn ended(code: i32, stdout: &str, stderr: &str) -> process::Completion {
        process::Completion {
            code: Some(code),
            signal: None,
            stdout: stdout.into(),
            stderr: stderr.into(),
            truncated: false,
            cancellation_deferred: false,
        }
    }

    #[test]
    fn pkexec_reports_the_key_a_dismissed_dialog_or_a_failure() {
        assert_eq!(
            polkit_result(Ok(ended(0, "/usr/libexec/pkgdeck-host-runner\n", ""))),
            Ok("/usr/libexec/pkgdeck-host-runner".into())
        );
        assert_eq!(
            polkit_result(Ok(ended(126, "", ""))),
            Err(ExecutionError::AuthorizationCancelled)
        );
        assert_eq!(
            polkit_result(Ok(ended(1, "", "no"))),
            Err(ExecutionError::Failed(ended(1, "", "no")))
        );
        assert_eq!(
            polkit_result(Err(ExecutionError::Cancelled)),
            Err(ExecutionError::Cancelled)
        );
    }

    #[test]
    fn the_administrator_dialog_reports_the_key_a_dismissal_or_a_failure() {
        assert_eq!(
            macos::approval_result(Ok(ended(0, "", ""))),
            Ok(macos::KEY.into())
        );
        assert_eq!(
            macos::approval_result(Ok(ended(1, "", "User canceled. (-128)"))),
            Err(ExecutionError::AuthorizationCancelled)
        );
        assert_eq!(
            macos::approval_result(Ok(ended(1, "", "visudo: bad"))),
            Err(ExecutionError::Failed(ended(1, "", "visudo: bad")))
        );
        assert_eq!(
            macos::approval_result(Err(ExecutionError::Cancelled)),
            Err(ExecutionError::Cancelled)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_cancelled_job_never_shows_the_password_dialog() {
        let cancel = Cancellation::default();
        cancel.cancel();
        for grant in [true, false] {
            assert_eq!(
                macos::set_approval(grant, &cancel),
                Err(ExecutionError::Cancelled)
            );
        }
        // sudo -n never asks for a password.
        let _ = macos::approved();
    }

    #[test]
    fn the_sudoers_entry_names_one_user_and_is_checked_first() {
        let line = macos::sudoers_line("astro").unwrap();
        assert_eq!(line, "astro ALL=(root) NOPASSWD:SETENV: ALL\n");
        assert!(macos::sudoers_line("a b").is_err());
        assert_eq!(
            macos::sudoers_path("first.last"),
            Path::new("/private/etc/sudoers.d/pkgdeck-unattended-first_last")
        );
        let script = macos::script("astro", true).unwrap();
        let visudo = script.find("visudo -cqf").unwrap();
        let install = script.find("/bin/mv -f").unwrap();
        assert!(visudo < install);
        assert!(script.contains("chmod 0440"));
        assert_eq!(
            macos::script("astro", false).unwrap(),
            "/bin/rm -f '/private/etc/sudoers.d/pkgdeck-unattended-astro'"
        );
    }
}
