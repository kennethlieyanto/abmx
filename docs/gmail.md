# Gmail

`abmx` uses the Gmail API rather than IMAP because `historyId` is a real
incremental cursor: the server knows what "new" means, so the common run is one
request instead of a mailbox scan.

## One-time setup

1. **Create a Google Cloud project** — <https://console.cloud.google.com/projectcreate>.
2. **Enable the Gmail API** — APIs & Services → Library → Gmail API → Enable.
3. **Configure the OAuth consent screen** (APIs & Services → OAuth consent screen):
   - User type: **External**.
   - Add your own Google account under **Test users**.
   - While the app is in "Testing", refresh tokens expire after 7 days. For a
     personal tool, either re-run `abmx auth gmail` weekly or publish the app
     (Publishing status → Publish app). Publishing an app that only uses
     `gmail.readonly` does not require Google verification for personal use, but
     the consent screen will show an "unverified app" warning.
4. **Create credentials** — APIs & Services → Credentials → Create credentials →
   OAuth client ID → **Desktop app**.
5. Point abmx at the downloaded JSON:

```bash
abmx setup
```

`abmx setup` asks for the JSON path, copies it to
`~/.config/abmx/gmail-client.json` (mode `0600`), and sets
`[fetch.gmail].credentials_file`. By hand, the config looks like this:

```toml
[fetch.gmail]
credentials_file = "gmail-client.json"
```

`credentials_file` is authoritative for `client_id`; an explicit `client_id`
still wins if both are present. The secret follows the usual precedence:
`ABMX_GMAIL_CLIENT_SECRET` → `client_secret` → `client_secret_file` →
the JSON file. Prefer the environment or a `client_secret_file` over pasting a
secret into `abmx.toml`.

## Authorize

```bash
abmx auth gmail
```

This starts a loopback listener on `127.0.0.1:<random port>`, prints an
authorization URL, and tries to open a browser. Approve read-only Gmail access;
the refresh token is written to
`~/.local/state/abmx/gmail-credentials.json` with mode `0600`.

Flags: `--no-browser` prints the URL only, `--port N` pins the loopback port.

### Headless server (or a browser on another machine)

`abmx auth gmail` listens on `127.0.0.1` **on the machine running it**. If you
approve in a browser on a different machine, the redirect goes to *that*
machine's localhost, where nothing is listening — you get "This site can't be
reached" even though authorization succeeded.

Use `--manual`:

```bash
abmx auth gmail --manual
```

1. It prints the authorization URL. Open it in any browser, on any machine.
2. The browser lands on `http://127.0.0.1:<port>/callback?code=…&state=…` and
   fails to load. That is expected.
3. Copy the **whole address** from the address bar and paste it into the
   terminal.
4. `abmx` checks `state` and exchanges the code using the PKCE verifier it
   still holds in memory.

Pasting just the `code=…` value also works; PKCE still binds the exchange to
this request, but the `state` check is skipped.

#### Alternative: forward the port over SSH

If you prefer the automatic flow, forward the loopback port from the browser's
machine:

```bash
# on the server
abmx auth gmail --port 43323 --no-browser

# on the browser's machine
ssh -N -L 43323:127.0.0.1:43323 you@server
```

Then open the printed URL. `--port` matters: the default is a random ephemeral
port, so the tunnel would not match.

#### Alternative: authorize elsewhere, copy the token

Run the flow on a machine that has a browser and `abmx`, then copy the
credential file. The `client_id` must be the same on both sides:

```bash
scp ~/.local/state/abmx/gmail-credentials.json server:~/.local/state/abmx/
ssh server 'chmod 600 ~/.local/state/abmx/gmail-credentials.json'
```

The file contains a client id and a refresh token — no mail, no password.

## What `abmx` asks Gmail for

Scope: `https://www.googleapis.com/auth/gmail.readonly`. Nothing else.

