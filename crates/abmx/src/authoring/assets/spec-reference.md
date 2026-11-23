# bank parser spec reference (Rhai)

A parser is a directory: `<corpus>/<id>/parser.rhai`. The directory name **is**
the parser id, and `fixtures/` sits beside the script. There is no TOML.

A mail parser defines `fn meta()` and `fn parse_mail(ctx)`. The script only
observes the email; identity, the offset, and `Strict` are decided by the host.

## Skeleton

```rhai
fn meta() {
    #{
        bank: "example",
        currency: "IDR",
        tz: "Asia/Jakarta",
        offset: "+07:00",
        number: "id",          // id: 1.234,56   en: 1,234.56
        kind: "purchase",      // decides the sign
        labels: [              // every constant label block in the template
            "Total Payment", "Merchant", "Transaction Date",
        ],
        api: 1,                // host API version
        version: 1,            // bump when behaviour changes
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
| `bank` | yes | Short bank name, e.g. `bca` |
| `currency` | yes | ISO-4217 |
| `tz` | no | IANA zone, informational |
| `offset` | no | Fixed offset for `occurred_at`, default `+00:00` |
| `number` | no | `en` (default) or `id` |
| `kind` | yes | `purchase`, `payment`, `transfer_in`, `transfer_out`, `fee`, `refund`, `cash_withdrawal` |
| `labels` | yes | Every constant label block, including ones you do not extract |
| `api` | no | Host API version, default 1 |
| `version` | no | Parser version, default 1 |
| `enabled` | no | Default `true` |
| `exponent` | no | Override the currency's minor-unit exponent |

`labels` partitions the body into regions: a region is everything between one
label and the next. A missing label merges two regions; an extra label splits a
value. Include all of them, in document order.

## ctx

| Expression | Result |
| --- | --- |
| `ctx.from` / `ctx.from_name` | sender address / display name |
| `ctx.subject` | subject |
| `ctx.sent_at` | email `Date:` as RFC 3339 |
| `ctx.offset` | the email's offset, e.g. `+07:00` |
| `ctx.blocks` | every block as an array of strings |
| `ctx.text` | all blocks joined with spaces |
| `ctx.has_label(label)` | whether the label appears |
| `ctx.region(label)` | the label's region as an array of strings |
| `ctx.text_in(label)` | the region collapsed to one string |
| `ctx.block_in(label, n)` | the Nth block of the region (0-based) |
| `ctx.regex_in(label, pattern)` | first capture in the joined region |
| `ctx.per_block(label, pattern)` | first block that matches |
| `ctx.matches_in(label, pattern)` | boolean |
| `ctx.money_in(label, style)` | region parsed as money, in minor units |
| `ctx.money_regex_in(label, pattern, style)` | capture parsed as money |
| `ctx.money(raw, style)` | arbitrary text parsed as money |
| `ctx.datetime_in(label, formats, locale)` | `YYYY-MM-DDTHH:MM:SS` |
| `ctx.date_in(label, formats, locale)` | `YYYY-MM-DD` |
| `ctx.time_in(label, formats)` | `HH:MM:SS` |
| `date_text(raw, formats, locale)` | parse a captured date string |
| `time_text(raw, formats)` | parse a captured time string |
| `strip_spaces(raw)` | remove all whitespace |

`*_in` datetime helpers are loose: they try the whole region and then every
token window, so `13 Sep 2026 12:44:09 WIB` parses with
`["%d %b %Y %H:%M:%S"]`.

Captures prefer the first **named** group, then group 1, then the whole match.

## Return value

Return a map, or an array of maps for a template that contains several
transactions:

```rhai
fn parse_mail(ctx) {
    [#{
        amount: ctx.money_in("Charge 1", "id"),
        occurred_at: ctx.datetime_in("Date", ["%d/%m/%Y %H:%M"], "en"),
        counterparty: "First",
    }, #{
        amount: ctx.money_in("Charge 2", "id"),
        occurred_at: ctx.datetime_in("Date", ["%d/%m/%Y %H:%M"], "en"),
        counterparty: "Second",
    }]
}
```

| Key | Required | Notes |
| --- | --- | --- |
| `amount` | yes | unsigned minor units; `kind` supplies the sign |
| `occurred_at` | yes | `YYYY-MM-DDTHH:MM:SS` or `YYYY-MM-DD` |
| `kind` | no | overrides `meta.kind` for this row |
| `status` | no | `settled` (default), `pending`, `failed`, `unknown` |
| `counterparty` | no | payee |
| `counterparty_account` | no | destination account |
| `account_last4` | no | last four digits of the funding account |
| `reference_id` | no | bank reference |
| `channel` | no | `QRIS`, `Transfer`, ... |
| `description` | no | free text |
| `extra` | no | map of named extra fields |
| `warnings` | no | non-empty makes the row heuristic |

Zero amounts are accepted but downgrade the row to heuristic, and the ledger
policy refuses them.

## Identity and money

- `id` is `hash(Message-ID, tx_index)`. A parser change never changes it.
- `amount_minor` is authoritative and signed; `kind` decides the sign:
  `purchase`, `payment`, `transfer_out`, `fee`, `cash_withdrawal` are outflows.
- `currency_exponent` is repeated on every row. IDR is 0; if a bank prints
  sub-unit precision, set `exponent` in `meta()`.
- The ledger account comes from `[receipts.accounts]`, never from the parser.

## Routing

Routing is not in the script. gmailctl applies `receipt/<id>`, and the main
config maps the id to a ledger account:

```toml
[receipts.accounts]
"example-qris" = "<actual-account-uuid>"
```

## Stop rules

- No amount in the samples → no parser.
- Samples that are different templates → the label is mislabeled; fix gmailctl.
- A value that is ambiguous → leave the optional field out.
- Never write a ledger UUID into a parser.
