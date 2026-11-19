# abmx

Sync bank transaction emails into an Actual Budget ledger.

**[Website and documentation](https://kennethlieyanto.github.io/abmx/)** ·
[Contributing](CONTRIBUTING.md) · [MIT](LICENSE)

```console
$ abmx sync
fetched 15, parsed 15 — import 13, broken 2, unmatched 0, failed 0
2026-11-12  -12,500 IDR  receipt/example.card_debit  EXAMPLE PAYEE
broken (2), not imported: receipt/example.journal_va  parse_failed
```

Parsers are runtime TOML, not compiled code. This repository ships no bank
parsers; author them in your own corpus under `~/.config/abmx/parsers/`.

## Development

```bash
nix develop        # or: direnv allow
just test          # unit, integration, and end-to-end tests
just check         # fmt + clippy, warnings denied
just ci            # everything CI runs
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the parser fixture rules, the Node
helper, and releases. User documentation lives on the website.
