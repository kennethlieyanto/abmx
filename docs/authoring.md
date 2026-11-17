# Authoring parsers with an AI

Two tools, two jobs:

- **gmailctl** decides what is a transaction and which template it is. It owns
  sender/subject/body matching, labels merchant receipts separately, and drops
  OTP and security mail.
- **abmx** parses the mail gmailctl routed to it and writes the ledger.

`abmx` never classifies mail and never matches senders. A label is the whole
contract.

## The labels

```
receipt                      merchant receipt (Grab, Pluxee, Google Play, ...)
receipt/example.card_debit  a ledger movement; the suffix is the parser id
receipt/example.journal_va  another parser, another template
Financial Statements         statements, not imported (yet)
Notifications                OTP, security alerts
```

`[labels].prefix` selects the routed namespace; the remainder must be a single
parser id (dots are conventional, not required: `example-qris` and
`example.card_debit` are both valid).

## The config

```toml
[labels]
prefix = "receipt/"
since  = 2026-11-10          # ignore mail received before this date

[labels.accounts]            # parser id -> Actual account uuid
"example.card_debit"       = "00000000-0000-0000-0000-000000000000"
"example.journal_transfer" = "00000000-0000-0000-0000-000000000000"
```

There is no `[policy]`: a labeled row is either imported or the label is broken.

## The loop

```bash
abmx status              # config, corpus, labels, accounts, auth (offline)
abmx status --online     # + Gmail label inventory + sink accounts
abmx status --strict     # + verify the newest three messages per label
abmx author              # prompt for the labels that need work
```

`abmx author`:

- inventories every label under the prefix;
- samples the newest three messages per label, honouring `since`;
- writes one representative `.eml` per label under
  `~/.local/state/abmx/author/<timestamp>/`;
- emits a prompt with block dumps and a variance table.

```bash
opencode run --auto "$(abmx author)"
```

Options: `--label <label>` restricts it to one route, `--samples N` changes the
evidence depth (default 3), `--input <dir>` works offline, `--json` emits the
bundle, and `--out FILE` writes the prompt instead of stdout.

## Statuses

| Status | Meaning | Fix |
| --- | --- | --- |
| `ok` | parser exists, account mapped, newest samples strict | — |
| `missing_parser` | no parser directory for the id | author it |
| `missing_account` | no `[labels.accounts]` entry, so the label is unmanaged | run `abmx setup` or add the entry |
| `parse_failed` | at least one sample is not strict, non-zero, or is `failed` | update the parser (or the gmailctl rule for `failed`) |
| `mixed_shapes` | the samples are different templates | fix the gmailctl rule; do **not** write a parser |
| `insufficient_samples` | fewer than three messages | wait for more mail or lower `--samples` |
| `empty` | the label has no mail | — |
| `ignored` | declined in `abmx setup` | `abmx setup --review` to reconsider |
| `bad_label` | the suffix is not a parser id | fix the gmailctl rule |
| `orphan_account` | an account entry has no routed label | add a gmailctl rule or remove the entry |

## The verification loop

`abmx parser check --parser <id>` verifies a parser against its colocated
fixtures. A spec is done when every sample is `strict`, non-zero, and not
`failed`.

`abmx status --strict` samples the live newest three under every managed label
and reports the same statuses, so a template change is visible without reading
logs.

## Reusable skill

```bash
abmx skill > /tmp/abmx-parser-authoring.md
```

`abmx skill` prints the authoring skill to stdout. Save it under your agent's
skills directory once; after that the prompt is simply "author the parsers for
these emails".

## When the sender or the template changes

- **Sender changes** are gmailctl's job. If the rule keys on the old address,
  mail stops being labeled and `abmx` never sees it. Fix the rule; `abmx` needs
  no change.
- **Template changes** keep the same label, so the parser fails and the label
  reports `parse_failed`. `abmx author --label …` re-samples the newest three and
  produces the update prompt.
- **Two layouts under one label** is `mixed_shapes`. The fix is a gmailctl rule
  that distinguishes them (a subject or a `has:` body fragment) and a second
  parser id — not a parser that guesses.
- A **failed transaction** should never reach `abmx`: the gmailctl rule must
  assert success (`has: 'BERHASIL'` / `not: { has: 'GAGAL' }`). If one slips
  through, the label is broken with `bank_declared_failed`.

## Privacy

The prompt contains real amounts, counterparties, and account numbers, and the
sample `.eml` files are written unredacted under
`~/.local/state/abmx/author/<timestamp>/`. Delete the work directory when you are
done.

Fixtures only need redaction when you contribute a parser to this repository
(`just redact`); a personal parser's fixture may keep the real sample.
