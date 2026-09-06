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
    verify: {dummy: {status: ok}}
    destroy: {dummy: {status: ok}}
    tests:
      assertion: {verifier: dummy, status: fail}
  verifier-error:
    create: {dummy: {status: ok}}
    verify: {dummy: {status: ok}}
    destroy: {dummy: {status: ok}}
    tests:
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
    let destroy = load_state(&destroy_input);
    assert_eq!(destroy["cvd"]["action"], "destroy");
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
