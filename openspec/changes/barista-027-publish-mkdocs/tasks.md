## 1. Relocate the product landing

- [x] 1.1 Port `docs/index.html` into the companion SvelteKit root route, copy
      its variable fonts into static assets, move metadata/preloads into
      `<svelte:head>`, and remove the root redirect without changing `/login`,
      `/app`, or `/admin` behavior.
- [x] 1.2 Isolate the public landing from the authenticated route shell and add a
      visible login/app destination while preserving the truth-audited content,
      semantic structure, internal anchor navigation, responsive behavior, and
      reduced-motion fallback.
- [x] 1.3 Replace source-relative documentation links with canonical GitHub
      Pages URLs, run the destination web build, and compare source versus port
      with browser checks at desktop and mobile widths plus keyboard-focus and
      reduced-motion checks.

## 2. Build the MkDocs site

- [x] 2.1 Add `mkdocs.yml` with comprehensive user-doc navigation, Material
      search/navigation features, local font loading, and a restrained warm
      Barista theme stylesheet; keep temporary upstream drafts out of primary
      navigation without deleting them.
- [x] 2.2 Correct links that cannot resolve within `docs_dir`, including the
      OpenSpec requirements link, and fix every warning surfaced by a strict
      MkDocs build rather than excluding current documentation.
- [x] 2.3 Add an ignored `site/` output and a pinned `task docs` command using
      `uvx`; include it in the repository quality gate so local and CI builds use
      the same strict command.
- [x] 2.4 Add an official GitHub Pages Actions workflow that builds on `main`,
      uploads `site/`, and deploys with only `contents: read`, `pages: write`, and
      `id-token: write` permissions.

## 3. Complete and verify the move

- [x] 3.1 Delete `docs/index.html` only after the destination landing and MkDocs
      builds pass; confirm no source link still targets the removed HTML or a
      `.md` URL from the moved landing.
- [x] 3.2 Run the strict docs build and inspect the generated artifact: root
      `index.html` comes from `docs/index.md`, navigation and search assets exist,
      local fonts load, and every generated/internal link resolves.
- [x] 3.3 Run the companion web build and browser smoke test once more from the
      final source trees, then run `git diff --check` in both repositories.
- [x] 3.4 Run `make check` without bypass in Barista and record that this
      publishing-only change claims no Phase 1 acceptance test T1–T12.
