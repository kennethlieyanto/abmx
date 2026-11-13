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

### Headless server (or a browser on another machine)

`txsync auth gmail` listens on `127.0.0.1` **on the machine running it**. If you
approve in a browser on a different machine, the redirect goes to *that*
machine's localhost, where nothing is listening — you get "This site can't be
reached" even though authorization succeeded.

Use `--manual`:

```bash
txsync auth gmail --manual
```

1. It prints the authorization URL. Open it in any browser, on any machine.
2. The browser lands on `http://127.0.0.1:<port>/callback?code=…&state=…` and
   fails to load. That is expected.
3. Copy the **whole address** from the address bar and paste it into the
   terminal.
4. `txsync` checks `state` and exchanges the code using the PKCE verifier it
   still holds in memory.

Pasting just the `code=…` value also works; PKCE still binds the exchange to
this request, but the `state` check is skipped.

#### Alternative: forward the port over SSH

If you prefer the automatic flow, forward the loopback port from the browser's
machine:

```bash
# on the server
txsync auth gmail --port 43323 --no-browser

# on the browser's machine
ssh -N -L 43323:127.0.0.1:43323 you@server
```

Then open the printed URL. `--port` matters: the default is a random ephemeral
port, so the tunnel would not match.

#### Alternative: authorize elsewhere, copy the token

Run the flow on a machine that has a browser and `txsync`, then copy the
credential file. The `client_id` must be the same on both sides:

```bash
scp ~/.local/state/txsync/gmail-credentials.json server:~/.local/state/txsync/
ssh server 'chmod 600 ~/.local/state/txsync/gmail-credentials.json'
```

The file contains a client id and a refresh token — no mail, no password.

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

## Revoking access

```bash
txsync auth logout              # revoke at Google, then delete the local file
txsync auth logout --no-revoke  # delete the local file only
```

The default revokes the refresh token at Google first, so a copy of the
credential file — a backup, an old machine, an image layer — stops working.
Revocation is per-token: other machines that authorized separately keep working.

If revocation fails (offline, DNS), the local file is still removed, the command
exits `0`, and it prints a warning with the manual URL:

<https://myaccount.google.com/permissions>

`auth logout` does **not** touch sync state. The cursor, delivered messages and
review queue stay, so the next sync continues where it left off. Use
`txsync reset` if you also want it to re-fetch everything.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `gmail: not authorized` | no credentials file | `txsync auth gmail` |
| `invalid_grant` on refresh | token expired (testing app) or revoked | re-run `txsync auth gmail` |
| `403 … insufficient authentication scopes` | wrong scope on the token | revoke at <https://myaccount.google.com/permissions>, re-authorize |
| full sync every run | `historyId` rejected as expired | normal after long downtime; the state database still prevents duplicates |
| `no refresh token returned` | Google only returns one on the first consent | remove app access, re-authorize (the flow already sends `prompt=consent`) |
| browser shows "This site can't be reached" after approving | the redirect's `127.0.0.1` is the browser's machine, not the CLI's | `txsync auth gmail --manual`, then paste the URL from the address bar |
| `state mismatch` | the pasted URL came from a different run | re-run `--manual` and paste the URL from that same run |
| nothing found | sender list does not match your bank's actual address | `txsync parsers`, then compare with a real email |

## Verifying without Gmail

You do not need OAuth to test the pipeline:

```bash
txsync plan --from-dir ./saved-emails/
```

Save a few real `.eml` files (or export them) and run against those. Same parser,
same policy, same plan.
