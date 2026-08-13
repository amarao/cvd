# Minimal provisioner scenario

This example contains one scenario, `provisioned-resource`, with only the
`create`, `converge`, `verify`, and `destroy` phases enabled. The Ansible
provisioner passes the `create` mapping (apart from its `ansible` options) as an
inventory to `create.yaml`. The configured `resources.hosts` entries are
exposed under the configured `mygroup2` inventory group and become CVD
resources. Facts published by successful `set_fact` tasks are merged into their
attributes, so both `vm1` and `vm2` acquire `public_ip: 127.0.0.55`.

The Ansible converger runs the local `converge.yml` playbook, and the Ansible
provisioner runs `destroy.yml` with the same resource inventory. The dummy
verifier reports a passing verification, and CVD records both resources as
destroyed after that phase succeeds.

Run it from the repository root (requires `ansible-playbook` on `PATH`):

```sh
cargo run -- run --file examples/minimal-provisioner/cvd.yml
```

Use a temporary state directory to inspect the recorded resources:

```sh
state_dir="$(mktemp -d)"
cargo run -- run --file examples/minimal-provisioner/cvd.yml --state-dir "$state_dir"
cargo run -- state-resources --deleted --state-dir "$state_dir"
cargo run -- state-view json --state-dir "$state_dir"
```

The normal run destroys the resource. Add `--keep` to retain it for inspection.
