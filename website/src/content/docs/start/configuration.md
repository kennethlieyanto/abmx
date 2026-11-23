---
title: Configuration
description: The abmx.toml reference, resolution order, label mapping, and secret precedence.
---

```toml
[general]
parsers = []                       # empty = implicit discovery
state = "~/.local/state/abmx/state.db"

[fetch.gmail]
client_id = "….apps.googleusercontent.com"
max_messages = 10000               # hard cap; exceeding it is an error

# Actual has no HTTP API. abmx spawns a bundled helper that drives the official
# Node client (@actual-app/api) against your actual server. See the Actual guide.
[sink.actual]
server_url = "http://localhost:5006"   # your Actual server, not a wrapper
budget_sync_id = "…"
api_key_file = "/run/credentials/abmx.service/actual-api-key"
# Servers without API keys: use `server_password` / `server_password_file`.
# data_dir = "~/.cache/abmx/actual"    # persistent copy of the budget
chunk_size = 50
# never (default) | settled | always — see the Actual guide
cleared = "never"
# notes_fields = ["description"]  # default; [] disables notes entirely

[receipts]
enabled = true               # false pauses receipts, keeping the mappings
prefix = "receipt/"          # literal, case-insensitive
since  = 2026-11-01          # history window; omit to scan everything

[receipts.accounts]                 # parser id -> Actual account uuid
"example.card_debit"       = "<actual account uuid>"
"example.journal_transfer" = "<actual account uuid>"
```

## Resolution order

`--config` → `$ABMX_CONFIG` → `$XDG_CONFIG_HOME/abmx/config.toml` →
`./abmx.toml`. Relative paths resolve against the config file's directory.

`abmx` never reads the working directory for parsers. Parser discovery is
`--parsers` → `$ABMX_PARSERS` → `~/.config/abmx/parsers/` →
`/etc/abmx/parsers/`. Run `abmx parser paths` to see the chain.

## Labels

`[receipts].enabled = false` pauses the receipt path without deleting its
mappings: the receipt source is skipped and `abmx status` reports it as disabled.
The same switch exists as `[statements].enabled` for the statement path.

`[receipts].prefix` selects the routed namespace. The remainder of the label must
be a single parser id; dots are conventional, not required
(`example-qris` and `example.card_debit` are both valid).

- Only labels with an entry in `[receipts.accounts]` are fetched. A label under
  the prefix with no entry is unmanaged and left alone by `sync`; `abmx status`
  reports it as a failure until it is mapped or ignored.
- `[receipts].ignore` records an explicit no from `abmx setup`.
- `[receipts].since` adds Gmail's `after:` filter. Without it, every labelled
  message is considered on every run.
- `abmx samples` deliberately ignores `since`, so authoring evidence is never
  starved by the import window.

## Secrets

Never put secrets in the config file if you can avoid it. Precedence is
**environment → literal → file**, so systemd can override whatever is on disk:

| Secret                 | Environment                   | Config field                                                |
| ---------------------- | ----------------------------- | ----------------------------------------------------------- |
| Gmail client secret    | `ABMX_GMAIL_CLIENT_SECRET`    | `client_secret` / `client_secret_file` / `credentials_file` |
| Actual API key         | `ABMX_ACTUAL_API_KEY`         | `api_key` / `api_key_file`                                  |
| Actual server password | `ABMX_ACTUAL_SERVER_PASSWORD` | `server_password` / `server_password_file`                  |
| Actual budget password | `ABMX_ACTUAL_BUDGET_PASSWORD` | `budget_password` / `budget_password_file`                  |

The Gmail refresh token lives in the token file. For a server or container, use
`abmx auth export` to write one self-contained bundle (client id, secret, and
refresh token) and point `[fetch.gmail].token` at it.

For local runs, copy the template and let direnv load it:

```bash
cp .env.example .env     # .env is gitignored
$EDITOR .env
```

`.envrc` runs `dotenv_if_exists .env`, so every `abmx` invocation in this
directory sees the secrets without exporting them by hand.

:::caution[Old wrapper key]
`ABMX_ACTUAL_API_KEY` now means **Actual's** budget-scoped key. If your `.env`
still holds the old `actual-http-api` wrapper key, remove it: the helper will
reject it as an invalid session token. On Actual builds without API keys, use
`ABMX_ACTUAL_SERVER_PASSWORD` instead.
:::

## State

`[general].state` is the SQLite database holding delivered-message state and the
contract version. It is local, disposable state: deleting it makes the next sync
reconsider messages, but `imported_id` keeps the ledger idempotent. Use
`abmx reset` to clear the delivered set deliberately, or `abmx reset --purge` to
delete the database file entirely. Neither touches the ledger or stored tokens.

## Sinks

Two sinks ship with abmx:

- `[sink.actual]` — see [Actual Budget](/guides/actual/).
- `[sink.file]` — append-only JSONL; see
  [the JSONL sink](/guides/actual/#alternative-the-jsonl-sink).
