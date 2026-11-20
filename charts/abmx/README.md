# abmx Helm chart

Runs [`abmx`](https://github.com/kennethlieyanto/abmx) as a CronJob: fetch Gmail,
parse bank receipts with the parser corpus, and import them into Actual Budget.

The chart intentionally has **no application-config defaults**. You supply your
own `config.toml` (labels, history window, account UUIDs, Actual server URL and
sync id). Secrets come from Kubernetes Secrets, normally managed by External
Secrets.

## Required configuration

`config.existingConfigMap` (recommended) or `config.data` must be set. The
existing ConfigMap must contain a `config.toml` key:

```toml
[fetch.gmail]
token = "/run/secrets/abmx/gmail-credentials.json"

[sink.actual]
server_url = "http://actual-budget-actualbudget.actual-budget.svc.cluster.local:5006"
budget_sync_id = "…"
data_dir = "/data/cache/actual"
timeout_secs = 300

[receipts]
prefix = "receipt/"
since = 2026-11-01

[receipts.accounts]
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

See `examples/abmx-config/` in the abmx repository for a tiny Helm chart that
generates both ConfigMaps from plain files in Git.

## Values

| Key | Default | Description |
| --- | --- | --- |
| `image.repository` | `ghcr.io/kennethlieyanto/abmx` | Image repository |
| `image.tag` | chart `appVersion` | Image tag; pin a release in production |
| `image.digest` | `""` | Pin an immutable image digest |
| `image.pullPolicy` | `IfNotPresent` | Image pull policy |
| `imagePullSecrets` | `[]` | Pull secrets |
| `schedule` | `*/15 * * * *` | CronJob schedule |
| `timeZone` | `""` | CronJob time zone |
| `concurrencyPolicy` | `Forbid` | CronJob concurrency policy |
| `activeDeadlineSeconds` | `1800` | Job deadline |
| `backoffLimit` | `0` | Job retries |
| `args` | `--config /etc/abmx/config.toml sync` | Container args |
| `extraArgs` | `[]` | Appended to `args` |
| `config.existingConfigMap` | `""` | ConfigMap with a `config.toml` key |
| `config.data` | `""` | Inline TOML (rendered through `tpl`) |
| `parsers.existingConfigMap` | `""` | ConfigMap of parser id → `config.toml` |
| `parsers.mountPath` | `/etc/abmx/parsers` | Mounted corpus path |
| `gmailAuth.existingSecret` | `""` | Secret with the exported bundle |
| `gmailAuth.key` | `gmail-credentials.json` | Secret key |
| `gmailAuth.mountPath` | `/run/secrets/abmx` | Mount directory |
| `secrets.existingSecret` | `""` | Env Secret for Actual credentials |
| `persistence.enabled` | `true` | Create a PVC for `/data` |
| `persistence.existingClaim` | `""` | Use an existing PVC |
| `persistence.size` | `1Gi` | PVC size |
| `resources` | 100m/256Mi → 1Gi | Container resources |
| `podSecurityContext` | non-root 1000 | Pod security context |
| `securityContext` | read-only root | Container security context |
| `extraEnv`, `extraVolumes`, `extraVolumeMounts` | `[]` | Additional pod configuration |
| `secrets.extraEnvFrom` | `[]` | Additional `envFrom` sources |
| `initContainers` | `[]` | Additional init containers |
| `nodeSelector`, `tolerations`, `affinity` | empty | Scheduling |

## Manual run

```bash
kubectl create job --from=cronjob/abmx abmx-manual
kubectl logs job/abmx-manual
```
