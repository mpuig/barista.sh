## 1. Assets and source-of-truth audit

- [x] 1.1 Add minimal self-hosted Bodoni Moda and Schibsted Grotesk WOFF2 assets under `docs/assets/fonts/`, include their OFL license material, and verify `font-display: swap` fallbacks work when font requests are blocked.
- [x] 1.2 Cross-check every landing-page metric, current capability, limitation, and roadmap statement against BRD NFR-1, ratified OpenSpec specs, and the current documentation before rewriting copy.
- [x] 1.3 Mirror the ratified `## Design Context` from `.impeccable.md` into `.github/copilot-instructions.md` without duplicating it.

## 2. Message and structure

- [x] 2.1 Rewrite the semantic structure of `docs/index.html` around the single headline sentence `Your [agent / runtime / environment / session], always ready. Only awake when it matters.`, rotating only the workload noun and defining readiness as retained memory, disk, and working context.
- [x] 2.2 Reorganize the supporting sections into measured proof, lifecycle, bounded use cases, available-now capabilities, wake triggers, honest limits, and the closing GitHub action while preserving valid documentation links.
- [x] 2.3 Mark command and scheduled wake as available today, request-driven wake as soon but still roadmap, and the bucket coordination protocol/CLI as shipped but node fleet membership as not yet available; avoid copy that implies a scheduler service, stateless PaaS, node-loss warm durability, or working multi-node operation.

## 3. Editorial visual system

- [x] 3.1 Replace the multicolor palette and decorative cup treatment with warm near-monochrome tokens, one restrained accent, hairline rules, generous whitespace, and accessible contrast.
- [x] 3.2 Implement the Bodoni Moda/Schibsted Grotesk type hierarchy with fluid display sizing, readable body measure and leading, restrained weights, and monospace limited to command/status output.
- [x] 3.3 Build the asymmetrical hero and session-status ledger in semantic HTML/CSS, integrating a CSS-only workload reel at the headline’s display size, with a stable width, a complete accessible heading label, and an `agent` reduced-motion fallback.
- [x] 3.4 Adapt the editorial grid, lifecycle sequence, use-case list, measured-proof note, capability lists, and actions for narrow, medium, and wide layouts without hiding critical content or creating horizontal overflow.
- [x] 3.5 Preserve skip navigation, logical headings, keyboard-visible focus, 44 CSS pixel touch targets, descriptive links, native disclosure behavior, 200% zoom usability, and a no-motion rendering under `prefers-reduced-motion`.

## 4. Verification

- [x] 4.1 Serve the static docs locally and inspect the page with `agent-browser` at 320/375, 768, and 1440 CSS pixel widths, capturing screenshots and checking layout stability, wrapping, link targets, focus order, and horizontal overflow.
- [x] 4.2 Verify font failure fallback, reduced motion, semantic accessibility-tree output, contrast, and that the page remains understandable with styles or animation unavailable.
- [x] 4.3 Re-audit all final claims against their cited sources and confirm that this documentation-only change claims no Phase 1 acceptance tests T1–T12.
- [x] 4.4 Run `make check` without bypass and leave every failure visible.
