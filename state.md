# State File Notes

## Purpose

The state file records enough information to inspect a run, recover cleanup,
and later support interactive phase repetition. It is CVD-owned data, not a
user-edited configuration file.

## Recommended location

For the dummy stub, default to:

```text
<project-root>/.cvd/state.json
```

For the stub, `project-root` is the directory containing the selected CVD
configuration file.

Also support `--state PATH`. Projects using the default should ignore `.cvd/`
in version control.

The project-local default is easy to discover and naturally associates state
with the tested project. Before supporting simultaneous or retained runs, move
to per-run files, for example `.cvd/runs/<run-id>/state.json`, with a small
pointer to the active run.

An OS user-state directory such as
`$XDG_STATE_HOME/cvd/<project-id>/runs/<run-id>/state.json` avoids writing into
the source tree and should be evaluated before the state format becomes stable.
It requires a defined, stable project identity and discovery command, so it is
not required for the dummy stub.

## Minimal persisted data

Use versioned JSON containing:

- state schema version;
- run identifier;
- canonical configuration path and a configuration fingerprint;
- requested scenario selector;
- whether keep mode is active;
- scenario paths and parent paths;
- current and completed phase status for each entered scenario;
- suite results;
- resource manifests, empty for the dummy provisioner;
- primary and cleanup errors; and
- run start and last-update timestamps.

Internal execution status is `pending` or `running`. A completed phase result is
`skipped`, `pass`, or `error`. Verifier results can additionally contain
`fail`.

## Write and recovery rules

- Write state before starting an external phase with status `running`.
- Write again immediately after the phase result is known.
- Write a temporary file in the state directory, flush it, and atomically rename
  it over the state file.
- Create parent directories when needed.
- Reject unsupported schema versions rather than guessing.
- Treat a loaded `running` phase as interrupted; report it without silently
  retrying it.
- Never discard the primary error when destruction also fails.
- Redact sensitive resource attributes before serialization. The dummy has no
  sensitive attributes, but the state shape must allow redaction later.

## Deferred decisions

- Stable project identity and the final default storage directory.
- Multiple retained runs and active-run selection.
- Cross-process locking and concurrent invocation behavior.
- Secret encryption versus omission or external secret references.
- Garbage collection of completed state and generated views.
- Compatibility and migration policy for state schema changes.
