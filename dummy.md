# Dummy Stub Implementation Plan

## Goal

Build the smallest usable CVD executable that can:

- load a scenario file;
- parse recursively nested scenarios;
- select and run a scenario tree;
- invoke `dummy` provisioner, converger, and verifier adapters; and
- persist lifecycle progress in run-specific state files.

The stub validates the lifecycle model. It runs configured Ansible playbooks
but does not create real infrastructure resources.

## Provisional scenario format

Use YAML for the stub. Keep the schema versioned and intentionally small:

```yaml
version: 1
provisioner: dummy
converger: dummy
verifier: dummy

scenarios:
  default:
    create:
    prepare:
      - ansible: prepare.yml
    converge:
    verify:
    cleanup:
    destroy:
    tests:
      smoke: {}
    nested:
      - name: restart
        create:
          terraform: tf/restart
        converge:
        verify:
        destroy:
        tests:
          after-restart: {}
```

Rules:

- Scenario names are unique among siblings and form stable slash-separated
  paths such as `default/restart`.
- Top-level `provisioner`, `converger`, and `verifier` values are defaults.
  Phase adapter mappings and test verifier fields can override them.
- A phase is enabled by the presence of its key. Its value may be null, a
  scalar, an adapter mapping, or an ordered list of adapter mappings. Omitted
  phases are recorded as `skipped`. An adapter mapping has exactly one adapter
  name and its opaque input.
- Children use the ordered `nested` list. Each has a `name` and either an inline
  body or an `include` path to a scenario fragment relative to the containing
  file.
- `terraform`, `pytest`, and `exec` action names remain dummy no-ops. The
  Ansible converger resolves and runs playbooks as described below. The verifier
  returns `pass`.
- A scenario can omit tests and children.
- Unknown fields are rejected so configuration mistakes are visible.
- Unknown implementation names are rejected. The recognized structural names
  are the dummy aliases listed above.

This schema is provisional. Do not add general plugin configuration until the
provisioner protocol is designed.

## CLI

Implement these commands:

```text
cvd run [SCENARIO] [--file cvd.yml] [--state-dir DIR] [--keep]
cvd state-view [VIEW] [--run RUN] [--file cvd.yml] [--state-dir DIR]
cvd state-resources [--deleted] [--run RUN] [--file cvd.yml] [--state-dir DIR]
cvd state-report [--run RUN] [--file cvd.yml] [--state-dir DIR]
```

Behavior:

- `SCENARIO` is an optional stable scenario path. Without it, run every
  top-level scenario in declaration order.
- Selecting a nested scenario runs the required ancestor chain, then the
  selected scenario and its descendants. Unrelated siblings are skipped.
- `--file` defaults to `cvd.yml` in the current directory.
- `--state-dir` overrides the default state location described in `state.md`.
- `--keep` records the run as retained and skips `destroy`.
- `state-view` renders the complete state for `RUN`; `RUN` defaults to `last`.
  `VIEW` is `yaml` by default or `json` when requested. It uses `--file` only
  to locate the default state directory and does not parse current configuration.
- `state-resources` lists existing resources for `RUN`. With `--deleted`, it
  also lists destroyed resources. Each item includes creation/destruction
  locations, existence, and attributes.
- `state-report` replays persisted scenario entrances, phase statuses, closing
  verdicts, and the run summary for `RUN`; it defaults to `last`. It reads
  state only, using `--file` only to locate the default state directory.
- Configuration, selector, duplicate-name, state I/O, and unsupported-provider
  errors produce a non-zero exit status with a concise message.

Print one bold `Scenario: <path>` entrance header per scenario, indent its phase
results, and end it with a bold `Scenario: <path>: <verdict>` line whose verdict
is `passed`, `error`, or `skipped`. End the run with a summary and return zero
only when it has no `error` or verifier `fail`. Disable terminal styling for
redirected output or when `NO_COLOR` is set. In styled output, render passed
phase and verdict lines in green, errors in red, and skipped lines in gray.
`state-report` uses the same formatting and styling, including any persisted
`pending` or `running` phase states.

## Stub lifecycle

For each entered scenario, process configured phases in canonical order:

```text
dependency -> create -> prepare -> converge -> idempotence
           -> verify(tests) -> children -> cleanup -> destroy
```

Enabled phases invoke the corresponding dummy adapter and report `pass`;
omitted phases report `skipped`. Run child scenarios in declaration order.
Destroy entered scenarios in reverse nesting order.

The dummy provisioner:

- accepts no configuration;
- creates one resource named `dummy`, of type `dummy`, with `ipv6: "::1"`; and
- marks that resource destroyed during `destroy`.

The dummy verifier:

- accepts no configuration;
- performs no tests; and
- reports `pass`.

The dummy converger ignores opaque action payloads. For `prepare`, `converge`,
`idempotence`, and `cleanup`, the Ansible converger accepts null, one playbook
string, or a list of playbook strings. Null selects the single existing
`<phase>.yaml` or `<phase>.yml`; both or neither is a configuration error.
Explicit playbooks must also exist when configuration is loaded. Included
scenario paths are relative to the included fragment. Execution passes the
resolved path to `ansible-playbook` with its working directory set to the root
configuration directory. A launch failure or non-zero exit is a phase `error`.

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
  converger.rs     converger trait and DummyConverger
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
2. Validate names, known implementation aliases, phase declarations, includes,
   and nested scenario paths.
3. Add CLI parsing and scenario selection.
4. Define dummy provisioner, converger, and verifier interfaces.
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
- Every run receives a new ID and remains inspectable through `state-view`;
  `last` resolves to the most recently started run.
- Every entered scenario records a dummy resource and its create/destroy
  provenance.
- Every dummy test reports `pass`.
- `--keep` leaves entered scenarios undestroyed in persisted state.
- Invalid selectors, unknown fields, invalid includes, and unsupported
  implementation names fail clearly.
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
