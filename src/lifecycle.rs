//! Recursive lifecycle execution for the dummy stub.

use std::{
    io::Write,
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;

use crate::{
    config::{Config, ConfiguredPhase, Scenario},
    converger::Converger,
    provisioner::Provisioner,
    state::{
        ErrorRecord, LifecyclePhase, PhaseStatus, RunState, StateError, StateStore, TestResult,
        VerifierStatus,
    },
    verifier::Verifier,
};

/// Summary used by the CLI to choose its exit status.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunOutcome {
    pub verifier_failures: usize,
    pub execution_errors: usize,
}

impl RunOutcome {
    pub fn succeeded(&self) -> bool {
        self.verifier_failures == 0 && self.execution_errors == 0
    }
}

#[derive(Debug, Error)]
pub enum LifecycleError {
    #[error(transparent)]
    State(#[from] StateError),
    #[error("could not write lifecycle output: {0}")]
    Output(#[from] std::io::Error),
    #[error("selected scenario `{0}` does not exist")]
    InvalidSelector(String),
}

/// Executes a selected subtree.  An ancestor is entered only far enough to
/// create its resource context; its verification and non-selected children are
/// deliberately skipped.
pub struct LifecycleRunner<'a, W: Write> {
    configuration: &'a Config,
    store: &'a StateStore,
    state: RunState,
    provisioner: &'a dyn Provisioner,
    converger: &'a dyn Converger,
    verifier: &'a dyn Verifier,
    output: W,
    styled_output: bool,
    outcome: RunOutcome,
}

impl<'a, W: Write> LifecycleRunner<'a, W> {
    pub fn new(
        configuration: &'a Config,
        store: &'a StateStore,
        state: RunState,
        provisioner: &'a dyn Provisioner,
        converger: &'a dyn Converger,
        verifier: &'a dyn Verifier,
        output: W,
    ) -> Self {
        Self {
            configuration,
            store,
            state,
            provisioner,
            converger,
            verifier,
            output,
            styled_output: false,
            outcome: RunOutcome::default(),
        }
    }

    pub fn with_styled_output(mut self, enabled: bool) -> Self {
        self.styled_output = enabled;
        self
    }

    pub fn run(mut self, selector: Option<&str>) -> Result<(RunOutcome, RunState), LifecycleError> {
        self.store.save(&self.state)?;

        let had_error = if let Some(selector) = selector {
            let segments: Vec<_> = selector.split('/').collect();
            if segments.is_empty() || segments.iter().any(|segment| segment.is_empty()) {
                return Err(LifecycleError::InvalidSelector(selector.to_owned()));
            }
            let Some(scenario) = self.configuration.scenarios.get(segments[0]) else {
                return Err(LifecycleError::InvalidSelector(selector.to_owned()));
            };
            self.run_scenario(scenario, segments[0], None, &segments[1..])
        } else {
            let mut had_error = false;
            for (name, scenario) in self.configuration.scenarios.iter() {
                // Top-level scenarios are independent.  A prior execution
                // error ends normal work, but cannot leave an entered scenario
                // undisposed because each call unwinds before returning.
                if had_error {
                    break;
                }
                had_error = self.run_scenario(scenario, name, None, &[]);
            }
            had_error
        };

        if had_error {
            self.outcome.execution_errors += 1;
        }
        self.store.save(&self.state)?;
        write_summary(&mut self.output, &self.state.run_id, &self.outcome)?;
        Ok((self.outcome, self.state))
    }

    /// `remaining_selector` is non-empty only while travelling through the
    /// minimal ancestor chain to a selected nested scenario.
    fn run_scenario(
        &mut self,
        scenario: &Scenario,
        path: &str,
        parent_path: Option<String>,
        remaining_selector: &[&str],
    ) -> bool {
        if self.write_scenario_entrance(path).is_err() {
            return true;
        }
        let had_error = self.run_scenario_inner(scenario, path, parent_path, remaining_selector);
        if self.write_scenario_verdict(path, had_error).is_err() {
            return true;
        }
        had_error
    }

