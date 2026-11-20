---
title: Bank statements
description: Import monthly PDF statements, including the inflows that never generate an email.
---

Email notifications are the live path. Statements are the settlement path: the
bank has already posted every row, including the credits that never produce an
email. `abmx statement sync` turns a monthly PDF into transactions, proves the
document against its own totals, and adds only the rows the ledger does not
already have.

```text
Gmail label financial_statements/<id>
        │
        ▼
   PDF attachment ──► decrypt ──► positioned text ──► rows + totals
                                                        │
                                          arithmetic check (all or nothing)
                                                        │
                                     read ledger ──► new rows only
                                                        │
                                     add as cleared, never guess
```

## Setup

`abmx setup` asks for the statement prefix after the receipt labels. It lists
the labels under that prefix, lets you accept or decline each one, maps accepted
parsers to ledger accounts, and asks for a password file when the PDFs are
encrypted.

```toml
[statements]
enabled = true
prefix = "financial_statements/"
since  = 2026-01-01
ignore = ["example.portfolio_summary"]   # not a ledger

[statements.accounts]
"example.consolidated_statement" = "<actual-account-uuid>"
# Multi-product document:
# "example.consolidated_statement:savings"     = "<uuid>"
# "example.consolidated_statement:credit_card" = "<uuid>"

[statements.passwords]
"example.consolidated_statement" = { file = "/run/secrets/abmx/statement" }
```

The password is a reference, never a literal: the config is often committed and
the file may be a mounted Secret. An owner-restricted PDF with no user password
needs no entry.

## What the first run does

```sh
abmx statement sync --dry-run   # parse, verify, compare with the ledger
abmx statement sync
```

The dry run prints every row and the decision:

```text
fetched 4, parsed 25 rows — new 25, already present 0, ambiguous 0 (added), failed 0
```

- **new** — no plausible ledger row; it will be added and marked cleared.
- **already present** — the ledger already carries this `imported_id`, or has
  exactly one row with the same account and amount within three days. Email
  notifications carry the transaction date; statements carry the posting date,
  which is why the window exists.
- **ambiguous** — more than one ledger row could be the same event. abmx adds
  the statement row and reports it rather than guessing.
- **failed** — the PDF could not be decrypted, has no text layer, or does not
  balance. Nothing from that document is written; other documents are not
  blocked.

A statement row is written `cleared`, because the bank has settled it.

## Importing one PDF by hand

When the bank only offers a portal or app download (BCA, for example), keep the
file and import it directly:

```bash
abmx statement import ~/Downloads/bca-savings.pdf --parser bca.savings_statement
```

- `--parser` is optional when the file lives under `<dir>/<parser-id>/` or
  `<corpus>/<parser-id>/fixtures/`.
- The password comes from `[statements.passwords]`; `--password-file` overrides
  it for one run.
- There is no Gmail call, no label routing, and no state write. The ledger's
  `imported_id` makes a repeated import a no-op.
- `--dry-run` prints the plan; `--json` prints it as JSON.
- The file never leaves the machine. Decryption happens in memory, and the
  decrypted PDF is not written to disk.
- It respects `[statements].enabled`, like `statement sync`.

On Kubernetes this is a one-shot Job with the PDF mounted, or simply a command
run on the machine that already has the file. A statement volume on the cluster
only pays off when something else writes into it (a NAS share, Syncthing, a
cloud sync client); a single manual upload is the same effort as emailing the
PDF to yourself.

## What this release does not do

- It does not flip existing rows that came from email to `cleared`. That is the
  next step, and it needs the ledger's transaction ids plus an update path.
- It does not parse scanned PDFs, spreadsheets, or portal exports. A PDF without
  a text layer fails loudly.
- It does not import holdings or portfolio summaries (Pluang, Stockbit). Those
  are not ledgers; decline them in setup and they are recorded in `ignore`.

## Development

Never point a statement test at the budget you actually use: the import writes,
and a sync also reads history to decide what is new. Use the throwaway server:

```sh
just dev-up      # Actual on http://localhost:5006, disposable volume
just dev-reset   # delete the budget and start over
```

See `dev/README.md`. The test suite itself is offline; the opt-in statement
tests read real PDFs from a directory outside the repository.

## PII

Statement PDFs, dumps, fixtures, and `--json` output contain the account
holder's name, address, account number, and transactions. Keep them out of the
repository, out of issues, and out of logs. The password file is `0600` and is
never read into the config.
