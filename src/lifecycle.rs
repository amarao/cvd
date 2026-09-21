//! Recursive lifecycle execution for the dummy stub.

use crate::process::Deadline;
use std::{
    io::Write,
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;

use crate::{
    config::{Config, ConfiguredPhase, Scenario, SequenceEntry, TestMap},
    converger::Converger,
    keep::KeepMode,
    provisioner::Provisioner,
    state::{
        ErrorRecord, LifecyclePhase, PhaseState, PhaseStatus, RunState, ScenarioState,
        SequencePhaseState, StateError, StateStore, TestResult, VerifierStatus,
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
    keep_mode: KeepMode,
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
            keep_mode: KeepMode::new(state.keep),
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

    pub fn with_keep_mode(mut self, keep_mode: KeepMode) -> Self {
        self.keep_mode = keep_mode;
        self
    }

    fn persist(&mut self) -> Result<(), StateError> {
        // Signal handlers only update the live policy. Keep one state writer
        // and snapshot that policy at the existing lifecycle checkpoints.
        self.state.set_keep(self.keep_mode.enabled());
        self.store.save(&self.state)
    }

    pub fn run(mut self, selector: Option<&str>) -> Result<(RunOutcome, RunState), LifecycleError> {
        self.persist()?;

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
        self.persist()?;
        write_summary(&mut self.output, &self.state, &self.outcome)?;
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
        if scenario.has_phase(ConfiguredPhase::Create) {
            if self
                .persist_or_record(path, LifecyclePhase::Create)
                .is_err()
            {
                return true;
            }
            if self
                .write_phase_running(path, &LifecyclePhase::Create)
                .is_err()
            {
                return true;
            }
            let definition = scenario
                .phase(ConfiguredPhase::Create)
                .expect("create phase presence was checked");
            let deadline = Deadline::new(scenario.timeout);
            let resources = match self.provisioner.create(path, definition, &deadline) {
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
            if self.run_converger_phase(scenario, path, LifecyclePhase::SideEffect) {
                return self.finish_after_failure(scenario, path);
            }
            if self.run_sequence(scenario, path) {
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
                had_error = self.run_converger_phase(scenario, path, LifecyclePhase::SideEffect);
            }
            if !had_error {
                had_error = self.run_sequence(scenario, path);
            }
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

    fn run_sequence(&mut self, scenario: &Scenario, path: &str) -> bool {
        for (index, entry) in scenario.sequence.iter().enumerate() {
            self.set_sequence_state(path, index, entry, PhaseState::running());
            if self.persist().is_err() {
                return true;
            }
            writeln!(
                self.output,
                "{path}::sequence[{index}]::{} running",
                phase_name_for_config(entry.phase)
            )
            .ok();
            let failed = if entry.phase == ConfiguredPhase::Verify {
                self.verify_tests(path, scenario, &entry.tests, Some(index))
            } else {
                let phase = match entry.phase {
                    ConfiguredPhase::Converge => LifecyclePhase::Converge,
                    _ => LifecyclePhase::SideEffect,
                };
                let definition = entry
                    .definition
                    .as_ref()
                    .expect("converger sequence entry has a definition");
                let inventory = if definition.ansible().is_some() {
                    match crate::inventory::write_view(
                        &mut self.state,
                        path,
                        phase.clone(),
                        self.store.path(),
                    ) {
                        Ok(value) => {
                            if self.persist().is_err() {
                                return true;
                            }
                            value
                        }
                        Err(error) => {
                            self.sequence_error(path, index, entry, &phase, error);
                            return true;
                        }
                    }
                } else {
                    None
                };
                let result = self.converger.run(
                    path,
                    phase.clone(),
                    definition,
                    inventory.as_deref(),
                    &self.state.visible_resources(path),
                    &mut self.output,
                    self.styled_output,
                    &Deadline::new(scenario.timeout),
                );
                match result {
                    Ok(()) => false,
                    Err(error) => {
                        self.sequence_error(path, index, entry, &phase, error.to_string());
                        true
                    }
                }
            };
            if !failed {
                self.set_sequence_state(
                    path,
                    index,
                    entry,
                    PhaseState::completed(PhaseStatus::Pass, None),
                );
                if self.persist().is_err() {
                    return true;
                }
                writeln!(
                    self.output,
                    "{path}::sequence[{index}]::{} pass",
                    phase_name_for_config(entry.phase)
                )
                .ok();
            }
            if failed {
                self.set_sequence_state(
                    path,
                    index,
                    entry,
                    PhaseState::completed(PhaseStatus::Error, None),
                );
                let _ = self.persist();
                if entry.phase == ConfiguredPhase::Verify {
                    writeln!(
                        self.output,
                        "{path}::sequence[{index}]::{} error",
                        phase_name_for_config(entry.phase)
                    )
                    .ok();
                }
                return true;
            }
        }
        false
    }

    fn set_sequence_state(
        &mut self,
        path: &str,
        index: usize,
        entry: &SequenceEntry,
        state: PhaseState,
    ) {
        let scenario = self
            .state
            .scenarios
            .get_mut(path)
            .expect("entered scenario");
        if scenario.sequence.len() <= index {
            scenario
                .sequence
                .resize_with(index + 1, || SequencePhaseState {
                    index,
                    phase: LifecyclePhase::Converge,
                    state: PhaseState::running(),
                });
        }
        scenario.sequence[index] = SequencePhaseState {
            index,
            phase: match entry.phase {
                ConfiguredPhase::Verify => LifecyclePhase::Verify,
                ConfiguredPhase::SideEffect => LifecyclePhase::SideEffect,
                _ => LifecyclePhase::Converge,
            },
            state,
        };
    }

    fn sequence_error(
        &mut self,
        path: &str,
        index: usize,
        entry: &SequenceEntry,
        phase: &LifecyclePhase,
        message: String,
    ) {
        self.state.record_primary_error(ErrorRecord::new(
            path,
            phase.clone(),
            format!("sequence[{index}]: {message}"),
        ));
        self.set_sequence_state(
            path,
            index,
            entry,
            PhaseState::completed(PhaseStatus::Error, None),
        );
        let _ = self.persist();
        writeln!(
            self.output,
            "{path}::sequence[{index}]::{} error",
            phase_name_for_config(entry.phase)
        )
        .ok();
    }

    fn verify(&mut self, path: &str, scenario: &Scenario) -> bool {
        self.verify_tests(path, scenario, &scenario.tests, None)
    }

    fn verify_tests(
        &mut self,
        path: &str,
        scenario: &Scenario,
        tests: &TestMap,
        sequence_index: Option<usize>,
    ) -> bool {
        if sequence_index.is_none() {
            if self
                .persist_or_record(path, LifecyclePhase::Verify)
                .is_err()
            {
                return true;
            }
            if self
                .write_phase_running(path, &LifecyclePhase::Verify)
                .is_err()
            {
                return true;
            }
        }

        let deadline = Deadline::new(scenario.timeout);
        for (test_name, test) in tests.iter() {
            let inventory = if test.pytest.is_some() || test.ansible.is_some() {
                crate::inventory::write_view(
                    &mut self.state,
                    path,
                    LifecyclePhase::Verify,
                    self.store.path(),
                )
                .and_then(|inventory| {
                    self.persist()
                        .map(|_| inventory)
                        .map_err(|error| format!("cannot persist inventory view: {error}"))
                })
            } else {
                Ok(None)
            };
            let result = match inventory {
                Ok(inventory) => self.verifier.verify(
                    path,
                    test_name,
                    test,
                    inventory.as_deref(),
                    &self.state.visible_resources(path),
                    &deadline,
                ),
                Err(error) => Err(crate::verifier::VerifierError(error)),
            };
            match result {
                Ok(status) => {
                    let verifier_error = status == VerifierStatus::Error;
                    if status == VerifierStatus::Fail {
                        self.outcome.verifier_failures += 1;
                    }
                    self.state.record_test_result(
                        path,
                        TestResult {
                            name: test_name.clone(),
                            sequence_index,
                            status,
                            message: None,
                            recorded_at: timestamp(),
                        },
                    );
                    if self.persist().is_err() {
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
                    self.state.record_test_result(
                        path,
                        TestResult {
                            name: test_name.clone(),
                            sequence_index,
                            status: VerifierStatus::Error,
                            message: Some(error.to_string()),
                            recorded_at: timestamp(),
                        },
                    );
                    self.execution_error(path, LifecyclePhase::Verify, error.to_string());
                    return true;
                }
            }
        }

        if sequence_index.is_some() {
            false
        } else {
            self.complete(path, LifecyclePhase::Verify, PhaseStatus::Pass)
                .is_err()
        }
    }

    fn destroy(&mut self, scenario: &Scenario, path: &str) -> bool {
        // Decide separately for every scenario, including error unwinding.
        // Once admitted, a destroy phase runs to completion even if a signal
        // changes the policy during the provider call.
        if self.keep_mode.enabled() || !scenario.has_phase(ConfiguredPhase::Destroy) {
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
        let _ = self.write_phase_running(path, &LifecyclePhase::Destroy);
        let definition = scenario
            .phase(ConfiguredPhase::Destroy)
            .expect("destroy phase presence was checked");
        let deadline = Deadline::new(scenario.timeout);
        match self
            .provisioner
            .destroy(path, &resources, definition, &deadline)
        {
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
        if self.write_phase_running(path, &phase).is_err() {
            return true;
        }
        let definition = scenario
            .phase(configured_phase(&phase))
            .expect("enabled converger phases have a definition");
        let deadline = Deadline::new(scenario.timeout);
        let inventory = if definition.ansible().is_some() {
            match crate::inventory::write_view(
                &mut self.state,
                path,
                phase.clone(),
                self.store.path(),
            ) {
                Ok(inventory) => {
                    if let Err(error) = self.persist() {
                        self.execution_error(
                            path,
                            phase,
                            format!("cannot persist inventory view: {error}"),
                        );
                        return true;
                    }
                    inventory
                }
                Err(error) => {
                    self.execution_error(path, phase, error);
                    return true;
                }
            }
        } else {
            None
        };
        let resources = self.state.visible_resources(path);
        match self.converger.run(
            path,
            phase.clone(),
            definition,
            inventory.as_deref(),
            &resources,
            &mut self.output,
            self.styled_output,
            &deadline,
        ) {
            Ok(()) => self.complete(path, phase, PhaseStatus::Pass).is_err(),
            Err(error) => {
                self.execution_error(path, phase, error.to_string());
                true
            }
        }
    }

    fn execution_error(&mut self, path: &str, phase: LifecyclePhase, message: String) {
        let record = ErrorRecord::new(path, phase.clone(), message);
        if phase == LifecyclePhase::Cleanup && self.state.primary_error.is_some() {
            self.state.record_cleanup_error(record);
        } else {
            self.state.record_primary_error(record);
        }
        let _ = self.complete(path, phase, PhaseStatus::Error);
    }

    fn persist_or_record(
        &mut self,
        path: &str,
        phase: LifecyclePhase,
    ) -> Result<(), LifecycleError> {
        self.state.mark_phase_running(path, phase);
        self.persist()?;
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
        self.persist()?;
        write_phase_result(
            &mut self.output,
            path,
            &phase,
            &status,
            self.state.scenarios.get(path),
            self.styled_output,
        )
        .map_err(Into::into)
    }

    fn write_scenario_entrance(&mut self, path: &str) -> Result<(), LifecycleError> {
        write_scenario_entrance(&mut self.output, path, self.styled_output).map_err(Into::into)
    }

    fn write_phase_running(
        &mut self,
        path: &str,
        phase: &LifecyclePhase,
    ) -> Result<(), LifecycleError> {
        writeln!(&mut self.output, "{}::{} running", path, phase_name(phase)).map_err(Into::into)
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
    write_summary(&mut output, state, &persisted_outcome(state))
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
        LifecyclePhase::Create,
        LifecyclePhase::Prepare,
        LifecyclePhase::Converge,
        LifecyclePhase::Idempotence,
        LifecyclePhase::Verify,
        LifecyclePhase::SideEffect,
    ] {
        if let Some(phase_state) = scenario.phases.get(&phase) {
            write_phase_result(
                output,
                path,
                &phase,
                &phase_state.status,
                Some(scenario),
                styled_output,
            )?;
        }
    }
    for item in &scenario.sequence {
        writeln!(
            output,
            "{path}::sequence[{}]::{} {}",
            item.index,
            phase_name(&item.phase),
            status_name(&item.state.status)
        )?;
    }
    for child_path in report_child_paths(state, path) {
        render_persisted_scenario(state, child_path, output, styled_output)?;
    }
    for phase in [LifecyclePhase::Cleanup, LifecyclePhase::Destroy] {
        if let Some(phase_state) = scenario.phases.get(&phase) {
            write_phase_result(
                output,
                path,
                &phase,
                &phase_state.status,
                Some(scenario),
                styled_output,
            )?;
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
    state: &RunState,
    outcome: &RunOutcome,
) -> std::io::Result<()> {
    let (created, destroyed) = resource_counts(state);
    writeln!(
        output,
        "summary: run {}, created {} resource(s), destroyed {} resource(s), {} error(s), {} verifier failure(s)",
        state.run_id, created, destroyed, outcome.execution_errors, outcome.verifier_failures
    )
}

fn resource_counts(state: &RunState) -> (usize, usize) {
    let resources = state
        .scenarios
        .values()
        .flat_map(|scenario| scenario.resources.resources.iter());
    let mut created = 0;
    let mut destroyed = 0;
    for resource in resources {
        created += 1;
        if resource.destroyed.is_some() {
            destroyed += 1;
        }
    }
    (created, destroyed)
}

fn write_phase_result<W: Write>(
    output: &mut W,
    path: &str,
    phase: &LifecyclePhase,
    status: &PhaseStatus,
    scenario: Option<&ScenarioState>,
    styled_output: bool,
) -> std::io::Result<()> {
    let phase_name = phase_name(phase);
    let color_status = status_name(status);
    let status_text = phase_status_text(phase_name, status, scenario);
    let separator = if matches!(phase_name, "create" | "destroy") && *status == PhaseStatus::Pass {
        ": "
    } else {
        " "
    };
    if styled_output {
        writeln!(
            output,
            "{}{path}::{phase_name}{separator}{status_text}\x1b[0m",
            result_color(color_status)
        )
    } else {
        writeln!(output, "{path}::{phase_name}{separator}{status_text}")
    }
}

fn phase_status_text(
    phase: &str,
    status: &PhaseStatus,
    scenario: Option<&ScenarioState>,
) -> String {
    if *status == PhaseStatus::Pass {
        let resources = scenario.map(|scenario| &scenario.resources.resources);
        match phase {
            "create" => return format!("{} resource added", resources.map_or(0, Vec::len)),
            "destroy" => {
                let count = resources
                    .map(|resources| {
                        resources
                            .iter()
                            .filter(|resource| resource.destroyed.is_some())
                            .count()
                    })
                    .unwrap_or(0);
                return format!("{count} resource deleted");
            }
            _ => {}
        }
    }
    status_name(status).to_owned()
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
        LifecyclePhase::Create => ConfiguredPhase::Create,
        LifecyclePhase::Prepare => ConfiguredPhase::Prepare,
        LifecyclePhase::Converge => ConfiguredPhase::Converge,
        LifecyclePhase::Idempotence => ConfiguredPhase::Idempotence,
        LifecyclePhase::SideEffect => ConfiguredPhase::SideEffect,
        LifecyclePhase::Verify => ConfiguredPhase::Verify,
        LifecyclePhase::Cleanup => ConfiguredPhase::Cleanup,
        LifecyclePhase::Destroy => ConfiguredPhase::Destroy,
    }
}

fn phase_name(phase: &LifecyclePhase) -> &'static str {
    match phase {
        LifecyclePhase::Create => "create",
        LifecyclePhase::Prepare => "prepare",
        LifecyclePhase::Converge => "converge",
        LifecyclePhase::Idempotence => "idempotence",
        LifecyclePhase::SideEffect => "side_effect",
        LifecyclePhase::Verify => "verify",
        LifecyclePhase::Cleanup => "cleanup",
        LifecyclePhase::Destroy => "destroy",
    }
}

fn phase_name_for_config(phase: ConfiguredPhase) -> &'static str {
    match phase {
        ConfiguredPhase::Converge => "converge",
        ConfiguredPhase::SideEffect => "side_effect",
        ConfiguredPhase::Verify => "verify",
        _ => unreachable!("unsupported sequence phase was rejected during parsing"),
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
scenarios:
  default:
    create:
      dummy:
    prepare:
      dummy:
    converge:
      dummy:
    side_effect:
      dummy:
    cleanup:
      dummy:
    destroy:
      dummy:
    verify:
      - {name: smoke, dummy: {}}
    nested:
      - name: restart
        create:
          dummy:
        converge:
          dummy:
        destroy:
          dummy:
        verify:
          - {name: after-restart, dummy: {}}
        nested:
          - name: deep
            create:
              dummy:
            verify:
            destroy:
              dummy:
      - name: ignored
        create:
          dummy:
        verify:
        destroy:
          dummy:
  independent:
    create:
      dummy:
    verify:
    destroy:
      dummy:
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
            state.scenarios["default"].phases[&LifecyclePhase::SideEffect].status,
            PhaseStatus::Pass
        );
        assert_eq!(
            state.scenarios["default/restart"].test_results[0].status,
            VerifierStatus::Pass
        );
        assert!(state.scenarios.values().all(|scenario| {
            let [resource] = scenario.resources.resources.as_slice() else {
                return false;
            };
            resource.id == "mock"
                && resource.resource_type == "mock"
                && resource.attributes.is_empty()
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
    fn configured_phases_run_in_lifecycle_order() {
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
        assert!(output.starts_with("Scenario: default\ndefault::create running\n"));
        assert!(output.contains("Scenario: default: passed"));
        assert!(output.contains("default::create running\n"));
        assert!(output.contains("default::converge running\n"));
        let create = output.find("default::create: 1 resource added").unwrap();
        let prepare = output.find("default::prepare pass").unwrap();
        let converge = output.find("default::converge pass").unwrap();
        let idempotence = output.find("default::idempotence skipped").unwrap();
        let side_effect = output.find("default::side_effect pass").unwrap();
        let verify = output.find("default::verify pass").unwrap();
        assert!(create < prepare);
        assert!(prepare < converge);
        assert!(converge < idempotence);
        assert!(idempotence < verify);
        assert!(verify < side_effect);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn sequence_runs_after_regular_phases_and_persists_indexed_results() {
        let config = Config::from_yaml(
            "version: 1\nscenarios:\n  root:\n    verify: {name: baseline, dummy: {}}\n    side_effect: {dummy: {}}\n    sequence:\n      - verify: {name: check, dummy: {}}\n      - side_effect: {dummy: {}}\n      - converge: {dummy: {}}\n    cleanup: {dummy: {}}\n",
        ).unwrap();
        let (store, directory) = test_store("sequence");
        let provisioner = DummyProvisioner;
        let converger = DummyConverger;
        let verifier = DummyVerifier;
        let bytes = Rc::new(RefCell::new(Vec::new()));
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("root"), false),
            &provisioner,
            &converger,
            &verifier,
            SharedWriter(bytes.clone()),
        );
        let (_, run_state) = runner.run(Some("root")).unwrap();
        let output = String::from_utf8(bytes.borrow().clone()).unwrap();
        let regular = output.find("root::side_effect pass").unwrap();
        let first = output.find("root::sequence[0]::verify pass").unwrap();
        let last = output.find("root::sequence[2]::converge pass").unwrap();
        let cleanup = output.find("root::cleanup pass").unwrap();
        assert!(regular < first && first < last && last < cleanup);
        assert_eq!(run_state.scenarios["root"].sequence.len(), 3);
        assert_eq!(
            run_state.scenarios["root"]
                .test_results
                .iter()
                .find(|r| r.name == "check")
                .unwrap()
                .sequence_index,
            Some(0)
        );
        let mut report = Vec::new();
        render_state_report(&run_state, &mut report, false).unwrap();
        assert!(
            String::from_utf8(report)
                .unwrap()
                .contains("root::sequence[2]::converge pass")
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn sequence_error_stops_later_entries_and_still_cleans_up() {
        let config = Config::from_yaml(
            "version: 1\nscenarios:\n  root:\n    sequence:\n      - converge: {dummy: {status: error}}\n      - verify: {name: should-not-run, dummy: {}}\n    cleanup: {dummy: {}}\n",
        )
        .unwrap();
        let (store, directory) = test_store("sequence-error");
        let provisioner = DummyProvisioner;
        let converger = DummyConverger;
        let verifier = DummyVerifier;
        let bytes = Rc::new(RefCell::new(Vec::new()));
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("root"), false),
            &provisioner,
            &converger,
            &verifier,
            SharedWriter(bytes.clone()),
        );
        let (outcome, run_state) = runner.run(Some("root")).unwrap();
        let output = String::from_utf8(bytes.borrow().clone()).unwrap();
        assert_eq!(outcome.execution_errors, 1);
        assert!(output.contains("root::sequence[0]::converge error"));
        assert!(output.contains("root::cleanup pass"));
        assert!(!output.contains("sequence[1]"));
        assert_eq!(
            run_state.scenarios["root"].sequence[0].state.status,
            PhaseStatus::Error
        );
        assert!(run_state.scenarios["root"].test_results.is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn side_effect_error_stops_child_work_and_still_unwinds() {
        let config = Config::from_yaml(&CONFIG.replacen(
            "    side_effect:\n      dummy:",
            "    side_effect:\n      dummy:\n        status: error",
            1,
        ))
        .unwrap();
        let (store, directory) = test_store("side-effect-error");
        let (outcome, state) = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default"), false),
            &DummyProvisioner,
            &DummyConverger,
            &DummyVerifier,
            Vec::new(),
        )
        .run(Some("default"))
        .unwrap();

        assert_eq!(outcome.execution_errors, 1);
        let scenario = &state.scenarios["default"];
        assert_eq!(
            scenario.phases[&LifecyclePhase::SideEffect].status,
            PhaseStatus::Error
        );
        assert_eq!(
            scenario.phases[&LifecyclePhase::Verify].status,
            PhaseStatus::Pass
        );
        assert!(!state.scenarios.contains_key("default/restart"));
        assert_eq!(
            scenario.phases[&LifecyclePhase::Cleanup].status,
            PhaseStatus::Pass
        );
        assert_eq!(
            scenario.phases[&LifecyclePhase::Destroy].status,
            PhaseStatus::Pass
        );
        assert_eq!(
            state.primary_error.as_ref().unwrap().phase,
            LifecyclePhase::SideEffect
        );
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
        assert!(output.contains("default::converge running\n"));
        assert!(!output.contains("\x1b[32mdefault::converge running"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn styled_output_colors_skipped_lines_and_preserves_bold_entrance() {
        let config = Config::from_yaml(
            r#"
version: 1
scenarios:
  empty: {}
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
        assert!(output.contains("\x1b[90mempty::prepare skipped\x1b[0m\n"));
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
        assert!(output.contains("\x1b[32mindependent::create: 1 resource added\x1b[0m\n"));
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
        assert!(report.contains("default::idempotence skipped\n"));
        assert!(report.contains("default::side_effect pass\n"));
        assert!(report.contains("Scenario: default/restart\n"));
        assert!(report.contains("default/restart::create: 1 resource added\n"));
        assert!(report.contains("Scenario: default/restart/deep\n"));
        assert!(report.contains("default/restart/deep::destroy: 1 resource deleted\n"));
        assert!(report.contains("Scenario: default/restart/deep: passed\n"));
        assert!(report.contains("Scenario: default/restart: passed\n"));
        assert!(report.contains("Scenario: default: passed\n"));
        let parent_verify = report.find("default::verify skipped\n").unwrap();
        let parent_side_effect = report.find("default::side_effect pass\n").unwrap();
        let child_entrance = report.find("Scenario: default/restart\n").unwrap();
        let child_verdict = report.find("Scenario: default/restart: passed\n").unwrap();
        let parent_cleanup = report.rfind("default::cleanup pass\n").unwrap();
        let parent_destroy = report
            .rfind("default::destroy: 1 resource deleted\n")
            .unwrap();
        let parent_verdict = report.rfind("Scenario: default: passed\n").unwrap();
        assert!(parent_verify < parent_side_effect);
        assert!(parent_side_effect < child_entrance);
        assert!(child_entrance < child_verdict);
        assert!(child_verdict < parent_cleanup);
        assert!(parent_cleanup < parent_destroy);
        assert!(parent_destroy < parent_verdict);
        assert!(report.ends_with(
            "summary: run test-run, created 3 resource(s), destroyed 3 resource(s), 0 error(s), 0 verifier failure(s)\n"
        ));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn styled_state_report_uses_live_colors_and_reports_running_phases() {
        let mut run_state = state(Some("empty"), false);
        run_state.enter_scenario("empty", None);
        run_state.complete_phase("empty", LifecyclePhase::Prepare, PhaseStatus::Skipped);
        run_state.complete_phase("empty", LifecyclePhase::Create, PhaseStatus::Pass);
        run_state.mark_phase_running("empty", LifecyclePhase::Converge);

        let mut report = Vec::new();
        render_state_report(&run_state, &mut report, true).unwrap();
        let report = String::from_utf8(report).unwrap();
        assert!(report.contains("\x1b[1mScenario: empty\x1b[0m\n"));
        assert!(report.contains("\x1b[90mempty::prepare skipped\x1b[0m\n"));
        assert!(report.contains("\x1b[32mempty::create: 0 resource added\x1b[0m\n"));
        assert!(report.contains("empty::converge running\x1b[0m\n"));
        assert!(report.contains("\x1b[31m\x1b[1mScenario: empty: error\x1b[0m\n"));
    }

    struct ErrorStatusVerifier;

    impl Verifier for ErrorStatusVerifier {
        fn verify(
            &self,
            _scenario_path: &str,
            _test_name: &str,
            _: &crate::config::Test,
            _: Option<&std::path::Path>,
            _: &[crate::state::Resource],
            _: &Deadline,
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
            _: &Deadline,
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
            _: &crate::config::PhaseDefinition,
            _: &Deadline,
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
            _: &Deadline,
        ) -> Result<ResourceManifest, ProvisionerError> {
            self.0.borrow_mut().push(format!("create:{scenario_path}"));
            Err(ProvisionerError("create failed".into()))
        }

        fn destroy(
            &self,
            scenario_path: &str,
            _: &ResourceManifest,
            _: &crate::config::PhaseDefinition,
            _: &Deadline,
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
        assert!(output.contains("\x1b[31mdefault::create error\x1b[0m\n"));
        assert!(output.contains("\x1b[31m\x1b[1mScenario: default: error\x1b[0m\n"));
        fs::remove_dir_all(directory).unwrap();
    }

    struct RecordingProvisioner(RefCell<Vec<String>>);

    impl Provisioner for RecordingProvisioner {
        fn create(
            &self,
            scenario_path: &str,
            _: &crate::config::PhaseDefinition,
            _: &Deadline,
        ) -> Result<ResourceManifest, ProvisionerError> {
            self.0.borrow_mut().push(format!("create:{scenario_path}"));
            Ok(ResourceManifest::default())
        }

        fn destroy(
            &self,
            scenario_path: &str,
            _: &ResourceManifest,
            _: &crate::config::PhaseDefinition,
            _: &Deadline,
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
