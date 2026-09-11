# Getting started

With a Rust toolchain that supports edition 2024 installed, clone the CVD
repository and run `cargo run --locked -- run -F examples/dummy` from its root
to try a complete lifecycle without creating external infrastructure. The
example's `cvd.yml` declares its phases and named tests, and the run records
state under `examples/dummy/.cvd/`; inspect the result with
`cargo run --locked -- state-report -F examples/dummy`. Use this example as a
starting point before configuring Ansible playbooks and real resources.
