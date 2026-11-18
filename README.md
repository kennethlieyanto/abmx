# abmx

Sync bank transaction emails into an Actual Budget ledger — automatically.

`abmx` fetches bank notification mail, parses it with a parser corpus you can
teach without rebuilding, decides what is safe to import, and writes it to a
ledger. It never guesses: an unsure parse, an unmapped account, or a row the
bank reported as failed is reported as a broken label and retried after the parser or Gmail rule is fixed.

```
Gmail (routed labels) ──► abmx ──► parser corpus ──► classify ──► Actual Budget
                          fetch     .eml → Transaction   import      apply
```

```
$ abmx sync
fetched 15, parsed 15 — import 13, broken 2, unmatched 0, failed 0

date                 amount label                        counterparty
2026-11-12     -128,000 IDR receipt/example.card_debit  EXAMPLE PAYEE
2026-11-11     -124,300 IDR receipt/example.card_debit  EXAMPLE MERCHANT
...

broken (2), not imported:
  receipt/example.journal_va   parse_failed   amount: no value matched
  next: abmx samples --label receipt/example.journal_va
  nothing was written to the ledger

applied: added 0, updated 0, skipped 0, errors 0
```

`-q` prints only the two summary lines; `-v` adds the parser id and source
message key to each row.

## Status

Early, and honest about it. One fetcher (Gmail), one real sink (Actual Budget),
one reference sink (JSONL file). The primitives are stable; the integrations are
thin. The parser corpus lives in this repository under `parsers/`.

## Install

```bash
# from a checkout
cargo install --path crates/abmx

# Nix
nix build .#          # ./result/bin/abmx, parser corpus bundled
nix develop           # rust, cargo, clippy, rustfmt, just, sqlite

# or just try it in place: .envrc adds target/debug to PATH
direnv allow          # first time only
cargo build
abmx --version
```

The Nix package bundles the corpus and points `ABMX_PARSERS` at it. A source
checkout does not: run `just install-parsers` to check the repo corpus and copy
it into `~/.config/abmx/parsers/`, or pass `--parsers <dir>` explicitly. `abmx`
never reads the working directory.

## Quickstart

```bash
abmx setup                      # config + Gmail + labels + accounts, interactively
abmx status                     # config, corpus, labels, accounts, auth
abmx sync --dry-run             # see the plan, write nothing
abmx sync                       # import everything mapped
```

`abmx setup` is idempotent: rerun it whenever you add a label, change accounts,
or move to a new machine. Every step it has already completed is skipped. See
[docs/setup.md](docs/setup.md) for the details.

Prefer doing it by hand?

```bash
abmx init                            # writes ~/.config/abmx/config.toml
$EDITOR ~/.config/abmx/config.toml   # Gmail client file + Actual endpoint
abmx auth gmail                      # OAuth2 + PKCE, stores a refresh token (0600)
abmx status
```

Routing is explicit and lives in two places:

- **gmailctl** labels the mail. A tracked message carries exactly one
  `receipt/<parser-id>` label; merchant receipts keep the plain `receipt`
  label and are never imported.
- **`[labels.accounts]`** maps that parser id to an Actual account. That table is
  the *only* definition of what abmx manages: labels without an entry are not
  fetched at all, so a half-configured mailbox is never a sync failure.

```toml
[labels]
prefix = "receipt/"
since  = 2026-11-01          # history window; omitting it scans all labelled mail
ignore = ["merchant.promo"]  # declined in `abmx setup`; asked about again with --review

[labels.accounts]
"example.card_debit" = "<actual account uuid>"
```

`since` is strongly recommended. Without it, every run scans every message under
the mapped labels; a recent window also keeps the parser focused on templates it
was authored against.

gmailctl owns classification: OTP and security mail go to `Notifications`,
promos to `Newsletter`, merchant receipts to plain `receipt`, statements to
`Financial Statements`. `abmx` discovers labels under `[labels].prefix`
(`receipt/` by default), ignores the plain `receipt` label, and imports only the
parser ids mapped in `[labels.accounts]`.

`abmx status` says what is missing. To author a parser, download real evidence
first:

```bash
abmx samples        # up to 3 newest messages per managed label
```

Then install the authoring skill once and point your agent at it:

```bash
mkdir -p ~/.agents/skills/abmx && abmx skill > ~/.agents/skills/abmx/SKILL.md
```

```
help me create parser for all my transaction email, use abmx parser authoring skill
```

The agent writes the parser into `~/.config/abmx/parsers/<id>/` with its
fixtures and verifies it with `abmx parser check --parser <id>`.

