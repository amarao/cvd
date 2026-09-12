# CVD Basic Requirements

## Purpose

CVD (Create-Verify-Destroy) is a test harness for infrastructure as code:

- existing Ansible/IaC projects with their own playbooks, inventory, and
  `ansible.cfg`;
- standalone Ansible roles;
- Ansible collections; and
- nested test scenarios with side effects and multiple tests.

The primary use case is full integration testing against real infrastructure:
physical servers, virtual machines, clusters, network switches, and provider
APIs. Small-scale role testing is supported, but the design must also serve
complete infrastructure projects and their existing environments.

CVD coordinates lifecycle actions. It does not prescribe how infrastructure is
provisioned or introduce its own infrastructure definition language. Preserve
existing project configuration by default.

## Project layout and paths

A selected `cvd.yaml` or `cvd.yml` is the entry point for a test configuration.
Its containing directory is the **project directory**, including when the
infrastructure code lives elsewhere. A configuration describes a test flow
using scenarios; a **run** is one invocation executing the selected scenarios.

Support two idiomatic layouts:

- A configuration at the infrastructure project's root, alongside playbooks,
  modules, tests, and CI configuration, describes one flow of sequential or
  nested scenarios.
- Separate directories, each with its own configuration, inventory, playbooks,
  and tests, describe independent flows such as `simple` and `extended`.
  Relative references such as `../..` allow test code to stay separate from
  production code. Select a flow with `cvd run -F simple` or
  `cvd run -F extended`.

Related checks should normally reuse playbooks within one configuration's
sequential or nested scenarios. Nesting also lets checks share a parent's live
resources. Independent flow directories do not require shared create/destroy
playbooks.

Resolve top-level inventory paths relative to the root configuration directory.
Resolve scenario includes relative to the including file, and Ansible playbook
and pytest paths relative to the file declaring the scenario. Convert these
CVD-managed paths to absolute paths before adapter invocation. Adapter-owned
input remains opaque: strings in `vars` and literal `pytest.args` are not
recursively interpreted as paths.

These rules let callers select a configuration from another directory without
changing their shell's working directory. Ansible and pytest subprocesses run
from the root configuration directory, providing a consistent location for
project configuration discovery and relative references inside user code.

## Core concepts

### Scenario

A **scenario** is the universal execution unit. A scenario can contain child
scenarios to arbitrary depth and can define any of these optional phases:

1. `create`
2. `prepare`
3. `converge`
4. `idempotence`
5. `verify`
6. child scenarios
7. `cleanup`
8. `destroy`

Each scenario explicitly declares the phases it enables as keys. Omitted phase
keys are recorded as skipped. `verify` contains named test definitions as
described below. Other phase values may be null, a scalar, one adapter mapping,
or an ordered list. Those values are opaque adapter input; null and scalar
values use the applicable default. Each adapter mapping contains exactly one
adapter name and its input.

A child scenario inherits its parent's state and resources. It can add
resources and state values of its own. Its cleanup and destruction affect
only state owned by that child. Resource masking is required but not yet
implemented; its inventory semantics remain deferred below.

A child's `converge` can perform a side effect, such as a restart or
reconfiguration, before its own verification. No separate side-effect phase is
needed. Siblings are independent; nested siblings share only inherited parent
state and resources.

Children are declared in an ordered `nested` list. Each entry has a `name` and
either an inline scenario body or an `include` path. An included file contains
one scenario body and resolves relative to the including file. An entry cannot
combine `include` with inline phases, verification, or children. Names form
stable slash-separated scenario paths.

Without a scenario selector, root scenarios execute sequentially in declaration
order. Each scenario finishes its applicable cleanup and destruction before
the next sibling starts; an execution error stops later siblings. Directory
layout alone does not establish nesting: the `nested` declarations define the
scenario hierarchy.

### Resource

A **resource** is any provisioned or discovered object, such as a virtual
machine, container, Kubernetes object, or bare-metal server.

Every resource has:

- a stable identifier for the scenario's duration, including child scenarios;
- a type;
- an owning scenario responsible for creation/destruction;
- structured attributes;
- whether it currently exists;
- the scenario and phase where it was created;
- the scenario and phase where it was destroyed, when applicable;
- optional relationships to other resources.

