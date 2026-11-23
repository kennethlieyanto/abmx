# abmx

Sync bank transaction emails and monthly PDF statements into an Actual Budget
ledger.

**[Website and documentation](https://kennethlieyanto.github.io/abmx/)** ·
[Contributing](CONTRIBUTING.md) · [MIT](LICENSE)

```console
$ abmx sync
fetched 15, parsed 15 — import 13, broken 2, unmatched 0, failed 0
2026-11-12  -12,500 IDR  receipt/example.card_debit  EXAMPLE PAYEE
broken (2), not imported: receipt/example.journal_va  parse_failed

$ abmx statement sync --dry-run
fetched 1, parsed 7 rows — new 7, already present 0, ambiguous 0, failed 0
```

Parsers are sandboxed Rhai scripts, not compiled code. This repository ships no
bank parsers; author them in your own corpus under `~/.config/abmx/parsers/`.
Statements are a second parser kind: one PDF, many rows, verified against the
statement's own opening/closing and credit/debit totals.

## Development

```bash
nix develop        # or: direnv allow
just test          # unit, integration, and end-to-end tests (offline)
just check         # fmt + clippy, warnings denied
just ci            # everything CI runs

just dev-up        # disposable Actual Budget server on :5006
just dev-reset     # delete the sandbox budget
```

Statement fixtures and password files are PII: they are never committed. See
`dev/README.md` for the sandbox and the opt-in real-statement tests.

See [CONTRIBUTING.md](CONTRIBUTING.md) for the parser fixture rules, the Node
helper, and releases. User documentation lives on the website.
