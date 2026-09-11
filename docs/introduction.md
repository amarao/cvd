# Introduction

CVD (Create-Verify-Destroy) runs integration tests for infrastructure as code
by coordinating resource creation, preparation, convergence, verification,
cleanup, and destruction. Scenarios describe the test lifecycle and can contain
child scenarios that share parent resources while owning their own additions.
CVD integrates with existing Ansible projects and supports Ansible and pytest
verification; this manual is an initial documentation stub for the evolving
project.