Already have `.eml` files? Put them under
`~/.config/abmx/samples/<parser-id>/` and ask the agent to author from those.
Then put `abmx sync` on a timer (see the NixOS module below): `sync` only
fetches mapped labels, so unmanaged ones are left alone rather than aborting the
run.

### You need

1. **A Gmail OAuth client** — one Google Cloud project, `gmail.readonly`, desktop
   app client. See [docs/gmail.md](docs/gmail.md).
2. **A ledger endpoint** — for Actual Budget that is
   [`actual-http-api`](https://github.com/jhonderson/actual-http-api) on `:5007`.
   From a checkout: `cp .env.example .env`, fill in the Actual password and API
   key, then `just actual-api`. See [docs/actual.md](docs/actual.md).
3. **Parsers** — nothing to install. `abmx samples` downloads evidence and an
   agent writes parsers under `~/.config/abmx/parsers/<id>/` (`config.toml`
   plus `fixtures/`). If you already have a corpus (this repository ships one),
   point at it with `--parsers`, `$ABMX_PARSERS`, or `[general].parsers`, or run
   `just install-parsers`.

## Why mail, not statements?

The obvious alternative is to parse the monthly statement instead. Statements
are complete and bank-authoritative, but they are also about a month late, and
the whole point here is to see a transaction the day it happens.

|                        | Email notification       | Monthly statement        |
| ---------------------- | ------------------------ | ------------------------ |
| Latency                | minutes                  | ~30 days                 |
| Granularity            | one event per message    | whole period             |
| Completeness           | lossy (thresholds, spam, channels the bank does not email) | bank-authoritative |
| Metadata               | merchant, channel, reference id | often less        |
| Balances               | usually not captured     | opening + closing        |
| Parsing cost           | stable HTML labels       | PDF/CSV/Excel, fragile   |

So email is the live path, and statements are the reconciliation path. Statement
import is planned, not implemented: when it lands it will backfill missed rows
and anchor per-account balances. Until then, `abmx` clears nothing
automatically — Actual's `cleared` flag stays yours to set against a statement.

## Authoring a parser with an AI

Parsers are runtime data: a `config.toml` per template, addressed by the parser
id in the label. gmailctl routes; `abmx` parses. The spec format is in
[docs/parsing.md](docs/parsing.md).

`abmx samples` downloads the newest three messages for every label in
`[labels.accounts]` into `$XDG_CONFIG_HOME/abmx/samples/<parser-id>/`, along
with a `manifest.json`. It never parses and never needs a parser. It also
deliberately ignores `[labels].since`: the import window must not starve a
parser of evidence. A label with fewer than three messages is a warning; the
agent should stay conservative.

Options: `--label <label|parser-id>` restricts the download, `--samples N`
changes the count, `--out DIR` changes the directory, and `--json` prints the
manifest.

### The verification loop

`abmx parser check --parser <id>` verifies the parser against its colocated
fixtures. A spec is done when every sample is `strict`, non-zero, and not
`failed`. `abmx status --strict` samples live mail for managed labels and reports
the same statuses.

If the samples under one label do not share a label skeleton, the label is
mislabeled. Fix the gmailctl rule; do not write one parser for two templates.

### Reusable skill

`abmx skill` prints one self-contained `SKILL.md` to stdout. Install it once:

```bash
mkdir -p ~/.agents/skills/abmx
abmx skill > ~/.agents/skills/abmx/SKILL.md
```

After that, the prompt is simply:

```
help me create parser for all my transaction email, use abmx parser authoring skill
```

## The four primitives

| Primitive   | Responsibility                            | Ships with                 |
| ----------- | ----------------------------------------- | -------------------------- |
| **Fetcher** | a source of raw RFC 5322 bytes            | Gmail API, local directory |
| **Policy**  | `Transaction` → import or broken           | one pure function          |
| **Sink**    | decided rows → the ledger                 | Actual Budget, JSONL file  |
| **State**   | delivered messages                        | SQLite                     |

A new mailbox is a new `Fetcher`. A new ledger is a new `Sink`. Neither requires
touching the runner, the CLI, or the other integrations. Adding a *bank*, by
contrast, is a TOML file — no rebuild. See
[docs/architecture.md](docs/architecture.md) for why the seams are where they
are.

### Why email-only input is a feature

The parser's contract is `.eml` bytes in, one `Transaction` out. Keeping that
contract narrow is what makes parsers testable against files. Sources that are
not email (a CSV export, a bank API) don't need to go through the parser at all —
they emit the same `Transaction` JSON and enter at the policy stage.

## How duplication is prevented

Three independent layers, because they fail in different ways:

1. **`imported_id`** — every row carries the parser's stable id (a hash of the
   email's `Message-ID`). Actual's `importTransactions` never adds the same
   `imported_id` twice, so replaying a plan is safe.
2. **State** — a delivered message is recorded so the next run doesn't even
   re-parse it. This is an optimisation for idempotent sinks and a requirement
   for sinks that have no idempotency key.
3. **Label scan + state** — every run scans the mapped labels under
   `[labels].prefix` and lets the state database skip messages already delivered. Mail labeled late
   is picked up on the next run; a broken row is never marked delivered, so it is
   retried after the parser is fixed.

## Config

```toml
[general]
parsers = []                       # empty = implicit discovery
state = "~/.local/state/abmx/state.db"

[fetch.gmail]
client_id = "….apps.googleusercontent.com"
max_messages = 10000               # hard cap; exceeding it is an error

# Actual has no REST API: `url` is the actual-http-api wrapper, not the
# Actual server (:5006). See docs/actual.md.
[sink.actual]
url = "http://localhost:5007"   # actual-http-api, not the Actual server
budget_sync_id = "…"
chunk_size = 50
# never (default) | settled | always — see docs/actual.md
cleared = "never"
# notes_fields = ["description", "reference_id", "channel", "parser"]

[labels]
prefix = "receipt/"          # literal, case-insensitive
since  = 2026-11-01               # history window; omit to scan everything

[labels.accounts]                 # parser id -> Actual account uuid
"example.card_debit"       = "<actual account uuid>"
"example.journal_transfer" = "<actual account uuid>"
```

Resolution order: `--config` → `$ABMX_CONFIG` → `$XDG_CONFIG_HOME/abmx/config.toml`
→ `./abmx.toml`. Relative paths resolve against the config file's directory.

### Secrets

Never in the config file if you can avoid it. Precedence is **environment →
literal → file**, so systemd can override whatever is on disk:

For local runs, copy the template and let direnv load it:

```bash
cp .env.example .env     # .env is gitignored
$EDITOR .env
```

`.envrc` runs `dotenv_if_exists .env`, so every `abmx` invocation in this
directory sees the secrets without exporting them by hand.

| Secret                 | Environment                   | Config field                                      |
| ---------------------- | ----------------------------- | ------------------------------------------------- |
| Gmail client secret    | `ABMX_GMAIL_CLIENT_SECRET`    | `client_secret` / `client_secret_file` / `credentials_file` |
| Actual API key         | `ABMX_ACTUAL_API_KEY`         | `api_key` / `api_key_file`                        |
| Actual budget password | `ABMX_ACTUAL_BUDGET_PASSWORD` | `budget_password` / `budget_password_file`        |

## CLI

```
abmx init [--path FILE] [--force]   write a starter config
abmx setup [--manual] [--review] [--remap]
                                      interactive config, Gmail, labels, accounts
abmx auth gmail [--manual]          OAuth2 + PKCE; --manual for a headless box
abmx auth logout [--no-revoke]      revoke the refresh token, then remove it locally
abmx status [--offline] [--strict] [--check] [--json]
                                      full health check; --offline skips network
abmx samples [--label L|PARSER] [--samples N] [--out DIR] [--json]
                                      download sample emails for parser authoring
abmx sync [--dry-run] [--json] [--diff]
abmx sync --from-dir DIR [--label L] [--limit N]
                                      fetch, parse, and write the ledger
abmx reset [--purge] [--yes]        forget delivered messages
abmx accounts [--json]              sink accounts and how [labels.accounts] maps onto them
abmx skill                          print the parser-authoring skill to stdout
abmx dump <path> [--json]           normalized block dump (authoring aid)

abmx parser paths [--json]          parser search order and which directory won
abmx parser specs [--format F]      loaded parsers and their files
abmx parser schema                  JSON Schema of the transaction contract
abmx parser parse <path> --parser ID [--format json|ndjson|table]
                                      parse a file or directory with one parser
abmx parser explain <path> --parser ID
                                      block regions behind each field
abmx parser lint [--parser ID]      static spec checks
abmx parser check [-v] [--parser ID]
                                      verify every parser against its fixtures
abmx parser init <id>               scaffold parsers/<id>/{config.toml,fixtures}
```

Every `abmx parser` command takes `--parsers <dir>` (repeatable) or
`$ABMX_PARSERS`; implicit discovery is `~/.config/abmx/parsers`, then
`/etc/abmx/parsers`. The working directory is never read. Fixtures are always
read from the winning parser's own `fixtures/` directory.

**stdout is data, stderr is diagnostics.** `sync --dry-run --json` writes the
plan JSON to stdout and the human summary to stderr, so it pipes cleanly:

```bash
abmx sync --dry-run --json > plan.json
```

`--diff` (with `--dry-run`) additionally reads the ledger and reports rows that
are already present, including any imported under a foreign `imported_id`.

`--from-dir` reads saved `.eml` files instead of calling Gmail; with a flat
directory it needs `--label`, unless exactly one parser is configured.

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | clean |
| `1` | the tool failed (config, auth, network, state) |
| `2` | one or more routed labels need a parser or account fix |

`sync --dry-run` exits `2` when a label is broken, so a timer or healthcheck can
alert without writing anything. A normal `sync` writes no rows at all when any
label is broken: the summary names the label and the next command.

## Actual Budget specifics

Actual has no REST API of its own — the official client is a Node library that
mutates a local copy of the budget. `abmx` talks to `actual-http-api`, which
wraps that library and exposes the reconciling import endpoint. Two details that
matter:

- **`amount` is value × 100 for every currency.** Rp 27,500 is sent as
  `2_750_000`. `abmx` does this conversion in the sink, with tests.
- **`imported_id` is the parser's id**, and `reimportDeleted` is `true`: the
  ledger is a projection of the mailbox, so a full sync recreates rows you
  deleted. Delete the email to make a transaction stay gone.
- **Rows are imported uncleared** (`cleared = "never"`). The parser's `settled`
  means the bank completed the transaction; Actual's `cleared` means you
  reconciled it against a statement. Set `cleared = "settled"` if you want the
  old behaviour.

## Security

- Gmail scope is `gmail.readonly`. Nothing else is requested.
- The refresh token is written with `0600` and never logged.
- The OAuth flow uses PKCE and a `127.0.0.1` redirect; there is no out-of-band
  flow (Google retired it).
- Ledger credentials are separate from mailbox credentials and live in different
  files.
- `abmx` reads mail and writes to a ledger. It never deletes mail and never
  deletes ledger rows.

## Testing

```bash
just test     # unit + integration + end-to-end, no network, no credentials
just check    # fmt + clippy -D warnings
just fixtures # every parser against its committed samples
just ci       # all of the above
```

The end-to-end test (`crates/abmx/tests/e2e.rs`) runs a real parse over a real
`.eml`, against a fake Gmail API and a fake `actual-http-api`, and asserts the
exact JSON body that reaches Actual — including the ×100 amount conversion.

## NixOS

```nix
{
  inputs.abmx.url = "git+ssh://git@github.com/kennethlieyanto/actual-gmail-tx-sync";

  services.abmx = {
    enable = true;
    configFile = /etc/abmx/config.toml;
    interval = "15min";
    secrets = {
      "actual-api-key" = config.age.secrets.actual-api-key.path;
      "gmail-client-secret" = config.age.secrets.gmail-client-secret.path;
    };
  };
}
```

Credentials land in `/run/credentials/abmx.service/<name>`, so the config refers
to them as `api_key_file = "/run/credentials/abmx.service/actual-api-key"`.

## Out of scope, by design

- Parsing bank templates in code. Templates are runtime TOML specs.
- Statement import and balance reconciliation. Planned; see "Why mail, not
  statements?" above.
- Transfers linking (importing both sides of a transfer as one).
- Pending → settled reconciliation. Today the parser emits no `pending` rows;
  when it does, this needs a matching rule, not a heuristic.
- Multi-currency conversion. `amount_minor` and `currency_exponent` are carried
  through unchanged.
- Webhooks and push sources. The `Sink` trait is push-shaped, but nothing calls
  it from a request handler yet.

## Layout

```
crates/abmx/         config, SQLite state, runner, CLI, AI authoring
crates/abmx-core/    traits and types. No I/O, no network.
crates/abmx-gmail/   Fetcher: Gmail API + OAuth2/PKCE
crates/abmx-actual/  Sink: actual-http-api
crates/abmx-file/    Sink: append-only JSONL (reference implementation)
crates/abmx-testkit/ a blocking HTTP mock server for tests
crates/abmx-tx/      the Transaction contract
crates/abmx-mail/    MIME and HTML normalization
crates/abmx-parse/   the declarative parser engine
crates/abmx-parser/  corpus discovery, routing, schema
parsers/             the bank corpus: <id>/config.toml + <id>/fixtures/
xtask/               fixture redaction helper
docker-compose.yml   actual-http-api wrapper for local testing
```

MIT.
