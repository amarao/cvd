use std::fmt;

use indexmap::IndexMap;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use thiserror::Error;

/// The only configuration schema understood by this stub.
pub const CONFIG_VERSION: u32 = 1;

/// A parsed and validated CVD configuration file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    version: u32,
    pub scenarios: ScenarioMap,
}

/// A recursively nestable scenario.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub provisioner: String,
    #[serde(default)]
    pub suites: SuiteMap,
    #[serde(default)]
    pub scenarios: ScenarioMap,
}

/// A named verifier invocation within a scenario.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub verifier: String,
}

/// Declaration-ordered named child scenarios.
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

/// Declaration-ordered named verifier invocations.
#[derive(Debug, Default)]
pub struct SuiteMap(IndexMap<String, Suite>);

impl SuiteMap {
    pub fn iter(&self) -> indexmap::map::Iter<'_, String, Suite> {
        self.0.iter()
    }
}

impl<'de> Deserialize<'de> for ScenarioMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_named_map(deserializer, "scenario").map(ScenarioMap::from)
    }
}

impl<'de> Deserialize<'de> for SuiteMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_named_map(deserializer, "suite").map(SuiteMap::from)
    }
}

trait NamedMapValue: Sized {}
impl NamedMapValue for Scenario {}
impl NamedMapValue for Suite {}

fn deserialize_named_map<'de, D, T>(
    deserializer: D,
    item: &'static str,
) -> Result<NamedMap<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + NamedMapValue,
{
    struct NamedMapVisitor<T> {
        item: &'static str,
        marker: std::marker::PhantomData<T>,
    }

    impl<'de, T> Visitor<'de> for NamedMapVisitor<T>
    where
        T: Deserialize<'de> + NamedMapValue,
    {
        type Value = NamedMap<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "a mapping of named {}s", self.item)
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut entries = IndexMap::new();
            while let Some((name, value)) = map.next_entry::<String, T>()? {
                if entries.contains_key(&name) {
                    return Err(de::Error::custom(format!(
                        "duplicate {} name `{name}`",
                        self.item
                    )));
                }
                entries.insert(name, value);
            }
            Ok(NamedMap(entries))
        }
    }

    deserializer.deserialize_map(NamedMapVisitor {
        item,
        marker: std::marker::PhantomData,
    })
}

/// Internal helper used to share strict deserialization for named maps.
struct NamedMap<T>(IndexMap<String, T>);

impl From<NamedMap<Scenario>> for ScenarioMap {
    fn from(value: NamedMap<Scenario>) -> Self {
        Self(value.0)
    }
}

impl From<NamedMap<Suite>> for SuiteMap {
    fn from(value: NamedMap<Suite>) -> Self {
        Self(value.0)
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid configuration: {0}")]
    Parse(#[from] serde_yaml::Error),
    #[error("unsupported configuration version {found}; supported version is {CONFIG_VERSION}")]
    UnsupportedVersion { found: u32 },
    #[error("invalid scenario `{path}`: {reason}")]
    InvalidScenario { path: String, reason: String },
    #[error("invalid suite `{path}`: {reason}")]
    InvalidSuite { path: String, reason: String },
}

impl Config {
    pub fn from_yaml(input: &str) -> Result<Self, ConfigError> {
        let config: Self = serde_yaml::from_str(input)?;
        config.validate()?;
        Ok(config)
    }

    #[cfg(test)]
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Looks up a stable slash-separated scenario path.
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

    /// Returns every scenario path in declaration order, including nested scenarios.
    #[cfg(test)]
    pub fn scenario_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        collect_paths(&self.scenarios, None, &mut paths);
        paths
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                found: self.version,
            });
        }
        validate_scenarios(&self.scenarios, None)
    }
}

#[cfg(test)]
fn collect_paths(scenarios: &ScenarioMap, parent: Option<&str>, paths: &mut Vec<String>) {
    for (name, scenario) in scenarios.iter() {
        let path = match parent {
            Some(parent) => format!("{parent}/{name}"),
            None => name.clone(),
        };
        paths.push(path.clone());
        collect_paths(&scenario.scenarios, Some(&path), paths);
    }
}

