# Actual Budget

Actual has **no REST API**. The official client (`@actual-app/api`) is a Node
library that downloads a copy of the budget database, mutates it, and syncs it
back. A Rust process cannot speak that protocol without reimplementing it.

`abmx` therefore talks to
[`actual-http-api`](https://github.com/jhonderson/actual-http-api), a small
wrapper that exposes the parts of the official client over HTTP. Everything
stays in Rust; nothing shells out to Node.

```
abmx ──HTTP──► actual-http-api ──► @actual-app/api ──► actual-server
```

## Quick start (docker compose)

The repo ships a compose file for the wrapper. It is meant to sit next to an
Actual server you already run.

```bash
cp .env.example .env
$EDITOR .env          # ACTUAL_SERVER_PASSWORD, ABMX_ACTUAL_API_KEY, ACTUAL_SERVER_URL
just actual-api       # docker compose up, waits until it serves /api-docs/
just actual-api-status
just actual-api-logs
just actual-api-down
```

Three variables matter:

| Variable | Meaning |
| --- | --- |
| `ACTUAL_SERVER_URL` | Actual as seen **from inside the container** |
| `ACTUAL_SERVER_PASSWORD` | your Actual server password |
| `ABMX_ACTUAL_API_KEY` | the wrapper's own key; `abmx` sends it as `x-api-key` |

`ACTUAL_SERVER_URL` is the one people get wrong. The container's `localhost` is
not your host's `localhost`:

| Where Actual runs | `ACTUAL_SERVER_URL` |
| --- | --- |
| on this host | `http://host.docker.internal:5006` |
| on another machine | `http://192.168.1.10:5006` |
| in this compose file | `http://actual-server:5006` |

The image tag must match your Actual server's version. Set
`ACTUAL_HTTP_API_TAG` in `.env` to your Actual version.

### A throwaway Actual instead

`docker-compose.yml` has a commented-out `actual-server` service. Uncomment it,
run `just actual-api`, open <http://localhost:5006>, set a password and create a
budget, then set `ACTUAL_SERVER_URL=http://actual-server:5006`. Nothing touches
your real budget.

## Manual `docker run`

If you would rather not use compose:

```bash
docker run -d --name actualhttpapi \
  -v "$PWD/var/actual-http-api:/data:rw" \
  -p 5007:5007 \
  --user "$(id -u):$(id -g)" \
  --add-host host.docker.internal:host-gateway \
  -e ACTUAL_SERVER_URL="http://host.docker.internal:5006" \
  -e ACTUAL_SERVER_PASSWORD="your-actual-server-password" \
  -e API_KEY="$(openssl rand -hex 24)" \
  jhonderson/actual-http-api:26.10.0
```

`API_KEY` is the wrapper's own credential. It is **not** your Actual password;
keep them separate so a leaked ledger API key does not expose the server.

## Configure `abmx`

```toml
# `url` is actual-http-api, the wrapper. It is not your Actual server (:5006):
# Actual has no REST API of its own.
[sink.actual]
url = "http://localhost:5007"   # actual-http-api, not the Actual server
budget_sync_id = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
# api_key_file = "/run/credentials/abmx.service/actual-api-key"
# budget_password_file = "/run/credentials/abmx.service/actual-budget-password"

[labels]
prefix = "Receipt/abmx:"

[labels.accounts]
"example-card" = "00000000-0000-0000-0000-000000000000"
```

Find the **Sync ID** in Actual: Settings → Show advanced settings → Sync ID.
Find account UUIDs with:

```bash
abmx accounts
```

That prints every account with its id and whether `[labels.accounts]` maps onto it. The
mapping is never inferred: the label is the routing identity, the account UUID is
the ledger destination, and guessing would be how transactions end up in the
wrong account.

If the budget is end-to-end encrypted, set `budget_password` (or
`ABMX_ACTUAL_BUDGET_PASSWORD`). The wrapper needs it on the first request after
a restart.

## What `abmx` sends

`POST /v1/budgets/{syncId}/accounts/{accountId}/transactions/import`

```json
{
  "transactions": [
    {
      "account": "<account uuid>",
      "date": "2026-10-30",
      "amount": -2750000,
      "imported_id": "mail_a631e3d89a232b55",
      "payee_name": "ExamplePay",
      "imported_payee": "ExamplePay",
      "notes": "DEBIT ONLINE | example.card_debit",
      "cleared": false
    }
  ],
  "defaultCleared": false,
  "reimportDeleted": true,
  "payeeNameNormalization": "original",
  "dryRun": false
}
```

Five decisions worth knowing:

### `amount` is value × 100 — for every currency

Actual stores money as an integer of "value × 100", whatever the currency. IDR
has no minor units, so **Rp 27,500 must be sent as `2_750_000`**. Getting this
wrong silently multiplies every transaction by 100.

`abmx` does the conversion in `abmx-actual/src/convert.rs`:

```
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

### Seeing what will happen first: `abmx sync --dry-run --diff`

`importTransactions` reconciles by `imported_id` **and** by a fuzzy
amount/date/payee match. That second path is where surprises live: a row that
already exists under a *different* importer's id (a CSV import, a bank sync)
can end up duplicated instead of reconciled.

`abmx sync --dry-run --diff` reads the ledger for the plan's date window and
classifies every row before anything is written:

```
ledger: 12 new, 2 already present, 1 foreign imported_id
  foreign  2026-11-06  -72,000 IDR  EXAMPLE MERCHANT  imported as example-2026-11-06-merchant
```

- `new` — nothing in the ledger has this account, date and amount.
- `already present` — a manual row or a row abmx itself imported; it will reconcile.
- `foreign imported_id` — the existing row came from another importer, so expect a
  duplicate. The exit code is still `0` unless a label is broken; treat the
  foreign line as a warning before applying for real.

The match is deliberately conservative: account + date + amount only. Payee
similarity is Actual's judgement, not ours.

### `reimportDeleted: true` — the ledger is a projection

`abmx` treats the mailbox as the source of truth and the ledger as a
projection of it:

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
  abmx sync       # rebuild from the mailbox
  ```

  `abmx reset` never touches the ledger — it only clears local state.

### `payeeNameNormalization: "original"`

The wrapper's default title-cases payee names, turning `ExamplePay` into `Examplepay`.
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

`abmx sync --dry-run` sets `dryRun: true` in the request body rather than
skipping the call. Actual tells you what it *would* have added, so a dry run
still validates the account ids and the payload.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `401 invalid api key` | `API_KEY` mismatch | check `ABMX_ACTUAL_API_KEY` against the container env |
| `404` on every request | wrong Sync ID | Settings → advanced → Sync ID |
| `Unknown error while interacting with Actual Api` | wrapper cannot reach Actual | check `ACTUAL_SERVER_URL` from *inside* the container; `just actual-api-logs` |
| amounts are 100× too big | you are not using `abmx`'s sink | do not hand-write the `amount` field |
| rows appear twice | a sink other than `importTransactions` was used | `abmx` always uses the reconciling import endpoint |
| `account not found` | mapping points at a deleted account | `abmx accounts`, then fix `[labels.accounts]` |
| container exits immediately | `./var/actual-http-api` not writable | `just actual-api` creates it owned by you; if it is root-owned, remove it and retry |

## Concurrency

`actual-http-api` holds a local copy of the budget and syncs it back. Two
concurrent writers can stomp each other. Run one `abmx` at a time — the NixOS
module uses a oneshot service and a timer, which serialises naturally. If you run
it from cron, wrap it:

```bash
flock -n /tmp/abmx.lock abmx sync
```

Avoid running a sync while you are actively editing the same budget in the web
UI. Actual's own sync will merge, but the window for confusion is not worth it.

## Alternative: the JSONL sink

If you would rather not run the wrapper, point `abmx` at a file and import it
yourself:

```toml
[sink.file]
path = "~/.local/share/abmx/ledger.jsonl"
```

Each line is `{ "tx_id", "account", "transaction" }` with the parser's contract
verbatim. You lose automatic dedupe and automatic writes, but nothing else.