| Call | When |
| --- | --- |
| `users.getProfile` | full sync, to read `historyId` before listing |
| `users.messages.list?q=…` | full sync |
| `users.labels.list` | discover `receipt/*` labels |
| `users.messages.list?labelIds=…` | every message under a configured label |
| `users.messages.get?id=…&format=raw` | each new message |

`format=raw` returns the full RFC 5322 message, which is exactly what the parser
consumes — no re-encoding, no body extraction in `abmx`.

## The query

There is no sender query and no history cursor. `abmx` lists the account's
labels, keeps those starting with `receipt/`, and asks for every message under each
one:

```
labelIds=<label id>  q=-in:spam -in:trash
```

- Only labels with an entry in `[labels.accounts]` are fetched. A label under
  the prefix with no entry is unmanaged and is left alone; `[labels].ignore`
  records an explicit "no" from `abmx setup`.
- A message carrying two managed `receipt/*` labels is an error; the
  routing is ambiguous.
- `[labels].since` adds Gmail's `after:` filter. Without it there is no date
  bound, so every labelled message is fetched and parsed.
- `--limit` applies only to `--from-dir`; for Gmail, narrow the history window
  instead.
- Full pagination is used. If `max_messages` is exceeded, the run fails rather
  than truncating.
- The state database skips messages already delivered, so a re-scan is cheap and
  mail labeled late is picked up on the next run.

## Labels for parser authoring

Labels are the routing identity, not a hint. The user applies exactly one
`receipt/<id>` label to tracked mail (a Gmail filter is the usual way), and
`[labels.accounts]` maps the parser id to a ledger account.

```bash
abmx samples --label receipt/example-qris
```

`abmx samples` downloads up to three messages per managed label into
`$XDG_CONFIG_HOME/abmx/samples/<parser-id>/` with `0600` permissions and writes
`manifest.json`. It never parses, never needs a parser, and never touches the
state database. Fewer than three samples is a warning; if the samples are
different templates the label is reported as `mixed_shapes`; fix the
label/filter instead of splitting the parser.

See [authoring.md](authoring.md) for the full loop.

## Revoking access

```bash
abmx auth logout              # revoke at Google, then delete the local file
abmx auth logout --no-revoke  # delete the local file only
```

The default revokes the refresh token at Google first, so a copy of the
credential file — a backup, an old machine, an image layer — stops working.
Revocation is per-token: other machines that authorized separately keep working.

If revocation fails (offline, DNS), the local file is still removed, the command
exits `0`, and it prints a warning with the manual URL:

<https://myaccount.google.com/permissions>

`auth logout` does **not** touch sync state. Delivered messages stay recorded,
so the next sync continues where it left off. Use
`abmx reset` if you also want it to re-fetch everything.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `gmail: not authorized` | no credentials file | `abmx auth gmail` |
| `invalid_grant` on refresh | token expired (testing app) or revoked | re-run `abmx auth gmail` |
| `403 … insufficient authentication scopes` | wrong scope on the token | revoke at <https://myaccount.google.com/permissions>, re-authorize |
| full sync every run | `historyId` rejected as expired | normal after long downtime; the state database still prevents duplicates |
| `no refresh token returned` | Google only returns one on the first consent | remove app access, re-authorize (the flow already sends `prompt=consent`) |
| browser shows "This site can't be reached" after approving | the redirect's `127.0.0.1` is the browser's machine, not the CLI's | `abmx auth gmail --manual`, then paste the URL from the address bar |
| `state mismatch` | the pasted URL came from a different run | re-run `--manual` and paste the URL from that same run |
| nothing found | sender list does not match your bank's actual address | `abmx status`, then compare with a real email |

## Verifying without Gmail

You do not need OAuth to test the pipeline:

```bash
abmx sync --dry-run --from-dir ./saved-emails --label "receipt/<id>"
```

Save a few real `.eml` files (or export them) and run against those. Same parser,
same policy, same plan.
