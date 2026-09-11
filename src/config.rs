use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use thiserror::Error;

pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    version: u32,
    #[serde(default)]
    inventory: Vec<PathBuf>,
    #[serde(default)]
    provisioner: Option<String>,
    converger: String,
    verifier: String,
    scenarios: NamedMap<RawScenario>,
}

#[derive(Debug)]
pub struct Config {
    pub inventory: Vec<PathBuf>,
    pub provisioner: String,
    pub converger: String,
    pub verifier: String,
    pub scenarios: ScenarioMap,
    source_material: String,
}

#[derive(Debug)]
pub struct Scenario {
    phases: BTreeMap<ConfiguredPhase, PhaseDefinition>,
    pub tests: TestMap,
    pub scenarios: ScenarioMap,
}

impl Scenario {
    pub fn has_phase(&self, phase: ConfiguredPhase) -> bool {
        self.phases.contains_key(&phase)
    }

    pub(crate) fn phase(&self, phase: ConfiguredPhase) -> Option<&PhaseDefinition> {
        self.phases.get(&phase)
    }

    #[cfg(test)]
    pub fn phase_value(&self, phase: ConfiguredPhase) -> Option<&serde_yaml::Value> {
        self.phases.get(&phase).map(|definition| &definition._value)
    }
}

#[derive(Debug)]
pub(crate) struct PhaseDefinition {
    _value: serde_yaml::Value,
    ansible: Option<AnsiblePhaseDefinition>,
}

impl PhaseDefinition {
    pub(crate) fn is_dummy_override(&self) -> bool {
        self._value.as_mapping().is_some_and(|mapping| {
            mapping.contains_key(serde_yaml::Value::String("dummy".to_owned()))
        })
    }

    pub(crate) fn dummy_status(&self) -> DummyStatus {
        let serde_yaml::Value::Mapping(action) = &self._value else {
            return DummyStatus::Ok;
        };
        let options = action
            .get(serde_yaml::Value::String("dummy".to_owned()))
            .expect("dummy action mappings are validated");
        parse_dummy_options(options)
            .expect("dummy action options are validated")
            .status
    }

