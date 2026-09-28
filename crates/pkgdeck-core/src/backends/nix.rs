//! The user's Nix profile (`nix profile`), never NixOS or Home Manager
//! configuration.
//!
//! Rows come from the version 3 profile JSON. Versions are read from store
//! paths for display only: whether an upgrade exists depends on evaluating
//! the flake, so update status stays unknown instead of being guessed.
use super::{bytes, invalid, NativeTransport, Transport};
use crate::{engine::*, package::*, process::*};
use serde::Deserialize;
use std::{collections::BTreeMap, ffi::OsString};

const ID: &str = "nix";
const CAPABILITIES: &[Capability] = &[
    Capability::Search,
    Capability::Details,
    Capability::Installed,
    Capability::Install,
    Capability::Remove,
    Capability::Upgrade,
];
/// Profile commands are still behind these features in official Nix.
const FEATURES: [&str; 2] = ["--extra-experimental-features", "nix-command flakes"];

pub struct Nix<T = NativeTransport> {
    transport: T,
}

#[derive(Deserialize)]
struct Profile {
    version: u32,
    #[serde(default)]
    elements: BTreeMap<String, Element>,
}
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Element {
    #[serde(default = "active")]
    active: bool,
    #[serde(default)]
    attr_path: Option<String>,
    #[serde(default)]
    original_url: Option<String>,
    #[serde(default)]
    store_paths: Vec<String>,
}
fn active() -> bool {
    true
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}

/// `/nix/store/<hash>-curl-8.20.0-bin` → `8.20.0`: the first dash-separated
/// part that starts with a digit, up to a known output suffix.
fn store_version(path: &str) -> Option<String> {
    let base = path.rsplit('/').next()?;
    let (_, rest) = base.split_once('-')?;
    let parts: Vec<&str> = rest.split('-').collect();
    let start = parts
        .iter()
        .position(|part| part.starts_with(|c: char| c.is_ascii_digit()))?;
    let mut end = parts.len();
    while end > start + 1
        && ["bin", "out", "dev", "lib", "man", "doc", "info"].contains(&parts[end - 1])
    {
        end -= 1;
    }
    Some(parts[start..end].join("-"))
}

impl<T: Transport> Nix<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    fn call(
        &self,
        args: &[&str],
        cancel: &Cancellation,
        write: bool,
    ) -> Result<Completion, ExecutionError> {
        let args: Vec<OsString> = FEATURES.iter().chain(args).map(OsString::from).collect();
        self.transport.dev_tool(ID, &args, cancel, write)
    }

    fn profile(&self, cancel: &Cancellation) -> Result<BTreeMap<String, Element>, EngineError> {
        let output = bytes(
            ID,
            self.call(&["profile", "list", "--json"], cancel, false)?,
        )?;
        let profile: Profile =
            serde_json::from_slice(&output).map_err(|error| invalid(ID, error))?;
        if profile.version != 3 {
            return Err(invalid(
                ID,
                format!(
                    "profile format version {} isn't supported yet",
                    profile.version
                ),
            ));
        }
        Ok(profile
            .elements
            .into_iter()
            .filter(|(name, element)| element.active && valid_name(name))
            .collect())
    }

    fn package(name: &str, element: &Element) -> Package {
        let source = match (&element.original_url, &element.attr_path) {
            (Some(url), Some(attr)) => format!("{url}#{}", attr.rsplit('.').next().unwrap_or(attr)),
            (Some(url), None) => url.clone(),
            _ => "a store path (can't be upgraded)".into(),
        };
        Package {
            id: PackageId {
                backend: ID.into(),
                name: name.into(),
                architecture: "unknown".into(),
                scope: Scope::User {
                    uid: rustix::process::getuid().as_raw(),
                },
                remote: None,
                reference: element.attr_path.clone(),
            },
            display_name: name.into(),
            summary: format!("Nix profile · from {source}"),
            installed_version: Some(
                element
                    .store_paths
                    .first()
                    .and_then(|path| store_version(path))
                    .unwrap_or_else(|| "installed".into()),
            ),
            candidate_version: None,
            update: UpdateAvailability::Unknown,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
        }
    }

    fn write(&self, args: &[&str], cancel: &Cancellation) -> Result<OperationOutcome, EngineError> {
        let completion = self.call(args, cancel, true)?;
        let deferred = completion.cancellation_deferred;
        bytes(ID, completion)?;
        Ok(OperationOutcome {
            cancellation_deferred: deferred,
        })
    }
}