Resource collections preserve declaration/provisioner order. An Ansible host
listed first remains first in generated inventories and resource views.

### State and view

**State** contains resources, configuration, paths, phase status, and test
results. A **view** represents state for another tool. Built-in JSON and YAML
views provide full-state interchange; application-specific views include
Ansible inventory, `kube.conf`, and `clouds.yml`.

CVD, provisioners, or user scripts consuming JSON/YAML can generate views.
Views are resources whose paths are stored in state; multiple views can
represent the same state.

### Test

A **test** is a named verifier invocation. A scenario can define one or more
tests directly under its `verify` mapping. Declaring those tests enables the
verification phase; there is no separate `tests` section. Omitted `verify`
is skipped. `verify: {}` or `verify:` enables an empty verification phase,
which passes without invoking a verifier. Verification accepts named test
mappings, not the opaque adapter payloads used by other lifecycle phases.
Legacy `tests` keys and scalar/list verify payloads are rejected.

### Adapters

Provisioner, converger, and verifier defaults are declared at configuration
top level, with explicit phase-adapter or test-verifier overrides. A
**provisioner** creates and destroys resources; a **converger** applies scenario
state changes; a **verifier** runs tests.

The current implementation supports `dummy` and `ansible` provisioners and
convergers; an omitted provisioner defaults to `dummy`. Verifiers support
`dummy`, `ansible`, and `pytest` (including pytest-testinfra). The Ansible
converger supports `prepare`, `converge`, and `cleanup`; real idempotence
checking and other adapters remain deferred.

The dummy provisioner returns one resource with ID and type `mock`, the dummy
converger performs no action, and the dummy verifier passes every named test by
default. Dummy phases accept `status: ok|error`; dummy tests accept
`status: ok|fail|error` to exercise result handling without external calls.

## Provisioning

CVD does not define what infrastructure `create` means. Provisioning is
delegated to a provisioner selected by the scenario.

Provisioners can be bundled, third-party, or project-local. Terraform,
Kubernetes, OpenStack, containers, and custom executables are possible
implementations.

### Minimal provisioner contract

A provisioner must support:

- `create`: create or discover resources and return a resource manifest; and
- `destroy`: destroy resources owned by the relevant scenario.

Create may allocate resources from an existing pool or create new objects.

Provisioner input is opaque to the lifecycle core. Each provisioner adapter
owns and validates its input schema. Output is returned to CVD and updated with
runtime information (phase, etc).

The provisioner contract must be available as a documented process protocol so
custom provisioners do not need to use CVD's implementation language. The exact
wire format and protocol versioning are deferred design decisions.

Scenario configuration supplies provisioner input, which is saved into state.
Provisioner results update state with actual resource information.

### Ansible provisioner contract

The built-in Ansible provisioner supports `create` and `destroy` through
explicit `ansible: {playbook: ..., vars: ...}` phase mappings. The
[provisioner guide](docs/provisioner.md) defines protocol-v1 inputs, manifest
validation, host bindings, and the dedicated destroy inventory.

Create must publish one complete, ordered manifest of actual resources. CVD
adds ownership, existence, and phase provenance. Destroy receives only existing
owned resources; a successful exit marks them destroyed, while failure leaves
them recorded as existing. Only create requires a result manifest.

Resources created before a complete result is accepted cannot yet be recovered
after failure or interruption. Incremental checkpoint semantics remain deferred.

## Native Ansible inventory and convergence

Ansible owns inventory interpretation, including inventory plugins, groups,
`group_vars`, `host_vars`, and variable precedence. CVD does not define an
alternative inventory language. Preserve the project's `ansible.cfg` and
inherited `ANSIBLE_CONFIG`; CVD does not generate a replacement configuration
or override the user's configuration selection. Its inventory environment and
reserved `cvd` inputs provide the lifecycle integration while Ansible retains
control of its other settings.

Optional top-level `inventory` is an ordered list of inventory source paths
(files, directories, or scripts), resolved relative to the root CVD
configuration:

```yaml
inventory: [inventory.yml]
provisioner: ansible
converger: ansible
verifier: dummy
```

