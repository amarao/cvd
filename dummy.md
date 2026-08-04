# Dummy Stub Implementation Plan

## Goal

Build the smallest usable CVD executable that can:

- load a scenario file;
- parse recursively nested scenarios;
- select and run a scenario tree;
- invoke a no-op `dummy` provisioner and verifier; and
- persist lifecycle progress in a state file.

The stub validates the lifecycle model. It does not run Ansible or create real
resources.

## Provisional scenario format

Use YAML for the stub. Keep the schema versioned and intentionally small:

```yaml
version: 1
scenarios:
  default:
    provisioner: dummy
    suites:
      smoke:
        verifier: dummy
    scenarios:
      restart:
        provisioner: dummy
        suites:
          after-restart:
            verifier: dummy
```

Rules:

- Scenario names are unique among siblings and form stable slash-separated
  paths such as `default/restart`.
- `provisioner: dummy` accepts no options. Any supplied options are rejected.
- `verifier: dummy` accepts no options, performs no tests, and returns `pass`.
- A scenario can omit suites and children.
- Unknown fields are rejected so configuration mistakes are visible.
- The stub supports only `dummy`; any other implementation name is an error.

This schema is provisional. Do not add general plugin configuration until the
provisioner protocol is designed.

## CLI

Implement one command first:

```text
cvd run [SCENARIO] [--file cvd.yml] [--state PATH] [--keep]
```

Behavior:

- `SCENARIO` is an optional stable scenario path. Without it, run every
  top-level scenario in declaration order.
- Selecting a nested scenario runs the required ancestor chain, then the
  selected scenario and its descendants. Unrelated siblings are skipped.
- `--file` defaults to `cvd.yml` in the current directory.
- `--state` overrides the default state location described in `state.md`.
- `--keep` records the run as retained and skips `destroy`.
- Configuration, selector, duplicate-name, state I/O, and unsupported-provider
  errors produce a non-zero exit status with a concise message.

Print one line per phase with the scenario path, phase, and result. End with a
summary and return zero only when the run has no `error` or verifier `fail`.

## Stub lifecycle

For each entered scenario, execute:

```text
create(dummy) -> verify(dummy suites) -> children -> destroy(dummy)
```

Record the other lifecycle phases as `skipped`; do not invent implementations
for them. Run child scenarios in declaration order. Destroy entered scenarios
in reverse nesting order.

The dummy provisioner:

- accepts no configuration;
- returns an empty resource manifest from `create`; and
- succeeds without action on `destroy`.

The dummy verifier:

- accepts no configuration;
- performs no tests; and
- reports `pass`.

On an execution error, retain the primary error, attempt applicable destruction
unless `--keep` is active, persist the final state, and exit non-zero.

## Code structure

Keep interfaces narrow, but separate concerns so real implementations can
replace the dummy later:

```text
src/
  main.rs          CLI entry point and exit status
  cli.rs           command definitions
  config.rs        YAML schema, validation, scenario lookup
  lifecycle.rs     recursive execution and ordering
  provisioner.rs   provisioner trait and DummyProvisioner
  verifier.rs      verifier trait and DummyVerifier
  state.rs         persisted model and atomic storage
```

Suggested dependencies:

- `clap` for CLI parsing;
- `serde` and `serde_yaml` for scenario configuration;
- `serde_json` for state; and
- `thiserror` for typed errors.

Do not add async execution, dynamic loading, or subprocess protocols to this
stub.

## Implementation sequence

1. Define and parse the versioned YAML model with strict unknown-field checks.
2. Validate names, implementations, empty dummy options, and nested scenario
   paths.
3. Add CLI parsing and scenario selection.
4. Define dummy provisioner and verifier interfaces.
5. Implement the recursive lifecycle walker and reverse-order destruction.
6. Implement state creation, transition writes, loading, and atomic replacement
   as described in `state.md`.
7. Connect reporting and exit codes.
8. Add an end-to-end fixture with at least two roots and two levels of nesting.

## Acceptance checks

- `cvd run` executes all roots and recursively visits children.
- `cvd run default/restart` runs only its ancestor chain and selected subtree.
- Scenario paths remain stable across parsing and state serialization.
- Each lifecycle transition is persisted before the next action starts.
- Dummy provisioner state contains an empty resource manifest.
- Every dummy suite reports `pass`.
- `--keep` leaves entered scenarios undestroyed in persisted state.
- Invalid selectors, unknown fields, dummy options, and unsupported
  implementations fail clearly.
- Destruction order is child before parent.
- Unit and integration tests pass with `cargo test`.

## Not included

- Real provisioning or verification.
- Ansible integration or generated views.
- Phase repetition and forced transitions.
- Cleanup-from-current-phase commands.
- Concurrent scenario execution.
- External plugins or process protocols.
- JUnit or CI-specific reports.

