//! Provisioner interfaces and implementations.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::Write,
    path::PathBuf,
    process,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use indexmap::IndexMap;
use thiserror::Error;

use crate::{
    config::{AnsibleCreateDefinition, PhaseDefinition},
    state::{LifecyclePhase, Resource, ResourceLocation, ResourceManifest},
};

static NEXT_EXCHANGE_DIRECTORY: AtomicU64 = AtomicU64::new(0);
const RESOURCE_FACTS_CALLBACK: &str = include_str!("cvd_resource_facts.py");

/// Creates and destroys resources owned by one scenario.
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
        playbooks: &[PathBuf],
        inventory: Option<&serde_json::Value>,
        output: &mut dyn Write,
        styled_output: bool,
    ) -> Result<(), ProvisionerError>;
}

/// Models one loopback resource and makes no external calls.
#[derive(Debug, Default)]
pub struct DummyProvisioner;

impl Provisioner for DummyProvisioner {
    fn create(
        &self,
        scenario_path: &str,
        _definition: &PhaseDefinition,
    ) -> Result<ResourceManifest, ProvisionerError> {
        Ok(ResourceManifest {
            resources: vec![Resource {
                id: "dummy".to_owned(),
                resource_type: "dummy".to_owned(),
                exists: true,
                created: ResourceLocation {
                    scenario_path: scenario_path.to_owned(),
                    phase: LifecyclePhase::Create,
                },
                destroyed: None,
                attributes: BTreeMap::from([(
                    "ipv6".to_owned(),
                    serde_json::Value::String("::1".to_owned()),
                )]),
                relationships: BTreeSet::new(),
                sensitive_attributes: BTreeSet::new(),
            }],
        })
    }

    fn destroy(
        &self,
        _scenario_path: &str,
        _resources: &ResourceManifest,
        _playbooks: &[PathBuf],
        _inventory: Option<&serde_json::Value>,
        _output: &mut dyn Write,
        _styled_output: bool,
    ) -> Result<(), ProvisionerError> {
        Ok(())
    }
}

/// Runs Ansible create definitions and falls back to the dummy provisioner for
/// other structural configurations. Destroy remains a no-op in this iteration.
#[derive(Debug)]
pub struct AnsibleProvisioner {
    working_directory: PathBuf,
}

impl AnsibleProvisioner {
    pub fn new(working_directory: impl Into<PathBuf>) -> Self {
        Self {
            working_directory: working_directory.into(),
        }
    }

    fn create_with_ansible(
        &self,
        scenario_path: &str,
        definition: &AnsibleCreateDefinition,
    ) -> Result<ResourceManifest, ProvisionerError> {
        let exchange = ExchangeDirectory::create()?;
        let result = self.run_create(scenario_path, definition, &exchange);
        let cleanup = fs::remove_dir_all(&exchange.root);
        match (result, cleanup) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(ProvisionerError(format!(
                "cannot remove Ansible exchange directory `{}`: {error}",
                exchange.root.display()
            ))),
            (Ok(manifest), Ok(())) => Ok(manifest),
        }
    }

    fn run_create(
        &self,
        scenario_path: &str,
        definition: &AnsibleCreateDefinition,
        exchange: &ExchangeDirectory,
    ) -> Result<ResourceManifest, ProvisionerError> {
        fs::write(
            &exchange.inventory,
            serde_json::to_vec_pretty(&definition.inventory)
                .expect("validated inventory is JSON serializable"),
        )
        .map_err(|error| {
            ProvisionerError(format!(
                "cannot write generated inventory `{}`: {error}",
                exchange.inventory.display()
            ))
        })?;
        fs::create_dir(&exchange.callback_directory).map_err(|error| {
            ProvisionerError(format!(
                "cannot create callback directory `{}`: {error}",
                exchange.callback_directory.display()
            ))
        })?;
        fs::write(&exchange.callback, RESOURCE_FACTS_CALLBACK).map_err(|error| {
            ProvisionerError(format!(
                "cannot write resource-facts callback `{}`: {error}",
                exchange.callback.display()
            ))
        })?;

        let mut callback_paths = vec![exchange.callback_directory.clone()];
        callback_paths.extend(
            env::var_os("ANSIBLE_CALLBACK_PLUGINS")
                .into_iter()
                .flat_map(|paths| env::split_paths(&paths).collect::<Vec<_>>()),
        );
        let callback_paths = env::join_paths(callback_paths).map_err(|error| {
            ProvisionerError(format!("cannot construct Ansible callback path: {error}"))
        })?;
        let callbacks_enabled = match env::var("ANSIBLE_CALLBACKS_ENABLED") {
            Ok(existing) if !existing.is_empty() => format!("{existing},cvd_resource_facts"),
            _ => "cvd_resource_facts".to_owned(),
        };

        let status = Command::new("ansible-playbook")
            .arg("-i")
            .arg(&exchange.inventory)
            .arg(&definition.playbook)
            .current_dir(&self.working_directory)
            .env("ANSIBLE_INVENTORY", &exchange.inventory)
            .env("ANSIBLE_CALLBACK_PLUGINS", callback_paths)
            .env("ANSIBLE_CALLBACKS_ENABLED", callbacks_enabled)
            .env("CVD_RESOURCE_FACTS_FILE", &exchange.facts)
            .status()
            .map_err(|error| {
                ProvisionerError(format!(
                    "could not run Ansible create playbook `{}` from `{}`: {error}",
                    definition.playbook.display(),
                    self.working_directory.display()
                ))
            })?;
        if !status.success() {
            return Err(ProvisionerError(format!(
                "Ansible create playbook `{}` exited with {status}",
                definition.playbook.display()
            )));
        }

        let returned: IndexMap<String, BTreeMap<String, serde_json::Value>> =
            serde_json::from_reader(fs::File::open(&exchange.facts).map_err(|error| {
                ProvisionerError(format!(
                    "Ansible create did not produce resource facts `{}`: {error}",
                    exchange.facts.display()
                ))
            })?)
            .map_err(|error| {
                ProvisionerError(format!("invalid returned resource facts: {error}"))
            })?;

        for host in returned.keys() {
            if !definition.resources.contains_key(host) {
                return Err(ProvisionerError(format!(
                    "Ansible returned facts for unknown resource `{host}`"
                )));
            }
        }
        let resources = definition
            .resources
            .iter()
            .map(|(host, configured)| {
                let mut attributes = configured.clone();
                if let Some(facts) = returned.get(host) {
                    attributes.extend(facts.clone());
                }
                Resource {
                    id: host.clone(),
                    resource_type: definition.group.clone(),
                    exists: true,
                    created: ResourceLocation {
                        scenario_path: scenario_path.to_owned(),
                        phase: LifecyclePhase::Create,
                    },
                    destroyed: None,
                    attributes,
                    relationships: BTreeSet::new(),
                    sensitive_attributes: BTreeSet::new(),
                }
            })
            .collect();
        Ok(ResourceManifest { resources })
    }
}

