---
title: Authoring parsers
description: The four-step loop for teaching abmx a new receipt or statement template.
---

`abmx` ships no bank parsers. A parser is a TOML file in your corpus,
`~/.config/abmx/parsers/<id>/config.toml`, and the loop is the same for receipts
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
3. **Install the skill.**
   ```bash
   abmx skill > ~/.agents/skills/abmx/SKILL.md
   ```
4. **Ask the agent.**
   ```text
   help me create parser for all missing receipt and statement, use abmx parser authoring skill
   ```

The agent reads the samples, writes the TOML, and verifies it against fixtures.
It should not touch the ledger or the account mappings; that stays yours.

## Pick the kind

- [Receipt parsers](/guides/authoring-receipts/) — one email, one transaction.
- [Statement parsers](/guides/authoring-statements/) — one PDF, many rows, with
  arithmetic verification.

The formats are in [Parser spec](/reference/parser-spec/) and
[Statement spec](/reference/statement-spec/).
