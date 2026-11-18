# Setup

`abmx setup` is the attended quickstart. It walks through the pieces that must be
true before `abmx sync` can import anything:

1. a config file exists and has a discovery prefix and history window;
2. Gmail is authorized;
3. the labels under that prefix have an explicit yes/no decision;
4. every accepted label maps to a ledger account.

It is idempotent. Rerunning it skips completed steps, so it doubles as the
"add a new bank label" and "move to another machine" command.

```bash
abmx setup                 # the normal path
abmx setup --manual        # headless OAuth: approve anywhere, paste the URL back
abmx setup --review        # re-offer labels you previously declined
abmx setup --remap         # re-ask account mappings that already exist
```

Everything `setup` writes is also editable by hand; it uses a format-preserving
TOML editor, so comments and unknown keys survive.

## What each step does

### 1. Configuration

If no config resolves, it offers to write the starter template to
`$XDG_CONFIG_HOME/abmx/config.toml`. If one exists (including `./abmx.toml` or a
`--config` path), it uses that and moves on.

### 2. Discovery rule

Setup asks for the Gmail label prefix:

```
Discovery rule
labels starting with this prefix are discovered; the rest becomes the parser id

Gmail label prefix › receipt/
```

- New configs default to `receipt/`; existing configs show their current value.
  Enter keeps it.
- Matching is case-insensitive, so `Receipt/example.card_debit` is found by
  `receipt/`.
- The rest of the label must be a parser id: `receipt/example.card_debit` becomes
  `example.card_debit`. Nested labels such as `receipt/2026/example` are not parser ids.
- The value is saved to `[labels].prefix`.

### 3. History window

Setup asks how far back to fetch:

```
History window
› Keep current (2026-11-01)
  Last 7 days
  Last 30 days
  Last 90 days
  All history
  Custom date
```

- New configs default to the last 30 days. Existing configs default to keeping
  their current `since`.
- `All history` is available but warns: it scans every labelled message and is
  slow, and old templates may not match the parser.
- The date is saved to `[labels].since`. It bounds `abmx sync` and
  `abmx status --strict`. `abmx samples` ignores it and always takes the newest
  three messages per managed label.

### 4. Gmail

If a stored refresh token works, this step is skipped. Otherwise it prints the
Google Cloud checklist and asks for the path to the downloaded OAuth client JSON
(Desktop app). The JSON is copied to `<config dir>/gmail-client.json` with mode
`0600`, and `[fetch.gmail].credentials_file` is set.

The authorization itself is the same PKCE loopback flow as
`abmx auth gmail`, and respects `--manual` and `--no-browser`. The refresh token
is written to `~/.local/state/abmx/gmail-credentials.json`.

The client JSON and the token are deliberately separate, just like gws and
gmailctl: the client identifies the app, the token identifies the user.

### 5. Labels

Setup lists every Gmail label under `[labels].prefix`. For each label that is
neither mapped nor declined, it asks:

```
Import transactions labelled `receipt/example.card_debit`?
```

- **Yes** leaves the label to be mapped in the next step.
- **No** records the parser id in `[labels].ignore`, so the question is not
  asked again unless you pass `--review`.

Labels whose suffix is not a parser id (for example
`receipt/some/thing`) are reported and skipped; rename the gmailctl label.

If there are no routed labels at all, setup prints an example gmailctl rule and
stops this step. abmx never creates Gmail labels or filters.

Existing `[labels.accounts]` entries with no Gmail label under the new prefix are
reported as warnings, so a prefix change is visible before `sync` hard-fails on
them.

### 6. Ledger accounts

If the ledger is not configured, setup asks for the Actual server URL, the budget
sync id, and an Actual API key (if your Actual build offers Settings → Security → API keys). Leave the key blank to use the
server password instead. It then validates the connection by listing accounts.

For every accepted label it offers the open accounts and writes:

```toml
[labels.accounts]
"example.card_debit" = "<actual account uuid>"
```

A parser does not have to exist yet. The next step is `abmx samples` plus the
authoring skill below.

## Author a parser with AI

```bash
abmx samples
mkdir -p ~/.agents/skills/abmx && abmx skill > ~/.agents/skills/abmx/SKILL.md
```

Then prompt your agent:

```
help me create parser for all my transaction email, use abmx parser authoring skill
```

`abmx samples` writes up to three messages per managed label to
`$XDG_CONFIG_HOME/abmx/samples/<parser-id>/` and never requires a parser.
Fewer than three samples is a warning; the agent should stay conservative.

## Secrets

| Secret | Where setup puts it |
| --- | --- |
| Gmail OAuth client | `<config dir>/gmail-client.json` (0600) |
| Actual API key | `<config dir>/actual-api-key` (0600), unless `ABMX_ACTUAL_API_KEY` is set |
| Actual server password | `<config dir>/actual-server-password` (0600), when no API key is entered |
| Actual budget password | `<config dir>/actual-budget-password` (0600), only when you enter one |

Environment variables always win over files, which keeps systemd
`LoadCredential` and `EnvironmentFile` setups working:

```
ABMX_GMAIL_CLIENT_SECRET
ABMX_ACTUAL_API_KEY
ABMX_ACTUAL_SERVER_PASSWORD
ABMX_ACTUAL_BUDGET_PASSWORD
```

## What "managed" means

`[labels.accounts]` is the single source of truth. A label under the prefix with
no account entry is **unmanaged**: it is not fetched, not parsed, and not an
error. `sync` prints a one-line note when such labels exist.

`[labels].ignore` remembers an explicit no. Both lists are validated as parser
ids and may not overlap. A *configured* account with no matching Gmail label is
still an error — that is drift, not an absence of a decision.

## After setup

```bash
abmx samples                    # download evidence for the mapped labels
abmx sync --dry-run             # inspect the plan once parsers exist
abmx status                     # full health check (Gmail, labels, accounts, sink)
abmx status --offline           # skip network checks
abmx sync                       # import
```
