use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Command-line interface for CVD.
#[derive(Debug, Parser)]
#[command(
    name = "cvd",
    version,
    about = "Create, verify, and destroy test resources"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run a scenario or a scenario subtree.
    Run(RunArgs),
}

#[derive(Debug, clap::Args)]
pub struct RunArgs {
    /// Stable slash-separated path of the scenario to run.
    #[arg(value_name = "SCENARIO")]
    pub scenario: Option<String>,

    /// Scenario configuration file.
    #[arg(short = 'f', long, default_value = "cvd.yml", value_name = "FILE")]
    pub file: PathBuf,

    /// State file path, overriding the project-local default.
    #[arg(long, value_name = "PATH")]
    pub state: Option<PathBuf>,

    /// Retain entered scenarios and skip destruction.
    #[arg(long)]
    pub keep: bool,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command};

    #[test]
    fn parses_run_with_defaults() {
        let cli = Cli::try_parse_from(["cvd", "run"]).unwrap();

        let Command::Run(run) = cli.command;
        assert_eq!(run.scenario, None);
        assert_eq!(run.file.to_string_lossy(), "cvd.yml");
        assert_eq!(run.state, None);
        assert!(!run.keep);
    }

    #[test]
    fn parses_run_options() {
        let cli = Cli::try_parse_from([
            "cvd",
            "run",
            "default/restart",
            "--file",
            "scenarios.yml",
            "--state",
            ".cvd/custom.json",
            "--keep",
        ])
        .unwrap();

        let Command::Run(run) = cli.command;
        assert_eq!(run.scenario.as_deref(), Some("default/restart"));
        assert_eq!(run.file.to_string_lossy(), "scenarios.yml");
        assert_eq!(run.state.unwrap().to_string_lossy(), ".cvd/custom.json");
        assert!(run.keep);
    }
}
