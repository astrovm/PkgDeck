//! Firmware is queried as devices and updated through fwupd's native policy.
use super::*;
use serde_json::Value;

pub struct Firmware<T = NativeTransport> {
    pub transport: T,
}
impl<T: Transport> Firmware<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
    fn query(&self, command: &str, cancel: &Cancellation) -> Result<Value, EngineError> {
        let args = ["--json".into(), command.into()];
        let result = self
            .transport
            .system_manager("fwupdmgr", &args, cancel, false);
        let result = match result {
            Ok(result) if result.code == Some(2) => return Ok(serde_json::json!({"Devices": []})),
            Ok(result) => result,
            Err(ExecutionError::Failed(result)) if result.code == Some(2) => {
                return Ok(serde_json::json!({"Devices": []}))
            }
            Err(error) => return Err(error.into()),
        };
        let bytes = bytes("fwupd", result)?;
        let value = serde_json::Deserializer::from_slice(&bytes)
            .into_iter::<Value>()
            .next()
            .ok_or_else(|| invalid("fwupd", "empty response"))?
            .map_err(|error| invalid("fwupd", error))?;
        if let Some(error) = value.get("Error") {
            return Err(invalid("fwupd", error));
        }
        Ok(value)
    }
    fn inventory(&self, cancel: &Cancellation) -> Result<Vec<PackageDetails>, EngineError> {
        let devices = self.query("get-devices", cancel)?;
        let updates = self.query("get-updates", cancel)?;
        let devices = devices["Devices"]
            .as_array()
            .ok_or_else(|| invalid("fwupd", "missing Devices"))?;
        let updates = updates["Devices"]
            .as_array()
            .ok_or_else(|| invalid("fwupd", "missing updates Devices"))?;
        devices
            .iter()
            .map(|device| {
                let id = device["DeviceId"]
                    .as_str()
                    .filter(|s| valid_device(s))
                    .ok_or_else(|| invalid("fwupd", "invalid device ID"))?;
                let update = updates
                    .iter()
                    .find(|update| update["DeviceId"].as_str() == Some(id));
                let release = update
                    .and_then(|update| update["Releases"].as_array())
                    .and_then(|releases| releases.first());
                let name = device["Name"].as_str().unwrap_or(id);
                let mut requirements = Vec::new();
                let flags = device["Flags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .chain(
                        update
                            .and_then(|value| value["Flags"].as_array())
                            .into_iter()
                            .flatten(),
                    )
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>();
                for (flag, label) in [
                    ("require-ac", "AC power required"),
                    ("needs-reboot", "Restart required"),
                    ("needs-shutdown", "Shutdown required"),
                ] {
                    if flags.contains(&flag) {
                        requirements.push(label);
                    }
                }
                let summary = std::iter::once("Firmware")
                    .chain(requirements)
                    .collect::<Vec<_>>()
                    .join(", ");
                let description = [
                    device["Summary"].as_str().unwrap_or(""),
                    device["UpdateError"].as_str().unwrap_or(""),
                    release
                        .and_then(|r| r["Description"].as_str())
                        .unwrap_or(""),
                    &summary,
                ]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
                Ok(PackageDetails {
                    package: Package {
                        id: PackageId {
                            backend: "fwupd".into(),
                            name: id.into(),
                            architecture: "device".into(),
                            scope: Scope::System,
                            remote: None,
                            reference: None,
                        },
                        display_name: name.into(),
                        summary,
                        installed_version: Some(device["Version"].as_str().unwrap_or("").into()),
                        candidate_version: release
                            .and_then(|r| r["Version"].as_str())
                            .map(str::to_owned),
                        update: if release.is_some() {
                            UpdateAvailability::Available
                        } else {
                            UpdateAvailability::Current
                        },
                        icon: None,
                        component_ids: vec![],
                        homepages: vec![],
                    },
                    description,
                    homepage: None,
                    dependencies: vec![],
                })
            })
            .collect()
    }
    fn write(
        &self,
        command: &str,
        id: Option<&str>,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        let mut args: Vec<OsString> = [
            "--assume-yes",
            "--no-reboot-check",
            "--no-unreported-check",
            command,
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        if let Some(id) = id {
            args.push(id.into());
        }
        let result = self
            .transport
            .system_manager("fwupdmgr", &args, cancel, true);
        let result = match result {
            Ok(result) | Err(ExecutionError::Failed(result))
                if result.code == Some(2) && command == "refresh" =>
            {
                progress(Progress::Message("No firmware metadata changes.".into()));
                return Ok(OperationOutcome {
                    cancellation_deferred: result.cancellation_deferred,
                });
            }
            result => result?,
        };
        let deferred = result.cancellation_deferred;
        let output = bytes("fwupd", result)?;
        if !output.is_empty() {
            progress(Progress::Message(String::from_utf8_lossy(&output).into()));
        }
        progress(Progress::Message(
            "Firmware update finished. Restart or shut down if the messages above say so.".into(),
        ));
        Ok(OperationOutcome {
            cancellation_deferred: deferred,
        })
    }
}
fn valid_device(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
impl<T: Transport> Backend for Firmware<T> {
    fn id(&self) -> &str {
        "fwupd"
    }
    fn capabilities(&self) -> &[Capability] {
        &[
            Capability::Search,
            Capability::Installed,
            Capability::Details,
            Capability::Refresh,
            Capability::Upgrade,
        ]
    }
    fn detect(&mut self, cancel: &Cancellation) -> Result<Availability, EngineError> {
        availability(self.transport.system_manager(
            "fwupdmgr",
            &["--version".into()],
            cancel,
            false,
        ))
    }
    fn search(&mut self, _: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(vec![])
    }
    fn installed(&mut self, cancel: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self
            .inventory(cancel)?
            .into_iter()
            .map(|d| d.package)
            .collect())
    }
    fn details(
        &mut self,
        id: &PackageId,
        cancel: &Cancellation,
    ) -> Result<PackageDetails, EngineError> {
        self.inventory(cancel)?
            .into_iter()
            .find(|d| d.package.id == *id)
            .ok_or(EngineError::NotFound)
    }
    fn execute(
        &mut self,
        operation: &Operation,
        cancel: &Cancellation,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        match operation {
            Operation::Refresh { backend } if backend == "fwupd" => {
                self.write("refresh", None, cancel, progress)
            }
            Operation::Upgrade(id) if id.backend == "fwupd" && valid_device(&id.name) => {
                let details = self.details(id, cancel)?;
                if details.package.update != UpdateAvailability::Available {
                    return Err(EngineError::NotFound);
                }
                progress(Progress::Message(details.package.summary));
                self.write("update", Some(&id.name), cancel, progress)
            }
            Operation::UpgradeAll { backend } if backend == "fwupd" => {
                let updates = self.installed(cancel)?;
                let available = updates
                    .into_iter()
                    .filter(|p| p.update == UpdateAvailability::Available)
                    .collect::<Vec<_>>();
                if available.is_empty() {
                    return Ok(OperationOutcome::default());
                }
                for package in available {
                    progress(Progress::Message(format!(
                        "{}: {}",
                        package.display_name, package.summary
                    )));
                }
                self.write("update", None, cancel, progress)
            }
            _ => Err(self.unsupported(operation.capability())),
        }
    }
}
