import datetime

from google.protobuf import timestamp_pb2 as _timestamp_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class SubstrateHealth(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    SUBSTRATE_HEALTH_UNSPECIFIED: _ClassVar[SubstrateHealth]
    SUBSTRATE_HEALTH_HEALTHY: _ClassVar[SubstrateHealth]
    SUBSTRATE_HEALTH_UNREACHABLE: _ClassVar[SubstrateHealth]

class EgressMode(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    EGRESS_MODE_UNSPECIFIED: _ClassVar[EgressMode]
    EGRESS_MODE_ALL: _ClassVar[EgressMode]
    EGRESS_MODE_HTTP_HTTPS_ONLY: _ClassVar[EgressMode]

class TtlAction(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    TTL_ACTION_UNSPECIFIED: _ClassVar[TtlAction]
    TTL_ACTION_PAUSE: _ClassVar[TtlAction]
    TTL_ACTION_STOP: _ClassVar[TtlAction]
    TTL_ACTION_DESTROY: _ClassVar[TtlAction]

class InstanceState(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    INSTANCE_STATE_UNSPECIFIED: _ClassVar[InstanceState]
    INSTANCE_STATE_CREATING: _ClassVar[InstanceState]
    INSTANCE_STATE_CREATED: _ClassVar[InstanceState]
    INSTANCE_STATE_STARTING: _ClassVar[InstanceState]
    INSTANCE_STATE_RUNNING: _ClassVar[InstanceState]
    INSTANCE_STATE_CHECKPOINTING: _ClassVar[InstanceState]
    INSTANCE_STATE_PAUSING: _ClassVar[InstanceState]
    INSTANCE_STATE_PAUSED: _ClassVar[InstanceState]
    INSTANCE_STATE_RESUMING: _ClassVar[InstanceState]
    INSTANCE_STATE_STOPPING: _ClassVar[InstanceState]
    INSTANCE_STATE_STOPPED: _ClassVar[InstanceState]
    INSTANCE_STATE_DESTROYING: _ClassVar[InstanceState]
    INSTANCE_STATE_DESTROYED: _ClassVar[InstanceState]
    INSTANCE_STATE_FAILED: _ClassVar[InstanceState]

class SnapshotKind(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    SNAPSHOT_KIND_UNSPECIFIED: _ClassVar[SnapshotKind]
    SNAPSHOT_KIND_MEMORY_AND_DISK: _ClassVar[SnapshotKind]
    SNAPSHOT_KIND_DISK_ONLY: _ClassVar[SnapshotKind]

class SnapshotTier(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    SNAPSHOT_TIER_UNSPECIFIED: _ClassVar[SnapshotTier]
    SNAPSHOT_TIER_LOCAL: _ClassVar[SnapshotTier]
    SNAPSHOT_TIER_OBJECT_STORE: _ClassVar[SnapshotTier]

class OperationState(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    OPERATION_STATE_UNSPECIFIED: _ClassVar[OperationState]
    OPERATION_STATE_QUEUED: _ClassVar[OperationState]
    OPERATION_STATE_RUNNING: _ClassVar[OperationState]
    OPERATION_STATE_DONE: _ClassVar[OperationState]
    OPERATION_STATE_FAILED: _ClassVar[OperationState]

class ErrorReason(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    ERROR_REASON_UNSPECIFIED: _ClassVar[ErrorReason]
    ERROR_REASON_INVALID_SPEC: _ClassVar[ErrorReason]
    ERROR_REASON_TEMPLATE_NOT_FOUND: _ClassVar[ErrorReason]
    ERROR_REASON_BUNDLE_MISMATCH: _ClassVar[ErrorReason]
    ERROR_REASON_CPU_CLASS_MISMATCH: _ClassVar[ErrorReason]
    ERROR_REASON_CAPABILITY_MISSING: _ClassVar[ErrorReason]
    ERROR_REASON_CONCURRENT_OPERATION: _ClassVar[ErrorReason]
    ERROR_REASON_GUEST_UNREACHABLE: _ClassVar[ErrorReason]
    ERROR_REASON_HOOK_TIMEOUT: _ClassVar[ErrorReason]
    ERROR_REASON_RESOURCES_EXHAUSTED: _ClassVar[ErrorReason]
    ERROR_REASON_SNAPSHOT_INVALIDATED: _ClassVar[ErrorReason]
    ERROR_REASON_SUBSTRATE_UNAVAILABLE: _ClassVar[ErrorReason]
    ERROR_REASON_CURSOR_TOO_OLD: _ClassVar[ErrorReason]
    ERROR_REASON_SNAPSHOT_NAME_CONFLICT: _ClassVar[ErrorReason]
    ERROR_REASON_FORK_MODE_UNAVAILABLE: _ClassVar[ErrorReason]
    ERROR_REASON_CAPSULE_VERIFICATION_FAILED: _ClassVar[ErrorReason]
    ERROR_REASON_CAPSULE_INCOMPATIBLE: _ClassVar[ErrorReason]
    ERROR_REASON_CAPSULE_NOT_FOUND: _ClassVar[ErrorReason]
    ERROR_REASON_OBJECT_STORE_UNAVAILABLE: _ClassVar[ErrorReason]
    ERROR_REASON_EPOCH_REVOKED: _ClassVar[ErrorReason]

class EventType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    EVENT_TYPE_UNSPECIFIED: _ClassVar[EventType]
    EVENT_TYPE_STATE_CHANGED: _ClassVar[EventType]
    EVENT_TYPE_OPERATION_PROGRESS: _ClassVar[EventType]
    EVENT_TYPE_READY_CHANGED: _ClassVar[EventType]
    EVENT_TYPE_TTL_WARNING: _ClassVar[EventType]
    EVENT_TYPE_DEGRADATION: _ClassVar[EventType]
    EVENT_TYPE_RESTORED: _ClassVar[EventType]
    EVENT_TYPE_WAKE_FIRED: _ClassVar[EventType]
    EVENT_TYPE_FENCED: _ClassVar[EventType]
    EVENT_TYPE_IDLE_FIRED: _ClassVar[EventType]
    EVENT_TYPE_LINEAGE_RECORDED: _ClassVar[EventType]
    EVENT_TYPE_EPOCH_ROTATED: _ClassVar[EventType]

class ForkMode(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    FORK_MODE_UNSPECIFIED: _ClassVar[ForkMode]
    FORK_MODE_COW: _ClassVar[ForkMode]
    FORK_MODE_FULL_COPY: _ClassVar[ForkMode]

class CapsuleStorage(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    CAPSULE_STORAGE_UNSPECIFIED: _ClassVar[CapsuleStorage]
    CAPSULE_STORAGE_LOCAL_DIR: _ClassVar[CapsuleStorage]
    CAPSULE_STORAGE_OBJECT_STORE: _ClassVar[CapsuleStorage]

class CapsuleObjectType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    CAPSULE_OBJECT_TYPE_UNSPECIFIED: _ClassVar[CapsuleObjectType]
    CAPSULE_OBJECT_TYPE_MEMORY: _ClassVar[CapsuleObjectType]
    CAPSULE_OBJECT_TYPE_DISK: _ClassVar[CapsuleObjectType]
    CAPSULE_OBJECT_TYPE_METADATA: _ClassVar[CapsuleObjectType]
SUBSTRATE_HEALTH_UNSPECIFIED: SubstrateHealth
SUBSTRATE_HEALTH_HEALTHY: SubstrateHealth
SUBSTRATE_HEALTH_UNREACHABLE: SubstrateHealth
EGRESS_MODE_UNSPECIFIED: EgressMode
EGRESS_MODE_ALL: EgressMode
EGRESS_MODE_HTTP_HTTPS_ONLY: EgressMode
TTL_ACTION_UNSPECIFIED: TtlAction
TTL_ACTION_PAUSE: TtlAction
TTL_ACTION_STOP: TtlAction
TTL_ACTION_DESTROY: TtlAction
INSTANCE_STATE_UNSPECIFIED: InstanceState
INSTANCE_STATE_CREATING: InstanceState
INSTANCE_STATE_CREATED: InstanceState
INSTANCE_STATE_STARTING: InstanceState
INSTANCE_STATE_RUNNING: InstanceState
INSTANCE_STATE_CHECKPOINTING: InstanceState
INSTANCE_STATE_PAUSING: InstanceState
INSTANCE_STATE_PAUSED: InstanceState
INSTANCE_STATE_RESUMING: InstanceState
INSTANCE_STATE_STOPPING: InstanceState
INSTANCE_STATE_STOPPED: InstanceState
INSTANCE_STATE_DESTROYING: InstanceState
INSTANCE_STATE_DESTROYED: InstanceState
INSTANCE_STATE_FAILED: InstanceState
SNAPSHOT_KIND_UNSPECIFIED: SnapshotKind
SNAPSHOT_KIND_MEMORY_AND_DISK: SnapshotKind
SNAPSHOT_KIND_DISK_ONLY: SnapshotKind
SNAPSHOT_TIER_UNSPECIFIED: SnapshotTier
SNAPSHOT_TIER_LOCAL: SnapshotTier
SNAPSHOT_TIER_OBJECT_STORE: SnapshotTier
OPERATION_STATE_UNSPECIFIED: OperationState
OPERATION_STATE_QUEUED: OperationState
OPERATION_STATE_RUNNING: OperationState
OPERATION_STATE_DONE: OperationState
OPERATION_STATE_FAILED: OperationState
ERROR_REASON_UNSPECIFIED: ErrorReason
ERROR_REASON_INVALID_SPEC: ErrorReason
ERROR_REASON_TEMPLATE_NOT_FOUND: ErrorReason
ERROR_REASON_BUNDLE_MISMATCH: ErrorReason
ERROR_REASON_CPU_CLASS_MISMATCH: ErrorReason
ERROR_REASON_CAPABILITY_MISSING: ErrorReason
ERROR_REASON_CONCURRENT_OPERATION: ErrorReason
ERROR_REASON_GUEST_UNREACHABLE: ErrorReason
ERROR_REASON_HOOK_TIMEOUT: ErrorReason
ERROR_REASON_RESOURCES_EXHAUSTED: ErrorReason
ERROR_REASON_SNAPSHOT_INVALIDATED: ErrorReason
ERROR_REASON_SUBSTRATE_UNAVAILABLE: ErrorReason
ERROR_REASON_CURSOR_TOO_OLD: ErrorReason
ERROR_REASON_SNAPSHOT_NAME_CONFLICT: ErrorReason
ERROR_REASON_FORK_MODE_UNAVAILABLE: ErrorReason
ERROR_REASON_CAPSULE_VERIFICATION_FAILED: ErrorReason
ERROR_REASON_CAPSULE_INCOMPATIBLE: ErrorReason
ERROR_REASON_CAPSULE_NOT_FOUND: ErrorReason
ERROR_REASON_OBJECT_STORE_UNAVAILABLE: ErrorReason
ERROR_REASON_EPOCH_REVOKED: ErrorReason
EVENT_TYPE_UNSPECIFIED: EventType
EVENT_TYPE_STATE_CHANGED: EventType
EVENT_TYPE_OPERATION_PROGRESS: EventType
EVENT_TYPE_READY_CHANGED: EventType
EVENT_TYPE_TTL_WARNING: EventType
EVENT_TYPE_DEGRADATION: EventType
EVENT_TYPE_RESTORED: EventType
EVENT_TYPE_WAKE_FIRED: EventType
EVENT_TYPE_FENCED: EventType
EVENT_TYPE_IDLE_FIRED: EventType
EVENT_TYPE_LINEAGE_RECORDED: EventType
EVENT_TYPE_EPOCH_ROTATED: EventType
FORK_MODE_UNSPECIFIED: ForkMode
FORK_MODE_COW: ForkMode
FORK_MODE_FULL_COPY: ForkMode
CAPSULE_STORAGE_UNSPECIFIED: CapsuleStorage
CAPSULE_STORAGE_LOCAL_DIR: CapsuleStorage
CAPSULE_STORAGE_OBJECT_STORE: CapsuleStorage
CAPSULE_OBJECT_TYPE_UNSPECIFIED: CapsuleObjectType
CAPSULE_OBJECT_TYPE_MEMORY: CapsuleObjectType
CAPSULE_OBJECT_TYPE_DISK: CapsuleObjectType
CAPSULE_OBJECT_TYPE_METADATA: CapsuleObjectType

class GetNodeInfoRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class NodeInfo(_message.Message):
    __slots__ = ("node_id", "arch", "cpu_class", "runtimes", "total_resources", "allocatable_resources", "agent_version", "fleet")
    NODE_ID_FIELD_NUMBER: _ClassVar[int]
    ARCH_FIELD_NUMBER: _ClassVar[int]
    CPU_CLASS_FIELD_NUMBER: _ClassVar[int]
    RUNTIMES_FIELD_NUMBER: _ClassVar[int]
    TOTAL_RESOURCES_FIELD_NUMBER: _ClassVar[int]
    ALLOCATABLE_RESOURCES_FIELD_NUMBER: _ClassVar[int]
    AGENT_VERSION_FIELD_NUMBER: _ClassVar[int]
    FLEET_FIELD_NUMBER: _ClassVar[int]
    node_id: str
    arch: str
    cpu_class: str
    runtimes: _containers.RepeatedCompositeFieldContainer[RuntimeInfo]
    total_resources: Resources
    allocatable_resources: Resources
    agent_version: str
    fleet: FleetInfo
    def __init__(self, node_id: _Optional[str] = ..., arch: _Optional[str] = ..., cpu_class: _Optional[str] = ..., runtimes: _Optional[_Iterable[_Union[RuntimeInfo, _Mapping]]] = ..., total_resources: _Optional[_Union[Resources, _Mapping]] = ..., allocatable_resources: _Optional[_Union[Resources, _Mapping]] = ..., agent_version: _Optional[str] = ..., fleet: _Optional[_Union[FleetInfo, _Mapping]] = ...) -> None: ...

class FleetInfo(_message.Message):
    __slots__ = ("bucket", "advertise", "held")
    BUCKET_FIELD_NUMBER: _ClassVar[int]
    ADVERTISE_FIELD_NUMBER: _ClassVar[int]
    HELD_FIELD_NUMBER: _ClassVar[int]
    bucket: str
    advertise: str
    held: _containers.RepeatedCompositeFieldContainer[HeldLease]
    def __init__(self, bucket: _Optional[str] = ..., advertise: _Optional[str] = ..., held: _Optional[_Iterable[_Union[HeldLease, _Mapping]]] = ...) -> None: ...

class HeldLease(_message.Message):
    __slots__ = ("name", "epoch", "instance_id")
    NAME_FIELD_NUMBER: _ClassVar[int]
    EPOCH_FIELD_NUMBER: _ClassVar[int]
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    name: str
    epoch: int
    instance_id: str
    def __init__(self, name: _Optional[str] = ..., epoch: _Optional[int] = ..., instance_id: _Optional[str] = ...) -> None: ...

class RuntimeInfo(_message.Message):
    __slots__ = ("name", "capabilities", "version", "health", "health_detail")
    NAME_FIELD_NUMBER: _ClassVar[int]
    CAPABILITIES_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    HEALTH_FIELD_NUMBER: _ClassVar[int]
    HEALTH_DETAIL_FIELD_NUMBER: _ClassVar[int]
    name: str
    capabilities: RuntimeCapabilities
    version: str
    health: SubstrateHealth
    health_detail: str
    def __init__(self, name: _Optional[str] = ..., capabilities: _Optional[_Union[RuntimeCapabilities, _Mapping]] = ..., version: _Optional[str] = ..., health: _Optional[_Union[SubstrateHealth, str]] = ..., health_detail: _Optional[str] = ...) -> None: ...

class RuntimeCapabilities(_message.Message):
    __slots__ = ("memory_snapshot", "disk_snapshot", "live_checkpoint", "guest_agent", "hardware_isolation", "lazy_restore", "cow_fork", "egress_control", "full_copy_fork", "object_store_snapshots", "capsule_export", "capsule_import", "safe_grant_rebind")
    MEMORY_SNAPSHOT_FIELD_NUMBER: _ClassVar[int]
    DISK_SNAPSHOT_FIELD_NUMBER: _ClassVar[int]
    LIVE_CHECKPOINT_FIELD_NUMBER: _ClassVar[int]
    GUEST_AGENT_FIELD_NUMBER: _ClassVar[int]
    HARDWARE_ISOLATION_FIELD_NUMBER: _ClassVar[int]
    LAZY_RESTORE_FIELD_NUMBER: _ClassVar[int]
    COW_FORK_FIELD_NUMBER: _ClassVar[int]
    EGRESS_CONTROL_FIELD_NUMBER: _ClassVar[int]
    FULL_COPY_FORK_FIELD_NUMBER: _ClassVar[int]
    OBJECT_STORE_SNAPSHOTS_FIELD_NUMBER: _ClassVar[int]
    CAPSULE_EXPORT_FIELD_NUMBER: _ClassVar[int]
    CAPSULE_IMPORT_FIELD_NUMBER: _ClassVar[int]
    SAFE_GRANT_REBIND_FIELD_NUMBER: _ClassVar[int]
    memory_snapshot: bool
    disk_snapshot: bool
    live_checkpoint: bool
    guest_agent: bool
    hardware_isolation: bool
    lazy_restore: bool
    cow_fork: bool
    egress_control: bool
    full_copy_fork: bool
    object_store_snapshots: bool
    capsule_export: bool
    capsule_import: bool
    safe_grant_rebind: bool
    def __init__(self, memory_snapshot: _Optional[bool] = ..., disk_snapshot: _Optional[bool] = ..., live_checkpoint: _Optional[bool] = ..., guest_agent: _Optional[bool] = ..., hardware_isolation: _Optional[bool] = ..., lazy_restore: _Optional[bool] = ..., cow_fork: _Optional[bool] = ..., egress_control: _Optional[bool] = ..., full_copy_fork: _Optional[bool] = ..., object_store_snapshots: _Optional[bool] = ..., capsule_export: _Optional[bool] = ..., capsule_import: _Optional[bool] = ..., safe_grant_rebind: _Optional[bool] = ...) -> None: ...

class InstanceSpec(_message.Message):
    __slots__ = ("instance_id", "template", "resources", "process", "hooks", "ttl_seconds", "ttl_action", "labels", "egress", "idle_action")
    class LabelsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    TEMPLATE_FIELD_NUMBER: _ClassVar[int]
    RESOURCES_FIELD_NUMBER: _ClassVar[int]
    PROCESS_FIELD_NUMBER: _ClassVar[int]
    HOOKS_FIELD_NUMBER: _ClassVar[int]
    TTL_SECONDS_FIELD_NUMBER: _ClassVar[int]
    TTL_ACTION_FIELD_NUMBER: _ClassVar[int]
    LABELS_FIELD_NUMBER: _ClassVar[int]
    EGRESS_FIELD_NUMBER: _ClassVar[int]
    IDLE_ACTION_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    template: TemplateRef
    resources: Resources
    process: Process
    hooks: Hooks
    ttl_seconds: int
    ttl_action: TtlAction
    labels: _containers.ScalarMap[str, str]
    egress: EgressPolicy
    idle_action: TtlAction
    def __init__(self, instance_id: _Optional[str] = ..., template: _Optional[_Union[TemplateRef, _Mapping]] = ..., resources: _Optional[_Union[Resources, _Mapping]] = ..., process: _Optional[_Union[Process, _Mapping]] = ..., hooks: _Optional[_Union[Hooks, _Mapping]] = ..., ttl_seconds: _Optional[int] = ..., ttl_action: _Optional[_Union[TtlAction, str]] = ..., labels: _Optional[_Mapping[str, str]] = ..., egress: _Optional[_Union[EgressPolicy, _Mapping]] = ..., idle_action: _Optional[_Union[TtlAction, str]] = ...) -> None: ...

class TemplateRef(_message.Message):
    __slots__ = ("oci", "runtime_bundle_ref", "template_hash", "arch")
    OCI_FIELD_NUMBER: _ClassVar[int]
    RUNTIME_BUNDLE_REF_FIELD_NUMBER: _ClassVar[int]
    TEMPLATE_HASH_FIELD_NUMBER: _ClassVar[int]
    ARCH_FIELD_NUMBER: _ClassVar[int]
    oci: OciImageRef
    runtime_bundle_ref: str
    template_hash: str
    arch: str
    def __init__(self, oci: _Optional[_Union[OciImageRef, _Mapping]] = ..., runtime_bundle_ref: _Optional[str] = ..., template_hash: _Optional[str] = ..., arch: _Optional[str] = ...) -> None: ...

class OciImageRef(_message.Message):
    __slots__ = ("image", "digest")
    IMAGE_FIELD_NUMBER: _ClassVar[int]
    DIGEST_FIELD_NUMBER: _ClassVar[int]
    image: str
    digest: str
    def __init__(self, image: _Optional[str] = ..., digest: _Optional[str] = ...) -> None: ...

class Resources(_message.Message):
    __slots__ = ("vcpu", "mem_mib", "disk_mib")
    VCPU_FIELD_NUMBER: _ClassVar[int]
    MEM_MIB_FIELD_NUMBER: _ClassVar[int]
    DISK_MIB_FIELD_NUMBER: _ClassVar[int]
    vcpu: int
    mem_mib: int
    disk_mib: int
    def __init__(self, vcpu: _Optional[int] = ..., mem_mib: _Optional[int] = ..., disk_mib: _Optional[int] = ...) -> None: ...

class Process(_message.Message):
    __slots__ = ("start_cmd", "ready_cmd", "env", "workdir")
    class EnvEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    START_CMD_FIELD_NUMBER: _ClassVar[int]
    READY_CMD_FIELD_NUMBER: _ClassVar[int]
    ENV_FIELD_NUMBER: _ClassVar[int]
    WORKDIR_FIELD_NUMBER: _ClassVar[int]
    start_cmd: _containers.RepeatedScalarFieldContainer[str]
    ready_cmd: _containers.RepeatedScalarFieldContainer[str]
    env: _containers.ScalarMap[str, str]
    workdir: str
    def __init__(self, start_cmd: _Optional[_Iterable[str]] = ..., ready_cmd: _Optional[_Iterable[str]] = ..., env: _Optional[_Mapping[str, str]] = ..., workdir: _Optional[str] = ...) -> None: ...

class Hooks(_message.Message):
    __slots__ = ("pre_snapshot_cmd", "post_restore_cmd", "pre_snapshot_timeout_ms", "post_restore_timeout_ms")
    PRE_SNAPSHOT_CMD_FIELD_NUMBER: _ClassVar[int]
    POST_RESTORE_CMD_FIELD_NUMBER: _ClassVar[int]
    PRE_SNAPSHOT_TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    POST_RESTORE_TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    pre_snapshot_cmd: _containers.RepeatedScalarFieldContainer[str]
    post_restore_cmd: _containers.RepeatedScalarFieldContainer[str]
    pre_snapshot_timeout_ms: int
    post_restore_timeout_ms: int
    def __init__(self, pre_snapshot_cmd: _Optional[_Iterable[str]] = ..., post_restore_cmd: _Optional[_Iterable[str]] = ..., pre_snapshot_timeout_ms: _Optional[int] = ..., post_restore_timeout_ms: _Optional[int] = ...) -> None: ...

class EgressPolicy(_message.Message):
    __slots__ = ("mediated", "mode")
    MEDIATED_FIELD_NUMBER: _ClassVar[int]
    MODE_FIELD_NUMBER: _ClassVar[int]
    mediated: bool
    mode: EgressMode
    def __init__(self, mediated: _Optional[bool] = ..., mode: _Optional[_Union[EgressMode, str]] = ...) -> None: ...

class Instance(_message.Message):
    __slots__ = ("spec", "state", "ready", "runtime", "created_at", "updated_at", "ttl_deadline", "latest_snapshot_id", "wake_at", "stop_reason", "network", "lineage", "execution_epoch")
    SPEC_FIELD_NUMBER: _ClassVar[int]
    STATE_FIELD_NUMBER: _ClassVar[int]
    READY_FIELD_NUMBER: _ClassVar[int]
    RUNTIME_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_FIELD_NUMBER: _ClassVar[int]
    UPDATED_AT_FIELD_NUMBER: _ClassVar[int]
    TTL_DEADLINE_FIELD_NUMBER: _ClassVar[int]
    LATEST_SNAPSHOT_ID_FIELD_NUMBER: _ClassVar[int]
    WAKE_AT_FIELD_NUMBER: _ClassVar[int]
    STOP_REASON_FIELD_NUMBER: _ClassVar[int]
    NETWORK_FIELD_NUMBER: _ClassVar[int]
    LINEAGE_FIELD_NUMBER: _ClassVar[int]
    EXECUTION_EPOCH_FIELD_NUMBER: _ClassVar[int]
    spec: InstanceSpec
    state: InstanceState
    ready: bool
    runtime: str
    created_at: _timestamp_pb2.Timestamp
    updated_at: _timestamp_pb2.Timestamp
    ttl_deadline: _timestamp_pb2.Timestamp
    latest_snapshot_id: str
    wake_at: _timestamp_pb2.Timestamp
    stop_reason: StopReason
    network: InstanceNetwork
    lineage: Lineage
    execution_epoch: int
    def __init__(self, spec: _Optional[_Union[InstanceSpec, _Mapping]] = ..., state: _Optional[_Union[InstanceState, str]] = ..., ready: _Optional[bool] = ..., runtime: _Optional[str] = ..., created_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., updated_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., ttl_deadline: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., latest_snapshot_id: _Optional[str] = ..., wake_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., stop_reason: _Optional[_Union[StopReason, _Mapping]] = ..., network: _Optional[_Union[InstanceNetwork, _Mapping]] = ..., lineage: _Optional[_Union[Lineage, _Mapping]] = ..., execution_epoch: _Optional[int] = ...) -> None: ...

class Lineage(_message.Message):
    __slots__ = ("lineage_id", "source_snapshot_id", "source_capsule_id", "parent_instance_id")
    LINEAGE_ID_FIELD_NUMBER: _ClassVar[int]
    SOURCE_SNAPSHOT_ID_FIELD_NUMBER: _ClassVar[int]
    SOURCE_CAPSULE_ID_FIELD_NUMBER: _ClassVar[int]
    PARENT_INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    lineage_id: str
    source_snapshot_id: str
    source_capsule_id: str
    parent_instance_id: str
    def __init__(self, lineage_id: _Optional[str] = ..., source_snapshot_id: _Optional[str] = ..., source_capsule_id: _Optional[str] = ..., parent_instance_id: _Optional[str] = ...) -> None: ...

class InstanceNetwork(_message.Message):
    __slots__ = ("address",)
    ADDRESS_FIELD_NUMBER: _ClassVar[int]
    address: str
    def __init__(self, address: _Optional[str] = ...) -> None: ...

class StopReason(_message.Message):
    __slots__ = ("requested", "exit_code", "detail")
    REQUESTED_FIELD_NUMBER: _ClassVar[int]
    EXIT_CODE_FIELD_NUMBER: _ClassVar[int]
    DETAIL_FIELD_NUMBER: _ClassVar[int]
    requested: bool
    exit_code: int
    detail: str
    def __init__(self, requested: _Optional[bool] = ..., exit_code: _Optional[int] = ..., detail: _Optional[str] = ...) -> None: ...

class Snapshot(_message.Message):
    __slots__ = ("snapshot_id", "instance_id", "kind", "cpu_class", "template_hash", "runtime_bundle_ref", "tier", "size_bytes", "created_at", "pre_snapshot_hook", "name")
    SNAPSHOT_ID_FIELD_NUMBER: _ClassVar[int]
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    KIND_FIELD_NUMBER: _ClassVar[int]
    CPU_CLASS_FIELD_NUMBER: _ClassVar[int]
    TEMPLATE_HASH_FIELD_NUMBER: _ClassVar[int]
    RUNTIME_BUNDLE_REF_FIELD_NUMBER: _ClassVar[int]
    TIER_FIELD_NUMBER: _ClassVar[int]
    SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_FIELD_NUMBER: _ClassVar[int]
    PRE_SNAPSHOT_HOOK_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    snapshot_id: str
    instance_id: str
    kind: SnapshotKind
    cpu_class: str
    template_hash: str
    runtime_bundle_ref: str
    tier: SnapshotTier
    size_bytes: int
    created_at: _timestamp_pb2.Timestamp
    pre_snapshot_hook: HookOutcome
    name: str
    def __init__(self, snapshot_id: _Optional[str] = ..., instance_id: _Optional[str] = ..., kind: _Optional[_Union[SnapshotKind, str]] = ..., cpu_class: _Optional[str] = ..., template_hash: _Optional[str] = ..., runtime_bundle_ref: _Optional[str] = ..., tier: _Optional[_Union[SnapshotTier, str]] = ..., size_bytes: _Optional[int] = ..., created_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., pre_snapshot_hook: _Optional[_Union[HookOutcome, _Mapping]] = ..., name: _Optional[str] = ...) -> None: ...

class HookOutcome(_message.Message):
    __slots__ = ("ran", "timed_out", "exit_code")
    RAN_FIELD_NUMBER: _ClassVar[int]
    TIMED_OUT_FIELD_NUMBER: _ClassVar[int]
    EXIT_CODE_FIELD_NUMBER: _ClassVar[int]
    ran: bool
    timed_out: bool
    exit_code: int
    def __init__(self, ran: _Optional[bool] = ..., timed_out: _Optional[bool] = ..., exit_code: _Optional[int] = ...) -> None: ...

class ErrorDetail(_message.Message):
    __slots__ = ("reason", "message")
    REASON_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    reason: ErrorReason
    message: str
    def __init__(self, reason: _Optional[_Union[ErrorReason, str]] = ..., message: _Optional[str] = ...) -> None: ...

class Operation(_message.Message):
    __slots__ = ("op_id", "kind", "instance_id", "state", "current_step", "error", "degraded", "created_at", "finished_at", "froze_workload", "actual_fork_mode", "capsule_id")
    OP_ID_FIELD_NUMBER: _ClassVar[int]
    KIND_FIELD_NUMBER: _ClassVar[int]
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    STATE_FIELD_NUMBER: _ClassVar[int]
    CURRENT_STEP_FIELD_NUMBER: _ClassVar[int]
    ERROR_FIELD_NUMBER: _ClassVar[int]
    DEGRADED_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_FIELD_NUMBER: _ClassVar[int]
    FINISHED_AT_FIELD_NUMBER: _ClassVar[int]
    FROZE_WORKLOAD_FIELD_NUMBER: _ClassVar[int]
    ACTUAL_FORK_MODE_FIELD_NUMBER: _ClassVar[int]
    CAPSULE_ID_FIELD_NUMBER: _ClassVar[int]
    op_id: str
    kind: str
    instance_id: str
    state: OperationState
    current_step: str
    error: ErrorDetail
    degraded: str
    created_at: _timestamp_pb2.Timestamp
    finished_at: _timestamp_pb2.Timestamp
    froze_workload: bool
    actual_fork_mode: ForkMode
    capsule_id: str
    def __init__(self, op_id: _Optional[str] = ..., kind: _Optional[str] = ..., instance_id: _Optional[str] = ..., state: _Optional[_Union[OperationState, str]] = ..., current_step: _Optional[str] = ..., error: _Optional[_Union[ErrorDetail, _Mapping]] = ..., degraded: _Optional[str] = ..., created_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., finished_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., froze_workload: _Optional[bool] = ..., actual_fork_mode: _Optional[_Union[ForkMode, str]] = ..., capsule_id: _Optional[str] = ...) -> None: ...

class CreateInstanceRequest(_message.Message):
    __slots__ = ("spec", "idempotency_key", "require_hardware_isolation")
    SPEC_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    REQUIRE_HARDWARE_ISOLATION_FIELD_NUMBER: _ClassVar[int]
    spec: InstanceSpec
    idempotency_key: str
    require_hardware_isolation: bool
    def __init__(self, spec: _Optional[_Union[InstanceSpec, _Mapping]] = ..., idempotency_key: _Optional[str] = ..., require_hardware_isolation: _Optional[bool] = ...) -> None: ...

class StartInstanceRequest(_message.Message):
    __slots__ = ("instance_id", "idempotency_key")
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    idempotency_key: str
    def __init__(self, instance_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class StopInstanceRequest(_message.Message):
    __slots__ = ("instance_id", "idempotency_key", "grace_seconds")
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    GRACE_SECONDS_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    idempotency_key: str
    grace_seconds: int
    def __init__(self, instance_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ..., grace_seconds: _Optional[int] = ...) -> None: ...

class PauseInstanceRequest(_message.Message):
    __slots__ = ("instance_id", "idempotency_key", "keep_memory", "require_memory")
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    KEEP_MEMORY_FIELD_NUMBER: _ClassVar[int]
    REQUIRE_MEMORY_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    idempotency_key: str
    keep_memory: bool
    require_memory: bool
    def __init__(self, instance_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ..., keep_memory: _Optional[bool] = ..., require_memory: _Optional[bool] = ...) -> None: ...

class ResumeInstanceRequest(_message.Message):
    __slots__ = ("instance_id", "snapshot_id", "idempotency_key", "require_memory")
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    SNAPSHOT_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    REQUIRE_MEMORY_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    snapshot_id: str
    idempotency_key: str
    require_memory: bool
    def __init__(self, instance_id: _Optional[str] = ..., snapshot_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ..., require_memory: _Optional[bool] = ...) -> None: ...

class CheckpointInstanceRequest(_message.Message):
    __slots__ = ("instance_id", "idempotency_key")
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    idempotency_key: str
    def __init__(self, instance_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class DestroyInstanceRequest(_message.Message):
    __slots__ = ("instance_id", "idempotency_key", "keep_snapshots")
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    KEEP_SNAPSHOTS_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    idempotency_key: str
    keep_snapshots: bool
    def __init__(self, instance_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ..., keep_snapshots: _Optional[bool] = ...) -> None: ...

class SetWakeRequest(_message.Message):
    __slots__ = ("instance_id", "wake_at")
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    WAKE_AT_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    wake_at: _timestamp_pb2.Timestamp
    def __init__(self, instance_id: _Optional[str] = ..., wake_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class GetInstanceRequest(_message.Message):
    __slots__ = ("instance_id",)
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    def __init__(self, instance_id: _Optional[str] = ...) -> None: ...

class ListInstancesRequest(_message.Message):
    __slots__ = ("states", "label_selector")
    class LabelSelectorEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    STATES_FIELD_NUMBER: _ClassVar[int]
    LABEL_SELECTOR_FIELD_NUMBER: _ClassVar[int]
    states: _containers.RepeatedScalarFieldContainer[InstanceState]
    label_selector: _containers.ScalarMap[str, str]
    def __init__(self, states: _Optional[_Iterable[_Union[InstanceState, str]]] = ..., label_selector: _Optional[_Mapping[str, str]] = ...) -> None: ...

class ListInstancesResponse(_message.Message):
    __slots__ = ("instances",)
    INSTANCES_FIELD_NUMBER: _ClassVar[int]
    instances: _containers.RepeatedCompositeFieldContainer[Instance]
    def __init__(self, instances: _Optional[_Iterable[_Union[Instance, _Mapping]]] = ...) -> None: ...

class ListSnapshotsRequest(_message.Message):
    __slots__ = ("instance_id",)
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    def __init__(self, instance_id: _Optional[str] = ...) -> None: ...

class ListSnapshotsResponse(_message.Message):
    __slots__ = ("snapshots",)
    SNAPSHOTS_FIELD_NUMBER: _ClassVar[int]
    snapshots: _containers.RepeatedCompositeFieldContainer[Snapshot]
    def __init__(self, snapshots: _Optional[_Iterable[_Union[Snapshot, _Mapping]]] = ...) -> None: ...

class DeleteSnapshotRequest(_message.Message):
    __slots__ = ("snapshot_id", "idempotency_key")
    SNAPSHOT_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    snapshot_id: str
    idempotency_key: str
    def __init__(self, snapshot_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class CreateSnapshotRequest(_message.Message):
    __slots__ = ("instance_id", "idempotency_key", "name")
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    idempotency_key: str
    name: str
    def __init__(self, instance_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ..., name: _Optional[str] = ...) -> None: ...

class GetOperationRequest(_message.Message):
    __slots__ = ("op_id",)
    OP_ID_FIELD_NUMBER: _ClassVar[int]
    op_id: str
    def __init__(self, op_id: _Optional[str] = ...) -> None: ...

class WatchEventsRequest(_message.Message):
    __slots__ = ("from_cursor", "instance_id")
    FROM_CURSOR_FIELD_NUMBER: _ClassVar[int]
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    from_cursor: int
    instance_id: str
    def __init__(self, from_cursor: _Optional[int] = ..., instance_id: _Optional[str] = ...) -> None: ...

class Event(_message.Message):
    __slots__ = ("cursor", "type", "instance_id", "op_id", "state", "message", "at", "stop_reason")
    CURSOR_FIELD_NUMBER: _ClassVar[int]
    TYPE_FIELD_NUMBER: _ClassVar[int]
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    OP_ID_FIELD_NUMBER: _ClassVar[int]
    STATE_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    AT_FIELD_NUMBER: _ClassVar[int]
    STOP_REASON_FIELD_NUMBER: _ClassVar[int]
    cursor: int
    type: EventType
    instance_id: str
    op_id: str
    state: InstanceState
    message: str
    at: _timestamp_pb2.Timestamp
    stop_reason: StopReason
    def __init__(self, cursor: _Optional[int] = ..., type: _Optional[_Union[EventType, str]] = ..., instance_id: _Optional[str] = ..., op_id: _Optional[str] = ..., state: _Optional[_Union[InstanceState, str]] = ..., message: _Optional[str] = ..., at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., stop_reason: _Optional[_Union[StopReason, _Mapping]] = ...) -> None: ...

class ExecFrame(_message.Message):
    __slots__ = ("start", "stdin", "stdout", "stderr", "resize", "exit")
    START_FIELD_NUMBER: _ClassVar[int]
    STDIN_FIELD_NUMBER: _ClassVar[int]
    STDOUT_FIELD_NUMBER: _ClassVar[int]
    STDERR_FIELD_NUMBER: _ClassVar[int]
    RESIZE_FIELD_NUMBER: _ClassVar[int]
    EXIT_FIELD_NUMBER: _ClassVar[int]
    start: ExecStart
    stdin: bytes
    stdout: bytes
    stderr: bytes
    resize: TermSize
    exit: ExitStatus
    def __init__(self, start: _Optional[_Union[ExecStart, _Mapping]] = ..., stdin: _Optional[bytes] = ..., stdout: _Optional[bytes] = ..., stderr: _Optional[bytes] = ..., resize: _Optional[_Union[TermSize, _Mapping]] = ..., exit: _Optional[_Union[ExitStatus, _Mapping]] = ...) -> None: ...

class ExecStart(_message.Message):
    __slots__ = ("instance_id", "cmd", "env", "workdir", "pty", "term_size", "user_activity")
    class EnvEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    CMD_FIELD_NUMBER: _ClassVar[int]
    ENV_FIELD_NUMBER: _ClassVar[int]
    WORKDIR_FIELD_NUMBER: _ClassVar[int]
    PTY_FIELD_NUMBER: _ClassVar[int]
    TERM_SIZE_FIELD_NUMBER: _ClassVar[int]
    USER_ACTIVITY_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    cmd: _containers.RepeatedScalarFieldContainer[str]
    env: _containers.ScalarMap[str, str]
    workdir: str
    pty: bool
    term_size: TermSize
    user_activity: bool
    def __init__(self, instance_id: _Optional[str] = ..., cmd: _Optional[_Iterable[str]] = ..., env: _Optional[_Mapping[str, str]] = ..., workdir: _Optional[str] = ..., pty: _Optional[bool] = ..., term_size: _Optional[_Union[TermSize, _Mapping]] = ..., user_activity: _Optional[bool] = ...) -> None: ...

class TermSize(_message.Message):
    __slots__ = ("rows", "cols")
    ROWS_FIELD_NUMBER: _ClassVar[int]
    COLS_FIELD_NUMBER: _ClassVar[int]
    rows: int
    cols: int
    def __init__(self, rows: _Optional[int] = ..., cols: _Optional[int] = ...) -> None: ...

class ExitStatus(_message.Message):
    __slots__ = ("code",)
    CODE_FIELD_NUMBER: _ClassVar[int]
    code: int
    def __init__(self, code: _Optional[int] = ...) -> None: ...

class ReadFileRequest(_message.Message):
    __slots__ = ("instance_id", "path", "offset", "limit")
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    PATH_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    LIMIT_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    path: str
    offset: int
    limit: int
    def __init__(self, instance_id: _Optional[str] = ..., path: _Optional[str] = ..., offset: _Optional[int] = ..., limit: _Optional[int] = ...) -> None: ...

class FileChunk(_message.Message):
    __slots__ = ("data", "eof")
    DATA_FIELD_NUMBER: _ClassVar[int]
    EOF_FIELD_NUMBER: _ClassVar[int]
    data: bytes
    eof: bool
    def __init__(self, data: _Optional[bytes] = ..., eof: _Optional[bool] = ...) -> None: ...

class WriteFileRequest(_message.Message):
    __slots__ = ("open", "chunk")
    OPEN_FIELD_NUMBER: _ClassVar[int]
    CHUNK_FIELD_NUMBER: _ClassVar[int]
    open: WriteOpen
    chunk: bytes
    def __init__(self, open: _Optional[_Union[WriteOpen, _Mapping]] = ..., chunk: _Optional[bytes] = ...) -> None: ...

class WriteOpen(_message.Message):
    __slots__ = ("instance_id", "path", "mode")
    INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    PATH_FIELD_NUMBER: _ClassVar[int]
    MODE_FIELD_NUMBER: _ClassVar[int]
    instance_id: str
    path: str
    mode: int
    def __init__(self, instance_id: _Optional[str] = ..., path: _Optional[str] = ..., mode: _Optional[int] = ...) -> None: ...

class WriteFileResponse(_message.Message):
    __slots__ = ("bytes_written",)
    BYTES_WRITTEN_FIELD_NUMBER: _ClassVar[int]
    bytes_written: int
    def __init__(self, bytes_written: _Optional[int] = ...) -> None: ...

class ForkInstanceRequest(_message.Message):
    __slots__ = ("source_snapshot_id", "target_instance_id", "idempotency_key", "require_cow", "target_spec")
    SOURCE_SNAPSHOT_ID_FIELD_NUMBER: _ClassVar[int]
    TARGET_INSTANCE_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    REQUIRE_COW_FIELD_NUMBER: _ClassVar[int]
    TARGET_SPEC_FIELD_NUMBER: _ClassVar[int]
    source_snapshot_id: str
    target_instance_id: str
    idempotency_key: str
    require_cow: bool
    target_spec: InstanceSpec
    def __init__(self, source_snapshot_id: _Optional[str] = ..., target_instance_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ..., require_cow: _Optional[bool] = ..., target_spec: _Optional[_Union[InstanceSpec, _Mapping]] = ...) -> None: ...

class CapsuleObject(_message.Message):
    __slots__ = ("digest", "length", "type")
    DIGEST_FIELD_NUMBER: _ClassVar[int]
    LENGTH_FIELD_NUMBER: _ClassVar[int]
    TYPE_FIELD_NUMBER: _ClassVar[int]
    digest: str
    length: int
    type: CapsuleObjectType
    def __init__(self, digest: _Optional[str] = ..., length: _Optional[int] = ..., type: _Optional[_Union[CapsuleObjectType, str]] = ...) -> None: ...

class CapsuleManifest(_message.Message):
    __slots__ = ("schema_version", "cpu_class", "template_hash", "runtime_bundle_ref", "kind", "objects", "lineage_id")
    SCHEMA_VERSION_FIELD_NUMBER: _ClassVar[int]
    CPU_CLASS_FIELD_NUMBER: _ClassVar[int]
    TEMPLATE_HASH_FIELD_NUMBER: _ClassVar[int]
    RUNTIME_BUNDLE_REF_FIELD_NUMBER: _ClassVar[int]
    KIND_FIELD_NUMBER: _ClassVar[int]
    OBJECTS_FIELD_NUMBER: _ClassVar[int]
    LINEAGE_ID_FIELD_NUMBER: _ClassVar[int]
    schema_version: str
    cpu_class: str
    template_hash: str
    runtime_bundle_ref: str
    kind: SnapshotKind
    objects: _containers.RepeatedCompositeFieldContainer[CapsuleObject]
    lineage_id: str
    def __init__(self, schema_version: _Optional[str] = ..., cpu_class: _Optional[str] = ..., template_hash: _Optional[str] = ..., runtime_bundle_ref: _Optional[str] = ..., kind: _Optional[_Union[SnapshotKind, str]] = ..., objects: _Optional[_Iterable[_Union[CapsuleObject, _Mapping]]] = ..., lineage_id: _Optional[str] = ...) -> None: ...

class Capsule(_message.Message):
    __slots__ = ("capsule_id", "manifest", "storage", "total_size_bytes", "created_at")
    CAPSULE_ID_FIELD_NUMBER: _ClassVar[int]
    MANIFEST_FIELD_NUMBER: _ClassVar[int]
    STORAGE_FIELD_NUMBER: _ClassVar[int]
    TOTAL_SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_FIELD_NUMBER: _ClassVar[int]
    capsule_id: str
    manifest: CapsuleManifest
    storage: CapsuleStorage
    total_size_bytes: int
    created_at: _timestamp_pb2.Timestamp
    def __init__(self, capsule_id: _Optional[str] = ..., manifest: _Optional[_Union[CapsuleManifest, _Mapping]] = ..., storage: _Optional[_Union[CapsuleStorage, str]] = ..., total_size_bytes: _Optional[int] = ..., created_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class ExportCapsuleRequest(_message.Message):
    __slots__ = ("snapshot_id", "idempotency_key", "tier")
    SNAPSHOT_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    TIER_FIELD_NUMBER: _ClassVar[int]
    snapshot_id: str
    idempotency_key: str
    tier: CapsuleStorage
    def __init__(self, snapshot_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ..., tier: _Optional[_Union[CapsuleStorage, str]] = ...) -> None: ...

class ImportCapsuleRequest(_message.Message):
    __slots__ = ("manifest", "storage", "idempotency_key")
    MANIFEST_FIELD_NUMBER: _ClassVar[int]
    STORAGE_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    manifest: CapsuleManifest
    storage: CapsuleStorage
    idempotency_key: str
    def __init__(self, manifest: _Optional[_Union[CapsuleManifest, _Mapping]] = ..., storage: _Optional[_Union[CapsuleStorage, str]] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class DeleteCapsuleRequest(_message.Message):
    __slots__ = ("capsule_id", "idempotency_key")
    CAPSULE_ID_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    capsule_id: str
    idempotency_key: str
    def __init__(self, capsule_id: _Optional[str] = ..., idempotency_key: _Optional[str] = ...) -> None: ...

class GetCapsuleRequest(_message.Message):
    __slots__ = ("capsule_id",)
    CAPSULE_ID_FIELD_NUMBER: _ClassVar[int]
    capsule_id: str
    def __init__(self, capsule_id: _Optional[str] = ...) -> None: ...

class ListCapsulesRequest(_message.Message):
    __slots__ = ("lineage_id",)
    LINEAGE_ID_FIELD_NUMBER: _ClassVar[int]
    lineage_id: str
    def __init__(self, lineage_id: _Optional[str] = ...) -> None: ...

class ListCapsulesResponse(_message.Message):
    __slots__ = ("capsules",)
    CAPSULES_FIELD_NUMBER: _ClassVar[int]
    capsules: _containers.RepeatedCompositeFieldContainer[Capsule]
    def __init__(self, capsules: _Optional[_Iterable[_Union[Capsule, _Mapping]]] = ...) -> None: ...
