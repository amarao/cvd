# Ansible provisioner with a Docker host

This example uses an Ansible create playbook to start one Alpine container and
publish it to CVD as a `docker.container` resource. The resource attributes are
suitable for treating the container as an Ansible host in a future inventory
view. The destroy playbook receives that exact persisted resource and removes
it by container ID.

Requirements: `ansible-playbook`, Docker, and permission to use the Docker
daemon. The Alpine image may be pulled on the first run.

```sh
cargo run -- run --file examples/ansible-docker/cvd.yml
```

Create does not derive resources from the requested variables. The playbook
reports what it actually created by atomically writing the aggregate JSON
manifest to `CVD_RESULT_FILE`. Destroy receives the owned resources under
`cvd.resources` through an `--extra-vars` JSON file.
