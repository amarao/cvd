//! Exercise deadlines at the CLI boundary with actual sleeping subprocesses.
use serde_json::Value;
use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = env::temp_dir().join(format!(
            "cvd-timeout-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(directory.join("bin")).unwrap();
        fs::write(directory.join("play.yml"), "---\n").unwrap();
        fs::write(directory.join("test.py"), "").unwrap();
        fs::write(
            directory.join("inventory.yml"),
            "all:\n  hosts:\n    node: {}\n",
        )
        .unwrap();
        for executable in [
            "ansible-playbook",
            "ansible-inventory",
            "ansible-config",
            "pytest",
        ] {
            let path = directory.join("bin").join(executable);
            fs::write(&path, MOCK).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let fixture = Self { directory };
        fixture.config(CONFIG);
        fixture
    }

    fn config(&self, yaml: &str) {
        fs::write(self.directory.join("cvd.yml"), yaml).unwrap();
    }

    fn run(&self, hangs: &str, extra: &[(&str, &str)], args: &[&str]) -> Value {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cvd"));
        command
            .args(["run", "-F"])
            .arg(&self.directory)
            .args(args)
            .env(
                "PATH",
                env::join_paths([
                    self.directory.join("bin"),
                    PathBuf::from("/usr/bin"),
                    PathBuf::from("/bin"),
                ])
                .unwrap(),
            )
            .env("TIMEOUT_HANGS", hangs)
            .env("TIMEOUT_DIRECTORY", &self.directory)
            .env_remove("ANSIBLE_INVENTORY");
        for &(key, value) in extra {
            command.env(key, value);
        }
        let started = Instant::now();
        let output = command.output().unwrap();
        assert!(
            !output.status.success(),
            "expected timeout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timeout failed to bound execution"
        );
        let run = fs::read_to_string(self.directory.join(".cvd/last-run")).unwrap();
        serde_json::from_slice(
            &fs::read(
                self.directory
                    .join(".cvd/runs")
                    .join(run.trim())
                    .join("state.json"),
            )
            .unwrap(),
        )
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

const CONFIG: &str = "version: 1
inventory: [inventory.yml]
scenarios:
  root:
    timeout: 1
    create: {ansible: {playbook: play.yml}}
    prepare: {ansible: {playbook: play.yml}}
    converge: {ansible: {playbook: play.yml}}
    idempotence: {dummy: {}}
    verify:
      - {name: first, ansible: {playbook: play.yml}}
      - {name: second, ansible: {playbook: play.yml}}
    side_effect: {ansible: {playbook: play.yml}}
    nested:
      - name: child
        verify: {name: check, dummy: {}}
    cleanup: {ansible: {playbook: play.yml}}
    destroy: {ansible: {playbook: play.yml}}
";

const MOCK: &str = r#"#!/usr/bin/python3
import json, os, pathlib, sys, time
directory = pathlib.Path(os.environ['TIMEOUT_DIRECTORY'])
program = pathlib.Path(sys.argv[0]).name
data = None
if program == 'ansible-playbook':
    data = json.loads(pathlib.Path(os.environ['CVD_INPUT_FILE']).read_text())['cvd']
    action = data['action']
else:
    action = program
with (directory / 'calls').open('a') as f: f.write(action + '\n')
hangs = os.environ.get('TIMEOUT_HANGS', '').split(',')
if action in hangs or (data and data['scenario_selector'] + ':' + action in hangs):
    time.sleep(60)
if action in os.environ.get('TIMEOUT_DELAYS', '').split(','):
    time.sleep(0.65)
if action == 'create':
    resource = {'id': 'created-vm', 'type': 'test.vm'}
    if os.environ.get('TIMEOUT_BINDING'):
        resource['attributes'] = {'ansible': {'inventory_hostname': 'node'}}
    pathlib.Path(data['result_file']).write_text(json.dumps({
        'manifest_version': 1, 'invocation_id': data['invocation_id'],
        'complete': True, 'resources': [resource]}))
elif program == 'ansible-inventory':
    print(json.dumps({'all': {'hosts': ['node']}}))
elif program == 'ansible-config':
    print(json.dumps([{'name': 'DEFAULT_HOST_LIST', 'value': [str(directory / 'inventory.yml')]}]))
"#;

#[test]
fn each_external_phase_times_out_and_unwinds_with_fresh_budgets() {
    for phase in [
        "create",
        "prepare",
        "converge",
        "verify",
        "side_effect",
        "cleanup",
        "destroy",
    ] {
        let fixture = Fixture::new();
        let state = fixture.run(phase, &[], &[]);
        let root = &state["scenarios"]["root"];
        assert_eq!(root["phases"][phase]["status"], "error", "{phase}: {state}");
        let error = if phase == "destroy" {
            &state["cleanup_errors"][0]
        } else {
            &state["primary_error"]
        };
        assert_eq!(error["phase"], phase);
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("timed out after 1 seconds")
        );
        assert_eq!(
            root["phases"]["destroy"]["status"],
            if phase == "destroy" { "error" } else { "pass" }
        );
        let resources = root["resources"]["resources"].as_array().unwrap();
        if phase == "create" {
            assert!(resources.is_empty());
        } else {
            assert_eq!(resources[0]["exists"], phase == "destroy");
        }
        if ["create", "prepare", "converge", "verify", "side_effect"].contains(&phase) {
            assert!(state["scenarios"].get("root/child").is_none());
        }
        if phase == "verify" {
            assert_eq!(root["test_results"].as_array().unwrap().len(), 1);
            assert_eq!(root["test_results"][0]["status"], "error");
        }
    }
}

#[test]
fn cleanup_and_destroy_timeouts_preserve_the_original_failure() {
    let fixture = Fixture::new();
    let state = fixture.run("converge,cleanup,destroy", &[], &[]);
    assert_eq!(state["primary_error"]["phase"], "converge");
    let errors = state["cleanup_errors"].as_array().unwrap();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0]["phase"], "cleanup");
    assert_eq!(errors[1]["phase"], "destroy");
    assert_eq!(
        state["scenarios"]["root"]["resources"]["resources"][0]["exists"],
        true
    );
}

