mod cli;
mod config;
mod lifecycle;
mod provisioner;
mod state;
mod verifier;

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io,
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use thiserror::Error;

use crate::{
    cli::{Cli, Command, RunArgs},
    config::Config,
    lifecycle::{LifecycleError, LifecycleRunner},
    provisioner::DummyProvisioner,
    state::{RunState, StateError, StateStore, default_state_path},
    verifier::DummyVerifier,
};

#[derive(Debug, Error)]
enum AppError {
    #[error(transparent)]
    Configuration(#[from] config::ConfigError),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error("cannot resolve configuration `{path}`: {source}")]
    Canonicalize {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot read configuration `{path}`: {source}")]
    ReadConfiguration {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("previous run was interrupted while `{scenario}` was in {phase}")]
    Interrupted { scenario: String, phase: String },
    #[error("run finished with {errors} error(s) and {failures} verifier failure(s)")]
    FailedRun { errors: usize, failures: usize },
}

fn main() {
    if let Err(error) = run_cli() {
        eprintln!("cvd: {error}");
        process::exit(1);
    }
}

fn run_cli() -> Result<(), AppError> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => run(args),
    }
}

fn run(args: RunArgs) -> Result<(), AppError> {
    let configuration_path =
        fs::canonicalize(&args.file).map_err(|source| AppError::Canonicalize {
            path: args.file.clone(),
            source,
        })?;
    let configuration_text =
        fs::read_to_string(&configuration_path).map_err(|source| AppError::ReadConfiguration {
            path: configuration_path.clone(),
            source,
        })?;
    let configuration = Config::from_yaml(&configuration_text)?;
    let state_path = args
        .state
        .unwrap_or_else(|| default_state_path(&configuration_path));
    let store = StateStore::new(state_path);

    // Validate a prior state before replacing it.  A running phase is never
    // retried implicitly: this command reports the interruption and leaves the
    // state available for inspection instead.
    if store.path().exists() {
        let prior = store.load()?;
        if let Some((scenario, phase)) = prior.scenarios.iter().find_map(|(path, scenario)| {
            scenario
                .phases
                .iter()
                .find(|(_, state)| state.status == state::PhaseStatus::Running)
                .map(|(phase, _)| (path.clone(), format!("{phase:?}").to_lowercase()))
        }) {
            return Err(AppError::Interrupted { scenario, phase });
        }
    }

    if let Some(selector) = args.scenario.as_deref()
        && configuration.scenario(selector).is_none()
    {
        return Err(LifecycleError::InvalidSelector(selector.to_owned()).into());
    }
    let state = RunState::new(
        run_identifier(),
        configuration_path,
        fingerprint(&configuration_text),
        args.scenario.clone(),
        args.keep,
    );
    let provisioner = DummyProvisioner;
    let verifier = DummyVerifier;
    let (outcome, _) = LifecycleRunner::new(
        &configuration,
        &store,
        state,
        &provisioner,
        &verifier,
        io::stdout(),
    )
    .run(args.scenario.as_deref())?;

    if outcome.succeeded() {
        Ok(())
    } else {
        Err(AppError::FailedRun {
            errors: outcome.execution_errors,
            failures: outcome.verifier_failures,
        })
    }
}

fn fingerprint(input: &str) -> String {
    // The standard library's hasher is sufficient for a stub fingerprint: it
    // distinguishes the exact loaded configuration without introducing a
    // cryptography dependency or claiming a public digest contract.
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("stub-hash-{:016x}", hasher.finish())
}

fn run_identifier() -> String {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("run-{}-{milliseconds}", process::id())
}

#[cfg(test)]
mod tests {
    use super::fingerprint;

    #[test]
    fn configuration_fingerprint_changes_with_content() {
        assert_ne!(fingerprint("version: 1"), fingerprint("version: 2"));
    }
}
