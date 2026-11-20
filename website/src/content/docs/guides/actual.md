---
title: Actual Budget
description: Why Actual needs a helper, how credentials and amounts work, and what abmx sends.
---

Actual has **no HTTP API**. The official client is [`@actual-app/api`], a Node
library that downloads a copy of the budget database, mutates it locally, and
syncs it back. There is no REST endpoint to call and no server-side budget
engine.

`abmx` therefore ships a **one-shot helper** around that library. For each
operation it spawns `node helper.js`, writes one JSON request to stdin, reads one
JSON response from stdout, and waits for it to exit. Nothing stays running: no
port, no service, no shared secret on the wire.

```text
abmx ──spawn──► actual-helper (Node) ──► @actual-app/api ──► actual-server
     ◄─JSON───                              (local copy)          :5006
```

[`@actual-app/api`]: https://actualbudget.org/docs/api/

## Configure

You need Node 22+ and the helper only if you install with plain
`cargo install`; the Nix package and the container bundle both.

```toml
[sink.actual]
server_url = "http://localhost:5006"   # the Actual server itself
budget_sync_id = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
api_key_file = "/run/credentials/abmx.service/actual-api-key"
# or, for servers without API keys:
# server_password_file = "/run/credentials/abmx.service/actual-server-password"
# data_dir = "~/.cache/abmx/actual"
```

Find the **Sync ID** in Actual: Settings → Show advanced settings → Sync ID.

`abmx setup` writes this section interactively, stores the credential in the
config directory with `0600`, and verifies it by reading the account list.
Afterwards:

```bash
abmx status    # helper version, server version, account map
abmx accounts  # account UUIDs, and which parsers are mapped
```

`data_dir` is `@actual-app/api`'s local copy of the budget. Keep it persistent
(the default is `$XDG_CACHE_HOME/abmx/actual`), otherwise every run downloads the
whole budget. It is disposable: delete it and the next run downloads a fresh
copy.

Running in a container? See [Docker](/guides/deployment/docker/).

## Credentials

An **Actual API key** is the preferred credential: it is scoped to one budget
and revocable in Settings → General → Security → API keys. If your Actual build
does not show that section, use the **server password** instead.

| Config | Environment | Meaning |
| --- | --- | --- |
| `api_key` / `api_key_file` | `ABMX_ACTUAL_API_KEY` | Actual budget-scoped API key |
| `server_password` / `server_password_file` | `ABMX_ACTUAL_SERVER_PASSWORD` | Actual server password |
| `budget_password` / `budget_password_file` | `ABMX_ACTUAL_BUDGET_PASSWORD` | end-to-end encryption password, if the budget has one |

If both credentials are present the API key wins. Secrets follow the usual
precedence: environment → literal → file.

:::caution[Upgrading from actual-http-api]
`ABMX_ACTUAL_API_KEY` used to be the wrapper's key. It now means Actual's own
key. Remove the old value from `.env` or you will get
`invalid or expired session token`. Replace `url = "…:5007"` with
`server_url = "…:5006"`; `abmx` refuses the old key with a migration message
rather than silently ignoring it.
:::

## What abmx sends

The helper calls `importTransactions`, Actual's reconciling import:

```json
{
  "method": "import",
  "params": {
    "budgetSyncId": "…",
    "account": "<account uuid>",
    "transactions": [
      {
        "account": "<account uuid>",
        "date": "2026-10-30",
        "amount": -2750000,
        "imported_id": "mail_a631e3d89a232b55",
        "payee_name": "ExamplePay",
        "imported_payee": "ExamplePay",
        "cleared": false
      }
    ],
    "options": {
      "defaultCleared": false,
      "reimportDeleted": true,
      "payeeNameNormalization": "original",
      "dryRun": false
    }
  }
}
```

All four options are documented `@actual-app/api` options; the helper is a
pass-through, not a second implementation.

`notes` is omitted when there is nothing to write. The default
`[sink.actual].notes_fields` is just `["description"]`; set it to `[]` to disable
notes entirely, or add fields such as `reference_id`, `channel`, or `parser`.

`abmx receipt sync --dry-run` previews the notes in its table (a `-` means the import
will omit the `notes` key).

Six decisions worth knowing:

### `amount` is value × 100 — for every currency

Actual stores money as an integer of "value × 100", whatever the currency. IDR
has no minor units, so **Rp 27,500 must be sent as `2_750_000`**. Getting this
wrong silently multiplies every transaction by 100.

`abmx` does the conversion in `crates/abmx-actual/src/convert.rs`:

```text
amount = amount_minor × 10^(2 − currency_exponent)
```

Currencies with three decimals (KWD, BHD, …) lose their last digit — Actual has
no representation for it. That is a property of the target, not of the
conversion.

Set Actual's **hide decimal places** preference for IDR, otherwise the UI shows
`27,500.00`.

### `imported_id` is the parser's id

That id is a hash of the email's `Message-ID`, so it is stable across parser
changes and re-runs. Actual's `importTransactions` will never add the same
`imported_id` twice.

### Seeing what will happen first: `abmx receipt sync --dry-run --diff`

`importTransactions` reconciles by `imported_id` **and** by a fuzzy
amount/date/payee match. That second path is where surprises live: a row that
already exists under a *different* importer's id (a CSV import, a bank sync) can
end up duplicated instead of reconciled.

`abmx receipt sync --dry-run --diff` reads the ledger for the plan's date window and
classifies every row before anything is written:

