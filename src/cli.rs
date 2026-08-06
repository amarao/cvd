use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

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
    /// Render a persisted state view for a previous run.
    StateView(StateViewArgs),
    /// List resources recorded for a previous run.
    StateResources(StateResourcesArgs),
    /// Replay a persisted run report.
    StateReport(StateReportArgs),
}

#[derive(Debug, clap::Args)]
pub struct RunArgs {
    /// Stable slash-separated path of the scenario to run.
    #[arg(value_name = "SCENARIO")]
    pub scenario: Option<String>,

    /// Scenario configuration file.
    #[arg(short = 'f', long, default_value = "cvd.yml", value_name = "FILE")]
    pub file: PathBuf,

    /// State directory, overriding the project-local default.
    #[arg(long, value_name = "DIR")]
    pub state_dir: Option<PathBuf>,

    /// Retain entered scenarios and skip destruction.
    #[arg(long)]
    pub keep: bool,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum ViewFormat {
    Yaml,
    Json,
}

#[derive(Debug, clap::Args)]
pub struct StateViewArgs {
    /// State representation to print.
    #[arg(value_enum, default_value_t = ViewFormat::Yaml, value_name = "VIEW")]
    pub view: ViewFormat,

    /// Run ID to inspect, or `last` for the most recently started run.
    #[arg(long, default_value = "last", value_name = "RUN")]
    pub run: String,

    /// Scenario configuration file, used only to find the default state directory.
    #[arg(short = 'f', long, default_value = "cvd.yml", value_name = "FILE")]
    pub file: PathBuf,

    /// State directory, overriding the project-local default.
    #[arg(long, value_name = "DIR")]
    pub state_dir: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
pub struct StateResourcesArgs {
    /// Include resources that were destroyed.
    #[arg(long)]
    pub deleted: bool,

    /// Run ID to inspect, or `last` for the most recently started run.
    #[arg(long, default_value = "last", value_name = "RUN")]
    pub run: String,

    /// Scenario configuration file, used only to find the default state directory.
    #[arg(short = 'f', long, default_value = "cvd.yml", value_name = "FILE")]
    pub file: PathBuf,

    /// State directory, overriding the project-local default.
    #[arg(long, value_name = "DIR")]
    pub state_dir: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
pub struct StateReportArgs {
    /// Run ID to report, or `last` for the most recently started run.
    #[arg(long, default_value = "last", value_name = "RUN")]
    pub run: String,

    /// Scenario configuration file, used only to find the default state directory.
    #[arg(short = 'f', long, default_value = "cvd.yml", value_name = "FILE")]
    pub file: PathBuf,

    /// State directory, overriding the project-local default.
    #[arg(long, value_name = "DIR")]
    pub state_dir: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command, ViewFormat};

    #[test]
    fn parses_run_with_defaults() {
        let cli = Cli::try_parse_from(["cvd", "run"]).unwrap();

        let Command::Run(run) = cli.command else {
            panic!("expected run command");
        };
        assert_eq!(run.scenario, None);
        assert_eq!(run.file.to_string_lossy(), "cvd.yml");
        assert_eq!(run.state_dir, None);
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
            "--state-dir",
            ".cvd/custom",
            "--keep",
        ])
        .unwrap();

        let Command::Run(run) = cli.command else {
            panic!("expected run command");
        };
        assert_eq!(run.scenario.as_deref(), Some("default/restart"));
        assert_eq!(run.file.to_string_lossy(), "scenarios.yml");
        assert_eq!(run.state_dir.unwrap().to_string_lossy(), ".cvd/custom");
        assert!(run.keep);
    }

    #[test]
    fn parses_state_view_defaults_and_selection() {
        let cli = Cli::try_parse_from(["cvd", "state-view"]).unwrap();
        let Command::StateView(show) = cli.command else {
            panic!("expected state-view command");
        };
        assert!(matches!(show.view, ViewFormat::Yaml));
        assert_eq!(show.run, "last");
        assert_eq!(show.file.to_string_lossy(), "cvd.yml");

        let cli = Cli::try_parse_from([
            "cvd",
            "state-view",
            "json",
            "--run",
            "run-123",
            "--state-dir",
            ".cvd",
        ])
        .unwrap();
        let Command::StateView(show) = cli.command else {
            panic!("expected state-view command");
        };
        assert!(matches!(show.view, ViewFormat::Json));
        assert_eq!(show.run, "run-123");
        assert_eq!(show.state_dir.unwrap().to_string_lossy(), ".cvd");
    }

    #[test]
    fn parses_state_resources_defaults_and_deleted() {
        let cli = Cli::try_parse_from(["cvd", "state-resources"]).unwrap();
        let Command::StateResources(resources) = cli.command else {
            panic!("expected state-resources command");
        };
        assert!(!resources.deleted);
        assert_eq!(resources.run, "last");

        let cli = Cli::try_parse_from(["cvd", "state-resources", "--deleted", "--run", "run-123"])
            .unwrap();
        let Command::StateResources(resources) = cli.command else {
            panic!("expected state-resources command");
        };
        assert!(resources.deleted);
        assert_eq!(resources.run, "run-123");
    }

    #[test]
    fn parses_state_report_defaults_and_selection() {
        let cli = Cli::try_parse_from(["cvd", "state-report"]).unwrap();
        let Command::StateReport(report) = cli.command else {
            panic!("expected state-report command");
        };
        assert_eq!(report.run, "last");
        assert_eq!(report.file.to_string_lossy(), "cvd.yml");
        assert_eq!(report.state_dir, None);

        let cli = Cli::try_parse_from([
            "cvd",
            "state-report",
            "--run",
            "run-123",
            "--file",
            "scenarios.yml",
            "--state-dir",
            ".cvd",
        ])
        .unwrap();
        let Command::StateReport(report) = cli.command else {
            panic!("expected state-report command");
        };
        assert_eq!(report.run, "run-123");
        assert_eq!(report.file.to_string_lossy(), "scenarios.yml");
        assert_eq!(report.state_dir.unwrap().to_string_lossy(), ".cvd");
    }
}
