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
}

impl AnsibleProvisioner {
    pub fn new(default_is_ansible: bool, working_directory: impl Into<PathBuf>) -> Self {
        Self {
            default_is_ansible,
            working_directory: working_directory.into(),
        }
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
        action: &'static str,
        scenario_path: &str,
        resources: &[Resource],
        definition: &crate::config::AnsiblePhaseDefinition,
        expect_result: bool,
    ) -> Result<Option<CreateResult>, ProvisionerError> {
        let exchange = Exchange::create()?;
        let input = AnsibleInput {
            cvd: CvdInput {
                protocol_version: PROTOCOL_VERSION,
                invocation_id: &exchange.invocation_id,
                action,
                scenario_path,
                vars: &definition.vars,
                resources,
            },
        };
        fs::write(
            &exchange.input,
            serde_json::to_vec_pretty(&input).map_err(|error| {
                ProvisionerError(format!("cannot encode Ansible input: {error}"))
            })?,
        )
        .map_err(|error| ProvisionerError(format!("cannot write Ansible input: {error}")))?;

        let status = Command::new("ansible-playbook")
            .arg(&definition.playbook)
            .arg("--extra-vars")
            .arg(format!("@{}", exchange.input.display()))
            .current_dir(&self.working_directory)
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
        if result.protocol_version != PROTOCOL_VERSION {
            return Err(ProvisionerError(format!(
                "unsupported Ansible result protocol version {}",
                result.protocol_version
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
            .run("create", scenario_path, &[], ansible, true)?
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
            "destroy",
            scenario_path,
            &resources.resources,
            ansible,
            false,
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
    action: &'static str,
    scenario_path: &'a str,
    vars: &'a serde_json::Value,
    resources: &'a [Resource],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateResult {
    protocol_version: u32,
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