    fn run_scenario_inner(
        &mut self,
        scenario: &Scenario,
        path: &str,
        parent_path: Option<String>,
        remaining_selector: &[&str],
    ) -> bool {
        self.state.enter_scenario(path, parent_path);
        if self.run_converger_phase(scenario, path, LifecyclePhase::Dependency) {
            return true;
        }
        if scenario.has_phase(ConfiguredPhase::Create) {
            if self
                .persist_or_record(path, LifecyclePhase::Create)
                .is_err()
            {
                return true;
            }
            if self.write_phase_running(&LifecyclePhase::Create).is_err() {
                return true;
            }
            let definition = scenario
                .phase(ConfiguredPhase::Create)
                .expect("create phase presence was checked");
            let resources = match self.provisioner.create(path, definition) {
                Ok(resources) => resources,
                Err(error) => {
                    self.execution_error(path, LifecyclePhase::Create, error.to_string());
                    return self.finish_after_failure(scenario, path);
                }
            };
            self.state.set_resources(path, resources);
            if self
                .complete(path, LifecyclePhase::Create, PhaseStatus::Pass)
                .is_err()
            {
                return self.finish_after_failure(scenario, path);
            }
        } else if self.skip(path, LifecyclePhase::Create).is_err() {
            return true;
        }

        for phase in [
            LifecyclePhase::Prepare,
            LifecyclePhase::Converge,
            LifecyclePhase::Idempotence,
        ] {
            if self.run_converger_phase(scenario, path, phase) {
                return self.finish_after_failure(scenario, path);
            }
        }

        let had_error = if let Some((child_name, tail)) = remaining_selector.split_first() {
            // Ancestors intentionally do not verify: they only establish the
            // resource state required by the selected child.
            if self.skip(path, LifecyclePhase::Verify).is_err() {
                return self.finish_after_failure(scenario, path);
            }
            let Some(child) = scenario.scenarios.get(child_name) else {
                self.execution_error(
                    path,
                    LifecyclePhase::Create,
                    format!(
                        "selected scenario `{}` does not exist",
                        path_with_child(path, child_name)
                    ),
                );
                return self.finish_after_failure(scenario, path);
            };
            let child_path = path_with_child(path, child_name);
            self.run_scenario(child, &child_path, Some(path.to_owned()), tail)
        } else {
            let mut had_error = if scenario.has_phase(ConfiguredPhase::Verify) {
                self.verify(path, scenario)
            } else {
                self.skip(path, LifecyclePhase::Verify).is_err()
            };
            if !had_error {
                for (child_name, child) in scenario.scenarios.iter() {
                    let child_path = path_with_child(path, child_name);
                    had_error = self.run_scenario(child, &child_path, Some(path.to_owned()), &[]);
                    if had_error {
                        break;
                    }
                }
            }
            had_error
        };

        let cleanup_error = self.run_converger_phase(scenario, path, LifecyclePhase::Cleanup);
        // Cleanup and destruction are best-effort even after a child or
        // verifier error.  Do not short-circuit destruction on `had_error`.
        let destroy_error = self.destroy(scenario, path);
        had_error || cleanup_error || destroy_error
    }

    fn verify(&mut self, path: &str, scenario: &Scenario) -> bool {
        if self
            .persist_or_record(path, LifecyclePhase::Verify)
            .is_err()
        {
            return true;
        }
        if self.write_phase_running(&LifecyclePhase::Verify).is_err() {
            return true;
        }

        for (test_name, _) in scenario.tests.iter() {
            match self.verifier.verify(path, test_name) {
                Ok(status) => {
                    let verifier_error = status == VerifierStatus::Error;
                    if status == VerifierStatus::Fail {
                        self.outcome.verifier_failures += 1;
                    }
                    self.state.record_test_result(
                        path,
                        TestResult {
                            name: test_name.clone(),
                            status,
                            message: None,
                            recorded_at: timestamp(),
                        },
                    );
                    if self.store.save(&self.state).is_err() {
                        return true;
                    }
                    if verifier_error {
                        self.execution_error(
                            path,
                            LifecyclePhase::Verify,
                            format!("test `{test_name}` verifier reported an execution error"),
                        );
                        return true;
                    }
                }
                Err(error) => {
                    self.execution_error(path, LifecyclePhase::Verify, error.to_string());
                    return true;
                }
            }
        }

        self.complete(path, LifecyclePhase::Verify, PhaseStatus::Pass)
            .is_err()
    }

    fn destroy(&mut self, scenario: &Scenario, path: &str) -> bool {
        if self.state.keep || !scenario.has_phase(ConfiguredPhase::Destroy) {
            return self.skip(path, LifecyclePhase::Destroy).is_err();
        }
        let resources = self
            .state
            .scenarios
            .get(path)
            .map(|scenario| scenario.resources.clone())
            .unwrap_or_default();
        // A failed write of the running transition must not stop provider
        // destruction. The final transition write is still attempted after
        // the provider call, so a recovered store can reflect it.
        let persistence_error = self
            .persist_or_record(path, LifecyclePhase::Destroy)
            .is_err();
        // The provider is still called when either reporting step fails; this
        // preserves best-effort destruction.
        let _ = self.write_phase_running(&LifecyclePhase::Destroy);
        let definition = scenario
            .phase(ConfiguredPhase::Destroy)
            .expect("destroy phase presence was checked");
        let inventory = self.ansible_inventory(scenario, path);
        match self.provisioner.destroy(
            path,
            &resources,
            definition.ansible_playbooks(),
            inventory.as_ref(),
            &mut self.output,
            self.styled_output,
        ) {
            Ok(()) => {
                self.state.mark_resources_destroyed(path);
                let completion_error = self
                    .complete(path, LifecyclePhase::Destroy, PhaseStatus::Pass)
                    .is_err();
                persistence_error || completion_error
            }
            Err(error) => {
                let record = ErrorRecord::new(path, LifecyclePhase::Destroy, error.to_string());
                self.state.record_cleanup_error(record);
                let completion_error = self
                    .complete(path, LifecyclePhase::Destroy, PhaseStatus::Error)
                    .is_err();
                let _ = persistence_error;
                let _ = completion_error;
                true
            }
        }
    }

