# Tasks: barista-029-ax-consumer-spike

## 1. Pin and build

- [x] 1.1 Pin `google/ax` at one commit into `work/ax-spike/ax/`; build the
      `ax` binary and run its local smoke test; record commit hash, Go
      version, hypeman version, and hardware in `work/ax-spike/NOTES.md`.
      Resolve the design's open question (which config key points the
      controller at an external `HarnessService` endpoint).
- [x] 1.2 Implement the stub `HarnessService` in `work/ax-spike/stubharness/`
      from AX's published `proto/ax.proto`: in-memory turn list + monotonic
      counter, `/proc/uptime` reported per reply, nothing persisted; static
      linux/arm64 build.
- [x] 1.3 Package the stub as an OCI image (follow `scenario/Dockerfile`) and
      verify it loads under both `hypeman` and `fake`.
- [x] 1.4 Bring up the node agent with the `hypeman` runtime; smoke-test
      `barista create/start/exec --json` against the stub image.

## 2. Wiring and endpoint discovery (Q1)

- [x] 2.1 Attempt endpoint discovery **through Contract A alone**
      (`GetInstance`/CLI `--json`); when that fails, record the minimal
      out-of-band procedure with exact steps as finding F1 (or F0 if the
      guest is unreachable from a host process at all — then continue on
      `fake` per the design risk).
- [x] 2.2 Point `ax serve`'s registry at the instance endpoint; drive one
      conversation turn end-to-end (`ax --input …`); record what
      "wait-until-ready-then-dial" required of the consumer (polling loop,
      timeout, retries) as part of Q1's verdict.

## 3. The pause, from the consumer's seat (Q2–Q4)

- [x] 3.1 Complete a turn, then `barista pause --require-memory`; record
      verbatim what the AX server logs and what a connected client sees (Q3,
      first half).
- [x] 3.2 `barista resume --require-memory`, then drive the next turn:
      assert the turn counter continued (no reset) and `/proc/uptime` proves
      no reboot (Q2).
- [x] 3.3 Run ≥10 pause/resume cycles; record the consumer-visible resume
      latency distribution (first retry → first reply) and compare against
      the ~370 ms restore baseline (Q4).
- [x] 3.4 Repeat the 3.1→3.2 flow on `fake` (DISK_ONLY): record what the
      consumer observes, and whether the degradation is distinguishable from
      a memory resume from AX's seat — number or named failure either way.
- [x] 3.5 Sever a client mid-stream across a pause and exercise AX's own
      catch-up (`--resume`, `--last-step`); record whether the conversation
      recovers and what was lost (Q3, second half).

## 4. Sizing and report

- [x] 4.1 Size the adapter (Q5): probe line count, seams touched, and a
      short list of what a first-class Barista backend for AX would require
      (native `harness.Harness` vs endpoint-only), with the fork cost named.
- [x] 4.2 (stretch) Approximate the turn-boundary pause (Q6): drive
      pause-at-turn-end from the driver script and size the idle window it
      removes versus a TTL-triggered pause — number or named failure.
- [x] 4.3 Write `docs/ax-consumer-evidence.md`: per-question verdicts
      (Q1–Q6, F-findings), environment header (AX commit, hypeman version,
      arm64/`vz` evidence-gap clause), and the recommended follow-up
      proposals — explicitly noting none is ratified by this change.
- [x] 4.4 `openspec validate barista-029-ax-consumer-spike` passes and
      `make check` is green (workspace untouched); `work/ax-spike/NOTES.md`
      documents the exact reproduction invocation. This change claims no
      Phase 1 acceptance test (proposal — Acceptance).
