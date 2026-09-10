Work in progress.

I want to have something better than Molecule and more flexible than Vagrant.

Also, I want to handle VMs and kubernetes resources on equal ground.

I know what I want, but llm is writing, under tight supervision and
good integration test coverage.

If it will work out, I will announce, if not, I will archive.

See [Writing an Ansible provisioner](docs/provisioner.md) for the playbook
contract, supplied variables, and create/destroy protocol.

Use `cvd run -F path/to/project` to discover `cvd.yaml` or `cvd.yml` in the
project directory, or `-f path/to/config.yml` for an explicit file. When both
names exist, `cvd.yaml` takes precedence. `-F` also works with state commands.

Use `cvd syntax-check -F path/to/project` (or `--file path/to/config.yml`) to
validate configuration syntax, schema, and referenced files without executing
the scenario.