    fn finish_after_failure(&mut self, scenario: &Scenario, path: &str) -> bool {
        // No later child or verification work is allowed after a failure.
        // Cleanup has no dummy implementation but remains explicit. Ignore
        // its state-write result so destruction is always attempted.
        let _ = self.run_converger_phase(scenario, path, LifecyclePhase::Cleanup);
        let _ = self.destroy(scenario, path);
        true
    }

    fn run_converger_phase(
        &mut self,
        scenario: &Scenario,
        path: &str,
        phase: LifecyclePhase,
    ) -> bool {
        if !scenario.has_phase(configured_phase(&phase)) {
            return self.skip(path, phase).is_err();
        }
        if self.persist_or_record(path, phase.clone()).is_err() {
            return true;
        }
        if self.write_phase_running(&phase).is_err() {
            return true;
        }
        let definition = scenario
            .phase(configured_phase(&phase))
            .expect("enabled converger phases have a definition");
        let inventory = self.ansible_inventory(scenario, path);
        match self.converger.run(
            path,
            phase.clone(),
            definition,
            inventory.as_ref(),
            &mut self.output,
            self.styled_output,
        ) {
            Ok(()) => self.complete(path, phase, PhaseStatus::Pass).is_err(),
            Err(error) => {
                self.execution_error(path, phase, error.to_string());
                true
            }
        }
    }

    fn ansible_inventory(&self, scenario: &Scenario, path: &str) -> Option<serde_json::Value> {
        let create = scenario
            .phase(ConfiguredPhase::Create)
            .and_then(|definition| definition.ansible_create())?;
        let mut inventory = create.inventory.clone();
        let mut selected = &mut inventory;
        for segment in &create.hosts_path {
            selected = selected.get_mut(segment)?;
        }
        let hosts = selected.as_object_mut()?;
        let resources = self.state.scenarios.get(path)?.resources.resources.iter();
        for resource in resources {
            let host = hosts.get_mut(&resource.id)?.as_object_mut()?;
            for (name, value) in &resource.attributes {
                host.insert(name.clone(), value.clone());
            }
            if !host.contains_key("ansible_host")
                && let Some(public_ip) = resource.attributes.get("public_ip")
            {
                host.insert("ansible_host".to_owned(), public_ip.clone());
            }
        }
        Some(inventory)
    }

    fn execution_error(&mut self, path: &str, phase: LifecyclePhase, message: String) {
        let record = ErrorRecord::new(path, phase.clone(), message);
        self.state.record_primary_error(record);
        let _ = self.complete(path, phase, PhaseStatus::Error);
    }

    fn persist_or_record(
        &mut self,
        path: &str,
        phase: LifecyclePhase,
    ) -> Result<(), LifecycleError> {
        self.state.mark_phase_running(path, phase);
        self.store.save(&self.state)?;
        Ok(())
    }

    fn complete(
        &mut self,
        path: &str,
        phase: LifecyclePhase,
        status: PhaseStatus,
    ) -> Result<(), LifecycleError> {
        self.state
            .complete_phase(path, phase.clone(), status.clone());
        self.store.save(&self.state)?;
        write_phase_result(&mut self.output, path, &phase, &status, self.styled_output)
            .map_err(Into::into)
    }

    fn write_scenario_entrance(&mut self, path: &str) -> Result<(), LifecycleError> {
        write_scenario_entrance(&mut self.output, path, self.styled_output).map_err(Into::into)
    }

    fn write_phase_running(&mut self, phase: &LifecyclePhase) -> Result<(), LifecycleError> {
        writeln!(&mut self.output, "{} running", phase_name(phase)).map_err(Into::into)
    }

    fn write_scenario_verdict(
        &mut self,
        path: &str,
        had_error: bool,
    ) -> Result<(), LifecycleError> {
        write_scenario_verdict(
            &mut self.output,
            &self.state,
            path,
            had_error,
            self.styled_output,
        )
        .map_err(Into::into)
    }

