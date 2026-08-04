//! Versioned, durable state for a CVD run.
//!
//! This module deliberately stores resource data as a general manifest rather
//! than as an Ansible inventory.  Provisioners own the meaning of resources;
//! CVD only persists the manifest they report.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

pub const STATE_SCHEMA_VERSION: u32 = 1;

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

/// The dummy-stub default, colocated with the selected configuration file.
pub fn default_state_path(configuration_path: &Path) -> PathBuf {
    configuration_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".cvd")
        .join("state.json")
}

/// A millisecond Unix timestamp.  An integer keeps state self-contained and
/// avoids choosing a date/time dependency for the stub.
pub type Timestamp = u64;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePhase {
    Dependency,
    Create,
    Prepare,
    Converge,
    Idempotence,
    Verify,
    Cleanup,
    Destroy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStatus {
    Pending,
    Running,
    Skipped,
    Pass,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseState {
    pub status: PhaseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<Timestamp>,
}

impl PhaseState {
    pub fn running() -> Self {
        Self {
            status: PhaseStatus::Running,
            started_at: Some(now()),
            completed_at: None,
        }
    }

    pub fn completed(status: PhaseStatus, started_at: Option<Timestamp>) -> Self {
        debug_assert!(matches!(
            status,
            PhaseStatus::Skipped | PhaseStatus::Pass | PhaseStatus::Error
        ));
        let timestamp = now();
        Self {
            status,
            started_at: started_at.or(Some(timestamp)),
            completed_at: Some(timestamp),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifierStatus {
    Pass,
    Fail,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuiteResult {
    pub name: String,
    pub status: VerifierStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub recorded_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    pub id: String,
    #[serde(rename = "type")]
    pub resource_type: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub relationships: BTreeSet<String>,
    /// Attribute names omitted from serialized views in a later resource
    /// protocol.  The dummy provisioner always returns an empty manifest.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub sensitive_attributes: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceManifest {
    #[serde(default)]
    pub resources: Vec<Resource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorRecord {
    pub scenario_path: String,
    pub phase: LifecyclePhase,
    pub message: String,
    pub recorded_at: Timestamp,
}

impl ErrorRecord {
    pub fn new(
        scenario_path: impl Into<String>,
        phase: LifecyclePhase,
        message: impl Into<String>,
    ) -> Self {
        Self {
            scenario_path: scenario_path.into(),
            phase,
            message: message.into(),
            recorded_at: now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioState {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_path: Option<String>,
    #[serde(default)]
    pub phases: BTreeMap<LifecyclePhase, PhaseState>,
    #[serde(default)]
    pub suite_results: Vec<SuiteResult>,
    #[serde(default)]
    pub resources: ResourceManifest,
}

impl ScenarioState {
    pub fn new(path: impl Into<String>, parent_path: Option<String>) -> Self {
        Self {
            path: path.into(),
            parent_path,
            phases: BTreeMap::new(),
            suite_results: Vec::new(),
            resources: ResourceManifest::default(),
        }
    }
}

/// The complete durable record of a single CVD invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub schema_version: u32,
    pub run_id: String,
    pub configuration_path: PathBuf,
    pub configuration_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_scenario: Option<String>,
    pub keep: bool,
    #[serde(default)]
    pub scenarios: BTreeMap<String, ScenarioState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_error: Option<ErrorRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleanup_errors: Vec<ErrorRecord>,
    pub started_at: Timestamp,
    pub updated_at: Timestamp,
}

impl RunState {
    pub fn new(
        run_id: impl Into<String>,
        configuration_path: PathBuf,
        configuration_fingerprint: impl Into<String>,
        requested_scenario: Option<String>,
        keep: bool,
    ) -> Self {
        let timestamp = now();
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            run_id: run_id.into(),
            configuration_path,
            configuration_fingerprint: configuration_fingerprint.into(),
            requested_scenario,
            keep,
            scenarios: BTreeMap::new(),
            primary_error: None,
            cleanup_errors: Vec::new(),
            started_at: timestamp,
            updated_at: timestamp,
        }
    }

    pub fn enter_scenario(&mut self, path: impl Into<String>, parent_path: Option<String>) {
        let path = path.into();
        self.scenarios
            .entry(path.clone())
            .or_insert_with(|| ScenarioState::new(path, parent_path));
        self.touch();
    }

    pub fn mark_phase_running(&mut self, scenario_path: &str, phase: LifecyclePhase) {
        self.scenario_mut(scenario_path)
            .phases
            .insert(phase, PhaseState::running());
        self.touch();
    }

    pub fn complete_phase(
        &mut self,
        scenario_path: &str,
        phase: LifecyclePhase,
        status: PhaseStatus,
    ) {
        assert!(matches!(
            status,
            PhaseStatus::Skipped | PhaseStatus::Pass | PhaseStatus::Error
        ));
        let scenario = self.scenario_mut(scenario_path);
        let started_at = scenario
            .phases
            .get(&phase)
            .and_then(|phase_state| phase_state.started_at);
        scenario
            .phases
            .insert(phase, PhaseState::completed(status, started_at));
        self.touch();
    }

    pub fn record_suite_result(&mut self, scenario_path: &str, result: SuiteResult) {
        self.scenario_mut(scenario_path).suite_results.push(result);
        self.touch();
    }

    pub fn set_resources(&mut self, scenario_path: &str, resources: ResourceManifest) {
        self.scenario_mut(scenario_path).resources = resources;
        self.touch();
    }

    /// Retains the first execution error as the primary error.  Later cleanup
    /// and destroy errors are recorded separately so the original failure is
    /// never lost.
    pub fn record_primary_error(&mut self, error: ErrorRecord) {
        if self.primary_error.is_none() {
            self.primary_error = Some(error);
            self.touch();
        }
    }

    pub fn record_cleanup_error(&mut self, error: ErrorRecord) {
        self.cleanup_errors.push(error);
        self.touch();
    }

    fn scenario_mut(&mut self, scenario_path: &str) -> &mut ScenarioState {
        self.scenarios.get_mut(scenario_path).unwrap_or_else(|| {
            panic!("scenario '{scenario_path}' must be entered before recording state")
        })
    }

    fn touch(&mut self) {
        self.updated_at = now();
    }
}

/// Persisted state reader/writer.  A store owns one target state-file path.
#[derive(Debug, Clone)]
pub struct StateStore {
    path: PathBuf,
}

impl StateStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<RunState, StateError> {
        let file = File::open(&self.path).map_err(|source| StateError::Io {
            path: self.path.clone(),
            source,
        })?;
        let state: RunState = serde_json::from_reader(BufReader::new(file)).map_err(|source| {
            StateError::Deserialize {
                path: self.path.clone(),
                source,
            }
        })?;

        if state.schema_version != STATE_SCHEMA_VERSION {
            return Err(StateError::UnsupportedSchemaVersion {
                path: self.path.clone(),
                found: state.schema_version,
                supported: STATE_SCHEMA_VERSION,
            });
        }

        Ok(state)
    }

    /// Replace the state file atomically after syncing the temporary file.
    pub fn save(&self, state: &RunState) -> Result<(), StateError> {
        if state.schema_version != STATE_SCHEMA_VERSION {
            return Err(StateError::UnsupportedSchemaVersion {
                path: self.path.clone(),
                found: state.schema_version,
                supported: STATE_SCHEMA_VERSION,
            });
        }

        let directory = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(directory).map_err(|source| StateError::Io {
            path: directory.to_path_buf(),
            source,
        })?;

        // `create_new` means a stale temporary file cannot be overwritten.
        // Retry with a fresh same-directory name if a prior process left one.
        for _ in 0..16 {
            let temporary_path = self.temporary_path(directory);
            match self.write_temporary(&temporary_path, state) {
                Ok(()) => {
                    return fs::rename(&temporary_path, &self.path).map_err(|source| {
                        StateError::Io {
                            path: self.path.clone(),
                            source,
                        }
                    });
                }
                Err(StateError::Io { source, .. })
                    if source.kind() == io::ErrorKind::AlreadyExists =>
                {
                    continue;
                }
                Err(error) => {
                    let _ = fs::remove_file(&temporary_path);
                    return Err(error);
                }
            }
        }

        Err(StateError::Io {
            path: self.path.clone(),
            source: io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not allocate a unique temporary state file",
            ),
        })
    }

    fn temporary_path(&self, directory: &Path) -> PathBuf {
        let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("state.json");
        directory.join(format!(".{name}.{}.{}.tmp", std::process::id(), sequence))
    }

    fn write_temporary(&self, temporary_path: &Path, state: &RunState) -> Result<(), StateError> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temporary_path)
            .map_err(|source| StateError::Io {
                path: temporary_path.to_path_buf(),
                source,
            })?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, state).map_err(|source| {
            StateError::Serialize {
                path: temporary_path.to_path_buf(),
                source,
            }
        })?;
        writer.write_all(b"\n").map_err(|source| StateError::Io {
            path: temporary_path.to_path_buf(),
            source,
        })?;
        writer.flush().map_err(|source| StateError::Io {
            path: temporary_path.to_path_buf(),
            source,
        })?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|source| StateError::Io {
                path: temporary_path.to_path_buf(),
                source,
            })
    }
}

#[derive(Debug)]
pub enum StateError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Deserialize {
        path: PathBuf,
        source: serde_json::Error,
    },
    Serialize {
        path: PathBuf,
        source: serde_json::Error,
    },
    UnsupportedSchemaVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "state I/O for {}: {source}", path.display())
            }
            Self::Deserialize { path, source } => {
                write!(
                    formatter,
                    "invalid state JSON in {}: {source}",
                    path.display()
                )
            }
            Self::Serialize { path, source } => {
                write!(
                    formatter,
                    "could not serialize state for {}: {source}",
                    path.display()
                )
            }
            Self::UnsupportedSchemaVersion {
                path,
                found,
                supported,
            } => write!(
                formatter,
                "unsupported state schema version {found} in {} (supports {supported})",
                path.display()
            ),
        }
    }
}

