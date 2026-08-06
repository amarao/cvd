//! Provisioner interface and the no-op implementation used by the stub.

use thiserror::Error;

use std::collections::{BTreeMap, BTreeSet};

use crate::state::{LifecyclePhase, Resource, ResourceLocation, ResourceManifest};

/// Creates and destroys resources owned by one scenario.
pub trait Provisioner {
    fn create(&self, scenario_path: &str) -> Result<ResourceManifest, ProvisionerError>;

    fn destroy(
        &self,
        scenario_path: &str,
        resources: &ResourceManifest,
    ) -> Result<(), ProvisionerError>;
}

/// The sole provisioner in the dummy stub. It models one loopback resource and
/// makes no external calls.
#[derive(Debug, Default)]
pub struct DummyProvisioner;

impl Provisioner for DummyProvisioner {
    fn create(&self, scenario_path: &str) -> Result<ResourceManifest, ProvisionerError> {
        Ok(ResourceManifest {
            resources: vec![Resource {
                id: "dummy".to_owned(),
                resource_type: "dummy".to_owned(),
                exists: true,
                created: ResourceLocation {
                    scenario_path: scenario_path.to_owned(),
                    phase: LifecyclePhase::Create,
                },
                destroyed: None,
                attributes: BTreeMap::from([(
                    "ipv6".to_owned(),
                    serde_json::Value::String("::1".to_owned()),
                )]),
                relationships: BTreeSet::new(),
                sensitive_attributes: BTreeSet::new(),
            }],
        })
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
