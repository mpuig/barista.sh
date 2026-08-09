# Design: nap-004-runtime-substrate-spike

## Decisions

1. **Requirements before evidence.** The `runtime-substrate` capability is
   written first, from what nap-002/nap-003 already depend on, so the evaluation
   scores hypeman against a fixed bar rather than against whatever it happens to
   be good at. Any future substrate — runsc, firecracker, hypeman — is judged by
   the same list.
2. **The vz snapshot question is the gate.** Task 1.2 runs before anything else,
   because a `no` there demotes the strongest argument for hypeman (local dev
   parity on Apple Silicon) and turns the rest of the evaluation into a
   Linux-only comparison against runsc — which is a much closer call.
3. **Contract C injection is the second gate.** Nap's differentiator lives in the
   guest agent. If `nap-guest-agent serve` cannot become the workload entrypoint
   inside hypeman's initrd + overlay boot, and the host cannot reach it over a
   unix socket, then adopting hypeman costs us readiness, hooks, TTL activity and
   restore-time duties — and no amount of snapshot speed pays for that.
4. **Measured, not borrowed** (Constitution III). "Resume in milliseconds",
   8× oversubscription and 1M browsers/month are the vendor's numbers for
   4-minute-lifetime browsers. Nap's workload is a long-lived agent session that
   pauses for minutes to hours. Every number in the annex comes from a run on our
   own hardware with an in-memory workload, or it is quoted and attributed.
5. **Timebox and a stop rule.** The spike ends when the annex can answer the
   Contract B checklist, or after its timebox, whichever comes first — with
   "insufficient evidence, keep runsc-first" being a legitimate and cheap
   outcome. A spike that cannot fail is not a spike.
6. **Nothing lands in `crates/`.** Probe code lives in `work/` (already
   gitignored). The temptation to "just start the backend while we're in here" is
   how a spike becomes an unratified architecture decision.

## Risks / Trade-offs

- **Two-day-old dependency.** Open-sourced 2026-08-04, early version range. MIT,
  so forkable, but the API will churn and the evaluation must weigh that against
  the maintenance cost of owning a runsc integration ourselves.
- **Workload mismatch.** hypeman optimises VM lifecycle as a hot path because its
  median VM lives 4 minutes. Nap's sessions are the opposite shape: long-lived,
  long-paused. Their standby-density work helps us; their fork/churn work mostly
  does not. This cuts against over-reading their scale evidence.
- **No shared-kernel tier.** hypeman is hypervisor-only. Adopting it exclusively
  removes the cheap shared-kernel option, which may matter for the voice-agent runtime's
  per-call runtimes (BRD §11.3) on density and cost. The dual-tier
  recommendation exists for this reason.
- **arm64 vs x86_64 evidence.** Findings on Apple Silicon may not transfer to
  x86_64 production. The annex must say which architecture each measurement came
  from, and flag any conclusion that rests on only one.
- **Spike-shaped bias.** Evaluating a shiny new substrate right after reading its
  launch post invites motivated reasoning. Mitigation: decision 1 (fixed bar) and
  decision 5 (a "keep runsc-first" outcome is explicitly acceptable).