impl Error for StateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Deserialize { source, .. } => Some(source),
            Self::Serialize { source, .. } => Some(source),
            Self::UnsupportedSchemaVersion { .. } => None,
        }
    }
}

fn now() -> Timestamp {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_test_directory(label: &str) -> PathBuf {
        let nonce = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("cvd-state-{label}-{}-{nonce}", std::process::id()))
    }

    fn test_state() -> RunState {
        let mut state = RunState::new(
            "run-1",
            PathBuf::from("/project/cvd.yml"),
            "configuration-sha256",
            Some("default/restart".into()),
            false,
        );
        state.enter_scenario("default", None);
        state.complete_phase("default", LifecyclePhase::Dependency, PhaseStatus::Skipped);
        state.mark_phase_running("default", LifecyclePhase::Create);
        state.complete_phase("default", LifecyclePhase::Create, PhaseStatus::Pass);
        state.enter_scenario("default/restart", Some("default".into()));
        state.record_suite_result(
            "default/restart",
            SuiteResult {
                name: "smoke".into(),
                status: VerifierStatus::Pass,
                message: None,
                recorded_at: now(),
            },
        );
        state
    }

    #[test]
    fn saves_loads_and_creates_parent_directories() {
        let directory = unique_test_directory("round-trip");
        let state_path = directory.join("nested").join("state.json");
        let store = StateStore::new(&state_path);
        let state = test_state();

        store.save(&state).expect("save state");
        let loaded = store.load().expect("load state");

        assert_eq!(loaded, state);
        assert!(state_path.exists());
        let temporary_files = fs::read_dir(state_path.parent().expect("parent"))
            .expect("read state directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(temporary_files, 0);
        fs::remove_dir_all(&directory).expect("remove only test directory");
    }

    #[test]
    fn defaults_state_to_the_configuration_directory() {
        assert_eq!(
            default_state_path(Path::new("project/cvd.yml")),
            PathBuf::from("project/.cvd/state.json")
        );
    }

    #[test]
    fn replacement_keeps_only_the_latest_complete_document() {
        let directory = unique_test_directory("replace");
        let state_path = directory.join("state.json");
        let store = StateStore::new(&state_path);
        let mut original = test_state();
        store.save(&original).expect("save initial state");

        original.keep = true;
        original.complete_phase(
            "default/restart",
            LifecyclePhase::Destroy,
            PhaseStatus::Skipped,
        );
        store.save(&original).expect("replace state");

        assert_eq!(store.load().expect("load replacement"), original);
        fs::remove_dir_all(&directory).expect("remove only test directory");
    }

    #[test]
    fn rejects_a_different_schema_version() {
        let directory = unique_test_directory("schema-version");
        fs::create_dir_all(&directory).expect("create test directory");
        let state_path = directory.join("state.json");
        fs::write(
            &state_path,
            r#"{"schema_version":999,"run_id":"run","configuration_path":"cvd.yml","configuration_fingerprint":"x","keep":false,"started_at":0,"updated_at":0}"#,
        )
        .expect("write test state");

        let error = StateStore::new(&state_path)
            .load()
            .expect_err("reject version");
        assert!(matches!(
            error,
            StateError::UnsupportedSchemaVersion { found: 999, .. }
        ));
        fs::remove_dir_all(&directory).expect("remove only test directory");
    }

    #[test]
    fn primary_error_is_not_replaced_by_cleanup_errors() {
        let mut state = test_state();
        let primary = ErrorRecord::new("default", LifecyclePhase::Create, "create failed");
        state.record_primary_error(primary.clone());
        state.record_primary_error(ErrorRecord::new(
            "default/restart",
            LifecyclePhase::Verify,
            "later failure",
        ));
        state.record_cleanup_error(ErrorRecord::new(
            "default",
            LifecyclePhase::Destroy,
            "destroy failed",
        ));

        assert_eq!(state.primary_error, Some(primary));
        assert_eq!(state.cleanup_errors.len(), 1);
    }

    #[test]
    fn completing_a_running_phase_retains_its_start_timestamp() {
        let mut state = test_state();
        state.enter_scenario("another", None);
        state.mark_phase_running("another", LifecyclePhase::Create);
        let started_at = state.scenarios["another"].phases[&LifecyclePhase::Create]
            .started_at
            .expect("running phase has a start timestamp");

        state.complete_phase("another", LifecyclePhase::Create, PhaseStatus::Pass);
        let completed = &state.scenarios["another"].phases[&LifecyclePhase::Create];
        assert_eq!(completed.started_at, Some(started_at));
        assert!(completed.completed_at.is_some());
    }
}
