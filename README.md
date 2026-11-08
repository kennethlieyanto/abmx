# txp

Parse Indonesian bank transaction emails into structured JSON.

Indonesian consumer bank accounts rarely have a usable API. Everyone ends up
with their own scraper or regex hack. `txp` is the shared piece: **`.eml` bytes
in, one normalized transaction out**, with a documented contract.

It does not fetch mail. It does not write to a ledger. It parses, and nothing
else.

```
$ txp parse statement.eml
[
  {
    "parser": "example.card_debit",
    "id": "mail_a631e3d89a232b55",
    "bank": "example",
    "account": "example-account",
    "kind": "purchase",
    "status": "settled",
    "amount_minor": -27500,
    "currency": "IDR",
    "currency_exponent": 0,
    "amount": "-27500",
    "occurred_at": "2026-10-30T17:28:17+07:00",
    "occurred_on": "2026-10-30",
    "counterparty": "ExamplePay",
    "account_last4": "4821",
    "channel": "DEBIT ONLINE",
    "extraction": "strict",
    "warnings": []
  }
]
```

## Install

The binary and the parser corpus ship as separate artifacts:

```bash
# binary
cargo install --path crates/txp          # from a checkout
# or download txp-<target>.tar.gz from a release

# parser corpus, independently versioned
tar xzf txp-parsers-<version>.tar.gz -C ~/.config/txp
```

### Nix

The flake provides a dev shell, the `txp` binary, and a build with the parser
corpus bundled:

```bash
nix develop            # rust, cargo, clippy, rustfmt, just
nix build .#           # ./result/bin/txp
nix run .# -- paths    # run the packaged binary
```

The packaged binary defaults `TXP_PARSERS` to its bundled corpus
(`$out/share/txp/parsers`). An explicit `--parsers` or `$TXP_PARSERS` still
takes precedence, so a local corpus can be layered on top.

As a Rust library:

```toml
txp = "0.1"
```

## Where parsers come from

Nothing is compiled into the binary. Specs are runtime configuration, resolved
in this order:

| Order | Source                         | Notes                                                           |
| ----- | ------------------------------ | --------------------------------------------------------------- |
| 1     | `--parsers <dir>` (repeatable) | all entries are **merged** by parser `id`; earlier wins         |
| 2     | `$TXP_PARSERS`                 | colon-separated; same merge semantics                           |
| 3     | `./parsers`                    | first **existing** directory wins; error if it has no specs     |
| 4     | `$XDG_CONFIG_HOME/txp/parsers` | `~/.config/txp/parsers`; only consulted if nothing above exists |
| 5     | `/etc/txp/parsers`             | containers and system installs; same rule                       |

Implicit discovery uses **one** corpus, deliberately: a stale user-level parser
can never silently contribute half your registry. To layer a local override on
top of a base corpus, say so explicitly with `--parsers` or `$TXP_PARSERS`.

Router policy resolves as `--router` → `$TXP_ROUTER` → `router.toml` inside the
first parser directory → built-in defaults.

`txp paths` shows the chain, which entries exist, and which one won:

```
$ txp paths
used      cwd     parsers
skipped   xdg     /home/you/.config/txp/parsers
missing   system  /etc/txp/parsers

env: TXP_PARSERS unset
router: parsers/router.toml (corpus)
note: 1 lower-precedence corpus ignored (first hit wins); pass --parsers to layer
```

`txp specs` shows what actually loaded and which file each parser came from.

## CLI

```bash
txp parse statement.eml                 # JSON array (default)
txp parse archive/ --format ndjson      # one object per line
txp parse archive/ --format table       # human readable
txp parse archive/ --account example-account # one account per run
txp parse archive/ --allow-heuristic    # exit 0 even with heuristic rows

txp paths                               # parser search order
txp specs --format table                # loaded parsers and their files
txp senders                             # sender addresses covered
txp schema                              # JSON Schema of the output contract
txp dump statement.eml                  # normalized blocks (authoring aid)
txp route statement.eml                 # which parser wins, and why
txp check -v                            # verify the fixture corpus
txp infer new-samples/ --id example.promo   # draft a parser spec
txp init example.promo                      # scaffold parsers/example.promo.toml
```

Exit codes matter for pipelines: `0` clean, `2` unmatched/failed, `3` heuristic
present, `1` tool error. Diagnostics go to stderr, so stdout is always just data.

### `just`

A `justfile` wraps the same commands:

```bash
just                      # list recipes
just txp parse statement.eml
just paths                # parser search order
just fixtures             # verify every parser against its fixtures
just ci                   # fmt + clippy + tests + fixtures
just schema
just redact               # sample/ -> build/redacted
just infer sample/example/promo example.promo
```

## Library

```rust
// Resolve the default corpus ($TXP_PARSERS, ./parsers, XDG, /etc).
let outcome = txp::parse_bytes(&eml_bytes)?;

match outcome {
    txp::Outcome::Parsed(tx) => {
        println!("{} {} {}", tx.occurred_on, tx.amount.amount_minor, tx.amount.currency);
        assert_eq!(tx.extraction, txp::Extraction::Strict);
    }
    txp::Outcome::Unmatched(reason) => eprintln!("not ours: {reason}"),
    txp::Outcome::Failed { parser, error } => eprintln!("{parser}: {error}"),
}
```

