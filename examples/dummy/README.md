# Dummy scenario example

This is a complete CVD configuration demonstrating phase keys, opaque action
payloads, ordered nested scenarios, and a relative scenario include. A phase is
enabled when its key is present; omitted phases are recorded as skipped.

The Ansible converger validates its playbooks when loading this configuration,
then runs `ansible-playbook` with each resolved path. Commands run from this
directory, which contains the root `cvd.yml`. A launch failure or non-zero exit
is reported as a phase error. The `terraform`, `pytest`, and `exec` actions
remain dummy no-ops; the dummy provisioner records a loopback resource and
verification reports `pass`.

Null Ansible phases select the matching `<phase>.yaml` or `<phase>.yml` in the
same directory as the scenario file. Explicit strings select named playbooks,
and lists run playbooks in order. The example includes minimal debug-task
playbooks for every enabled Ansible phase.

From the repository root, run every root scenario:

```sh
cargo run -- run --file examples/dummy/cvd.yml
```

Run output groups flush-left phases after a bold `Scenario: <path>` entrance and
closes each scenario with a bold `Scenario: <path>: <verdict>` line. On an
interactive terminal, passed lines are green, errors are red, and skipped lines
are gray. Redirected output and output produced with `NO_COLOR` set are plain
text.

Run just the nested `configuration-check` scenario. CVD enters only the
required `default/install` ancestor chain and the selected scenario:

```sh
cargo run -- run default/install/configuration-check --file examples/dummy/cvd.yml
```

Keep entered scenarios for inspection (the dummy destroy phase is skipped):

```sh
cargo run -- run default --file examples/dummy/cvd.yml --keep
```

By default CVD writes a state history beside this configuration under
`examples/dummy/.cvd/runs/<run-id>/state.json`. The run summary prints the ID.
Inspect the latest run as YAML (the default view):

```sh
cargo run -- state-view --file examples/dummy/cvd.yml
```

Render a specific run as JSON:

```sh
cargo run -- state-view json --run run-123 --file examples/dummy/cvd.yml
```

List resources that still exist in the latest run, or include destroyed ones:

```sh
cargo run -- state-resources --file examples/dummy/cvd.yml
cargo run -- state-resources --deleted --file examples/dummy/cvd.yml
```

Replay the latest persisted lifecycle report, or select a specific run:

```sh
cargo run -- state-report --file examples/dummy/cvd.yml
cargo run -- state-report --run run-123 --file examples/dummy/cvd.yml
```

`state-report` reads the stored run only; it does not parse or execute the
current scenario configuration. Its interactive output uses the same bold
scenario entrances and verdicts, including verdict colors, as `run`.

Each entered scenario creates a `dummy` resource with type `dummy` and
`ipv6: "::1"`. A normal run destroys it; `--keep` leaves it existing.

To keep the example directory clean, provide a temporary state directory:

```sh
state_dir="$(mktemp -d)"
cargo run -- run --file examples/dummy/cvd.yml --state-dir "$state_dir"
cargo run -- state-view json --state-dir "$state_dir"
```
