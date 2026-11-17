# Architecture

`abmx` is one project with two foundations and one pipeline.

- **Parsing** is data-extensible: a new bank is a TOML spec, loaded at runtime,
  no rebuild.
- **Syncing** is code-extensible: a new mailbox or ledger is a crate and a trait
  implementation.

That asymmetry is deliberate. Bank templates churn weekly and are a regex table;
mailboxes and ledgers carry credentials, pagination, retries, and vendor quirks.
Forcing both through the same extension mechanism would be worse than keeping
them apart.

```
┌──────────────── sync foundation ────────────────┐
│  Fetcher · Sink · Policy · Plan · Report         │  abmx-core
└───────▲──────────────────────────────▲──────────┘
        │                              │
 abmx-gmail                      abmx-actual
 (Gmail API)                     (actual-http-api)
        │                              │
┌───────┴──────────────────────────────┴──────────┐
│  abmx: config · SQLite state · runner · CLI      │
└───────────────────────▲─────────────────────────┘
                        │
┌────────────────── parser foundation ─────────────┐
│  abmx-parser  discovery · routing · schema       │
│  abmx-parse   the declarative spec engine        │
│  abmx-mail    MIME + HTML → blocks               │
│  abmx-tx      the Transaction contract           │
└──────────────────────────────────────────────────┘
```

The seams on the sync side exist so that a new mailbox or ledger is a new crate,
not a change to the runner. The parser is a library the runner calls; it is not
a second process.

## Why not mirror the parser's stages?

The parser has input → label → template → output because parsing is its hard
problem. The synchronizer's hard problem is different: **transport, identity, and
state**. Copying the stages would give symmetry but the wrong abstraction —
`abmx` never routes, and its "output" is not the product; the _write_ is.

| Parser            | Sync                | Difference                                              |
| ----------------- | ------------------- | ------------------------------------------------------- |
| input             | **Fetcher**         | same role, transport varies per user                    |
| label routing     | gmailctl + `[labels.accounts]` | the label names the parser; the config maps it to an account |
| parser spec       | `abmx-parser`       | not reimplemented                                       |
| extraction result | `abmx-tx` contract  | the shared vocabulary between the halves                |
| output schema     | **Policy → Sink**   | the decision and the write are separate                 |
| —                 | **State**           | no equivalent in the parser, and where correctness lives |

## The primitives

### Fetcher

```rust
pub trait Fetcher {
    fn name(&self) -> &str;
    fn fetch(&self, request: FetchRequest<'_>) -> Result<FetchBatch>;
}
```

- **Synchronous.** `abmx` is a batch process; a sync trait is usable from an
  async caller via `spawn_blocking`, and the reverse is not. This also keeps
  parser calls out of a runtime.
- **Object-safe.** `Box<dyn Fetcher>` means the runner never grows a match arm
  per source.
- **Bytes, not transactions.** A fetcher produces RFC 5322 bytes. Parsing is
  always the parser's job.
- **Label-tagged messages.** Each `RawMessage` carries the `receipt/*` label that
  selected it, so the caller chooses the parser from the label and the account from `[labels.accounts]` without guessing.

### Classification

```rust
pub fn check_transaction(tx: &Transaction, accounts: &AccountMap) -> Result<(), BrokenReason>;
```

A pure function: no clock, no I/O, no randomness. It answers one question — is
this labeled row safe to write? `BrokenReason` names the defect (`heuristic`,
`zero_amount`, `unmapped_account`, `bank_declared_failed`), which is what makes
the failure actionable. There is no per-row human queue.

### Sink

```rust
pub trait Sink {
    fn name(&self) -> &str;
    fn idempotent(&self) -> bool { false }
    fn accounts(&self) -> Result<Vec<SinkAccount>>;
    fn apply(&self, rows: &[ImportRow], options: &ApplyOptions) -> Result<Report>;
}
```

- **`idempotent()` is the important one.** It asks: does the sink deduplicate on
  `tx_id`? Actual does (on `imported_id`). A file writer does not. The runner only
  maintains applied-state when the sink cannot do it itself — otherwise you build
  a second source of truth that drifts.
- **Push and pull are not separate primitives.** A pull sink (JSONL, QIF) writes
  an artifact instead of calling an API. Same `apply`.
- **Partial success is a `Report`, not an error.** A sink that imported 49 of 50
  rows should say so. The Actual sink bisects a failing chunk into single rows so
  one bad row cannot cost the rest.
- **`snapshot()` is the read side.** `abmx sync --dry-run --diff` needs to know what the ledger
  already holds, and only the sink knows how to ask. The method is read-only by
  contract, and a sink that cannot answer returns an error rather than an empty
  list, so "no matches" is never confused with "I cannot see".

### State

SQLite, not a JSON blob:

