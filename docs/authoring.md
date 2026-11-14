# Authoring parsers with an AI

`abmx-parser` turns a bank email into a transaction; it does not know a new bank's
template until someone writes one. `abmx` can put everything an agent needs
into a single prompt, then verify the result against the real samples.

This is assistant tooling, not an LLM integration: `abmx` never calls a model.
It writes Markdown and TOML, and you decide where those files go.

## The loop

```text
samples ──► generate prompt ──► agent writes a draft ──► verify ──► install
             ▲                                             │
             └────────── abmx dump / more samples ◄──────┘
```

1. Collect a handful of `.eml` files that share a structure.
2. Generate the prompt.
3. Let the agent author a spec in the draft directory named in the prompt.
4. Run the verification command the prompt gives you until every sample is
   `strict`.
5. Approve installation into the parser corpus and the account mapping.

## Generate a prompt

```bash
abmx generate prompt parser \
  --input ./eml-dir \
  [--id bank.template] \
  [--corpus ~/.config/abmx/parsers] \
  [--out ./parser-prompt.md] \
  [--max-samples 8]
```

`--input` may be a directory or one `.eml` file; `--input -` reads one message
from stdin. Immediate subdirectories become groups (useful when an export
already keeps one folder per template).

`--out` writes a file, or a directory containing `parser-prompt.md`. Without it
the prompt goes to stdout, so this also works:

```bash
abmx generate prompt parser --input ./eml-dir | pbcopy
```

The prompt contains:

- the task, stop rules, and the draft-first installation rule;
- every sample's sender, subject, date, message id, and indexed block dump;
- a per-sender variance table: which block indices are constant labels, which
  vary, and which look like prose;
- routing against your current corpus, so a duplicate parser is obvious;
- existing account keys (sink UUIDs are omitted; the agent can run
  `abmx accounts --json` locally);
- the full parser spec reference and a list of common extraction mistakes.

If the samples do not share a block shape, the prompt says so and asks whether
to split the template rather than pretending one spec covers everything.

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

## Verify a draft

```bash
abmx verify parser ./drafts/my.bank.toml --input ./eml-dir
abmx verify parser ./drafts/my.bank.toml --input ./eml-dir --json
```

The candidate is copied into a temporary corpus; the live corpus is never
modified, and a same-id sibling cannot shadow it. Output is one row per sample:

| Status | Meaning |
| --- | --- |
| `strict` | every declared field matched and the amount is non-zero |
| `heuristic` | parsed with warnings, almost always a zero amount or a wrong region |
| `unmatched` | the candidate did not claim the email |
| `failed` | the candidate claimed it but extraction errored |

Exit codes follow the parser: `0` all strict, `3` at least one heuristic, `2` unmatched
or failed, `1` the tool failed.

## Inspect blocks

`abmx dump` shows the normalized view:

```bash
abmx dump ./eml-dir/01.eml
abmx dump ./eml-dir --json
```

This is the command to use when a verification row looks wrong: the block dump
usually makes the missing label or the over-broad regex obvious.

## Gmail labels as hints

A Gmail label is a convenient way to say "these emails share a structure":

```bash
abmx generate prompt parser \
  --from-gmail-label 'example/QRIS,example/Transfer' \
  --id-from label \
  --out ./parser-prompt.md
```

- Messages are fetched with `label:"<name>" -in:spam -in:trash`.
- Samples are written to `<state>/abmx/samples/<timestamp>/<label-slug>/`
  with `0700` directories and `0600` files.
- `--id-from label` slugifies the label into `bank.type`; `--id-from infer`
  asks the agent to propose an id and confirm it with you.
- A message with several of the requested labels is assigned to the first one.
- A label is a hint, not proof. If block shapes differ, the prompt asks the
  agent to split the template or flag the label rather than forcing one spec.

Sampling needs `[fetch.gmail]` and a stored token (`abmx auth gmail`), but it
never advances the sync cursor and never reads the state database.

## Account mapping

Two different names for the same account:

```toml
# parsers/my.bank.toml
[defaults]
account = "example-account"      # parser-side key

# abmx.toml
[accounts]
"example-account" = "<actual account uuid>"
```

The agent is instructed to run `abmx accounts --json`, ask which ledger
account the key belongs to, and only then add the mapping. It must never put a
UUID in the parser file. An unmapped key is not corruption: rows go to review,
and mapping the key imports them on the next sync.

## Privacy

The prompt contains real amounts, counterparties, and account numbers. Prefer a
local model, or redact the samples before sending them anywhere. Gmail sampling
writes raw mail to disk under the state directory; delete the sample directory
when you are done.

## Scope

`abmx` generates text and verifies specs. It does not install a parser into a
live corpus on its own, does not edit `abmx.toml`, and does not call a model.
Those steps stay with the agent and with you.
