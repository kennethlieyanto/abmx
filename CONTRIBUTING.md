# Contributing

The parser corpus is the heart of the project. Adding a bank is a data change,
not a code change.

## Adding a parser

```bash
mkdir -p sample/<bank>/<type>
cp ~/saved-emails/*.eml sample/<bank>/<type>/     # 2-3 emails minimum

abmx parser infer sample/<bank>/<type> --id <bank>.<type> > /tmp/draft.toml
abmx parser init <bank>.<type> --parsers parsers
$EDITOR parsers/<bank>.<type>/config.toml         # paste the draft, fix the TODOs
abmx dump sample/<bank>/<type>/one.eml            # see the blocks
cargo run -p xtask -- redact --in sample/<bank>/<type> --out /tmp/redacted
cp /tmp/redacted/*.eml parsers/<bank>.<type>/fixtures/
just check-parser <bank>.<type>
```

`just check-parser` must pass before you open a pull request.

To make the parser active outside the checkout, `just install-parser <bank>.<type>`
checks it and copies the whole directory into `~/.config/abmx/parsers/`.

## Fixture rules

- **Never commit raw emails.** `sample/` and `redact.toml` are gitignored because
  they contain real names, card numbers and account numbers.
- Run `just redact` (or `cargo run -p xtask -- redact`) and commit only the output.
- Add any new real value you see to `redact.toml`, then re-run redaction. The
  tool fails the run if a sensitive value survives.
- Two emails may legitimately be identical. Keep both fixtures: they prove that
  identity is stable.

## Spec rules

- `spec_version` only changes with the TOML format. Do not bump it for a new
  parser.
- Bump `version` inside the spec when the extraction behaviour changes.
- One directory per parser: `parsers/<bank>.<type>/config.toml`, with samples in
  `parsers/<bank>.<type>/fixtures/`. The directory name **is** the parser id;
  there is no `id` field in the spec.
- Add a fixture for every layout variation you know about. A bank that sends two
  layouts is two `requires`/`rejects` fingerprints, usually in the same parser if
  the fields are identical, or two parser directories if they are not.
- Prefer `requires`/`rejects` label fingerprints over sender rules when several
  templates share a sender and subject.

## Before pushing

```bash
just ci          # fmt, clippy, tests, parser fixtures
just helper-test # the Node helper (run `just helper-install` once)
```

That runs formatting, clippy with warnings denied, the whole test suite, and
every parser against its fixtures. CI also runs the helper's Node tests and the
real helper process.

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
Parser-only changes under `parsers/` are data, not code: they ship with the next
tag like everything else, because the corpus is bundled into the binary's
`share/` directory.

## Scope

Two extension surfaces, two rules:

- **A new bank** is a parser directory under `parsers/` (`config.toml` plus
  `fixtures/`). No Rust.
- **A new mailbox or ledger** is a crate implementing `Fetcher` or `Sink`, plus
  one builder in `abmx/src/config.rs`. Keep vendor quirks inside the crate.

Keep `abmx-core` vendor-neutral. The parser contract is documented in
[parsing.md](docs/parsing.md); the system design is in
[architecture.md](docs/architecture.md).
