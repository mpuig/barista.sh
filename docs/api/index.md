# Node Agent API

The gRPC service every client talks to: `barista.node.v1alpha1.NodeAgent`, served
over TCP or a unix domain socket. The `barista` CLI is a thin client over it, and so
is everything else.

The protobuf schema is the single source of truth. Generated clients exist for
Rust and Python; any gRPC-capable language works.

```sh
grpcurl -plaintext 127.0.0.1:7070 list barista.node.v1alpha1.NodeAgent
```

## Service

### Identity and capabilities

| RPC | Returns |
|---|---|
| `GetNodeInfo(GetNodeInfoRequest)` | `NodeInfo` |

### Lifecycle

Lifecycle mutations are asynchronous, idempotent operations except `SetWake`,
which is a last-writer-wins assignment returning the updated `Instance`.

| RPC | Notes |
|---|---|
| `CreateInstance(CreateInstanceRequest)` | Takes the spec, an idempotency key, and `require_hardware_isolation`. |
| `StartInstance(StartInstanceRequest)` | From `CREATED` or `STOPPED`. From `STOPPED` this is a cold boot. |
| `StopInstance(StopInstanceRequest)` | `grace_seconds` before kill. |
| `PauseInstance(PauseInstanceRequest)` | `keep_memory` (unset means true), `require_memory`. |
| `ResumeInstance(ResumeInstanceRequest)` | Target is `instance_id` (latest snapshot) or `snapshot_id`. |
| `CheckpointInstance(CheckpointInstanceRequest)` | Live snapshot. Requires `live_checkpoint`. |
| `CreateSnapshot(CreateSnapshotRequest)` | Retained, optionally named. Declares `froze_workload` when the source was running. |
| `DestroyInstance(DestroyInstanceRequest)` | `keep_snapshots`. |
| `SetWake(SetWakeRequest)` | Absolute wake timestamp; unset clears the alarm. |

### Introspection

| RPC | Returns |
|---|---|
| `GetInstance(GetInstanceRequest)` | `Instance` |
| `ListInstances(ListInstancesRequest)` | Filter by `states` and `label_selector`. |
| `ListSnapshots(ListSnapshotsRequest)` | Empty `instance_id` lists the whole node. |
| `DeleteSnapshot(DeleteSnapshotRequest)` | Returns an `Operation`. |
| `GetOperation(GetOperationRequest)` | `Operation` |
| `WatchEvents(WatchEventsRequest)` | `stream Event` |

### Guest passthrough

| RPC | Notes |
|---|---|
| `Exec(stream ExecFrame) → stream ExecFrame` | Bidirectional: stdio, resize, exit status. One stream per exec. |
| `ReadFile(ReadFileRequest) → stream FileChunk` | `offset`, `limit` (0 = to EOF). |
| `WriteFile(stream WriteFileRequest) → WriteFileResponse` | First frame is `WriteOpen`. |

Passthrough calls count as user activity and reset the TTL.

## Messages

### InstanceSpec

Immutable after create.

```protobuf
message InstanceSpec {
  string instance_id = 1;            // client-chosen ULID, unique per node
  TemplateRef template = 2;
  Resources resources = 3;           // vcpu, mem_mib, disk_mib
  Process process = 4;               // start_cmd, ready_cmd, env, workdir
  Hooks hooks = 5;                   // pre_snapshot_cmd, post_restore_cmd + timeouts
  uint64 ttl_seconds = 6;            // 0 = no TTL; reset on activity
  TtlAction ttl_action = 7;          // PAUSE (default) | STOP | DESTROY
  map<string, string> labels = 8;
  EgressPolicy egress = 9;
}
```

### TemplateRef

```protobuf
message TemplateRef {
  OciImageRef oci = 1;
  string runtime_bundle_ref = 3;     // pinned; must match exactly on resume
  string template_hash = 4;
  string arch = 5;                   // aarch64 | x86_64
}

message OciImageRef {
  string image = 1;                  // human-readable label; not identity
  string digest = 2;                 // sha256:… — required
}
```

An empty `digest` is rejected with `INVALID_SPEC`.

### Instance

```protobuf
message Instance {
  InstanceSpec spec = 1;
  InstanceState state = 2;
  bool ready = 3;                    // ready_cmd result — not a state
  string runtime = 4;
  google.protobuf.Timestamp created_at = 5;
  google.protobuf.Timestamp updated_at = 6;
  google.protobuf.Timestamp ttl_deadline = 7;   // absent when no TTL
  string latest_snapshot_id = 8;
  google.protobuf.Timestamp wake_at = 9;        // absent when no alarm
}
```