    pub(crate) fn ansible(&self) -> Option<&AnsiblePhaseDefinition> {
        self.ansible.as_ref()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnsiblePhaseDefinition {
    pub(crate) playbook: PathBuf,
    #[serde(default = "empty_json_object")]
    pub(crate) vars: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAnsibleOptions {
    playbook: PathBuf,
    #[serde(default = "empty_json_object")]
    vars: serde_json::Value,
}

fn empty_json_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DummyStatus {
    #[default]
    Ok,
    Error,
    Fail,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DummyOptions {
    status: DummyStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConfiguredPhase {
    Create,
    Prepare,
    Converge,
    Idempotence,
    Verify,
    Cleanup,
    Destroy,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Test {
    #[serde(default)]
    pub(crate) ansible: Option<AnsiblePhaseDefinition>,
    #[serde(default)]
    pub(crate) pytest: Option<PytestDefinition>,
    #[serde(default)]
    pub verifier: Option<String>,
    #[serde(default)]
    pub(crate) status: DummyStatus,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PytestDefinition {
    pub(crate) path: PathBuf,
    #[serde(default)]
    pub(crate) args: Vec<String>,
}

#[derive(Debug, Default)]
pub struct ScenarioMap(IndexMap<String, Scenario>);

impl ScenarioMap {
    pub fn get(&self, name: &str) -> Option<&Scenario> {
        self.0.get(name)
    }

    pub fn iter(&self) -> indexmap::map::Iter<'_, String, Scenario> {
        self.0.iter()
    }
}

#[derive(Debug, Default)]
pub struct TestMap(IndexMap<String, Test>);

impl TestMap {
    pub fn iter(&self) -> indexmap::map::Iter<'_, String, Test> {
        self.0.iter()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScenario {
    #[serde(default)]
    create: AbsentOrValue,
    #[serde(default)]
    prepare: AbsentOrValue,
    #[serde(default)]
    converge: AbsentOrValue,
    #[serde(default)]
    idempotence: AbsentOrValue,
    #[serde(default)]
    verify: AbsentOrValue,
    #[serde(default)]
    cleanup: AbsentOrValue,
    #[serde(default)]
    destroy: AbsentOrValue,
    #[serde(default)]
    nested: Vec<RawNestedScenario>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNestedScenario {
    name: String,
    #[serde(default)]
    include: Option<PathBuf>,
    #[serde(default)]
    create: AbsentOrValue,
    #[serde(default)]
    prepare: AbsentOrValue,
    #[serde(default)]
    converge: AbsentOrValue,
    #[serde(default)]
    idempotence: AbsentOrValue,
    #[serde(default)]
    verify: AbsentOrValue,
    #[serde(default)]
    cleanup: AbsentOrValue,
    #[serde(default)]
    destroy: AbsentOrValue,
    #[serde(default)]
    nested: Vec<RawNestedScenario>,
}

impl RawNestedScenario {
    fn into_scenario(self) -> RawScenario {
        RawScenario {
            create: self.create,
            prepare: self.prepare,
            converge: self.converge,
            idempotence: self.idempotence,
            verify: self.verify,
            cleanup: self.cleanup,
            destroy: self.destroy,
            nested: self.nested,
        }
    }

    fn has_inline_content(&self) -> bool {
        self.create.is_present()
            || self.prepare.is_present()
            || self.converge.is_present()
            || self.idempotence.is_present()
            || self.verify.is_present()
            || self.cleanup.is_present()
            || self.destroy.is_present()
            || !self.nested.is_empty()
    }
}

/// Distinguishes an omitted phase key from an explicitly null phase value.
#[derive(Debug, Default)]
struct AbsentOrValue(Option<serde_yaml::Value>);

impl AbsentOrValue {
    fn is_present(&self) -> bool {
        self.0.is_some()
    }
}

impl<'de> Deserialize<'de> for AbsentOrValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_yaml::Value::deserialize(deserializer).map(|value| Self(Some(value)))
    }
}

#[derive(Debug)]
struct NamedMap<T>(IndexMap<String, T>);

impl<T> Default for NamedMap<T> {
    fn default() -> Self {
        Self(IndexMap::new())
    }
}

impl<'de, T> Deserialize<'de> for NamedMap<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct NamedMapVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T> Visitor<'de> for NamedMapVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = NamedMap<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a mapping with unique names")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut entries = IndexMap::new();
                while let Some((name, value)) = map.next_entry::<String, T>()? {
                    if entries.contains_key(&name) {
                        return Err(de::Error::custom(format!("duplicate name `{name}`")));
                    }
                    entries.insert(name, value);
                }
                Ok(NamedMap(entries))
            }
        }

        deserializer.deserialize_map(NamedMapVisitor(std::marker::PhantomData))
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid configuration: {0}")]
    Parse(#[from] serde_yaml::Error),
    #[error("invalid CVD configuration: {reason}")]
    InvalidDocument { reason: &'static str },
    #[error("unsupported configuration version {found}; supported version is {CONFIG_VERSION}")]
    UnsupportedVersion { found: u32 },
    #[error("invalid top-level {kind}: {reason}")]
    InvalidDefault { kind: &'static str, reason: String },
    #[error("invalid scenario `{path}`: {reason}")]
    InvalidScenario { path: String, reason: String },
    #[error("invalid test `{path}`: {reason}")]
    InvalidTest { path: String, reason: String },
    #[error("cannot read included scenario `{path}`: {source}")]
    ReadInclude {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("included scenario cycle at `{0}`")]
    IncludeCycle(PathBuf),
    #[error("scenario include `{path}` cannot be combined with inline phases or children")]
    IncludeWithInlineContent { path: String },
    #[error("invalid scenario `{scenario}` {phase} phase: {reason}")]
    InvalidPhase {
        scenario: String,
        phase: &'static str,
        reason: String,
    },
}

impl Config {
    #[cfg(test)]
    pub fn from_yaml(input: &str) -> Result<Self, ConfigError> {
        Self::from_yaml_with_base(input, None)
    }

    pub fn from_yaml_at(input: &str, configuration_path: &Path) -> Result<Self, ConfigError> {
        Self::from_yaml_with_base(input, configuration_path.parent())
    }

    fn from_yaml_with_base(input: &str, base: Option<&Path>) -> Result<Self, ConfigError> {
        let document: serde_yaml::Value = serde_yaml::from_str(input)?;
        if !document.is_mapping() {
            let reason = if document.is_sequence() {
                "expected a mapping at the document root, but found a sequence; this may be an Ansible playbook, so select cvd.yaml or cvd.yml instead."
            } else {
                "expected a mapping at the document root with `version`, `converger`, `verifier`, and `scenarios` keys"
            };
            return Err(ConfigError::InvalidDocument { reason });
        }
        let raw: RawConfig = serde_yaml::from_str(input)?;
        if raw.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion { found: raw.version });
        }
        let mut include_stack = Vec::new();
        let mut source_material = input.to_owned();
        let scenarios = resolve_named_scenarios(
            raw.scenarios,
            base,
            &mut include_stack,
            &mut source_material,
            None,
        )?;
        let inventory = raw
            .inventory
            .into_iter()
            .map(|source| {
                let path = base.unwrap_or_else(|| Path::new(".")).join(source);
                fs::canonicalize(&path).map_err(|error| ConfigError::InvalidDefault {
                    kind: "inventory",
                    reason: format!("cannot resolve `{}`: {error}", path.display()),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let config = Self {
            inventory,
            provisioner: raw.provisioner.unwrap_or_else(|| "dummy".to_owned()),
            converger: raw.converger,
            verifier: raw.verifier,
            scenarios,
            source_material,
        };
        config.validate()?;
        Ok(config)
    }

    #[cfg(test)]
    pub fn version(&self) -> u32 {
        CONFIG_VERSION
    }

    pub fn scenario(&self, path: &str) -> Option<&Scenario> {
        let mut segments = path.split('/');
        let first = segments.next()?;
        if first.is_empty() {
            return None;
        }
        let mut scenario = self.scenarios.get(first)?;
        for segment in segments {
            if segment.is_empty() {
                return None;
            }
            scenario = scenario.scenarios.get(segment)?;
        }
        Some(scenario)
    }

    pub fn source_material(&self) -> &str {
        &self.source_material
    }

    #[cfg(test)]
    pub fn scenario_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        collect_paths(&self.scenarios, None, &mut paths);
        paths
    }

    fn validate(&self) -> Result<(), ConfigError> {
        validate_default("provisioner", &self.provisioner, &["dummy", "ansible"])?;
        validate_default("converger", &self.converger, &["dummy", "ansible"])?;
        validate_default("verifier", &self.verifier, &["dummy", "pytest", "ansible"])?;
        validate_scenarios(self, &self.scenarios, None)
    }
}

fn validate_default(
    kind: &'static str,
    implementation: &str,
    supported: &[&str],
) -> Result<(), ConfigError> {
    if supported.contains(&implementation) {
        Ok(())
    } else {
        Err(ConfigError::InvalidDefault {
            kind,
            reason: format!("unsupported implementation `{implementation}`"),
        })
    }
}

fn resolve_named_scenarios(
    raw: NamedMap<RawScenario>,
    base: Option<&Path>,
    include_stack: &mut Vec<PathBuf>,
    source_material: &mut String,
    parent: Option<&str>,
) -> Result<ScenarioMap, ConfigError> {
    let mut scenarios = IndexMap::new();
    for (name, raw_scenario) in raw.0 {
        let path = parent.map_or_else(|| name.clone(), |parent| format!("{parent}/{name}"));
        let scenario = resolve_scenario(raw_scenario, base, include_stack, source_material, &path)?;
        scenarios.insert(name, scenario);
    }
    Ok(ScenarioMap(scenarios))
}

fn resolve_scenario(
    raw: RawScenario,
    base: Option<&Path>,
    include_stack: &mut Vec<PathBuf>,
    source_material: &mut String,
    path: &str,
) -> Result<Scenario, ConfigError> {
    let RawScenario {
        create,
        prepare,
        converge,
        idempotence,
        verify,
        cleanup,
        destroy,
        nested,
    } = raw;
    let mut phases = BTreeMap::new();
    let mut tests = NamedMap::<Test>::default();
    if let Some(value) = verify.0 {
        if !value.is_null() {
            tests = serde_yaml::from_value(value).map_err(|error| ConfigError::InvalidPhase {
                scenario: path.to_owned(),
                phase: "verify",
                reason: format!("verify must be a mapping of named tests: {error}"),
            })?;
        }
        phases.insert(
            ConfiguredPhase::Verify,
            PhaseDefinition {
                _value: serde_yaml::Value::Null,
                ansible: None,
            },
        );
    }

    for (phase, value) in [
        (ConfiguredPhase::Create, create),
        (ConfiguredPhase::Prepare, prepare),
        (ConfiguredPhase::Converge, converge),
        (ConfiguredPhase::Idempotence, idempotence),
        (ConfiguredPhase::Cleanup, cleanup),
        (ConfiguredPhase::Destroy, destroy),
    ] {
        let Some(value) = value.0 else {
            continue;
        };
        validate_phase_value(&value, phase).map_err(|reason| ConfigError::InvalidPhase {
            scenario: path.to_owned(),
            phase: configured_phase_name(phase),
            reason,
        })?;
        let ansible = resolve_ansible_options(&value, phase, base).map_err(|reason| {
            ConfigError::InvalidPhase {
                scenario: path.to_owned(),
                phase: configured_phase_name(phase),
                reason,
            }
        })?;
        phases.insert(
            phase,
            PhaseDefinition {
                _value: value,
                ansible,
            },
        );
    }

    let mut child_scenarios = IndexMap::new();
    for child in nested {
        let name = child.name.clone();
        let child_path = format!("{path}/{name}");
        if child_scenarios.contains_key(&name) {
            return Err(ConfigError::InvalidScenario {
                path: child_path,
                reason: "duplicate sibling name".to_owned(),
            });
        }
        let resolved = if let Some(include) = child.include.clone() {
            if child.has_inline_content() {
                return Err(ConfigError::IncludeWithInlineContent { path: child_path });
            }
            let include_path = base.unwrap_or_else(|| Path::new(".")).join(include);
            let canonical =
                fs::canonicalize(&include_path).map_err(|source| ConfigError::ReadInclude {
                    path: include_path.clone(),
                    source,
                })?;
            if include_stack.contains(&canonical) {
                return Err(ConfigError::IncludeCycle(canonical));
            }
            let text =
                fs::read_to_string(&canonical).map_err(|source| ConfigError::ReadInclude {
                    path: canonical.clone(),
                    source,
                })?;
            source_material.push_str("\n-- included scenario: ");
            source_material.push_str(&canonical.to_string_lossy());
            source_material.push_str(" --\n");
            source_material.push_str(&text);
            let raw_child = serde_yaml::from_str(&text)?;
            include_stack.push(canonical.clone());
            let resolved = resolve_scenario(
                raw_child,
                canonical.parent(),
                include_stack,
                source_material,
                &child_path,
            );
            include_stack.pop();
            resolved?
        } else {
            resolve_scenario(
                child.into_scenario(),
                base,
                include_stack,
                source_material,
                &child_path,
            )?
        };
        child_scenarios.insert(name, resolved);
    }

    for (name, test) in &mut tests.0 {
        if let Some(ansible) = &mut test.ansible {
            let target = base
                .unwrap_or_else(|| Path::new("."))
                .join(&ansible.playbook);
            if !ansible.vars.is_object() || ansible.playbook.as_os_str().is_empty() {
                return Err(ConfigError::InvalidTest {
                    path: format!("{path}::{name}"),
                    reason: "Ansible requires a nonempty playbook and vars mapping".to_owned(),
                });
            }
            ansible.playbook = fs::canonicalize(&target)
                .ok()
                .filter(|p| p.is_file())
                .ok_or_else(|| ConfigError::InvalidTest {
                    path: format!("{path}::{name}"),
                    reason: format!("playbook `{}` was not found", target.display()),
                })?;
        }
        if let Some(pytest) = &mut test.pytest {
            if pytest.path.as_os_str().is_empty() {
                return Err(ConfigError::InvalidTest {
                    path: format!("{path}::{name}"),
                    reason: "pytest path must not be empty".to_owned(),
                });
            }
            let target = base.unwrap_or_else(|| Path::new(".")).join(&pytest.path);
            pytest.path = fs::canonicalize(&target).map_err(|error| ConfigError::InvalidTest {
                path: format!("{path}::{name}"),
                reason: format!("cannot resolve pytest path `{}`: {error}", target.display()),
            })?;
        }
    }
    Ok(Scenario {
        phases,
        tests: TestMap(tests.0),
        scenarios: ScenarioMap(child_scenarios),
    })
}

#[cfg(test)]
fn collect_paths(scenarios: &ScenarioMap, parent: Option<&str>, paths: &mut Vec<String>) {
    for (name, scenario) in scenarios.iter() {
        let path = parent.map_or_else(|| name.clone(), |parent| format!("{parent}/{name}"));
        paths.push(path.clone());
        collect_paths(&scenario.scenarios, Some(&path), paths);
    }
}

fn validate_scenarios(
    config: &Config,
    scenarios: &ScenarioMap,
    parent: Option<&str>,
) -> Result<(), ConfigError> {
    for (name, scenario) in scenarios.iter() {
        let path = parent.map_or_else(|| name.clone(), |parent| format!("{parent}/{name}"));
        validate_name(name).map_err(|reason| ConfigError::InvalidScenario {
            path: path.clone(),
            reason,
        })?;
        for (test_name, test) in scenario.tests.iter() {
            let test_path = format!("{path}::{test_name}");
            validate_name(test_name).map_err(|reason| ConfigError::InvalidTest {
                path: test_path.clone(),
                reason,
            })?;
            let verifier = test.verifier.as_deref().unwrap_or(&config.verifier);
            let reason = match verifier {
                _ if test.ansible.is_some() && verifier != "ansible" => {
                    Some("Ansible options require the ansible verifier".to_owned())
                }
                "ansible" if test.pytest.is_some() => {
                    Some("pytest options require the pytest verifier".to_owned())
                }
                "ansible" if test.ansible.is_none() => {
                    Some("ansible verifier requires `ansible: {playbook: ...}`".to_owned())
                }
                "ansible" if test.status != DummyStatus::Ok => {
                    Some("dummy status controls cannot be used with ansible".to_owned())
                }
                "ansible" => None,
                "dummy" if test.pytest.is_some() => {
                    Some("pytest options require the pytest verifier".to_owned())
                }
                "dummy" => None,
                "pytest" if test.pytest.is_none() => {
                    Some("pytest verifier requires `pytest: {path: ...}`".to_owned())
                }
                "pytest" if test.status != DummyStatus::Ok => {
                    Some("dummy status controls cannot be used with pytest".to_owned())
                }
                "pytest" => None,
                _ => Some(format!("unsupported implementation `{verifier}`")),
            };
            if let Some(reason) = reason {
                return Err(ConfigError::InvalidTest {
                    path: test_path,
                    reason,
                });
            }
        }
        validate_scenarios(config, &scenario.scenarios, Some(&path))?;
    }
    Ok(())
}

fn validate_phase_value(value: &serde_yaml::Value, phase: ConfiguredPhase) -> Result<(), String> {
    match value {
        serde_yaml::Value::Null
        | serde_yaml::Value::Bool(_)
        | serde_yaml::Value::Number(_)
        | serde_yaml::Value::String(_) => Ok(()),
        serde_yaml::Value::Mapping(mapping) => validate_action_mapping(mapping, phase),
        serde_yaml::Value::Sequence(actions) => {
            let strings = actions
                .iter()
                .all(|action| matches!(action, serde_yaml::Value::String(_)));
            let mappings = actions
                .iter()
                .all(|action| matches!(action, serde_yaml::Value::Mapping(_)));
            if !strings && !mappings {
                return Err(
                    "a list must contain only scalar values or only adapter mappings".to_owned(),
                );
            }
            if mappings {
                for action in actions {
                    let serde_yaml::Value::Mapping(mapping) = action else {
                        unreachable!("all list items were checked as mappings");
                    };
                    if mapping.contains_key(serde_yaml::Value::String("ansible".to_owned())) {
                        return Err(
                            "Ansible phases require one adapter mapping, not a list".to_owned()
                        );
                    }
                    validate_action_mapping(mapping, phase)?;
                }
            }
            Ok(())
        }
        serde_yaml::Value::Tagged(_) => Err("tagged YAML values are not supported".to_owned()),
    }
}

fn validate_action_mapping(
    mapping: &serde_yaml::Mapping,
    phase: ConfiguredPhase,
) -> Result<(), String> {
    if mapping.len() != 1 {
        return Err("an adapter mapping must contain exactly one adapter name".to_owned());
    }
    let adapter = mapping.keys().next().expect("mapping length was checked");
    let serde_yaml::Value::String(adapter) = adapter else {
        return Err("an adapter name must be a string".to_owned());
    };
    let options = mapping.values().next().expect("mapping length was checked");
    match adapter.as_str() {
        "dummy" => {
            let options = parse_dummy_options(options)?;
            if options.status == DummyStatus::Fail {
                return Err("dummy phase status must be `ok` or `error`".to_owned());
            }
        }
        "ansible"
            if matches!(
                phase,
                ConfiguredPhase::Create
                    | ConfiguredPhase::Destroy
                    | ConfiguredPhase::Prepare
                    | ConfiguredPhase::Converge
                    | ConfiguredPhase::Cleanup
            ) =>
        {
            let _: RawAnsibleOptions = serde_yaml::from_value(options.clone())
                .map_err(|error| format!("invalid Ansible options: {error}"))?;
        }
        "ansible" => {
            return Err(
                "Ansible is supported only for create, prepare, converge, cleanup, and destroy"
                    .to_owned(),
            );
        }
        _ => return Err(format!("unsupported implementation `{adapter}`")),
    }
    Ok(())
}

fn parse_dummy_options(value: &serde_yaml::Value) -> Result<DummyOptions, String> {
    if matches!(value, serde_yaml::Value::Null) {
        return Ok(DummyOptions::default());
    }
    serde_yaml::from_value(value.clone()).map_err(|error| format!("invalid dummy options: {error}"))
}

fn resolve_ansible_options(
    value: &serde_yaml::Value,
    phase: ConfiguredPhase,
    base: Option<&Path>,
) -> Result<Option<AnsiblePhaseDefinition>, String> {
    let serde_yaml::Value::Mapping(mapping) = value else {
        return Ok(None);
    };
    let key = serde_yaml::Value::String("ansible".to_owned());
    let Some(options) = mapping.get(&key) else {
        return Ok(None);
    };
    if !matches!(
        phase,
        ConfiguredPhase::Create
            | ConfiguredPhase::Destroy
            | ConfiguredPhase::Prepare
            | ConfiguredPhase::Converge
            | ConfiguredPhase::Cleanup
    ) {
        return Err(
            "Ansible is supported only for create, prepare, converge, cleanup, and destroy"
                .to_owned(),
        );
    }
    let raw: RawAnsibleOptions = serde_yaml::from_value(options.clone())
        .map_err(|error| format!("invalid Ansible options: {error}"))?;
    if !raw.vars.is_object() {
        return Err("Ansible `vars` must be a mapping".to_owned());
    }
    let path = base.unwrap_or_else(|| Path::new(".")).join(raw.playbook);
    let playbook = fs::canonicalize(&path)
        .ok()
        .filter(|path| path.is_file())
        .ok_or_else(|| format!("playbook `{}` was not found", path.display()))?;
    Ok(Some(AnsiblePhaseDefinition {
        playbook,
        vars: raw.vars,
    }))
}

fn configured_phase_name(phase: ConfiguredPhase) -> &'static str {
    match phase {
        ConfiguredPhase::Create => "create",
        ConfiguredPhase::Prepare => "prepare",
        ConfiguredPhase::Converge => "converge",
        ConfiguredPhase::Idempotence => "idempotence",
        ConfiguredPhase::Verify => "verify",
        ConfiguredPhase::Cleanup => "cleanup",
        ConfiguredPhase::Destroy => "destroy",
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name must not be empty".to_owned());
    }
    if name == "." || name == ".." {
        return Err("name must not be `.` or `..`".to_owned());
    }
    if name.contains('/') {
        return Err("name must not contain `/` because paths use `/` as a separator".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{CONFIG_VERSION, Config, ConfigError, ConfiguredPhase};

    const NESTED: &str = r#"
version: 1
provisioner: dummy
converger: dummy
verifier: dummy
scenarios:
  default:
    create:
    prepare:
      - dummy:
    converge: site.yml
    cleanup:
    destroy:
    verify:
      smoke: {}
    nested:
      - name: restart
        create:
          dummy:
        converge:
        verify:
        destroy:
        nested:
          - name: after
            create:
            verify:
            destroy:
  independent:
    create:
    verify:
    destroy:
"#;

    #[test]
    fn validates_ansible_verifier_options() {
        let root = std::env::temp_dir().join(format!("cvd-verify-config-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("verify.yml"), "---\n").unwrap();
        let base = "version: 1\nconverger: dummy\nverifier: ansible\nscenarios:\n  root:\n    verify:\n      check:\n";
        let valid = format!("{base}        ansible: {{playbook: verify.yml}}\n");
        let config = Config::from_yaml_at(&valid, &root.join("cvd.yml")).unwrap();
        let test = config
            .scenario("root")
            .unwrap()
            .tests
            .iter()
            .next()
            .unwrap()
            .1;
        assert_eq!(
            test.ansible.as_ref().unwrap().playbook,
            root.join("verify.yml")
        );
        assert!(test.ansible.as_ref().unwrap().vars.is_object());
        for invalid in [
            format!("{base}        status: ok\n"),
            valid.replace("verify.yml", "missing.yml"),
            valid.replace("playbook: verify.yml", "playbook: verify.yml, vars: []"),
            valid.replace(
                "playbook: verify.yml",
                "playbook: verify.yml, unknown: true",
            ),
            format!("{valid}        status: fail\n"),
            format!("{valid}        verifier: dummy\n"),
        ] {
            assert!(
                Config::from_yaml_at(&invalid, &root.join("cvd.yml")).is_err(),
                "{invalid}"
            );
        }
        assert!(
            Config::from_yaml_at(
                &format!("{valid}        verifier: ansible\n")
                    .replace("verifier: ansible\nscenarios", "verifier: dummy\nscenarios"),
                &root.join("cvd.yml")
            )
            .is_ok()
        );
    }

    #[test]
    fn parses_phase_payloads_tests_and_ordered_nested_scenarios() {
        let config = Config::from_yaml(NESTED).unwrap();
        assert_eq!(config.version(), CONFIG_VERSION);
        assert_eq!(config.converger, "dummy");
        assert_eq!(
            config.scenario_paths(),
            [
                "default",
                "default/restart",
                "default/restart/after",
                "independent"
            ]
        );
        let default = config.scenario("default").unwrap();
        assert!(default.has_phase(ConfiguredPhase::Prepare));
        assert_eq!(
            default.phase_value(ConfiguredPhase::Converge),
            Some(&serde_yaml::Value::String("site.yml".to_owned()))
        );
        assert!(
            !config
                .scenario("default/restart")
                .unwrap()
                .has_phase(ConfiguredPhase::Prepare)
        );
    }

    #[test]
    fn resolves_scenario_fragment_includes_relative_to_the_including_file() {
        let directory =
            std::env::temp_dir().join(format!("cvd-config-include-{}", std::process::id()));
        let nested_directory = directory.join("nested");
        fs::create_dir_all(&nested_directory).unwrap();
        fs::write(
            nested_directory.join("child.yml"),
            "create:\nverify:\ndestroy:\n",
        )
        .unwrap();
        let root = directory.join("cvd.yml");
        let yaml = NESTED.replacen(
            "      - name: restart\n        create:\n          dummy:\n        converge:\n        verify:\n        destroy:\n        nested:\n          - name: after\n            create:\n            verify:\n            destroy:\n",
            "      - name: restart\n        include: nested/child.yml\n",
            1,
        );
        let config = Config::from_yaml_at(&yaml, &root).unwrap();
        assert!(
            config
                .scenario("default/restart")
                .unwrap()
                .has_phase(ConfiguredPhase::Create)
        );
        let first_source = config.source_material().to_owned();
        fs::write(
            nested_directory.join("child.yml"),
            "create:\nprepare:\nverify:\ndestroy:\n",
        )
        .unwrap();
        let changed = Config::from_yaml_at(&yaml, &root).unwrap();
        assert_ne!(first_source, changed.source_material());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repository_example_uses_the_supported_structure() {
        let path = fs::canonicalize("examples/dummy/cvd.yml").unwrap();
        let yaml = fs::read_to_string(&path).unwrap();
        let config = Config::from_yaml_at(&yaml, &path).unwrap();
        assert_eq!(config.scenarios.iter().count(), 1);
        let scenario = config.scenario("default").unwrap();
        assert!(scenario.has_phase(ConfiguredPhase::Create));
        assert!(scenario.has_phase(ConfiguredPhase::Destroy));
        assert!(scenario.scenarios.iter().next().is_none());
    }

    #[test]
    fn docker_example_resolves_native_inventory_and_converger() {
        let path = fs::canonicalize("examples/ansible-docker/cvd.yml").unwrap();
        let yaml = fs::read_to_string(&path).unwrap();
        let config = Config::from_yaml_at(&yaml, &path).unwrap();
        assert_eq!(config.converger, "ansible");
        assert_eq!(
            config.inventory,
            vec![fs::canonicalize("examples/ansible-docker/inventory.yml").unwrap()]
        );
        assert!(
            config
                .scenarios
                .iter()
                .next()
                .unwrap()
                .1
                .phase(ConfiguredPhase::Converge)
                .unwrap()
                .ansible()
                .is_some()
        );
        let missing = yaml.replace("inventory.yml", "missing-inventory.yml");
        assert!(matches!(
            Config::from_yaml_at(&missing, &path),
            Err(ConfigError::InvalidDefault {
                kind: "inventory",
                ..
            })
        ));
        for phase in ["prepare", "cleanup"] {
            let existing = format!("    {phase}:\n      ansible:\n        playbook: {phase}.yml\n");
            let yaml = yaml
                .replace(&existing, "")
                .replace("    converge:", &format!("    {phase}:"));
            Config::from_yaml_at(&yaml, &path).unwrap();
        }
        for phase in ["verify", "idempotence"] {
            let yaml = yaml
                .replace("    verify:\n", "")
                .replace("    converge:", &format!("    {phase}:"));
            assert!(matches!(
                Config::from_yaml_at(&yaml, &path),
                Err(ConfigError::InvalidPhase { .. })
            ));
        }
        let list = yaml.replace(
            "    converge:\n      ansible:\n        playbook: converge.yml",
            "    converge:\n      - ansible:\n          playbook: converge.yml",
        );
        assert!(matches!(
            Config::from_yaml_at(&list, &path),
            Err(ConfigError::InvalidPhase { .. })
        ));
    }

    #[test]
    fn pytest_paths_follow_included_scenarios_and_validate_adapter_options() {
        let directory =
            std::env::temp_dir().join(format!("cvd-pytest-config-{}", std::process::id()));
        let child = directory.join("nested");
        fs::create_dir_all(&child).unwrap();
        fs::write(child.join("test_host.py"), "def test_host(): pass\n").unwrap();
        let fragment = "verify:\n  host:\n    verifier: pytest\n    pytest:\n      path: test_host.py\n      args: ['-k', 'a name']\n";
        fs::write(child.join("scenario.yml"), fragment).unwrap();
        let yaml = "version: 1\nconverger: dummy\nverifier: dummy\nscenarios:\n  root:\n    nested:\n      - name: child\n        include: nested/scenario.yml\n";
        let root = directory.join("cvd.yml");
        let config = Config::from_yaml_at(yaml, &root).unwrap();
        let (_, test) = config
            .scenario("root/child")
            .unwrap()
            .tests
            .iter()
            .next()
            .unwrap();
        let pytest = test.pytest.as_ref().unwrap();
        assert_eq!(
            pytest.path,
            fs::canonicalize(child.join("test_host.py")).unwrap()
        );
        assert_eq!(pytest.args, ["-k", "a name"]);
        for bad in [
            fragment.replace("test_host.py", "absent.py"),
            fragment.replace("test_host.py", "''"),
            fragment.replace("    verifier: pytest", "    verifier: dummy"),
            fragment.replace(
                "    pytest:\n      path: test_host.py\n      args: ['-k', 'a name']\n",
                "",
            ),
            fragment.replace(
                "    verifier: pytest",
                "    verifier: pytest\n    status: fail",
            ),
            fragment.replace("args: ['-k', 'a name']", "args: '-k name'"),
        ] {
            fs::write(child.join("scenario.yml"), bad).unwrap();
            assert!(Config::from_yaml_at(yaml, &root).is_err());
        }
        fs::write(
            child.join("scenario.yml"),
            fragment.replace("    verifier: pytest\n", ""),
        )
        .unwrap();
        Config::from_yaml_at(&yaml.replace("verifier: dummy", "verifier: pytest"), &root).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_missing_and_cyclic_includes() {
        let directory =
            std::env::temp_dir().join(format!("cvd-config-cycle-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let root = directory.join("cvd.yml");
        let root_yaml = r#"
version: 1
provisioner: dummy
converger: dummy
verifier: dummy
scenarios:
  default:
    nested:
      - name: loop
        include: loop.yml
"#;
        assert!(matches!(
            Config::from_yaml_at(root_yaml, &root),
            Err(ConfigError::ReadInclude { .. })
        ));

        fs::write(
            directory.join("loop.yml"),
            "nested:\n  - name: again\n    include: loop.yml\n",
        )
        .unwrap();
        assert!(matches!(
            Config::from_yaml_at(root_yaml, &root),
            Err(ConfigError::IncludeCycle(_))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_unknown_fields_bad_names_duplicate_children_and_inline_include_content() {
        let unknown = NESTED.replacen("    create:", "    unexpected: true\n    create:", 1);
        assert!(matches!(
            Config::from_yaml(&unknown),
            Err(ConfigError::Parse(_))
        ));

        let bad_name = NESTED.replace("  default:", "  bad/path:");
        assert!(matches!(
            Config::from_yaml(&bad_name),
            Err(ConfigError::InvalidScenario { .. })
        ));

        let duplicate = NESTED.replacen(
            "      - name: restart\n",
            "      - name: restart\n        create:\n      - name: restart\n",
            1,
        );
        assert!(matches!(
            Config::from_yaml(&duplicate),
            Err(ConfigError::InvalidScenario { .. })
        ));

        let include_and_inline = NESTED.replacen(
            "      - name: restart\n",
            "      - name: restart\n        include: child.yml\n",
            1,
        );
        assert!(matches!(
            Config::from_yaml(&include_and_inline),
            Err(ConfigError::IncludeWithInlineContent { .. })
        ));
    }

    #[test]
    fn rejects_unknown_implementations_and_duplicate_test_names() {
        let implementation = NESTED.replacen("converger: dummy", "converger: unknown", 1);
        assert!(matches!(
            Config::from_yaml(&implementation),
            Err(ConfigError::InvalidDefault { .. })
        ));

        let duplicate_test =
            NESTED.replacen("      smoke: {}", "      smoke: {}\n      smoke: {}", 1);
        assert!(matches!(
            Config::from_yaml(&duplicate_test),
            Err(ConfigError::Parse(_))
        ));

        let malformed_action = NESTED.replacen(
            "      - dummy:",
            "      - dummy:\n        other: second-action",
            1,
        );
        assert!(matches!(
            Config::from_yaml(&malformed_action),
            Err(ConfigError::InvalidPhase { .. })
        ));
    }

    #[test]
    fn validates_dummy_phase_and_test_statuses() {
        let valid = NESTED
            .replacen(
                "    create:",
                "    create:\n      dummy:\n        status: error",
                1,
            )
            .replacen("      smoke: {}", "      smoke:\n        status: fail", 1);
        Config::from_yaml(&valid).unwrap();

        for options in ["status: fail", "status: unknown", "unknown: true"] {
            let invalid = NESTED.replacen(
                "    create:",
                &format!("    create:\n      dummy:\n        {options}"),
                1,
            );
            assert!(matches!(
                Config::from_yaml(&invalid),
                Err(ConfigError::InvalidPhase { .. })
            ));
        }

        let invalid_test = NESTED.replacen(
            "      smoke: {}",
            "      smoke:\n        status: unknown",
            1,
        );
        assert!(matches!(
            Config::from_yaml(&invalid_test),
            Err(ConfigError::InvalidPhase { .. })
        ));
    }

    #[test]
    fn verify_contains_tests_and_rejects_legacy_or_opaque_payloads() {
        let base = "version: 1\nconverger: dummy\nverifier: dummy\nscenarios:\n  root:\n";
        for value in ["    verify:\n", "    verify: {}\n"] {
            let config = Config::from_yaml(&format!("{base}{value}")).unwrap();
            let scenario = config.scenario("root").unwrap();
            assert!(scenario.has_phase(ConfiguredPhase::Verify));
            assert_eq!(scenario.tests.iter().count(), 0);
        }
        let config = Config::from_yaml(&format!("{base}    create:\n")).unwrap();
        assert!(
            !config
                .scenario("root")
                .unwrap()
                .has_phase(ConfiguredPhase::Verify)
        );
        let config = Config::from_yaml(&format!(
            "{base}    verify:\n      z: {{}}\n      a: {{status: fail}}\n"
        ))
        .unwrap();
        assert_eq!(
            config
                .scenario("root")
                .unwrap()
                .tests
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["z", "a"]
        );
        for value in [
            "    tests: {}\n",
            "    verify: script.py\n",
            "    verify: []\n",
            "    verify:\n      same: {}\n      same: {}\n",
            "    verify: {pytest: {path: test.py}}\n",
        ] {
            assert!(
                Config::from_yaml(&format!("{base}{value}")).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn rejects_unsupported_version() {
        let config = NESTED.replacen("version: 1", "version: 2", 1);
        assert!(matches!(
            Config::from_yaml(&config),
            Err(ConfigError::UnsupportedVersion { found: 2 })
        ));
    }
}
