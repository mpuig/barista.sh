# Design: capsule location strength

Until capsule locations become a separate relation, the single storage field is a monotonic summary. `OBJECT_STORE` dominates `LOCAL_DIR`: repeated registration promotes but never downgrades. Registration returns the durable row, and object references are inserted only for a newly created capsule. The RPC therefore reports persisted facts rather than the caller's requested tier.
