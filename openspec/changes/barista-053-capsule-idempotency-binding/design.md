# Design: capsule key reservation

The capsule operation table is the serialization point. `INSERT OR IGNORE` reserves a unique key with verb, canonical request, operation id, and `RUNNING` state before work begins. The loser reads that row: an exact replay receives its operation; a verb/request mismatch fails. Completion updates only the reserved running row. Startup converts interrupted running rows to durable failures, retaining key binding across crashes.

Reserving before work makes settlement an obligation, so the work-plus-settle runs as a detached task (the instance path's executor shape) and the handler merely awaits it: tonic dropping the handler on client disconnect abandons the await, never the settle. A panic in the work is caught and journaled as the operation's `FAILED` outcome for the same reason. Startup recovery keeps covering the only abandonment left — process death.
