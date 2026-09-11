//! Ansible runtime inventory views. Ansible interprets the original sources;
//! CVD only projects explicit host bindings from visible, existing resources.
use crate::state::{LifecyclePhase, Resource, ResourceLocation, RunState};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub(crate) fn hosts(state: &RunState, path: &str) -> Result<Map<String, Value>, String> {
    let mut hosts = Map::new();
    // Walk ancestors explicitly; lexical scenario order must not reorder hosts.
    let mut ancestor = String::new();
    for component in path.split('/') {
        if !ancestor.is_empty() {
            ancestor.push('/');
        }
        ancestor.push_str(component);
        let Some(scenario) = state.scenarios.get(&ancestor) else {
            continue;
        };
        for resource in &scenario.resources.resources {
            if !resource.exists {
                continue;
            }
            let Some(binding) = resource.attributes.get("ansible") else {
                continue;
            };
            let binding = binding
                .as_object()
                .ok_or("resource attributes.ansible must be a mapping")?;
            if binding
                .keys()
                .any(|key| key != "inventory_hostname" && key != "vars")
            {
                return Err("unknown field in resource attributes.ansible".to_owned());
            }
            let name = binding
                .get("inventory_hostname")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .ok_or(
                    "resource attributes.ansible.inventory_hostname must be a nonempty string",
                )?;
            let vars = binding.get("vars").cloned().unwrap_or_else(|| json!({}));
            if !vars.is_object() {
                return Err(format!("host `{name}` vars must be a mapping"));
            }
            if hosts.insert(name.to_owned(), vars).is_some() {
                return Err(format!(
                    "multiple visible resources bind inventory host `{name}`"
                ));
            }
        }
    }
    Ok(hosts)
}

