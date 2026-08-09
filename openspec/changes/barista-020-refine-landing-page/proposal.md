## Why

The current landing page explains Barista’s pause/resume mechanism, but its bright, playful presentation obscures the sharper product promise: long-running stateful compute can remain ready without remaining active. The site should lead with that outcome in a quieter, more credible form for platform engineers and agent builders.

## What Changes

- Reframe the landing-page hero as one typographic sentence: “Your [agent / runtime / environment / session], always ready. Only awake when it matters.”, with only the workload noun rotating.
- Rewrite the supporting narrative so it consistently connects timely wake, exact-state continuity, zero idle compute, and the bounded workloads that fit the session model.
- Replace the colorful illustrated style with a light, warm, restrained editorial system led by typography, whitespace, hairline rules, and one sparingly used accent.
- Keep the coffee metaphor in concise language and small service details rather than a dominant cup illustration or multicolor sections.
- Present manual and scheduled wake as available today and request-driven wake as roadmap, without implying that Barista is a scheduler service or stateless PaaS.
- Correct the existing fleet claim: the bucket protocol and CLI ship, while node fleet membership remains the separate `barista-019-fleet-membership` change and is not yet an available multi-node mode.
- Preserve semantic structure, keyboard access, responsive behavior, readable contrast, and reduced-motion behavior; provide a stable accessible rendering of any rotating hero term.
- Keep the page static and lightweight, with no framework or runtime JavaScript dependency; self-host the minimal font assets needed for a consistent typographic identity.

## Capabilities

No product capability requirements change. This documentation-only change opts out of delta specs through `skip_specs: true`.

### New Capabilities

None.

### Modified Capabilities

None.

## Impact

- Primary implementation surface: `docs/index.html`, plus local licensed font files under `docs/assets/fonts/`.
- Design context: `.impeccable.md` records the ratified audience, personality, aesthetic direction, and design principles for this and future interface work; the same section is mirrored into `.github/copilot-instructions.md` at the user’s request.
- No protobuf, CLI, runtime, persistence, coordination, API, or dependency contract changes.
- No Phase 1 acceptance tests T1–T12 are claimed; definition of done is the page-specific checks in `tasks.md` plus the mandatory `make check` gate.

## Constitution Check

- **Schema-first:** unaffected; no contract types or protobufs change.
- **Honest capabilities:** the copy will distinguish command/scheduled wake available today from request-driven wake on the roadmap, and will not present Barista as a scheduler or general stateless service platform.
- **Crash-safe operations:** unaffected; no runtime mutation path changes.
- **Adopt the substrate:** unaffected; the page continues to describe Barista’s session layer without claiming substrate mechanics as Barista implementations.
- **Small, complete change:** one static landing page, its local font assets, and its design context are the complete scope.
- **Verification:** no T1–T12 behavior is modified or claimed; `make check` remains mandatory.
