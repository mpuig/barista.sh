## Context

See `proposal.md` for the publishing conflict. The source landing is one static
HTML document with inline CSS and two local variable fonts. Its destination is
an existing SvelteKit application whose root currently redirects to `/app` or
`/login`; its root layout also wraps every route in the authenticated product
shell.

The public repository already has a complete Markdown information architecture
under `docs/`, with `docs/index.md` as its navigation page. GitHub Pages has no
workflow here today. The established visual context is the repository's
`.impeccable.md`: serene, precise, warm editorial design with current/planned
claims kept visibly separate.

## Goals / Non-Goals

**Goals:**

- Preserve the landing's content hierarchy, visual identity, accessibility,
  reduced-motion behavior, and responsive layouts in SvelteKit.
- Keep `/login`, `/app`, and `/admin` behavior intact after `/` becomes public.
- Publish every intended Markdown page as navigable HTML at the public
  repository's GitHub Pages root.
- Make local and CI documentation builds deterministic and strict.

**Non-Goals:**

- Redesigning or rewriting the truth-audited landing content.
- Coupling the Barista runtime or packages to the external web site.
- Hosting the authenticated product UI on GitHub Pages.
- Publishing temporary upstream issue drafts in the primary MkDocs navigation;
  they remain buildable and linkable evidence.

## Decisions

### 1. Port the landing into the existing SvelteKit root

The static document's body becomes the root `+page.svelte`; metadata and font
preloads move into `<svelte:head>`, and the fonts are copied into the web app's
static assets. The root redirect is removed. The root layout renders the landing
without the authenticated shell at `/`, while retaining the existing shell on
`/login`, `/app`, and `/admin`. The landing exposes an explicit login/app link so
moving the root does not make the hosted product undiscoverable.

The page retains its current semantic sections and CSS rather than being split
into speculative components. That is the simpler alternative for a static,
one-page composition and minimizes visual drift during the move.

A raw `static/index.html` was rejected: it would bypass SvelteKit routing and
metadata, compete with the existing root route, and make authenticated navigation
behavior depend on adapter file precedence.

### 2. Publish Markdown at the Pages root with MkDocs Material

Once `docs/index.html` leaves this repository, `docs/index.md` naturally becomes
MkDocs' root page. `mkdocs.yml` declares the complete user-documentation
navigation and uses Material for search, mobile navigation, accessible code
blocks, and stable Markdown link rewriting. A small extra stylesheet applies the
same warm ink/paper palette and local Barista fonts without trying to reproduce
the product landing inside the reference site.

The generated site lives in ignored `site/`; source Markdown remains canonical.
The canonical Pages URL is `https://mpuig.github.io/barista.sh/`, which the
external landing uses for documentation links.

Native Jekyll was the simpler dependency-free alternative, but it needs front
matter/plugin conventions on every page and offers weaker navigation/search.
MkDocs already understands this documentation tree and validates its relative
links during a strict build.

### 3. Pin one build command and reuse it locally and in Pages

A `task docs` command invokes pinned MkDocs and Material versions through `uvx`.
The official GitHub Pages actions run that same command, upload `site/`, and
deploy only from `main`. The repository quality gate invokes the docs build so a
broken link or configuration cannot merge while Pages remains green by accident.

The workflow uses Pages' scoped `pages: write` and `id-token: write` permissions;
it receives no repository or deployment secrets.

### 4. Move only after both outputs are proven

The landing is first ported and checked in the destination. MkDocs is then built
with the original still present but excluded explicitly if necessary. Only after
both builds and visual checks pass is `docs/index.html` deleted. Font files remain
in the public docs assets for MkDocs styling and are copied to the destination,
so neither output reaches across repositories at runtime.

## Risks / Trade-offs

- **The Svelte port could drift visually.** → Compare desktop and mobile
  screenshots against the source page, test keyboard focus, and preserve the
  existing reduced-motion branch before deletion.
- **Changing `/` removes the automatic login redirect.** → Keep direct `/login`
  and `/app` routes and add a visible destination-aware CTA on the landing.
- **Strict MkDocs builds may expose old links outside `docs_dir`.** → Convert the
  OpenSpec link to its canonical GitHub URL and fix every warning rather than
  suppressing it.
- **Two repositories must deploy in order.** → Publish the docs first; the moved
  landing's docs links then target an existing site. Rollback is independent in
  each repository.
- **`uvx` may need a first network fetch locally.** → Pin versions for stable
  resolution; CI caches uv and always starts from a declared environment.

## Migration Plan

1. Port the landing and fonts to the SvelteKit site, preserve product-route
   access, and pass its build plus browser checks.
2. Add MkDocs configuration, theme CSS, build task, and Pages workflow; build
   the complete public site locally.
3. Replace out-of-tree Markdown links and point the destination landing at the
   canonical Pages URLs.
4. Delete the original `docs/index.html`, rebuild both repositories, run Barista's
   full quality gate, then deploy documentation before the external landing.
5. Roll back either repository independently if its deployment fails; Git
   history retains the original static page.
