---
title: Authoring statement parsers
description: From a labeled statement PDF to a verified TOML parser in four steps.
---

## Before you start

- gmailctl labels the statement under `[statements].prefix` (default
  `financial_statements/`), or you have the PDF locally for `statement import`.
- The label is not in `[statements].ignore`.
- If the PDF is encrypted, keep the password in a file. `abmx statement dump`
  reports `locked_pdf` when one is needed.
- Statement PDFs and dumps are PII. Keep them out of git.

## 1. Evidence

```bash
abmx statement samples
# ~/.config/abmx/statement-samples/<parser-id>/01.pdf … 03.pdf
```

`--label <label|parser-id>` works for a label that is not mapped yet;
`--samples N` changes the count. You can also use `abmx samples` to fetch receipt
and statement evidence in one run.

For a PDF that never arrives by email, import it after the parser exists:

```bash
abmx statement import <file.pdf> --parser <id>
```

## 2. Skill

```bash
abmx skill > ~/.agents/skills/abmx/SKILL.md
```

## 3. Prompt

```text
help me create parser for all missing receipt and statement, use abmx parser authoring skill
```

## What the agent does

- `abmx statement dump <one.pdf>` to get the positioned text.
- Writes `~/.config/abmx/parsers/<id>/config.toml` with `input = "pdf"`.
- Copies the PDFs into `fixtures/`.
- Runs `abmx statement check --parser <id> -v` until every fixture is strict.

Strict means the document proves itself: opening + credits − debits = closing,
every row's running balance follows, and the printed totals match. There is no
per-row heuristic; one mismatch rejects the document.

## Wire it

```toml
[statements.accounts]
"<parser-id>" = "<actual-account-uuid>"

[statements.passwords]          # only when encrypted
"<parser-id>" = { file = "secrets/<id>.password" }
```

Then `abmx statement sync --dry-run` and `abmx statement sync`, or
`abmx statement import <file> --parser <id>` for a local PDF. See
[Statement spec](/reference/statement-spec/).

## When it fails

| Result | What it means | Fix |
| --- | --- | --- |
| `locked_pdf` | password is missing or wrong | set `[statements.passwords]` |
| `no_text_layer` | the PDF is scanned | stop; use a text PDF or CSV — OCR is not supported |
| the balance does not follow | wrong column, sign, or a missed row | fix the x-ranges, direction, or skip rules |
| totals mismatch | a row was missed or misclassified | check `skip` and the amount column |
| date outside the period | a footer or header was parsed as a row | add a `skip` rule |

Never weaken the arithmetic to make a fixture pass.
