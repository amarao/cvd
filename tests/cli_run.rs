use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::Value;

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn test_directory(label: &str) -> PathBuf {
    let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("cvd-cli-{label}-{}-{sequence}", std::process::id()))
}

fn fixture_config(destination: &Path) -> PathBuf {
    fs::create_dir_all(destination).expect("create test directory");
    let configuration = destination.join("cvd.yml");
    fs::copy("tests/fixtures/nested.yml", &configuration).expect("copy fixture");
    configuration
}

fn run(arguments: &[&str]) -> std::process::Output {
    command(arguments).output().expect("run cvd binary")
}

fn command(arguments: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cvd"));
    command.args(arguments);
    command
}

fn load_state(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read state")).expect("parse state JSON")
}

fn last_run_id(state_directory: &Path) -> String {
    fs::read_to_string(state_directory.join("last-run"))
        .expect("read last-run")
        .trim()
        .to_owned()
}

#[test]
fn nested_selector_runs_ancestor_chain_and_selected_subtree() {
    let directory = test_directory("nested-selector");
    let configuration = fixture_config(&directory);
    let state_directory = directory.join("state");
    let output = run(&[
        "run",
        "default/restart",
        "--file",
        configuration.to_str().unwrap(),
        "--state-dir",
        state_directory.to_str().unwrap(),
    ]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("summary: run run-"));
    assert!(!stdout.contains("\x1b["));
    assert!(stdout.contains("Scenario: default/restart\n"));
    assert!(stdout.contains("Scenario: default/restart: passed\n"));
    let state = load_state(
        &state_directory
            .join("runs")
            .join(last_run_id(&state_directory))
            .join("state.json"),
    );
    let scenarios = state["scenarios"].as_object().unwrap();
    assert!(scenarios.contains_key("default"));
    assert!(scenarios.contains_key("default/restart"));
    assert!(scenarios.contains_key("default/restart/deep"));
    assert!(!scenarios.contains_key("default/ignored"));
    assert!(!scenarios.contains_key("independent"));
    assert_eq!(
        scenarios["default"]["phases"]["verify"]["status"],
        "skipped"
    );
    assert_eq!(scenarios["default"]["phases"]["prepare"]["status"], "pass");
    assert_eq!(scenarios["default"]["phases"]["cleanup"]["status"], "pass");
    assert_eq!(
        scenarios["default/restart"]["phases"]["prepare"]["status"],
        "skipped"
    );
    assert_eq!(
        scenarios["default/restart"]["phases"]["cleanup"]["status"],
        "skipped"
    );
    assert_eq!(
        scenarios["default/restart"]["resources"]["resources"][0]["id"],
        "mock"
    );
    assert_eq!(
        scenarios["default/restart"]["resources"]["resources"][0]["type"],
        "mock"
    );
    assert_eq!(
        scenarios["default/restart"]["resources"]["resources"][0]["exists"],
        false
    );
    assert_eq!(
        scenarios["default/restart/deep"]["phases"]["destroy"]["status"],
        "pass"
    );
    fs::remove_dir_all(directory).expect("remove only test directory");
}

#[test]
fn state_view_defaults_to_yaml_for_last_run_and_supports_explicit_json_run() {
    let directory = test_directory("state-view");
    let configuration = fixture_config(&directory);
    let state_directory = directory.join(".cvd");
    let run_output = run(&[
        "run",
        "default",
        "--file",
        configuration.to_str().unwrap(),
        "--keep",
    ]);
    assert!(run_output.status.success());
    let run_id = last_run_id(&state_directory);

    let yaml = run(&["state-view", "--file", configuration.to_str().unwrap()]);
    assert!(yaml.status.success());
    let yaml = String::from_utf8(yaml.stdout).unwrap();
    assert!(!yaml.contains("\x1b["));
    assert!(yaml.contains(&format!("run_id: {run_id}")));
    assert!(yaml.contains("scenarios:"));
    assert!(yaml.contains("status: skipped"));

    let json = run(&[
        "state-view",
        "json",
        "--run",
        &run_id,
        "--file",
        configuration.to_str().unwrap(),
    ]);
    assert!(json.status.success());
    let state: Value = serde_json::from_slice(&json.stdout).expect("valid JSON view");
    assert_eq!(state["run_id"], run_id);
    assert!(!state["scenarios"].as_object().unwrap().is_empty());
    if directory.exists() {
        fs::remove_dir_all(directory).expect("remove only test directory");
    }
}

