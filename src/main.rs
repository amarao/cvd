mod cli;
mod config;
mod converger;
mod lifecycle;
mod provisioner;
mod state;
mod verifier;

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::{self, IsTerminal, Write},
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use thiserror::Error;

use crate::{
    cli::{Cli, Command, RunArgs, StateReportArgs, StateResourcesArgs, StateViewArgs, ViewFormat},
    config::Config,
    converger::AnsibleConverger,
    lifecycle::{LifecycleError, LifecycleRunner, render_state_report},
    provisioner::AnsibleProvisioner,
    state::{RunState, StateError, StateRepository, StateRepositoryError, default_state_directory},
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
    #[error(transparent)]
    StateRepository(#[from] StateRepositoryError),
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
    #[error("could not render state view: {0}")]
    RenderView(#[from] serde_yaml::Error),
    #[error("could not write state view: {0}")]
    WriteView(#[source] io::Error),
    #[error("could not write state report: {0}")]
    WriteReport(#[source] io::Error),
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
        Command::StateView(args) => state_view(args),
        Command::StateResources(args) => state_resources(args),
        Command::StateReport(args) => state_report(args),
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
    let configuration = Config::from_yaml_at(&configuration_text, &configuration_path)?;
    let repository = StateRepository::new(
        args.state_dir
            .unwrap_or_else(|| default_state_directory(&configuration_path)),
    );

    if let Some(selector) = args.scenario.as_deref()
        && configuration.scenario(selector).is_none()
    {
        return Err(LifecycleError::InvalidSelector(selector.to_owned()).into());
    }
    let run_id = run_identifier();
    let state = RunState::new(
        run_id,
        configuration_path.clone(),
        fingerprint(configuration.source_material()),
        args.scenario.clone(),
        args.keep,
    );
    // Publishing `last-run` happens only after this initial state is durable,
    // making an interrupted newest run inspectable without overwriting history.
    let store = repository.start_run(&state)?;
    let working_directory = configuration_path
        .parent()
        .expect("a canonical configuration path has a parent");
    let provisioner = AnsibleProvisioner::new(working_directory);
    let converger = AnsibleConverger::new(working_directory);
    let verifier = DummyVerifier;
    let output = io::stdout();
    let styled_output =
        terminal_styling_enabled(output.is_terminal(), std::env::var_os("NO_COLOR").is_some());
    let (outcome, _) = LifecycleRunner::new(
        &configuration,
        &store,
        state,
        &provisioner,
        &converger,
        &verifier,
        output,
    )
    .with_styled_output(styled_output)
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

fn state_view(args: StateViewArgs) -> Result<(), AppError> {
    let state_directory = args
        .state_dir
        .unwrap_or_else(|| default_state_directory(&args.file));
    let repository = StateRepository::new(state_directory);
    let (_, store) = repository.open_run(&args.run)?;
    let state = store.load()?;
    let view = match args.view {
        ViewFormat::Yaml => serde_yaml::to_string(&state)?,
        ViewFormat::Json => serde_json::to_string_pretty(&state)
            .expect("RunState serialization uses only serializable fields"),
    };
    write_view(&view)
}

fn state_resources(args: StateResourcesArgs) -> Result<(), AppError> {
    let state_directory = args
        .state_dir
        .unwrap_or_else(|| default_state_directory(&args.file));
    let repository = StateRepository::new(state_directory);
    let (_, store) = repository.open_run(&args.run)?;
    let state = store.load()?;
    let resources = state
        .scenarios
        .values()
        .flat_map(|scenario| scenario.resources.resources.iter())
        .filter(|resource| args.deleted || resource.exists)
        .collect::<Vec<_>>();
    let view = serde_yaml::to_string(&resources)?;
    write_view(&view)
}

fn state_report(args: StateReportArgs) -> Result<(), AppError> {
    let state_directory = args
        .state_dir
        .unwrap_or_else(|| default_state_directory(&args.file));
    let repository = StateRepository::new(state_directory);
    let (_, store) = repository.open_run(&args.run)?;
    let state = store.load()?;
    let output = io::stdout();
    let styled_output =
        terminal_styling_enabled(output.is_terminal(), std::env::var_os("NO_COLOR").is_some());
    render_state_report(&state, output, styled_output).map_err(AppError::WriteReport)
}

fn write_view(view: &str) -> Result<(), AppError> {
    let mut output = io::stdout().lock();
    output
        .write_all(view.as_bytes())
        .map_err(AppError::WriteView)?;
    if !view.ends_with('\n') {
        output.write_all(b"\n").map_err(AppError::WriteView)?;
    }
    Ok(())
}

fn terminal_styling_enabled(is_terminal: bool, no_color_is_set: bool) -> bool {
    is_terminal && !no_color_is_set
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
    let sequence = NEXT_RUN_IDENTIFIER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("run-{}-{milliseconds}-{sequence}", process::id())
}

static NEXT_RUN_IDENTIFIER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
mod tests {
    use super::{fingerprint, terminal_styling_enabled};

    #[test]
    fn configuration_fingerprint_changes_with_content() {
        assert_ne!(fingerprint("version: 1"), fingerprint("version: 2"));
    }

    #[test]
    fn terminal_styling_requires_a_terminal_without_no_color() {
        assert!(terminal_styling_enabled(true, false));
        assert!(!terminal_styling_enabled(false, false));
        assert!(!terminal_styling_enabled(true, true));
    }
}
