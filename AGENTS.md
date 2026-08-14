# Agent Guidance

## Project intent

CVD is a streamlined replacement for Molecule's role as an Ansible test
harness. It tests existing Ansible/IaC projects, roles, and collections while
remaining agnostic about how infrastructure is created.

Read `REQUIREMENTS.md` before proposing architecture or implementation work. It
is the current product source of truth.

For the current dummy stub, also follow `dummy.md` and `state.md`. They are an
implementation plan and state-storage notes; `REQUIREMENTS.md` takes precedence
if they conflict.

## Product vocabulary

Use these terms consistently:

- **scenario**: the recursively nestable lifecycle unit;
- **scenario path**: the stable selector for a scenario in the tree;
- **resource**: any provisioned or discovered object;
- **state**: persisted resources, configuration, paths, phase status, and test
  results;
- **view**: a concrete representation of state for another tool;
- **test**: a named verifier invocation;
- **provisioner**: an implementation that creates and destroys resources; and
- **converger**: an implementation that applies scenario state changes; and
- **verifier**: an Ansible, pytest, or executable test adapter.

Model a side effect as a child scenario whose `converge` performs the mutation.
Do not add a separate side-effect lifecycle unless requirements change.

## Product constraints

- Preserve existing project configuration by default.
- Require each scenario to declare enabled phases as keys. Phase values are
  opaque adapter input; record omitted phase keys as skipped.
- Use top-level provisioner, converger, and verifier values as defaults, with
  optional narrower overrides.
- Keep the core resource model broader than Ansible inventory hosts.
- Preserve resource and inventory host ordering from provisioner input.
- Keep provisioner configuration opaque to the lifecycle core; each built-in
  adapter owns its schema. Third-party provisioners return resource information
  through the process protocol, whose exact format is deferred.
- Child scenarios inherit parent state and resources, can mask parent resources,
  and own only the resources they add.
- Treat built-in JSON and YAML state views as the base interchange forms. CVD,
  provisioners, or user scripts can provide additional application-specific
  views such as Ansible inventory, `kube.conf`, or `clouds.yml`.
- Persist each run independently and expose `last` as the most recently
  started run; never overwrite run history during normal execution.
- Persist resource existence and create/destroy provenance. `state-resources`
  hides destroyed resources unless `--deleted` is requested.
- Design cleanup for partial setup, failed convergence, failed verification,
  interruption, and interactive reruns.
- Use `skipped`, `pass`, and `error` for phase results. Verifiers can additionally
  report assertion failures as `fail`; these do not prevent continued execution
  unless `fails_are_fatal` is configured. Always retain the originating scenario
  path and lifecycle phase for errors.
- Favor explicit, inspectable state and stable selectors over hidden behavior.
- Group interactive phase output under one bold entrance per scenario and emit
  a closing scenario verdict; keep redirected output free of ANSI styling.
- Prefix every phase start and result line with its full
  `scenario/subscenario::phase` path.
- Treat stopping, retaining state, repeating a phase, forcing a phase
  transition, and cleaning up from the current phase as core interactive
  capabilities.
- Keep sibling scenarios independent. Selecting a nested scenario runs only the
  ancestor setup chain needed to reach it and skips unrelated siblings.
- Represent children as an ordered `nested` list with named inline scenarios or
  relative scenario-fragment includes.
- Resolve Ansible converger playbooks at configuration load time relative to
  the containing scenario file; reject missing or ambiguous default playbooks.
- Run `ansible-playbook` with resolved paths from the root configuration
  directory; classify launch failures and non-zero exits as phase errors.
- For an Ansible `create`, treat the phase mapping apart from `ansible` options
  as generated inventory. Select resources using its configured group name or
  dotted path, pass that inventory through `ANSIBLE_INVENTORY` to later Ansible
  phases, merge facts from successful `set_fact` tasks into host attributes, and
  use `public_ip` as `ansible_host` when no explicit host address exists.
- Run remaining applicable cleanup and destroy operations after an error in
  cleanup or destroy, while skipping later child, converge, and verify work.
- Allow explicit keep mode to suppress cleanup or destruction for inspection.

## Scope discipline

Do not assume Molecule compatibility. It belongs in migration documentation,
not the core design.

Do not introduce a CVD infrastructure language, remote scheduler, GUI, plugin
marketplace, automatic retry system, or broad test-framework abstraction unless
the requirements explicitly expand.

Avoid selecting concrete technologies for deferred decisions without recording
the decision and rationale. Prefer small design notes or ADRs for choices that
affect configuration, public protocols, persisted state, or plugin APIs.

## Working practices

- Keep documentation concise and update `REQUIREMENTS.md` when product decisions
  change.
- Add tests for lifecycle ordering, ownership, cleanup, result classification,
  state persistence, and nested selection before relying on those behaviors.
- Preserve user changes and do not modify generated build output in `target/`.
- Run the narrowest relevant checks first; for Rust changes, format with
  `cargo fmt` and run `cargo test` when applicable.
- Do not silently resolve items under **Deferred decisions**. State assumptions
  and ask for a decision when they materially affect public behavior.
