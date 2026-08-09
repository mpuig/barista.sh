# Design — explicit snapshots

## Decision 1: `Pause` keeps `standby`; explicit snapshots are a separate intent

The obvious design — every pause creates an explicit `/snapshots` object — is
rejected on the 5.5 measurement: pause freeze already scales at ~1.2–1.7 s/GiB
with no budget (BRD §6 v0.8), and an explicit snapshot is a second copy of the
same bytes. Paying it on *every* pause to serve the rare caller who wants a
retained artifact would put the cost precisely where it hurts most.

Instead, two intents stay distinct:

- **Pause** = "stop paying for this session, bring it back exactly" — `standby`,
  one instance-internal image, unchanged.
- **Explicit snapshot** = "give me a restorable artifact of this state" — what
  T9 needs once (created by the test), and what B10 golden templates will need
  as a first-class verb *later*. Phase 1 exposes no new Contract A verb for it:
  the only consumer today is T9, and the test can create it through the backend.
  When a real consumer wants named snapshots, that is a v1alpha2 `CreateSnapshot`
  RPC — the seam is the backend method, already shaped like the endpoint.

## Decision 2: resume-by-id stops collapsing

`ops.rs` currently collapses "resume snapshot S" into "resume in place" when S
is the latest, and the hypeman backend fails any other id — honest, because
`standby` kept only one image. With `/snapshots/{id}/restore` the backend can
honour an older id, so the collapse inverts: the id is passed through, and only
the *instance's own latest standby image* keeps the in-place path. Restore
preconditions (task 3.5) apply identically — the journal row for S carries the
same keys, and `restore::decide` does not care where the bytes live.

## Decision 3: T9's protocol

1. Start the workload, pause (standby — unrelated to the property under test).
2. Create one explicit snapshot from the instance.
3. Restore it; draw randomness inside the `POST_RESTORE` hook; record.
4. Restore **the same snapshot id** again; draw again.
5. The draws differ → reseed inside the duty sequence works on identical bytes.

Open verification item (task 1.1): whether hypeman permits snapshot creation
from `Standby` state or only from `Running`. The vendored spec is ambiguous;
the drift test cannot answer semantics, so this is a live-VM question and the
task order puts it first. If only-from-Running, step 2 moves before the pause,
which changes nothing about the property.

## Decision 4: the preflight arch check is best-effort and loud, never a gate

Findings §1's root cause was a wrong-arch guest binary that presented as a
kernel panic three layers away. The suggested upstream regression test — assert
ELF `e_machine` matches the host before building the initrd — can run on our
side too, but only where the node agent shares a filesystem with the substrate
(the only Phase 1 deployment). So:

- If `/var/lib/hypeman/system/initrd/<arch>/latest/initrd` is readable, scan the
  cpio members `init.bin` and `usr/local/bin/guest-agent` for their ELF headers
  and compare `e_machine` to the host arch. Mismatch → preflight **reports it by
  name**, like the missing-`caddy` / `mkfs.erofs` checks.
- Unreadable or remote → the check says nothing. A warning that fires on every
  legitimate remote deployment trains operators to ignore it.
- Compressed initrd (gzip/zstd) → decompress-in-memory if cheap, otherwise
  report "could not inspect" distinctly from "inspected, fine" — the same
  asked/could-not-ask honesty the quiesce hook records.

## Decision 5: fork is documented, not built

`POST /snapshots/{id}/fork` is the endpoint B39 fork-on-resume would consume:
one snapshot, N instances, each entering the restore-duty sequence (which this
change proves diverges their entropy — that is T9's point). No Contract A verb
exists for it and the spec defers it to v1alpha2. Building it now would be
speculative machinery (constitution IV). What this change contributes to it is
exactly the two properties it needs proven: same-bytes restore works, and
restored twins diverge.