Explicit control:

```rust
let (parser, discovery) = txp::Parser::load(&txp::Options {
    parsers: vec!["/etc/txp/parsers".into()],
    router: None,
})?;

for candidate in &discovery.candidates {
    eprintln!("{:?} {} has_specs={}", candidate.source, candidate.path.display(), candidate.has_specs);
}

let results = parser.parse_path("emails/")?;
```

`txp::discover(&options)` returns the resolution result without reading specs.
`txp::schema()` returns the JSON Schema. `txp::specs()` lists the default corpus.

## Supported banks

| Bank              | Parsers                                                            |
| ----------------- | ------------------------------------------------------------------ |
| example               | `card-debit`, `journal-qris`, `journal-va`, `journal-transfer`     |
| wallet (example Digital) | `incoming-transfer`, `qris-payment`, `card-online`, `online-order` |
| banktwo (banktwo')  | `qris-payment`, `bank-transfer`                                    |

Three of the source emails contained three different layouts each, so the unit is
a **template**, not an email. The three example journal layouts share a sender _and_ a
subject; they are separated by label fingerprints (`requires` / `rejects`).

## How it works

```
.eml ──► normalize ──► route ──► extract ──► validate ──► Transaction
         txp-mail      router    TOML spec   checks
```

The body becomes an ordered list of text blocks, one per block-level HTML
element. Each parser declares every constant label in its template; the engine
partitions the blocks on those labels, so a _region_ is everything between one
label and the next. That is what makes `Rp` / `100.000` / `,00` and
`XXXX XXXX XXXX` / `4821` work without per-bank code, and what survives a CSS or
table relayout.

The engine is fixed Rust. **Every bank template is one data file.**

```
parsers/
  example.card_debit.toml        one file per parser; the file stem is the id
  ...
  router.toml                ignore rules and overrides
fixtures/
  example.card_debit/01.eml      test-only, never deployed
  ...

crates/txp-core/    Transaction, Money, id. No I/O.
crates/txp-mail/    MIME + HTML -> ordered blocks. Only crate that sees raw mail.
crates/txp-parse/   spec loading, router, region engine, decoders.
crates/txp/         facade library + the `txp` binary.
xtask/              dev tasks (fixture redaction), not published
SPEC.md             the output and spec contract
```

## Adding a parser

```bash
mkdir -p sample/<bank>/<type> && cp *.eml sample/<bank>/<type>/

just infer sample/<bank>/<type> <bank>.<type> > /tmp/draft.toml
just init <bank>.<type>
$EDITOR parsers/<bank>.<type>.toml          # paste the draft, fix the TODOs
just dump sample/<bank>/<type>/one.eml      # see the blocks
just redact sample/<bank>/<type> --out build/redacted
cp build/redacted/*.eml fixtures/<bank>.<type>/
just fixtures-one <bank>.<type>
```

`txp dump <file>` prints the block list; that is the input to every authoring
decision. See [SPEC.md](SPEC.md) for the full spec reference.

## Deploying as configuration

A flat file per parser means the corpus is a single directory of small files,
which is exactly what a ConfigMap, a PVC or an image layer wants:

```yaml
env:
  - name: TXP_PARSERS
    value: /etc/txp/parsers
volumeMounts:
  - name: parsers
    mountPath: /etc/txp/parsers
    readOnly: true
volumes:
  - name: parsers
    configMap:
      name: txp-parsers # keys: example.card_debit.toml, ..., router.toml
```

- ConfigMap keys cannot contain `/`, which is why specs are flat: `<id>.toml`.
- Mount the whole directory, not `subPath`, or updates never propagate.
- kubelet refreshes ConfigMap volumes eventually; `txp` is a one-shot process, so
  every invocation reads the directory fresh.
- ConfigMap volumes are symlink farms (`..data/` plus key symlinks); the loader
  follows them and ignores the bookkeeping entries.
- Read-only root filesystems are fine; the only write path is `txp init`.

## Fixtures and privacy

Fixtures are real emails with names and account numbers replaced by
deterministic fakes. The rules live in `redact.toml`, which is **gitignored**, as
is `sample/` — raw mail never enters git history.

`just redact` rewrites only text nodes, re-serializes a minimal HTML message, and
verifies that the block structure survives. Run it before committing any fixture.

## Out of scope, by design

- Fetching mail (Gmail, IMAP, `.eml` export — your choice, your credentials).
- Scheduling, retries, archives.
- Account mapping and ledger writes.
- Multi-currency conversion.

`id` exists so the consumer can deduplicate without inventing identity.
`amount_minor` + `currency_exponent` exist so the consumer can convert to
whatever integer format its ledger wants.

## Status

Early. Three banks, ten parsers, one country. Bank formats change, so `txp check`
is the contract that keeps the corpus honest.
