# Dummy scenario

This example exercises one complete lifecycle without making external calls.
The dummy provisioner creates one fixed resource named `mock`; all other
enabled phases and the `smoke` test only pretend to run and report success.
After the pretend destroy, CVD records the resource as destroyed. Pass `--keep`
to leave it recorded as existing.

Dummy phase calls accept `status: ok` or `status: error`; dummy tests also
accept `status: fail`. Every phase is written explicitly in this example.
Change a status to exercise error handling, cleanup, and reporting; an `error`
or verifier `fail` makes the run exit nonzero.

With the top-level dummy defaults, an empty phase such as `prepare:` is
shorthand for a successful dummy call. Omitting the phase key skips it instead.

```sh
cargo run -- run --file examples/dummy/cvd.yml
cargo run -- state-view --file examples/dummy/cvd.yml
cargo run -- state-resources --deleted --file examples/dummy/cvd.yml
cargo run -- state-report --file examples/dummy/cvd.yml
```

Use `--state-dir DIR` to keep state outside the example's `.cvd` directory.
