# Docker with native Ansible inventory

`inventory.yml` declares hosts, group memberships, and group variables using
ordinary Ansible syntax. Add hosts to `cvd_managed` to create more containers;
change `container_image` per host or group. The create playbook is reusable and
knows nothing about application groups such as `webservers`.

Requirements: `ansible-playbook`, `ansible-inventory`, Docker access, and the
`community.docker` collection (for its Docker connection plugin), and
`pytest` with `pytest-testinfra` in the active Python environment.

```sh
cargo run -- run --file examples/ansible-docker/cvd.yml
```

Create first pulls each distinct `container_image` into the local Docker cache
and records its actual image ID as a `docker.image` resource. Images have no
Ansible host binding, so they are passed to destroy in `cvd.resources`. Hosts
sharing an image share one image resource. This example takes ownership of the
cached image even if it was already present, and purges it during destroy.
Image removal does not use force: Docker can refuse removal when another
container or additional tags still reference it, making destroy fail.

Create then reports each actual container ID with an
`attributes.ansible` binding to its inventory hostname. CVD persists those
resources and generates a private runtime inventory overlay before converge.
Ansible loads the original inventory first and the overlay last, retaining
normal group variables and connection-variable precedence. Converge targets
`webservers` through the discovered Docker connection, using `raw` because the
Alpine image has no Python. The named `greeting` test runs pytest/testinfra
against `webservers` and checks the file content. The test module selects the
group with `testinfra_hosts = ["ansible://webservers"]`; pytest arguments
only control reporting. CVD sets `ANSIBLE_INVENTORY`
to the original sources followed by the runtime overlay; testinfra's Ansible
backend reads it without an inventory CLI argument. Destroy removes only
persisted owned container IDs, then purges the recorded image IDs.

Each named entry under `verify` supplies `pytest.path` (relative to its scenario file) and optional
`pytest.args` passed literally to `pytest`. For now every nonzero pytest exit
is an `error`, including failed assertions, collection errors, and no tests.
Cleanup and destruction still run.

Hosts outside `cvd_managed` can be existing machines with ordinary connection
variables. They are not created or destroyed by these playbooks; any play
that targets their groups can still modify them.

`inventory: [inventory.yml]` explicitly selects inventory sources. Omit it to
use the project's `ansible.cfg` or `ANSIBLE_INVENTORY`; CVD preserves those
sources when adding its overlay. Original source paths remain intact, so
Ansible can load adjacent `group_vars` and `host_vars` files.

Use `--keep` to retain containers and cached images. The persisted scenario's
`views.ansible_inventory.attributes.path` points to the generated overlay.
The overlay is a record of that invocation; after destroy it is not a list of
currently existing containers. Resource state records destruction separately.

The aggregate manifest is published only after all create tasks succeed.
Containers and images created before a failed or interrupted publication cannot yet be
recovered automatically; incremental provisioning checkpoints remain deferred.

Create writes its manifest to `cvd.result_file`. CVD also supplies
`cvd.input_file` and `cvd.invocation_id`; the corresponding `CVD_*` environment
variables remain available for scripts.

Destroy targets `cvd_managed` directly. CVD rebuilds this group from the
scenario's owned host resources and exposes each as `cvd_resource`; the Docker
playbook removes `cvd_resource.id` using a local connection. A second controller
play loops over `cvd.resources` to purge the cached images after the containers
have been removed.
The original inventory is not loaded during destroy.
