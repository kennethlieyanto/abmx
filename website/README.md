# abmx website

The documentation site for [abmx](https://github.com/kennethlieyanto/abmx), built
with Astro and Starlight and deployed to GitHub Pages at
<https://kennethlieyanto.github.io/abmx/>.

This is the canonical home for user documentation. The repository README is
deliberately short; contributor instructions are in `CONTRIBUTING.md`.

## Local development

```bash
npm ci
npm run dev        # http://localhost:4321/abmx/
npm run check      # astro check (types + content)
npm run build      # static output in dist/
npm run preview    # serve dist/ at /abmx/
```

`just site-dev`, `just site`, and `just site-check` from the repository root do
the same without changing directory.

## Writing docs

Content lives in `src/content/docs/`; the sidebar is defined in
`astro.config.mjs`. The landing page is `src/content/docs/index.mdx` and uses
Starlight's splash template.

Internal links in Markdown may be root-relative:

```md
See [Gmail](/guides/gmail/) for the Google Cloud setup.
```

A Sätteri HAST plugin (`src/plugins/base-links.mjs`) prefixes Astro's `base`
(`/abmx`) at build time, so those links work on the project site.

Links in **MDX JSX attributes** and in **frontmatter** (hero actions) do not pass
through that plugin. Write those with the base explicitly:

```mdx
<LinkCard title="Install" href="/abmx/start/install/" />
```

`starlight-links-validator` fails the build on broken internal links.

The [release notes](https://kennethlieyanto.github.io/abmx/release-notes/) page is generated: `scripts/sync-release-notes.mjs`
copies the repository's release-please `CHANGELOG.md` into
`src/content/docs/release-notes.md`. It runs automatically before `dev`, `build`,
and `check`, so edit `CHANGELOG.md`, not the generated page.

## Install script

`public/install.sh` is served at
<https://kennethlieyanto.github.io/abmx/install.sh> and is the one-liner in the
install guide. It downloads `abmx-<target>.tar.gz` and the adjacent `.sha256`
from GitHub Releases, so keep its target names in sync with
`.github/workflows/release.yml`.

## Deployment

`.github/workflows/website.yml` builds the site and deploys it with GitHub Pages
on pushes to `main`. The repository must have Settings → Pages → Source set to
“GitHub Actions” once.

The deployed path is controlled by `base` in `astro.config.mjs`. If the
repository is renamed or a custom domain is added, update `base` and `site`
together, then check the explicit `/abmx/` links in `index.mdx`.

## Older CPUs

Astro 7's native compiler binding requires AVX2, and Pagefind's WASM build also
fails on some pre-Haswell x86_64 hosts. Local workaround:

```bash
npm run compiler:wasi              # install the WASI compiler binding
ABMX_NO_PAGEFIND=1 npm run build   # skip the search index
```

CI runs on a current GitHub runner and uses the native bindings with Pagefind
enabled, so this only affects local builds on older machines.
