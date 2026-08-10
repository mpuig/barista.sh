# Design: barista-030-instance-endpoint

## Context

See `proposal.md — Why` (F1, `docs/ax-consumer-evidence.md` §Q1). Current
state that shapes the approach:

- The hypeman guest channel already resolves the sandbox address **per
  connect** from the substrate (`runtime/hypeman/channel.rs`: "the address is
  re-resolved per connect"), deliberately not cached — an address can change
  across restore.
- `fake`'s guest channel is not network-based at all
  (`channel_is_network_reachable() == false`; it reaches the agent over a
  Docker-exec-style transport against `http://guest.invalid`).
- `Instance` (node.proto) ends at field 10 (`stop_reason`); field 11 is free.
- CLI `--json` emits proto field names, so the field surfaces with zero CLI
  work.

## Goals / Non-Goals

**Goals**

- A consumer co-located with the node (Contract A is loopback-only) can learn
  the address its workload is dialable at, from `GetInstance`/`ListInstances`
  alone, while the instance runs.
- Absence is meaningful: no address ⇒ this runtime/state provides none.

**Non-Goals**

- Ports. Barista does not know which port a workload listens on; the consumer
  does (evidence §Q1 — AX knew its own 50053).
- Cross-host reachability, tunnels, or the gateway (§I non-goal, Phase 3+).
- Readiness semantics: `ready` already exists and is orthogonal.
- Exposing `fake`'s container IP (see Decision 3).

## Decisions

1. **A field on `Instance`, not a new RPC.** `InstanceNetwork network = 11`
   with a single `string address = 1` (extensible message rather than a bare
   string, so a future field — e.g. a gateway-published name — does not force
   field churn). *Simpler alternative named:* bare `string workload_address`;
   rejected because a one-field message costs nothing now and F1's history
   says this surface will grow. *Composite alternative named:* a
   `WaitReady`-returns-endpoint RPC (annex recommendation 1's second shape);
   rejected for now — consumers already poll `get` for `ready`, and a field
   serves both the poller and the watcher without a new verb.

2. **Resolved live at read time, like the channel does.** `service.rs`
   populates `network` on `GetInstance`/`ListInstances` by calling the new
   trait method when (and only when) the journal state is `RUNNING`; the
   method asks the substrate per call. No caching, no new journal column —
   the journal stores facts this node authored; the address is the
   substrate's fact. *Simpler alternative named:* cache at `start`/`resume`;
   rejected — invalidation across restore is exactly the bug the channel's
   per-connect resolution already refuses to have. Cost: one local substrate
   HTTP GET per `get` on hypeman (the same call the channel makes per
   connect); `ListInstances` bounds it with a small concurrent fan-out.

3. **`fake` reports nothing, deliberately.** Its container IP is real on a
   Linux node and unreachable from a macOS node host — a field that is true on
   one platform and a lie on the other is the silent degradation §I forbids.
   The trait default (`Ok(None)`) is the implementation. *Simpler alternative
   named:* report `docker inspect`'s IP anyway (the spike used it on Linux);
   rejected — `fake` is tooling-only, and consumers coding against it would
   ship a macOS-breaking dependency.

4. **Trait method, not capability flag.** `async fn workload_address(&self,
   h: &Handle) -> Result<Option<String>>`, default `Ok(None)`. No new
   `RuntimeCapabilities` bit: the signal is per-instance and per-moment
   (paused ⇒ none), not per-runtime — a static capability would over-claim.
   `channel_is_network_reachable()` stays what it is (a channel property);
   this is a workload property.

5. **Substrate errors degrade to absence with a log, not a failed Get.** A
   `GetInstance` must not start failing because one enrichment call failed;
   the field goes absent and the WARN names the reason. The consumer's
   contract is "absent means unavailable", which stays true.

## Risks / Trade-offs

- [Read amplification on `ListInstances` for hypeman nodes with many running
  instances] → bounded concurrent fan-out (same pattern reconcile uses for
  probes); measured before merge; if it bites, the escape hatch is populating
  only on `GetInstance` (documented in the delta spec as permitted).
- [Address changes across a resume while a consumer holds the old one] → the
  same reality the guest channel handles; the field is documented as
  "current as of this read"; consumers re-read after resume (AX's per-turn
  dial pattern does this naturally).
- [Tests need a running hypeman] → hypeman-gated like the rest
  (`BARISTA_TEST_RUNTIME=hypeman`); CI's permitted-skip list already covers
  it; the fake-negative and state-gating tests run everywhere.

## Migration Plan

Additive proto change: regenerate (`task proto`), descriptor diff reviewed as
additive-only. No data migration; no consumer breaks (absent field decodes as
default). Rollback is removing the field before any release pins it.

## Open Questions

- none.
