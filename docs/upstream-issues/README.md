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
