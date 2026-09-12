#![cfg(unix)]

use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::{Value, json};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

// The adapters send actual signals to their waiting CVD parent. Each adapter
// invocation waits for its acknowledgement before returning and observes the
// preceding state checkpoint. This covers notification timing as well as live
// destruction decisions and persistence.
fn run_case(
    initial_keep: bool,
    selector: Option<&str>,
    signals: &[(&str, &str)],
    failure: Option<&str>,
    expected_destroyed: &[&str],
) {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    let directory = env::temp_dir().join(format!(
        "cvd-keep-signals-{}-{sequence}",
        std::process::id()
    ));
    let bin = directory.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(directory.join("phase.yml"), "---\n").unwrap();
    fs::write(directory.join("probe.py"), "").unwrap();
    fs::write(directory.join("inventory.yml"), "all: {hosts: {}}\n").unwrap();
    fs::write(
        directory.join("cvd.yml"),
        r#"version: 1
inventory: [inventory.yml]
provisioner: ansible
converger: ansible
verifier: pytest
scenarios:
  root:
    create: &action
      ansible: {playbook: phase.yml}
    converge: *action
    verify: &tests
      probe:
        pytest: {path: probe.py}
    cleanup: *action
    destroy: *action
    nested:
      - name: first
        create: *action
        converge: *action
        verify: *tests
        cleanup: *action
        destroy: *action
      - name: second
        create: *action
        converge: *action
        verify: *tests
        cleanup: *action
        destroy: *action
  other:
    create: *action
    converge: *action
    verify: *tests
    cleanup: *action
    destroy: *action
"#,
    )
    .unwrap();
    for name in ["ansible-playbook", "pytest"] {
        let executable = bin.join(name);
        fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json, os, pathlib, signal, sys, time

if pathlib.Path(sys.argv[0]).name == 'pytest':
    context = pathlib.Path(os.environ['ANSIBLE_INVENTORY'].split(',')[-1])
    input_file = context.parent / 'input.json'
else:
    input_file = pathlib.Path(os.environ['CVD_INPUT_FILE'])
cvd = json.loads(input_file.read_text())['cvd']
directory = pathlib.Path(cvd['directory'])
state_dir = directory / '.cvd'
run_id = (state_dir / 'last-run').read_text().strip()
state = json.loads((state_dir / 'runs' / run_id / 'state.json').read_text())
scenario, action = cvd['scenario_selector'], cvd['action']
call = scenario + '::' + action
with (directory / 'calls.jsonl').open('a') as capture:
    capture.write(json.dumps({'call': call, 'keep': state['keep']}) + '\n')

requested_signal = json.loads(os.environ['CVD_TEST_SIGNALS']).get(call)
if requested_signal:
    mode = 'enabled' if requested_signal == 'SIGUSR1' else 'disabled'
    message = ('\n\nCVD: keep mode ' + mode + ' (' + requested_signal + ')\n').encode()
    stderr = directory / 'stderr.log'
    previous = stderr.read_bytes().count(message)
    os.kill(os.getppid(), getattr(signal, requested_signal))
    # The adapter cannot finish until the listener prints. A message deferred
    # to a lifecycle checkpoint would time out here and fail the test.
    deadline = time.monotonic() + 5
    while stderr.read_bytes().count(message) == previous:
        assert time.monotonic() < deadline, 'no acknowledgement while adapter was running'
        time.sleep(0.01)
    current = json.loads((state_dir / 'runs' / run_id / 'state.json').read_text())
    assert current['scenarios'][scenario]['phases'][action]['status'] == 'running'
    assert current['keep'] == state['keep'], 'notification must not write state'

if action == 'create':
    result = {'manifest_version': 1, 'invocation_id': cvd['invocation_id'],
              'complete': True, 'resources': [{'id': scenario, 'type': 'test.resource'}]}
    pathlib.Path(cvd['result_file']).write_text(json.dumps(result))
elif action == 'destroy':
    assert [r['id'] for r in cvd['resources']] == [scenario]
    assert all(r['created']['scenario_path'] == scenario for r in cvd['resources'])

if call == os.environ.get('CVD_TEST_FAILURE'):
    sys.exit(2)
"#,
        )
        .unwrap();
        fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut paths = vec![bin];
    paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    let signals: serde_json::Map<String, Value> = signals
        .iter()
        .map(|(call, signal)| ((*call).to_owned(), json!(signal)))
        .collect();
    let mut command = Command::new(env!("CARGO_BIN_EXE_cvd"));
    command
        .args(["run", "--directory"])
        .arg(&directory)
        .stderr(fs::File::create(directory.join("stderr.log")).unwrap())
        .env_remove("ANSIBLE_INVENTORY")
        .env("PATH", env::join_paths(paths).unwrap())
        .env("CVD_TEST_SIGNALS", serde_json::to_string(&signals).unwrap());
    if initial_keep {
        command.arg("--keep");
    }
    if let Some(selector) = selector {
        command.arg(selector);
    }
    if let Some(failure) = failure {
        command.env("CVD_TEST_FAILURE", failure);
    }
    let output = command.output().unwrap();
    let stderr = fs::read_to_string(directory.join("stderr.log")).unwrap();
    // Even an intentionally failed run must exit normally, not via SIGUSR1/2.
    assert_eq!(
        output.status.code(),
        Some(if failure.is_some() { 1 } else { 0 }),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        stderr
    );

    let calls: Vec<Value> = fs::read_to_string(directory.join("calls.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let mut keep = initial_keep;
    let mut expected_notifications = String::new();
    for record in &calls {
        let call = record["call"].as_str().unwrap();
        assert_eq!(record["keep"], keep, "snapshot before {call}");
        if let Some(signal) = signals.get(call) {
            keep = signal == "SIGUSR1";
            let mode = if keep { "enabled" } else { "disabled" };
            expected_notifications.push_str(&format!(
                "\n\nCVD: keep mode {mode} ({})\n",
                signal.as_str().unwrap()
            ));
        }
    }
    assert!(
        stderr.starts_with(&expected_notifications),
        "stderr: {stderr:?}"
    );
    assert_eq!(stderr.matches("CVD: keep mode").count(), signals.len());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("CVD: keep mode"));
    for call in signals.keys() {
        assert!(calls.iter().any(|record| record["call"] == *call));
    }
    let destroyed: Vec<_> = calls
        .iter()
        .filter_map(|record| record["call"].as_str().unwrap().strip_suffix("::destroy"))
        .collect();
    assert_eq!(destroyed, expected_destroyed);

    let state_dir = directory.join(".cvd");
    let run_id = fs::read_to_string(state_dir.join("last-run")).unwrap();
    let state: Value = serde_json::from_slice(
        &fs::read(
            state_dir
                .join("runs")
                .join(run_id.trim())
                .join("state.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(state["keep"], keep);
    let scenarios = state["scenarios"].as_object().unwrap();
    let expected_paths = if selector.is_some() || failure.is_some() {
        vec!["root", "root/first"]
    } else {
        vec!["other", "root", "root/first", "root/second"]
    };
    assert_eq!(
        scenarios.keys().map(String::as_str).collect::<Vec<_>>(),
        expected_paths
    );
    for (path, scenario) in scenarios {
        let destroyed = expected_destroyed.contains(&path.as_str());
        let cleanup_failed = failure == Some(format!("{path}::cleanup").as_str());
        assert_eq!(
            scenario["phases"]["cleanup"]["status"],
            if cleanup_failed { "error" } else { "pass" }
        );
        assert_eq!(
            scenario["phases"]["destroy"]["status"],
            if destroyed { "pass" } else { "skipped" }
        );
        let resources = scenario["resources"]["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0]["exists"], !destroyed);
        assert_eq!(resources[0]["created"]["scenario_path"], *path);
    }
    if let Some(failure) = failure {
        let (path, phase) = failure.split_once("::").unwrap();
        assert_eq!(state["primary_error"]["scenario_path"], path);
        assert_eq!(state["primary_error"]["phase"], phase);
        assert_eq!(scenarios[path]["phases"][phase]["status"], "error");
        if phase == "verify" {
            assert_eq!(scenarios[path]["test_results"][0]["status"], "error");
        }
    } else {
        assert!(state.get("primary_error").is_none());
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn sigusr1_during_create_keeps_selected_child_and_ancestors() {
    run_case(
        false,
        Some("root/first"),
        &[("root/first::create", "SIGUSR1")],
        None,
        &[],
    );
}

#[test]
fn sigusr2_during_cleanup_overrides_cli_keep_and_destroys_child_before_parent() {
    run_case(
        true,
        Some("root/first"),
        &[("root/first::cleanup", "SIGUSR2")],
        None,
        &["root/first", "root"],
    );
}

#[test]
fn repeated_sigusr1_enables_keep_without_toggling_it() {
    run_case(
        false,
        Some("root/first"),
        &[
            ("root/first::converge", "SIGUSR1"),
            ("root/first::verify", "SIGUSR1"),
            ("root/first::cleanup", "SIGUSR1"),
        ],
        None,
        &[],
    );
}

#[test]
fn repeated_sigusr2_disables_keep_without_toggling_it() {
    run_case(
        true,
        Some("root/first"),
        &[
            ("root::converge", "SIGUSR2"),
            ("root/first::verify", "SIGUSR2"),
            ("root/first::cleanup", "SIGUSR2"),
        ],
        None,
        &["root/first", "root"],
    );
}

#[test]
fn alternating_signals_apply_to_future_destroys_without_retrying_skipped_children() {
    run_case(
        false,
        Some("root/first"),
        &[
            ("root::converge", "SIGUSR1"),
            ("root/first::converge", "SIGUSR2"),
            ("root/first::cleanup", "SIGUSR1"),
            ("root::cleanup", "SIGUSR2"),
        ],
        None,
        &["root"],
    );
}

#[test]
fn sigusr1_during_failed_verification_keeps_resources_and_preserves_error() {
    run_case(
        false,
        None,
        &[("root/first::verify", "SIGUSR1")],
        Some("root/first::verify"),
        &[],
    );
}

#[test]
fn sigusr2_during_failed_cleanup_still_allows_best_effort_destruction() {
    run_case(
        true,
        None,
        &[("root/first::cleanup", "SIGUSR2")],
        Some("root/first::cleanup"),
        &["root/first", "root"],
    );
}

#[test]
fn sigusr1_during_destroy_allows_active_destroy_to_finish_and_keeps_parent() {
    run_case(
        false,
        Some("root/first"),
        &[("root/first::destroy", "SIGUSR1")],
        None,
        &["root/first"],
    );
}

#[test]
fn signal_policy_applies_to_later_siblings_and_independent_roots() {
    run_case(
        false,
        None,
        &[
            ("root/first::cleanup", "SIGUSR1"),
            ("root/second::cleanup", "SIGUSR2"),
        ],
        None,
        &["root/second", "root", "other"],
    );
}
