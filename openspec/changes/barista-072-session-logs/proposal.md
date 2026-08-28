# Proposal: bounded session application logs

## Why

A session's entrypoint writes its only useful startup and failure diagnostics to the substrate's application log. Contract A exposes lifecycle events and exec output, but it cannot read that application log. During the public GitHub Factory acceptance the workload printed a precise configuration error and exited, while the user-facing event stream showed no process output. Diagnosis required host-level access to Hypeman's private data directory.

The BRD's NFR-5 and B50 require session-scoped observability and inspectable suspended sessions. Hypeman already owns and serves the serial application log; Barista should proxy that capability rather than reimplement storage or tailing.

## What changes

- Add an additive server-streaming `WatchLogs` Contract A RPC for one instance.
- Expose only the substrate's application/serial log, never VMM or substrate-operator logs.
- Support a bounded historical tail and optional follow mode.
- Preserve log lines as opaque bytes and keep authorization at the existing node boundary.
- Implement Hypeman by proxying its authenticated SSE log endpoint and implement an honest fake-runtime stream for tests.
- Add `barista logs [--follow] [--tail N] <instance>`.

## What does not change

- Logs are not lifecycle events and receive no event cursor or replay guarantee.
- Barista does not copy logs into its SQLite operation journal.
- Barista does not redact workload output; an authorized reader sees what its workload emitted.
- This does not add metrics or tracing.

## Acceptance

- Contract descriptors and generated clients include the additive RPC and messages.
- A Hypeman-backed test returns bounded historical application lines and follows a new line.
- Invalid bounds are refused before reaching the substrate.
- CLI tests prove historical and follow rendering without changing operation-follow semantics.
- `make check` passes.
