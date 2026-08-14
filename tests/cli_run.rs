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
        "dummy"
    );
    assert_eq!(
        scenarios["default/restart"]["resources"]["resources"][0]["attributes"]["ipv6"],
        "::1"
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
    assert!(deleted.contains("id: dummy"));
    assert!(deleted.contains("type: dummy"));
    assert!(deleted.contains("exists: false"));
    assert!(deleted.contains("scenario_path: independent"));
    assert!(deleted.contains("phase: create"));
    assert!(deleted.contains("phase: destroy"));
    assert!(deleted.contains("ipv6: ::1"));

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
    assert!(existing.contains("id: dummy"));
    assert!(existing.contains("exists: true"));
    assert!(existing.contains("ipv6: ::1"));
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
    assert!(latest.contains("dependency skipped\n"));
    assert!(latest.contains("create 1 added\n"));
    assert!(latest.contains("Scenario: independent: passed\n"));
    assert!(latest.ends_with(&format!(
        "summary: run {independent_run_id}, 0 error(s), 0 verifier failure(s)\n"
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
    assert!(explicit.contains("create 1 added\n"));
    assert!(explicit.contains("Scenario: default/restart: passed\n"));
    assert!(explicit.ends_with(&format!(
        "summary: run {nested_run_id}, 0 error(s), 0 verifier failure(s)\n"
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
fn ansible_converger_runs_resolved_playbooks_from_the_configuration_directory() {
    let directory = test_directory("ansible-converger");
    fs::create_dir_all(&directory).unwrap();
    let configuration = directory.join("cvd.yml");
    fs::write(
        &configuration,
        r#"
version: 1
provisioner: dummy
converger: ansible
verifier: dummy
scenarios:
  default:
    prepare:
    converge: converge-custom.yml
    idempotence: [first.yml, second.yml]
    cleanup:
      ansible: cleanup-custom.yml
"#,
    )
    .unwrap();
    for playbook in [
        "prepare.yaml",
        "converge-custom.yml",
        "first.yml",
        "second.yml",
        "cleanup-custom.yml",
    ] {
        fs::write(directory.join(playbook), "---\n").unwrap();
    }

    let binary_directory = directory.join("bin");
    fs::create_dir_all(&binary_directory).unwrap();
    let ansible_playbook = binary_directory.join("ansible-playbook");
    fs::write(
        &ansible_playbook,
        "#!/bin/sh\nprintf 'cwd=%s playbook=%s\\n' \"$PWD\" \"$1\" >> \"$CVD_ANSIBLE_TRACE\"\nexit \"${CVD_ANSIBLE_EXIT:-0}\"\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&ansible_playbook).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&ansible_playbook, permissions).unwrap();
    let trace = directory.join("ansible.trace");
    let mut search_path = vec![binary_directory];
    search_path.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    let search_path = env::join_paths(search_path).unwrap();

    let state_directory = directory.join("state");
    let arguments = [
        "run",
        "default",
        "--file",
        configuration.to_str().unwrap(),
        "--state-dir",
        state_directory.to_str().unwrap(),
    ];
    let output = command(&arguments)
        .env("PATH", &search_path)
        .env("CVD_ANSIBLE_TRACE", &trace)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout).unwrap();
    let expected = [
        "prepare.yaml",
        "converge-custom.yml",
        "first.yml",
        "second.yml",
        "cleanup-custom.yml",
    ]
    .map(|playbook| format!("ansible-playbook {}\n", directory.join(playbook).display()));
    let mut previous = 0;
    for line in &expected {
        let position = output
            .find(line)
            .unwrap_or_else(|| panic!("missing `{line}` in {output}"));
        assert!(position >= previous);
        previous = position;
    }
    let trace_output = fs::read_to_string(&trace).unwrap();
    let trace_lines = trace_output.lines().collect::<Vec<_>>();
    assert_eq!(trace_lines.len(), 5);
    for (line, playbook) in trace_lines.iter().zip([
        "prepare.yaml",
        "converge-custom.yml",
        "first.yml",
        "second.yml",
        "cleanup-custom.yml",
    ]) {
        assert_eq!(
            *line,
            format!(
                "cwd={} playbook={}",
                directory.display(),
                directory.join(playbook).display()
            )
        );
    }

    let failed_state_directory = directory.join("failed-state");
    let failed_arguments = [
        "run",
        "default",
        "--file",
        configuration.to_str().unwrap(),
        "--state-dir",
        failed_state_directory.to_str().unwrap(),
    ];
    let failed = command(&failed_arguments)
        .env("PATH", search_path)
        .env("CVD_ANSIBLE_TRACE", &trace)
        .env("CVD_ANSIBLE_EXIT", "7")
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("1 error(s)"));
    assert!(String::from_utf8_lossy(&failed.stdout).contains("Scenario: default: error"));
    let failed_state = load_state(
        &failed_state_directory
            .join("runs")
            .join(last_run_id(&failed_state_directory))
            .join("state.json"),
    );
    assert_eq!(
        failed_state["scenarios"]["default"]["phases"]["prepare"]["status"],
        "error"
    );
    assert!(
        failed_state["primary_error"]["message"]
            .as_str()
            .unwrap()
            .contains("exited with exit status: 7")
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn ansible_provisioner_passes_inventory_and_collects_resource_facts() {
    let directory = test_directory("ansible-provisioner");
    fs::create_dir_all(&directory).unwrap();
    let binary_directory = directory.join("bin");
    fs::create_dir_all(&binary_directory).unwrap();
    let ansible_playbook = binary_directory.join("ansible-playbook");
    fs::write(
        &ansible_playbook,
        r#"#!/bin/sh
if [ "$1" = "-i" ]; then
  cp "$2" "$CVD_CAPTURED_INVENTORY"
  returned_facts=${CVD_RETURNED_FACTS:-'{"vm1":{"public_ip":"127.0.0.1"},"vm2":{"public_ip":"127.0.0.1"}}'}
  printf '%s\n' "$returned_facts" > "$CVD_RESOURCE_FACTS_FILE"
else
  cp "$ANSIBLE_INVENTORY" "$CVD_CONVERGE_INVENTORY"
fi
exit 0
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&ansible_playbook).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&ansible_playbook, permissions).unwrap();

    let mut search_path = vec![binary_directory];
    search_path.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    let search_path = env::join_paths(search_path).unwrap();
    let state_directory = directory.join("state");
    let captured_inventory = directory.join("inventory.json");
    let converge_inventory = directory.join("converge-inventory.json");
    let configuration = fs::canonicalize("examples/minimal-provisioner/cvd.yml").unwrap();
    let arguments = [
        "run",
        "provisioned-resource",
        "--file",
        configuration.to_str().unwrap(),
        "--state-dir",
        state_directory.to_str().unwrap(),
    ];
    let output = command(&arguments)
        .env("PATH", &search_path)
        .env("CVD_CAPTURED_INVENTORY", &captured_inventory)
        .env("CVD_CONVERGE_INVENTORY", &converge_inventory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let inventory = load_state(&captured_inventory);
    assert_eq!(
        inventory["mygroup2"]["hosts"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["vm1", "vm2"]
    );
    assert_eq!(inventory["mygroup2"]["hosts"]["vm1"]["flavor"], "SSD.30");
    assert_eq!(inventory["mygroup2"]["hosts"]["vm2"]["flavor"], "SSD.40");
    let converge_inventory_json = load_state(&converge_inventory);
    assert_eq!(
        converge_inventory_json["mygroup2"]["hosts"]["vm1"]["public_ip"],
        "127.0.0.1"
    );
    assert_eq!(
        converge_inventory_json["mygroup2"]["hosts"]["vm1"]["ansible_host"],
        "127.0.0.1"
    );
    assert_eq!(
        converge_inventory_json["mygroup2"]["hosts"]["vm2"]["public_ip"],
        "127.0.0.1"
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let inventory_line = stdout.find("ANSIBLE_INVENTORY=").unwrap();
    let command_line = stdout.find("ansible-playbook").unwrap();
    assert!(inventory_line < command_line);

    let state = load_state(
        &state_directory
            .join("runs")
            .join(last_run_id(&state_directory))
            .join("state.json"),
    );
    let resources = state["scenarios"]["provisioned-resource"]["resources"]["resources"]
        .as_array()
        .unwrap();
    assert_eq!(resources.len(), 2);
    assert_eq!(
        resources
            .iter()
            .map(|resource| resource["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["vm1", "vm2"]
    );
    for (resource, (id, flavor)) in resources.iter().zip([("vm1", "SSD.30"), ("vm2", "SSD.40")]) {
        assert_eq!(resource["id"], id);
        assert_eq!(resource["type"], "mygroup2");
        assert_eq!(resource["attributes"]["flavor"], flavor);
        assert_eq!(resource["attributes"]["public_ip"], "127.0.0.1");
        assert_eq!(resource["created"]["scenario_path"], "provisioned-resource");
        assert_eq!(resource["created"]["phase"], "create");
        assert_eq!(
            resource["destroyed"]["scenario_path"],
            "provisioned-resource"
        );
        assert_eq!(resource["destroyed"]["phase"], "destroy");
        assert_eq!(resource["exists"], false);
    }

    let invalid_state_directory = directory.join("invalid-state");
    let invalid_arguments = [
        "run",
        "provisioned-resource",
        "--file",
        configuration.to_str().unwrap(),
        "--state-dir",
        invalid_state_directory.to_str().unwrap(),
    ];
    let invalid = command(&invalid_arguments)
        .env("PATH", &search_path)
        .env("CVD_CAPTURED_INVENTORY", &captured_inventory)
        .env("CVD_CONVERGE_INVENTORY", &converge_inventory)
        .env(
            "CVD_RETURNED_FACTS",
            r#"{"unknown":{"public_ip":"192.0.2.1"}}"#,
        )
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    let invalid_state = load_state(
        &invalid_state_directory
            .join("runs")
            .join(last_run_id(&invalid_state_directory))
            .join("state.json"),
    );
    assert_eq!(
        invalid_state["scenarios"]["provisioned-resource"]["phases"]["create"]["status"],
        "error"
    );
    assert!(
        invalid_state["primary_error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown resource `unknown`")
    );
    fs::remove_dir_all(directory).unwrap();
}