#[test]
fn runs_are_retained_and_last_pointer_selects_the_newest_run() {
    let directory = test_directory("run-history");
    let configuration = fixture_config(&directory);
    let state_directory = directory.join("state");
    for selector in ["default", "independent"] {
        let output = run(&[
            "run",
            selector,
            "--file",
            configuration.to_str().unwrap(),
            "--state-dir",
            state_directory.to_str().unwrap(),
        ]);
        assert!(output.status.success());
    }
    let newest = last_run_id(&state_directory);
    let runs = fs::read_dir(state_directory.join("runs"))
        .expect("read runs")
        .filter_map(Result::ok)
        .count();
    assert_eq!(runs, 2);
    let state = load_state(
        &state_directory
            .join("runs")
            .join(&newest)
            .join("state.json"),
    );
    assert_eq!(state["requested_scenario"], "independent");
    fs::remove_dir_all(directory).expect("remove only test directory");
}

#[test]
fn state_view_rejects_absent_unsafe_and_unknown_runs() {
    let directory = test_directory("state-view-errors");
    let state_directory = directory.join("state");
    let absent = run(&[
        "state-view",
        "--state-dir",
        state_directory.to_str().unwrap(),
    ]);
    assert!(!absent.status.success());
    assert!(String::from_utf8_lossy(&absent.stderr).contains("no last run"));

    for run_id in ["../escape", "missing"] {
        let output = run(&[
            "state-view",
            "json",
            "--run",
            run_id,
            "--state-dir",
            state_directory.to_str().unwrap(),
        ]);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(if run_id == "missing" {
            "unknown run"
        } else {
            "invalid run ID"
        }));
    }
    if directory.exists() {
        fs::remove_dir_all(directory).expect("remove only test directory");
    }
}

#[test]
fn state_resources_filters_deleted_resources_and_shows_provenance() {
    let directory = test_directory("state-resources");
    let configuration = fixture_config(&directory);
    let state_directory = directory.join("state");

    let destroyed_run = run(&[
        "run",
        "independent",
        "--file",
        configuration.to_str().unwrap(),
        "--state-dir",
        state_directory.to_str().unwrap(),
    ]);
    assert!(destroyed_run.status.success());
    let destroyed_run_id = last_run_id(&state_directory);

    let existing = run(&[
        "state-resources",
        "--state-dir",
        state_directory.to_str().unwrap(),
    ]);
    assert!(existing.status.success());
    assert_eq!(String::from_utf8(existing.stdout).unwrap().trim(), "[]");

    let deleted = run(&[
        "state-resources",
        "--deleted",
        "--run",
        &destroyed_run_id,
        "--state-dir",
        state_directory.to_str().unwrap(),
    ]);
    assert!(deleted.status.success());
    let deleted = String::from_utf8(deleted.stdout).unwrap();
    assert!(!deleted.contains("\x1b["));
    assert!(deleted.contains("id: mock"));
    assert!(deleted.contains("type: mock"));
    assert!(deleted.contains("exists: false"));
    assert!(deleted.contains("scenario_path: independent"));
    assert!(deleted.contains("phase: create"));
    assert!(deleted.contains("phase: destroy"));

    let kept_run = run(&[
        "run",
        "independent",
        "--file",
        configuration.to_str().unwrap(),
        "--state-dir",
        state_directory.to_str().unwrap(),
        "--keep",
    ]);
    assert!(kept_run.status.success());
    let existing = run(&[
        "state-resources",
        "--state-dir",
        state_directory.to_str().unwrap(),
    ]);
    assert!(existing.status.success());
    let existing = String::from_utf8(existing.stdout).unwrap();
    assert!(existing.contains("id: mock"));
    assert!(existing.contains("exists: true"));
    assert!(existing.contains("destroyed: null"));

    fs::remove_dir_all(directory).expect("remove only test directory");
}

