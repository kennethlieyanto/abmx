---
title: Introduction
description: What abmx does, how it thinks, and what it deliberately does not do.
---

`abmx` fetches bank notification mail, parses it with a parser corpus you can
teach without rebuilding, decides what is safe to import, and writes it to an
Actual Budget ledger. It never guesses: an unsure parse, an unmapped account, or
a row the bank reported as failed is reported as a broken label and retried
after the parser or Gmail rule is fixed.

```
Gmail (routed labels) ──► abmx ──► parser corpus ──► classify ──► Actual Budget
                          fetch     .eml → Transaction   import      apply
```

## Status

Early, and honest about it. One fetcher (Gmail), one real sink (Actual Budget),
one reference sink (JSONL file). The primitives are stable; the integrations are
thin. `abmx` ships no bank parsers; author them at runtime under
`~/.config/abmx/parsers/`. The repository contains only a synthetic example under
`examples/parsers/`.

## Why mail, not statements?

The obvious alternative is to parse the monthly statement instead. Statements
are complete and bank-authoritative, but they are also about a month late, and
the whole point here is to see a transaction the day it happens.

|                        | Email notification                                       | Monthly statement         |
| ---------------------- | -------------------------------------------------------- | ------------------------- |
| Latency                | minutes                                                  | ~30 days                  |
| Granularity            | one event per message                                    | whole period              |
| Completeness           | lossy (thresholds, spam, channels the bank does not email) | bank-authoritative      |
| Metadata               | merchant, channel, reference id                          | often less                |
| Balances               | usually not captured                                     | opening + closing         |
| Parsing cost           | stable HTML labels                                       | PDF/CSV/Excel, fragile    |

So email is the live path, and statements are the reconciliation path. Statement
import is planned, not implemented: when it lands it will backfill missed rows
and anchor per-account balances. Until then, `abmx` clears nothing
automatically — Actual's `cleared` flag stays yours to set against a statement.

## Routing is explicit

Two places own routing, and nothing else does:

- **gmailctl** labels the mail. A tracked message carries exactly one
  `receipt/<parser-id>` label; merchant receipts keep the plain `receipt` label
  and are never imported.
- **`[labels.accounts]`** maps that parser id to an Actual account. That table is
  the *only* definition of what abmx manages: labels without an entry are not
  fetched at all, so a half-configured mailbox is never a sync failure.

`abmx` never classifies mail and never matches senders. A label is the whole
contract.

## How duplication is prevented

Three independent layers, because they fail in different ways:

1. **`imported_id`** — every row carries the parser's stable id (a hash of the
   email's `Message-ID`). Actual's `importTransactions` never adds the same
   `imported_id` twice, so replaying a plan is safe.
2. **State** — a delivered message is recorded so the next run does not
   re-download or re-parse it. This is an optimisation for idempotent sinks and a
   requirement for sinks that have no idempotency key.
3. **Label scan + state** — every run lists the mapped labels under
   `[labels].prefix`, drops messages already recorded as delivered, then
   downloads and parses only the remaining ones. Mail labeled late is picked up
   on the next run; a broken row is never marked delivered, so it is retried
   after the parser is fixed.

## The four primitives

| Primitive   | Responsibility                    | Ships with                 |
| ----------- | --------------------------------- | -------------------------- |
| **Fetcher** | a source of raw RFC 5322 bytes    | Gmail API, local directory |
| **Policy**  | `Transaction` → import or broken  | one pure function          |
| **Sink**    | decided rows → the ledger         | Actual Budget, JSONL file  |
| **State**   | delivered messages                | SQLite                     |

A new mailbox is a new `Fetcher`. A new ledger is a new `Sink`. Neither requires
touching the runner, the CLI, or the other integrations. Adding a *bank*, by
contrast, is a TOML file — no rebuild. See
[Architecture](/reference/architecture/) for why the seams are where they are.

## Out of scope, by design

- Parsing bank templates in code. Templates are runtime TOML specs.
- Statement import and balance reconciliation. Planned.
- Transfers linking (importing both sides of a transfer as one).
- Pending → settled reconciliation. Today the parser emits no `pending` rows;
  when it does, this needs a matching rule, not a heuristic.
- Multi-currency conversion. `amount_minor` and `currency_exponent` are carried
  through unchanged.
- Webhooks and push sources. The `Sink` trait is push-shaped, but nothing calls
  it from a request handler yet.

Ready to run it? Start with [Install](/start/install/) or the
[Quickstart](/start/quickstart/).
