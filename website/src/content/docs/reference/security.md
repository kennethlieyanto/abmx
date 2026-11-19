---
title: Security and privacy
description: What abmx can access, where credentials live, and what it never does.
---

## Scope

- Gmail scope is `gmail.readonly`. Nothing else is requested.
- The refresh token is written with `0600` and never logged.
- The OAuth flow uses PKCE and a `127.0.0.1` redirect; there is no out-of-band
  flow (Google retired it).
- Ledger credentials are separate from mailbox credentials and live in different
  files.
- `abmx` reads mail and writes to a ledger. It never deletes mail and never
  deletes ledger rows.
- There is no telemetry and no analytics call.

## Credentials on disk

| File | Mode | Contents |
| --- | --- | --- |
| `~/.local/state/abmx/gmail-credentials.json` | `0600` | client id + refresh token |
| `<config dir>/gmail-client.json` | `0600` | OAuth client JSON |
| `<config dir>/actual-api-key` | `0600` | Actual budget-scoped key |
| `<config dir>/actual-server-password` | `0600` | Actual server password |
| `<config dir>/actual-budget-password` | `0600` | budget encryption password |

`abmx auth export` writes one self-contained bundle (client id, secret, refresh
token, scopes). Treat it as password-equivalent: anyone holding it can read the
mailbox until `abmx auth logout` revokes the token.

Secret precedence is **environment → literal → file**, so systemd
`LoadCredential` and Kubernetes Secrets can override whatever is on disk.
See [Configuration](/start/configuration/#secrets) for the full table.

## Parser privacy

`abmx samples` downloads real mail to
`$XDG_CONFIG_HOME/abmx/samples/<parser-id>/` with `0600` permissions. Those files
contain real amounts, counterparties, and account numbers. Delete the directory
when you are done authoring.

A personal parser's fixtures may keep the real sample: they live under your
config directory, not in this repository. Committed fixtures are synthetic
(`@example.invalid`, example merchants, arbitrary amounts). The repository's
`CONTRIBUTING.md` documents the redaction rules for contributions.

## The Actual helper

`abmx` spawns the bundled Node helper with the budget credentials in its
environment. The helper talks to your Actual server, holds a local copy of the
budget under `data_dir`, and exits after one operation. There is no listening
port, no shared secret on the wire, and no long-running service.

The helper's stdout is protocol; its logging goes to stderr and is quiet unless
`ABMX_ACTUAL_DEBUG=1` is set. See [Actual Budget](/guides/actual/).

## Network calls

At runtime, `abmx` talks to:

- `oauth2.googleapis.com` and `gmail.googleapis.com` for authorization and mail;
- your Actual server;
- `127.0.0.1` for the OAuth loopback redirect.

It makes no other outbound requests. Version checks are local: the bundled
`@actual-app/api` version is compared with the server's version during
`abmx status`.

## Threat model notes

- A parser is data, not code, but its regexes run against mail you fetched. A
  pathological regex can be slow; prefer anchored, shape-based patterns over
  backtracking-heavy ones.
- The ledger is a projection of the mailbox. Anyone with mailbox write access
  can influence future imports; that is inherent to the design, not a bug.
- The state database is a cache. Losing it never causes duplicate imports
  because the Actual sink deduplicates on `imported_id`.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting:
<https://github.com/kennethlieyanto/abmx/security/advisories/new>.

Do not open a public issue for a credential leak or an authentication bypass.
