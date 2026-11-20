# statement parser spec (PDF)

A statement parser lives at `<corpus>/<id>/config.toml`; the directory name is
the parser id. It is loaded from the same corpus as receipt parsers and is
distinguished by `input = "pdf"`. Statements are imported by
`abmx statement sync` (or the combined `abmx sync`).

## Authoring loop

1. Get one or more PDFs for the same bank/template. Never commit them: they
   contain PII.
2. Dump the positioned text:

   ```sh
   abmx statement dump statement.pdf --password-file password.txt
   ```

   Each line shows `y | [x] "text"`. Rows are lines whose date column matches a
   date; the column ranges come from the x values on those lines.
3. Write `config.toml`.
4. Verify against every fixture:

   ```sh
   abmx statement check --parsers <corpus> --parser <id> --password-file password.txt -v
   ```

   A statement is done only when every fixture is `strict`. There is no
   per-row heuristic: one arithmetic mismatch rejects the whole document.

To import one finished PDF without Gmail:

```sh
abmx statement import statement.pdf --parser <parser-id>
```

The ledger deduplicates by `imported_id`, so repeating the import is safe; no
state is written and the PDF never leaves the machine.

## What `strict` means

The parser proves the document against its own numbers, per section:

- `opening + Σ(credits) − Σ(debits) == closing`;
- for every row in document order:
  `previous_balance + signed_amount == row_balance`;
- `Σ parsed credits == the printed credit total`;
- `Σ parsed debits == the printed debit total`;
- every row date falls inside the printed period.

If a page is dropped, a row is missed at a page boundary, a sign is inverted, or
a column range is off by a point, at least one invariant fails. Do not weaken
the spec to make it pass; fix the column, the direction marker, or the skip
rules.

## Spec

```toml
spec_version = 1
version = 1
input = "pdf"

[defaults]
bank = "example"
currency = "IDR"
tz = "Asia/Jakarta"
offset = "+07:00"          # fixed offset for occurred_at
number = "en"              # en: 1,234.56   id: 1.234,56

[document]
# Must capture start and end.
period = { regex = 'Period\s*:\s*(?P<start>\d{1,2}/\d{2}/\d{2})\s*s/d\s*(?P<end>\d{1,2}/\d{2}/\d{2})',
           formats = ["%d/%m/%y"] }
# Optional; used only for identity, never emitted.
account = { regex = '(?P<value>\d{3}-\d{2}-\d{7}-\d)' }
# Optional; catches a truncated download.
pages = { regex = 'Page\s+\d+\s+of\s+(?P<value>\d+)' }

[[sections]]
key = "savings"            # becomes Transaction.account; map `<id>:savings`
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
# A line is a row when this cell matches. The opening row is skipped by text.
row_when = { column = "date", regex = '^\d{2}/\d{2}$' }
skip = ['^Saldo Awal$']
# The printed transaction date has no year; it is resolved against the period.
date = { format = "%d/%m" }
# The marker means money left the account. No marker means a credit.
direction = { out_marker = "D" }
```

## Rules and pitfalls

- Columns are geometric. Do not try to parse a row with one line-wide regex:
  descriptions can contain dates and digit strings that look like amounts.
- `description` must end before the leftmost amount start; `amount` must end
  before the marker; `balance` must start after the marker.
- A row without the marker is a credit. If a bank prints `C`, add it in a
  future `direction.in_marker`; a mistake shows up as a broken credit total.
- Amounts are parsed at scale 2 and converted to the currency's minor units.
  An amount with more precision than the currency allows is an error, not a
  silent truncation.
- Multi-page is normal: pages are read in page-tree order and rows continue
  across them. Repeated headers and page numbers are ignored by the date-cell
  predicate. Test a long month before trusting a new template.
- The account is resolved from `transaction.account`: the config must contain
  `<parser-id>` for a single-product document or `<parser-id>:<section-key>`
  for a multi-product one.
- The password is never part of the spec. It comes from
  `[statements.passwords].<id>` (`file` or `env`).

## PII

The dump, the fixtures, and `--json` output contain the account holder's name,
address, account number, and transactions. Keep them outside the repository and
never paste them into an issue.
