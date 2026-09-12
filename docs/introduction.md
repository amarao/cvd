# Introduction

CVD (short for Create-Verify-Destroy) is a tool for implementing end-to-end integration
tests for IaaC (infrastructure as a code). It gives an ability to run tests on
ephimerial resources (hosts, virtual machines, clusters, even real servers).

It's done via a well-known pattern of 'deploy on ephimerial staging'.

The foundational concepts are:

* Production code: the code which will be deployed in the production. It can be
  a playbook (with roles), or group of Helm charts, or Terraform modules, or combination
  of those.
* Ephimeral: created and destroyed. Ephimeral resources do not outlive a CVD run
  (except for debugging purposes). That means, CVD is inherently not-for-production.
  Do not use it for production deployments. Use it to test your code before doing production
  deployments.
* Test rigs also known as 'preparation code'. A code which allow to adjust staging
  ephimeral environment to be suitable for deploying production code (note: deploying
  production code is not the same as deployment into production environment, CVD is build
  to deploy production code into non-production environments). Example of such may be
  updating network configuration, providing stub implementations for external systems,
  cleaning apt caches, adding other team members ssh keys in non-production way (e.g. for
  debugging only), etc.
* Convergence: running production code to get to the expected state. In the ideal world
  convergence must be idempotent, that means, running it second time it should make no
  changes. It also should be re-converngent (if some configuration drift is introduced,
  it should converge back to the expected state).
* Tests (this phase is often called 'verify'), which checks that system is in the expected
  state. Tests can be mutating (e.g. try to run container to see if Podman is working),
  or they can be smoke tests (e.g. check if site displays something instead of 'Welcome to Nginx').
* Side effect: something happening with converged system. Examples of such may be
  'hard reboot for all cluster' (we want to test that system boot back after total
  outage) in working state, or 'adding/removing nodes', 'adding user'.
* Scenario: convergence and chain of side effects (maybe, with some tests in between).

## Example

Let's say we are doing full-scale testing for our deployment of kubespray and
system components into a cluster. It's not a plain 'kubernetes', it's a Kubernetes
with additional components (Rook, Velero, etc).

* We `create` our ephimeral infrastructure. We order some scalable baremetal servers.
* We `prepare` them (our ephimeral infrastructure is build on cheaper servers, so we
  need to imitate that we have a lot of disks for Rook, we create few LVs on a VG
  build on a single drive).
* We `converge` Kubespray and few helms which we expect to have.
* We `verify` that we got working cluster. We create a deployment, run Velero
  backup, get confirmation that it's backed up, and, maybe, even restore this deployment
  into other namespace. And we check that restored application is accessible via
  ingress (so our SSL code is provisioning certificates as expected).
* We introduce some `side-effect` (`echo b > /proc/sysrq-trigger`, kinda cruel, but
  really imitating a brief outage).
* We check that we alerts from Prometheus that some nodes are down.
* We also check that those alerts stop firing, that means, cluster is come back alive.
* And we check other 3102 aspects of our system (logs are collected, there are no
  failed systemd units, Ceph cluster is healthy, you name it).
* We run some `cleanup` code
* Finally, we `destroy` our ephimeral infrastructure (cancelling baremetal servers we've
  ordered at create phase).

All this with a simple `cvd run` command.

Let's assume some of our tests failed (let's say we got no alert that node is down when we
reboot it, and Grafana is down after reboot). We got 'red tests'. CVD record the issue,
allow other tests to pass (to get all found problems in one go), and run cleanup.

After that it reports that some tests are failed and exit with non-zero exit code to
fail your CI, or your local run.

CVD is also allows to keep ephimeral infra for future debugging (`--keep` flag or
`SIGUSR1` signal to the cvd process).


