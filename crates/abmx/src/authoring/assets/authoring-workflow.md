# Authoring a bank parser from samples

## What you are building

`.eml` bytes in, one or more normalized transactions out. A parser is a Rhai
script, `<corpus>/<id>/parser.rhai`, loaded at runtime; the directory name is the
id. The host provides extraction helpers, identity, the email's offset, and the
`strict` decision. The script decides what to read.

Routing is explicit: gmailctl applies `receipt/<id>` and the main config maps that
id to a ledger account. The parser never inspects the sender or subject.

## Workflow

1. **Run `abmx samples`.** It downloads up to three messages per non-ignored
   label into `$XDG_CONFIG_HOME/abmx/samples/<parser-id>/`. `manifest.json`
   records each label's count. Fewer than three is a warning; do not invent a
   template from one sample.
2. **Read every sample's block dump.** Use `abmx dump <dir>` or
   `abmx parser explain <one.eml> --parser <id>`. Blocks are the only thing the
   parser sees; HTML tags and CSS are irrelevant.
3. **Confirm one template per label.** If the samples for a label differ in
   shape, stop: the label is mislabeled. Do not write a parser that guesses.
4. **Choose the parser id.** It must match the label suffix:
   `receipt/example-qris` → `example-qris`.
5. **Scaffold.** `abmx parser init example-qris` writes `parser.rhai` and
   `fixtures/`. Use `--kind statement` for a PDF parser.
6. **Write `meta()`.** `bank`, `currency`, `kind`, and `labels`. `labels` must
   contain every constant label block, in document order, including labels you
   do not extract: the region boundaries depend on them.
7. **Write `parse_mail(ctx)`.** Start with `amount` and `occurred_at`; add
   optional fields only where the blocks prove them. Put secondary values in
   `extra`. Prefer `money_regex_in` when the region contains other numbers, and
   `strip_spaces` for references printed in groups.
8. **Status.** If the template prints a status, map the exact words. There is no
   subject-based status.
9. **Verify.** Copy the samples to `fixtures/01.eml`, `02.eml`, … and run
   `abmx parser check --parser <parser-id> --parsers <corpus>`. Repeat until
   every sample is `strict`.
10. **Leave the corpus clean.** If it cannot pass, delete the script you wrote
    and report why. Never leave a broken parser behind.

## Statements

The same loop, with `abmx statement dump` instead of `abmx dump`, and
`abmx statement check` instead of `abmx parser check`. A statement parser
implements `parse_statement(doc)` and must satisfy the arithmetic; see the
statement reference.

## Stop rules

- No monetary amount in the samples → no parser. Report why.
- Samples that are different templates → report the label as mislabeled.
- Never invent a ledger account, a UUID, or an account mapping.
- A value that is ambiguous → leave the optional field out rather than guessing.
- Do not write into another parser's directory.

## Writing into abmx

1. Write `<corpus>/<parser-id>/parser.rhai` and copy the samples to `fixtures/`.
2. Confirm `[receipts.accounts]` already maps this parser id to the ledger
   account. Do not change it unless the user asks.
3. Run `abmx parser check --parser <parser-id> --parsers <corpus>`.
4. Run `abmx status` and `abmx receipt sync --dry-run`.
