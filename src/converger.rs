//! Converger interfaces and implementations.

use std::{
    env, fs,
    io::Write,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use thiserror::Error;

use crate::{config::PhaseDefinition, state::LifecyclePhase};

static NEXT_INVENTORY_FILE: AtomicU64 = AtomicU64::new(0);

/// Executes state-changing scenario phases other than resource provisioning.
pub trait Converger {
    fn run(
        &self,
        scenario_path: &str,
        phase: LifecyclePhase,
        definition: &PhaseDefinition,
        inventory: Option<&serde_json::Value>,
        output: &mut dyn Write,
        styled_output: bool,
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
        _inventory: Option<&serde_json::Value>,
        _output: &mut dyn Write,
        _styled_output: bool,
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
        inventory: Option<&serde_json::Value>,
        output: &mut dyn Write,
        styled_output: bool,
    ) -> Result<(), ConvergerError> {
        let inventory_path = inventory.map(write_inventory).transpose()?;
        if let Some(path) = &inventory_path {
            let line = format!("ANSIBLE_INVENTORY={}", path.display());
            if styled_output {
                writeln!(output, "\x1b[90m{line}\x1b[0m")
            } else {
                writeln!(output, "{line}")
            }
            .map_err(|error| {
                ConvergerError(format!("could not report Ansible inventory: {error}"))
            })?;
        }
        let result = (|| {
            for playbook in definition.ansible_playbooks() {
                writeln!(output, "ansible-playbook {}", playbook.display()).map_err(|error| {
                    ConvergerError(format!("could not report Ansible command: {error}"))
                })?;
                let mut command = Command::new("ansible-playbook");
                command.arg(playbook).current_dir(&self.working_directory);
                if let Some(path) = &inventory_path {
                    command.env("ANSIBLE_INVENTORY", path);
                }
                let status = command.status().map_err(|error| {
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
        })();
        if let Some(path) = inventory_path {
            let _ = fs::remove_file(path);
        }
        result
    }
}

fn write_inventory(inventory: &serde_json::Value) -> Result<PathBuf, ConvergerError> {
    let sequence = NEXT_INVENTORY_FILE.fetch_add(1, Ordering::Relaxed);
    let path = env::temp_dir().join(format!(
        "cvd-ansible-inventory-{}-{sequence}.json",
        std::process::id()
    ));
    let encoded = serde_json::to_vec_pretty(inventory)
        .map_err(|error| ConvergerError(format!("could not encode Ansible inventory: {error}")))?;
    fs::write(&path, encoded).map_err(|error| {
        ConvergerError(format!(
            "could not write Ansible inventory `{}`: {error}",
            path.display()
        ))
    })?;
    Ok(path)
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct ConvergerError(pub String);
