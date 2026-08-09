import datetime

from google.protobuf import timestamp_pb2 as _timestamp_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class HookKind(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    HOOK_KIND_UNSPECIFIED: _ClassVar[HookKind]
    HOOK_KIND_PRE_SNAPSHOT: _ClassVar[HookKind]
    HOOK_KIND_POST_RESTORE: _ClassVar[HookKind]
HOOK_KIND_UNSPECIFIED: HookKind
HOOK_KIND_PRE_SNAPSHOT: HookKind
HOOK_KIND_POST_RESTORE: HookKind

class HealthRequest(_message.Message):
    __slots__ = ("run_ready_cmd",)
    RUN_READY_CMD_FIELD_NUMBER: _ClassVar[int]
    run_ready_cmd: bool
    def __init__(self, run_ready_cmd: _Optional[bool] = ...) -> None: ...

class HealthResponse(_message.Message):
    __slots__ = ("alive", "ready", "ready_cmd_exit", "last_user_activity", "guest_time")
    ALIVE_FIELD_NUMBER: _ClassVar[int]
    READY_FIELD_NUMBER: _ClassVar[int]
    READY_CMD_EXIT_FIELD_NUMBER: _ClassVar[int]
    LAST_USER_ACTIVITY_FIELD_NUMBER: _ClassVar[int]
    GUEST_TIME_FIELD_NUMBER: _ClassVar[int]
    alive: bool
    ready: bool
    ready_cmd_exit: int
    last_user_activity: _timestamp_pb2.Timestamp
    guest_time: _timestamp_pb2.Timestamp
    def __init__(self, alive: _Optional[bool] = ..., ready: _Optional[bool] = ..., ready_cmd_exit: _Optional[int] = ..., last_user_activity: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ..., guest_time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

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
    __slots__ = ("cmd", "env", "workdir", "pty", "term_size", "user_activity")
    class EnvEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    CMD_FIELD_NUMBER: _ClassVar[int]
    ENV_FIELD_NUMBER: _ClassVar[int]
    WORKDIR_FIELD_NUMBER: _ClassVar[int]
    PTY_FIELD_NUMBER: _ClassVar[int]
    TERM_SIZE_FIELD_NUMBER: _ClassVar[int]
    USER_ACTIVITY_FIELD_NUMBER: _ClassVar[int]
    cmd: _containers.RepeatedScalarFieldContainer[str]
    env: _containers.ScalarMap[str, str]
    workdir: str
    pty: bool
    term_size: TermSize
    user_activity: bool
    def __init__(self, cmd: _Optional[_Iterable[str]] = ..., env: _Optional[_Mapping[str, str]] = ..., workdir: _Optional[str] = ..., pty: _Optional[bool] = ..., term_size: _Optional[_Union[TermSize, _Mapping]] = ..., user_activity: _Optional[bool] = ...) -> None: ...

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
    __slots__ = ("path", "offset", "limit")
    PATH_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    LIMIT_FIELD_NUMBER: _ClassVar[int]
    path: str
    offset: int
    limit: int
    def __init__(self, path: _Optional[str] = ..., offset: _Optional[int] = ..., limit: _Optional[int] = ...) -> None: ...

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
    __slots__ = ("path", "mode")
    PATH_FIELD_NUMBER: _ClassVar[int]
    MODE_FIELD_NUMBER: _ClassVar[int]
    path: str
    mode: int
    def __init__(self, path: _Optional[str] = ..., mode: _Optional[int] = ...) -> None: ...

class WriteFileResponse(_message.Message):
    __slots__ = ("bytes_written",)
    BYTES_WRITTEN_FIELD_NUMBER: _ClassVar[int]
    bytes_written: int
    def __init__(self, bytes_written: _Optional[int] = ...) -> None: ...

class StatPathRequest(_message.Message):
    __slots__ = ("path",)
    PATH_FIELD_NUMBER: _ClassVar[int]
    path: str
    def __init__(self, path: _Optional[str] = ...) -> None: ...

class StatPathResponse(_message.Message):
    __slots__ = ("exists", "is_dir", "size_bytes", "mode", "modified_at")
    EXISTS_FIELD_NUMBER: _ClassVar[int]
    IS_DIR_FIELD_NUMBER: _ClassVar[int]
    SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    MODE_FIELD_NUMBER: _ClassVar[int]
    MODIFIED_AT_FIELD_NUMBER: _ClassVar[int]
    exists: bool
    is_dir: bool
    size_bytes: int
    mode: int
    modified_at: _timestamp_pb2.Timestamp
    def __init__(self, exists: _Optional[bool] = ..., is_dir: _Optional[bool] = ..., size_bytes: _Optional[int] = ..., mode: _Optional[int] = ..., modified_at: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class RunHookRequest(_message.Message):
    __slots__ = ("kind", "timeout_ms")
    KIND_FIELD_NUMBER: _ClassVar[int]
    TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    kind: HookKind
    timeout_ms: int
    def __init__(self, kind: _Optional[_Union[HookKind, str]] = ..., timeout_ms: _Optional[int] = ...) -> None: ...

class RunHookResponse(_message.Message):
    __slots__ = ("ran", "timed_out", "exit_code", "stdout_tail", "stderr_tail")
    RAN_FIELD_NUMBER: _ClassVar[int]
    TIMED_OUT_FIELD_NUMBER: _ClassVar[int]
    EXIT_CODE_FIELD_NUMBER: _ClassVar[int]
    STDOUT_TAIL_FIELD_NUMBER: _ClassVar[int]
    STDERR_TAIL_FIELD_NUMBER: _ClassVar[int]
    ran: bool
    timed_out: bool
    exit_code: int
    stdout_tail: str
    stderr_tail: str
    def __init__(self, ran: _Optional[bool] = ..., timed_out: _Optional[bool] = ..., exit_code: _Optional[int] = ..., stdout_tail: _Optional[str] = ..., stderr_tail: _Optional[str] = ...) -> None: ...

class RestoreDutiesRequest(_message.Message):
    __slots__ = ("entropy", "host_time")
    ENTROPY_FIELD_NUMBER: _ClassVar[int]
    HOST_TIME_FIELD_NUMBER: _ClassVar[int]
    entropy: bytes
    host_time: _timestamp_pb2.Timestamp
    def __init__(self, entropy: _Optional[bytes] = ..., host_time: _Optional[_Union[datetime.datetime, _timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class RestoreDutiesResponse(_message.Message):
    __slots__ = ("entropy_bytes_mixed", "entropy_credited", "clock_drift_ms", "clock_stepped", "degraded")
    ENTROPY_BYTES_MIXED_FIELD_NUMBER: _ClassVar[int]
    ENTROPY_CREDITED_FIELD_NUMBER: _ClassVar[int]
    CLOCK_DRIFT_MS_FIELD_NUMBER: _ClassVar[int]
    CLOCK_STEPPED_FIELD_NUMBER: _ClassVar[int]
    DEGRADED_FIELD_NUMBER: _ClassVar[int]
    entropy_bytes_mixed: int
    entropy_credited: bool
    clock_drift_ms: int
    clock_stepped: bool
    degraded: str
    def __init__(self, entropy_bytes_mixed: _Optional[int] = ..., entropy_credited: _Optional[bool] = ..., clock_drift_ms: _Optional[int] = ..., clock_stepped: _Optional[bool] = ..., degraded: _Optional[str] = ...) -> None: ...
