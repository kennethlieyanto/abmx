# bank parser spec reference

A parser lives at `parsers/<id>/config.toml`. The directory name **is** the
parser id; there is no `id` field. `spec_version` must be the version below.
Changes to behaviour bump `version`; they never change a transaction's stable
`id`.

```toml
spec_version = 1
version = 1                 # bump when the spec changes
enabled = true

[match]
senders  = ["alerts@bank.example"]   # exact, case-insensitive, or "*@bank.example"
subject  = '^Payment successful$'     # regex against the decoded subject
requires = ["Amount", "Merchant"]     # every one of these blocks must be present
rejects  = ["Transfer Amount"]        # none of these may be present
contains = ['^Order\s+ID']            # regex must match some block
excludes = ['^OTP']                   # regex must match no block

[labels]
# ALL constant label blocks in the template, in document order.
# The engine partitions the blocks on these; a region is everything from one
# label to the next. Values may span several blocks (`Rp` / `100.000` / `,00`).
list = ["Amount", "Merchant", "Transaction Date"]

[defaults]
bank     = "example"
account  = "example-account"   # parser-side account KEY, never a ledger UUID
currency = "IDR"
tz       = "Asia/Jakarta"
kind     = "purchase"

[status]                  # optional; omit for settled
source = "subject"        # subject | field
label  = "Status"         # required when source = "field"
map    = { BERHASIL = "settled", GAGAL = "failed" }
default = "unknown"

[extract.amount]          # required
label  = "Total Payment"
regex  = '(?P<value>[0-9][0-9.,]*(?:\s*,\d{2})?)'
number = "en"             # en: 1,234.56    id: 1.234,56

[extract.occurred_at]     # required, single-field form
label   = "Transaction Date"
regex   = '(?P<value>\d{2} \w{3} \d{4} \d{2}:\d{2}:\d{2})'
formats = ["%d %b %Y %H:%M:%S"]
locale  = "en"            # "id" understands Okt/Agu/Mei/Des

[extract.counterparty]
label = "Merchant"

[extract.counterparty_account]
label = "Recipient"
block = 1                 # 0-based index inside the label's region
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

Split date and time when the bank renders them as separate fields:

```toml
[extract.occurred_at]
date = { label = "Date", regex = '(?P<value>\d{1,2} \w{3} \d{4})', formats = ["%d %b %Y"] }
time = { label = "Time", regex = '(?P<value>\d{2}:\d{2}:\d{2})', formats = ["%H:%M:%S"] }
```

## Required fields

- `[extract.amount]`
- `[extract.occurred_at]` (either `label` + `formats`, or `date` + `time`)

Every other `[extract.*]` section is optional. Omit it and the output field stays
`null`.

## How extraction works

1. The body is normalized to an ordered list of visible text blocks, one per
   block-level HTML element. Tags and attributes are never part of the contract.
2. The engine partitions the blocks on `labels.list`. A region is everything
   between one label and the next, which is why a value may span several blocks.
3. Each field picks a value from its region:

| Setting | Behaviour |
| --- | --- |
| nothing | the whole region, joined with spaces |
| `regex` | first match in the joined region |
| `per_block = true` + `regex` | first block that matches |
| `block = N` | the Nth block of the region (optionally filtered by `regex`) |

The captured group is the first **named** group (`value`, `last4`, ...), else
group 1, else the whole match.

Prefer a pattern that matches the value's shape over an end anchor: a region can
run into the footer, and `[\d.,]+$` will happily capture a phone number.

## Identity and money

- `id` is `hash(Message-ID, tx_index)`, never derived from parsed output.
- `amount_minor` is authoritative and signed. `currency_exponent` is repeated so
  consumers need no currency table (IDR = 0, USD = 2).
- `kind` is one of `purchase`, `payment`, `transfer_in`, `transfer_out`, `fee`,
  `refund`, `cash_withdrawal`, `unknown`.
- `status` is one of `settled`, `pending`, `failed`, `unknown`. A failed row must
  not be imported.

## Output fields a parser can fill

| Field | Source |
| --- | --- |
| `counterparty` | `[extract.counterparty]` |
| `counterparty_account` | `[extract.counterparty_account]` |
| `account_last4` | `[extract.account]` |
| `reference_id` | `[extract.reference_id]` |
| `channel` | `[extract.channel]` |
| `description` | `[extract.description]` |
| `extra` | `[extract.extra]` (a table of named fields) |

`bank`, `account`, `currency`, `tz`, and `kind` come from `[defaults]`.

## Routing

- One spec owns one *template*, not one sender. Several templates may share a
  sender and a subject; separate them with `requires` (a label that only this
  template has) and `rejects` (a label that a sibling template has).
- `requires` / `rejects` match **whole normalized blocks**, case-insensitively.
  `contains` / `excludes` are regexes over any block.
- `[status] source = "subject"` needs a named `status` capture in the subject
  regex, for example `'^Payment (?P<status>successful|failed)$'`.
- A more specific candidate (more `requires` + `contains`) wins when several
  parsers are eligible.

## Versioning rules

- `spec_version` changes only when the TOML format changes.
- `parser.version` increments when a spec's behaviour changes.
- `SCHEMA_VERSION` changes only on breaking output changes.