    fn skip(&mut self, path: &str, phase: LifecyclePhase) -> Result<(), LifecycleError> {
        self.complete(path, phase, PhaseStatus::Skipped)
    }
}

fn path_with_child(parent: &str, child: &str) -> String {
    format!("{parent}/{child}")
}

/// Render persisted state using the same scenario and phase lines as a live
/// lifecycle run. State is the sole input; current configuration is not read.
/// Scenario paths are stored in a `BTreeMap`, so roots and siblings use its
/// deterministic lexical order.
pub fn render_state_report<W: Write>(
    state: &RunState,
    mut output: W,
    styled_output: bool,
) -> std::io::Result<()> {
    for path in report_root_paths(state) {
        render_persisted_scenario(state, path, &mut output, styled_output)?;
    }
    write_summary(&mut output, &state.run_id, &persisted_outcome(state))
}

fn render_persisted_scenario<W: Write>(
    state: &RunState,
    path: &str,
    output: &mut W,
    styled_output: bool,
) -> std::io::Result<()> {
    let scenario = &state.scenarios[path];
    write_scenario_entrance(output, path, styled_output)?;
    for phase in [
        LifecyclePhase::Dependency,
        LifecyclePhase::Create,
        LifecyclePhase::Prepare,
        LifecyclePhase::Converge,
        LifecyclePhase::Idempotence,
        LifecyclePhase::Verify,
    ] {
        if let Some(phase_state) = scenario.phases.get(&phase) {
            write_phase_result(output, path, &phase, &phase_state.status, styled_output)?;
        }
    }
    for child_path in report_child_paths(state, path) {
        render_persisted_scenario(state, child_path, output, styled_output)?;
    }
    for phase in [LifecyclePhase::Cleanup, LifecyclePhase::Destroy] {
        if let Some(phase_state) = scenario.phases.get(&phase) {
            write_phase_result(output, path, &phase, &phase_state.status, styled_output)?;
        }
    }
    write_scenario_verdict(
        output,
        state,
        path,
        persisted_scenario_has_error(state, path),
        styled_output,
    )
}

fn report_root_paths(state: &RunState) -> Vec<&str> {
    state
        .scenarios
        .iter()
        .filter_map(|(path, scenario)| {
            scenario
                .parent_path
                .as_deref()
                .filter(|parent| state.scenarios.contains_key(*parent))
                .is_none()
                .then_some(path.as_str())
        })
        .collect()
}

fn report_child_paths<'a>(state: &'a RunState, parent_path: &str) -> Vec<&'a str> {
    state
        .scenarios
        .iter()
        .filter_map(|(path, scenario)| {
            (scenario.parent_path.as_deref() == Some(parent_path)).then_some(path.as_str())
        })
        .collect()
}

fn write_summary<W: Write>(
    output: &mut W,
    run_id: &str,
    outcome: &RunOutcome,
) -> std::io::Result<()> {
    writeln!(
        output,
        "summary: run {run_id}, {} error(s), {} verifier failure(s)",
        outcome.execution_errors, outcome.verifier_failures
    )
}

fn write_phase_result<W: Write>(
    output: &mut W,
    _path: &str,
    phase: &LifecyclePhase,
    status: &PhaseStatus,
    styled_output: bool,
) -> std::io::Result<()> {
    let phase = phase_name(phase);
    let status = status_name(status);
    if styled_output {
        writeln!(output, "{}{phase} {status}\x1b[0m", result_color(status))
    } else {
        writeln!(output, "{phase} {status}")
    }
}

fn write_scenario_entrance<W: Write>(
    output: &mut W,
    path: &str,
    styled_output: bool,
) -> std::io::Result<()> {
    if styled_output {
        writeln!(output, "\x1b[1mScenario: {path}\x1b[0m")
    } else {
        writeln!(output, "Scenario: {path}")
    }
}

fn write_scenario_verdict<W: Write>(
    output: &mut W,
    state: &RunState,
    path: &str,
    had_error: bool,
    styled_output: bool,
) -> std::io::Result<()> {
    let verdict = scenario_verdict(state, path, had_error);
    if styled_output {
        writeln!(
            output,
            "{}\x1b[1mScenario: {path}: {verdict}\x1b[0m",
            result_color(verdict)
        )
    } else {
        writeln!(output, "Scenario: {path}: {verdict}")
    }
}

fn scenario_verdict(state: &RunState, path: &str, had_error: bool) -> &'static str {
    if had_error {
        "error"
    } else if state.scenarios.get(path).is_some_and(|scenario| {
        scenario
            .phases
            .values()
            .any(|phase| phase.status == PhaseStatus::Pass)
    }) {
        "passed"
    } else {
        "skipped"
    }
}

fn persisted_scenario_has_error(state: &RunState, path: &str) -> bool {
    state.scenarios.iter().any(|(candidate_path, scenario)| {
        is_same_or_descendant(candidate_path, path)
            && scenario.phases.values().any(|phase| {
                matches!(
                    phase.status,
                    PhaseStatus::Pending | PhaseStatus::Running | PhaseStatus::Error
                )
            })
    }) || state
        .primary_error
        .as_ref()
        .is_some_and(|error| is_same_or_descendant(&error.scenario_path, path))
        || state
            .cleanup_errors
            .iter()
            .any(|error| is_same_or_descendant(&error.scenario_path, path))
}

