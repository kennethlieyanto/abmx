# Contributing

The parser corpus is the heart of the project. Adding a bank is a data change,
not a code change.

## Adding a parser

```bash
mkdir -p sample/<bank>/<type>
cp ~/saved-emails/*.eml sample/<bank>/<type>/     # 2-3 emails minimum

abmx parser infer sample/<bank>/<type> --id <bank>.<type> > /tmp/draft.toml
abmx parser init <bank>.<type>
$EDITOR parsers/<bank>.<type>.toml                # paste the draft, fix the TODOs
abmx dump sample/<bank>/<type>/one.eml            # see the blocks
cargo run -p xtask -- redact --in sample/<bank>/<type> --out /tmp/redacted
cp /tmp/redacted/*.eml fixtures/<bank>.<type>/
abmx parser check --parser <bank>.<type> -v
```

`abmx parser check` must pass before you open a pull request.

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
- One file per parser: `parsers/<bank>.<type>.toml`, fixture directory
  `fixtures/<bank>.<type>/`. The file stem must equal the spec `id`.
- Add a fixture for every layout variation you know about. A bank that sends two
  layouts is two `requires`/`rejects` fingerprints, usually in the same parser if
  the fields are identical, or two parser directories if they are not.
- Prefer `requires`/`rejects` label fingerprints over sender rules when several
  templates share a sender and subject.

## Before pushing

```bash
just ci
```

That runs formatting, clippy with warnings denied, the whole test suite, and
every parser against its fixtures.

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

- **A new bank** is a TOML spec under `parsers/` plus fixtures. No Rust.
- **A new mailbox or ledger** is a crate implementing `Fetcher` or `Sink`, plus
  one builder in `abmx/src/config.rs`. Keep vendor quirks inside the crate.

Keep `abmx-core` vendor-neutral. The parser contract is documented in
[parsing.md](docs/parsing.md); the system design is in
[architecture.md](docs/architecture.md).
