//! Provisioner interface and the no-op implementation used by the stub.

use thiserror::Error;

use crate::state::ResourceManifest;

/// Creates and destroys resources owned by one scenario.
pub trait Provisioner {
    fn create(&self, scenario_path: &str) -> Result<ResourceManifest, ProvisionerError>;

    fn destroy(
        &self,
        scenario_path: &str,
        resources: &ResourceManifest,
    ) -> Result<(), ProvisionerError>;
}

/// The sole provisioner in the dummy stub.  It owns no resources and makes no
/// external calls.
#[derive(Debug, Default)]
pub struct DummyProvisioner;

impl Provisioner for DummyProvisioner {
    fn create(&self, _scenario_path: &str) -> Result<ResourceManifest, ProvisionerError> {
        Ok(ResourceManifest::default())
    }

    fn destroy(
        &self,
        _scenario_path: &str,
        _resources: &ResourceManifest,
    ) -> Result<(), ProvisionerError> {
        Ok(())
    }
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct ProvisionerError(pub String);
