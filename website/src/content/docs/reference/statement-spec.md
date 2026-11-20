---
title: Statement spec
description: The TOML format for a PDF statement parser, the column geometry, and the arithmetic that makes a parse strict.
---

A statement parser is a directory in the same corpus as a receipt parser:
`<corpus>/<id>/config.toml`. The directory name is the parser id, and
`input = "pdf"` tells the loader which engine compiles it. Statements are run by
`abmx statement sync` (or the combined `abmx sync`).

Two things are versioned: the spec format (`spec_version` below) and the shared
transaction contract (`SCHEMA_VERSION`, see [Transaction](/reference/transaction/)).

## Authoring loop

```sh
# 1. Evidence: positioned text, not the PDF
abmx statement dump statement.pdf --password-file password.txt

# 2. Write <corpus>/<id>/config.toml and put the PDFs in <corpus>/<id>/fixtures/

# 3. Verify every fixture, including the arithmetic
abmx statement check --parsers <corpus> --parser <id> --password-file password.txt -v
```

`dump` prints one line per text line: `y | [x] "text"`. Rows are the lines whose
date cell matches a date; the x values on those lines are the column ranges.
The dump contains the account holder's details, so keep it out of the
repository and out of issues.

## What strict means

Emails are strict when every declared field matched. A statement is strict when
its own numbers agree, per section:

1. `opening + Σ(credits) − Σ(debits) == closing`
2. for every row in document order:
   `previous_balance + signed_amount == row_balance`
3. `Σ parsed credits == the printed credit total`
4. `Σ parsed debits == the printed debit total`
5. every row date falls inside the printed period

One mismatch rejects the whole document. There is no partial import and no
per-row heuristic; a statement that cannot be proven is not written. The
invariants are also what catches multi-page mistakes: a dropped page, a row
split at a page boundary, or pages read out of order all break the balance walk.

## Format

```toml
spec_version = 1
version = 1
input = "pdf"

[defaults]
bank = "example"
currency = "IDR"
tz = "Asia/Jakarta"
offset = "+07:00"          # fixed offset used for occurred_at
number = "en"              # en: 1,234.56   id: 1.234,56

[document]
# Must capture start and end.
period = { regex = 'Period\s*:\s*(?P<start>\d{1,2}/\d{2}/\d{2})\s*s/d\s*(?P<end>\d{1,2}/\d{2}/\d{2})',
           formats = ["%d/%m/%y"] }
# Optional; hashed into the document identity, never emitted.
account = { regex = '(?P<value>\d{3}-\d{2}-\d{7}-\d)' }
# Optional; catches a truncated download.
pages = { regex = 'Page\s+\d+\s+of\s+(?P<value>\d+)' }

[[sections]]
key = "savings"            # becomes `Transaction.account`
start = 'Tabungan\s*/\s*Savings'   # a line that opens this section
opening = { regex = 'Opening Balance\s*:\s*(?P<value>[\d.,]+)' }
closing = { regex = 'Closing Balance\s*:\s*(?P<value>[\d.,]+)' }
total_credit = { regex = 'Total Credits\s*:\s*(?P<value>[\d.,]+)' }
total_debit = { regex = 'Total Debits\s*:\s*(?P<value>[\d.,]+)' }

[sections.columns]
# Half-open x ranges in PDF points: min <= x < max.
date = { x = [35, 96] }
value_date = { x = [96, 139] }
description = { x = [139, 452] }
amount = { x = [452, 521] }
marker = { x = [521, 560] }
balance = { x = [560, 650] }

[sections.table]
# A line is a row when this cell matches.
row_when = { column = "date", regex = '^\d{2}/\d{2}$' }
# Matched against the description cell and the whole line.
skip = ['^Saldo Awal$']
# The printed transaction date has no year; resolve it against the period.
date = { format = "%d/%m" }
# A marked row left the account; an unmarked row is a credit.
direction = { out_marker = "D" }
```

`description` must end before the leftmost amount start, `amount` must end
before the marker, and `balance` must start after it. Those ranges are why the
table is read column-wise rather than with one line-wide regex: a description
may contain dates (`03-08-26`) or long digit strings that look exactly like
amounts.

### Multi-product documents

A consolidated statement can contain several products. Give each product its
own `[[sections]]` entry and key, and map `<parser-id>:<key>` in the config.
The arithmetic is checked per section, because each product has its own opening
and closing balance.

## Identity

- **Document**: `hash(bank, account, period_start, period_end)`. Re-fetching the
  same statement is a no-op; a re-issued statement for the same period with
  different content fails the arithmetic or produces different row ids rather
  than silently double-importing.
- **Row**: content-based, including the running balance. Two identical
  same-day, same-amount rows are still distinct because their balances differ.

Ids are stable across parser versions and do not depend on the file name or the
Gmail message id.

## Passwords

The spec never contains a password. Configure a reference in `abmx.toml`:

```toml
[statements.passwords]
"example.consolidated_statement" = { file = "/run/secrets/abmx/statement" }
# or: { env = "ABMX_STATEMENT_PASSWORD_EXAMPLE" }
```

An owner-restricted PDF with an empty user password needs no configuration.
Decryption happens in-process: the password is never passed on a command line
and the decrypted PDF is never written to disk. A wrong password is reported as
`wrong password`, a missing one as `password protected`, and a scanned PDF as
`no extractable text layer` — never as a regex failure.
