Work in progress.

I want to have something better than Molecule and more flexible than Vagrant.

Also, I want to handle VMs and kubernetes resources on equal ground.

I know what I want, but llm will write, under tight supervision and
good test coverage.

If it will work out, I will announce, if not, I will archive.

See [Writing an Ansible provisioner](docs/provisioner.md) for the playbook
contract, supplied variables, and create/destroy protocol.

Use `cvd run -F path/to/project` to select `path/to/project/cvd.yaml`, or
`-f path/to/config.yml` for an explicit file. `-F` also works with state commands.
