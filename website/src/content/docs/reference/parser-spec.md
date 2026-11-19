---
title: Parser spec
description: The TOML format for a bank template, how specs are discovered, and how extraction works.
---

Two things are versioned here: the **transaction output** (`SCHEMA_VERSION`, see
the [Transaction contract](/reference/transaction/)) and the **parser spec
format** (`spec_version` in every `<corpus>/<id>/config.toml`).

## Where specs are loaded from

Parser specs are runtime configuration; none are compiled into the binary.

**Explicit sources merge.** Every directory named by `--parsers` or
`$ABMX_PARSERS` is loaded in order and merged by parser `id`; when two
directories define the same `id` the earlier one wins and the later one is
reported as shadowed. Two parsers with the same id cannot coexist inside one
corpus, because the directory name is the id. This allows a local override of a
single bank without copying the whole corpus.

**Implicit discovery uses one corpus.** When neither flag nor env is set, the
first directory that **exists** wins:

| Order | Source                          |
| ----- | ------------------------------- |
| 1     | `$XDG_CONFIG_HOME/abmx/parsers` |
| 2     | `/etc/abmx/parsers`             |

If that directory contains no specs, loading fails rather than falling through
to a lower-precedence corpus: an empty corpus in the place you asked it to look
is a broken install, and silently reading a different one is worse.

`abmx parser paths` prints the chain and marks skipped candidates. There is no
router file: a parser is selected by the `receipt/*` label mapped to it in
`abmx.toml`.

## Input

One RFC 5322 message (`.eml`) per file, or a directory of them. Anything that can
produce `.eml` bytes works: a mail export, an IMAP fetch, a Gmail API response
decoded from base64url, or a saved test fixture.

The parser needs:

- the block list from the body,
- the `Date` header (used only to give `occurred_at` its timezone offset),
- the `Message-ID` (identity),
- a `text/html` or `text/plain` body (extraction).

## Parser spec format

One parser per label-visible template, in `<corpus>/<id>/config.toml`. The
directory name **is** the parser id; there is no `id` field to disagree with it.
`spec_version` must match the engine's supported version. There is no `[match]`
section and no account field.

```toml
spec_version = 2
version = 1                 # bump when the spec changes
enabled = true

[labels]
list = ["Amount", "Merchant", "Date"]  # ALL constant label blocks in the template

[defaults]
bank     = "example"
currency = "IDR"
tz       = "Asia/Jakarta"
kind     = "purchase"

[status]                    # optional; omit for settled
label  = "Status"           # required when the template prints one
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
2. The engine partitions the blocks on `labels.list`. A _region_ is everything
   between one label and the next, which is why a value may span several blocks
   (`Rp` / `100.000` / `,00`).
3. Each field picks a value from its region:

| Setting                      | Behaviour                                                    |
| ---------------------------- | ------------------------------------------------------------ |
| nothing                      | the whole region, joined with spaces                         |
| `regex`                      | first match in the joined region                             |
| `per_block = true` + `regex` | first block that matches                                     |
| `block = N`                  | the Nth block of the region (optionally filtered by `regex`) |

The captured group is the first **named** group (`value`, `last4`, ...), else
group 1, else the whole match.

Prefer a pattern that matches the value's shape over an end anchor: a region can
run into the footer, and `[\d.,]+$` will happily capture a phone number.

## Versioning rules

- `spec_version` changes only when the TOML format changes.
- `parser.version` increments when a spec's behaviour changes. It is recorded on
  every row, but it never affects `id`.
- `SCHEMA_VERSION` increments on breaking changes to the output JSON.

## Authoring aids

```bash
abmx parser init <id>                       # scaffold the directory
abmx dump <file>                            # normalized block dump
abmx parser explain <file> --parser <id>    # the region behind each field
abmx parser lint --parser <id>              # static spec checks
abmx parser check --parser <id> -v          # verify against fixtures
abmx parser specs --format table            # loaded parsers and their files
abmx parser schema                          # JSON Schema of the output contract
```

The full workflow, including the AI-assisted path, is in
[Authoring parsers](/guides/authoring/).
