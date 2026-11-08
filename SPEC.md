# The `txp` contract

Two things are versioned here: the **transaction output** (`SCHEMA_VERSION`) and
the **parser spec format** (`spec_version` in every `parsers/<id>.toml`).

Run `txp schema` for the machine-readable JSON Schema, generated from the Rust
types so it cannot drift from the implementation.

## Where specs are loaded from

Parser specs are runtime configuration; none are compiled into the binary. The
first source that yields a corpus wins:

| Order | Source |
|---|---|
| 1 | `--parsers <dir>` (repeatable) |
| 2 | `$TXP_PARSERS` (colon-separated) |
| 3 | `./parsers` |
| 4 | `$XDG_CONFIG_HOME/txp/parsers` |
| 5 | `/etc/txp/parsers` |

Explicit entries (1 and 2) **merge by parser `id`, earlier wins**; a duplicate id
inside one directory is an error. This allows a local override of a single bank
without copying the whole corpus.

Router policy resolves as `--router` → `$TXP_ROUTER` → `router.toml` inside the
first parser directory → built-in defaults. `txp paths` prints the chain.

## Input

One RFC 5322 message (`.eml`) per file, or a directory of them. Anything that can
produce `.eml` bytes works: a mail export, an IMAP fetch, a Gmail API response
decoded from base64url, or a saved test fixture.

The parser needs:

- the `From` address and `Subject` (routing),
- the `Date` header (invariant and timezone offset),
- the `Message-ID` (identity),
- a `text/html` or `text/plain` body (extraction).

## Output

`txp parse <path> --format json` writes a JSON array of transactions to **stdout**.
Diagnostics always go to **stderr**. Nothing else is written to stdout, so it is
safe to pipe.

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

| Field | Type | Meaning |
|---|---|---|
| `source.message_id` | string? | Value of the `Message-ID` header, without angle brackets |
| `source.file` | string? | Where the message came from, when known |
| `source.tx_index` | integer | Index of the transaction inside the email. Always `0` today; reserved for invoices and statements that contain several transactions |
| `id` | string | Deterministic identity: `hash(message_id, tx_index)`. Never changes when a parser changes. Use it as your dedupe key |
| `parser`, `parser_version` | string, integer | Which spec produced this row |
| `bank`, `account` | string | Parser-declared account key (`example-account`, `wallet-account`). Map it to your own account externally |
| `kind` | enum | `purchase`, `payment`, `transfer_in`, `transfer_out`, `fee`, `refund`, `cash_withdrawal`, `unknown` |
| `status` | enum | `settled`, `pending`, `failed`, `unknown`. `failed` must not be imported |
| `amount_minor` | integer | Signed amount in the currency's minor units. Negative is an outflow |
| `currency` | string | ISO-4217 code |
| `currency_exponent` | integer | ISO-4217 minor-unit exponent (IDR = 0, USD = 2). Repeated so consumers need no currency table |
| `amount` | string | Exact decimal rendering of `amount_minor`. Display only |
| `occurred_at` | string | RFC 3339 timestamp with the offset the email carried |
| `occurred_on` | string | Calendar date in the account timezone. This is the date a ledger should use |
| `timezone` | string | IANA zone from the parser spec |
| `counterparty` | string? | Merchant, recipient, or payer as printed in the email |
| `counterparty_account` | string? | Their account or PAN, when printed |
| `account_last4` | string? | Card/account hint extracted from the email |
| `reference_id` | string? | Bank reference, when the email carries one |
| `channel` | string? | `QRIS`, `BI-FAST`, `DEBIT ONLINE`, ... |
| `description` | string? | Free-text description from the email |
| `extra` | object | Additional parser-specific fields |
| `extraction` | enum | `strict` (all selectors matched, invariants held) or `heuristic` (see `warnings`) |
| `warnings` | array | Why the row is heuristic |

### Invariants

A row is `strict` only when:

- the amount is non-zero,
- the parsed transaction date is within **3 days** of the email `Date:` header.

The date invariant is deliberately independent of the parser spec: it is the
check that catches a selector that grabbed the wrong number off the page.

### Identity

`id` is derived from email metadata only, never from parsed output. Consequences:

- Re-parsing after a spec change never creates a second row.
- Two byte-identical emails (a real thing; see `banktwo/qris-payment/fixtures/02` and
  `03`) collapse to one `id`.
