---
title: Setup
description: What abmx setup asks, what it writes, and when to rerun it.
---

`abmx setup` is the attended quickstart. It walks through the pieces that must be
true before `abmx sync` can import anything:

1. a config file exists and has a discovery prefix and history window;
2. the sources you want (receipts, statements, or both) are enabled;
3. Gmail is authorized;
4. the labels under each prefix have an explicit import decision;
5. every accepted label maps to a ledger account.

It is idempotent. Rerunning it skips completed steps, so it doubles as the
"add a new bank label" and "move to another machine" command.

```bash
abmx setup                 # the normal path
abmx setup --manual        # force headless OAuth: approve anywhere, paste the URL back
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

### 2. Sources

Setup asks which sources to configure:

```text
Which sources should abmx sync?
◇ Receipts     one .eml -> one transaction
◇ Statements   a PDF in an email -> transaction data
```

Both are pre-selected to match the current `[receipts].enabled` and
`[statements].enabled` values, and at least one must stay selected. Declining a
source pauses it in the config and skips all of its questions for this run.
Mappings are kept, so re-running setup with the source enabled resumes where you
left off.

### 3. Gmail

If a stored refresh token works, this step is skipped. Otherwise it prints the
Google Cloud checklist and asks for the path to the downloaded OAuth client JSON
(Desktop app). The JSON is copied to `<config dir>/gmail-client.json` with mode
`0600`, and `[fetch.gmail].credentials_file` is set.

The authorization itself is the same PKCE flow as `abmx auth login`. It tries
to open a browser; if none can be opened it switches to the manual paste flow
by itself. `--manual` forces that flow and `--no-browser` keeps the loopback
listener while printing the URL. The refresh token is written to
`~/.local/state/abmx/gmail-credentials.json`.

The client JSON and the token are deliberately separate, just like gws and
gmailctl: the client identifies the app, the token identifies the user. See
[Gmail](/guides/gmail/) for the Google Cloud side.

### 4. Receipts

When receipts are enabled, setup prints a one-line reminder that each `.eml`
under the prefix becomes one structured transaction, with links to
[configuration](/start/configuration/) and the
[parser spec](/reference/parser-spec/) for the parsing knobs. Then it asks for
the discovery rule:

```text
Discovery rule
labels starting with this prefix are discovered; the rest becomes the parser id

Gmail label prefix › receipt/
```

- New configs default to `receipt/`; existing configs show their current value.
  Enter keeps it.
- Matching is case-insensitive, so `Receipt/example.card_debit` is found by
  `receipt/`.
- The rest of the label must be a parser id: `receipt/example.card_debit` becomes
  `example.card_debit`. Nested labels such as `receipt/2026/example` are not
  parser ids.
- The value is saved to `[receipts].prefix`.

### 5. History window

Setup asks how far back to fetch:

```text
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
- The date is saved to `[receipts].since`. It bounds `abmx receipt sync` and
  `abmx status --strict`. `abmx samples` ignores it and always takes the newest
  three messages per non-ignored label.

### 6. Receipt labels

Setup lists every valid Gmail label under `[receipts].prefix` in one checklist:

```text
Import which receipt labels?
◆ receipt/example.card_debit   mapped
◆ receipt/example.qris         new
◇ receipt/example.old          previously declined
```

- **Mapped** labels start checked. Unchecking one removes its
  `[receipts.accounts]` entry and records the parser id in `[receipts].ignore`.
- **New** labels start checked. Leaving one checked sends it to the account
  mapping step; unchecking records it in `[receipts].ignore`.
- **Previously declined** labels appear (unchecked) only with `--review`.
  Checking one clears the ignore entry and sends it to account mapping.

Labels whose suffix is not a parser id (for example `receipt/some/thing`) are
reported and skipped; rename the gmailctl label.

If there are no routed labels at all, setup prints an example gmailctl rule and
stops this step. abmx never creates Gmail labels or filters.

Existing `[receipts.accounts]` entries with no Gmail label under the new prefix are
reported as warnings, so a prefix change is visible before `sync` hard-fails on
them.

### 7. Ledger accounts

If the ledger is not configured, setup asks for the Actual server URL, the
budget sync id, and an Actual API key (if your Actual build offers
Settings → Security → API keys). Leave the key blank to use the server password
instead. It then validates the connection by listing accounts.

For every accepted label it offers the open accounts and writes:

```toml
[receipts.accounts]
"example.card_debit" = "<actual account uuid>"
```

A parser does not have to exist yet. The next step is `abmx samples` plus
[parser authoring](/guides/authoring/).

### 8. Statements

The last step asks for the statement label prefix (`financial_statements/` by
default), then shows the same checklist for the labels under it. It starts with a
short reminder that a statement email's PDF becomes structured transaction data,
with links to [statement settings](/guides/statements/) and the
[statement spec](/reference/statement-spec/).

- Checked labels are mapped to a ledger account in the next prompt.
- Unchecking a mapped label removes all of its `[statements.accounts]` entries
  (including `parser:<section>` keys) and records it in `[statements].ignore`.
- Unchecking a new label records it in `[statements].ignore`.
- Previously declined labels appear unchecked with `--review`.

Encrypted PDFs are configured by hand through `[statements.passwords]`; setup
does not ask for the password. With `--remap`, checked mapped labels go straight
to the account picker with a **Keep the current mapping** option.

## Secrets

| Secret                  | Where setup puts it                                                     |
| ----------------------- | ----------------------------------------------------------------------- |
| Gmail OAuth client      | `<config dir>/gmail-client.json` (0600)                                 |
| Actual API key          | `<config dir>/actual-api-key` (0600), unless `ABMX_ACTUAL_API_KEY` is set |
| Actual server password  | `<config dir>/actual-server-password` (0600), when no API key is entered |
| Actual budget password  | `<config dir>/actual-budget-password` (0600), only when you enter one    |

Environment variables always win over files, which keeps systemd
`LoadCredential` and `EnvironmentFile` setups working:

```text
ABMX_GMAIL_CLIENT_SECRET
ABMX_ACTUAL_API_KEY
ABMX_ACTUAL_SERVER_PASSWORD
ABMX_ACTUAL_BUDGET_PASSWORD
```

## What "managed" means

`[receipts.accounts]` is the single source of truth. A label under the prefix with
no account entry is **unmanaged**: it is not fetched, not parsed, and not an
error. `sync` prints a one-line note when such labels exist.

`[receipts].ignore` remembers an explicit no. Both lists are validated as parser
ids and may not overlap. A *configured* account with no matching Gmail label is
still an error — that is drift, not an absence of a decision.

## After setup

```bash
abmx samples                            # download evidence for every labeled source
abmx receipt sync --dry-run     # inspect the plan once parsers exist
abmx status                     # full health check (Gmail, labels, accounts, sink)
abmx status --offline           # skip network checks
abmx receipt sync               # import
```
