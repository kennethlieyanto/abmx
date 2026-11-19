# Contributing

Parsers are runtime data, not code. `abmx` deliberately ships **no bank
parsers**: your corpus lives in `~/.config/abmx/parsers/` (or wherever
`--parsers`/`$ABMX_PARSERS` points). This repository contains one synthetic
example under `examples/parsers/` for documentation and tests.

Do not open a pull request adding a real bank parser or real email fixtures.
Those belong in your private runtime corpus.

## Authoring a parser locally

```bash
mkdir -p ~/.config/abmx/samples/<id>
cp ~/saved-emails/*.eml ~/.config/abmx/samples/<id>/   # 2-3 emails minimum

abmx parser init <id>                                  # writes ~/.config/abmx/parsers/<id>
$EDITOR ~/.config/abmx/parsers/<id>/config.toml
abmx dump ~/.config/abmx/samples/<id>/one.eml          # see the blocks
abmx parser check --parser <id> --parsers ~/.config/abmx/parsers
```

For a fully synthetic contribution, edit `examples/parsers/example.bank/` and
run:

```bash
abmx parser check --parsers examples/parsers
```

`just ci` runs that check as part of the test suite.

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

## Spec rules

- `spec_version` only changes with the TOML format. Do not bump it for a new
  parser.
- Bump `version` inside the spec when the extraction behaviour changes.
- One directory per parser: `<corpus>/<id>/config.toml`, with samples in
  `<corpus>/<id>/fixtures/`. The directory name **is** the parser id; there is
  no `id` field in the spec.
- Add a fixture for every layout variation you know about. A bank that sends two
  layouts is two `requires`/`rejects` fingerprints, usually in the same parser if
  the fields are identical, or two parser directories if they are not.
- Prefer `requires`/`rejects` label fingerprints over sender rules when several
  templates share a sender and subject.

## Before pushing

```bash
just ci          # fmt, clippy, tests, synthetic parser fixtures
just helper-test # the Node helper (run `just helper-install` once)
```

That runs formatting, clippy with warnings denied, the whole test suite, and the
synthetic example parser against its fixture. CI also runs the helper's Node
tests and the real helper process.

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

## Releasing

There is no release automation. The workspace shares one version
(`[workspace.package].version`), and a release is a tag:

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

The Nix flake reads the version from `Cargo.toml`, so bump it before tagging.
Parser corpora are not part of the release: they are runtime data managed by the
user (or by a deployment's ConfigMap).

## Scope

Two extension surfaces, two rules:

- **A new bank** is a parser directory in your runtime corpus (`config.toml`
  plus `fixtures/`). No Rust, and no change to this repository.
- **A new mailbox or ledger** is a crate implementing `Fetcher` or `Sink`, plus
  one builder in `abmx/src/config.rs`. Keep vendor quirks inside the crate.

Keep `abmx-core` vendor-neutral. The parser contract is documented in
[parsing.md](docs/parsing.md); the system design is in
[architecture.md](docs/architecture.md).
