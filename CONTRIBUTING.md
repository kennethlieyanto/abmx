# Contributing

The parser corpus is the project. Adding a bank is a data change, not a code
change.

## Adding a parser

```bash
mkdir -p sample/<bank>/<type>
cp ~/saved-emails/*.eml sample/<bank>/<type>/     # 2-3 emails minimum

txp infer sample/<bank>/<type> --id <bank>.<type> > /tmp/draft.toml
txp init <bank>.<type>
$EDITOR parsers/<bank>.<type>.toml                # paste the draft, fix the TODOs
txp dump sample/<bank>/<type>/one.eml               # see the blocks
cargo xtask redact --in sample/<bank>/<type> --out /tmp/redacted
cp /tmp/redacted/*.eml fixtures/<bank>.<type>/
txp check --parser <bank>.<type> -v
```

`txp check` must pass before you open a pull request.

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

## Scope

This project parses `.eml` bytes. It does not fetch mail, schedule jobs, map
accounts or write to a ledger. Please keep pull requests inside that boundary;
the contract in [SPEC.md](SPEC.md) is what makes it reusable by other programs.
