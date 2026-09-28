//! AUR packages on Arch: installed packages Pacman's repositories don't have
//! (`pacman -Qm`), checked against the AUR's current versions.
//!
//! Updates only, and only through the user's own AUR helper (paru, then
//! yay): PkgDeck never builds a PKGBUILD itself and never installs or
//! removes AUR packages. Building one runs its PKGBUILD, and in June 2026
//! malicious commits reached about 1,500 AUR packages, so every update's
//! preview links the PKGBUILD's change history to review before confirming.
//! These packages are not listed under Pacman, so each appears once.
use super::{bytes, invalid, NativeTransport, Transport};
use crate::{engine::*, host::Authorization, package::*, process::*};
use serde::Deserialize;
use std::{cmp::Ordering, collections::BTreeMap, ffi::OsString};

const ID: &str = "aur";
const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Installed,
    Capability::Details,
    Capability::Upgrade,
];
/// Helpers tried in order, with the flags that make them build without
/// asking questions. How they get root follows PkgDeck's permission setting
/// (see `permission_flags`).
const HELPERS: [(&str, &[&str]); 2] = [
    ("paru", &["-S", "--needed", "--noconfirm", "--skipreview"]),
    (
        "yay",
        &[
            "-S",
            "--needed",
            "--noconfirm",
            "--answerdiff=None",
            "--answerclean=None",
        ],
    ),
];

/// The helper runs `pacman` as root the way PkgDeck would: through the
/// desktop's password prompt (pkexec), or an existing sudo login without
/// prompting, since the helper has no terminal to ask in.
fn permission_flags(authorization: Authorization) -> [&'static str; 1] {
    match authorization {
        Authorization::Polkit => ["--sudo=/usr/bin/pkexec"],
        Authorization::SudoNonInteractive => ["--sudoflags=-n"],
    }
}
/// Names per AUR RPC request, well under its URL length limit.
const BATCH: usize = 100;

pub struct Aur<T = NativeTransport> {
    transport: T,
    errors: Vec<EngineError>,
}

#[derive(Deserialize, Debug)]
struct Reply {
    #[serde(default)]
    results: Vec<Info>,
    #[serde(default)]
    error: Option<String>,
}
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
struct Info {
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(rename = "URL", default)]
    url: Option<String>,
    #[serde(default)]
    maintainer: Option<String>,
    #[serde(default)]
    out_of_date: Option<i64>,
    #[serde(default)]
    package_base: Option<String>,
}

/// Pacman's package names: no URL syntax can leak into the RPC query.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._+-".contains(&b))
}

