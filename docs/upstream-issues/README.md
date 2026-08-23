# Upstream issue drafts — `hypeman`

Ready-to-file distillations of `docs/upstream-hypeman-findings.md`, in filing
order. §2 of the findings (macOS guest network) is **not** here — it is already
upstream as hypeman **#358**.

Filing these is what eventually deletes Barista's patched-initrd workaround and the
version-skew asterisk on every linux/arm64 measurement. Until the first one
lands upstream, the findings doc stays the source of truth; these are copies
shaped for their issue tracker, not new information.

| draft | findings § | severity |
|---|---|---|
| `01-arm64-release-embeds-x86-64-guest-binaries.md` | §1 | blocking |
| `02-ingress-dns-collides-with-systemd-resolved.md` | §3 | minor, silent |
| `03-missing-mkfs-erofs-fails-images-without-a-cause.md` | §4 | minor, diagnostics |
| `04-bad-hypervisor-answers-a-bare-500.md` | §5 | minor, diagnostics |
| `05-egress-policy-is-validated-but-not-enforced.md` | §6 | **high, silent** |
| `06-expose-vsock-for-a-third-party-guest-agent.md` | §7 | feature request |
| `07-forked-guest-keeps-source-network-identity.md` | fork (barista-046) | high, silent |
| `08-transfer-snapshot-bytes-for-portable-snapshots.md` | capsules (barista-046 §4) | feature request |

`05` is the one to file first, and it is a different kind of finding from the
four above it: those cost a debugging session, this one costs a wrong security
decision. A caller reads `201` as confinement and gets open outbound, and the
API offers no response it could have checked instead.

`06` and `08` are the entries here that are not defects. Both are filed the way
a feature request earns its place — by naming what it replaces. `06` would
delete a mechanism Barista has already built and would rather not own
(`barista-021` — per-instance mutual TLS). `08` would delete the only remaining
alternative for moving a snapshot between hosts, which is reading hypeman's
`data_dir` behind its back: that hard-codes an on-disk layout into a consumer,
breaks over a network transport, and races the background compression the API
does not expose. It is the one blocking barista-046 §4 from reporting
`capsule_export` / `capsule_import` as anything but `false`.

Filing `08` early is deliberate even though the implementation is not waiting on
it. Barista develops against a fork build (`scripts/dev-deploy-fork.sh`), as it
already does for #419 — but an API shape is much cheaper to change while it is
still a proposal than after a consumer has been built on it.
