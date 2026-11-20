---
title: Authoring receipt parsers
description: From a labeled email to a verified TOML parser in four steps.
---

## Before you start

- gmailctl labels the mail under `[receipts].prefix` (default `receipt/`).
- The label is not in `[receipts].ignore`.
- `abmx status --offline` shows a `receipts` section. A label with no parser is
  the one to author.

## 1. Evidence

```bash
abmx receipt samples
# ~/.config/abmx/samples/<parser-id>/01.eml … 03.eml
```

`--label <label|parser-id>` narrows it, `--samples N` changes the count, and
`--out DIR` changes the directory. Sampling ignores `[receipts].since`, so a
quiet import window never starves the parser.

You can also use `abmx samples` to fetch receipt and statement evidence in one
run.

## 2. Skill

```bash
abmx skill > ~/.agents/skills/abmx/SKILL.md
```

## 3. Prompt

```text
help me create parser for all missing receipt and statement, use abmx parser authoring skill
```

## What the agent does

- `abmx dump <one.eml>` to see the block regions.
- Writes `~/.config/abmx/parsers/<id>/config.toml`.
- Copies the samples into `fixtures/`.
- Runs `abmx parser check --parser <id> -v` until every fixture is `strict`.

A receipt is done when every sample is `strict` and the amount is non-zero. A
`heuristic` row is not done.

## Wire it

```toml
[receipts.accounts]
"<parser-id>" = "<actual-account-uuid>"
```

Then `abmx receipt sync --dry-run` and `abmx receipt sync` (or `abmx sync` for
both sources). See [Parser spec](/reference/parser-spec/).

## When it fails

| Result | What it means | Fix |
| --- | --- | --- |
| `heuristic` | not every declared field matched | fix the label region or regex; use `abmx parser explain` |
| zero amount | the wrong block was matched | narrow the amount regex |
| `failed` | extraction errored | `abmx parser explain` names the field |
| `missing` | the parser id is not in the corpus | fix the directory name/id |
| `unmatched` | the label has no parser | create the parser or fix the label |

Do not teach one parser to guess between templates. If the samples under one
label are different layouts, fix the gmailctl rule or split the label.
