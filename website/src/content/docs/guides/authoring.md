---
title: Authoring parsers
description: The four-step loop for teaching abmx a new receipt or statement template.
---

`abmx` ships no bank parsers. A parser is a Rhai script in your corpus,
`~/.config/abmx/parsers/<id>/parser.rhai`, and the loop is the same for receipts
and statements.

## The four steps

1. **Make sure the mail is discovered.** gmailctl must apply the label under
   `[receipts].prefix` or `[statements].prefix`, and the label must not be in the
   matching `ignore` list. `abmx status` shows the sections.
2. **Download evidence.**
   ```bash
   abmx samples
   ```
   Receipt samples go to `~/.config/abmx/samples/<id>/`; statement PDFs to
   `~/.config/abmx/statement-samples/<id>/`. The command samples every
   non-ignored label under both prefixes, mapped or not. Narrow it with
   `abmx receipt samples --label …` or `abmx statement samples --label …`.
3. **Install the skills.**
   ```bash
   abmx skill install
   ```
4. **Ask the agent.**
   ```text
   help me create receipt parsers for all my transaction emails, use the abmx parser authoring skill
   help me create statement parsers for all my statement emails, use the abmx parser authoring skill
   ```

The agent reads the samples, writes the script, and verifies it against
fixtures. It should not touch the ledger or the account mappings; that stays
yours.

## Pick the kind

- [Receipt parsers](/guides/authoring-receipts/) — one email, one or more
  transactions.
- [Statement parsers](/guides/authoring-statements/) — one PDF, many rows, with
  arithmetic verification.

The formats are in [Parser spec](/reference/parser-spec/) and
[Statement spec](/reference/statement-spec/).

## Why scripts instead of a schema

A declarative parser is pleasant until a template needs state: a period printed
as a month name, a running balance printed only on the last row of a date group,
several transactions in one email. Adding a field per case turns the schema into
a language without functions or tests. A small sandboxed language is the honest
version of that.

The safety that made the old schema trustworthy is not in the data format; it is
in the contract. Scripts return observations. The host assigns identity and
strictness, and the statement verifier owns the arithmetic. A parser cannot
assert that it is correct.
