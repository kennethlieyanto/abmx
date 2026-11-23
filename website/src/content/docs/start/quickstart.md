---
title: Quickstart
description: From a fresh install to imported transactions in four commands.
---

```bash
abmx setup                      # config + Gmail + labels + accounts, interactively
abmx status                     # config, corpus, labels, accounts, auth
abmx receipt sync --dry-run             # see the plan, write nothing
abmx receipt sync               # import everything mapped
```

`abmx setup` is idempotent: rerun it whenever you add a label, change accounts,
or move to a new machine. Every step it has already completed is skipped. See
[Setup](/start/setup/) for exactly what it asks.

## Before you start

You need a Gmail OAuth client and an Actual server. Both have one-time guides:

- [Gmail](/guides/gmail/) — Google Cloud project, `gmail.readonly`, desktop
  client, then `abmx auth login`.
- [Actual Budget](/guides/actual/) — server URL, budget Sync ID, and an API key
  or server password.

## Prefer doing it by hand?

Create the file yourself, using [Configuration](/start/configuration/) as the
reference:

```bash
mkdir -p ~/.config/abmx
$EDITOR ~/.config/abmx/config.toml   # Gmail client file + Actual endpoint
abmx auth login                      # OAuth2 + PKCE, stores a refresh token (0600)
abmx status
```

Every value `abmx setup` writes is also editable by hand; it uses a
format-preserving TOML editor, so comments and unknown keys survive. The full
file is documented in [Configuration](/start/configuration/).

## Routing is two decisions

1. **gmailctl labels the mail.** A tracked message carries exactly one
   `receipt/<parser-id>` label. Merchant receipts keep the plain `receipt` label
   and are never imported. OTP, security, promos, and statements go elsewhere.
2. **`[receipts.accounts]` maps the parser id to an Actual account.** Only mapped
   ids are fetched, so `sync` leaves unmanaged labels alone; `abmx status`
   reports them as failures until they are mapped or ignored.

```toml
[receipts]
prefix = "receipt/"
since  = 2026-11-01          # history window; omitting it scans all labelled mail

[receipts.accounts]
"example.card_debit" = "<actual account uuid>"
```

`since` is strongly recommended. Without it, every run scans every message under
the mapped labels; a recent window also keeps the parser focused on templates it
was authored against.

## Author the first parser

`abmx` ships no bank parsers. Download real evidence, then let an agent write
the parser:

```bash
abmx samples                # evidence for labeled receipts and statements
abmx skill install          # authoring skill under ~/.agents/skills/abmx
```

Then prompt your agent:

```
help me create receipt and statement parsers for all my emails, use the abmx parser authoring skill
```

The agent writes the parser into `~/.config/abmx/parsers/<id>/` with its fixtures
and verifies it with `abmx parser check --parser <id>`. See
[Authoring parsers](/guides/authoring/) for the full loop and the status
meanings.

Already have `.eml` files? Put them under
`~/.config/abmx/samples/receipts/<parser-id>/` and ask the agent to author from
those.

## What a broken label looks like

`sync` writes no rows at all when any label is broken. The summary names the
label, the reason, and the next command:

```text
broken (2), not imported:
  receipt/example.journal_va   parse_failed   amount: no value matched
  next: abmx receipt samples --label receipt/example.journal_va
  nothing was written to the ledger
```

Fix the parser or the Gmail rule, then run `abmx receipt sync` again. A broken row is
never marked delivered, so it is retried.

## Put it on a timer

Once `abmx receipt sync` is clean, run it periodically. `receipt sync` only fetches mapped
labels, so unmanaged ones are left alone rather than aborting the run. See
[NixOS](/guides/deployment/nixos/),
[Kubernetes](/guides/deployment/kubernetes/), or
[Docker](/guides/deployment/docker/).
