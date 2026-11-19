---
title: Kubernetes
description: Run abmx as a CronJob with the Helm chart, ConfigMaps, and secrets.
---

`charts/abmx` runs `abmx sync` as a CronJob. It consumes two ConfigMaps — your
application `config.toml` and (optionally) a parser corpus — plus Secrets for
credentials, so GitOps changes do not require rebuilding the image.

From a checkout of the repository:

```bash
# once, on a machine with a browser
abmx auth gmail
abmx auth export --out abmx-gmail-auth.json

# then point the chart at your config and parser ConfigMaps
helm install abmx charts/abmx -f my-values.yaml
```

## Required configuration

Set `config.existingConfigMap` (recommended) or `config.data`. The existing
ConfigMap must contain a `config.toml` key:

```toml
[fetch.gmail]
token = "/run/secrets/abmx/gmail-credentials.json"

[sink.actual]
server_url = "http://actual-budget-actualbudget.actual-budget.svc.cluster.local:5006"
budget_sync_id = "…"
data_dir = "/data/cache/actual"
timeout_secs = 300

[labels]
prefix = "receipt/"
since = 2026-11-01

[labels.accounts]
"example.card_debit" = "<actual account uuid>"
```

`/etc/abmx/config.toml` is mounted from the ConfigMap. The state database and
the Actual budget cache live on the `/data` PVC.

## Secrets

```bash
# Gmail bundle produced by `abmx auth export`
kubectl create secret generic abmx-gmail-auth \
  --from-file=gmail-credentials.json=abmx-gmail-auth.json

# Actual credentials (API key preferred)
kubectl create secret generic abmx-secrets \
  --from-literal=ABMX_ACTUAL_API_KEY=…
# fallback for Actual versions without API keys:
#   --from-literal=ABMX_ACTUAL_SERVER_PASSWORD=…
# end-to-end encrypted budgets:
#   --from-literal=ABMX_ACTUAL_BUDGET_PASSWORD=…
```

Then set `gmailAuth.existingSecret` and `secrets.existingSecret`.

## Parser corpus

`abmx` ships **no built-in parser corpus**. Point `parsers.existingConfigMap` at
a ConfigMap whose **keys are parser ids** and whose **values are that parser's
`config.toml`**:

```yaml
data:
  example.card_debit: |
    spec_version = 2
    …
  example.qris_payment: |
    spec_version = 2
    …
```

An init container expands those flat keys into `<id>/config.toml` directories,
and `ABMX_PARSERS` points at that directory. The image has no fallback corpus,
so this ConfigMap is the source of truth.

`examples/abmx-config/` is a tiny chart that turns plain `config.toml` and
`files/parsers/<id>/config.toml` files in Git into those ConfigMaps.

## Values

The chart README documents every value, including image pinning, `schedule`,
`timeZone`, `concurrencyPolicy`, resources, and persistence. The important ones:

| Key | Default | Description |
| --- | --- | --- |
| `schedule` | `*/15 * * * *` | CronJob schedule |
| `concurrencyPolicy` | `Forbid` | Never let two syncs overlap |
| `config.existingConfigMap` | `""` | ConfigMap with a `config.toml` key |
| `parsers.existingConfigMap` | `""` | ConfigMap of parser id → `config.toml` |
| `gmailAuth.existingSecret` | `""` | Secret with the exported bundle |
| `secrets.existingSecret` | `""` | Env Secret for Actual credentials |
| `persistence.size` | `1Gi` | PVC for `/data` |

See [`charts/abmx/README.md`](https://github.com/kennethlieyanto/abmx/blob/main/charts/abmx/README.md)
for the complete table.

## Manual run

```bash
kubectl create job --from=cronjob/abmx abmx-manual
kubectl logs job/abmx-manual
```

`concurrencyPolicy: Forbid` is the safe default. The Actual helper is not safe
for concurrent writers against the same budget; see
[Concurrency](/guides/actual/#concurrency).
