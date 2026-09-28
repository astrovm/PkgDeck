//! Let Homebrew manage a copy of an app someone installed themselves.
//!
//! `brew install --cask --adopt` keeps the app where it is and records the
//! cask as installed. Two things make it unsafe to run blindly:
//!
//! - For casks that update themselves (`auto_updates`, which covers every
//!   app here), Homebrew skips its own "is this the same app?" check.
//! - If any later step fails, such as linking a command whose place is
//!   taken, Homebrew's rollback copies the adopted app into its Caskroom and
//!   then deletes that folder: the app is gone.
//!
//! So PkgDeck adopts only allowlisted apps, checks the publisher signature,
//! version, architecture and every place the cask writes to beforehand, and
//! keeps an APFS clone of the app (instant, no extra space) until Homebrew
//! has finished, restoring it if the app went missing.
use super::{conda::compare_versions, invalid};
use crate::{engine::*, package::*, process::*};
use serde_json::Value;
use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
};
#[cfg(target_os = "macos")]
use {
    super::bytes,
    crate::host::Host,
    std::{ffi::OsString, fs, time::Duration},
};

const ID: &str = "homebrew-cask";

/// An app PkgDeck knows how to hand to one cask. The publisher team comes from
/// the app's Developer ID signature; `None` accepts only an ad-hoc signature.
pub(super) struct Rule {
    pub token: &'static str,
    pub bundle_id: &'static str,
    pub team: Option<&'static str>,
    /// Cask parts allowed beyond `KNOWN_ARTIFACTS`. Only the recovery test
    /// fixture uses it, to make Homebrew fail after adopting the app.
    pub also: &'static [&'static str],
}

const RULES: &[Rule] = &[
    Rule {
        token: "visual-studio-code",
        bundle_id: "com.microsoft.VSCode",
        team: Some("UBF8T346G9"),
        also: &[],
    },
    Rule {
        token: "firefox",
        bundle_id: "org.mozilla.firefox",
        team: Some("43AQ936H96"),
        also: &[],
    },
    // Read from the signed app in the cask's own DMG (Dynalist Inc.).
    Rule {
        token: "obsidian",
        bundle_id: "md.obsidian",
        team: Some("6JSW4SJWN9"),
        also: &[],
    },
    // Only test builds know the CI fixture, an ad-hoc signed app in a local tap.
    #[cfg(debug_assertions)]
    Rule {
        token: "pkgdeck/fixtures/pkgdeck-adopt-fixture",
        bundle_id: "io.github.astrovm.pkgdeck.adopt-fixture",
        team: None,
        also: &[],
    },
    // The same fixture app, whose cask fails in a postflight step after the
    // app was adopted: Homebrew's rollback then deletes it, and the CI test
    // checks that PkgDeck puts it back.
    #[cfg(debug_assertions)]
    Rule {
        token: "pkgdeck/fixtures/pkgdeck-adopt-failure",
        bundle_id: "io.github.astrovm.pkgdeck.adopt-fixture",
        team: None,
        also: &["postflight"],
    },
];

/// Cask parts PkgDeck can check. Anything else could fail after the app was
/// adopted and trigger Homebrew's destructive rollback.
const KNOWN_ARTIFACTS: &[&str] = &["app", "binary", "command_wrapper", "uninstall", "zap"];

pub(super) fn rule(token: &str) -> Option<&'static Rule> {
    RULES.iter().find(|rule| rule.token == token)
}

/// What adoption needs from the system; tests replace it.
pub(super) trait AdoptIo: Send {
    /// Anything at this path, including a dangling symlink.
    fn occupied(&self, path: &Path) -> bool;
    /// A real folder, not a symlink to one.
    fn is_folder(&self, path: &Path) -> bool;
    fn is_file(&self, path: &Path) -> bool;
    /// A symlink at `link` that resolves to the same file as `source`.
    fn links_to(&self, link: &Path, source: &Path) -> bool;
    fn plist(&self, path: &Path, cancel: &Cancellation) -> Result<Value, EngineError>;
    /// The Developer ID team of a valid signature; `None` for ad-hoc.
    /// An invalid or missing signature is an error.
    fn team(&self, app: &Path, cancel: &Cancellation) -> Result<Option<String>, EngineError>;
    fn architectures(
        &self,
        executable: &Path,
        cancel: &Cancellation,
    ) -> Result<Vec<String>, EngineError>;
    /// Clone the app aside; returns the clone's path.
    fn backup(&self, app: &Path, cancel: &Cancellation) -> Result<PathBuf, EngineError>;
    fn restore(&self, backup: &Path, app: &Path, cancel: &Cancellation) -> Result<(), EngineError>;
    fn discard(&self, backup: &Path);
}

