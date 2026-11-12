# Actual Budget

Actual has **no REST API**. The official client (`@actual-app/api`) is a Node
library that downloads a copy of the budget database, mutates it, and syncs it
back. A Rust process cannot speak that protocol without reimplementing it.

`txsync` therefore talks to
[`actual-http-api`](https://github.com/jhonderson/actual-http-api), a small
wrapper that exposes the parts of the official client over HTTP. Everything
stays in Rust; nothing shells out to Node.

```
txsync ──HTTP──► actual-http-api ──► @actual-app/api ──► actual-server
```

## Quick start (docker compose)

The repo ships a compose file for the wrapper. It is meant to sit next to an
Actual server you already run.

```bash
cp .env.example .env
$EDITOR .env          # ACTUAL_SERVER_PASSWORD, TXSYNC_ACTUAL_API_KEY, ACTUAL_SERVER_URL
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
| `TXSYNC_ACTUAL_API_KEY` | the wrapper's own key; `txsync` sends it as `x-api-key` |

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

## Configure `txsync`

```toml
[sink.actual]
url = "http://localhost:5007"
budget_sync_id = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
# api_key_file = "/run/credentials/txsync.service/actual-api-key"
# budget_password_file = "/run/credentials/txsync.service/actual-budget-password"

[accounts]
"example-account" = "00000000-0000-0000-0000-000000000000"
```

Find the **Sync ID** in Actual: Settings → Show advanced settings → Sync ID.
Find account UUIDs with:

```bash
txsync accounts
```

That prints every account with its id and whether `[accounts]` maps onto it. The
mapping is never inferred: `example-account` is a `txp` key, not an Actual name, and
guessing would be how transactions end up in the wrong account.

If the budget is end-to-end encrypted, set `budget_password` (or
`TXSYNC_ACTUAL_BUDGET_PASSWORD`). The wrapper needs it on the first request after
a restart.

## What `txsync` sends

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
      "cleared": true
    }
  ],
  "defaultCleared": true,
  "reimportDeleted": false,
  "payeeNameNormalization": "original",
  "dryRun": false
}
```

Four decisions worth knowing:

### `amount` is value × 100 — for every currency

Actual stores money as an integer of "value × 100", whatever the currency. IDR
has no minor units, so **Rp 27,500 must be sent as `2_750_000`**. Getting this
wrong silently multiplies every transaction by 100.

`txsync` does the conversion in `txsync-actual/src/convert.rs`:

```
amount = amount_minor × 10^(2 − currency_exponent)
```

Currencies with three decimals (KWD, BHD, …) lose their last digit — Actual has
no representation for it. That is a property of the target, not of the
conversion.

Set Actual's **hide decimal places** preference for IDR, otherwise the UI shows
`27,500.00`.

### `imported_id` is `txp`'s id

That id is a hash of the email's `Message-ID`, so it is stable across parser
changes and re-runs. Actual's `importTransactions` will never add the same
`imported_id` twice.

### `reimportDeleted: false`

If you delete a transaction in Actual, `txsync` will not resurrect it on the next
run. This matches the file-import UI's default.

### `payeeNameNormalization: "original"`

The wrapper's default title-cases payee names, turning `ExamplePay` into `Examplepay`.
`original` keeps what the bank printed.

## `dryRun`

`txsync sync --dry-run` sets `dryRun: true` in the request body rather than
skipping the call. Actual tells you what it *would* have added, so a dry run
still validates the account ids and the payload.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `401 invalid api key` | `API_KEY` mismatch | check `TXSYNC_ACTUAL_API_KEY` against the container env |
| `404` on every request | wrong Sync ID | Settings → advanced → Sync ID |
| `Unknown error while interacting with Actual Api` | wrapper cannot reach Actual | check `ACTUAL_SERVER_URL` from *inside* the container; `just actual-api-logs` |
| amounts are 100× too big | you are not using `txsync`'s sink | do not hand-write the `amount` field |
| rows appear twice | a sink other than `importTransactions` was used | `txsync` always uses the reconciling import endpoint |
| `account not found` | mapping points at a deleted account | `txsync accounts`, then fix `[accounts]` |
| container exits immediately | `./var/actual-http-api` not writable | `just actual-api` creates it owned by you; if it is root-owned, remove it and retry |

## Concurrency

`actual-http-api` holds a local copy of the budget and syncs it back. Two
concurrent writers can stomp each other. Run one `txsync` at a time — the NixOS
module uses a oneshot service and a timer, which serialises naturally. If you run
it from cron, wrap it:

```bash
flock -n /tmp/txsync.lock txsync sync
```

Avoid running a sync while you are actively editing the same budget in the web
UI. Actual's own sync will merge, but the window for confusion is not worth it.

## Alternative: the JSONL sink

If you would rather not run the wrapper, point `txsync` at a file and import it
yourself:

```toml
[sink.file]
path = "~/.local/share/txsync/ledger.jsonl"
```

Each line is `{ "tx_id", "account", "transaction" }` with `txp`'s contract
verbatim. You lose automatic dedupe and automatic writes, but nothing else.
