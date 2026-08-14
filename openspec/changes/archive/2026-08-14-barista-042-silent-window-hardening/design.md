## Context

See proposal.md — Why. Two silent windows, both review follow-ups (2026-08-14,
M3 and M4 residuals), both one-enforcement-point.

Constraints that shape the approach:

- **The non-destructive rule is ratified and stays.** `fleet-coordination`
  requires that coordination unavailability keep already-owned sessions running
  and destroy nothing. Whatever this change adds must be observability only —
  a report, never an action.
- **Renewals are the only witness of expiry risk.** A lease expires at
  `last successful renewal + TTL`, and renewal is the first thing every pass
  does. So "how long has the bucket been unreachable *for renewals*" is the
  quantity that decides whether takeover is possible — not listing failures,
  not acquisition failures, which can occur while renewals are landing fine.
- **The guest's file surface has exactly one unbounded wait.** `write_file`
  loops `inbound.next().await`; everything else on the management socket is
  request/response or server-streamed. `Exec` also reads an inbound stream, but
  its idleness is legitimate (below).
- **`tonic::Streaming` cannot be synthesized in a test.** `exec.rs` solved this
  deliberately: `serve` is generic over a `Stream` bound (`ClientFrames`),
  because synthesising endings — a break, a half-close — is the only way to
  test them. A stream that goes quiet is exactly such an ending.

## Goals / Non-Goals

**Goals:**
- A partition that outlasts the lease TTL produces a degradation event per held
  session, naming the session, the possibility of a second owner, and the
  ratified reason the node keeps running — once per episode, re-armed by the
  first successful renewal.
- A `WriteFile` stream whose frame gap exceeds the bound fails with a status
  that says the stream went quiet, distinguishable from a slow upload; the file
  handle and RPC are released.
- Both thresholds argue for themselves: the TTL because it is the exact moment
  takeover becomes legal, the 60 s frame gap in its constant's own comment.

**Non-Goals:**
- Any destructive response to a partition (self-fence, release, pause). See D1.
- Any size bound on `WriteFile`. See D3.
- Any inactivity bound on `Exec`. See D4.
- Persisting the episode across an agent restart. See D2.

## Decisions

### D1 — Report at exactly the TTL; never act. The K×TTL auto-self-fence is rejected

The threshold is `Timing.ttl`, the fleet's own number: the TTL is the earliest
moment another node may legally take over a name this node failed to renew, so
a smaller constant would cry wolf and a larger one would miss real dual
execution. No new knob is introduced. The episode clock starts at the first
*failed* renewal; the last successful renewal was strictly earlier, so when the
report fires the lease has certainly been expired rather than merely
might-have-been — the conservative direction for an alarm that names a serious
condition.

The rejected alternative was to *act*: after K×TTL of unreachability, stop the
local workloads on the theory that someone else probably owns them now. A node
cannot distinguish a global bucket outage — where no node can acquire anything,
takeover is impossible, and self-fencing would stop every session in the fleet
for zero safety gain — from an asymmetric partition where it alone is cut off.
The existing machinery already handles the dangerous half correctly: ETag
fencing refuses the superseded node's writes with no clock agreement, and the
first renewal after the partition heals comes back `Fenced`, which stops the
workload. What was missing was only the signal, so the signal is all this
change adds.

**Once per episode, reset on contact.** `Renewed::Held` and `Renewed::Fenced`
both end the episode — a refusal is still an answer, and an answering bucket
means renewals are landing again. The reset is what makes a second partition
report afresh instead of inheriting the first one's "already said it".

### D2 — Episode state lives on `Fleet`, in memory, behind a pure transition rule

The state is two fields (`since_ms`, `reported`) in a `Mutex<Option<Outage>>`
on the `Fleet` struct — the same shape and lifetime as `holds_reported`, and
for the same reason: report on change, not on schedule. The transition itself
is a pure function (`outage_after_renewals`), so the threshold semantics are a
table a unit test pins without a bucket or a real clock, in the tradition of
`intent_for` and `release_intent`.

