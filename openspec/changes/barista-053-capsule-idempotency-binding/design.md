# Design: capsule key reservation

The capsule operation table is the serialization point. `INSERT OR IGNORE` reserves a unique key with verb, canonical request, operation id, and `RUNNING` state before work begins. The loser reads that row: an exact replay receives its operation; a verb/request mismatch fails. Completion updates only the reserved running row. Startup converts interrupted running rows to durable failures, retaining key binding across crashes.