#[test]
fn state_report_replays_last_and_explicit_runs_without_reading_configuration() {
    let directory = test_directory("state-report");
    let configuration = fixture_config(&directory);
    let state_directory = directory.join("state");

    let nested = run(&[
        "run",
        "default/restart",
        "--file",
        configuration.to_str().unwrap(),
        "--state-dir",
        state_directory.to_str().unwrap(),
    ]);
    assert!(nested.status.success());
    let nested_run_id = last_run_id(&state_directory);

    let independent = run(&[
        "run",
        "independent",
        "--file",
        configuration.to_str().unwrap(),
        "--state-dir",
        state_directory.to_str().unwrap(),
    ]);
    assert!(independent.status.success());
    let independent_run_id = last_run_id(&state_directory);

    // Reports locate state but never parse the configuration that produced it.
    fs::write(&configuration, "this is not valid CVD configuration").unwrap();

    let latest = run(&[
        "state-report",
        "--file",
        configuration.to_str().unwrap(),
        "--state-dir",
        state_directory.to_str().unwrap(),
    ]);
    assert!(latest.status.success());
    let latest = String::from_utf8(latest.stdout).unwrap();
    assert!(!latest.contains("\x1b["));
    assert!(latest.contains("Scenario: independent\n"));
    assert!(latest.contains("independent::dependency skipped\n"));
    assert!(latest.contains("independent::create: 1 resource added\n"));
    assert!(latest.contains("Scenario: independent: passed\n"));
    assert!(latest.ends_with(&format!(
        "summary: run {independent_run_id}, created 1 resource(s), destroyed 1 resource(s), 0 error(s), 0 verifier failure(s)\n"
    )));

    let explicit = run(&[
        "state-report",
        "--run",
        &nested_run_id,
        "--file",
        configuration.to_str().unwrap(),
        "--state-dir",
        state_directory.to_str().unwrap(),
    ]);
    assert!(explicit.status.success());
    let explicit = String::from_utf8(explicit.stdout).unwrap();
    assert!(explicit.contains("Scenario: default\n"));
    assert!(explicit.contains("Scenario: default/restart\n"));
    assert!(explicit.contains("Scenario: default/restart/deep\n"));
    assert!(explicit.contains("default/restart::create: 1 resource added\n"));
    assert!(explicit.contains("Scenario: default/restart: passed\n"));
    assert!(explicit.ends_with(&format!(
        "summary: run {nested_run_id}, created 3 resource(s), destroyed 3 resource(s), 0 error(s), 0 verifier failure(s)\n"
    )));

    fs::remove_dir_all(directory).expect("remove only test directory");
}

#[test]
fn state_report_rejects_absent_unsafe_and_unknown_runs() {
    let directory = test_directory("state-report-errors");
    let state_directory = directory.join("state");
    let absent = run(&[
        "state-report",
        "--state-dir",
        state_directory.to_str().unwrap(),
    ]);
    assert!(!absent.status.success());
    assert!(String::from_utf8_lossy(&absent.stderr).contains("no last run"));

    for run_id in ["../escape", "missing"] {
        let output = run(&[
            "state-report",
            "--run",
            run_id,
            "--state-dir",
            state_directory.to_str().unwrap(),
        ]);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(if run_id == "missing" {
            "unknown run"
        } else {
            "invalid run ID"
        }));
    }
    if directory.exists() {
        fs::remove_dir_all(directory).expect("remove only test directory");
    }
}

