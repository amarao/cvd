# Phases

* Create (Provisioner)
* Prepare (Converger)
* Converge (Converger)
* Idempotence (Converger) «not implemented yet»
* Verify (Verifier)
* Side effect (Converger)
* Cleanup (Converger)
* Destroy (Provisioner)

(some phases may be skipped, e.g. prepare/cleanup phases are rarely used).

## Provisioner

Provisioner is responsible for create/destroy phases. There are different provisioners,
but if one provisioner was used for create, it will be called for destroy phase.

Supported provisioners:

* Ansible
* CloudFormation «not implemented yet»
* Dummy (do nothing, for demo/self-testing purposes)
* Exec «not implemented yet»
* Heat (not sure I want to implement this) «not implemented yet»
* Kubectl/Helm «not implemented yet»
* Terraform «not implemented yet»

## Converger

Converger is responsible for deploying production code on freshly created ephemeral
infrastructure. It is also responsible for doing preparation/cleanup and for introducing
side effects. A scenario's `side_effect` runs after its verification and before its
children. This lets a parent verify a baseline and apply an effect, then lets a child
verify the changed or recovered state. It is possible to combine different convergers
(e.g., to use Terraform for the converge phase, run one side effect with Ansible, and
another with 'exec').

Supported convergers:

* Ansible (playbooks)
* Dummy
* Exec «not implemented yet»
* Kubectl/Helm «not implemented yet»
* Terraform «not implemented yet»

## Verifier

Verifier is responsible for running tests on converged infrastructure.

Supported verifiers:

* Ansible (playbooks)
* Dummy
* Exec «not implemented yet»
* Pytest (testinfra)
