# Tasks

- [x] Add request fingerprints to capsule operation rows.
- [x] Reserve keys atomically before effects.
- [x] Reject verb or request mismatches with `INVALID_SPEC`.
- [x] Recover interrupted reservations and replay failures.
- [x] Add request/verb binding regression coverage.
- [x] Detach capsule work-plus-settle from the handler future; settle panics as journaled failures.
- [x] Cover the dropped-handler and panicking-work settlement paths.
- [x] Stop the cancel refusal claiming a RUNNING capsule row has settled.
