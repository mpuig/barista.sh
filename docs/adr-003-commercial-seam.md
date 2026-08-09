# ADR-003 — The commercial seam: a hosted multi-tenant service, out of tree

> Status: **PROPOSED 2026-08-09** — awaiting human ratification (constitution
> §V). Nothing in this ADR takes effect, and no BRD or constitution text is
> amended, until it is ratified. This is a boundary decision, not an evaluation:
> there is no spike behind it, only the two constraints the human set (§1) and
> the seam that follows from them.
>
> Scope note: this ADR governs **this (public) repository only**. The commercial
> service it refers to lives in a separate private repository with its own
> governance; its internal design is out of scope here by construction, and that
> is the whole point.

## 1. The question

Barista is to be offered as a **paid, hosted, multi-tenant service** — customers
pay to run their own fleets of agent/worker sessions. The human set two
constraints that frame the decision:

1. The service is built in a **separate, private repository**.
2. Isolation is **mixed by plan**: shared nodes on a basic tier, dedicated nodes
   on an enterprise tier — reached through a staged rollout (trusted-user trial →
   beta → production).

BRD OQ2 (`docs/BRD.md` — "Single-tenant or multi-tenant? … Multi-tenancy still
open") and NFR-4 ("multi-tenancy/isolation model — all undefined") are the
placeholders this was always meant to land in. The question this ADR answers is
narrow and structural: **given a private repo, which capabilities enter *this*
repository, and which do not?** The pricing, the tenant model, and the billing
pipeline are the private repo's to design; they are named here only to be placed
on the far side of the seam.

## 2. The decision

**The public repository does not become multi-tenant. Multi-tenancy remains a
non-goal here (§I), and is imposed from outside by the private service.**

The reasoning is a one-way dependency and a trust boundary that already exists:

- **Dependency direction is one-way.** The private service depends on this repo
  (as a versioned artifact: the proto contract package plus the released
  `barista` node/guest agent binaries). This repo never depends on, references,
  or knows about the private service. Anything else couples the OSS to a
  commercial roadmap and violates §IV's "clean seam for likely change".
- **The trust boundary is already where it needs to be.** Contract A carries no
  authentication and binds to loopback by design (`crates/barista-node-agent/src/lib.rs`
  refuses any non-loopback listen address; `src/main.rs` — "Owner-only,
  explicitly"). The private gateway sits in front of it and *is* the boundary
  between untrusted callers and the node. The node agent therefore needs no
  concept of a customer: its only caller is a trusted operator, exactly as today.
- **Multi-tenancy is a property imposed above the contract, not a field inside
  it.** A tenant is realised as (a) a namespace on session names and bucket
  prefixes — `desired/<tenant>/<name>` — which the node already permits since
  names and the bucket prefix are free-form; and (b) node-pool selection —
  dedicated pools per enterprise tenant, a shared pool for the basic tier. Both
  are choices the private placement layer makes when it writes desired state and
  when it labels nodes. The node agent enforces neither and needs to learn
  nothing.

The consequence worth stating plainly: **the "owner" this repo already
knows is a node holding a lease** (`openspec/specs/fleet-coordination/spec.md`
— "Exactly one owner per session name", owner = node). It is not, and under this
ADR does not become, a customer. Tenancy is genuinely net-new, and it is net-new
*in the private repo*.

## 3. What enters this repository — the neutral seams

Only capabilities that are useful to **any** operator, independent of the
commercial product, may land here. Two do, and they are the entire public
surface of this change:

1. **Structured usage events on the existing event stream.** `WatchEvents`
   (`proto/barista/node/v1alpha1/node.proto`) is operational/lifecycle today. It
   gains session-scoped usage facts — session-seconds, resource-hours, snapshot
   bytes-retained, wake count — as ordinary events. These are observability for
   any operator; the *rating and billing* that consume them are private. This is
   a proposal to be specified as a change (candidate `barista-022-usage-events`),
   not an edit.
2. **An authenticated Contract A for a remote caller, opt-in.** The gateway will
   usually be co-located with the node agent, and loopback then remains the whole
   story. Where an operator runs the caller off-host, Contract A gains a pluggable
   authenticating transport (mTLS, reusing `barista-021`'s posture) so the
   loopback-only rule can be relaxed *deliberately* rather than by removing the
   guard. Default is unchanged: loopback, no auth. Candidate
   `barista-023-node-api-auth`.

Nothing else. In particular, the tenant model, the gateway, customer auth/RBAC,
metering aggregation, rating, billing, quotas, per-tenant rate limits, the
dashboard, tenant-aware placement, and abuse detection are **out of tree** and
named in §4 only to be excluded.

## 4. What stays out of this repository — the private service

For the record, so the seam is unambiguous. The private repo owns:

- the **tenant / account model** and its store (Postgres per OQ4's stack);
- the **public authenticated gateway** (API keys / OAuth, RBAC, per-tenant rate
  limits) — the sole caller of Contract A, and the realisation of the BRD's
  long-deferred Phase 5 "session interface" (wake-on-request, request parking,
  hibernating connections — B7/B44/B54);
- **namespacing enforcement** (choosing `desired/<tenant>/<name>` and the pool);
- **metering → rating → billing** (Stripe), consuming §3.1's events;
- **quotas** — deliberately here and not in the node, consistent with this repo's
  own prior refusals to smuggle policy in as constants (`nap-015`, `nap-008`
  designs);
- **tenant-aware placement** over shared vs dedicated node pools, and
  **abuse detection** for the shared tier.

## 5. The one gate this ADR does not close: §13.4

The basic (shared-node) tier runs **untrusted customer code on shared hardware**.
That makes the BRD's §13.4 hardware-isolation guarantee — open to this day —
**load-bearing before the shared tier ships**, not before the enterprise tier.
This ADR does not answer it; it places it. The public capability already exists
and already fails closed: `require_hardware_isolation` returns `CAPABILITY_MISSING`
rather than silently degrading (T12; `crates/barista-node-agent/src/admission.rs`),
and `nap-014` gives per-instance egress the same honest-or-refuse treatment. The
private service must *require* both on the shared pool and answer §13.4 in its own
design before a paying stranger's code lands on a node beside another's. Until
then the staged rollout ships **dedicated nodes only** — an isolation model this
repo already delivers, with a blast radius of one tenant per node.

## 6. Effects on ratification

None are executed until the human ratifies. On ratification:

1. BRD OQ2 is answered: *multi-tenant — yes, as a private downstream consumer;
   this repo stays single-tenant by construction.* NFR-4's multi-tenancy clause
   is answered the same way — the model lives out of tree.
2. **No constitution amendment is required, and that is the test this framing had
   to pass.** §I still holds unedited: Barista serves session-centric compute for
   its consumers, and the hosted service is a fourth such consumer (the BRD already
   contemplates a fourth consumer) reached through the same contract — not a new
   purpose for this repo. Had the clean answer required editing §I, it would have
   been the wrong answer.
3. Two changes become proposable here: `barista-022-usage-events` and
   `barista-023-node-api-auth` (§3). Both go through the ordinary OpenSpec
   workflow; the latter, if it touches the wire, goes through the ratified
   `contracts` capability. The design target is that the private service needs
   **zero** new proto fields — tenancy imposed above the contract, never inside it.
4. The private repo's design document (architecture, tenant model, rollout) is
   authored there, referencing this ADR as its upstream boundary.

**Stop: this ADR takes effect only on human ratification** (constitution §V),
which also authorises the two candidate changes in §3.
