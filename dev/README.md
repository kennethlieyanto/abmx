# Development sandbox

`just dev-up` starts a **throwaway** Actual Budget server on
`http://localhost:5006`. Everything it writes lives in the `actual-data` Docker
volume, so `just dev-reset` deletes the whole budget and starts over. Never point
a statement sync at the server you actually use.

## First run

```bash
just dev-up
# open http://localhost:5006, create a budget named "dev", then:
#   Settings -> General -> Security -> API keys        -> create a key
#   Settings -> Show advanced settings                 -> copy the Sync ID
cp dev/abmx.toml.example dev/abmx.toml
# fill in budget_sync_id and a file next to abmx.toml with the API key
```

`dev/abmx.toml` is ignored by git, as is any password file beside it.

```bash
just dev-accounts   # account UUIDs from the stub
just dev-check      # parser fixtures against your private corpus
just dev-statements # statement sync against the stub (dry run first)
```

## Why not the real server?

`importTransactions` writes. A statement run also reads the ledger to decide
what is new, so a test against the real budget can both add rows and draw wrong
conclusions from your real history. The stub keeps both concerns local and
disposable.

## Tests

`just test` never touches Docker or the network. The opt-in statement tests are:

```bash
ABMX_STATEMENT_SAMPLES=... cargo test -p abmx-doc --test real
ABMX_STATEMENT_SAMPLES=... cargo test -p abmx-table --test real
```

Those read real PDFs from outside the repository; nothing in the fixtures is
committed.
