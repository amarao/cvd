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

