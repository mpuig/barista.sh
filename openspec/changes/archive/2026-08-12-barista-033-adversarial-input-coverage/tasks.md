## 1. Fuzz scaffolding (nightly, non-required)

- [x] 1.1 Add a `fuzz/` workspace (`fuzz/Cargo.toml`) with `cargo-fuzz`
  (libFuzzer) wired for `barista-guest-agent` and `barista-node-agent`; its own
  `[workspace]` plus root `exclude = ["fuzz"]` keep it out of the stable
  `make check` build, so the pinned 1.94.1 toolchain is unaffected.
- [x] 1.2 Add a nightly GitHub Actions workflow (`.github/workflows/fuzz.yml`:
  `workflow_dispatch` + `schedule`, `continue-on-error`, **not** a required check)
  that installs nightly, builds each target, runs each for a bounded budget
  (`-max_total_time=90`), and uploads any crash artifact. `RUSTUP_TOOLCHAIN=nightly`
  so the root pin cannot shadow it. Posture modeled on `beta` + `acceptance.yml`.
- [x] 1.3 Corpus is **generated at run time, not committed** (`fuzz/corpus` is
  gitignored): libFuzzer grows one from scratch each run, so 100 opaque machine-
  generated blobs have no place in a source repo. CI can cache it to stay warm.

## 2. Fuzz targets on the reachable surfaces

Target list refined during implementation (see design.md D2): `DeclareIdleRequest`
is an empty message (no parse surface) and `exec::serve` spawns the workload, so
the targets drive only pure, side-effect-free parse/validate paths.

- [x] 2.1 `bootstrap_decode`: `decode_value::<Process>` / `::<Hooks>` — the
  `base64(prost(...))` the substrate hands the guest verbatim at boot. 1.4M runs, 0 crashes.
- [x] 2.2 `spec_admit`: decode arbitrary bytes as `InstanceSpec`, then
  `admission::admit` — Contract A's parse plus the below-both-entrances validation
  (the highest-value target: `admit` is our logic). 667K runs, 0 crashes.
- [x] 2.3 `exec_frame_decode`: `ExecFrame` / `ExecStart` decode — the streamed
  frame parse, decode only (never `serve`). 4.0M runs, 0 crashes.
- [x] 2.4 `write_file_frame_decode`: `ReadFileRequest` / `WriteFileRequest` decode
  — the client-supplied path/mode parse, no filesystem touch. 5.2M runs, 0 crashes.

## 3. Deterministic tests in the stable suite (`make check`)

- [x] 3.1 Guest hostile-frame tests: a server-side-only frame, a wrong-typed first
  frame, and an oversized frame on the exec/file stream each yield an error and no
  panic — pins the "hostile management frame stream" scenario.
- [x] 3.2 Guest `DeclareIdle` malformed-input test: arbitrary/garbage bytes return
  an error and the agent keeps serving — pins the "malformed idle declaration"
  scenario. (DeclareIdleRequest carries no fields, so the malformation is at the
  wire; asserted by survival on the workload socket.)
- [x] 3.3 Guest bootstrap corrupt-decode test: a truncated/invalid bootstrap fails
  with a named error rather than panicking — pins the "corrupt bootstrap" scenario.
- [x] 3.4 Node Contract A malformed-message test: a truncated/invalid protobuf on
  the loopback surface fails as an error and the agent keeps serving. (Wire
  garbage over the gRPC port; semantic malformation is `admission::admit`, driven
  by the fuzz target and the admission unit tests.)
- [x] 3.5 Journal-liveness test: submit many operations concurrently against a real
  SQLite journal + stub runtime; assert every operation reaches a terminal state
  and an independent read keeps succeeding throughout (G3, behind the
  `await_holding_lock` lint).

## 4. Verification

- [x] 4.1 The cargo gates of `make check` pass locally: `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo fmt --check`, and the affected suites
  (guest-agent lib+contract+workload_idle, node `adversarial_node`). The remaining
  `task ci` gates (mkdocs, `buf lint`, `gen-check`, Docker `guest-bin`, pytest) are
  untouched by this change — no proto, doc, or Dockerfile changed — and run in CI.
- [x] 4.2 Each target ran locally under nightly for its budget with no crash
  (~11M executions total). Negative control confirmed: an injected `panic!` was
  caught immediately as `libFuzzer: deadly signal`, then reverted — so a green run
  means the harness actually detects crashes.
- [x] 4.3 No regression: T5 (`t5_crash.rs`) passes (2/0). T7 needs the KVM beta
  node so it is not run here, but it is structurally unaffected — the only
  production change is a behaviour-preserving refactor (`decode_value` extracted
  from `decode`); everything else is additive tests and one nightly job.
