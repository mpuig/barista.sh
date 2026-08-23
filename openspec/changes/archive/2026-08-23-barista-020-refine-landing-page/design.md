## Context

See `proposal.md` — Why. `docs/index.html` is a standalone English marketing page with embedded CSS, no build pipeline, and no JavaScript. Its current multicolor sections, oversized cup illustration, thick outlines, and heavy display weights make the page feel playful where the intended audience needs calm technical credibility.

The content must remain true to the product sources:

- BRD §1 defines the target as named, long-lived, single-writer sessions for agent sandboxes, development environments, and stateful interactive applications; stateless PaaS is out of scope.
- BRD R-SNAP-1 and the Phase 1 spec driving case define the differentiator as resuming the same workload with full memory context intact.
- BRD NFR-1 records the 368 ms median and its measurement conditions; the page may repeat the measured number but not broaden it into an availability guarantee.
- Scheduled wake is delivered by the `instance-lifecycle` capability and derives from B56. Wake-on-request remains the Phase 5 gateway work (B7/B44).
- ADR-002 and BRD Phase 3 explicitly reject a scheduler service. The landing page must not imply otherwise.
- `docs/concepts/fleet-coordination.md` records that the bucket protocol and CLI ship but node-side fleet membership does not; `barista-019-fleet-membership` is still planning that wiring. The landing page must not call multi-node operation ready.

The confirmed design context is in `.impeccable.md`: platform engineers and agent builders; serene, precise, editorial; light and warm; coffee references kept subtle.

## Goals / Non-Goals

**Goals:**

- Make the readiness/wake promise understandable before explaining pause mechanics.
- Show breadth across agents, runtimes, environments, and sessions without suggesting general stateless hosting.
- Establish a consistent warm editorial system whose hierarchy comes from typography, spacing, and rules rather than multiple colored surfaces.
- Preserve all important proof, capability boundaries, and current-versus-roadmap distinctions.
- Keep the page fast, responsive, semantic, keyboard accessible, and useful without motion.

**Non-Goals:**

- Changing product capabilities, documentation outside the landing page, or any runtime/API contract.
- Introducing a site generator, component framework, analytics, or runtime JavaScript dependency.
- Promising wake-on-request, node-loss durability, a scheduler service, or general service hosting.
- Creating a broad design system beyond the tokens and patterns needed by this page.

## Decisions

### 1. Lead with readiness and use rest as the supporting mechanism

The hero will use this hierarchy:

- One headline sentence: `Your agent, always ready. Only awake when it matters.`, with only the noun rotating through `runtime`, `environment`, and `session`.
- Qualification: Barista retains memory, disk, and working context while the session rests, then resumes the same process on command or on schedule.

The workload subject and promise will form one `h1` at one display size: `Your agent, always ready. Only awake when it matters.` The noun rotates through `agent`, `runtime`, `environment`, and `session`; braces and other syntax stay out of the reel so only the workload noun changes. Line breaks provide editorial composition without demoting the subject to a smaller label. The reel uses only CSS transforms, keeps its width stable at the longest term, and has a static full-list equivalent for assistive technology. Under `prefers-reduced-motion`, it stops at `agent`. `service` and `runner` will not appear: the former implies the deferred request gateway/general PaaS, while the latter commonly implies disposable CI execution. This maps the message to BRD §1’s target personas and R-SNAP-1.

The simpler alternative was to keep all four terms in one small static line. Browser review showed that line became visually subordinate and failed to communicate flexibility at a glance. The reel makes the changing workload the page’s one signature motion while the sleep-first headline remains rejected because it explains the mechanism without establishing Barista’s active role.

### 2. Restructure the page as an editorial argument, not a sequence of color blocks

The page will retain the useful factual material but reorganize it into this reading order:

1. **Hero:** promise, concise qualification, calls to action, and a restrained session-status ledger.
2. **Core idea:** “Release the sandbox. Keep the session.” The hero copy and service record together explain continuity rather than reconstruction, without repeating the thesis in a separate section. The 368 ms median, five measured pauses, zero paused CPU/host RAM, and no-SDK boundary stay as concise prose evidence rather than a metric table.
3. **Lifecycle:** create, run, rest, wake, shown as a ruled sequence rather than cards.
4. **Where it fits:** cloud agent harnesses and long-running agents/workers marked today; stateful online services marked soon and bounded to the planned request gateway—not general stateless hosting.
5. **Available now:** exact-state pause/resume, CLI lifecycle, snapshots, scheduled wake, and the coordination protocol/CLI—explicitly not node-side multi-node operation yet.
6. **Ways to wake:** command and schedule marked available; request marked soon while remaining explicit roadmap. This is the main honesty boundary between B56 and B7/B44.
7. **Capability honesty and limits:** explicit guarantees, degradation, outage behavior, and the native macOS reachability gap.
8. **Closing:** GitHub call to action using the readiness message rather than another coffee pun.

The simpler alternative was a copy-only hero edit. It would leave the rest of the page visually and verbally centered on the old playful sleep metaphor, weakening the new message.