- The archive filename does not matter.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | every input produced a `strict` transaction |
| `2` | at least one email was unmatched or failed to parse |
| `3` | at least one transaction was `heuristic` (suppress with `--allow-heuristic`) |
| `1` | the tool itself failed |

## Money

`amount_minor` is authoritative. To render it: `amount_minor / 10^currency_exponent`.
To convert to a system that always uses two decimals:
`amount_minor * 10^(2 - currency_exponent)`.

Do not do arithmetic on `amount`.

## Parser spec format

One file per router-visible template, at `parsers/<id>.toml`. The file stem
must equal the spec `id`, and `spec_version` must match the engine's supported
version.

```toml
spec_version = 1
id      = "bank.template"   # stable; stored in every output row
version = 1                 # bump when the spec changes
enabled = true

[match]
senders  = ["alerts@bank.example"]  # exact, case-insensitive, or "*@bank.example"
subject  = '^Payment successful$'    # regex against the decoded subject
requires = ["Amount", "Merchant"]    # every one of these blocks must be present
rejects  = ["Transfer Amount"]       # none of these may be present
contains = ['^Order\s+ID']           # regex must match some block
excludes = ['^OTP']                  # regex must match no block

[labels]
list = ["Amount", "Merchant", "Date"]  # ALL constant label blocks in the template

[defaults]
bank     = "example"
account  = "example-account"
currency = "IDR"
tz       = "Asia/Jakarta"
kind     = "purchase"

[status]                    # optional; omit for settled
source = "subject"          # subject | field
label  = "Status"           # required when source = "field"
map    = { BERHASIL = "settled", GAGAL = "failed" }
default = "unknown"

[extract.amount]            # required
label  = "Total Payment"
regex  = '(?P<value>[0-9][0-9.,]*(?:\s*,\d{2})?)'
number = "en"               # en: 1,234.56    id: 1.234,56

[extract.occurred_at]       # required, single-field form
label   = "Transaction Date"
regex   = '(?P<value>\d{2} \w{3} \d{4} \d{2}:\d{2}:\d{2})'
formats = ["%d %b %Y %H:%M:%S"]
locale  = "en"              # "id" understands Okt/Agu/Mei/Des

[extract.counterparty]
label = "Merchant"

[extract.counterparty_account]
label = "Recipient"
block = 1                   # 0-based index inside the label's region
regex = '(?P<value>\d+)\s*$'

[extract.account]
label = "Nomor Kartu"
regex = '(?P<last4>\d{4})\s*$'

[extract.reference_id]
label     = "No. Ref wallet"
regex     = '(?P<value>[0-9][0-9 -]{5,}[0-9])'
normalize = "strip_spaces"

[extract.channel]
label = "Tipe Transaksi"
block = 0

[extract.description]
label = "Remarks"

[extract.extra]
location = { label = "Merchant Location" }
```

Split date and time (when the bank renders them as separate fields):

```toml
[extract.occurred_at]
date = { label = "Date", regex = '(?P<value>\d{1,2} \w{3} \d{4})', formats = ["%d %b %Y"] }
time = { label = "Time", regex = '(?P<value>\d{2}:\d{2}:\d{2})', formats = ["%H:%M:%S"] }
```

### Which fields are required

`amount` and `occurred_at` are required. Every other `[extract.*]` field is
optional; omit the section and the output field stays `null`.

### How extraction works

1. The body is normalized to an ordered list of visible text blocks, one per
   block-level HTML element. Tags and attributes are never part of the contract.
2. The engine partitions the blocks on `labels.list`. A *region* is everything
   between one label and the next, which is why a value may span several blocks
   (`Rp` / `100.000` / `,00`).
3. Each field picks a value from its region:

| Setting | Behaviour |
|---|---|
| nothing | the whole region, joined with spaces |
| `regex` | first match in the joined region |
| `per_block = true` + `regex` | first block that matches |
| `block = N` | the Nth block of the region (optionally filtered by `regex`) |

The captured group is the first **named** group (`value`, `last4`, ...), else
group 1, else the whole match.

Prefer a pattern that matches the value's shape over an end anchor: a region can
run into the footer, and `[\d.,]+$` will happily capture a phone number.

## Versioning rules

- `spec_version` changes only when the TOML format changes.
- `parser.version` increments when a spec's behaviour changes. It is recorded on
  every row, but it never affects `id`.
- `SCHEMA_VERSION` increments on breaking changes to the output JSON.
