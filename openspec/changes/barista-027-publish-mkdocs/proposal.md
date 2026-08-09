## Why

Barista's hand-built product landing page and its Markdown documentation
currently compete for `docs/index.*`, so GitHub Pages cannot give each a clean
home. Relocating the landing to the separately deployed web site lets the public
repository publish `docs/index.md` as a conventional, searchable documentation
site.

## What Changes

- Port the existing landing page, visual language, responsive behavior, and font
assets into the external site's SvelteKit root route while preserving access to
its `/login` and `/app` routes.
- Replace landing-local Markdown links with stable links to the published public
Barista documentation.
- Remove `docs/index.html` from this repository after the destination build and
visual checks pass; keep `docs/index.md` as the documentation home.
- Add a pinned MkDocs Material configuration, warm Barista-specific theme
adjustments, comprehensive navigation, and a strict local build command.
- Add a GitHub Pages Actions workflow that publishes the generated MkDocs site
from `main`.
- Correct source links that cannot resolve inside MkDocs' `docs_dir`, and add
local link/build verification.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. This is documentation publishing and an out-of-tree presentation move; it
changes no runtime, API, CLI, or capability requirement. The change therefore
sets `skip_specs: true`.

## Impact

In this repository the change affects `docs/`, MkDocs configuration, developer
documentation tooling, and `.github/workflows/`. The destination site's SvelteKit
root and static font assets change in the companion web repository; no runtime
or package dependency points from Barista back to that site.

Definition of done: the destination web build passes and the landing is usable
at desktop and mobile widths; `mkdocs build --strict`, repository-local link
checks, `git diff --check`, and `make check` pass; the Pages artifact has
`index.html` generated from `docs/index.md`. This change claims no Phase 1
acceptance test T1–T12.

## Constitution Check

- **Schema-first:** no protobuf or generated contract change.
- **Honest capabilities:** the moved landing retains the current/planned labels
  established by `barista-024-documentation-truth`.
- **Crash-safe:** no mutation or journal behavior changes.
- **Simple by default:** MkDocs and the official Pages actions replace custom
  Markdown conversion; the landing uses the destination's existing SvelteKit
  stack rather than introducing another web runtime.