```text
ledger: 12 new, 2 already present, 1 foreign imported_id
  foreign  2026-11-06  -72,000 IDR  EXAMPLE MERCHANT  imported as example-2026-11-06-merchant
```

- `new` — nothing in the ledger has this account, date and amount.
- `already present` — a manual row or a row abmx itself imported; it will reconcile.
- `foreign imported_id` — the existing row came from another importer, so expect
  a duplicate. The exit code is still `0` unless a label is broken; treat the
  foreign line as a warning before applying for real.

The match is deliberately conservative: account + date + amount only. Payee
similarity is Actual's judgement, not ours.

### `reimportDeleted: true` — the ledger is a projection

`abmx` treats the mailbox as the source of truth and the ledger as a projection
of it:

| What you delete | What happens |
| --- | --- |
| the **email** | the row is never re-imported |
| the **ledger row** | the next full sync recreates it |

This is the Terraform model: lose the state file, apply again, and the resources
come back. `imported_id` keeps it idempotent — rows that still exist are matched
and ignored, so only genuinely missing ones are recreated.

```toml
[sink.actual]
reimport_deleted = true    # default
```

Set it to `false` if you want ledger deletions to be permanent (that is what
Actual's file-import UI does). Then the only way to remove something is to delete
or filter the email.

Two consequences:

- **Duplicates come back too.** If you deleted a row because it was a duplicate,
  a full sync recreates it. Fix the source instead, or merge rather than delete.
- **Incremental syncs won't notice a deletion.** They only ask Gmail for new
  messages, so use `abmx reset` to force a full re-scan:

  ```bash
  abmx reset      # forget cursors + delivered messages
  abmx receipt sync  # rebuild from the mailbox
  ```

  `abmx reset` never touches the ledger — it only clears local state.

### `payeeNameNormalization: "original"`

Actual's default title-cases payee names, turning `ExamplePay` into `Examplepay`.
`original` keeps what the bank printed.

### `cleared` — off by default

Actual's importer resolves it as `cleared: trans.cleared ?? defaultCleared`, so
the per-row value wins.

`abmx` sends `cleared: false` by default:

```toml
[sink.actual]
cleared = "never"    # never | settled | always
```

| Policy | Behaviour |
| --- | --- |
| `never` (default) | import uncleared; clearing stays your reconciliation step |
| `settled` | clear rows whose parser status is `settled` |
| `always` | clear everything |

The distinction matters: the parser's `status: settled` means "the bank says the
transaction completed", while Actual's `cleared` means "I reconciled this
against my bank statement". Making the second claim automatically would be
guessing, which is why it is opt-in.

Changing the policy only affects future imports; rows already imported stay
cleared until you un-clear them in Actual.

## `dryRun`

`abmx receipt sync --dry-run` sets `dryRun: true` in the request rather than skipping the
call. Actual tells you what it *would* have added, so a dry run still validates
the account ids and the payload.

## Version compatibility

`abmx` bundles exactly one `@actual-app/api` version. Actual ships the server and
the client together, so `abmx status` compares them:

```text
sink
  ok    actual: 2 accounts
  ok    helper 26.11.0 (@actual-app/api)
  ok    server 26.11.0
```

- A **minor mismatch** warns and continues. An older client usually still works.
- A **year jump** (server 27, helper 26) refuses to write before touching the
  budget. Update `abmx`.
- If the server is unreachable, there is no version warning; the sync itself
  reports the network error.

Update by installing a new `abmx`: Nix `nix flake update`, container pull, or
`cargo install` plus `just helper-install`. Maintainers bump the pin with
`just helper-bump 26.12.0`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `could not run 'node'` | Node is not on `PATH` | install Node 22+, or set `[sink.actual].node` / `ABMX_NODE` |
| `the Actual helper script was not found` | helper not installed | use the Nix package/container, set `helper`, or `ABMX_ACTUAL_HELPER` |
| `invalid or expired session token` | stale wrapper key in `.env`, or a wrong/lost API key | remove `ABMX_ACTUAL_API_KEY` or create a new key |
| `invalid-password` | wrong server password | check `server_password` |
| `budget-not-found` | wrong Sync ID | Settings → advanced → Sync ID |
| `network-failure` | server unreachable | check `server_url` from the machine running abmx |
| `missing-key` / `decrypt-failure` | end-to-end encrypted budget | set `budget_password` |
| `the Actual helper timed out` | slow budget sync | raise `timeout_secs` |
| amounts are 100× too big | you are not using abmx's sink | do not hand-write the `amount` field |
| rows appear twice | a sink other than `importTransactions` was used | `abmx` always uses the reconciling import |
| `account not found` | mapping points at a deleted account | `abmx accounts`, then fix `[receipts.accounts]` |

Set `ABMX_ACTUAL_DEBUG=1` to forward the library's own log (it is quiet by
default) to stderr.

## Concurrency

The helper holds a local copy of the budget and syncs it back. Two concurrent
writers can stomp each other. Run one `abmx` at a time — the NixOS module uses a
oneshot service and a timer, which serialises naturally. If you run it from
cron, wrap it:

```bash
flock -n /tmp/abmx.lock abmx sync
```

Avoid running a sync while you are actively editing the same budget in the web
UI. Actual's own sync will merge, but the window for confusion is not worth it.

## Alternative: the JSONL sink

If you would rather not run Node, point `abmx` at a file and import it yourself:

```toml
[sink.file]
path = "~/.local/share/abmx/ledger.jsonl"
```

Each line is `{ "tx_id", "account", "transaction" }` with the parser's contract
verbatim. You lose automatic dedupe and automatic writes, but nothing else.
