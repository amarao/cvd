//! Verifier interface and the no-op implementation used by the stub.

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
