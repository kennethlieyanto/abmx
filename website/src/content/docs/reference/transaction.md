---
title: Transaction contract
description: The JSON a parser emits, field by field, and the identity and money rules consumers rely on.
---

`abmx parser parse <path> --format json` writes a JSON array of transactions to
**stdout**. Diagnostics always go to **stderr**. Nothing else is written to
stdout, so it is safe to pipe.

Run `abmx parser schema` for the machine-readable JSON Schema, generated from the
Rust types so it cannot drift from the implementation.

```json
{
  "source": {
    "message_id": "txredact.1e988369dd5c5fbc@example.invalid",
    "file": "fixtures/01.eml",
    "tx_index": 0
  },
  "id": "mail_a631e3d89a232b55",

  "parser": "example.card_debit",
  "parser_version": 1,
  "bank": "example",
  "account": "example-account",

  "kind": "purchase",
  "status": "settled",

  "amount_minor": -27500,
  "currency": "IDR",
  "currency_exponent": 0,
  "amount": "-27500",

  "occurred_at": "2026-10-30T17:28:17+07:00",
  "occurred_on": "2026-10-30",
  "timezone": "Asia/Jakarta",

  "counterparty": "ExamplePay",
  "counterparty_account": null,
  "account_last4": "4821",
  "reference_id": null,
  "channel": "DEBIT ONLINE",
  "description": null,
  "extra": {},

  "extraction": "strict",
  "warnings": []
}
```

| Field                      | Type            | Meaning                                                                                                                             |
| -------------------------- | --------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `source.message_id`        | string?         | Value of the `Message-ID` header, without angle brackets                                                                            |
| `source.file`              | string?         | Where the message came from, when known                                                                                             |
| `source.tx_index`          | integer         | Index of the transaction inside the email. Always `0` today; reserved for invoices and statements that contain several transactions |
| `id`                       | string          | Deterministic identity: `hash(message_id, tx_index)`. Never changes when a parser changes. Use it as your dedupe key                |
| `parser`, `parser_version` | string, integer | Which spec produced this row                                                                                                        |
| `bank`, `account`          | string          | Parser-declared account key. Map it to your own account externally                                                                  |
| `kind`                     | enum            | `purchase`, `payment`, `transfer_in`, `transfer_out`, `fee`, `refund`, `cash_withdrawal`, `unknown`                                 |
| `status`                   | enum            | `settled`, `pending`, `failed`, `unknown`. `failed` must not be imported                                                            |
| `amount_minor`             | integer         | Signed amount in the currency's minor units. Negative is an outflow                                                                 |
| `currency`                 | string          | ISO-4217 code                                                                                                                       |
| `currency_exponent`        | integer         | ISO-4217 minor-unit exponent (IDR = 0, USD = 2). Repeated so consumers need no currency table                                       |
| `amount`                   | string          | Exact decimal rendering of `amount_minor`. Display only                                                                             |
| `occurred_at`              | string          | RFC 3339 timestamp with the offset the email carried                                                                                |
| `occurred_on`              | string          | Calendar date in the account timezone. This is the date a ledger should use                                                         |
| `timezone`                 | string          | IANA zone from the parser spec                                                                                                      |
| `counterparty`             | string?         | Merchant, recipient, or payer as printed in the email                                                                               |
| `counterparty_account`     | string?         | Their account or PAN, when printed                                                                                                  |
| `account_last4`            | string?         | Card/account hint extracted from the email                                                                                          |
| `reference_id`             | string?         | Bank reference, when the email carries one                                                                                          |
| `channel`                  | string?         | `QRIS`, `BI-FAST`, `DEBIT ONLINE`, ...                                                                                              |
| `description`              | string?         | Free-text description from the email                                                                                                |
| `extra`                    | object          | Additional parser-specific fields                                                                                                   |
| `extraction`               | enum            | `strict` (every declared field matched and the amount is non-zero) or `heuristic` (see `warnings`)                                  |
| `warnings`                 | array           | Why the row is heuristic                                                                                                            |

## Plausibility checks

One check runs outside the spec: **a zero amount marks the row `heuristic`**,
because a parser that extracts `0` has almost certainly matched the wrong block.

Transaction dates are trusted as printed. Banks send alerts late, batch them, or
mail a settlement notice days after the fact, so the gap between the transaction
date and the email `Date:` header carries no meaning and is not checked.

## Identity

`id` is derived from email metadata only, never from parsed output. Consequences:

- Re-parsing after a spec change never creates a second row.
- Two byte-identical emails (a real thing) collapse to one `id`.
- The archive filename does not matter.

The Actual sink uses this id as `imported_id`; see
[Actual Budget](/guides/actual/#imported_id-is-the-parsers-id).

## Money

`amount_minor` is authoritative. To render it:

```text
amount = amount_minor / 10^currency_exponent
```

To convert to a system that always uses two decimals:

```text
amount = amount_minor × 10^(2 − currency_exponent)
```

Do not do arithmetic on `amount`.
