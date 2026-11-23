# abmx parser authoring

abmx ships no bank parsers. A parser is a Rhai script in the corpus,
`~/.config/abmx/parsers/<parser-id>/parser.rhai`, verified against fixtures.

Read exactly one reference for the task at hand:

| Task | Read |
| --- | --- |
| A transaction email (`.eml`) becomes one or more transactions | [`author-receipt-parser.md`](author-receipt-parser.md) |
| An email carrying PDF statement attachments becomes many verified rows | [`author-statement-parser.md`](author-statement-parser.md) |

If the user asks for both, do both, one parser at a time. Do not mix the two
contracts: receipts implement `parse_mail(ctx)`; statements implement
`parse_statement(doc)`.

## Evidence first

```sh
abmx samples                      # all enabled sources
abmx receipt samples [--label]    # receipt .eml files
abmx statement samples [--label]  # statement PDFs
```

Samples land in `$XDG_CONFIG_HOME/abmx/samples/receipts/<id>/` and
`$XDG_CONFIG_HOME/abmx/samples/statements/<id>/`. Fewer than three is a warning:
be conservative and never invent a template from one message.

## Non-negotiables

- The ledger account and UUID come from config (`abmx accounts`, `abmx setup`);
  never invent or edit them.
- One label maps to one parser id. If a label's samples are different templates,
  the label is mislabeled; fix gmailctl, do not guess in the parser.
- Verify against the parser's own fixtures. Done means every fixture is
  `strict`.
- Never leave a broken parser in the corpus; delete it and report why.
- Never weaken statement arithmetic to make a fixture pass.

## After wiring

Confirm the parser id is mapped in `[receipts.accounts]` or
`[statements.accounts]`, then run `abmx status` and the matching sync with
`--dry-run`.
