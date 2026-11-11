# Architecture

`txsync` is four seams and one pipeline. The seams exist so that a new mailbox or
a new ledger is a new crate, not a change to the runner.

```
             ┌──────────────── txsync-core ────────────────┐
             │  Fetcher · Sink · Policy · Plan · Report     │
             └───────▲─────────────────────────────▲────────┘
                     │                             │
        txsync-gmail │                             │ txsync-actual
        (Gmail API)  │                             │ (actual-http-api)
                     │                             │
             ┌───────┴─────────────────────────────┴────────┐
             │  txsync: config · SQLite state · runner · CLI │
             └───────────────────▲───────────────────────────┘
                                 │
                          txp (library)
                      .eml bytes → Transaction
```

## Why not mirror `txp`'s stages?

`txp` has input → router → parser → output because parsing is its hard problem
and bank templates churn weekly. `txsync`'s hard problem is different:
**transport, identity, and state**. Copying the stages would give symmetry but
the wrong abstraction — `txsync` never routes, and its "output" is not the
product; the *write* is.

| `txp` | `txsync` | Difference |
| --- | --- | --- |
| input | **Fetcher** | same role, transport varies per user |
| router | — | routing is parsing |
| parser spec | `txp` itself | not reimplemented |
| output schema | **Policy → Sink** | the decision and the write are separate |
| — | **State** | no equivalent in `txp`, and where the correctness lives |

## The primitives

### Fetcher

```rust
pub trait Fetcher {
    fn name(&self) -> &str;
    fn fetch(&self, request: FetchRequest<'_>) -> Result<FetchBatch>;
}
```

- **Synchronous.** `txsync` is a batch process; `txp` is synchronous; a sync
  trait is usable from an async caller via `spawn_blocking`, and the reverse is
  not. This also keeps `txp` calls out of a runtime.
- **Object-safe.** `Box<dyn Fetcher>` means the runner never grows a match arm
  per source.
- **Bytes, not transactions.** A fetcher produces RFC 5322 bytes. Parsing is
  always `txp`'s job.
- **Opaque cursor.** Gmail's is a `historyId`; a directory's is `None`. Typing it
  as `serde_json::Value` keeps the trait object-safe and the state schema-free.

### Policy

```rust
pub fn decide(tx: &Transaction, policy: &PolicyConfig, accounts: &AccountMap) -> Decision;
```

A pure function: no clock, no I/O, no randomness. Same input, same decision —
which is what makes plans reproducible and review queues meaningful. The whole
decision layer is tested by a truth table, not by integration tests.

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

### State

SQLite, not a JSON blob:

```sql
meta      (key, value)                       -- txp schema version, tool version
cursors   (name, value, updated_at)          -- gmail historyId, per fetcher
messages  (key, sink, tx_id, disposition, seen_at)
review    (tx_id, message, reason, payload, created_at)
```

What is **not** stored: which transactions the sink accepted. That belongs to an
idempotent sink. `messages` records what `txsync` itself delivered.

## Plan / apply

`plan` reads state and writes nothing. `apply` is the only writer. That split
gives you, for free:

- `--dry-run` (print the plan, apply nothing),
- review-before-write (`plan > plan.json`, inspect, then `apply --plan`),
- replay after a sink outage,
- deterministic test fixtures.

`Plan` carries the full transaction, not a reference, so it survives without the
mailbox. It also carries `txp_schema_version` and its own `PLAN_VERSION`; both are
checked before applying.

## Cursor discipline

The rule: **advance the cursor only after every row in the batch succeeded.**

```
fetch ──► parse ──► decide ──► plan ──► sink.apply ──┬─ all rows ok ──► save cursor
                                                    └─ any error ─────► keep cursor
```

A crash or a partial failure re-fetches a handful of messages. The state database
skips what already landed, and an idempotent sink would ignore duplicates anyway.
Advancing early would silently drop mail — the wrong failure mode.

For Gmail specifically:

- A full sync reads `profile.historyId` **before** listing, so anything arriving
  during the listing appears in the next `history.list`.
- A truncated batch does not advance the cursor at all.
- An expired history window (404) falls back to a full sync.

## Adding a fetcher

1. New crate, `txsync-<name>`, depend on `txsync-core`.
2. Implement `Fetcher`. Return `RawMessage`s with a stable key prefixed by the
   transport (`imap:<uid>`, `dir:<name>`) so keys never collide in shared state.
3. Add a builder in `txsync/src/config.rs` and one arm in `build_fetcher`.
4. Test with `txsync-testkit`: a mock HTTP server for API sources, or a temp
   directory for file sources.

Do not add a plugin ABI. Two implementations is the threshold for extracting a
trait; one is a guess.

## Adding a sink

1. New crate implementing `Sink`.
2. Answer `idempotent()` honestly. If the target has no idempotency key, say
   `false` and let the runner track deliveries.
3. Do all vendor quirks — currency scaling, payee normalization, field renames —
   inside the sink. `txsync-core` must stay vendor-neutral.
4. Return a `Report` with per-row errors rather than failing the batch.

## Dependency on `txp`

`txsync-core` depends on `txp` (the facade) and `txp-core` (the contract types).
The second dependency exists because `txp` does not currently re-export
`Transaction`; when it does, one `use` line changes.

The contract is guarded at runtime: `txp::SCHEMA_VERSION` is written into the
state database on first use, and a mismatch refuses to run. A parser-contract bump
must be a deliberate migration, not a silent reinterpretation of old rows.

## What is deliberately not here

- A plugin loader, dynamic dispatch by name, or a sink DSL.
- An async runtime.
- Generic input framing. The extension point for non-email sources is the
  `Transaction` schema, not `txp`'s input layer.
