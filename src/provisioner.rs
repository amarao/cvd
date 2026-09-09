//! Provisioner implementations.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{self, Command},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    config::{DummyStatus, PhaseDefinition},
    state::{LifecyclePhase, Resource, ResourceLocation, ResourceManifest},
};

static NEXT_EXCHANGE: AtomicU64 = AtomicU64::new(0);
const PROTOCOL_VERSION: u32 = 1;
const MANIFEST_VERSION: u32 = 1;

pub trait Provisioner {
    fn create(
        &self,
        scenario_path: &str,
        definition: &PhaseDefinition,
    ) -> Result<ResourceManifest, ProvisionerError>;

    fn destroy(
        &self,
        scenario_path: &str,
        resources: &ResourceManifest,
        definition: &PhaseDefinition,
    ) -> Result<(), ProvisionerError>;
}

/// Creates one fixed mock resource and pretends to destroy it.
#[derive(Debug, Default)]
pub struct DummyProvisioner;

impl Provisioner for DummyProvisioner {
    fn create(
        &self,
        scenario_path: &str,
        definition: &PhaseDefinition,
    ) -> Result<ResourceManifest, ProvisionerError> {
        if definition.dummy_status() == DummyStatus::Error {
            return Err(ProvisionerError(format!(
                "dummy create error for `{scenario_path}`"
            )));
        }
        Ok(ResourceManifest {
            resources: vec![Resource {
                id: "mock".to_owned(),
                resource_type: "mock".to_owned(),
                exists: true,
                created: ResourceLocation {
                    scenario_path: scenario_path.to_owned(),
                    phase: LifecyclePhase::Create,
                },
                destroyed: None,
                attributes: BTreeMap::new(),
                relationships: BTreeSet::new(),
                sensitive_attributes: BTreeSet::new(),
            }],
        })
    }

    fn destroy(
        &self,
        _scenario_path: &str,
        _resources: &ResourceManifest,
        definition: &PhaseDefinition,
    ) -> Result<(), ProvisionerError> {
        match definition.dummy_status() {
            DummyStatus::Ok => Ok(()),
            DummyStatus::Error => Err(ProvisionerError("dummy destroy error".to_owned())),
            DummyStatus::Fail => unreachable!("phase status is validated"),
        }
    }
}

/// Runs Ansible create/destroy playbooks using a versioned JSON-file contract.
#[derive(Debug)]
pub struct AnsibleProvisioner {
    default_is_ansible: bool,
    working_directory: PathBuf,
    inventory: crate::inventory::AnsibleInventory,
}

impl AnsibleProvisioner {
    pub fn new(default_is_ansible: bool, working_directory: impl Into<PathBuf>) -> Self {
        let working_directory = working_directory.into();
        Self {
            default_is_ansible,
            inventory: crate::inventory::AnsibleInventory::new(&working_directory, Vec::new()),
            working_directory,
        }
    }

    pub fn with_inventory(mut self, inventory: Vec<PathBuf>) -> Self {
        self.inventory =
            crate::inventory::AnsibleInventory::new(&self.working_directory, inventory);
        self
    }

    pub(crate) fn converge(
        &self,
        scenario_path: &str,
        action: &'static str,
        definition: &crate::config::AnsiblePhaseDefinition,
        overlay: Option<&std::path::Path>,
        resources: &[Resource],
    ) -> Result<(), ProvisionerError> {
        self.inventory
            .validate_bindings(
                overlay,
                definition.playbook.parent().expect("resolved playbook"),
            )
            .map_err(|error| ProvisionerError(error.to_string()))?;
        self.run(scenario_path, action, resources, definition, false, overlay)
            .map(|_| ())
    }