Deliberately not journaled. A restarted agent that is still partitioned fails
its first renewal within one pass and starts a fresh episode; the worst case is
one extra event per session after a restart-during-partition, and the
alternative — journaling coordination state — would give the record a second
author. In-memory is also what makes "reset on contact" trivially correct.

### D3 — A per-frame-gap timeout on `WriteFile`; the byte cap is rejected

Every `inbound.next()` in `write_file` is wrapped in a 60 s timeout. The timer
restarts on every frame, so it bounds *silence*, not size or total duration: a
multi-GB upload that keeps sending chunks never meets it, however slow the
link. The constant's justification lives on the constant, in the tradition of
`DEFAULT_HOOK_TIMEOUT` and `DRAIN_GRACE`.

The rejected alternative was a byte cap. The sandbox's own disk budget already
bounds what a write can consume, and ENOSPC reports the overrun through the
existing `io_status` path with the filesystem's own authority; a second,
invented number would duplicate that bound less honestly and break legitimate
large uploads at whatever value was guessed.

**Status code: `DEADLINE_EXCEEDED`, not `ABORTED`.** What expired is literally
a time bound — the server's inactivity deadline on a stream whose sender
stopped participating — and a caller's generic handling of `DEADLINE_EXCEEDED`
(report, give up) is the right handling here. `ABORTED` conventionally invites
retrying a concurrency conflict that does not exist. The message says the
stream went *quiet* and names the per-frame-gap rule, so a slow-but-progressing
caller reading the error knows it was not about speed.

**The partial file is the existing contract, stated.** Bytes received before
the timeout are on disk, exactly as they would be after a mid-stream transport
error today; the abort adds no new failure shape, it only converts "forever"
into "60 s, then the same shape a broken connection has always had".

### D4 — `Exec` is deliberately excluded

An interactive PTY session is legitimately idle for long stretches — a human
thinking at a shell prompt is not a wedged stream, and killing it after a quiet
minute would break the surface's primary use. `Exec` also already has both of
its endings handled (`InputEnd::PeerFinished` / `Broken`), and holds no file
handle. The bound applies only where silence is indistinguishable from
abandonment and a resource is pinned: the write path.

### D5 — Testability: extract the body, keep the tonic shim

`write_file`'s body moves to a helper generic over
`Stream<Item = Result<WriteFileRequest, Status>>`; the trait method becomes a
thin wrapper over `r.into_inner()`. This is `exec::serve`'s precedent applied
to the file path: the quiet ending is synthesized as a finite stream chained to
`futures_util::stream::pending()`, and `#[tokio::test(start_paused = true)]`
fires the 60 s timer without real waiting. The fleet piece is tested with an
in-memory store that can be partitioned on demand (a wrapper whose methods fail
while a flag is set) — `fleet_release.rs`'s in-memory reasoning, extended to
the one condition a real backend cannot be told to produce.

## Risks / Trade-offs

- **Event volume during a long partition.** → Bounded by construction: one
  event per held session per episode, not per pass. A fleet-wide outage on a
  node holding N sessions emits N events, once.
- **A false alarm during a global outage.** → Accepted and worded for: the
  message says the lease *may* have expired and another node *may* own the
  name — during a global outage nobody can take anything, and the event is
  still true and still useful (the operator learns the node is cut off).
- **A legitimate WriteFile caller slower than one frame per 60 s.** → No such
  caller exists (the CLI streams a local file; chunks are 64 KiB), and one that
  appeared would be indistinguishable from an abandoned stream by any local
  evidence. The error names the rule so the failure is diagnosable.
- **`start_paused` with real file I/O.** → The timer is only armed around
  `inbound.next()`, never around a file operation, so auto-advanced time cannot
  fire it while a write is genuinely in flight.

## Migration Plan

1. Artifacts (this change), then code: fleet episode + event, guest timeout +
   extraction, tests beside each.
2. No schema, proto, or on-disk change anywhere; rollback is a straight revert.
3. Docker-gated pieces of `make check` (guest musl build, MinIO-backed fleet
   tests) self-skip locally and run in CI, as they do for every change.
