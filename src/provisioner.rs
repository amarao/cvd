//! Provisioner interface and the mock implementation used by the dummy stub.

use crate::{
    config::{DummyStatus, PhaseDefinition},
    state::{LifecyclePhase, Resource, ResourceLocation, ResourceManifest},
};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub trait Provisioner {
    fn create(
        &self,
        scenario_path: &str,
        definition: &PhaseDefinition,
    ) -> Result<ResourceManifest, ProvisionerError>;

    fn destroy(
        &self,
        scenario_path: &str,
        resources: &ResourceManifest,
        definition: &PhaseDefinition,
    ) -> Result<(), ProvisionerError>;
}

/// Creates one fixed mock resource and pretends to destroy it.
#[derive(Debug, Default)]
pub struct DummyProvisioner;

impl Provisioner for DummyProvisioner {
    fn create(
        &self,
        scenario_path: &str,
        definition: &PhaseDefinition,
    ) -> Result<ResourceManifest, ProvisionerError> {
        if definition.dummy_status() == DummyStatus::Error {
            return Err(ProvisionerError(format!(
                "dummy create error for `{scenario_path}`"
            )));
        }
        Ok(ResourceManifest {
            resources: vec![Resource {
                id: "mock".to_owned(),
                resource_type: "mock".to_owned(),
                exists: true,
                created: ResourceLocation {
                    scenario_path: scenario_path.to_owned(),
                    phase: LifecyclePhase::Create,
                },
                destroyed: None,
                attributes: BTreeMap::new(),
                relationships: BTreeSet::new(),
                sensitive_attributes: BTreeSet::new(),
            }],
        })
    }

    fn destroy(
        &self,
        _scenario_path: &str,
        _resources: &ResourceManifest,
        definition: &PhaseDefinition,
    ) -> Result<(), ProvisionerError> {
        match definition.dummy_status() {
            DummyStatus::Ok => Ok(()),
            DummyStatus::Error => Err(ProvisionerError("dummy destroy error".to_owned())),
            DummyStatus::Fail => unreachable!("phase status is validated"),
        }
    }
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct ProvisionerError(pub String);
