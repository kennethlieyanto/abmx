---
title: NixOS
description: Run abmx as a hardened systemd oneshot service on a timer.
---

The flake exports a NixOS module. It runs `abmx sync` as a oneshot service with
a timer, and the unit is hardened: read-only system, no home access, and only
the state/cache directories writable.

```nix
{
  inputs.abmx.url = "github:kennethlieyanto/abmx";

  services.abmx = {
    enable = true;
    configFile = /etc/abmx/config.toml;
    interval = "15min";
    secrets = {
      "actual-api-key" = config.age.secrets.actual-api-key.path;
      "gmail-credentials" = config.age.secrets.gmail-credentials.path;
    };
  };
}
```

Credentials land in `/run/credentials/abmx.service/<name>`, so the config refers
to them as regular files:

```toml
[fetch.gmail]
token = "/run/credentials/abmx.service/gmail-credentials"

[sink.actual]
server_url = "http://localhost:5006"
budget_sync_id = "…"
api_key_file = "/run/credentials/abmx.service/actual-api-key"
```

The Gmail credential is the portable bundle from
`abmx auth export --out gmail-credentials.json`, not the token file alone: the
service user never runs the browser flow.

## Options

| Option | Default | Meaning |
| --- | --- | --- |
| `enable` | `false` | Enable the service and timer |
| `package` | the flake's package | Package to run |
| `configFile` | required | Path to `abmx.toml` |
| `interval` | `"15min"` | Timer interval between syncs |
| `user` | `"abmx"` | System user the service runs as |
| `secrets` | `{}` | Files exposed as systemd credentials |
| `extraArgs` | `[]` | Extra arguments appended to `abmx sync` |

## What the module sets up

- A system user and group (`abmx`) with home `/var/lib/abmx`.
- `StateDirectory=abmx` and `CacheDirectory=abmx`; the Actual budget cache is
  pointed at `/var/cache/abmx/actual`.
- `ProtectSystem=strict`, `ProtectHome=true`, `PrivateTmp=true`,
  `NoNewPrivileges=true`, and a restricted address-family list.
- A persistent timer with a 30-second randomized delay and a 2-minute start
  delay after boot.

The service runs one sync at a time, which matters for Actual: the helper holds
a local copy of the budget and syncs it back, so concurrent writers can stomp
each other. See [Concurrency](/guides/actual/#concurrency).

## Operate it

```bash
systemctl start abmx.service       # run once
systemctl status abmx.service
journalctl -u abmx.service -f
systemctl list-timers abmx.timer
```

The state database lives in `/var/lib/abmx/`, and the Actual cache in
`/var/cache/abmx/`. Both are disposable; deleting the state database makes the
next sync reconsider old mail, but `imported_id` keeps the ledger idempotent.