/// An adoption PkgDeck checked and is ready to run.
#[derive(Debug)]
pub(super) struct Plan {
    pub token: String,
    pub app: PathBuf,
    pub name: String,
    /// What was checked, shown before confirming. The engine plans again
    /// just before running, so a copy that changed since blocks adoption.
    pub version: String,
    pub team: Option<String>,
}

impl Plan {
    /// The confirmation text: what happens, and what was checked.
    pub fn preview(&self) -> String {
        format!(
            "{} is already in {} (version {}, signed by {}). Homebrew will manage this copy instead of installing another (brew install --cask --adopt). PkgDeck checked its publisher, edition, architecture, version and every file the cask adds, and keeps a copy of the app until Homebrew finishes.",
            self.name,
            self.app.parent().unwrap_or(Path::new("/")).display(),
            self.version,
            self.team.as_deref().unwrap_or("an ad-hoc signature"),
        )
    }
}

fn refuse(app: &str, reason: impl std::fmt::Display) -> EngineError {
    invalid(
        ID,
        format!("{app} is already installed, and PkgDeck can't hand it to Homebrew: {reason}. Nothing was changed."),
    )
}

fn artifact_kind(artifact: &Value) -> Option<&str> {
    artifact
        .as_object()?
        .keys()
        .map(String::as_str)
        .find(|key| *key != "target")
}

/// Decide whether installing `cask` (one `brew info --json=v2` cask) means
/// adopting an existing app. `Ok(None)`: no app is in the way, so a normal
/// install runs. An existing app that fails any check is an error, never a
/// normal install, which Homebrew would refuse or, with `--force`, overwrite.
pub(super) fn plan(
    io: &dyn AdoptIo,
    cask: &Value,
    cancel: &Cancellation,
) -> Result<Option<Plan>, EngineError> {
    let token = cask["full_token"].as_str().unwrap_or_default();
    let artifacts = cask["artifacts"].as_array().cloned().unwrap_or_default();
    let apps: Vec<&Value> = artifacts
        .iter()
        .filter(|a| artifact_kind(a) == Some("app"))
        .collect();
    let [app] = apps.as_slice() else {
        return Ok(None);
    };
    let Some(target) = app["target"].as_str().map(PathBuf::from) else {
        return Ok(None);
    };
    if !io.occupied(&target) {
        return Ok(None);
    }
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().trim_end_matches(".app").to_owned())
        .unwrap_or_else(|| token.to_owned());
    let Some(rule) = rule(token) else {
        return Err(refuse(
            &name,
            format!("{token} is not one of the casks PkgDeck can check an existing copy against"),
        ));
    };
    if let Some(unknown) = artifacts
        .iter()
        .filter_map(artifact_kind)
        .find(|kind| !KNOWN_ARTIFACTS.contains(kind) && !rule.also.contains(kind))
    {
        return Err(refuse(
            &name,
            format!("the cask now also installs a {unknown}, which PkgDeck doesn't check yet"),
        ));
    }
    if !target.is_absolute() || !io.is_folder(&target) {
        return Err(refuse(
            &name,
            format!("{} is not an app folder", target.display()),
        ));
    }
    if io.occupied(&target.join("Contents/_MASReceipt")) {
        return Err(refuse(
            &name,
            "it came from the App Store, which keeps updating it",
        ));
    }
    let info = io.plist(&target.join("Contents/Info.plist"), cancel)?;
    if info["CFBundleIdentifier"].as_str() != Some(rule.bundle_id) {
        return Err(refuse(
            &name,
            format!("its bundle identifier is not {}", rule.bundle_id),
        ));
    }
    let team = io
        .team(&target, cancel)
        .map_err(|error| refuse(&name, format!("its signature doesn't verify ({error})")))?;
    if team.as_deref() != rule.team {
        return Err(refuse(
            &name,
            format!(
                "it is signed by {}, not the publisher {} expects",
                team.as_deref().unwrap_or("no Developer ID"),
                rule.team.unwrap_or("an ad-hoc signature")
            ),
        ));
    }
    let executable = info["CFBundleExecutable"]
        .as_str()
        .filter(|exe| !exe.is_empty() && !exe.contains('/'))
        .ok_or_else(|| refuse(&name, "its Info.plist names no executable"))?;
    let host = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    if !io
        .architectures(&target.join("Contents/MacOS").join(executable), cancel)?
        .iter()
        .any(|arch| arch == host)
    {
        return Err(refuse(
            &name,
            format!("it doesn't run natively on this Mac ({host})"),
        ));
    }
    let version = info["CFBundleShortVersionString"]
        .as_str()
        .unwrap_or_default();
    let cask_version = cask["version"].as_str().unwrap_or_default();
    let cask_version = cask_version.split(',').next().unwrap_or(cask_version);
    if version.is_empty() || compare_versions(version, cask_version) == Ordering::Less {
        return Err(refuse(
            &name,
            format!(
                "it is version {}, older than the cask's {cask_version}; update it first",
                if version.is_empty() {
                    "unknown"
                } else {
                    version
                }
            ),
        ));
    }
    // Every other place the cask writes must be free, or already be the
    // link Homebrew would make; a conflict there fails after adoption.
    for artifact in &artifacts {
        let kind = artifact_kind(artifact).unwrap_or_default();
        if !matches!(kind, "binary" | "command_wrapper") {
            continue;
        }
        let Some(place) = artifact["target"].as_str().map(PathBuf::from) else {
            return Err(refuse(
                &name,
                format!("Homebrew didn't say where its {kind} goes"),
            ));
        };
        if kind == "binary" {
            let source = artifact[kind][0]
                .as_str()
                .map(PathBuf::from)
                .unwrap_or_default();
            if !source.starts_with(&target) || !io.is_file(&source) {
                return Err(refuse(
                    &name,
                    format!("the command {} is missing from this copy", source.display()),
                ));
            }
            if io.occupied(&place) && !io.links_to(&place, &source) {
                return Err(refuse(
                    &name,
                    format!(
                        "{} already exists and belongs to something else",
                        place.display()
                    ),
                ));
            }
        } else if io.occupied(&place) {
            return Err(refuse(&name, format!("{} already exists", place.display())));
        }
    }
    Ok(Some(Plan {
        token: token.to_owned(),
        app: target,
        name,
        version: version.to_owned(),
        team,
    }))
}

