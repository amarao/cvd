# Ansible provisioner

## Lifecycle

CVD runs each scenario through its declared phases in this order:

```text
dependency → create → prepare → converge → idempotence → verify
           → child scenarios → cleanup → destroy
```

An Ansible provisioner handles the `create` and `destroy` phases. Create
creates or discovers resources before the scenario's work begins. Destroy
removes or releases those resources after cleanup. Omitted phases are skipped.

Children scenarios can use parent resources but create/destroy only the resources
they add. Their cleanup and destruction finish before the parent's.
If create or later work fails, CVD still attempts configured cleanup and destruction.

## Provisioner contract

CVD calls each playbook with a `cvd` variable containing the scenario identity,
phase-specific input, and resource information (see below). The playbook must fulfill the
contract for its phase:

| Phase | CVD provided | A playbook must |
| --- | --- | --- |
| `create` | Original inventory, creation variables, an empty resource list, and a result-file path. | Create or discover resources, write a complete JSON resource manifest with created resources, and exit successfully. |
| `destroy` | Owned hosts in `cvd_managed`, their `cvd_resource` data, destruction variables, and other resources in `cvd.resources`. | Remove or release those objects and exit successfully. No result file is required. |

CVD records accepted resources and uses connection data from manifest to
prepare inventory for later phases.

Create and destroy are separate playbook invocations: playbook-local variables
do not carry between them.

Destroy must use the recorded resources rather than reconstructing targets
from inventory or creation input.

### Playbook input

Playbook paths are relative to the scenario file. CVD runs playbooks from the
root configuration directory (directory with `cvd.yaml`).
Create playbook uses the original inventory from cvd.yaml. Destroy playbook
uses a dedicated inventory of recorded owned hosts. Ansible configuration
still applies. Each phase may have its own optional `vars` section.

Note: `cvd` variable and `cvd_` prefix are reserved for CVD use, don't set or
change those variables to avoid breakage. Future versions may introduce new variables
with `cvd_` prefix.

CVD supplies `cvd` as an Ansible extra variable into create and destroy playbooks.
During destroy, each managed host also has the inventory variable `cvd_resource`:

| Variable | Meaning |
| --- | --- |
| `cvd.protocol_version` | Protocol version; currently `1`. |
| `cvd.directory` | Always supplied: the absolute directory containing the root CVD configuration file. |
| `cvd.input_file` | Path to the JSON input containing the top-level `cvd` mapping. |
| `cvd.result_file` | Path where create playbook must write its JSON result. |
| `cvd.invocation_id` | Identifier for this invocation; copy it into the create result. |
| `cvd.action` | `create` or `destroy`. |
| `cvd.scenario_selector` | Scenario selector, such as `default/restart`. |
| `cvd.vars` | This phase's configured `vars`, or an empty mapping. |
| `cvd.resources` | Empty during create; owned non-host resources during destroy; all existing visible resources during convergence. |
| `cvd.resources_by_type` | The same resources grouped by their exact `type`; absent types are omitted. |
| `cvd_resource` | During destroy, the complete recorded resource for the current `cvd_managed` host. |

Normal inventory variables, `inventory_hostname`, `groups`, and `hostvars`
remain available. The controller also receives these environment variables:

| Variable | Meaning |
| --- | --- |
| `CVD_DIRECTORY` | Same value as `cvd.directory`; also available to pytest. |
| `CVD_INPUT_FILE` | Same value as `cvd.input_file`. |
| `CVD_RESULT_FILE` | Same value as `cvd.result_file`. |
| `CVD_INVOCATION_ID` | Same value as `cvd.invocation_id`. |

In playbooks, use `cvd.input_file`, `cvd.result_file`, and `cvd.invocation_id`
directly. The environment variables remain available for scripts. These values
are supplied to both provisioner and converger playbooks; file paths are valid
only for the current invocation. `cvd.directory` refers to the project directory
(the directory with `cvd.yaml` file).

A supplied result path does not mean a result file is required during destroy or convergence.

### Resources manifest (create output)

Create playbook need to write one JSON document to `cvd.result_file` (for example,
`dest: "{{ cvd.result_file }}"` in a `copy` task) containing:

