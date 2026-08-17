# A forked instance keeps the source's in-VM network identity

**Versions:** hypeman-api built from `feat/report-fork-mode` (fork + fork_mode +
fork tag override), vz on Apple Silicon (`Virtualization.framework`).

## What happens

Fork a retained standby snapshot into a new instance brought up `Running`:

```jsonc
// POST /snapshots/{snapshotId}/fork
{ "name": "child", "target_state": "Running", "tags": { "…": "…" } }
```

The fork succeeds (`201`) and hypeman allocates the fork a **new** network
identity — a fresh IP, MAC, and tap:

```
allocated network  instance_name=…-child  ip=192.168.64.174  mac=02:00:00:87:3a:a2  tap=hype-b1r530q9
```

But the forked guest is unreachable at `192.168.64.174`. Its kernel still has
the **source's** IP on `eth0`, because:

- the guest's IP is configured **once, statically, at initrd boot**
  (`lib/system/init/network.go`: `ip addr add <GuestIP>/<CIDR> dev eth0`), and
- a fork **resumes from the source's memory image** — init never re-runs — so
  `eth0` keeps the source's address while hypeman hands the fork a different one
  at the host.

Nothing in the daemon log reconfigures the fork's guest: there is an `allocated
network` line for the fork and **no reconfigure line**.

## Why it is hard to notice

The fork returns `201` and `GET /instances/{id}` reports the new IP, so a caller
reads the fork as reachable at that IP. It is not: the address exists on the
host side (tap/NAT) but no service inside the guest is bound to it. The failure
is a connection timeout at the advertised address, not an error on the fork.

## The mechanism already exists — restore uses it, fork does not

hypeman's guest agent has a `ReconfigureNetwork` RPC that rewrites the guest's
MAC/IP/gateway over vsock via netlink (`lib/system/guest_agent/network.go`), and
the **restore** path calls it:

```
lib/instances/restore.go:344   if allocatedNet != nil && !stored.SkipGuestAgent {
lib/instances/restore.go:350       reconfigureGuestNetwork(ctx, stored, allocatedNet)
```

The reconfigure is gated on `allocatedNet != nil`, which is only set in
restore's *fresh-allocate* branch. A fork pre-allocates its network during
fork-prepare (`prepareForkWithAliasReadLock` with a `ForkNetworkConfig`), so when
the fork's `Running` transition runs `restoreInstance`, that branch is not taken,
`allocatedNet` stays `nil`, and the reconfigure is skipped.

## Suggested fix

Reconfigure the forked guest's network to its allocated identity on the fork's
`Running` transition, the same way restore does — either by ensuring
`allocatedNet` is set for a fork that took a new identity, or by calling
`reconfigureGuestNetwork` explicitly in the snapshot/instance fork `Running`
path after the allocation is known.

## Open question for the fix

Whether `ReconfigureNetwork` reaches the guest depends on hypeman's guest agent
running in the deployment. A consumer that injects its **own** guest entrypoint
(as Barista does) may not have hypeman's vsock guest agent present, in which case
the exec fallback (`reconfigureGuestNetworkWithExec`) is what runs — that path
should be confirmed to work, or the reconfigure should be performed by whatever
guest agent the deployment does run.

## Impact on the consumer

Fork works end to end at the substrate — the child VM boots from the source's
state, gets its own host network, and is correctly tagged — but a consumer
cannot reach a service inside the fork until the guest picks up its new IP. Every
other fork guarantee (isolation, lineage, measured mode) holds.
