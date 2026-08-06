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
    provisioner: String,
    converger: String,
    verifier: String,
    scenarios: NamedMap<RawScenario>,
}

#[derive(Debug)]
pub struct Config {
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
    ansible_playbooks: Vec<PathBuf>,
}

impl PhaseDefinition {
    pub(crate) fn ansible_playbooks(&self) -> &[PathBuf] {
        &self.ansible_playbooks
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConfiguredPhase {
    Dependency,
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
    pub verifier: Option<String>,
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
    dependency: AbsentOrValue,
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
    tests: NamedMap<Test>,
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
    dependency: AbsentOrValue,
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
    tests: NamedMap<Test>,
    #[serde(default)]
    nested: Vec<RawNestedScenario>,
}

impl RawNestedScenario {
    fn into_scenario(self) -> RawScenario {
        RawScenario {
            dependency: self.dependency,
            create: self.create,
            prepare: self.prepare,
            converge: self.converge,
            idempotence: self.idempotence,
            verify: self.verify,
            cleanup: self.cleanup,
            destroy: self.destroy,
            tests: self.tests,
            nested: self.nested,
        }
    }

    fn has_inline_content(&self) -> bool {
        self.dependency.is_present()
            || self.create.is_present()
            || self.prepare.is_present()
            || self.converge.is_present()
            || self.idempotence.is_present()
            || self.verify.is_present()
            || self.cleanup.is_present()
            || self.destroy.is_present()
            || !self.tests.0.is_empty()
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
    #[error("scenario include `{path}` cannot be combined with inline phases, tests, or children")]
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
            &raw.converger,
            None,
        )?;
        let config = Self {
            provisioner: raw.provisioner,
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
        for (kind, implementation) in [
            ("provisioner", self.provisioner.as_str()),
            ("converger", self.converger.as_str()),
            ("verifier", self.verifier.as_str()),
        ] {
            validate_implementation(implementation)
                .map_err(|reason| ConfigError::InvalidDefault { kind, reason })?;
        }
        validate_scenarios(self, &self.scenarios, None)
    }
}

fn resolve_named_scenarios(
    raw: NamedMap<RawScenario>,
    base: Option<&Path>,
    include_stack: &mut Vec<PathBuf>,
    source_material: &mut String,
    default_converger: &str,
    parent: Option<&str>,
) -> Result<ScenarioMap, ConfigError> {
    let mut scenarios = IndexMap::new();
    for (name, raw_scenario) in raw.0 {
        let path = parent.map_or_else(|| name.clone(), |parent| format!("{parent}/{name}"));
        let scenario = resolve_scenario(
            raw_scenario,
            base,
            include_stack,
            source_material,
            default_converger,
            &path,
        )?;
        scenarios.insert(name, scenario);
    }
    Ok(ScenarioMap(scenarios))
}

fn resolve_scenario(
    raw: RawScenario,
    base: Option<&Path>,
    include_stack: &mut Vec<PathBuf>,
    source_material: &mut String,
    default_converger: &str,
    path: &str,
) -> Result<Scenario, ConfigError> {
    let RawScenario {
        dependency,
        create,
        prepare,
        converge,
        idempotence,
        verify,
        cleanup,
        destroy,
        tests,
        nested,
    } = raw;
    let mut phases = BTreeMap::new();
    for (phase, value) in [
        (ConfiguredPhase::Dependency, dependency),
        (ConfiguredPhase::Create, create),
        (ConfiguredPhase::Prepare, prepare),
        (ConfiguredPhase::Converge, converge),
        (ConfiguredPhase::Idempotence, idempotence),
        (ConfiguredPhase::Verify, verify),
        (ConfiguredPhase::Cleanup, cleanup),
        (ConfiguredPhase::Destroy, destroy),
    ] {
        let Some(value) = value.0 else {
            continue;
        };
        validate_phase_value(&value).map_err(|reason| ConfigError::InvalidPhase {
            scenario: path.to_owned(),
            phase: configured_phase_name(phase),
            reason,
        })?;
        let ansible_playbooks = resolve_ansible_playbooks(phase, &value, default_converger, base)
            .map_err(|reason| ConfigError::InvalidPhase {
            scenario: path.to_owned(),
            phase: configured_phase_name(phase),
            reason,
        })?;
        phases.insert(
            phase,
            PhaseDefinition {
                _value: value,
                ansible_playbooks,
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
                default_converger,
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
                default_converger,
                &child_path,
            )?
        };
        child_scenarios.insert(name, resolved);
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
        for (phase, definition) in &scenario.phases {
            validate_phase_value(&definition._value).map_err(|reason| {
                ConfigError::InvalidScenario {
                    path: path.clone(),
                    reason: format!("invalid {} phase: {reason}", configured_phase_name(*phase)),
                }
            })?;
        }
        for (test_name, test) in scenario.tests.iter() {
            let test_path = format!("{path}::{test_name}");
            validate_name(test_name).map_err(|reason| ConfigError::InvalidTest {
                path: test_path.clone(),
                reason,
            })?;
            let verifier = test.verifier.as_deref().unwrap_or(&config.verifier);
            validate_implementation(verifier).map_err(|reason| ConfigError::InvalidTest {
                path: test_path,
                reason,
            })?;
        }
        validate_scenarios(config, &scenario.scenarios, Some(&path))?;
    }
    Ok(())
}

fn validate_implementation(implementation: &str) -> Result<(), String> {
    if matches!(
        implementation,
        "dummy" | "ansible" | "terraform" | "pytest" | "exec"
    ) {
        Ok(())
    } else {
        Err(format!("unsupported implementation `{implementation}`"))
    }
}

fn validate_phase_value(value: &serde_yaml::Value) -> Result<(), String> {
    match value {
        serde_yaml::Value::Null
        | serde_yaml::Value::Bool(_)
        | serde_yaml::Value::Number(_)
        | serde_yaml::Value::String(_) => Ok(()),
        serde_yaml::Value::Mapping(mapping) => validate_action_mapping(mapping),
        serde_yaml::Value::Sequence(actions) => {
            let strings = actions
                .iter()
                .all(|action| matches!(action, serde_yaml::Value::String(_)));
            let mappings = actions
                .iter()
                .all(|action| matches!(action, serde_yaml::Value::Mapping(_)));
            if !strings && !mappings {
                return Err(
                    "a list must contain only playbook names or only adapter mappings".to_owned(),
                );
            }
            if mappings {
                for action in actions {
                    let serde_yaml::Value::Mapping(mapping) = action else {
                        unreachable!("all list items were checked as mappings");
                    };
                    validate_action_mapping(mapping)?;
                }
            }
            Ok(())
        }
        serde_yaml::Value::Tagged(_) => Err("tagged YAML values are not supported".to_owned()),
    }
}

fn validate_action_mapping(mapping: &serde_yaml::Mapping) -> Result<(), String> {
    if mapping.len() != 1 {
        return Err("an adapter mapping must contain exactly one adapter name".to_owned());
    }
    let adapter = mapping.keys().next().expect("mapping length was checked");
    let serde_yaml::Value::String(adapter) = adapter else {
        return Err("an adapter name must be a string".to_owned());
    };
    validate_implementation(adapter)
}

fn resolve_ansible_playbooks(
    phase: ConfiguredPhase,
    value: &serde_yaml::Value,
    default_converger: &str,
    base: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    if !matches!(
        phase,
        ConfiguredPhase::Prepare
            | ConfiguredPhase::Converge
            | ConfiguredPhase::Idempotence
            | ConfiguredPhase::Cleanup
    ) {
        return Ok(Vec::new());
    }

    match value {
        serde_yaml::Value::Mapping(mapping) => {
            let (adapter, parameters) = action_mapping(mapping);
            if adapter == "ansible" {
                resolve_playbook_parameters(parameters, phase, base)
            } else {
                Ok(Vec::new())
            }
        }
        serde_yaml::Value::Sequence(actions)
            if actions
                .iter()
                .all(|action| matches!(action, serde_yaml::Value::Mapping(_))) =>
        {
            let mut playbooks = Vec::new();
            for action in actions {
                let serde_yaml::Value::Mapping(mapping) = action else {
                    unreachable!("all list items were checked as mappings");
                };
                let (adapter, parameters) = action_mapping(mapping);
                if adapter == "ansible" {
                    playbooks.extend(resolve_playbook_parameters(parameters, phase, base)?);
                }
            }
            Ok(playbooks)
        }
        _ if default_converger == "ansible" => resolve_playbook_parameters(value, phase, base),
        _ => Ok(Vec::new()),
    }
}

fn action_mapping(mapping: &serde_yaml::Mapping) -> (&str, &serde_yaml::Value) {
    let (adapter, parameters) = mapping
        .iter()
        .next()
        .expect("adapter mappings are validated before resolution");
    let serde_yaml::Value::String(adapter) = adapter else {
        unreachable!("adapter mapping keys are validated as strings");
    };
    (adapter, parameters)
}

fn resolve_playbook_parameters(
    parameters: &serde_yaml::Value,
    phase: ConfiguredPhase,
    base: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    match parameters {
        serde_yaml::Value::Null => resolve_default_playbook(phase, base).map(|name| vec![name]),
        serde_yaml::Value::String(name) => Ok(vec![resolve_playbook(name, base)?]),
        serde_yaml::Value::Sequence(playbooks) => playbooks
            .iter()
            .map(|playbook| {
                let serde_yaml::Value::String(name) = playbook else {
                    return Err("Ansible playbook lists must contain only strings".to_owned());
                };
                resolve_playbook(name, base)
            })
            .collect(),
        _ => Err(
            "Ansible parameters must be null, a playbook name, or a list of playbook names"
                .to_owned(),
        ),
    }
}

fn resolve_default_playbook(
    phase: ConfiguredPhase,
    base: Option<&Path>,
) -> Result<PathBuf, String> {
    let base = base.unwrap_or_else(|| Path::new("."));
    let phase = configured_phase_name(phase);
    let yaml_name = format!("{phase}.yaml");
    let yml_name = format!("{phase}.yml");
    let yaml_exists = base.join(&yaml_name).is_file();
    let yml_exists = base.join(&yml_name).is_file();
    match (yaml_exists, yml_exists) {
        (true, false) => resolve_playbook(&yaml_name, Some(base)),
        (false, true) => resolve_playbook(&yml_name, Some(base)),
        (true, true) => Err(format!(
            "both default playbooks `{yaml_name}` and `{yml_name}` exist"
        )),
        (false, false) => Err(format!(
            "default playbook `{yaml_name}` or `{yml_name}` was not found"
        )),
    }
}

fn resolve_playbook(name: &str, base: Option<&Path>) -> Result<PathBuf, String> {
    let path = base.unwrap_or_else(|| Path::new(".")).join(name);
    fs::canonicalize(&path)
        .ok()
        .filter(|path| path.is_file())
        .ok_or_else(|| format!("playbook `{}` was not found", path.display()))
}

fn configured_phase_name(phase: ConfiguredPhase) -> &'static str {
    match phase {
        ConfiguredPhase::Dependency => "dependency",
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
      - exec: prepare-command
    converge: site.yml
    verify:
    cleanup:
    destroy:
    tests:
      smoke: {}
    nested:
      - name: restart
        create:
          terraform: tf/restart
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
            "      - name: restart\n        create:\n          terraform: tf/restart\n        converge:\n        verify:\n        destroy:\n        nested:\n          - name: after\n            create:\n            verify:\n            destroy:\n",
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
    fn resolves_ansible_defaults_strings_and_playbook_lists_at_load_time() {
        let directory =
            std::env::temp_dir().join(format!("cvd-config-ansible-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        for playbook in ["prepare.yaml", "converge.yml", "first.yml", "second.yml"] {
            fs::write(directory.join(playbook), "---\n").unwrap();
        }
        let root = directory.join("cvd.yml");
        let yaml = r#"
version: 1
provisioner: dummy
converger: ansible
verifier: dummy
scenarios:
  default:
    prepare:
    converge: converge.yml
    idempotence: [first.yml, second.yml]
    cleanup:
      ansible: [second.yml, first.yml]
"#;
        let config = Config::from_yaml_at(yaml, &root).unwrap();
        let scenario = config.scenario("default").unwrap();
        assert_eq!(
            scenario
                .phase(ConfiguredPhase::Prepare)
                .unwrap()
                .ansible_playbooks(),
            [directory.join("prepare.yaml")]
        );
        assert_eq!(
            scenario
                .phase(ConfiguredPhase::Converge)
                .unwrap()
                .ansible_playbooks(),
            [directory.join("converge.yml")]
        );
        assert_eq!(
            scenario
                .phase(ConfiguredPhase::Idempotence)
                .unwrap()
                .ansible_playbooks(),
            [directory.join("first.yml"), directory.join("second.yml")]
        );
        assert_eq!(
            scenario
                .phase(ConfiguredPhase::Cleanup)
                .unwrap()
                .ansible_playbooks(),
            [directory.join("second.yml"), directory.join("first.yml")]
        );

        fs::write(directory.join("prepare.yml"), "---\n").unwrap();
        assert!(matches!(
            Config::from_yaml_at(yaml, &root),
            Err(ConfigError::InvalidPhase { .. })
        ));
        fs::remove_file(directory.join("prepare.yml")).unwrap();
        fs::remove_file(directory.join("converge.yml")).unwrap();
        assert!(matches!(
            Config::from_yaml_at(yaml, &root),
            Err(ConfigError::InvalidPhase { .. })
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repository_example_uses_the_supported_structure() {
        let path = fs::canonicalize("examples/dummy/cvd.yml").unwrap();
        let yaml = fs::read_to_string(&path).unwrap();
        let config = Config::from_yaml_at(&yaml, &path).unwrap();
        assert!(
            config
                .scenario("default/install/configuration-check")
                .is_some()
        );
        assert!(
            config
                .scenario("default/install/Upgrade scenario")
                .is_some()
        );
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
            "      - exec: prepare-command",
            "      - exec: prepare-command\n        dummy: second-action",
            1,
        );
        assert!(matches!(
            Config::from_yaml(&malformed_action),
            Err(ConfigError::InvalidPhase { .. })
        ));
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