#[test]
fn full_dummy_example_runs_every_phase_successfully() {
    let directory = test_directory("dummy-full");
    let state_directory = directory.join("state");
    let configuration = fs::canonicalize("examples/dummy-full/cvd.yml").unwrap();
    let output = run(&[
        "run",
        "--file",
        configuration.to_str().unwrap(),
        "--state-dir",
        state_directory.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let state = load_state(
        &state_directory
            .join("runs")
            .join(last_run_id(&state_directory))
            .join("state.json"),
    );
    let scenario = &state["scenarios"]["full-dummy-lifecycle"];
    for phase in [
        "dependency",
        "create",
        "prepare",
        "converge",
        "idempotence",
        "verify",
        "cleanup",
        "destroy",
    ] {
        assert_eq!(scenario["phases"][phase]["status"], "pass", "phase {phase}");
    }
    assert_eq!(scenario["test_results"][0]["status"], "pass");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn dummy_status_options_control_errors_and_verifier_failures() {
    let directory = test_directory("dummy-statuses");
    fs::create_dir_all(&directory).unwrap();
    let configuration = directory.join("cvd.yml");
    fs::write(
        &configuration,
        r#"version: 1
provisioner: dummy
converger: dummy
verifier: dummy
scenarios:
  create-error:
    create: {dummy: {status: error}}
    destroy: {dummy: {status: ok}}
  converge-error:
    create: {dummy: {status: ok}}
    converge: {dummy: {status: error}}
    cleanup: {dummy: {status: ok}}
    destroy: {dummy: {status: ok}}
  verifier-fail:
    create: {dummy: {status: ok}}
    destroy: {dummy: {status: ok}}
    verify:
      assertion: {verifier: dummy, status: fail}
  verifier-error:
    create: {dummy: {status: ok}}
    destroy: {dummy: {status: ok}}
    verify:
      broken: {verifier: dummy, status: error}
  destroy-error:
    create: {dummy: {status: ok}}
    destroy: {dummy: {status: error}}
"#,
    )
    .unwrap();
    for (selector, result_path, expected) in [
        ("create-error", "/phases/create/status", "error"),
        ("converge-error", "/phases/converge/status", "error"),
        ("verifier-fail", "/test_results/0/status", "fail"),
        ("verifier-error", "/test_results/0/status", "error"),
        ("destroy-error", "/phases/destroy/status", "error"),
    ] {
        let state_directory = directory.join(format!("state-{selector}"));
        let output = run(&[
            "run",
            selector,
            "--file",
            configuration.to_str().unwrap(),
            "--state-dir",
            state_directory.to_str().unwrap(),
        ]);
        assert!(!output.status.success(), "selector {selector}");
        let state = load_state(
            &state_directory
                .join("runs")
                .join(last_run_id(&state_directory))
                .join("state.json"),
        );
        let scenario = &state["scenarios"][selector];
        assert_eq!(
            scenario.pointer(result_path).unwrap(),
            expected,
            "selector {selector}"
        );
        if selector == "destroy-error" {
            assert_eq!(scenario["resources"]["resources"][0]["exists"], true);
        }
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ansible_provisioner_imports_create_manifest_and_supplies_it_to_destroy() {
    let directory = test_directory("ansible-provisioner");
    let bin = directory.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let create_playbook = directory.join("create.yml");
    let destroy_playbook = directory.join("destroy.yml");
    fs::write(&create_playbook, "---\n").unwrap();
    fs::write(&destroy_playbook, "---\n").unwrap();
    let configuration = directory.join("cvd.yml");
    fs::write(
        &configuration,
        r#"version: 1
provisioner: ansible
converger: dummy
verifier: dummy
scenarios:
  host:
    create:
      ansible:
        playbook: create.yml
        vars:
          requested_name: example
    destroy:
      ansible:
        playbook: destroy.yml
"#,
    )
    .unwrap();
    let executable = bin.join("ansible-playbook");
    fs::write(
        &executable,
        r#"#!/bin/sh
if grep -q '"action": "create"' "$CVD_INPUT_FILE"; then
  cp "$CVD_INPUT_FILE" "$CVD_CAPTURE_CREATE"
  printf '{"protocol_version":1,"invocation_id":"%s","complete":true,"resources":[{"id":"container-123","type":"docker.container","attributes":{"name":"example","ansible_connection":"docker"},"relationships":[],"sensitive_attributes":[]}]}' "$CVD_INVOCATION_ID" > "$CVD_RESULT_FILE"
else
  cp "$CVD_INPUT_FILE" "$CVD_CAPTURE_DESTROY"
fi
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&executable, permissions).unwrap();
    let create_input = directory.join("create-input.json");
    let destroy_input = directory.join("destroy-input.json");
    let state_directory = directory.join("state");
    let mut paths = vec![bin];
    paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    let output = command(&[
        "run",
        "host",
        "--file",
        configuration.to_str().unwrap(),
        "--state-dir",
        state_directory.to_str().unwrap(),
    ])
    .env("PATH", env::join_paths(paths).unwrap())
    .env("CVD_CAPTURE_CREATE", &create_input)
    .env("CVD_CAPTURE_DESTROY", &destroy_input)
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let create = load_state(&create_input);
    assert_eq!(create["cvd"]["action"], "create");
    assert_eq!(create["cvd"]["vars"]["requested_name"], "example");
    assert_eq!(create["cvd"]["resources"], serde_json::json!([]));
    for field in ["input_file", "result_file"] {
        assert!(Path::new(create["cvd"][field].as_str().unwrap()).is_absolute());
    }
    let destroy = load_state(&destroy_input);
    assert_eq!(destroy["cvd"]["action"], "destroy");
    for field in ["input_file", "result_file", "invocation_id"] {
        assert_ne!(create["cvd"][field], destroy["cvd"][field]);
    }
    assert_eq!(destroy["cvd"]["resources"][0]["id"], "container-123");
    assert_eq!(destroy["cvd"]["resources"][0]["type"], "docker.container");
    assert_eq!(destroy["cvd"]["resources"][0]["exists"], true);

    let state = load_state(
        &state_directory
            .join("runs")
            .join(last_run_id(&state_directory))
            .join("state.json"),
    );
    let resource = &state["scenarios"]["host"]["resources"]["resources"][0];
    assert_eq!(resource["id"], "container-123");
    assert_eq!(resource["created"]["scenario_path"], "host");
    assert_eq!(resource["destroyed"]["phase"], "destroy");
    assert_eq!(resource["exists"], false);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ansible_inventory_overlay_preserves_sources_and_cleanup_after_errors() {
    for mode in [
        "pass",
        "converge-error",
        "unknown-host",
        "duplicate",
        "malformed",
        "keep",
    ] {
        let directory = test_directory(&format!("inventory-{mode}"));
        let bin = directory.join("bin");
        fs::create_dir_all(&bin).unwrap();
        for playbook in ["create.yml", "converge.yml", "cleanup.yml", "destroy.yml"] {
            fs::write(directory.join(playbook), "---\n").unwrap();
        }
        fs::write(
            directory.join("inventory.yml"),
            "all:\n  hosts:\n    web: {}\n",
        )
        .unwrap();
        let configuration = directory.join("cvd.yml");
        fs::write(&configuration, "version: 1\ninventory: [inventory.yml]\nprovisioner: ansible\nconverger: ansible\nverifier: dummy\nscenarios:\n  host:\n    create:\n      ansible:\n        playbook: create.yml\n    converge:\n      ansible:\n        playbook: converge.yml\n    cleanup:\n      ansible:\n        playbook: cleanup.yml\n    destroy:\n      ansible:\n        playbook: destroy.yml\n").unwrap();
        let executable = bin.join("ansible-playbook");
        fs::write(&executable, r#"#!/usr/bin/env python3
import json, os, pathlib, sys
cvd = json.load(open(os.environ['CVD_INPUT_FILE']))['cvd']
assert cvd['input_file'] == os.environ['CVD_INPUT_FILE']
assert cvd['result_file'] == os.environ['CVD_RESULT_FILE']
assert cvd['invocation_id'] == os.environ['CVD_INVOCATION_ID']
mode = os.environ['CVD_TEST_MODE']
args = sys.argv[1:]
sources = [args[i+1] for i, arg in enumerate(args) if arg == '--inventory']
assert pathlib.Path(sources[0]).name == ('destroy-inventory.yml' if cvd['action'] == 'destroy' else 'inventory.yml')
record = {'action': cvd['action'], 'sources': sources, 'resources': cvd['resources']}
if cvd['action'] == 'create':
    assert len(sources) == 1
    binding = {'inventory_hostname': 'typo' if mode == 'unknown-host' else 'web', 'vars': {'ansible_host': 'actual-container', 'ansible_connection': 'local'}}
    if mode == 'malformed': binding['vars'] = []
    resources = [{'id': 'container-id', 'type': 'docker.container', 'attributes': {'ansible': binding}}, {'id': 'network-id', 'type': 'docker.network'}]
    if mode == 'duplicate': resources.append({'id': 'other-id', 'type': 'container', 'attributes': {'ansible': binding}})
    json.dump({'protocol_version': 1, 'invocation_id': cvd['invocation_id'], 'complete': True, 'resources': resources}, open(cvd['result_file'], 'w'))
elif cvd['action'] in ['converge', 'cleanup']:
    assert len(sources) == 2
    record['overlay'] = pathlib.Path(sources[1]).read_text()
elif cvd['action'] == 'destroy':
    assert len(sources) == 1
    assert cvd['resources'][0]['id'] == 'network-id'
    assert 'container-id' in pathlib.Path(sources[0]).read_text()
with open(os.environ['CVD_TEST_CAPTURE'], 'a') as f: f.write(json.dumps(record)+'\n')
if mode == 'converge-error' and cvd['action'] == 'converge': sys.exit(2)
"#).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let inspect = bin.join("ansible-inventory");
        fs::write(&inspect, "#!/bin/sh\nprintf '%s\\n' '{\"all\":{\"children\":[\"webservers\"]},\"webservers\":{\"hosts\":[\"web\"]},\"_meta\":{\"hostvars\":{}}}'\n").unwrap();
        fs::set_permissions(&inspect, fs::Permissions::from_mode(0o755)).unwrap();
        let capture = directory.join("calls.jsonl");
        let state_directory = directory.join("state");
        let mut paths = vec![bin];
        paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
        let mut invocation = command(&[
            "run",
            "--file",
            configuration.to_str().unwrap(),
            "--state-dir",
            state_directory.to_str().unwrap(),
        ]);
        if mode == "keep" {
            invocation.arg("--keep");
        }
        let output = invocation
            .env("PATH", env::join_paths(paths).unwrap())
            .env("CVD_TEST_MODE", mode)
            .env("CVD_TEST_CAPTURE", &capture)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            matches!(mode, "pass" | "keep"),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let calls: Vec<Value> = fs::read_to_string(capture)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(calls.first().unwrap()["action"], "create");
        assert_eq!(
            calls.last().unwrap()["action"],
            if mode == "keep" { "cleanup" } else { "destroy" }
        );
        let state = load_state(
            &state_directory
                .join("runs")
                .join(last_run_id(&state_directory))
                .join("state.json"),
        );
        let scenario = &state["scenarios"]["host"];
        assert_eq!(
            scenario["resources"]["resources"][0]["exists"],
            mode == "keep"
        );
        if matches!(mode, "pass" | "keep" | "converge-error") {
            let overlay = calls[1]["overlay"].as_str().unwrap();
            assert!(overlay.contains("actual-container"));
            assert!(!overlay.contains("network-id"));
            assert!(
                Path::new(
                    scenario["views"]["ansible_inventory"]["attributes"]["path"]
                        .as_str()
                        .unwrap()
                )
                .exists()
            );
            assert_eq!(scenario["phases"]["cleanup"]["status"], "pass");
        }
        if !matches!(mode, "pass" | "keep") {
            assert_eq!(state["primary_error"]["scenario_path"], "host");
            assert_eq!(state["primary_error"]["phase"], "converge");
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
#[ignore = "requires Ansible, pytest, and pytest-testinfra"]
fn native_ansible_preserves_group_vars_host_vars_and_default_sources() {
    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let destination = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &destination);
            } else {
                fs::copy(entry.path(), destination).unwrap();
            }
        }
    }
    for source in ["explicit", "config", "environment"] {
        let directory = test_directory(&format!("native-inventory-{source}"));
        copy_tree(Path::new("tests/fixtures/ansible-inventory"), &directory);
        let configuration = directory.join("cvd.yml");
        if source != "explicit" {
            let yaml = fs::read_to_string(&configuration)
                .unwrap()
                .replace("inventory: [inventory.yml]\n", "");
            fs::write(&configuration, yaml).unwrap();
        }
        let state_directory = directory.join("state");
        let mut invocation = command(&[
            "run",
            "--file",
            configuration.to_str().unwrap(),
            "--state-dir",
            state_directory.to_str().unwrap(),
        ]);
        invocation
            .env("ANSIBLE_LOCAL_TEMP", directory.join("local-tmp"))
            .env("ANSIBLE_REMOTE_TEMP", directory.join("remote-tmp"))
            .env("ANSIBLE_CONFIG", directory.join("ansible.cfg"))
            .env_remove("ANSIBLE_INVENTORY");
        if source == "environment" {
            // A distinct source proves that the environment overrides cfg.
            fs::rename(
                directory.join("inventory.yml"),
                directory.join("environment.yml"),
            )
            .unwrap();
            invocation.env("ANSIBLE_INVENTORY", directory.join("environment.yml"));
        }
        let output = invocation.output().unwrap();
        assert!(
            output.status.success(),
            "{source}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let state = load_state(
            &state_directory
                .join("runs")
                .join(last_run_id(&state_directory))
                .join("state.json"),
        );
        let scenario = &state["scenarios"]["native"];
        assert_eq!(scenario["phases"]["converge"]["status"], "pass");
        assert_eq!(scenario["phases"]["verify"]["status"], "pass");
        assert_eq!(scenario["test_results"][0]["status"], "pass");
        assert_eq!(scenario["resources"]["resources"][0]["exists"], false);
        assert!(scenario["views"]["ansible_inventory"]["attributes"]["path"].is_string());
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn pytest_receives_inventory_and_all_unsuccessful_exits_are_errors() {
    for mode in [
        "pass",
        "1",
        "2",
        "3",
        "4",
        "5",
        "6",
        "spawn-error",
        "keep",
        "nested",
        "external-only",
    ] {
        let directory = test_directory(&format!("pytest-{mode}"));
        let bin = directory.join("bin");
        fs::create_dir_all(&bin).unwrap();
        for file in [
            "create.yml",
            "destroy.yml",
            "inventory.yml",
            "test_example.py",
        ] {
            fs::write(directory.join(file), "# test fixture\n").unwrap();
        }
        let configuration = directory.join("cvd.yml");
        let create = if mode == "external-only" {
            ""
        } else {
            "    create:\n      ansible:\n        playbook: create.yml\n"
        };
        let yaml = format!(
            r#"version: 1
inventory: [inventory.yml]
provisioner: ansible
converger: dummy
verifier: pytest
scenarios:
  root:
{create}    cleanup:
    destroy:
      ansible:
        playbook: destroy.yml
    verify:
      first:
        pytest:
          path: test_example.py
          args: ["-k", "a name with spaces", "$(not-a-shell)"]
      second:
        verifier: dummy
    nested:
      - name: child
        verify:
          inherited:
            pytest:
              path: test_example.py
"#
        );
        fs::write(&configuration, yaml).unwrap();
        let ansible = bin.join("ansible-playbook");
        fs::write(&ansible, r#"#!/usr/bin/env python3
import json, os
cvd = json.load(open(os.environ['CVD_INPUT_FILE']))['cvd']
assert cvd['input_file'] == os.environ['CVD_INPUT_FILE']
assert cvd['result_file'] == os.environ['CVD_RESULT_FILE']
assert cvd['invocation_id'] == os.environ['CVD_INVOCATION_ID']
if cvd['action'] == 'create':
    json.dump({'protocol_version': 1, 'invocation_id': cvd['invocation_id'], 'complete': True, 'resources': [{'id': 'actual-web', 'type': 'container', 'attributes': {'ansible': {'inventory_hostname': 'web', 'vars': {'ansible_host': 'runtime-web'}}}}]}, open(cvd['result_file'], 'w'))
else:
    json.dump(cvd['resources'], open(os.environ['CVD_TEST_DESTROY'], 'w'))
"#).unwrap();
        fs::set_permissions(&ansible, fs::Permissions::from_mode(0o755)).unwrap();
        let inspect = bin.join("ansible-inventory");
        fs::write(
            &inspect,
            "#!/bin/sh\nprintf '%s\\n' '{\"all\":{\"hosts\":[\"web\"]}}'\n",
        )
        .unwrap();
        fs::set_permissions(&inspect, fs::Permissions::from_mode(0o755)).unwrap();
        let pytest = bin.join("pytest");
        fs::write(
            &pytest,
            if mode == "spawn-error" {
                "#!/cvd-missing-interpreter\n"
            } else {
                r#"#!/usr/bin/env python3
import json, os, pathlib, sys
sources = os.environ['ANSIBLE_INVENTORY'].split(',')
record = {'sources': sources, 'args': sys.argv[1:], 'cwd': os.getcwd()}
if len(sources) > 1: record['overlay'] = pathlib.Path(sources[-1]).read_text()
with open(os.environ['CVD_TEST_CAPTURE'], 'a') as f: f.write(json.dumps(record) + '\n')
mode = os.environ['CVD_TEST_MODE']
sys.exit(int(mode) if mode.isdigit() else 0)
"#
            },
        )
        .unwrap();
        fs::set_permissions(&pytest, fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![bin];
        paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
        let state_directory = directory.join("state");
        let capture = directory.join("pytest.jsonl");
        let destroy = directory.join("destroy.json");
        let mut invocation = command(&[
            "run",
            "--file",
            configuration.to_str().unwrap(),
            "--state-dir",
            state_directory.to_str().unwrap(),
        ]);
        if mode == "nested" {
            invocation.arg("root/child");
        }
        if mode == "keep" {
            invocation.arg("--keep");
        }
        let output = invocation
            .env("PATH", env::join_paths(paths).unwrap())
            .env("CVD_TEST_MODE", mode)
            .env("CVD_TEST_CAPTURE", &capture)
            .env("CVD_TEST_DESTROY", &destroy)
            .env("ANSIBLE_INVENTORY", "must-be-overridden")
            .output()
            .unwrap();
        let success = matches!(mode, "pass" | "keep" | "nested" | "external-only");
        assert_eq!(
            output.status.success(),
            success,
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let state = load_state(
            &state_directory
                .join("runs")
                .join(last_run_id(&state_directory))
                .join("state.json"),
        );
        let path = if mode == "nested" {
            "root/child"
        } else {
            "root"
        };
        let scenario = &state["scenarios"][path];
        let result = &scenario["test_results"][0];
        assert_eq!(result["status"], if success { "pass" } else { "error" });
        if !success {
            assert_eq!(scenario["test_results"].as_array().unwrap().len(), 1);
            assert_eq!(state["primary_error"]["phase"], "verify");
            assert_eq!(state["primary_error"]["scenario_path"], path);
            assert!(result["message"].as_str().unwrap().contains("pytest"));
        }
        if mode != "spawn-error" {
            let first: Value =
                serde_json::from_str(fs::read_to_string(capture).unwrap().lines().next().unwrap())
                    .unwrap();
            assert_eq!(
                first["sources"][0],
                directory.join("inventory.yml").to_str().unwrap()
            );
            assert_eq!(first["cwd"], directory.to_str().unwrap());
            if mode == "external-only" {
                assert_eq!(first["sources"].as_array().unwrap().len(), 1);
            } else {
                assert_eq!(first["sources"].as_array().unwrap().len(), 2);
                assert!(first["overlay"].as_str().unwrap().contains("runtime-web"));
                assert_eq!(
                    scenario["views"]["ansible_inventory"]["created"]["phase"],
                    "verify"
                );
            }
            if mode != "nested" {
                assert_eq!(first["args"][1], "a name with spaces");
                assert_eq!(first["args"][2], "$(not-a-shell)");
            }
            assert_eq!(
                first["args"].as_array().unwrap().last().unwrap(),
                directory.join("test_example.py").to_str().unwrap()
            );
        }
        if mode == "keep" {
            assert!(!destroy.exists());
        } else {
            assert!(destroy.exists());
        }
        if mode != "external-only" {
            assert_eq!(
                state["scenarios"]["root"]["resources"]["resources"][0]["exists"],
                mode == "keep"
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }
}
