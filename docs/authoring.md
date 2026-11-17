# Authoring parsers

A parser is a `config.toml` per bank template. Routing is **label-driven**: the
user applies exactly one `abmx:<id>` Gmail label to each message they want
imported, and `abmx.toml` maps that label to a parser and a ledger account.
There is no sender/subject inference.

```toml
[labels]
"abmx:example-qris" = { parser = "example-qris", account = "<actual account uuid>" }
```

## The contract

- One label = one template = one parser = one ledger account.
- A message with two `abmx:*` labels aborts the run.
- A Gmail `abmx:*` label with mail but no `[labels]` entry aborts the run.
- A `[labels]` entry with no Gmail label is `orphan_config`.
- abmx stays `gmail.readonly`; it never creates or edits labels. The user labels
  mail manually, usually with a Gmail filter.

## The workflow

1. **Label the new mail.** Create `abmx:<id>` in Gmail and apply it (manually or
   with a filter). Recommended id: the parser directory name, e.g.
   `abmx:example-qris` → `example-qris`.
2. **Run `abmx author`.** It inventories every `abmx:*` label and takes the last
   three messages under each one:

   ```bash
   abmx author
   opencode --prompt "$(abmx author)"
   ```

3. **Read the status.** For each label:

   | status | meaning |
   | --- | --- |
   | `ok` | parser exists and all three samples are `strict` |
   | `missing_parser` | no parser directory |
   | `missing_mapping` | parser exists, no `[labels]` entry |
   | `parse_failed` | the parser rejects at least one sample |
   | `mixed_shapes` | the samples are different templates: the label is wrong |
   | `insufficient_samples` | fewer than three messages; do not trust a parser yet |
   | `empty` | the label has no mail |
   | `orphan_config` | the config names a label that Gmail does not have |

4. **Author one parser per label.** The prompt contains the block dumps and a
   variance table for the three samples. `labels.list` must contain every
   constant label block, in document order. The parser has no `[match]` section
   and no account field; it only extracts amount, date, status, and optional
   metadata.
5. **Ask for the ledger account.** Run `abmx accounts --json` and let the user
   choose. Add the `[labels]` entry. Never invent a UUID.
6. **Verify all three samples.**
   ```bash
   abmx verify parser ~/.config/abmx/parsers/example-qris
   abmx verify parser ~/.config/abmx/parsers/example-qris --label abmx:example-qris
   abmx parser lint --parser example-qris
   ```
7. **Sync.**
   ```bash
   abmx labels --check
   abmx sync --dry-run
   abmx sync
   ```

## Why three samples

One sample cannot distinguish a label from a value: a fixed merchant name or a
fee looks exactly like a label. Three samples are the minimum for the variance
table to show which blocks change. If the three samples are different templates,
the label is mislabeled; fix the Gmail label/filter. Do not split one label into
several parsers and do not relax extraction to make one sample pass.

## Debugging

- `abmx parser explain <one.eml> --parser <id>` prints every declared label and
  the block region the engine will use.
- `abmx parser lint --parser <id>` checks for labels missing from `labels.list`,
  listed labels that never appear, end-anchored amount regexes, and missing
  fixtures.
- `abmx dump <one.eml>` prints the normalized block list.
- `abmx verify parser --label` runs against live mail, so fixture drift is
  visible before a sync.

## Reusable skills

```bash
abmx generate skill parser --global   # ~/.config/opencode/skills
```

The authoring skill contains the spec reference, the workflow, and the pitfalls;
the install skill covers the `[labels]` mapping and the fail-closed checks.
