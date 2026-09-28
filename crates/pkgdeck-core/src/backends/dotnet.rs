//! Placeholder, replaced by the real adapter.
use super::{NativeTransport, Transport};
use crate::{engine::*, package::*, process::*};

pub struct DotnetTools<T = NativeTransport> {
    #[allow(dead_code)]
    transport: T,
}

impl<T: Transport> DotnetTools<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T: Transport> Backend for DotnetTools<T> {
    fn id(&self) -> &str {
        "dotnet"
    }
    fn capabilities(&self) -> &[Capability] {
        &[]
    }
    fn detect(&mut self, _: &Cancellation) -> Result<Availability, EngineError> {
        Ok(Availability::Unavailable("not implemented yet".into()))
    }
}