pub(crate) fn write_view(
    state: &mut RunState,
    scenario: &str,
    phase: LifecyclePhase,
    state_path: &Path,
) -> Result<Option<PathBuf>, String> {
    let hosts = hosts(state, scenario)?;
    if hosts.is_empty() {
        return Ok(None);
    }
    // Encode selector bytes, rather than use user-provided path components.
    let key: String = scenario.bytes().map(|byte| format!("{byte:02x}")).collect();
    let directory = state_path
        .parent()
        .ok_or("state file has no parent")?
        .join("views")
        .join(key);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let path = directory.join("ansible-inventory.yml");
    let temporary = directory.join("ansible-inventory.tmp");
    let data = serde_yaml::to_string(&json!({"all": {"hosts": hosts}}))
        .map_err(|error| error.to_string())?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(data.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| error.to_string())?;
    fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
    let path = fs::canonicalize(path).map_err(|error| error.to_string())?;
    let views = &mut state
        .scenarios
        .get_mut(scenario)
        .ok_or("scenario is not entered")?
        .views;
    let view = views
        .entry("ansible_inventory".to_owned())
        .or_insert_with(|| Resource {
            id: "ansible_inventory".to_owned(),
            resource_type: "view.ansible.inventory".to_owned(),
            exists: true,
            created: ResourceLocation {
                scenario_path: scenario.to_owned(),
                phase,
            },
            destroyed: None,
            attributes: BTreeMap::from([("path".to_owned(), json!(path))]),
            relationships: BTreeSet::new(),
            sensitive_attributes: BTreeSet::new(),
        });
    view.attributes.insert("path".to_owned(), json!(path));
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(name: &str) -> Resource {
        Resource {
            id: name.into(),
            resource_type: "container".into(),
            exists: true,
            created: ResourceLocation {
                scenario_path: "root".into(),
                phase: LifecyclePhase::Create,
            },
            destroyed: None,
            attributes: BTreeMap::from([(
                "ansible".into(),
                json!({"inventory_hostname": name, "vars": {"ansible_host": format!("actual-{name}")}}),
            )]),
            relationships: BTreeSet::new(),
            sensitive_attributes: BTreeSet::new(),
        }
    }

    fn state() -> RunState {
        let mut state = RunState::new("inventory", "cvd.yml".into(), "x", None, false);
        state.enter_scenario("root", None);
        state.enter_scenario("root/child", Some("root".into()));
        state.enter_scenario("root/sibling", Some("root".into()));
        state.scenarios.get_mut("root").unwrap().resources.resources =
            vec![resource("z"), resource("a")];
        state
            .scenarios
            .get_mut("root/child")
            .unwrap()
            .resources
            .resources
            .push(resource("child"));
        state
            .scenarios
            .get_mut("root/sibling")
            .unwrap()
            .resources
            .resources
            .push(resource("sibling"));
        state
    }

    #[test]
    fn projects_only_live_ancestor_and_owned_bindings_in_resource_order() {
        let mut state = state();
        let mut nonhost = resource("network");
        nonhost.attributes.clear();
        let mut deleted = resource("deleted");
        deleted.exists = false;
        state
            .scenarios
            .get_mut("root")
            .unwrap()
            .resources
            .resources
            .extend([nonhost, deleted]);
        let result = hosts(&state, "root/child").unwrap();
        assert_eq!(
            result.keys().map(String::as_str).collect::<Vec<_>>(),
            ["z", "a", "child"]
        );
        assert_eq!(result["z"]["ansible_host"], "actual-z");
        assert_eq!(hosts(&state, "root").unwrap().len(), 2);
    }

    #[test]
    fn rejects_ambiguous_and_malformed_bindings() {
        let mut state = state();
        state
            .scenarios
            .get_mut("root/child")
            .unwrap()
            .resources
            .resources
            .push(resource("z"));
        assert!(
            hosts(&state, "root/child")
                .unwrap_err()
                .contains("multiple visible resources")
        );
        for binding in [
            json!(null),
            json!({}),
            json!({"inventory_hostname": " "}),
            json!({"inventory_hostname": "z", "vars": []}),
            json!({"inventory_hostname": "z", "groups": []}),
        ] {
            state.scenarios.get_mut("root").unwrap().resources.resources[0]
                .attributes
                .insert("ansible".into(), binding);
            assert!(hosts(&state, "root").is_err());
        }
    }

    #[test]
    fn view_is_private_persisted_and_keeps_yaml_host_order() {
        let directory = std::env::temp_dir().join(format!("cvd-view-test-{}", std::process::id()));
        let mut state = state();
        let path = write_view(
            &mut state,
            "root/child",
            LifecyclePhase::Converge,
            &directory.join("state.json"),
        )
        .unwrap()
        .unwrap();
        let yaml = fs::read_to_string(&path).unwrap();
        assert!(yaml.find("  z:").unwrap() < yaml.find("  a:").unwrap());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let store = crate::state::StateStore::new(directory.join("state.json"));
        store.save(&state).unwrap();
        let loaded = store.load().unwrap();
        let view = &loaded.scenarios["root/child"].views["ansible_inventory"];
        assert_eq!(view.attributes["path"], json!(path));
        assert!(view.exists);
        assert_eq!(loaded.scenarios["root/child"].resources.resources.len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct InventoryError(pub String);

/// Append source paths to Ansible's comma-separated inventory environment.
pub(crate) fn inventory_environment(
    mut value: OsString,
    sources: &[PathBuf],
) -> Result<OsString, InventoryError> {
    use std::os::unix::ffi::OsStrExt;

    for source in sources {
        if source.as_os_str().as_bytes().contains(&b',') {
            return Err(InventoryError("inventory source paths containing commas cannot be passed through ANSIBLE_INVENTORY".to_owned()));
        }
        if !value.is_empty() && !value.as_bytes().ends_with(b",") {
            value.push(",");
        }
        value.push(source);
    }
    Ok(value)
}

/// Shared source resolution for Ansible and pytest/testinfra invocations.
#[derive(Debug)]
pub(crate) struct AnsibleInventory {
    working_directory: PathBuf,
    sources: Vec<PathBuf>,
}

impl AnsibleInventory {
    pub(crate) fn new(working_directory: impl Into<PathBuf>, sources: Vec<PathBuf>) -> Self {
        Self {
            working_directory: working_directory.into(),
            sources,
        }
    }

    fn resolved_sources(&self) -> Result<Vec<PathBuf>, InventoryError> {
        let mut sources = self.sources.clone();
        if sources.is_empty() {
            let result = std::process::Command::new("ansible-config")
                .args(["dump", "--format", "json"])
                .current_dir(&self.working_directory)
                .output()
                .map_err(|error| {
                    InventoryError(format!(
                        "cannot read Ansible inventory configuration: {error}"
                    ))
                })?;
            if !result.status.success() {
                return Err(InventoryError(
                    "ansible-config failed while resolving inventory sources".to_owned(),
                ));
            }
            let settings: Vec<serde_json::Value> =
                serde_json::from_slice(&result.stdout).map_err(|error| {
                    InventoryError(format!("invalid ansible-config output: {error}"))
                })?;
            let values = settings
                .iter()
                .find(|setting| setting["name"] == "DEFAULT_HOST_LIST")
                .and_then(|setting| setting["value"].as_array())
                .ok_or_else(|| {
                    InventoryError("ansible-config omitted DEFAULT_HOST_LIST".to_owned())
                })?;
            for value in values {
                sources.push(PathBuf::from(value.as_str().ok_or_else(|| {
                    InventoryError("invalid DEFAULT_HOST_LIST".to_owned())
                })?));
            }
        }
        Ok(sources)
    }

    pub(crate) fn apply(
        &self,
        command: &mut std::process::Command,
        overlay: Option<&Path>,
    ) -> Result<(), InventoryError> {
        // With no additions, let Ansible use its normal default selection.
        if !self.sources.is_empty() || overlay.is_some() {
            command.env("ANSIBLE_INVENTORY", self.environment(overlay)?);
        }
        Ok(())
    }

    pub(crate) fn environment(&self, overlay: Option<&Path>) -> Result<OsString, InventoryError> {
        let inherited = std::env::var_os("ANSIBLE_INVENTORY").unwrap_or_default();
        let mut sources = if inherited.is_empty() {
            self.resolved_sources()?
        } else {
            self.sources.clone()
        };
        if let Some(overlay) = overlay {
            sources.push(overlay.to_owned());
        }
        inventory_environment(inherited, &sources)
    }

    pub(crate) fn validate_bindings(
        &self,
        overlay: Option<&Path>,
        playbook_directory: &Path,
    ) -> Result<(), InventoryError> {
        if let Some(overlay) = overlay {
            // Resolve names with Ansible itself, including inventory plugins.
            // A typo in a reported binding must not introduce a new host.
            let mut command = std::process::Command::new("ansible-inventory");
            self.apply(&mut command, None)?;
            let result = command
                .arg("--list")
                .arg("--playbook-dir")
                .arg(playbook_directory)
                .current_dir(&self.working_directory)
                .output()
                .map_err(|error| {
                    InventoryError(format!("cannot inspect Ansible inventory: {error}"))
                })?;
            if !result.status.success() {
                return Err(InventoryError(
                    "ansible-inventory failed while validating host bindings".to_owned(),
                ));
            }
            let inventory: serde_json::Value =
                serde_json::from_slice(&result.stdout).map_err(|error| {
                    InventoryError(format!("invalid ansible-inventory output: {error}"))
                })?;
            let mut names = BTreeSet::new();
            if let Some(groups) = inventory.as_object() {
                for (name, group) in groups {
                    if name == "_meta" {
                        continue;
                    }
                    if let Some(hosts) = group.get("hosts").and_then(serde_json::Value::as_array) {
                        names.extend(hosts.iter().filter_map(serde_json::Value::as_str));
                    }
                }
            }
            let overlay: serde_json::Value =
                serde_yaml::from_slice(&fs::read(overlay).map_err(|error| {
                    InventoryError(format!("cannot read inventory view: {error}"))
                })?)
                .map_err(|error| InventoryError(format!("invalid inventory view: {error}")))?;
            for host in overlay["all"]["hosts"]
                .as_object()
                .expect("CVD inventory view")
                .keys()
            {
                if !names.contains(host.as_str()) {
                    return Err(InventoryError(format!(
                        "resource binds unknown inventory host `{host}`"
                    )));
                }
            }
        }
        Ok(())
    }
}
