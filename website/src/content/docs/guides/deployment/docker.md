---
title: Docker
description: Run abmx from the container image with config, state, and secrets mounted.
---

The repository's `Dockerfile` contains the Rust binary and the Node helper, so
the image needs nothing else at runtime. Build it locally or use the published
image:

```bash
docker build -t abmx .
# or
docker pull ghcr.io/kennethlieyanto/abmx
```

## Run a sync

```bash
mkdir -p var/abmx
docker run --rm --user "$(id -u):$(id -g)" \
  -v "$PWD/var/abmx:/data" \
  -v "$PWD/abmx.toml:/config/abmx.toml:ro" \
  -e ABMX_ACTUAL_SERVER_PASSWORD \
  abmx --config /config/abmx.toml sync
```

`/data` holds the config-adjacent state, the Gmail token, and the budget cache.
Pass secrets as environment variables or mount a credentials file; the helper
and Node come from the image.

For an Actual server that offers API keys, prefer the key over the server
password:

```bash
docker run --rm --user "$(id -u):$(id -g)" \
  -v "$PWD/var/abmx:/data" \
  -v "$PWD/abmx.toml:/config/abmx.toml:ro" \
  -e ABMX_ACTUAL_API_KEY \
  abmx --config /config/abmx.toml sync
```

## Authentication without a browser

The container has no browser. Authorize on a machine that does, export the
bundle, and mount it:

```bash
abmx auth export --out gmail-credentials.json
```

```toml
[fetch.gmail]
token = "/run/secrets/abmx/gmail-credentials.json"
```

```bash
docker run --rm --user "$(id -u):$(id -g)" \
  -v "$PWD/var/abmx:/data" \
  -v "$PWD/abmx.toml:/config/abmx.toml:ro" \
  -v "$PWD/gmail-credentials.json:/run/secrets/abmx/gmail-credentials.json:ro" \
  -e ABMX_ACTUAL_API_KEY \
  abmx --config /config/abmx.toml sync
```

The bundle is a password-equivalent secret: anyone holding it can read the
mailbox until `abmx auth logout` revokes the refresh token. Mount it read-only
and keep it out of image layers.

## State and cache

- `/data` is the writable directory. Keep it on a volume; without it, every run
  re-fetches messages and re-downloads the Actual budget.
- If your config uses `data_dir`, point it inside `/data`.
- Run as a fixed non-root UID with `--user`, and make sure the volume is owned
  by that UID. The chart and the NixOS module do the equivalent.

## Scheduling

Run one sync at a time. The Actual helper holds a local copy of the budget and
syncs it back, so concurrent writers can stomp each other. Use cron, a systemd
timer, or a Kubernetes CronJob — not two of them for the same budget. See
[Concurrency](/guides/actual/#concurrency).
