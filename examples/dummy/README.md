# Dummy scenario example

This is a complete, no-op CVD configuration. `dummy` accepts no options,
creates no resources, and its verifier runs no tests and reports `pass`.

From the repository root, run every root scenario:

```sh
cargo run -- run --file examples/dummy/cvd.yml
```

Run just the nested `configuration-check` scenario. CVD enters only the
required `default/restart` ancestor chain and the selected scenario:

```sh
cargo run -- run default/restart/configuration-check --file examples/dummy/cvd.yml
```

Keep entered scenarios for inspection (the dummy destroy phase is skipped):

```sh
cargo run -- run default --file examples/dummy/cvd.yml --keep
```

By default CVD writes state beside this configuration at
`examples/dummy/.cvd/state.json`. Inspect it with:

```sh
jq . examples/dummy/.cvd/state.json
```

To keep the example directory clean, provide a temporary state path:

```sh
state_dir="$(mktemp -d)"
cargo run -- run --file examples/dummy/cvd.yml --state "$state_dir/state.json"
jq . "$state_dir/state.json"
```
