# Docker with native Ansible inventory

`inventory.yml` declares hosts, group memberships, and group variables using
ordinary Ansible syntax. Add hosts to `cvd_managed` to create more containers;
change the base image in `Dockerfile`. The create playbook is reusable and
knows nothing about application groups such as `webservers`.

Requirements: `ansible-playbook`, `ansible-inventory`, Docker access, and the
`community.docker` collection (for its Docker connection plugin), and
`pytest` with `pytest-testinfra` in the active Python environment.

```sh
cargo run -- run --file examples/ansible-docker/cvd.yml
```

Create first builds the local `Dockerfile` (`FROM alpine:3.20`) and tags the
result `cvd-example:<invocation_id>`. A build-time invocation label gives it a
separate image ID even though the Dockerfile only contains `FROM`. This keeps
the base image and other runs' images outside this scenario's ownership.
All managed containers share the built image.

The built image's actual ID is recorded as a `docker.image` resource with its
tag in `attributes.reference`. It has no Ansible host binding, so destroy
receives it in `cvd.resources` and `cvd.resources_by_type['docker.image']`.
Destroy removes the containers first, then removes the built image by ID
without force. The base image and Docker build cache remain available.

Create then reports each actual container ID with an
`attributes.ansible` binding to its inventory hostname. CVD persists those
resources and generates a private runtime inventory overlay before converge.
Ansible loads the original inventory first and the overlay last, retaining
normal group variables and connection-variable precedence. Prepare checks that
each web container is reachable and `/tmp` is writable, then removes any old
greeting file. Converge targets
`webservers` through the discovered Docker connection, using `raw` because the
Alpine image has no Python. The named `greeting_ansible` test overrides the
default verifier with `ansible` and runs `verify.yml` against the same inventory.
It reads `/tmp/cvd-greeting` with `raw` and asserts that its content matches the
host's `greeting` variable. The named `greeting` test runs pytest/testinfra
against `webservers` and checks the file content. The test module selects the
group with `testinfra_hosts = ["ansible://webservers"]`; pytest arguments
only control reporting. CVD sets `ANSIBLE_INVENTORY`
to the original sources followed by the runtime overlay; testinfra's Ansible
backend reads it without an inventory CLI argument. Destroy removes only
persisted owned container IDs, then purges the recorded image IDs. Before
destroy, cleanup removes `/tmp/cvd-greeting` if it exists, including after
verification errors. Prepare and cleanup use `raw` so they need no Python in
the container; removal reports a change only when the file existed.

The Ansible test supplies `ansible.playbook`; the pytest test supplies
`pytest.path` and optional `pytest.args` passed literally to `pytest`. Both
paths are relative to the scenario file. Every nonzero Ansible or pytest exit
is an `error`, including failed assertions. Later tests stop, and normal
cleanup and destruction still run.

Hosts outside `cvd_managed` can be existing machines with ordinary connection
variables. They are not created or destroyed by these playbooks; any play
that targets their groups can still modify them.

`inventory: [inventory.yml]` explicitly selects inventory sources. Omit it to
use the project's `ansible.cfg` or `ANSIBLE_INVENTORY`; CVD preserves those
sources when adding its overlay. Original source paths remain intact, so
Ansible can load adjacent `group_vars` and `host_vars` files.

Use `--keep` to retain containers and built images. Cleanup still runs, so the
greeting file is removed even when containers are retained. The persisted scenario's
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
play loops over `cvd.resources_by_type.get('docker.image', [])` to purge images after the containers
have been removed.
The original inventory is not loaded during destroy.

Prepare, converge, and cleanup also receive `cvd.resources` and
`cvd.resources_by_type`, including existing resources inherited from parents.
Create builds separate container and image lists and combines them in the
manifest; the manifest format is unchanged.
