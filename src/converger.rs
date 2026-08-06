//! Converger interfaces and implementations.

use std::{path::PathBuf, process::Command};

use thiserror::Error;

use crate::{config::PhaseDefinition, state::LifecyclePhase};

/// Executes state-changing scenario phases other than resource provisioning.
pub trait Converger {
    fn run(
        &self,
        scenario_path: &str,
        phase: LifecyclePhase,
        definition: &PhaseDefinition,
    ) -> Result<(), ConvergerError>;
}

#[derive(Debug, Default)]
#[cfg(test)]
pub struct DummyConverger;

#[cfg(test)]
impl Converger for DummyConverger {
    fn run(
        &self,
        _scenario_path: &str,
        _phase: LifecyclePhase,
        _definition: &PhaseDefinition,
    ) -> Result<(), ConvergerError> {
        Ok(())
    }
}

/// Runs resolved playbooks from the directory containing the root CVD file.
#[derive(Debug)]
pub struct AnsibleConverger {
    working_directory: PathBuf,
}

impl AnsibleConverger {
    pub fn new(working_directory: impl Into<PathBuf>) -> Self {
        Self {
            working_directory: working_directory.into(),
        }
    }
}

impl Converger for AnsibleConverger {
    fn run(
        &self,
        _scenario_path: &str,
        _phase: LifecyclePhase,
        definition: &PhaseDefinition,
    ) -> Result<(), ConvergerError> {
        for playbook in definition.ansible_playbooks() {
            let status = Command::new("ansible-playbook")
                .arg(playbook)
                .current_dir(&self.working_directory)
                .status()
                .map_err(|error| {
                    ConvergerError(format!(
                        "could not run `ansible-playbook {}` from `{}`: {error}",
                        playbook.display(),
                        self.working_directory.display()
                    ))
                })?;
            if !status.success() {
                return Err(ConvergerError(format!(
                    "`ansible-playbook {}` exited with {status}",
                    playbook.display()
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct ConvergerError(pub String);
