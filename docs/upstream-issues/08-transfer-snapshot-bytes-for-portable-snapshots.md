# Feature request: transfer a snapshot's bytes so a snapshot can leave its host

**Versions:** hypeman API 0.3.0, measured against a build of `main` at
`8853445` (the fork carrying #419).

## The request

Let an API consumer read a snapshot's bytes out of one host and write them into
another, so a snapshot becomes a portable artifact rather than a host-local one.

## What exists today

The snapshot resource is complete for same-host use and closed for anything
else. Six operations, and none of them moves bytes:

```
POST/GET /instances/{id}/snapshots            create, list
POST     /instances/{id}/snapshots/{sid}/restore
GET      /snapshots            GET/DELETE /snapshots/{id}
POST     /snapshots/{id}/fork
```

A snapshot is an id, some metadata (`kind`, `size_bytes`, compression state,
`source_instance_id`), and a directory of files on the server's `data_dir` —
the memory file, device state, disks. `restore` and `fork` both consume that
directory in place. Nothing reads it out, and nothing writes one in.

So a snapshot cannot be moved between hosts by any supported means. There is no
partial path either: no download, no upload, no archive form, no
`Content-Range` on anything snapshot-shaped.

## Why fork and restore are not equivalent

Both are same-host operations by construction — they take a snapshot id and
find its directory locally. They answer "run this state again *here*", which is
a different question from "run this state *there*".

The gap matters most in the cases the snapshot feature is otherwise good at.
A `Standby` snapshot is a resumable machine, and a resumable machine that
cannot leave its host is one hardware failure from being worthless: the host
that holds the only copy is also the host that can lose it. Same for capacity —
a snapshot cannot be rebalanced onto a less loaded machine, because it cannot
be sent to one.

## Why we are not just reading `data_dir`

We could. Our node agent is co-located with hypeman on the same box, so the
files are right there. We are asking instead of doing it because reaching around
the API costs the thing the API is for: it hard-codes hypeman's on-disk layout
into a consumer, it breaks the moment hypeman is reached over the network rather
than over loopback (our hosted control plane already talks to nodes over mutual
TLS), and it silently reads a snapshot that hypeman may be compressing in the
background — see the constraint below, which is exactly the class of bug an API
boundary exists to prevent.

## What would be enough

No protocol opinion above the bytes, and no new archive format. Per-object
access, because a consumer that content-addresses each file separately (we do)
can then deduplicate shared objects between snapshots and verify each one
independently — a tarball would force it to unpack and re-hash everything to
learn what it already knew.

**Export — two operations:**

```
GET /snapshots/{id}/objects
    → [{ "object_id": "...", "type": "memory|disk|metadata",
          "size_bytes": 123, "compression": {...} }]

GET /snapshots/{id}/objects/{object_id}
    → application/octet-stream  (Range requests welcome, not required)
```

**Import — staged, then committed:**

```
POST /snapshots            (kind, source metadata) → id, state=staging
PUT  /snapshots/{id}/objects/{object_id}   application/octet-stream
POST /snapshots/{id}/commit → state=ready, restorable
```

A staged snapshot must not be restorable or forkable. That is the property that
makes a torn transfer harmless: a partially uploaded snapshot is refused rather
than half-restored, and `commit` is the single point where hypeman can check it
got everything it was promised. Binary bodies are not new here —
`CreateImage` and `CreateBuild` already take `multipart`/`io.Reader`.

## The one constraint that is easy to get wrong

**Compression state has to be explicit in the manifest, and the exported bytes
have to match what the manifest says.**

Per `lib/snapshot/README.md`, memory compression "runs asynchronously after the
snapshot is already durable on disk", and `compression_state` moves
`none → compressing → compressed`. So the memory file's bytes change while the
snapshot's id and metadata do not.

For a consumer that content-addresses what it exports, that means two exports
of the *same* snapshot can legitimately produce two different content ids
depending on when they ran — which destroys the property that makes a portable
snapshot verifiable at all. We assert that property in our own tests, and we
cannot assert it against an export that silently changes shape.

Any of these resolves it; the first is the least work:

- report the algorithm and level per object in the manifest, and stream the
  bytes as they are on disk (so the consumer records *which* form it has);
- or let the caller request a form (`?compression=none`) and decompress on
  export;
- or refuse export while `compression_state == "compressing"`, so the
  transitional state is never observable.

## Two things import must preserve

- **`kind`.** The README is explicit that `Standby` "does not allow hypervisor
  switching on restore/fork" while `Stopped` does. An imported snapshot that
  loses its kind loses that rule with it.
- **The source hypervisor.** `Snapshot.SourceHypervisor` exists internally but
  is not in the API's `Snapshot` schema. For a same-host snapshot that is fine —
  the hypervisor cannot have changed. For an imported one it is the difference
  between a restore and a corrupt guest, so it needs to travel with the
  snapshot and be checked at restore.

## What this enables

For us, a portable snapshot is the substrate half of a "capsule": a session's
exact state, content-addressed, verifiable, and restorable on a different
machine — so an agent session survives the loss of the host it was created on,
and can be handed between environments. We have built everything above the
substrate for it already (staged object store, content ids, verification,
compatibility gating, restore-or-refuse semantics) and gated it behind
capabilities that currently report `false`, because the bytes cannot cross the
API boundary. Those two endpoints are what would turn it on.