fn is_same_or_descendant(candidate_path: &str, path: &str) -> bool {
    candidate_path == path
        || candidate_path
            .strip_prefix(path)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn persisted_outcome(state: &RunState) -> RunOutcome {
    RunOutcome {
        verifier_failures: state
            .scenarios
            .values()
            .flat_map(|scenario| &scenario.test_results)
            .filter(|result| result.status == VerifierStatus::Fail)
            .count(),
        execution_errors: usize::from(
            state.primary_error.is_some()
                || !state.cleanup_errors.is_empty()
                || state.scenarios.values().any(|scenario| {
                    scenario.phases.values().any(|phase| {
                        matches!(
                            phase.status,
                            PhaseStatus::Pending | PhaseStatus::Running | PhaseStatus::Error
                        )
                    })
                }),
        ),
    }
}

fn configured_phase(phase: &LifecyclePhase) -> ConfiguredPhase {
    match phase {
        LifecyclePhase::Dependency => ConfiguredPhase::Dependency,
        LifecyclePhase::Create => ConfiguredPhase::Create,
        LifecyclePhase::Prepare => ConfiguredPhase::Prepare,
        LifecyclePhase::Converge => ConfiguredPhase::Converge,
        LifecyclePhase::Idempotence => ConfiguredPhase::Idempotence,
        LifecyclePhase::Verify => ConfiguredPhase::Verify,
        LifecyclePhase::Cleanup => ConfiguredPhase::Cleanup,
        LifecyclePhase::Destroy => ConfiguredPhase::Destroy,
    }
}

fn phase_name(phase: &LifecyclePhase) -> &'static str {
    match phase {
        LifecyclePhase::Dependency => "dependency",
        LifecyclePhase::Create => "create",
        LifecyclePhase::Prepare => "prepare",
        LifecyclePhase::Converge => "converge",
        LifecyclePhase::Idempotence => "idempotence",
        LifecyclePhase::Verify => "verify",
        LifecyclePhase::Cleanup => "cleanup",
        LifecyclePhase::Destroy => "destroy",
    }
}

fn status_name(status: &PhaseStatus) -> &'static str {
    match status {
        PhaseStatus::Pending => "pending",
        PhaseStatus::Running => "running",
        PhaseStatus::Skipped => "skipped",
        PhaseStatus::Pass => "pass",
        PhaseStatus::Error => "error",
    }
}

fn result_color(result: &str) -> &'static str {
    match result {
        "skipped" => "\x1b[90m",
        "pass" | "passed" => "\x1b[32m",
        "error" | "fail" | "failed" => "\x1b[31m",
        _ => "",
    }
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        fs, io,
        path::PathBuf,
        rc::Rc,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;
    use crate::{
        config::Config,
        converger::DummyConverger,
        provisioner::{DummyProvisioner, ProvisionerError},
        state::{PhaseStatus, ResourceManifest},
        verifier::{DummyVerifier, Verifier},
    };

    static NEXT_TEST_PATH: AtomicU64 = AtomicU64::new(0);

    const CONFIG: &str = r#"
version: 1
provisioner: dummy
converger: dummy
verifier: dummy
scenarios:
  default:
    create:
    prepare:
    converge:
    verify:
    cleanup:
    destroy:
    tests:
      smoke: {}
    nested:
      - name: restart
        create:
        converge:
        verify:
        destroy:
        tests:
          after-restart: {}
        nested:
          - name: deep
            create:
            verify:
            destroy:
      - name: ignored
        create:
        verify:
        destroy:
  independent:
    create:
    verify:
    destroy:
