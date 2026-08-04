use std::{
    fs,
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
    Command::new(env!("CARGO_BIN_EXE_cvd"))
        .args(arguments)
        .output()
        .expect("run cvd binary")
}

fn load_state(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read state")).expect("parse state JSON")
}

#[test]
fn nested_selector_runs_ancestor_chain_and_selected_subtree() {
    let directory = test_directory("nested-selector");
    let configuration = fixture_config(&directory);
    let state = directory.join("selected-state.json");
    let output = run(&[
        "run",
        "default/restart",
        "--file",
        configuration.to_str().unwrap(),
        "--state",
        state.to_str().unwrap(),
    ]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("default create pass"));
    assert!(stdout.contains("default/restart create pass"));
    assert!(stdout.contains("default/restart/deep create pass"));
    let state = load_state(&state);
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
    assert_eq!(
        scenarios["default/restart"]["resources"]["resources"],
        serde_json::json!([])
    );
    assert_eq!(
        scenarios["default/restart/deep"]["phases"]["destroy"]["status"],
        "pass"
    );
    fs::remove_dir_all(directory).expect("remove only test directory");
}

#[test]
fn keep_persists_entered_scenarios_without_destruction() {
    let directory = test_directory("keep");
    let configuration = fixture_config(&directory);
    let state = directory.join("kept-state.json");
    let output = run(&[
        "run",
        "default/restart",
        "--file",
        configuration.to_str().unwrap(),
        "--state",
        state.to_str().unwrap(),
        "--keep",
    ]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let state = load_state(&state);
    assert_eq!(state["keep"], true);
    for scenario in state["scenarios"].as_object().unwrap().values() {
        assert_eq!(scenario["phases"]["destroy"]["status"], "skipped");
    }
    fs::remove_dir_all(directory).expect("remove only test directory");
}
