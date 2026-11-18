# Authoring a bank parser from samples

## What you are building

`.eml` bytes in, one normalized transaction out. A parser is a data file, not
code; the extraction engine is fixed. The hard part is deciding what the
template's constant labels are and where each value lives between them.

Routing is explicit: a Gmail label `receipt/<id>` is mapped in `abmx.toml` to a
parser and a ledger account. The parser never inspects the sender or subject.

## Workflow

1. **Run `abmx samples`.** It downloads up to three messages per managed label
   into `$XDG_CONFIG_HOME/abmx/samples/<parser-id>/` (usually
   `~/.config/abmx/samples/<parser-id>/`). `manifest.json` records each label's
   count. Fewer than three is a warning; be conservative and do not invent a
   template.
2. **Read every sample's block dump.** Use
   `abmx dump <samples-dir>` or `abmx parser explain <one.eml> --parser <id>`.
   Blocks are the only thing the parser sees. HTML tags and CSS classes are
   irrelevant.
3. **Confirm one template per label.** The available samples for a label must
   share a label skeleton. If they do not, stop: the user or a Gmail filter has
   mislabeled mail. Do not split the label into several parsers.
4. **Choose the parser id.** It must match the label suffix: `receipt/example-qris`
   → `example-qris`. The directory name is the id.
5. **Write `[labels].list`.** Include every constant label block in document
   order, including labels you do not extract. A missing label merges two
   regions and makes the next field wrong; an extra label splits a value.
6. **Write `[defaults]`.** `bank` is a short bank name. `kind` is the
   direction/type of this template. `tz` is the account's IANA zone. `currency`
   is ISO-4217. There is no `account` field.
7. **Write `[extract.*]`.**
   - `amount`: match the value shape, not the end of the region.
   - `occurred_at`: use the body date, not the email `Date:` header. Use
     `locale = "id"` for Indonesian month abbreviations.
   - Add optional fields only when the blocks prove them.
   - Put secondary fields (location, terminal, acquirer) in `[extract.extra]`.
8. **Write `[status]`** when the template prints a status block. Map the exact
   words printed by the bank. There is no subject-based status.
9. **Write into the corpus and verify.** Put the spec at
   `<corpus>/<parser-id>/config.toml`, copy the samples into
   `<corpus>/<parser-id>/fixtures/01.eml`, `02.eml`, … then run
   `abmx parser check --parser <parser-id> --parsers <corpus>`. Repeat until
   every sample is `strict`.
10. **Leave the corpus clean.** If you cannot make it pass, delete the
    `config.toml` you wrote and report why. Never leave a broken spec behind.

## Stop rules

- If the samples contain no monetary amount, do not create a parser. Report why.
- If the samples are different templates, do not author; report the label as
  mislabeled.
- Never invent a ledger account, a UUID, or an account mapping. The account is
  already chosen by the user in `[labels.accounts]`.
- If a value is ambiguous, leave the optional field out rather than guessing.
- Do not write into another parser's directory.

## Writing into abmx

1. Write `<corpus>/<parser-id>/config.toml` and copy the samples to
   `<corpus>/<parser-id>/fixtures/`.
2. Confirm the `[labels.accounts]` entry already maps this parser id to the
   ledger account. Do not change it unless the user asks.
3. Run `abmx parser check --parser <parser-id> --parsers <corpus>`.
4. Run `abmx status --online` and `abmx sync --dry-run`.
