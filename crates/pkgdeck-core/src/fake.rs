//! A made-up source for tests: it answers with what it was given and
//! remembers what it ran. Tests that need a source to behave in one
//! particular way keep a fixture of their own.
use crate::{
    engine::{Backend, EngineError},
    package::*,
    process::Cancellation,
};
use std::sync::{Arc, Mutex};

pub struct Fake {
    id: String,
    capabilities: Vec<Capability>,
    availability: Availability,
    packages: Vec<Package>,
    cleanup: Vec<CleanupItem>,
    answer: Result<OperationOutcome, EngineError>,
    /// Each operation it ran, in order.
    pub ran: Arc<Mutex<Vec<Operation>>>,
}
impl Fake {
    /// An available source that can do `capabilities`, lists nothing yet,
    /// and finishes every change it gets.
    pub fn new(id: &str, capabilities: &[Capability]) -> Self {
        Self {
            id: id.into(),
            capabilities: capabilities.to_vec(),
            availability: Availability::Available,
            packages: vec![],
            cleanup: vec![],
            answer: Ok(OperationOutcome::default()),
            ran: Arc::default(),
        }
    }
    /// What its check finds instead of an available source.
    pub fn unavailable(mut self, reason: &str) -> Self {
        self.availability = Availability::Unavailable(reason.into());
        self
    }
    /// The packages it finds and has installed.
    pub fn listing(mut self, packages: Vec<Package>) -> Self {
        self.packages = packages;
        self
    }
    pub fn cleaning(mut self, items: Vec<CleanupItem>) -> Self {
        self.cleanup = items;
        self
    }
    /// What every change answers.
    pub fn answering(mut self, answer: Result<OperationOutcome, EngineError>) -> Self {
        self.answer = answer;
        self
    }
}
impl Backend for Fake {
    fn id(&self) -> &str {
        &self.id
    }
    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(self.availability.clone())
    }
    fn search(&mut self, _: &str, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self.packages.clone())
    }
    fn installed(&mut self, _: &Cancellation) -> Result<Vec<Package>, EngineError> {
        Ok(self.packages.clone())
    }
    fn cleanup(&mut self, _: &Cancellation) -> Result<Vec<CleanupItem>, EngineError> {
        Ok(self.cleanup.clone())
    }
    fn execute(
        &mut self,
        operation: &Operation,
        _: &Cancellation,
        _: &mut dyn FnMut(Progress),
    ) -> Result<OperationOutcome, EngineError> {
        self.ran.lock().unwrap().push(operation.clone());
        self.answer.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_answers_with_what_it_was_given() {
        let cancel = Cancellation::default();
        let package = Package {
            id: PackageId {
                backend: "fake".into(),
                name: "tool".into(),
                architecture: "all".into(),
                scope: Scope::System,
                remote: None,
                reference: None,
            },
            display_name: "Tool".into(),
            summary: String::new(),
            installed_version: Some("1".into()),
            candidate_version: None,
            update: UpdateAvailability::Current,
            icon: None,
            component_ids: vec![],
            homepages: vec![],
            adopt_with: None,
        };
        let task = CleanupItem {
            id: CleanupId {
                backend: "fake".into(),
                key: "cache".into(),
            },
            kind: CleanupKind::PackageCache,
            title: "Cache".into(),
            summary: String::new(),
            preview: String::new(),
            bytes: None,
            restorable: false,
        };
        let mut fake = Fake::new("fake", &[Capability::Search])
            .listing(vec![package.clone()])
            .cleaning(vec![task.clone()])
            .answering(Err(EngineError::NotFound));
        assert_eq!(fake.id(), "fake");
        assert_eq!(fake.capabilities(), [Capability::Search]);
        assert_eq!(fake.detect(&cancel), Ok(Availability::Available));
        assert_eq!(fake.search("x", &cancel), Ok(vec![package.clone()]));
        assert_eq!(fake.installed(&cancel), Ok(vec![package.clone()]));
        assert_eq!(fake.cleanup(&cancel), Ok(vec![task]));
        let remove = Operation::Remove(package.id);
        assert_eq!(
            fake.execute(&remove, &cancel, &mut |_| {}),
            Err(EngineError::NotFound)
        );
        assert_eq!(*fake.ran.lock().unwrap(), [remove]);
        let mut gone = Fake::new("gone", &[]).unavailable("not found");
        assert_eq!(
            gone.detect(&cancel),
            Ok(Availability::Unavailable("not found".into()))
        );
    }
}