### Snapshot

```protobuf
message Snapshot {
  string snapshot_id = 1;
  string instance_id = 2;
  SnapshotKind kind = 3;             // MEMORY_AND_DISK | DISK_ONLY
  string cpu_class = 4;              // restore-compat key
  string template_hash = 5;          // invalidation key
  string runtime_bundle_ref = 6;     // must match exactly on resume
  SnapshotTier tier = 7;             // LOCAL today; OBJECT_STORE is reserved
  uint64 size_bytes = 8;
  google.protobuf.Timestamp created_at = 9;
  HookOutcome pre_snapshot_hook = 10;
  string name = 11;                  // set for named snapshots
}
```

### Operation

```protobuf
message Operation {
  string op_id = 1;
  string kind = 2;                   // "create" | "start" | "pause" | …
  string instance_id = 3;
  OperationState state = 4;          // QUEUED | RUNNING | DONE | FAILED
  string current_step = 5;
  ErrorDetail error = 6;             // set when FAILED
  string degraded = 7;               // set when it succeeded through a downgrade
  google.protobuf.Timestamp created_at = 8;
  google.protobuf.Timestamp finished_at = 9;
}
```

`degraded` is never empty for a downgraded success, and a downgrade always also
emits a `DEGRADATION` event.

### NodeInfo

```protobuf
message NodeInfo {
  string node_id = 1;
  string arch = 2;
  string cpu_class = 3;              // snapshot restore-compat key
  repeated RuntimeInfo runtimes = 4;
  Resources total_resources = 5;
  Resources allocatable_resources = 6;
  string agent_version = 7;
  FleetInfo fleet = 8;               // bucket configured, leases held
}

message RuntimeCapabilities {
  bool memory_snapshot = 1;
  bool disk_snapshot = 2;
  bool live_checkpoint = 3;
  bool guest_agent = 4;
  bool hardware_isolation = 5;
  bool lazy_restore = 6;
  bool cow_fork = 7;
  bool egress_control = 8;
}
```

`RuntimeInfo.health` is an enum, not a bool: `HEALTHY`, `UNREACHABLE`, or
`UNSPECIFIED` for an agent too old to report it. `UNREACHABLE` constrains
mutations only — running sessions keep running and keep being reported as
running.

### Event

```protobuf
message Event {
  uint64 cursor = 1;                 // monotonic per node
  EventType type = 2;
  string instance_id = 3;
  string op_id = 4;
  InstanceState state = 5;           // for STATE_CHANGED
  string message = 6;
  google.protobuf.Timestamp at = 7;
}
```

`WatchEventsRequest.from_cursor` of 0 means new events only. A non-zero cursor
replays everything after it, or fails with `CURSOR_TOO_OLD` if retention has
already deleted those events.

## Enums

**`InstanceState`** — `CREATING`, `CREATED`, `STARTING`, `RUNNING`,
`CHECKPOINTING`, `PAUSING`, `PAUSED`, `RESUMING`, `STOPPING`, `STOPPED`,
`DESTROYING`, `DESTROYED`, `FAILED`.

**`OperationState`** — `QUEUED`, `RUNNING`, `DONE`, `FAILED`.

**`SnapshotKind`** — `MEMORY_AND_DISK`, `DISK_ONLY`.

**`SnapshotTier`** — `LOCAL`, `OBJECT_STORE`.

**`TtlAction`** — `PAUSE` (the default when unspecified), `STOP`, `DESTROY`.

**`EventType`** — `STATE_CHANGED`, `OPERATION_PROGRESS`, `READY_CHANGED`,
`TTL_WARNING`, `DEGRADATION`, `WAKE_FIRED`, `RESTORED`, `FENCED`.

**`SubstrateHealth`** — `HEALTHY`, `UNREACHABLE`, `UNSPECIFIED`.

**`ErrorReason`** — see [Errors](errors.md).

## Versioning

The package is `barista.node.v1alpha1`. Breaking changes are gated by `buf
breaking`: fields are reserved rather than reused, and a break requires explicit
ratification. Hand-written duplicates of contract types are not supported —
generate your client from the schema.

## Related

- [Guest Agent API](guest-agent.md)
- [Errors](errors.md)
- [CLI commands](../cli.md)
