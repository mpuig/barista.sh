# Design: nap-018-barista-rename

## 1. Rename the proto package, or brand over it?

**Decision: rename the package.**

The cheaper-looking option is to keep `nap.node.v1alpha1` on the wire and call
the product Barista everywhere else. It costs nothing today and never turns
`buf breaking` red.

It is rejected because the cost it avoids is one-time and the cost it creates
is permanent. The protos are, by Constitution I, the only contract; they are
the artifact the three consumer teams read most and the one place where a
name is load-bearing rather than decorative. A contract whose package says
`nap` for a product called Barista makes every new integrator ask which name
is real, forever. The simplest design (Constitution IV) is the one where the
name in the contract and the name of the thing are the same string.

The window matters. This is affordable *because* no consumer has pinned a
released package. That is a fact about today, and it is the whole argument
for doing it now instead of when it is convenient.

## 2. How much of the contract changes?

**Decision: the package path, and nothing else.**

Message names, field names, field numbers, `oneof` layouts, service names,
RPC names and streaming shapes stay byte-for-byte identical. The diff is two
`package` lines plus a directory move.

This is deliberate and it is what makes the migration mechanical rather than
risky: a consumer regenerates, changes its import path, and every call site
compiles unchanged. It also means the acceptance tests are a real proof —
if T1–T12 (except T2, T11) pass afterwards without edits to their assertions,
the change provably did not alter behaviour.

The temptation to fold in the coffee vocabulary here (`Session` → `Shot`,
`Snapshot` → `Crema`) is rejected on two grounds. Constitution II: one change,
one outcome. And more importantly, the contract is read by three teams who do
not live inside the metaphor — a schema in café jargon is a schema that needs
a translation table in the README within a year. The coffee belongs to the
brand and the CLI surface, not to the wire.

## 3. What happens to `nap-001` … `nap-017`?

**Decision: they keep their names. New changes start at `barista-019`.**

Seventeen change IDs appear in archived proposals, in the archived changes'
own directory names, and — the binding case — inside the constitution's
amendment log, where v1.1.0 explains the renumbering of
`nap-004-runtime-substrate-spike` and v1.2.0 and v1.3.0 turn on what
`nap-005` became.

Rewriting those strings would make the amendment log describe a sequence of
events that did not happen under the names it now claims. The log's only
value is that it is a faithful record of how the project actually decided
things; a record edited for cosmetic consistency is worth less than an
inconsistent one that is true.

The cost is that two prefixes coexist in the repository forever. That cost is
visible, bounded, and confined to `openspec/changes/archive/` plus one section
of `CLAUDE.md`. It is the right trade.

This change is itself numbered `nap-018` for the same reason: at the moment it
is written, the project is still Nap. The prefix changes *after* it, not
during it.

## 4. The breaking gate — already solved, do not re-solve

`Taskfile.yml` runs `buf breaking --against ".git#branch=main"` and `buf.yaml`
selects rule category `FILE`, which includes `FILE_SAME_PACKAGE`. A package
rename therefore fails the gate by construction.

An earlier draft of this design proposed a new one-time exception to
Constitution III, granted by the amendment. That was wrong: the ratified
`contracts` capability **already defines this mechanism**, and has since
nap-001.

> An authorized breaking change SHALL be expressed as a scoped exception in
> `buf.yaml` that names its ratification, and that exception SHALL NOT survive
> the change that introduced it: once the comparison baseline includes the
> break, the gate SHALL pass with the exception removed. A breaking change
> SHALL NOT be accommodated by weakening the baseline the gate compares
> against.

So there is no design decision here and no constitution exception to grant.
This change is an ordinary consumer of an existing ratified requirement: add
the scoped exception naming its ratification, remove it in the same change,
prove the gate is green afterwards. The delta spec does not modify
`Contract evolution discipline`, and amendment v1.4.0 says nothing about §III.

Worth stating because it is the second time this repository has been ahead of
the proposal: the requirement's third scenario — *an authorized break leaves no
permanent hole* — is exactly this change's task 7.4.

## 5. Order of operations

The rename must be one atomic merge, not a staged migration. A half-renamed
workspace does not compile, and the contract cannot be half-addressed.

Within the merge the order is forced by the dependency graph:

1. `proto/` — the package lines and the directory move
2. `barista-proto` / `barista-proto-gen` — regeneration, so the new symbols exist
3. the three agent/CLI crates that consume them
4. `barista-fleet`
5. docs, specs, scripts, CI, scenario harness
6. removal of the one-time breaking exception

Steps 1–4 are the compile-order; nothing between them is independently
mergeable.

## 6. What proves it worked

Not a green build alone — a green build only proves the rename was
syntactically complete. The proof is the acceptance suite: T1–T12 except T2
and T11 passing with **no edits to their assertions**. If an assertion had to
change, something other than a name changed, and the premise of this design
is falsified.

Second proof: an audit for surviving `nap` identifiers outside the archive and
the amendment log. Expected result is zero.

## 7. Proposed constitution amendment v1.4.0

To be appended to `CLAUDE.md` → Governance **only after the human accepts**.
Drafted here rather than applied, because recording an amendment is the act of
ratifying it and that is not mine to perform.

> - `v1.4.0 — 2026-08-08 — The project is renamed Nap → Barista.`
>   **Reason:** the product ships as **Barista** (`barista.sh`). The binding
>   constraint "schema-first: the `nap.node.v1alpha1` / `nap.guest.v1alpha1`
>   protos are the only contract" names the contract by a package path that no
>   longer matches the product, and the protos are the artifact all three
>   consumers read most. Renaming is affordable exactly once — while no
>   consumer has pinned a released package — and this is that moment.
>   **Consequence:** §I's schema-first constraint now reads
>   `barista.node.v1alpha1` / `barista.guest.v1alpha1`; crates become
>   `barista-*` and the delivered binary becomes `barista`; the ratified
>   capability `nap-cli` becomes `barista-cli`. Message names, field names and
>   field numbers are unchanged, so no capability, guarantee or acceptance
>   test is affected. **No exception to §III is granted or needed** — the
>   ratified `contracts` capability already governs authorized breaking
>   changes, and the rename uses that mechanism unmodified: a scoped `buf.yaml`
>   exception naming this ratification, removed inside the same change.
>   **Migration:** identifiers and paths only. Change IDs `nap-001` …
>   `nap-018`, the archived changes carrying them, and the amendment entries
>   above are **not** renamed — they are a historical record, and editing them
>   for cosmetic consistency would make this log describe events under names
>   they never had. New changes begin at `barista-019`. The Phase 1 sequence,
>   ADR-001 v2, ADR-002, and the deferral of T2 and T11 are untouched.

## 8. Rejected: a deprecation window serving both packages

Generating and serving `nap.*` and `barista.*` side by side would let each
consumer migrate on its own schedule. It is rejected as speculative machinery
(Constitution IV): it doubles the generated surface and adds a dual-dispatch
seam in the node agent to solve a coordination problem that, on this
proposal's premise, does not exist — no consumer has pinned.

If the human's answer to Open Question 1 is that a window *is* needed, that
premise is false and this section is the one to reopen: the window is
buildable, but it is a different and larger change, and it should be proposed
as one rather than smuggled in here.