    fn selected<'a>(
        &self,
        definition: &'a PhaseDefinition,
    ) -> Result<Option<&'a crate::config::AnsiblePhaseDefinition>, ProvisionerError> {
        match definition.ansible() {
            some @ Some(_) => Ok(some),
            None if self.default_is_ansible => Err(ProvisionerError(
                "the Ansible provisioner requires an explicit `ansible` phase mapping".to_owned(),
            )),
            None => Ok(None),
        }
    }

    fn run(
        &self,
        scenario_path: &str,
        action: &'static str,
        resources: &[Resource],
        definition: &crate::config::AnsiblePhaseDefinition,
        expect_result: bool,
        overlay: Option<&std::path::Path>,
    ) -> Result<Option<CreateResult>, ProvisionerError> {
        let exchange = Exchange::create()?;
        let (destroy_inventory, other_resources) = destroy_targets(scenario_path, resources);
        let resources = if action == "destroy" {
            other_resources.as_slice()
        } else {
            resources
        };
        let input = AnsibleInput {
            cvd: CvdInput {
                protocol_version: PROTOCOL_VERSION,
                invocation_id: &exchange.invocation_id,
                input_file: &exchange.input,
                result_file: &exchange.result,
                directory: &self.working_directory,
                action,
                scenario_selector: scenario_path,
                vars: &definition.vars,
                resources,
                resources_by_type: resources_by_type(resources),
            },
        };
        fs::write(
            &exchange.input,
            serde_json::to_vec_pretty(&input).map_err(|error| {
                ProvisionerError(format!("cannot encode Ansible input: {error}"))
            })?,
        )
        .map_err(|error| ProvisionerError(format!("cannot write Ansible input: {error}")))?;

        let mut command = Command::new("ansible-playbook");
        if action == "destroy" {
            let path = exchange.directory.join("destroy-inventory.yml");
            let data = serde_yaml::to_string(&destroy_inventory)
                .map_err(|error| ProvisionerError(error.to_string()))?;
            fs::write(&path, data).map_err(|error| ProvisionerError(error.to_string()))?;
            command.arg("--inventory").arg(path);
        } else {
            self.inventory
                .apply(&mut command, overlay)
                .map_err(|error| ProvisionerError(error.to_string()))?;
        }
        let status = command
            .arg(&definition.playbook)
            .arg("--extra-vars")
            .arg(format!("@{}", exchange.input.display()))
            .current_dir(&self.working_directory)
            .env("CVD_DIRECTORY", &self.working_directory)
            .env("CVD_INPUT_FILE", &exchange.input)
            .env("CVD_RESULT_FILE", &exchange.result)
            .env("CVD_INVOCATION_ID", &exchange.invocation_id)
            .status()
            .map_err(|error| {
                ProvisionerError(format!(
                    "cannot run Ansible {action} playbook `{}`: {error}",
                    definition.playbook.display()
                ))
            })?;
        if !status.success() {
            return Err(ProvisionerError(format!(
                "Ansible {action} playbook `{}` exited with {status}",
                definition.playbook.display()
            )));
        }
        if !expect_result {
            return Ok(None);
        }
        let result: CreateResult =
            serde_json::from_slice(&fs::read(&exchange.result).map_err(|error| {
                ProvisionerError(format!(
                    "Ansible create did not publish `{}`: {error}",
                    exchange.result.display()
                ))
            })?)
            .map_err(|error| ProvisionerError(format!("invalid Ansible create result: {error}")))?;
        if result.manifest_version != MANIFEST_VERSION {
            return Err(ProvisionerError(format!(
                "unsupported Ansible manifest version {}",
                result.manifest_version
            )));
        }
        if result.invocation_id != exchange.invocation_id {
            return Err(ProvisionerError(
                "Ansible result invocation ID does not match this execution".to_owned(),
            ));
        }
        if !result.complete {
            return Err(ProvisionerError(
                "Ansible create result is not marked complete".to_owned(),
            ));
        }
        Ok(Some(result))
    }
}

