# Authoring a statement parser

A statement parser turns the PDF attachment(s) of a statement email into many
verified transactions. It is a Rhai script at `<corpus>/<id>/parser.rhai`; the
directory name is the parser id. The corpus is normally
`~/.config/abmx/parsers/` (see `[general].parsers`). The script reports
observations; the host proves the arithmetic.

Routing is explicit: gmailctl applies `financial_statements/<id>` (or your
configured prefix) and `[statements.accounts]` maps that id to a ledger account.
One email can carry several PDFs, and one PDF can hold several product
sections.

## Loop

1. **Sample.** `abmx statement samples` downloads the PDFs attached to the
   newest messages per non-ignored label into
   `$XDG_CONFIG_HOME/abmx/statement-samples/<parser-id>/`. Narrow it with
   `--label <label|parser-id>`. Fewer than three is a warning.
2. **Dump the positioned text.** `abmx statement dump <one.pdf>` prints
   `y | [x] "text"` per run. Rows are the lines whose date cell matches a date;
   the x values on those lines give the column ranges.
3. **Confirm one template.** If a label's PDFs differ in layout, stop: the label
   is mislabeled. Never write a parser that guesses.
4. **Pick the id.** `financial_statements/bca-savings` becomes `bca-savings`.
5. **Scaffold.** `abmx parser init <id> --kind statement` writes `parser.rhai`
   and `fixtures/`.
6. **Write `meta()` and `parse_statement(doc)`.** Build one section per product
   and one row per transaction line.
7. **Verify.** Copy the PDFs to `fixtures/` and run
   `abmx statement check --parsers <corpus> --parser <id> --password-file <file> -v`
   until every fixture is `strict`.
8. **Clean up.** If it cannot pass, delete the script and report why.

## What `strict` means

The host proves each section:

- `opening + Σ(credits) − Σ(debits) == closing`;
- for every row in order, `previous + signed_amount == expected`, and every
  printed balance equals `expected`;
- `Σ credits` and `Σ debits` match the printed totals when they are printed;
- every row date is inside the period;
- the printed page count matches when provided.

A row does not need a printed balance. The host walks missing balances and
verifies every balance the bank does print. One mismatch rejects the document;
there is no partial import and no per-row heuristic.

## Skeleton

```rhai
fn meta() {
    #{
        bank: "bca",
        currency: "IDR",
        tz: "Asia/Jakarta",
        offset: "+07:00",
        number: "en",     // en: 1,234.56   id: 1.234,56
        exponent: 2,      // only when the bank prints sub-unit precision
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

## The document (`doc`)

| Expression | Result |
| --- | --- |
| `doc.page_count` | number of pages read |
| `doc.text` | all lines joined with newlines |
| `doc.lines()` | array of `Line` |
| `doc.regex(pattern)` | first capture over the whole text |
| `doc.regex_group(pattern, n)` | capture group `n` (number or name) |
| `doc.has(pattern)` | boolean |
| `doc.money(pattern)` | capture parsed as scale-2 money |
| `doc.money_text(raw)` | arbitrary text parsed as scale-2 money |
| `doc.int(pattern)` | capture parsed as an integer |
| `doc.date_text(raw, formats, locale)` | `YYYY-MM-DD` |
| `doc.month(label)` | `#{start, end}` covering the printed month |

Money from `doc.money` is **scale 2** (hundredths), independent of the currency:
`278301099` is `2,783,010.99`. `doc.month(label)` understands Indonesian and
English month names, full or abbreviated.

## A line (`Line`)

| Expression | Result |
| --- | --- |
| `line.text` | the whole line, runs concatenated |
| `line.page` / `line.y` | provenance |
| `line.cell(x0, x1)` | runs in `[x0, x1)` concatenated and collapsed |
| `line.words(x0, x1)` | the same runs joined with single spaces |
| `line.matches(pattern, x0, x1)` | regex over the cell |
| `line.contains(text, x0, x1)` | substring over the cell |
| `line.money(x0, x1)` | cell parsed as scale-2 money |
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
            amount: 7150000,      // unsigned magnitude; `out` decides the sign
            out: true,
            balance: 271151099,   // omit or () when not printed
        } ],
    } ],
}
```

A multi-product statement returns several sections. Map
`<parser-id>:<section-key>` in `[statements.accounts]`.

## Rules and pitfalls

- Columns are geometric. A line-wide regex eventually matches a date inside a
  description, so read the date from the date column.
- Row dates usually omit the year; the host resolves them against the period,
  including a period that crosses a year boundary.
- A row amount is the unsigned magnitude. `out` decides the sign.
- Repeated headers and page footers have an empty date cell; the date predicate
  ignores them. Verify a multi-page month before trusting a template.
- A marker column is not optional when the bank prints one; without it every row
  is a credit and the debit total fails.
- A row without a printed balance is normal: the host walks it.
- Do not weaken the arithmetic to make a fixture pass. Fix the columns, the
  marker, or the skip rule.
- `exponent` is only for accounts that genuinely keep fractional currency (BCA
  savings credits interest with cents). Otherwise leave it alone.
- A scanned PDF has no text layer. Report it; OCR is not supported.
- The password never goes in the script. It comes from
  `[statements.passwords].<id>` (`file` or `env`), or `--password-file`.

## Routing and secrets

```toml
[statements.accounts]
"<parser-id>" = "<actual-account-uuid>"

[statements.passwords]          # only for encrypted PDFs
"<parser-id>" = { file = "secrets/<id>.password" }
```

The UUID comes from `abmx accounts` or `abmx setup`; never guess it. The parser
never sees the password and never writes an account.

## PII

Dumps, fixtures, and `--json` output contain the account holder's details and
transactions. Keep them out of the repository and out of issues.

## Stop rules

- No text layer, no arithmetic anchors, or no period → no parser. Report why.
- Samples that are different templates → report the label as mislabeled.
- Never invent a ledger account, a UUID, or an account mapping.
- Ambiguous column → leave the row out and report it rather than guessing.
- Never weaken verification to pass a fixture.
