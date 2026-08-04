//! Verifier interface and the no-op implementation used by the stub.

use thiserror::Error;

use crate::state::VerifierStatus;

/// Runs one named verifier invocation in a scenario.
pub trait Verifier {
    fn verify(
        &self,
        scenario_path: &str,
        suite_name: &str,
    ) -> Result<VerifierStatus, VerifierError>;
}

/// The sole verifier in the dummy stub.  It runs no tests and passes.
#[derive(Debug, Default)]
pub struct DummyVerifier;

impl Verifier for DummyVerifier {
    fn verify(
        &self,
        _scenario_path: &str,
        _suite_name: &str,
    ) -> Result<VerifierStatus, VerifierError> {
        Ok(VerifierStatus::Pass)
    }
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct VerifierError(pub String);