/// Whether the app at `plan.app` is still the one that was adopted.
fn intact(io: &dyn AdoptIo, plan: &Plan, cancel: &Cancellation) -> bool {
    let Some(rule) = rule(&plan.token) else {
        return false;
    };
    io.is_folder(&plan.app)
        && io
            .plist(&plan.app.join("Contents/Info.plist"), cancel)
            .is_ok_and(|info| info["CFBundleIdentifier"].as_str() == Some(rule.bundle_id))
        && io
            .team(&plan.app, cancel)
            .is_ok_and(|team| team.as_deref() == rule.team)
}

/// Run a checked adoption. `install` runs `brew install --cask --adopt`, and
/// `installed` confirms Homebrew now lists the cask.
pub(super) fn run(
    io: &dyn AdoptIo,
    plan: &Plan,
    cancel: &Cancellation,
    progress: &mut dyn FnMut(Progress),
    install: &mut dyn FnMut() -> Result<Completion, EngineError>,
    installed: &mut dyn FnMut() -> bool,
) -> Result<OperationOutcome, EngineError> {
    let backup = io.backup(&plan.app, cancel)?;
    progress(Progress::Message(format!(
        "{} is already in {}. Handing it to Homebrew; a copy is kept until this finishes.",
        plan.name,
        plan.app.parent().unwrap_or(Path::new("/")).display()
    )));
    let result = install();
    // Checks run even after cancellation: Homebrew may have finished anyway.
    let check = Cancellation::default();
    let kept = intact(io, plan, &check);
    let adopted = result.is_ok() && kept && installed();
    if adopted {
        io.discard(&backup);
        progress(Progress::Message(format!(
            "Homebrew now manages {}.",
            plan.name
        )));
        return Ok(OperationOutcome {
            cancellation_deferred: result
                .map(|done| done.cancellation_deferred)
                .unwrap_or(false),
        });
    }
    let failure = match result {
        Err(error) => error.to_string(),
        Ok(_) => "Homebrew didn't finish adopting it".to_owned(),
    };
    if kept {
        io.discard(&backup);
        return Err(invalid(
            ID,
            format!("{failure}. {} was left as it was.", plan.name),
        ));
    }
    match io.restore(&backup, &plan.app, &check) {
        Ok(()) => {
            io.discard(&backup);
            Err(invalid(ID, format!("{failure}. Homebrew removed {} while undoing its changes; PkgDeck put it back.", plan.name)))
        }
        Err(error) => Err(invalid(ID, format!(
            "{failure}. Homebrew removed {} and PkgDeck couldn't put it back ({error}); your copy is at {}.",
            plan.name,
            backup.display()
        ))),
    }
}

