# Agent guidance

Read [REQUIREMENTS.md](REQUIREMENTS.md) before architecture or implementation
work. It defines product behavior, vocabulary, scope, and deferred decisions.
[state.md](state.md) specifies persistence; the
[provisioner guide](docs/provisioner.md) specifies the Ansible playbook contract.
REQUIREMENTS.md takes precedence if they conflict.

- Update the relevant spec when product decisions change. Keep the provisioner
  guide aligned with supplied variables and the result protocol.
- Record decisions affecting configuration, public protocols, persisted state,
  or plugin APIs with their rationale. Do not silently resolve deferred
  decisions; ask when they materially affect public behavior.
- Add tests for changes to lifecycle ordering, ownership, cleanup, result
  classification, state persistence, and nested selection.
- Preserve user changes and do not edit generated output in `target/`.
- Run the narrowest relevant checks first. For Rust changes, run `cargo fmt`
  and `cargo test` when applicable.