#[test]
fn named_tests_share_one_verify_budget() {
    let fixture = Fixture::new();
    let state = fixture.run("", &[("TIMEOUT_DELAYS", "verify")], &[]);
    let tests = state["scenarios"]["root"]["test_results"]
        .as_array()
        .unwrap();
    assert_eq!(tests.len(), 2);
    assert_eq!(tests[0]["status"], "pass");
    assert_eq!(tests[1]["status"], "error");
    assert_eq!(state["primary_error"]["phase"], "verify");
}

#[test]
fn inventory_helpers_are_bounded_by_the_consuming_phase() {
    for helper in ["ansible-config", "ansible-inventory"] {
        let fixture = Fixture::new();
        fixture.config(&CONFIG.replace("inventory: [inventory.yml]\n", ""));
        let state = fixture.run(helper, &[("TIMEOUT_BINDING", "1")], &[]);
        assert_eq!(state["primary_error"]["phase"], "prepare");
        assert!(
            state["primary_error"]["message"]
                .as_str()
                .unwrap()
                .contains("timed out")
        );
        assert_eq!(
            state["scenarios"]["root"]["phases"]["destroy"]["status"],
            "pass"
        );
    }
}

#[test]
fn pytest_timeout_is_a_named_test_error_and_keep_retains_resources() {
    let fixture = Fixture::new();
    fixture.config(&CONFIG.replace(
        "{name: first, ansible: {playbook: play.yml}}",
        "{name: first, pytest: {path: test.py}}",
    ));
    let state = fixture.run("pytest", &[], &["--keep"]);
    let root = &state["scenarios"]["root"];
    assert_eq!(state["primary_error"]["phase"], "verify");
    assert_eq!(root["test_results"][0]["name"], "first");
    assert_eq!(root["test_results"][0]["status"], "error");
    assert_eq!(root["phases"]["cleanup"]["status"], "pass");
    assert_eq!(root["phases"]["destroy"]["status"], "skipped");
    assert_eq!(root["resources"]["resources"][0]["exists"], true);
}

#[test]
fn inventory_setup_and_playbook_share_the_same_deadline() {
    let fixture = Fixture::new();
    let state = fixture.run(
        "",
        &[
            ("TIMEOUT_BINDING", "1"),
            ("TIMEOUT_DELAYS", "ansible-inventory,prepare"),
        ],
        &[],
    );
    assert_eq!(state["primary_error"]["phase"], "prepare");
    assert!(
        state["primary_error"]["message"]
            .as_str()
            .unwrap()
            .contains("timed out")
    );
}

#[test]
fn selected_child_inherits_its_timeout_and_unwinds_parent_resources() {
    let fixture = Fixture::new();
    fixture.config(&CONFIG.replace(
        "- name: child\n",
        "- name: child\n        converge: {ansible: {playbook: play.yml}}\n",
    ));
    let state = fixture.run("root/child:converge", &[], &["root/child"]);
    assert_eq!(state["primary_error"]["scenario_path"], "root/child");
    assert!(
        state["primary_error"]["message"]
            .as_str()
            .unwrap()
            .contains("timed out after 1 seconds")
    );
    let root = &state["scenarios"]["root"];
    assert_eq!(root["phases"]["verify"]["status"], "skipped");
    assert_eq!(root["phases"]["destroy"]["status"], "pass");
    assert_eq!(root["resources"]["resources"][0]["exists"], false);
    assert!(
        state["scenarios"]["root/child"]["resources"]["resources"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
