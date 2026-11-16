# Authoring parsers with an AI

`abmx-parser` turns a bank email into a transaction; it does not know a new bank's
template until someone writes one. `abmx author` finds the templates that are
missing, packages the evidence, and hands an agent everything it needs to write
and verify the spec.

This is assistant tooling, not an LLM integration: `abmx` never calls a model.
It writes Markdown and `.eml` files; you decide which agent reads them.

## The loop

```text
mailbox ──► abmx author ──► group by sender ──► candidate prompt
                                                 │
                          ┌──────────────────────┘
                          ▼
                 agent writes config.toml + fixtures
                          │
                          ▼
              abmx parser check  /  abmx verify parser
```

1. `abmx author` scans a window of mail, groups by sender, and keeps the groups
   that contain an amount and a date.
2. Groups already routed by an installed parser are reported as `known` and do
   not reach the agent.
3. For each candidate, `abmx` writes one representative `.eml` under
   `~/.local/state/abmx/author/<timestamp>/` and emits a prompt with a block
   dump.
4. The agent confirms the sender, parser id, account key, and ledger mapping,
   then writes `~/.config/abmx/parsers/<id>/config.toml` with the sample in
   `fixtures/`.
5. `abmx parser check --parser <id>` must be green before the parser is trusted.

## Fast path: `abmx author`

```bash
abmx author                         # scan 30 days of Gmail
opencode run --auto "$(abmx author)"
```

Options:

| Flag | Effect |
| --- | --- |
| `--days N` | scan window, default 30 |
| `--label L` | restrict the scan to one Gmail label |
| `--senders a@b,c@d` | skip discovery for these senders |
| `--input DIR` | offline bootstrap from `.eml` files |
| `--max-messages N` | cap the metadata pass, default 1000 |
| `--max-groups N` | cap the prompt, default 20 |
| `--concurrency N` | parallel header requests, default 8, max 16 |
| `--out FILE` | write the prompt to a file instead of stdout |

The scan is read-only: it does **not** advance the sync cursor, write the state
database, or touch a ledger. Progress (`headers 320/1204`) goes to stderr, so
stdout stays pipeable; `-q` silences it.

A group is a candidate when a representative message contains both an
amount-like block and a date-like block. OTP, verification, and security subjects
are dropped. The agent still decides whether a candidate is a **bank movement**
or a **merchant receipt**: a Shopee or Tokopedia order is not a ledger movement
when the bank already notifies that account.

When the prompt is pre-answered (senders and account mappings), the agent can run
end to end without a question:

```bash
abmx author --senders example@example.invalid,noreply@banktwo.example \
  --out /tmp/author.md
opencode run --auto --file /tmp/author.md "Follow the prompt."
```

## Manual prompt

`abmx generate prompt parser` renders the same kind of prompt from a directory of
`.eml` files, without scanning Gmail:

```bash
abmx generate prompt parser \
  --input ./eml-dir \
  [--id bank.template] \
  [--corpus ~/.config/abmx/parsers] \
  [--out ./parser-prompt.md] \
  [--max-samples 8]
```

`--input` may be a directory or one `.eml` file; `--input -` reads one message
from stdin. Immediate subdirectories become groups (useful when an export already
keeps one folder per template). Without `--out` the prompt goes to stdout:

```bash
abmx generate prompt parser --input ./eml-dir | pbcopy
```

The prompt contains the task and stop rules, every sample's sender, subject, date,
message id, and block dump, a per-sender variance table, routing against the
current corpus, existing account keys, and the spec reference.

## Generate the skills

The prompt is self-contained, but interactive sessions benefit from the same
knowledge as reusable OpenCode skills:

```bash
abmx generate skill parser --out .opencode/skills
abmx generate skill parser --global      # ~/.config/opencode/skills
```

Two skills are written:

| Skill | Use |
| --- | --- |
| `abmx-parser-authoring` | From block dumps to a spec, including routing, labels, and the verification loop |
| `abmx-parser-install` | Account-key mapping, config edits, and end-to-end validation |

They share `references/spec-reference.md`, so the prompt and the skills cannot
drift apart without a test noticing. Files are only overwritten with `--force`,
so local edits survive a regeneration.

Once installed, the recurring prompt is just "author the parsers for this mail";
the skill knows the corpus layout and the verification loop.

## Verify a draft

```bash
abmx verify parser ~/.config/abmx/parsers/my.bank
abmx verify parser ~/.config/abmx/parsers/my.bank --json
```

`verify` accepts a parser directory (defaults to its `fixtures/`) or a
`config.toml` plus `--id`. The candidate is copied into a temporary corpus, so
the live corpus is never modified and a same-id sibling cannot shadow it. Output
is one row per sample:

| Status | Meaning |
| --- | --- |
| `strict` | every declared field matched and the amount is non-zero |
| `heuristic` | parsed with warnings, almost always a zero amount or a wrong region |
| `unmatched` | the candidate did not claim the email |
| `failed` | the candidate claimed it but extraction errored |

Exit codes follow the parser: `0` all strict, `3` at least one heuristic, `2`
unmatched or failed, `1` the tool failed.

## Inspect blocks

`abmx dump` shows the normalized view:

```bash
abmx dump ./eml-dir/01.eml
abmx dump ./eml-dir --json
```

This is the command to use when a verification row looks wrong: the block dump
usually makes the missing label or the over-broad regex obvious.

## Gmail labels as hints

A label is a useful way to narrow a scan without a full mailbox pass:

```bash
abmx author --label receipt
```

- Messages are fetched with `label:"<name>"` plus `-in:spam -in:trash` and the
  scan window.
- `abmx generate prompt parser --from-gmail-label` writes samples to
  `<state>/abmx/samples/<timestamp>/<label-slug>/` with `0700` directories and
  `0600` files.
- `--id-from label` slugifies the label into `bank.type`; `--id-from infer` asks
  the agent to propose an id and confirm it with you.
- A label is a hint, not proof. If block shapes differ, the prompt asks the agent
  to split the template rather than forcing one spec.

Sampling needs `[fetch.gmail]` and a stored token (`abmx auth gmail`), but it
never advances the sync cursor and never reads the state database.

## Account mapping

Two different names for the same account:

```toml
# ~/.config/abmx/parsers/my.bank/config.toml
[defaults]
account = "example-account"      # parser-side key

# ~/.config/abmx/config.toml
[accounts]
"example-account" = "<actual account uuid>"
```

The agent is instructed to run `abmx accounts --json`, ask which ledger account
the key belongs to, and only then add the mapping. It must never put a UUID in
the parser file. An unmapped key is not corruption: rows go to review, and
mapping the key imports them on the next sync.

## Privacy

The prompt contains real amounts, counterparties, and account numbers, and the
candidate `.eml` files are written unredacted under
`~/.local/state/abmx/author/<timestamp>/`. They stay on your machine; send them
to a local model if you want to keep it that way. Delete the work directory when
you are done.

Fixtures only need redaction when you contribute the parser to this repository
(`just redact`); a personal parser's fixture may keep the real sample.

## Scope

`abmx` generates text, scans read-only, and verifies specs. It never calls a
model and never writes a parser spec itself. Writing `config.toml`, choosing the
account mapping, and installing a contributed parser stay with the agent and with
you.