| Field | Required value |
| --- | --- |
| `manifest_version` | Manifest version; currently `1`. |
| `invocation_id` | The supplied `cvd.invocation_id`. |
| `complete` | `true`, once all resource results are assembled. |
| `resources` | An ordered list of actual created or discovered resources; may be empty. |

Each resource requires a nonempty `id` and `type`. IDs must be unique within
the result and identify the actual objects that destroy will remove or
release. An optional `attributes` mapping carries additional data. CVD adds
ownership, existence, and creation/destruction metadata.

Publish the aggregate result once, on the controller, using
`ansible.builtin.copy` or `ansible.builtin.template` with atomic writes enabled.
Do not enable `unsafe_writes` or let individual hosts overwrite each other's
results. A failed playbook or missing, malformed, incomplete, or mismatched
result makes create fail. Resources created before a result is successfully
accepted cannot yet be recovered automatically.

### Inventory host bindings

To enrich an existing inventory host, include its inventory name and discovered
connection variables in the resource's attributes:

```yaml
attributes:
  ansible:
    inventory_hostname: "{{ inventory_hostname }}"
    vars:
      ansible_host: "{{ docker_run.stdout }}"
      ansible_connection: community.docker.docker
```

`inventory_hostname` is required; `vars` is optional. Keep group membership and
group variables in the original inventory. A create play may target a group
such as `cvd_managed`; that name is the host-provisioning convention during
create. CVD rebuilds it from owned host resources during destroy.
Non-host resources omit the binding. Unknown inventory names or duplicate
bindings fail when the inventory is consumed.

### Destroy result

Target `cvd_managed` to destroy hosts, using each host's `cvd_resource`:

```yaml
- name: Destroy owned containers
  hosts: cvd_managed
  connection: local
  gather_facts: false
  tasks:
    - name: Remove the container
      ansible.builtin.command:
        argv: [docker, rm, --force, "{{ cvd_resource.id }}"]
      changed_when: true
```

CVD includes only existing hosts owned by this scenario. Parent, external, and
never-created hosts are excluded. The original inventory is not loaded for
destroy: use `cvd_resource.attributes` and `cvd.vars` for provider inputs.
Connections default to local so removal can run even when a target is unreachable.

Handle non-host resources in a separate `hosts: localhost` play. Select the
appropriate list without a type-filtering `when`:

```yaml
- name: Purge cached images
  ansible.builtin.command:
    argv: [docker, image, rm, "{{ item.id }}"]
  loop: "{{ cvd.resources_by_type.get('docker.image', []) }}"
  changed_when: true
```

`cvd.resources` remains available for generic operations. Both the managed group and that list can be empty. Tolerate
objects already gone. If a malformed binding has no usable host name, its
resource appears in `cvd.resources`; duplicate names get unique destroy aliases.
Always use the recorded resource ID for removal.

Return success only when all applicable objects are removed or released. CVD
then records them as destroyed. A failed destroy leaves them recorded as
existing; no result document is required. See the complete
[Docker destroy playbook](../examples/ansible-docker/destroy.yml).

### Using resource lists across phases

During create, both resource views are empty. Supply requested objects through
inventory variables or your own lists in `cvd.vars`, such as `images` and
`networks`. Report actual results in one manifest list; separate lists can be
combined with `resources: "{{ created_containers + cached_images }}"`.

During prepare, converge, and cleanup, both views contain existing resources
from the scenario and its ancestors, including host resources. During destroy,
they contain only owned non-host resources; inherited resources are excluded.
The flat list keeps resource order, and each typed list preserves the relative
order of its entries. Type names containing dots use dictionary access, such
as `cvd.resources_by_type.get('docker.image', [])`.

## Ansible verification

Set `verifier: ansible` at the top level or on a named test:

```yaml
verify:
  service:
    verifier: ansible
    ansible:
      playbook: verify.yml
      vars:
        expected_port: 8080
```

Verification receives the original inventory plus the scenario's runtime
inventory overlay. It also receives the same `cvd` variables as convergence,
with `cvd.action: verify`, test inputs in `cvd.vars`, and all existing visible
resources in `cvd.resources` and `cvd.resources_by_type`. Playbook paths resolve
relative to the file declaring the scenario. No result file is required.
A successful Ansible exit passes the test. Any nonzero exit, including an
assertion failure, is an `error`: later tests and child scenarios stop, and
normal cleanup and destruction are attempted.