"#;

    fn test_store(label: &str) -> (StateStore, PathBuf) {
        let unique = NEXT_TEST_PATH.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "cvd-lifecycle-{label}-{}-{unique}",
            std::process::id()
        ));
        (StateStore::new(directory.join("state.json")), directory)
    }

    fn state(selector: Option<&str>, keep: bool) -> RunState {
        RunState::new(
            "test-run",
            PathBuf::from("/project/cvd.yml"),
            "fingerprint",
            selector.map(str::to_owned),
            keep,
        )
    }

    #[derive(Clone)]
    struct SharedWriter(Rc<RefCell<Vec<u8>>>);

    impl io::Write for SharedWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn nested_selection_runs_only_ancestor_setup_and_selected_subtree() {
        let config = Config::from_yaml(CONFIG).unwrap();
        let (store, directory) = test_store("selection");
        let provisioner = DummyProvisioner;
        let converger = DummyConverger;
        let verifier = DummyVerifier;
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default/restart"), false),
            &provisioner,
            &converger,
            &verifier,
            Vec::new(),
        );

        let (outcome, state) = runner.run(Some("default/restart")).unwrap();
        assert!(outcome.succeeded());
        assert_eq!(
            state.scenarios.keys().collect::<Vec<_>>(),
            ["default", "default/restart", "default/restart/deep"]
        );
        assert!(!state.scenarios.contains_key("default/ignored"));
        assert!(!state.scenarios.contains_key("independent"));
        assert_eq!(
            state.scenarios["default"].phases[&LifecyclePhase::Verify].status,
            PhaseStatus::Skipped
        );
        assert_eq!(
            state.scenarios["default/restart"].test_results[0].status,
            VerifierStatus::Pass
        );
        assert!(state.scenarios.values().all(|scenario| {
            let [resource] = scenario.resources.resources.as_slice() else {
                return false;
            };
            resource.id == "dummy"
                && resource.resource_type == "dummy"
                && resource.attributes["ipv6"] == "::1"
                && !resource.exists
                && resource.created.scenario_path == scenario.path
                && resource.created.phase == LifecyclePhase::Create
                && resource.destroyed.as_ref().is_some_and(|location| {
                    location.scenario_path == scenario.path
                        && location.phase == LifecyclePhase::Destroy
                })
        }));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn create_completes_before_configured_converger_phases() {
        let config = Config::from_yaml(CONFIG).unwrap();
        let (store, directory) = test_store("create-order");
        let provisioner = DummyProvisioner;
        let converger = DummyConverger;
        let verifier = DummyVerifier;
        let bytes = Rc::new(RefCell::new(Vec::new()));
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default"), false),
            &provisioner,
            &converger,
            &verifier,
            SharedWriter(bytes.clone()),
        );

        runner.run(Some("default")).unwrap();
        let output = String::from_utf8(bytes.borrow().clone()).unwrap();
        assert!(!output.contains("\x1b["));
        assert!(output.contains("Scenario: default\n"));
        assert!(output.contains("Scenario: default: passed"));
        assert!(output.contains("create running\n"));
        assert!(output.contains("converge running\n"));
        let create = output.find("create pass").unwrap();
        let prepare = output.find("prepare pass").unwrap();
        assert!(create < prepare);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn running_phase_lines_are_plain_even_in_styled_output() {
        let config = Config::from_yaml(CONFIG).unwrap();
        let (store, directory) = test_store("running-phase-output");
        let bytes = Rc::new(RefCell::new(Vec::new()));
        LifecycleRunner::new(
            &config,
            &store,
            state(Some("default"), false),
            &DummyProvisioner,
            &DummyConverger,
            &DummyVerifier,
            SharedWriter(bytes.clone()),
        )
        .with_styled_output(true)
        .run(Some("default"))
        .unwrap();
        let output = String::from_utf8(bytes.borrow().clone()).unwrap();
        assert!(output.contains("create running\n"));
        assert!(!output.contains("\x1b[32mcreate running"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn styled_output_colors_skipped_lines_and_preserves_bold_entrance() {
        let config = Config::from_yaml(
            r#"
version: 1
provisioner: dummy
converger: dummy
verifier: dummy
scenarios:
  empty:
    tests: {}
"#,
        )
        .unwrap();
        let (store, directory) = test_store("scenario-output");
        let bytes = Rc::new(RefCell::new(Vec::new()));
        LifecycleRunner::new(
            &config,
            &store,
            state(Some("empty"), false),
            &DummyProvisioner,
            &DummyConverger,
            &DummyVerifier,
            SharedWriter(bytes.clone()),
        )
        .with_styled_output(true)
        .run(Some("empty"))
        .unwrap();

        let output = String::from_utf8(bytes.borrow().clone()).unwrap();
        assert_eq!(output.matches("\x1b[1mScenario: empty\x1b[0m").count(), 1);
        assert!(output.contains("\x1b[90mdependency skipped\x1b[0m\n"));
        assert!(output.contains("\x1b[90m\x1b[1mScenario: empty: skipped\x1b[0m\n"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn styled_output_colors_passed_phase_and_scenario_lines_green() {
        let config = Config::from_yaml(CONFIG).unwrap();
        let (store, directory) = test_store("green-output");
        let bytes = Rc::new(RefCell::new(Vec::new()));
        LifecycleRunner::new(
            &config,
            &store,
            state(Some("independent"), false),
            &DummyProvisioner,
            &DummyConverger,
            &DummyVerifier,
            SharedWriter(bytes.clone()),
        )
        .with_styled_output(true)
        .run(Some("independent"))
        .unwrap();

        let output = String::from_utf8(bytes.borrow().clone()).unwrap();
        assert!(output.contains("\x1b[32mcreate pass\x1b[0m\n"));
        assert!(output.contains("\x1b[32m\x1b[1mScenario: independent: passed\x1b[0m\n"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn state_report_replays_nested_run_output_in_stable_path_order() {
        let config = Config::from_yaml(CONFIG).unwrap();
        let (store, directory) = test_store("state-report");
        let (_, state) = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default/restart"), false),
            &DummyProvisioner,
            &DummyConverger,
            &DummyVerifier,
            Vec::new(),
        )
        .run(Some("default/restart"))
        .unwrap();

        let mut report = Vec::new();
        render_state_report(&state, &mut report, false).unwrap();
        let report = String::from_utf8(report).unwrap();
        assert!(!report.contains("\x1b["));
        assert!(report.contains("Scenario: default\n"));
        assert!(report.contains("dependency skipped\n"));
        assert!(report.contains("Scenario: default/restart\n"));
        assert!(report.contains("create pass\n"));
        assert!(report.contains("Scenario: default/restart/deep\n"));
        assert!(report.contains("destroy pass\n"));
        assert!(report.contains("Scenario: default/restart/deep: passed\n"));
        assert!(report.contains("Scenario: default/restart: passed\n"));
        assert!(report.contains("Scenario: default: passed\n"));
        let parent_verify = report.find("verify skipped\n").unwrap();
        let child_entrance = report.find("Scenario: default/restart\n").unwrap();
        let child_verdict = report.find("Scenario: default/restart: passed\n").unwrap();
        let parent_cleanup = report.rfind("cleanup pass\n").unwrap();
        let parent_destroy = report.rfind("destroy pass\n").unwrap();
        let parent_verdict = report.rfind("Scenario: default: passed\n").unwrap();
        assert!(parent_verify < child_entrance);
        assert!(child_entrance < child_verdict);
        assert!(child_verdict < parent_cleanup);
        assert!(parent_cleanup < parent_destroy);
        assert!(parent_destroy < parent_verdict);
        assert!(report.ends_with("summary: run test-run, 0 error(s), 0 verifier failure(s)\n"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn styled_state_report_uses_live_colors_and_reports_running_phases() {
        let mut run_state = state(Some("empty"), false);
        run_state.enter_scenario("empty", None);
        run_state.complete_phase("empty", LifecyclePhase::Dependency, PhaseStatus::Skipped);
        run_state.complete_phase("empty", LifecyclePhase::Create, PhaseStatus::Pass);
        run_state.mark_phase_running("empty", LifecyclePhase::Converge);

        let mut report = Vec::new();
        render_state_report(&run_state, &mut report, true).unwrap();
        let report = String::from_utf8(report).unwrap();
        assert!(report.contains("\x1b[1mScenario: empty\x1b[0m\n"));
        assert!(report.contains("\x1b[90mdependency skipped\x1b[0m\n"));
        assert!(report.contains("\x1b[32mcreate pass\x1b[0m\n"));
        assert!(report.contains("converge running\x1b[0m\n"));
        assert!(report.contains("\x1b[31m\x1b[1mScenario: empty: error\x1b[0m\n"));
    }

    struct ErrorStatusVerifier;

    impl Verifier for ErrorStatusVerifier {
        fn verify(
            &self,
            _scenario_path: &str,
            _test_name: &str,
        ) -> Result<VerifierStatus, crate::verifier::VerifierError> {
            Ok(VerifierStatus::Error)
        }
    }

    #[test]
    fn verifier_error_status_is_persisted_and_stops_normal_work() {
        let config = Config::from_yaml(CONFIG).unwrap();
        let (store, directory) = test_store("verifier-status-error");
        let provisioner = DummyProvisioner;
        let converger = DummyConverger;
        let verifier = ErrorStatusVerifier;
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default"), false),
            &provisioner,
            &converger,
            &verifier,
            Vec::new(),
        );

        let (outcome, state) = runner.run(Some("default")).unwrap();
        assert!(!outcome.succeeded());
        assert_eq!(outcome.execution_errors, 1);
        assert_eq!(
            state.scenarios["default"].test_results[0].status,
            VerifierStatus::Error
        );
        assert_eq!(
            state.scenarios["default"].phases[&LifecyclePhase::Verify].status,
            PhaseStatus::Error
        );
        assert_eq!(
            state.primary_error.as_ref().unwrap().phase,
            LifecyclePhase::Verify
        );
        assert!(!state.scenarios.contains_key("default/restart"));
        assert_eq!(
            state.scenarios["default"].phases[&LifecyclePhase::Destroy].status,
            PhaseStatus::Pass
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    struct PersistencePoisoningProvisioner {
        state_directory: PathBuf,
        calls: RefCell<Vec<String>>,
    }

    impl Provisioner for PersistencePoisoningProvisioner {
        fn create(
            &self,
            scenario_path: &str,
            _: &crate::config::PhaseDefinition,
        ) -> Result<ResourceManifest, ProvisionerError> {
            self.calls
                .borrow_mut()
                .push(format!("create:{scenario_path}"));
            // All writes through StateStore have succeeded up to this point.
            // Replacing its parent directory with a file makes the following
            // state update fail deterministically without a new test seam.
            fs::remove_dir_all(&self.state_directory).unwrap();
            fs::write(&self.state_directory, "not a directory").unwrap();
            Ok(ResourceManifest::default())
        }

        fn destroy(
            &self,
            scenario_path: &str,
            _: &ResourceManifest,
            _: &[std::path::PathBuf],
            _: Option<&serde_json::Value>,
            _: &mut dyn std::io::Write,
            _: bool,
        ) -> Result<(), ProvisionerError> {
            self.calls
                .borrow_mut()
                .push(format!("destroy:{scenario_path}"));
            Ok(())
        }
    }

    #[test]
    fn post_create_state_write_failure_still_attempts_destruction() {
        let config = Config::from_yaml(CONFIG).unwrap();
        let (store, directory) = test_store("post-create-state-failure");
        let provisioner = PersistencePoisoningProvisioner {
            state_directory: directory.clone(),
            calls: RefCell::new(Vec::new()),
        };
        let converger = DummyConverger;
        let verifier = DummyVerifier;
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default"), false),
            &provisioner,
            &converger,
            &verifier,
            Vec::new(),
        );

        assert!(matches!(
            runner.run(Some("default")),
            Err(LifecycleError::State(_))
        ));
        assert_eq!(
            provisioner.calls.into_inner(),
            ["create:default", "destroy:default"]
        );
        fs::remove_file(directory).unwrap();
    }

    struct FailingCreateProvisioner(RefCell<Vec<String>>);

    impl Provisioner for FailingCreateProvisioner {
        fn create(
            &self,
            scenario_path: &str,
            _: &crate::config::PhaseDefinition,
        ) -> Result<ResourceManifest, ProvisionerError> {
            self.0.borrow_mut().push(format!("create:{scenario_path}"));
            Err(ProvisionerError("create failed".into()))
        }

        fn destroy(
            &self,
            scenario_path: &str,
            _: &ResourceManifest,
            _: &[std::path::PathBuf],
            _: Option<&serde_json::Value>,
            _: &mut dyn std::io::Write,
            _: bool,
        ) -> Result<(), ProvisionerError> {
            self.0.borrow_mut().push(format!("destroy:{scenario_path}"));
            Ok(())
        }
    }

    #[test]
    fn create_error_records_failure_and_still_unwinds() {
        let config = Config::from_yaml(CONFIG).unwrap();
        let (store, directory) = test_store("create-error");
        let provisioner = FailingCreateProvisioner(RefCell::new(Vec::new()));
        let converger = DummyConverger;
        let verifier = DummyVerifier;
        let bytes = Rc::new(RefCell::new(Vec::new()));
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default"), false),
            &provisioner,
            &converger,
            &verifier,
            SharedWriter(bytes.clone()),
        )
        .with_styled_output(true);

        let (outcome, state) = runner.run(Some("default")).unwrap();
        assert!(!outcome.succeeded());
        assert_eq!(outcome.execution_errors, 1);
        assert_eq!(
            state.primary_error.as_ref().unwrap().phase,
            LifecyclePhase::Create
        );
        assert_eq!(
            state.scenarios["default"].phases[&LifecyclePhase::Cleanup].status,
            PhaseStatus::Pass
        );
        assert_eq!(
            state.scenarios["default"].phases[&LifecyclePhase::Destroy].status,
            PhaseStatus::Pass
        );
        assert_eq!(
            provisioner.0.into_inner(),
            ["create:default", "destroy:default"]
        );
        let output = String::from_utf8(bytes.borrow().clone()).unwrap();
        assert!(output.contains("\x1b[31mcreate error\x1b[0m\n"));
        assert!(output.contains("\x1b[31m\x1b[1mScenario: default: error\x1b[0m\n"));
        fs::remove_dir_all(directory).unwrap();
    }

    struct RecordingProvisioner(RefCell<Vec<String>>);

    impl Provisioner for RecordingProvisioner {
        fn create(
            &self,
            scenario_path: &str,
            _: &crate::config::PhaseDefinition,
        ) -> Result<ResourceManifest, ProvisionerError> {
            self.0.borrow_mut().push(format!("create:{scenario_path}"));
            Ok(ResourceManifest::default())
        }

        fn destroy(
            &self,
            scenario_path: &str,
            _: &ResourceManifest,
            _: &[std::path::PathBuf],
            _: Option<&serde_json::Value>,
            _: &mut dyn std::io::Write,
            _: bool,
        ) -> Result<(), ProvisionerError> {
            self.0.borrow_mut().push(format!("destroy:{scenario_path}"));
            Ok(())
        }
    }

    #[test]
    fn destroys_children_before_parents_and_keep_suppresses_destruction() {
        let config = Config::from_yaml(CONFIG).unwrap();
        let (store, directory) = test_store("destroy-order");
        let provisioner = RecordingProvisioner(RefCell::new(Vec::new()));
        let converger = DummyConverger;
        let verifier = DummyVerifier;
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default/restart"), false),
            &provisioner,
            &converger,
            &verifier,
            Vec::new(),
        );
        runner.run(Some("default/restart")).unwrap();
        assert_eq!(
            provisioner.0.into_inner(),
            [
                "create:default",
                "create:default/restart",
                "create:default/restart/deep",
                "destroy:default/restart/deep",
                "destroy:default/restart",
                "destroy:default",
            ]
        );
        std::fs::remove_dir_all(directory).unwrap();

        let (store, directory) = test_store("keep");
        let provisioner = RecordingProvisioner(RefCell::new(Vec::new()));
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default/restart"), true),
            &provisioner,
            &converger,
            &verifier,
            Vec::new(),
        );
        let (_, state) = runner.run(Some("default/restart")).unwrap();
        assert!(
            provisioner
                .0
                .into_inner()
                .iter()
                .all(|event| !event.starts_with("destroy:"))
        );
        assert!(state.scenarios.values().all(|scenario| {
            scenario.phases[&LifecyclePhase::Destroy].status == PhaseStatus::Skipped
        }));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