impl<T: Transport> Aur<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            errors: vec![],
        }
    }

    fn pacman(&self, args: &[&str], cancel: &Cancellation) -> Result<String, EngineError> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let output = match self
            .transport
            .system_manager("pacman", &args, cancel, false)
        {
            // pacman -Q exits 1 when nothing matches, such as no foreign packages.
            Err(ExecutionError::Failed(result))
                if result.code == Some(1) && result.stdout.is_empty() =>
            {
                vec![]
            }
            result => bytes(ID, result?)?,
        };
        String::from_utf8(output).map_err(|error| invalid(ID, error))
    }

    /// Foreign packages and their installed versions. Without synced
    /// repository databases every package looks foreign, so that is an error
    /// rather than a list of the whole system.
    fn foreign(&self, cancel: &Cancellation) -> Result<BTreeMap<String, String>, EngineError> {
        let parse = |text: &str| -> BTreeMap<String, String> {
            text.lines()
                .filter_map(|line| line.split_once(' '))
                .filter(|(name, _)| valid_name(name))
                .map(|(name, version)| (name.to_owned(), version.trim().to_owned()))
                .collect()
        };
        let foreign = parse(&self.pacman(&["-Qm"], cancel)?);
        if !foreign.is_empty() && foreign.len() == parse(&self.pacman(&["-Q"], cancel)?).len() {
            return Err(invalid(ID, "Pacman's repository databases aren't synced, so AUR packages can't be told apart; refresh Pacman first"));
        }
        Ok(foreign)
    }

    fn info(
        &self,
        names: &[&String],
        cancel: &Cancellation,
    ) -> Result<BTreeMap<String, Info>, EngineError> {
        let mut found = BTreeMap::new();
        for chunk in names.chunks(BATCH) {
            let query: Vec<String> = chunk.iter().map(|name| format!("arg[]={name}")).collect();
            let url = format!("https://aur.archlinux.org/rpc/v5/info?{}", query.join("&"));
            let args: Vec<OsString> = [
                "-q",
                "--fail",
                "--silent",
                "--show-error",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--connect-timeout",
                "5",
                "--max-time",
                "20",
                "--max-filesize",
                "8388608",
                "--user-agent",
                "PkgDeck",
                "--",
                &url,
            ]
            .iter()
            .map(OsString::from)
            .collect();
            let output = bytes(
                ID,
                self.transport
                    .system_manager("curl", &args, cancel, false)?,
            )?;
            let reply: Reply =
                serde_json::from_slice(&output).map_err(|error| invalid(ID, error))?;
            if let Some(error) = reply.error {
                return Err(invalid(ID, format!("AUR: {error}")));
            }
            found.extend(
                reply
                    .results
                    .into_iter()
                    .map(|info| (info.name.clone(), info)),
            );
        }
        Ok(found)
    }

    /// `vercmp`, Pacman's own version order (epochs, pkgrel, alpha tags).
    fn newer(
        &self,
        candidate: &str,
        installed: &str,
        cancel: &Cancellation,
    ) -> Result<bool, EngineError> {
        if candidate == installed {
            return Ok(false);
        }
        let output = bytes(
            ID,
            self.transport.system_manager(
                "vercmp",
                &[candidate.into(), installed.into()],
                cancel,
                false,
            )?,
        )?;
        let order: i32 = String::from_utf8_lossy(&output)
            .trim()
            .parse()
            .map_err(|error| invalid(ID, error))?;
        Ok(order.cmp(&0) == Ordering::Greater)
    }

    fn package(
        name: &str,
        installed: &str,
        info: Option<&Info>,
        candidate: Option<Option<String>>,
    ) -> Package {
        Package {
            id: PackageId {
                backend: ID.into(),
                name: name.into(),
                architecture: std::env::consts::ARCH.into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: name.into(),
            // Only a completed check can say a package is missing from the AUR.
            summary: match (info, &candidate) {
                (Some(info), _) => info
                    .description
                    .clone()
                    .unwrap_or_else(|| "AUR package".into()),
                (None, Some(_)) => "Not in the AUR: built locally or removed from the AUR".into(),
                (None, None) => "AUR package".into(),
            },
            installed_version: Some(installed.into()),
            update: match &candidate {
                Some(Some(_)) => UpdateAvailability::Available,
                Some(None) => UpdateAvailability::Current,
                None => UpdateAvailability::Unknown,
            },
            candidate_version: candidate.flatten(),
            icon: None,
            component_ids: vec![],
            homepages: info.and_then(|info| info.url.clone()).into_iter().collect(),
            adopt_with: None,
        }
    }

    /// The first installed AUR helper.
    fn helper(
        &self,
        cancel: &Cancellation,
    ) -> Result<(&'static str, &'static [&'static str]), EngineError> {
        for (helper, flags) in HELPERS {
            match self
                .transport
                .dev_tool(helper, &["--version".into()], cancel, false)
            {
                Ok(_) => return Ok((helper, flags)),
                Err(ExecutionError::Cancelled) => return Err(EngineError::Cancelled),
                Err(_) => {}
            }
        }
        Err(invalid(
            ID,
            "updating AUR packages needs an AUR helper; install paru or yay",
        ))
    }

    /// The installed and AUR versions of an outdated AUR package.
    fn update_of(
        &self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<Option<(String, Info)>, EngineError> {
        if id.backend != ID || !valid_name(&id.name) {
            return Err(EngineError::NotFound);
        }
        let installed = self
            .foreign(cancel)?
            .remove(&id.name)
            .ok_or(EngineError::NotFound)?;
        let Some(found) = self.info(&[&id.name], cancel)?.remove(&id.name) else {
            return Err(invalid(
                ID,
                format!(
                    "{} isn't in the AUR, so there is nothing to update from",
                    id.name
                ),
            ));
        };
        Ok(self
            .newer(&found.version, &installed, cancel)?
            .then_some((installed, found)))
    }

    fn rows(&mut self, check: bool, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.errors.clear();
        let foreign = self.foreign(cancel)?;
        let names: Vec<&String> = foreign.keys().collect();
        let info = if check && !names.is_empty() {
            match self.info(&names, cancel) {
                Ok(info) => Some(info),
                Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
                // Offline: still list the packages, with unknown updates.
                Err(error) => {
                    self.errors.push(error);
                    None
                }
            }
        } else {
            None
        };
        let mut rows = vec![];
        for (name, installed) in &foreign {
            let found = info.as_ref().and_then(|info| info.get(name));
            let candidate = match (&info, found) {
                (Some(_), Some(found)) => Some(
                    self.newer(&found.version, installed, cancel)?
                        .then(|| found.version.clone()),
                ),
                (Some(_), None) => Some(None),
                (None, _) => None,
            };
            rows.push(Self::package(name, installed, found, candidate));
        }
        Ok(rows)
    }
}

impl<T: Transport> Backend for Aur<T> {
    fn id(&self) -> &str {
        ID
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    #[cfg(not(target_os = "linux"))]
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Unavailable(
            "The AUR is for Arch Linux".into(),
        ))
    }
    #[cfg(target_os = "linux")]
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        match self.pacman(&["-Qmq"], cancel) {
            Ok(_) => Ok(Availability::Available),
            Err(EngineError::Execution(ExecutionError::Disabled(reason))) => {
                Ok(Availability::Unavailable(reason))
            }
            Err(error) => Err(error),
        }
    }
    fn query_errors(&self) -> Vec<EngineError> {
        self.errors.clone()
    }
    fn may_have(&self, name: &str) -> bool {
        valid_name(name)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        // Installed AUR packages only: installing means building a PKGBUILD.
        let query = query.to_lowercase();
        Ok(self
            .rows(false, cancel)?
            .into_iter()
            .filter(|row| row.id.name.contains(&query))
            .collect())
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        self.rows(true, cancel)
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let foreign = self.foreign(cancel)?;
        let installed = foreign
            .get(&id.name)
            .filter(|_| id.backend == ID)
            .ok_or(EngineError::NotFound)?;
        let info = self.info(&[&id.name], cancel)?;
        let found = info.get(&id.name);
        let mut description = vec![];
        if let Some(found) = found {
            description.push(found.description.clone().unwrap_or_default());
            description.push(format!("AUR version: {}", found.version));
            description.push(format!(
                "Maintainer: {}",
                found.maintainer.as_deref().unwrap_or("none (orphaned)")
            ));
            if found.out_of_date.is_some() {
                description.push("Flagged out of date on the AUR.".into());
            }
        } else {
            description.push(
                "This package isn't in the AUR: it was built locally or removed from the AUR."
                    .into(),
            );
        }
        description.push("Updates run your AUR helper (paru or yay), which builds the new PKGBUILD. Review what changed in it before updating: building runs it.".into());
        let candidate = match found {
            Some(found) => Some(
                self.newer(&found.version, installed, cancel)?
                    .then(|| found.version.clone()),
            ),
            None => Some(None),
        };
        Ok(PackageDetails {
            package: Self::package(&id.name, installed, found, candidate),
            description: description.join("\n\n"),
            homepage: Some(format!("https://aur.archlinux.org/packages/{}", id.name)),
            dependencies: vec![],
        })
    }
    fn operation_plan(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
    ) -> Result<Option<TransactionPlan>, EngineError> {
        let Operation::Upgrade(id) = operation else {
            return Ok(None);
        };
        let Some((installed, found)) = self.update_of(id, cancel)? else {
            return Ok(None);
        };
        let (helper, _) = self.helper(cancel)?;
        let base = found.package_base.as_deref().unwrap_or(&found.name);
        Ok(Some(TransactionPlan {
            operation: operation.clone(),
            native_preview: format!(
                "{helper} will build {} {installed} → {} from its PKGBUILD, which runs code from the AUR. Review what changed first: https://aur.archlinux.org/cgit/aur.git/log/?h={base}",
                id.name, found.version
            ),
            changes: vec![PlannedChange {
                action: PlannedAction::Upgrade,
                name: id.name.clone(),
                installed_version: Some(installed),
                candidate_version: Some(found.version),
            }],
            download_bytes: None,
            disk_bytes: None,
            restart_required: None,
adopts: None,
        }))
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let Operation::Upgrade(id) = operation else {
            return Err(self.unsupported(operation.capability()));
        };
        let Some((installed, found)) = self.update_of(id, cancel)? else {
            return Ok(OperationOutcome::default());
        };
        let (helper, flags) = self.helper(cancel)?;
        if cancel.requested() {
            return Err(EngineError::Cancelled);
        }
        progress(Progress::Message(format!(
            "Building {} {} with {helper}. If you cancel, PkgDeck waits for it to finish.",
            id.name, found.version
        )));
        let mut args: Vec<OsString> = flags
            .iter()
            .chain(&permission_flags(self.transport.authorization()))
            .map(OsString::from)
            .collect();
        args.push("--".into());
        args.push(id.name.clone().into());
        let completion = match self.transport.dev_tool(helper, &args, cancel, true) {
            Err(ExecutionError::Failed(result)) => {
                let stderr = String::from_utf8_lossy(&result.stderr);
                // pkexec exits 126 when its prompt is dismissed.
                return Err(
                    if stderr.contains("sudo:") || stderr.contains("Not authorized") {
                        ExecutionError::AuthorizationDenied
                    } else if result.code == Some(126) {
                        ExecutionError::AuthorizationCancelled
                    } else {
                        ExecutionError::Failed(result)
                    }
                    .into(),
                );
            }
            result => result?,
        };
        let deferred = completion.cancellation_deferred;
        bytes(ID, completion)?;
        // Writes may finish after cancellation; check with a fresh read.
        let now = self.foreign(&Cancellation::default())?.remove(&id.name);
        if now.as_deref() == Some(installed.as_str()) || now.is_none() {
            return Err(invalid(
                ID,
                format!("{helper} finished, but {} is still {installed}", id.name),
            ));
        }
        Ok(OperationOutcome {
            cancellation_deferred: deferred,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Fake {
        unsynced: bool,
        /// Every package is from a repository: pacman -Qm exits 1.
        none_foreign: bool,
        offline: bool,
        helper: Option<&'static str>,
        sudo_fails: bool,
        /// The helper's build fails with this exit code and error output.
        build_fails: Option<(i32, &'static str)>,
        /// The helper's build succeeds without installing anything.
        builds_nothing: bool,
        /// The installed yay version, which a helper run updates.
        yay: Arc<Mutex<Option<String>>>,
        calls: Arc<Mutex<Vec<String>>>,
        /// Every pacman query fails, as with a locked or broken database.
        pacman_broken: bool,
        /// Replaces the AUR RPC answer.
        rpc: Option<Result<Completion, ExecutionError>>,
        /// Replaces vercmp's answer.
        vercmp: Option<Result<Completion, ExecutionError>>,
        /// Probing for the helper is interrupted.
        helper_cancelled: bool,
    }
    impl Fake {
        fn yay(&self) -> String {
            self.yay
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "12.0.0-1".into())
        }
    }
    fn done(text: &str) -> Completion {
        Completion {
            code: Some(0),
            signal: None,
            stdout: text.as_bytes().to_vec(),
            stderr: vec![],
            truncated: false,
            cancellation_deferred: false,
        }
    }
    fn ignore(_: Progress) {}
    impl Transport for Fake {
        fn system_manager(
            &self,
            executable: &str,
            args: &[OsString],
            _: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            assert!(!write, "only the AUR helper writes");
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            self.calls
                .lock()
                .unwrap()
                .push(format!("{executable} {}", args.join(" ")));
            match (executable, args[0].as_str()) {
                ("pacman", _) if self.pacman_broken => Err(ExecutionError::Failed(Completion {
                    code: Some(2),
                    stderr: b"error: failed to init transaction (unable to lock database)".to_vec(),
                    ..done("")
                })),
                ("pacman", "-Qm" | "-Qmq") if self.none_foreign => {
                    Err(ExecutionError::Failed(Completion {
                        code: Some(1),
                        ..done("")
                    }))
                }
                ("pacman", "-Qm") if self.unsynced => Ok(done("bash 5.3-1\nyay 12.0.0-1\n")),
                ("pacman", "-Qm") => Ok(done(&format!(
                    "yay {}\nlocal-tool 1.0-1\nparu 2.1.0-1\n",
                    self.yay()
                ))),
                ("pacman", "-Qmq") => Ok(done("yay\n")),
                ("pacman", "-Q") => Ok(done("bash 5.3-1\nyay 12.0.0-1\n")),
                ("curl", _) if self.rpc.is_some() => self.rpc.clone().unwrap(),
                ("curl", _) if self.offline => Err(ExecutionError::Failed(Completion {
                    code: Some(6),
                    ..done("")
                })),
                ("curl", _) => {
                    let url = args.last().unwrap();
                    assert!(
                        url.starts_with("https://aur.archlinux.org/rpc/v5/info?arg[]="),
                        "{url}"
                    );
                    Ok(done(
                        r#"{"resultcount":2,"results":[
                        {"Name":"yay","PackageBase":"yay","Version":"13.0.1-1","Description":"Yet another yogurt","URL":"https://github.com/Jguer/yay","Maintainer":"jguer","OutOfDate":null},
                        {"Name":"paru","Version":"2.1.0-1","Description":"Feature packed AUR helper","Maintainer":null,"OutOfDate":1790000000}
                    ],"type":"multiinfo","version":5}"#,
                    ))
                }
                _ if self.vercmp.is_some() => self.vercmp.clone().unwrap(),
                _ => {
                    assert_eq!(executable, "vercmp");
                    // Only the 12.x yay is older than the AUR's 13.0.1-1.
                    Ok(done(if args[1].starts_with("12.") {
                        "1\n"
                    } else {
                        "-1\n"
                    }))
                }
            }
        }
        fn dev_tool(
            &self,
            executable: &str,
            args: &[OsString],
            _: &Cancellation,
            write: bool,
        ) -> Result<Completion, ExecutionError> {
            if self.helper_cancelled {
                return Err(ExecutionError::Cancelled);
            }
            if Some(executable) != self.helper {
                return Err(ExecutionError::Disabled(format!("{executable} not found")));
            }
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            self.calls
                .lock()
                .unwrap()
                .push(format!("{executable} {}", args.join(" ")));
            if !write {
                return Ok(done("paru v2.1.0"));
            }
            if self.sudo_fails {
                return Err(ExecutionError::Failed(Completion {
                    code: Some(1),
                    stderr: b"sudo: a password is required".to_vec(),
                    ..done("")
                }));
            }
            if let Some((code, stderr)) = self.build_fails {
                return Err(ExecutionError::Failed(Completion {
                    code: Some(code),
                    stderr: stderr.as_bytes().to_vec(),
                    ..done("")
                }));
            }
            if !self.builds_nothing {
                *self.yay.lock().unwrap() = Some("13.0.1-1".into());
            }
            Ok(done(""))
        }
    }

    #[test]
    fn foreign_packages_are_checked_against_the_aur() {
        let fake = Fake::default();
        let mut aur = Aur::new(fake.clone());
        let rows = aur.installed(&Cancellation::default()).unwrap();
        let summary: Vec<(&str, UpdateAvailability, Option<&str>)> = rows
            .iter()
            .map(|row| {
                (
                    row.id.name.as_str(),
                    row.update.clone(),
                    row.candidate_version.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("local-tool", UpdateAvailability::Current, None),
                ("paru", UpdateAvailability::Current, None),
                ("yay", UpdateAvailability::Available, Some("13.0.1-1")),
            ]
        );
        assert!(rows[0].summary.starts_with("Not in the AUR"));
        assert_eq!(rows[2].homepages, vec!["https://github.com/Jguer/yay"]);
        // Equal versions never need vercmp.
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.starts_with("vercmp 2.1.0-1")));
        let details = aur.details(&rows[1].id, &Cancellation::default()).unwrap();
        assert!(details.description.contains("orphaned"));
        assert!(details.description.contains("Flagged out of date"));
        assert!(details.description.contains("Review what changed"));
        // Without an AUR helper there is nothing to build with.
        let error = aur
            .execute(
                &Operation::Upgrade(rows[2].id.clone()),
                &Cancellation::default(),
                &mut ignore,
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("install paru or yay"), "{error}");
        assert!(aur
            .execute(
                &Operation::Remove(rows[2].id.clone()),
                &Cancellation::default(),
                &mut ignore
            )
            .is_err());
    }

    #[test]
    fn updates_build_with_the_helper_after_a_reviewable_preview() {
        let fake = Fake {
            helper: Some("paru"),
            ..Fake::default()
        };
        let mut aur = Aur::new(fake.clone());
        let rows = aur.installed(&Cancellation::default()).unwrap();
        let yay = Operation::Upgrade(rows[2].id.clone());
        let plan = aur
            .operation_plan(&yay, &Cancellation::default())
            .unwrap()
            .unwrap();
        assert!(
            plan.native_preview
                .starts_with("paru will build yay 12.0.0-1 → 13.0.1-1"),
            "{}",
            plan.native_preview
        );
        assert!(plan
            .native_preview
            .contains("https://aur.archlinux.org/cgit/aur.git/log/?h=yay"));
        aur.execute(&yay, &Cancellation::default(), &mut ignore)
            .unwrap();
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .contains(&"paru -S --needed --noconfirm --skipreview --sudoflags=-n -- yay".into()));
        assert_eq!(
            permission_flags(Authorization::Polkit),
            ["--sudo=/usr/bin/pkexec"]
        );
        assert_eq!(fake.yay(), "13.0.1-1");
        // Now current: no preview, and updating again runs nothing.
        assert!(aur
            .operation_plan(&yay, &Cancellation::default())
            .unwrap()
            .is_none());
        fake.calls.lock().unwrap().clear();
        aur.execute(&yay, &Cancellation::default(), &mut |_| {})
            .unwrap();
        assert!(!fake.calls.lock().unwrap().iter().any(|c| c.contains("-S")));
    }

    #[test]
    fn a_missing_sudo_login_is_an_authorization_failure() {
        let fake = Fake {
            helper: Some("yay"),
            sudo_fails: true,
            ..Fake::default()
        };
        let mut aur = Aur::new(fake.clone());
        let rows = aur.installed(&Cancellation::default()).unwrap();
        assert!(matches!(
            aur.execute(
                &Operation::Upgrade(rows[2].id.clone()),
                &Cancellation::default(),
                &mut |_| {}
            ),
            Err(EngineError::Execution(ExecutionError::AuthorizationDenied))
        ));
        assert!(fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.starts_with("yay -S --needed --noconfirm --answerdiff=None")));
        // Searches skip the AUR check, so they never claim a package left it.
        let found = aur.search("yay", &Cancellation::default()).unwrap();
        assert_eq!(found[0].summary, "AUR package");
    }

    #[test]
    fn offline_checks_still_list_packages() {
        let mut aur = Aur::new(Fake {
            offline: true,
            ..Fake::default()
        });
        let rows = aur.installed(&Cancellation::default()).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows
            .iter()
            .all(|row| row.update == UpdateAvailability::Unknown));
        assert_eq!(aur.query_errors().len(), 1);
    }

    #[test]
    fn a_system_without_aur_packages_lists_none() {
        let mut aur = Aur::new(Fake {
            none_foreign: true,
            ..Fake::default()
        });
        assert_eq!(
            aur.detect(&Cancellation::default()).unwrap(),
            if cfg!(target_os = "linux") {
                Availability::Available
            } else {
                Availability::Unavailable("The AUR is for Arch Linux".into())
            }
        );
        assert!(aur.installed(&Cancellation::default()).unwrap().is_empty());
    }

    #[test]
    fn unsynced_databases_are_an_error_not_the_whole_system() {
        let mut aur = Aur::new(Fake {
            unsynced: true,
            ..Fake::default()
        });
        let error = aur
            .installed(&Cancellation::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("aren't synced"), "{error}");
    }

    #[test]
    fn only_outdated_aur_packages_can_be_updated() {
        let fake = Fake {
            helper: Some("paru"),
            ..Fake::default()
        };
        let mut aur = Aur::new(fake.clone());
        assert!(aur.may_have("yay-bin") && !aur.may_have("-Syu"));
        // PkgDeck never builds a new AUR package or removes one.
        assert!(aur.capabilities().contains(&Capability::Upgrade));
        assert!(!aur.capabilities().contains(&Capability::Install));
        assert!(!aur.capabilities().contains(&Capability::Remove));
        let cancel = Cancellation::default();
        let rows = aur.installed(&cancel).unwrap();
        let details = aur.details(&rows[0].id, &cancel).unwrap();
        assert!(details
            .description
            .starts_with("This package isn't in the AUR"));
        assert_eq!(details.package.update, UpdateAvailability::Current);
        let update = |aur: &mut Aur<Fake>, id: &PackageId, cancel: &Cancellation| {
            aur.execute(&Operation::Upgrade(id.clone()), cancel, &mut |_| {})
        };
        let error = update(&mut aur, &rows[0].id, &cancel).unwrap_err();
        assert!(
            error.to_string().contains("local-tool isn't in the AUR"),
            "{error}"
        );
        let mut foreign = rows[2].id.clone();
        foreign.backend = "pacman".into();
        assert!(matches!(
            update(&mut aur, &foreign, &cancel),
            Err(EngineError::NotFound)
        ));
        assert!(matches!(
            aur.details(&foreign, &cancel),
            Err(EngineError::NotFound)
        ));
        let mut absent = rows[2].id.clone();
        absent.name = "absent".into();
        assert!(matches!(
            update(&mut aur, &absent, &cancel),
            Err(EngineError::NotFound)
        ));
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(matches!(
            update(&mut aur, &rows[2].id, &cancelled),
            Err(EngineError::Cancelled)
        ));
        assert!(!fake.calls.lock().unwrap().iter().any(|c| c.contains("-S")));
        // Other operations have no plan and never run.
        assert!(aur
            .operation_plan(&Operation::Remove(rows[2].id.clone()), &cancel)
            .unwrap()
            .is_none());
    }

    #[test]
    fn helper_failures_keep_their_meaning() {
        for (build_fails, expected) in [
            (
                (127, "==> ERROR: Not authorized"),
                ExecutionError::AuthorizationDenied,
            ),
            ((126, ""), ExecutionError::AuthorizationCancelled),
        ] {
            let mut aur = Aur::new(Fake {
                helper: Some("paru"),
                build_fails: Some(build_fails),
                ..Fake::default()
            });
            let rows = aur.installed(&Cancellation::default()).unwrap();
            let error = aur
                .execute(
                    &Operation::Upgrade(rows[2].id.clone()),
                    &Cancellation::default(),
                    &mut |_| {},
                )
                .unwrap_err();
            assert_eq!(error, EngineError::Execution(expected));
        }
        // A failed build is reported with its own output.
        let mut aur = Aur::new(Fake {
            helper: Some("paru"),
            build_fails: Some((1, "error: failed to build 'yay-13.0.1-1'")),
            ..Fake::default()
        });
        let rows = aur.installed(&Cancellation::default()).unwrap();
        assert!(matches!(
            aur.execute(
                &Operation::Upgrade(rows[2].id.clone()),
                &Cancellation::default(),
                &mut |_| {}
            ),
            Err(EngineError::Execution(ExecutionError::Failed(result))) if result.code == Some(1)
        ));
        // A build that installed nothing is not a success.
        let mut aur = Aur::new(Fake {
            helper: Some("paru"),
            builds_nothing: true,
            ..Fake::default()
        });
        let rows = aur.installed(&Cancellation::default()).unwrap();
        let error = aur
            .execute(
                &Operation::Upgrade(rows[2].id.clone()),
                &Cancellation::default(),
                &mut |_| {},
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("paru finished, but yay is still 12.0.0-1"),
            "{error}"
        );
    }

    #[test]
    fn aur_rpc_failures_are_reported_not_hidden() {
        let cancel = Cancellation::default();
        let mut aur = Aur::new(Fake {
            rpc: Some(Ok(done(
                r#"{"error":"Too many package results.","results":[]}"#,
            ))),
            ..Fake::default()
        });
        let rows = aur.installed(&cancel).unwrap();
        assert!(rows
            .iter()
            .all(|row| row.update == UpdateAvailability::Unknown));
        assert_eq!(
            aur.query_errors(),
            [invalid(ID, "AUR: Too many package results.")]
        );
        assert_eq!(
            aur.details(&rows[0].id, &cancel),
            Err(invalid(ID, "AUR: Too many package results."))
        );

        let mut aur = Aur::new(Fake {
            rpc: Some(Ok(Completion {
                truncated: true,
                ..done("{}")
            })),
            ..Fake::default()
        });
        let row = aur.search("yay", &cancel).unwrap().remove(0);
        assert_eq!(
            aur.details(&row.id, &cancel),
            Err(invalid(ID, "metadata exceeded output limit"))
        );

        // Cancelling the check cancels the listing instead of hiding updates.
        let mut aur = Aur::new(Fake {
            rpc: Some(Err(ExecutionError::Cancelled)),
            ..Fake::default()
        });
        assert_eq!(aur.installed(&cancel), Err(EngineError::Cancelled));
        assert!(aur.query_errors().is_empty());
    }

    #[test]
    fn version_comparison_failures_fail_the_listing() {
        let cancel = Cancellation::default();
        for (answer, expected) in [
            (
                Err(ExecutionError::Io("vercmp crashed".into())),
                EngineError::Execution(ExecutionError::Io("vercmp crashed".into())),
            ),
            (
                Ok(Completion {
                    truncated: true,
                    ..done("1\n")
                }),
                invalid(ID, "metadata exceeded output limit"),
            ),
        ] {
            let mut aur = Aur::new(Fake {
                vercmp: Some(answer),
                ..Fake::default()
            });
            assert_eq!(aur.installed(&cancel), Err(expected));
        }
        let mut aur = Aur::new(Fake {
            vercmp: Some(Ok(done("newer\n"))),
            ..Fake::default()
        });
        assert!(matches!(
            aur.installed(&cancel),
            Err(EngineError::InvalidResponse { backend, .. }) if backend == ID
        ));
    }

    #[test]
    fn a_locally_newer_build_is_current() {
        let fake = Fake::default();
        *fake.yay.lock().unwrap() = Some("14.0.0-1".into());
        let mut aur = Aur::new(fake);
        let rows = aur.installed(&Cancellation::default()).unwrap();
        assert_eq!(rows[2].id.name, "yay");
        assert_eq!(rows[2].update, UpdateAvailability::Current);
        assert_eq!(rows[2].candidate_version, None);
    }

    #[test]
    fn cancelling_the_helper_probe_builds_nothing() {
        let fake = Fake {
            helper: Some("paru"),
            helper_cancelled: true,
            ..Fake::default()
        };
        let mut aur = Aur::new(fake.clone());
        let cancel = Cancellation::default();
        let rows = aur.installed(&cancel).unwrap();
        assert_eq!(
            aur.execute(
                &Operation::Upgrade(rows[2].id.clone()),
                &cancel,
                &mut ignore
            ),
            Err(EngineError::Cancelled)
        );
        assert_eq!(
            aur.operation_plan(&Operation::Upgrade(rows[2].id.clone()), &cancel),
            Err(EngineError::Cancelled)
        );
        assert!(!fake.calls.lock().unwrap().iter().any(|c| c.contains("-S")));
    }

    #[test]
    fn a_broken_pacman_is_an_error_not_an_empty_list() {
        let cancel = Cancellation::default();
        let mut healthy = Aur::new(Fake::default());
        let mut broken = Aur::new(Fake {
            pacman_broken: true,
            ..Fake::default()
        });
        if cfg!(target_os = "linux") {
            assert_eq!(healthy.detect(&cancel), Ok(Availability::Available));
            assert!(matches!(
                broken.detect(&cancel),
                Err(EngineError::Execution(ExecutionError::Failed(result))) if result.code == Some(2)
            ));
        }
        assert!(matches!(
            broken.installed(&cancel),
            Err(EngineError::Execution(ExecutionError::Failed(result))) if result.code == Some(2)
        ));
    }
}
