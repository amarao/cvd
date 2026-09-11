//! Shared invocation context for Ansible playbooks and pytest inventory.

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Serialize;

use crate::state::Resource;

static NEXT_INVOCATION: AtomicU64 = AtomicU64::new(0);
const PROTOCOL_VERSION: u32 = 1;

#[derive(Serialize)]
pub(crate) struct Input<'a> {
    cvd: CvdInput<'a>,
}

#[derive(Serialize)]
struct CvdInput<'a> {
    protocol_version: u32,
    invocation_id: &'a str,
    input_file: &'a Path,
    result_file: &'a Path,
    directory: &'a Path,
    action: &'static str,
    scenario_selector: &'a str,
    vars: &'a serde_json::Value,
    resources: &'a [Resource],
    resources_by_type: BTreeMap<&'a str, Vec<&'a Resource>>,
}

pub(crate) struct Invocation {
    pub(crate) directory: PathBuf,
    pub(crate) input: PathBuf,
    pub(crate) result: PathBuf,
    pub(crate) invocation_id: String,
}

impl Invocation {
    pub(crate) fn create() -> Result<Self, String> {
        let sequence = NEXT_INVOCATION.fetch_add(1, Ordering::Relaxed);
        let invocation_id = format!("{}-{sequence}", process::id());
        let directory = std::env::temp_dir().join(format!("cvd-invocation-{invocation_id}"));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|error| {
                format!(
                    "cannot create CVD invocation directory `{}`: {error}",
                    directory.display()
                )
            })?;
        Ok(Self {
            input: directory.join("input.json"),
            result: directory.join("result.json"),
            directory,
            invocation_id,
        })
    }

    pub(crate) fn write_input<'a>(
        &'a self,
        directory: &'a Path,
        action: &'static str,
        scenario_selector: &'a str,
        vars: &'a serde_json::Value,
        resources: &'a [Resource],
    ) -> Result<Input<'a>, String> {
        let input = Input {
            cvd: CvdInput {
                protocol_version: PROTOCOL_VERSION,
                invocation_id: &self.invocation_id,
                input_file: &self.input,
                result_file: &self.result,
                directory,
                action,
                scenario_selector,
                vars,
                resources,
                resources_by_type: resources_by_type(resources),
            },
        };
        let data = serde_json::to_vec_pretty(&input)
            .map_err(|error| format!("cannot encode CVD input: {error}"))?;
        write_private(&self.input, &data)?;
        Ok(input)
    }

    pub(crate) fn write_inventory(&self, input: &Input<'_>) -> Result<PathBuf, String> {
        let path = self.directory.join("cvd-inventory.yml");
        let inventory = serde_json::json!({"all": {"vars": {"cvd": input.cvd}}});
        let data = serde_yaml::to_string(&inventory)
            .map_err(|error| format!("cannot encode CVD inventory: {error}"))?;
        write_private(&path, data.as_bytes())?;
        Ok(path)
    }
}

impl Drop for Invocation {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| file.write_all(data))
        .map_err(|error| format!("cannot write CVD context file `{}`: {error}", path.display()))
}

pub(crate) fn resources_by_type(resources: &[Resource]) -> BTreeMap<&str, Vec<&Resource>> {
    let mut grouped = BTreeMap::new();
    for resource in resources {
        grouped
            .entry(resource.resource_type.as_str())
            .or_insert_with(Vec::new)
            .push(resource);
    }
    grouped
}
