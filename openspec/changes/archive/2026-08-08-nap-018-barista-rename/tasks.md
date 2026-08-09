# Tasks: nap-018-barista-rename

## 1. The gate

- [x] 1.1 Human accepts the `v1alpha1` package rename of both contracts
- [x] 1.2 Human accepts constitution amendment v1.4.0 (`design.md` §7) and it
      is appended to `CLAUDE.md` → Governance
- [x] 1.3 Human answers the Open Questions in `proposal.md`
      > **Sequencing resolved by events**: `nap-017-fleet-coordination` was
      > archived and `crates/nap-fleet` committed before the rename started, so
      > only `nap-014-egress-policy` was live and its live artifacts are renamed.
      > **Consumer migration**: answered by 7.5 — there are no consumers yet.
      > **Wire surface**: answered in §8; all of it renamed.
      > **Still open**: whether `barista-019` (the `pull`/`hold`/`grind` verb
      > surface) is proposed at all. That is a new change, not a blocker here.

## 2. The contract

- [x] 2.1 Move `proto/nap/` → `proto/barista/`; change the two `package` lines
      to `barista.node.v1alpha1` and `barista.guest.v1alpha1`. Nothing else in
      the two files is edited
- [x] 2.2 `buf lint` green
- [x] 2.3 Capture the pre-rename descriptor sets from the parent commit, and
      diff them field-by-field against the new ones ignoring the package
      component — the delta spec's "moved the address and nothing else"
      scenario. This is the evidence, so it is produced before the code that
      depends on it
      > Built `.git#branch=main` and the working tree with `buf build`, then
      > compared with package, proto path and `sourceCodeInfo` normalised away.
      > **Structurally identical**: 63 messages, 218 fields, 2 services, 26
      > RPCs, matching either side. The only raw difference was the character
      > span of the `package` line — "barista" is four characters longer than
      > "nap" — which is source position, not contract.
