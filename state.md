# State File Notes

## Purpose

The state file records enough information to inspect a run, recover cleanup,
and later support interactive phase repetition. It is CVD-owned data, not a
user-edited configuration file.

## Current storage layout

Default to:

```text
<project-root>/.cvd/runs/<run-id>/state.json
```

`project-root` is the directory containing the selected CVD configuration file.

`<project-root>/.cvd/last-run` atomically stores the most recently started run
ID followed by a newline. `cvd state-view`, `cvd state-resources`, and
`cvd state-report` resolve `last` through that pointer;
explicit run IDs are restricted to one safe path component.

Also support `--state-dir DIR`. Projects using the default should ignore
`.cvd/` in version control.

Each invocation creates a new run file without overwriting history. Persist
initial state before updating `last-run`, so interrupted runs remain inspectable.
The final default location and stable project identity remain deferred.

## Minimal persisted data

Use versioned JSON containing:

- state schema version;
- run identifier;
- canonical configuration path and a configuration fingerprint;
- requested scenario selector;
- whether keep mode is active;
- scenario paths and parent paths;
- current and completed phase status for each entered scenario;
- test results;
- resource manifests, including existence, attributes, and create/destroy
  scenario-phase locations;
- primary and cleanup errors; and
- run start and last-update timestamps.

Internal execution status is `pending` or `running`. A completed phase result is
`skipped`, `pass`, or `error`. Verifier results can additionally contain
`fail`.

## Write and recovery rules

- Write state before starting a phase with status `running`.
- Write again immediately after the phase result is known.
- Write a temporary file in the state directory, flush it, and atomically rename
  it over the state file.
- Create parent directories when needed.
- Reject unsupported schema versions rather than guessing.
- Treat a loaded `running` phase as interrupted; report it without silently
  retrying it.
- `state-report` reads only persisted state and replays the live scenario
  report layout. Roots and siblings use deterministic lexical path order;
  declaration order is not persisted.
- Never discard the primary error when destruction also fails.

## Ansible inventory views

Scenario state optionally contains a `views` mapping. `ansible_inventory` is a
CVD-owned resource of type `view.ansible.inventory`; its `attributes.path`
locates the generated YAML file under the run's `views/<encoded-selector>/`
directory. View resources are separate from provisioner resource manifests,
so adapters never destroy them and resource change counts exclude them.

The directory is private (0700), and the overlay is written atomically with
0600 permissions. Its host data is derived from persisted live ancestor and
scenario resources before each Ansible converger invocation or Ansible/pytest
test. Save the view path before launching the adapter. It remains available
after destruction as a record of the last rendered inventory, not a live
resource-existence query. Within schema version 3, records without the optional
field load with no views. Original inventory sources and variable files are
not copied.

## Deferred decisions

- Stable project identity and the final default storage directory.
- Cross-process locking and concurrent invocation behavior.
- Garbage collection of completed state and generated views.
- Compatibility and migration policy for state schema changes.