```sql
meta      (key, value)                       -- parser schema version, tool version
messages  (key, sink, tx_id, disposition, seen_at)
```

What is **not** stored: which transactions the sink accepted. That belongs to an
idempotent sink. `messages` records what `abmx` itself delivered.

The model, in one line: **the mailbox is the source of truth, the ledger is a
projection, and this database is a cache.** Losing the cache costs a re-fetch,
never correctness — `imported_id` makes re-application idempotent. Deleting a
projected row is drift, and a full sync recreates it; to make something stay
gone, remove the source (`abmx reset` forces the rebuild).

## Plan / apply

Planning reads the delivered set; it never records progress. Applying is the
only writer. That split gives you, for free:

- `--dry-run` (print the plan, write nothing),
- `--json` (the plan on stdout, the summary on stderr),
- `--diff` (compare the plan with the ledger without applying),
- deterministic test fixtures.

`Plan` carries the full transaction, not a reference, so it survives without the
mailbox. It also carries `abmx_schema_version` and its own `PLAN_VERSION`; both
are checked before applying.

## Label discipline

There is no cursor. Every run lists the account's labels, keeps the
`receipt/*` ones that are mapped in `[labels.accounts]`, paginates the
messages under each one, and lets the state database skip what was already
delivered.

```
list labels ──► keep [labels.accounts] ────────► list messages per label
      │                                      │
      │ mapped id has no Gmail label         ▼
      └──────────── abort ─────────── fetch raw + tag label
                                             │
                                             ▼
                               parse with the label's parser
                                             │
                              not strict ────┴──► abort the run
                                             │
                                             ▼
                                   decide ──► apply
```

Why this is safer than a cursor:

- Mail labeled late is picked up on the next run; a cursor would have skipped it.
- A partial failure leaves the failed message unseen, so it is retried.
- An unmapped Gmail label is ignored, not a sync failure. A *mapped* id with no
  Gmail label, a missing parser, or a message carrying two managed labels
  aborts the run instead of silently importing noise or skipping it.
- A cap is a hard error, never a silent truncation.

The cost is listing message ids per label on every run. That is cheap; raw
bodies are fetched only for messages the state has not seen.

## Adding a fetcher

1. New crate, `abmx-<name>`, depend on `abmx-core`.
2. Implement `Fetcher`. Return `RawMessage`s with a stable key prefixed by the
   transport (`imap:<uid>`, `dir:<name>`) and the `receipt/*` label that selected
   them, so keys never collide in shared state.
3. Add a builder in `abmx/src/config.rs` and one arm in `build_fetcher`.

## Adding a fetcher

1. New crate, `abmx-<name>`, depend on `abmx-core`.
2. Implement `Fetcher`. Return `RawMessage`s with a stable key prefixed by the
   transport (`imap:<uid>`, `dir:<name>`) so keys never collide in shared state.
3. Add a builder in `abmx/src/config.rs` and one arm in `build_fetcher`.
4. Test with `abmx-testkit`: a mock HTTP server for API sources, or a temp
   directory for file sources.

Do not add a plugin ABI. Two implementations is the threshold for extracting a
trait; one is a guess.

## Adding a sink

1. New crate implementing `Sink`.
2. Answer `idempotent()` honestly. If the target has no idempotency key, say
   `false` and let the runner track deliveries.
3. Do all vendor quirks — currency scaling, payee normalization, field renames —
   inside the sink. `abmx-core` must stay vendor-neutral.
4. Return a `Report` with per-row errors rather than failing the batch.

## Adding a bank

No code. Write `parsers/<id>/config.toml`, add samples under
`parsers/<id>/fixtures/`, and run `abmx parser check --parser <id> --parsers
parsers`. `abmx parser init`, `infer`, `generate`, and `verify` exist to make
that loop fast. See [parsing.md](parsing.md) for the spec format and
[authoring.md](authoring.md) for the AI-assisted workflow.

The corpus is runtime data: `abmx` searches `--parsers`, `$ABMX_PARSERS`,
`~/.config/abmx/parsers`, then `/etc/abmx/parsers`. It never reads the working
directory. The Nix package bundles the corpus and defaults `ABMX_PARSERS` at it.

## The parser contract

`abmx-tx` owns the `Transaction` type shared by both halves. `abmx-parser`
re-exports it, and `abmx-core` depends on it directly. The contract is guarded at
runtime: `abmx-parser::SCHEMA_VERSION` is written into the state database on
first use, and a mismatch refuses to run. A contract bump must be a deliberate
migration, not a silent reinterpretation of old rows.

## What is deliberately not here

- A plugin loader, dynamic dispatch by name, or a sink DSL.
- An async runtime.
- A second binary. The parser is a library plus a `abmx parser` command group;
  it is not a separate product with its own release cadence.
- Generic input framing. The extension point for non-email sources is the
  `Transaction` schema, not the parser's input layer.