impl Provisioner for AnsibleProvisioner {
    fn create(
        &self,
        scenario_path: &str,
        definition: &PhaseDefinition,
    ) -> Result<ResourceManifest, ProvisionerError> {
        let Some(ansible) = self.selected(definition)? else {
            return DummyProvisioner.create(scenario_path, definition);
        };
        let result = self
            .run(scenario_path, "create", &[], ansible, true, None)?
            .expect("create requests a result");
        let mut ids = BTreeSet::new();
        let resources = result
            .resources
            .into_iter()
            .map(|resource| {
                if resource.id.is_empty() {
                    return Err(ProvisionerError("resource ID must not be empty".to_owned()));
                }
                if resource.resource_type.is_empty() {
                    return Err(ProvisionerError(format!(
                        "resource `{}` type must not be empty",
                        resource.id
                    )));
                }
                if !ids.insert(resource.id.clone()) {
                    return Err(ProvisionerError(format!(
                        "duplicate resource ID `{}`",
                        resource.id
                    )));
                }
                Ok(Resource {
                    id: resource.id,
                    resource_type: resource.resource_type,
                    exists: true,
                    created: ResourceLocation {
                        scenario_path: scenario_path.to_owned(),
                        phase: LifecyclePhase::Create,
                    },
                    destroyed: None,
                    attributes: resource.attributes,
                    relationships: resource.relationships,
                    sensitive_attributes: resource.sensitive_attributes,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ResourceManifest { resources })
    }

    fn destroy(
        &self,
        scenario_path: &str,
        resources: &ResourceManifest,
        definition: &PhaseDefinition,
    ) -> Result<(), ProvisionerError> {
        let Some(ansible) = self.selected(definition)? else {
            return DummyProvisioner.destroy(scenario_path, resources, definition);
        };
        self.run(
            scenario_path,
            "destroy",
            &resources.resources,
            ansible,
            false,
            None,
        )?;
        Ok(())
    }
}

#[derive(Serialize)]
struct AnsibleInput<'a> {
    cvd: CvdInput<'a>,
}

#[derive(Serialize)]
struct CvdInput<'a> {
    protocol_version: u32,
    invocation_id: &'a str,
    input_file: &'a std::path::Path,
    result_file: &'a std::path::Path,
    directory: &'a std::path::Path,
    action: &'static str,
    scenario_selector: &'a str,
    vars: &'a serde_json::Value,
    resources: &'a [Resource],
    resources_by_type: BTreeMap<&'a str, Vec<&'a Resource>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateResult {
    manifest_version: u32,
    invocation_id: String,
    complete: bool,
    resources: Vec<ReportedResource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportedResource {
    id: String,
    #[serde(rename = "type")]
    resource_type: String,
    #[serde(default)]
    attributes: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    relationships: BTreeSet<String>,
    #[serde(default)]
    sensitive_attributes: BTreeSet<String>,
}

struct Exchange {
    directory: PathBuf,
    input: PathBuf,
    result: PathBuf,
    invocation_id: String,
}

impl Exchange {
    fn create() -> Result<Self, ProvisionerError> {
        let sequence = NEXT_EXCHANGE.fetch_add(1, Ordering::Relaxed);
        let invocation_id = format!("{}-{sequence}", process::id());
        let directory = std::env::temp_dir().join(format!("cvd-ansible-{invocation_id}"));
        fs::create_dir(&directory).map_err(|error| {
            ProvisionerError(format!(
                "cannot create Ansible exchange directory `{}`: {error}",
                directory.display()
            ))
        })?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(|error| {
            ProvisionerError(format!("cannot secure Ansible exchange directory: {error}"))
        })?;
        Ok(Self {
            input: directory.join("input.json"),
            result: directory.join("result.json"),
            directory,
            invocation_id,
        })
    }
}

impl Drop for Exchange {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct ProvisionerError(pub String);

/// Destruction targets come exclusively from persisted ownership, never the
/// original inventory (which can contain uncreated or inherited hosts).
fn destroy_targets(scenario: &str, resources: &[Resource]) -> (serde_json::Value, Vec<Resource>) {
    let mut hosts = serde_json::Map::new();
    let mut others = Vec::new();
    for resource in resources
        .iter()
        .filter(|r| r.exists && r.created.scenario_path == scenario)
    {
        let name = resource
            .attributes
            .get("ansible")
            .and_then(|binding| binding.get("inventory_hostname"))
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.trim().is_empty());
        if let Some(name) = name {
            let mut alias = name.to_owned();
            let mut suffix = 2;
            while hosts.contains_key(&alias) {
                alias = format!("{name}__cvd_{suffix}");
                suffix += 1;
            }
            hosts.insert(
                alias,
                serde_json::json!({"ansible_connection": "local", "cvd_resource": resource}),
            );
        } else {
            others.push(resource.clone());
        }
    }
    (
        serde_json::json!({"all": {"children": {"cvd_managed": {"hosts": hosts}}}}),
        others,
    )
}

fn resources_by_type(resources: &[Resource]) -> BTreeMap<&str, Vec<&Resource>> {
    let mut grouped = BTreeMap::new();
    for resource in resources {
        grouped
            .entry(resource.resource_type.as_str())
            .or_insert_with(Vec::new)
            .push(resource);
    }
    grouped
}

#[cfg(test)]
mod destroy_tests {
    use super::*;

    #[test]
    fn create_manifest_requires_manifest_version() {
        let mut manifest = serde_json::json!({
            "manifest_version": 1,
            "invocation_id": "test-call",
            "complete": true,
            "resources": []
        });
        let result: CreateResult = serde_json::from_value(manifest.clone()).unwrap();
        assert_eq!(result.manifest_version, MANIFEST_VERSION);

        let fields = manifest.as_object_mut().unwrap();
        fields.remove("manifest_version");
        fields.insert("protocol_version".to_owned(), serde_json::json!(1));
        assert!(serde_json::from_value::<CreateResult>(manifest).is_err());
    }
    use serde_json::json;

    fn resource(id: &str, name: Option<&str>) -> Resource {
        Resource {
            id: id.into(),
            resource_type: "test".into(),
            exists: true,
            created: ResourceLocation {
                scenario_path: "root/child".into(),
                phase: LifecyclePhase::Create,
            },
            destroyed: None,
            attributes: name
                .map(|name| {
                    BTreeMap::from([(
                        "ansible".into(),
                        json!({"inventory_hostname": name, "vars": {"ansible_connection": "ssh"}}),
                    )])
                })
                .unwrap_or_default(),
            relationships: BTreeSet::new(),
            sensitive_attributes: BTreeSet::new(),
        }
    }

    #[test]
    fn converger_visibility_and_grouping_preserve_inheritance_and_order() {
        let mut state = crate::state::RunState::new("run", "cvd.yml".into(), "x", None, false);
        state.enter_scenario("root", None);
        state.enter_scenario("root/child", Some("root".into()));
        state.enter_scenario("root/sibling", Some("root".into()));
        let mut host = resource("parent-host", Some("web"));
        host.created.scenario_path = "root".into();
        host.resource_type = "container".into();
        let mut image = resource("parent-image", None);
        image.created.scenario_path = "root".into();
        image.resource_type = "docker.image".into();
        let mut child_image = image.clone();
        child_image.id = "child-image".into();
        child_image.created.scenario_path = "root/child".into();
        let mut deleted = resource("deleted", None);
        deleted.exists = false;
        state.scenarios.get_mut("root").unwrap().resources.resources =
            vec![host.clone(), image.clone()];
        state
            .scenarios
            .get_mut("root/child")
            .unwrap()
            .resources
            .resources = vec![child_image.clone(), deleted];
        state
            .scenarios
            .get_mut("root/sibling")
            .unwrap()
            .resources
            .resources = vec![resource("sibling", None)];
        let visible = state.visible_resources("root/child");
        assert_eq!(visible, [host, image, child_image]);
        let grouped = resources_by_type(&visible);
        assert_eq!(grouped["docker.image"], [&visible[1], &visible[2]]);
        assert_eq!(grouped["container"], [&visible[0]]);
        assert!(!grouped.contains_key("missing"));
        assert!(resources_by_type(&[]).is_empty());
        assert_eq!(state.visible_resources("root").len(), 2);
    }

    #[test]
    fn destroy_separates_owned_hosts_from_other_resources_in_order() {
        let mut parent = resource("parent", Some("parent"));
        parent.created.scenario_path = "root".into();
        let mut deleted = resource("deleted", Some("deleted"));
        deleted.exists = false;
        let resources = vec![
            parent,
            resource("id-z", Some("z")),
            resource("network", None),
            resource("id-a", Some("a")),
            deleted,
        ];
        let (inventory, others) = destroy_targets("root/child", &resources);
        let hosts = inventory["all"]["children"]["cvd_managed"]["hosts"]
            .as_object()
            .unwrap();
        assert_eq!(
            hosts.keys().map(String::as_str).collect::<Vec<_>>(),
            ["z", "a"]
        );
        assert_eq!(hosts["z"]["cvd_resource"], json!(resources[1]));
        assert_eq!(hosts["z"]["ansible_connection"], "local");
        assert_eq!(others, vec![resources[2].clone()]);
        let (empty, others) = destroy_targets("unrelated", &resources);
        assert!(
            empty["all"]["children"]["cvd_managed"]["hosts"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        assert!(others.is_empty());
    }

    #[test]
    fn invalid_host_bindings_do_not_lose_resources_during_cleanup() {
        let mut malformed = resource("malformed", None);
        malformed.attributes.insert("ansible".into(), json!(null));
        let resources = vec![
            resource("one", Some("web")),
            resource("two", Some("web")),
            resource("three", Some("web__cvd_2")),
            malformed,
        ];
        let (inventory, others) = destroy_targets("root/child", &resources);
        let hosts = inventory["all"]["children"]["cvd_managed"]["hosts"]
            .as_object()
            .unwrap();
        assert_eq!(hosts.len(), 3);
        assert_eq!(
            hosts
                .values()
                .map(|host| host["cvd_resource"]["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["one", "two", "three"]
        );
        assert_eq!(others[0].id, "malformed");
    }
}