fn validate_scenarios(scenarios: &ScenarioMap, parent: Option<&str>) -> Result<(), ConfigError> {
    for (name, scenario) in scenarios.iter() {
        let path = match parent {
            Some(parent) => format!("{parent}/{name}"),
            None => name.clone(),
        };
        validate_name(name).map_err(|reason| ConfigError::InvalidScenario {
            path: path.clone(),
            reason,
        })?;

        if scenario.provisioner != "dummy" {
            return Err(ConfigError::InvalidScenario {
                path: path.clone(),
                reason: format!(
                    "unsupported provisioner `{}`; only `dummy` is supported",
                    scenario.provisioner
                ),
            });
        }

        for (suite_name, suite) in scenario.suites.iter() {
            let suite_path = format!("{path}::{suite_name}");
            validate_name(suite_name).map_err(|reason| ConfigError::InvalidSuite {
                path: suite_path.clone(),
                reason,
            })?;
            if suite.verifier != "dummy" {
                return Err(ConfigError::InvalidSuite {
                    path: suite_path,
                    reason: format!(
                        "unsupported verifier `{}`; only `dummy` is supported",
                        suite.verifier
                    ),
                });
            }
        }

        validate_scenarios(&scenario.scenarios, Some(&path))?;
    }
    Ok(())
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
    use super::{CONFIG_VERSION, Config, ConfigError};

    const NESTED: &str = r#"
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
        scenarios:
          after:
            provisioner: dummy
  independent:
    provisioner: dummy
"#;

    #[test]
    fn parses_nested_scenarios_in_declaration_order() {
        let config = Config::from_yaml(NESTED).unwrap();

        assert_eq!(config.version(), CONFIG_VERSION);
        assert_eq!(
            config.scenario_paths(),
            [
                "default",
                "default/restart",
                "default/restart/after",
                "independent"
            ]
        );
        assert!(config.scenario("default/restart").is_some());
        assert!(config.scenario("default/restart/after").is_some());
        assert!(config.scenario("default/missing").is_none());
        assert!(config.scenario("default//restart").is_none());
    }

    #[test]
    fn rejects_unknown_fields_and_dummy_options() {
        let unknown = r#"
version: 1
scenarios:
  default:
    provisioner: dummy
    unexpected: true
"#;
        assert!(matches!(
            Config::from_yaml(unknown),
            Err(ConfigError::Parse(_))
        ));

        let dummy_options = r#"
version: 1
scenarios:
  default:
    provisioner:
      name: dummy
"#;
        assert!(matches!(
            Config::from_yaml(dummy_options),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn rejects_unsupported_implementations_and_invalid_names() {
        let provisioner = NESTED.replace("provisioner: dummy", "provisioner: terraform");
        assert!(matches!(
            Config::from_yaml(&provisioner),
            Err(ConfigError::InvalidScenario { .. })
        ));

        let verifier = NESTED.replace("verifier: dummy", "verifier: pytest");
        assert!(matches!(
            Config::from_yaml(&verifier),
            Err(ConfigError::InvalidSuite { .. })
        ));

        let bad_name = NESTED.replace("  default:", "  bad/path:");
        assert!(matches!(
            Config::from_yaml(&bad_name),
            Err(ConfigError::InvalidScenario { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_sibling_scenario_and_suite_names() {
        let duplicate_scenario = r#"
version: 1
scenarios:
  default:
    provisioner: dummy
  default:
    provisioner: dummy
"#;
        assert!(matches!(
            Config::from_yaml(duplicate_scenario),
            Err(ConfigError::Parse(_))
        ));

        let duplicate_suite = r#"
version: 1
scenarios:
  default:
    provisioner: dummy
    suites:
      smoke:
        verifier: dummy
      smoke:
        verifier: dummy
"#;
        assert!(matches!(
            Config::from_yaml(duplicate_suite),
            Err(ConfigError::Parse(_))
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
