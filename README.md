# abmx

Sync bank transaction emails into an Actual Budget ledger — automatically.

`abmx` fetches bank notification mail, parses it with a parser corpus you can
teach without rebuilding, decides what is safe to import, and writes it to a
ledger. It never guesses: an unsure parse, an unmapped account, or a row the
bank reported as failed lands in a review queue instead.

```
Gmail ──► abmx ──► parser corpus ──► policy ──► plan ──► Actual Budget
         fetch     .eml → Transaction   decide    diff     apply
```

```
$ abmx sync
fetched 15, parsed 12 — import 12, review 0, skip 0, unmatched 3, failed 0

date                 amount account        decision counterparty
2026-11-12     -128,000 IDR example-account    import   EXAMPLE PAYEE
2026-11-11     -124,300 IDR example-account    import   EXAMPLE MERCHANT
...

unmatched (3), no parser claimed them:
  2026-11-06  "Login with Biometrics Activation Notification"   example@example.invalid
  2026-11-06  "New Device Accessing myBank"                      example@example.invalid
  2026-11-06  "Your example ID Password has been changed"           example@example.invalid

applied: added 12, updated 0, skipped 0, errors 0
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
abmx init                       # writes ~/.config/abmx/config.toml
$EDITOR ~/.config/abmx/config.toml   # Gmail client id + Actual endpoint
abmx auth gmail                 # OAuth2 + PKCE, stores a refresh token (0600)
abmx labels                     # inspect the abmx:* labels and their parsers
abmx author                     # build a prompt for labels without a parser
abmx sync                       # import everything labeled
```

Routing is explicit. The user applies exactly one `abmx:<id>` label to a tracked
message (a Gmail filter is the usual way), and `[labels]` in the config maps that
label to a parser and a ledger account:

```toml
[labels]
"abmx:example-qris" = { parser = "example-qris", account = "<actual account uuid>" }
```

`abmx author` is the one step with a human in the loop. It takes the **last
three** messages under every `abmx:*` label, writes them to
`~/.local/state/abmx/author/<timestamp>/`, and emits a prompt with their block
dumps:

```bash
opencode --prompt "$(abmx author)"
```

```
5 label(s), 15 sample(s); 1 need work
  abmx:example-qris            missing_parser        3  -
  abmx:example-journal         ok                    3  example-journal
```

The agent writes one parser per label, asks which ledger account it belongs to,
and verifies all three samples. If the three are different templates, the label
is mislabeled and the agent stops instead of guessing. Nothing to install,
nothing to rebuild.

Already have `.eml` files? `abmx author --input <dir>` does the same offline.
To make the next one a one-liner, install the skill once:

```bash
abmx generate skill parser --global
```

Then put `abmx sync` on a timer (see the NixOS module below). `sync` refuses to
run when an `abmx:*` label has no parser or mapping, so nothing is silently
skipped.

### You need

1. **A Gmail OAuth client** — one Google Cloud project, `gmail.readonly`, desktop
   app client. See [docs/gmail.md](docs/gmail.md).
