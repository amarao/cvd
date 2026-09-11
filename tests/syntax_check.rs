use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn test_directory(label: &str) -> PathBuf {
    let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!(
        "cvd-syntax-check-{label}-{}-{sequence}",
        std::process::id()
    ))
}

fn syntax_check(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cvd"))
        .args(["syntax-check"])
        .args(arguments)
        .output()
        .expect("run cvd syntax-check")
}

fn write_file(path: &Path, contents: &[u8]) {
    fs::write(path, contents).expect("write test configuration");
}

fn assert_valid(output: std::process::Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("is valid"));
}

fn assert_invalid(output: std::process::Output, expected_error: &str) {
    assert!(
        !output.status.success(),
        "syntax-check unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(expected_error),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_empty_document() {
    let directory = test_directory("empty");
    fs::create_dir_all(&directory).unwrap();
    let configuration = directory.join("cvd.yml");
    write_file(&configuration, b"");

    assert_invalid(
        syntax_check(&["--file", configuration.to_str().unwrap()]),
        "expected a mapping at the document root",
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rejects_a_sequence_instead_of_a_configuration_mapping() {
    let directory = test_directory("sequence");
    fs::create_dir_all(&directory).unwrap();
    let configuration = directory.join("cvd.yml");
    write_file(&configuration, b"- an Ansible playbook\n");

    assert_invalid(
        syntax_check(&["--file", configuration.to_str().unwrap()]),
        "expected a mapping at the document root, but found a sequence",
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rejects_non_yaml_text() {
    let directory = test_directory("text");
    fs::create_dir_all(&directory).unwrap();
    let configuration = directory.join("cvd.yml");
    write_file(&configuration, b"version: [this is not YAML\n");

    assert_invalid(
        syntax_check(&["--file", configuration.to_str().unwrap()]),
        "invalid configuration",
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rejects_non_yaml_binary_input() {
    let directory = test_directory("binary");
    fs::create_dir_all(&directory).unwrap();
    let configuration = directory.join("cvd.yml");
    write_file(&configuration, &[0xff, 0xfe, 0xfd]);

    assert_invalid(
        syntax_check(&["--file", configuration.to_str().unwrap()]),
        "cannot read configuration",
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rejects_a_configuration_with_a_missing_referenced_file() {
    let directory = test_directory("missing-reference");
    fs::create_dir_all(&directory).unwrap();
    let configuration = directory.join("cvd.yml");
    write_file(
        &configuration,
        b"version: 1\nconverger: dummy\nverifier: dummy\nscenarios:\n  default:\n    create:\n      ansible:\n        playbook: missing.yml\n",
    );

    assert_invalid(
        syntax_check(&["--file", configuration.to_str().unwrap()]),
        "playbook",
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn accepts_a_minimal_configuration() {
    let directory = test_directory("minimal");
    fs::create_dir_all(&directory).unwrap();
    let configuration = directory.join("cvd.yml");
    write_file(
        &configuration,
        b"version: 1\nconverger: dummy\nverifier: dummy\nscenarios: {}\n",
    );

    assert_valid(syntax_check(&["--file", configuration.to_str().unwrap()]));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn accepts_a_complicated_scenario() {
    let directory = test_directory("complicated");
    fs::create_dir_all(&directory).unwrap();
    let configuration = directory.join("cvd.yml");
    write_file(
        &configuration,
        b"version: 1\nprovisioner: dummy\nconverger: dummy\nverifier: dummy\nscenarios:\n  complicated:\n    create:\n      dummy: {}\n    prepare: prepare input\n    converge:\n      - first convergence input\n      - second convergence input\n    idempotence:\n    verify:\n      smoke: {}\n      expected-failure:\n        status: fail\n    nested:\n      - name: restart\n        create:\n        converge:\n          dummy:\n            status: ok\n        verify:\n          after-restart: {}\n        cleanup:\n        destroy:\n      - name: deeper\n        nested:\n          - name: final\n            verify:\n              final-check: {}\n    cleanup:\n      dummy: {}\n    destroy:\n",
    );

    assert_valid(syntax_check(&["--file", configuration.to_str().unwrap()]));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn directory_option_discovers_cvd_yaml_and_cvd_yml() {
    for filename in ["cvd.yaml", "cvd.yml"] {
        let directory = test_directory(filename);
        fs::create_dir_all(&directory).unwrap();
        write_file(
            &directory.join(filename),
            b"version: 1\nconverger: dummy\nverifier: dummy\nscenarios: {}\n",
        );

        assert_valid(syntax_check(&["-F", directory.to_str().unwrap()]));

        fs::remove_dir_all(directory).unwrap();
    }
}
