---
title: Statement spec
description: The Rhai format for a PDF statement parser, the line API, and the arithmetic that makes a parse strict.
---

A statement parser is a script in the same corpus as a mail parser:
`<corpus>/<id>/parser.rhai`, with `fixtures/` beside it. It defines `meta()` and
`fn parse_statement(doc)`. Statements are run by `abmx statement sync` (or the
combined `abmx sync`).

The script turns positioned lines into observations. The host proves the
arithmetic and builds transactions, so a script cannot declare a document
strict or invent a balance.

## Authoring loop

```sh
# 1. Evidence: positioned text, not the PDF
abmx statement dump statement.pdf --password-file password.txt

# 2. Write <corpus>/<id>/parser.rhai and put the PDFs in <corpus>/<id>/fixtures/

# 3. Verify every fixture, including the arithmetic
abmx statement check --parsers <corpus> --parser <id> --password-file password.txt -v
```

`dump` prints one line per text line: `y | [x] "text"`. Rows are the lines whose
date cell matches a date; the x values on those lines are the column ranges. The
dump contains the account holder's details, so keep it out of the repository.

## What strict means

Per section:

1. `opening + Σ(credits) − Σ(debits) == closing`
2. for every row in document order:
   `previous_balance + signed_amount == expected`; a printed balance must equal
   `expected`
3. `Σ parsed credits == the printed credit total` (when printed)
4. `Σ parsed debits == the printed debit total` (when printed)
5. every row date falls inside the printed period
6. the printed page count matches, when the script provides it

A row does **not** need a printed balance. Banks that print a balance only on the
last row of a date group are supported: the host walks the missing balances and
verifies every balance the bank prints. One mismatch rejects the document.

## Format

```rhai
fn meta() {
    #{
        bank: "bca",
        currency: "IDR",
        tz: "Asia/Jakarta",
        offset: "+07:00",     // fixed offset used for occurred_at
        number: "en",         // en: 1,234.56   id: 1.234,56
        exponent: 2,          // only when the bank prints sub-unit precision
        api: 1,
        version: 1,
    }
}

fn parse_statement(doc) {
    let rows = [];
    for line in doc.lines() {
        if !line.matches(`^\d{2}/\d{2}$`, 35.0, 88.0) { continue; }
        let description = line.words(88.0, 380.0);
        if description == "SALDO AWAL" { continue; }

        rows.push(#{
            date: line.cell(35.0, 88.0),
            description: description,
            amount: line.money(380.0, 435.0),
            out: line.cell(435.0, 460.0) == "DB",
            balance: line.money_any(500.0, 600.0),   // () when empty
        });
    }

    #{
        period: doc.month("PERIODE"),               // PERIODE : OKTOBER 2026
        account: doc.regex(`NO\. REKENING\s*:?\s*(\d{10,})`),
        sections: [#{
            key: "savings",
            opening: doc.money(`SALDO AWAL\s*:\s*([\d.,]+)`),
            closing: doc.money(`SALDO AKHIR\s*:\s*([\d.,]+)`),
            total_credit: doc.money(`MUTASI CR\s*:\s*([\d.,]+)`),
            total_debit: doc.money(`MUTASI DB\s*:\s*([\d.,]+)`),
            rows: rows,
        }],
    }
}
```

A numeric period is equally fine:

```rhai
let period = #{
    start: doc.date_text(doc.regex_group(`Period\s*:\s*([\d/]+)`, 1), ["%d/%m/%y"], "en"),
    end: doc.date_text(doc.regex_group(`s/d\s*([\d/]+)`, 1), ["%d/%m/%y"], "en"),
};
```

## doc

| Expression | Result |
| --- | --- |
| `doc.page_count` | pages read |
| `doc.text` | every line, newline separated |
| `doc.lines()` | array of `Line` |
| `doc.regex(pattern)` | first capture over the whole text |
| `doc.regex_group(pattern, n)` | capture group `n` (number or name) |
| `doc.has(pattern)` | boolean |
| `doc.money(pattern)` | capture parsed as scale-2 money |
| `doc.money_text(raw)` | arbitrary text as scale-2 money |
| `doc.int(pattern)` | capture parsed as an integer |
| `doc.date_text(raw, formats, locale)` | `YYYY-MM-DD` |
| `doc.month(label)` | `#{start, end}` for the printed month |

Money is **scale 2** regardless of currency: `278301099` is `2,783,010.99`.
`doc.month` understands Indonesian and English month names, full or abbreviated.

## Line

| Expression | Result |
| --- | --- |
| `line.text` | the whole line |
| `line.page`, `line.y` | provenance |
| `line.cell(x0, x1)` | runs in `[x0, x1)` concatenated and collapsed |
| `line.words(x0, x1)` | the same runs joined with spaces |
| `line.matches(pattern, x0, x1)` | regex over the cell |
| `line.contains(text, x0, x1)` | substring over the cell |
| `line.money(x0, x1)` | cell as scale-2 money |
| `line.money_any(x0, x1)` | money, or `()` when the cell is empty |

Use `cell` for dates, amounts, markers, and balances: it mirrors the printed
layout. Use `words` for descriptions, so adjacent runs do not fuse.

## Return value

```rhai
#{
    period: #{ start: "2026-10-01", end: "2026-10-31" },
    account: "0332221432",        // optional, identity only
    pages: 3,                     // optional, catches a truncated download
    sections: [ #{
        key: "savings",           // becomes Transaction.account
        opening: 278301099,
        closing: 560291264,
        total_credit: 500007765,  // optional
        total_debit: 218017600,   // optional
        date_format: "%d/%m",     // default
        rows: [ #{
            date: "02/10",
            description: "TGL: 02/10",
            amount: 7150000,
            out: true,
            balance: 271151099,   // omit or () when not printed
        } ],
    } ],
}
```

A consolidated document returns several sections, each with its own arithmetic.
Map `<parser-id>` for a single product or `<parser-id>:<section-key>` for several
in `[statements.accounts]`.

## Identity

- **Document**: `hash(bank, account, period_start, period_end)`. Re-fetching the
  same statement is a no-op.
- **Row**: content-based, including the computed running balance. Two identical
  same-day, same-amount rows are still distinct because their balances differ.

Ids are stable across parser versions and do not depend on the file name or the
Gmail message id.

## Sandbox

The script runs in a fresh scope per document, with no filesystem, network,
clock, environment, `eval`, or imports. Operations, call depth, allocation, and
wall time are capped.

## Passwords

The script never contains a password. Configure a reference in `abmx.toml`:

```toml
[statements.passwords]
"example.consolidated_statement" = { file = "/run/secrets/abmx/statement" }
# or: { env = "ABMX_STATEMENT_PASSWORD_EXAMPLE" }
```

An owner-restricted PDF with an empty user password needs no configuration.
Decryption happens in-process: the password is never passed on a command line
and the decrypted PDF is never written to disk. A wrong password is reported as
`wrong password`, a missing one as `password protected`, and a scanned PDF as
`no extractable text layer` — never as a script failure.
