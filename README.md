# CVD

CVD is Converge - Verify - Destroy tool, allowing to run integration
tests for IaaC (Infrastructure as a code) code.

*Note: Work in progress.*

## Motivation

I want to have something better than Molecule and more flexible than Vagrant.

Also, I want to handle VMs, Terraform and Kubernetes resources on equal ground.

I know what I want, but writing is delegatged to LLMs, under tight supervision and
good integration test coverage.

If it will work out, I will announce it, if not, I will archive.

## Current status

I did few iterations of `cvd.yaml` structure, got ansible-pytest working together.

There is general okayish for provisioner (create/destroy), for converger (converge) and
for verifier (well, tests). Verifier is already ansible and pytest.

## Plans

* Make a showcase with VMs (ansible)
* Look deeper at nested scenarios. I still in doubts what is 'nested scenario'. Should it be nested dict or should it be nested directory?
* Start working on Terraform integration. Specifically, I'm interested in Terraform provisioner (and inventory generation). Also, how
do we describe 'what to create' in cvd.yaml for terraform? Gonna be fun.
* Make a round with Kubernetes as converger. I don't see any issues with that (just generate kube.conf)

# Long readme (under writing)

(intro here)

The focus for CVD is on actual integration with real servers (virtual machines), real clusters, real network switches, providers, etc.
It can be used for small-scale testing for roles, but primary goal is full-fledged integration tests.


## Directory structure

CVD works with a cvd.yaml file. This is entry point into a specific directory (Note: if we have scenarios, how do we call 'the directory'?).
Everything in cvd.yaml is referenced relative to the directory where cvd.yaml is living. That means you don't need to change working
directory to run specific cvd scenario (scenario?).

CVD configuration is expected to live in the same directory as your iaac code (playbooks, helms, terraform root module, etc).

There are two idiomatic way to use cvd:

**Small scale** configuration. cvd.yaml in the root directory of the project.

```
├── playbooks/
├── modules/
├── tests/
├── cvd.yml
└── your-ci.yaml
```

In this configuration you are desribing a single test flow: list of scenarios to run sequentially, maybe with some nesting.

**Large scale** configuration. Multiple flows (should I call them flows? scnearios?) are living in different directories.
It increase presence of '../..' syndrom in CVD configuration, but allows clear separation between 'production' code and test scenarios.

In such case CVD is called `cvd run` (it will discover cvd.yaml or cvd.yml automatically).

```
├── simple
│   ├── cleanup.yml
│   ├── converge.yml
│   ├── create.yml
│   ├── cvd.yml
│   ├── destroy.yml
│   ├── inventory.yml
│   ├── prepare.yml
│   ├── tests
│   │   └── test_something.py
│   └── verify.yml
├── extended
│   ├── cleanup.yml
│   ├── converge_install.yml
│   ├── converge_upgrade.yml
│   ├── converge_rolling_reboot.yml
│   ├── other_create.yml
│   ├── cvd.yml
│   ├── other_destroy.yml
│   ├── inventory.yml
│   ├── prepare.yml
│   ├── tests_common
│   │   └── test_something.py
│   ├── tests_upgrade
│   │   └── test_upgrade.py
│   ├── tests_rolling
│   │   └── test_rolling.yaml  # playbook
```

In such case cvd flow (?) is called `cvd -F extended run` or `cvd -F simple run`.

Note: It's not expected to reuse playbooks (create/destroy) between flows (?) because if you want
to reuse code, you've better to use nested or sequential scenarios within a single cvd.yaml.

## Inventory

Note: in CVD inventory desribes things you have and will have after create phase. It's very
similar to Ansible inventory, but it has lifecycle, and it can be used even for non-ansible things,
like Kuberenetes and Terraform modules.

Inventory consists of two parts: content of `inventory.yaml` (looks like a normal ansible inventory),
which contains external resources (the one you don't create/destroy during cvd session), and *declaration*
of resources which must be created by create phase. (?) So we call this thing 'session' (instead of run).

Create phase, creates those resources and writes resource manifest. Resources are remembered by CVD
and used to generate so-called session inventory. If resource is looks like a host (something you want
to be a host in the Ansible inventory during converge/verify phases, e.g. real servers, virtual machines,
lxc containers, even pseudohosts to work with APIs), it will be included in the session inventory and joined
together with initial inventory (so we call it initial inventory).

Nested scenarios are assuming resources, created by outer scenario as 'external' (so they create/destroy
only own resources). Each such inventory is added in the list of inventories (for Ansible).

## Ansible calling convention

CVD aims to keep ansible configuration as prestine as much as possible. There are two (?) alterations:

1. `ANSIBLE_INVENTORY` environment variabler is used to pass inventory. If you add command-line --inventory, you are in danger zone.
  Existing `ANSIBLE_INVENTORY` is honored and is added before initial inventory, using comma.
2. cvd variables are passed using -e command line.
3. All file paths in cvd.yaml are expanded to absolute paths before been passed to the ansible.

Working directory is unchanged. That means, if you call it from a directory different from cvd.yaml, you need

It is possible to break ansible calls from CVD by setting very odd ANSIBLE_CONFIG variable, don't do it.

Pytest (for testifra) is getting the same ANSIBLE_INVENTORY, so any ansible integration (e.g. module calls) should
work out of the box. `cvd` variable is passed via an additional inventory snippet.


