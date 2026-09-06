//! Converger interface and the no-op implementation used by the dummy stub.

use crate::{
    config::{DummyStatus, PhaseDefinition},
    state::LifecyclePhase,
};
use std::io::Write;
use thiserror::Error;

pub trait Converger {
    fn run(
        &self,
        scenario_path: &str,
        phase: LifecyclePhase,
        definition: &PhaseDefinition,
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
