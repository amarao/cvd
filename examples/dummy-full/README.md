# Full dummy-call example

This scenario spells out every lifecycle phase with explicit dummy options.
It creates one mock resource and otherwise makes no external calls.

```sh
cargo run -- run --file examples/dummy-full/cvd.yml
```

Every phase accepts `status: ok` or `status: error`. Dummy tests additionally
accept `status: fail`. Change one value to exercise lifecycle error, cleanup,
and reporting behavior. An `error` run or a verifier `fail` exits non-zero.