2. **A ledger endpoint** — for Actual Budget that is
   [`actual-http-api`](https://github.com/jhonderson/actual-http-api) on `:5007`.
   From a checkout: `cp .env.example .env`, fill in the Actual password and API
   key, then `just actual-api`. See [docs/actual.md](docs/actual.md).
3. **Parsers** — nothing to install. `abmx author` creates them under
   `~/.config/abmx/parsers/<id>/` (`config.toml` plus `fixtures/`). If you
   already have a corpus (this repository ships one), point at it with
   `--parsers`, `$ABMX_PARSERS`, or `[general].parsers`, or run
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

Parsers are runtime data: a `config.toml` per bank template, loaded from a
directory. Adding one does not require rebuilding or writing code. The spec
format is documented in [docs/parsing.md](docs/parsing.md).

`abmx author` does the packaging:

- **inventory** — list every `abmx:*` Gmail label and whether it has a parser and
  an account.
- **sample** — fetch the last three messages under each label, plus the variance
  table that tells the agent which blocks are labels and which are values.
- **refuse to guess** — a label with fewer than three samples is
  `insufficient_samples`; a label whose samples are different templates is
  `mixed_shapes` and the agent is told to fix the label, not split the parser.

The agent writes each parser straight into `~/.config/abmx/parsers/<id>/` with
its fixtures and verifies it against all three samples.

```bash
opencode --prompt "$(abmx author)"
```

Options: `--label abmx:example-qris` restricts it to one label, `--samples N`
changes the evidence depth, `--json` emits the machine-readable bundle, and
`--input <dir>` works offline.

### The verification loop

The rules are the same whatever produced the draft. `abmx parser check` verifies
every parser against its colocated fixtures; `abmx verify parser <dir>` checks
one candidate against a directory of samples; `abmx verify parser --label`
checks the live last-three under a label. A spec is only done when every sample
is `strict`; `heuristic` means a zero amount or a missing field, and a missing
parser means the label and the corpus disagree. `abmx parser explain` shows the
region behind each field, and `abmx parser lint` catches the common spec
mistakes before a sync.

```bash
abmx parser check --parser example.card_debit
abmx verify parser ~/.config/abmx/parsers/example.card_debit
```

### Manual prompt

If you want to drive the authoring yourself, `abmx author --input` renders a
prompt from a directory of `.eml` files (one subdirectory per label):

```bash
abmx author --input ./eml-dir --out ./prompt.md
opencode run --auto --file ./prompt.md "Follow the prompt."
```

### Reusable skills

For recurring authoring, install the skills once:

```bash
abmx generate skill parser --global   # ~/.config/opencode/skills
abmx generate skill parser --out .opencode/skills
```

Then the prompt is simply "author the parsers for these emails"; the skill knows
the corpus layout, the verification loop, and the account-mapping rules.

## The four primitives

| Primitive   | Responsibility                            | Ships with                 |
| ----------- | ----------------------------------------- | -------------------------- |
| **Fetcher** | a source of raw RFC 5322 bytes            | Gmail API, local directory |
| **Policy**  | `Transaction` → import / review / skip    | one pure function          |
| **Sink**    | decided rows → the ledger                 | Actual Budget, JSONL file  |
| **State**   | delivered messages, review queue          | SQLite                     |

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
3. **Label scan + state** — every run scans the configured `abmx:*` labels and
   lets the state database skip messages already delivered. Mail labeled late is
   picked up on the next run; a partial failure leaves the failed message unseen
   so it is retried.

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
"abmx:example-journal" = { parser = "example-journal", account = "<actual account uuid>" }
"abmx:example-qris"    = { parser = "example-qris",    account = "<actual account uuid>" }
"abmx:grab"        = { parser = "grab",        account = "<actual account uuid>" }

[policy]
heuristic = "review"               # review | import | skip
failed = "skip"                    # skip | review | import
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

| Secret                 | Environment                   | Config field                               |
| ---------------------- | ----------------------------- | ------------------------------------------ |
| Gmail client secret    | `ABMX_GMAIL_CLIENT_SECRET`    | `client_secret` / `client_secret_file`     |
| Actual API key         | `ABMX_ACTUAL_API_KEY`         | `api_key` / `api_key_file`                 |
| Actual budget password | `ABMX_ACTUAL_BUDGET_PASSWORD` | `budget_password` / `budget_password_file` |

## CLI

```
abmx init [--path FILE] [--force]   write a starter config
abmx auth gmail [--manual]          OAuth2 + PKCE; --manual for a headless box
abmx auth logout [--no-revoke]      revoke the refresh token, then remove it locally
abmx doctor [--online] [--strict]     check config, parsers, labels, state, sink, auth
abmx labels [--check] [--json]      every abmx:* label, its parser, and its account
abmx author [--label L] [--samples N] [--json] [--input DIR] [--out FILE]
                                      inventory labels and emit a prompt for the ones needing work
abmx plan [--from-dir DIR] [--out FILE]
                                      fetch + parse + decide, print the plan as JSON
abmx apply [--plan FILE] [--dry-run]
                                      write a plan (current or saved)
abmx sync [--dry-run]               plan + apply; the timer entrypoint
abmx diff [--json] [--since D] [--until D]
                                      compare the plan against what the ledger already holds
abmx reset [--review] [--purge] [--yes]
                                      forget delivered messages
abmx review [--json]                rows waiting for a human
abmx approve <id>… [--dry-run]      import reviewed rows
abmx accounts [--json]              sink accounts and how [labels] maps onto them
abmx parsers                        loaded parsers and the labels they serve
abmx dump <path> [--json]           normalized block dump (authoring aid)
abmx verify parser <dir|config.toml> [--id ID] [--input DIR] [--json]
                                      check a candidate parser against samples
abmx verify parser <dir> --label abmx:example-qris [--all] [--limit N]
                                      check it against the live last-three
abmx generate skill parser …        write the OpenCode authoring skills

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

**stdout is data, stderr is diagnostics.** `abmx plan` writes the plan JSON to
stdout and the human summary to stderr, so it pipes cleanly:

```bash
abmx plan --from-dir ./mail > plan.json
abmx apply --plan plan.json --dry-run
```

`--from-dir` reads saved `.eml` files instead of calling Gmail. It is the fastest
way to test a config, and the recovery path if a mailbox provider breaks.

### Exit codes

| Code | Meaning                                                                                                                                    |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `0`  | clean                                                                                                                                      |
| `1`  | the tool failed (config, network, state)                                                                                                   |
| `2`  | at least one message or row failed                                                                                                         |
| `3`  | `plan` / `review` / `diff` only: rows are waiting for review, or `diff` found a row the ledger already holds under a foreign `imported_id` |

`sync` and `apply` return `0` when the only outstanding work is a review row —
a timer that fails every 15 minutes because one heuristic email is waiting is a
timer nobody reads. Use `abmx review` (exit `3` when non-empty) to detect them.

`abmx parser` follows the parser's own convention: `0` all strict, `2` unmatched
or failed, `3` heuristic unless `--allow-heuristic`.

## Policy

| Situation                   | Default | Why                                   |
| --------------------------- | ------- | ------------------------------------- |
| `status = failed`           | skip    | the bank says it did not happen       |
| `extraction = heuristic`    | review  | the amount is plausible, not verified |
| zero amount                 | review  | almost always the wrong block matched |
| label not in `[labels]`     | review  | never guess a ledger account          |
| everything else             | import  |                                       |

Review rows are **re-checked on every sync**, even though their messages are
in the review queue: the queue stores the transaction, so fixing `[labels]` or
`[policy]` imports them on the next run — no reset and no per-row approval.
`abmx approve` is only for forcing a row the policy still rejects.

`plan` and `--dry-run` do not record the queue (a dry run must not write state),
so they say "would be held for review" rather than pointing at `abmx review`.

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
