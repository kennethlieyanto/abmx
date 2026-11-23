# Contributing

This repository is the engine and tooling. User documentation lives on the
[website](https://kennethlieyanto.github.io/abmx/); the site's source is in
`website/src/content/docs/`. Please send user-facing docs changes there, not to
the README.

Parsers are runtime data, not code. `abmx` deliberately ships **no bank
parsers**: your corpus lives in `~/.config/abmx/parsers/` (or wherever
`--parsers`/`$ABMX_PARSERS` points). This repository contains one synthetic
example under `examples/parsers/` for documentation and tests.

Do not open a pull request adding a real bank parser or real email fixtures.
Those belong in your private runtime corpus.

## Development

```bash
nix develop        # rust, cargo, clippy, just, node, sqlite
just ci            # fmt, clippy, tests, synthetic parser fixtures
just helper-test   # the Node helper (run `just helper-install` once)
```

`just test` runs the workspace tests. `just check` is formatting plus clippy
with warnings denied. CI also runs the helper's Node tests and the real helper
process.

## Fixture rules

- **Never commit raw emails.** `sample/` and `redact.toml` are gitignored because
  they contain real names, card numbers, and account numbers.
- Run `just redact` (or `cargo run -p xtask -- redact`) and keep the output in
  your private corpus.
- Add any new real value you see to `redact.toml`, then re-run redaction. The
  tool fails the run if a sensitive value survives.
- The only committed fixtures are synthetic: `@example.invalid` addresses,
  example merchants, and arbitrary amounts.
- Two emails may legitimately be identical. Keep both fixtures in your corpus:
  they prove that identity is stable.

## Parser rules

- `meta().api` only changes when the host API breaks existing scripts. Do not
  bump it for a new parser.
- Bump `meta().version` when the script's extraction behaviour changes.
- One directory per parser: `<corpus>/<id>/parser.rhai`, with samples in
  `<corpus>/<id>/fixtures/`. The directory name **is** the parser id.
- A script returns observations only. Identity, `strict`, and the statement
  arithmetic stay in the host; never move them into a parser.
- Add a fixture for every layout variation you know about. A bank that sends two
  layouts gets two fixtures; if the extracted fields differ, use two parser
  directories.
- The full format is in the [parser spec](https://kennethlieyanto.github.io/abmx/reference/parser-spec/)
  and the authoring workflow is in [authoring parsers](https://kennethlieyanto.github.io/abmx/guides/authoring/).

## Before pushing

```bash
just ci          # fmt, clippy, tests, synthetic parser fixtures
just helper-test # the Node helper (run `just helper-install` once)
```

If you changed the site or docs, also run:

```bash
just site-check
```

On x86_64 hosts without AVX2 (pre-Haswell CPUs), see `website/README.md` for
the WASI compiler and Pagefind workaround.

## The Actual helper

`crates/abmx-actual/helper/` is a small CommonJS program around the official
`@actual-app/api` package. It is not a service: `abmx` spawns it per operation
and reads one JSON response from stdout.

Rules:

- **Pin the exact `@actual-app/api` version** in `package.json`; the lockfile is
  committed. Actual ships the server and this client together.
- **Keep it a pass-through.** If a behaviour needs code, prefer the official API
  option over inventing one in the helper.
- **stdout is protocol.** The helper redirects the library's own logging to
  stderr; `ABMX_ACTUAL_DEBUG=1` shows it. Never add a `console.log`.
- **No server, no port, no inbound secret.** Credentials go to the child through
  its environment.

To move to a new Actual release:

```bash
just helper-bump 26.12.0
# then update npmDepsHash in flake.nix (the command prints the reminder)
nix build .#abmx-actual-helper
just helper-test
cargo test --workspace
```

Mention the bundled client version in the release notes, e.g. "bundles
`@actual-app/api` 26.12.0". `abmx status` compares it with the server: a minor
skew warns, a year jump refuses to write.

## Documentation site

The site is Astro + Starlight in `website/` and is deployed to GitHub Pages at
`/abmx/`. Content lives in `website/src/content/docs/`.

```bash
cd website
npm ci
npm run dev        # http://localhost:4321/abmx/
npm run check      # astro check (types + content)
npm run build
```

Internal links in Markdown may be root-relative (`/guides/gmail/`); a Sätteri
plugin prefixes Astro's `base` at build time. Links in MDX JSX attributes and in
frontmatter (hero actions) are not rewritten, so write those with `/abmx/`
explicitly.

On x86_64 hosts without AVX2 (pre-Haswell CPUs), Astro's native compiler binding
and Pagefind can fail. Use:

```bash
npm run compiler:wasi              # install the WASI compiler binding
ABMX_NO_PAGEFIND=1 npm run build   # skip the search index locally
```

CI builds on a modern GitHub runner with the native bindings and Pagefind
enabled.

## Releasing

Releases are automated with
[release-please](https://github.com/googleapis/release-please). `main` must use
Conventional Commit messages (`feat:`, `fix:`, `feat!:`); release-please turns
them into one pending release PR.

1. Merge the pending `chore: release` PR when ready to ship.
2. release-please bumps the workspace version, `version.txt`, `Cargo.lock`, and
   `CHANGELOG.md`, then creates the `vX.Y.Z` tag and GitHub Release.
3. The `release` workflow uploads the Linux/macOS tarballs; the `image`
   workflow publishes the container.

The repository needs a `RELEASE_PLEASE_TOKEN` secret: a fine-grained PAT scoped
to this repository with **Contents**, **Pull requests**, and **Issues**
read/write. The default `GITHUB_TOKEN` cannot trigger the tag and release
workflows. Rotate the PAT before it expires. Also set
Settings → Actions → General → Workflow permissions to "Read and write".

The root `Cargo.toml` is a virtual workspace, and release-please's Rust strategy
cannot read `[workspace.package].version` (it requires a literal
`[package].version`). `release-please-config.json` therefore uses the `simple`
strategy with `version.txt`, plus a TOML `extra-files` updater for
`$.workspace.package.version`. `version.txt` is generated, not hand-maintained;
CI fails if it disagrees with `Cargo.toml`. The release-please workflow also
commits the `Cargo.lock` sync onto the release PR so `--locked` builds work.

The first release is bootstrapped with `initial-version: 0.1.0` and a
`bootstrap-sha`; release-please ignores `bootstrap-sha` once a release exists. Do
not rename release assets: `website/public/install.sh` depends on
`abmx-<target>.tar.gz` and the adjacent `.sha256`.

The helper's `package.json` version is not managed by release-please; it tracks
the pinned `@actual-app/api`. Parser corpora are not part of a release: they are
runtime data managed by the user (or by a deployment's ConfigMap).

## Scope

Two extension surfaces, two rules:

- **A new bank** is a parser directory in your runtime corpus (`config.toml`
  plus `fixtures/`). No Rust, and no change to this repository.
- **A new mailbox or ledger** is a crate implementing `Fetcher` or `Sink`, plus
  one builder in `abmx/src/config.rs`. Keep vendor quirks inside the crate.

Keep `abmx-core` vendor-neutral. The parser contract is documented in the
[parser spec](https://kennethlieyanto.github.io/abmx/reference/parser-spec/); the
system design is in [architecture](https://kennethlieyanto.github.io/abmx/reference/architecture/).
