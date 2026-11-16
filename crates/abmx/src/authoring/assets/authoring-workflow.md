# Authoring a bank parser from samples

## What you are building

`.eml` bytes in, one normalized transaction out. A parser is a data file, not
code; the extraction engine is fixed. The hard part is deciding what the
template's constant labels are and where each value lives between them.

## Workflow

1. **Read every sample's block dump.** Blocks are the only thing the parser
   sees. HTML tags and CSS classes are irrelevant.
2. **Decide how many templates are present.** A template is a distinct block
   shape and routing identity. Samples that share a sender and subject may still
   be several templates; samples that differ only in values are one template.
3. **For each template, choose the parser id.** Convention is `bank.template`,
   lower-case, stable forever. If the user explicitly supplied an id, use it.
4. **Write `[match]`.** Copy the exact sender address (or a narrow `*@domain`
   pattern). Anchor the subject with `^...$`. Add `requires` / `rejects` only
   when siblings would otherwise be ambiguous.
5. **Write `[labels].list`.** Include every constant label block in document
   order, including labels you do not extract. A missing label merges two
   regions and makes the next field wrong; an extra label splits a value.
6. **Write `[defaults]`.** `bank` is a short bank name. `account` is a stable
   parser-side key. `kind` is the direction/type of this template. `tz` is the
   account's IANA zone. `currency` is ISO-4217.
7. **Write `[extract.*]`.**
   - `amount`: match the value shape, not the end of the region.
   - `occurred_at`: use the body date, not the email `Date:` header. Use
     `locale = "id"` for Indonesian month abbreviations.
   - Add optional fields only when the blocks prove them.
   - Put secondary fields (location, terminal, acquirer) in `[extract.extra]`.
8. **Write `[status]`** when the template can report a failure. Map the exact
   words printed by the bank. Omit it for settled-only templates.
9. **Verify.** Run `abmx verify parser <draft> --input <samples>` and read
   every row. Repeat until all samples are `strict`.
10. **Install only after the user agrees.** Draft specs do not belong in the
    live corpus until they have been reviewed.

## Reading verification output

- `strict` — every declared field matched and the amount is non-zero.
- `heuristic` — the row parsed but carries warnings, usually a zero amount.
  This is almost always the wrong region, not a real zero-value transaction.
- `unmatched` — no parser claimed the email in the candidate corpus. For a
  single-spec verification that means `[match]` is wrong or too narrow.
- `failed` — the spec claimed the email but extraction errored. The message
  names the field that could not be extracted.

Exit codes: `0` all strict, `3` at least one heuristic, `2` unmatched or failed,
`1` the tool itself failed.

## Stop rules

- If the samples contain no monetary amount, do not create a parser. Report why.
- If a sample already routes to an existing parser and extracts correctly, do not
  create a duplicate. Report the existing parser id instead.
- If a value is ambiguous, leave the optional field out rather than guessing.
- Never invent a ledger account, a UUID, or a parser id the user has not agreed
  to when there is any doubt.

## Installing into abmx

A parser only becomes useful when abmx can load it and route real mail to it:

1. Copy the verified parser directory `parsers/<id>/` (both `config.toml` and
   `fixtures/`) into a directory listed by `[general].parsers`, or into the
   implicit corpus at `~/.config/abmx/parsers/`. The directory name is the
   parser id; the spec has no `id` field.
2. Make sure `[defaults].account` matches a key in `[accounts]`. The value in
   `[accounts]` is the sink's account id, not a guess.
3. Run `abmx parsers` to confirm the spec loaded and its sender is advertised.
4. Run `abmx doctor` and `abmx sync --dry-run --from-dir <samples>` to prove
   the end-to-end path.
5. Ask the user before changing `[accounts]`, a live corpus, or a sink.
