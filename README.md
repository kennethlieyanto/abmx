# txsync

Deliver parsed bank transactions into a budget ledger — automatically.

`txsync` is the missing half of [`txp`](https://github.com/kennethlieyanto/txp).
`txp` turns one bank email into one normalized transaction and stops there, on
purpose. `txsync` does the delivery: it fetches mail, hands the bytes to `txp` as
a **library**, decides what is safe to import, and writes it to a ledger.

```
Gmail ──► txsync ──► txp (library) ──► policy ──► plan ──► Actual Budget
         fetch       .eml → Transaction   decide    diff     apply
```

```
$ txsync sync
txsync: fetched 3  parsed 3  import 2  review 1  skip 0  unmatched 0  failed 0
txsync: applied: added 2  updated 0  skipped 0  errors 0
```

Nothing is guessed. A transaction the parser was unsure about, an account that
has no mapping, or a row the bank reported as failed never reaches the ledger —
it lands in a review queue instead.

## Status

Early, and honest about it. One fetcher (Gmail), one real sink (Actual Budget),
one reference sink (JSONL file). The primitives are stable; the integrations are
thin.

## Install

```bash
# from a checkout, with ../txp next to it
cargo install --path crates/txsync

# Nix
nix build .#          # ./result/bin/txsync
nix develop           # rust, cargo, clippy, rustfmt, just, sqlite

# or just try it in place: .envrc adds target/debug to PATH
direnv allow          # first time only
cargo build
txsync --version
```

`txp` is not published to crates.io yet, so the workspace consumes it from a
sibling checkout (`txp = { path = "../txp/crates/txp" }`). `just txp-git v0.1.0`
switches to the pinned git tag; `just txp-local` switches back.

## Quickstart

```bash
txsync init                       # writes ~/.config/txsync/config.toml
$EDITOR ~/.config/txsync/config.toml
txsync auth gmail                 # OAuth2 + PKCE, stores a refresh token (0600)
txsync doctor --online            # config, corpus, credentials, sink, mapping
txsync sync --dry-run             # see the plan, write nothing
txsync sync                       # for real
```

Then put `txsync sync` on a timer (see the NixOS module below).

### You need

1. **A `txp` parser corpus** — `txsync` does not ship bank templates.
   ```bash
   tar xzf txp-parsers-<version>.tar.gz -C ~/.config/txp
   txp paths     # confirms what txsync will load
   ```
2. **A Gmail OAuth client** — one Google Cloud project, `gmail.readonly`, desktop
   app client. See [docs/gmail.md](docs/gmail.md).
3. **A ledger endpoint** — for Actual Budget that is
   [`actual-http-api`](https://github.com/jhonderson/actual-http-api) on `:5007`.
   From a checkout: `cp .env.example .env`, fill in the Actual password and API
   key, then `just actual-api`. See [docs/actual.md](docs/actual.md).

## The four primitives

| Primitive | Responsibility | Ships with |
| --- | --- | --- |
| **Fetcher** | a source of raw RFC 5322 bytes | Gmail API, local directory |
| **Policy** | `Transaction` → import / review / skip | one pure function |
| **Sink** | decided rows → the ledger | Actual Budget, JSONL file |
| **State** | cursors, delivered messages, review queue | SQLite |

A new mailbox is a new `Fetcher`. A new ledger is a new `Sink`. Neither requires
touching the runner, the CLI, or the other integrations. See
[docs/architecture.md](docs/architecture.md) for why the seams are where they
are.

### Why email-only input is a feature

`txp`'s contract is `.eml` bytes in, one `Transaction` out. Keeping that contract
narrow is what makes parsers testable against files. Sources that are not email
(a CSV export, a bank API) don't need to go through `txp` at all — they emit the
same `Transaction` JSON and enter at the policy stage.

## How duplication is prevented

Three independent layers, because they fail in different ways:

1. **`imported_id`** — every row carries `txp`'s stable id (a hash of the email's
   `Message-ID`). Actual's `importTransactions` never adds the same `imported_id`
   twice, so replaying a plan is safe.
2. **State** — a delivered message is recorded so the next run doesn't even
   re-parse it. This is an optimisation for idempotent sinks and a requirement
   for sinks that have no idempotency key.
3. **Cursor discipline** — Gmail's `historyId` only advances when *every* row in
   the batch succeeded. A partial failure leaves the cursor in place; the next
   run replays and the state database skips what already landed.

## Config

```toml
[general]
parsers = []                       # empty = txp's implicit discovery
state = "~/.local/state/txsync/state.db"

[fetch.gmail]
client_id = "….apps.googleusercontent.com"
lookback_days = 7                  # floor for full syncs
max_messages = 200
# query_extra = "-label:txsync-ignored"
# labels = ["INBOX"]

[sink.actual]
url = "http://localhost:5007"
budget_sync_id = "…"
chunk_size = 50
# notes_fields = ["description", "reference_id", "channel", "parser"]

[accounts]
"example-account" = "<actual account uuid>"
"wallet-account" = "<actual account uuid>"

[policy]
heuristic = "review"               # review | import | skip
failed = "skip"                    # skip | review | import
```

Resolution order: `--config` → `$TXSYNC_CONFIG` → `$XDG_CONFIG_HOME/txsync/config.toml`
→ `./txsync.toml`. Relative paths resolve against the config file's directory.

### Secrets

Never in the config file if you can avoid it. Precedence is **environment →
literal → file**, so systemd can override whatever is on disk:

For local runs, copy the template and let direnv load it:

```bash
cp .env.example .env     # .env is gitignored
$EDITOR .env
```

`.envrc` runs `dotenv_if_exists .env`, so every `txsync` invocation in this
directory sees the secrets without exporting them by hand.

| Secret | Environment | Config field |
| --- | --- | --- |
| Gmail client secret | `TXSYNC_GMAIL_CLIENT_SECRET` | `client_secret` / `client_secret_file` |
| Actual API key | `TXSYNC_ACTUAL_API_KEY` | `api_key` / `api_key_file` |
| Actual budget password | `TXSYNC_ACTUAL_BUDGET_PASSWORD` | `budget_password` / `budget_password_file` |

## CLI

```
txsync init [--path FILE] [--force]   write a starter config
txsync auth gmail                     OAuth2 with PKCE and a loopback redirect
txsync doctor [--online]              check config, corpus, state, credentials, sink
txsync plan [--from-dir DIR] [--out FILE]
                                      fetch + parse + decide, print the plan as JSON
txsync apply [--plan FILE] [--dry-run]
                                      write a plan (current or saved)
txsync sync [--dry-run]               plan + apply; the timer entrypoint
txsync review [--json]                rows waiting for a human
txsync approve <id>… [--dry-run]      import reviewed rows
txsync accounts [--json]              sink accounts and how [accounts] maps onto them
txsync parsers                        loaded txp parsers and the senders they claim
```

**stdout is data, stderr is diagnostics.** `txsync plan` writes the plan JSON to
stdout and the human summary to stderr, so it pipes cleanly:

```bash
txsync plan --from-dir ./mail > plan.json
txsync apply --plan plan.json --dry-run
```

`--from-dir` reads saved `.eml` files instead of calling Gmail. It is the fastest
way to test a config, and the recovery path if a mailbox provider breaks.

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | clean |
| `1` | the tool failed (config, network, state) |
| `2` | at least one message or row failed |
| `3` | `plan` / `review` only: nothing failed, but rows are waiting for review |

`sync` and `apply` return `0` when the only outstanding work is a review row —
a timer that fails every 15 minutes because one heuristic email is waiting is a
timer nobody reads. Use `txsync review` (exit `3` when non-empty) to detect them.

## Policy

| Situation | Default | Why |
| --- | --- | --- |
| `status = failed` | skip | the bank says it did not happen |
| `extraction = heuristic` | review | the amount is plausible, not verified |
| zero amount | review | almost always the wrong block matched |
| account not in `[accounts]` | review | never guess a ledger account |
| everything else | import | |

Review and skip rows are **not** marked delivered, so changing the policy picks
them up on the next run. Rows already in the review queue can be imported
individually with `txsync approve`.

## Actual Budget specifics

Actual has no REST API of its own — the official client is a Node library that
mutates a local copy of the budget. `txsync` talks to `actual-http-api`, which
wraps that library and exposes the reconciling import endpoint. Two details that
matter:

- **`amount` is value × 100 for every currency.** Rp 27,500 is sent as
  `2_750_000`. `txsync` does this conversion in the sink, with tests.
- **`imported_id` is `txp`'s id**, and `reimportDeleted` is `false`, so deleting
  a row in Actual is permanent.

## Security

- Gmail scope is `gmail.readonly`. Nothing else is requested.
- The refresh token is written with `0600` and never logged.
- The OAuth flow uses PKCE and a `127.0.0.1` redirect; there is no out-of-band
  flow (Google retired it).
- Ledger credentials are separate from mailbox credentials and live in different
  files.
- `txsync` reads mail and writes to a ledger. It never deletes mail and never
  deletes ledger rows.

## Testing

```bash
just test     # unit + integration + end-to-end, no network, no credentials
just check    # fmt + clippy -D warnings
just ci       # both
```

The end-to-end test (`crates/txsync/tests/e2e.rs`) runs a real `txp` parse over a
real `.eml`, against a fake Gmail API and a fake `actual-http-api`, and asserts
the exact JSON body that reaches Actual — including the ×100 amount conversion.

## NixOS

```nix
{
  inputs.txsync.url = "git+ssh://git@github.com/kennethlieyanto/txsync";

  services.txsync = {
    enable = true;
    configFile = /etc/txsync/config.toml;
    interval = "15min";
    secrets = {
      "actual-api-key" = config.age.secrets.actual-api-key.path;
      "gmail-client-secret" = config.age.secrets.gmail-client-secret.path;
    };
  };
}
```

Credentials land in `/run/credentials/txsync.service/<name>`, so the config refers
to them as `api_key_file = "/run/credentials/txsync.service/actual-api-key"`.

## Out of scope, by design

- Parsing bank templates. That is `txp`.
- Transfers linking (importing both sides of a transfer as one).
- Pending → settled reconciliation. Today `txp` emits no `pending` rows; when it
  does, this needs a matching rule, not a heuristic.
- Multi-currency conversion. `amount_minor` and `currency_exponent` are carried
  through unchanged.
- Webhooks and push sources. The `Sink` trait is push-shaped, but nothing calls
  it from a request handler yet.

## Layout

```
crates/txsync-core/    traits and types. No I/O, no network.
crates/txsync-gmail/   Fetcher: Gmail API + OAuth2/PKCE
crates/txsync-actual/  Sink: actual-http-api
crates/txsync-file/    Sink: append-only JSONL (reference implementation)
crates/txsync/         config, SQLite state, runner, CLI
crates/txsync-testkit/ a blocking HTTP mock server for tests
docker-compose.yml     actual-http-api wrapper for local testing
```

MIT.