impl Provisioner for AnsibleProvisioner {
    fn create(
        &self,
        scenario_path: &str,
        definition: &PhaseDefinition,
    ) -> Result<ResourceManifest, ProvisionerError> {
        match definition.ansible_create() {
            Some(definition) => self.create_with_ansible(scenario_path, definition),
            None => DummyProvisioner.create(scenario_path, definition),
        }
    }

    fn destroy(
        &self,
        _scenario_path: &str,
        _resources: &ResourceManifest,
        playbooks: &[PathBuf],
        inventory: Option<&serde_json::Value>,
        output: &mut dyn Write,
        styled_output: bool,
    ) -> Result<(), ProvisionerError> {
        if playbooks.is_empty() {
            return Ok(());
        }
        let inventory_path = inventory
            .map(|inventory| self.write_destroy_inventory(inventory))
            .transpose()?;
        let result = (|| {
            if let Some(inventory_path) = &inventory_path {
                let line = format!("ANSIBLE_INVENTORY={}", inventory_path.display());
                if styled_output {
                    writeln!(output, "\x1b[90m{line}\x1b[0m")
                } else {
                    writeln!(output, "{line}")
                }
                .map_err(|error| {
                    ProvisionerError(format!("could not report Ansible inventory: {error}"))
                })?;
            }
            for playbook in playbooks {
                writeln!(output, "ansible-playbook {}", playbook.display()).map_err(|error| {
                    ProvisionerError(format!("could not report Ansible command: {error}"))
                })?;
                let status = Command::new("ansible-playbook")
                    .arg(playbook)
                    .current_dir(&self.working_directory)
                    .envs(
                        inventory_path
                            .iter()
                            .map(|path| ("ANSIBLE_INVENTORY", path)),
                    )
                    .status()
                    .map_err(|error| {
                        ProvisionerError(format!(
                            "could not run Ansible destroy playbook `{}`: {error}",
                            playbook.display()
                        ))
                    })?;
                if !status.success() {
                    return Err(ProvisionerError(format!(
                        "Ansible destroy playbook `{}` exited with {status}",
                        playbook.display()
                    )));
                }
            }
            Ok(())
        })();
        if let Some(inventory_path) = inventory_path {
            let _ = fs::remove_file(inventory_path);
        }
        result
    }
}

impl AnsibleProvisioner {
    fn write_destroy_inventory(
        &self,
        inventory: &serde_json::Value,
    ) -> Result<PathBuf, ProvisionerError> {
        let sequence = NEXT_EXCHANGE_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "cvd-ansible-destroy-inventory-{}-{sequence}.json",
            process::id()
        ));
        let data = serde_json::to_vec_pretty(inventory).map_err(|error| {
            ProvisionerError(format!("cannot encode destroy inventory: {error}"))
        })?;
        fs::write(&path, data).map_err(|error| {
            ProvisionerError(format!(
                "cannot write destroy inventory `{}`: {error}",
                path.display()
            ))
        })?;
        Ok(path)
    }
}

struct ExchangeDirectory {
    root: PathBuf,
    inventory: PathBuf,
    facts: PathBuf,
    callback_directory: PathBuf,
    callback: PathBuf,
}

impl ExchangeDirectory {
    fn create() -> Result<Self, ProvisionerError> {
        let sequence = NEXT_EXCHANGE_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!(
            "cvd-ansible-provisioner-{}-{sequence}",
            process::id()
        ));
        fs::create_dir(&root).map_err(|error| {
            ProvisionerError(format!(
                "cannot create Ansible exchange directory `{}`: {error}",
                root.display()
            ))
        })?;
        let callback_directory = root.join("callback_plugins");
        Ok(Self {
            inventory: root.join("inventory.json"),
            facts: root.join("resource-facts.json"),
            callback: callback_directory.join("cvd_resource_facts.py"),
            callback_directory,
            root,
        })
    }
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct ProvisionerError(pub String);
