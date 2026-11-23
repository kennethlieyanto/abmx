# Authoring a receipt parser

A receipt parser turns one transaction email (`.eml`) into one or more
transactions. It is a Rhai script at `<corpus>/<id>/parser.rhai`; the directory
name is the parser id. The corpus is normally `~/.config/abmx/parsers/` (see
`[general].parsers`). The host owns identity, the email offset, and the `strict`
verdict; the script only reads blocks.

The script never inspects the sender or subject. Routing is explicit: gmailctl
applies `receipt/<id>` and `[receipts.accounts]` maps that id to a ledger
account.

## Loop

1. **Sample.** `abmx receipt samples` writes the newest three messages per
   non-ignored label to `$XDG_CONFIG_HOME/abmx/samples/receipts/<parser-id>/`.
   Narrow it with `--label <label|parser-id>`. Fewer than three is a warning.
2. **Read the blocks.** `abmx dump <dir>` shows exactly what the parser sees;
   HTML and CSS are already gone.
3. **Confirm one template.** If a label's samples differ in shape, stop: the
   label is mislabeled. Do not write a parser that guesses.
4. **Pick the id.** `receipt/example-qris` becomes `example-qris`.
5. **Scaffold.** `abmx parser init example-qris` writes `parser.rhai` and
   `fixtures/`.
6. **Write `meta()`.** Fill `bank`, `currency`, `kind`, and `labels`. `labels`
   must contain every constant label block in document order, including ones you
   do not extract.
7. **Write `parse_mail(ctx)`.** Start with `amount` and `occurred_at`; add
   optional fields only where the samples prove them. Prefer `money_regex_in`
   when a region holds several numbers, and `strip_spaces` for grouped
   references.
8. **Status.** Map the printed words exactly. There is no subject-based status.
9. **Verify.** Copy the samples to `fixtures/01.eml`, `02.eml`, … and run
   `abmx parser check --parser <id> --parsers <corpus>` until all are `strict`.
10. **Clean up.** If it cannot pass, delete the script and report why. Never
    leave a broken parser in the corpus.

## Parser contract

```rhai
fn meta() {
    #{
        bank: "example",
        currency: "IDR",
        tz: "Asia/Jakarta",
        offset: "+07:00",
        number: "id",          // id: 1.234,56   en: 1,234.56
        kind: "purchase",      // decides the sign
        labels: ["Total Payment", "Merchant", "Transaction Date"],
        api: 1,
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

`bank` and `currency` are required. `number` is `en` (default) or `id`. `kind`
is `purchase`, `payment`, `transfer_in`, `transfer_out`, `fee`, `refund`, or
`cash_withdrawal`. `labels` partitions the body into regions: a missing label
merges two regions, an extra label splits a value. `offset` defaults to
`+00:00`; `api` is the host API version; `version` is yours.

## Reading the email (`ctx`)

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

Captures prefer the first named group, then group 1, then the whole match. The
`*_in` date helpers try the whole region and every token window, so
`13 Sep 2026 12:44:09 WIB` parses with `["%d %b %Y %H:%M:%S"]`.

## Returning rows

Return a map, or an array of maps when one email holds several transactions:

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

## Money, dates, identity

- `kind` signs the amount: `purchase`, `payment`, `transfer_out`, `fee`, and
  `cash_withdrawal` are outflows; `transfer_in` and `refund` are inflows. A
  printed minus sign does not decide it.
- `id` is `hash(Message-ID, tx_index)`; never write it.
- IDR has exponent 0. Set `exponent` only for genuine sub-unit precision. The
  wrong `number` style scales an amount by 100.
- Indonesian months need `locale = "id"`; English month names always work.
- The email `Date:` header is delivery time, not the transaction time. Read
  `occurred_at` from the body.
- Zero amounts are accepted but become `heuristic`, and the ledger refuses them.

## Verification

```sh
abmx parser check --parser <id> --parsers <corpus> [-v]
abmx parser parse <samples-dir> --parser <id> --parsers <corpus> --format table
abmx parser explain <one.eml> --parser <id>
abmx parser lint --parser <id>
```

`parser check` uses the parser's own `fixtures/`; `parser parse` runs over a file
or directory of `.eml` samples.

| Result | Meaning |
| --- | --- |
| `strict` | every declared field matched and the amount is non-zero; accept |
| `heuristic` | parsed with warnings (usually a zero amount); almost always the wrong region |
| `failed` | extraction errored; `parser explain` names the region and field |
| `unmatched` / `missing` | the parser id is not in the corpus; fix the id or directory |

Exit codes: `0` all strict, `3` at least one heuristic, `2` failed, `1` the tool
failed. Done means every fixture is `strict`.

## Pitfalls

- **Missing label.** The previous region swallows the next field, so an amount
  can match a date or a phone number. Include every constant label block.
- **Extra label.** A value that happened to be constant across samples is
  treated as a label and disappears from its region.
- **Multi-value region.** `text_in` collapses several blocks; use
  `block_in(label, n)` when the layout is positional.
- **End-anchored amount regex.** `[\d.,]+$` captures the last number, often a fee
  or footer reference. Match the shape instead.
- **Subject-based status.** There is no subject routing and no status field in
  `meta()`. If the outcome is only in the subject, the label must distinguish it.
- **Guessed account.** The parser has no account field; never write a UUID into
  a script.
- **Not bumping `version`.** Reusing a version after a behaviour change makes
  historical rows indistinguishable.
- **Unknown host function.** Rhai is dynamically typed, so a typo fails at the
  call site. `abmx parser check` executes every fixture; run it.

## Routing

```toml
[receipts.accounts]
"example-qris" = "<actual-account-uuid>"
```

Apply exactly one `receipt/<parser-id>` label per tracked message. A message
with two abmx labels aborts the run; a label without an entry is not fetched;
`[receipts].ignore` records declined labels. The UUID comes from `abmx accounts`
or `abmx setup`; never guess it.

## Stop rules

- No monetary amount in the samples → no parser. Report why.
- Samples that are different templates → report the label as mislabeled.
- Never invent a ledger account, a UUID, or an account mapping.
- Ambiguous value → leave the optional field out rather than guessing.
- Do not write into another parser's directory.
