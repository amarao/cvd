# Dummy scenario

This example exercises one complete lifecycle without making external calls.
The dummy provisioner creates one fixed resource named `mock`; all other
enabled phases and the `smoke` test only pretend to run and report success.
After the pretend destroy, CVD records the resource as destroyed. Pass `--keep`
to leave it recorded as existing.

Dummy phase calls accept `status: ok` or `status: error`; dummy tests also
accept `status: fail`. The full explicit form is shown in `../dummy-full`.

```sh
cargo run -- run --file examples/dummy/cvd.yml
cargo run -- state-view --file examples/dummy/cvd.yml
cargo run -- state-resources --deleted --file examples/dummy/cvd.yml
cargo run -- state-report --file examples/dummy/cvd.yml
```

Use `--state-dir DIR` to keep state outside the example's `.cvd` directory.
