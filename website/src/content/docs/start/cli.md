---
title: CLI reference
description: Every abmx command, the receipt and statement groups, stdout rules, and exit codes.
---

```text
abmx setup [--manual] [--no-browser] [--review] [--remap]
                                    interactive config, Gmail, receipts, statements
abmx auth login [--manual]          OAuth2 + PKCE; pastes the callback when headless
abmx auth export [--out FILE] [--stdout] [--force]
                                    write a portable credential bundle
abmx auth import --file FILE        install an exported bundle locally
abmx auth logout [--no-revoke]      revoke the refresh token, then remove it locally
abmx status [--offline] [--strict] [--check] [--json]
                                    full health check; --offline skips network
abmx reset [--purge] [--yes]        forget delivered receipts and statements
abmx accounts [--json]              sink accounts and how [receipts.accounts] maps onto them
abmx skill                          print the parser-authoring skill to stdout
abmx dump <path> [--json]           normalized block dump (authoring aid)

abmx sync [--dry-run] [--json] [--limit N]
                                    sync every enabled source: receipts, then statements
abmx samples [--samples N] [--json]
                                    download evidence for both enabled sources
abmx receipt sync [--dry-run] [--json] [--diff] [--limit N]
                                    receipts only: fetch, parse, and write
abmx receipt sync --from-dir DIR [--label L]
                                    offline receipt sync from saved .eml files
abmx receipt samples [--label L|PARSER] [--samples N] [--out DIR] [--json]
                                    sample emails only

abmx statement dump <file.pdf> [--password-file F] [--json]
                                    positioned text dump (statement authoring aid)
abmx statement check [--parser ID] [--password-file F] [-v]
                                    verify statement parsers and their arithmetic
abmx statement samples [--label LABEL|PARSER] [--samples N] [--out DIR] [--json]
                                    download PDF statements for parser authoring
abmx statement import <file.pdf> [--parser ID] [--password-file F] [--dry-run] [--json]
                                    import one statement PDF by hand
abmx statement sync [--dry-run] [--json] [--limit N]
                                    parse, verify, and add statement rows

abmx parser paths [--json]          parser search order and which directory won
abmx parser specs [--format F]      loaded parsers and their files
abmx parser schema                  JSON Schema of the transaction contract
abmx parser parse <path> --parser ID [--format json|ndjson|table]
                                    parse a file or directory with one parser
abmx parser explain <path> --parser ID
                                    block regions behind each field
abmx parser lint [--parser ID]      fixtures and label coverage checks
abmx parser check [-v] [--parser ID]
                                    verify every parser against its fixtures
abmx parser init <id> [--kind mail|statement]
                                    scaffold ~/.config/abmx/parsers/<id>/{parser.rhai,fixtures}
```

`abmx sync` is the union: it runs the receipt source first and the statement
source second, skipping any source with `enabled = false`. The order matters —
the statement path reads the ledger to deduplicate, so it should see the receipt
rows from the same run. Source-specific flags live on the source commands:
`--from-dir`, `--label`, and `--diff` only make sense for receipts, and
`--password-file`/`--parser` only for statements.

Every `abmx parser` and `abmx statement` command takes `--parsers <dir>`
(repeatable) or `$ABMX_PARSERS`; implicit discovery is
`~/.config/abmx/parsers`, then `/etc/abmx/parsers`. The working directory is
never read. Fixtures are always read from the winning parser's own `fixtures/`
directory. Statement parsers use the same corpus; `input = "pdf"` selects the
table engine. See [Parser spec](/reference/parser-spec/),
[Statement spec](/reference/statement-spec/), and
[Transaction contract](/reference/transaction/) for the formats.

## stdout is data, stderr is diagnostics

`abmx sync --dry-run --json` writes an object with one plan per enabled source:

```json
{ "receipts": { ... }, "statements": { ... } }
```

`abmx receipt sync --dry-run --json` and `abmx statement sync --dry-run --json`
write a single plan. Human summaries always go to stderr, so the JSON pipes
cleanly:

```bash
abmx sync --dry-run --json > plans.json
abmx receipt sync --dry-run --json > receipts.json
```

`-q` prints only the summary lines; `-v` adds the parser id and source message
key to each row.

`--diff` (with `abmx receipt sync --dry-run`) additionally reads the ledger and
reports rows that are already present, including any imported under a foreign
`imported_id`.

`abmx samples` downloads evidence for every enabled source: receipt emails and
statement PDFs. It samples every valid label under the prefixes that is not in
the matching `ignore` list, mapped or not. The source-specific `--label` and
`--out` flags live on `abmx receipt samples` and `abmx statement samples`. Like
sync, sampling ignores the `since` windows and writes `0600` artifacts under the
XDG config directory (`samples/` and `statement-samples/`).

`abmx statement import` is the manual path for a bank that only offers a portal
or app download: the PDF stays local, there is no Gmail call, and the ledger's
`imported_id` makes a repeated import a no-op. `--parser` is inferred from a
`<dir>/<parser-id>/` layout when omitted.

`abmx receipt sync --from-dir` reads saved `.eml` files instead of calling
Gmail; with a flat directory it needs `--label`, unless exactly one parser is
configured. This is the offline path for testing a parser against real mail:

```bash
abmx receipt sync --dry-run --from-dir ./saved-emails --label "receipt/<id>"
```

## Exit codes

| Code | Meaning                                                                |
| ---- | ---------------------------------------------------------------------- |
| `0`  | clean                                                                  |
| `1`  | the tool failed (config, auth, network, state)                         |
| `2`  | one or more routed labels or statement documents need a fix            |

`abmx sync` runs both sources even if one fails and returns the worse code. A
dry run exits `2` when a receipt label or a statement document is broken, so a
timer or healthcheck can alert without writing anything. `abmx receipt sync`
writes no receipt rows when any label is broken; `abmx statement sync` writes no
rows from a document that fails to parse or balance.
