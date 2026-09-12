# Keep mode

## Keeping resources

By default CVD removes all created resources after verify and cleanup phases.

When something went wrong during converge or verify phases, operators may want to inspect
the resulting resources.

There are a few ways to instruct CVD to keep resources after creation:

1. `--keep` command line option to keep created resources (destroy phase is skipped).
2. During a run it's possible to send a unix signal (`kill` utility) to switch between
   keep and destroy modes (only when run on Linux/Unix):
   - `SIGUSR1` enables keep mode.
   - `SIGUSR2` disables keep mode.
3. «not implemented yet» option in `cvd.yaml` «no name yet» adds a pause before destroy 
   if any failures or errors are detected. During this waiting period user can press (to be decided)
   buttons to pause CVD indefinitely or press (to be decided) to switch to keep mode.
4. «not implemented yet» If «no name yet» option is set in the `cvd.yaml`, CVD prints
   pre-signed links and listens on a specific port to accept HTTP GET requests (with pre-signed link)
   to enable/disable keep mode or to wait.
   This is useful for CI runs when CVD is running in a CI server.
5. «not implemented yet» If «no name yet» option is set in the `cvd.yaml`, CVD query a specific
    URI (set in cvd.yaml), if (to be decided what is in the answer) it enables keep mode.
6. «not implemented yet» If «no name yet» option is set in the `cvd.yaml` CVD polls a specific
   URI, and wait until this that URI returns a specific HTTP code (so called 'wait mode').
   It allows to delay resource destruction until debugging session is finished.

## Debugging session

«not implemented yet»

It's possible to enable debugging session (practically, a shell) in case of failure.

There are three types of debug sessions:

* blocking ingress session (during 'wait' period on failed tests before resource destruction).
* blocking egress session (reverse shell), run after failure.
* non-blocking session (allow to connect to CDV during different phase execution). Even non-blocking
  session pause resource destruction until session is over.

Authorization is provided via private/public keys, password or by printing pre-signed link in the logs.

«not implemented yet, need clarification»

## Failures during create phase

CVD provisioner is relying on resource manifest from the (user provided) code. If there are failures
during provisioning, only resources declared in mainfest are destroyed. This can leads to resource
leaks.

Termination of CVD during create phase may leave created resources without generated manifest,
causing resource leak.

## Cleaning up resources after enabling keep mode

If CVD was instructed to keep resources, it's possible to clean up them later, only if
content of `.cvd` directory is preseved (usually true for operator machines, usually is false for CI
runs on ephemeral CI runners).

«need clarification on how»

## Resource leak

If content of  `.cvd` directory is lost, or there was a failure during create/destroy phases,
some resources may be left allocated (created) forever. CVD does not provide any means of
'garbage collection' for them, but as one possible solution, metadata (labels, tas)
can be used to mark CDV-created resources for later cleanup (manual or with user-managed automation).