/// The real system: plutil, codesign, lipo and APFS clones.
#[cfg(target_os = "macos")]
pub(super) struct NativeAdopt(pub Host);

#[cfg(target_os = "macos")]
impl NativeAdopt {
    fn read(
        &self,
        tool: &str,
        args: &[&OsString],
        cancel: &Cancellation,
    ) -> Result<Completion, EngineError> {
        let args: Vec<OsString> = args.iter().map(|arg| (*arg).clone()).collect();
        Ok(self.0.read(
            Path::new(tool),
            &args,
            Limits {
                timeout: Duration::from_secs(120),
                output_bytes: 1024 * 1024,
            },
            cancel,
        )?)
    }
    fn backups(&self) -> Result<PathBuf, EngineError> {
        let home = self
            .0
            .var("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
            .ok_or_else(|| invalid(ID, "HOME is not set"))?;
        Ok(home.join("Library/Application Support/PkgDeck/Adoption backups"))
    }
    fn clone_tree(&self, from: &Path, to: &Path, cancel: &Cancellation) -> Result<(), EngineError> {
        // -c clones on APFS, so a whole app costs no space until it changes.
        let result = self.0.standalone_write(
            Path::new("/bin/cp"),
            &[
                "-c".into(),
                "-R".into(),
                "-p".into(),
                "--".into(),
                from.into(),
                to.into(),
            ],
            &[],
            cancel,
        )?;
        bytes(ID, result).map(|_| ())
    }
}

#[cfg(target_os = "macos")]
impl AdoptIo for NativeAdopt {
    fn occupied(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok()
    }
    fn is_folder(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
    }
    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }
    fn links_to(&self, link: &Path, source: &Path) -> bool {
        fs::symlink_metadata(link).is_ok_and(|meta| meta.file_type().is_symlink())
            && matches!((fs::canonicalize(link), fs::canonicalize(source)), (Ok(a), Ok(b)) if a == b)
    }
    fn plist(&self, path: &Path, cancel: &Cancellation) -> Result<Value, EngineError> {
        let result = self.read(
            "/usr/bin/plutil",
            &[
                &"-convert".into(),
                &"json".into(),
                &"-o".into(),
                &"-".into(),
                &"--".into(),
                &path.into(),
            ],
            cancel,
        )?;
        serde_json::from_slice(&bytes(ID, result)?).map_err(|error| invalid(ID, error))
    }
    fn team(&self, app: &Path, cancel: &Cancellation) -> Result<Option<String>, EngineError> {
        let app = OsString::from(app);
        bytes(
            ID,
            self.read(
                "/usr/bin/codesign",
                &[&"--verify".into(), &"--strict".into(), &"--".into(), &app],
                cancel,
            )?,
        )?;
        let details = self.read(
            "/usr/bin/codesign",
            &[&"-dv".into(), &"--verbose=2".into(), &"--".into(), &app],
            cancel,
        )?;
        let text = String::from_utf8_lossy(&details.stderr).into_owned();
        bytes(ID, details)?;
        let team = text
            .lines()
            .find_map(|line| line.strip_prefix("TeamIdentifier="))
            .map(str::trim)
            .filter(|team| *team != "not set" && !team.is_empty())
            .map(str::to_owned);
        Ok(team)
    }
    fn architectures(
        &self,
        executable: &Path,
        cancel: &Cancellation,
    ) -> Result<Vec<String>, EngineError> {
        let output = bytes(
            ID,
            self.read(
                "/usr/bin/lipo",
                &[&"-archs".into(), &executable.into()],
                cancel,
            )?,
        )?;
        Ok(String::from_utf8_lossy(&output)
            .split_whitespace()
            .map(str::to_owned)
            .collect())
    }
    fn backup(&self, app: &Path, cancel: &Cancellation) -> Result<PathBuf, EngineError> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_millis())
            .unwrap_or_default();
        let folder = self.backups()?.join(stamp.to_string());
        fs::create_dir_all(&folder).map_err(ExecutionError::from)?;
        let copy = folder.join(
            app.file_name()
                .ok_or_else(|| invalid(ID, "app path has no name"))?,
        );
        if let Err(error) = self.clone_tree(app, &copy, cancel) {
            let _ = fs::remove_dir_all(&folder);
            return Err(error);
        }
        Ok(copy)
    }
    fn restore(&self, backup: &Path, app: &Path, cancel: &Cancellation) -> Result<(), EngineError> {
        if self.occupied(app) {
            return Err(invalid(
                ID,
                format!("something else is now at {}", app.display()),
            ));
        }
        if fs::rename(backup, app).is_ok() {
            return Ok(());
        }
        self.clone_tree(backup, app, cancel)
    }
    fn discard(&self, backup: &Path) {
        // Only ever the timestamped folder this adoption created.
        if let Some(folder) = backup.parent().filter(|folder| {
            self.backups()
                .is_ok_and(|root| folder.parent() == Some(root.as_path()))
        }) {
            let _ = fs::remove_dir_all(folder);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// An app folder described in memory, plus a log of what ran.
    #[derive(Clone)]
    struct Fake {
        present: Arc<Mutex<bool>>,
        receipt: bool,
        bundle: &'static str,
        executable: &'static str,
        team: Result<Option<&'static str>, &'static str>,
        archs: Vec<&'static str>,
        version: &'static str,
        taken: Vec<&'static str>,
        links: Vec<&'static str>,
        restore_fails: bool,
        log: Arc<Mutex<Vec<String>>>,
    }
    const APP: &str = "/Applications/Visual Studio Code.app";
    const CODE: &str = "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code";
    fn fake() -> Fake {
        Fake {
            present: Arc::new(Mutex::new(true)),
            receipt: false,
            bundle: "com.microsoft.VSCode",
            executable: "Electron",
            team: Ok(Some("UBF8T346G9")),
            archs: vec!["x86_64", "arm64"],
            version: "1.139.1",
            taken: vec![],
            links: vec![],
            restore_fails: false,
            log: Arc::default(),
        }
    }
    impl AdoptIo for Fake {
        fn occupied(&self, path: &Path) -> bool {
            let path = path.to_string_lossy();
            (path.starts_with(APP)
                && *self.present.lock().unwrap()
                && (self.receipt || !path.contains("_MASReceipt")))
                || self.taken.iter().any(|taken| path == *taken)
                || self.links.iter().any(|link| path == *link)
        }
        fn is_folder(&self, path: &Path) -> bool {
            path == Path::new(APP) && *self.present.lock().unwrap()
        }
        fn is_file(&self, path: &Path) -> bool {
            path == Path::new(CODE) && *self.present.lock().unwrap()
        }
        fn links_to(&self, link: &Path, source: &Path) -> bool {
            self.links.iter().any(|l| link == Path::new(l)) && source == Path::new(CODE)
        }
        fn plist(&self, _: &Path, _: &Cancellation) -> Result<Value, EngineError> {
            Ok(serde_json::json!({
                "CFBundleIdentifier": self.bundle,
                "CFBundleExecutable": self.executable,
                "CFBundleShortVersionString": self.version,
            }))
        }
        fn team(&self, _: &Path, _: &Cancellation) -> Result<Option<String>, EngineError> {
            self.team
                .map(|team| team.map(Into::into))
                .map_err(|error| invalid(ID, error))
        }
        fn architectures(
            &self,
            executable: &Path,
            _: &Cancellation,
        ) -> Result<Vec<String>, EngineError> {
            assert!(executable.ends_with("Contents/MacOS/Electron"));
            Ok(self.archs.iter().map(|arch| (*arch).into()).collect())
        }
        fn backup(&self, _: &Path, _: &Cancellation) -> Result<PathBuf, EngineError> {
            self.log.lock().unwrap().push("backup".into());
            Ok("/backups/1/Visual Studio Code.app".into())
        }
        fn restore(&self, _: &Path, _: &Path, _: &Cancellation) -> Result<(), EngineError> {
            self.log.lock().unwrap().push("restore".into());
            if self.restore_fails {
                return Err(invalid(ID, "disk full"));
            }
            *self.present.lock().unwrap() = true;
            Ok(())
        }
        fn discard(&self, _: &Path) {
            self.log.lock().unwrap().push("discard".into());
        }
    }
    fn cask() -> Value {
        serde_json::json!({
            "full_token": "visual-studio-code",
            "version": "1.139.1",
            "auto_updates": true,
            "artifacts": [
                {"uninstall": [{"quit": "com.microsoft.VSCode"}]},
                {"app": ["Visual Studio Code.app"], "target": APP},
                {"binary": [CODE], "target": "/opt/homebrew/bin/code"},
                {"zap": [{"trash": ["~/.vscode"]}]}
            ]
        })
    }
    fn reason(result: Result<Option<Plan>, EngineError>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn a_checked_copy_is_adopted() {
        let plan = plan(&fake(), &cask(), &Cancellation::default())
            .unwrap()
            .unwrap();
        assert_eq!(plan.app, Path::new(APP));
        assert_eq!(plan.name, "Visual Studio Code");
        // A newer self-updated copy, a link that is already Homebrew's, and a
        // universal or native build are all fine.
        let mut newer = fake();
        newer.version = "1.140.0";
        newer.links = vec!["/opt/homebrew/bin/code"];
        newer.archs = vec![if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x86_64"
        }];
        assert!(plan_ok(&newer));
    }
    fn plan_ok(io: &Fake) -> bool {
        plan(io, &cask(), &Cancellation::default())
            .unwrap()
            .is_some()
    }

    #[test]
    fn nothing_in_the_way_is_a_normal_install() {
        let absent = fake();
        *absent.present.lock().unwrap() = false;
        assert!(plan(&absent, &cask(), &Cancellation::default())
            .unwrap()
            .is_none());
    }

    #[test]
    fn only_the_recovery_fixture_may_fail_after_the_app_step() {
        let mut vscode = cask();
        vscode["artifacts"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"postflight": null}));
        let reason = reason(plan(&fake(), &vscode, &Cancellation::default()));
        assert!(reason.contains("also installs a postflight"), "{reason}");
        if cfg!(debug_assertions) {
            assert_eq!(
                rule("pkgdeck/fixtures/pkgdeck-adopt-failure").unwrap().also,
                ["postflight"]
            );
        }
        assert!(rule("visual-studio-code").unwrap().also.is_empty());
    }

    type Change = Box<dyn Fn(&mut Fake, &mut Value)>;

    #[test]
    fn every_check_refuses_before_anything_changes() {
        let cases: Vec<(Change, &str)> = vec![
            (
                Box::new(|_, c| c["full_token"] = "some-other-app".into()),
                "not one of the casks",
            ),
            (
                Box::new(|_, c| {
                    c["artifacts"]
                        .as_array_mut()
                        .unwrap()
                        .push(serde_json::json!({"pkg": ["x.pkg"]}))
                }),
                "also installs a pkg",
            ),
            (Box::new(|f, _| f.receipt = true), "App Store"),
            (
                Box::new(|f, _| f.bundle = "com.microsoft.VSCodeInsiders"),
                "bundle identifier",
            ),
            (
                Box::new(|f, _| f.team = Ok(Some("EVILTEAM00"))),
                "signed by EVILTEAM00",
            ),
            (
                Box::new(|f, _| f.team = Ok(None)),
                "signed by no Developer ID",
            ),
            (
                Box::new(|f, _| f.team = Err("invalid signature")),
                "signature doesn't verify",
            ),
            (
                Box::new(|f, _| f.archs = vec!["ppc"]),
                "doesn't run natively",
            ),
            (
                Box::new(|f, _| f.version = "1.100.0"),
                "older than the cask's 1.139.1",
            ),
            (
                Box::new(|f, _| f.taken = vec!["/opt/homebrew/bin/code"]),
                "belongs to something else",
            ),
            (
                Box::new(|_, c| c["artifacts"][2]["binary"][0] = "/elsewhere/code".into()),
                "missing from this copy",
            ),
        ];
        for (change, expected) in cases {
            let (mut io, mut cask) = (fake(), cask());
            change(&mut io, &mut cask);
            let reason = reason(plan(&io, &cask, &Cancellation::default()));
            assert!(reason.contains(expected), "{expected}: {reason}");
            assert!(reason.contains("Nothing was changed"), "{reason}");
            assert!(
                io.log.lock().unwrap().is_empty(),
                "no backup for {expected}"
            );
        }
        // The app is still found when Homebrew lists a comma version.
        let mut versioned = cask();
        versioned["version"] = "1.139.1,abcdef".into();
        assert!(plan(&fake(), &versioned, &Cancellation::default())
            .unwrap()
            .is_some());
    }

    fn adopt(
        io: &Fake,
        install: Result<(), &'static str>,
        removes_app: bool,
        listed: bool,
    ) -> Result<OperationOutcome, EngineError> {
        let plan = plan(io, &cask(), &Cancellation::default())
            .unwrap()
            .unwrap();
        let present = io.present.clone();
        run(
            io,
            &plan,
            &Cancellation::default(),
            &mut |_| {},
            &mut || {
                if removes_app {
                    *present.lock().unwrap() = false;
                }
                install
                    .map(|()| Completion {
                        code: Some(0),
                        signal: None,
                        stdout: vec![],
                        stderr: vec![],
                        truncated: false,
                        cancellation_deferred: false,
                    })
                    .map_err(|error| invalid(ID, error))
            },
            &mut || listed,
        )
    }

    #[test]
    fn success_keeps_the_app_and_drops_the_copy() {
        let io = fake();
        adopt(&io, Ok(()), false, true).unwrap();
        assert_eq!(*io.log.lock().unwrap(), vec!["backup", "discard"]);
    }

    #[test]
    fn a_failed_rollback_that_removed_the_app_is_undone() {
        let io = fake();
        let error = adopt(&io, Err("It seems there is already a Binary"), true, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("PkgDeck put it back"), "{error}");
        assert_eq!(
            *io.log.lock().unwrap(),
            vec!["backup", "restore", "discard"]
        );
        assert!(*io.present.lock().unwrap());
    }

    #[test]
    fn a_failure_that_left_the_app_alone_just_reports() {
        let io = fake();
        let error = adopt(&io, Err("download failed"), false, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("left as it was"), "{error}");
        assert_eq!(*io.log.lock().unwrap(), vec!["backup", "discard"]);
        // Homebrew claiming success without listing the cask is a failure too.
        let io = fake();
        assert!(adopt(&io, Ok(()), false, false).is_err());
    }

    /// The real clone, restore and cleanup, on throwaway folders.
    #[cfg(target_os = "macos")]
    #[test]
    fn native_backups_clone_restore_and_clean_up() {
        use crate::host::Runtime;
        let root = std::env::temp_dir().join(format!("pkgdeck-adopt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let app = root.join("Apps/Fixture.app");
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::write(app.join("Contents/MacOS/fixture"), "binary").unwrap();
        let env = [
            ("HOME", root.join("home")),
            ("PATH", "/usr/bin:/bin".into()),
        ]
        .into_iter()
        .map(|(key, value)| (OsString::from(key), value.into_os_string()))
        .collect();
        let io = NativeAdopt(Host::new(Runtime::Native, env));
        let cancel = Cancellation::default();
        let backup = io.backup(&app, &cancel).unwrap();
        assert!(backup
            .starts_with(root.join("home/Library/Application Support/PkgDeck/Adoption backups")));
        assert_eq!(
            fs::read(backup.join("Contents/MacOS/fixture")).unwrap(),
            b"binary"
        );
        // Something else in the app's place is never overwritten.
        assert!(io.restore(&backup, &app, &cancel).is_err());
        fs::remove_dir_all(&app).unwrap();
        io.restore(&backup, &app, &cancel).unwrap();
        assert_eq!(
            fs::read(app.join("Contents/MacOS/fixture")).unwrap(),
            b"binary"
        );
        io.discard(&backup);
        let backups = root.join("home/Library/Application Support/PkgDeck/Adoption backups");
        assert_eq!(fs::read_dir(&backups).unwrap().count(), 0);
        // Discarding only ever removes a folder under the backups root.
        io.discard(&app.join("Contents/MacOS/fixture"));
        assert!(app.exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_copy_that_cannot_be_put_back_is_kept_and_named() {
        let mut io = fake();
        io.restore_fails = true;
        let error = adopt(&io, Err("rollback"), true, false)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("your copy is at /backups/1/Visual Studio Code.app"),
            "{error}"
        );
        assert_eq!(*io.log.lock().unwrap(), vec!["backup", "restore"]);
    }

    /// The checks that read the file system directly, on throwaway files:
    /// a symlink never counts as the app folder, and only a link to the
    /// cask's own command counts as Homebrew's.
    #[cfg(target_os = "macos")]
    #[test]
    fn file_system_checks_tell_links_from_real_files() {
        use crate::host::Runtime;
        let root = std::env::temp_dir().join(format!("pkgdeck-adopt-fs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let app = root.join("Fixture.app");
        let command = app.join("fixture");
        fs::create_dir_all(&app).unwrap();
        fs::write(&command, "binary").unwrap();
        fs::write(root.join("other"), "other").unwrap();
        let link = |name: &str, to: &Path| {
            let path = root.join(name);
            std::os::unix::fs::symlink(to, &path).unwrap();
            path
        };
        let app_link = link("App link.app", &app);
        let ours = link("ours", &command);
        let theirs = link("theirs", &root.join("other"));
        let dangling = link("dangling", &root.join("gone"));
        let io = NativeAdopt(Host::new(Runtime::Native, Default::default()));
        assert!(io.is_folder(&app));
        assert!(!io.is_folder(&app_link));
        assert!(!io.is_folder(&command));
        assert!(io.is_file(&command));
        assert!(!io.is_file(&app));
        assert!(io.occupied(&dangling));
        assert!(!io.occupied(&root.join("gone")));
        assert!(io.links_to(&ours, &command));
        assert!(!io.links_to(&theirs, &command));
        assert!(!io.links_to(&dangling, &command));
        // The command itself is not a link to it.
        assert!(!io.links_to(&command, &command));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn casks_without_one_placed_app_install_normally() {
        let io = fake();
        let cancel = Cancellation::default();
        let mut no_app = cask();
        no_app["artifacts"].as_array_mut().unwrap().remove(1);
        assert!(plan(&io, &no_app, &cancel).unwrap().is_none());
        let mut two_apps = cask();
        let second = two_apps["artifacts"][1].clone();
        two_apps["artifacts"].as_array_mut().unwrap().push(second);
        assert!(plan(&io, &two_apps, &cancel).unwrap().is_none());
        let mut no_target = cask();
        no_target["artifacts"][1] = serde_json::json!({"app": ["Visual Studio Code.app"]});
        assert!(plan(&io, &no_target, &cancel).unwrap().is_none());
    }

    #[test]
    fn unusual_copies_and_casks_are_refused() {
        let cancel = Cancellation::default();
        for (executable, version, expected) in [
            ("", "1.139.1", "names no executable"),
            ("../Electron", "1.139.1", "names no executable"),
            ("Electron", "", "version unknown"),
        ] {
            let mut io = fake();
            io.executable = executable;
            io.version = version;
            let reason = reason(plan(&io, &cask(), &cancel));
            assert!(reason.contains(expected), "{expected}: {reason}");
        }
        let mut relative = cask();
        relative["artifacts"][1]["target"] = "Visual Studio Code.app".into();
        let mut io = fake();
        io.taken = vec!["Visual Studio Code.app"];
        assert!(reason(plan(&io, &relative, &cancel)).contains("is not an app folder"));
        let mut unplaced = cask();
        unplaced["artifacts"][2] = serde_json::json!({"binary": [CODE]});
        assert!(
            reason(plan(&fake(), &unplaced, &cancel)).contains("didn't say where its binary goes")
        );
        // A wrapper script Homebrew writes itself must not replace anything.
        let mut wrapper = cask();
        wrapper["artifacts"][2] =
            serde_json::json!({"command_wrapper": [CODE], "target": "/opt/homebrew/bin/code"});
        assert!(plan(&fake(), &wrapper, &cancel).unwrap().is_some());
        let mut io = fake();
        io.taken = vec!["/opt/homebrew/bin/code"];
        assert!(
            reason(plan(&io, &wrapper, &cancel)).contains("/opt/homebrew/bin/code already exists")
        );
    }

    #[test]
    fn a_deferred_cancellation_is_reported_and_an_unknown_app_is_never_intact() {
        let io = fake();
        let plan = plan(&io, &cask(), &Cancellation::default())
            .unwrap()
            .unwrap();
        let outcome = run(
            &io,
            &plan,
            &Cancellation::default(),
            &mut |_| {},
            &mut || {
                Ok(Completion {
                    code: Some(0),
                    signal: None,
                    stdout: vec![],
                    stderr: vec![],
                    truncated: false,
                    cancellation_deferred: true,
                })
            },
            &mut || true,
        )
        .unwrap();
        assert!(outcome.cancellation_deferred);
        let stranger = Plan {
            token: "some-other-app".into(),
            ..plan
        };
        assert!(!intact(&io, &stranger, &Cancellation::default()));
    }
}