### 3. Use a warm near-monochrome palette with one restrained accent

The palette will be dominated by warm paper and coffee-tinted ink:

- paper: warm high-lightness neutral;
- ink: very dark brown-black rather than pure black;
- muted text and rules: lower-contrast tints of the same hue;
- accent: one muted oxblood/terracotta tone, reserved for the wake phrase, status marker, focus ring, and primary interaction.

Sections will differ through spacing, rules, and subtle alternating paper tones: the lifecycle and available-now chapters use the deeper paper while the use-case and wake chapters return to the light ground. This creates rhythm without turning the page into saturated mint, sky, coral, and apricot panels. Gradients, oversized circles, thick cartoon outlines, and generic drop-shadow cards will be removed. Contrast will meet WCAG AA.

The simpler alternative was to desaturate the existing five-color palette. Reducing saturation would still leave too many competing section identities; one accent better supports a calm, precise reading experience.

### 4. Pair Bodoni Moda with Schibsted Grotesk

The display face will be **Bodoni Moda**, used at restrained weights for the hero and major headings. Its high-contrast forms provide the elegant editorial voice. The text/UI face will be **Schibsted Grotesk**, a calm grotesk made for editorial reading and clear technical labels. Command output alone may use the existing platform monospace stack.

The fonts will be self-hosted as minimal variable WOFF2 assets under `docs/assets/fonts/`, accompanied by their OFL license material. `@font-face` will use `font-display: swap`; the page will preload only the above-the-fold faces and retain metric-compatible fallbacks. Headings will use a small fluid type scale with optical sizing; body copy remains at least `1rem`, 1.55–1.7 line-height, and approximately 65 characters per line.

The initial reflex choices—Newsreader, Instrument Serif, and IBM Plex Sans—were rejected as common defaults for this brief. The existing system-only Iowan/Avenir pairing was the simpler alternative, but it renders inconsistently across Linux, Windows, and Android and does not create a dependable public identity.

### 5. Replace the cup illustration with a session-status ledger

The right side of the hero will become a typographic operational ledger showing one session moving from `READY / RESTING` to an alarm and then `RUNNING / SAME MEMORY`. Thin rules, timestamps, and one small service mark retain the Barista metaphor without a literal oversized cup. The ledger will use claims already supported by the page and source documents.

This is more complex than omitting the hero visual entirely, but the ledger earns its space by explaining the product state transition and anchoring the asymmetrical layout. It remains plain HTML and CSS, not decorative data visualization.

### 6. Use one purposeful CSS reel and no runtime JavaScript

The workload term is the page’s signature motion: a slow vertical reel cycles through four nouns using transform only, with long readable holds and a stable container width. A visually hidden phrase exposes the complete set to assistive technology without live announcements. `prefers-reduced-motion: reduce` stops the reel on `agent`; the content and promise remain complete. The only other motion is a subtle page-entry transition affecting opacity and transform. Interactive feedback is limited to focus, underline, and small transform/color changes.

The simpler alternative was a JavaScript word rotator. CSS is sufficient for a fixed sequence and avoids a runtime dependency, timing script, accessibility announcements, and layout shift.

### 7. Adapt the editorial grid rather than merely shrinking it

Wide screens will use an asymmetrical content/annotation grid and generous whitespace. Medium screens collapse status annotations beside their content. Small screens become one linear reading column; the workload set wraps deliberately, lifecycle rules become vertical, and calls to action remain at least 44 CSS pixels tall. No critical content is hidden at any viewport.

Semantic landmarks, heading order, native `details`, keyboard-visible focus, descriptive link text, and decorative `aria-hidden` treatment will be preserved. The page will not disable zoom.

## Risks / Trade-offs

- **“Always ready” may be read as high availability or node-loss durability.** → Define readiness immediately as retained local session state, keep the remote snapshot tier and node-loss behavior in the roadmap/limits content, and avoid uptime language.
- **A fashion-associated display serif could make the page feel ornamental.** → Use Bodoni Moda only for large headings at regular/medium weights; Schibsted Grotesk carries all explanatory and technical content.
- **Self-hosted fonts add bytes and license files.** → Use WOFF2 variable/subset files, load only two families, preload sparingly, and keep fallbacks fully functional.
- **The subject set may wrap awkwardly on narrow screens.** → Treat it as a dedicated responsive typographic component, allow intentional multi-line wrapping, and test at 320–375 CSS pixels and 200% zoom.
- **A quieter page could hide calls to action.** → Reserve the sole accent and strongest contrast for the primary action and visible focus states.
- **Landing-page measurements can become stale or lose caveats.** → Cross-check each number and capability label against BRD NFR-1 and current ratified specs during implementation.

## Migration Plan

1. Add licensed local font assets and declarations.
2. Replace the HTML/CSS in `docs/index.html` while preserving its public path and documentation links.
3. Validate semantics, links, responsive layouts, reduced motion, and copy claims locally.
4. Run `make check` without bypass.
5. Roll back by restoring the previous `docs/index.html` and removing the added font assets; no data or API migration is involved.
