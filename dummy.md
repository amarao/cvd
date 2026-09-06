# Dummy Stub Implementation Plan

## Goal

The dummy executable validates configuration, lifecycle ordering, selection,
state persistence, and reporting without invoking external tools.

- The dummy provisioner creates one fixed mock resource for every enabled
  `create` phase and only pretends to destroy it.
- The dummy converger pretends that every enabled converger phase succeeds.
- The dummy verifier returns `pass` for every named test.

The lifecycle core records the fixed resource as destroyed after `destroy`
succeeds. It has ID `mock`, type `mock`, and no attributes. Creation and
destruction provenance come from the scenario path and phase. `--keep` skips
destroy and leaves the resource existing.

## Configuration

The stub accepts version 1 YAML. Top-level `provisioner`, `converger`, and
`verifier` values must be `dummy`. Phase values are persisted as opaque input
but do not alter dummy behavior. A phase is enabled only when its key is
present; omitted phases are recorded as skipped.

Scenarios may contain named tests, recursively nested scenarios, and relative
scenario-fragment includes. These features remain because they exercise the
core lifecycle and state model.

```yaml
version: 1
provisioner: dummy
converger: dummy
verifier: dummy
scenarios:
  default:
    create:
    prepare:
    converge:
    verify:
    cleanup:
    destroy:
    tests:
      smoke: {}
```

No real provisioner, converger, verifier, inventory generation, playbook
resolution, or external command execution is part of this stub.

## CLI

```text
cvd run [SCENARIO] [--file cvd.yml] [--state-dir DIR] [--keep]
cvd state-view [VIEW] [--run RUN] [--file cvd.yml] [--state-dir DIR]
cvd state-resources [--deleted] [--run RUN] [--file cvd.yml] [--state-dir DIR]
cvd state-report [--run RUN] [--file cvd.yml] [--state-dir DIR]
```

Output and persisted-state semantics are defined in `REQUIREMENTS.md` and
`state.md`.
