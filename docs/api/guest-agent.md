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

## Transport and bootstrap

The agent dials the host and authenticates with a per-session token carried in
gRPC metadata (`barista-instance-token`). **It never accepts inbound connections.**

| Runtime | Status | Injection and channel |
|---|---|---|
| `hypeman` | Implemented | Guest binary and credential volume at sandbox create; outbound runtime-provided channel. |
| `fake` | Implemented for tooling | Entrypoint wrapper and outbound Docker bridge. |
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
