# statement parser reference (Rhai, PDF)

A statement parser lives at `<corpus>/<id>/parser.rhai`, beside `fixtures/`. It
defines `fn meta()` and `fn parse_statement(doc)`. The script turns positioned
text into observations; the host proves the arithmetic.

## Authoring loop

1. Get PDFs for the same bank and template:

   ```sh
   abmx statement samples --label financial_statements/<id> --samples 3
   ```

   Never commit PDFs: they contain PII.
2. Dump the positioned text:

   ```sh
   abmx statement dump statement.pdf --password-file password.txt
   ```

   Each line is `y | [x] "text"`. Rows are the lines whose date cell matches a
   date; the x values on those lines give the column ranges.
3. Write `parser.rhai`, copy the PDFs into `fixtures/`.
4. Verify:

   ```sh
   abmx statement check --parsers <corpus> --parser <id> --password-file password.txt -v
   ```

   Done means every fixture is `strict`. There is no partial import.

## What strict means

The host proves the document per section:

- `opening + Σ(credits) − Σ(debits) == closing`;
- for every row in order: `previous + signed_amount == expected`; a printed
  balance must equal `expected`;
- `Σ credits == the printed credit total` (when printed);
- `Σ debits == the printed debit total` (when printed);
- every row date is inside the period;
- the printed page count matches (when provided).

A row does **not** need a printed balance. Banks that print a balance only on the
last row of a date group are supported: the host walks the missing balances and
verifies every balance the bank does print. One mismatch rejects the document.

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

## doc

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

Money returned by `doc.money` is **scale 2** (hundredths), independent of the
currency: `278301099` is `2,783,010.99`.

`doc.month(label)` understands Indonesian and English month names, full or
abbreviated, and returns the first and last day of that month.

## Line

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
layout exactly. Use `words` for descriptions, so adjacent runs do not fuse.

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

A multi-product statement returns several sections; map
`<parser-id>:<section-key>` in `[statements.accounts]`.

## Rules and pitfalls

- Columns are geometric. A line-wide regex will eventually match a date inside a
  description.
- Dates in rows usually omit the year; they are resolved against the period,
  including a period that crosses a year boundary.
- A row amount is the unsigned magnitude. `out` decides the sign.
- Descriptions can contain the date; read them from the description column, not
  from the whole line.
- Repeated headers and page footers have an empty date cell, so the date
  predicate ignores them. Verify a multi-page month before trusting a template.
- Do not weaken the arithmetic to make a fixture pass: fix the columns, the
  marker, or the skip rule.
- The password never goes in the script. It comes from
  `[statements.passwords].<id>` (`file` or `env`).
- `exponent` is only for accounts that genuinely keep fractional currency
  (BCA savings credits interest with cents). Otherwise leave it alone.

## PII

Dumps, fixtures, and `--json` output contain the account holder's details and
transactions. Keep them out of the repository and out of issues.
