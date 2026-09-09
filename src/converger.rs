//! Converger interface and the no-op implementation used by the dummy stub.

use crate::{
    config::{DummyStatus, PhaseDefinition},
    state::LifecyclePhase,
};
use std::{io::Write, path::Path};
use thiserror::Error;

pub trait Converger {
    fn run(
        &self,
        scenario_path: &str,
        phase: LifecyclePhase,
        definition: &PhaseDefinition,
        inventory: Option<&Path>,
        output: &mut dyn Write,
        styled_output: bool,
    ) -> Result<(), ConvergerError>;
}

/// Pretends that every enabled converger phase succeeds.
#[derive(Debug, Default)]
pub struct DummyConverger;

impl Converger for DummyConverger {
    fn run(
        &self,
        scenario_path: &str,
        phase: LifecyclePhase,
        definition: &PhaseDefinition,
        _inventory: Option<&Path>,
        _output: &mut dyn Write,
        _styled_output: bool,
    ) -> Result<(), ConvergerError> {
        match definition.dummy_status() {
            DummyStatus::Ok => Ok(()),
            DummyStatus::Error => Err(ConvergerError(format!(
                "dummy {phase:?} error for `{scenario_path}`"
            ))),
            DummyStatus::Fail => unreachable!("phase status is validated"),
        }
    }
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct ConvergerError(pub String);

/// Ansible converger with explicit dummy overrides.
pub struct AnsibleConverger {
    pub runtime: crate::provisioner::AnsibleProvisioner,
    pub default_is_ansible: bool,
}

impl Converger for AnsibleConverger {
    fn run(
        &self,
        scenario_path: &str,
        phase: LifecyclePhase,
        definition: &PhaseDefinition,
        inventory: Option<&Path>,
        output: &mut dyn Write,
        styled_output: bool,
    ) -> Result<(), ConvergerError> {
        if let Some(ansible) = definition.ansible() {
            let action = match phase {
                LifecyclePhase::Prepare => "prepare",
                LifecyclePhase::Converge => "converge",
                LifecyclePhase::Cleanup => "cleanup",
                _ => {
                    return Err(ConvergerError(
                        "unsupported Ansible converger phase".to_owned(),
                    ));
                }
            };
            self.runtime
                .converge(scenario_path, action, ansible, inventory)
                .map_err(|error| ConvergerError(error.to_string()))
        } else if self.default_is_ansible && !definition.is_dummy_override() {
            Err(ConvergerError(
                "the Ansible converger requires an explicit `ansible` phase mapping".to_owned(),
            ))
        } else {
            DummyConverger.run(
                scenario_path,
                phase,
                definition,
                inventory,
                output,
                styled_output,
            )
        }
    }
}
