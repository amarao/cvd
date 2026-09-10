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
    /// Validate configuration syntax and referenced files without running it.
    SyntaxCheck(SyntaxCheckArgs),
    /// Render a persisted state view for a previous run.
    StateView(StateViewArgs),
    /// List resources recorded for a previous run.
    StateResources(StateResourcesArgs),
    /// Replay a persisted run report.
    StateReport(StateReportArgs),
}

#[derive(Debug, clap::Args)]
pub struct SyntaxCheckArgs {
    /// Scenario configuration file.
    #[arg(short = 'f', long, default_value = "cvd.yml", value_name = "FILE")]
    pub file: PathBuf,

    /// Directory containing cvd.yaml or cvd.yml (alternative to --file).
    #[arg(short = 'F', long, value_name = "DIR", conflicts_with = "file")]
    pub directory: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
pub struct RunArgs {
    /// Stable slash-separated path of the scenario to run.
    #[arg(value_name = "SCENARIO")]
    pub scenario: Option<String>,

    /// Scenario configuration file.
    #[arg(short = 'f', long, default_value = "cvd.yml", value_name = "FILE")]
    pub file: PathBuf,

    /// Directory containing cvd.yaml or cvd.yml (alternative to --file).
    #[arg(short = 'F', long, value_name = "DIR", conflicts_with = "file")]
    pub directory: Option<PathBuf>,

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

    /// Directory containing cvd.yaml or cvd.yml (alternative to --file).
    #[arg(short = 'F', long, value_name = "DIR", conflicts_with = "file")]
    pub directory: Option<PathBuf>,

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

    /// Directory containing cvd.yaml or cvd.yml (alternative to --file).
    #[arg(short = 'F', long, value_name = "DIR", conflicts_with = "file")]
    pub directory: Option<PathBuf>,

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

    /// Directory containing cvd.yaml or cvd.yml (alternative to --file).
    #[arg(short = 'F', long, value_name = "DIR", conflicts_with = "file")]
    pub directory: Option<PathBuf>,

    /// State directory, overriding the project-local default.
    #[arg(long, value_name = "DIR")]
    pub state_dir: Option<PathBuf>,
}

pub fn configuration_file(file: &std::path::Path, directory: Option<&std::path::Path>) -> PathBuf {
    let Some(directory) = directory else {
        return file.to_owned();
    };
    let yaml = directory.join("cvd.yaml");
    if yaml.is_file() {
        return yaml;
    }
    let yml = directory.join("cvd.yml");
    if yml.is_file() {
        return yml;
    }
    yaml
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command, ViewFormat};

    #[test]
    fn directory_option_is_available_on_all_commands_and_conflicts_with_file() {
        for command in [
            "run",
            "syntax-check",
            "state-view",
            "state-resources",
            "state-report",
        ] {
            let parsed = Cli::try_parse_from(["cvd", command, "-F", "project"]).unwrap();
            let (file, directory) = match parsed.command {
                Command::Run(args) => (args.file, args.directory),
                Command::SyntaxCheck(args) => (args.file, args.directory),
                Command::StateView(args) => (args.file, args.directory),
                Command::StateResources(args) => (args.file, args.directory),
                Command::StateReport(args) => (args.file, args.directory),
            };
            assert_eq!(
                super::configuration_file(&file, directory.as_deref()),
                std::path::PathBuf::from("project/cvd.yaml")
            );
            assert!(
                Cli::try_parse_from(["cvd", command, "-f", "other.yml", "-F", "project"]).is_err()
            );
            assert!(Cli::try_parse_from(["cvd", command, "-F"]).is_err());
        }
    }

    #[test]
    fn parses_syntax_check_with_defaults_and_file_selection() {
        let cli = Cli::try_parse_from(["cvd", "syntax-check"]).unwrap();
        let Command::SyntaxCheck(check) = cli.command else {
            panic!("expected syntax-check command");
        };
        assert_eq!(check.file.to_string_lossy(), "cvd.yml");
        assert_eq!(check.directory, None);

        let cli = Cli::try_parse_from(["cvd", "syntax-check", "--file", "scenario.yml"]).unwrap();
        let Command::SyntaxCheck(check) = cli.command else {
            panic!("expected syntax-check command");
        };
        assert_eq!(check.file.to_string_lossy(), "scenario.yml");
    }

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