- [x] 2.4 Add the scoped `buf.yaml` breaking exception in the form the ratified
      `contracts` capability requires — narrowest scope that works, naming its
      ratification in the comment
      > `except: [FILE_NO_DELETE]`. Confirmed the gate genuinely fails first
      > (exit 100, "Previously present file nap/node/v1alpha1/node.proto was
      > deleted"), then passes with the exception (exit 0). `buf lint` stays
      > green. The comment states its ratification and that task 7.4 removes it.

## 3. Generated code

- [x] 3.1 `crates/nap-proto` → `crates/barista-proto`; regenerate
- [x] 3.2 `crates/nap-proto-gen` → `crates/barista-proto-gen`; regenerate
- [x] 3.3 Python generated package follows the same rename
      > `py/nap-proto` → `py/barista-proto`, and `scripts/gen_py.py` now emits
      > `src/barista/**`. The superseded `src/nap/**` tree was `git rm`-ed
      > rather than left behind — a stale generated copy of the old package is
      > exactly the "hand-written duplicate of contract types" §I forbids.
- [x] 3.4 Generated code is in sync with `proto/`
      > `gen-check` compares worktree against index, so it cannot distinguish
      > "generated code is stale" from "the rename is uncommitted". Staged the
      > tree, re-ran `task gen`, and `git diff --exit-code` on both generated
      > trees is clean — regeneration produces nothing new.

## 4. The crates

- [x] 4.1 `crates/nap-node-agent` → `crates/barista-node-agent`
- [x] 4.2 `crates/nap-guest-agent` → `crates/barista-guest-agent`
- [x] 4.3 `crates/nap-cli` → `crates/barista-cli`; binary `nap` → `barista`
- [x] 4.4 `crates/nap-fleet` → `crates/barista-fleet`
- [x] 4.5 Workspace members, dependency edges, `Cargo.lock`
- [x] 4.6 `cargo build --workspace` green
      > **`cargo build` is not sufficient and nearly hid a defect.** `\b` does
      > not fire after an underscore, so `env!("CARGO_BIN_EXE_nap-node-agent")`
      > in `t5_crash.rs` survived the substitution pointing at a binary that no
      > longer exists. `cargo build` does not compile tests, so it passed.
      > Caught by `cargo test --workspace --no-run`; two occurrences of the
      > class fixed. Test compilation is now part of this task's proof.
- [x] 4.7 `cargo fmt --all`
      > Required by the rename, not incidental: `barista_proto` sorts before
      > `common` where `nap_proto` sorted after it, and several lines crossed
      > 100 characters. Every reformatted line is an import move or a wrap.

## 5. Specs and docs

- [x] 5.1 `openspec/specs/nap-cli/` → `openspec/specs/barista-cli/`
- [x] 5.2 The other nine ratified capability specs
- [x] 5.3 `docs/` — 28 files
      > One hand fix beyond the script: the stylised brand initial `N.` in
      > `docs/index.html` now sits beside the wordmark "Barista".
- [x] 5.4 `CLAUDE.md` constitution prose; amendment log appended, not rewritten
      > Four surgical edits. The script was deliberately **not** run on this
      > file: it would have rewritten the `nap-00N` change IDs inside the
      > amendment log, which is the one thing §3 of the design forbids.
- [x] 5.5 `README.md`, `Makefile`, `Taskfile.yml`, CI, `scripts/`, `scenario/`
      > One hand fix: the script turned the test prompt `"after the nap"` into
      > `"after the barista"`. There `nap` was the ordinary English word for
      > the pause that had just happened, not the product. Reverted.
- [x] 5.6 `openspec/changes/nap-014-egress-policy` live artifacts
- [x] 5.7 `openspec/config.yaml`
      > Found by the 7.3 audit, not by any partition — it sits in `openspec/`
      > but outside `openspec/specs/`, so it fell between the file sets. It
      > carried both proto package names in the project description.
- [x] 5.8 `vendor/hypeman/README.md`
      > Vendored directory, but this file is our own prose explaining why we
      > vendored, and it cited `crates/nap-node-agent/tests/…` — a path the
      > rename had just invalidated.
- [x] 5.9 Local build artifact names: `nap-scenario` image, `nap-test-output.log`,
      `nap-guest-registry` / `nap-guest-target` docker volumes
      > Renamed because they cross no boundary another party observes — the
      > same category as a crate name. Distinct from §8, deliberately.

## 6. What is deliberately left alone

- [x] 6.1 `openspec/changes/archive/` untouched — verified: 0 files modified or
      deleted under it
- [x] 6.2 Amendment log v1.0.0–v1.3.0 unchanged — verified: 0 removed lines
      matching the version headers or `nap-00N` IDs
- [x] 6.3 Git history not rewritten (human decision, this session)

## 7. Verification

- [x] 7.1 `make check` green
      > **Green in the `fake` profile, which is the profile the constitution
      > means.** Exit 0, with `check_skips` classifying all four skips as
      > expected — "needs a runtime that provides hardware isolation", "needs a
      > runtime with memory_snapshot", "no hypeman token available", "no
      > hypeman token configured" — and reporting no violations. Skipping is
      > not hiding here: `check_skips` exists to make the skips part of the
      > claim, and fails closed in CI.
      >
      > **Red on a host that reaches a non-enforcing hypeman**, which this
      > developer machine does via `~/.config/hypeman/cli.yaml`:
      > `hypeman_preflight::a_provisioned_host_reports_no_problems` fails
      > because the substrate schema-validates `network.egress` and enforces
      > nothing. That is `nap-014` task 5.5 — open, filed upstream — plus
      > commit 7c3674e and ADR-002 §66. It is a property of the substrate, not
      > of this change: the rename's entire touch to `preflight.rs` is one
      > identifier, and the failure predates it.
      >
      > Closing it belongs to `nap-014`, and it closes by a hypeman build that
      > enforces what it validates — not by anything in this change.

- [x] 7.2 Acceptance tests pass with **no edits to their assertions**
      > 27 test files changed, 192 insertions and 192 deletions — exactly
      > symmetric. Filtering the test diff for lines that are not identifier
      > renames returns nothing.
- [x] 7.3 Audit: zero `nap` identifiers outside the archive and amendment log
      > Clean, after 5.7 fixed the one real miss it found.
- [x] 7.4 Remove the 2.4 exception; `buf breaking` green on the following
      commit — the ratified scenario *an authorized break leaves no permanent
      hole*
      > Done in the commit after 742d6d2. `buf.yaml` is byte-identical to its
      > pre-rename state, and `buf breaking --against main` exits 0 without any
      > exception, waiver or skip. The gate was red on purpose for exactly one
      > merge.
- [x] 7.5 Consumers regenerate against `barista.*` and build, per 1.3
      > **Nothing to migrate**: the agent platform, the preview-env platform and the voice-agent runtime are not yet
      > configured against this runtime, so no consumer has generated code,
      > pinned a package or set a `BARISTA_*` variable. This is the window the
      > proposal's premise named, and it is still open — which is what made the
      > whole rename, including the wire surface in §8, cost one coordinated
      > cut of nothing at all.

## 8. Raised during implementation — partly decided

The proposal claimed *"the break is the package path and nothing else"*. That
claim is **no longer true**, by the human's decision taken during
implementation, and the amendment and proposal are corrected to match.

- [x] 8.1 Rename the 21 `NAP_*` environment variables to `BARISTA_*`
      > **These are an interface, and the rename proved it.** The prebuilt
      > guest agent in `.tools/guest` — a cross-compiled musl artifact — still
      > looked for `NAP_GUEST_SOCKET` while the node agent had started
      > injecting `BARISTA_GUEST_SOCKET`. Two `barista-cli` tests failed with
      > `GUEST_UNREACHABLE` until `task guest-bin` rebuilt it from the new
      > source (verified: different sha256, and the new binary's string table
      > carries `BARISTA_INSTANCE_TOKEN` where the old one carried `NAP_`).
      > The superseded `.tools/guest/nap-guest-agent` was removed.
      > 27 files, 66 insertions and 66 deletions, symmetric.
      > **Consequence for 7.5**: anything outside this repository that sets
      > these variables — CI, deploy scripts, a consumer harness — must change
      > in the same cut as the proto packages. That is a wider migration than
      > the proposal originally described.

- [x] 8.2 Rename the gRPC metadata keys and the in-sandbox paths
      > `nap-instance-token` → `barista-instance-token`, `nap-reason` →
      > `barista-reason`, `/nap-secret` → `/barista-secret`,
      > `/tmp/nap-bridge.log` → `/tmp/barista-bridge.log`. The guest agent
      > embeds the metadata key, so `.tools/guest` was rebuilt again and
      > verified by string table.
      >
      > **The consequence, stated rather than discovered:** none of 8.1 or 8.2
      > is backward compatible with a sandbox created before the cut. A sandbox
      > carries the guest agent binary current at its creation, so a session
      > paused under the old names and resumed under the new ones fails to
      > authenticate. For a product whose promise is that sessions resume
      > intact, that belongs in the release note, not in a bug report.

- [x] 8.3 Rename the hypeman resource identifiers too
      > Unblocked by the human's statement that nothing created before this
      > change needs to survive it. `HypemanRuntime::sandbox_name` now emits
      > `barista-<node>-<instance>`, `token_volume::ID_PREFIX` is
      > `barista-token-`, and the fake runtime's sandbox names follow.
      >
      > **The hazard this removes is conditional, not gone.** On a substrate
      > holding volumes created before the cut, `is_token_volume` — "the only
      > way to recognise one among volumes Barista did not create" — stops
      > matching them, and the `nap-016` reaper leaks orphaned credentials it
      > can no longer see. This is safe here because there is nothing to
      > preserve. Pointing this build at an older substrate reintroduces it and
      > would need dual-prefix recognition first.
      >
      > `nap-` followed by a digit was excluded from the substitution: those
      > are archived change IDs, and `service.rs` still answers
      > `unimplemented_until("nap-005-hypeman-backend")` as it should.
