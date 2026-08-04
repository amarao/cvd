//! Recursive lifecycle execution for the dummy stub.

use std::{
    io::Write,
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;

use crate::{
    config::{Config, Scenario},
    provisioner::Provisioner,
    state::{
        ErrorRecord, LifecyclePhase, PhaseStatus, RunState, StateError, StateStore, SuiteResult,
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
    verifier: &'a dyn Verifier,
    output: W,
    outcome: RunOutcome,
}

impl<'a, W: Write> LifecycleRunner<'a, W> {
    pub fn new(
        configuration: &'a Config,
        store: &'a StateStore,
        state: RunState,
        provisioner: &'a dyn Provisioner,
        verifier: &'a dyn Verifier,
        output: W,
    ) -> Self {
        Self {
            configuration,
            store,
            state,
            provisioner,
            verifier,
            output,
            outcome: RunOutcome::default(),
        }
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
        writeln!(
            self.output,
            "summary: {} error(s), {} verifier failure(s)",
            self.outcome.execution_errors, self.outcome.verifier_failures
        )?;
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
        self.state.enter_scenario(path, parent_path);
        // Keep the externally visible phase order even for unimplemented
        // phases.  A skipped dependency is recorded before create begins.
        if self.skip(path, LifecyclePhase::Dependency).is_err() {
            return true;
        }
        if self
            .persist_or_record(path, LifecyclePhase::Create)
            .is_err()
        {
            return true;
        }

        let resources = match self.provisioner.create(path) {
            Ok(resources) => resources,
            Err(error) => {
                self.execution_error(path, LifecyclePhase::Create, error.to_string());
                return self.finish_after_failure(path);
            }
        };
        self.state.set_resources(path, resources);
        if self
            .complete(path, LifecyclePhase::Create, PhaseStatus::Pass)
            .is_err()
        {
            return self.finish_after_failure(path);
        }

        // These phases are part of the persisted public state even though the
        // dummy implementation does not implement them.  They are recorded
        // only after create has completed, matching lifecycle order.
        for phase in [
            LifecyclePhase::Prepare,
            LifecyclePhase::Converge,
            LifecyclePhase::Idempotence,
        ] {
            if self.skip(path, phase).is_err() {
                return self.finish_after_failure(path);
            }
        }

        let had_error = if let Some((child_name, tail)) = remaining_selector.split_first() {
            // Ancestors intentionally do not verify: they only establish the
            // resource state required by the selected child.
            if self.skip(path, LifecyclePhase::Verify).is_err() {
                return self.finish_after_failure(path);
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
                return self.finish_after_failure(path);
            };
            let child_path = path_with_child(path, child_name);
            self.run_scenario(child, &child_path, Some(path.to_owned()), tail)
        } else {
            let mut had_error = self.verify(path, scenario);
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

        let cleanup_error = self.skip(path, LifecyclePhase::Cleanup).is_err();
        // Cleanup and destruction are best-effort even after a child or
        // verifier error.  Do not short-circuit destruction on `had_error`.
        let destroy_error = self.destroy(path);
        had_error || cleanup_error || destroy_error
    }

    fn verify(&mut self, path: &str, scenario: &Scenario) -> bool {
        if self
            .persist_or_record(path, LifecyclePhase::Verify)
            .is_err()
        {
            return true;
        }

        for (suite_name, _) in scenario.suites.iter() {
            match self.verifier.verify(path, suite_name) {
                Ok(status) => {
                    let verifier_error = status == VerifierStatus::Error;
                    if status == VerifierStatus::Fail {
                        self.outcome.verifier_failures += 1;
                    }
                    self.state.record_suite_result(
                        path,
                        SuiteResult {
                            name: suite_name.clone(),
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
                            format!("verifier suite `{suite_name}` reported an execution error"),
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

    fn destroy(&mut self, path: &str) -> bool {
        if self.state.keep {
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
        match self.provisioner.destroy(path, &resources) {
            Ok(()) => {
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

    fn finish_after_failure(&mut self, path: &str) -> bool {
        // No later child or verification work is allowed after a failure.
        // Cleanup has no dummy implementation but remains explicit. Ignore
        // its state-write result so destruction is always attempted.
        let _ = self.skip(path, LifecyclePhase::Cleanup);
        let _ = self.destroy(path);
        true
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
        writeln!(
            self.output,
            "{path} {} {}",
            phase_name(&phase),
            status_name(&status)
        )?;
        Ok(())
    }

    fn skip(&mut self, path: &str, phase: LifecyclePhase) -> Result<(), LifecycleError> {
        self.complete(path, phase, PhaseStatus::Skipped)
    }
}

fn path_with_child(parent: &str, child: &str) -> String {
    format!("{parent}/{child}")
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
        provisioner::{DummyProvisioner, ProvisionerError},
        state::{PhaseStatus, ResourceManifest},
        verifier::{DummyVerifier, Verifier},
    };

    static NEXT_TEST_PATH: AtomicU64 = AtomicU64::new(0);

    const CONFIG: &str = r#"
version: 1
scenarios:
  default:
    provisioner: dummy
    suites:
      smoke:
        verifier: dummy
    scenarios:
      restart:
        provisioner: dummy
        suites:
          after-restart:
            verifier: dummy
        scenarios:
          deep:
            provisioner: dummy
      ignored:
        provisioner: dummy
  independent:
    provisioner: dummy
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
        let verifier = DummyVerifier;
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default/restart"), false),
            &provisioner,
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
            state.scenarios["default/restart"].suite_results[0].status,
            VerifierStatus::Pass
        );
        assert!(
            state
                .scenarios
                .values()
                .all(|scenario| scenario.resources.resources.is_empty())
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn create_completes_before_later_unimplemented_phases_are_recorded() {
        let config = Config::from_yaml(CONFIG).unwrap();
        let (store, directory) = test_store("create-order");
        let provisioner = DummyProvisioner;
        let verifier = DummyVerifier;
        let bytes = Rc::new(RefCell::new(Vec::new()));
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default"), false),
            &provisioner,
            &verifier,
            SharedWriter(bytes.clone()),
        );

        runner.run(Some("default")).unwrap();
        let output = String::from_utf8(bytes.borrow().clone()).unwrap();
        let create = output.find("default create pass").unwrap();
        let prepare = output.find("default prepare skipped").unwrap();
        assert!(create < prepare);
        std::fs::remove_dir_all(directory).unwrap();
    }

    struct ErrorStatusVerifier;

    impl Verifier for ErrorStatusVerifier {
        fn verify(
            &self,
            _scenario_path: &str,
            _suite_name: &str,
        ) -> Result<VerifierStatus, crate::verifier::VerifierError> {
            Ok(VerifierStatus::Error)
        }
    }

    #[test]
    fn verifier_error_status_is_persisted_and_stops_normal_work() {
        let config = Config::from_yaml(CONFIG).unwrap();
        let (store, directory) = test_store("verifier-status-error");
        let provisioner = DummyProvisioner;
        let verifier = ErrorStatusVerifier;
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default"), false),
            &provisioner,
            &verifier,
            Vec::new(),
        );

        let (outcome, state) = runner.run(Some("default")).unwrap();
        assert!(!outcome.succeeded());
        assert_eq!(outcome.execution_errors, 1);
        assert_eq!(
            state.scenarios["default"].suite_results[0].status,
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
        fn create(&self, scenario_path: &str) -> Result<ResourceManifest, ProvisionerError> {
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
        let verifier = DummyVerifier;
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default"), false),
            &provisioner,
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
        fn create(&self, scenario_path: &str) -> Result<ResourceManifest, ProvisionerError> {
            self.0.borrow_mut().push(format!("create:{scenario_path}"));
            Err(ProvisionerError("create failed".into()))
        }

        fn destroy(
            &self,
            scenario_path: &str,
            _: &ResourceManifest,
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
        let verifier = DummyVerifier;
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default"), false),
            &provisioner,
            &verifier,
            Vec::new(),
        );

        let (outcome, state) = runner.run(Some("default")).unwrap();
        assert!(!outcome.succeeded());
        assert_eq!(outcome.execution_errors, 1);
        assert_eq!(
            state.primary_error.as_ref().unwrap().phase,
            LifecyclePhase::Create
        );
        assert_eq!(
            state.scenarios["default"].phases[&LifecyclePhase::Cleanup].status,
            PhaseStatus::Skipped
        );
        assert_eq!(
            state.scenarios["default"].phases[&LifecyclePhase::Destroy].status,
            PhaseStatus::Pass
        );
        assert_eq!(
            provisioner.0.into_inner(),
            ["create:default", "destroy:default"]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    struct RecordingProvisioner(RefCell<Vec<String>>);

    impl Provisioner for RecordingProvisioner {
        fn create(&self, scenario_path: &str) -> Result<ResourceManifest, ProvisionerError> {
            self.0.borrow_mut().push(format!("create:{scenario_path}"));
            Ok(ResourceManifest::default())
        }

        fn destroy(
            &self,
            scenario_path: &str,
            _: &ResourceManifest,
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
        let verifier = DummyVerifier;
        let runner = LifecycleRunner::new(
            &config,
            &store,
            state(Some("default/restart"), false),
            &provisioner,
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
