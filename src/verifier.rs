//! Named dummy, Ansible, and pytest verifier invocations.

use std::{
    path::{Path, PathBuf},
    process::Command,
};
use thiserror::Error;

use crate::{
    config::{DummyStatus, Test},
    state::VerifierStatus,
};

/// Runs one named verifier invocation in a scenario.
pub trait Verifier {
    fn verify(
        &self,
        scenario_path: &str,
        test_name: &str,
        test: &Test,
        inventory: Option<&Path>,
        resources: &[crate::state::Resource],
    ) -> Result<VerifierStatus, VerifierError>;
}

/// Returns the configured dummy result without running an external test.
#[derive(Debug, Default)]
pub struct DummyVerifier;

impl Verifier for DummyVerifier {
    fn verify(
        &self,
        _scenario_path: &str,
        _test_name: &str,
        test: &Test,
        _inventory: Option<&Path>,
        _resources: &[crate::state::Resource],
    ) -> Result<VerifierStatus, VerifierError> {
        Ok(match test.status {
            DummyStatus::Ok => VerifierStatus::Pass,
            DummyStatus::Fail => VerifierStatus::Fail,
            DummyStatus::Error => VerifierStatus::Error,
        })
    }
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct VerifierError(pub String);

/// Pytest also runs testinfra when its plugin is installed in the selected environment.
pub struct RuntimeVerifier {
    pub default_verifier: String,
    pub ansible: crate::provisioner::AnsibleProvisioner,
    pub working_directory: PathBuf,
    pub inventory: crate::inventory::AnsibleInventory,
}

impl Verifier for RuntimeVerifier {
    fn verify(
        &self,
        scenario_path: &str,
        test_name: &str,
        test: &Test,
        overlay: Option<&Path>,
        resources: &[crate::state::Resource],
    ) -> Result<VerifierStatus, VerifierError> {
        let selected = test.verifier.as_deref().unwrap_or(&self.default_verifier);
        if selected == "ansible" {
            let definition = test
                .ansible
                .as_ref()
                .ok_or_else(|| VerifierError("ansible verifier requires a playbook".to_owned()))?;
            self.ansible
                .converge(scenario_path, "verify", definition, overlay, resources)
                .map_err(|error| {
                    VerifierError(format!(
                        "Ansible test `{scenario_path}::{test_name}`: {error}"
                    ))
                })?;
            return Ok(VerifierStatus::Pass);
        }
        if selected == "dummy" {
            return DummyVerifier.verify(scenario_path, test_name, test, overlay, resources);
        }
        let pytest = test
            .pytest
            .as_ref()
            .ok_or_else(|| VerifierError("pytest verifier requires a test path".to_owned()))?;
        self.inventory
            .validate_bindings(overlay, &self.working_directory)
            .map_err(|error| VerifierError(error.to_string()))?;
        let inventory = self
            .inventory
            .environment(overlay)
            .map_err(|error| VerifierError(error.to_string()))?;
        let status = Command::new("pytest")
            .args(&pytest.args)
            .arg(&pytest.path)
            .current_dir(&self.working_directory)
            .env("CVD_DIRECTORY", &self.working_directory)
            .env("ANSIBLE_INVENTORY", inventory)
            .status()
            .map_err(|error| {
                VerifierError(format!(
                    "cannot run pytest for `{scenario_path}::{test_name}`: {error}"
                ))
            })?;
        if status.success() {
            Ok(VerifierStatus::Pass)
        } else {
            Err(VerifierError(format!(
                "pytest for `{scenario_path}::{test_name}` exited with {status}"
            )))
        }
    }
}
