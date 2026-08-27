# Guest Agent API

`barista.guest.v1alpha1.GuestAgent` is the contract between a node and the daemon
inside its sandboxes. It is **internal**: the Node Agent is its only client, and
it is never exposed to you directly.

It is documented because its semantics are visible through the Node Agent API —
readiness, hook outcomes, restore drift — and because knowing what it does tells
you what your image has to cooperate with.

## Service

```protobuf
service GuestAgent {
  rpc Health(HealthRequest) returns (HealthResponse);
  rpc Exec(stream ExecFrame) returns (stream ExecFrame);
  rpc ReadFile(ReadFileRequest) returns (stream FileChunk);
  rpc WriteFile(stream WriteFileRequest) returns (WriteFileResponse);
  rpc StatPath(StatPathRequest) returns (StatPathResponse);
  rpc RunHook(RunHookRequest) returns (RunHookResponse);
  rpc RunRestoreDuties(RestoreDutiesRequest) returns (RestoreDutiesResponse);
}

// The one surface the *workload* calls, on a separate in-sandbox unix socket
// whose path is injected as BARISTA_WORKLOAD_SOCKET (barista-031).
service WorkloadService {
  rpc DeclareIdle(DeclareIdleRequest) returns (DeclareIdleResponse);
}
```

`WorkloadService` is served on its own socket, unauthenticated (caller and agent
share the sandbox's one trust domain), and carries only `DeclareIdle` — the
management RPCs above are not reachable on it. The agent records the declaration
and reports it as `HealthResponse.idle_declared`; the Node Agent decides what to
do with it (`InstanceSpec.idle_action`). See
[the idle hint](../concepts/sleep-and-wake.md) and the
[guest agent concept](../concepts/guest-agent.md).

## What an `Exec` command's environment contains

Three layers, applied in this order. The order is the contract, because each
step is what makes the next one safe to state:

1. **The bootstrap scrub.** Every variable in the bootstrap channel — the
   instance token, the guest and workload socket paths, the TCP port, the three
   TLS file paths, the encoded process and hooks specs — is removed. That
   environment is the *agent's*, not its children's (barista-043).
2. **The workload's `Process.env`.** An `Exec` runs *in* the session, not next
   to it, so it observes the same environment as the process already running
   there. This is where a platform resolves an app's declared secrets — a
   delegated grant among them — and it is how a client reads back a credential
   the provider resolved for it (barista-071).
3. **The request's `ExecStart.env`.** Applied last, so a variable the caller
   names explicitly is delivered unchanged: the authenticated request is the
   host speaking, and step 1 removes an inherited default, not an explicit
   grant.

Step 2 is not a new exposure. An `Exec` runs same-uid with the workload, so
`/proc/<workload>/environ` was already readable to it; what changed is that the
value is delivered rather than recovered. The asymmetry with step 1 is
deliberate and holds in both directions: the **workload** is untrusted code that
must not acquire the agent's credentials by default, whereas an **`Exec`** is
the host re-entering a session it owns to read values the host itself put there.

`BARISTA_WORKLOAD_SOCKET` is injected for the workload alone, after its spec
env, so it is not part of `Process.env` and does not reach an exec'd command. An
exec'd command has no contract claim on the idle-declaration surface.

## Transport and bootstrap

The agent authenticates with a per-session token carried in gRPC metadata
(`barista-instance-token`), and, on the `hypeman` transport, per-instance
mutual TLS. Connection direction is transport-dependent: on `hypeman` the
agent binds a TCP listener inside the VM (port 7071) that the host dials; on
`fake` (and the deferred `runsc` path) the host reaches it through an exec
bridge or unix socket, with no inbound network port.

| Runtime | Status | Injection and channel |
|---|---|---|
| `hypeman` | Implemented | Guest binary and credential volume at sandbox create; host dials the guest's in-VM listener (port 7071). |
| `fake` | Implemented for tooling | Entrypoint wrapper and Docker exec bridge; no inbound listener. |
| `runsc` | Deferred | The transport shape is reserved for the rank-2 tier; no backend is implemented. |

The token is a credential with a lifecycle: its volume is created with the
session, tagged to the owning node, and reaped when the session goes away —
including when the session never made it into the journal.

## Health and readiness

```protobuf
message HealthResponse {
  bool alive = 1;
  bool ready = 2;                    // last ready_cmd verdict
  int32 ready_cmd_exit = 3;
  google.protobuf.Timestamp last_user_activity = 4;
  google.protobuf.Timestamp guest_time = 5;   // for clock-drift metrics
  google.protobuf.Timestamp idle_declared = 6; // last DeclareIdle, else absent
}
```

`last_user_activity` is the guest's own activity clock, which is what TTL
decisions are made against. `idle_declared` carries the workload's last
`DeclareIdle`; the Node Agent guards it against both the run epoch and
`last_user_activity` before acting.

## Hooks

```protobuf
message RunHookRequest {
  HookKind kind = 1;                 // PRE_SNAPSHOT | POST_RESTORE
  uint32 timeout_ms = 2;
}

message RunHookResponse {
  bool ran = 1;                      // false when no hook is configured
  bool timed_out = 2;
  int32 exit_code = 3;
  string stdout_tail = 4;
  string stderr_tail = 5;
}
```

The outcome of `PRE_SNAPSHOT` is recorded on the `Snapshot` record, so you can
tell after the fact whether a snapshot was taken over a quiesced workload or a
timed-out one.

## Restore duties

This is a separate RPC from `RunHook` for a specific reason: `RunHook` runs
*your* commands and cannot carry host-supplied material.

```protobuf
message RestoreDutiesRequest {
  bytes entropy = 1;                          // fresh host CSPRNG bytes — required
  google.protobuf.Timestamp host_time = 2;    // step the guest clock to this
}

message RestoreDutiesResponse {
  uint32 entropy_bytes_mixed = 1;
  bool entropy_credited = 2;                  // credited, or only mixed
  int64 clock_drift_ms = 3;                   // guest minus host, before the step
  bool clock_stepped = 4;
  string degraded = 5;                        // empty when every duty ran as intended
}
```

Ordering is normative: duties run **before** `POST_RESTORE`, so your reconnect
command already sees fresh entropy and a stepped clock.

`entropy` is required. A reseed with nothing to mix cannot de-duplicate two
restores of one snapshot, so the agent rejects the request rather than reporting
success. Reseeding forces a CRNG reseed as well as mixing, because a restored
guest's CRNG key and reseed timer come back byte-identical — mixing alone leaves
the first draws repeatable.

`entropy_credited` and `clock_stepped` are reported separately from `degraded`
so a sandbox that lacks the capability to credit entropy or set the clock says
exactly which duty it could not perform.

## Related

- [The guest agent](../concepts/guest-agent.md)
- [Snapshots](../concepts/snapshots.md)
- [Node Agent API](index.md)
