---
title: Parser spec
description: The Rhai format for a mail parser, the host API, and how extraction works.
---

Two things are versioned here: the **transaction output** (`SCHEMA_VERSION`, see
the [Transaction contract](/reference/transaction/)) and the **host API**
(`meta().api` in every `<corpus>/<id>/parser.rhai`).

A parser is a script, not a data file. That is deliberate: bank templates are
open-ended, and a declarative schema eventually grows control flow without
becoming a language. The script gets a small, sandboxed host API; identity,
offsets, and the `strict` decision stay in Rust.

## Where parsers are loaded from

Parser scripts are runtime configuration; none are compiled into the binary.

**Explicit sources merge.** Every directory named by `--parsers` or
`$ABMX_PARSERS` is loaded in order and merged by parser `id`; when two
directories define the same `id` the earlier one wins and the later one is
reported as shadowed.

**Implicit discovery uses one corpus.** When neither flag nor env is set, the
first directory that **exists** wins:

| Order | Source                          |
| ----- | ------------------------------- |
| 1     | `$XDG_CONFIG_HOME/abmx/parsers` |
| 2     | `/etc/abmx/parsers`             |

There is no router file: a parser is selected by the `receipt/*` label mapped to
it in `abmx.toml`.

## Layout

```text
<corpus>/<id>/
  parser.rhai
  fixtures/
    01.eml
```

The directory name **is** the parser id. The script defines `meta()` and exactly
one entry point: `parse_mail(ctx)` or `parse_statement(doc)`. Defining both, or
neither, is a load error.

## Input

One RFC 5322 message (`.eml`) per fixture. The parser needs the body blocks, the
`Date` header (for the timezone offset), and the `Message-ID` (identity).

## Format

```rhai
fn meta() {
    #{
        bank: "example",
        currency: "IDR",
        tz: "Asia/Jakarta",
        offset: "+07:00",        // fixed offset used for occurred_at
        number: "id",            // id: 1.234,56   en: 1,234.56
        kind: "purchase",        // decides the sign
        labels: [                // every constant label block, in order
            "Total Payment", "Merchant", "Transaction Date",
        ],
        api: 1,
        version: 1,
    }
}

fn parse_mail(ctx) {
    #{
        amount: ctx.money_regex_in("Total Payment", `\d[0-9.,]*`, "id"),
        occurred_at: ctx.datetime_in("Transaction Date", ["%d %b %Y %H:%M:%S"], "id"),
        counterparty: ctx.text_in("Merchant"),
    }
}
```

## meta()

| Key | Required | Meaning |
| --- | --- | --- |
| `bank` | yes | short bank name |
| `currency` | yes | ISO-4217 |
| `tz` | no | IANA zone, informational |
| `offset` | no | fixed offset, default `+00:00` |
| `number` | no | `en` (default) or `id` |
| `kind` | yes | `purchase`, `payment`, `transfer_in`, `transfer_out`, `fee`, `refund`, `cash_withdrawal` |
| `labels` | yes | every constant label block |
| `api` | no | host API version, default 1 |
| `version` | no | parser version, default 1 |
| `enabled` | no | default `true` |
| `exponent` | no | override the currency's minor-unit exponent |

`labels` partitions the body: a region is everything between one label and the
next. Include labels you do not extract, because they are boundaries. A missing
label merges two regions; an extra label splits a value.

## ctx

| Expression | Result |
| --- | --- |
| `ctx.from`, `ctx.from_name`, `ctx.subject` | envelope fields |
| `ctx.sent_at`, `ctx.offset` | RFC 3339 date and the email's offset |
| `ctx.blocks` | array of every block |
| `ctx.has_label(label)` | is the label present |
| `ctx.region(label)` | array of blocks in the region |
| `ctx.text_in(label)` | region collapsed to one string |
| `ctx.block_in(label, n)` | Nth block of the region |
| `ctx.regex_in(label, pattern)` | first capture in the region |
| `ctx.per_block(label, pattern)` | first block that matches |
| `ctx.matches_in(label, pattern)` | boolean |
| `ctx.money_in(label, style)` | region as money, in minor units |
| `ctx.money_regex_in(label, pattern, style)` | capture as money |
| `ctx.money(raw, style)` | arbitrary text as money |
| `ctx.datetime_in(label, formats, locale)` | `YYYY-MM-DDTHH:MM:SS` |
| `ctx.date_in(label, formats, locale)` | `YYYY-MM-DD` |
| `ctx.time_in(label, formats)` | `HH:MM:SS` |
| `date_text`, `time_text` | parse a captured value |
| `strip_spaces(raw)` | remove whitespace |

The `*_in` date helpers try the whole region and then every token window, so
`13 Sep 2026 12:44:09 WIB` parses with `["%d %b %Y %H:%M:%S"]`. Captures prefer
the first named group, then group 1, then the whole match.

## Return value

A map, or an array of maps when one email carries several transactions:

| Key | Required | Notes |
| --- | --- | --- |
| `amount` | yes | unsigned minor units; `kind` supplies the sign |
| `occurred_at` | yes | `YYYY-MM-DDTHH:MM:SS` or `YYYY-MM-DD` |
| `kind` | no | per-row override |
| `status` | no | `settled`, `pending`, `failed`, `unknown` |
| `counterparty` | no | payee |
| `counterparty_account` | no | destination account |
| `account_last4` | no | funding card/account |
| `reference_id` | no | bank reference |
| `channel` | no | `QRIS`, `Transfer`, ... |
| `description` | no | free text |
| `extra` | no | map of named fields |
| `warnings` | no | non-empty downgrades to heuristic |

## Sandbox

The script runs in a fresh scope per email, with no filesystem, network, clock,
environment, `eval`, or imports. Operations, call depth, allocation, and wall
time are capped. `print` is swallowed; a parser that needs to explain itself
returns a warning.

## Identity and money

- `id` is `hash(Message-ID, tx_index)`, never derived from parsed output.
- `amount_minor` is authoritative and signed. `currency_exponent` is repeated on
  every row (IDR = 0, USD = 2), unless `meta().exponent` overrides it.
- `kind` decides the sign; a printed minus sign is not trusted.
- The ledger account comes from `[receipts.accounts]`, never from the script.

## Versioning

- `api` changes only when the host API breaks scripts.
- `version` increments when a script's behaviour changes.
- `SCHEMA_VERSION` changes only on breaking output changes.

## Authoring aids

```bash
abmx parser init <id>                       # scaffold parser.rhai and fixtures/
abmx dump <file>                            # normalized block dump
abmx parser explain <file> --parser <id>    # the regions the script sees
abmx parser lint --parser <id>              # fixtures and label coverage
abmx parser check --parser <id> -v          # run every fixture
abmx parser specs --format table            # loaded parsers and their files
abmx parser schema                          # JSON Schema of the output contract
```

The full workflow is in [Authoring parsers](/guides/authoring/).