CVD passes inventory through the subprocess's comma-separated
`ANSIBLE_INVENTORY`, without inventory CLI arguments. Existing
`ANSIBLE_INVENTORY` sources come first, followed by top-level `inventory`
sources in declaration order, then the runtime overlay when present. Explicit
CVD sources append to the inherited environment so projects can combine
externally selected inventories with scenario-specific inventories. An unset
or empty environment value adds no prefix.

When neither inherited nor explicit sources are present, Ansible's existing
configuration selects the inventory. Before appending a runtime source, CVD
asks `ansible-config dump --format json` for `DEFAULT_HOST_LIST` so the added
source does not replace configured inventory. With explicit CVD sources and
no inherited sources, only the explicit sources and optional overlay are used.
Original source locations are retained; CVD does not copy or flatten them,
preserving adjacent variable-file discovery. CVD-added inventory paths
containing commas are rejected because the environment variable cannot
represent them unambiguously. Only subprocess environments are changed.

Destroy sets `ANSIBLE_INVENTORY` to its dedicated owned-host inventory alone.
It does not append inherited, configured, or runtime-overlay sources, preserving
the rule that destruction targets come exclusively from recorded ownership.

A reusable create playbook can target `cvd_managed` with `connection: local`
and `gather_facts: false`. Host provisioning playbooks target this group by
convention during create.
CVD rebuilds it from recorded ownership during destroy; original membership
alone never establishes ownership.
Normal inventory host/group variables supply adapter-specific creation input;
`cvd.vars` remains available for arbitrary, non-host provisioning input.
Hosts outside the playbook's creation targets are used as declared, without
CVD taking lifecycle ownership of them. Convergence can still modify them.

### Runtime host binding

A reported resource can optionally bind to an existing inventory host:

```yaml
id: actual-provider-id
type: docker.container
attributes:
  ansible:
    inventory_hostname: web
    vars:
      ansible_host: actual-container-id
      ansible_connection: community.docker.docker
```

`inventory_hostname` is a nonempty string; `vars` is an optional mapping,
defaulting to empty. These are the only binding fields. Resource identity and
ownership remain independent of inventory identity. Resources without this
binding, such as networks or volumes, do not enter the generated inventory.

Bound hosts can represent physical servers, virtual machines, containers, or
pseudohosts used to call APIs. Inventory participation depends on the explicit
binding rather than on a particular resource type.

Before each Ansible `prepare`, `converge`, or `cleanup` invocation, and before
each Ansible or pytest test in `verify`, CVD generates
a private YAML inventory overlay from existing resources owned by that scenario
and its ancestors. Unrelated siblings and destroyed resources are excluded.
Ancestor resources precede child resources and each manifest's order is
preserved. Multiple visible resources binding the same host, or malformed
bindings, produce an error in the consuming phase. Resources are already
persisted at that point, so normal cleanup and destruction can still proceed.

CVD validates reported inventory host names against the original sources using
`ansible-inventory --list` with the consuming playbook's directory. Unknown
names are errors, rather than silently creating new inventory hosts. CVD cannot
infer that a playbook failed to report an intended host solely from group
membership: the create playbook is responsible for a complete manifest.

Ansible loads original sources first and the overlay last. The overlay contains
only host variables; group membership and group variables stay in the original
inventory. Ordinary Ansible precedence applies: a later inventory source does
not override higher-precedence variable sources such as extra vars.

Ansible converger phases require explicit `ansible: {playbook: ..., vars: ...}`
mappings, with the same path and `cvd` extra-vars conventions as the provisioner.
`cvd.action` names the phase. `cvd.resources` contains existing resources
visible to the scenario (ancestors first, followed by its own resources),
including hosts and non-hosts. Siblings, destroyed resources, and generated
view artifacts are excluded. Runtime host data also comes through inventory. No result manifest is required:
a nonzero exit or launch failure is a phase error. Explicit `dummy` mappings
can override the top-level converger. Lists containing Ansible actions are
rejected until ordered real-adapter lists are implemented.

Generated overlays are persisted under the run's `views` directory and recorded
as view resources under `scenarios[PATH].views.ansible_inventory`, with their
file path in `attributes.path`. They are separate from provisioner-owned
resources and do not affect create/destroy counts. The file records the latest
rendered view for that scenario; destruction does not erase that historical
artifact. Source inventory files remain user-owned and are not snapshotted.

