---
title: CLI reference
description: Every abmx command, the parser command group, stdout rules, and exit codes.
---

```text
abmx init [--path FILE] [--force]   write a starter config
abmx setup [--manual] [--review] [--remap]
                                    interactive config, Gmail, labels, accounts
abmx auth gmail [--manual]          OAuth2 + PKCE; --manual for a headless box
abmx auth export [--out FILE] [--stdout] [--force]
                                    write a portable credential bundle
abmx auth import --file FILE        install an exported bundle locally
abmx auth logout [--no-revoke]      revoke the refresh token, then remove it locally
abmx status [--offline] [--strict] [--check] [--json]
                                    full health check; --offline skips network
abmx samples [--label L|PARSER] [--samples N] [--out DIR] [--json]
                                    download sample emails for parser authoring
abmx sync [--dry-run] [--json] [--diff] [--limit N]
                                    fetch, parse, and write the ledger
abmx sync --from-dir DIR [--label L]  offline sync from saved .eml files
abmx reset [--purge] [--yes]        forget delivered messages
abmx accounts [--json]              sink accounts and how [labels.accounts] maps onto them
abmx skill                          print the parser-authoring skill to stdout
abmx dump <path> [--json]           normalized block dump (authoring aid)

abmx parser paths [--json]          parser search order and which directory won
abmx parser specs [--format F]      loaded parsers and their files
abmx parser schema                  JSON Schema of the transaction contract
abmx parser parse <path> --parser ID [--format json|ndjson|table]
                                    parse a file or directory with one parser
abmx parser explain <path> --parser ID
                                    block regions behind each field
abmx parser lint [--parser ID]      static spec checks
abmx parser check [-v] [--parser ID]
                                    verify every parser against its fixtures
abmx parser init <id>               scaffold ~/.config/abmx/parsers/<id>/{config.toml,fixtures}
```

Every `abmx parser` command takes `--parsers <dir>` (repeatable) or
`$ABMX_PARSERS`; implicit discovery is `~/.config/abmx/parsers`, then
`/etc/abmx/parsers`. The working directory is never read. Fixtures are always
read from the winning parser's own `fixtures/` directory. See
[Parser spec](/reference/parser-spec/) for the format and
[Transaction contract](/reference/transaction/) for the output.

## stdout is data, stderr is diagnostics

`sync --dry-run --json` writes the plan JSON to stdout and the human summary to
stderr, so it pipes cleanly:

```bash
abmx sync --dry-run --json > plan.json
```

`-q` prints only the two summary lines; `-v` adds the parser id and source
message key to each row.

`--diff` (with `--dry-run`) additionally reads the ledger and reports rows that
are already present, including any imported under a foreign `imported_id`.

`--from-dir` reads saved `.eml` files instead of calling Gmail; with a flat
directory it needs `--label`, unless exactly one parser is configured. This is
the offline path for testing a parser against real mail:

```bash
abmx sync --dry-run --from-dir ./saved-emails --label "receipt/<id>"
```

## Exit codes

| Code | Meaning                                                                |
| ---- | ---------------------------------------------------------------------- |
| `0`  | clean                                                                  |
| `1`  | the tool failed (config, auth, network, state)                         |
| `2`  | one or more routed labels need a parser or account fix                 |

`sync --dry-run` exits `2` when a label is broken, so a timer or healthcheck can
alert without writing anything. A normal `sync` writes no rows at all when any
label is broken: the summary names the label and the next command.

## Parser exit codes

`abmx parser parse` and `check` use a second scale, because "heuristic" is a
distinct outcome from "failed":

| Code | Meaning                                                                      |
| ---- | ---------------------------------------------------------------------------- |
| `0`  | every input produced a `strict` transaction                                  |
| `2`  | at least one email was unmatched or failed to parse                          |
| `3`  | at least one transaction was `heuristic` (suppress with `--allow-heuristic`) |
| `1`  | the tool itself failed                                                       |

## Status

`abmx status` runs the checks in one place: config, parser corpus, labels,
accounts, Gmail auth, and the sink. `--offline` skips the Gmail and sink network
calls. `--check` is the healthcheck shape used by timers. `--strict` samples live
mail for managed labels and reports per-label status. `--json` is for scripts.

Statuses such as `missing_parser`, `parse_failed`, and `mixed_shapes` are
documented in [Authoring parsers](/guides/authoring/#statuses).