impl<T: Transport> Backend for Nix<T> {
    fn id(&self) -> &str {
        ID
    }
    fn capabilities(&self) -> &[Capability] {
        CAPABILITIES
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        match self
            .transport
            .dev_tool(ID, &["--version".into()], cancel, false)
        {
            Ok(_) => Ok(Availability::Available),
            Err(ExecutionError::Disabled(reason)) => Ok(Availability::Unavailable(reason)),
            Err(ExecutionError::Cancelled) => Err(EngineError::Cancelled),
            Err(error) => Err(error.into()),
        }
    }
    fn may_have(&self, name: &str) -> bool {
        valid_name(name)
    }
    fn search(&mut self, query: &str, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        let query = query.to_lowercase();
        let installed = self.profile(cancel)?;
        let mut rows: Vec<Package> = installed
            .iter()
            .filter(|(name, _)| name.to_lowercase().contains(&query))
            .map(|(name, element)| Self::package(name, element))
            .collect();
        // Searching nixpkgs evaluates all of it (minutes); offer the exact
        // attribute instead, which the install then confirms.
        if valid_name(&query) && !installed.contains_key(&query) {
            let mut offer = Self::package(
                &query,
                &Element {
                    active: true,
                    attr_path: None,
                    original_url: Some("flake:nixpkgs".into()),
                    store_paths: vec![],
                },
            );
            offer.summary = format!("Install nixpkgs#{query} into your Nix profile");
            offer.installed_version = None;
            rows.push(offer);
        }
        Ok(rows)
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .profile(cancel)?
            .iter()
            .map(|(name, element)| Self::package(name, element))
            .collect())
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        let profile = self.profile(cancel)?;
        let element = profile
            .get(&id.name)
            .filter(|_| id.backend == ID)
            .ok_or(EngineError::NotFound)?;
        let mut description = vec![Self::package(&id.name, element).summary];
        description.extend(
            element
                .store_paths
                .iter()
                .map(|path| format!("Store path: {path}")),
        );
        description.push(if element.original_url.is_some() {
            "Upgrading re-evaluates its flake; PkgDeck can't tell in advance whether that changes anything.".into()
        } else {
            "Installed from a store path, so there is nothing to upgrade from.".into()
        });
        Ok(PackageDetails {
            package: Self::package(&id.name, element),
            description: description.join("\n\n"),
            homepage: None,
            dependencies: vec![],
        })
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let id = match operation {
            Operation::Install(id) | Operation::Remove(id) | Operation::Upgrade(id) => id,
            other => return Err(self.unsupported(other.capability())),
        };
        if id.backend != ID || !valid_name(&id.name) {
            return Err(invalid(ID, "foreign or invalid profile element"));
        }
        let before = self.profile(cancel)?;
        let outcome = match operation {
            Operation::Install(_) => {
                if before.contains_key(&id.name) {
                    return Ok(OperationOutcome::default());
                }
                let flake = format!("nixpkgs#{}", id.name);
                progress(Progress::Message(format!(
                    "Adding {flake} to your Nix profile"
                )));
                // `add` replaced `install` in Nix 2.25; older Nix only has install.
                match self.write(&["profile", "add", &flake], cancel) {
                    Err(EngineError::Execution(ExecutionError::Failed(result)))
                        if String::from_utf8_lossy(&result.stderr).contains("'add'") =>
                    {
                        self.write(&["profile", "install", &flake], cancel)?
                    }
                    result => result?,
                }
            }
            Operation::Remove(_) => {
                if !before.contains_key(&id.name) {
                    return Err(EngineError::NotFound);
                }
                progress(Progress::Message(format!(
                    "Removing {} from your Nix profile",
                    id.name
                )));
                self.write(&["profile", "remove", &id.name], cancel)?
            }
            _ => {
                let element = before.get(&id.name).ok_or(EngineError::NotFound)?;
                if element.original_url.is_none() {
                    return Err(invalid(ID, format!("{} was installed from a store path, so there is nothing to upgrade from", id.name)));
                }
                progress(Progress::Message(format!(
                    "Upgrading {} from its flake",
                    id.name
                )));
                self.write(&["profile", "upgrade", &id.name], cancel)?
            }
        };
        // Writes may finish after cancellation; verify with a fresh read.
        let after = self.profile(&Cancellation::default())?;
        let present = after.contains_key(&id.name);
        if matches!(operation, Operation::Remove(_)) == present {
            return Err(invalid(
                ID,
                format!("Nix finished, but {} is not in the expected state", id.name),
            ));
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::AptAction;
    use std::sync::{Arc, Mutex};

    #[test]
    fn versions_come_from_store_paths() {
        for (path, version) in [
            (
                "/nix/store/kwhxkl8yn5y8wqiq11jsybagw3fbc4iv-hello-2.12.3",
                Some("2.12.3"),
            ),
            (
                "/nix/store/8gdgwydsf6gia9j178nymxwm2bl0z3m3-curl-8.20.0-bin",
                Some("8.20.0"),
            ),
            (
                "/nix/store/q1nq6fylbczrsj7k9bqc9dzf660q63v3-bash-interactive-5.3p9",
                Some("5.3p9"),
            ),
            (
                "/nix/store/bjwcdygzs2gsbxqh9jyisisafqw7xlpd-iana-etc-20251215",
                Some("20251215"),
            ),
            ("/nix/store/aaaa-no-version-here", None),
        ] {
            assert_eq!(store_version(path).as_deref(), version, "{path}");
        }
    }

    #[derive(Clone, Default)]
    struct Fake {
        profile: Arc<Mutex<String>>,
        calls: Arc<Mutex<Vec<String>>>,
        old_nix: bool,
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
    const HELLO: &str = r#""hello":{"active":true,"attrPath":"legacyPackages.aarch64-linux.hello","originalUrl":"flake:nixpkgs","outputs":null,"priority":5,"storePaths":["/nix/store/kwhxkl8yn5y8wqiq11jsybagw3fbc4iv-hello-2.12.3"],"url":"https://releases.nixos.org/nixpkgs/x/nixexprs.tar.zst"}"#;
    const CURL: &str = r#""curl":{"active":true,"priority":5,"storePaths":["/nix/store/8gdgwydsf6gia9j178nymxwm2bl0z3m3-curl-8.20.0-bin"]}"#;
    impl Transport for Fake {
        fn apt_query(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &Cancellation,
        ) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
        fn apt_write(&self, _: AptAction, _: &Cancellation) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
        fn brew(
            &self,
            _: &[OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
        fn flatpak(
            &self,
            _: &[OsString],
            _: &Cancellation,
            _: bool,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            unreachable!()
        }
        fn dev_tool(
            &self,
            executable: &str,
            args: &[OsString],
            _: &Cancellation,
            _: bool,
        ) -> Result<Completion, ExecutionError> {
            assert_eq!(executable, "nix");
            let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
            if args[0] == "--version" {
                return Ok(done("nix (Nix) 2.35.2"));
            }
            assert_eq!(&args[..2], &FEATURES.map(String::from));
            let line = args[2..].join(" ");
            self.calls.lock().unwrap().push(line.clone());
            let mut profile = self.profile.lock().unwrap();
            match line.as_str() {
                "profile list --json" => Ok(done(&format!(
                    r#"{{"elements":{{{profile}}},"version":3}}"#
                ))),
                "profile add nixpkgs#hello" if self.old_nix => {
                    Err(ExecutionError::Failed(Completion {
                        code: Some(1),
                        stderr: b"error: 'add' is not a recognised command".to_vec(),
                        ..done("")
                    }))
                }
                "profile add nixpkgs#hello" | "profile install nixpkgs#hello" => {
                    *profile = format!("{profile},{HELLO}");
                    Ok(done(""))
                }
                "profile remove hello" => {
                    *profile = CURL.into();
                    Ok(done(""))
                }
                "profile upgrade hello" => Ok(done("")),
                other => panic!("unexpected {other}"),
            }
        }
    }

    #[test]
    fn profile_rows_never_guess_updates() {
        let fake = Fake {
            profile: Arc::new(Mutex::new(format!("{CURL},{HELLO}"))),
            ..Fake::default()
        };
        let mut nix = Nix::new(fake.clone());
        let rows = nix.installed(&Cancellation::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|row| row.update == UpdateAvailability::Unknown));
        assert_eq!(rows[1].summary, "Nix profile · from flake:nixpkgs#hello");
        assert_eq!(
            rows[0].summary,
            "Nix profile · from a store path (can't be upgraded)"
        );
        assert_eq!(rows[0].installed_version.as_deref(), Some("8.20.0"));
        nix.execute(
            &Operation::Upgrade(rows[1].id.clone()),
            &Cancellation::default(),
            &mut |_| {},
        )
        .unwrap();
        let error = nix
            .execute(
                &Operation::Upgrade(rows[0].id.clone()),
                &Cancellation::default(),
                &mut |_| {},
            )
            .unwrap_err();
        assert!(error.to_string().contains("store path"), "{error}");
        nix.execute(
            &Operation::Remove(rows[1].id.clone()),
            &Cancellation::default(),
            &mut |_| {},
        )
        .unwrap();
        assert!(!fake.profile.lock().unwrap().contains("hello"));
    }

    #[test]
    fn installs_offer_the_exact_attribute_and_work_on_older_nix() {
        for old_nix in [false, true] {
            let fake = Fake {
                profile: Arc::new(Mutex::new(CURL.into())),
                old_nix,
                ..Fake::default()
            };
            let mut nix = Nix::new(fake.clone());
            let offers = nix.search("hello", &Cancellation::default()).unwrap();
            assert_eq!(offers.len(), 1);
            assert!(unverified_search_offer(&offers[0]));
            nix.execute(
                &Operation::Install(offers[0].id.clone()),
                &Cancellation::default(),
                &mut |_| {},
            )
            .unwrap();
            assert!(fake.profile.lock().unwrap().contains("hello"));
            assert_eq!(
                fake.calls
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|call| call == "profile install nixpkgs#hello"),
                old_nix
            );
        }
    }

    #[test]
    fn other_profile_formats_are_refused() {
        let fake = Fake::default();
        *fake.profile.lock().unwrap() = String::new();
        let mut nix = Nix::new(fake);
        assert!(nix.installed(&Cancellation::default()).unwrap().is_empty());
        // Nix 2.19 and older wrote version 2, a list without names.
        let old: Profile = serde_json::from_str(r#"{"version":2}"#).unwrap();
        assert_eq!(old.version, 2);
    }
}
