# Introduction

CVD (short for Create-Verify-Destroy) is a tool for implementing end-to-end integration
tests for IaaC (infrastructure as code). It provides the ability to run tests on
ephemeral resources (virtual machines, clusters, baremetal servers).

It's done via a well-known pattern of 'deploy on ephemeral staging'. CVD helps to glue
together 'create/destroy', converge and verify parts of this process.

It is heavily influenced by (Molecule)[https://docs.ansible.com/projects/molecule/], but
trying to overcome it's limitations (lack of nested scenarios, rigid structure, hard tool
preferences).

The foundational concepts are:

* Production code: the code that will be deployed in production. It can be
  a playbook (with roles), or a group of Helm charts, or Terraform modules, or a combination
  of those.
* Ephemeral: created and destroyed. Ephemeral resources do not outlive a CVD run
  (except for debugging purposes). This means CVD is inherently not for production.
  Do not use it for production deployments. Use it to test your code before doing production
  deployments.
* Test rigs, also known as 'preparation code'. Code that allows adjusting staging
  ephemeral environments to be suitable for deploying production code (note: deploying
  production code is not the same as deployment into a production environment; CVD is built
  to deploy production code into non-production environments). Examples may include
  updating network configuration, providing stub implementations for external systems,
  cleaning apt caches, and adding other team members' SSH keys in a non-production way (e.g., for
  debugging only).
* Convergence: running production code to achieve the expected state. In the ideal world,
  convergence must be idempotent, meaning running it a second time should make no
  changes. It should also be re-convergent (if some configuration drift is introduced,
  it should converge back to the expected state).
* Tests (this phase is often called 'verify') that check the system is in the expected
  state. Tests can be mutating (e.g., running a container to see if Podman is working),
  or they can be smoke tests (e.g., checking if the site displays something instead of 'Welcome to Nginx').
* Side effect: something that happens to a converged system. Examples may include
  a hard reboot of the entire cluster (we want to test that the system boots back after a total
  outage) from a working state, or adding/removing nodes, or adding a user.
* Scenario: convergence and chain of side effects (maybe, with some tests in between).

## Example

Let's say we are doing full-scale testing for our deployment of Kubespray and
some components into Kubenetes cluster. It's not a plain Kubernetes; it's Kubernetes
with additional components (Rook, Velero, Prometheus).

* We `create` our ephemeral infrastructure. We order few scalable baremetal servers.
* We `prepare` them (our ephemeral infrastructure is built on cheaper servers, so we
  need to simulate having many disks for Rook; we create a few LVs on a VG
  built on a single drive). CVD generate an inventory with connection information for
  newly created servers.
* We `converge` (install) Kubespray and the deploy Helm charts into it.
* We `verify` that we have a working cluster. We create a deployment, run a Velero
  backup, confirm that it's been backed up, and then restore this deployment
  into another namespace. We verify that the restored application is accessible via
  ingress (so our SSL code is provisioning certificates as expected).
* We introduce a `side-effect` (`echo b > /proc/sysrq-trigger`, admittedly harsh, but
  truly simulating a brief outage).
* We verify that we receive alerts from Prometheus indicating that some nodes are down.
* We also verify that those alerts stop firing, meaning the cluster has come back online.
* We verify other aspects of our system (logs are collected, there are no
  failed systemd units, Ceph cluster is healthy, and so on).
* We run some `cleanup` code
* Finally, we `destroy` our ephemeral infrastructure (cancelling baremetal servers we've
  ordered at create phase). It will happen even if there are failed tests.
* CVD reports results (success, failure) for the run.

All this happens with a simple `cvd run` command.