Resource masking is not yet implemented. Excluding a resource from an overlay
would not remove its host from original inventory sources; execution selection
for masked hosts requires a separate design before masking is implemented.

### Resource lists by type

Ansible invocations receive `cvd.resources_by_type`, grouping the same full
records as `cvd.resources` by exact type and preserving order within each type.
Missing types are omitted. The [provisioner guide](docs/provisioner.md#using-resource-lists-across-phases)
defines resource visibility for each phase. Grouping is derived input; the
create manifest and persisted resource list remain flat.

## Ansible verifier

`verifier: ansible` selects Ansible playbooks for named tests; each test can
also override the top-level verifier with `verifier: ansible`:

```yaml
verify:
  service:
    ansible:
      playbook: verify.yml
      vars:
        expected_port: 8080
```

Playbooks resolve relative to the scenario's configuration file and run from
the root configuration directory. They receive original inventory sources plus
the scenario's runtime overlay, and the same `cvd` input as convergence:
`cvd.action` is `verify`, `cvd.vars` holds the test's variables, and resources
include existing visible ancestor and scenario resources. No result manifest
is required. Exit zero means `pass`; any nonzero exit (including failed Ansible
assertions), launch error, or inventory error means `error`. CVD retains the
named test and scenario/verify provenance, stops later tests and children, and
attempts normal cleanup and destruction. Dummy status controls and pytest
options are rejected for Ansible tests.

## Pytest verifier

`verifier: pytest` selects pytest for named tests; a test can override the
top-level default with its own `verifier`. Testinfra uses this same adapter
through the installed `pytest-testinfra` plugin:

```yaml
verifier: pytest
scenarios:
  default:
    verify:
      web:
        pytest:
          path: tests/test_web.py
          args: ["-q"]
```

A testinfra module can select its inventory group directly:

```python
testinfra_hosts = ["ansible://webservers"]
```

Each pytest test requires `pytest.path`, a file or directory resolved relative
to the file containing its scenario. It must exist at configuration load time.
Optional `pytest.args` is an ordered string list passed literally, without a
shell. CVD runs `pytest ARGS PATH` from the root configuration directory using
`pytest` from the current `PATH`, so an activated virtual environment works.
The test's verifier must be `pytest` to accept pytest options; dummy status
controls do not configure pytest results. No testinfra flags or host selection
are injected automatically; ordinary pytest tests are also supported.

CVD sets `ANSIBLE_INVENTORY` for each pytest process using the same source order
as Ansible playbooks: inherited environment sources, explicit CVD sources,
then the scenario's runtime overlay, if any. A separate inventory snippet
containing the current `cvd` context is appended last. Ansible defaults are resolved
through `ansible-config` when inherited and explicit sources are absent.
Original sources remain at their own paths, retaining group and
host variable files. Testinfra's `ansible://` backend consumes this environment
variable through Ansible; tests need no inventory CLI argument. Inventory
source paths containing commas are rejected because this environment variable
cannot represent them unambiguously. The parent process environment is not
modified. Pytest users need Ansible installed when resolving its configured
inventory defaults or validating runtime bindings; testinfra is required only
for tests using its fixtures/backend.

The overlay is freshly derived before verification, including when converge
was omitted and when a selected child inherits parent resources. Its path is
persisted as a view before launching pytest. External inventory hosts remain
available without becoming provisioner-owned resources.

### CVD context in Testinfra

Before each named pytest invocation, CVD generates a private inventory snippet
with `all.vars.cvd`. Testinfra tests using the `ansible://` backend access it
with `host.ansible.get_variables()["cvd"]`; no CVD-specific Python fixture or
plugin is required. Inventory transport uses Testinfra's existing variable
interface and keeps the original inventory files intact.

The context uses the same protocol-v1 fields as Ansible playbook input:
`protocol_version`, a fresh `invocation_id`, `directory`, `input_file`,
`result_file`, `action: verify`, `scenario_selector`, `vars`, `resources`, and
`resources_by_type`. Pytest currently has no configured `vars` field, so
`cvd.vars` is an empty mapping. Resource views include all existing resources
visible to the current scenario, ancestors first, including non-host resources;
unrelated siblings and destroyed resources are excluded. Each named test gets
its own context, even when several tests belong to the same scenario.

The snippet defines variables only: it introduces no hosts, groups beyond
Ansible's built-in `all`, or connection settings. It is supplied even when
there are no runtime host bindings or no CVD-owned resources. Ansible's normal
inventory variable precedence applies; `cvd` remains a reserved name and
user inventory must not override it. Tests using other Testinfra backends do
not gain an Ansible variable interface from this snippet.

The snippet and a JSON input file containing the same top-level `cvd` mapping
live in a private per-invocation directory (0700, files 0600). They remain
available while pytest runs and are removed after success, failure, or a launch
error. The result path is supplied for context consistency; pytest does not
publish a result file. These files are transient invocation inputs rather than
persisted resource views, preventing reuse of stale test-specific context.

For this initial adapter, exit zero is `pass`; **every nonzero pytest exit is
`error`**, including assertion failures, collection/internal errors, interruption,
and no tests collected. Launch and inventory-preparation errors are also
`error`. CVD records the named test result and error message with scenario and
`verify` provenance, stops later tests/children, and attempts normal cleanup
and destruction. `--keep` retains its existing destruction-suppression behavior.
Distinguishing pytest assertion failures as `fail` is deferred. This is an
explicit temporary exception to the general verifier result classification.

## Execution and selection

All commands accept mutually exclusive `-F DIR` / `--directory DIR` and
`-f FILE` / `--file FILE`. Directory selection discovers `cvd.yaml` or
`cvd.yml`, preferring `cvd.yaml`. Without either option, use `cvd.yml` in the
current directory. Explicit file selection uses the supplied path. State
commands use that path only to locate the default `.cvd` directory; the
configuration file need not exist.

The CLI must support:

- Selecting a scenario subtree, running only the ancestor setup chain needed
  to reach it and skipping unrelated siblings. For `A/3`, set up `A`, then run
  `A/3`; skip `A/1` and `A/2`.
- Stopping at a phase and repeating it, including repeating the last phase
  before cleanup has run.
- Persisting the current phase between invocations and forcing a transition.
- Keeping resources/state for inspection and cleaning up from the current phase.

Exact interactive transition and recovery rules remain deferred.

Example of scenario selectors:

```text
default
default/restart
default/instance_provision/second-reconfiguration
```

## Results, failures and errors

Completed phase results are `skipped` (omitted or skipped by the operator),
`pass` (successful), or `error` (lifecycle or verifier execution failed).
Verifiers can additionally report assertion failures as `fail`; these are
recorded and execution continues.
The current Ansible and pytest exceptions are specified above.

Every error retains its scenario path and phase, such as
`default/restart::prepare`. Final reports include failures and errors; either
causes a nonzero run exit. Verifiers may also produce stored reports such as
JUnit or CI annotations. An internal panic may leave resource state uncertain.

## Cleanup and destroy semantics

Cleanup is stack-based and is best-effort:

- the active child is cleaned before its parent;
- a scenario's `cleanup` runs if scenario execution was entered and create was
  successful or skipped, including after `converge`, `verify`, or child failure;
- `destroy` applies only to resources owned by that scenario;
- `destroy` may run even if create was not called;
- a partial `create` may persist discovered/created resources so destruction can be
  attempted (provider specific);
- cleanup and destruction errors do not hide the primary failure;
- keep mode can suppress cleanup or destruction for inspection;
- cleanup/destroy errors skip later child, converge, and verify work, but do not
  prevent remaining applicable cleanup and destroy operations.

Current `--keep` suppresses destruction only; suppressing cleanup remains
unimplemented.

### Changing keep mode during a run

On Unix, `cvd run` handles `SIGUSR1` to enable keep mode and `SIGUSR2` to
disable it. `--keep` selects the initial value; either signal can override
that value while the run is executing, including while CVD waits for an
Ansible or pytest subprocess. Send signals to the CVD process PID, not its
subprocesses or process group. Other commands do not install these handlers.

Signals set a run-wide policy, rather than toggling it: repeated `SIGUSR1`
keeps it enabled, and repeated `SIGUSR2` keeps it disabled. The most recently
handled signal determines the policy for upcoming destruction decisions.
Ordinary Unix signal delivery applies; these signals are not a queued command
protocol.

Check the current policy separately when entering each scenario's `destroy`
phase, including error unwinding, ancestor destruction, and later siblings or
roots. Enabled keep mode records that destruction as `skipped`, retaining
the scenario's resource existence records. Disabling it restores ordinary
ownership-scoped destruction for subsequent destroy phases. Cleanup continues
under either setting. Signals themselves are not failures and do not alter
recorded verifier results or errors.

A destroy phase already entered runs to completion. Enabling keep cannot
cancel that destruction or restore deleted resources. Disabling keep does not
revisit destruction already skipped for an earlier scenario. This boundary
preserves provider execution and lifecycle ordering while allowing operators
to retain resources when they discover a problem during a long-running phase.

On receipt, a background listener promptly prints an acknowledgement to stderr,
including while an Ansible or pytest subprocess is still running. Each message
starts with two newline characters and ends with one newline:

```text
\n\nCVD: keep mode enabled (SIGUSR1)\n
\n\nCVD: keep mode disabled (SIGUSR2)\n
```

The escapes above represent actual newline characters. Repeated requests for
the same setting also receive acknowledgements. Output may interleave with
adapter output. Messages are live notifications and are not persisted or
replayed by `state-report`. Notification output is best-effort: a full internal
buffer or stderr failure must not prevent the policy update or interrupt an
adapter. The listener drains queued notifications on normal run exit.

Signal handlers update the live policy and notify the listener without blocking;
formatting and terminal output happen outside the handlers. This gives operators
feedback during long-running phases without making signal handling wait for
terminal output. The next normal lifecycle state write, including the final
write, snapshots it into the existing `keep` field
as described in [state.md](state.md). This keeps state persistence in the
lifecycle thread and avoids filesystem operations inside signal handlers.

## State and reporting

[state.md](state.md) defines persisted data, run history, and atomic writes.
Each run is independent; `last` selects the most recently started run.
`state-resources` hides destroyed resources unless `--deleted` is requested.

Interactive run output groups phases under a single scenario entrance header.
All scenario, phase, command, and verdict lines are emitted without indentation.
Successful `create` and `destroy` phases report resource changes as
`create: N resource added` and `destroy: N resource deleted`; their styling uses the normal passed
(green) color. `state-report` reproduces the same count-bearing lines.
Every phase start and result line is prefixed with its full
`scenario/subscenario::phase` path.
Run summaries report total created and destroyed resource counts alongside
errors and verifier failures.
Entrance and verdict lines begin with `Scenario: `; for example,
`Scenario: default/restart` and `Scenario: default/restart: passed`. The full
entrance and verdict lines are bold, and each scenario ends with a `passed`,
`error`, or `skipped` verdict. Passed phase and verdict lines are green, error
lines are red, and skipped lines are gray. Redirected output and output produced
with `NO_COLOR` set contain no ANSI styling.

`state-report` replays this scenario-grouped report from one persisted run. It
does not execute lifecycle actions or parse the current configuration, and it
reports persisted `pending` or `running` phase states without retrying them.

`syntax-check` loads and validates a CVD configuration without executing any
lifecycle action or creating run state. It validates the configuration schema
and all referenced inventory sources, scenario includes, Ansible playbooks, and
pytest paths. A valid configuration exits with status 0; any invalid syntax,
schema, or missing referenced file produces an error and a nonzero status.

## Initial non-goals

The initial version does not require:

- Molecule configuration compatibility or automatic conversion;
- a universal infrastructure definition language;
- a full inventory management system;
- a remote execution service or distributed scheduler;
- automatic flaky-test retries;
- a graphical interface;
- a plugin marketplace; or
- compatibility abstractions for every test framework.

A Molecule migration guide can be written separately.

## Planned examples

Provide an Ansible-driven virtual-machine showcase demonstrating integration
tests against actual VMs, including creation, convergence, verification, and
destruction. This is planned example coverage; the VM provider and example
implementation remain to be chosen.

## Deferred decisions

These choices require focused design work before implementation:

- stabilization and compatibility policy for the current version-1 YAML schema;
- general third-party provisioner process protocol and versioning beyond the
  built-in Ansible contract;
- Ansible inventory execution selection for masked resources and richer view overrides;
- final persistence layout (see state.md);
- exact safe-phase and rerun rules after interrupted executions;
- plugin discovery and distribution; and
- aggregation rules for nested scenario results.
