# Gmail

`txsync` uses the Gmail API rather than IMAP because `historyId` is a real
incremental cursor: the server knows what "new" means, so the common run is one
request instead of a mailbox scan.

## One-time setup

1. **Create a Google Cloud project** — <https://console.cloud.google.com/projectcreate>.
2. **Enable the Gmail API** — APIs & Services → Library → Gmail API → Enable.
3. **Configure the OAuth consent screen** (APIs & Services → OAuth consent screen):
   - User type: **External**.
   - Add your own Google account under **Test users**.
   - While the app is in "Testing", refresh tokens expire after 7 days. For a
     personal tool, either re-run `txsync auth gmail` weekly or publish the app
     (Publishing status → Publish app). Publishing an app that only uses
     `gmail.readonly` does not require Google verification for personal use, but
     the consent screen will show an "unverified app" warning.
4. **Create credentials** — APIs & Services → Credentials → Create credentials →
   OAuth client ID → **Desktop app**.
5. Copy the **client ID** and **client secret** into `txsync.toml`:

```toml
[fetch.gmail]
client_id = "1234567890-abcdef.apps.googleusercontent.com"
client_secret_file = "/run/credentials/txsync.service/gmail-client-secret"
```

Prefer `TXSYNC_GMAIL_CLIENT_SECRET` or `client_secret_file` over pasting the
secret into the file.

## Authorize

```bash
txsync auth gmail
```

This starts a loopback listener on `127.0.0.1:<random port>`, prints an
authorization URL, and tries to open a browser. Approve read-only Gmail access;
the refresh token is written to
`~/.local/state/txsync/gmail-credentials.json` with mode `0600`.

Flags: `--no-browser` prints the URL only, `--port N` pins the loopback port.

### Headless server

The loopback flow needs a browser somewhere. Run `txsync auth gmail` on your
laptop and copy the credentials file to the server:

```bash
scp ~/.local/state/txsync/gmail-credentials.json server:~/.local/state/txsync/
```

The file only contains a client id and a refresh token — no mail, no password.

## What `txsync` asks Gmail for

Scope: `https://www.googleapis.com/auth/gmail.readonly`. Nothing else.

| Call | When |
| --- | --- |
| `users.getProfile` | full sync, to read `historyId` before listing |
| `users.messages.list?q=…` | full sync |
| `users.history.list?startHistoryId=…&historyTypes=messageAdded` | incremental |
| `users.messages.get?id=…&format=raw` | every new message |

`format=raw` returns the full RFC 5322 message, which is exactly what `txp`
consumes — no re-encoding, no body extraction in `txsync`.

## The query

The sender list is not maintained by hand: `txsync` reads every `senders` entry
from the loaded `txp` corpus.

```
from:(alerts@example.invalid OR bank.example) -in:spam -in:trash newer_than:7d
```

- `*@bank.example` in a parser spec becomes `bank.example`.
- `lookback_days` is a floor for full syncs, not a filter on incremental runs.
- `query_extra` is appended verbatim, e.g. `"-label:txsync-ignored"`.
- `labels = ["INBOX"]` restricts the search further if you have filters.

The query only applies to full syncs. Incremental runs ask for everything added
since the cursor and let `txp` route it.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `gmail: not authorized` | no credentials file | `txsync auth gmail` |
| `invalid_grant` on refresh | token expired (testing app) or revoked | re-run `txsync auth gmail` |
| `403 … insufficient authentication scopes` | wrong scope on the token | revoke at <https://myaccount.google.com/permissions>, re-authorize |
| full sync every run | `historyId` rejected as expired | normal after long downtime; the state database still prevents duplicates |
| `no refresh token returned` | Google only returns one on the first consent | remove app access, re-authorize (the flow already sends `prompt=consent`) |
| nothing found | sender list does not match your bank's actual address | `txsync parsers`, then compare with a real email |

## Verifying without Gmail

You do not need OAuth to test the pipeline:

```bash
txsync plan --from-dir ./saved-emails/
```

Save a few real `.eml` files (or export them) and run against those. Same parser,
same policy, same plan.
